// Polar ACI color picker — the AutoRasm-style concentric wheel + two
// "excluded" rows for the named (1..=9) and grayscale (250..=255) bands.
//
// 241 ACI colors fill the main wheel (ACI 0 + ACI 10..=249):
//   * center circle = brightest among them (luminance pick)
//   * concentric rings outward, packed with fixed-radius circles
//   * per-ring colors angularly sorted by HSL hue so red sits near 0°,
//     green near 210°, blue near 120°, etc.
//
// 9 named colors (ACI 1..=9) and 6 grays (ACI 250..=255) live as
// permanent excluded swatches BELOW the wheel — they read poorly in
// the polar layout because they cluster at the extremes of saturation
// + luminance.
//
// The wheel's slot → ACI mapping is user-permutable via Swap mode and
// can be persisted to JSON. Layout positions never change; only the
// ACI assignments at each slot do.
//
// Spec reference: ~/workspace/RUST_CAD/ACI_Picker_UI.html (HTML mockup).

use cad_kernel::color::aci_palette;
use eframe::egui;

// ---- layout constants — keep in lock-step with the HTML reference -------
const CIRCLE_RADIUS:   f32 = 8.0;
const RADIAL_GAP:      f32 = 3.0;
const TANGENTIAL_GAP:  f32 = 3.0;

/// ACI indices kept OUT of the polar wheel — they're surfaced as
/// excluded-row swatches instead.
pub const EXCLUDED_NAMED:     std::ops::RangeInclusive<u8> = 1..=9;
pub const EXCLUDED_GRAY:      std::ops::RangeInclusive<u8> = 250..=255;
/// Number of ACI bytes that live on the main wheel after exclusions.
/// (256 − 9 − 6 = 241.)
pub const WHEEL_SLOT_COUNT:   usize = 256 - 9 - 6;

#[derive(Clone, Copy)]
struct Slot {
    /// Position in widget-local coords (relative to the wheel center).
    dx: f32,
    dy: f32,
}

/// Shared state for the picker: precomputed slot positions + the live
/// permutation from slot index → ACI byte. Owned by `App`.
pub struct AciPickerState {
    /// Slot positions in widget-local coords. Length is exactly
    /// `WHEEL_SLOT_COUNT`. slot 0 = center.
    slots:                 Vec<Slot>,
    /// position → ACI byte. `mapping[slot_idx]` is the ACI shown at that
    /// wheel slot. The factory default is computed once and re-used by
    /// Reset. Length is exactly `WHEEL_SLOT_COUNT`.
    pub mapping:           Vec<u8>,
    default_mapping:       Vec<u8>,
    /// Swap-mode lets the user click two slots to swap the ACI bytes at
    /// those positions (used to tune the wheel to taste; persisted via
    /// `save_mapping`). Swap mode applies only to the wheel — excluded
    /// rows are fixed.
    pub swap_mode:         bool,
    /// First slot picked in swap mode; None means awaiting the first.
    selected_for_swap:     Option<usize>,
    /// Hovered ACI (whether from wheel or excluded rows), for the
    /// readout line. Recomputed each frame.
    pub hovered_aci:       Option<u8>,
    /// Manual-entry buffer for the "ACI #" text box.
    pub manual_entry:      String,
}

impl Default for AciPickerState {
    fn default() -> Self {
        let (slots, default_mapping) = build_layout_and_default_mapping();
        Self {
            mapping: default_mapping.clone(),
            slots,
            default_mapping,
            swap_mode: false,
            selected_for_swap: None,
            hovered_aci: None,
            manual_entry: String::new(),
        }
    }
}

impl AciPickerState {
    /// Try to replace the current mapping with one persisted to disk.
    /// Silent no-op if the file is missing, unparseable, the wrong
    /// length, or contains an excluded ACI — the picker falls back to
    /// the deterministic default layout.
    pub fn try_load_mapping(&mut self, path: &std::path::Path) {
        let Ok(bytes) = std::fs::read(path) else { return };
        let Ok(text) = std::str::from_utf8(&bytes) else { return };
        let trimmed = text.trim().trim_start_matches('[').trim_end_matches(']');
        let mut out: Vec<u8> = Vec::with_capacity(WHEEL_SLOT_COUNT);
        for tok in trimmed.split(',') {
            let tok = tok.trim();
            if tok.is_empty() { continue; }
            let Ok(n) = tok.parse::<u16>() else { return };
            if n > 255 { return; }
            let n = n as u8;
            if EXCLUDED_NAMED.contains(&n) || EXCLUDED_GRAY.contains(&n) {
                // Saved file is from before the exclusion refactor.
                return;
            }
            out.push(n);
            if out.len() > WHEEL_SLOT_COUNT { return; }
        }
        if out.len() == WHEEL_SLOT_COUNT { self.mapping = out; }
    }

