use super::*;

/// Short one-letter badge string for the active osnap kinds, shown on the
/// toolbar button so the user can see at a glance what's enabled.
pub(super) fn active_snap_letters(s: SnapSet) -> String {
    let mut buf = String::with_capacity(7);
    for k in SnapKind::ALL {
        if s.is_enabled(k) {
            buf.push(k.name().chars().next().unwrap());
        }
    }
    if buf.is_empty() {
        buf.push('—');
    }
    buf
}

// ---- Registry-driven settings page widgets --------------------------------

/// A left-sidebar section entry: name + variable count, selectable.
pub(super) fn settings_section_item(
    ui: &mut egui::Ui,
    name: &str,
    count: usize,
    selected: bool,
) -> bool {
    let h = 26.0;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let bg = if selected {
        egui::Color32::from_rgb(38, 54, 72)
    } else if resp.hovered() {
        egui::Color32::from_rgb(32, 37, 46)
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 3.0, bg);
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(3.0, h)),
            0.0,
            egui::Color32::from_rgb(91, 155, 213),
        );
    }
    let txt_c = if selected {
        egui::Color32::from_rgb(225, 232, 240)
    } else {
        egui::Color32::from_rgb(170, 178, 190)
    };
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        txt_c,
    );
    ui.painter().text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(110, 118, 130),
    );
    resp.clicked()
}

/// (colour, label) for a variable status.
pub(super) fn settings_status_meta(s: crate::varreg::Status) -> (egui::Color32, &'static str) {
    use crate::varreg::Status;
    match s {
        Status::Active => (egui::Color32::from_rgb(67, 181, 129), "ACTIVE"),
        Status::Planned => (egui::Color32::from_rgb(91, 155, 213), "PLANNED"),
        Status::Stub => (egui::Color32::from_rgb(120, 128, 140), "STUB"),
        Status::Tentative => (egui::Color32::from_rgb(212, 168, 83), "TENTAT."),
    }
}

/// A small pill badge for a variable's status.
pub(super) fn settings_status_badge(ui: &mut egui::Ui, s: crate::varreg::Status) {
    let (col, label) = settings_status_meta(s);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), egui::FontId::proportional(9.0), col);
    let pad = egui::vec2(6.0, 3.0);
    let (rect, _resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
    ui.painter().rect(
        rect,
        3.0,
        col.linear_multiply(0.12),
        egui::Stroke::new(1.0, col.linear_multiply(0.5)),
    );
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, col);
}

/// The status legend shown in the panel header.
pub(super) fn settings_legend(ui: &mut egui::Ui) {
    use crate::varreg::Status;
    for s in [
        Status::Tentative,
        Status::Stub,
        Status::Planned,
        Status::Active,
    ] {
        let (col, label) = settings_status_meta(s);
        ui.label(egui::RichText::new(label).size(9.0).color(col));
    }
}

// ---- Legacy settings-window widgets (kept for env preview helpers) ---------
//
// Each row pairs the cryptic field name (bold, monospace) with a plain-
// English description and a type-appropriate input. The cryptic name is
// what gets persisted; the description is just for humans.

#[allow(dead_code)]
pub(super) fn env_row(ui: &mut egui::Ui, key: &str, desc: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [70.0, 18.0],
            egui::Label::new(egui::RichText::new(key).monospace().strong()),
        );
        ui.add_sized(
            [200.0, 18.0],
            egui::Label::new(egui::RichText::new(desc).small()),
        );
        body(ui);
    });
}

pub(super) fn env_bool(ui: &mut egui::Ui, key: &str, desc: &str, v: &mut bool) {
    env_row(ui, key, desc, |ui| {
        ui.checkbox(v, "");
    });
}

pub(super) fn env_u8(ui: &mut egui::Ui, key: &str, desc: &str, v: &mut u8, lo: u8, hi: u8) {
    env_row(ui, key, desc, |ui| {
        ui.add(egui::Slider::new(v, lo..=hi));
    });
}

pub(super) fn env_u8_choice(
    ui: &mut egui::Ui,
    key: &str,
    desc: &str,
    v: &mut u8,
    choices: &[&str],
) {
    env_row(ui, key, desc, |ui| {
        let sel = (*v as usize).min(choices.len().saturating_sub(1));
        egui::ComboBox::from_id_salt(key)
            .selected_text(choices.get(sel).copied().unwrap_or(""))
            .show_ui(ui, |ui| {
                for (i, label) in choices.iter().enumerate() {
                    ui.selectable_value(v, i as u8, *label);
                }
            });
    });
}

pub(super) fn env_color(ui: &mut egui::Ui, key: &str, desc: &str, v: &mut u32) {
    env_row(ui, key, desc, |ui| {
        let mut rgb = [
            ((*v >> 16) & 0xFF) as u8,
            ((*v >> 8) & 0xFF) as u8,
            (*v & 0xFF) as u8,
        ];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            *v = ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | (rgb[2] as u32);
        }
        ui.monospace(format!("0x{:06X}", *v));
    });
}

pub(super) fn env_text(ui: &mut egui::Ui, key: &str, desc: &str, v: &mut String) {
    env_row(ui, key, desc, |ui| {
        ui.add(egui::TextEdit::singleline(v).desired_width(180.0));
    });
}

/// Live preview of those settings that have a visible effect — currently
/// snap target / pickbox / crosshair (sizes shown around a virtual cursor)
/// and the grip colour + size on a sample line. Other settings (dialog
/// modes, xref load mode) have no meaningful visual preview and are
/// skipped here.
pub(super) fn draw_settings_preview(ui: &mut egui::Ui, env: &UserEnv) {
    let u_to_col = |rgb: u32| {
        egui::Color32::from_rgb(
            ((rgb >> 16) & 0xFF) as u8,
            ((rgb >> 8) & 0xFF) as u8,
            (rgb & 0xFF) as u8,
        )
    };
    let bg = egui::Color32::from_rgb(18, 22, 28);
    let edge = egui::Color32::from_rgb(70, 80, 95);
    let dobj = egui::Color32::from_rgb(170, 200, 230);
    let cursor_col = egui::Color32::from_rgb(255, 220, 100);

    // ---- Panel 1: snap & picking ----
    ui.label(egui::RichText::new("Snap & picking").monospace());
    let (resp1, p1) = ui.allocate_painter(egui::vec2(240.0, 200.0), egui::Sense::hover());
    let r1 = resp1.rect;
    p1.rect_filled(r1, 2.0, bg);
    p1.rect_stroke(r1, 2.0, egui::Stroke::new(1.0, edge));

    // Cursor sits at the panel centre; draw crosshair lines spanning
    // CrsHrS% of the panel's shorter side, pickbox of PkBxSz, snap circle
    // of SpTGSZ.
    let c = r1.center();
    let short = r1.width().min(r1.height());
    let hair = short * (env.CrsHrS as f32 / 100.0) * 0.5;
    let pen_hair = egui::Stroke::new(1.0, cursor_col.gamma_multiply(0.6));
    p1.line_segment(
        [egui::pos2(c.x - hair, c.y), egui::pos2(c.x + hair, c.y)],
        pen_hair,
    );
    p1.line_segment(
        [egui::pos2(c.x, c.y - hair), egui::pos2(c.x, c.y + hair)],
        pen_hair,
    );

    // Snap target radius (SpTGSZ) — solid cyan circle around cursor
    p1.circle_stroke(
        c,
        env.SpTGSZ as f32,
        egui::Stroke::new(1.2, egui::Color32::from_rgb(80, 230, 240)),
    );
    // Pickbox (PkBxSz) — yellow square around cursor
    let half = env.PkBxSz as f32 * 0.5;
    p1.rect_stroke(
        egui::Rect::from_min_max(
            egui::pos2(c.x - half, c.y - half),
            egui::pos2(c.x + half, c.y + half),
        ),
        0.0,
        egui::Stroke::new(1.0, cursor_col),
    );
    // Tiny labels next to each visual
    p1.text(
        egui::pos2(c.x + hair + 4.0, c.y),
        egui::Align2::LEFT_CENTER,
        format!("CrsHrS={}%", env.CrsHrS),
        egui::FontId::monospace(10.0),
        pen_hair.color,
    );
    p1.text(
        egui::pos2(c.x + env.SpTGSZ as f32 + 4.0, c.y + env.SpTGSZ as f32 + 4.0),
        egui::Align2::LEFT_TOP,
        format!("SpTGSZ={}", env.SpTGSZ),
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(80, 230, 240),
    );
    p1.text(
        egui::pos2(c.x + half + 4.0, c.y - half - 2.0),
        egui::Align2::LEFT_BOTTOM,
        format!("PkBxSz={}", env.PkBxSz),
        egui::FontId::monospace(10.0),
        cursor_col,
    );

    ui.add_space(8.0);

    // ---- Panel 2: grips + highlight + selection preview ----
    ui.label(egui::RichText::new("Grips & highlight").monospace());
    let (resp2, p2) = ui.allocate_painter(egui::vec2(240.0, 180.0), egui::Sense::hover());
    let r2 = resp2.rect;
    p2.rect_filled(r2, 2.0, bg);
    p2.rect_stroke(r2, 2.0, egui::Stroke::new(1.0, edge));

    // Sample line — drawn highlighted IF HltSel is on, otherwise normal.
    let line_a = egui::pos2(r2.left() + 30.0, r2.top() + 50.0);
    let line_b = egui::pos2(r2.right() - 30.0, r2.bottom() - 50.0);
    let line_col = if env.HltSel {
        egui::Color32::from_rgb(255, 200, 80) // selected = yellow
    } else {
        dobj
    };
    p2.line_segment([line_a, line_b], egui::Stroke::new(2.0, line_col));

    // Grips on this line — only drawn when GrpEnb is on.
    if env.GrpEnb {
        let g = env.GrpSz as f32;
        let unsel = u_to_col(env.GrClrU);
        let hot = u_to_col(env.GrClrS);
        let mid = egui::pos2(0.5 * (line_a.x + line_b.x), 0.5 * (line_a.y + line_b.y));
        for (centre, col) in [(line_a, unsel), (mid, hot), (line_b, unsel)] {
            p2.rect(
                egui::Rect::from_center_size(centre, egui::vec2(g, g)),
                1.0,
                col,
                egui::Stroke::new(1.0, egui::Color32::WHITE),
            );
        }
        p2.text(
            line_a + egui::vec2(8.0, -14.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "GrpSz={}  GrClrU=0x{:06X}  GrClrS=0x{:06X}",
                env.GrpSz, env.GrClrU, env.GrClrS
            ),
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(180, 200, 220),
        );
    } else {
        p2.text(
            r2.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            "GrpEnb = OFF (no grips drawn)",
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(180, 200, 220),
        );
    }

    // SelPrv preview cue — faint cyan ghost line above the sample, only
    // shown when the toggle is on.
    if env.SelPrv {
        let ghost_a = egui::pos2(r2.left() + 30.0, r2.top() + 25.0);
        let ghost_b = egui::pos2(r2.right() - 30.0, r2.top() + 25.0);
        p2.line_segment(
            [ghost_a, ghost_b],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 240, 255)),
        );
        p2.text(
            ghost_a + egui::vec2(0.0, -2.0),
            egui::Align2::LEFT_BOTTOM,
            "SelPrv: hover preview shown",
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(120, 240, 255),
        );
    }

    ui.add_space(4.0);
    ui.small("Dialog / xref settings have no visual preview.");
}

pub(super) fn snap_blurb(k: SnapKind) -> &'static str {
    match k {
        SnapKind::End => "endpoints of lines & arcs",
        SnapKind::Mid => "midpoints",
        SnapKind::Cen => "centres of circles & arcs",
        SnapKind::Qua => "quadrants of circles & arcs (E / N / W / S)",
        SnapKind::Int => "intersections between two dobjects",
        SnapKind::Per => "perpendicular foot   (needs anchor click)",
        SnapKind::Tan => "tangent point        (needs anchor click)",
        SnapKind::Nea => "nearest point on the curve",
    }
}

/// Tessellate a polyline (vertex chain + per-vertex bulges) into
/// connected screen-space points. Straight segments add just the
/// endpoint; arc segments add intermediate samples whose density
/// scales with the on-screen arc length so the curve stays smooth at
/// any zoom. Used by every polyline render path (normal / thick /
/// dashed) so a committed pline with arc segments shows the actual
/// arcs, not their chords.
pub(super) fn polyline_tessellated_screen_pts(
    p: &Polyline,
    app: &CadApp,
    rect: egui::Rect,
) -> Vec<egui::Pos2> {
    if p.vertices.len() < 2 {
        return Vec::new();
    }
    let n = p.vertices.len();
    let pairs = if p.closed { n } else { n - 1 };
    let mut out: Vec<egui::Pos2> = Vec::with_capacity(n);
    out.push(app.w2s(p.vertices[0].pos, rect));
    for i in 0..pairs {
        let a = p.vertices[i].pos;
        let b = p.vertices[(i + 1) % n].pos;
        let bulge = p.vertices[i].bulge;
        append_pline_segment_screen_pts(a, b, bulge, app, rect, &mut out);
    }
    out
}

/// Append the tessellation of ONE polyline segment from `a` to `b` with
/// the given bulge to `out`. `a` is assumed already present at the end
/// of `out`; this function adds the intermediate samples + `b`.
/// Straight segment (bulge ≈ 0) appends just `b`.
pub(super) fn append_pline_segment_screen_pts(
    a: Vec2,
    b: Vec2,
    bulge: f64,
    app: &CadApp,
    rect: egui::Rect,
    out: &mut Vec<egui::Pos2>,
) {
    if bulge.abs() < 1e-9 {
        out.push(app.w2s(b, rect));
        return;
    }
    let chord = b - a;
    let chord_len = chord.len();
    if chord_len < EPS {
        out.push(app.w2s(b, rect));
        return;
    }
    let theta = 4.0 * bulge.atan();
    let half = theta * 0.5;
    let sin_half = half.sin();
    if sin_half.abs() < EPS {
        out.push(app.w2s(b, rect));
        return;
    }
    let r = chord_len / (2.0 * sin_half.abs());
    let chord_hat = chord / chord_len;
    let perp = Vec2::new(-chord_hat.y, chord_hat.x); // CCW perp
    let mid = (a + b) * 0.5;
    let centre_off = r * half.cos();
    let centre = mid + perp * (if bulge > 0.0 { centre_off } else { -centre_off });
    let start_ang = (a - centre).angle();
    let end_ang = (b - centre).angle();
    let sweep = if bulge > 0.0 {
        (end_ang - start_ang).rem_euclid(std::f64::consts::TAU)
    } else {
        -((start_ang - end_ang).rem_euclid(std::f64::consts::TAU))
    };
    let arc_len_px = (r as f32 * app.scale) * sweep.abs() as f32;
    let n = (arc_len_px * 0.4).clamp(6.0, 256.0) as usize;
    // Skip i=0 (== a, already in `out`); end with i=n (== b).
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let ang = start_ang + sweep * t;
        let p = centre + Vec2::new(r * ang.cos(), r * ang.sin());
        out.push(app.w2s(p, rect));
    }
}

/// Append the world-space tessellation of ONE polyline segment from
/// `a` to `b` with the given bulge to `out`. `a` is assumed already
/// present at the end of `out`; this function adds the intermediate
/// samples + `b`. Straight segment (bulge ≈ 0) appends just `b`.
/// Sample density is fixed at 24 per arc segment — enough for smooth
/// hatch boundaries at any zoom without paying the per-zoom
/// retessellation cost of the screen-space variant.
pub(super) fn append_arc_world_samples(a: Vec2, b: Vec2, bulge: f64, out: &mut Vec<Vec2>) {
    if bulge.abs() < 1e-9 {
        out.push(b);
        return;
    }
    let chord = b - a;
    let chord_len = chord.len();
    if chord_len < EPS {
        out.push(b);
        return;
    }
    let theta = 4.0 * bulge.atan();
    let half = theta * 0.5;
    let sin_half = half.sin();
    if sin_half.abs() < EPS {
        out.push(b);
        return;
    }
    let r = chord_len / (2.0 * sin_half.abs());
    let chord_hat = chord / chord_len;
    let perp = Vec2::new(-chord_hat.y, chord_hat.x);
    let mid = (a + b) * 0.5;
    let centre_off = r * half.cos();
    let centre = mid + perp * (if bulge > 0.0 { centre_off } else { -centre_off });
    let start_ang = (a - centre).angle();
    let end_ang = (b - centre).angle();
    let sweep = if bulge > 0.0 {
        (end_ang - start_ang).rem_euclid(std::f64::consts::TAU)
    } else {
        -((start_ang - end_ang).rem_euclid(std::f64::consts::TAU))
    };
    let n: usize = 24;
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let ang = start_ang + sweep * t;
        out.push(centre + Vec2::new(r * ang.cos(), r * ang.sin()));
    }
}

/// Even-odd ray-cast point-in-polygon test. Returns true iff `p` lies
/// in the interior of the closed polygon traced by the iterator. Half-
/// open Y-test avoids double-counting horizontal edge endpoints — the
/// standard correct PIP. Used today by the hatch pick-point boundary
/// finder; would also fit hit-testing closed splines / regions when
/// those land.
/// Paint a hatch pattern preview into `bound` (a screen-space rect).
/// Used by the attributes dialog's main preview panel AND every
/// thumbnail in the pattern picker grid. `px_per_unit` controls how
/// many screen pixels one pattern-unit covers (the catalog's natural
/// spacings of a few mm) — main preview ≈ 10 px/unit, thumbnails ≈ 4-6.
pub(super) fn paint_pattern_preview(
    p: &egui::Painter,
    bound: egui::Rect,
    name: &str,
    user_scale: f64,
    user_angle: f64, // radians
    accent: egui::Color32,
    px_per_unit: f32,
    stroke_w: f32,
) {
    let pat = cad_kernel::patterns::lookup(name);
    let centre = bound.center();
    match &pat {
        cad_kernel::patterns::Pattern::Families(families) => {
            for fam in families {
                let theta = fam.angle + user_angle;
                let spacing_px = (fam.spacing * user_scale) as f32 * px_per_unit;
                if spacing_px < 1.0 {
                    continue;
                }
                let cos = theta.cos() as f32;
                let sin = theta.sin() as f32;
                let diag = (bound.width() * bound.width() + bound.height() * bound.height()).sqrt();
                let n_lines = (diag / spacing_px).ceil() as i32;
                for k in -n_lines..=n_lines {
                    let s = (k as f32) * spacing_px;
                    let nx = -sin;
                    let ny = cos;
                    let mid = egui::pos2(centre.x + nx * s, centre.y + ny * s);
                    let a = egui::pos2(mid.x - cos * diag, mid.y - sin * diag);
                    let b = egui::pos2(mid.x + cos * diag, mid.y + sin * diag);
                    if let Some((p1, p2)) = clip_line_to_rect(a, b, bound) {
                        p.line_segment([p1, p2], egui::Stroke::new(stroke_w, accent));
                    }
                }
            }
        }
        cad_kernel::patterns::Pattern::Tile {
            period_x,
            period_y,
            segments,
            circles,
        } => {
            let px = (*period_x * user_scale) as f32 * px_per_unit;
            let py = (*period_y * user_scale) as f32 * px_per_unit;
            if px < 1.0 || py < 1.0 {
                return;
            }
            let cos = user_angle.cos() as f32;
            let sin = user_angle.sin() as f32;
            let diag = (bound.width() * bound.width() + bound.height() * bound.height()).sqrt();
            let nx_cells = (diag / px).ceil() as i32 + 1;
            let ny_cells = (diag / py).ceil() as i32 + 1;
            for j in -ny_cells..=ny_cells {
                for i in -nx_cells..=nx_cells {
                    let ox = (i as f32) * px;
                    let oy = (j as f32) * py;
                    for seg in segments {
                        let ax_p = ox + (seg.x1 * user_scale) as f32 * px_per_unit;
                        let ay_p = oy + (seg.y1 * user_scale) as f32 * px_per_unit;
                        let bx_p = ox + (seg.x2 * user_scale) as f32 * px_per_unit;
                        let by_p = oy + (seg.y2 * user_scale) as f32 * px_per_unit;
                        let ax = centre.x + ax_p * cos - ay_p * sin;
                        let ay = centre.y + ax_p * sin + ay_p * cos;
                        let bx = centre.x + bx_p * cos - by_p * sin;
                        let by = centre.y + bx_p * sin + by_p * cos;
                        if let Some((p1, p2)) =
                            clip_line_to_rect(egui::pos2(ax, ay), egui::pos2(bx, by), bound)
                        {
                            p.line_segment([p1, p2], egui::Stroke::new(stroke_w, accent));
                        }
                    }
                    // Circles in the tile cell (e.g. CONCENTRIC).
                    // Painted only when their bbox overlaps `bound` —
                    // simple culling keeps the preview snappy.
                    for c in circles {
                        let r_px = (c.radius * user_scale) as f32 * px_per_unit;
                        if r_px < 0.5 {
                            continue;
                        }
                        let cx_p = ox + (c.cx * user_scale) as f32 * px_per_unit;
                        let cy_p = oy + (c.cy * user_scale) as f32 * px_per_unit;
                        let cx = centre.x + cx_p * cos - cy_p * sin;
                        let cy = centre.y + cx_p * sin + cy_p * cos;
                        let pos = egui::pos2(cx, cy);
                        // Cull if entirely outside bound expanded by r.
                        if pos.x + r_px < bound.left()
                            || pos.x - r_px > bound.right()
                            || pos.y + r_px < bound.top()
                            || pos.y - r_px > bound.bottom()
                        {
                            continue;
                        }
                        p.circle_stroke(pos, r_px, egui::Stroke::new(stroke_w, accent));
                    }
                }
            }
        }
    }
}

/// Polar angle dial — a circular widget for picking an angle in
/// degrees. Click or drag anywhere inside to set the angle to the
/// vector from centre to cursor (CCW from +X, screen-Y-inverted so 0°
/// points RIGHT and 90° points UP).
///
/// Visual: outer ring + 12 tick marks (cardinals heavier) + cardinal
/// labels + amber indicator line + tip dot. Numeric readout below.
///
/// `size` = pixel diameter (the widget allocates a square `size × (size
/// + 32)` — extra 32 px for the readout).
pub(super) fn polar_angle_picker(
    ui: &mut egui::Ui,
    angle_deg: &mut f64,
    size: f32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size, size + 32.0), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let dial_rect = egui::Rect::from_min_size(rect.left_top(), egui::vec2(size, size));
    let centre = dial_rect.center();
    let r = (size * 0.5) - 14.0;
    // Backdrop + outer ring.
    painter.circle_filled(centre, r, egui::Color32::from_rgb(18, 22, 28));
    painter.circle_stroke(
        centre,
        r,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 130, 145)),
    );
    // 12 tick marks (every 30°), cardinals heavier.
    for k in 0..12 {
        let t = (k as f32) * std::f32::consts::TAU / 12.0;
        let is_cardinal = k % 3 == 0;
        let inner = r - if is_cardinal { 9.0 } else { 5.0 };
        let p1 = centre + egui::vec2(inner * t.cos(), -inner * t.sin());
        let p2 = centre + egui::vec2(r * t.cos(), -r * t.sin());
        let col = if is_cardinal {
            egui::Color32::from_rgb(190, 200, 215)
        } else {
            egui::Color32::from_rgb(95, 105, 120)
        };
        let w = if is_cardinal { 1.4 } else { 0.9 };
        painter.line_segment([p1, p2], egui::Stroke::new(w, col));
    }
    // Cardinal labels (0/90/180/270).
    let lbl_r = r + 11.0;
    let labels = [
        (0.0_f32, "0°"),
        (90.0, "90°"),
        (180.0, "180°"),
        (270.0, "270°"),
    ];
    for (deg, txt) in labels {
        let t = deg.to_radians();
        let pos = centre + egui::vec2(lbl_r * t.cos(), -lbl_r * t.sin());
        painter.text(
            pos,
            egui::Align2::CENTER_CENTER,
            txt,
            egui::FontId::monospace(9.0),
            egui::Color32::from_rgb(170, 185, 210),
        );
    }
    // Indicator — amber line + dot at current angle.
    let theta = (*angle_deg as f32).to_radians();
    let tip = centre + egui::vec2(r * theta.cos(), -r * theta.sin());
    painter.line_segment(
        [centre, tip],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 180, 80)),
    );
    painter.circle_filled(centre, 3.0, egui::Color32::from_rgb(255, 180, 80));
    painter.circle_filled(tip, 5.5, egui::Color32::from_rgb(255, 180, 80));
    painter.circle_stroke(
        tip,
        5.5,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(20, 20, 20)),
    );
    // Capture click / drag — set angle from cursor vector.
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let d = pos - centre;
            // Screen Y grows DOWN; flip to math Y for CCW-from-+X angle.
            let mut deg = (-d.y).atan2(d.x).to_degrees() as f64;
            if deg < 0.0 {
                deg += 360.0;
            }
            *angle_deg = deg;
        }
    }
    // Numeric readout under the dial.
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 14.0),
        egui::Align2::CENTER_CENTER,
        format!("{:.1}°", *angle_deg),
        crate::theme::typ::data_value(),
        egui::Color32::from_rgb(220, 230, 240),
    );
    response
}

/// Containment depth of every loop in `loops`: how many OTHER loops contain
/// that loop's first vertex (bbox-pre-filtered — only loops whose bbox
/// encloses the probe run the point-in-polygon test). Single source of
/// truth for SOLID hatch even-odd: even depth → fill, odd depth → hole.
/// Valid hatch boundaries don't cross, so one probe vertex per loop is
/// enough (a loop cannot straddle another loop's boundary). Degenerate
/// (empty) loops report depth 0.
pub(super) fn loop_depths(loops: &[Vec<Vec2>]) -> Vec<usize> {
    let bboxes: Vec<(Vec2, Vec2)> = loops
        .iter()
        .map(|l| {
            let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
            let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            for v in l {
                mn = Vec2::new(mn.x.min(v.x), mn.y.min(v.y));
                mx = Vec2::new(mx.x.max(v.x), mx.y.max(v.y));
            }
            (mn, mx)
        })
        .collect();
    (0..loops.len())
        .map(|li| {
            let Some(probe) = loops[li].first().copied() else {
                return 0;
            };
            loops
                .iter()
                .enumerate()
                .filter(|&(j, l)| {
                    if j == li || l.len() < 3 {
                        return false;
                    }
                    let (mn, mx) = bboxes[j];
                    if probe.x < mn.x || probe.x > mx.x || probe.y < mn.y || probe.y > mx.y {
                        return false;
                    }
                    point_in_polygon(probe, l.iter().copied())
                })
                .count()
        })
        .collect()
}

