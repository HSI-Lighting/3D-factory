//! Dimension entities + DimStyle table.
//!
//! V1 covers the AutoCAD dimension kinds the user is likely to draw
//! first: linear (horizontal / vertical / aligned) + radius + diameter.
//! Angular / arc-length / ordinate / leader are queued for the next
//! slice; the `DimKind` enum is open so they slot in without a data-
//! migration break.
//!
//! `DimStyle` carries the full ~70-DIMVAR AutoCAD parity set — most
//! fields just store their value for fidelity and don't affect v1
//! rendering yet. The renderer reads the subset it needs (arrow size,
//! text height, ext line offsets, decimals, colors, gap); the rest
//! round-trip through DXF when that lands.
//!
//! Naming: fields use descriptive Rust names — `arrow_size` not
//! `dimasz`. A DXF group-code table maps each field to its DIMVAR
//! name when serializing; this keeps the kernel readable without
//! losing the AutoCAD vocabulary at the interop boundary.

use crate::math::Vec2;

// ---------------------------------------------------------------------------
// DimKind — the geometric shape of the dimension.
// ---------------------------------------------------------------------------

/// Linear-dimension orientation. `Horizontal` and `Vertical` ignore the
/// p1→p2 angle and project onto the world X/Y axes respectively;
/// `Aligned` measures the actual chord length along p1→p2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinearOrtho {
    Horizontal,
    Vertical,
    Aligned,
}

#[derive(Clone, Debug)]
pub enum DimKind {
    /// Distance between two def points; rendered with two extension
    /// lines + a dimension line + two arrows + a text label.
    /// `dimline_pos` is any point through which the dim line must
    /// pass; the actual dim line is parallel to (h/v/aligned) at that
    /// perpendicular offset from p1↔p2.
    Linear {
        p1:          Vec2,
        p2:          Vec2,
        dimline_pos: Vec2,
        ortho:       LinearOrtho,
    },
    /// Radius of a circle / arc. `center` is the circle's centre;
    /// `on_circle` is the user-picked point on the circumference; the
    /// `leader_end` is where the dim text + leader tail sit.
    Radius {
        center:     Vec2,
        on_circle:  Vec2,
        leader_end: Vec2,
    },
    /// Diameter — two-arrow leader through `center` from one side of
    /// the circle to the other. `leader_end` positions the text label.
    Diameter {
        center:     Vec2,
        on_circle:  Vec2,
        leader_end: Vec2,
    },
    /// Angular (AutoCAD DIMANGULAR) — the angle between two rays
    /// meeting at a vertex. `p1` / `p2` are points on the two rays
    /// (they define the directions; the extension lines run from the
    /// vertex outward through them). `arc_pos` is the point the dim
    /// ARC passes through — it sets the arc radius AND which side of
    /// the vertex the arc sits on (minor or major angle).
    Angular {
        vertex:  Vec2,
        p1:      Vec2,
        p2:      Vec2,
        arc_pos: Vec2,
    },
    /// Arc-length (AutoCAD DIMARC) — the length along a circular arc.
    /// `start_angle`/`sweep` describe the measured arc (signed sweep,
    /// CCW positive); `leader_end` positions the text + leader tail.
    ArcLen {
        center:      Vec2,
        radius:      f64,
        start_angle: f64,
        sweep:       f64,
        leader_end:  Vec2,
    },
    /// Ordinate (AutoCAD DIMORDINATE) — the X or Y distance of a
    /// feature point from a datum origin. `is_x` = measure along X.
    Ordinate {
        datum:      Vec2,
        point:      Vec2,
        leader_end: Vec2,
        is_x:       bool,
    },
    /// Jogged radius (AutoCAD DIMJOGGED) — radius leader with a jog
    /// (two-segment leader) for large radii.
    JoggedRadius {
        center:     Vec2,
        on_circle:  Vec2,
        leader_end: Vec2,
        jog_pos:    Vec2,
    },
}

// ---------------------------------------------------------------------------
// Dim — the entity itself.
// ---------------------------------------------------------------------------

/// One dimension entity. `kind` carries the geometric data;
/// `style` indexes `Document.dim_styles` (0 = STANDARD).
/// `text_override` lets the user replace the auto-computed value with
/// a literal string (e.g. "≈ R5" or "<>" to keep the measured value
/// plus a suffix). AutoCAD calls this "Mtext override" — for v1 we
/// store a single string; it replaces the measured text verbatim when
/// non-empty.
#[derive(Clone, Debug)]
pub struct Dim {
    pub kind:          DimKind,
    pub style:         u32,
    pub text_override: Option<String>,
}

impl Dim {
    /// Numeric value the dimension measures, in world units (linear)
    /// or world units (radius/diameter). Always positive.
    pub fn measured_value(&self) -> f64 {
        match &self.kind {
            DimKind::Linear { p1, p2, ortho, .. } => match ortho {
                LinearOrtho::Horizontal => (p2.x - p1.x).abs(),
                LinearOrtho::Vertical   => (p2.y - p1.y).abs(),
                LinearOrtho::Aligned    => (*p2 - *p1).len(),
            },
            DimKind::Radius { center, on_circle, .. } |
            DimKind::Diameter { center, on_circle, .. } => {
                let r = (*on_circle - *center).len();
                if matches!(self.kind, DimKind::Diameter { .. }) { r * 2.0 } else { r }
            }
            // Angular — the angle between the two rays, in DEGREES
            // (0..=180). The arc may be drawn on either side; the
            // measured value is always the angle between the rays.
            DimKind::Angular { vertex, p1, p2, .. } => {
                let a1 = (*p1 - *vertex).angle();
                let a2 = (*p2 - *vertex).angle();
                let mut d = (a2 - a1).abs();
                if d > std::f64::consts::PI {
                    d = std::f64::consts::TAU - d;
                }
                d.to_degrees()
            }
            // Arc-length — the arc distance along the sweep.
            DimKind::ArcLen { radius, sweep, .. } => radius * sweep.abs(),
            // Ordinate — the X or Y distance from the datum.
            DimKind::Ordinate { datum, point, is_x, .. } =>
                if *is_x { (point.x - datum.x).abs() } else { (point.y - datum.y).abs() },
            // Jogged radius — same measured value as a radius.
            DimKind::JoggedRadius { center, on_circle, .. } =>
                (*on_circle - *center).len(),
        }
    }

    /// Text the renderer should draw — either the user's override or
    /// the measured value formatted via the style. The style supplies
    /// linear scale, decimal places, prefix/suffix, and zero suppression.
    pub fn formatted_text(&self, style: &DimStyle) -> String {
        if let Some(s) = &self.text_override {
            if !s.is_empty() {
                // "<>" is the AutoCAD convention for "insert measured
                // value here"; honour it so users can prefix/suffix
                // around the live measurement.
                if s.contains("<>") {
                    let mv = self.format_measured(style);
                    return s.replace("<>", &mv);
                }
                return s.clone();
            }
        }
        let mv = self.format_measured(style);
        // Radius / diameter get the AutoCAD R / ⌀ prefix unless the
        // user overrides via DIMPOST. Angular dims get a ° suffix.
        let prefix = match &self.kind {
            DimKind::Radius { .. }   => "R",
            DimKind::Diameter { .. } => "\u{2300}",      // ⌀
            DimKind::JoggedRadius { .. } => "R",
            DimKind::Linear { .. }   => "",
            DimKind::Angular { .. }  => "",
            DimKind::ArcLen { .. }   => "",
            DimKind::Ordinate { is_x, .. } => if *is_x { "X=" } else { "Y=" },
        };
        let (post_pre, post_suf) = parse_dimpost(&style.linear_post);
        // post_pre comes BEFORE the prefix (rare); post_suf comes after
        // the number. Most users only set the suffix.
        match &self.kind {
            DimKind::Angular { .. } => {
                let mut s = format!("{}{}{}{}",
                    post_pre, prefix, mv, post_suf);
                // Degree symbol for angular measurements (AutoCAD shows
                // e.g. 45°). Not applied when the user gave an override.
                if !s.ends_with('\u{00B0}') { s.push('\u{00B0}'); }
                s
            }
            _ => format!("{}{}{}{}", post_pre, prefix, mv, post_suf),
        }
    }

