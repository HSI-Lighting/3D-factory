use super::*;


// ---- snap markers & dashed extension line ---------------------------------

/// Per-snap-kind glyph at the snap point. Each kind gets a distinct shape so
/// the user knows what was matched without reading the label:
///   END  square outline
///   MID  triangle (point up)
///   CEN  small circle + centre dot
///   INT  X
///   PER  ⊥ symbol
///   TAN  circle with a tangent stub
///   NEA  hourglass (two opposing triangles)
pub(super) fn draw_snap_glyph(p: &egui::Painter, c: egui::Pos2, k: SnapKind, col: egui::Color32) {
    let s = 6.0; // half-extent
    let stroke = egui::Stroke::new(1.6, col);
    match k {
        SnapKind::End => {
            let r = egui::Rect::from_min_max(
                egui::pos2(c.x - s, c.y - s),
                egui::pos2(c.x + s, c.y + s),
            );
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
            // Diamond — AutoCAD's quadrant marker
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
            p.line_segment(
                [egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)],
                stroke,
            );
            p.line_segment(
                [egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)],
                stroke,
            );
        }
        SnapKind::Per => {
            // upright ⊥: vertical stroke + horizontal baseline
            p.line_segment([egui::pos2(c.x, c.y - s), egui::pos2(c.x, c.y + s)], stroke);
            p.line_segment(
                [egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y + s)],
                stroke,
            );
        }
        SnapKind::Tan => {
            p.circle_stroke(c, s * 0.75, stroke);
            // horizontal tangent stub through the top of the small circle
            let y = c.y - s * 0.75;
            p.line_segment([egui::pos2(c.x - s, y), egui::pos2(c.x + s, y)], stroke);
        }
        SnapKind::Nea => {
            // hourglass / bowtie
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

/// Draw a dashed line a → b with the dash phase shifted by `phase` pixels so
/// the dashes appear to drift along the line. Used for the "imaginary
/// extension" trail of PER/TAN snaps on line dobjects.
/// Visible "blip" marker drawn at a captured base point (move base,
/// copy base, rotate pivot, scale pivot, …). Small filled square with
/// a + cross through it — same vocabulary as the drafting cursor so
/// the user reads it as "this is the locked-in point".
///
/// `color` is used for both fill and stroke so each command can have
/// its own accent (move = orange, copy = green, etc.). The marker is
/// drawn in screen-space px and doesn't scale with zoom.
pub(super) fn draw_base_blip(p: &egui::Painter, pos: egui::Pos2, color: egui::Color32) {
    let half = 4.5_f32; // filled square half-edge
    let arm = 10.0_f32; // cross arm half-length
    let sq = egui::Rect::from_center_size(pos, egui::vec2(half * 2.0, half * 2.0));
    p.rect_filled(sq, 1.0, color);
    p.rect_stroke(sq, 1.0, egui::Stroke::new(1.4, color));
    let stroke = egui::Stroke::new(1.4, color);
    p.line_segment(
        [
            egui::pos2(pos.x - arm, pos.y),
            egui::pos2(pos.x + arm, pos.y),
        ],
        stroke,
    );
    p.line_segment(
        [
            egui::pos2(pos.x, pos.y - arm),
            egui::pos2(pos.x, pos.y + arm),
        ],
        stroke,
    );
}

pub(super) fn draw_dashed_line(
    p: &egui::Painter,
    a: egui::Pos2,
    b: egui::Pos2,
    dash_len: f32,
    gap_len: f32,
    phase: f32,
    stroke: egui::Stroke,
) {
    let d = b - a;
    let total = d.length();
    if total < 1e-3 {
        return;
    }
    let dir = d / total;
    let period = dash_len + gap_len;
    let mut t = -(phase.rem_euclid(period));
    while t < total {
        let s = t.max(0.0);
        let e = (t + dash_len).min(total);
        if e > s + 0.1 {
            p.line_segment([a + dir * s, a + dir * e], stroke);
        }
        t += period;
    }
}

/// Dashed arc indicator along a circle of (center_w, radius_w) starting at
/// `start_ang` (world radians) and sweeping `sweep` (signed, world radians).
/// Used for the PER/TAN "imaginary extension" on arc dobjects — the
/// extension follows the underlying circle's curvature, not a chord.
pub(super) fn draw_dashed_arc(
    p: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    center_w: Vec2,
    radius_w: f64,
    start_ang: f64,
    sweep: f64,
    dash_len_px: f32,
    gap_len_px: f32,
    phase_px: f32,
    stroke: egui::Stroke,
) {
    let arc_len_px = (radius_w as f32 * app.scale) * sweep.abs() as f32;
    if arc_len_px < 1.0 {
        return;
    }
    let period = dash_len_px + gap_len_px;
    let mut t = -(phase_px.rem_euclid(period));
    while t < arc_len_px {
        let s = t.max(0.0);
        let e = (t + dash_len_px).min(arc_len_px);
        if e > s + 0.1 {
            let s_frac = (s / arc_len_px) as f64;
            let e_frac = (e / arc_len_px) as f64;
            let s_ang = start_ang + sweep * s_frac;
            let e_ang = start_ang + sweep * e_frac;
            // Subdivide each dash enough that the curvature reads as smooth.
            let subdiv = (((e - s) / 2.0).ceil() as usize).max(1);
            let mut pts = Vec::with_capacity(subdiv + 1);
            for i in 0..=subdiv {
                let f = i as f64 / subdiv as f64;
                let a = s_ang + (e_ang - s_ang) * f;
                let pw = Vec2::new(
                    center_w.x + radius_w * a.cos(),
                    center_w.y + radius_w * a.sin(),
                );
                pts.push(app.w2s(pw, rect));
            }
            p.add(egui::Shape::line(pts, stroke));
        }
        t += period;
    }
}

/// Filled grip handles on the selected dobject. The set of grip locations
/// follows the AutoCAD convention:
///   Line   — both endpoints + midpoint
///   Circle — centre + N/S/E/W quadrant points
///   Arc    — centre + both endpoints + midpoint
pub(super) fn draw_grips(painter: &egui::Painter, rect: egui::Rect, app: &CadApp, g: &Geom) {
    let col = egui::Color32::from_rgb(80, 170, 255);
    let outline = egui::Stroke::new(1.0, egui::Color32::WHITE);
    let s = 4.0; // half-extent of grip square (screen px)
    let draw = |w: Vec2| {
        let sp = app.w2s(w, rect);
        let r = egui::Rect::from_min_max(
            egui::pos2(sp.x - s, sp.y - s),
            egui::pos2(sp.x + s, sp.y + s),
        );
        painter.rect(r, 1.0, col, outline);
    };
    match g {
        Geom::Line(l) => {
            draw(l.a);
            draw(l.b);
            draw((l.a + l.b) * 0.5);
        }
        Geom::Circle(c) => {
            draw(c.center);
            draw(c.center + Vec2::new(c.radius, 0.0));
            draw(c.center + Vec2::new(-c.radius, 0.0));
            draw(c.center + Vec2::new(0.0, c.radius));
            draw(c.center + Vec2::new(0.0, -c.radius));
        }
        Geom::Arc(a) => {
            draw(a.center);
            let (p1, p2) = a.endpoints();
            draw(p1);
            draw(p2);
            let m = a.start_angle + a.sweep_angle * 0.5;
            draw(a.center + Vec2::new(a.radius * m.cos(), a.radius * m.sin()));
        }
        Geom::Ellipse(el) => {
            draw(el.center);
            // axis-end grips (the QUA points)
            for t in [
                0.0,
                std::f64::consts::FRAC_PI_2,
                std::f64::consts::PI,
                3.0 * std::f64::consts::FRAC_PI_2,
            ] {
                draw(el.point_at(t));
            }
        }
        Geom::EllipseArc(ea) => {
            draw(ea.ellipse.center);
            let (p1, p2) = ea.endpoints();
            draw(p1);
            draw(p2);
            let m = ea.start_param + ea.sweep_param * 0.5;
            draw(ea.ellipse.point_at(m));
        }
        Geom::Point(pt) => {
            draw(pt.location);
        }
        Geom::Polyline(p) => {
            for v in &p.vertices {
                draw(v.pos);
            }
        }
        Geom::Hatch(_) => {
            // Hatch MVP exposes no grips — boundary vertices may become
            // PolyVertex grips later. Nothing to draw.
        }
        Geom::Spline(s) => {
            // Spline grips = every control point. Dragging one
            // reshapes the curve locally (see GripRole::SplineCtrlPt
            // in the kernel).
            for p in &s.control_points {
                draw(*p);
            }
        }
        Geom::Wall(w) => {
            // Wall grips = centerline endpoints + centerline midpoint.
            draw(w.start);
            draw(w.end);
            draw((w.start + w.end) * 0.5);
        }
        Geom::Text(t) => {
            // Text — single anchor grip.
            draw(t.position);
        }
        Geom::Dimension(d) => {
            // Dimension — three grips: the two def points + the
            // dim-line / leader anchor.
            for gp in d.grip_points() {
                draw(gp);
            }
        }
        Geom::BlockRef(br) => {
            // BlockRef — one grip at the insertion point (drag = move).
            draw(br.insert);
        }
        _ => {}
    }
}

/// Map a TextStyle font_name to the egui FontId the renderer uses.
/// V1 supports two built-in fonts (`standard` → proportional sans-serif,
/// `monospace` → monospace); anything else falls back to proportional so
/// styles loaded from DXF with custom font names keep rendering. When
/// LFF / SHX font loading lands the swap-in happens here only.
pub(super) fn font_id_for_font_name(name: &str, size_px: f32) -> egui::FontId {
    match name.to_ascii_lowercase().as_str() {
        "monospace" | "hack" | "mono" => egui::FontId::monospace(size_px),
        _ => egui::FontId::proportional(size_px),
    }
}

/// Ellipse rubber-band: major guide line + perpendicular guide from major to
/// cursor + a live full-ellipse polyline using the cursor's distance from the
/// major-axis as the semi-minor.
pub(super) fn draw_ellipse_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    centre: Vec2,
    major_end: Vec2,
    cursor_world: Vec2,
    dash: egui::Stroke,
    hint: egui::Stroke,
) {
    painter.line_segment([app.w2s(centre, rect), app.w2s(major_end, rect)], hint);
    let major = major_end - centre;
    if major.len() < EPS {
        return;
    }
    let v_hat = major.normalized().perp();
    let semi_minor = (cursor_world - centre).dot(v_hat).abs();
    if let Some(el) = ellipse_center_major_minor(centre, major_end, semi_minor) {
        draw_polyline_full_ellipse(painter, rect, app, &el, dash);
    }
    let along = (cursor_world - centre).dot(major.normalized());
    let foot = centre + major.normalized() * along;
    painter.line_segment([app.w2s(foot, rect), app.w2s(cursor_world, rect)], hint);
}

pub(super) fn draw_polyline_full_ellipse(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    el: &Ellipse,
    stroke: egui::Stroke,
) {
    let n = 64;
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = (i as f64 / n as f64) * std::f64::consts::TAU;
        pts.push(app.w2s(el.point_at(t), rect));
    }
    painter.add(egui::Shape::line(pts, stroke));
}

/// Unsigned perpendicular distance from a world-space point `p` to the
/// given geometry. Used by Offset's Through-point mode to compute the
/// distance the new parallel copy should be from the source. Returns
/// `f64::NAN` for geometry types we don't support (caller should treat
/// NaN as failure).
/// Parse a single distance value leniently. Accepts:
///   "2"          → Some(2.0)
///   " 2.5 "      → Some(2.5)
///   "d1=2"       → Some(2.0)   (strips "d1=", "d2=", "r=" prefixes)
///   "r=3"        → Some(3.0)
///   "r=2*2"      → Some(4.0)   (the rest goes through the calculator)
/// Returns None for anything else (including empty string — caller
/// should test for empty before calling).
pub(super) fn parse_dist_lenient(store: &crate::calc::CalcStore, raw: &str) -> Option<f64> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    // Match the prefix case-insensitively, but slice it off the ORIGINAL-case
    // string — variables are case-sensitive, so `r=H` must evaluate `H`, not
    // a lowercased `h`. The prefixes are ASCII, so byte slicing is safe.
    let lower = t.to_ascii_lowercase();
    let cleaned: &str = if lower.starts_with("d1=") {
        &t[3..]
    } else if lower.starts_with("d2=") {
        &t[3..]
    } else if lower.starts_with("r=") {
        &t[2..]
    } else {
        t
    };
    let c = cleaned.trim();
    if let Ok(v) = c.parse::<f64>() {
        return Some(v);
    }
    crate::calc::eval(store, c).ok()
}

