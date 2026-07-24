// Lineweight — DXF group code 370 semantics.
//
// AutoCAD enumerates lineweights in 0.01 mm units; the enum here distinguishes
// sentinel inheritance (ByLayer / ByBlock / Default) from absolute widths.
// `Custom(mm)` carries a millimetre value for the renderer to convert into
// pixel widths via the current viewport scale.

use crate::layer::LayerTable;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lineweight {
    /// Inherit from the entity's layer.
    ByLayer,
    /// Inherit from the containing block (falls back to ByLayer outside blocks).
    ByBlock,
    /// Whatever the document's default lineweight is (typically 0.25 mm).
    Default,
    /// Absolute width in millimetres.
    Custom(f32),
}

impl Default for Lineweight {
    fn default() -> Self { Lineweight::ByLayer }
}

/// Document default — what `Lineweight::Default` resolves to.
pub const DEFAULT_LINEWEIGHT_MM: f32 = 0.25;

/// Resolve a Dobject's lineweight through the ByLayer / ByBlock / Default chain.
/// Returns absolute millimetres. The renderer scales by viewport ppi/zoom.
pub fn resolve_lineweight(lw: Lineweight, layer_id: u32, layers: &LayerTable) -> f32 {
    match lw {
        Lineweight::Custom(mm) => mm,
        Lineweight::Default    => DEFAULT_LINEWEIGHT_MM,
        Lineweight::ByLayer | Lineweight::ByBlock => {
            let lyr_lw = layers.get(layer_id)
                .map(|l| l.lineweight)
                .unwrap_or(Lineweight::Default);
            match lyr_lw {
                Lineweight::Custom(mm) => mm,
                _                      => DEFAULT_LINEWEIGHT_MM,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::layer::{Layer, LayerTable};

    #[test]
    fn resolve_custom_is_passthrough() {
        let t = LayerTable::with_defaults();
        assert_eq!(resolve_lineweight(Lineweight::Custom(0.5), 0, &t), 0.5);
    }

    #[test]
    fn resolve_bylayer_reads_layer() {
        let mut t = LayerTable::with_defaults();
        let id = t.add(Layer {
            name:       "HEAVY".into(),
            color:      Color::ByLayer,
            linetype:   0,
            lineweight: Lineweight::Custom(1.0),
            visible:    true, locked: false, frozen: false, plottable: true,
        });
        assert_eq!(resolve_lineweight(Lineweight::ByLayer, id, &t), 1.0);
    }
}