    fn format_measured(&self, style: &DimStyle) -> String {
        let v = self.measured_value() * style.linear_scale;
        let rounded = round_to(v, style.rounding);
        let s = format!("{:.*}", style.decimal_places as usize, rounded);
        suppress_zeros(s, style.zero_suppress)
    }

    /// Conservative bbox — does not account for text width because the
    /// renderer owns text layout. Includes def points and the dim
    /// line position; sufficient for spatial-index culling.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        let pts: Vec<Vec2> = match &self.kind {
            DimKind::Linear { p1, p2, dimline_pos, .. } =>
                vec![*p1, *p2, *dimline_pos],
            DimKind::Radius { center, on_circle, leader_end } |
            DimKind::Diameter { center, on_circle, leader_end } =>
                vec![*center, *on_circle, *leader_end],
            DimKind::Angular { vertex, p1, p2, arc_pos } =>
                vec![*vertex, *p1, *p2, *arc_pos],
            DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } => {
                let a0 = start_angle + if *sweep < 0.0 { *sweep } else { 0.0 };
                let a1 = start_angle + if *sweep > 0.0 { *sweep } else { 0.0 };
                let mut v = vec![*leader_end,
                    *center + Vec2::new(a0.cos() * radius, a0.sin() * radius),
                    *center + Vec2::new(a1.cos() * radius, a1.sin() * radius)];
                // Quadrant samples make the bbox tight for wide arcs.
                let mut a = a0.min(a1);
                let end = a0.max(a1);
                while a <= end {
                    v.push(*center + Vec2::new(a.cos() * radius, a.sin() * radius));
                    a += std::f64::consts::FRAC_PI_2;
                }
                v
            }
            DimKind::Ordinate { datum, point, leader_end, .. } =>
                vec![*datum, *point, *leader_end],
            DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos } =>
                vec![*center, *on_circle, *leader_end, *jog_pos],
        };
        let mut min = pts[0];
        let mut max = pts[0];
        for p in &pts[1..] {
            if p.x < min.x { min.x = p.x; }
            if p.y < min.y { min.y = p.y; }
            if p.x > max.x { max.x = p.x; }
            if p.y > max.y { max.y = p.y; }
        }
        (min, max)
    }

    /// Grip points — the user-grabable handles. Linear: 3 grips
    /// (p1, p2, dimline_pos). Radius / Diameter: 3 grips
    /// (center, on_circle, leader_end). The app layer maps these
    /// to specific `GripRole`s.
    pub fn grip_points(&self) -> Vec<Vec2> {
        match &self.kind {
            DimKind::Linear { p1, p2, dimline_pos, .. } =>
                vec![*p1, *p2, *dimline_pos],
            DimKind::Radius { center, on_circle, leader_end } |
            DimKind::Diameter { center, on_circle, leader_end } =>
                vec![*center, *on_circle, *leader_end],
            DimKind::Angular { vertex, p1, p2, arc_pos } =>
                vec![*vertex, *p1, *p2, *arc_pos],
            DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } => {
                let a0 = start_angle;
                let a1 = start_angle + sweep;
                vec![*center,
                     *center + Vec2::new(a0.cos() * radius, a0.sin() * radius),
                     *center + Vec2::new(a1.cos() * radius, a1.sin() * radius),
                     *leader_end]
            }
            DimKind::Ordinate { datum, point, leader_end, .. } =>
                vec![*datum, *point, *leader_end],
            DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos } =>
                vec![*center, *on_circle, *leader_end, *jog_pos],
        }
    }

    /// The VISIBLE line segments of the dimension — extension lines + the
    /// dimension line for linear dims; the leader/dim line for radius &
    /// diameter. Used for click hit-testing so the user can pick the
    /// dimension by clicking ON the line they see (not only its def points).
    /// Arrowheads/text aren't included; the dim-line endpoints cover the
    /// arrow region and grip_points() covers the text anchor.
    pub fn outline_segments(&self) -> Vec<(Vec2, Vec2)> {        match &self.kind {
            DimKind::Linear { p1, p2, dimline_pos, ortho } => {
                let u = match ortho {
                    LinearOrtho::Horizontal => Vec2::new(1.0, 0.0),
                    LinearOrtho::Vertical   => Vec2::new(0.0, 1.0),
                    LinearOrtho::Aligned => {
                        let d = *p2 - *p1;
                        if d.len() < 1e-9 { Vec2::new(1.0, 0.0) } else {
                            let l = d.len(); Vec2::new(d.x / l, d.y / l)
                        }
                    }
                };
                // Project each def point onto the dim line (through
                // `dimline_pos`, direction `u`) to get the dim-line ends.
                let proj = |q: Vec2| {
                    let t = (q - *dimline_pos).dot(u);
                    Vec2::new(dimline_pos.x + u.x * t, dimline_pos.y + u.y * t)
                };
                let d1 = proj(*p1);
                let d2 = proj(*p2);
                vec![(*p1, d1), (*p2, d2), (d1, d2)]   // ext1, ext2, dim line
            }
            DimKind::Radius { center, on_circle, leader_end } =>
                vec![(*center, *on_circle), (*on_circle, *leader_end)],
            DimKind::Diameter { center, on_circle, leader_end } => {
                // Diameter line runs through the centre to the far side.
                let opp = Vec2::new(center.x * 2.0 - on_circle.x,
                                    center.y * 2.0 - on_circle.y);
                vec![(opp, *on_circle), (*on_circle, *leader_end)]
            }
            DimKind::Angular { vertex, p1, p2, arc_pos } => {
                // Extension lines from the vertex outward through the
                // two rays, plus a tessellated dim ARC.
                let mut segs = vec![(*vertex, *p1), (*vertex, *p2)];
                let r = (*arc_pos - *vertex).len();
                if r > 1e-9 {
                    let (a1, sweep) = angular_arc(vertex, p1, p2, arc_pos);
                    let end = a1 + sweep;
                    let n = (24.0_f64.max(sweep.abs() * r * 0.25)).min(256.0) as usize;
                    let step = sweep / n as f64;
                    let mut prev = *vertex + Vec2::new(a1.cos(), a1.sin()) * r;
                    for i in 1..=n {
                        let t = a1 + step * i as f64;
                        let p = *vertex + Vec2::new(t.cos(), t.sin()) * r;
                        segs.push((prev, p));
                        prev = p;
                    }
                    let _ = end;
                }
                segs
            }
            DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } => {
                // The dim arc (tessellated) + the leader tail.
                let r = *radius;
                let a1 = start_angle;
                let a2 = start_angle + sweep;
                let n = (8.0_f64.max(sweep.abs() * r * 0.25)).min(128.0) as usize;
                let step = sweep / n as f64;
                let mut segs = Vec::new();
                let mut prev = *center + Vec2::new(a1.cos(), a1.sin()) * r;
                for i in 1..=n {
                    let ang = a1 + step * i as f64;
                    let p = *center + Vec2::new(ang.cos(), ang.sin()) * r;
                    segs.push((prev, p));
                    prev = p;
                }
                segs.push((*center + Vec2::new(a2.cos(), a2.sin()) * r, *leader_end));
                segs
            }
            DimKind::Ordinate { point, leader_end, .. } =>
                vec![(*point, *leader_end)],
            DimKind::JoggedRadius { on_circle, leader_end, jog_pos, .. } =>
                vec![(*on_circle, *jog_pos), (*jog_pos, *leader_end)],
        }
    }

    /// Return a copy of this Dim with every defining point passed
    /// through `f`. Used by the kernel transforms (translated, rotated,
    /// scaled, mirrored) so each transform implementation reduces to a
    /// single line.
    pub fn with_points_mapped<F: Fn(Vec2) -> Vec2>(&self, f: F) -> Dim {
        let new_kind = match &self.kind {
            DimKind::Linear { p1, p2, dimline_pos, ortho } => DimKind::Linear {
                p1:          f(*p1),
                p2:          f(*p2),
                dimline_pos: f(*dimline_pos),
                ortho:       *ortho,
            },
            DimKind::Radius { center, on_circle, leader_end } => DimKind::Radius {
                center:     f(*center),
                on_circle:  f(*on_circle),
                leader_end: f(*leader_end),
            },
            DimKind::Diameter { center, on_circle, leader_end } => DimKind::Diameter {
                center:     f(*center),
                on_circle:  f(*on_circle),
                leader_end: f(*leader_end),
            },
            DimKind::Angular { vertex, p1, p2, arc_pos } => DimKind::Angular {
                vertex:  f(*vertex),
                p1:      f(*p1),
                p2:      f(*p2),
                arc_pos: f(*arc_pos),
            },
            DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } =>
                DimKind::ArcLen {
                    center: f(*center), radius: *radius,
                    start_angle: *start_angle, sweep: *sweep,
                    leader_end: f(*leader_end),
                },
            DimKind::Ordinate { datum, point, leader_end, is_x } =>
                DimKind::Ordinate {
                    datum: f(*datum), point: f(*point),
                    leader_end: f(*leader_end), is_x: *is_x,
                },
            DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos } =>
                DimKind::JoggedRadius {
                    center: f(*center), on_circle: f(*on_circle),
                    leader_end: f(*leader_end), jog_pos: f(*jog_pos),
                },
        };
        Dim { kind: new_kind, style: self.style, text_override: self.text_override.clone() }
    }

    /// Everything the dimension VISUALLY consists of, resolved against a
    /// `DimStyle`: extension lines, the dim line (or leader legs), arrow
    /// heads as `(tip, inward_dir)` and the text label anchor + rotation.
    /// This is the single source of truth for both the app renderer
    /// (draw_dimension) and plot export (Geom::to_vector_primitives —
    /// issue #10), so a drawn dimension and an exported one always match.
    pub fn render_geometry(&self, style: &DimStyle) -> DimRenderGeometry {
        let ext_extend_w = style.ext_line_extend * style.overall_scale;
        let ext_offset_w = style.ext_line_offset * style.overall_scale;
        let text_gap_w   = style.text_gap        * style.overall_scale;
        let text_h_w     = style.text_height     * style.overall_scale;
        let mut g = DimRenderGeometry {
            ext_lines: Vec::new(), dim_line: None, dim_arc: None, leaders: Vec::new(),
            arrows: Vec::new(), text_pos: Vec2::new(0.0, 0.0),
            text_angle: 0.0, text_on_dim_line: false,
        };
        match &self.kind {
            DimKind::Linear { p1, p2, dimline_pos, ortho } => {
                let chord = *p2 - *p1;
                if chord.len() < 1e-9 { g.text_pos = *p1; return g; }
                let (u, n) = match ortho {
                    LinearOrtho::Aligned => {
                        let u = chord.normalized();
                        (u, Vec2::new(-u.y, u.x))
                    }
                    LinearOrtho::Horizontal => (Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)),
                    LinearOrtho::Vertical   => (Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)),
                };
                let dim_offset = (*dimline_pos - *p1).dot(n);
                let n_signed = if dim_offset >= 0.0 { n } else { -n };
                let off_mag  = dim_offset.abs();
                let (a, b) = match ortho {
                    LinearOrtho::Aligned => (
                        *p1 + n_signed * off_mag,
                        *p2 + n_signed * off_mag,
                    ),
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
                    g.ext_lines.push((
                        *p1 + n_from_p1 * ext_offset_w,
                        a   + n_from_p1 * ext_extend_w,
                    ));
                }
                if !style.ext_suppress_2 {
                    g.ext_lines.push((
                        *p2 + n_from_p2 * ext_offset_w,
                        b   + n_from_p2 * ext_extend_w,
                    ));
                }
                g.dim_line = Some((a, b));
                let dim_dir = (b - a).normalized();
                g.arrows.push((a,  dim_dir));
                g.arrows.push((b, -dim_dir));
                // Text placement from DIMTAD (text_vert_pos): 0 = on the line
                // (line gets trimmed), 4 = below, else above.
                let mid  = (a + b) * 0.5;
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
                    if ang >  std::f64::consts::FRAC_PI_2 { ang -= std::f64::consts::PI; }
                    if ang < -std::f64::consts::FRAC_PI_2 { ang += std::f64::consts::PI; }
                    g.text_angle = ang;
                }
                g
            }
            DimKind::Radius { center, on_circle, leader_end } => {
                // ONE radius line, centre → arc edge, arrow at the edge. The
                // pick point (`leader_end`) only positions the TEXT.
                g.leaders.push((*center, *on_circle));
                let radial = (*on_circle - *center).normalized();
                g.arrows.push((*on_circle, -radial));
                let outward = (*leader_end - *center).normalized();
                g.text_pos = *leader_end + outward * (text_gap_w + text_h_w * 0.5);
                g
            }
            DimKind::Diameter { center, on_circle, leader_end } => {
                // ONE diameter line through the centre, arrows at both ends.
                let opp = *center * 2.0 - *on_circle;
                g.leaders.push((*on_circle, opp));
                let radial = (*on_circle - *center).normalized();
                g.arrows.push((*on_circle, -radial));
                g.arrows.push((opp,         radial));
                let outward = (*leader_end - *center).normalized();
                g.text_pos = *leader_end + outward * (text_gap_w + text_h_w * 0.5);
                g
            }
            DimKind::Angular { vertex, p1, p2, arc_pos } => {
                // Extension lines run from the vertex outward through the
                // rays (with the style's offset/extend applied).
                let ray1 = (*p1 - *vertex).normalized();
                let ray2 = (*p2 - *vertex).normalized();
                g.ext_lines.push((
                    *vertex + ray1 * ext_offset_w,
                    *p1     + ray1 * ext_extend_w,
                ));
                g.ext_lines.push((
                    *vertex + ray2 * ext_offset_w,
                    *p2     + ray2 * ext_extend_w,
                ));
                let r = (*arc_pos - *vertex).len();
                if r > 1e-9 {
                    let (a1, sweep) = angular_arc(vertex, p1, p2, arc_pos);
                    g.dim_arc = Some((*vertex, r, a1, sweep));
                    // Arrows at both arc ends, tangent to the arc.
                    let end_ang = a1 + sweep;
                    let tan_start = Vec2::new(-(a1.sin()), a1.cos());
                    let tan_end   = Vec2::new(-(end_ang.sin()), end_ang.cos());
                    let dir_start = if sweep >= 0.0 { tan_start } else { -tan_start };
                    let dir_end   = if sweep >= 0.0 { -tan_end } else { tan_end };
                    g.arrows.push((
                        *vertex + Vec2::new(a1.cos(), a1.sin()) * r, dir_start));
                    g.arrows.push((
                        *vertex + Vec2::new(end_ang.cos(), end_ang.sin()) * r, dir_end));
                    // Text at the arc midpoint, outside the arc.
                    let mid = a1 + sweep * 0.5;
                    let lift = text_gap_w + text_h_w * 0.5;
                    g.text_pos = *vertex + Vec2::new(mid.cos(), mid.sin()) * (r + lift);
                    // Text angle = tangent at midpoint (readability-corrected).
                    let mut tang = Vec2::new(-(mid.sin()), mid.cos());
                    let mut ang = tang.y.atan2(tang.x);
                    if ang >  std::f64::consts::FRAC_PI_2 { ang -= std::f64::consts::PI; }
                    if ang < -std::f64::consts::FRAC_PI_2 { ang += std::f64::consts::PI; }
                    g.text_angle = ang;
                    let _ = tang;
                } else {
                    g.text_pos = *arc_pos;
                }
                g
            }
            DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } => {
                // The dim ARC (drawn at the measured radius) with arrows at
                // both ends + a leader from the arc end to the text.
                let a1 = start_angle;
                let a2 = start_angle + sweep;
                g.dim_arc = Some((*center, *radius, *a1, *sweep));
                let s1 = *center + Vec2::new(a1.cos(), a1.sin()) * *radius;
                let s2 = *center + Vec2::new(a2.cos(), a2.sin()) * *radius;
                let tan1 = Vec2::new(-(a1.sin()), a1.cos());
                let tan2 = Vec2::new(-(a2.sin()), a2.cos());
                let dir1 = if *sweep >= 0.0 { tan1 } else { -tan1 };
                let dir2 = if *sweep >= 0.0 { -tan2 } else { tan2 };
                g.arrows.push((s1, dir1));
                g.arrows.push((s2, dir2));
                g.leaders.push((s2, *leader_end));
                g.text_pos = *leader_end + (*leader_end - s2).normalized()
                    * (text_gap_w + text_h_w * 0.5);
                g
            }
            DimKind::Ordinate { datum, point, leader_end, is_x } => {
                // A short perpendicular tick at the datum, the leader from
                // the measured point, and the text at the leader end.
                let dir = *leader_end - *point;
                let d = dir.len();
                let u = if d > 1e-9 { dir / d } else { Vec2::new(1.0, 0.0) };
                let n = Vec2::new(-u.y, u.x);
                let tick = text_h_w * 0.4;
                g.ext_lines.push((
                    *datum - n * tick,
                    *datum + n * tick,
                ));
                // Leader from the point; bend toward the leader end.
                g.leaders.push((*point, *leader_end));
                g.arrows.push((*point, if *is_x { -u } else { -u }));
                g.text_pos = *leader_end + u * (text_gap_w + text_h_w * 0.5);
                g
            }
            DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos } => {
                // Radial dim line centre→circle (arrow at the circle), then
                // a JOGGED leader: on_circle → jog_pos → leader_end.
                g.leaders.push((*center, *on_circle));
                let radial = (*on_circle - *center).normalized();
                g.arrows.push((*on_circle, -radial));
                g.leaders.push((*on_circle, *jog_pos));
                g.leaders.push((*jog_pos, *leader_end));
                g.text_pos = *leader_end + (*leader_end - *jog_pos).normalized()
                    * (text_gap_w + text_h_w * 0.5);
                g
            }
        }
    }
}