/// Tokenize a distance phrase into numeric values, lenient about
/// separators. Accepts both whitespace and commas (and "d1=", "d2=",
/// "r=" prefixes on each token). E.g. "2,3" → [2,3]; "d1=2 d2=3" →
/// [2,3]; "2 3 4" → [2,3,4]. Each numeric token may be an expression.
pub(super) fn parse_dist_tokens(store: &crate::calc::CalcStore, raw: &str) -> Vec<f64> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(|s| {
            if s.is_empty() {
                None
            } else {
                parse_dist_lenient(store, s)
            }
        })
        .collect()
}

/// UCS indicator — small "where's (0,0)" marker. Anchors at the world
/// origin's screen position when that's inside the canvas; otherwise
/// pins to the bottom-left corner of the canvas with a fixed offset.
/// Visual: red center disc + thin gray ring + X / Y axis arrows with
/// labels + placeholder "User logo" box on the X arrow.
///
/// Toggled by `env.UcsIcn`. Future: `env.UcsAvP` will replace the
/// placeholder box with a user-supplied avatar image.
pub(super) fn draw_ucs_icon(painter: &egui::Painter, rect: egui::Rect, app: &CadApp) {
    // Anchor depends on env.UcsMod:
    //   0 = corner ALWAYS (default — simplest legend)
    //   1 = AtOrigin when world (0,0) is inside a padded canvas,
    //       else fall back to corner (AutoCAD UCSICON ORigin mode)
    let pad = 50.0;
    let safe = egui::Rect::from_min_max(
        rect.min + egui::vec2(pad, pad),
        rect.max - egui::vec2(pad, pad),
    );
    let origin_world_screen = app.w2s(Vec2::ZERO, rect);
    let at_origin = app.env.UcsMod == 1 && safe.contains(origin_world_screen);
    let anchor = if at_origin {
        origin_world_screen
    } else {
        egui::pos2(rect.left() + pad, rect.bottom() - pad)
    };

    // Colors — matched to the user's SVG (red center, dark gray ring).
    let red = egui::Color32::from_rgb(142, 25, 19); // #8E1913
    let gray = egui::Color32::from_rgb(160, 165, 175);
    let label = egui::Color32::from_rgb(220, 225, 235);
    let logo_fill = egui::Color32::from_rgb(245, 245, 245);
    let logo_stroke = egui::Color32::from_rgb(120, 120, 120);

    // Center disc + ring.
    painter.circle_filled(anchor, 8.5, red);
    painter.circle_stroke(anchor, 12.0, egui::Stroke::new(1.0, gray));

    // 4 small radial ticks at N/S/E/W (just outside the ring).
    for (dx, dy) in [(13.5, 0.0), (-13.5, 0.0), (0.0, 13.5), (0.0, -13.5)] {
        let p0 = anchor + egui::vec2(dx, dy);
        let p1 = anchor + egui::vec2(dx * 0.85, dy * 0.85);
        painter.line_segment([p0, p1], egui::Stroke::new(1.0, gray));
    }

    // Axis lengths in pixels.
    let axis_len = 58.0;
    let arrow = 7.0;
    let pen = egui::Stroke::new(1.2, label);

    // X axis — to the right.
    let x_tip = anchor + egui::vec2(axis_len, 0.0);
    let x_tail = anchor + egui::vec2(14.5, 0.0);
    painter.line_segment([x_tail, x_tip], pen);
    painter.line_segment([x_tip, x_tip + egui::vec2(-arrow, arrow * 0.65)], pen);
    painter.line_segment([x_tip, x_tip + egui::vec2(-arrow, -arrow * 0.65)], pen);
    painter.text(
        x_tip + egui::vec2(4.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        "X",
        crate::theme::typ::data_value(),
        label,
    );

    // Y axis — upward (screen-up is -y).
    let y_tip = anchor + egui::vec2(0.0, -axis_len);
    let y_tail = anchor + egui::vec2(0.0, -14.5);
    painter.line_segment([y_tail, y_tip], pen);
    painter.line_segment([y_tip, y_tip + egui::vec2(arrow * 0.65, arrow)], pen);
    painter.line_segment([y_tip, y_tip + egui::vec2(-arrow * 0.65, arrow)], pen);
    painter.text(
        y_tip + egui::vec2(-4.0, -2.0),
        egui::Align2::RIGHT_BOTTOM,
        "Y",
        crate::theme::typ::data_value(),
        label,
    );

    // "User logo" placeholder box on the X axis — sits roughly
    // half-way along, above the axis line. Replaced by a real
    // avatar image once env.UcsAvP loading lands.
    let logo_w = 30.0;
    let logo_h = 16.0;
    let logo_center = anchor + egui::vec2(axis_len * 0.55, -logo_h * 0.5 - 3.0);
    let logo_rect = egui::Rect::from_center_size(logo_center, egui::vec2(logo_w, logo_h));
    painter.rect(
        logo_rect,
        3.0,
        logo_fill,
        egui::Stroke::new(0.8, logo_stroke),
    );
    painter.text(
        logo_center,
        egui::Align2::CENTER_CENTER,
        if app.env.UcsAvP.is_empty() {
            "User\nlogo"
        } else {
            "User\nlogo"
        },
        egui::FontId::proportional(7.0),
        egui::Color32::from_rgb(80, 80, 80),
    );

    // Only in AtOrigin mode and only when the origin is actually
    // off-screen → show the delta hint, so the user knows the corner
    // pin is a fallback. In Corner-always mode, the icon IS the
    // reference legend; no apology needed.
    if app.env.UcsMod == 1 && !at_origin {
        let dx = origin_world_screen.x - anchor.x;
        let dy = origin_world_screen.y - anchor.y;
        painter.text(
            anchor + egui::vec2(0.0, 18.0),
            egui::Align2::LEFT_TOP,
            format!("(origin off-screen Δ=({:+.0},{:+.0}) px)", dx, -dy),
            egui::FontId::monospace(9.0),
            egui::Color32::from_rgb(150, 160, 175),
        );
    }
}

pub(super) fn style_effective_aci(
    color: cad_kernel::Color,
    layer: u32,
    layers: &cad_kernel::layer::LayerTable,
) -> Option<u8> {
    match color {
        cad_kernel::Color::Aci(i) => Some(i),
        cad_kernel::Color::ByLayer | cad_kernel::Color::ByBlock => {
            match layers.get(layer).map(|l| l.color) {
                Some(cad_kernel::Color::Aci(i)) => Some(i),
                _ => None,
            }
        }
        cad_kernel::Color::TrueColorRef(_) => None,
    }
}
pub(super) fn paint_ctb_caps_joins(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    pts: &[Vec2],
    closed: bool,
    width_px: f32,
    cap: cad_kernel::plotstyle::EndStyle,
    join: cad_kernel::plotstyle::JoinStyle,
    color: egui::Color32,
    corners: bool,
) {
    let spts: Vec<egui::Pos2> = pts.iter().map(|p| app.w2s(*p, rect)).collect();
    paint_caps_joins_screen(painter, &spts, closed, width_px, cap, join, color, corners);
}
/// Paint ONE cached hatch entry with the egui painter: pattern segs/circs as
/// 0.9 px strokes in `color`, solid batches in DEPTH order (even = `color`,
/// odd = `hole_bg` over-draw). Batches are appended to a single mesh in
/// order, so nested islands still repaint over their holes. `hole_bg` is the
/// canvas colour in model space but the PAGE WHITE inside a layout viewport.
pub(super) fn paint_cached_hatch(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    entry: &HatchCacheEntry,
    color: egui::Color32,
    hole_bg: egui::Color32,
) {
    let stroke = egui::Stroke::new(0.9, color);
    for (a, b) in &entry.segs {
        painter.line_segment([app.w2s(*a, rect), app.w2s(*b, rect)], stroke);
    }
    for (c, r) in &entry.circs {
        let rpx = (*r as f32) * app.scale;
        if rpx >= 0.5 {
            painter.circle_stroke(app.w2s(*c, rect), rpx, stroke);
        }
    }
    if !entry.solid.is_empty() {
        let mut mesh = egui::Mesh::default();
        let mut push_tri = |tri: &[Vec2; 3], col: egui::Color32| {
            let base = mesh.vertices.len() as u32;
            for v in tri {
                mesh.colored_vertex(app.w2s(*v, rect), col);
            }
            mesh.add_triangle(base, base + 1, base + 2);
        };
        for (is_fill, tris) in &entry.solid {
            let col = if *is_fill { color } else { hole_bg };
            for tri in tris {
                push_tri(tri, col);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

pub(super) fn paint_dobject_ctb(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    d: &cad_kernel::DObject,
    color: egui::Color32,
    paper_scale: f32,
    pen: Option<&cad_kernel::plotstyle::PlotStyleTable>,
    layers: &cad_kernel::layer::LayerTable,
) {
    use cad_kernel::plotstyle::{EndStyle, JoinStyle};
    let aci = style_effective_aci(d.style.color, d.style.layer, layers);
    // Pen width: CTB Fixed wins; UseObject → the object's own lineweight.
    let width_mm: f32 = match pen.and_then(|t| aci.map(|a| t.style(a).lineweight)) {
        Some(cad_kernel::plotstyle::PlotWidth::Fixed(w)) => w,
        _ => cad_kernel::lineweight::resolve_lineweight(d.style.lineweight, d.style.layer, layers),
    };
    let width = (width_mm * paper_scale).clamp(1.0, 64.0);
    // Pen cap/join (canvas emulation — egui has no per-stroke styles). Same
    // resolution as the plot scene: UseObject → Round defaults.
    let cap = match pen.and_then(|t| aci.map(|a| t.style(a).end_style)) {
        Some(EndStyle::UseObject) | None => EndStyle::Round,
        Some(other) => other,
    };
    let join = match pen.and_then(|t| aci.map(|a| t.style(a).join_style)) {
        Some(JoinStyle::UseObject) | None => JoinStyle::Round,
        Some(other) => other,
    };
    // Linetype: CTB Id wins; UseObject → the entity's own chain (style, layer).
    let override_lt = pen
        .and_then(|t| aci.map(|a| t.style(a).linetype))
        .and_then(|lt| match lt {
            cad_kernel::plotstyle::PlotLinetype::UseObject => None,
            cad_kernel::plotstyle::PlotLinetype::Id(id) => Some(id),
        });
    let lt_id = override_lt.unwrap_or_else(|| d.style.linetype);
    let pattern = match app.doc.linetypes.get(lt_id).or_else(|| {
        if override_lt.is_some() {
            None
        } else {
            layers
                .get(d.style.layer)
                .and_then(|l| app.doc.linetypes.get(l.linetype))
        }
    }) {
        Some(lt) if !lt.is_continuous() => lt.pattern.clone(),
        _ => {
            // Continuous stroke: cap/join emulation (dashed strokes keep the
            // plain dash rendering — per-dash caps are skipped as an
            // approximation; the exports apply the real styles per segment).
            if matches!(
                &d.geom,
                Geom::Line(_)
                    | Geom::Polyline(_)
                    | Geom::Circle(_)
                    | Geom::Arc(_)
                    | Geom::Ellipse(_)
                    | Geom::EllipseArc(_)
                    | Geom::Spline(_)
            ) {
                let closed = match &d.geom {
                    Geom::Circle(_) | Geom::Ellipse(_) => true,
                    Geom::Polyline(p) => p.closed,
                    // Spline has no closed flag — first ≈ last control point
                    // is the "Close" convention.
                    Geom::Spline(s) => {
                        let n = s.control_points.len();
                        n > 2 && (s.control_points[0] - s.control_points[n - 1]).len() < 1e-6
                    }
                    _ => false,
                };
                let corners = matches!(&d.geom, Geom::Polyline(_));
                // A Bevel pen needs butt SEGMENTS (egui `Shape::line` miters by
                // itself and a bevel can't un-draw its spike). Round/Miter keep
                // the single-strip render (one shape per polyline, not one per
                // segment) with the cap/join emulation layered on top.
                if let Geom::Polyline(p) = &d.geom {
                    if p.widths.is_empty() {
                        if join == JoinStyle::Bevel {
                            for pl in app.preview_world_polylines(&d.geom) {
                                let spts: Vec<egui::Pos2> =
                                    pl.iter().map(|p| app.w2s(*p, rect)).collect();
                                paint_butt_polyline(painter, &spts, width, color);
                                paint_caps_joins_screen(
                                    painter, &spts, closed, width, cap, join, color, corners,
                                );
                            }
                            return;
                        }
                        draw_dobject_thick(painter, rect, app, &d.geom, color, width);
                        for pl in app.preview_world_polylines(&d.geom) {
                            paint_ctb_caps_joins(
                                painter, rect, app, &pl, closed, width, cap, join, color, corners,
                            );
                        }
                        return;
                    }
                }
                draw_dobject_thick(painter, rect, app, &d.geom, color, width);
                for pl in app.preview_world_polylines(&d.geom) {
                    paint_ctb_caps_joins(
                        painter, rect, app, &pl, closed, width, cap, join, color, corners,
                    );
                }
            } else {
                draw_dobject_thick(painter, rect, app, &d.geom, color, width);
            }
            return;
        }
    };
    let lt_scale = if d.style.linetype_scale > 1e-6 {
        d.style.linetype_scale
    } else {
        1.0
    };
    let scale_px = app.scale * lt_scale;
    let stroke = egui::Stroke::new(width, color);
    match &d.geom {
        Geom::Line(_)
        | Geom::Polyline(_)
        | Geom::Circle(_)
        | Geom::Arc(_)
        | Geom::Ellipse(_)
        | Geom::EllipseArc(_)
        | Geom::Spline(_) => {
            for pl in app.preview_world_polylines(&d.geom) {
                let pts: Vec<egui::Pos2> = pl.iter().map(|p| app.w2s(*p, rect)).collect();
                paint_pattern_polyline(painter, &pts, &pattern, scale_px, stroke);
            }
        }
        _ => draw_dobject_thick(painter, rect, app, &d.geom, color, width),
    }
}

pub(super) fn distance_world_to_geom(g: &Geom, p: Vec2) -> f64 {
    match g {
        Geom::Line(l) => {
            let d = l.b - l.a;
            let len_sq = d.len_sq();
            if len_sq < 1e-12 {
                return f64::NAN;
            }
            // Perpendicular distance from point to infinite line.
            ((p - l.a).cross(d)).abs() / len_sq.sqrt()
        }
        Geom::Circle(c) => ((p - c.center).len() - c.radius).abs(),
        Geom::Arc(a) => ((p - a.center).len() - a.radius).abs(),
        // For Polyline/Spline/Ellipse: pick the closest segment of the
        // bbox/centerline as a rough proxy. Better than NAN; user can
        // refine later. (Real perpendicular-to-curve distance is a
        // separate kernel job.)
        _ => f64::NAN,
    }
}

/// POINT display marker name (PDMODE §6 list — the styles the picker offers).
pub(super) fn point_style_name(pd: u8) -> &'static str {
    match pd {
        0 => "Dot",
        2 => "Plus",
        3 => "Cross",
        32 => "Circle·Dot",
        35 => "Circle·X",
        64 => "Square·Dot",
        67 => "Square·X",
        96 => "Circle+Square",
        _ => "custom",
    }
}

/// POINT display marker, PDMODE-dispatched, centred at `c` (screen px) with
/// enclosing half-size `half` (px). PDMODE decode: base `pdmode & 7` = 0 dot /
/// 2 plus / 3 cross (1 = blank); `& 32` adds a circle; `& 64` adds a square.
pub(super) fn paint_point_style(
    p: &egui::Painter,
    c: egui::Pos2,
    pdmode: u8,
    half: f32,
    stroke: egui::Stroke,
) {
    let col = stroke.color;
    let base = pdmode & 7;
    let circle = pdmode & 32 != 0;
    let square = pdmode & 64 != 0;
    if square {
        p.rect_stroke(
            egui::Rect::from_center_size(c, egui::vec2(half * 2.0, half * 2.0)),
            egui::Rounding::ZERO,
            stroke,
        );
    }
    if circle {
        p.circle_stroke(c, if square { half * 0.9 } else { half }, stroke);
    }
    let arm = half * 0.98;
    match base {
        2 => {
            p.line_segment([c - egui::vec2(arm, 0.0), c + egui::vec2(arm, 0.0)], stroke);
            p.line_segment([c - egui::vec2(0.0, arm), c + egui::vec2(0.0, arm)], stroke);
        }
        3 => {
            let d = arm * 0.72;
            p.line_segment([c - egui::vec2(d, d), c + egui::vec2(d, d)], stroke);
            p.line_segment([c + egui::vec2(d, -d), c - egui::vec2(d, -d)], stroke);
        }
        _ => {}
    }
    if base == 0 {
        let dr = if circle || square {
            (half * 0.26).max(1.2)
        } else {
            (half * 0.42).max(1.6)
        };
        p.circle_filled(c, dr, col);
    }
}

/// Screen half-size (px) for a point marker from `Point.size` (PDSIZE): size <
/// 0 = |size|% of the viewport height; size > 0 = drawing units; 0 = default
/// ~5% of the view.
pub(super) fn point_half_px(size: f32, scale: f32, viewport_h_px: f32) -> f32 {
    let h = if size < 0.0 {
        (-size / 100.0) * viewport_h_px * 0.5
    } else if size > 0.0 {
        size * scale * 0.5
    } else {
        0.05 * viewport_h_px * 0.5
    };
    h.clamp(2.0, viewport_h_px * 0.5)
}

pub(super) fn draw_dobject(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    g: &Geom,
    color: egui::Color32,
) {
    draw_dobject_thick(painter, rect, app, g, color, 1.6);
}

/// Build the centerline of a polyline as `(point, full_width)` samples in
/// world space. Arc segments are tessellated (real arcs); width is linearly
/// interpolated start→end within each segment. Shared vertices appear once, so
/// the result is a single continuous path — the input the width-strip filler
/// needs for gap-free mitred corners.
pub(super) fn polyline_width_centerline(p: &Polyline) -> Vec<(Vec2, f64)> {
    let n = p.vertices.len();
    if n < 2 {
        return Vec::new();
    }
    let seg_count = if p.closed { n } else { n - 1 };
    let mut cl: Vec<(Vec2, f64)> = Vec::new();
    for i in 0..seg_count {
        let a = p.vertices[i].pos;
        let b = p.vertices[(i + 1) % n].pos;
        let bulge = p.vertices[i].bulge;
        let (sw, ew) = p.widths.get(i).copied().unwrap_or((0.0, 0.0));
        let mut pts = vec![a];
        append_arc_world_samples(a, b, bulge, &mut pts); // a + interior + b
        let mm = pts.len().max(1);
        for (j, pt) in pts.iter().enumerate() {
            let t = if mm > 1 {
                j as f64 / (mm - 1) as f64
            } else {
                0.0
            };
            let w = sw + (ew - sw) * t; // at j==0, w == sw (this seg's start)
            if i > 0 && j == 0 {
                // The shared start vertex is already in `cl` as segment i-1's
                // end. Re-emit a COINCIDENT point only when the width STEPS
                // here (sw_i != ew_{i-1}); otherwise dedup. This stops the first
                // segment after a width change from falsely tapering.
                if let Some(&(_, prev_w)) = cl.last() {
                    if (prev_w - w).abs() > 1e-9 {
                        cl.push((*pt, w));
                    }
                }
                continue;
            }
            cl.push((*pt, w));
        }
    }
    cl
}

/// Fill a width strip along a `(point, full_width)` centerline. ROBUST against
/// any geometry (sharp / reflex / self-intersecting): each segment is filled as
/// its OWN independent rectangle (using that segment's normal — never skewed,
/// never spikes), and each interior vertex gets a JOINT fill = the convex hull
/// of the four segment-end corners plus the clamped miter apex (sharp corner
/// when within the miter limit, clean bevel beyond it). Widths are world units
/// (scale with zoom); endpoints butt-cap.
pub(super) fn fill_width_strip(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    cl: &[(Vec2, f64)],
    color: egui::Color32,
) {
    let m = cl.len();
    if m < 2 {
        return;
    }
    let unit = |v: Vec2| {
        let l = v.len();
        if l > EPS {
            v / l
        } else {
            Vec2::new(0.0, 0.0)
        }
    };
    let perp = |v: Vec2| Vec2::new(-v.y, v.x);
    let isect = |p0: Vec2, d0: Vec2, p1: Vec2, d1: Vec2| -> Option<Vec2> {
        let den = d0.x * d1.y - d0.y * d1.x;
        if den.abs() < 1e-9 {
            return None;
        }
        let dp = p1 - p0;
        Some(p0 + d0 * ((dp.x * d1.y - dp.y * d1.x) / den))
    };
    // Fill the convex hull of `pts` (angle-sorted around their centroid) as a
    // triangle fan — order-independent, so callers needn't pre-sort.
    let fill_hull = |pts: &[Vec2]| {
        if pts.len() < 3 {
            return;
        }
        let n = pts.len() as f64;
        let c = pts.iter().fold(Vec2::new(0.0, 0.0), |a, &p| a + p) / n;
        let mut s: Vec<Vec2> = pts.to_vec();
        s.sort_by(|a, b| {
            (a.y - c.y)
                .atan2(a.x - c.x)
                .partial_cmp(&(b.y - c.y).atan2(b.x - c.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let scr: Vec<egui::Pos2> = s.iter().map(|p| app.w2s(*p, rect)).collect();
        // ONE convex polygon (not a triangle fan) so egui only anti-aliases the
        // outer boundary — no internal hairline seams across the fill.
        painter.add(egui::Shape::convex_polygon(scr, color, egui::Stroke::NONE));
    };
    const MITER_LIMIT: f64 = 8.0;
    // Per-segment unit direction.
    let seg_dir: Vec<Vec2> = (0..m - 1).map(|k| unit(cl[k + 1].0 - cl[k].0)).collect();
    // Per-vertex miter decision + shared left/right miter points. A vertex
    // MITERS (smooth shared seam) when the apex stays within the limit;
    // otherwise it BEVELS (each segment keeps its own normal; a hull fills the
    // gap) — so sharp/reflex corners can't spike.
    let mut mitered = vec![false; m];
    let mut ml = vec![Vec2::new(0.0, 0.0); m];
    let mut mr = vec![Vec2::new(0.0, 0.0); m];
    for k in 1..m - 1 {
        let (da, db) = (seg_dir[k - 1], seg_dir[k]);
        if da.len() < 0.5 || db.len() < 0.5 {
            continue;
        }
        let h = cl[k].1 * 0.5;
        let (na, nb) = (perp(da), perp(db));
        let l = isect(cl[k].0 + na * h, da, cl[k].0 + nb * h, db);
        let r = isect(cl[k].0 - na * h, da, cl[k].0 - nb * h, db);
        if let (Some(lp), Some(rp)) = (l, r) {
            let dmax = (lp - cl[k].0).len().max((rp - cl[k].0).len());
            if dmax <= MITER_LIMIT * h {
                mitered[k] = true;
                ml[k] = lp;
                mr[k] = rp;
            }
        }
    }
    // Segment quads — mitered corners reuse the SHARED apex (seamless edge);
    // non-mitered corners use the segment's own normal (bevel).
    for k in 0..m - 1 {
        let dseg = seg_dir[k];
        if dseg.len() < 0.5 {
            continue;
        }
        let n = perp(dseg);
        let h0 = cl[k].1 * 0.5;
        let h1 = cl[k + 1].1 * 0.5;
        let (l0, r0) = if mitered[k] {
            (ml[k], mr[k])
        } else {
            (cl[k].0 + n * h0, cl[k].0 - n * h0)
        };
        let (l1, r1) = if mitered[k + 1] {
            (ml[k + 1], mr[k + 1])
        } else {
            (cl[k + 1].0 + n * h1, cl[k + 1].0 - n * h1)
        };
        fill_hull(&[l0, l1, r1, r0]);
    }
    // Bevel fill at sharp (non-mitered) interior vertices.
    for k in 1..m - 1 {
        if mitered[k] {
            continue;
        }
        let (da, db) = (seg_dir[k - 1], seg_dir[k]);
        if da.len() < 0.5 || db.len() < 0.5 {
            continue;
        }
        let h = cl[k].1 * 0.5;
        let (na, nb) = (perp(da), perp(db));
        fill_hull(&[
            cl[k].0 + na * h,
            cl[k].0 - na * h,
            cl[k].0 + nb * h,
            cl[k].0 - nb * h,
            cl[k].0,
        ]);
    }
}

/// Render a committed polyline with per-segment widths as one filled, mitred
/// tapered strip.
pub(super) fn draw_polyline_widths(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    p: &Polyline,
    color: egui::Color32,
) {
    let cl = polyline_width_centerline(p);
    fill_width_strip(painter, rect, app, &cl, color);
}

/// Render a DObject honouring its style.linetype (resolved through
/// ByLayer when the dobject's linetype is `Continuous`/0). Falls back
/// to a solid draw via `draw_dobject_thick` for Continuous patterns —
/// the dashed path only kicks in when the resolved pattern is non-
/// empty. Currently shapes: Line, Wall (left+right sides). Other
/// variants fall through to solid (Arc/Circle/etc. linetype rendering
/// is a follow-up — needs polyline-from-tessellation + dash).
/// Stroke a screen-space polyline with a dash/gap pattern. `pattern` is in
/// WORLD units; `scale` is px per world unit. Even pattern index = dash,
/// odd = gap. A dash whose ON-SCREEN length is below ~1.5 px renders as a
/// filled DOT so dot/centre/border linetypes stay readable. The pattern
/// PHASE carries across polyline segments so corners look continuous.
/// Empty / sub-pixel patterns fall back to a solid polyline.
pub(super) fn paint_pattern_polyline(
    painter: &egui::Painter,
    pts: &[egui::Pos2],
    pattern_world: &[f32],
    scale: f32,
    stroke: egui::Stroke,
) {
    if pts.len() < 2 {
        return;
    }
    let pat: Vec<f32> = pattern_world
        .iter()
        .map(|p| (p.abs() * scale).max(0.0))
        .collect();
    let total: f32 = pat.iter().sum();
    if pat.is_empty() || total < 0.5 {
        painter.add(egui::Shape::line(pts.to_vec(), stroke));
        return;
    }
    let dot_thresh = 1.5_f32; // px: dash shorter than this = dot
    let dot_r = (stroke.width * 0.65).max(1.0);
    let n = pat.len();
    let mut idx = 0usize;
    let mut remaining = pat[0];
    while remaining <= 1e-4 && idx + 1 < n {
        idx += 1;
        remaining = pat[idx];
    }
    let mut pen_down = idx % 2 == 0;
    let mut dot_pending = pen_down && pat[idx] <= dot_thresh;
    for w in pts.windows(2) {
        let a = w[0];
        let b = w[1];
        let seg = b - a;
        let seg_len = seg.length();
        if seg_len < 1e-6 {
            continue;
        }
        let dir = seg / seg_len;
        let mut pos = 0.0f32;
        while pos < seg_len - 1e-4 {
            if pen_down && dot_pending {
                painter.circle_filled(a + dir * pos, dot_r, stroke.color);
                dot_pending = false;
            }
            let step = remaining.min(seg_len - pos);
            if pen_down && pat[idx] > dot_thresh {
                painter.line_segment([a + dir * pos, a + dir * (pos + step)], stroke);
            }
            pos += step;
            remaining -= step;
            if remaining <= 1e-4 {
                let mut guard = 0;
                loop {
                    idx = (idx + 1) % n;
                    remaining = pat[idx];
                    pen_down = idx % 2 == 0;
                    guard += 1;
                    if remaining > 1e-4 || guard > n {
                        break;
                    }
                }
                dot_pending = pen_down && pat[idx] <= dot_thresh;
            }
        }
    }
}

/// Paint a linetype's pattern as a horizontal SAMPLE inside `rect` (for the
/// graphical picker). The pattern is normalised so ~2.2 cycles span the
/// width, so every linetype shows its character regardless of its world
/// magnitudes. Continuous draws a plain line.
pub(super) fn paint_linetype_sample(
    painter: &egui::Painter,
    rect: egui::Rect,
    pattern: &[f32],
    color: egui::Color32,
) {
    let y = rect.center().y;
    let stroke = egui::Stroke::new(1.4, color);
    let pts = [
        egui::pos2(rect.left() + 3.0, y),
        egui::pos2(rect.right() - 3.0, y),
    ];
    if pattern.is_empty() {
        painter.line_segment(pts, stroke);
        return;
    }
    let total: f32 = pattern.iter().map(|p| p.abs()).sum();
    let w = (rect.width() - 6.0).max(1.0);
    let scale = if total > 1e-6 { (w / total) / 2.2 } else { 1.0 };
    paint_pattern_polyline(painter, &pts, pattern, scale, stroke);
}

/// Graphical linetype dropdown — each row shows the rendered SAMPLE of the
/// pattern (line / dashes / dots, like LibreCAD's picker) next to the name,
/// plus an inline sample of the current one before the combo. Returns the
/// id the user clicked, if any. `lts` = (id, pattern, name).
pub(super) fn graphical_linetype_combo(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash,
    cur_id: u32,
    lts: &[(u32, Vec<f32>, String)],
) -> Option<u32> {
    let mut picked = None;
    let sample_col = ui.visuals().text_color();
    let cur_name = lts
        .iter()
        .find(|(i, _, _)| *i == cur_id)
        .map(|(_, _, n)| n.clone())
        .unwrap_or_else(|| "?".into());
    // Inline sample of the CURRENT linetype before the combo.
    let (cr, _) = ui.allocate_exact_size(egui::vec2(44.0, 14.0), egui::Sense::hover());
    if let Some((_, pat, _)) = lts.iter().find(|(i, _, _)| *i == cur_id) {
        paint_linetype_sample(ui.painter(), cr, pat, sample_col);
    }
    egui::ComboBox::from_id_salt(salt)
        .selected_text(cur_name)
        .width(150.0)
        .show_ui(ui, |ui| {
            for (id, pat, name) in lts {
                let desired = egui::vec2(ui.available_width().max(200.0), 18.0);
                let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
                if *id == cur_id {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
                }
                let sr = egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(2.0, 0.0),
                    egui::vec2(60.0, rect.height()),
                );
                paint_linetype_sample(ui.painter(), sr, pat, sample_col);
                ui.painter().text(
                    egui::pos2(sr.right() + 8.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    crate::theme::typ::body(),
                    ui.visuals().text_color(),
                );
                if resp.clicked() {
                    picked = Some(*id);
                }
            }
        });
    picked
}

pub(super) fn paint_dobject_with_style(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    d: &cad_kernel::DObject,
    color: egui::Color32,
) {
    use cad_kernel::LinetypeTable;
    // Effective linetype id: dobject's own when explicit; otherwise the
    // layer's (ByLayer). Continuous on both → solid render.
    let lt_id = if d.style.linetype == LinetypeTable::CONTINUOUS {
        app.doc
            .layers
            .get(d.style.layer)
            .map(|l| l.linetype)
            .unwrap_or(LinetypeTable::CONTINUOUS)
    } else {
        d.style.linetype
    };
    let pattern = match app.doc.linetypes.get(lt_id) {
        Some(lt) if !lt.is_continuous() => lt.pattern.clone(),
        _ => {
            draw_dobject(painter, rect, app, &d.geom, color);
            return;
        }
    };
    // Effective px scale = camera px/unit × per-dobject linetype scale.
    let lt_scale = if d.style.linetype_scale > 1e-6 {
        d.style.linetype_scale
    } else {
        1.0
    };
    let scale_px = app.scale * lt_scale;
    let stroke = egui::Stroke::new(1.6, color);
    match &d.geom {
        // Stroked geometry: tessellate to world polylines (curves sampled),
        // then walk the full pattern. Wall/Hatch/Text/Dimension/BlockRef
        // keep their dedicated solid render (fill / glyphs / recursion).
        Geom::Line(_)
        | Geom::Polyline(_)
        | Geom::Circle(_)
        | Geom::Arc(_)
        | Geom::Ellipse(_)
        | Geom::EllipseArc(_)
        | Geom::Spline(_) => {
            for pl in app.preview_world_polylines(&d.geom) {
                let pts: Vec<egui::Pos2> = pl.iter().map(|p| app.w2s(*p, rect)).collect();
                paint_pattern_polyline(painter, &pts, &pattern, scale_px, stroke);
            }
        }
        _ => draw_dobject(painter, rect, app, &d.geom, color),
    }
}

/// Screen-space face polylines for a wall — the SINGLE source the solid,
/// dashed-selection, and linetype render paths all share so they can't
/// diverge. Straight walls get neighbour-mitred faces via
/// `cad_wall::solve_faces`; curved walls get EXACT concentric arc samples
/// (`Wall::face_polylines`, radial normals — no chord-tilt gap at fillet
/// joints) with a zoom-adaptive sample count (same pattern as the Arc
/// renderer, so close zoom shows no faceting).
/// Batt-INSULATION symbol for a wall: a sine wave running along the centerline,
/// oscillating across the cavity (amplitude auto-fit to the thickness). Returns
/// world-space points (caller maps to screen). Works for straight + curved
/// walls (sampled along the centerline, offset by the local normal).
pub(super) fn wall_insulation_wave(w: &cad_kernel::Wall) -> Vec<Vec2> {
    let amp = (w.thickness * 0.5 * 0.72).max(1e-9);
    let wavelen = w.thickness.max(1e-6); // ~one loop per wall width
    let step = (wavelen / 14.0).max(1e-6); // ~14 samples per wave
                                           // Dense centerline samples (straight: build them; curved: sample the arc).
    let cl: Vec<Vec2> = {
        let d = w.end - w.start;
        let len = d.len();
        if len < 1e-9 {
            return Vec::new();
        }
        let n = ((len / step).ceil() as usize).max(2);
        if w.is_curved() {
            w.centerline_polyline(n)
        } else {
            (0..=n)
                .map(|i| w.start + d * (i as f64 / n as f64))
                .collect()
        }
    };
    if cl.len() < 2 {
        return Vec::new();
    }
    // Walk arc length; offset each sample by amp·sin(2π·s/λ) along local normal.
    let mut out = Vec::with_capacity(cl.len());
    let mut acc = 0.0_f64;
    for i in 0..cl.len() {
        let dir = if i + 1 < cl.len() {
            cl[i + 1] - cl[i]
        } else {
            cl[i] - cl[i - 1]
        };
        let u = if dir.len() > 1e-12 {
            dir.normalized()
        } else {
            Vec2::new(1.0, 0.0)
        };
        let nperp = u.perp();
        let off = amp * (std::f64::consts::TAU * acc / wavelen).sin();
        out.push(cl[i] + nperp * off);
        if i + 1 < cl.len() {
            acc += (cl[i + 1] - cl[i]).len();
        }
    }
    out
}

/// Screen-space face polylines for a wall, as LISTS of pieces per side. A
/// straight wall is normally one piece per side, but an X-crossing splits a
/// face into several disjoint segments (the opening at the junction). Curved
/// walls return one sampled polyline per side.
pub(super) fn wall_face_screen_pts(
    app: &CadApp,
    rect: egui::Rect,
    w: &cad_kernel::Wall,
) -> (Vec<Vec<egui::Pos2>>, Vec<Vec<egui::Pos2>>) {
    if w.is_curved() {
        let n = match cad_kernel::bulge_arc(w.start, w.end, w.bulge) {
            Some((_c, r, _a0, sweep)) => {
                let arc_px = ((r + w.thickness * 0.5) * sweep.abs()) as f32 * app.scale;
                (arc_px * 0.25).clamp(12.0, 256.0) as usize
            }
            None => 28,
        };
        match w.face_polylines(n) {
            Some((l, r)) => (
                vec![l.iter().map(|p| app.w2s(*p, rect)).collect()],
                vec![r.iter().map(|p| app.w2s(*p, rect)).collect()],
            ),
            None => (Vec::new(), Vec::new()),
        }
    } else {
        let walls: Vec<cad_kernel::Wall> = app
            .doc
            .dobjects
            .iter()
            .filter_map(|d| {
                if let Geom::Wall(x) = &d.geom {
                    Some(x.clone())
                } else {
                    None
                }
            })
            .collect();
        let to_screen = |segs: Vec<(Vec2, Vec2)>| -> Vec<Vec<egui::Pos2>> {
            segs.iter()
                .map(|(a, b)| vec![app.w2s(*a, rect), app.w2s(*b, rect)])
                .collect()
        };
        match cad_wall::solve_face_segments(w, &walls) {
            Some((ls, rs)) => (to_screen(ls), to_screen(rs)),
            None => match (w.left_line(), w.right_line()) {
                (Some(l), Some(r)) => (
                    vec![vec![app.w2s(l.a, rect), app.w2s(l.b, rect)]],
                    vec![vec![app.w2s(r.a, rect), app.w2s(r.b, rect)]],
                ),
                _ => (Vec::new(), Vec::new()),
            },
        }
    }
}

/// Same as `draw_dobject` with a parameterised stroke width. Used for the
/// trim-cutter / extend-boundary pulse overlay (Item 5) which draws a
/// thicker animated outline above the dobject's normal color.
pub(super) fn draw_dobject_thick(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    g: &Geom,
    color: egui::Color32,
    width: f32,
) {
    let stroke = egui::Stroke::new(width, color);
    match g {
        Geom::Line(l) => {
            painter.line_segment([app.w2s(l.a, rect), app.w2s(l.b, rect)], stroke);
        }
        Geom::Xline(x) => {
            // Infinite line — draw only the part inside the visible area.
            if let Some(seg) = app.xline_visible_segment(x, rect) {
                painter.line_segment([app.w2s(seg.a, rect), app.w2s(seg.b, rect)], stroke);
            }
        }
        Geom::Ray(r) => {
            if let Some(seg) = app.ray_visible_segment(r, rect) {
                painter.line_segment([app.w2s(seg.a, rect), app.w2s(seg.b, rect)], stroke);
            }
        }
        Geom::Donut(d) => {
            // Filled ring: outer disc in the object colour, hole punched in
            // the canvas background (CPU painter — same look as the hatch
            // cache's hole over-draw).
            let bg = egui::Color32::from_rgb(18, 22, 28);
            let n = 48;
            let ring = |r: f64, col: egui::Color32| {
                let pts: Vec<egui::Pos2> = (0..=n)
                    .map(|i| {
                        let t = std::f64::consts::TAU * (i as f64 / n as f64);
                        app.w2s(d.center + Vec2::new(r * t.cos(), r * t.sin()), rect)
                    })
                    .collect();
                painter.add(egui::Shape::Path(egui::epaint::PathShape {
                    points: pts,
                    closed: true,
                    fill: col,
                    stroke: egui::epaint::PathStroke::NONE,
                }));
            };
            ring(d.outer_radius, stroke.color);
            if d.inner_radius > 1e-9 {
                ring(d.inner_radius, bg);
            }
        }
        Geom::Region(rg) => {
            // Filled region — solid fill in the object colour.
            let pts: Vec<egui::Pos2> = rg.loop_pts.iter().map(|p| app.w2s(*p, rect)).collect();
            if pts.len() >= 3 {
                painter.add(egui::Shape::Path(egui::epaint::PathShape {
                    points: pts,
                    closed: true,
                    fill: stroke.color,
                    stroke: egui::epaint::PathStroke::NONE,
                }));
            }
        }
        Geom::CenterMark(cm) => {
            // Crosshair mark: two crossing lines of length `size` through
            // the centre, rotated by `rotation`.
            let c = app.w2s(cm.center, rect);
            let half = (cm.size as f32 * 0.5) * app.scale;
            let (sin, cos) = (cm.rotation as f32).sin_cos();
            let d = egui::vec2(cos * half, sin * half);
            let n = egui::vec2(-sin * half, cos * half);
            painter.line_segment([c - d, c + d], stroke);
            painter.line_segment([c - n, c + n], stroke);
        }
        Geom::Wipeout(w) => {
            // Mask: filled with the paper/canvas colour (AutoCAD wipeout).
            let pts: Vec<egui::Pos2> = w.pts.iter().map(|p| app.w2s(*p, rect)).collect();
            if pts.len() >= 3 {
                painter.add(egui::Shape::Path(egui::epaint::PathShape {
                    points: pts,
                    closed: true,
                    fill: crate::theme::color::SURFACE_0,
                    stroke: egui::epaint::PathStroke::NONE,
                }));
            }
        }
        Geom::Xref(x) => {
            // Resolved children, transformed + drawn recursively (their own
            // per-dobject styles are flattened into the xref's stroke colour).
            for d in &x.cached {
                let tg = x.transform_geom(&d.geom);
                draw_dobject(painter, rect, app, &tg, stroke.color);
            }
        }
        Geom::Circle(c) => {
            let center = app.w2s(c.center, rect);
            let r_px = c.radius as f32 * app.scale;
            painter.circle_stroke(center, r_px, stroke);
        }
        Geom::Arc(a) => {
            let r_px = (a.radius as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.5).clamp(8.0, 256.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = a.start_angle + (i as f64 / n as f64) * a.sweep_angle;
                let p = Vec2::new(
                    a.center.x + a.radius * t.cos(),
                    a.center.y + a.radius * t.sin(),
                );
                pts.push(app.w2s(p, rect));
            }
            painter.add(egui::Shape::line(pts, stroke));
        }
        Geom::Ellipse(el) => {
            // Tessellation density grows with the visible size on screen so
            // small ellipses stay cheap and large ones stay smooth.
            let r_px = (el.semi_major() as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.7).clamp(16.0, 512.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = (i as f64 / n as f64) * std::f64::consts::TAU;
                pts.push(app.w2s(el.point_at(t), rect));
            }
            painter.add(egui::Shape::line(pts, stroke));
        }
        Geom::EllipseArc(ea) => {
            let r_px = (ea.ellipse.semi_major() as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.7).clamp(12.0, 512.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = ea.start_param + (i as f64 / n as f64) * ea.sweep_param;
                pts.push(app.w2s(ea.ellipse.point_at(t), rect));
            }
            painter.add(egui::Shape::line(pts, stroke));
        }
        Geom::Point(pt) => {
            // PDMODE / PDSIZE point marker (the style/size stamped when the
            // point was placed — see current_point_style/current_point_size).
            let sp = app.w2s(pt.location, rect);
            let half = point_half_px(pt.size, app.scale, rect.height());
            paint_point_style(painter, sp, pt.style, half, stroke);
        }
        Geom::Polyline(p) => {
            // Tapered-width polyline → filled strips per segment. Otherwise the
            // normal bulge-aware stroke (arc segments render as real arcs).
            let has_width = p
                .widths
                .iter()
                .any(|&(s, e)| s.abs() > 1e-9 || e.abs() > 1e-9);
            if has_width {
                draw_polyline_widths(painter, rect, app, p, color);
            } else {
                let pts = polyline_tessellated_screen_pts(p, app, rect);
                if !pts.is_empty() {
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
        }
        Geom::Hatch(_) => {
            // Hatch render needs to resolve boundary handles against
            // the Document — done by `App::render_hatch_fill` which
            // the main render loop short-circuits to BEFORE reaching
            // this free renderer. Reaching here means a caller passed
            // a Hatch without going through the main loop; that's a
            // bug, so silently drop instead of crashing.
            let _ = (color, stroke);
        }
        Geom::Spline(s) => {
            // NURBS → screen polyline. Density scales with on-screen
            // size of the control polygon; pickbox-precision at most
            // typical zooms. Refine when someone zooms in past the
            // chord error of a 64-sample tessellation.
            let (min, max) = s.bbox();
            let bbox_diag_px = ((max - min).len() as f32) * app.scale;
            let n = (bbox_diag_px * 0.5).clamp(32.0, 512.0) as usize;
            let samples = s.tessellate(n);
            if samples.len() < 2 {
                return;
            }
            let pts: Vec<egui::Pos2> = samples.iter().map(|w| app.w2s(*w, rect)).collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
        Geom::Wall(w) => {
            // Resolve the WallStyle → face color override + poché fill.
            let wstyle = app.doc.wall_styles.get(w.style);
            let face_col = match wstyle {
                Some(s) if s.face_color != 0 => {
                    let (r, g, b) = aci_palette(s.face_color.min(255) as u8);
                    egui::Color32::from_rgb(r, g, b)
                }
                _ => color,
            };
            let face_stroke = egui::Stroke::new(width, face_col);
            let fill_aci = wstyle.map(|s| s.fill_color).unwrap_or(0);

            // Faces from the shared derivation (mitred straights, X-crossing
            // cleaned, or exact concentric curved arcs). Each side is a LIST of
            // pieces (a crossing splits a face at the junction opening).
            let (left_faces, right_faces) = wall_face_screen_pts(app, rect, w);

            // Poché fill — only when the band is a single un-broken quad/strip
            // (no crossing); a split face has an ambiguous interior, skip it.
            if fill_aci != 0
                && left_faces.len() == 1
                && right_faces.len() == 1
                && left_faces[0].len() >= 2
                && left_faces[0].len() == right_faces[0].len()
            {
                let (lp, rp) = (&left_faces[0], &right_faces[0]);
                let (fr, fg, fb) = aci_palette(fill_aci.min(255) as u8);
                let fill = egui::Color32::from_rgba_unmultiplied(fr, fg, fb, 80);
                let mut mesh = egui::Mesh::default();
                for i in 0..lp.len() {
                    mesh.colored_vertex(lp[i], fill);
                    mesh.colored_vertex(rp[i], fill);
                }
                for i in 0..lp.len() - 1 {
                    let a = (2 * i) as u32;
                    mesh.add_triangle(a, a + 1, a + 2);
                    mesh.add_triangle(a + 1, a + 3, a + 2);
                }
                painter.add(egui::Shape::mesh(mesh));
            }
            // Faces — draw every piece.
            let n_face = left_faces
                .iter()
                .chain(right_faces.iter())
                .map(|p| p.len())
                .max()
                .unwrap_or(2);
            for pl in left_faces.into_iter().chain(right_faces.into_iter()) {
                if pl.len() >= 2 {
                    painter.add(egui::Shape::line(pl, face_stroke));
                }
            }

            // Batt-INSULATION symbol (sine wave) in the cavity.
            if wstyle.map(|s| s.insulation).unwrap_or(false) {
                let wave = wall_insulation_wave(w);
                if wave.len() >= 2 {
                    let pts: Vec<egui::Pos2> = wave.iter().map(|p| app.w2s(*p, rect)).collect();
                    painter.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(width.max(1.0), face_col),
                    ));
                }
            }

            // Centerline — in the wall STYLE's centerline linetype (app-layer map) when
            // set, else the `WlCnL` toggle's default dashed line. Neither → faces only.
            let cl_ltype = app.wall_centerline_ltype.get(&w.style).copied();
            if cl_ltype.is_some() || app.env.WlCnL {
                let cl_col =
                    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 110);
                let cl_stroke = egui::Stroke::new(width * 0.8, cl_col);
                // Same sample count as the faces so the overlays agree.
                let clpts: Vec<egui::Pos2> = w
                    .centerline_polyline(n_face.max(3) - 1)
                    .iter()
                    .map(|p| app.w2s(*p, rect))
                    .collect();
                let pattern = cl_ltype
                    .and_then(|id| app.doc.linetypes.get(id))
                    .filter(|lt| !lt.is_continuous())
                    .map(|lt| lt.pattern.clone());
                match pattern {
                    Some(pat) => {
                        paint_pattern_polyline(painter, &clpts, &pat, app.scale, cl_stroke)
                    }
                    None if cl_ltype.is_some() => {
                        // explicit CONTINUOUS centerline → solid line
                        if clpts.len() >= 2 {
                            painter.add(egui::Shape::line(clpts, cl_stroke));
                        }
                    }
                    None => {
                        // no per-style linetype, WlCnL on → the original default dashed
                        for s in egui::Shape::dashed_line(&clpts, cl_stroke, 6.0, 4.0) {
                            painter.add(s);
                        }
                    }
                }
            }
        }
        Geom::Text(t) => {
            // V1: render via egui's bundled fonts. Future LFF / SHX
            // stroke-font swap-in will use the same Text data fields.
            // Skip empty strings — egui::Painter::text on "" still
            // allocates a layout.
            if t.text.is_empty() {
                return;
            }
            // Map world-space height → screen pixels via the camera scale.
            // Below ~4 px the text is illegible anyway; skip the layout.
            let size_px = (t.height * app.scale as f64) as f32;
            if size_px < 4.0 {
                return;
            }
            let anchor = app.w2s(t.position, rect);
            let align = match (t.h_align, t.v_align) {
                (cad_kernel::TextHAlign::Left, cad_kernel::TextVAlign::Top) => {
                    egui::Align2::LEFT_TOP
                }
                (cad_kernel::TextHAlign::Left, cad_kernel::TextVAlign::Middle) => {
                    egui::Align2::LEFT_CENTER
                }
                (cad_kernel::TextHAlign::Left, _) => egui::Align2::LEFT_BOTTOM,
                (cad_kernel::TextHAlign::Center, cad_kernel::TextVAlign::Top) => {
                    egui::Align2::CENTER_TOP
                }
                (cad_kernel::TextHAlign::Center, cad_kernel::TextVAlign::Middle) => {
                    egui::Align2::CENTER_CENTER
                }
                (cad_kernel::TextHAlign::Center, _) => egui::Align2::CENTER_BOTTOM,
                (cad_kernel::TextHAlign::Right, cad_kernel::TextVAlign::Top) => {
                    egui::Align2::RIGHT_TOP
                }
                (cad_kernel::TextHAlign::Right, cad_kernel::TextVAlign::Middle) => {
                    egui::Align2::RIGHT_CENTER
                }
                (cad_kernel::TextHAlign::Right, _) => egui::Align2::RIGHT_BOTTOM,
            };
            // Resolve font: the entity's explicit font, else its style's —
            // the SAME chain TXTEXP and hatch tracing use (single source of
            // truth, can't drift). Pick the matching egui FontId; unknown
            // names fall back to proportional so old DXF files keep
            // rendering.
            let ttf_font = app.resolve_text_font(t);
            let font_id = font_id_for_font_name(&ttf_font, size_px);
            // Rotation: egui's `text` doesn't support angles directly.
            // Use a Galley + manual transform if we ever need rotated
            // text. v1: ignore the angle and warn only when non-zero
            // would be visually wrong (deferred — most CAD text is at
            // angle 0 anyway).
            let _ = t.angle;
            painter.text(anchor, align, &t.text, font_id, color);
        }
        Geom::Dimension(d) => {
            draw_dimension(painter, rect, app, d, color);
        }
        Geom::BlockRef(br) => {
            draw_blockref(painter, rect, app, br, color, width, 0);
        }
        _ => {}
    }
}

/// Render a block instance: every contained dobject transformed into
/// world space and drawn with its own resolved style; `Color::ByBlock`
/// contents take the instance's color (this is where ByBlock finally
/// means something). Nested instances recurse; `depth` is a hard cap —
/// cycles can't form in v1 (see block.rs) but render stays bounded
/// regardless.
pub(super) fn draw_blockref(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    br: &cad_kernel::BlockRef,
    instance_color: egui::Color32,
    width: f32,
    depth: u8,
) {
    if depth > 8 {
        return;
    }
    let Some(blk) = app.doc.blocks.get(br.block) else {
        // Dangling reference — draw a small red marker at the insertion
        // point instead of silently vanishing.
        let p = app.w2s(br.insert, rect);
        painter.circle_stroke(
            p,
            5.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(220, 60, 60)),
        );
        return;
    };
    // Parametric blocks: derive contents at this instance's values first.
    let derived = app.block_derived_geoms(blk, &br.param_values);
    for (cd, dg) in blk.dobjects.iter().zip(&derived) {
        let g = br.transform_geom(dg, blk.base);
        let col = if matches!(cd.style.color, Color::ByBlock) {
            instance_color
        } else {
            let (r, gn, b) = resolve_color(
                cd.style.color,
                cd.style.layer,
                &app.doc.layers,
                &app.doc.truecolors,
            );
            egui::Color32::from_rgb(r, gn, b)
        };
        if let Geom::BlockRef(nested) = &g {
            draw_blockref(painter, rect, app, nested, col, width, depth + 1);
        } else {
            draw_dobject_thick(painter, rect, app, &g, col, width);
        }
    }
}

/// Minimum on-screen size for dimension annotations so arrowheads and
/// the text never vanish when the dim is small relative to the geometry
/// it measures (the classic "DIMSCALE too small" problem — annotation
/// elements are 0.18 world units by default). World-space sizing still
/// applies ABOVE these floors, so zooming in / raising `overall_scale`
/// grows them normally.
const DIM_MIN_ARROW_PX: f32 = 8.0;
const DIM_MIN_TEXT_PX: f32 = 11.0;

/// Role-tagged world-space geometry for a dimension. SINGLE source of
/// truth shared by the solid renderer (`draw_dimension`) and the dashed
/// selection overlay so the two can't drift. Lines are split by role so
/// the renderer can color extension vs dim lines independently.
pub(super) struct DimGeo {
    /// Extension lines — drawn in the EXT-line color.
    ext_lines: Vec<(Vec2, Vec2)>,
    /// The Linear dim line (None for radius/diameter) — gap-trim candidate.
    dim_line: Option<(Vec2, Vec2)>,
    /// Radius/diameter leader legs — drawn in the DIM-line color.
    leaders: Vec<(Vec2, Vec2)>,
    /// Arrowheads as `(tip, inward_dir)`.
    arrows: Vec<(Vec2, Vec2)>,
    text_pos: Vec2,
    /// World rotation for the text (0 = horizontal). Aligned linear dims
    /// set this to the readability-corrected dim-line angle.
    text_angle: f64,
    /// True when text sits centered ON the dim line (DIMTAD 0) — the
    /// renderer breaks the dim line to leave a gap for it.
    text_on_dim_line: bool,
}

impl DimGeo {
    /// Every structural line, role-agnostic — for the dashed overlay.
    pub(super)     fn all_lines(&self) -> Vec<(Vec2, Vec2)> {
        let mut v = self.ext_lines.clone();
        if let Some(dl) = self.dim_line {
            v.push(dl);
        }
        v.extend(self.leaders.iter().copied());
        v
    }
}

pub(super) fn dim_render_geometry(d: &cad_kernel::Dim, style: &cad_kernel::DimStyle) -> DimGeo {
    use cad_kernel::{DimKind, LinearOrtho};
    let ext_extend_w = style.ext_line_extend * style.overall_scale;
    let ext_offset_w = style.ext_line_offset * style.overall_scale;
    let text_gap_w = style.text_gap * style.overall_scale;
    let text_h_w = style.text_height * style.overall_scale;
    let mut g = DimGeo {
        ext_lines: Vec::new(),
        dim_line: None,
        leaders: Vec::new(),
        arrows: Vec::new(),
        text_pos: Vec2::new(0.0, 0.0),
        text_angle: 0.0,
        text_on_dim_line: false,
    };
    match &d.kind {
        DimKind::Linear {
            p1,
            p2,
            dimline_pos,
            ortho,
        } => {
            let chord = *p2 - *p1;
            if chord.len() < 1e-9 {
                g.text_pos = *p1;
                return g;
            }
            let (u, n) = match ortho {
                LinearOrtho::Aligned => {
                    let u = chord.normalized();
                    (u, Vec2::new(-u.y, u.x))
                }
                LinearOrtho::Horizontal => (Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)),
                LinearOrtho::Vertical => (Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)),
            };
            let dim_offset = (*dimline_pos - *p1).dot(n);
            let n_signed = if dim_offset >= 0.0 { n } else { -n };
            let off_mag = dim_offset.abs();
            let (a, b) = match ortho {
                LinearOrtho::Aligned => (*p1 + n_signed * off_mag, *p2 + n_signed * off_mag),
                LinearOrtho::Horizontal => (
                    Vec2::new(p1.x, dimline_pos.y),
                    Vec2::new(p2.x, dimline_pos.y),
                ),
                LinearOrtho::Vertical => (
                    Vec2::new(dimline_pos.x, p1.y),
                    Vec2::new(dimline_pos.x, p2.y),
                ),
            };
            let n_from_p1 = (a - *p1).normalized();
            let n_from_p2 = (b - *p2).normalized();
            if !style.ext_suppress_1 {
                g.ext_lines
                    .push((*p1 + n_from_p1 * ext_offset_w, a + n_from_p1 * ext_extend_w));
            }
            if !style.ext_suppress_2 {
                g.ext_lines
                    .push((*p2 + n_from_p2 * ext_offset_w, b + n_from_p2 * ext_extend_w));
            }
            g.dim_line = Some((a, b));
            let dim_dir = (b - a).normalized();
            g.arrows.push((a, dim_dir));
            g.arrows.push((b, -dim_dir));
            // Text placement from DIMTAD (text_vert_pos): 0 = on the line
            // (line gets trimmed), 4 = below, else above.
            let mid = (a + b) * 0.5;
            let lift = text_gap_w + text_h_w * 0.5;
            let (pos, on_line) = match style.text_vert_pos {
                0 => (mid, true),
                4 => (mid - n_signed * lift, false),
                _ => (mid + n_signed * lift, false),
            };
            g.text_pos = pos;
            g.text_on_dim_line = on_line;
            // Aligned dims rotate the text with the line; horizontal
            // otherwise (DIMTIH). Keep it upright (no upside-down text).
            if !style.text_inside_horiz {
                let mut ang = u.y.atan2(u.x);
                if ang > std::f64::consts::FRAC_PI_2 {
                    ang -= std::f64::consts::PI;
                }
                if ang < -std::f64::consts::FRAC_PI_2 {
                    ang += std::f64::consts::PI;
                }
                g.text_angle = ang;
            }
            g
        }
        DimKind::Radius {
            center,
            on_circle,
            leader_end,
        } => {
            g.leaders.push((*center, *on_circle));
            g.leaders.push((*on_circle, *leader_end));
            let radial = (*on_circle - *center).normalized();
            g.arrows.push((*on_circle, -radial));
            let outward = (*leader_end - *center).normalized();
            g.text_pos = *leader_end + outward * (text_gap_w + text_h_w * 0.5);
            g
        }
        DimKind::Diameter {
            center,
            on_circle,
            leader_end,
        } => {
            let opp = *center * 2.0 - *on_circle;
            g.leaders.push((*on_circle, opp));
            g.leaders.push((*on_circle, *leader_end));
            let radial = (*on_circle - *center).normalized();
            g.arrows.push((*on_circle, -radial));
            g.arrows.push((opp, radial));
            let outward = (*leader_end - *center).normalized();
            g.text_pos = *leader_end + outward * (text_gap_w + text_h_w * 0.5);
            g
        }
        _ => g,
    }
}

/// Full dimension renderer — extension lines (ext color), dim line /
/// leaders (dim color), arrowheads (filled / hollow / architectural tick),
/// and the text label (text color, optional alignment with the line, and
/// a line-gap when centered on the dim line). Per-element colors fall back
/// to the dobject's resolved `color` when the style field is 0 (ByBlock).
/// Annotation sizes clamp to a screen-space floor so they never vanish.
pub(super) fn draw_dimension(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    d: &cad_kernel::Dim,
    color: egui::Color32,
) {
    let style = app
        .doc
        .dim_styles
        .get(d.style)
        .or_else(|| app.doc.dim_styles.get(0))
        .cloned()
        .unwrap_or_else(cad_kernel::DimStyle::standard);
    let scale = app.scale.max(1e-6);
    // Per-element color: a non-zero (non-ByBlock) style field wins,
    // otherwise the dobject's resolved color.
    let resolve = |aci: u32| {
        if aci != 0 {
            let (r, g, b) = aci_palette(aci.min(255) as u8);
            egui::Color32::from_rgb(r, g, b)
        } else {
            color
        }
    };
    let dim_col = resolve(style.color_dim_line);
    let ext_col = resolve(style.color_ext_line);
    let text_col = resolve(style.color_text);
    let dim_stroke = egui::Stroke::new(1.2, dim_col);
    let ext_stroke = egui::Stroke::new(1.2, ext_col);

    let geo = dim_render_geometry(d, &style);

    // Extension lines + leaders.
    for (a, b) in &geo.ext_lines {
        painter.line_segment([app.w2s(*a, rect), app.w2s(*b, rect)], ext_stroke);
    }
    for (a, b) in &geo.leaders {
        painter.line_segment([app.w2s(*a, rect), app.w2s(*b, rect)], dim_stroke);
    }

    // Lay out the text once — its width drives the dim-line gap.
    let text = d.formatted_text(&style);
    let size_px =
        ((style.text_height * style.overall_scale * scale as f64) as f32).max(DIM_MIN_TEXT_PX);
    let galley = if text.is_empty() {
        None
    } else {
        Some(painter.layout_no_wrap(text.clone(), egui::FontId::proportional(size_px), text_col))
    };

    // Dim line (Linear), broken around centered text.
    if let Some((a, b)) = geo.dim_line {
        let gap_w = if geo.text_on_dim_line {
            galley.as_ref().map(|g| {
                g.size().x as f64 / scale as f64 * 0.5 + style.text_gap * style.overall_scale
            })
        } else {
            None
        };
        match gap_w {
            Some(hw) => {
                let u = (b - a).normalized();
                let len = (b - a).len();
                let g1 = geo.text_pos - u * hw;
                let g2 = geo.text_pos + u * hw;
                let da = (g1 - a).dot(u);
                let db = (g2 - a).dot(u);
                if da > 0.0 && db < len && da < db {
                    painter.line_segment([app.w2s(a, rect), app.w2s(g1, rect)], dim_stroke);
                    painter.line_segment([app.w2s(g2, rect), app.w2s(b, rect)], dim_stroke);
                } else {
                    painter.line_segment([app.w2s(a, rect), app.w2s(b, rect)], dim_stroke);
                }
            }
            None => {
                painter.line_segment([app.w2s(a, rect), app.w2s(b, rect)], dim_stroke);
            }
        }
    }

    // Arrowheads / ticks — world-sized, floored so they never vanish.
    let arrow_size_w =
        (style.arrow_size * style.overall_scale).max(DIM_MIN_ARROW_PX as f64 / scale as f64);
    let tick_w =
        (style.tick_size * style.overall_scale).max(DIM_MIN_ARROW_PX as f64 / scale as f64);
    for (tip, dir) in &geo.arrows {
        if style.tick_size > 0.0 {
            // Architectural tick — a 45° slash centered on the def point.
            let dn = dir.normalized();
            let c = std::f64::consts::FRAC_1_SQRT_2;
            let t = Vec2::new(dn.x * c - dn.y * c, dn.x * c + dn.y * c);
            painter.line_segment(
                [
                    app.w2s(*tip + t * tick_w, rect),
                    app.w2s(*tip - t * tick_w, rect),
                ],
                egui::Stroke::new(1.6, dim_col),
            );
        } else if style.arrow_filled {
            draw_filled_arrow(painter, rect, app, *tip, *dir, arrow_size_w, dim_col);
        } else {
            // Hollow arrowhead — outline only.
            let dn = dir.normalized();
            let perp = Vec2::new(-dn.y, dn.x);
            let base = *tip + dn * arrow_size_w;
            let b1 = base + perp * (arrow_size_w * 0.35);
            let b2 = base - perp * (arrow_size_w * 0.35);
            let ts = app.w2s(*tip, rect);
            let b1s = app.w2s(b1, rect);
            let b2s = app.w2s(b2, rect);
            let s = egui::Stroke::new(1.4, dim_col);
            painter.line_segment([ts, b1s], s);
            painter.line_segment([b1s, b2s], s);
            painter.line_segment([b2s, ts], s);
        }
    }

    // Text — centered at text_pos, rotated by text_angle (world → screen
    // needs the Y flip).
    if let Some(galley) = galley {
        let anchor = app.w2s(geo.text_pos, rect);
        let screen_angle = -geo.text_angle as f32;
        let sz = galley.size();
        let rot = egui::emath::Rot2::from_angle(screen_angle);
        let pos = anchor - rot * (sz * 0.5);
        let mut shape = egui::epaint::TextShape::new(pos, galley, text_col);
        shape.angle = screen_angle;
        painter.add(shape);
    }
}

/// Filled triangle arrowhead. The arrow's TIP is at `tip_w`; the base
/// runs perpendicular to `dir_w` and the arrow stretches `size_w` back
/// along `-dir_w`. `dir_w` is the unit vector pointing INTO the arrow
/// (i.e. the direction the dim/leader line is travelling at the tip).
pub(super) fn draw_filled_arrow(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    tip_w: Vec2,
    dir_w: Vec2,
    size_w: f64,
    color: egui::Color32,
) {
    if size_w < 1e-9 {
        return;
    }
    let perp = Vec2::new(-dir_w.y, dir_w.x);
    let base_center = tip_w + dir_w * size_w;
    let half_base = size_w * 0.35; // ~20° half-angle, AutoCAD default
    let b1 = base_center + perp * half_base;
    let b2 = base_center - perp * half_base;
    let tip_s = app.w2s(tip_w, rect);
    let b1_s = app.w2s(b1, rect);
    let b2_s = app.w2s(b2, rect);
    painter.add(egui::Shape::convex_polygon(
        vec![tip_s, b1_s, b2_s],
        color,
        egui::Stroke::NONE,
    ));
}

/// Compact color chip for the Dim Style form: swatch + ACI/ByBlock label
/// + "Pick…" button + a ByBlock checkbox. Mutates `val` for ByBlock and
/// returns `true` when the user asks to open the shared polar ACI wheel
/// (the caller then sets the appropriate `AciPickRequest::DimStyleForm`).
pub(super) fn dim_color_swatch(ui: &mut egui::Ui, val: &mut Option<u8>) -> bool {
    let mut want = false;
    let (r, g, b) = match *val {
        Some(a) => aci_palette(a),
        None => (140, 140, 160),
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 18.0), egui::Sense::click());
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(0.7, egui::Color32::from_rgb(70, 80, 95)),
    );
    if resp.on_hover_text("Click to pick an ACI color").clicked() {
        want = true;
    }
    ui.label(match *val {
        Some(a) => format!("ACI {}", a),
        None => "ByBlock".to_string(),
    });
    if ui.small_button("Pick…").clicked() {
        want = true;
    }
    let mut bb = val.is_none();
    if ui.checkbox(&mut bb, "ByBlock").changed() {
        *val = if bb { None } else { Some(7) };
    }
    want
}

/// Sample preview for the Dimension Style Manager. Renders OUR OWN
/// reference drawing — a rounded-corner plate with a bolt hole — annotated
/// with a horizontal + vertical linear dim, a diameter, and a radius, each
/// drawn from the given style's arrow size / text height / decimals /
/// color. Deliberately NOT AutoCAD's L-bracket sample. Self-contained:
/// maps a fixed sample-world box into `rect` (no global camera).
/// Sample preview for the Wall Style Manager — a short wall segment showing
/// the style's thickness, poché fill, and face color.
pub(super) fn draw_wall_style_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    style: &cad_kernel::WallStyle,
    centerline_pattern: Option<&[f32]>,
) {
    let cx = rect.center().x;
    let cy = rect.center().y;
    let half_len = rect.width() * 0.32;
    // Map thickness to px (STANDARD 0.2 → ~26px), clamped to fit.
    let h = ((style.thickness as f32) * 130.0).clamp(8.0, rect.height() * 0.45);
    let r = egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(half_len * 2.0, h));
    if style.fill_color != 0 {
        let (fr, fg, fb) = aci_palette(style.fill_color.min(255) as u8);
        painter.rect_filled(
            r,
            0.0,
            egui::Color32::from_rgba_unmultiplied(fr, fg, fb, 90),
        );
    }
    let face_col = if style.face_color != 0 {
        let (a, b, c) = aci_palette(style.face_color.min(255) as u8);
        egui::Color32::from_rgb(a, b, c)
    } else {
        egui::Color32::from_rgb(236, 241, 248)
    };
    let s = egui::Stroke::new(1.6, face_col);
    painter.line_segment([r.left_top(), r.right_top()], s);
    painter.line_segment([r.left_bottom(), r.right_bottom()], s);
    let cl = egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 140, 160));
    let clpts = [
        egui::pos2(r.left() - 12.0, cy),
        egui::pos2(r.right() + 12.0, cy),
    ];
    match centerline_pattern {
        // Dashed/center/hidden linetype → show it (fit ~3 repeats across the preview).
        Some(pat) if !pat.is_empty() => {
            let total: f32 = pat.iter().map(|x| x.abs()).sum::<f32>().max(1e-4);
            let scale = (r.width() * 0.5) / (total * 3.0);
            paint_pattern_polyline(painter, &clpts, pat, scale, cl);
        }
        // Explicit CONTINUOUS centerline → solid.
        Some(_) => {
            painter.line_segment(clpts, cl);
        }
        // No centerline linetype set → the reference dashed middle line, as before.
        None => {
            for seg in egui::Shape::dashed_line(&clpts, cl, 6.0, 4.0) {
                painter.add(seg);
            }
        }
    }
    painter.text(
        egui::pos2(cx, r.bottom() + 14.0),
        egui::Align2::CENTER_TOP,
        format!("t = {:.3}", style.thickness),
        crate::theme::typ::data_value(),
        face_col,
    );
}

