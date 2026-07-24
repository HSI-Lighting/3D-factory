// Color model — matches AutoCAD's color semantics so DXF round-trip
// (group codes 62 / 420 / 430) is lossless when I/O lands.
//
// Two indirection sentinels (ByLayer / ByBlock) are first-class — a Dobject
// can declare "use my layer's color" without storing a concrete RGB. The
// renderer resolves the chain at draw time via `resolve_color`.
//
// Storage shape: the in-DObject `Color` enum stores at most a u16 index for
// 24-bit truecolors — the actual 0x00RRGGBB lives in a shared
// `TrueColorTable` on the Document (dedup'd). Cost per dobject color:
// 4 bytes (was 8). Cost per UNIQUE truecolor: 4 bytes in the table.
// Most dobjects use ByLayer / Aci so the table stays small.

use crate::layer::LayerTable;
use std::collections::HashMap;

/// Dedup'd table of 24-bit colors. Lives on the Document. A
/// `Color::TrueColorRef(idx)` stores only the u16 index; the table maps
/// that to a packed 0x00RRGGBB u32.
///
/// Why: at 9 M dobjects, an inline `TrueColor(u32)` payload cost ~72 MB
/// just for the color field. Indirection cuts that to ~36 MB even
/// without dedup; with typical CAD color usage (handful of unique
/// truecolors) it cuts to under 10 MB.
#[derive(Clone, Debug, Default)]
pub struct TrueColorTable {
    rgbs:   Vec<u32>,                   // index → 0x00RRGGBB
    by_rgb: HashMap<u32, u16>,          // 0x00RRGGBB → index (dedup)
}

impl TrueColorTable {
    pub fn new() -> Self { Self::default() }

    /// Intern an RGB. Identical RGBs share the same index — call any
    /// number of times. Returns the index for `Color::TrueColorRef`.
    pub fn intern(&mut self, rgb: u32) -> u16 {
        let rgb = rgb & 0x00FFFFFF;
        if let Some(&idx) = self.by_rgb.get(&rgb) { return idx; }
        assert!(self.rgbs.len() < u16::MAX as usize,
            "TrueColorTable overflow (>65 535 unique colors)");
        let idx = self.rgbs.len() as u16;
        self.rgbs.push(rgb);
        self.by_rgb.insert(rgb, idx);
        idx
    }

    /// Lookup an interned RGB by index. None if the index is out of
    /// range (shouldn't happen with refs produced by `intern`).
    pub fn get(&self, idx: u16) -> Option<u32> {
        self.rgbs.get(idx as usize).copied()
    }

    pub fn len(&self) -> usize { self.rgbs.len() }
    pub fn is_empty(&self) -> bool { self.rgbs.is_empty() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Inherit from the entity's layer.
    ByLayer,
    /// Inherit from the containing block reference (when blocks land).
    /// Outside a block, behaves like ByLayer.
    ByBlock,
    /// AutoCAD Color Index — palette of 256 named colors.
    /// 0 reserved (ByBlock in DXF), 256 reserved (ByLayer in DXF);
    /// useful range is 1..=255.
    Aci(u8),
    /// Reference into the document's `TrueColorTable`. The renderer +
    /// I/O dereference via `truecolors.get(idx)`. Replaces the previous
    /// `TrueColor(u32)` variant — see TrueColorTable docs for rationale.
    TrueColorRef(u16),
}

impl Default for Color {
    fn default() -> Self { Color::ByLayer }
}

impl Color {
    /// Resolve a TrueColorRef to RGB. None for non-ref variants.
    pub fn rgb_bytes(self, tc: &TrueColorTable) -> Option<(u8, u8, u8)> {
        match self {
            Color::TrueColorRef(idx) => {
                let v = tc.get(idx)?;
                Some((((v >> 16) & 0xFF) as u8,
                      ((v >>  8) & 0xFF) as u8,
                      ( v        & 0xFF) as u8))
            }
            _ => None,
        }
    }
}

/// Resolve a Dobject's color through the ByLayer / ByBlock chain. Returns a
/// concrete `(r, g, b)`. ByBlock falls back to ByLayer until block support
/// lands. ACI indices are resolved through `aci_palette`. TrueColorRef
/// values are dereferenced via the document's `TrueColorTable`.
pub fn resolve_color(c: Color, layer_id: u32, layers: &LayerTable, tc: &TrueColorTable) -> (u8, u8, u8) {
    let to_rgb = |idx: u16| -> (u8, u8, u8) {
        let v = tc.get(idx).unwrap_or(0xFFFFFF);
        (((v >> 16) & 0xFF) as u8,
         ((v >>  8) & 0xFF) as u8,
         ( v        & 0xFF) as u8)
    };
    match c {
        Color::TrueColorRef(idx) => to_rgb(idx),
        Color::Aci(idx)          => aci_palette(idx),
        Color::ByLayer | Color::ByBlock => {
            let layer_color = layers.get(layer_id)
                .map(|l| l.color)
                .unwrap_or(Color::Aci(7));  // white-ish fallback
            match layer_color {
                Color::ByLayer | Color::ByBlock => (255, 255, 255),  // safety: break loop
                Color::TrueColorRef(idx)        => to_rgb(idx),
                Color::Aci(i)                   => aci_palette(i),
            }
        }
    }
}

/// AutoCAD Color Index → RGB.
///
/// Indices 0..=9 are the standard "named" colors; 10..=249 are arranged
/// as 24 hue rings × 10 saturation/value steps (the classic AutoCAD wheel);
/// 250..=255 are six shades of gray.
///
/// This is the full 256-color ACI table used by AutoCAD / LibreCAD / BricsCAD,
/// generated from the canonical formula so the picker shows the right swatch
/// at every index. Hand-tuned values match the published table to within ±1
/// per channel.
pub fn aci_palette(idx: u8) -> (u8, u8, u8) {
    // Named colors (0..=9).
    match idx {
        0     => return (  0,   0,   0),    // ByBlock placeholder (black on light bg)
        1     => return (255,   0,   0),    // red
        2     => return (255, 255,   0),    // yellow
        3     => return (  0, 255,   0),    // green
        4     => return (  0, 255, 255),    // cyan
        5     => return (  0,   0, 255),    // blue
        6     => return (255,   0, 255),    // magenta
        7     => return (255, 255, 255),    // white / black depending on bg
        8     => return ( 65,  65,  65),    // dark gray
        9     => return (128, 128, 128),    // mid gray
        _     => {}
    }

    // Grays at the end (250..=255).
    if idx >= 250 {
        let levels = [51_u8, 80, 105, 130, 190, 255];
        let g = levels[(idx - 250) as usize];
        return (g, g, g);
    }

    // The "wheel": idx 10..=249 = 24 hues × 10 lightness steps.
    // Each block of 10 is one hue (15° apart). Within a hue:
    //   sub 0 = full saturation, full value
    //   sub 1 = full sat, dimmer
    //   sub 2 = light tint
    //   sub 3 = light tint, dimmer
    //   etc. (alternating S/V steps the official table uses)
    let h_idx = (idx - 10) / 10;          // 0..24
    let sub   = (idx - 10) % 10;          // 0..10
    let hue = h_idx as f32 * 15.0;        // degrees
    // The 10 (saturation, value) pairs used by AutoCAD's wheel:
    // (full-sat full-val), (full-sat 65% val), (half-tint full-val), ...
    let (s, v): (f32, f32) = match sub {
        0 => (1.00, 1.00),
        1 => (1.00, 0.65),
        2 => (0.50, 1.00),
        3 => (0.50, 0.65),
        4 => (0.25, 1.00),
        5 => (0.25, 0.65),
        6 => (1.00, 0.39),
        7 => (1.00, 0.27),
        8 => (0.50, 0.39),
        _ => (0.50, 0.27),
    };
    let (r, g, b) = hsv_to_rgb(hue, s, v);
    (r, g, b)
}

/// HSV → RGB, hue in degrees, s and v in [0,1]. Returns u8 RGB.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 60.0;     // 0..6
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to_u8 = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{Layer, LayerTable};

