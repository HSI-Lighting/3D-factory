//! Single-line text entity + style table.
//!
//! Modelled on LibreCAD's `RS_Text` / `RS_TextData` but cut to the bone
//! for v1:
//!   * Single-line only (MText / multi-line / inline formatting deferred).
//!   * No special-character codes (%%c, %%d, %%p) — they pass through as
//!     literal text for now.
//!   * One vertical alignment family (Baseline / Bottom / Middle / Top).
//!   * One horizontal alignment family (Left / Center / Right) —
//!     LibreCAD's Aligned / Middle / Fit are deferred (they need a
//!     second point + width factor; not needed for dim labels).
//!
//! Rendering: the canvas paints text via `egui::Painter::text` at the
//! computed position. The kernel stores only the data — the visual
//! representation is the app's concern. A future swap to vector-stroke
//! fonts (LFF / SHX) re-uses every field on `Text`; only the renderer
//! changes.

use crate::math::Vec2;

/// Vertical alignment of the text relative to `position`. The reference
/// line for each option:
///   * `Baseline` — the writing baseline (most CAD text uses this).
///   * `Bottom`   — descender bottom.
///   * `Middle`   — vertical centre of cap-height.
///   * `Top`      — cap-height top.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VAlign {
    #[default]
    Baseline,
    Bottom,
    Middle,
    Top,
}

/// Horizontal alignment of the text relative to `position`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Paragraph list decoration. The marker is applied at RENDER time (it is NOT
/// part of `text`), so numbered lists auto-renumber when lines are edited and an
/// underline can target the letters only rather than the marker.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextListKind {
    #[default]
    None,
    Bulleted,
    Numbered,
}

/// Text — a single string OR a multi-line paragraph (when `text` contains
/// '\n'), rendered as ONE dobject. Plain UTF-8; CAD escape codes (`%%c`, `%%d`,
/// `%%p`) pass through literally for v1. List markers / numbering are a
/// `list_mode` PROPERTY applied at render, never baked into `text`.
#[derive(Clone, Debug)]
pub struct Text {
    /// Anchor point. The text's `h_align`/`v_align` determine which
    /// corner / edge of the rendered glyph box sits AT this point.
    pub position: Vec2,
    /// Cap height in world units. The rendered glyph box height is
    /// approximately this value (font-dependent ascent / descent
    /// extends slightly above / below).
    pub height:   f64,
    /// Rotation in RADIANS measured CCW from the +X axis. Applied
    /// about `position`.
    pub angle:    f64,
    /// The text string. Newlines are NOT honoured in v1 — render them
    /// as a literal control char or drop. Multi-line wants `MText`.
    pub text:     String,
    pub h_align:  HAlign,
    pub v_align:  VAlign,
    /// Index into `Document.text_styles`. `0` = the reserved
    /// `STANDARD` style; never deletable.
    pub style:    u32,
    // ── Per-entity render properties (SNAPSHOT at placement) ──────────────
    // These make every Text INDEPENDENT: editing a style (the "defaults
    // template") never retro-changes already-placed text. The renderer reads
    // THESE, not the style. `font_name == ""` means "inherit the style's font"
    // (back-compat for text created before these fields existed).
    /// Font family name. "" = inherit from the referenced style.
    pub font_name:     String,
    /// Synthetic bold (renderer thickens the edge).
    pub bold:          bool,
    /// Italic shear angle in radians (0 = upright).
    pub oblique:       f64,
    /// Horizontal scale (1.0 = normal).
    pub width_factor:  f64,
    /// Stroke-only (outline) rendering instead of solid fill.
    pub outline_only:  bool,
    /// Pen width (world units) for outline strokes; 0.0 = hairline.
    pub outline_width: f64,
    /// Underline — the renderer draws a line under the glyphs.
    pub underline:     bool,
    /// Paragraph list decoration (bullet / number), applied at render time.
    pub list_mode:     TextListKind,
    /// Line spacing as a multiple of `height` for multi-line paragraphs
    /// (`text` containing '\n'). 1.5 = the default CAD leading.
    pub line_spacing:  f64,
}

impl Text {
    /// Empty Text at origin, height 1, no rotation. Useful starting
    /// point for builders; the empty string still has a position +
    /// bbox so it doesn't break renders.
    pub fn empty() -> Self {
        Self {
            position: Vec2::ZERO,
            height:   1.0,
            angle:    0.0,
            text:     String::new(),
            h_align:  HAlign::Left,
            v_align:  VAlign::Baseline,
            style:    TextStyleTable::STANDARD,
            font_name:     String::new(),
            bold:          false,
            oblique:       0.0,
            width_factor:  1.0,
            outline_only:  false,
            outline_width: 0.0,
            underline:     false,
            list_mode:     TextListKind::None,
            line_spacing:  1.5,
        }
    }