pub(super) fn draw_dim_style_preview(painter: &egui::Painter, rect: egui::Rect, style: &cad_kernel::DimStyle) {
    let pad = 16.0_f32;
    let (xmin, xmax, ymin, ymax) = (-5.0_f32, 44.0, -3.0, 31.0);
    let ww = xmax - xmin;
    let wh = ymax - ymin;
    let s = ((rect.width() - 2.0 * pad) / ww)
        .min((rect.height() - 2.0 * pad) / wh)
        .max(0.1);
    let ox = rect.center().x - s * (xmin + xmax) / 2.0;
    let oy = rect.center().y + s * (ymin + ymax) / 2.0; // y-up world
    let w2p = |x: f32, y: f32| egui::pos2(ox + x * s, oy - y * s);

    let obj_col = egui::Color32::from_rgb(232, 176, 86);
    let resolve = |c: u32, fallback: egui::Color32| {
        if c != 0 {
            let (r, g, b) = aci_palette(c.min(255) as u8);
            egui::Color32::from_rgb(r, g, b)
        } else {
            fallback
        }
    };
    let dim_col = resolve(style.color_dim_line, egui::Color32::from_rgb(236, 241, 248));
    let ext_col = resolve(style.color_ext_line, egui::Color32::from_rgb(150, 170, 200));
    let text_col = resolve(style.color_text, egui::Color32::from_rgb(236, 241, 248));
    let obj_stroke = egui::Stroke::new(1.6, obj_col);
    let dim_stroke = egui::Stroke::new(1.3, dim_col);
    let ext_stroke = egui::Stroke::new(1.1, ext_col);

    // Annotation sizes derive from the style but clamp so the preview is
    // always legible (default 0.18-ish would be sub-pixel vs the sample).
    let annot = 64.0_f32;
    let arrow_px = (style.arrow_size as f32 * annot).clamp(6.0, 17.0);
    let text_px = (style.text_height as f32 * annot).clamp(9.0, 26.0);

    let fmt = |v: f64| {
        let dp = style.decimal_places.max(0) as usize;
        let mut t = format!("{:.*}", dp, v);
        if style.decimal_separator != '.' {
            t = t.replace('.', &style.decimal_separator.to_string());
        }
        t
    };
    // Arrowhead honoring filled / hollow / architectural tick. `dir` is the
    // screen-space unit pointing INTO the arrow (along the dim line).
    let arrow = |tip: egui::Pos2, dir: egui::Vec2| {
        if dir.length() < 1e-3 {
            return;
        }
        let d = dir.normalized();
        if style.tick_size > 0.0 {
            let c = std::f32::consts::FRAC_1_SQRT_2;
            let t = egui::vec2(d.x * c - d.y * c, d.x * c + d.y * c);
            painter.line_segment(
                [tip + t * arrow_px, tip - t * arrow_px],
                egui::Stroke::new(1.6, dim_col),
            );
        } else {
            let perp = egui::vec2(-d.y, d.x);
            let base = tip + d * arrow_px;
            let b1 = base + perp * (arrow_px * 0.34);
            let b2 = base - perp * (arrow_px * 0.34);
            if style.arrow_filled {
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, b1, b2],
                    dim_col,
                    egui::Stroke::NONE,
                ));
            } else {
                painter.line_segment([tip, b1], dim_stroke);
                painter.line_segment([b1, b2], dim_stroke);
                painter.line_segment([b2, tip], dim_stroke);
            }
        }
    };
    // Rotated, centered text. `angle` is world radians (0 = horizontal).
    // Returns the galley size in sample-world units (for line-gap math).
    let draw_label = |cx: f32, cy: f32, angle: f32, txt: &str| -> egui::Vec2 {
        let galley = painter.layout_no_wrap(
            txt.to_string(),
            egui::FontId::proportional(text_px),
            text_col,
        );
        let sz = galley.size();
        let anchor = w2p(cx, cy);
        let sa = -angle;
        let rot = egui::emath::Rot2::from_angle(sa);
        let pos = anchor - rot * (sz * 0.5);
        let mut shape = egui::epaint::TextShape::new(pos, galley, text_col);
        shape.angle = sa;
        painter.add(shape);
        egui::vec2(sz.x / s, sz.y / s)
    };

    // ---- object: rounded-corner plate + bolt hole -------------------
    let mut outline = vec![
        w2p(4.0, 4.0),
        w2p(40.0, 4.0),
        w2p(40.0, 24.0),
        w2p(10.0, 24.0),
    ];
    for i in 0..=12 {
        let deg = 90.0 + (i as f32 / 12.0) * 90.0; // top-left fillet
        let r = deg.to_radians();
        outline.push(w2p(10.0 + 6.0 * r.cos(), 18.0 + 6.0 * r.sin()));
    }
    outline.push(w2p(4.0, 4.0));
    painter.add(egui::Shape::line(outline, obj_stroke));

    let mut hole = Vec::with_capacity(41);
    for i in 0..=40 {
        let a = (i as f32 / 40.0) * std::f32::consts::TAU;
        hole.push(w2p(28.0 + 5.0 * a.cos(), 12.0 + 5.0 * a.sin()));
    }
    painter.add(egui::Shape::line(hole, obj_stroke));

    let lift = 1.6_f32; // sample-world text offset for above/below

    // ---- top horizontal linear dim (shows text placement) -----------
    {
        let (ax, bx, ay) = (10.0_f32, 40.0_f32, 27.0_f32);
        painter.line_segment([w2p(10.0, 24.3), w2p(10.0, ay + 0.6)], ext_stroke);
        painter.line_segment([w2p(40.0, 24.3), w2p(40.0, ay + 0.6)], ext_stroke);
        let mid = ((ax + bx) * 0.5, ay);
        let (tx, ty, on_line) = match style.text_vert_pos {
            0 => (mid.0, mid.1, true),
            4 => (mid.0, mid.1 - lift, false),
            _ => (mid.0, mid.1 + lift, false),
        };
        let sz = draw_label(tx, ty, 0.0, &fmt(30.0)); // horizontal line → text upright
        let a_s = w2p(ax, ay);
        let b_s = w2p(bx, ay);
        if on_line {
            let hw = sz.x * 0.5 + 0.4;
            painter.line_segment([a_s, w2p(tx - hw, ay)], dim_stroke);
            painter.line_segment([w2p(tx + hw, ay), b_s], dim_stroke);
        } else {
            painter.line_segment([a_s, b_s], dim_stroke);
        }
        arrow(a_s, egui::vec2(b_s.x - a_s.x, b_s.y - a_s.y));
        arrow(b_s, egui::vec2(a_s.x - b_s.x, a_s.y - b_s.y));
    }
    // ---- left vertical linear dim (shows aligned vs horizontal) -----
    {
        let (ax, ay, by) = (0.0_f32, 4.0_f32, 24.0_f32);
        painter.line_segment([w2p(3.6, 4.0), w2p(ax - 0.6, 4.0)], ext_stroke);
        painter.line_segment([w2p(3.6, 24.0), w2p(ax - 0.6, 24.0)], ext_stroke);
        let mid_y = (ay + by) * 0.5;
        let aligned = !style.text_inside_horiz;
        let angle = if aligned {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        };
        let (tx, ty, on_line) = match style.text_vert_pos {
            0 => (ax, mid_y, true),
            4 => (ax + lift, mid_y, false),
            _ => (ax - lift, mid_y, false),
        };
        let sz = draw_label(tx, ty, angle, &fmt(20.0));
        // extent along the (vertical) line: text width if aligned, else height
        let along = if aligned { sz.x } else { sz.y };
        let a_s = w2p(ax, ay);
        let b_s = w2p(ax, by);
        if on_line {
            let hw = along * 0.5 + 0.4;
            painter.line_segment([a_s, w2p(ax, ty - hw)], dim_stroke);
            painter.line_segment([w2p(ax, ty + hw), b_s], dim_stroke);
        } else {
            painter.line_segment([a_s, b_s], dim_stroke);
        }
        arrow(a_s, egui::vec2(b_s.x - a_s.x, b_s.y - a_s.y));
        arrow(b_s, egui::vec2(a_s.x - b_s.x, a_s.y - b_s.y));
    }
    // ---- diameter through the hole ----------------------------------
    {
        let (cx, cy) = (28.0_f32, 12.0_f32);
        let (dx, dy) = (0.70711_f32, 0.70711_f32);
        let pa = w2p(cx - 5.0 * dx, cy - 5.0 * dy);
        let pb = w2p(cx + 5.0 * dx, cy + 5.0 * dy);
        let cs = w2p(cx, cy);
        painter.line_segment([pa, pb], dim_stroke);
        arrow(pa, cs - pa);
        arrow(pb, cs - pb);
        draw_label(
            cx + 5.0 * dx + 2.0,
            cy + 5.0 * dy + 2.0,
            0.0,
            &format!("⌀{}", fmt(10.0)),
        );
    }
    // ---- radius on the rounded corner -------------------------------
    {
        let (ccx, ccy) = (10.0_f32, 18.0_f32);
        let ang = 135.0_f32.to_radians();
        let (apx, apy) = (ccx + 6.0 * ang.cos(), ccy + 6.0 * ang.sin());
        let (lex, ley) = (apx - 3.6, apy + 3.6);
        let aps = w2p(apx, apy);
        let les = w2p(lex, ley);
        let ccs = w2p(ccx, ccy);
        painter.line_segment([aps, les], dim_stroke);
        arrow(aps, ccs - aps);
        draw_label(lex - 2.0, ley + 1.0, 0.0, &format!("R{}", fmt(6.0)));
    }
}

