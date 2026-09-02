//! cad_text — TTF text rendering engine for RUST-AutoRASM.
//!
//! Produces **world-space** glyph geometry (fill triangles + outline segments)
//! for a text string. The app packs these into the existing GPU fill
//! (`FillVertex`) and line (`LineInstance`) pipelines, so text rides the same
//! B25 render cache as every other primitive — no bespoke text pipeline.
//!
//! Design + rationale: `kernel mentor MD/TEXT_GPU_TTF_PLAN.md`.
//!
//! Kernel independence: nothing here is stored on `cad_kernel::Text`. The glyph
//! cache lives in the app layer (keyed by handle), so the kernel entity stays
//! pure `Send + Sync` data and there is no `cad_kernel ⇄ cad_text` cycle.
//!
//! **Phase 0 (this file): public API as compiling stubs.** The real engine
//! (font enumeration + shaping + tessellation) arrives in Phase 1.

use std::collections::HashMap;

use cad_kernel::Vec2;
// Reuse the kernel's alignment enums — one source of truth, no conversion.
pub use cad_kernel::text::{HAlign, VAlign};

mod font;
mod render;

use crate::font::{FontIndex, FontSource};
use crate::render::TextRenderer;

/// How a glyph is drawn: solid triangle fill, or stroked outline only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMode {
    Fill,
    Outline,
}

impl Default for FillMode {
    fn default() -> Self {
        FillMode::Fill
    }
}

/// World-space glyph geometry for one rendered string — final coordinates with
/// position, height scale, rotation, and alignment already baked in. The app
/// only has to add `world_offset` and a packed color when emitting instances.
#[derive(Clone, Debug)]
pub struct RenderedGlyphs {
    /// Triangle soup: each entry is one filled triangle (3 world points).
    pub fills: Vec<[Vec2; 3]>,
    /// Glyph perimeter as CLOSED contours (each a world-space polyline; the
    /// caller closes it). Used to anti-alias the fill edge and to draw
    /// outline-mode / bold strokes with proper joins.
    pub outlines: Vec<Vec<Vec2>>,
    /// Per-glyph closed polygon loops in WORLD space: `(outer, holes)` — one
    /// entry per glyph. The outer contour plus its (possibly empty) hole
    /// contours, classified by containment. This is the geometry TXTEXP turns
    /// into closed Polyline dobjects; hatch island resolution works on the
    /// loops unchanged.
    pub glyph_polygons: Vec<(Vec<Vec2>, Vec<Vec<Vec2>>)>,
    /// World-space bounding box `(min, max)` of the rendered text
    /// (`(ZERO, ZERO)` for empty input).
    pub bbox: (Vec2, Vec2),
    /// Widest line's shaped pen ADVANCE in world units (includes trailing-space
    /// advance, unlike `bbox` which is ink-only). Lets callers position text
    /// that follows — e.g. to trim an underline past a list marker.
    pub advance: f64,
}

impl Default for RenderedGlyphs {
    // `Vec2` has no `Default` derive, so the bbox tuple can't either — spell it.
    fn default() -> Self {
        RenderedGlyphs {
            fills: Vec::new(),
            outlines: Vec::new(),
            glyph_polygons: Vec::new(),
            bbox: (Vec2::ZERO, Vec2::ZERO),
            advance: 0.0,
        }
    }
}

impl RenderedGlyphs {
    /// True when nothing was produced (empty string, missing font, or a
    /// height that rounds below the render threshold).
    pub fn is_empty(&self) -> bool {
        self.fills.is_empty() && self.outlines.is_empty()
    }
}

