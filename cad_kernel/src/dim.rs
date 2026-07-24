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
        // user overrides via DIMPOST.
        let prefix = match &self.kind {
            DimKind::Radius { .. }   => "R",
            DimKind::Diameter { .. } => "\u{2300}",      // ⌀
            DimKind::Linear { .. }   => "",
        };
        let (post_pre, post_suf) = parse_dimpost(&style.linear_post);
        // post_pre comes BEFORE the prefix (rare); post_suf comes after
        // the number. Most users only set the suffix.
        format!("{}{}{}{}", post_pre, prefix, mv, post_suf)
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
        }
    }

    /// The VISIBLE line segments of the dimension — extension lines + the
    /// dimension line for linear dims; the leader/dim line for radius &
    /// diameter. Used for click hit-testing so the user can pick the
    /// dimension by clicking ON the line they see (not only its def points).
    /// Arrowheads/text aren't included; the dim-line endpoints cover the
    /// arrow region and grip_points() covers the text anchor.
    pub fn outline_segments(&self) -> Vec<(Vec2, Vec2)> {
        match &self.kind {
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
        };
        Dim { kind: new_kind, style: self.style, text_override: self.text_override.clone() }
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
