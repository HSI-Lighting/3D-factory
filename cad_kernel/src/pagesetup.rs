//! PAGESETUP — the document's saved page configuration for model-space
//! plotting (AutoCAD PAGESETUP). Layouts carry their own setup inline; this
//! is the model-space default the Plot dialog starts from and PAGESETUP
//! edits.

use crate::plotstyle::{Orientation, PaperSize};

/// Saved page setup for model-space plots.
#[derive(Clone, Debug, PartialEq)]
pub struct PageSetup {
    pub paper: PaperSize,
    pub orientation: Orientation,
    pub margins_mm: f64,
    /// true = Fit to paper, false = fixed 1:N.
    pub scale_fit: bool,
    /// The `N` in 1:N when `!scale_fit`.
    pub scale_n: f64,
    pub unit_inch: bool,
    pub ctb_name: Option<String>,
}

impl Default for PageSetup {
    fn default() -> Self {
        PageSetup {
            paper: PaperSize::A4,
            orientation: Orientation::Portrait,
            margins_mm: 0.0,
            scale_fit: true,
            scale_n: 1.0,
            unit_inch: false,
            ctb_name: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_a4_portrait_fit() {
        let d = PageSetup::default();
        assert_eq!(d.paper, PaperSize::A4);
        assert_eq!(d.orientation, Orientation::Portrait);
        assert!(d.scale_fit);
    }
}