    /// Persist current mapping as a JSON array `[n, n, ...]`.
    pub fn save_mapping(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut s = String::with_capacity(WHEEL_SLOT_COUNT * 4 + 4);
        s.push('[');
        for (i, v) in self.mapping.iter().enumerate() {
            if i > 0 { s.push(','); }
            s.push_str(&v.to_string());
        }
        s.push(']');
        std::fs::write(path, s)
    }

    pub fn reset_to_default(&mut self) {
        self.mapping = self.default_mapping.clone();
        self.selected_for_swap = None;
    }

    /// Render the polar wheel inside the given UI. Returns `Some(aci)`
    /// only on the frame the user *commits* a pick (click in pick mode);
    /// returns `None` while swapping, hovering, or idle.
    pub fn wheel_ui(&mut self, ui: &mut egui::Ui) -> Option<u8> {
        // Allocate a square area large enough for all rings + padding.
        let size = self.required_size();
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(size, size), egui::Sense::click()
        );
        let painter = ui.painter_at(rect);
        let center = rect.center();

        painter.rect_filled(rect, rect.width() * 0.5,
            egui::Color32::from_rgb(40, 44, 52));

        // Hover detection.
        let mut hovered_slot: Option<usize> = None;
        if let Some(p) = resp.hover_pos() {
            let r2 = CIRCLE_RADIUS * CIRCLE_RADIUS;
            for (i, slot) in self.slots.iter().enumerate() {
                let cx = center.x + slot.dx;
                let cy = center.y + slot.dy;
                let dx = p.x - cx; let dy = p.y - cy;
                if dx * dx + dy * dy <= r2 {
                    hovered_slot = Some(i);
                    break;
                }
            }
        }
        if let Some(i) = hovered_slot {
            self.hovered_aci = Some(self.mapping[i]);
        }

        // Paint each slot.
        for (i, slot) in self.slots.iter().enumerate() {
            let (r, g, b) = aci_palette(self.mapping[i]);
            let pos = egui::pos2(center.x + slot.dx, center.y + slot.dy);
            painter.circle_filled(pos, CIRCLE_RADIUS,
                egui::Color32::from_rgb(r, g, b));
            painter.circle_stroke(pos, CIRCLE_RADIUS,
                egui::Stroke::new(0.6, egui::Color32::from_rgb(110, 120, 135)));
        }

        // Highlight selected (swap mode) — orange ring.
        if self.swap_mode {
            if let Some(i) = self.selected_for_swap {
                if let Some(slot) = self.slots.get(i) {
                    let pos = egui::pos2(center.x + slot.dx, center.y + slot.dy);
                    painter.circle_stroke(pos, CIRCLE_RADIUS + 3.5,
                        egui::Stroke::new(2.5, egui::Color32::from_rgb(245, 165, 35)));
                }
            }
        }
        // Highlight hover — pale ring.
        if let Some(i) = hovered_slot {
            if let Some(slot) = self.slots.get(i) {
                let pos = egui::pos2(center.x + slot.dx, center.y + slot.dy);
                painter.circle_stroke(pos, CIRCLE_RADIUS + 2.0,
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 235, 245)));
            }
        }

        // Click handling.
        if resp.clicked() {
            if let Some(i) = hovered_slot {
                if self.swap_mode {
                    match self.selected_for_swap {
                        None    => { self.selected_for_swap = Some(i); }
                        Some(a) => {
                            self.mapping.swap(a, i);
                            self.selected_for_swap = None;
                        }
                    }
                    return None;
                } else {
                    return Some(self.mapping[i]);
                }
            }
        }
        None
    }

    /// Render an "excluded row" — a horizontal strip of fixed-position
    /// ACI swatches. Returns `Some(aci)` on click. Hover updates the
    /// shared `hovered_aci`. Excluded rows are NOT subject to swap mode.
    pub fn excluded_row_ui(
        &mut self,
        ui: &mut egui::Ui,
        acis: impl IntoIterator<Item = u8>,
    ) -> Option<u8> {
        let mut clicked: Option<u8> = None;
        ui.horizontal(|ui| {
            for aci in acis {
                let (r, g, b) = aci_palette(aci);
                ui.vertical(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(28.0, 22.0), egui::Sense::click());
                    ui.painter().rect_filled(
                        rect, 3.0, egui::Color32::from_rgb(r, g, b));
                    ui.painter().rect_stroke(
                        rect, 3.0,
                        egui::Stroke::new(0.7, egui::Color32::from_rgb(110, 120, 135)));
                    if resp.hovered() {
                        self.hovered_aci = Some(aci);
                    }
                    if resp.on_hover_text(format!("ACI {}", aci)).clicked() {
                        clicked = Some(aci);
                    }
                    ui.small(format!("{}", aci));
                });
            }
        });
        clicked
    }

    /// Pixel-edge size of the wheel's bounding square. Derived once at
    /// construction from the slot table's outermost radius.
    fn required_size(&self) -> f32 {
        let mut max_r = 0.0_f32;
        for s in &self.slots {
            let r = (s.dx * s.dx + s.dy * s.dy).sqrt();
            if r > max_r { max_r = r; }
        }
        (max_r + CIRCLE_RADIUS) * 2.0 + 12.0
    }
}