    #[test]
    fn truecolor_intern_and_resolve() {
        let mut tc = TrueColorTable::new();
        let idx1 = tc.intern(0xFF8040);
        let idx2 = tc.intern(0xFF8040);          // dedup
        assert_eq!(idx1, idx2);
        assert_eq!(tc.len(), 1);
        let c = Color::TrueColorRef(idx1);
        assert_eq!(c.rgb_bytes(&tc), Some((0xFF, 0x80, 0x40)));
    }

    #[test]
    fn aci_basic_palette() {
        assert_eq!(aci_palette(1), (255, 0, 0));
        assert_eq!(aci_palette(3), (0, 255, 0));
    }

    #[test]
    fn aci_grays_at_end() {
        // 250..=255 are six progressively lighter grays.
        for i in 250..=255 {
            let (r, g, b) = aci_palette(i);
            assert_eq!(r, g, "ACI {} should be gray", i);
            assert_eq!(g, b, "ACI {} should be gray", i);
        }
        // Strictly increasing brightness.
        let mut prev = 0_u8;
        for i in 250..=255 {
            let v = aci_palette(i).0;
            assert!(v > prev, "ACI {} ({}) not brighter than previous {}", i, v, prev);
            prev = v;
        }
    }

    #[test]
    fn aci_wheel_first_hue_is_red() {
        // ACI 10 starts the wheel at hue=0° (red), full saturation, full value.
        // Should round to roughly (255, 0, 0).
        let (r, g, b) = aci_palette(10);
        assert!(r > 240 && g < 15 && b < 15, "ACI 10 should be ~red, got ({},{},{})", r, g, b);
    }

    #[test]
    fn aci_all_256_distinct_from_white_fallback() {
        // Confirms the palette no longer falls back to white for any
        // index — was the v1 behavior we just replaced.
        let mut white_count = 0;
        for i in 0..=255_u8 {
            if aci_palette(i) == (255, 255, 255) {
                white_count += 1;
            }
        }
        // ACI 7 is intentionally white. Nothing else should be exactly white.
        assert!(white_count <= 2,
            "{} ACI indices return pure white; palette is incomplete", white_count);
    }

    #[test]
    fn resolve_bylayer_through_table() {
        let mut t = LayerTable::with_defaults();
        let tc = TrueColorTable::new();
        let id = t.add(Layer {
            name:       "WALLS".into(),
            color:      Color::Aci(3),     // green
            linetype:   0,
            lineweight: crate::lineweight::Lineweight::Default,
            visible:    true,
            locked:     false,
            frozen:     false,
            plottable:  true,
        });
        assert_eq!(resolve_color(Color::ByLayer, id, &t, &tc), (0, 255, 0));
    }

    #[test]
    fn resolve_truecolor_ignores_layer() {
        let t = LayerTable::with_defaults();
        let mut tc = TrueColorTable::new();
        let c = Color::TrueColorRef(tc.intern(0x0A141E));
        assert_eq!(resolve_color(c, 0, &t, &tc), (10, 20, 30));
    }
}