    /// Conservative bbox in world coords. Width is estimated as
    /// `0.6 * height * char_count` (ISO 3098-ish average) over the WIDEST line
    /// (plus a list-marker allowance) — exact per-glyph widths land with the
    /// renderer. A multi-line paragraph (`text` with '\n') grows DOWNWARD from
    /// the first line by `line_spacing × height` per line. Single-line text with
    /// no list is identical to before.
    ///
    /// IGNORES rotation — returns the axis-aligned bbox of the text
    /// AT angle 0 around `position`. The renderer + spatial index
    /// callers either apply the rotation themselves or accept the
    /// loose bbox as a culling key (same approach as `Wall::bbox`).
    pub fn bbox_unrotated(&self) -> (Vec2, Vec2) {
        let lines: Vec<&str> = if self.text.is_empty() {
            vec![""]
        } else {
            self.text.split('\n').collect()
        };
        let n = lines.len().max(1);
        // Allowance for the render-time list marker ("• " / "N. ").
        let marker_chars = match self.list_mode {
            TextListKind::None     => 0,
            TextListKind::Bulleted => 2,
            TextListKind::Numbered => 3,
        };
        let max_chars =
            lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) + marker_chars;
        let w = (max_chars as f64) * self.height * 0.6;
        let h = self.height;
        let line_h = h * if self.line_spacing > 1e-6 { self.line_spacing } else { 1.5 };
        let (left, right) = match self.h_align {
            HAlign::Left   => (0.0, w),
            HAlign::Center => (-w * 0.5, w * 0.5),
            HAlign::Right  => (-w, 0.0),
        };
        let (bottom1, top) = match self.v_align {
            VAlign::Baseline => (0.0, h),
            VAlign::Bottom   => (0.0, h),
            VAlign::Middle   => (-h * 0.5, h * 0.5),
            VAlign::Top      => (-h, 0.0),
        };
        let bottom = bottom1 - (n as f64 - 1.0) * line_h;
        (
            Vec2::new(self.position.x + left,   self.position.y + bottom),
            Vec2::new(self.position.x + right,  self.position.y + top),
        )
    }

    /// Distance from a point to the text's (rotated) bounding box: `0.0` when
    /// the point lies inside the box, otherwise the distance to its nearest
    /// edge. So a click ANYWHERE on the text picks it — not just within a few
    /// pixels of the anchor corner (the old `position.dist(p)` behaviour, which
    /// forced users to window-select text). Honours `angle` by inverse-rotating
    /// the query point into the un-rotated frame about `position`.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let local = if self.angle.abs() < 1e-12 {
            p
        } else {
            // Rotate p by -angle about `position` (text rotated +angle ⇔ point
            // rotated -angle in the text's own frame).
            let (s, c) = (-self.angle).sin_cos();
            let d = p - self.position;
            Vec2::new(
                self.position.x + d.x * c - d.y * s,
                self.position.y + d.x * s + d.y * c,
            )
        };
        let (min, max) = self.bbox_unrotated();
        // Distance from `local` to the axis-aligned box [min,max] (0 if inside).
        let dx = (min.x - local.x).max(0.0).max(local.x - max.x);
        let dy = (min.y - local.y).max(0.0).max(local.y - max.y);
        (dx * dx + dy * dy).sqrt()
    }

    /// Font family this text renders with: its explicit `font_name`, else its
    /// style's font, else `"standard"`. ONE source of truth — the renderer,
    /// TXTEXP, and hatch boundary tracing all go through this so the fallback
    /// chain can never drift apart.
    pub fn resolved_font_name(&self, styles: &TextStyleTable) -> String {
        if !self.font_name.is_empty() {
            self.font_name.clone()
        } else {
            styles
                .get(self.style)
                .map(|s| s.font_name.clone())
                .unwrap_or_else(|| "standard".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Leader — an MLEADER-style callout: a leader line chain with an
// arrowhead at the FIRST vertex and an optional Text label anchored at
// the last vertex (AutoCAD MLEADER with single-line annotation).
//
// Stored data: the chain `pts` (world coords, first = arrow tip) plus a
// full `Text` label (position/height/angle/string/style/render specs),
// so bbox / distance / grips / transforms all reuse the Text machinery.
// Rendering (polyline + arrowhead + text) is the app's concern.
// ---------------------------------------------------------------------------

/// Multi-leader callout (MLEADER). `pts[0]` is the arrowhead tip;
/// intermediate vertices are bends; the label sits at the final vertex.
#[derive(Clone, Debug)]
pub struct Leader {
    /// Leader chain vertices; the FIRST is the arrowhead tip, the last
    /// is where the text label anchors (AutoCAD's landing point).
    pub pts:   Vec<Vec2>,
    /// The annotation. `label.position` is the landing anchor; the
    /// renderer offsets the text next to the landing point.
    pub label: Text,
    /// Arrowhead on/off (AutoCAD's "arrow first" flag). Default true.
    pub arrow: bool,
}

impl Leader {
    /// A straight two-point leader with an empty label at `b`.
    pub fn new(a: Vec2, b: Vec2) -> Self {
        Self {
            pts:   vec![a, b],
            label: Text { position: b, ..Text::empty() },
            arrow: true,
        }
    }

    /// Bbox over the leader chain + the label's unrotated bbox.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        let (mut mn, mut mx) = self.label.bbox_unrotated();
        for p in &self.pts {
            mn.x = mn.x.min(p.x); mn.y = mn.y.min(p.y);
            mx.x = mx.x.max(p.x); mx.y = mx.y.max(p.y);
        }
        (mn, mx)
    }

    /// Distance to the leader chain (the visible line) OR the label box —
    /// whichever is nearer, so clicking the arrow, the line, or the text
    /// all pick the leader.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let mut best = self.label.distance_to_point(p);
        for w in self.pts.windows(2) {
            let d = crate::geom::Line { a: w[0], b: w[1] }.distance_to_point(p);
            if d < best { best = d; }
        }
        best
    }
}

// ---------------------------------------------------------------------------
// TextStyle — analog of LayerTable / LinetypeTable / DimStyleTable.
// One entry per named style; dobjects reference styles by index.
// ---------------------------------------------------------------------------

/// Named text style. References a font (by name — the font registry
/// lives outside the kernel for v1 since rendering uses egui's bundled
/// font; the field is preserved so DXF round-trip later round-trips the
/// name even if we render with a different font).
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub name:           String,
    /// Font reference. For v1 just a name string ("standard"); the
    /// renderer ignores it and uses egui's bundled font. When LFF
    /// parsing lands the renderer looks the name up in a font cache.
    pub font_name:      String,
    /// Width multiplier (1.0 = normal). DXF group 41.
    pub width_factor:   f64,
    /// Oblique angle in radians (italic shear). 0.0 = upright.
    pub oblique:        f64,
    /// Default text height; 0.0 = use the entity's own `Text.height`.
    /// Non-zero forces every Text on this style to render at this height.
    pub default_height: f64,
    /// Synthetic bold — the TTF renderer thickens the glyph edge (no separate
    /// bold font file is loaded). `false` = regular weight.
    pub bold:           bool,
    /// Outline (stroke-only) rendering instead of solid fill.
    pub outline_only:   bool,
    /// Pen width (world units) for `outline_only` strokes. 0.0 = a hairline
    /// (one screen pixel).
    pub outline_width:  f64,
    /// Underline — renderer draws a line under the glyphs.
    pub underline:      bool,
}

