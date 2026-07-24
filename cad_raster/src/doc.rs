//! `RasterDoc` — the Photoshop-style raster-layer buffer. An ordered stack
//! composited top→bottom into a **working raster** that detection + marking run
//! on. The peel loop is layer ops: mark an asset → it becomes a Mask layer →
//! convert it → DObjects on a CAD layer → hide/subtract → repeat (the working
//! raster gets simpler each peel).

use image::{DynamicImage, GenericImageView, GrayImage};

use crate::adjust::Adjustment;
use crate::trace::AssetKind;

/// What a raster layer is.
#[derive(Clone, Debug)]
pub enum LayerKind {
    /// A non-destructive image adjustment applied to the composite below it.
    Adjust(Adjustment),
    /// A painted/detected mask isolating ONE semantic asset.
    Mask(Mask),
}

/// A raster layer in the stack (above the immutable base).
#[derive(Clone, Debug)]
pub struct RasterLayer {
    pub name:    String,
    pub visible: bool,
    pub opacity: f32,
    pub kind:    LayerKind,
}

/// A binary mask isolating one semantic asset (255 = included). Carries the
/// asset TYPE the human assigned, which selects the convert engine (see
/// `crate::trace`). Maps to a named CAD layer on convert.
#[derive(Clone, Debug)]
pub struct Mask {
    pub name:      String,
    pub asset:     AssetKind,
    /// Target CAD layer name the converted DObjects land on (e.g. "WALLS").
    pub cad_layer: String,
    /// 255 = pixel belongs to this asset, 0 = excluded. Same size as the base.
    pub buf:       GrayImage,
}

/// The raster document: an immutable base scan + a stack of adjustment / mask
/// layers.
pub struct RasterDoc {
    /// The loaded scan (read-only original; always the bottom of the stack).
    pub base:   DynamicImage,
    /// Adjustment + mask layers, bottom-to-top.
    pub layers: Vec<RasterLayer>,
}

impl RasterDoc {
    /// Load a raster from disk (PNG / JPEG / BMP / TIFF via the `image` crate).
    pub fn load<P: AsRef<std::path::Path>>(path: P) -> Result<Self, image::ImageError> {
        Ok(Self { base: image::open(path)?, layers: Vec::new() })
    }

    /// Wrap an already-decoded image (e.g. for tests / in-memory pipelines).
    pub fn from_image(base: DynamicImage) -> Self {
        Self { base, layers: Vec::new() }
    }

    pub fn width(&self)  -> u32 { self.base.dimensions().0 }
    pub fn height(&self) -> u32 { self.base.dimensions().1 }

    /// Composite the working raster: the base with every ENABLED adjustment
    /// layer applied in order. Mask layers don't change the working raster
    /// (they isolate assets for conversion / subtraction).
    pub fn working(&self) -> DynamicImage {
        let mut img = self.base.clone();
        for l in &self.layers {
            if !l.visible { continue; }
            if let LayerKind::Adjust(a) = &l.kind {
                if a.enabled { img = a.kind.apply(&img); }
            }
        }
        img
    }

    /// Push an adjustment layer on top.
    pub fn add_adjustment(&mut self, a: Adjustment) {
        self.layers.push(RasterLayer {
            name: a.name.clone(), visible: true, opacity: 1.0,
            kind: LayerKind::Adjust(a),
        });
    }

    /// Push a mask (asset) layer on top.
    pub fn add_mask(&mut self, m: Mask) {
        self.layers.push(RasterLayer {
            name: m.name.clone(), visible: true, opacity: 1.0,
            kind: LayerKind::Mask(m),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjust::{AdjustKind, Adjustment};
    use image::{DynamicImage, Rgba, RgbaImage};

    #[test]
    fn working_applies_adjustment_stack() {
        let base = DynamicImage::ImageRgba8(
            RgbaImage::from_pixel(8, 8, Rgba([120, 120, 120, 255])));
        let mut doc = RasterDoc::from_image(base);
        assert_eq!((doc.width(), doc.height()), (8, 8));
        doc.add_adjustment(Adjustment::new(AdjustKind::Grayscale));
        doc.add_adjustment(Adjustment::new(AdjustKind::Threshold(100)));
        let w = doc.working().to_luma8();
        // grey 120 > 100 → white everywhere.
        assert!(w.pixels().all(|p| p.0[0] == 255));
    }
}