/// Even-odd classification for SOLID hatch fills: loop `i` is a FILL iff an
/// EVEN number of OTHER loops contain it (a sample vertex of `i` is inside
/// them). Outer loop → 0 containers → fill; first island → 1 → hole;
/// island-in-hole → 2 → fill. Two DISJOINT regions are both fills — the
/// old index-parity rule (`i % 2`) wrongly made every second disjoint
/// region a hole, drawn in background colour and effectively invisible.
pub(super) fn solid_loop_is_fill(loops: &[Vec<Vec2>], i: usize) -> bool {
    loop_depths(loops).get(i).map_or(false, |d| d % 2 == 0)
}

pub(super) fn point_in_polygon<I: IntoIterator<Item = Vec2>>(p: Vec2, verts: I) -> bool {
    let vs: Vec<Vec2> = verts.into_iter().collect();
    let n = vs.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let pi = vs[i];
        let pj = vs[j];
        if (pi.y > p.y) != (pj.y > p.y) {
            let x_int = pi.x + (p.y - pi.y) * (pj.x - pi.x) / (pj.y - pi.y);
            if p.x < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Clip a screen-space line segment to an axis-aligned rectangle —
/// parametric Liang-Barsky-lite. Returns the visible portion or None
/// if the segment misses the rect entirely. Used only by the hatch
/// preview's pattern renderer; the canvas hatch render uses the
/// polygon-clip path against the real boundary loops.
pub(super) fn clip_line_to_rect(
    a: egui::Pos2,
    b: egui::Pos2,
    r: egui::Rect,
) -> Option<(egui::Pos2, egui::Pos2)> {
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let clip = |p: f32, q: f32, t0: &mut f32, t1: &mut f32| -> bool {
        if p.abs() < 1e-9 {
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > *t1 {
                return false;
            }
            if r > *t0 {
                *t0 = r;
            }
        } else {
            if r < *t0 {
                return false;
            }
            if r < *t1 {
                *t1 = r;
            }
        }
        true
    };
    if !clip(-dx, a.x - r.left(), &mut t0, &mut t1) {
        return None;
    }
    if !clip(dx, r.right() - a.x, &mut t0, &mut t1) {
        return None;
    }
    if !clip(-dy, a.y - r.top(), &mut t0, &mut t1) {
        return None;
    }
    if !clip(dy, r.bottom() - a.y, &mut t0, &mut t1) {
        return None;
    }
    if t1 < t0 {
        return None;
    }
    Some((
        egui::pos2(a.x + dx * t0, a.y + dy * t0),
        egui::pos2(a.x + dx * t1, a.y + dy * t1),
    ))
}

/// Tessellate a circle into an N-vertex closed loop (world coords).
/// Sample density is fixed — fill tessellation doesn't need to track
/// zoom. 64 is smooth at typical printable sizes; bump to 128 if a
/// large-radius hatch looks faceted.
pub(super) fn tessellate_circle_loop(centre: Vec2, radius: f64, n: usize) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f64) / (n as f64) * std::f64::consts::TAU;
        out.push(Vec2::new(
            centre.x + radius * t.cos(),
            centre.y + radius * t.sin(),
        ));
    }
    out
}

/// Tolerance for treating a polyline's first/last vertices as
/// coincident — i.e. the polyline is geometrically a loop even if its
/// `closed` flag is still false. The debug log surfaces this case with
/// "endpoint gap = … ← visually closed but `closed=false`".
pub(super) const POLYLINE_EFFECTIVELY_CLOSED_EPS: f64 = 1e-3;

/// True if `p` should be treated as a closed loop for hatch purposes:
/// either its `closed` flag is set, OR its first and last vertices
/// coincide within `POLYLINE_EFFECTIVELY_CLOSED_EPS`. Polylines with
/// fewer than 3 vertices can't form a loop.
pub(super) fn polyline_is_effectively_closed(p: &Polyline) -> bool {
    if p.vertices.len() < 3 {
        return false;
    }
    if p.closed {
        return true;
    }
    let first = p.vertices.first().map(|v| v.pos);
    let last = p.vertices.last().map(|v| v.pos);
    match (first, last) {
        (Some(a), Some(b)) => (a - b).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS,
        _ => false,
    }
}

/// True if a spline should be treated as a closed loop for hatch purposes:
/// ≥3 control points and its first and last control points coincide within
/// `POLYLINE_EFFECTIVELY_CLOSED_EPS`. (The CAD "Close" appends the first control
/// point so the curve returns to its start — so closed = first ≈ last cp.)
pub(super) fn spline_is_effectively_closed(s: &Spline) -> bool {
    let n = s.control_points.len();
    if n < 3 {
        return false;
    }
    (s.control_points[0] - s.control_points[n - 1]).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS
}

/// Copy a set of dobjects with NEW handles (hatch boundary handles remapped),
/// applying `xform` to each geometry. Used by the script `copy` op — the
/// copies must be independent of their sources, which cloned handles would
/// not be (two dobjects sharing one handle corrupt the spatial index).
pub(super) fn duplicate_dobjects(
    sources: &[DObject],
    xform: impl Fn(&Geom) -> Geom,
) -> Vec<DObject> {
    let mut hmap: HashMap<cad_kernel::Handle, cad_kernel::Handle> =
        HashMap::with_capacity(sources.len());
    let mut out: Vec<DObject> = Vec::with_capacity(sources.len());
    for s in sources {
        let nh = cad_kernel::next_handle();
        hmap.insert(s.handle, nh);
        out.push(DObject {
            geom: xform(&s.geom),
            style: s.style,
            handle: nh,
        });
    }
    for d in &mut out {
        if let Geom::Hatch(h) = &mut d.geom {
            for bh in &mut h.boundary_handles {
                if let Some(&nh) = hmap.get(bh) {
                    *bh = nh;
                }
            }
        }
    }
    out
}

/// RESOLVED style of one dobject against a given document — the script-facing
/// summary: color `"aci N"`/`"bylayer"`/`"byblock"`/`"#RRGGBB"`, linetype NAME
/// (ByLayer/Continuous → the layer's), lineweight mm, visible.
pub(super) fn doc_entity_style_summary(doc: &Document, d: &DObject) -> (String, String, f32, bool) {
    let layer = doc.layers.get(d.style.layer);
    let color = match d.style.color {
        Color::ByLayer => layer
            .map(|l| CadApp::script_color_string(l.color, doc))
            .unwrap_or_else(|| "bylayer".into()),
        Color::ByBlock => "byblock".into(),
        Color::Aci(n) => format!("aci {}", n),
        Color::TrueColorRef(idx) => match d.style.color.rgb_bytes(&doc.truecolors) {
            Some((r, g, b)) => format!("#{:02X}{:02X}{:02X}", r, g, b),
            None => format!("truecolor {}", idx),
        },
    };
    let linetype = {
        use cad_kernel::LinetypeTable;
        let id = match d.style.linetype {
            LinetypeTable::BYLAYER | LinetypeTable::CONTINUOUS => layer
                .map(|l| l.linetype)
                .unwrap_or(LinetypeTable::CONTINUOUS),
            other => other,
        };
        doc.linetypes
            .get(id)
            .map(|lt| lt.name.clone())
            .unwrap_or_else(|| format!("#{}", id))
    };
    let lineweight =
        cad_kernel::lineweight::resolve_lineweight(d.style.lineweight, d.style.layer, &doc.layers);
    (color, linetype, lineweight, d.style.visible)
}

pub(super) fn preview_transform(
    doc: &mut Document,
    indices: &[usize],
    f: impl Fn(&Geom) -> Geom,
) -> usize {
    let mut targets: Vec<usize> = Vec::new();
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &i in indices {
        if i < doc.dobjects.len() && seen.insert(i) {
            targets.push(i);
        }
    }
    let mut extra: Vec<cad_kernel::Handle> = Vec::new();
    for &i in &targets {
        if let Some(d) = doc.dobjects.get(i) {
            if let Geom::Hatch(h) = &d.geom {
                extra.extend(h.boundary_handles.iter().copied());
            }
        }
    }
    for handle in extra {
        if let Some(bi) = doc.index_of_handle(handle) {
            if seen.insert(bi) {
                targets.push(bi);
            }
        }
    }
    for &i in &targets {
        if let Some(d) = doc.dobjects.get_mut(i) {
            d.geom = f(&d.geom);
        }
    }
    targets.len()
}

/// P1 preview helper — per-entity style change against the shadow document.
pub(super) fn preview_style(
    doc: &mut Document,
    indices: &[usize],
    mut f: impl FnMut(&mut DObject),
) -> usize {
    let mut n = 0;
    for &i in indices {
        if let Some(d) = doc.dobjects.get_mut(i) {
            f(d);
            n += 1;
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Plot-style table helpers (ported from upstream RUST-AutoRASM) — shared by
// the Plot Style Table Editor's Form View, Table View, and the Plot dialog.
// ─────────────────────────────────────────────────────────────────────────────

fn plotstyle_common<T: PartialEq + Copy>(
    sel: &[u8],
    table: &cad_kernel::plotstyle::PlotStyleTable,
    f: impl Fn(&cad_kernel::plotstyle::PlotStyle) -> T,
) -> Option<T> {
    let mut it = sel.iter().map(|&a| f(table.style(a)));
    let first = it.next()?;
    if it.all(|v| v == first) {
        Some(first)
    } else {
        None
    }
}

/// Pending edits collected while rendering the property panel; applied to every
/// selected style after the frame (avoids borrow conflicts with the read side).
#[derive(Default)]
pub(super) struct PlotStyleEdit {
    plot_color: Option<cad_kernel::plotstyle::PlotColor>,
    dither: Option<bool>,
    grayscale: Option<bool>,
    pen: Option<cad_kernel::plotstyle::PenNum>,
    vpen: Option<cad_kernel::plotstyle::PenNum>,
    screening: Option<u8>,
    linetype: Option<cad_kernel::plotstyle::PlotLinetype>,
    adaptive: Option<bool>,
    lineweight: Option<cad_kernel::plotstyle::PlotWidth>,
    end_style: Option<cad_kernel::plotstyle::EndStyle>,
    join_style: Option<cad_kernel::plotstyle::JoinStyle>,
    fill_style: Option<cad_kernel::plotstyle::FillStyle>,
}

/// Label + optional swatch rgb for a plot color.
pub(super) fn lbl_plot_color(
    c: cad_kernel::plotstyle::PlotColor,
) -> (String, Option<(u8, u8, u8)>) {
    use cad_kernel::plotstyle::PlotColor as PC;
    match c {
        PC::UseObject => ("Use object color".into(), None),
        PC::Black => ("Black".into(), Some((0, 0, 0))),
        PC::Aci(i) => (format!("Color {}", i), Some(aci_palette(i))),
        PC::Rgb(r, g, b) => (format!("RGB {},{},{}", r, g, b), Some((r, g, b))),
    }
}

pub(super) fn lbl_plot_width(w: cad_kernel::plotstyle::PlotWidth) -> String {
    use cad_kernel::plotstyle::PlotWidth as PW;
    match w {
        PW::UseObject => "Use object".into(),
        PW::Fixed(mm) => format!("{:.2} mm", mm),
    }
}

pub(super) fn lbl_pen(p: cad_kernel::plotstyle::PenNum) -> String {
    use cad_kernel::plotstyle::PenNum as PN;
    match p {
        PN::Automatic => "Automatic".into(),
        PN::N(n) => format!("{}", n),
    }
}

/// Paper-size label for the Plot dialog dropdown.
pub(super) fn paper_label(p: cad_kernel::plotstyle::PaperSize) -> &'static str {
    use cad_kernel::plotstyle::PaperSize as P;
    match p {
        P::A4 => "A4",
        P::A3 => "A3",
        P::A2 => "A2",
        P::A1 => "A1",
        P::A0 => "A0",
        P::Letter => "Letter",
        P::Custom { .. } => "Custom",
    }
}

pub(super) fn lbl_end(e: cad_kernel::plotstyle::EndStyle) -> &'static str {
    use cad_kernel::plotstyle::EndStyle as E;
    match e {
        E::UseObject => "Use object",
        E::Butt => "Butt",
        E::Square => "Square",
        E::Round => "Round",
        E::Diamond => "Diamond",
    }
}

pub(super) fn lbl_join(j: cad_kernel::plotstyle::JoinStyle) -> &'static str {
    use cad_kernel::plotstyle::JoinStyle as J;
    match j {
        J::UseObject => "Use object",
        J::Miter => "Miter",
        J::Bevel => "Bevel",
        J::Round => "Round",
        J::Diamond => "Diamond",
    }
}

pub(super) fn lbl_fill(f: cad_kernel::plotstyle::FillStyle) -> &'static str {
    use cad_kernel::plotstyle::FillStyle as F;
    match f {
        F::UseObject => "Use object",
        F::Solid => "Solid",
        F::Checkerboard => "Checkerboard",
        F::Crosshatch => "Crosshatch",
        F::Diamonds => "Diamonds",
        F::HorizontalBars => "Horizontal bars",
        F::SlantLeft => "Slant left",
        F::SlantRight => "Slant right",
        F::SquareDots => "Square dots",
        F::VerticalBars => "Vertical bars",
    }
}

/// An On/Off dropdown that shows "*Varies*" for a mixed selection and writes the
/// picked bool to `out` (applied to all selected styles after the frame).
pub(super) fn on_off_combo(
    ui: &mut egui::Ui,
    id: &str,
    common: Option<bool>,
    out: &mut Option<bool>,
) {
    let txt = match common {
        Some(true) => "On",
        Some(false) => "Off",
        None => "*Varies*",
    };
    egui::ComboBox::from_id_salt(id)
        .width(150.0)
        .selected_text(txt)
        .show_ui(ui, |ui| {
            if ui.selectable_label(false, "On").clicked() {
                *out = Some(true);
            }
            if ui.selectable_label(false, "Off").clicked() {
                *out = Some(false);
            }
        });
}

/// A pen-number field: a DragValue where 0 shows as "Auto" (PenNum::Automatic),
/// 1..=max as the pen number. Writes the picked value to `out` on change.
pub(super) fn pen_field(
    ui: &mut egui::Ui,
    _id: &str,
    common: Option<cad_kernel::plotstyle::PenNum>,
    max: u16,
    out: &mut Option<cad_kernel::plotstyle::PenNum>,
) {
    use cad_kernel::plotstyle::PenNum;
    let mut v: i32 = match common {
        Some(PenNum::N(n)) => n as i32,
        _ => 0,
    };
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .range(0..=max as i32)
            .update_while_editing(false)
            .custom_formatter(|n, _| {
                if n <= 0.0 {
                    "Auto".to_string()
                } else {
                    format!("{}", n as i32)
                }
            }),
    );
    if r.changed() {
        *out = Some(if v <= 0 {
            PenNum::Automatic
        } else {
            PenNum::N(v as u16)
        });
    }
    if common.is_none() {
        r.on_hover_text("*Varies* — set to override all selected");
    }
}

/// The 12 CTB properties in Form-View order — the single enum both the Form View
/// and Table View iterate, so the two can never drift.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PlotProp {
    Color,
    Dither,
    Grayscale,
    Pen,
    VPen,
    Screening,
    Linetype,
    Adaptive,
    Lineweight,
    EndStyle,
    JoinStyle,
    FillStyle,
}

impl PlotProp {
    pub(super) const ALL: [PlotProp; 12] = [
        PlotProp::Color,
        PlotProp::Dither,
        PlotProp::Grayscale,
        PlotProp::Pen,
        PlotProp::VPen,
        PlotProp::Screening,
        PlotProp::Linetype,
        PlotProp::Adaptive,
        PlotProp::Lineweight,
        PlotProp::EndStyle,
        PlotProp::JoinStyle,
        PlotProp::FillStyle,
    ];
    pub(super) fn label(self) -> &'static str {
        match self {
            PlotProp::Color => "Color",
            PlotProp::Dither => "Dither",
            PlotProp::Grayscale => "Grayscale",
            PlotProp::Pen => "Pen #",
            PlotProp::VPen => "Virtual pen #",
            PlotProp::Screening => "Screening",
            PlotProp::Linetype => "Linetype",
            PlotProp::Adaptive => "Adaptive",
            PlotProp::Lineweight => "Lineweight",
            PlotProp::EndStyle => "Line end style",
            PlotProp::JoinStyle => "Line join style",
            PlotProp::FillStyle => "Fill style",
        }
    }
    /// §1a store-only (round-trips + editable, but no plot effect yet).
    pub(super) fn store_only(self) -> bool {
        matches!(
            self,
            PlotProp::Dither
                | PlotProp::Pen
                | PlotProp::VPen
                | PlotProp::Adaptive
                | PlotProp::FillStyle
        )
    }
}

/// The COMPACT display value of a property for one style — a short label (fits a
/// narrow grid cell) + optional swatch rgb. Used by every Table View cell.
pub(super) fn plotprop_cell(
    prop: PlotProp,
    s: &cad_kernel::plotstyle::PlotStyle,
    lt_list: &[(u32, String)],
) -> (String, Option<(u8, u8, u8)>) {
    use cad_kernel::plotstyle::{
        EndStyle as E, FillStyle as F, JoinStyle as J, PenNum, PlotColor as PC, PlotLinetype,
        PlotWidth,
    };
    let onoff = |b: bool| {
        if b {
            "On".to_string()
        } else {
            "Off".to_string()
        }
    };
    let pen = |p: PenNum| match p {
        PenNum::Automatic => "Auto".to_string(),
        PenNum::N(n) => format!("{n}"),
    };
    match prop {
        PlotProp::Color => match s.plot_color {
            PC::UseObject => ("Obj".to_string(), None),
            PC::Black => ("Blk".to_string(), Some((0, 0, 0))),
            PC::Aci(i) => (format!("C{i}"), Some(aci_palette(i))),
            PC::Rgb(r, g, b) => ("RGB".to_string(), Some((r, g, b))),
        },
        PlotProp::Dither => (onoff(s.dither), None),
        PlotProp::Grayscale => (onoff(s.grayscale), None),
        PlotProp::Pen => (pen(s.pen_number), None),
        PlotProp::VPen => (pen(s.virtual_pen), None),
        PlotProp::Screening => (format!("{}%", s.screening), None),
        PlotProp::Linetype => (
            match s.linetype {
                PlotLinetype::UseObject => "Obj".to_string(),
                PlotLinetype::Id(id) => lt_list
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, n)| n.chars().take(6).collect::<String>())
                    .unwrap_or_else(|| format!("LT{id}")),
            },
            None,
        ),
        PlotProp::Adaptive => (onoff(s.adaptive), None),
        PlotProp::Lineweight => (
            match s.lineweight {
                PlotWidth::UseObject => "Obj".to_string(),
                PlotWidth::Fixed(mm) => format!("{:.2}", mm),
            },
            None,
        ),
        PlotProp::EndStyle => (
            match s.end_style {
                E::UseObject => "Obj",
                E::Butt => "Butt",
                E::Square => "Sqr",
                E::Round => "Rnd",
                E::Diamond => "Dmd",
            }
            .to_string(),
            None,
        ),
        PlotProp::JoinStyle => (
            match s.join_style {
                J::UseObject => "Obj",
                J::Miter => "Mitr",
                J::Bevel => "Bevl",
                J::Round => "Rnd",
                J::Diamond => "Dmd",
            }
            .to_string(),
            None,
        ),
        PlotProp::FillStyle => (
            match s.fill_style {
                F::UseObject => "Obj",
                F::Solid => "Sld",
                F::Checkerboard => "Chk",
                F::Crosshatch => "Xhch",
                F::Diamonds => "Dmd",
                F::HorizontalBars => "HBar",
                F::SlantLeft => "SlL",
                F::SlantRight => "SlR",
                F::SquareDots => "SDot",
                F::VerticalBars => "VBar",
            }
            .to_string(),
            None,
        ),
    }
}

/// The INTERACTIVE editor control for one property — the single control shared
/// by the Form View property panel AND the Table View row editor. Computes the
/// common value across `sel` ("*Varies*" if mixed) and records the picked value
/// into `edit` (applied to every selected color after the frame). `salt` keeps
/// egui ids unique between the two views.
#[allow(clippy::too_many_arguments)]
pub(super) fn plotprop_editor(
    ui: &mut egui::Ui,
    prop: PlotProp,
    sel: &[u8],
    table: &cad_kernel::plotstyle::PlotStyleTable,
    ladder: &[f32],
    lt_list: &[(u32, String)],
    edit: &mut PlotStyleEdit,
    open_color_picker: &mut bool,
    salt: &str,
) {
    use crate::theme::color as tc;
    use cad_kernel::plotstyle::{
        EndStyle, FillStyle, JoinStyle, PlotColor, PlotLinetype, PlotWidth,
    };
    let varies = "*Varies*";
    match prop {
        PlotProp::Color => {
            let cc = plotstyle_common(sel, table, |s| s.plot_color);
            ui.horizontal(|ui| {
                // The swatch itself is clickable → opens the ACI colour picker.
                let (rect, sresp) =
                    ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
                if let Some(c) = cc {
                    if let (_, Some((r, g, b))) = lbl_plot_color(c) {
                        ui.painter()
                            .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
                    }
                }
                let sbrd = if sresp.hovered() {
                    tc::ACCENT
                } else {
                    tc::BORDER
                };
                ui.painter()
                    .rect_stroke(rect, 2.0, egui::Stroke::new(1.0, sbrd));
                if sresp.clicked() {
                    *open_color_picker = true;
                    // Remember the swatch rect so the colour chart opens at its right (§2).
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("plotstyle_color_anchor"), rect));
                }
                if sresp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let txt = cc
                    .map(|c| lbl_plot_color(c).0)
                    .unwrap_or_else(|| varies.into());
                ui.menu_button(txt, |ui| {
                    if ui.button("Use object color").clicked() {
                        edit.plot_color = Some(PlotColor::UseObject);
                        ui.close_menu();
                    }
                    if ui.button("Black").clicked() {
                        edit.plot_color = Some(PlotColor::Black);
                        ui.close_menu();
                    }
                    if ui.button("Select Color…").clicked() {
                        *open_color_picker = true;
                        ui.close_menu();
                    }
                });
            });
        }
        PlotProp::Dither => {
            let v = plotstyle_common(sel, table, |s| s.dither);
            on_off_combo(ui, &format!("{salt}_dith"), v, &mut edit.dither);
        }
        PlotProp::Grayscale => {
            let v = plotstyle_common(sel, table, |s| s.grayscale);
            on_off_combo(ui, &format!("{salt}_gray"), v, &mut edit.grayscale);
        }
        PlotProp::Pen => {
            let v = plotstyle_common(sel, table, |s| s.pen_number);
            pen_field(ui, &format!("{salt}_pen"), v, 32, &mut edit.pen);
        }
        PlotProp::VPen => {
            let v = plotstyle_common(sel, table, |s| s.virtual_pen);
            pen_field(ui, &format!("{salt}_vpen"), v, 255, &mut edit.vpen);
        }
        PlotProp::Screening => {
            let sv = plotstyle_common(sel, table, |s| s.screening);
            let mut sc = sv.unwrap_or(100) as i32;
            let r = ui.add(
                egui::DragValue::new(&mut sc)
                    .range(0..=100)
                    .suffix(" %")
                    .update_while_editing(false),
            );
            if r.changed() {
                edit.screening = Some(sc.clamp(0, 100) as u8);
            }
            if sv.is_none() {
                r.on_hover_text(varies);
            }
        }
        PlotProp::Linetype => {
            let lv = plotstyle_common(sel, table, |s| s.linetype);
            let txt = match lv {
                Some(PlotLinetype::UseObject) => "Use object linetype".to_string(),
                Some(PlotLinetype::Id(id)) => lt_list
                    .iter()
                    .find(|(i, _)| *i == id)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| format!("Linetype {id}")),
                None => varies.to_string(),
            };
            egui::ComboBox::from_id_salt(format!("{salt}_lt"))
                .width(150.0)
                .selected_text(txt)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(false, "Use object linetype").clicked() {
                        edit.linetype = Some(PlotLinetype::UseObject);
                    }
                    for (id, nm) in lt_list {
                        if ui.selectable_label(false, nm).clicked() {
                            edit.linetype = Some(PlotLinetype::Id(*id));
                        }
                    }
                });
        }
        PlotProp::Adaptive => {
            let v = plotstyle_common(sel, table, |s| s.adaptive);
            on_off_combo(ui, &format!("{salt}_adap"), v, &mut edit.adaptive);
        }
        PlotProp::Lineweight => {
            let wv = plotstyle_common(sel, table, |s| s.lineweight);
            let txt = wv.map(lbl_plot_width).unwrap_or_else(|| varies.into());
            egui::ComboBox::from_id_salt(format!("{salt}_lw"))
                .width(150.0)
                .selected_text(txt)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, "Use object lineweight")
                        .clicked()
                    {
                        edit.lineweight = Some(PlotWidth::UseObject);
                    }
                    for &mm in ladder {
                        if ui
                            .selectable_label(false, format!("{:.2} mm", mm))
                            .clicked()
                        {
                            edit.lineweight = Some(PlotWidth::Fixed(mm));
                        }
                    }
                });
        }
        PlotProp::EndStyle => {
            let ev = plotstyle_common(sel, table, |s| s.end_style);
            let txt = ev
                .map(|e| lbl_end(e).to_string())
                .unwrap_or_else(|| varies.into());
            egui::ComboBox::from_id_salt(format!("{salt}_end"))
                .width(150.0)
                .selected_text(txt)
                .show_ui(ui, |ui| {
                    for e in [
                        EndStyle::UseObject,
                        EndStyle::Butt,
                        EndStyle::Square,
                        EndStyle::Round,
                        EndStyle::Diamond,
                    ] {
                        if ui.selectable_label(false, lbl_end(e)).clicked() {
                            edit.end_style = Some(e);
                        }
                    }
                });
        }
        PlotProp::JoinStyle => {
            let jv = plotstyle_common(sel, table, |s| s.join_style);
            let txt = jv
                .map(|j| lbl_join(j).to_string())
                .unwrap_or_else(|| varies.into());
            egui::ComboBox::from_id_salt(format!("{salt}_join"))
                .width(150.0)
                .selected_text(txt)
                .show_ui(ui, |ui| {
                    for j in [
                        JoinStyle::UseObject,
                        JoinStyle::Miter,
                        JoinStyle::Bevel,
                        JoinStyle::Round,
                        JoinStyle::Diamond,
                    ] {
                        if ui.selectable_label(false, lbl_join(j)).clicked() {
                            edit.join_style = Some(j);
                        }
                    }
                });
        }
        PlotProp::FillStyle => {
            let fv = plotstyle_common(sel, table, |s| s.fill_style);
            let txt = fv
                .map(|f| lbl_fill(f).to_string())
                .unwrap_or_else(|| varies.into());
            egui::ComboBox::from_id_salt(format!("{salt}_fill"))
                .width(150.0)
                .selected_text(txt)
                .show_ui(ui, |ui| {
                    for f in [
                        FillStyle::UseObject,
                        FillStyle::Solid,
                        FillStyle::Checkerboard,
                        FillStyle::Crosshatch,
                        FillStyle::Diamonds,
                        FillStyle::HorizontalBars,
                        FillStyle::SlantLeft,
                        FillStyle::SlantRight,
                        FillStyle::SquareDots,
                        FillStyle::VerticalBars,
                    ] {
                        if ui.selectable_label(false, lbl_fill(f)).clicked() {
                            edit.fill_style = Some(f);
                        }
                    }
                });
        }
    }
}