/// For an angular dim: the ray angle of `p1` around `vertex`, and the
/// sweep from p1's ray to p2's ray that passes THROUGH `arc_pos`.
/// Returns (start_angle, sweep) with sweep in (-2π, 2π) — sign tells
/// the renderer which way the arc turns.
fn angular_arc(
    vertex: &Vec2, p1: &Vec2, p2: &Vec2, arc_pos: &Vec2,
) -> (f64, f64) {
    let a1 = (*p1 - *vertex).angle();
    let a2 = (*p2 - *vertex).angle();
    let ap = (*arc_pos - *vertex).angle();
    // CCW sweep a1 → a2 (0..2π).
    let ccw = (a2 - a1).rem_euclid(std::f64::consts::TAU);
    // Does the CCW arc pass through ap?
    let t = (ap - a1).rem_euclid(std::f64::consts::TAU);
    if t <= ccw {
        (a1, ccw)
    } else {
        // The other way: CW sweep (negative).
        (a1, ccw - std::f64::consts::TAU)
    }
}

/// The resolved visual geometry of a dimension (see
/// [`Dim::render_geometry`]).
#[derive(Clone, Debug)]
pub struct DimRenderGeometry {
    /// Extension lines — drawn in the EXT-line color.
    pub ext_lines: Vec<(Vec2, Vec2)>,
    /// The Linear dim line (None for radius/diameter) — gap-trim candidate.
    pub dim_line:  Option<(Vec2, Vec2)>,
    /// Angular dim ARC as (center, radius, start_angle, sweep) — the
    /// swept arc between the two rays, passing through the arc_pos
    /// click. Sweep sign = turn direction (CCW positive).
    pub dim_arc:   Option<(Vec2, f64, f64, f64)>,
    /// Radius/diameter leader legs — drawn in the DIM-line color.
    pub leaders:   Vec<(Vec2, Vec2)>,
    /// Arrowheads as `(tip, inward_dir)`.
    pub arrows:    Vec<(Vec2, Vec2)>,
    /// Text label anchor (world).
    pub text_pos:  Vec2,
    /// World rotation for the text (0 = horizontal). Aligned linear dims
    /// set this to the readability-corrected dim-line angle.
    pub text_angle: f64,
    /// True when the text sits ON the dim line (the line is gap-trimmed).
    pub text_on_dim_line: bool,
}