/// Everything needed to render one string. The app's per-handle cache key
/// mirrors these fields, so identical requests reuse cached geometry.
#[derive(Clone, Copy, Debug)]
pub struct TextRequest<'a> {
    pub text: &'a str,
    /// Font family name (as chosen in the text style / dialog). An unknown name
    /// falls back to the embedded font.
    pub font_name: &'a str,
    /// Anchor point in world coordinates.
    pub position: Vec2,
    /// Cap height in world units.
    pub height: f64,
    /// Rotation in radians, CCW about `position`.
    pub angle: f64,
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub fill_mode: FillMode,
    /// Italic shear angle in radians (from the text style's `oblique`). 0 =
    /// upright. Applied as an x-shear proportional to height.
    pub slant: f64,
    /// Horizontal scale (the style's `width_factor`). 1.0 = normal.
    pub x_scale: f64,
}

/// Global font registry + per-font renderers.
///
/// Scans the system font directories once, then lazily loads + caches a
/// `TextRenderer` per resolved font source. A requested family that isn't
/// found falls back to the system default, and the EMBEDDED fonts guarantee a
/// usable font even on a system with no installed fonts — so `is_ready()` is
/// always true and RTL (Arabic/Hebrew) text always renders.
pub struct FontManager {
    index: FontIndex,
    /// Loaded renderers keyed by resolved font source (so two family names that
    /// resolve to the same file share one renderer + glyph cache).
    loaded: HashMap<FontSource, Option<TextRenderer>>,
}

impl FontManager {
    /// Build the manager and enumerate available fonts (one-time scan).
    pub fn new() -> Self {
        FontManager {
            index: FontIndex::scan(),
            loaded: HashMap::new(),
        }
    }

    /// Family names available to the picker (sorted, original case). Includes
    /// the embedded fonts, so an RTL-capable font is always offered. The list
    /// is cached (no per-call sort/alloc) and the first call also scans any
    /// `.ttc`/`.otc` collections deferred from the startup scan.
    pub fn names(&mut self) -> &[String] {
        self.index.names()
    }

    /// True when a usable font exists. ALWAYS true — the embedded fonts
    /// (Liberation Sans default + DejaVu Sans RTL fallback) ship with the
    /// crate, so callers never need the egui `painter.text` fallback.
    pub fn is_ready(&self) -> bool {
        true
    }

    /// Render `req` to world-space glyph geometry. Returns an empty result
    /// only when the string is empty or a font fails to parse (never because
    /// of a missing font).
    pub fn render(&mut self, req: &TextRequest<'_>) -> RenderedGlyphs {
        self.render_with(req, false)
    }

    /// Render for TXTEXP — additionally fills `RenderedGlyphs.glyph_polygons`
    /// (world-space outer + hole loops per glyph). The per-frame text render
    /// path skips that work; only the one-shot explode command needs it.
    pub fn render_explode(&mut self, req: &TextRequest<'_>) -> RenderedGlyphs {
        self.render_with(req, true)
    }

    fn render_with(&mut self, req: &TextRequest<'_>, want_polygons: bool) -> RenderedGlyphs {
        let source = self.index.resolve(req.font_name);
        // Lazily load + parse the font on first use; cache success OR failure so
        // we don't re-read a bad file every frame.
        if !self.loaded.contains_key(&source) {
            let renderer = match &source {
                FontSource::Path(p, idx) => std::fs::read(p)
                    .ok()
                    .and_then(|b| TextRenderer::from_bytes_with_index(b, *idx)),
                FontSource::Embedded(bytes) => TextRenderer::from_bytes(bytes.to_vec()),
            };
            self.loaded.insert(source.clone(), renderer);
        }
        match self.loaded.get_mut(&source) {
            Some(Some(r)) => r.render_with_polygons(req, want_polygons),
            _ => RenderedGlyphs::default(),
        }
    }
}