/// Apply a collected `PlotStyleEdit` to every selected ACI style.
pub(super) fn apply_plot_edit(
    table: &mut cad_kernel::plotstyle::PlotStyleTable,
    sel: &[u8],
    edit: &PlotStyleEdit,
) {
    for &a in sel {
        let st = table.style_mut(a);
        if let Some(v) = edit.plot_color {
            st.plot_color = v;
        }
        if let Some(v) = edit.dither {
            st.dither = v;
        }
        if let Some(v) = edit.grayscale {
            st.grayscale = v;
        }
        if let Some(v) = edit.pen {
            st.pen_number = v;
        }
        if let Some(v) = edit.vpen {
            st.virtual_pen = v;
        }
        if let Some(v) = edit.screening {
            st.screening = v;
        }
        if let Some(v) = edit.linetype {
            st.linetype = v;
        }
        if let Some(v) = edit.adaptive {
            st.adaptive = v;
        }
        if let Some(v) = edit.lineweight {
            st.lineweight = v;
        }
        if let Some(v) = edit.end_style {
            st.end_style = v;
        }
        if let Some(v) = edit.join_style {
            st.join_style = v;
        }
        if let Some(v) = edit.fill_style {
            st.fill_style = v;
        }
    }
}

/// The app's CTB folder — `<exe_dir>/ctb`, created on demand. User-defined CTB
/// tables live here as `.pst` files (AutoCAD's Plot Styles folder analog).
pub(super) fn ctb_folder() -> std::path::PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("ctb")
}

/// All saved CTB tables in the app's CTB folder as (name, path), name-sorted.
/// The name is the file stem (the table's name — see `load_plot_table`).
pub(super) fn ctb_list() -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(ctb_folder()) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension()
                .map(|e| e.eq_ignore_ascii_case("pst"))
                .unwrap_or(false)
            {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push((stem.to_string(), p));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

/// Built-in CTB choices: (display name, canonical stored key). "None" stores
/// nothing (`None`); the others store their canonical key.
pub(super) const CTB_BUILTINS: [(&str, Option<&str>); 4] = [
    ("None", None),
    ("Monochrome", Some("monochrome")),
    ("Grayscale", Some("grayscale")),
    ("Full Color", Some("fullcolor")),
];

/// Lowercased names a saved CTB must NOT collide with — the built-in display
/// names + canonical keys. The pickers dedup against these case-insensitively,
/// so a colliding file could never be selected/assigned.
pub(super) const CTB_RESERVED: [&str; 5] =
    ["none", "monochrome", "grayscale", "full color", "fullcolor"];

/// The default table for a BUILT-IN CTB — the same semantics the hardcoded
/// `apply_ctb` transform implements when no saved file exists: monochrome →
/// every ACI plots black, grayscale → luminance, fullcolor → object colours.
/// None for names that are not editable built-ins.
pub(super) fn ctb_builtin_seed(name: &str) -> Option<cad_kernel::plotstyle::PlotStyleTable> {
    use cad_kernel::plotstyle::PlotColor;
    let mut t = cad_kernel::plotstyle::PlotStyleTable::named(name.to_string());
    match name {
        "monochrome" => {
            for a in 0..=255u8 {
                t.style_mut(a).plot_color = PlotColor::Black;
            }
            Some(t)
        }
        "grayscale" => {
            for a in 0..=255u8 {
                t.style_mut(a).grayscale = true;
            }
            Some(t)
        }
        "fullcolor" => Some(t),
        _ => None,
    }
}

/// Layer-status glyph ink colours — the owner palette shared by the rasterised
/// SVG glyphs (`blit_layer_glyph` tints them) and the custom-drawn ones.
pub(super) const LYR_MUTED: egui::Color32 = egui::Color32::from_rgb(0x93, 0xA1, 0xAC);
pub(super) const LYR_DANGER: egui::Color32 = egui::Color32::from_rgb(0xE5, 0x48, 0x4D);

/// Rasterize an SVG glyph (from `layer_glyphs.rs`) to a white texture at a
/// generous resolution; `blit_layer_glyph` tints it per state. `key` names the
/// texture (e.g. "on", "off", "close").
pub(super) fn raster_layer_glyph(
    ctx: &egui::Context,
    key: &str,
    svg: &str,
) -> Option<egui::TextureHandle> {
    use resvg::{tiny_skia, usvg};
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg.as_bytes(), &opt).ok()?;
    let size = tree.size();
    // Render so the longest side is ~48 px — sharp at the 13-16 px blit sizes.
    let target = 48.0;
    let scale = target / size.width().max(size.height());
    let pw = (size.width() * scale).ceil().max(1.0) as u32;
    let ph = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // Force WHITE so `blit_layer_glyph` can tint by multiplying (the SVG fills
    // are already white in layer_glyphs.rs, but be defensive about it).
    let mut data = pixmap.data().to_vec();
    for px in data.chunks_exact_mut(4) {
        if px[3] > 0 {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
        }
    }
    let color = egui::ColorImage::from_rgba_premultiplied([pw as usize, ph as usize], &data);
    Some(ctx.load_texture(
        format!("layer_glyph_{}", key),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

/// Blit a layer-status glyph into `r`, tinted `tint` (a `layer_glyph_tex`
/// entry). No-op when the texture is missing.
pub(super) fn blit_layer_glyph(
    p: &egui::Painter,
    r: egui::Rect,
    t: Option<(egui::TextureId, egui::Vec2)>,
    tint: egui::Color32,
) {
    if let Some((id, sz)) = t {
        let a = sz.x / sz.y.max(0.001);
        let (w, h) = if a >= 1.0 {
            (r.width(), r.width() / a)
        } else {
            (r.height() * a, r.height())
        };
        let fit = egui::Rect::from_center_size(r.center(), egui::vec2(w, h));
        p.image(
            id,
            fit,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }
}

/// Screen-space stroke for the plot preview: a polyline drawn as BUTT segments
/// (each segment its own line), so corner wedges can be painted separately by
/// `paint_caps_joins_screen`.
pub(super) fn paint_butt_polyline(
    painter: &egui::Painter,
    spts: &[egui::Pos2],
    width_px: f32,
    color: egui::Color32,
) {
    if spts.len() < 2 {
        return;
    }
    let stroke = egui::Stroke::new(width_px, color);
    for w in spts.windows(2) {
        painter.line_segment([w[0], w[1]], stroke);
    }
}

/// Screen-space cap/join emulation for a stroke drawn as butt segments.
/// - Caps (open strokes): Round/Diamond = filled discs; Square = endpoints
///   extended by half the width; Butt = nothing.
/// - Joins at real corners (`corners`): Round/Diamond = discs, Bevel = the
///   filled gap triangle between the two butt corners, Miter = the wedge to
///   the edge intersection (4× miter limit, falling back to bevel).
/// `corners=false` (curve tessellations) skips join wedges — their samples are
/// dense enough that the butt segments overlap seamlessly. Closed strokes may
/// or may not duplicate their first point at the end.
pub(super) fn paint_caps_joins_screen(
    painter: &egui::Painter,
    spts: &[egui::Pos2],
    closed: bool,
    width_px: f32,
    cap: cad_kernel::plotstyle::EndStyle,
    join: cad_kernel::plotstyle::JoinStyle,
    color: egui::Color32,
    corners: bool,
) {
    use cad_kernel::plotstyle::{EndStyle, JoinStyle};
    if spts.len() < 2 {
        return;
    }
    // Unique vertex count: closed strokes may duplicate the first point.
    let mut m = spts.len();
    if closed && m > 1 && (spts[m - 1] - spts[0]).length() < 0.5 {
        m -= 1;
    }
    let hw = (width_px * 0.5).max(0.75);
    // Caps — open strokes only.
    if !closed {
        match cap {
            EndStyle::Round | EndStyle::Diamond => {
                painter.circle_filled(spts[0], hw, color);
                painter.circle_filled(spts[m - 1], hw, color);
            }
            EndStyle::Square => {
                let stroke = egui::Stroke::new(width_px, color);
                let ext = |a: egui::Pos2, b: egui::Pos2| -> egui::Pos2 {
                    let v = b - a;
                    let l = v.length();
                    if l < 1e-3 {
                        return a;
                    }
                    a - v * (hw / l)
                };
                painter.line_segment([ext(spts[0], spts[1]), spts[0]], stroke);
                painter.line_segment([ext(spts[m - 1], spts[m - 2]), spts[m - 1]], stroke);
            }
            EndStyle::Butt | EndStyle::UseObject => {}
        }
    }
    if !corners || m < 3 {
        return;
    }
    // Join wedges at real corners — the geometry comes from the SHARED
    // `cad_kernel::math::join_wedge` (also used by the PNG rasterizer), so the
    // preview and the exported raster cannot drift apart.
    let mut wedge = |v: egui::Pos2, u1: egui::Vec2, u2: egui::Vec2| {
        let cross = u1.x * u2.y - u1.y * u2.x;
        if cross.abs() < 0.02 {
            return; // near-collinear — no visible wedge (cheap cull)
        }
        let kv = cad_kernel::math::Vec2::new(v.x as f64, v.y as f64);
        let kd1 = cad_kernel::math::Vec2::new(u1.x as f64, u1.y as f64);
        let kd2 = cad_kernel::math::Vec2::new(u2.x as f64, u2.y as f64);
        let Some(w) = cad_kernel::math::join_wedge(kv, kd1, kd2, hw as f64) else {
            return;
        };
        let a = egui::pos2(w.a.x as f32, w.a.y as f32);
        let b = egui::pos2(w.b.x as f32, w.b.y as f32);
        match join {
            JoinStyle::Round | JoinStyle::Diamond => {
                painter.circle_filled(v, hw, color);
            }
            JoinStyle::Bevel => {
                painter.add(egui::Shape::convex_polygon(
                    vec![v, a, b],
                    color,
                    egui::Stroke::NONE,
                ));
            }
            JoinStyle::Miter => {
                let poly = match w.apex {
                    Some(apex) => vec![v, a, egui::pos2(apex.x as f32, apex.y as f32), b],
                    None => vec![v, a, b],
                };
                painter.add(egui::Shape::convex_polygon(poly, color, egui::Stroke::NONE));
            }
            JoinStyle::UseObject => {}
        }
    };
    if closed {
        for i in 0..m {
            let prev = (i + m - 1) % m;
            let next = (i + 1) % m;
            let d1 = spts[i] - spts[prev];
            let d2 = spts[next] - spts[i];
            if d1.length() < 1e-3 || d2.length() < 1e-3 {
                continue;
            }
            wedge(spts[i], d1 / d1.length(), d2 / d2.length());
        }
    } else {
        for i in 1..m - 1 {
            let d1 = spts[i] - spts[i - 1];
            let d2 = spts[i + 1] - spts[i];
            if d1.length() < 1e-3 || d2.length() < 1e-3 {
                continue;
            }
            wedge(spts[i], d1 / d1.length(), d2 / d2.length());
        }
    }
}

/// Tessellate any single closed geometry to a vertex loop in world
/// coords. Mirrors what `App::resolve_hatch_loops` does for one
/// boundary, but on raw geometry rather than via handle resolution —
/// used by the cheap-path island detector to PIP-test other dobjects'
/// boundaries against a chosen outer.
///
/// Open polylines whose endpoints meet within
/// `POLYLINE_EFFECTIVELY_CLOSED_EPS` are also accepted — this is the
/// "drew it as a closed loop but forgot to type `c` Enter" case the
/// user's polyline #6 surfaced. The duplicated last vertex (if any)
/// is dropped so the polygon doesn't double-count its starting edge.
///
/// Returns an empty Vec for everything else (Line / Arc / open
/// Polyline with separated endpoints / Spline / Point / Hatch).
pub(super) fn closed_dobject_polygon(g: &Geom) -> Vec<Vec2> {
    match g {
        Geom::Polyline(p) if polyline_is_effectively_closed(p) => {
            let n = p.vertices.len();
            // Treat the polyline as if `closed=true`. If the last
            // vertex duplicates the first (the v06=(x,y) ≡ v00 case
            // from polyline #6 in the user log), drop it so the
            // implied closing edge isn't drawn twice.
            let effective_n = if !p.closed {
                let f = p.vertices[0].pos;
                let l = p.vertices[n - 1].pos;
                if (f - l).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS {
                    n - 1
                } else {
                    n
                }
            } else {
                n
            };
            let mut v: Vec<Vec2> = Vec::with_capacity(effective_n * 4);
            v.push(p.vertices[0].pos);
            for k in 0..effective_n {
                let a = p.vertices[k].pos;
                let b = p.vertices[(k + 1) % effective_n].pos;
                append_arc_world_samples(a, b, p.vertices[k].bulge, &mut v);
            }
            v
        }
        Geom::Circle(c) => tessellate_circle_loop(c.center, c.radius, 64),
        Geom::Ellipse(e) => tessellate_ellipse_loop(e, 64),
        _ => Vec::new(),
    }
}

/// Tessellate an Ellipse to an N-vertex closed loop using the kernel's
/// own `point_at`. Same density rationale as circles.
pub(super) fn tessellate_ellipse_loop(e: &Ellipse, n: usize) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f64) / (n as f64) * std::f64::consts::TAU;
        out.push(e.point_at(t));
    }
    out
}

/// Find the t-value at which an infinite line (origin `o`, unit
/// direction `u`) crosses the SEGMENT a→b. Returns None if the line
/// misses the segment (intersection lies outside [0,1] along the
/// segment) or if the line is parallel to the segment. Used by the
/// hatch-pattern renderer to clip each parallel line against each
/// boundary edge.
/// Collapse near-identical t-values in a SORTED hit list. A pattern line
/// passing exactly through a boundary VERTEX is reported once per adjacent
/// edge — two hits at the same t — which corrupts even-odd pairing (the
/// duplicate pair consumes itself and the segment vanishes). Dedupe with a
/// relative epsilon so tiny hatches still merge correctly.
///
/// Kept as a unit-tested helper: the RENDER paths now delegate to the kernel
/// `hatch_line_intervals` (which dedupes per-loop internally), but this
/// documents the original fix and its regression lock.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn dedupe_hits(hits: &mut Vec<f64>) {
    if hits.len() < 2 {
        return;
    }
    hits.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let scale = hits.last().unwrap().abs().max(1.0);
    let eps = 1e-9 * scale;
    let mut w = 1;
    for i in 1..hits.len() {
        if (hits[i] - hits[w - 1]).abs() > eps {
            hits[w] = hits[i];
            w += 1;
        }
    }
    hits.truncate(w);
}

/// Soft drop shadow above a panel's top edge (the command bar's
/// canvas-facing edge). `steps` bands of fading black alpha.
pub(super) fn paint_top_shadow(p: &egui::Painter, rect: egui::Rect, steps: i32) {
    for i in 0..steps {
        let t = 1.0 - i as f32 / steps as f32;
        let a = (118.0 * t).max(14.0) as u8;
        let d = i as f32;
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() - d - 1.0),
                egui::pos2(rect.right(), rect.top() - d),
            ),
            egui::Rounding::ZERO,
            egui::Color32::from_black_alpha(a),
        );
    }
}

pub(super) fn line_segment_intersect_t(o: Vec2, u: Vec2, a: Vec2, b: Vec2) -> Option<f64> {
    let d = b - a;
    // Parametric line: o + t*u; segment: a + s*d. Solve for (t, s).
    //   o.x + t*u.x = a.x + s*d.x
    //   o.y + t*u.y = a.y + s*d.y
    // → [u.x  -d.x] [t]   [a.x - o.x]
    //   [u.y  -d.y] [s] = [a.y - o.y]
    let det = u.x * (-d.y) - (-d.x) * u.y;
    if det.abs() < 1e-12 {
        return None;
    } // parallel
    let rhs_x = a.x - o.x;
    let rhs_y = a.y - o.y;
    let t = (rhs_x * (-d.y) - (-d.x) * rhs_y) / det;
    let s = (u.x * rhs_y - u.y * rhs_x) / det;
    // Segment-end test — accept hits AT endpoints (half-open avoids
    // double-counting at shared vertices via the standard even-odd
    // ray-cast convention, but for hatch pattern lines we want every
    // edge crossing once).
    if s < -1e-9 || s > 1.0 + 1e-9 {
        return None;
    }
    Some(t)
}

/// AutoCAD polyline bulge for a 3-point arc through (p1, p2, p3) in
/// THAT ORDER. Returns 0.0 (straight segment) when the three points are
/// collinear or coincident. The sign matches the polyline convention:
/// positive bulge = arc bends to the LEFT of the chord p1→p3 (CCW).
pub(super) fn bulge_from_three_points(p1: Vec2, p2: Vec2, p3: Vec2) -> f64 {
    use cad_kernel::arc_three_points;
    let Some(arc) = arc_three_points(p1, p2, p3) else {
        return 0.0;
    };
    let r = arc.radius;
    if r < EPS {
        return 0.0;
    }
    let center = arc.center;
    // Angles at the center for the two endpoints + the midpoint pick.
    let a1 = (p1 - center).angle();
    let a3 = (p3 - center).angle();
    let a2 = (p2 - center).angle();
    // Try CCW direction first: (a3 - a1) mod TAU. If the swept range
    // contains a2, the polyline travels CCW (positive bulge); else CW.
    let ccw_sweep = (a3 - a1).rem_euclid(std::f64::consts::TAU);
    let mid_offset = (a2 - a1).rem_euclid(std::f64::consts::TAU);
    let signed_theta = if mid_offset <= ccw_sweep + 1e-9 {
        ccw_sweep
    } else {
        -(std::f64::consts::TAU - ccw_sweep)
    };
    (signed_theta / 4.0).tan()
}

/// Shoelace double-area of a point polygon (signed; CCW positive). The
/// callers halve it — kept as double-area to avoid intermediate division.
pub(super) fn polygon_shoelace(pts: &[Vec2]) -> f64 {
    let mut twice = 0.0;
    for w in pts.windows(2) {
        twice += w[0].x * w[1].y - w[1].x * w[0].y;
    }
    let n = pts.len();
    if n >= 3 {
        twice += pts[n - 1].x * pts[0].y - pts[0].x * pts[n - 1].y;
    }
    twice
}

/// Point at arc length `s` along a pre-sampled polyline (`pts` + cumulative
/// lengths `cum`, total `total`). Used by DIVIDE / MEASURE.
pub(super) fn point_at_arclen(pts: &[Vec2], cum: &[f64], total: f64, s: f64) -> Vec2 {
    let s = s.clamp(0.0, total);
    let mut i = 1;
    while i < pts.len() && cum[i] < s {
        i += 1;
    }
    let i = i.min(pts.len() - 1);
    let seg = cum[i] - cum[i - 1];
    let f = if seg > 1e-12 {
        (s - cum[i - 1]) / seg
    } else {
        0.0
    };
    pts[i - 1] + (pts[i] - pts[i - 1]) * f
}

/// A closed loop (circle / full ellipse / closed polyline) — DIVIDE places
/// `n` marks around it, vs `n−1` interior marks on an open curve.
pub(super) fn geom_is_closed_loop(g: &Geom) -> bool {
    matches!(g, Geom::Circle(_) | Geom::Ellipse(_)) || matches!(g, Geom::Polyline(p) if p.closed)
}