impl DimRenderGeometry {
    /// Every visible line (extension lines + dim line + leaders) — used by
    /// click hit-testing so the user can pick a dimension by clicking on
    /// any line they see.
    pub fn all_lines(&self) -> Vec<(Vec2, Vec2)> {
        let mut out = self.ext_lines.clone();
        if let Some((a, b)) = self.dim_line {
            out.push((a, b));
        }
        if let Some((c, r, a1, sweep)) = self.dim_arc {
            // Tessellate the arc so hit-testing sees its curve.
            let n = (24.0_f64.max(sweep.abs() * r * 0.25)).min(256.0) as usize;
            let step = sweep / n as f64;
            let mut prev = c + Vec2::new(a1.cos(), a1.sin()) * r;
            for i in 1..=n {
                let t = a1 + step * i as f64;
                let p = c + Vec2::new(t.cos(), t.sin()) * r;
                out.push((prev, p));
                prev = p;
            }
        }
        out.extend(self.leaders.iter().copied());
        out
    }
}

// ---------------------------------------------------------------------------
// DimStyle — the ~70-DIMVAR AutoCAD-parity set.
// ---------------------------------------------------------------------------
//
// Field naming uses descriptive Rust names. The DXF/RSM serializer
// owns the mapping from these to DIMVAR codes (DIMASZ, DIMTXT, …).
// Per the project's `feedback_rust_cad_settings_naming` memo, the
// cryptic short-name convention is reserved for app-level settings
// (UserEnv); per-entity style data uses readable names.

