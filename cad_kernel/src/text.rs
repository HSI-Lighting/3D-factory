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

/// Single-line text. The string is plain UTF-8 — special CAD escape
/// codes like `%%c` (diameter symbol), `%%d` (degree), `%%p` (plus/
/// minus) pass through literally for v1. Wire those at app-render time
/// when needed (replace before drawing).
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
        }
    }

    /// Conservative bbox in world coords. Width is estimated as
    /// `0.6 * height * char_count` (ISO 3098-ish average) — exact
    /// per-glyph widths land when the LFF parser does. Height span =
    /// height above baseline; descent is omitted for v1 (rare in
    /// dim labels, the primary consumer).
    ///
    /// IGNORES rotation — returns the axis-aligned bbox of the text
    /// AT angle 0 around `position`. The renderer + spatial index
    /// callers either apply the rotation themselves or accept the
    /// loose bbox as a culling key (same approach as `Wall::bbox`).
    pub fn bbox_unrotated(&self) -> (Vec2, Vec2) {
        let w = (self.text.chars().count() as f64) * self.height * 0.6;
        let h = self.height;
        let (left, right) = match self.h_align {
            HAlign::Left   => (0.0, w),
            HAlign::Center => (-w * 0.5, w * 0.5),
            HAlign::Right  => (-w, 0.0),
        };
        let (bottom, top) = match self.v_align {
            VAlign::Baseline => (0.0, h),
            VAlign::Bottom   => (0.0, h),
            VAlign::Middle   => (-h * 0.5, h * 0.5),
            VAlign::Top      => (-h, 0.0),
        };
        (
            Vec2::new(self.position.x + left,   self.position.y + bottom),
            Vec2::new(self.position.x + right,  self.position.y + top),
        )
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
}