/// Sample a curve dobject into a dense polyline of world points for the
/// arc-length distribution. Returns empty for non-curve geoms.
pub(super) fn sample_path_points(g: &Geom, n: usize) -> Vec<Vec2> {
    use std::f64::consts::TAU;
    let n = n.max(2);
    match g {
        Geom::Line(l) => (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                l.a + (l.b - l.a) * t
            })
            .collect(),
        Geom::Circle(c) => (0..=n)
            .map(|i| {
                let a = i as f64 / n as f64 * TAU;
                c.center + Vec2::new(a.cos(), a.sin()) * c.radius
            })
            .collect(),
        Geom::Arc(a) => (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                let ang = a.start_angle + a.sweep_angle * t;
                a.center + Vec2::new(ang.cos(), ang.sin()) * a.radius
            })
            .collect(),
        Geom::Ellipse(e) => (0..=n)
            .map(|i| e.point_at(i as f64 / n as f64 * TAU))
            .collect(),
        Geom::EllipseArc(ea) => (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                ea.ellipse.point_at(ea.start_param + ea.sweep_param * t)
            })
            .collect(),
        Geom::Spline(s) => s.tessellate(n),
        Geom::Polyline(p) => {
            let mut out: Vec<Vec2> = p.vertices.iter().map(|v| v.pos).collect();
            if p.closed {
                if let Some(f) = p.vertices.first() {
                    out.push(f.pos);
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// DIVIDE / MEASURE mark positions along `geom`: divide → `n` equal parts
/// (n marks on a closed loop, n−1 interior marks on an open curve); measure
/// → as many `dist`-long steps from the start as fit.
pub(super) fn divmeasure_positions_for(
    geom: &Geom,
    count_or_dist: f64,
    is_measure: bool,
) -> Vec<Vec2> {
    let pts = sample_path_points(geom, 600);
    if pts.len() < 2 {
        return Vec::new();
    }
    let mut cum = vec![0.0_f64; pts.len()];
    for i in 1..pts.len() {
        cum[i] = cum[i - 1] + pts[i].dist(pts[i - 1]);
    }
    let total = cum[pts.len() - 1];
    if total < 1e-9 {
        return Vec::new();
    }
    let mut out = Vec::new();
    if is_measure {
        let d = count_or_dist.abs().max(1e-6);
        if d >= total {
            return Vec::new();
        }
        // As many marks as fit, stepped from the start end.
        let n = ((total - 1e-9) / d).floor() as usize;
        for k in 1..=n {
            out.push(point_at_arclen(&pts, &cum, total, k as f64 * d));
            if out.len() >= 4000 {
                break;
            }
        }
    } else {
        let n = (count_or_dist as usize).max(2);
        let ks: Vec<usize> = if geom_is_closed_loop(geom) {
            (0..n).collect()
        } else {
            (1..n).collect()
        };
        for k in ks {
            out.push(point_at_arclen(
                &pts,
                &cum,
                total,
                total * k as f64 / n as f64,
            ));
            if out.len() >= 4000 {
                break;
            } // same cap as the measure path
        }
    }
    out
}

/// Short, capitalised label for a Dobject's underlying geometry. Used
/// in dialog/window titles where the user wants to know "what kind of
/// dobject am I editing?" without reading a full describe() line.
pub(super) fn dobject_kind_name(g: &Geom) -> &'static str {
    match g {
        Geom::Line(_) => "Line",
        Geom::Circle(_) => "Circle",
        Geom::Arc(_) => "Arc",
        Geom::Ellipse(_) => "Ellipse",
        Geom::EllipseArc(_) => "EllipseArc",
        Geom::Point(_) => "Point",
        Geom::Polyline(_) => "Polyline",
        Geom::Hatch(_) => "Hatch",
        Geom::Spline(_) => "Spline",
        Geom::Wall(_) => "Wall",
        Geom::Text(_) => "Text",
        Geom::Dimension(_) => "Dimension",
        Geom::BlockRef(_) => "Block",
        Geom::Xline(_) => "Xline",
        Geom::Ray(_) => "Ray",
        Geom::Donut(_) => "Donut",
        Geom::Wipeout(_) => "Wipeout",
        Geom::CenterMark(_) => "CenterMark",
        Geom::Region(_) => "Region",
        Geom::Xref(_) => "Xref",
        _ => "Entity",
    }
}

pub(super) fn describe(g: &Geom) -> String {
    match g {
        Geom::Line(l) => format!(
            "line ({:.2},{:.2}) → ({:.2},{:.2})",
            l.a.x, l.a.y, l.b.x, l.b.y
        ),
        Geom::Circle(c) => format!(
            "circle c=({:.2},{:.2}) r={:.2}",
            c.center.x, c.center.y, c.radius
        ),
        Geom::Arc(a) => format!(
            "arc c=({:.2},{:.2}) r={:.2} {:.1}°+{:.1}°",
            a.center.x,
            a.center.y,
            a.radius,
            a.start_angle.to_degrees(),
            a.sweep_angle.to_degrees()
        ),
        Geom::Ellipse(el) => format!(
            "ellipse c=({:.2},{:.2}) a={:.2} ratio={:.3} rot={:.1}°",
            el.center.x,
            el.center.y,
            el.semi_major(),
            el.ratio,
            el.major.angle().to_degrees()
        ),
        Geom::EllipseArc(ea) => format!(
            "ellipsearc c=({:.2},{:.2}) a={:.2} ratio={:.3} {:.1}°+{:.1}°",
            ea.ellipse.center.x,
            ea.ellipse.center.y,
            ea.ellipse.semi_major(),
            ea.ellipse.ratio,
            ea.start_param.to_degrees(),
            ea.sweep_param.to_degrees()
        ),
        Geom::Point(pt) => format!(
            "point ({:.2},{:.2}) style={} size={:.2}",
            pt.location.x, pt.location.y, pt.style, pt.size
        ),
        Geom::Polyline(p) => format!(
            "polyline {} verts{} len={:.2}",
            p.vertices.len(),
            if p.closed { " (closed)" } else { "" },
            p.length()
        ),
        Geom::Hatch(h) => format!(
            "hatch {} boundary loop(s) ({:?})",
            h.boundary_handles.len(),
            h.pattern
        ),
        Geom::Spline(s) => format!(
            "spline degree={} {} ctrl pts{}",
            s.degree,
            s.control_points.len(),
            if s.weights.iter().all(|w| (*w - 1.0).abs() < 1e-9) {
                ""
            } else {
                " (rational)"
            }
        ),
        Geom::Wall(w) => format!(
            "wall ({:.2},{:.2}) → ({:.2},{:.2}) thk={:.3} len={:.3}",
            w.start.x,
            w.start.y,
            w.end.x,
            w.end.y,
            w.thickness,
            w.length()
        ),
        Geom::Text(t) => format!(
            "text \"{}\" @ ({:.2},{:.2}) h={:.3}",
            t.text, t.position.x, t.position.y, t.height
        ),
        Geom::Dimension(d) => {
            use cad_kernel::DimKind;
            let kind_name = match &d.kind {
                DimKind::Linear { ortho, .. } => match ortho {
                    cad_kernel::LinearOrtho::Horizontal => "dim h",
                    cad_kernel::LinearOrtho::Vertical => "dim v",
                    cad_kernel::LinearOrtho::Aligned => "dim aligned",
                },
                DimKind::Radius { .. } => "dim radius",
                DimKind::Diameter { .. } => "dim diameter",
                _ => "dim",
            };
            format!(
                "{} = {:.3} style={}",
                kind_name,
                d.measured_value(),
                d.style
            )
        }
        Geom::BlockRef(br) => format!(
            "block #{} @ ({:.3},{:.3}) s={:.3} rot={:.2}°",
            br.block,
            br.insert.x,
            br.insert.y,
            br.scale,
            br.rotation.to_degrees()
        ),
        _ => format!("{}", dobject_kind_name(g)),
    }
}

/// Verbose dump — every coordinate the algorithm sees, no summarisation.
/// Used by the hatch debug log so the user can verify "is this polyline
/// actually closed? do its first and last verts coincide?". Polyline
/// vertices include their bulge so arc segments don't appear straight in
/// the dump. Spline dumps control points + weights. Hatch lists handle
/// IDs in order. Coordinates print at 3 decimal places — enough to spot
/// "1e-3 gap between supposedly coincident endpoints" without flooding
/// the log on big polylines.
/// Full property dump for the LIST command — every meaningful
/// measurement (length, perimeter, area, angles, bbox) per geom type,
/// one line per measurement. Returns a Vec<String> so the caller can
/// indent and push each line to the command history.
pub(super) fn list_full_details(g: &Geom) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    match g {
        Geom::Line(l) => {
            let d = l.b - l.a;
            let len = d.len();
            let mid = (l.a + l.b) * 0.5;
            out.push(format!("type           : line"));
            out.push(format!("from           : ({:.4}, {:.4})", l.a.x, l.a.y));
            out.push(format!("to             : ({:.4}, {:.4})", l.b.x, l.b.y));
            out.push(format!("midpoint       : ({:.4}, {:.4})", mid.x, mid.y));
            out.push(format!("length         : {:.6}", len));
            out.push(format!("ΔX, ΔY         : {:+.6}, {:+.6}", d.x, d.y));
            out.push(format!(
                "angle (X-axis) : {:+.4}°",
                d.y.atan2(d.x).to_degrees()
            ));
        }
        Geom::Circle(c) => {
            let area = std::f64::consts::PI * c.radius * c.radius;
            let circ = std::f64::consts::TAU * c.radius;
            out.push(format!("type           : circle"));
            out.push(format!(
                "center         : ({:.4}, {:.4})",
                c.center.x, c.center.y
            ));
            out.push(format!("radius         : {:.6}", c.radius));
            out.push(format!("diameter       : {:.6}", c.radius * 2.0));
            out.push(format!("circumference  : {:.6}", circ));
            out.push(format!("area           : {:.6}", area));
        }
        Geom::Arc(a) => {
            let arc_len = a.radius * a.sweep_angle.abs();
            let chord_len = 2.0 * a.radius * (a.sweep_angle.abs() * 0.5).sin();
            let sector_area = 0.5 * a.radius * a.radius * a.sweep_angle.abs();
            let p_start = Vec2::new(
                a.center.x + a.radius * a.start_angle.cos(),
                a.center.y + a.radius * a.start_angle.sin(),
            );
            let p_end = Vec2::new(
                a.center.x + a.radius * (a.start_angle + a.sweep_angle).cos(),
                a.center.y + a.radius * (a.start_angle + a.sweep_angle).sin(),
            );
            out.push(format!("type           : arc"));
            out.push(format!(
                "center         : ({:.4}, {:.4})",
                a.center.x, a.center.y
            ));
            out.push(format!("radius         : {:.6}", a.radius));
            out.push(format!(
                "start angle    : {:+.4}°",
                a.start_angle.to_degrees()
            ));
            out.push(format!(
                "sweep angle    : {:+.4}°",
                a.sweep_angle.to_degrees()
            ));
            out.push(format!(
                "start point    : ({:.4}, {:.4})",
                p_start.x, p_start.y
            ));
            out.push(format!("end point      : ({:.4}, {:.4})", p_end.x, p_end.y));
            out.push(format!("arc length     : {:.6}", arc_len));
            out.push(format!("chord length   : {:.6}", chord_len));
            out.push(format!("sector area    : {:.6}", sector_area));
        }
        Geom::Ellipse(el) => {
            let a = el.semi_major();
            let b = el.semi_minor();
            // Ramanujan's approximation for ellipse perimeter.
            let h = ((a - b) / (a + b)).powi(2);
            let perim =
                std::f64::consts::PI * (a + b) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()));
            let area = std::f64::consts::PI * a * b;
            out.push(format!("type           : ellipse"));
            out.push(format!(
                "center         : ({:.4}, {:.4})",
                el.center.x, el.center.y
            ));
            out.push(format!("semi-major (a) : {:.6}", a));
            out.push(format!("semi-minor (b) : {:.6}", b));
            out.push(format!("ratio b/a      : {:.6}", el.ratio));
            out.push(format!(
                "rotation       : {:+.4}°",
                el.major.angle().to_degrees()
            ));
            out.push(format!("perimeter (≈)  : {:.6}  (Ramanujan)", perim));
            out.push(format!("area           : {:.6}", area));
        }
        Geom::EllipseArc(ea) => {
            let a = ea.ellipse.semi_major();
            let b = ea.ellipse.semi_minor();
            let area =
                std::f64::consts::PI * a * b * (ea.sweep_param.abs() / std::f64::consts::TAU);
            out.push(format!("type           : ellipse arc"));
            out.push(format!(
                "center         : ({:.4}, {:.4})",
                ea.ellipse.center.x, ea.ellipse.center.y
            ));
            out.push(format!("semi-major (a) : {:.6}", a));
            out.push(format!("semi-minor (b) : {:.6}", b));
            out.push(format!(
                "rotation       : {:+.4}°",
                ea.ellipse.major.angle().to_degrees()
            ));
            out.push(format!(
                "start param    : {:+.4}°",
                ea.start_param.to_degrees()
            ));
            out.push(format!(
                "sweep param    : {:+.4}°",
                ea.sweep_param.to_degrees()
            ));
            out.push(format!(
                "sector area    : {:.6}  (fraction of full ellipse)",
                area
            ));
        }
        Geom::Point(pt) => {
            out.push(format!("type           : point"));
            out.push(format!(
                "location       : ({:.4}, {:.4})",
                pt.location.x, pt.location.y
            ));
        }
        Geom::Polyline(p) => {
            let n = p.vertices.len();
            // Perimeter = sum of segment lengths (existing helper).
            let perim = p.length();
            // Shoelace area — only meaningful for closed polylines.
            // Bulge contribution to area is approximated by chord; refine
            // later if precision matters.
            let area = if p.closed && n >= 3 {
                let mut a = 0.0;
                for i in 0..n {
                    let p0 = p.vertices[i].pos;
                    let p1 = p.vertices[(i + 1) % n].pos;
                    a += p0.x * p1.y - p1.x * p0.y;
                }
                Some(a.abs() * 0.5)
            } else {
                None
            };
            // Bbox.
            let mut min = p.vertices[0].pos;
            let mut max = min;
            for v in &p.vertices {
                if v.pos.x < min.x {
                    min.x = v.pos.x;
                }
                if v.pos.y < min.y {
                    min.y = v.pos.y;
                }
                if v.pos.x > max.x {
                    max.x = v.pos.x;
                }
                if v.pos.y > max.y {
                    max.y = v.pos.y;
                }
            }
            out.push(format!("type           : polyline"));
            out.push(format!("vertices       : {}", n));
            out.push(format!("closed         : {}", p.closed));
            out.push(format!("perimeter      : {:.6}", perim));
            if let Some(a) = area {
                out.push(format!("area (shoelace): {:.6}", a));
            }
            out.push(format!("bbox min       : ({:.4}, {:.4})", min.x, min.y));
            out.push(format!("bbox max       : ({:.4}, {:.4})", max.x, max.y));
            out.push(format!(
                "bbox size      : {:.4} × {:.4}",
                max.x - min.x,
                max.y - min.y
            ));
        }
        Geom::Spline(s) => {
            out.push(format!("type           : spline"));
            out.push(format!("degree         : {}", s.degree));
            out.push(format!("control pts    : {}", s.control_points.len()));
            out.push(format!(
                "rational       : {}",
                s.weights.iter().any(|w| (*w - 1.0).abs() > 1e-9)
            ));
        }
        Geom::Hatch(h) => {
            out.push(format!("type           : hatch"));
            out.push(format!("pattern        : {:?}", h.pattern));
            out.push(format!("boundary loops : {}", h.boundary_handles.len()));
        }
        Geom::Wall(w) => {
            let len = w.length();
            let mid = (w.start + w.end) * 0.5;
            out.push(format!("type           : wall"));
            out.push(format!(
                "start          : ({:.4}, {:.4})",
                w.start.x, w.start.y
            ));
            out.push(format!("end            : ({:.4}, {:.4})", w.end.x, w.end.y));
            out.push(format!("midpoint       : ({:.4}, {:.4})", mid.x, mid.y));
            out.push(format!("length         : {:.6}", len));
            out.push(format!("thickness      : {:.6}", w.thickness));
        }
        Geom::Text(t) => {
            out.push(format!("type           : text"));
            out.push(format!("string         : \"{}\"", t.text));
            out.push(format!(
                "position       : ({:.4}, {:.4})",
                t.position.x, t.position.y
            ));
            out.push(format!("height         : {:.6}", t.height));
            out.push(format!("angle          : {:+.4}°", t.angle.to_degrees()));
            out.push(format!("h_align        : {:?}", t.h_align));
            out.push(format!("v_align        : {:?}", t.v_align));
            out.push(format!("style id       : {}", t.style));
        }
        Geom::Dimension(d) => {
            use cad_kernel::DimKind;
            out.push(format!("type           : dimension"));
            match &d.kind {
                DimKind::Linear {
                    p1,
                    p2,
                    dimline_pos,
                    ortho,
                } => {
                    out.push(format!("kind           : linear ({:?})", ortho));
                    out.push(format!("p1             : ({:.4}, {:.4})", p1.x, p1.y));
                    out.push(format!("p2             : ({:.4}, {:.4})", p2.x, p2.y));
                    out.push(format!(
                        "dimline_pos    : ({:.4}, {:.4})",
                        dimline_pos.x, dimline_pos.y
                    ));
                }
                DimKind::Radius {
                    center,
                    on_circle,
                    leader_end,
                } => {
                    out.push(format!("kind           : radius"));
                    out.push(format!(
                        "center         : ({:.4}, {:.4})",
                        center.x, center.y
                    ));
                    out.push(format!(
                        "on_circle      : ({:.4}, {:.4})",
                        on_circle.x, on_circle.y
                    ));
                    out.push(format!(
                        "leader_end     : ({:.4}, {:.4})",
                        leader_end.x, leader_end.y
                    ));
                }
                DimKind::Diameter {
                    center,
                    on_circle,
                    leader_end,
                } => {
                    out.push(format!("kind           : diameter"));
                    out.push(format!(
                        "center         : ({:.4}, {:.4})",
                        center.x, center.y
                    ));
                    out.push(format!(
                        "on_circle      : ({:.4}, {:.4})",
                        on_circle.x, on_circle.y
                    ));
                    out.push(format!(
                        "leader_end     : ({:.4}, {:.4})",
                        leader_end.x, leader_end.y
                    ));
                }
                _ => out.push(format!("kind           : other")),
            }
            out.push(format!("measured value : {:.6}", d.measured_value()));
            out.push(format!("style id       : {}", d.style));
            if let Some(s) = &d.text_override {
                out.push(format!("text override  : \"{}\"", s));
            }
        }
        Geom::BlockRef(br) => {
            out.push("type           : block reference".to_string());
            out.push(format!("block id       : {}", br.block));
            out.push(format!(
                "insert         : ({:.4}, {:.4})",
                br.insert.x, br.insert.y
            ));
            out.push(format!("scale          : {:.4}", br.scale));
            out.push(format!("rotation       : {:.4}°", br.rotation.to_degrees()));
        }
        _ => out.push(format!("type           : {}", dobject_kind_name(g))),
    }
    out
}

pub(super) fn describe_verbose(g: &Geom) -> String {
    match g {
        Geom::Line(l) => format!(
            "line  a=({:.3},{:.3})  b=({:.3},{:.3})  len={:.3}",
            l.a.x,
            l.a.y,
            l.b.x,
            l.b.y,
            (l.b - l.a).len()
        ),
        Geom::Circle(c) => format!(
            "circle  c=({:.3},{:.3})  r={:.3}",
            c.center.x, c.center.y, c.radius
        ),
        Geom::Arc(a) => format!(
            "arc  c=({:.3},{:.3})  r={:.3}  start={:.2}°  sweep={:.2}°",
            a.center.x,
            a.center.y,
            a.radius,
            a.start_angle.to_degrees(),
            a.sweep_angle.to_degrees()
        ),
        Geom::Ellipse(el) => format!(
            "ellipse  c=({:.3},{:.3})  a={:.3}  ratio={:.3}  rot={:.2}°",
            el.center.x,
            el.center.y,
            el.semi_major(),
            el.ratio,
            el.major.angle().to_degrees()
        ),
        Geom::EllipseArc(ea) => format!(
            "ellipsearc  c=({:.3},{:.3})  a={:.3}  ratio={:.3}  start={:.2}°  sweep={:.2}°",
            ea.ellipse.center.x,
            ea.ellipse.center.y,
            ea.ellipse.semi_major(),
            ea.ellipse.ratio,
            ea.start_param.to_degrees(),
            ea.sweep_param.to_degrees()
        ),
        Geom::Point(pt) => format!("point  ({:.3},{:.3})", pt.location.x, pt.location.y),
        Geom::Polyline(p) => {
            let n = p.vertices.len();
            let head = format!(
                "polyline  {} verts  closed={}  len={:.3}",
                n,
                p.closed,
                p.length()
            );
            let mut s = head;
            for (i, v) in p.vertices.iter().enumerate() {
                let suffix = if v.bulge.abs() > 1e-9 {
                    format!(" bulge={:.4}", v.bulge)
                } else {
                    String::new()
                };
                s.push_str(&format!(
                    "\n        v{:02}=({:.3},{:.3}){}",
                    i, v.pos.x, v.pos.y, suffix
                ));
            }
            if n >= 2 {
                let first = p.vertices[0].pos;
                let last = p.vertices[n - 1].pos;
                let gap = (last - first).len();
                s.push_str(&format!(
                    "\n        endpoint gap (v00→v{:02}) = {:.6}{}",
                    n - 1,
                    gap,
                    if !p.closed && gap < 1e-3 {
                        "  ← visually closed but `closed=false`"
                    } else {
                        ""
                    }
                ));
            }
            s
        }
        Geom::Hatch(h) => {
            let mut s = format!(
                "hatch  pattern={:?}  {} boundary handle(s)",
                h.pattern,
                h.boundary_handles.len()
            );
            for (i, hh) in h.boundary_handles.iter().enumerate() {
                s.push_str(&format!("\n        loop{} → handle #{}", i, hh));
            }
            s
        }
        Geom::Spline(s) => {
            let mut out = format!(
                "spline  degree={}  {} ctrl pts  rational={}",
                s.degree,
                s.control_points.len(),
                !s.weights.iter().all(|w| (*w - 1.0).abs() < 1e-9)
            );
            for (i, p) in s.control_points.iter().enumerate() {
                out.push_str(&format!(
                    "\n        cp{:02}=({:.3},{:.3}) w={:.3}",
                    i,
                    p.x,
                    p.y,
                    s.weights.get(i).copied().unwrap_or(1.0)
                ));
            }
            out
        }
        Geom::Wall(w) => format!(
            "wall  start=({:.3},{:.3})  end=({:.3},{:.3})  thk={:.3}  len={:.3}",
            w.start.x,
            w.start.y,
            w.end.x,
            w.end.y,
            w.thickness,
            w.length(),
        ),
        Geom::Text(t) => format!(
            "text  pos=({:.3},{:.3})  h={:.3}  ang={:+.2}°  \"{}\"",
            t.position.x,
            t.position.y,
            t.height,
            t.angle.to_degrees(),
            t.text,
        ),
        Geom::Dimension(d) => format!(
            "dimension  measured={:.4}  style=#{}  override={:?}",
            d.measured_value(),
            d.style,
            d.text_override,
        ),
        Geom::BlockRef(br) => format!(
            "blockref  block=#{}  insert=({:.3},{:.3})  scale={:.3}  rot={:+.2}°",
            br.block,
            br.insert.x,
            br.insert.y,
            br.scale,
            br.rotation.to_degrees(),
        ),
        _ => dobject_kind_name(g).to_string(),
    }
}

// ---- icon tool-button -------------------------------------------------------

/// Color picker UI — ACI palette as PRIMARY, TrueColor as secondary.
/// Returns true if the value changed. See
/// `feedback_rust_cad_color_aci_primary` memo. TrueColor values are
/// interned via the document's `TrueColorTable` (see Color storage
/// refactor memo); the picker takes a &mut TrueColorTable for that.
///
/// Clicking the "Pick ACI…" button sets `*wants_pick = true`; the caller
/// then opens the shared polar-wheel window (see `render_aci_picker_window`
/// + the ACI picker UI reference at `~/workspace/RUST_CAD/ACI_Picker_UI.html`).
pub(super) fn aci_color_picker(
    ui: &mut egui::Ui,
    _id: impl std::hash::Hash,
    value: &mut Color,
    truecolors: &mut TrueColorTable,
    wants_pick: &mut bool,
) -> bool {
    let mut changed = false;

    // Current-value summary swatch + label
    let (r, g, b) = match *value {
        Color::Aci(i) => aci_palette(i),
        Color::TrueColorRef(idx) => {
            let v = truecolors.get(idx).unwrap_or(0xFFFFFF);
            (
                ((v >> 16) & 0xFF) as u8,
                ((v >> 8) & 0xFF) as u8,
                (v & 0xFF) as u8,
            )
        }
        Color::ByLayer => (180, 180, 200),
        Color::ByBlock => (140, 140, 160),
    };
    let summary = match *value {
        Color::ByLayer => "ByLayer".to_string(),
        Color::ByBlock => "ByBlock".to_string(),
        Color::Aci(i) => format!("ACI {}", i),
        Color::TrueColorRef(idx) => {
            let v = truecolors.get(idx).unwrap_or(0xFFFFFF);
            format!("RGB #{:06X}", v & 0x00FFFFFF)
        }
    };

    ui.horizontal(|ui| {
        // Clickable summary chip — same affordance as the layer panel
        // swatch: click to open the polar ACI picker.
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 18.0), egui::Sense::click());
        ui.painter()
            .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(0.7, egui::Color32::from_rgb(70, 80, 95)),
        );
        if resp.on_hover_text("Click to pick an ACI color").clicked() {
            *wants_pick = true;
        }
        ui.label(summary);
        if ui.small_button("Pick ACI…").clicked() {
            *wants_pick = true;
        }
    });

    // Secondary controls — ByLayer / ByBlock / TrueColor fallback
    ui.horizontal(|ui| {
        if ui.small_button("ByLayer").clicked() {
            *value = Color::ByLayer;
            changed = true;
        }
        if ui.small_button("ByBlock").clicked() {
            *value = Color::ByBlock;
            changed = true;
        }
        // TrueColor fallback — opens egui's RGB picker. Commits by
        // interning the RGB and storing only a small ref on `value`.
        let mut rgb = [r, g, b];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            let packed = ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | (rgb[2] as u32);
            *value = Color::TrueColorRef(truecolors.intern(packed));
            changed = true;
        }
        ui.small("TrueColor…");
    });

    changed
}

/// Text-only toolbar button styled to match `tool_button`'s color scheme
/// and height so the toolbar reads as one consistent strip. Used for the
/// panel-toggle buttons (snap, grips, settings, layers, pens, info, array)
/// that don't have a drafted icon.
///
/// `active` highlights the button in the same blue as a selected drafting
/// tool — visual cue that the corresponding panel is open / feature is on.
// ---------------------------------------------------------------------------
// Quick Access Toolbar — top line. A customizable strip of file/common-action
// icon shortcuts, plus the AutoRASM logo on the left and the product title on
// the right. The set of shortcuts is edited through the `▾` customize drop
// window. Command (drafting) icons are a separate strip, handled elsewhere.
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum QatAction {
    New,
    Open,
    Save,
    SaveAs,
    Undo,
    Redo,
}

impl QatAction {
    /// Every action offered in the customize drop window, in display order.
    pub(super) fn all() -> [QatAction; 6] {
        [
            QatAction::New,
            QatAction::Open,
            QatAction::Save,
            QatAction::SaveAs,
            QatAction::Undo,
            QatAction::Redo,
        ]
    }
    /// The shortcuts shown by default (first run / before customization).
    pub(super) fn default_set() -> Vec<QatAction> {
        QatAction::all().to_vec()
    }
    /// Human label for the customize list + tooltips.
    pub(super) fn label(self) -> &'static str {
        match self {
            QatAction::New => "New",
            QatAction::Open => "Open…",
            QatAction::Save => "Save",
            QatAction::SaveAs => "Save As",
            QatAction::Undo => "Undo",
            QatAction::Redo => "Redo",
        }
    }
}

/// Try to load the brand logo PNG into a texture. Looks in a few likely spots
/// so it works from the repo root, the crate dir, or next to the exe. Returns
/// None if the file isn't there (caller draws a placeholder).
pub(super) fn load_logo_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    // Prefer the vector SVG (crisp at any size); fall back to a PNG.
    let svg = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo.svg"),
        "cad_app/assets/logo.svg",
        "assets/logo.svg",
    ];
    for path in svg {
        if let Ok(bytes) = std::fs::read(path) {
            if let Some(tex) = rasterize_svg_logo(ctx, &bytes) {
                return Some(tex);
            }
        }
    }
    let png = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo.png"),
        "cad_app/assets/logo.png",
        "assets/logo.png",
    ];
    for path in png {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(img) = image::load_from_memory(&bytes) else {
            continue;
        };
        let img = img.resize(256, 256, image::imageops::FilterType::Lanczos3);
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let color =
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
        return Some(ctx.load_texture("autorasm_logo", color, egui::TextureOptions::LINEAR));
    }
    None
}

/// Rasterize an SVG to a texture at a generous resolution so it stays sharp.
pub(super) fn rasterize_svg_logo(ctx: &egui::Context, data: &[u8]) -> Option<egui::TextureHandle> {
    use resvg::{tiny_skia, usvg};
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(data, &opt).ok()?;
    let size = tree.size();
    // Render so the longest side is ~256 px (≥ any sensible on-screen size).
    let target = 256.0;
    let scale = target / size.width().max(size.height());
    let pw = (size.width() * scale).ceil().max(1.0) as u32;
    let ph = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let color =
        egui::ColorImage::from_rgba_premultiplied([pw as usize, ph as usize], pixmap.data());
    Some(ctx.load_texture("autorasm_logo", color, egui::TextureOptions::LINEAR))
}

/// Painted placeholder logo (brand-teal gear ring + gold "R"), used until the
/// real PNG is dropped at assets/logo.png.
pub(super) fn draw_logo_placeholder(p: &egui::Painter, rect: egui::Rect) {
    let c = rect.center();
    let teal = egui::Color32::from_rgb(38, 78, 98);
    let gold = egui::Color32::from_rgb(214, 184, 122);
    let radius = rect.width().min(rect.height()) * 0.34;
    for k in 0..10 {
        let a = (k as f32) / 10.0 * std::f32::consts::TAU;
        let dir = egui::vec2(a.cos(), a.sin());
        p.line_segment(
            [c + dir * radius, c + dir * (radius + 4.0)],
            egui::Stroke::new(3.0, teal),
        );
    }
    p.circle_stroke(c, radius, egui::Stroke::new(3.0, teal));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        "R",
        egui::FontId::proportional(radius * 1.1),
        gold,
    );
}

/// One Quick Access shortcut button — a small flat icon tile. Icon stroke
/// uses the same colour as the menu-category text so the bar reads uniform.
pub(super) fn qat_button(ui: &mut egui::Ui, act: QatAction) -> bool {
    let col = ui.visuals().widgets.inactive.fg_stroke.color;
    let size = egui::vec2(28.0, 28.0);
    let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
    let rect = resp.rect;
    if resp.hovered() {
        painter.rect(
            rect,
            4.0,
            egui::Color32::from_rgb(48, 58, 72),
            egui::Stroke::NONE,
        );
    }
    let icol = if resp.hovered() {
        egui::Color32::from_rgb(225, 235, 245)
    } else {
        col
    };
    paint_qat_icon(&painter, rect.shrink(6.0), act, icol);
    resp.on_hover_text(act.label()).clicked()
}

