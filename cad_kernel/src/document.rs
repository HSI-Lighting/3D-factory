// Document — the in-memory drawing container.
//
// Analog of AutoCAD's AcDbDatabase / LibreCAD's RS_Graphic. Holds the
// Dobject list plus all the resource tables they reference (layers,
// linetypes). Future tables (blocks, text_styles, dim_styles, ucs_list,
// named_views) slot in as new fields without touching call sites that
// already work with the current ones.

use crate::color::TrueColorTable;
use crate::dobject::DObject;
use crate::layer::LayerTable;
use crate::linetype::LinetypeTable;
use crate::pen::PenTable;
use crate::text::TextStyleTable;
use crate::dim::DimStyleTable;
use crate::wallstyle::WallStyleTable;
use crate::block::BlockTable;
use crate::math::Vec2;
use std::sync::Arc;

/// An embedded reference raster image — an underlay you draft over (NOT
/// vectorized). `data` is the ORIGINAL encoded file bytes (PNG/JPEG/…), shared
/// via `Arc` so cloning the Document for undo stays cheap. Placement is in world
/// units: `insert` is the image's TOP-LEFT corner, the image spans `world_w`
/// to the right and `world_h` downward (1 world unit per source pixel by
/// default). Persisted in the native RSM file (v4+); DXF does not embed it.
#[derive(Clone, Debug)]
pub struct RasterImage {
    pub name:    String,
    pub data:    Arc<Vec<u8>>,
    pub insert:  Vec2,
    pub world_w: f64,
    pub world_h: f64,
}

#[derive(Clone)]
pub struct Document {
    pub dobjects:    Vec<DObject>,
    pub layers:      LayerTable,
    pub linetypes:   LinetypeTable,
    pub pens:        PenTable,
    /// Shared 24-bit color table. Dobjects with `Color::TrueColorRef(idx)`
    /// look up their RGB here. Dedup'd on `intern`, so a million dobjects
    /// in the same color cost ~4 bytes once.
    pub truecolors:  TrueColorTable,
    /// Named text styles. Dobjects with `Geom::Text(t)` reference an
    /// entry via `t.style`. Index 0 is reserved STANDARD.
    pub text_styles: TextStyleTable,
    /// Named dimension styles. Dobjects with `Geom::Dimension(d)`
    /// reference an entry via `d.style`. Index 0 is reserved STANDARD.
    pub dim_styles:  DimStyleTable,
    /// Named wall styles (Dry Wall / Structural / …). Walls reference an
    /// entry via `w.style`. Index 0 is reserved STANDARD.
    pub wall_styles: WallStyleTable,
    /// Block definitions. Dobjects with `Geom::BlockRef(br)` reference an
    /// entry via `br.block`. No reserved id-0 entry — starts empty.
    pub blocks:      BlockTable,
    /// Embedded reference raster underlays. Drafted over, not vectorized;
    /// persisted in RSM (v4+). Empty by default.
    pub raster_images: Vec<RasterImage>,
    // Reserved for future slices — leave the field list extensible:
    // pub ucs_list:    UcsList,
    // pub named_views: NamedViewList,
    // pub doc_settings: DocSettings,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            dobjects:    Vec::new(),
            layers:      LayerTable::with_defaults(),
            linetypes:   LinetypeTable::with_defaults(),
            pens:        PenTable::default(),
            truecolors:  TrueColorTable::new(),
            text_styles: TextStyleTable::with_defaults(),
            dim_styles:  DimStyleTable::with_defaults(),
            wall_styles: WallStyleTable::with_defaults(),
            blocks:      BlockTable::default(),
            raster_images: Vec::new(),
        }
    }
}

impl Document {
    /// Append a Dobject. Returns its new index in `dobjects`.
    pub fn push(&mut self, mut d: DObject) -> usize {
        // If the Dobject is being added with the default Style, pull the
        // active layer in so it inherits the user's current layer choice.
        if d.style.layer == LayerTable::LAYER_ZERO && self.layers.active != LayerTable::LAYER_ZERO {
            d.style.layer = self.layers.active;
        }
        let i = self.dobjects.len();
        self.dobjects.push(d);
        i
    }

    /// Count of layers (always ≥ 1 thanks to layer "0").
    pub fn layer_count(&self) -> usize { self.layers.len() }

    /// Convenience — does this Dobject pass per-layer render gating?
    pub fn is_visible(&self, dobj_index: usize) -> bool {
        let Some(d) = self.dobjects.get(dobj_index) else { return false; };
        if !d.style.visible { return false; }
        self.layers.renders(d.style.layer)
    }

    /// Convenience — can this Dobject be selected / edited?
    pub fn is_selectable(&self, dobj_index: usize) -> bool {
        let Some(d) = self.dobjects.get(dobj_index) else { return false; };
        if !d.style.visible { return false; }
        self.layers.selectable(d.style.layer)
    }

    /// Find a Dobject by its handle. Linear scan — fine at thousands;
    /// add a `HashMap<Handle, usize>` cache when the count climbs.
    /// Used today by the Hatch render path to resolve its boundary
    /// references each frame so moving the boundary auto-updates the
    /// hatch fill.
    pub fn find_by_handle(&self, h: crate::dobject::Handle) -> Option<&DObject> {
        self.dobjects.iter().find(|d| d.handle == h)
    }

    /// Index lookup by handle. Same scan as `find_by_handle`; returned
    /// index is valid only until the next mutation of `self.dobjects`.
    pub fn index_of_handle(&self, h: crate::dobject::Handle) -> Option<usize> {
        self.dobjects.iter().position(|d| d.handle == h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Geom};
    use crate::layer::Layer;
    use crate::math::Vec2;

    #[test]
    fn push_inherits_active_layer() {
        let mut doc = Document::default();
        let walls = doc.layers.add(Layer {
            name: "WALLS".into(), ..Layer::layer_zero()
        });
        doc.layers.active = walls;
        let i = doc.push(DObject::new(Geom::Circle(Circle {
            center: Vec2::ZERO, radius: 5.0,
        })));
        assert_eq!(doc.dobjects[i].style.layer, walls);
    }

    #[test]
    fn invisible_layer_hides_dobject() {
        let mut doc = Document::default();
        let hidden = doc.layers.add(Layer {
            name: "HIDDEN".into(), visible: false, ..Layer::layer_zero()
        });
        doc.layers.active = hidden;
        let i = doc.push(DObject::new(Geom::Circle(Circle {
            center: Vec2::ZERO, radius: 5.0,
        })));
        assert!(!doc.is_visible(i));
        assert!(!doc.is_selectable(i));
    }

    #[test]
    fn locked_layer_blocks_selection_but_renders() {
        let mut doc = Document::default();
        let locked = doc.layers.add(Layer {
            name: "LOCKED".into(), locked: true, ..Layer::layer_zero()
        });
        doc.layers.active = locked;
        let i = doc.push(DObject::new(Geom::Circle(Circle {
            center: Vec2::ZERO, radius: 5.0,
        })));
        assert!(doc.is_visible(i));
        assert!(!doc.is_selectable(i));
    }
}