#[derive(Clone, Debug, PartialEq)]
pub struct DimStyle {
    pub name:                String,

    // ---- arrows -----------------------------------------------------
    /// DIMASZ — arrow head size (world units).
    pub arrow_size:          f64,
    /// DIMBLK — name of the arrow block (empty = filled triangle).
    pub arrow_block:         String,
    /// DIMBLK1 / DIMBLK2 — separate per-end arrow block names; only
    /// used when `separate_arrows` is true.
    pub arrow_block_1:       String,
    pub arrow_block_2:       String,
    /// DIMSAH — when true, each arrow uses its block_1 / block_2 name
    /// instead of `arrow_block`.
    pub separate_arrows:     bool,
    /// DIMLDRBLK — leader arrow block name.
    pub leader_block:        String,
    /// DIMTSZ — tick size; when > 0 the arrows render as oblique
    /// architectural ticks of this size instead of arrowheads.
    pub tick_size:           f64,
    /// Whether the triangular arrowhead is filled solid (true) or drawn
    /// as an open/hollow outline (false). Ignored when `tick_size > 0`
    /// (ticks are always strokes). Not a stock DIMVAR — AutoCAD encodes
    /// open vs filled via the arrow block name; we keep an explicit flag.
    pub arrow_filled:        bool,

    // ---- text -------------------------------------------------------
    /// DIMTXT — text height in world units.
    pub text_height:         f64,
    /// DIMGAP — gap between the dim line and the text.
    pub text_gap:            f64,
    /// DIMTXSTY — text style name (resolved against `Document.text_styles`).
    pub text_style_name:     String,
    /// DIMTAD — text vertical position (0 = centred on dim line,
    /// 1 = above dim line, 2 = outside view, 3 = JIS, 4 = below).
    pub text_vert_pos:       i32,
    /// DIMJUST — text horizontal justification (0 = centre, 1 = next
    /// to first ext, 2 = next to second ext, 3 = above first ext,
    /// 4 = above second ext).
    pub text_horiz_just:     i32,
    /// DIMTVP — explicit text vertical position offset (used when
    /// DIMTAD = 0).
    pub text_vert_offset:    f64,
    /// DIMTIH — text inside extensions reads horizontal.
    pub text_inside_horiz:   bool,
    /// DIMTOH — text outside extensions reads horizontal.
    pub text_outside_horiz:  bool,
    /// DIMTIX — force text inside extensions.
    pub text_force_inside:   bool,
    /// DIMTOFL — force dim line inside extensions even when text
    /// gets placed outside.
    pub text_force_dimline:  bool,
    /// DIMUPT — user-positioned text (true: user clicks the text
    /// position; false: auto-centred between extensions).
    pub text_user_positioned: bool,
    /// DIMTMOVE — text move rule (0 = with dim line, 1 = move dim
    /// line with text, 2 = move text only, leader added).
    pub text_move_rule:      i32,

    // ---- linear units -----------------------------------------------
    /// DIMLUNIT — linear unit format (1 = scientific, 2 = decimal,
    /// 3 = engineering, 4 = architectural, 5 = fractional, 6 = Windows
    /// desktop).
    pub linear_unit_format:  i32,
    /// DIMDEC — linear decimal places.
    pub decimal_places:      i32,
    /// DIMRND — round measured values to this increment. 0 = no rounding.
    pub rounding:            f64,
    /// DIMZIN — zero-suppression flags (0 = none, 4 = leading,
    /// 8 = trailing, 12 = both, 1 / 2 = feet-only / inches-only).
    pub zero_suppress:       i32,
    /// DIMFRAC — fraction format for unit formats 4 & 5 (0 = horiz,
    /// 1 = diagonal, 2 = not stacked).
    pub fraction_format:     i32,
    /// DIMDSEP — decimal separator character.
    pub decimal_separator:   char,
    /// DIMLFAC — linear scale factor applied to measured value.
    pub linear_scale:        f64,
    /// DIMPOST — prefix/suffix for the formatted text (e.g. " mm",
    /// or "<>U" where "<>" is the measurement placeholder).
    pub linear_post:         String,

    // ---- alternate units --------------------------------------------
    /// DIMALT — display alternate units alongside primary.
    pub alt_units_enabled:   bool,
    /// DIMALTU — alt unit format (same options as DIMLUNIT).
    pub alt_unit_format:     i32,
    /// DIMALTD — alt unit decimal places.
    pub alt_decimal_places:  i32,
    /// DIMALTF — alt unit scale factor (default 25.4 mm/inch).
    pub alt_scale:           f64,
    /// DIMALTRND — alt rounding increment.
    pub alt_rounding:        f64,
    /// DIMALTZ — alt zero suppression.
    pub alt_zero_suppress:   i32,
    /// DIMAPOST — alt prefix/suffix.
    pub alt_post:            String,
    /// DIMARCSYM — arc length symbol position (0 = preceding text,
    /// 1 = above text, 2 = not displayed).
    pub arc_length_symbol:   i32,

    // ---- angular units ----------------------------------------------
    /// DIMAUNIT — angular unit format (0 = decimal degrees, 1 = DMS,
    /// 2 = grads, 3 = radians, 4 = surveyor's units).
    pub angular_unit_format: i32,
    /// DIMADEC — angular decimal places (-1 = use DIMDEC).
    pub angular_decimal_places: i32,
    /// DIMAZIN — angular zero suppression.
    pub angular_zero_suppress: i32,

    // ---- tolerance --------------------------------------------------
    /// DIMTOL — display tolerance pair.
    pub tolerance_enabled:   bool,
    /// DIMTP / DIMTM — upper / lower tolerance values.
    pub tolerance_plus:      f64,
    pub tolerance_minus:     f64,
    /// DIMTDEC — tolerance decimal places.
    pub tolerance_decimal_places: i32,
    /// DIMTFAC — tolerance text scale factor.
    pub tolerance_text_scale: f64,
    /// DIMTOLJ — tolerance vertical justification (0 = bottom,
    /// 1 = middle, 2 = top).
    pub tolerance_vert_just: i32,
    /// DIMTZIN — tolerance zero suppression.
    pub tolerance_zero_suppress: i32,
    /// DIMLIM — display tolerance as limits.
    pub limits_enabled:      bool,
    /// DIMALTTD / DIMALTTZ — alt tolerance decimal places / zero
    /// suppression.
    pub alt_tolerance_decimal_places: i32,
    pub alt_tolerance_zero_suppress:  i32,