/// Simple geometric glyphs for the QAT actions (placeholder art — clean and
/// recognizable without needing image assets).
pub(super) fn paint_qat_icon(
    painter: &egui::Painter,
    r: egui::Rect,
    act: QatAction,
    col: egui::Color32,
) {
    let st = egui::Stroke::new(1.6, col);
    match act {
        QatAction::New => {
            // A page with a folded top-right corner.
            let fold = r.width() * 0.32;
            let tl = r.left_top();
            let tr = egui::pos2(r.right() - fold, r.top());
            let pts = vec![
                tl,
                tr,
                egui::pos2(r.right(), r.top() + fold),
                r.right_bottom(),
                r.left_bottom(),
                tl,
            ];
            painter.add(egui::Shape::closed_line(pts, st));
            painter.line_segment([tr, egui::pos2(r.right(), r.top() + fold)], st);
            painter.line_segment([tr, egui::pos2(r.right() - fold, r.top() + fold)], st);
            painter.line_segment(
                [
                    egui::pos2(r.right() - fold, r.top() + fold),
                    egui::pos2(r.right(), r.top() + fold),
                ],
                st,
            );
        }
        QatAction::Open => {
            // An open folder (trapezoid lid).
            let y0 = r.top() + r.height() * 0.30;
            painter.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(r.left(), y0),
                    egui::pos2(r.left() + r.width() * 0.45, y0),
                    egui::pos2(r.left() + r.width() * 0.55, r.top() + r.height() * 0.12),
                    egui::pos2(r.right(), r.top() + r.height() * 0.12),
                    egui::pos2(r.right(), r.bottom()),
                    egui::pos2(r.left(), r.bottom()),
                ],
                st,
            ));
        }
        QatAction::Save | QatAction::SaveAs => {
            // A floppy disk: outer square + label notch + shutter.
            painter.rect_stroke(r, 2.0, st);
            let top = egui::Rect::from_min_max(
                egui::pos2(r.left() + r.width() * 0.22, r.top()),
                egui::pos2(r.right() - r.width() * 0.18, r.top() + r.height() * 0.34),
            );
            painter.rect_stroke(top, 0.0, st);
            let body = egui::Rect::from_min_max(
                egui::pos2(r.left() + r.width() * 0.18, r.top() + r.height() * 0.52),
                egui::pos2(r.right() - r.width() * 0.18, r.bottom() - r.height() * 0.10),
            );
            painter.rect_stroke(body, 0.0, st);
            // Save-As: a small pencil stroke over the disk to mark "as…".
            if matches!(act, QatAction::SaveAs) {
                painter.line_segment(
                    [
                        egui::pos2(r.right() - 1.0, r.top() - 1.0),
                        egui::pos2(r.right() - r.width() * 0.45, r.top() + r.height() * 0.45),
                    ],
                    egui::Stroke::new(1.6, col),
                );
            }
        }
        QatAction::Undo | QatAction::Redo => {
            // A curved arrow. Redo is the mirror of Undo.
            let mirror = matches!(act, QatAction::Redo);
            let cy = r.center().y;
            let mut pts = Vec::new();
            for k in 0..=16 {
                let t = k as f32 / 16.0;
                let a = std::f32::consts::PI * (0.15 + t * 0.85);
                let x = r.center().x + a.cos() * r.width() * 0.40;
                let y = cy + a.sin() * r.height() * 0.34;
                pts.push(egui::pos2(
                    if mirror { 2.0 * r.center().x - x } else { x },
                    y,
                ));
            }
            painter.add(egui::Shape::line(pts.clone(), st));
            // Arrowhead at the start of the arc.
            if let Some(&head) = pts.first() {
                let dx = if mirror { -1.0 } else { 1.0 };
                painter.line_segment([head, head + egui::vec2(4.0 * dx, -4.0)], st);
                painter.line_segment([head, head + egui::vec2(4.0 * dx, 4.0)], st);
            }
        }
    }
}

/// The "customize" affordance — a short bar with a down-chevron beneath it
/// (the Office/AutoCAD Quick-Access drop button the user described as "arrow
/// with a small line on top").
pub(super) fn qat_customize_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    // Chevron uses the same colour as the menu/category text.
    let col = ui.visuals().widgets.inactive.fg_stroke.color;
    let size = egui::vec2(20.0, 28.0);
    let _ = open; // no special background when the drop window is open
    let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
    let rect = resp.rect;
    if resp.hovered() {
        painter.rect(
            rect,
            4.0,
            egui::Color32::from_rgb(48, 58, 72),
            egui::Stroke::NONE,
        );
    }
    let cx = rect.center().x;
    let top = rect.top() + 10.0;
    // Short horizontal bar.
    painter.line_segment(
        [egui::pos2(cx - 5.0, top), egui::pos2(cx + 5.0, top)],
        egui::Stroke::new(1.6, col),
    );
    // Down chevron beneath.
    painter.add(egui::Shape::line(
        vec![
            egui::pos2(cx - 5.0, top + 4.0),
            egui::pos2(cx, top + 9.0),
            egui::pos2(cx + 5.0, top + 4.0),
        ],
        egui::Stroke::new(1.6, col),
    ));
    resp.on_hover_text("Customize Quick Access Toolbar")
}

pub(super) fn panel_button(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    // Allocate space matching tool_button height (52 px) but text-width
    // sized so the label decides the width.
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(225, 235, 245),
    );
    let pad_x = 10.0;
    let size = egui::vec2((galley.size().x + pad_x * 2.0).max(56.0), 52.0);
    let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
    let rect = resp.rect;
    let bg = if active {
        egui::Color32::from_rgb(60, 110, 175)
    } else if resp.hovered() {
        egui::Color32::from_rgb(48, 58, 72)
    } else {
        egui::Color32::from_rgb(28, 34, 42)
    };
    painter.rect(
        rect,
        5.0,
        bg,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
    );
    let text_pos = rect.center() - egui::vec2(galley.size().x * 0.5, galley.size().y * 0.5);
    painter.galley(text_pos, galley, egui::Color32::from_rgb(225, 235, 245));
    resp.clicked()
}

/// One-line "why" for a room build that failed — history and status line both
/// use it, so the plan row, the factory list and the Make-room menu agree.
pub(super) fn room_error_why(e: crate::factory::RoomError) -> String {
    match e {
        crate::factory::RoomError::NoBuilding => {
            "make a building first — a built room carves OUT of a solid".to_string()
        }
        crate::factory::RoomError::NoSuchRoom => "room vanished — try again".to_string(),
        crate::factory::RoomError::Profile(p) => format!(
            "outline {}",
            match p {
                cad_solid::ProfileError::TooFewPoints => "needs at least 3 corners",
                cad_solid::ProfileError::Degenerate => "encloses no area",
                cad_solid::ProfileError::SelfIntersecting => "crosses itself",
            }
        ),
    }
}

/// Toolbar button for a ONE-SHOT command (Move, Copy, Erase, Dist, …)
/// — no `Tool` enum membership, just runs `cmd` on click. Matches the
/// drafting `tool_button` height + color so the toolbar reads as one
/// strip. `glyph_kind` picks a hand-drawn placeholder icon; replace
/// with proper graphics later.
// ── Command seed arrays (2D draw / modify) ─────────────────────────────
// (icon-id, command-string, tooltip). Draw icons are painted by
// `draw_draw_glyph`; Modify icons reuse `draw_cmd_glyph` via GlyphKind.
// The mode command panel lists every command in these arrays.
pub(super) const DRAW_CMDS: &[(&str, &str, &'static str)] = &[
    ("pointer", "pointer", "Selection pointer  (Esc)"),
    ("line", "line", "Line  (L)"),
    ("pline", "pline", "Polyline  (PL)"),
    ("circle", "circle", "Circle  (C)"),
    ("arc", "arc", "Arc  (A)"),
    ("rect", "rectangle", "Rectangle  (REC)"),
    ("ellipse", "ellipse", "Ellipse  (EL)"),
    ("ellarc", "ellipsearc", "Elliptical arc"),
    ("point", "point", "Point  (PO)"),
    ("spline", "spline", "Spline  (SPL)"),
    ("wall", "wall", "Wall"),
    ("text", "text", "Text  (T)"),
    ("dim", "dim", "Dimension  (DIM)"),
    ("hatch", "hatch", "Hatch  (H)"),
];
pub(super) const MODIFY_CMDS: &[(GlyphKind, &str, &'static str)] = &[
    (GlyphKind::Move, "move", "Move  (M)"),
    (GlyphKind::Copy, "copy", "Copy  (CO)"),
    (GlyphKind::Rotate, "rotate", "Rotate  (RO)"),
    (GlyphKind::Scale, "scale", "Scale  (SC)"),
    (GlyphKind::Mirror, "mirror", "Mirror  (MI)"),
    (GlyphKind::Stretch, "stretch", "Stretch  (S)"),
    (GlyphKind::Align, "align", "Align"),
    (GlyphKind::Trim, "trim", "Trim  (TR)"),
    (GlyphKind::Extend, "extend", "Extend  (EX)"),
    (GlyphKind::Fillet, "fillet", "Fillet  (F)"),
    (GlyphKind::Chamfer, "chamfer", "Chamfer  (CHA)"),
    (GlyphKind::Offset, "offset", "Offset  (O)"),
    (GlyphKind::Join, "join", "Join"),
    (GlyphKind::Break, "break", "Break"),
    (GlyphKind::Lengthen, "lengthen 1", "Lengthen"),
    (GlyphKind::Reverse, "reverse", "Reverse"),
    (GlyphKind::ArrayGrid, "array", "Array  (AR)"),
    (GlyphKind::MatchProps, "matchprop", "Match properties"),
    (GlyphKind::ChangeLayer, "layer", "Layer  (LA)"),
    (GlyphKind::Erase, "erase", "Erase  (E)"),
    (GlyphKind::Block, "block", "Make block"),
    (GlyphKind::Insert, "insert", "Insert block"),
    (GlyphKind::Explode, "explode", "Explode  (X)"),
];

/// Line-art glyphs for the Draw rail — parallels `draw_cmd_glyph`, same
/// stroke style, centered at `c` (~±10px).
/// Natural bounding-box sizes (max extent × 2) the glyph fns were designed at.
/// A caller passes `scale = target_icon_box / GLYPH_BOX_*` so every glyph fills
/// the SAME icon box regardless of which fn drew it (§4/§7 — one uniform icon
/// size). The rail passes 1.0 (its glyphs are unchanged).
pub(super) const GLYPH_BOX_DRAW: f32 = 18.0;
pub(super) const GLYPH_BOX_METHOD: f32 = 18.0;
pub(super) const GLYPH_BOX_CMD: f32 = 24.0;

// ---- Category-dropdown row metrics (MENU_DROPDOWN_MENTOR) — matched to the
// palette so a menu row is dimensionally identical (band 26 / icon box 20 /
// icon-gap 14). Every value a named const; no magic numbers at the call sites.
pub(super) const MENU_ROW_H: f32 = 26.0; // row / hover band (§1)
pub(super) const MENU_PAD: f32 = 12.0; // inner pad — left (edge→icon) AND right (trailing→edge) (§1)
pub(super) const MENU_ICON: f32 = 20.0; // icon column width (box = ROW_H − 6 = 20)
pub(super) const MENU_ICON_GAP: f32 = 14.0; // icon → name
pub(super) const MENU_CODE_GAP: f32 = 6.0; // name → (CODE), and longest-line → shortcut zone
pub(super) const MENU_ARROW_GAP: f32 = 32.0; // longest-line → submenu-arrow column (§2, wide gap)
pub(super) const MENU_ARROW_W: f32 = 10.0; // arrow column width
/// Grace period before a hover-opened flyout closes after the pointer leaves both
/// the arrow and the flyout — lets the user travel diagonally onto it (§2).
pub(super) const MENU_FLYOUT_CLOSE_DELAY: f64 = 0.30;

/// §9 — which generalized flyout a frame shows. Its rows are BUILT FRESH each frame
/// from `&self` (`flyout_items`) so dynamic content (block names, current
/// method/style, toggle state, live labels) is always current; a click commits via
/// `flyout_activate`. Submenus nest: a `Styles` row opens `DimStylePick`, etc.
/// ARRAY method — which generator the Array dialog's Apply runs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum ArrayMethod {
    Linear,
    Polar,
    Path,
}

/// PATH-array anchor along the path (measure mode): first / last / centred.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PathAnchor {
    Start,
    Middle,
    End,
}

/// QSELECT dialog state (AutoCAD QSELECT analog). `None` filters mean
/// "All". `include` = the matches become the new selection; `exclude` =
/// everything EXCEPT the matches. Floating-only panel.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct QSelectState {
    pub open: bool,
    pub kind_filter: Option<String>,
    pub layer_filter: Option<String>,
    pub color_filter: Option<u8>,
    pub linetype_filter: Option<String>,
    pub include: bool,
    pub dock_state: crate::dock::DockState,
}