impl TextStyle {
    /// The mandatory built-in STANDARD style — always present at id 0
    /// (mirrors LayerTable's LAYER_BASE convention). DXF interop expects
    /// a style called "STANDARD" to exist; do not rename id 0.
    pub fn standard() -> Self {
        Self {
            name:           "STANDARD".into(),
            font_name:      "standard".into(),
            width_factor:   1.0,
            oblique:        0.0,
            default_height: 0.0,
            bold:           false,
            outline_only:   false,
            outline_width:  0.0,
            underline:      false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextStyleTable {
    pub styles: Vec<TextStyle>,
}

impl TextStyleTable {
    /// Reserved id of the STANDARD style — always present, can't be
    /// deleted. DXF interop assumes id 0 = STANDARD.
    pub const STANDARD: u32 = 0;

    /// Constructed with `STANDARD` only.
    pub fn with_defaults() -> Self {
        Self { styles: vec![TextStyle::standard()] }
    }

    pub fn get(&self, id: u32) -> Option<&TextStyle> {
        self.styles.get(id as usize)
    }

    pub fn add(&mut self, s: TextStyle) -> u32 {
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

impl Default for TextStyleTable {
    fn default() -> Self { Self::with_defaults() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_present_at_id_zero() {
        let t = TextStyleTable::with_defaults();
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(0).unwrap().name, "STANDARD");
    }

    #[test]
    fn find_is_case_insensitive() {
        let t = TextStyleTable::with_defaults();
        assert_eq!(t.find("standard"), Some(0));
        assert_eq!(t.find("STANDARD"), Some(0));
        assert_eq!(t.find("nope"), None);
    }

    #[test]
    fn empty_text_has_valid_bbox() {
        let t = Text::empty();
        let (min, max) = t.bbox_unrotated();
        // Empty string has zero width — bbox collapses horizontally.
        assert!((max.x - min.x).abs() < 1e-9);
        assert!((max.y - min.y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn multiline_paragraph_bbox_grows_downward() {
        // Two lines → the box extends below the anchor by one line's spacing,
        // and its width tracks the WIDER line. One dobject, one bbox.
        let mut t = Text::empty();
        t.height = 10.0;
        t.line_spacing = 1.5;
        t.text = "short\nmuch longer line".into();
        let (min, max) = t.bbox_unrotated();
        // Top = +height (first line), bottom = -(n-1)*1.5*height = -15.
        assert!((max.y - 10.0).abs() < 1e-9);
        assert!((min.y + 15.0).abs() < 1e-9, "second line drops the box by 1.5×h");
        // Width follows the longer line (16 chars × 10 × 0.6 = 96).
        assert!((max.x - min.x - 96.0).abs() < 1e-6);
        // A single-line Text is unchanged (baseline..height, width = chars×0.6×h).
        let mut s = Text::empty();
        s.height = 10.0;
        s.text = "short".into();
        let (smin, smax) = s.bbox_unrotated();
        assert!((smax.y - smin.y - 10.0).abs() < 1e-9);
        assert!((smax.x - smin.x - 30.0).abs() < 1e-6);
    }

    #[test]
    fn click_inside_text_body_picks_it() {
        // Reproduces the reported bug: a click on the text body (far from the
        // anchor corner) must register a hit. "asd" h=20 at (-13.037,20.853),
        // click at (-7.1, 26.573) — inside the box, ~8 units from the anchor.
        let mut t = Text::empty();
        t.text = "asd".into();
        t.height = 20.0;
        t.position = Vec2::new(-13.037, 20.853);
        // Inside the box → distance 0 (old anchor-distance was ~8.3).
        assert_eq!(t.distance_to_point(Vec2::new(-7.1, 26.573)), 0.0);
        // A point well outside is still far.
        assert!(t.distance_to_point(Vec2::new(100.0, 100.0)) > 50.0);
        // Rotating 90° about the anchor moves the box; a point rotated the same
        // way about the anchor still lands inside. Local body offset (5.72,6.0)
        // maps to world offset (-6.0,5.72) under a +90° box rotation.
        t.angle = std::f64::consts::FRAC_PI_2;
        assert_eq!(
            t.distance_to_point(Vec2::new(-13.037 - 6.0, 20.853 + 5.72)),
            0.0
        );
    }

    #[test]
    fn bbox_respects_horizontal_alignment() {
        let mut t = Text::empty();
        t.text = "hi".into();
        t.height = 2.0;
        t.h_align = HAlign::Center;
        let (min, max) = t.bbox_unrotated();
        // Width = 2 chars * 2.0 * 0.6 = 2.4; centred → -1.2 .. +1.2
        assert!((min.x + 1.2).abs() < 1e-9);
        assert!((max.x - 1.2).abs() < 1e-9);
    }

    #[test]
    fn resolved_font_name_chain() {
        // Explicit entity font wins; else style font; else "standard".
        let mut styles = TextStyleTable::with_defaults();
        styles.styles[0].font_name = "style font".into();
        styles.add(TextStyle {
            name: "S2".into(),
            font_name: "another".into(),
            ..TextStyle::standard()
        });

        let mut t = Text::empty();
        assert_eq!(t.resolved_font_name(&styles), "style font");

        t.font_name = "explicit".into();
        assert_eq!(t.resolved_font_name(&styles), "explicit");

        t.font_name = String::new();
        t.style = 1;
        assert_eq!(t.resolved_font_name(&styles), "another");

        t.style = 99; // dangling style → "standard"
        assert_eq!(t.resolved_font_name(&styles), "standard");
    }
}
