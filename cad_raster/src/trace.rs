//! Type-aware TRACE dispatch — the rewritten Stage 2. Outline AND centerline
//! are both correct, for different content; the human's asset TAG selects the
//! engine, which runs on the masked sub-raster only and emits `cad_kernel`
//! DObjects. The engines themselves land in later slices (see RASTER_TO_VECTOR.md);
//! this is the stable dispatch the editor calls.

use cad_kernel::Geom;
use image::{DynamicImage, GrayImage};

/// The semantic type a human assigned to a mask layer — picks the convert
/// engine. (`Copy` so masks/UI pass it freely.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// → OCR → `Text` (NOT geometry — this is what kept old CAD sane).
    Text,
    /// → extension-line + arrowhead recognition + linked OCR value → `Dimension`.
    Dimension,
    /// → outline trace + curve-fit; repeated symbols → template-match → block.
    Furniture,
    /// → centerline trace (thin) + double-line→wall recognition → `Wall`.
    Wall,
    /// → centerline + Hough line/circle + least-squares arc fit.
    LineArt,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Text      => "Text",
            AssetKind::Dimension => "Dimension",
            AssetKind::Furniture => "Furniture",
            AssetKind::Wall      => "Wall",
            AssetKind::LineArt   => "Line-art",
        }
    }
}

/// Convert one masked asset into DObjects using the type-appropriate engine.
/// `mask` (255 = included) scopes `working` to the asset's pixels.
///
/// SLICE 1: dispatch + signature are stable; engines return empty for now.
/// Each TODO is its own follow-up slice.
pub fn convert(asset: AssetKind, _mask: &GrayImage, _working: &DynamicImage) -> Vec<Geom> {
    match asset {
        // TODO(slice 5): OCR the masked region → Text dobjects.
        AssetKind::Text => Vec::new(),
        // TODO(slice 5): recognise extension lines + arrowheads, link OCR value.
        AssetKind::Dimension => Vec::new(),
        // TODO(slice 5): outline trace + curve-fit; repeat clusters → blocks.
        AssetKind::Furniture => Vec::new(),
        // TODO(slice 3/5): centerline thin + double-line pairing → Wall.
        AssetKind::Wall => Vec::new(),
        // TODO(slice 3): centerline + Hough + arc fit → Line/Arc/Circle/Spline.
        AssetKind::LineArt => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma, RgbaImage, Rgba};

    #[test]
    fn dispatch_is_stable_and_empty_for_now() {
        let mask = GrayImage::from_pixel(4, 4, Luma([255]));
        let working = DynamicImage::ImageRgba8(
            RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255])));
        for k in [AssetKind::Text, AssetKind::Dimension, AssetKind::Furniture,
                  AssetKind::Wall, AssetKind::LineArt] {
            assert!(convert(k, &mask, &working).is_empty());
        }
    }
}
