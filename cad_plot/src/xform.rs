//! Model→paper transform — the core plot math.
//!
//! Produces page coordinates in **millimetres** with a **bottom-left origin**
//! (matching `printpdf::Point`), so no manual Y-flip is needed: model +Y (up)
//! maps to page +Y (up).
//!
//! CRITICAL INVARIANT (the #1 thing to not get wrong): this transform positions
//! geometry only. It scales *coordinates*, never *lineweights*. A plotted stroke
//! width is a physical mm value (`plot_width_mm × lw_scale`) applied unchanged at
//! emit time — so 0.25 mm prints 0.25 mm at 1:1 AND at 1:100. See `plot::tests`
//! for the guard that asserts identical stroke widths across scales.

use cad_kernel::math::Vec2;
use cad_kernel::plotstyle::{Offset, Orientation, PlotConfig, PlotScale};

/// An isotropic affine `page_mm = model * s + t`, plus the physical page size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageXform {
    /// mm-on-paper per model-unit (isotropic — same in X and Y).
    pub s:  f64,
    pub tx: f64,
    pub ty: f64,
    /// Physical page size in mm, AFTER the orientation swap.
    pub page_w_mm: f64,
    pub page_h_mm: f64,
}

impl PageXform {
    /// Map a world point to page millimetres (bottom-left origin).
    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.s + self.tx, y * self.s + self.ty)
    }

    /// Build the transform from the plot config and the world bounding box.
    ///
    /// `bbox_min`/`bbox_max` are the world-space extents of whatever area is
    /// being plotted (already resolved by the caller: Extents = doc bbox,
    /// Window = the two picked points, Display = the view rect).
    pub fn build(cfg: &PlotConfig, bbox_min: Vec2, bbox_max: Vec2) -> PageXform {
        // Physical page, swapped for landscape.
        let (pw0, ph0) = cfg.paper.dims_mm();
        let (page_w, page_h) = match cfg.orientation {
            Orientation::Portrait  => (pw0 as f64, ph0 as f64),
            Orientation::Landscape => (ph0 as f64, pw0 as f64),
        };
        let m = cfg.margins_mm.max(0.0) as f64;
        // Printable area (never negative).
        let printable_w = (page_w - 2.0 * m).max(1.0);
        let printable_h = (page_h - 2.0 * m).max(1.0);

        // World span (guard degenerate / empty bbox).
        let bw = (bbox_max.x - bbox_min.x).max(1e-9);
        let bh = (bbox_max.y - bbox_min.y).max(1e-9);

        // Scale: mm-on-paper per model-unit.
        let s = match cfg.scale {
            PlotScale::Fit => (printable_w / bw).min(printable_h / bh),
            // Ratio{model, paper_mm}: `paper_mm` mm on paper == `model` model
            // units. e.g. 1:100 with model units in mm → Ratio{100, 1} → 0.01.
            PlotScale::Ratio { model, paper_mm } => {
                if model.abs() < 1e-12 { 1.0 } else { paper_mm / model }
            }
        };

        // Content span on paper (mm).
        let content_w = bw * s;
        let content_h = bh * s;

        // Translation so `bbox_min` lands at the intended lower-left anchor.
        let (tx, ty) = match cfg.offset {
            Offset::Center => {
                // Centre the content within the printable area.
                let ox = m + (printable_w - content_w) * 0.5;
                let oy = m + (printable_h - content_h) * 0.5;
                (ox - bbox_min.x * s, oy - bbox_min.y * s)
            }
            Offset::Xy { x_mm, y_mm } => {
                // Offset measured from the lower-left of the printable area.
                let ox = m + x_mm as f64;
                let oy = m + y_mm as f64;
                (ox - bbox_min.x * s, oy - bbox_min.y * s)
            }
        };

        PageXform { s, tx, ty, page_w_mm: page_w, page_h_mm: page_h }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::plotstyle::{Offset, Orientation, PaperSize, PlotArea, PlotScale, PlotTarget};
    use std::path::PathBuf;

    fn cfg_with(scale: PlotScale, orient: Orientation, offset: Offset) -> PlotConfig {
        PlotConfig {
            output:      PlotTarget::PdfFile(PathBuf::new()),
            paper:       PaperSize::A3,
            orientation: orient,
            area:        PlotArea::Extents,
            scale,
            offset,
            lw_scale:    1.0,
            monochrome:  false,
            margins_mm:  5.0,
            plot_layout_index: None,
            ctb_tables:  Default::default(),
        }
    }

    #[test]
    fn landscape_swaps_page_dims() {
        // A3 portrait = 297×420; landscape = 420×297.
        let c = cfg_with(PlotScale::Fit, Orientation::Landscape, Offset::Center);
        let t = PageXform::build(&c, Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        assert_eq!(t.page_w_mm, 420.0);
        assert_eq!(t.page_h_mm, 297.0);
    }

    #[test]
    fn ratio_1_to_100_gives_scale_0_01() {
        let c = cfg_with(PlotScale::Ratio { model: 100.0, paper_mm: 1.0 },
                         Orientation::Landscape, Offset::Center);
        let t = PageXform::build(&c, Vec2::new(0.0, 0.0), Vec2::new(1000.0, 1000.0));
        assert!((t.s - 0.01).abs() < 1e-12, "expected s=0.01, got {}", t.s);
    }

    #[test]
    fn fit_centers_content_in_printable() {
        // A square drawing on landscape A3, Fit + Center → content centred.
        let c = cfg_with(PlotScale::Fit, Orientation::Landscape, Offset::Center);
        let (mn, mx) = (Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
        let t = PageXform::build(&c, mn, mx);
        // The four corners must sit inside the printable area, and the
        // bbox centre must map to the page centre.
        let (cx, cy) = t.apply(50.0, 50.0);
        assert!((cx - t.page_w_mm / 2.0).abs() < 1e-6, "x centre off: {}", cx);
        assert!((cy - t.page_h_mm / 2.0).abs() < 1e-6, "y centre off: {}", cy);
    }

    #[test]
    fn no_y_flip_up_is_up() {
        // A point higher in model space maps higher on the page (bottom-left origin).
        let c = cfg_with(PlotScale::Fit, Orientation::Landscape, Offset::Center);
        let t = PageXform::build(&c, Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
        let (_, y_low) = t.apply(50.0, 10.0);
        let (_, y_high) = t.apply(50.0, 90.0);
        assert!(y_high > y_low, "model +Y should map to page +Y");
    }

    #[test]
    fn fit_content_fits_within_printable() {
        // A wide drawing: Fit must not exceed the printable width/height.
        let c = cfg_with(PlotScale::Fit, Orientation::Portrait, Offset::Center);
        let (mn, mx) = (Vec2::new(-500.0, -20.0), Vec2::new(500.0, 20.0));
        let t = PageXform::build(&c, mn, mx);
        for &(x, y) in &[(mn.x, mn.y), (mx.x, mn.y), (mx.x, mx.y), (mn.x, mx.y)] {
            let (px, py) = t.apply(x, y);
            assert!(px >= 5.0 - 1e-6 && px <= t.page_w_mm - 5.0 + 1e-6, "x {} out of printable", px);
            assert!(py >= 5.0 - 1e-6 && py <= t.page_h_mm - 5.0 + 1e-6, "y {} out of printable", py);
        }
    }
}