/// Render a Dobject's geometry as a DASHED polyline (used today for the
/// pointer-mode selection look; see `feedback_rust_cad_pointer_is_selector`).
/// Reuses the same per-variant tessellation as `draw_dobject`, then passes
/// the resulting polyline through `egui::Shape::dashed_line`.
pub(super) fn draw_dobject_dashed(
    painter: &egui::Painter,
    rect: egui::Rect,
    app: &CadApp,
    g: &Geom,
    color: egui::Color32,
    dash: f32,
    gap: f32,
) {
    let stroke = egui::Stroke::new(1.6, color);
    let push_dashed = |pts: Vec<egui::Pos2>| {
        for s in egui::Shape::dashed_line(&pts, stroke, dash, gap) {
            painter.add(s);
        }
    };
    match g {
        Geom::Line(l) => {
            push_dashed(vec![app.w2s(l.a, rect), app.w2s(l.b, rect)]);
        }
        Geom::Circle(c) => {
            // Tessellate the circle into a closed polygon and dash it.
            let r_px = (c.radius as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.7).clamp(24.0, 256.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = (i as f64 / n as f64) * std::f64::consts::TAU;
                let p = Vec2::new(
                    c.center.x + c.radius * t.cos(),
                    c.center.y + c.radius * t.sin(),
                );
                pts.push(app.w2s(p, rect));
            }
            push_dashed(pts);
        }
        Geom::Arc(a) => {
            let r_px = (a.radius as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.5).clamp(8.0, 256.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = a.start_angle + (i as f64 / n as f64) * a.sweep_angle;
                let p = Vec2::new(
                    a.center.x + a.radius * t.cos(),
                    a.center.y + a.radius * t.sin(),
                );
                pts.push(app.w2s(p, rect));
            }
            push_dashed(pts);
        }
        Geom::Ellipse(el) => {
            let r_px = (el.semi_major() as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.7).clamp(16.0, 512.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = (i as f64 / n as f64) * std::f64::consts::TAU;
                pts.push(app.w2s(el.point_at(t), rect));
            }
            push_dashed(pts);
        }
        Geom::EllipseArc(ea) => {
            let r_px = (ea.ellipse.semi_major() as f32 * app.scale).max(1.0);
            let n = ((r_px * 0.7).clamp(12.0, 512.0)) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = ea.start_param + (i as f64 / n as f64) * ea.sweep_param;
                pts.push(app.w2s(ea.ellipse.point_at(t), rect));
            }
            push_dashed(pts);
        }
        Geom::Point(pt) => {
            // Point glyph stays solid even when "selected" — dashing a
            // 4-pixel cross is meaningless.
            let sp = app.w2s(pt.location, rect);
            let s = 4.0_f32;
            painter.line_segment(
                [egui::pos2(sp.x - s, sp.y), egui::pos2(sp.x + s, sp.y)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(sp.x, sp.y - s), egui::pos2(sp.x, sp.y + s)],
                stroke,
            );
        }
        Geom::Polyline(p) => {
            // Same bulge-aware tessellation as the solid render — when
            // the user dash-highlights a polyline with arc segments
            // the dashes follow the arcs, not the chords.
            let pts = polyline_tessellated_screen_pts(p, app, rect);
            if !pts.is_empty() {
                push_dashed(pts);
            }
        }
        Geom::Hatch(_) => {
            // The boundary dobjects already render their OWN selection
            // outline when selected — and editing happens on the
            // boundary, not on the hatch itself. Hatch dashed-overlay
            // is a no-op until we have a selectable-hatch UI flow.
        }
        Geom::Spline(s) => {
            // Selection highlight — same tessellation as the solid
            // render, dashed.
            let (min, max) = s.bbox();
            let bbox_diag_px = ((max - min).len() as f32) * app.scale;
            let n = (bbox_diag_px * 0.5).clamp(32.0, 512.0) as usize;
            let samples = s.tessellate(n);
            if samples.len() < 2 {
                return;
            }
            let pts: Vec<egui::Pos2> = samples.iter().map(|w| app.w2s(*w, rect)).collect();
            push_dashed(pts);
        }
        Geom::Wall(w) => {
            // Dashed selection overlay — same shared face derivation as
            // the solid renderer (mitred straights, exact curved arcs) so
            // the highlight matches what is actually drawn. The old
            // left_line()/right_line() chords showed straight dashes
            // across curved walls.
            let (l_faces, r_faces) = wall_face_screen_pts(app, rect, w);
            for pl in l_faces.into_iter().chain(r_faces.into_iter()) {
                if pl.len() >= 2 {
                    push_dashed(pl);
                }
            }
        }
        Geom::Text(t) => {
            // Dashed-selection overlay on Text isn't a dashed string
            // (egui can't dash glyph outlines); draw a small dashed
            // rectangle around the anchor so the user sees it's
            // selected. The actual text underneath stays visible from
            // the regular render pass.
            let half = (t.height as f32 * app.scale * 0.5).max(3.0);
            let p = app.w2s(t.position, rect);
            let r = egui::Rect::from_center_size(p, egui::vec2(half * 2.0, half * 2.0));
            let _ = push_dashed; // closure available; we use lines directly
            for s in egui::Shape::dashed_line(
                &[
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ],
                stroke,
                dash,
                gap,
            ) {
                painter.add(s);
            }
        }
        Geom::Dimension(d) => {
            // Dashed-selection overlay — dash the dim's REAL structural
            // lines (extension + dim line, or the leader legs) via the
            // shared `dim_render_geometry`, so the highlight matches what
            // is actually drawn instead of a placeholder triangle.
            let style = app
                .doc
                .dim_styles
                .get(d.style)
                .or_else(|| app.doc.dim_styles.get(0))
                .cloned()
                .unwrap_or_else(cad_kernel::DimStyle::standard);
            let geo = dim_render_geometry(d, &style);
            for (a, b) in &geo.all_lines() {
                for s in egui::Shape::dashed_line(
                    &[app.w2s(*a, rect), app.w2s(*b, rect)],
                    stroke,
                    dash,
                    gap,
                ) {
                    painter.add(s);
                }
            }
        }
        Geom::BlockRef(br) => {
            // Dashed-selection overlay — recurse into the transformed
            // contents so the whole instance highlights as one entity.
            if let Some(blk) = app.doc.blocks.get(br.block) {
                for cd in &blk.dobjects {
                    let g = br.transform_geom(&cd.geom, blk.base);
                    draw_dobject_dashed(painter, rect, app, &g, color, dash, gap);
                }
            }
        }
        _ => {}
    }
}

// ===================================================================