impl Default for FontManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Bytes of the embedded RTL-capable font (DejaVu Sans — Arabic + Hebrew +
/// Latin). The app registers this in egui's font fallback chain so dialog text
/// boxes / menus show real Arabic + Hebrew glyphs instead of `.notdef` boxes.
/// (egui does no bidi shaping, but the glyph shapes are correct.)
pub fn rtl_fallback_font_bytes() -> &'static [u8] {
    crate::font::rtl_fallback_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(text: &'a str, fill: FillMode) -> TextRequest<'a> {
        TextRequest {
            text,
            font_name: "standard", // unknown → default font
            position: Vec2::new(100.0, 50.0),
            height: 10.0,
            angle: 0.0,
            h_align: HAlign::Left,
            v_align: VAlign::Baseline,
            fill_mode: fill,
            slant: 0.0,
            x_scale: 1.0,
        }
    }

    #[test]
    fn empty_string_renders_nothing() {
        let mut fm = FontManager::new();
        assert!(fm.render(&req("", FillMode::Fill)).is_empty());
    }

    #[test]
    fn hello_produces_geometry_when_a_font_exists() {
        let mut fm = FontManager::new();
        let filled = fm.render(&req("Hello", FillMode::Fill));
        assert!(!filled.fills.is_empty(), "fill mode should emit triangles");
        // bbox should be near the anchor and non-degenerate.
        let (min, max) = filled.bbox;
        assert!(max.x > min.x && max.y > min.y, "bbox must be non-empty");

        let stroked = fm.render(&req("Hello", FillMode::Outline));
        assert!(
            !stroked.outlines.is_empty(),
            "outline mode should emit segments"
        );
    }

    #[test]
    fn curvy_glyph_is_smooth_not_faceted() {
        // A curvy glyph ('O') must flatten to MANY points — if curves were being
        // dropped (the faceting bug), a contour would have only a handful.
        let mut fm = FontManager::new();
        let g = fm.render(&req("O", FillMode::Outline));
        let max_pts = g.outlines.iter().map(|c| c.len()).max().unwrap_or(0);
        assert!(
            max_pts >= 32,
            "curve should flatten smoothly, got only {max_pts} points"
        );
    }

    #[test]
    fn embedded_fonts_make_engine_always_ready() {
        // The embedded Liberation Sans + DejaVu Sans ship with the crate, so
        // the engine is ready even on a machine with ZERO installed fonts.
        let mut fm = FontManager::new();
        assert!(fm.is_ready());
        let names = fm.names();
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case("DejaVu Sans")),
            "RTL-capable font must be offered in the picker: {names:?}"
        );
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case("Liberation Sans")),
            "embedded default must be offered in the picker: {names:?}"
        );
    }

    #[test]
    fn unknown_font_name_still_renders() {
        // Resolution falls back system → embedded default; never empty.
        let mut fm = FontManager::new();
        let g = fm.render(&req("NoSuchFontFamily", FillMode::Fill));
        assert!(!g.fills.is_empty(), "unknown font must fall back and render");
    }

    #[test]
    fn arabic_text_renders_with_default_style() {
        let mut fm = FontManager::new();
        let g = fm.render(&req("\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}", FillMode::Fill));
        assert!(
            !g.fills.is_empty(),
            "Arabic must render even when the default font lacks Arabic glyphs"
        );
        assert!(g.advance > 0.0);
    }

    #[test]
    fn hebrew_text_renders_with_default_style() {
        let mut fm = FontManager::new();
        let g = fm.render(&req("\u{05e9}\u{05dc}\u{05d5}\u{05dd}", FillMode::Fill));
        assert!(!g.fills.is_empty());
        assert!(g.advance > 0.0);
    }

    #[test]
    fn mixed_rtl_ltr_text_renders() {
        // Arabic embedded in a Latin line — bidi run reordering + the RTL
        // fallback face must both work without panicking.
        let mut fm = FontManager::new();
        let g = fm.render(&req("abc \u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629} 123", FillMode::Fill));
        assert!(!g.fills.is_empty());
        assert!(g.advance > 0.0);
        let (min, max) = g.bbox;
        assert!(max.x > min.x && max.y > min.y);
    }

    #[test]
    fn rtl_text_renders_in_explicit_rtl_font() {
        // Requesting the RTL-capable font by name renders Arabic directly.
        let mut fm = FontManager::new();
        let mut r = req("\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}", FillMode::Fill);
        r.font_name = "DejaVu Sans";
        let g = fm.render(&r);
        assert!(!g.fills.is_empty());
    }
}
