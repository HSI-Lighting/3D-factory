//! Non-destructive raster ADJUSTMENTS — the Photoshop-style prep layer. Each is
//! applied to the composite to produce the "working raster" that detection and
//! marking run on. All are real (image-crate pixel ops), no placeholders.

use image::{DynamicImage, GenericImageView, GrayImage, Luma, Rgba, RgbaImage};

/// One adjustment layer (a named, toggleable image op).
#[derive(Clone, Debug)]
pub struct Adjustment {
    pub name:    String,
    pub enabled: bool,
    pub kind:    AdjustKind,
}

impl Adjustment {
    pub fn new(kind: AdjustKind) -> Self {
        Self { name: kind.label().into(), enabled: true, kind }
    }
}

/// The image operations a prep layer can perform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AdjustKind {
    /// Drop colour → single-channel grey. Kills colour noise before tracing.
    Grayscale,
    /// Brightness shift (−255..255) + contrast factor (1.0 = unchanged).
    BrightnessContrast { brightness: i32, contrast: f32 },
    /// Binarise at a grey threshold (0..255): pixels > t → white, else black.
    Threshold(u8),
    /// Invert (white-on-black scans → black-on-white).
    Invert,
    /// Keep only pixels within `tol` of `target` colour (the rest → white).
    /// The "isolate by colour" peel for colour-coded drawings.
    IsolateColor { target: [u8; 3], tol: u8 },
}

impl AdjustKind {
    pub fn label(&self) -> &'static str {
        match self {
            AdjustKind::Grayscale            => "Grayscale",
            AdjustKind::BrightnessContrast { .. } => "Brightness / Contrast",
            AdjustKind::Threshold(_)         => "Threshold",
            AdjustKind::Invert               => "Invert",
            AdjustKind::IsolateColor { .. }  => "Isolate colour",
        }
    }

    /// Apply this adjustment, returning a new image (non-destructive).
    pub fn apply(&self, img: &DynamicImage) -> DynamicImage {
        match *self {
            AdjustKind::Grayscale => img.grayscale(),
            AdjustKind::BrightnessContrast { brightness, contrast } =>
                img.adjust_contrast(contrast).brighten(brightness),
            AdjustKind::Invert => {
                let mut rgba = img.to_rgba8();
                image::imageops::invert(&mut rgba);
                DynamicImage::ImageRgba8(rgba)
            }
            AdjustKind::Threshold(t) => {
                let g = img.to_luma8();
                let out = GrayImage::from_fn(g.width(), g.height(), |x, y| {
                    let v = g.get_pixel(x, y).0[0];
                    Luma([if v > t { 255 } else { 0 }])
                });
                DynamicImage::ImageLuma8(out)
            }
            AdjustKind::IsolateColor { target, tol } => {
                let rgba = img.to_rgba8();
                let tol = tol as i32;
                let out = RgbaImage::from_fn(rgba.width(), rgba.height(), |x, y| {
                    let p = rgba.get_pixel(x, y).0;
                    let d = (p[0] as i32 - target[0] as i32).abs()
                        .max((p[1] as i32 - target[1] as i32).abs())
                        .max((p[2] as i32 - target[2] as i32).abs());
                    if d <= tol { Rgba(p) } else { Rgba([255, 255, 255, 255]) }
                });
                DynamicImage::ImageRgba8(out)
            }
        }
    }
}

/// `dims` helper used by the doc/analyzer so callers don't pull the trait in.
pub fn dims(img: &DynamicImage) -> (u32, u32) { img.dimensions() }

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn checker() -> DynamicImage {
        // 4×4 image, half red half near-white.
        let img = RgbaImage::from_fn(4, 4, |x, _| {
            if x < 2 { Rgba([200, 20, 20, 255]) } else { Rgba([250, 250, 250, 255]) }
        });
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn grayscale_then_threshold_is_binary() {
        let g = AdjustKind::Grayscale.apply(&checker());
        let b = AdjustKind::Threshold(128).apply(&g).to_luma8();
        for p in b.pixels() { assert!(p.0[0] == 0 || p.0[0] == 255); }
    }

    #[test]
    fn isolate_color_keeps_target_drops_rest() {
        let out = AdjustKind::IsolateColor { target: [200, 20, 20], tol: 30 }
            .apply(&checker())
            .to_rgba8();
        // Red half kept, white half forced to white.
        assert_eq!(out.get_pixel(0, 0).0, [200, 20, 20, 255]);
        assert_eq!(out.get_pixel(3, 0).0, [255, 255, 255, 255]);
    }
}