impl Default for QSelectState {
    fn default() -> Self {
        QSelectState {
            open: false,
            kind_filter: None,
            layer_filter: None,
            color_filter: None,
            linetype_filter: None,
            include: true,
            dock_state: crate::dock::DockState::Floating(Default::default()),
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) enum FlyMenu {
    Method(String),  // arc/circle/fillet method list
    Insert,          // block names → `insert <name>`
    Import,          // File → Import (raster / vector)
    Export,          // File → Export (PDF / SVG / PNG quick export)
    PlotStyleTables, // File → Plot Style Tables (CTB manager)
    Dimension,       // Formative → Dimension
    Styles,          // Formative → Styles
    DimStylePick,    // Styles → current dim-style picker
    WallStylePick,   // Styles → current wall-style picker
    Debug,           // Tools → Debug tools
    Scripts,         // Tools → Scripts (scripts/*.py → `run <name>`)
    CommandBar,      // command-bar grip right-click (Close)
}

/// One open flyout in the stack (`menu_flyouts`): which menu + the row rect it hangs
/// off (the flyout is placed to that rect's right). Frame 0 is spawned by a menubar
/// row's ▸; deeper frames by a flyout row's ▸ (§9 nesting).
#[derive(Clone)]
pub(super) struct FlyFrame {
    pub(super) menu: FlyMenu,
    pub(super) anchor: egui::Rect,
}

/// The icon slot for a flyout row (§7). `Key` → `icon_for`; `Method` → the
/// method-aware glyph; `None` → reserved empty slot (or overridden by a §8 check).
pub(super) enum FlyIcon {
    Key(&'static str),
    Method(String, String),
    None,
}

/// One generalized-flyout row (§9), built fresh from `&self`. `Row` renders through
/// the shared `paint_menu_row`; `Heading`/`Divider`/`Disabled` are the §8 non-command
/// rows. `check` = a §8 toggle (cyan check in the slot); `submenu` = opens a child
/// flyout on hover; `act` = what a click commits (`flyout_activate`).
pub(super) enum FlyItem {
    Row {
        icon: FlyIcon,
        name: String,
        col: egui::Color32,
        code: Option<(String, egui::Color32)>,
        check: Option<bool>,
        submenu: Option<FlyMenu>,
        act: Option<FlyAct>,
    },
    Heading(String),
    Divider,
    Disabled(String),
}

impl FlyItem {
    /// A plain command row: icon + name → `act` on click.
    pub(super) fn act(icon: FlyIcon, name: &str, act: FlyAct) -> Self {
        FlyItem::Row {
            icon,
            name: name.into(),
            col: PP_TEXT,
            code: None,
            check: None,
            submenu: None,
            act: Some(act),
        }
    }
    /// A §8 checkbox row: cyan check in the slot when `on`; click toggles via `act`.
    pub(super) fn toggle(name: &str, on: bool, act: FlyAct) -> Self {
        FlyItem::Row {
            icon: FlyIcon::None,
            name: name.into(),
            col: PP_TEXT,
            code: None,
            check: Some(on),
            submenu: None,
            act: Some(act),
        }
    }
    /// A submenu row: opens child `menu` on ▸ hover (§9 nesting).
    pub(super) fn sub(icon: FlyIcon, name: &str, menu: FlyMenu) -> Self {
        FlyItem::Row {
            icon,
            name: name.into(),
            col: PP_TEXT,
            code: None,
            check: None,
            submenu: Some(menu),
            act: None,
        }
    }
    /// The `(name, RowT)` this row measures + paints as (color-correct; §1/§2).
    pub(super) fn meas(&self) -> (&str, RowT<'_>) {
        match self {
            FlyItem::Divider => ("", RowT::Plain),
            FlyItem::Heading(t) | FlyItem::Disabled(t) => (t.as_str(), RowT::Plain),
            FlyItem::Row {
                name,
                code,
                submenu,
                ..
            } => {
                let rt = match (code, submenu.is_some()) {
                    (Some((c, cc)), true) => RowT::CodeArrow(c, *cc),
                    (Some((c, cc)), false) => RowT::Code(c, *cc),
                    (None, true) => RowT::Arrow,
                    (None, false) => RowT::Plain,
                };
                (name.as_str(), rt)
            }
        }
    }
}

/// What a flyout row commits on click (`flyout_activate`) — a data enum so flyout
/// content stays pure data (no stored closures). Covers every submenu's actions:
/// method pick, block insert, image import, style set, and the Tools→Debug toggles
/// / index / intersect / destructive-clear.
#[derive(Clone)]
pub(super) enum FlyAct {
    Run(String),
    Method(String, String),
    Insert(String),
    Import(bool),                    // true = raster underlay, false = vector trace
    Export(String),                  // File → Export quick export: pdf / svg / png
    NewCtb,                          // File → Plot Style Tables → New CTB…
    OpenCtb(String),                 // File → Plot Style Tables → a built-in CTB
    OpenCtbFile(std::path::PathBuf), // File → Plot Style Tables → a saved .pst
    SetDimStyle(u32),
    SetWallStyle(u32),
    ToggleBool(BoolField),
    EnsureIndex,
    IntersectView,
    IntersectArm,
    IntersectClear,
    ClearAll,
    RunScript(String), // Scripts → run scripts/<name>.py (no args)
    RescanScripts,     // Scripts → rescan scripts/*.py into the list
    OpenScriptEditor,  // Scripts → open the in-app script editor panel
    OpenScriptingDoc,  // Scripts → open the full API reference window
    CloseCommandBar,   // command-bar grip → hide the bar (reopen via Tools)
}

/// A `&mut self` bool a flyout toggle flips (Tools→Debug), named so `FlyAct` stays
/// `Clone` data instead of a stored `&mut`.
#[derive(Clone, Copy)]
pub(super) enum BoolField {
    ScreenStats,
    Debug,
    TrimDebug,
    HatchDebug,
    UiInspect,
    CmdDump,
}

/// Icon spec for a custom-painted menu row (§7 — the caller resolves it via
/// `icon_for`, never hardcoded inline). `Method` carries `(cmd, key)` for the
/// method-aware glyph; `Qat` reuses the toolbar New/Open/Save art; `Check(on)` is
/// a §8 toggle (cyan check in the slot when on). `None` reserves the empty 20px slot.
pub(super) enum MenuIcon<'a> {
    Draw(&'static str),
    Cmd(GlyphKind),
    Method(&'a str, &'a str),
    Qat(QatAction),
    Check(bool),
    None,
}

/// A row's trailing content (§1) — ONE enum for both `menu_hug_geometry` (measure)
/// and `paint_menu_row` (paint). LINE part (grouped right after the name): `Code` =
/// cyan method (CODE) in Mono (§4), `Hint` = muted Wall parenthetical (§5). ZONE
/// part (the trailing zone at/after the arrow column): `Arrow` = submenu ▸,
/// `Shortcut` = right-aligned muted Geist keys (§1). `CodeArrow` = a method row that ALSO
/// opens a submenu (`Circle (CR) ▸`). Per §1 a row has at most one ZONE element
/// (shortcut XOR arrow); the zone element is what the width rule measures.
#[derive(Clone, Copy)]
pub(super) enum RowT<'a> {
    Plain,
    Code(&'a str, egui::Color32),
    Hint(&'a str),
    Arrow,
    CodeArrow(&'a str, egui::Color32),
    Shortcut(&'a str),
}

/// Paint ONE conformant dropdown row (MENU_DROPDOWN §1) — the SINGLE painter every
/// category menu + flyout routes through. Full-width surface-2 hover band (26),
/// aligned 20px icon box (`icon_for`-resolved), Geist-13 `name` in `name_col`, the
/// `trail` line/zone content, on the shared `arrow_x` column (§2). Returns
/// `(body_clicked, arrow_hovered, rect)` — body runs the command, the ▸ opens a
/// submenu on hover.
pub(super) fn paint_menu_row(
    ui: &mut egui::Ui,
    width: f32,
    arrow_x: f32,
    icon: MenuIcon,
    name: &str,
    name_col: egui::Color32,
    trail: RowT,
) -> (bool, bool, egui::Rect) {
    // Interaction is a REAL frameless Button so egui's menu recognises it as an
    // item (keeps the menu open through the click, fires `clicked()` on release);
    // a bare allocated rect makes egui close the menu on press → clicks are lost.
    // §3: the highlight spans the FRAME's own inner x-range (`max_rect`), not the
    // row's w-wide rect, so it reaches both borders. `ui.set_width(w)` upstream
    // makes the two equal → the highlight physically touches both inner borders.
    let hl_x = ui.max_rect().x_range();
    let resp = ui.add_sized(
        egui::vec2(width, MENU_ROW_H),
        egui::Button::new("").frame(false),
    );
    let rect = resp.rect;
    let band = egui::Rect::from_x_y_ranges(hl_x, rect.y_range());
    let p = ui.painter_at(band);
    if resp.hovered() {
        p.rect_filled(band, 0.0, crate::theme::color::SURFACE_2);
    }

    // Icon — uniform thin muted glyph, box = ROW_H − 6, aligned column.
    let g_ink = crate::theme::color::TEXT_MUTED;
    let g_pen = egui::Stroke::new(1.0, g_ink);
    let ibox = MENU_ROW_H - 6.0;
    let gc = egui::pos2(rect.left() + MENU_PAD + MENU_ICON / 2.0, rect.center().y);
    let dot = |q: egui::Pos2| {
        p.circle_filled(q, 1.1, g_ink);
    };
    match icon {
        MenuIcon::Draw(s) => draw_draw_glyph(&p, gc, s, g_pen, dot, g_ink, ibox / GLYPH_BOX_DRAW),
        MenuIcon::Cmd(k) => draw_cmd_glyph(&p, gc, k, g_pen, dot, g_ink, ibox / GLYPH_BOX_CMD),
        MenuIcon::Method(cmd, key) => {
            draw_method_glyph(&p, gc, cmd, key, g_pen, g_ink, ibox / GLYPH_BOX_METHOD)
        }
        MenuIcon::Qat(act) => paint_qat_icon(
            &p,
            egui::Rect::from_center_size(gc, egui::vec2(ibox, ibox)),
            act,
            g_ink,
        ),
        MenuIcon::Check(on) => {
            // §8 toggle checkbox — the Inspector component (INSPECTOR_DESIGN §5 /
            // THEME §5.10): a 16×16 r4 box ALWAYS rendered in the icon slot.
            //   OFF → surface-0 fill + 1px muted `border` (tells the user it's a
            //         toggle that can receive a check — never a blank slot).
            //   ON  → same box in cyan `accent` with an on-accent check glyph.
            let bx = egui::Rect::from_center_size(gc, egui::vec2(16.0, 16.0));
            let r4 = egui::Rounding::same(crate::theme::radius::SM);
            if on {
                p.rect(bx, r4, PP_ACCENT, egui::Stroke::new(1.0, PP_ACCENT));
                let oa = crate::theme::color::ON_ACCENT;
                let cy = bx.center().y;
                p.line_segment(
                    [
                        egui::pos2(bx.left() + 3.5, cy + 0.5),
                        egui::pos2(bx.left() + 6.5, cy + 3.5),
                    ],
                    egui::Stroke::new(1.6, oa),
                );
                p.line_segment(
                    [
                        egui::pos2(bx.left() + 6.5, cy + 3.5),
                        egui::pos2(bx.right() - 3.0, cy - 3.5),
                    ],
                    egui::Stroke::new(1.6, oa),
                );
            } else {
                p.rect(bx, r4, PP_BG_LO, egui::Stroke::new(1.0, PP_BORDER));
            }
        }
        MenuIcon::None => {}
    }

    // Name, then any LINE trailing right after it (Code cyan / Hint muted, §1/§5).
    let name_x = rect.left() + MENU_PAD + MENU_ICON + MENU_ICON_GAP;
    let nr = p.text(
        egui::pos2(name_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        crate::theme::typ::body(),
        name_col,
    );
    match trail {
        RowT::Code(c, col) | RowT::CodeArrow(c, col) => {
            p.text(
                egui::pos2(nr.right() + MENU_CODE_GAP, rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("({})", c),
                crate::theme::typ::data_code(),
                col,
            );
        }
        RowT::Hint(h) => {
            p.text(
                egui::pos2(nr.right() + MENU_CODE_GAP, rect.center().y),
                egui::Align2::LEFT_CENTER,
                h,
                crate::theme::typ::body(),
                crate::theme::color::TEXT_MUTED,
            );
        }
        _ => {}
    }

    // ZONE trailing (§1): a ▸ on the aligned arrow column, OR a right-aligned
    // shortcut at the right inner edge. Never both. Shortcuts render in Geist
    // sans (`typ::hint`, muted) — Mono is reserved for (CODE) + numbers (§1).
    let has_arrow = matches!(trail, RowT::Arrow | RowT::CodeArrow(..));
    if has_arrow {
        let ax = rect.left() + arrow_x;
        let cy = rect.center().y;
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(ax, cy - 3.5),
                egui::pos2(ax + 5.0, cy),
                egui::pos2(ax, cy + 3.5),
            ],
            crate::theme::color::TEXT_MUTED,
            egui::Stroke::NONE,
        ));
    }
    if let RowT::Shortcut(sc) = trail {
        p.text(
            egui::pos2(rect.right() - MENU_PAD, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            sc,
            crate::theme::typ::hint(),
            crate::theme::color::TEXT_MUTED,
        );
    }

    // The ▸ opens the flyout on HOVER (§2); the row's CLICK runs the command.
    let arrow_hovered = has_arrow
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|q| rect.contains(q) && q.x >= rect.left() + arrow_x - MENU_CODE_GAP);
    (resp.clicked(), arrow_hovered, rect)
}

/// The ONE hug-geometry source for every category dropdown + flyout (MENU_DROPDOWN
/// §1/§2/§7): from each row's `(name, RowT)`, return `(arrow_x, width)` — the shared
/// arrow column and the hug width. `line` = the longest icon→name(+CODE/hint) run
/// (arrow column = line + gap, §2); `zone` = the widest trailing element (arrow OR
/// shortcut). Every menu calls this so all hug + align identically, no per-menu
/// math. Call under the popup `ui` (needs its fonts).
pub(super) fn menu_hug_geometry(ui: &egui::Ui, rows: &[(&str, RowT)]) -> (f32, f32) {
    let fn_ = crate::theme::typ::body(); // name — Geist 13
    let fc_ = crate::theme::typ::data_code(); // (CODE) — Mono 11
    let fh_ = crate::theme::typ::hint(); // shortcut — Geist 11 (§1)
    let nw = |s: &str| {
        ui.fonts(|f| {
            f.layout_no_wrap(s.to_string(), fn_.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    let cw = |s: &str| {
        ui.fonts(|f| {
            f.layout_no_wrap(s.to_string(), fc_.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    let hw = |s: &str| {
        ui.fonts(|f| {
            f.layout_no_wrap(s.to_string(), fh_.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    let mut line = 0.0_f32; // icon → end of name (+ any left-grouped CODE/hint)
    let mut has_arrow = false; // any submenu ▸ in this menu
    let mut max_sc = 0.0_f32; // widest right-aligned shortcut
    for (name, trail) in rows {
        let lt = match trail {
            RowT::Code(c, _) | RowT::CodeArrow(c, _) => MENU_CODE_GAP + cw(&format!("({})", c)),
            RowT::Hint(h) => MENU_CODE_GAP + nw(h),
            _ => 0.0,
        };
        line = line.max(nw(name) + lt);
        match trail {
            RowT::Arrow | RowT::CodeArrow(..) => has_arrow = true,
            RowT::Shortcut(s) => max_sc = max_sc.max(hw(s)),
            _ => {}
        }
    }
    // `base` = right edge of the longest full line. The submenu-arrow column sits a
    // WIDE gap past it (§2: line + 32); shortcuts keep the small 6 gap and right-align
    // to the 12 right pad — so the arrow gap and shortcut spacing are independent.
    let base = MENU_PAD + MENU_ICON + MENU_ICON_GAP + line;
    let arrow_x = base + MENU_ARROW_GAP;
    let mut right = base;
    if has_arrow {
        right = right.max(arrow_x + MENU_ARROW_W);
    }
    if max_sc > 0.0 {
        right = right.max(base + MENU_CODE_GAP + max_sc);
    }
    (arrow_x, right + MENU_PAD)
}

/// Open a custom-painted category dropdown (MENU_DROPDOWN chrome, ONE place):
/// zero the popup's horizontal inner margin so a full-width row reaches the frame
/// border (edge-to-edge hover, §3) — saved + restored on the menubar `ui` around
/// the button — and flush the row spacing to 0. The `body` then measures via
/// `menu_hug_geometry`, `ui.set_width(w)`, and paints rows with `paint_menu_row`.
/// A menu button that opens on CLICK and never switches on hover.
///
/// Reported as: "while selecting something from the drop down menu, when i move the cursor it
/// opens the menu of whatever is below it."
///
/// egui's `menu_button` opens a menu when `button.clicked() || (button.hovered() && some other
/// menu is open)` — `stationary_interaction`, egui 0.30 `menu.rs:386`. Hover-switching is right
/// for a single-row menu bar. These bars WRAP, so the next row of buttons sits directly beneath
/// the one you opened, and `style.spacing.menu_spacing` leaves a few pixels of it exposed between
/// the button and its own panel. Moving the cursor straight down into the panel crosses that strip
/// and swaps the menu out from under you.
///
/// That rule reads the BUTTON's own `hovered`, so clearing it removes hover-switching and nothing
/// else: clicking still opens, clicking again still closes, and the panel behaves normally once
/// open. Every button shares one bar id, so opening one still closes the last.
///
/// The layout is deliberately left alone. The first attempt at this made the bar a single
/// scrolling row — which removed the second row, and with it the symptom, by changing something
/// nobody asked to change.
pub(crate) fn click_menu_button<R>(
    ui: &mut egui::Ui,
    title: impl Into<egui::WidgetText>,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    // ONE id for the whole bar: `ui.id()` belongs to the wrapping layout and is shared by every
    // button in it, so they behave as one menu bar rather than as N independent popups.
    let bar_id = ui.id().with("click_only_menu_bar");
    let mut state = egui::menu::BarState::load(ui.ctx(), bar_id);
    // The response given to `bar_menu` has its hover cleared; the one handed BACK to the caller
    // keeps it, so `.on_hover_text(…)` on these buttons still works.
    let resp = ui.button(title);
    let mut gated = resp.clone();
    gated.hovered = false; // ← the whole fix
    let inner = state.bar_menu(&gated, body).map(|ir| ir.inner);
    state.store(ui.ctx(), bar_id);
    egui::InnerResponse::new(inner, resp)
}

pub(super) fn custom_menu(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let saved_mm = ui.style().spacing.menu_margin;
    ui.style_mut().spacing.menu_margin.left = 0.0;
    ui.style_mut().spacing.menu_margin.right = 0.0;
    ui.menu_button(title, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        body(ui);
    });
    ui.style_mut().spacing.menu_margin = saved_mm; // restore for native menus
}

/// A group-divider hairline spanning the menu's full content width (MENU_DROPDOWN
/// §5) — 1px `border`, edge-to-edge like the hover (painted at `max_rect().x`),
/// with a little vertical breathing room. `w` sizes the allocated spacer row.
pub(super) fn menu_divider(ui: &mut egui::Ui, w: f32) {
    let x = ui.max_rect().x_range();
    let (dr, _) = ui.allocate_exact_size(egui::vec2(w, 5.0), egui::Sense::hover());
    ui.painter().hline(
        x,
        dr.center().y,
        egui::Stroke::new(1.0, crate::theme::color::BORDER),
    );
}

/// §8 section heading — a non-interactive caption row (`PALETTES`, `INQUIRY`, …):
/// Geist-Medium **11/500 UPPERCASE**, dim (`#66707A`), at the NAME column (icon slot
/// empty), no hover. The caller puts a `menu_divider` above it (except the first).
pub(super) fn menu_heading_row(ui: &mut egui::Ui, w: f32, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, MENU_ROW_H - 4.0), egui::Sense::hover());
    let name_x = rect.left() + MENU_PAD + MENU_ICON + MENU_ICON_GAP;
    ui.painter_at(rect).text(
        egui::pos2(name_x, rect.center().y + 1.0),
        egui::Align2::LEFT_CENTER,
        text.to_uppercase(),
        crate::theme::typ::caption(),
        egui::Color32::from_rgb(0x66, 0x70, 0x7A),
    );
}

/// §7 — the ONE icon source, keyed by a stable command key. Returns the row's icon
/// SLOT so a menu NEVER hardcodes a glyph inline; a later icon = one entry here, a
/// full redesign = swap the glyph bodies behind these keys. Existing toolbar art
/// (New/Open/Save/Undo/Redo) is reused via `Qat`. Missing → `None` (reserved slot).
/// Method commands aren't keyed here — their row builds `MenuIcon::Method(cmd, key)`
/// with the runtime current method (method-aware, §1); the glyph itself still lives
/// in one place (`draw_method_glyph`).
pub(super) fn icon_for(key: &str) -> MenuIcon<'static> {
    match key {
        // File — reuse the QAT toolbar glyphs.
        "file.new" => MenuIcon::Qat(QatAction::New),
        "file.open" => MenuIcon::Qat(QatAction::Open),
        "file.save" => MenuIcon::Qat(QatAction::Save),
        "file.saveas" => MenuIcon::Qat(QatAction::SaveAs),
        // Draw.
        "draw.line" => MenuIcon::Draw("line"),
        "draw.rectangle" => MenuIcon::Draw("rect"),
        "draw.ellipse" => MenuIcon::Draw("ellipse"),
        "draw.ellipsearc" => MenuIcon::Draw("ellarc"),
        "draw.pline" => MenuIcon::Draw("pline"),
        "draw.spline" => MenuIcon::Draw("spline"),
        "draw.point" => MenuIcon::Draw("point"),
        "draw.hatch" => MenuIcon::Draw("hatch"),
        "draw.wall" => MenuIcon::Draw("wall"),
        "draw.block" => MenuIcon::Cmd(GlyphKind::Block),
        "draw.insert" => MenuIcon::Cmd(GlyphKind::Insert),
        // Edit.
        "edit.undo" => MenuIcon::Cmd(GlyphKind::Undo),
        "edit.redo" => MenuIcon::Cmd(GlyphKind::Redo),
        "edit.copy" => MenuIcon::Cmd(GlyphKind::Copy),
        "edit.erase" => MenuIcon::Cmd(GlyphKind::Erase),
        "edit.matchprop" => MenuIcon::Cmd(GlyphKind::MatchProps),
        // Modify.
        "modify.move" => MenuIcon::Cmd(GlyphKind::Move),
        "modify.copy" => MenuIcon::Cmd(GlyphKind::Copy),
        "modify.rotate" => MenuIcon::Cmd(GlyphKind::Rotate),
        "modify.scale" => MenuIcon::Cmd(GlyphKind::Scale),
        "modify.mirror" => MenuIcon::Cmd(GlyphKind::Mirror),
        "modify.stretch" => MenuIcon::Cmd(GlyphKind::Stretch),
        "modify.align" => MenuIcon::Cmd(GlyphKind::Align),
        "modify.trim" => MenuIcon::Cmd(GlyphKind::Trim),
        "modify.extend" => MenuIcon::Cmd(GlyphKind::Extend),
        "modify.chamfer" => MenuIcon::Cmd(GlyphKind::Chamfer),
        "modify.offset" => MenuIcon::Cmd(GlyphKind::Offset),
        "modify.join" => MenuIcon::Cmd(GlyphKind::Join),
        "modify.break" => MenuIcon::Cmd(GlyphKind::Break),
        "modify.lengthen" => MenuIcon::Cmd(GlyphKind::Lengthen),
        "modify.reverse" => MenuIcon::Cmd(GlyphKind::Reverse),
        "modify.array" => MenuIcon::Cmd(GlyphKind::ArrayGrid),
        "modify.explode" => MenuIcon::Cmd(GlyphKind::Explode),
        "modify.matchprop" => MenuIcon::Cmd(GlyphKind::MatchProps),
        "modify.chlayer" => MenuIcon::Cmd(GlyphKind::ChangeLayer),
        "modify.erase" => MenuIcon::Cmd(GlyphKind::Erase),
        // Utilities / inquiry.
        "util.dist" => MenuIcon::Cmd(GlyphKind::Dist),
        "util.list" => MenuIcon::Cmd(GlyphKind::List),
        // Formative.
        "formative.dimension" => MenuIcon::Draw("dim"),
        _ => MenuIcon::None,
    }
}

// ---- generalized top-level flyouts (§9) + command / tool-button glyphs ----

impl CadApp {
    /// Open (or re-anchor) a top-level flyout `menu` off `anchor` (§9). Called from a
    /// menubar row's ▸ hover. If the SAME base menu is already open, only its anchor
    /// updates (its child submenu is preserved); a different base resets the stack.
    pub(super) fn open_flyout(&mut self, menu: FlyMenu, anchor: egui::Rect) {
        match self.menu_flyouts.first() {
            Some(f) if f.menu == menu => {
                self.menu_flyouts[0].anchor = anchor;
            }
            _ => {
                self.menu_flyouts = vec![FlyFrame { menu, anchor }];
            }
        }
        self.menu_flyout_hot = true;
    }

    /// The rows a flyout shows — BUILT FRESH from `&self` each frame (§9) so dynamic
    /// content (block names, current method/style, toggle state, live labels) is
    /// always current. Pure data; a click's effect lives in `flyout_activate`.
    fn flyout_items(&self, menu: &FlyMenu) -> Vec<FlyItem> {
        let mut_ = crate::theme::color::TEXT_MUTED;
        match menu {
            FlyMenu::Method(cmd) => {
                let cur = self.current_method_key(cmd);
                rail_flyout_items(cmd)
                    .into_iter()
                    .map(|(short, full, key)| {
                        let is_cur = key == cur; // §5: current method row = cyan
                        FlyItem::Row {
                            icon: FlyIcon::Method(cmd.clone(), key.clone()),
                            name: full,
                            col: if is_cur { PP_ACCENT } else { PP_TEXT },
                            code: Some((short, if is_cur { PP_ACCENT } else { mut_ })),
                            check: None,
                            submenu: None,
                            act: Some(FlyAct::Method(cmd.clone(), key)),
                        }
                    })
                    .collect()
            }
            FlyMenu::Insert => {
                let names: Vec<String> = self
                    .doc
                    .blocks
                    .blocks
                    .iter()
                    .map(|b| b.name.clone())
                    .collect();
                if names.is_empty() {
                    return vec![FlyItem::Disabled("(no blocks defined)".into())];
                }
                names
                    .into_iter()
                    .map(|n| {
                        FlyItem::act(FlyIcon::Key("draw.block"), &n, FlyAct::Insert(n.clone()))
                    })
                    .collect()
            }
            FlyMenu::Import => vec![
                FlyItem::act(
                    FlyIcon::None,
                    "Image as raster (underlay)…",
                    FlyAct::Import(true),
                ),
                FlyItem::act(
                    FlyIcon::None,
                    "Image → vector (trace editor)…",
                    FlyAct::Import(false),
                ),
            ],
            FlyMenu::Export => vec![
                FlyItem::act(FlyIcon::None, "PDF…", FlyAct::Export("pdf".into())),
                FlyItem::act(FlyIcon::None, "SVG…", FlyAct::Export("svg".into())),
                FlyItem::act(
                    FlyIcon::None,
                    "PNG (300 dpi)…",
                    FlyAct::Export("png".into()),
                ),
            ],
            FlyMenu::PlotStyleTables => {
                // CTB manager: New / built-ins / saved .pst tables + Plot Style
                // Table Editor. The saved list is rebuilt every frame from the
                // app's ctb folder (see `ctb_list`).
                let saved = ctb_list();
                let mut items = vec![
                    FlyItem::Heading("Plot Style Tables".into()),
                    FlyItem::act(FlyIcon::None, "New CTB…", FlyAct::NewCtb),
                    FlyItem::act(
                        FlyIcon::None,
                        "Edit current table…",
                        FlyAct::Run("plotstyle".into()),
                    ),
                    FlyItem::Divider,
                ];
                for (builtin, key) in CTB_BUILTINS.iter().filter_map(|(d, k)| k.map(|k| (d, k))) {
                    items.push(FlyItem::act(
                        FlyIcon::None,
                        builtin,
                        FlyAct::OpenCtb(key.to_string()),
                    ));
                }
                if !saved.is_empty() {
                    items.push(FlyItem::Divider);
                    for (name, path) in &saved {
                        items.push(FlyItem::act(
                            FlyIcon::None,
                            name,
                            FlyAct::OpenCtbFile(path.clone()),
                        ));
                    }
                }
                items.push(FlyItem::Divider);
                items.push(FlyItem::Disabled(
                    "Delete CTB…  (select a table first)".into(),
                ));
                items
            }
            FlyMenu::Dimension => vec![
                FlyItem::act(
                    FlyIcon::Key("formative.dimension"),
                    "Dimension  (smart: linear · radius · diameter)",
                    FlyAct::Run("dim".into()),
                ),
                FlyItem::Divider,
                FlyItem::act(
                    FlyIcon::None,
                    "Dimension Style…",
                    FlyAct::Run("dimstyle".into()),
                ),
            ],
            FlyMenu::Styles => {
                let dimn = self
                    .doc
                    .dim_styles
                    .styles
                    .get(self.current_dim_style as usize)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "STANDARD".into());
                let walln = self
                    .doc
                    .wall_styles
                    .styles
                    .get(self.current_wall_style as usize)
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "STANDARD".into());
                vec![
                    FlyItem::Heading("Managers".into()),
                    FlyItem::act(FlyIcon::None, "Text Style…", FlyAct::Run("style".into())),
                    FlyItem::act(
                        FlyIcon::None,
                        "Dimension Style…",
                        FlyAct::Run("dimstyle".into()),
                    ),
                    FlyItem::act(
                        FlyIcon::None,
                        "Wall Style…",
                        FlyAct::Run("wallstyle".into()),
                    ),
                    FlyItem::Divider,
                    FlyItem::Heading("Current".into()),
                    FlyItem::sub(
                        FlyIcon::None,
                        &format!("Dim style:  {}", dimn),
                        FlyMenu::DimStylePick,
                    ),
                    FlyItem::sub(
                        FlyIcon::None,
                        &format!("Wall style:  {}", walln),
                        FlyMenu::WallStylePick,
                    ),
                    FlyItem::Divider,
                    FlyItem::Disabled("Opening Style…  (planned)".into()),
                ]
            }
            FlyMenu::DimStylePick => self
                .doc
                .dim_styles
                .styles
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let cur = i as u32 == self.current_dim_style;
                    FlyItem::Row {
                        icon: FlyIcon::None,
                        name: s.name.clone(),
                        col: if cur { PP_ACCENT } else { PP_TEXT },
                        code: None,
                        check: None,
                        submenu: None,
                        act: Some(FlyAct::SetDimStyle(i as u32)),
                    }
                })
                .collect(),
            FlyMenu::WallStylePick => self
                .doc
                .wall_styles
                .styles
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let cur = i as u32 == self.current_wall_style;
                    FlyItem::Row {
                        icon: FlyIcon::None,
                        name: s.name.clone(),
                        col: if cur { PP_ACCENT } else { PP_TEXT },
                        code: None,
                        check: None,
                        submenu: None,
                        act: Some(FlyAct::SetWallStyle(i as u32)),
                    }
                })
                .collect(),
            FlyMenu::Debug => {
                let idx_fresh = !(self.index_dirty || self.index.is_none());
                let idx_label = if idx_fresh {
                    "Spatial index ✓ (fresh)"
                } else {
                    "Rebuild spatial index ⟲"
                };
                let click_label = if self.intersect_pending_click {
                    "∩ click — waiting for click…"
                } else {
                    "∩ click — arm for next click"
                };
                vec![
                    FlyItem::toggle(
                        "Screen Stats",
                        self.screen_stats_open,
                        FlyAct::ToggleBool(BoolField::ScreenStats),
                    ),
                    FlyItem::toggle(
                        "Render mode (CPU/GPU + APX)",
                        self.debug_open,
                        FlyAct::ToggleBool(BoolField::Debug),
                    ),
                    FlyItem::toggle(
                        "Trim Debug Log",
                        self.trim_debug_open,
                        FlyAct::ToggleBool(BoolField::TrimDebug),
                    ),
                    FlyItem::toggle(
                        "Hatch Debug Log",
                        self.hatch_debug_open,
                        FlyAct::ToggleBool(BoolField::HatchDebug),
                    ),
                    FlyItem::toggle(
                        "UI inspect (element sizes)",
                        self.ui_inspect,
                        FlyAct::ToggleBool(BoolField::UiInspect),
                    ),
                    FlyItem::toggle(
                        "Command registry dump",
                        self.cmd_dump_open,
                        FlyAct::ToggleBool(BoolField::CmdDump),
                    ),
                    FlyItem::Divider,
                    FlyItem::act(FlyIcon::None, idx_label, FlyAct::EnsureIndex),
                    FlyItem::Divider,
                    FlyItem::Heading("Intersect visualizer".into()),
                    FlyItem::act(
                        FlyIcon::None,
                        "∩ view (whole viewport)",
                        FlyAct::IntersectView,
                    ),
                    FlyItem::act(FlyIcon::None, click_label, FlyAct::IntersectArm),
                    FlyItem::act(FlyIcon::None, "Clear ∩ overlay", FlyAct::IntersectClear),
                    FlyItem::Divider,
                    FlyItem::act(
                        FlyIcon::None,
                        "Clear all dobjects (DESTRUCTIVE)",
                        FlyAct::ClearAll,
                    ),
                ]
            }
            FlyMenu::Scripts => {
                // WP-SCRIPT: one row per scripts/*.py (menu access runs without
                // args — `run <name> [args…]` on the command line passes inputs).
                if self.py_examples.is_empty() {
                    vec![
                        FlyItem::Disabled(
                            "(no scripts in scripts/ — save one from the Python console)".into(),
                        ),
                        FlyItem::Divider,
                        FlyItem::act(FlyIcon::None, "Script editor…", FlyAct::OpenScriptEditor),
                        FlyItem::act(FlyIcon::None, "Scripting guide…", FlyAct::OpenScriptingDoc),
                        FlyItem::act(FlyIcon::None, "Python console", FlyAct::Run("py".into())),
                        FlyItem::act(FlyIcon::None, "Reload script list", FlyAct::RescanScripts),
                    ]
                } else {
                    let mut items: Vec<FlyItem> = self
                        .py_examples
                        .iter()
                        .map(|(n, _)| FlyItem::act(FlyIcon::None, n, FlyAct::RunScript(n.clone())))
                        .collect();
                    items.push(FlyItem::Divider);
                    items.push(FlyItem::act(
                        FlyIcon::None,
                        "Script editor…",
                        FlyAct::OpenScriptEditor,
                    ));
                    items.push(FlyItem::act(
                        FlyIcon::None,
                        "Scripting guide…",
                        FlyAct::OpenScriptingDoc,
                    ));
                    items.push(FlyItem::act(
                        FlyIcon::None,
                        "Python console",
                        FlyAct::Run("py".into()),
                    ));
                    items.push(FlyItem::act(
                        FlyIcon::None,
                        "Reload script list",
                        FlyAct::RescanScripts,
                    ));
                    items
                }
            }
            // Command-bar grip right-click: docked strip, so just Close (reopen
            // via Tools → Command line).
            FlyMenu::CommandBar => vec![FlyItem::act(
                FlyIcon::None,
                "Close command bar",
                FlyAct::CloseCommandBar,
            )],
        }
    }

    /// Commit a flyout row's action (§9). Pure dispatch — `&mut self` here, so the
    /// data-only `FlyAct` resolves against live state.
    fn flyout_activate(&mut self, act: FlyAct) {
        match act {
            FlyAct::Run(cmd) => self.run_command(&cmd),
            FlyAct::Method(cmd, key) => self.dispatch_method(&cmd, &key),
            FlyAct::Insert(n) => self.run_command(&format!("insert {}", n)),
            FlyAct::Import(raster) => self.open_file_dialog(
                if raster {
                    FileDialogMode::ImportRaster
                } else {
                    FileDialogMode::ImportImage
                },
                "",
            ),
            FlyAct::Export(fmt) => {
                // Quick export: whole drawing straight to the Save dialog, then
                // write + open via the shared run_plot path (same flow as the
                // Plot dialog's own OK — the dialog's format choice wins, so
                // the ext is only the default).
                self.plot_pdf_browse = true;
                self.plot_run_after_save = true;
                let ext = match fmt.as_str() {
                    "svg" => "svg",
                    "png" => "png",
                    _ => "pdf",
                };
                self.open_file_dialog(FileDialogMode::Save, &format!(".{}", ext));
            }
            FlyAct::NewCtb => {
                let saved = ctb_list();
                self.new_ctb_name = format!("CTB {}", saved.len() + 1);
                self.new_ctb_open = true;
            }
            FlyAct::OpenCtb(name) => self.ctb_open_builtin(&name),
            FlyAct::OpenCtbFile(path) => self.ctb_open_editor(&path),
            FlyAct::SetDimStyle(id) => {
                let name = self
                    .doc
                    .dim_styles
                    .styles
                    .get(id as usize)
                    .map(|s| s.name.clone());
                self.current_dim_style = id;
                if let Some(n) = name {
                    self.history
                        .push(format!("  dim style: '{}' set current", n));
                }
            }
            FlyAct::SetWallStyle(id) => {
                // Capture thickness + name in ONE immutable borrow, then mutate env
                // (the same WlThk sync the Wall Style Manager's Set Current does).
                let info = self
                    .doc
                    .wall_styles
                    .get(id)
                    .map(|s| (s.thickness, s.name.clone()));
                self.current_wall_style = id;
                if let Some((thk, name)) = info {
                    self.env.WlThk = thk;
                    let _ = self.env.save();
                    self.history
                        .push(format!("  wall style: '{}' set current", name));
                }
            }
            FlyAct::ToggleBool(f) => {
                let b = match f {
                    BoolField::ScreenStats => &mut self.screen_stats_open,
                    BoolField::Debug => &mut self.debug_open,
                    BoolField::TrimDebug => &mut self.trim_debug_open,
                    BoolField::HatchDebug => &mut self.hatch_debug_open,
                    BoolField::UiInspect => &mut self.ui_inspect,
                    BoolField::CmdDump => &mut self.cmd_dump_open,
                };
                *b = !*b;
            }
            FlyAct::EnsureIndex => {
                self.ensure_index();
            }
            FlyAct::IntersectView => self.intersect_view_pending = true,
            FlyAct::IntersectArm => self.intersect_pending_click = !self.intersect_pending_click,
            FlyAct::IntersectClear => {
                self.intersections.clear();
                self.last_intersect_label.clear();
            }
            FlyAct::ClearAll => {
                self.clear_all();
                self.history.push("  cleared".into());
            }
            FlyAct::RunScript(name) => self.run_script_command(Some(name), Vec::new()),
            FlyAct::RescanScripts => self.scan_py_examples(),
            FlyAct::OpenScriptEditor => self.py_editor_open_panel(),
            FlyAct::OpenScriptingDoc => self.open_scripting_doc(),
            FlyAct::CloseCommandBar => self.cmd_window_open = false,
        }
    }

    /// Render the open flyout STACK at the TOP LEVEL (§9) — independent of the parent
    /// dropdown so it survives egui auto-closing the dropdown on the ▸, and captures
    /// its own clicks (no leak to the canvas). Every frame is the SAME custom rows;
    /// submenu ▸-hover opens a child frame; a click commits + closes; leaving the
    /// whole stack for `MENU_FLYOUT_CLOSE_DELAY` closes it (travel-tolerant, §2).
    pub(crate) fn render_menu_flyouts(&mut self, ctx: &egui::Context) {
        if self.menu_flyouts.is_empty() {
            self.menu_flyout_hot = false;
            return;
        }
        let frames = self.menu_flyouts.clone();
        let mut frame_rects: Vec<egui::Rect> = Vec::with_capacity(frames.len());
        // child[i] = the submenu (menu, anchor) whose ▸ is hovered in frame i.
        let mut child: Vec<Option<(FlyMenu, egui::Rect)>> = vec![None; frames.len()];
        let mut action: Option<FlyAct> = None;

        for (i, frame) in frames.iter().enumerate() {
            let anchor = frame.anchor;
            let area = egui::Area::new(egui::Id::new(("menu_flyout", i)))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(anchor.right() + 2.0, anchor.top()))
                .constrain(true)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .rounding(egui::Rounding::same(crate::theme::radius::SM))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                            let items = self.flyout_items(&frame.menu);
                            let rows: Vec<(&str, RowT)> =
                                items.iter().map(|it| it.meas()).collect();
                            let (arrow_x, w) = menu_hug_geometry(ui, &rows);
                            ui.set_width(w);
                            for it in &items {
                                match it {
                                    FlyItem::Divider => menu_divider(ui, w),
                                    FlyItem::Heading(t) => menu_heading_row(ui, w, t),
                                    FlyItem::Disabled(t) => {
                                        let _ = paint_menu_row(
                                            ui,
                                            w,
                                            arrow_x,
                                            MenuIcon::None,
                                            t,
                                            crate::theme::color::TEXT_MUTED,
                                            RowT::Plain,
                                        );
                                    }
                                    FlyItem::Row {
                                        icon,
                                        name,
                                        col,
                                        code,
                                        check,
                                        submenu,
                                        act,
                                    } => {
                                        let micon = if let Some(on) = check {
                                            MenuIcon::Check(*on)
                                        } else {
                                            match icon {
                                                FlyIcon::Key(k) => icon_for(k),
                                                FlyIcon::Method(c, k) => MenuIcon::Method(c, k),
                                                FlyIcon::None => MenuIcon::None,
                                            }
                                        };
                                        let rowt = match (code, submenu.is_some()) {
                                            (Some((c, cc)), true) => RowT::CodeArrow(c, *cc),
                                            (Some((c, cc)), false) => RowT::Code(c, *cc),
                                            (None, true) => RowT::Arrow,
                                            (None, false) => RowT::Plain,
                                        };
                                        let (clk, ahov, rrect) =
                                            paint_menu_row(ui, w, arrow_x, micon, name, *col, rowt);
                                        if let Some(sm) = submenu {
                                            if ahov {
                                                child[i] = Some((sm.clone(), rrect));
                                            }
                                        }
                                        if clk {
                                            if let Some(a) = act {
                                                if action.is_none() {
                                                    action = Some(a.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            pp_cap_ui(ui, "menu flyout");
                        });
                });
            frame_rects.push(area.response.rect);
        }

        let ptr = ctx.pointer_hover_pos();
        let over_any = ptr.map_or(false, |p| frame_rects.iter().any(|r| r.contains(p)));
        let arrow_any = child.iter().any(|c| c.is_some());

        // Rebuild the stack from arrow-hovers: keep frame 0 (menubar-spawned), extend
        // with any hovered submenu child (§9 nesting).
        let mut new_stack: Vec<FlyFrame> = vec![frames[0].clone()];
        for i in 0..frames.len() {
            if i + 1 > new_stack.len() {
                break;
            }
            if let Some((menu, anchor)) = &child[i] {
                new_stack.truncate(i + 1);
                new_stack.push(FlyFrame {
                    menu: menu.clone(),
                    anchor: *anchor,
                });
            }
        }
        // Travel tolerance: if no new arrow extended but the pointer is over a deeper
        // existing frame, keep the deeper frames (don't trim while traversing them).
        if new_stack.len() < frames.len() {
            let deeper = ptr.map_or(false, |p| {
                frame_rects[new_stack.len()..].iter().any(|r| r.contains(p))
            });
            if deeper {
                new_stack = frames.clone();
            }
        }

        // Global close: pointer off everything, no top-arrow / submenu-arrow hovered.
        let mut close_all = false;
        if self.menu_flyout_hot || over_any || arrow_any {
            self.menu_flyout_leave_t = None;
        } else {
            let now = ctx.input(|i| i.time);
            match self.menu_flyout_leave_t {
                Some(t) if now - t >= MENU_FLYOUT_CLOSE_DELAY => close_all = true,
                _ => {
                    if self.menu_flyout_leave_t.is_none() {
                        self.menu_flyout_leave_t = Some(now);
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(60));
                }
            }
        }
        self.menu_flyout_hot = false; // re-armed by the menubar each frame

        if let Some(a) = action {
            self.flyout_activate(a);
            close_all = true;
        }
        if close_all {
            self.menu_flyouts.clear();
            self.menu_flyout_leave_t = None;
        } else {
            self.menu_flyouts = new_stack;
        }
    }
}

pub(super) fn draw_draw_glyph(
    p: &egui::Painter,
    c: egui::Pos2,
    id: &str,
    pen: egui::Stroke,
    dot: impl Fn(egui::Pos2),
    _ink: egui::Color32,
    scale: f32,
) {
    // `scale` maps the fn's natural box (`GLYPH_BOX_DRAW`) to the caller's icon
    // box so glyphs are one uniform physical size across surfaces (§4/§7). The
    // rail passes 1.0 (unchanged); palette/menus pass `icon_box / natural`.
    use egui::vec2;
    let v = |x: f32, y: f32| c + vec2(x * scale, y * scale);
    let poly = |pts: Vec<egui::Pos2>| {
        p.add(egui::Shape::line(pts, pen));
    };
    let ring = |cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32| {
        let n = 22;
        let mut pts = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let t = a0 + (a1 - a0) * (i as f32 / n as f32);
            pts.push(c + vec2((cx + rx * t.cos()) * scale, (cy - ry * t.sin()) * scale));
        }
        p.add(egui::Shape::line(pts, pen));
    };
    let tau = std::f32::consts::TAU;
    let rstroke = |a: egui::Pos2, b: egui::Pos2| {
        p.rect(
            egui::Rect::from_two_pos(a, b),
            0.0,
            egui::Color32::TRANSPARENT,
            pen,
        )
    };
    match id {
        "pointer" => {
            poly(vec![
                v(-5.0, -8.0),
                v(-5.0, 6.0),
                v(-1.0, 2.0),
                v(2.0, 8.0),
                v(4.0, 7.0),
                v(1.0, 1.0),
                v(6.0, 1.0),
                v(-5.0, -8.0),
            ]);
        }
        "line" => {
            p.line_segment([v(-7.0, 7.0), v(7.0, -7.0)], pen);
            dot(v(-7.0, 7.0));
            dot(v(7.0, -7.0));
        }
        "pline" => {
            poly(vec![v(-8.0, 5.0), v(-3.0, -3.0), v(2.0, 3.0), v(8.0, -5.0)]);
        }
        "circle" => {
            p.circle_stroke(c, 8.0 * scale, pen);
            dot(c);
        }
        "arc" => {
            ring(0.0, 4.0, 9.0, 9.0, 0.06 * tau, 0.44 * tau);
        }
        "rect" => {
            rstroke(v(-8.0, -5.0), v(8.0, 5.0));
        }
        "ellipse" => {
            ring(0.0, 0.0, 9.0, 5.5, 0.0, tau);
        }
        "point" => {
            p.line_segment([v(-6.0, 0.0), v(6.0, 0.0)], pen);
            p.line_segment([v(0.0, -6.0), v(0.0, 6.0)], pen);
            dot(c);
        }
        "spline" => {
            let n = 20;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let x = -9.0 + 18.0 * (i as f32 / n as f32);
                let y = 4.5 * (x * 0.55).sin();
                pts.push(v(x, -y));
            }
            poly(pts);
        }
        "hatch" => {
            rstroke(v(-8.0, -6.0), v(8.0, 6.0));
            for k in -1..=1 {
                let o = k as f32 * 5.0;
                p.line_segment([v(o - 1.0, 6.0), v(o + 5.0, -6.0)], pen);
            }
        }
        "text" => {
            poly(vec![v(-6.0, 8.0), v(0.0, -8.0), v(6.0, 8.0)]);
            p.line_segment([v(-3.5, 2.0), v(3.5, 2.0)], pen);
        }
        "ellarc" => {
            ring(0.0, 0.0, 9.0, 5.5, 0.12 * tau, 0.66 * tau);
        }
        "wall" => {
            rstroke(v(-9.0, -3.5), v(9.0, 3.5));
            p.line_segment([v(-9.0, 0.0), v(9.0, 0.0)], pen);
        }
        "dim" => {
            p.line_segment([v(-8.0, 3.0), v(-8.0, -5.0)], pen);
            p.line_segment([v(8.0, 3.0), v(8.0, -5.0)], pen);
            p.line_segment([v(-8.0, 0.0), v(8.0, 0.0)], pen);
            p.line_segment([v(-8.0, 0.0), v(-5.0, -2.0)], pen);
            p.line_segment([v(-8.0, 0.0), v(-5.0, 2.0)], pen);
            p.line_segment([v(8.0, 0.0), v(5.0, -2.0)], pen);
            p.line_segment([v(8.0, 0.0), v(5.0, 2.0)], pen);
        }
        _ => {}
    }
}

/// Method list for a rail command's ▼ flyout, as `(short, full, key)` triples.
/// `short` is the compact code shown next to the glyph (e.g. "3P", "TTR"),
/// `full` is the hover tooltip, `key` is interpreted by `dispatch_method`.
/// Empty = the command has no methods (no ▼ shown).
/// Command-palette fuzzy match: `true` if every char of `q` (already lowercased)
/// appears IN ORDER (subsequence) within `hay` (lowercased `title` + `keywords`).
/// Substring typing is a subset, so plain queries work too. Empty query matches
/// all. UI metadata search ONLY — never the parser or parser aliases (D6).
pub(super) fn palette_fuzzy(q: &str, hay: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let mut hc = hay.chars();
    'next: for qc in q.chars() {
        for c in hc.by_ref() {
            if c == qc {
                continue 'next;
            }
        }
        return false; // ran out of haystack before matching qc
    }
    true
}

pub(super) fn rail_flyout_items(cmd: &str) -> Vec<(String, String, String)> {
    match cmd {
        "arc" => ALL_ARC_METHODS
            .iter()
            .enumerate()
            .map(|(i, m)| (m.short().to_string(), m.name().to_string(), i.to_string()))
            .collect(),
        "circle" => vec![
            ("CR".into(), "Center, Radius".into(), "center".into()),
            ("3P".into(), "3-Point".into(), "3p".into()),
            ("2P".into(), "2-Point".into(), "2p".into()),
            ("TTR".into(), "Tan, Tan, Radius".into(), "ttr".into()),
        ],
        "fillet" => vec![
            ("F".into(), "Fillet (pick two)".into(), "run".into()),
            ("PL".into(), "Polyline (whole)".into(), "poly".into()),
            ("×N".into(), "Multiple".into(), "multi".into()),
        ],
        _ => Vec::new(),
    }
}

/// Draw a SMALL method-specific glyph (for the ▼ flyout rows and for the rail
/// icon once a method has been used). Compact (~7px) primitives so it reads at
/// flyout size. `cmd`+`key` select the variant; falls back to nothing if
/// unknown (caller then draws the base command glyph).
pub(super) fn draw_method_glyph(
    p: &egui::Painter,
    c: egui::Pos2,
    cmd: &str,
    key: &str,
    pen: egui::Stroke,
    ink: egui::Color32,
    scale: f32,
) {
    // `scale` maps the fn's natural box (`GLYPH_BOX_METHOD`) to the caller's icon
    // box (§4/§7). Rail passes 1.0; palette/menus pass `icon_box / natural`.
    use std::f32::consts::{PI, TAU};
    let v = |dx: f32, dy: f32| egui::pos2(c.x + dx * scale, c.y + dy * scale);
    let d = |q: egui::Pos2| p.circle_filled(q, 1.5 * scale.max(0.8), ink);
    match cmd {
        "circle" => {
            let r = 7.0;
            p.circle_stroke(c, r * scale, pen);
            match key {
                "3p" => {
                    for k in 0..3 {
                        let a = TAU * (k as f32 / 3.0) - PI / 2.0;
                        d(v(r * a.cos(), r * a.sin()));
                    }
                }
                "2p" => {
                    p.line_segment([v(-r, 0.0), v(r, 0.0)], pen);
                    d(v(-r, 0.0));
                    d(v(r, 0.0));
                }
                "ttr" => {
                    // circle nestled in a right-angle pair of tangents
                    p.line_segment([v(-9.0, 8.0), v(9.0, 8.0)], pen);
                    p.line_segment([v(-9.0, 8.0), v(-9.0, -8.0)], pen);
                }
                _ => {
                    d(c);
                    p.line_segment([c, v(r, 0.0)], pen);
                } // center+radius
            }
        }
        "arc" => {
            let n = 18;
            let pts: Vec<egui::Pos2> = (0..=n)
                .map(|i| {
                    let t = PI * (i as f32 / n as f32);
                    v(-8.0 * t.cos(), -8.0 * t.sin())
                })
                .collect();
            p.add(egui::Shape::line(pts.clone(), pen));
            match ALL_ARC_METHODS.get(key.parse::<usize>().unwrap_or(0)) {
                Some(ArcMethod::ThreePoints) => {
                    d(pts[0]);
                    d(pts[n / 2]);
                    d(pts[n]);
                }
                Some(ArcMethod::StartCenterEnd) | Some(ArcMethod::CenterStartEnd) => {
                    d(pts[0]);
                    d(c);
                    d(pts[n]);
                }
                _ => {
                    d(pts[0]);
                    d(pts[n]);
                }
            }
        }
        "fillet" => {
            // An L of two segments joined by a quarter-round corner.
            p.line_segment([v(-8.0, 7.0), v(-1.0, 7.0)], pen);
            p.line_segment([v(7.0, -8.0), v(7.0, -1.0)], pen);
            let q: Vec<egui::Pos2> = (0..=6)
                .map(|i| {
                    let t = (PI / 2.0) * (i as f32 / 6.0);
                    v(7.0 - 8.0 * t.cos(), 7.0 - 8.0 * t.sin())
                })
                .collect();
            p.add(egui::Shape::line(q, pen));
            match key {
                "poly" => {
                    d(v(-1.0, 7.0));
                    d(v(7.0, -1.0));
                } // emphasise both ends
                "multi" => {
                    p.line_segment([v(2.0, -6.0), v(6.0, -2.0)], pen);
                    p.line_segment([v(6.0, -6.0), v(2.0, -2.0)], pen);
                } // ×
                _ => {}
            }
        }
        _ => {}
    }
}

/// Placeholder glyphs for `cmd_button`. Each variant is a few
/// strokes — gets the idea across; we'll swap to real icons later.
/// `pub(crate)` so the command registry ([`crate::command::IconId`]) can name it.
#[derive(Copy, Clone, Debug)]
pub(crate) enum GlyphKind {
    Move,
    Copy,
    Rotate,
    Scale,
    Mirror,
    Stretch,
    Align,
    Trim,
    Extend,
    Fillet,
    Chamfer,
    Offset,
    Join,
    Break,
    Lengthen,
    Erase,
    MatchProps,
    ChangeLayer,
    ArrayGrid,
    Reverse,
    Dist,
    List,
    Undo,
    Redo,
    Block,
    Insert,
    Explode,
}

pub(super) fn draw_cmd_glyph(
    p: &egui::Painter,
    c: egui::Pos2,
    kind: GlyphKind,
    pen: egui::Stroke,
    dot: impl Fn(egui::Pos2),
    ink: egui::Color32,
    scale: f32,
) {
    // `scale` maps the fn's natural box (`GLYPH_BOX_CMD`) to the caller's icon box
    // (§4/§7). Rail passes 1.0; palette/menus pass `icon_box / natural`. All the
    // glyph geometry flows through `v`, so scaling it scales the whole icon.
    use egui::{vec2, Pos2 as _};
    let v = |x: f32, y: f32| c + vec2(x * scale, y * scale);
    match kind {
        GlyphKind::Move => {
            // four-headed arrow
            p.line_segment([v(-10.0, 0.0), v(10.0, 0.0)], pen);
            p.line_segment([v(0.0, -10.0), v(0.0, 10.0)], pen);
            for (dx, dy) in [(10.0, 0.0), (-10.0, 0.0), (0.0, 10.0), (0.0, -10.0)] {
                let tip = v(dx, dy);
                let (nx, ny) = (-dx * 0.3, -dy * 0.3);
                p.line_segment([tip, tip + vec2(nx - ny, ny + nx)], pen);
                p.line_segment([tip, tip + vec2(nx + ny, ny - nx)], pen);
            }
        }
        GlyphKind::Copy => {
            // two overlapping rectangles
            p.rect_stroke(
                egui::Rect::from_min_max(v(-11.0, -8.0), v(3.0, 6.0)),
                0.0,
                pen,
            );
            p.rect_stroke(
                egui::Rect::from_min_max(v(-3.0, -2.0), v(11.0, 10.0)),
                0.0,
                pen,
            );
        }
        GlyphKind::Rotate => {
            // 270° arc + arrowhead
            let r = 11.0;
            let mut pts = Vec::with_capacity(33);
            for i in 0..=32 {
                let t =
                    -std::f32::consts::FRAC_PI_2 + (i as f32 / 32.0) * std::f32::consts::TAU * 0.78;
                pts.push(v(r * t.cos(), r * t.sin()));
            }
            p.add(egui::Shape::line(pts, pen));
            let last = v(
                r * (-std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * 0.78).cos(),
                r * (-std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * 0.78).sin(),
            );
            p.line_segment([last, last + vec2(4.0, 1.0)], pen);
            p.line_segment([last, last + vec2(1.0, 4.5)], pen);
            dot(c);
        }
        GlyphKind::Scale => {
            // small rect bottom-left, big rect top-right, diagonal
            p.rect_stroke(
                egui::Rect::from_min_max(v(-11.0, 2.0), v(-2.0, 11.0)),
                0.0,
                pen,
            );
            p.rect_stroke(
                egui::Rect::from_min_max(v(0.0, -11.0), v(12.0, 1.0)),
                0.0,
                pen,
            );
            p.line_segment([v(-2.0, 2.0), v(0.0, 1.0)], pen);
        }
        GlyphKind::Mirror => {
            // shape + mirrored shape across vertical axis
            p.line_segment([v(0.0, -11.0), v(0.0, 11.0)], egui::Stroke::new(0.8, ink));
            p.line_segment([v(-10.0, 8.0), v(-3.0, -8.0)], pen);
            p.line_segment([v(3.0, -8.0), v(10.0, 8.0)], pen);
            p.line_segment([v(-10.0, 8.0), v(-3.0, 8.0)], egui::Stroke::new(0.8, ink));
            p.line_segment([v(3.0, 8.0), v(10.0, 8.0)], egui::Stroke::new(0.8, ink));
        }
        GlyphKind::Stretch => {
            // dashed rectangle + outward arrow
            for s in egui::Shape::dashed_line(
                &[
                    v(-11.0, -7.0),
                    v(5.0, -7.0),
                    v(5.0, 5.0),
                    v(-11.0, 5.0),
                    v(-11.0, -7.0),
                ],
                egui::Stroke::new(1.0, ink),
                3.0,
                2.0,
            ) {
                p.add(s);
            }
            p.line_segment([v(2.0, -1.0), v(11.0, -1.0)], pen);
            p.line_segment([v(11.0, -1.0), v(8.0, -4.0)], pen);
            p.line_segment([v(11.0, -1.0), v(8.0, 2.0)], pen);
        }
        GlyphKind::Align => {
            // two segments aligning to a target line
            p.line_segment([v(-12.0, 5.0), v(-2.0, -5.0)], pen);
            p.line_segment([v(4.0, 5.0), v(12.0, -5.0)], pen);
            p.line_segment([v(-12.0, 8.0), v(12.0, 8.0)], egui::Stroke::new(0.8, ink));
        }
        GlyphKind::Trim => {
            // X over a line — "cut here"
            p.line_segment([v(-12.0, 5.0), v(12.0, 5.0)], pen);
            p.line_segment([v(-5.0, -5.0), v(5.0, 5.0)], pen);
            p.line_segment([v(-5.0, 5.0), v(5.0, -5.0)], pen);
        }
        GlyphKind::Extend => {
            // line + arrow extending to a wall
            p.line_segment([v(-12.0, 0.0), v(6.0, 0.0)], pen);
            p.line_segment([v(6.0, 0.0), v(3.0, -3.0)], pen);
            p.line_segment([v(6.0, 0.0), v(3.0, 3.0)], pen);
            p.line_segment([v(11.0, -10.0), v(11.0, 10.0)], egui::Stroke::new(2.0, ink));
        }
        GlyphKind::Fillet => {
            // two lines joined by an arc (rounded corner)
            p.line_segment([v(-11.0, 8.0), v(-4.0, 8.0)], pen);
            let r = 7.0;
            let mut pts = Vec::with_capacity(17);
            for i in 0..=16 {
                let t =
                    std::f32::consts::FRAC_PI_2 - (i as f32 / 16.0) * std::f32::consts::FRAC_PI_2;
                pts.push(v(-4.0 + r * t.sin() - r, 8.0 + r - r * t.cos() - r) + vec2(7.0, 0.0));
            }
            // Simpler: draw the arc as a quarter circle joining the two lines
            let arc_center = v(-4.0, 1.0);
            let mut pts = Vec::with_capacity(17);
            for i in 0..=16 {
                let t =
                    -std::f32::consts::FRAC_PI_2 + (i as f32 / 16.0) * std::f32::consts::FRAC_PI_2;
                pts.push(arc_center + vec2(7.0 * t.cos(), 7.0 * t.sin()));
            }
            p.add(egui::Shape::line(pts, pen));
            p.line_segment([v(3.0, 1.0), v(3.0, 10.0)], pen);
        }
        GlyphKind::Chamfer => {
            // two lines joined by a diagonal (beveled corner)
            p.line_segment([v(-11.0, 8.0), v(-4.0, 8.0)], pen);
            p.line_segment([v(-4.0, 8.0), v(3.0, 1.0)], pen);
            p.line_segment([v(3.0, 1.0), v(3.0, 10.0)], pen);
        }
        GlyphKind::Offset => {
            // two parallel arcs
            let r1 = 9.0;
            let r2 = 5.0;
            let mut a = Vec::new();
            let mut b = Vec::new();
            for i in 0..=24 {
                let t = -std::f32::consts::FRAC_PI_2 + (i as f32 / 24.0) * std::f32::consts::PI;
                a.push(v(r1 * t.cos(), r1 * t.sin()));
                b.push(v(r2 * t.cos(), r2 * t.sin()));
            }
            p.add(egui::Shape::line(a, pen));
            p.add(egui::Shape::line(b, pen));
        }
        GlyphKind::Join => {
            // two arrows pointing at each other meeting at center
            p.line_segment([v(-12.0, 0.0), v(-2.0, 0.0)], pen);
            p.line_segment([v(-2.0, 0.0), v(-5.0, -3.0)], pen);
            p.line_segment([v(-2.0, 0.0), v(-5.0, 3.0)], pen);
            p.line_segment([v(12.0, 0.0), v(2.0, 0.0)], pen);
            p.line_segment([v(2.0, 0.0), v(5.0, -3.0)], pen);
            p.line_segment([v(2.0, 0.0), v(5.0, 3.0)], pen);
        }
        GlyphKind::Break => {
            // line with gap in middle
            p.line_segment([v(-12.0, 0.0), v(-3.0, 0.0)], pen);
            p.line_segment([v(3.0, 0.0), v(12.0, 0.0)], pen);
            p.line_segment([v(-3.0, -5.0), v(-3.0, 5.0)], egui::Stroke::new(0.8, ink));
            p.line_segment([v(3.0, -5.0), v(3.0, 5.0)], egui::Stroke::new(0.8, ink));
        }
        GlyphKind::Lengthen => {
            // short segment → arrow lengthening
            p.line_segment([v(-12.0, 0.0), v(-2.0, 0.0)], pen);
            p.line_segment([v(-12.0, -4.0), v(-12.0, 4.0)], pen);
            p.line_segment([v(-2.0, 0.0), v(11.0, 0.0)], egui::Stroke::new(1.0, ink));
            p.line_segment([v(11.0, 0.0), v(8.0, -3.0)], pen);
            p.line_segment([v(11.0, 0.0), v(8.0, 3.0)], pen);
        }
        GlyphKind::Erase => {
            // pencil eraser block + diagonal
            p.rect_filled(
                egui::Rect::from_min_max(v(-10.0, -2.0), v(2.0, 8.0)),
                3.0,
                egui::Color32::from_rgb(220, 100, 80),
            );
            p.rect_stroke(
                egui::Rect::from_min_max(v(-10.0, -2.0), v(2.0, 8.0)),
                3.0,
                egui::Stroke::new(1.0, ink),
            );
            p.line_segment([v(2.0, -2.0), v(11.0, -11.0)], pen);
            p.line_segment([v(2.0, 8.0), v(11.0, -1.0)], pen);
        }
        GlyphKind::MatchProps => {
            // brush + downward strokes
            p.rect_stroke(
                egui::Rect::from_min_max(v(-8.0, -10.0), v(8.0, -2.0)),
                1.0,
                pen,
            );
            for x in [-6.0, -2.0, 2.0, 6.0] {
                p.line_segment([v(x, -2.0), v(x, 9.0)], egui::Stroke::new(1.1, ink));
            }
        }
        GlyphKind::ChangeLayer => {
            // stacked rectangles
            p.rect_stroke(
                egui::Rect::from_min_max(v(-11.0, 4.0), v(7.0, 10.0)),
                1.0,
                pen,
            );
            p.rect_stroke(
                egui::Rect::from_min_max(v(-8.0, -2.0), v(10.0, 4.0)),
                1.0,
                pen,
            );
            p.rect_stroke(
                egui::Rect::from_min_max(v(-5.0, -8.0), v(13.0, -2.0)),
                1.0,
                pen,
            );
        }
        GlyphKind::ArrayGrid => {
            // 3x3 dots
            for y in [-8.0, 0.0, 8.0] {
                for x in [-8.0, 0.0, 8.0] {
                    p.rect_stroke(
                        egui::Rect::from_min_max(v(x - 3.0, y - 3.0), v(x + 3.0, y + 3.0)),
                        0.5,
                        egui::Stroke::new(1.0, ink),
                    );
                }
            }
        }
        GlyphKind::Reverse => {
            // curved arrow flipping direction
            let mut pts = Vec::with_capacity(20);
            for i in 0..=19 {
                let t = std::f32::consts::PI - (i as f32 / 19.0) * std::f32::consts::PI;
                pts.push(v(10.0 * t.cos(), 5.0 - 5.0 * t.sin()));
            }
            p.add(egui::Shape::line(pts, pen));
            let tip = v(-10.0, 5.0);
            p.line_segment([tip, tip + vec2(4.0, -3.0)], pen);
            p.line_segment([tip, tip + vec2(4.0, 3.0)], pen);
        }
        GlyphKind::Dist => {
            // ruler-style: line with tick marks + arrowheads
            p.line_segment([v(-12.0, 0.0), v(12.0, 0.0)], pen);
            for x in [-12.0, -6.0, 0.0, 6.0, 12.0] {
                p.line_segment([v(x, -4.0), v(x, 0.0)], egui::Stroke::new(0.8, ink));
            }
            p.line_segment([v(-12.0, 0.0), v(-9.0, -3.0)], pen);
            p.line_segment([v(-12.0, 0.0), v(-9.0, 3.0)], pen);
            p.line_segment([v(12.0, 0.0), v(9.0, -3.0)], pen);
            p.line_segment([v(12.0, 0.0), v(9.0, 3.0)], pen);
        }
        GlyphKind::List => {
            // three lines representing a list
            for y in [-7.0, 0.0, 7.0] {
                p.line_segment([v(-10.0, y), v(-6.0, y)], egui::Stroke::new(1.6, ink));
                p.line_segment([v(-3.0, y), v(11.0, y)], pen);
            }
        }
        GlyphKind::Undo => {
            // curved arrow ← — arc centered on x=0 (was drawn in the left half,
            // making the icon look shifted vs its neighbours in the menu column).
            let mut pts = Vec::with_capacity(17);
            for i in 0..=16 {
                let t = (i as f32 / 16.0) * std::f32::consts::PI;
                pts.push(v(9.0 * t.cos(), -9.0 * t.sin()));
            }
            p.add(egui::Shape::line(pts, pen));
            let tip = v(0.0, 0.0);
            p.line_segment([tip, tip + vec2(4.0, -3.0)], pen);
            p.line_segment([tip, tip + vec2(4.0, 3.0)], pen);
        }
        GlyphKind::Redo => {
            // mirror of undo → — arc centered on x=0.
            let mut pts = Vec::with_capacity(17);
            for i in 0..=16 {
                let t = (i as f32 / 16.0) * std::f32::consts::PI;
                pts.push(v(-9.0 * t.cos(), -9.0 * t.sin()));
            }
            p.add(egui::Shape::line(pts, pen));
            let tip = v(0.0, 0.0);
            p.line_segment([tip, tip + vec2(-4.0, -3.0)], pen);
            p.line_segment([tip, tip + vec2(-4.0, 3.0)], pen);
        }
        GlyphKind::Block => {
            // outer frame holding contained geometry + a base-point grip
            // at the lower-left corner (the handle an instance carries).
            let thin = egui::Stroke::new(0.9, ink);
            p.rect_stroke(
                egui::Rect::from_min_max(v(-10.0, -9.0), v(8.0, 7.0)),
                1.0,
                pen,
            );
            // contained-geometry hint: a small circle + diagonal line.
            p.circle_stroke(v(-1.0, -1.0), 4.0, thin);
            p.line_segment([v(-7.0, 5.0), v(5.0, -6.0)], thin);
            dot(v(-10.0, 7.0)); // base point grip
        }
        GlyphKind::Insert => {
            // a block (small square) dropping onto an insertion node:
            // square top-left, arrow down-right to a target dot.
            p.rect_stroke(
                egui::Rect::from_min_max(v(-12.0, -11.0), v(-2.0, -1.0)),
                1.0,
                pen,
            );
            p.line_segment([v(-2.0, -1.0), v(7.0, 8.0)], pen);
            p.line_segment([v(7.0, 8.0), v(1.0, 7.0)], pen);
            p.line_segment([v(7.0, 8.0), v(6.0, 2.0)], pen);
            dot(v(9.0, 10.0)); // insertion point
        }
        GlyphKind::Explode => {
            // a core square bursting into outward shards (scatter).
            p.rect_stroke(
                egui::Rect::from_min_max(v(-4.0, -4.0), v(4.0, 4.0)),
                0.0,
                pen,
            );
            for (dx, dy) in [
                (1.0, 0.8),
                (-1.0, 0.8),
                (1.0, -0.8),
                (-1.0, -0.8),
                (0.0, 1.25),
                (0.0, -1.25),
            ] {
                p.line_segment([v(6.0 * dx, 6.0 * dy), v(11.0 * dx, 11.0 * dy)], pen);
            }
        }
    }
}

/// Command word a draw tool maps to (for the last-command buffer, so an empty
/// Enter repeats a ribbon/icon command). `None` for the pointer.
pub(super) fn tool_command_word(t: Tool) -> Option<&'static str> {
    Some(match t {
        Tool::Line => "line",
        Tool::Circle => "circle",
        Tool::Arc => "arc",
        Tool::Ellipse => "ellipse",
        Tool::EllipseArc => "ellipsearc",
        Tool::Point => "point",
        Tool::Polyline => "polyline",
        Tool::Spline => "spline",
        Tool::Wall => "wall",
        Tool::Text => "text",
        Tool::Dim => "dim",
        Tool::Rectangle => "rec",
        Tool::None => return None,
    })
}

pub(super) fn tool_button(ui: &mut egui::Ui, current: &mut Tool, this: Tool, label: &str) -> bool {
    let selected = *current == this;
    let (resp, painter) = ui.allocate_painter(egui::vec2(56.0, 52.0), egui::Sense::click());
    let rect = resp.rect;
    let bg = if selected {
        egui::Color32::from_rgb(60, 110, 175)
    } else if resp.hovered() {
        egui::Color32::from_rgb(48, 58, 72)
    } else {
        egui::Color32::from_rgb(28, 34, 42)
    };
    painter.rect(
        rect,
        5.0,
        bg,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
    );

    let c = rect.center() - egui::vec2(0.0, 4.0);
    let icon_col = egui::Color32::from_rgb(225, 235, 245);
    let pen = egui::Stroke::new(1.8, icon_col);
    let dot = |p: egui::Pos2| painter.circle_filled(p, 1.8, icon_col);
    match this {
        Tool::None => {
            // arrow / pointer
            painter.line_segment([c + egui::vec2(-8.0, -8.0), c + egui::vec2(6.0, 6.0)], pen);
            painter.line_segment([c + egui::vec2(-8.0, -8.0), c + egui::vec2(-3.0, 2.0)], pen);
            painter.line_segment([c + egui::vec2(-8.0, -8.0), c + egui::vec2(2.0, -3.0)], pen);
        }
        Tool::Line => {
            painter.line_segment(
                [c + egui::vec2(-14.0, 10.0), c + egui::vec2(14.0, -10.0)],
                pen,
            );
            dot(c + egui::vec2(-14.0, 10.0));
            dot(c + egui::vec2(14.0, -10.0));
        }
        Tool::Circle => {
            painter.circle_stroke(c, 13.0, pen);
            dot(c);
        }
        Tool::Arc => {
            // half-circle + center dot + two endpoint dots (center-start-end variant)
            let n = 24;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = std::f32::consts::PI * (i as f32 / n as f32);
                pts.push(c + egui::vec2(-13.0 * t.cos(), -13.0 * t.sin()));
            }
            painter.add(egui::Shape::line(pts, pen));
            dot(c);
            dot(c + egui::vec2(-13.0, 0.0));
            dot(c + egui::vec2(13.0, 0.0));
        }
        Tool::Ellipse => {
            // squashed ellipse — a 2:1 ratio so it reads distinctly from the circle
            let n = 32;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = std::f32::consts::TAU * (i as f32 / n as f32);
                pts.push(c + egui::vec2(14.0 * t.cos(), 7.0 * t.sin()));
            }
            painter.add(egui::Shape::line(pts, pen));
            dot(c);
            dot(c + egui::vec2(14.0, 0.0)); // major-end
            dot(c + egui::vec2(0.0, -7.0)); // minor-end
        }
        Tool::EllipseArc => {
            // top-half of a squashed ellipse — same proportions as the
            // ellipse icon, but only the upper sweep is drawn.
            let n = 24;
            let mut pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = std::f32::consts::PI * (i as f32 / n as f32);
                pts.push(c + egui::vec2(-14.0 * t.cos(), -7.0 * t.sin()));
            }
            painter.add(egui::Shape::line(pts, pen));
            dot(c);
            dot(c + egui::vec2(-14.0, 0.0)); // start
            dot(c + egui::vec2(14.0, 0.0)); // end
        }
        Tool::Point => {
            // a small '+' glyph
            painter.line_segment([c + egui::vec2(-9.0, 0.0), c + egui::vec2(9.0, 0.0)], pen);
            painter.line_segment([c + egui::vec2(0.0, -9.0), c + egui::vec2(0.0, 9.0)], pen);
            dot(c);
        }
        Tool::Rectangle => {
            // a rectangle outline with two opposite-corner dots (the two
            // clicks the user makes).
            let r = egui::Rect::from_center_size(c, egui::vec2(26.0, 18.0));
            painter.rect_stroke(r, 0.0, pen);
            dot(r.left_top());
            dot(r.right_bottom());
        }
        Tool::Polyline => {
            // a 3-segment chevron-ish shape with vertex dots
            let p1 = c + egui::vec2(-14.0, 8.0);
            let p2 = c + egui::vec2(-4.0, -8.0);
            let p3 = c + egui::vec2(6.0, 6.0);
            let p4 = c + egui::vec2(14.0, -4.0);
            painter.line_segment([p1, p2], pen);
            painter.line_segment([p2, p3], pen);
            painter.line_segment([p3, p4], pen);
            dot(p1);
            dot(p2);
            dot(p3);
            dot(p4);
        }
        Tool::Spline => {
            // Smooth S-curve sampled from a cubic NURBS through 4
            // control points (rendered AS the icon — eats its own
            // dogfood). The 4 control dots show where the user clicks
            // when drafting; the curve shows the result. Hint pens
            // sketch the control polygon underneath so the icon also
            // teaches the data model at a glance.
            let p1 = c + egui::vec2(-14.0, 8.0);
            let p2 = c + egui::vec2(-5.0, -10.0);
            let p3 = c + egui::vec2(5.0, 10.0);
            let p4 = c + egui::vec2(14.0, -8.0);
            // Faint chord polygon (the "control polygon")
            let hint = egui::Stroke::new(
                0.6,
                egui::Color32::from_rgba_unmultiplied(icon_col.r(), icon_col.g(), icon_col.b(), 90),
            );
            painter.line_segment([p1, p2], hint);
            painter.line_segment([p2, p3], hint);
            painter.line_segment([p3, p4], hint);
            // The curve itself — a cubic Bézier sample is close enough
            // visually to a degree-3 clamped uniform NURBS through 4
            // control points (which IS a single Bézier in this case).
            let mut prev = p1;
            for i in 1..=24 {
                let t = i as f32 / 24.0;
                let u = 1.0 - t;
                let pt = egui::pos2(
                    u * u * u * p1.x
                        + 3.0 * u * u * t * p2.x
                        + 3.0 * u * t * t * p3.x
                        + t * t * t * p4.x,
                    u * u * u * p1.y
                        + 3.0 * u * u * t * p2.y
                        + 3.0 * u * t * t * p3.y
                        + t * t * t * p4.y,
                );
                painter.line_segment([prev, pt], pen);
                prev = pt;
            }
            // Control-point dots
            dot(p1);
            dot(p2);
            dot(p3);
            dot(p4);
        }
        Tool::Wall => {
            // Two parallel horizontals — the wall's two side lines.
            // Endpoint dots show the click anchors (the centerline
            // endpoints the user actually clicks). The "thickness" is
            // the gap between the two parallels.
            painter.line_segment(
                [c + egui::vec2(-14.0, -4.0), c + egui::vec2(14.0, -4.0)],
                pen,
            );
            painter.line_segment([c + egui::vec2(-14.0, 4.0), c + egui::vec2(14.0, 4.0)], pen);
            dot(c + egui::vec2(-14.0, 0.0));
            dot(c + egui::vec2(14.0, 0.0));
        }
        Tool::Text => {
            // Big "A" — universal text icon. Built from 3 line strokes
            // so it scales with the toolbar text pen.
            painter.line_segment([c + egui::vec2(-8.0, 8.0), c + egui::vec2(0.0, -10.0)], pen);
            painter.line_segment([c + egui::vec2(0.0, -10.0), c + egui::vec2(8.0, 8.0)], pen);
            painter.line_segment([c + egui::vec2(-5.0, 2.0), c + egui::vec2(5.0, 2.0)], pen);
            dot(c + egui::vec2(0.0, 10.0)); // baseline anchor hint
        }
        Tool::Dim => {
            // Horizontal dim line with two arrows + two extension lines.
            // Compact CAD-icon language for "dimension".
            // Dim line
            painter.line_segment([c + egui::vec2(-10.0, 0.0), c + egui::vec2(10.0, 0.0)], pen);
            // Left arrowhead
            painter.line_segment(
                [c + egui::vec2(-10.0, 0.0), c + egui::vec2(-6.0, -3.0)],
                pen,
            );
            painter.line_segment([c + egui::vec2(-10.0, 0.0), c + egui::vec2(-6.0, 3.0)], pen);
            // Right arrowhead
            painter.line_segment([c + egui::vec2(10.0, 0.0), c + egui::vec2(6.0, -3.0)], pen);
            painter.line_segment([c + egui::vec2(10.0, 0.0), c + egui::vec2(6.0, 3.0)], pen);
            // Two extension lines
            painter.line_segment(
                [c + egui::vec2(-10.0, 8.0), c + egui::vec2(-10.0, -3.0)],
                pen,
            );
            painter.line_segment([c + egui::vec2(10.0, 8.0), c + egui::vec2(10.0, -3.0)], pen);
        }
    }

    painter.text(
        rect.center_bottom() - egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_BOTTOM,
        label,
        egui::FontId::proportional(10.0),
        icon_col,
    );

    if resp.clicked() {
        *current = if selected { Tool::None } else { this };
        return true;
    }
    false
}

// ---- hatch command button (one-shot — opens dialog) -----------------------

/// Custom-painted Hatch button. Hatch isn't a persistent draw tool
/// (no `Tool::Hatch` variant), so this lives outside `tool_button` —
/// click → `run_command("hatch")` → opens the Choose Hatch Attributes
/// dialog. Icon: a square outline with 45° hatching, corner
/// registration ticks (so it reads as a "boundary"), and a small
/// pick-point cursor at the bottom-right (signifying the Pick Point
/// flow). User reference: the freenom-style "hatch" pictogram with
/// crosshairs at every corner.
pub(super) fn hatch_command_button(ui: &mut egui::Ui) -> bool {
    let (resp, painter) = ui.allocate_painter(egui::vec2(56.0, 52.0), egui::Sense::click());
    let rect = resp.rect;
    let bg = if resp.hovered() {
        egui::Color32::from_rgb(48, 58, 72)
    } else {
        egui::Color32::from_rgb(28, 34, 42)
    };
    painter.rect(
        rect,
        5.0,
        bg,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
    );

    let c = rect.center() - egui::vec2(0.0, 4.0);
    let icon_col = egui::Color32::from_rgb(225, 235, 245);
    let pen = egui::Stroke::new(1.6, icon_col);
    let thin = egui::Stroke::new(1.0, icon_col);

    // Boundary square
    let half = 11.0_f32;
    let sq = egui::Rect::from_center_size(c, egui::vec2(half * 2.0, half * 2.0));
    painter.rect_stroke(sq, 0.0, pen);

    // Hatching: 5 diagonal lines at 45°, clipped to the square. Drawing
    // long lines and clipping is simpler than computing exact entry/exit
    // points per line — egui's `with_clip_rect` does the rest.
    let inner = painter.with_clip_rect(sq);
    let span = half * 4.0;
    let spacing = (half * 2.0) / 4.0; // 5 lines: at -2s, -s, 0, +s, +2s
    for k in -2..=2 {
        let off = k as f32 * spacing;
        let p1 = egui::pos2(c.x - span + off, c.y + span + off);
        let p2 = egui::pos2(c.x + span + off, c.y - span + off);
        inner.line_segment([p1, p2], thin);
    }

    // Corner registration ticks — a short "L" extending outward from
    // each corner of the boundary. Reads as "this is a boundary I'm
    // selecting" rather than just an arbitrary outline.
    let ext = 4.5_f32;
    for &(corner, dx, dy) in &[
        (sq.left_top(), -1.0_f32, -1.0_f32),
        (sq.right_top(), 1.0, -1.0),
        (sq.left_bottom(), -1.0, 1.0),
        (sq.right_bottom(), 1.0, 1.0),
    ] {
        painter.line_segment([corner, corner + egui::vec2(dx * ext, 0.0)], pen);
        painter.line_segment([corner, corner + egui::vec2(0.0, dy * ext)], pen);
    }

    // Pick-point cursor at lower-right: a small offset pickbox + arrow
    // pointing INTO the main square. Communicates "click inside to
    // pick the boundary" at a glance.
    let pb_center = sq.right_bottom() + egui::vec2(3.0, 3.0);
    let pb_half = 2.5_f32;
    let pb_rect = egui::Rect::from_center_size(pb_center, egui::vec2(pb_half * 2.0, pb_half * 2.0));
    painter.rect_stroke(pb_rect, 0.0, pen);
    // Arrow from pickbox toward sq's center
    let arrow_tip = pb_center + egui::vec2(-3.0, -3.0);
    let arrow_tail = pb_center + egui::vec2(-0.5, -0.5);
    painter.line_segment([arrow_tail, arrow_tip], pen);
    painter.line_segment([arrow_tip, arrow_tip + egui::vec2(2.5, 0.0)], pen);
    painter.line_segment([arrow_tip, arrow_tip + egui::vec2(0.0, 2.5)], pen);

    painter.text(
        rect.center_bottom() - egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_BOTTOM,
        "hatch",
        egui::FontId::proportional(10.0),
        icon_col,
    );

    resp.clicked()
}

// ---- arc tool button (toolbar, one per quick-access method) ---------------

pub(super) fn arc_tool_button(
    ui: &mut egui::Ui,
    current_tool: &mut Tool,
    current_method: &mut ArcMethod,
    method: ArcMethod,
    label: &str,
) -> bool {
    let selected = *current_tool == Tool::Arc && *current_method == method;
    let (resp, painter) = ui.allocate_painter(egui::vec2(56.0, 52.0), egui::Sense::click());
    let rect = resp.rect;
    let bg = if selected {
        egui::Color32::from_rgb(60, 110, 175)
    } else if resp.hovered() {
        egui::Color32::from_rgb(48, 58, 72)
    } else {
        egui::Color32::from_rgb(28, 34, 42)
    };
    painter.rect(
        rect,
        5.0,
        bg,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
    );

    let c = rect.center() - egui::vec2(0.0, 4.0);
    let icon_col = egui::Color32::from_rgb(225, 235, 245);
    let stroke = egui::Stroke::new(1.6, icon_col);
    let dot = |pt: egui::Pos2| painter.circle_filled(pt, 2.2, icon_col);

    // shared half-arc
    let n = 24;
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = std::f32::consts::PI * (i as f32 / n as f32);
        pts.push(c + egui::vec2(-13.0 * t.cos(), -13.0 * t.sin()));
    }
    painter.add(egui::Shape::line(pts.clone(), stroke));

    // method-specific dots — crucially, ThreePoints has no centre dot
    match method {
        ArcMethod::ThreePoints => {
            dot(pts[0]);
            dot(pts[n / 2]);
            dot(pts[n]);
        }
        ArcMethod::StartCenterEnd | ArcMethod::CenterStartEnd => {
            dot(pts[0]);
            dot(c); // centre
            dot(pts[n]);
        }
        _ => {
            dot(pts[0]);
            dot(pts[n]);
        }
    }

    painter.text(
        rect.center_bottom() - egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_BOTTOM,
        label,
        egui::FontId::proportional(10.0),
        icon_col,
    );

    if resp.clicked() {
        *current_tool = Tool::Arc;
        *current_method = method;
        return true;
    }
    false
}

// ---- arc method picker row ------------------------------------------------
//
// Each row paints a small representative icon on the left and the method name
// on the right. Selected rows are highlighted; frozen rows are dimmed and not
// clickable.

pub(super) fn arc_method_row(ui: &mut egui::Ui, current: ArcMethod, this: ArcMethod) -> bool {
    let enabled = this.enabled();
    let row_w = ui.available_width().max(280.0);
    let (resp, painter) = ui.allocate_painter(
        egui::vec2(row_w, 40.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let rect = resp.rect;
    let selected = current == this;

    let bg = if !enabled {
        egui::Color32::TRANSPARENT
    } else if selected {
        egui::Color32::from_rgb(48, 95, 165)
    } else if resp.hovered() {
        egui::Color32::from_rgba_unmultiplied(80, 90, 110, 90)
    } else {
        egui::Color32::TRANSPARENT
    };
    if bg.a() > 0 {
        painter.rect(rect, 4.0, bg, egui::Stroke::NONE);
    }
    if selected {
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 180, 255)),
        );
    }