// ---- deterministic layout + default mapping ----------------------------

fn build_layout_and_default_mapping() -> (Vec<Slot>, Vec<u8>) {
    // 1. Collect the ACI indices that BELONG on the wheel — everything
    //    except the two excluded bands. Luminance-sort so the brightest
    //    goes to the center; rest fill rings in luminance-descending
    //    order, then per-ring they re-sort angularly by hue.
    let mut indices: Vec<u8> = (0..=255_u8)
        .filter(|i| !EXCLUDED_NAMED.contains(i) && !EXCLUDED_GRAY.contains(i))
        .collect();
    assert_eq!(indices.len(), WHEEL_SLOT_COUNT,
        "wheel should hold exactly {} colors (256 minus excluded bands)",
        WHEEL_SLOT_COUNT);
    indices.sort_by(|a, b| {
        luminance(aci_palette(*b)).partial_cmp(&luminance(aci_palette(*a)))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let center_aci = indices.remove(0);

    // 2. Compute ring radii + per-ring capacity by walking outward,
    //    stopping when we've covered the remaining indices.
    let diameter = CIRCLE_RADIUS * 2.0;
    let radial_step = diameter + RADIAL_GAP;
    let first_ring_radius = diameter + RADIAL_GAP;
    let mut rings: Vec<(f32, usize)> = Vec::new();
    let mut remaining = indices.len();
    let mut ring_i = 0_usize;
    while remaining > 0 {
        let radius = first_ring_radius + (ring_i as f32) * radial_step;
        let circ = std::f32::consts::TAU * radius;
        let cap = ((circ / (diameter + TANGENTIAL_GAP)).floor() as usize).max(1);
        let take = cap.min(remaining);
        rings.push((radius, take));
        remaining = remaining.saturating_sub(take);
        ring_i += 1;
        if ring_i > 64 { break; }   // safety
    }

    // 3. Walk the luminance-sorted indices, slice into rings, then within
    //    each ring sort by ideal hue angle and assign to ring slots.
    let mut slots = Vec::with_capacity(WHEEL_SLOT_COUNT);
    let mut mapping: Vec<u8> = vec![0_u8; WHEEL_SLOT_COUNT];

    // Center slot.
    slots.push(Slot { dx: 0.0, dy: 0.0 });
    mapping[0] = center_aci;

    let mut head = 0_usize;
    for (radius, cap) in &rings {
        let chunk = &indices[head .. head + *cap];
        head += *cap;

        let mut by_angle: Vec<(u8, f32)> = chunk.iter()
            .map(|aci| (*aci, ideal_angle_for(aci_palette(*aci))))
            .collect();
        by_angle.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let angle_step = std::f32::consts::TAU / (*cap as f32);
        for (i, (aci, _)) in by_angle.into_iter().enumerate() {
            let theta = (i as f32) * angle_step;
            let dx = *radius * theta.cos();
            let dy = *radius * theta.sin();
            let slot_idx = slots.len();
            slots.push(Slot { dx, dy });
            mapping[slot_idx] = aci;
        }
    }

    (slots, mapping)
}

// ---- color math helpers -----------------------------------------------

fn luminance((r, g, b): (u8, u8, u8)) -> f32 {
    0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32)
}

/// Map an RGB triple to its "ideal" angular slot on the picker wheel.
/// Hue values from HSL are remapped through `hue_map` so the visible
/// arrangement matches the reference (red ~0°, yellow ~315°, green
/// ~210°, blue ~120°, magenta ~45°).
fn ideal_angle_for(rgb: (u8, u8, u8)) -> f32 {
    let r = rgb.0 as f32; let g = rgb.1 as f32; let b = rgb.2 as f32;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    let s = if max < 1.0 { 0.0 } else { d / max };

    if s < 0.12 {
        return if l > 160.0 { 270.0_f32.to_radians() } else { 90.0_f32.to_radians() };
    }

    let mut h = if max == min {
        0.0
    } else if max == r {
        let mut h = (g - b) / d;
        if g < b { h += 6.0; }
        h
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h /= 6.0;
    h *= 360.0;

    const HUE_MAP: &[(f32, f32)] = &[
        (0.0,   0.0),  (30.0,  315.0), (60.0,  240.0), (90.0,  210.0),
        (120.0, 180.0),(180.0, 150.0), (240.0, 120.0), (260.0, 100.0),
        (280.0,  80.0),(300.0,  45.0), (360.0,   0.0),
    ];
    let mut deg = 0.0_f32;
    for w in HUE_MAP.windows(2) {
        let (h1, a1) = w[0];
        let (h2, a2) = w[1];
        if h >= h1 && h <= h2 {
            let t = if h2 > h1 { (h - h1) / (h2 - h1) } else { 0.0 };
            let a1_eff = if a1 == 0.0 && a2 == 315.0 { 360.0 } else { a1 };
            deg = (a1_eff + t * (a2 - a1_eff)).rem_euclid(360.0);
            break;
        }
    }
    deg.to_radians()
}

// ---- tests ------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_has_correct_slot_count() {
        let (slots, mapping) = build_layout_and_default_mapping();
        assert_eq!(slots.len(), WHEEL_SLOT_COUNT);
        assert_eq!(mapping.len(), WHEEL_SLOT_COUNT);
        // Every wheel ACI appears exactly once; no excluded ACI sneaks in.
        let mut seen = [false; 256];
        for v in mapping.iter() {
            assert!(!EXCLUDED_NAMED.contains(v), "ACI {} is excluded but appeared on the wheel", v);
            assert!(!EXCLUDED_GRAY.contains(v),  "ACI {} is excluded but appeared on the wheel", v);
            assert!(!seen[*v as usize], "ACI {} appears more than once on the wheel", v);
            seen[*v as usize] = true;
        }
        // Conversely, every non-excluded ACI must appear.
        for i in 0..=255_u8 {
            let on_wheel = seen[i as usize];
            let is_excluded = EXCLUDED_NAMED.contains(&i) || EXCLUDED_GRAY.contains(&i);
            assert_eq!(on_wheel, !is_excluded, "ACI {} placement mismatch", i);
        }
    }

    #[test]
    fn center_is_brightest_among_wheel_acis() {
        let (_slots, mapping) = build_layout_and_default_mapping();
        let centre_lum = luminance(aci_palette(mapping[0]));
        for v in mapping.iter().skip(1) {
            let l = luminance(aci_palette(*v));
            assert!(centre_lum + 0.5 >= l);
        }
    }

    #[test]
    fn save_and_load_mapping_roundtrip() {
        let mut a = AciPickerState::default();
        a.mapping.swap(5, 200);
        a.mapping.swap(17, 42);

        let path = std::env::temp_dir().join("rust_cad_aci_picker_test.json");
        a.save_mapping(&path).unwrap();

        let mut b = AciPickerState::default();
        b.try_load_mapping(&path);
        assert_eq!(a.mapping, b.mapping);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_rejects_excluded_or_wrong_length() {
        let mut a = AciPickerState::default();
        let orig = a.mapping.clone();
        let path = std::env::temp_dir().join("rust_cad_aci_picker_bad.json");

        // Wrong length (256, like the pre-exclusion file format).
        let too_long: String = (0..=255_u8).map(|i| i.to_string())
            .collect::<Vec<_>>().join(",");
        std::fs::write(&path, format!("[{}]", too_long)).unwrap();
        a.try_load_mapping(&path);
        assert_eq!(a.mapping, orig, "pre-exclusion file must be rejected");

        // Right length but contains an excluded ACI (5).
        let mut bad = orig.clone();
        bad[10] = 5;
        let s: String = bad.iter().map(|i| i.to_string())
            .collect::<Vec<_>>().join(",");
        std::fs::write(&path, format!("[{}]", s)).unwrap();
        a.try_load_mapping(&path);
        assert_eq!(a.mapping, orig, "file with excluded ACI must be rejected");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn corrupt_file_is_silent_noop() {
        let mut a = AciPickerState::default();
        let orig = a.mapping.clone();
        let path = std::env::temp_dir().join("rust_cad_aci_picker_corrupt.json");
        std::fs::write(&path, b"not json").unwrap();
        a.try_load_mapping(&path);
        assert_eq!(a.mapping, orig);
        let _ = std::fs::remove_file(path);
    }
}