    // ---- extension lines --------------------------------------------
    /// DIMEXE — distance the extension line extends BEYOND the dim line.
    pub ext_line_extend:     f64,
    /// DIMEXO — gap between the def point and the start of the ext line.
    pub ext_line_offset:     f64,
    /// DIMSE1 / DIMSE2 — suppress ext line 1 / 2.
    pub ext_suppress_1:      bool,
    pub ext_suppress_2:      bool,
    /// DIMFXL / DIMFXLON — fixed extension line length (when enabled,
    /// ext lines have this exact length regardless of dim line offset).
    pub ext_fixed_length:    f64,
    pub ext_fixed_length_on: bool,
    /// DIMLTEX1 / DIMLTEX2 — per-ext-line linetype names.
    pub ext_linetype_1:      String,
    pub ext_linetype_2:      String,

    // ---- dim line ---------------------------------------------------
    /// DIMDLE — distance the dim line extends BEYOND the ext lines
    /// when tick-style arrows are used.
    pub dim_line_extend:     f64,
    /// DIMDLI — baseline-stacking increment (vertical gap between
    /// stacked baseline dims).
    pub dim_line_baseline_inc: f64,
    /// DIMSD1 / DIMSD2 — suppress dim line halves on the 1st / 2nd
    /// arrow side.
    pub dim_suppress_1:      bool,
    pub dim_suppress_2:      bool,
    /// DIMSOXD — suppress dim line outside ext lines.
    pub dim_suppress_outside: bool,
    /// DIMLTYPE — dim line linetype name.
    pub dim_linetype:        String,

    // ---- colors -----------------------------------------------------
    /// DIMCLRD — dim line color (0 = ByBlock).
    pub color_dim_line:      u32,
    /// DIMCLRE — ext line color.
    pub color_ext_line:      u32,
    /// DIMCLRT — text color.
    pub color_text:          u32,
    /// DIMTFILL — text background fill (0 = none, 1 = drawing bg,
    /// 2 = explicit fill_color).
    pub text_fill_mode:      i32,
    /// DIMTFILLCLR — explicit text fill color.
    pub text_fill_color:     u32,

    // ---- lineweights ------------------------------------------------
    /// DIMLWD / DIMLWE — dim line / ext line lineweights (-2 = ByBlock,
    /// -1 = ByLayer, otherwise hundredths of a mm).
    pub lineweight_dim_line: i16,
    pub lineweight_ext_line: i16,

    // ---- scale + radius -dim-specific -------------------------------
    /// DIMSCALE — overall scale factor multiplying every other length.
    pub overall_scale:       f64,
    /// DIMCEN — center mark size (positive = mark, negative = mark +
    /// crosshair lines, 0 = none).
    pub center_mark_size:    f64,
    /// DIMJOGANG — angle of the jog symbol on jogged radius dims.
    pub jog_angle:           f64,

    // ---- arrow-fit + text-fit ---------------------------------------
    /// DIMATFIT — what to move when arrows + text don't fit (0 = both
    /// outside, 1 = arrows first, 2 = text first, 3 = whatever fits).
    pub arrow_text_fit:      i32,
}

impl DimStyle {
    /// AutoCAD's STANDARD style with default values. Always id 0 in
    /// `DimStyleTable`.
    pub fn standard() -> Self {
        Self {
            name:                "STANDARD".into(),

            arrow_size:          0.18,
            arrow_block:         String::new(),
            arrow_block_1:       String::new(),
            arrow_block_2:       String::new(),
            separate_arrows:     false,
            leader_block:        String::new(),
            tick_size:           0.0,
            arrow_filled:        true,

            text_height:         0.18,
            text_gap:            0.09,
            text_style_name:     "STANDARD".into(),
            text_vert_pos:       0,
            text_horiz_just:     0,
            text_vert_offset:    0.0,
            text_inside_horiz:   true,
            text_outside_horiz:  true,
            text_force_inside:   false,
            text_force_dimline:  false,
            text_user_positioned: false,
            text_move_rule:      0,

            linear_unit_format:  2,
            decimal_places:      4,
            rounding:            0.0,
            zero_suppress:       0,
            fraction_format:     0,
            decimal_separator:   '.',
            linear_scale:        1.0,
            linear_post:         String::new(),

            alt_units_enabled:   false,
            alt_unit_format:     2,
            alt_decimal_places:  2,
            alt_scale:           25.4,
            alt_rounding:        0.0,
            alt_zero_suppress:   0,
            alt_post:            String::new(),
            arc_length_symbol:   0,

            angular_unit_format: 0,
            angular_decimal_places: -1,
            angular_zero_suppress: 0,

            tolerance_enabled:   false,
            tolerance_plus:      0.0,
            tolerance_minus:     0.0,
            tolerance_decimal_places: 4,
            tolerance_text_scale: 1.0,
            tolerance_vert_just: 1,
            tolerance_zero_suppress: 0,
            limits_enabled:      false,
            alt_tolerance_decimal_places: 2,
            alt_tolerance_zero_suppress:  0,

            ext_line_extend:     0.18,
            ext_line_offset:     0.0625,
            ext_suppress_1:      false,
            ext_suppress_2:      false,
            ext_fixed_length:    1.0,
            ext_fixed_length_on: false,
            ext_linetype_1:      String::new(),
            ext_linetype_2:      String::new(),

            dim_line_extend:     0.0,
            dim_line_baseline_inc: 0.38,
            dim_suppress_1:      false,
            dim_suppress_2:      false,
            dim_suppress_outside: false,
            dim_linetype:        String::new(),

            color_dim_line:      0,
            color_ext_line:      0,
            color_text:          0,
            text_fill_mode:      0,
            text_fill_color:     0,

            lineweight_dim_line: -2,
            lineweight_ext_line: -2,

            overall_scale:       1.0,
            center_mark_size:    0.09,
            jog_angle:           std::f64::consts::FRAC_PI_4 + 0.0,  // 45° ish

            arrow_text_fit:      3,
        }
    }
}

// ---------------------------------------------------------------------------
// DimStyleTable — analog of TextStyleTable / LayerTable.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DimStyleTable {
    pub styles: Vec<DimStyle>,
}

impl DimStyleTable {
    pub const STANDARD: u32 = 0;

