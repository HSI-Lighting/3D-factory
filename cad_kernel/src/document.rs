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

/// How a document's unit was arrived at. The distinction matters more than the number:
/// an ASSUMED unit is a fallback nobody chose, and the app must never write it out as a
/// positive assertion (e.g. into a DXF `$INSUNITS`) or act on it destructively.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UnitSource {
    /// Nobody said. Every drawing made before units existed lands here.
    #[default]
    Assumed,
    /// The file declared it (e.g. DXF `$INSUNITS`, or an RSM unit record).
    Declared,
    /// The user set it explicitly (the UNITS command).
    User,
}

/// The real-world size of one drawing unit.
///
/// The 2D side draws in whatever the user likes — architectural plans here are millimetres,
/// so a 3 m wall is drawn 3000 long. The 3D Factory, the renderer, the parametric generators
/// and the lux engine are all METRES. This type is what lets the two meet: it is the single
/// declaration of what a drawing's numbers mean, so the handful of 2D↔3D boundaries can
/// convert instead of guessing.
///
/// `metres_per_unit` is deliberately the stored direction (multiply doc → metres), because
/// that is the direction the boundaries actually need.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DocUnits {
    /// Metres in one drawing unit. 1.0 = the drawing is already metres; 0.001 = millimetres.
    pub metres_per_unit: f64,
    pub source: UnitSource,
}

impl Default for DocUnits {
    /// **1 unit = 1 metre, assumed.** This is the zero-delta default: every drawing that
    /// existed before units did behaves EXACTLY as it did, because every boundary multiplies
    /// by 1.0. Nothing is rescaled by loading a file — only an explicit user action changes a
    /// drawing's unit.
    fn default() -> Self {
        Self { metres_per_unit: 1.0, source: UnitSource::Assumed }
    }
}

impl DocUnits {
    pub const MM: f64 = 0.001;
    pub const CM: f64 = 0.01;
    pub const M: f64 = 1.0;
    pub const INCH: f64 = 0.0254;
    pub const FOOT: f64 = 0.3048;

    pub fn new(metres_per_unit: f64, source: UnitSource) -> Self {
        Self { metres_per_unit, source }
    }

    /// Drawing units → metres. The direction every 2D→3D boundary needs.
    #[inline]
    pub fn to_metres(&self, v: f64) -> f64 {
        v * self.metres_per_unit
    }

    /// Metres → drawing units. The direction every 3D→2D boundary needs, and the one that
    /// keeps a metre-shaped default (a 0.2 m wall thickness) honest when it is stored as a
    /// document-unit length.
    #[inline]
    pub fn from_metres(&self, v: f64) -> f64 {
        if self.metres_per_unit.abs() > 1e-12 { v / self.metres_per_unit } else { v }
    }

    /// A short label for the UI ("mm", "m", …), or a bare ratio when it is not a standard unit.
    pub fn label(&self) -> String {
        match self.metres_per_unit {
            x if (x - Self::MM).abs() < 1e-9 => "mm".into(),
            x if (x - Self::CM).abs() < 1e-9 => "cm".into(),
            x if (x - Self::M).abs() < 1e-9 => "m".into(),
            x if (x - Self::INCH).abs() < 1e-9 => "in".into(),
            x if (x - Self::FOOT).abs() < 1e-9 => "ft".into(),
            x => format!("{x} m/unit"),
        }
    }
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
    /// What one drawing unit is worth in the real world. Defaults to 1 unit = 1 metre,
    /// `Assumed` — which is exactly how the app behaved before units existed, so adding this
    /// field changes nothing until something explicitly sets it.
    pub units: DocUnits,
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
            units:       DocUnits::default(),
        }
    }
}

impl Document {
    /// TAKE EVERY SHARED TABLE FROM `src` — everything a dobject refers to BY INDEX.
    ///
    /// A dobject is not self-contained. Its style names a layer id, a linetype id and maybe a
    /// true-colour slot; a `Text` names a text style; a `Dimension` names a dimension style; a
    /// `Wall` names a wall style; a `BlockRef` names a block. Every one is an index into a table
    /// on the Document, so an object is only meaningful ALONGSIDE the tables it was drawn against.
    /// Move it to a document with different tables and it keeps its numbers and changes its
    /// meaning: index 3 was "Dashed" and is now "Center", index 2 was a 5 mm annotation style and
    /// is now a 200 mm title style. No error — a drawing that is subtly and quietly wrong.
    ///
    /// This exists because a face sketch is a whole `Document` of its own, built from
    /// `Document::default()`, so it starts with DEFAULT tables while the drawing it belongs to has
    /// the user's. Layers were carried across when someone noticed their colours were wrong, and
    /// blocks when someone noticed the Insert list was empty — one at a time, each after it broke.
    /// The remaining six would have been found the same way.
    ///
    /// NOT `dobjects`, obviously. And NOT `units`: what one drawing unit means is a fact about a
    /// FILE, and a sketch that adopted the plan's unit would rescale everything already drawn on
    /// it. That is a real defect with a fix of its own; carrying it here as a side effect of a
    /// table merge would be a silent rescale of existing work.
    pub fn adopt_tables_from(&mut self, src: &Document) {
        self.layers = src.layers.clone();
        self.linetypes = src.linetypes.clone();
        self.pens = src.pens.clone();
        self.truecolors = src.truecolors.clone();
        self.text_styles = src.text_styles.clone();
        self.dim_styles = src.dim_styles.clone();
        self.wall_styles = src.wall_styles.clone();
        self.blocks = src.blocks.clone();
    }

    /// A document holding only this one's TABLES — no dobjects, no raster underlays.
    ///
    /// For carrying a table set across a swap, where cloning the whole document would copy every
    /// object in it for nothing. Pair with [`Self::adopt_tables_from`].
    pub fn clone_tables_only(&self) -> Document {
        let mut d = Document::default();
        d.adopt_tables_from(self);
        d.units = self.units;
        d
    }

    /// Append a Dobject. Returns its new index in `dobjects`.
    /// Roughly how much memory this document occupies, in bytes.
    ///
    /// APPROXIMATE, and deliberately cheap: one pass over the dobjects summing each one's own size
    /// plus its owned buffers. Block definitions are counted too, because a plan made of blocks
    /// keeps most of its geometry in the table rather than in the objects.
    ///
    /// It exists so the undo stack can be bounded by MEMORY rather than by a count of steps. A
    /// count is the wrong unit: 64 steps of a 200-object sketch is nothing, and 64 steps of a
    /// 1.5-million-object plan is tens of gigabytes of identical-looking history.
    pub fn approx_bytes(&self) -> usize {
        let objs: usize = self.dobjects.iter().map(|d| d.geom.approx_bytes()).sum();
        let blocks: usize = self
            .blocks
            .blocks
            .iter()
            .map(|b| b.name.capacity() + b.dobjects.iter().map(|d| d.geom.approx_bytes()).sum::<usize>())
            .sum();
        // The per-DObject overhead beyond its geom (handle, style, flags) rides on the struct size.
        let per_obj = self.dobjects.capacity() * (std::mem::size_of::<crate::dobject::DObject>()
            - std::mem::size_of::<crate::geom::Geom>());
        objs + blocks + per_obj + std::mem::size_of::<Document>()
    }

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
