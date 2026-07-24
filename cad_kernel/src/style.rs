// Style — the "Common properties" every Dobject carries.
//
// The full set listed in `Dobject_Properties.md` lands here over time. Today
// we model the five that the renderer + Layer panel will use first:
// layer, color, linetype, linetype_scale, lineweight, visible. The rest
// (transparency, plot_style, material, annotation, xdata, …) will be added
// as features need them — design intent is that a Dobject's full property
// set lives in this struct, NOT scattered across the geometry variants.

use crate::color::Color;
use crate::layer::{LayerId, LayerTable};
use crate::lineweight::Lineweight;
use crate::linetype::LinetypeTable;

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub layer:          LayerId,
    pub color:          Color,
    pub linetype:       u32,        // LinetypeId
    pub linetype_scale: f32,
    pub lineweight:     Lineweight,
    pub visible:        bool,
}

impl Default for Style {
    /// Defaults match a fresh AutoCAD entity: layer "0", everything ByLayer,
    /// linetype scale 1.0, visible.
    fn default() -> Self {
        Self {
            layer:          LayerTable::LAYER_ZERO,
            color:          Color::ByLayer,
            linetype:       LinetypeTable::CONTINUOUS,
            linetype_scale: 1.0,
            lineweight:     Lineweight::ByLayer,
            visible:        true,
        }
    }
}

impl Style {
    /// Style on the given layer with everything else ByLayer / default.
    /// Most Dobjects are created this way — the active layer at creation
    /// time supplies its color/linetype/lineweight automatically.
    pub fn on_layer(layer: LayerId) -> Self {
        Self { layer, ..Self::default() }
    }
}