    pub fn with_defaults() -> Self {
        Self { styles: vec![DimStyle::standard()] }
    }
    pub fn get(&self, id: u32) -> Option<&DimStyle> {
        self.styles.get(id as usize)
    }
    pub fn add(&mut self, s: DimStyle) -> u32 {
        let id = self.styles.len() as u32;
        self.styles.push(s);
        id
    }
    pub fn find(&self, name: &str) -> Option<u32> {
        self.styles.iter().position(|s| s.name.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }
    pub fn len(&self) -> usize { self.styles.len() }
    pub fn is_empty(&self) -> bool { self.styles.is_empty() }
}

impl Default for DimStyleTable {
    fn default() -> Self { Self::with_defaults() }
}

// ---------------------------------------------------------------------------
// Formatting helpers.
// ---------------------------------------------------------------------------

/// Round `v` to the nearest multiple of `step`. `step == 0` = no
/// rounding (return v unchanged).
fn round_to(v: f64, step: f64) -> f64 {
    if step.abs() < 1e-12 { v } else { (v / step).round() * step }
}

/// AutoCAD DIMZIN-style zero suppression. Bit values that matter here:
///   * 0  — none (display all zeros)
///   * 4  — suppress LEADING zeros (0.5 → .5)
///   * 8  — suppress TRAILING zeros (0.5000 → 0.5)
///   * 12 — both
/// The feet/inches bits (1, 2) are ignored for v1 — only decimal
/// formatting is supported. Empty string after suppression collapses
/// to "0".
fn suppress_zeros(mut s: String, flags: i32) -> String {
    let suppress_trailing = (flags & 8) != 0;
    let suppress_leading  = (flags & 4) != 0;
    if suppress_trailing && s.contains('.') {
        while s.ends_with('0') { s.pop(); }
        if s.ends_with('.') { s.pop(); }
    }
    if suppress_leading {
        if let Some(rest) = s.strip_prefix("0.") {
            s = format!(".{}", rest);
        }
    }
    if s.is_empty() { return "0".into(); }
    s
}

/// Parse a DIMPOST-style string into a (prefix, suffix) pair. AutoCAD
/// uses `<>` as the measured-value placeholder; we honour that. If no
/// `<>` is present, the whole string is treated as a SUFFIX (the
/// common case — e.g. " mm").
fn parse_dimpost(post: &str) -> (String, String) {
    if post.is_empty() { return (String::new(), String::new()); }
    if let Some(idx) = post.find("<>") {
        let pre  = &post[..idx];
        let suf  = &post[idx + 2..];
        (pre.to_string(), suf.to_string())
    } else {
        (String::new(), post.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_present_at_id_zero() {
        let t = DimStyleTable::with_defaults();
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(0).unwrap().name, "STANDARD");
    }

    #[test]
    fn measured_value_linear_aligned() {
        let d = Dim {
            kind: DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(3.0, 4.0),
                dimline_pos: Vec2::new(0.0, 5.0),
                ortho: LinearOrtho::Aligned,
            },
            style: 0,
            text_override: None,
        };
        assert!((d.measured_value() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn measured_value_linear_horizontal_ignores_y() {
        let d = Dim {
            kind: DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(7.0, 99.0),
                dimline_pos: Vec2::new(0.0, 10.0),
                ortho: LinearOrtho::Horizontal,
            },
            style: 0,
            text_override: None,
        };
        assert!((d.measured_value() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn measured_value_diameter_is_twice_radius() {
        let d = Dim {
            kind: DimKind::Diameter {
                center: Vec2::new(0.0, 0.0),
                on_circle: Vec2::new(5.0, 0.0),
                leader_end: Vec2::new(10.0, 0.0),
            },
            style: 0,
            text_override: None,
        };
        assert!((d.measured_value() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn formatted_text_includes_radius_prefix() {
        let st = DimStyle::standard();
        let d = Dim {
            kind: DimKind::Radius {
                center: Vec2::new(0.0, 0.0),
                on_circle: Vec2::new(5.0, 0.0),
                leader_end: Vec2::new(10.0, 0.0),
            },
            style: 0,
            text_override: None,
        };
        let s = d.formatted_text(&st);
        assert!(s.starts_with('R'), "got: {}", s);
        assert!(s.contains("5"), "got: {}", s);
    }

    #[test]
    fn formatted_text_diameter_prefix() {
        let st = DimStyle::standard();
        let d = Dim {
            kind: DimKind::Diameter {
                center: Vec2::new(0.0, 0.0),
                on_circle: Vec2::new(5.0, 0.0),
                leader_end: Vec2::new(10.0, 0.0),
            },
            style: 0,
            text_override: None,
        };
        let s = d.formatted_text(&st);
        assert!(s.starts_with('\u{2300}'), "got: {}", s);
    }

    #[test]
    fn text_override_with_placeholder_substitutes_value() {
        let st = DimStyle::standard();
        let d = Dim {
            kind: DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(5.0, 0.0),
                dimline_pos: Vec2::new(0.0, 1.0),
                ortho: LinearOrtho::Aligned,
            },
            style: 0,
            text_override: Some("~<> mm".into()),
        };
        assert!(d.formatted_text(&st).starts_with("~5"));
        assert!(d.formatted_text(&st).ends_with(" mm"));
    }

    #[test]
    fn zero_suppression_trailing_works() {
        assert_eq!(suppress_zeros("1.5000".into(), 8), "1.5");
        assert_eq!(suppress_zeros("1.0000".into(), 8), "1");
    }

    #[test]
    fn zero_suppression_leading_works() {
        assert_eq!(suppress_zeros("0.5".into(), 4), ".5");
    }

    #[test]
    fn linear_scale_multiplies_value() {
        let mut st = DimStyle::standard();
        st.linear_scale = 25.4;     // mm per inch
        st.decimal_places = 2;
        let d = Dim {
            kind: DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(1.0, 0.0),
                dimline_pos: Vec2::new(0.0, 1.0),
                ortho: LinearOrtho::Aligned,
            },
            style: 0,
            text_override: None,
        };
        assert!(d.formatted_text(&st).starts_with("25.40"));
    }

    #[test]
    fn rounding_step_applies() {
        let mut st = DimStyle::standard();
        st.rounding = 0.5;
        st.decimal_places = 2;
        let d = Dim {
            kind: DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(3.4, 0.0),
                dimline_pos: Vec2::new(0.0, 1.0),
                ortho: LinearOrtho::Aligned,
            },
            style: 0,
            text_override: None,
        };
        // 3.4 rounds to 3.5 at step 0.5
        let s = d.formatted_text(&st);
        assert!(s.starts_with("3.50"), "got: {}", s);
    }
}

#[cfg(test)]
mod angular_tests {
    use super::*;
    use crate::math::Vec2;

    fn close(a: Vec2, b: Vec2) -> bool { (a - b).len() < 1e-6 }

    fn angular(vertex: Vec2, p1: Vec2, p2: Vec2, arc_pos: Vec2) -> Dim {
        Dim {
            kind: DimKind::Angular { vertex, p1, p2, arc_pos },
            style: 0,
            text_override: None,
        }
    }

    #[test]
    fn measured_value_right_angle_is_90_degrees() {
        let d = angular(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(4.0, 4.0),
        );
        assert!((d.measured_value() - 90.0).abs() < 1e-9,
            "got {}", d.measured_value());
    }

    #[test]
    fn measured_value_45_degrees() {
        let d = angular(
            Vec2::new(5.0, 5.0),
            Vec2::new(10.0, 5.0),
            Vec2::new(5.0 + 7.071, 5.0 + 7.071),
            Vec2::new(7.0, 7.0),
        );
        assert!((d.measured_value() - 45.0).abs() < 1e-6,
            "got {}", d.measured_value());
    }

    #[test]
    fn measured_value_obtuse_135() {
        let d = angular(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(-7.071, 7.071),
            Vec2::new(-2.0, 2.0),
        );
        assert!((d.measured_value() - 135.0).abs() < 1e-6,
            "got {}", d.measured_value());
    }

    #[test]
    fn measured_value_never_exceeds_180() {
        // The SMALLER angle between the rays — p2 at 270° vs p1 at 0°.
        let d = angular(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, -10.0),
            Vec2::new(1.0, -1.0),
        );
        assert!((d.measured_value() - 90.0).abs() < 1e-9,
            "got {}", d.measured_value());
    }

    #[test]
    fn formatted_text_has_degree_symbol() {
        let d = angular(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(4.0, 4.0),
        );
        let st = DimStyle::standard();
        let s = d.formatted_text(&st);
        assert!(s.ends_with('\u{00B0}'), "got: {s}");
        assert!(s.contains("90"), "got: {s}");
    }

    #[test]
    fn render_geometry_arc_passes_through_arc_pos() {
        let vertex = Vec2::new(0.0, 0.0);
        let p1 = Vec2::new(10.0, 0.0);
        let p2 = Vec2::new(0.0, 10.0);
        let arc_pos = Vec2::new(5.0, 5.0);
        let d = angular(vertex, p1, p2, arc_pos);
        let st = DimStyle::standard();
        let g = d.render_geometry(&st);
        let (c, r, a1, sweep) = g.dim_arc.expect("angular dim must emit an arc");
        assert!(close(c, vertex));
        assert!((r - (arc_pos - vertex).len()).abs() < 1e-9, "radius {r}");
        // The arc's start angle is the p1 ray.
        assert!((a1 - 0.0).abs() < 1e-9);
        // Sweep is CCW p1(0°) → p2(90°): +90°.
        assert!((sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-9,
            "sweep {sweep}");
        // Two extension lines + an arc + 2 arrows + text.
        assert_eq!(g.ext_lines.len(), 2);
        assert_eq!(g.arrows.len(), 2);
        // Midpoint of the arc at 45° radius r → text sits outside it.
        let mid = vertex + Vec2::new(45f64.to_radians().cos(),
                                     45f64.to_radians().sin()) * r;
        assert!((g.text_pos - mid).len() > 1e-3, "text lifts off the arc");
        assert!((g.text_pos - vertex).len() > r, "text beyond the arc radius");
    }

    #[test]
    fn render_geometry_major_side_arc_pos_sweeps_the_long_way() {
        // arc_pos on the MAJOR side (240°) → the arc sweeps 270° CW-ish
        // instead of the 90° CCW minor arc.
        let vertex = Vec2::new(0.0, 0.0);
        let p1 = Vec2::new(10.0, 0.0);
        let p2 = Vec2::new(0.0, 10.0);
        let arc_pos = Vec2::new(
            4.0 * 240f64.to_radians().cos(),
            4.0 * 240f64.to_radians().sin());
        let d = angular(vertex, p1, p2, arc_pos);
        let st = DimStyle::standard();
        let g = d.render_geometry(&st);
        let (_c, _r, _a1, sweep) = g.dim_arc.expect("arc");
        // 240° is NOT on the 0→90 CCW arc → sweep goes the other way
        // (negative, |sweep| = 270°).
        assert!(sweep < 0.0, "sweep {sweep}");
        assert!((sweep.abs() - 3.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }

    #[test]
    fn bbox_covers_vertex_and_arc() {
        let d = angular(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(5.0, 5.0),
        );
        let (mn, mx) = d.bbox();
        assert!(mn.x <= 0.0 && mn.y <= 0.0);
        assert!(mx.x >= 10.0 && mx.y >= 10.0);
    }

    #[test]
    fn grips_and_transform_keep_all_four_points() {
        let d = angular(
            Vec2::new(1.0, 1.0),
            Vec2::new(11.0, 1.0),
            Vec2::new(1.0, 11.0),
            Vec2::new(5.0, 5.0),
        );
        assert_eq!(d.grip_points().len(), 4);
        let t = d.with_points_mapped(|p| p + Vec2::new(100.0, 0.0));
        match t.kind {
            DimKind::Angular { vertex, p1, p2, arc_pos } => {
                assert!(close(vertex, Vec2::new(101.0, 1.0)));
                assert!(close(p1, Vec2::new(111.0, 1.0)));
                assert!(close(p2, Vec2::new(101.0, 11.0)));
                assert!(close(arc_pos, Vec2::new(105.0, 5.0)));
            }
            _ => panic!("kind lost"),
        }
    }

    #[test]
    fn outline_segments_include_rays_and_arc() {
        let d = angular(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(5.0, 5.0),
        );
        let segs = d.outline_segments();
        // 2 rays + tessellated arc segments.
        assert!(segs.len() >= 26, "got {}", segs.len());
    }
}

#[cfg(test)]
mod dim_ext_kinds_tests {
    use super::*;

    #[test]
    fn arc_len_measures_along_the_sweep() {
        let d = Dim {
            kind: DimKind::ArcLen {
                center: Vec2::ZERO, radius: 10.0,
                start_angle: 0.0, sweep: std::f64::consts::FRAC_PI_2,
                leader_end: Vec2::new(15.0, 5.0),
            },
            style: 0, text_override: None,
        };
        // 10 * π/2
        assert!((d.measured_value() - 10.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        // Sweep sign ignored.
        let neg = Dim {
            kind: DimKind::ArcLen {
                center: Vec2::ZERO, radius: 10.0,
                start_angle: 0.0, sweep: -std::f64::consts::FRAC_PI_2,
                leader_end: Vec2::new(15.0, 5.0),
            },
            style: 0, text_override: None,
        };
        assert!((neg.measured_value() - 10.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }

    #[test]
    fn ordinate_measures_the_axis_delta() {
        let x = Dim {
            kind: DimKind::Ordinate {
                datum: Vec2::new(1.0, 5.0), point: Vec2::new(7.0, 5.5),
                leader_end: Vec2::new(9.0, 5.5), is_x: true,
            },
            style: 0, text_override: None,
        };
        assert!((x.measured_value() - 6.0).abs() < 1e-9);
        assert_eq!(x.formatted_text(&DimStyle::standard()), "X=6.0000");
        let y = Dim {
            kind: DimKind::Ordinate {
                datum: Vec2::new(1.0, 5.0), point: Vec2::new(7.0, 11.0),
                leader_end: Vec2::new(7.0, 13.0), is_x: false,
            },
            style: 0, text_override: None,
        };
        assert!((y.measured_value() - 6.0).abs() < 1e-9);
        assert_eq!(y.formatted_text(&DimStyle::standard()), "Y=6.0000");
    }

    #[test]
    fn jogged_radius_measures_the_radius() {
        let d = Dim {
            kind: DimKind::JoggedRadius {
                center: Vec2::ZERO, on_circle: Vec2::new(4.0, 3.0),
                leader_end: Vec2::new(12.0, 6.0), jog_pos: Vec2::new(8.0, 2.0),
            },
            style: 0, text_override: None,
        };
        assert!((d.measured_value() - 5.0).abs() < 1e-9);
        assert!(d.formatted_text(&DimStyle::standard()).starts_with('R'));
        // outline: jogged leader is 2 segments.
        assert_eq!(d.outline_segments().len(), 2);
    }

    #[test]
    fn arc_len_grips_and_transforms() {
        let d = Dim {
            kind: DimKind::ArcLen {
                center: Vec2::ZERO, radius: 10.0,
                start_angle: 0.0, sweep: std::f64::consts::FRAC_PI_2,
                leader_end: Vec2::new(15.0, 5.0),
            },
            style: 0, text_override: None,
        };
        assert_eq!(d.grip_points().len(), 4);
        // with_points_mapped is the single transform point — check it maps
        // every def point of the ArcLen kind.
        let t = d.with_points_mapped(|p| p + Vec2::new(100.0, 0.0));
        let DimKind::ArcLen { center, leader_end, .. } = &t.kind else { panic!("kind lost") };
        assert!((center.x - 100.0).abs() < 1e-9);
        assert!((leader_end.x - 115.0).abs() < 1e-9);
    }
}