    // ICON area
    let icon_c = rect.left_center() + egui::vec2(28.0, 0.0);
    let line_col = if !enabled {
        egui::Color32::from_rgb(95, 100, 110)
    } else if selected {
        egui::Color32::from_rgb(230, 240, 255)
    } else {
        egui::Color32::from_rgb(225, 235, 250)
    };
    let dot_col = if !enabled {
        egui::Color32::from_rgb(110, 118, 130)
    } else {
        egui::Color32::from_rgb(80, 160, 250)
    };
    paint_arc_method_icon(&painter, icon_c, this, line_col, dot_col);

    // TEXT
    let text_col = if !enabled {
        egui::Color32::from_rgb(125, 130, 140)
    } else if selected {
        egui::Color32::from_rgb(230, 245, 255)
    } else {
        egui::Color32::from_rgb(225, 232, 245)
    };
    painter.text(
        egui::pos2(rect.left() + 62.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        this.name(),
        egui::FontId::proportional(13.5),
        text_col,
    );

    if !enabled {
        painter.text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "frozen",
            egui::FontId::proportional(10.0),
            egui::Color32::from_rgb(130, 135, 150),
        );
    }

    enabled && resp.clicked()
}

pub(super) fn paint_arc_method_icon(
    p: &egui::Painter,
    c: egui::Pos2,
    m: ArcMethod,
    line_col: egui::Color32,
    dot_col: egui::Color32,
) {
    use std::f32::consts::FRAC_PI_2;
    let stroke = egui::Stroke::new(1.5, line_col);
    let thin = egui::Stroke::new(1.0, line_col);
    let dot = |pt: egui::Pos2| p.circle_filled(pt, 3.0, dot_col);

    // shared quarter-arc going from (-r, 0) to (0, -r) in icon-space
    let r = 16.0;
    let n = 20;
    let arc_pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let t = FRAC_PI_2 * (i as f32 / n as f32);
            c + egui::vec2(-r * t.cos(), -r * t.sin() + 6.0)
        })
        .collect();
    p.add(egui::Shape::line(arc_pts.clone(), stroke));

    let start = arc_pts[0];
    let mid = arc_pts[n / 2];
    let end = arc_pts[n];
    let center = c + egui::vec2(0.0, 6.0);

    // small arrow helper (a short segment with a chevron)
    let arrow = |p: &egui::Painter, from: egui::Pos2, to: egui::Pos2| {
        p.line_segment([from, to], thin);
        let dir = (to - from).normalized();
        let perp = egui::vec2(-dir.y, dir.x);
        let tip = to;
        let back = tip - dir * 4.0;
        p.line_segment([tip, back + perp * 2.0], thin);
        p.line_segment([tip, back - perp * 2.0], thin);
    };

    match m {
        ArcMethod::ThreePoints => {
            dot(start);
            dot(mid);
            dot(end);
        }
        ArcMethod::StartCenterEnd => {
            dot(start);
            dot(end);
            dot(center);
            p.line_segment([center, start], thin);
        }
        ArcMethod::CenterStartEnd => {
            dot(center);
            dot(start);
            dot(end);
            arrow(p, center, end - egui::vec2(2.0, 0.0));
        }
        ArcMethod::StartCenterAngle => {
            dot(start);
            dot(center);
            arrow(
                p,
                center + egui::vec2(8.0, 0.0),
                center + egui::vec2(8.0, -8.0),
            );
        }
        ArcMethod::StartCenterLength => {
            dot(start);
            dot(center);
            p.line_segment(
                [start, start + egui::vec2(14.0, -6.0)],
                egui::Stroke::new(1.0, dot_col),
            );
        }
        ArcMethod::StartEndAngle => {
            dot(start);
            dot(end);
            arrow(p, c + egui::vec2(-6.0, -4.0), c + egui::vec2(2.0, -10.0));
        }
        ArcMethod::StartEndDirection => {
            dot(start);
            dot(end);
            arrow(p, start, start + egui::vec2(0.0, -12.0));
        }
        ArcMethod::StartEndRadius => {
            dot(start);
            dot(end);
            arrow(p, c + egui::vec2(2.0, 4.0), start + egui::vec2(2.0, 0.0));
        }
        ArcMethod::CenterStartAngle => {
            dot(center);
            dot(start);
            arrow(
                p,
                center + egui::vec2(6.0, 0.0),
                center + egui::vec2(6.0, -10.0),
            );
        }
        ArcMethod::CenterStartLength => {
            dot(center);
            dot(start);
            arrow(p, start, end);
        }
        ArcMethod::Continue => {
            // arrow tail merging into the arc start
            arrow(p, start - egui::vec2(8.0, -4.0), start);
        }
    }
}
