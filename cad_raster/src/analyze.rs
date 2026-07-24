//! Convertibility ANALYZER — score the working raster before tracing so the
//! workflow doesn't try to vectorise a photo into garbage. Heuristic, fast:
//! approximate colour count + edge density → a class + confidence.

use image::{DynamicImage, GenericImageView};
use std::collections::HashSet;

/// What the raster looks like — drives which engines make sense.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RasterClass {
    /// Few colours, high structured-edge density → centerline trace + fit.
    LineArt,
    /// Big flat colour regions → outline trace / fill.
    FilledRegions,
    /// Continuous tone → not a good vector candidate (warn the user).
    PhotoNotSuitable,
}

#[derive(Clone, Debug)]
pub struct Report {
    /// Distinct colours after a coarse quantise (line art = few).
    pub approx_colors: usize,
    /// Fraction of pixels that sit on a strong intensity edge (0..1).
    pub edge_density:  f32,
    pub class:         RasterClass,
    /// 0..1 confidence in the class.
    pub confidence:    f32,
}

/// Analyze a working raster. Samples on a stride for speed on big scans.
pub fn analyze(img: &DynamicImage) -> Report {
    let (w, h) = img.dimensions();
    let rgba = img.to_rgba8();
    let stride = ((w.max(h) / 512).max(1)) as u32;   // cap work on huge images

    // Coarse colour count (quantise to 4 bits/channel).
    let mut colors: HashSet<u16> = HashSet::new();
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let p = rgba.get_pixel(x, y).0;
            let key = ((p[0] as u16 >> 4) << 8)
                    | ((p[1] as u16 >> 4) << 4)
                    |  (p[2] as u16 >> 4);
            colors.insert(key);
            x += stride;
        }
        y += stride;
    }

    // Edge density on the luma image (simple horizontal+vertical gradient).
    let g = img.to_luma8();
    let mut edge = 0u64;
    let mut total = 0u64;
    let mut y = 1;
    while y + 1 < h {
        let mut x = 1;
        while x + 1 < w {
            let c  = g.get_pixel(x, y).0[0] as i32;
            let gx = (g.get_pixel(x + 1, y).0[0] as i32 - c).abs();
            let gy = (g.get_pixel(x, y + 1).0[0] as i32 - c).abs();
            if gx.max(gy) > 40 { edge += 1; }
            total += 1;
            x += stride;
        }
        y += stride;
    }
    let edge_density = if total > 0 { edge as f32 / total as f32 } else { 0.0 };

    // Classify.
    let approx_colors = colors.len();
    let (class, confidence) = if approx_colors <= 24 && edge_density > 0.02 {
        (RasterClass::LineArt, 0.85)
    } else if approx_colors <= 256 {
        (RasterClass::FilledRegions, 0.6)
    } else {
        (RasterClass::PhotoNotSuitable, 0.7)
    };

    Report { approx_colors, edge_density, class, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Luma, GrayImage};

    #[test]
    fn line_art_image_classifies_as_line_art() {
        // Black vertical lines on white → few colours, clear edges.
        let g = GrayImage::from_fn(64, 64, |x, _|
            Luma([if x % 8 == 0 { 0 } else { 255 }]));
        let r = analyze(&DynamicImage::ImageLuma8(g));
        assert_eq!(r.class, RasterClass::LineArt);
        assert!(r.approx_colors <= 24);
    }
}
