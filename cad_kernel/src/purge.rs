//! PURGE — remove unreferenced named objects (layers, linetypes, text/dim/
//! wall styles, block definitions).
//!
//! Reference counting walks every dobject (model space + layouts + block
//! definitions). Reserved entries are always kept: layer 0, linetype
//! "Continuous" (id 0), STANDARD text/dim/wall styles (id 0). Removals run
//! from the HIGHEST id downward so index shifts never disturb the ids still
//! referenced by dobjects.

use crate::document::Document;
use crate::geom::Geom;

/// What PURGE removed (per category, by display name).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PurgeReport {
    pub layers:      Vec<String>,
    pub linetypes:   Vec<String>,
    pub text_styles: Vec<String>,
    pub dim_styles:  Vec<String>,
    pub wall_styles: Vec<String>,
    pub blocks:      Vec<String>,
}

impl PurgeReport {
    /// Total number of removed entries.
    pub fn total(&self) -> usize {
        self.layers.len() + self.linetypes.len() + self.text_styles.len()
            + self.dim_styles.len() + self.wall_styles.len() + self.blocks.len()
    }

    pub fn is_empty(&self) -> bool { self.total() == 0 }
}

/// All dobjects that can reference named objects: model space, every layout's
/// entities, and every block definition's dobjects.
fn all_dobjects(doc: &Document) -> impl Iterator<Item = &crate::dobject::DObject> {
    doc.dobjects.iter()
        .chain(doc.layouts.iter().flat_map(|l| l.entities.iter()))
        .chain(doc.blocks.blocks.iter().flat_map(|b| b.dobjects.iter()))
}

/// Purge unreferenced named objects from `doc`. Returns the report.
pub fn purge(doc: &mut Document) -> PurgeReport {
    /// The built-in linetype set (see `LinetypeTable::with_defaults`) — kept
    /// even when unreferenced, so the picker never loses standard entries.
    const STANDARD_LINETYPE_COUNT: usize = 25;

    let n_layers = doc.layers.layers.len();
    let n_lt = doc.linetypes.linetypes.len();
    let n_ts = doc.text_styles.styles.len();
    let n_ds = doc.dim_styles.styles.len();
    let n_ws = doc.wall_styles.styles.len();
    let n_blk = doc.blocks.blocks.len();

    let mut used_layers = vec![false; n_layers];
    let mut used_lt = vec![false; n_lt];
    let mut used_ts = vec![false; n_ts];
    let mut used_ds = vec![false; n_ds];
    let mut used_ws = vec![false; n_ws];
    let mut used_blk = vec![false; n_blk];

    // Layer/linetype/style refs come from STYLE; blocks from BlockRef. One
    // pass marks everything (the style ids index the same tables everywhere).
    for d in all_dobjects(doc) {
        let s = &d.style;
        if (s.layer as usize) < n_layers { used_layers[s.layer as usize] = true; }
        if (s.linetype as usize) < n_lt { used_lt[s.linetype as usize] = true; }
        match &d.geom {
            Geom::Text(t) => {
                if (t.style as usize) < n_ts { used_ts[t.style as usize] = true; }
            }
            Geom::AttrDef(a) => {
                if (a.style as usize) < n_ts { used_ts[a.style as usize] = true; }
            }
            Geom::Leader(l) => {
                if (l.label.style as usize) < n_ts { used_ts[l.label.style as usize] = true; }
            }
            Geom::Dimension(dm) => {
                if (dm.style as usize) < n_ds { used_ds[dm.style as usize] = true; }
            }
            Geom::Wall(w) => {
                if (w.style as usize) < n_ws { used_ws[w.style as usize] = true; }
            }
            Geom::BlockRef(br) => {
                if (br.block as usize) < n_blk { used_blk[br.block as usize] = true; }
            }
            _ => {}
        }
    }
    // Viewport frozen-layer overrides reference layers too.
    for vp in doc.layouts.iter().flat_map(|l| l.viewports.iter()) {
        for &f in &vp.frozen_layers {
            if (f as usize) < n_layers { used_layers[f as usize] = true; }
        }
    }

    // Blocks can nest: a kept definition may reference another block.
    let mut changed = true;
    while changed {
        changed = false;
        for (i, b) in doc.blocks.blocks.iter().enumerate() {
            if !used_blk[i] { continue; }
            for d in &b.dobjects {
                if let Geom::BlockRef(br) = &d.geom {
                    let j = br.block as usize;
                    if j < n_blk && !used_blk[j] {
                        used_blk[j] = true;
                        changed = true;
                    }
                }
            }
        }
    }

    let mut report = PurgeReport::default();

    // Collect removals per table: reserved ids (0, and the standard linetype
    // set) are never removed; unused entries are.
    let drop_layer: Vec<usize> = (1..n_layers)
        .filter(|&i| !used_layers[i]).collect();
    let drop_lt: Vec<usize> = (STANDARD_LINETYPE_COUNT..n_lt)
        .filter(|&i| !used_lt[i]).collect();
    let drop_ts: Vec<usize> = (1..n_ts).filter(|&i| !used_ts[i]).collect();
    let drop_ds: Vec<usize> = (1..n_ds).filter(|&i| !used_ds[i]).collect();
    let drop_ws: Vec<usize> = (1..n_ws).filter(|&i| !used_ws[i]).collect();
    let drop_blk: Vec<usize> = (0..n_blk).filter(|&i| !used_blk[i]).collect();

    // Removing entries shifts every id above the hole; references must be
    // remapped: new_id(i) = i − count(removed < i).
    fn remap(id: u32, removed: &[usize]) -> u32 {
        let mut n = 0;
        for &r in removed {
            if (r as u32) < id { n += 1; } else { break; }
        }
        id - n
    }
    fn fix_layer(
        doc: &mut Document,
        drop_layer: &[usize],
        drop_lt: &[usize],
        drop_ts: &[usize],
        drop_ds: &[usize],
        drop_ws: &[usize],
        drop_blk: &[usize],
    ) {
        // Iterate every dobject BY MUTABLE PATH: model space, layouts,
        // block definitions.
        for d in doc.dobjects.iter_mut()
            .chain(doc.layouts.iter_mut().flat_map(|l| l.entities.iter_mut()))
            .chain(doc.blocks.blocks.iter_mut().flat_map(|b| b.dobjects.iter_mut()))
        {
            d.style.layer = remap(d.style.layer, drop_layer);
            d.style.linetype = remap(d.style.linetype, drop_lt);
            match &mut d.geom {
                Geom::Text(t) => t.style = remap(t.style, &drop_ts),
                Geom::AttrDef(a) => a.style = remap(a.style, &drop_ts),
                Geom::Leader(l) => l.label.style = remap(l.label.style, &drop_ts),
                Geom::Dimension(dm) => dm.style = remap(dm.style, &drop_ds),
                Geom::Wall(w) => w.style = remap(w.style, &drop_ws),
                Geom::BlockRef(br) => br.block = remap(br.block, drop_blk),
                _ => {}
            }
        }
        for vp in doc.layouts.iter_mut().flat_map(|l| l.viewports.iter_mut()) {
            for f in vp.frozen_layers.iter_mut() {
                *f = remap(*f, drop_layer);
            }
        }
    }

    // Remap FIRST (ids are still valid while the tables are intact), then
    // remove entries highest-first so in-table indices stay aligned.
    fix_layer(doc, &drop_layer, &drop_lt, &drop_ts, &drop_ds, &drop_ws, &drop_blk);
    for &i in drop_layer.iter().rev() {
        if let Some(l) = doc.layers.layers.get(i) {
            report.layers.push(l.name.clone());
        }
        doc.layers.layers.remove(i);
    }
    // `active` must follow the same remap (remove() would also do this, but
    // we removed directly above).
    doc.layers.active = remap(doc.layers.active, &drop_layer);
    for &i in drop_lt.iter().rev() {
        if let Some(l) = doc.linetypes.linetypes.get(i) {
            report.linetypes.push(l.name.clone());
        }
        doc.linetypes.linetypes.remove(i);
    }
    for &i in drop_ts.iter().rev() {
        if let Some(s) = doc.text_styles.styles.get(i) {
            report.text_styles.push(s.name.clone());
        }
        doc.text_styles.styles.remove(i);
    }
    for &i in drop_ds.iter().rev() {
        if let Some(s) = doc.dim_styles.styles.get(i) {
            report.dim_styles.push(s.name.clone());
        }
        doc.dim_styles.styles.remove(i);
    }
    for &i in drop_ws.iter().rev() {
        if let Some(s) = doc.wall_styles.styles.get(i) {
            report.wall_styles.push(s.name.clone());
        }
        doc.wall_styles.styles.remove(i);
    }
    for &i in drop_blk.iter().rev() {
        if let Some(b) = doc.blocks.blocks.get(i) {
            report.blocks.push(b.name.clone());
        }
        doc.blocks.blocks.remove(i);
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dobject::DObject;
    use crate::geom::{Circle, Line};
    use crate::math::Vec2;
    use crate::style::Style;

    #[test]
    fn removes_unreferenced_layer_and_block() {
        let mut doc = Document::default();
        // Extra unused layer + used layer.
        let unused = doc.layers.add(crate::layer::Layer { name: "UnusedLayer".into(), color: crate::color::Color::Aci(7),
            linetype: 0, lineweight: crate::lineweight::Lineweight::Default,
            visible: true, locked: false, frozen: false, plottable: true, order: 0 });
        let used = doc.layers.add(crate::layer::Layer { name: "UsedLayer".into(), color: crate::color::Color::Aci(7),
            linetype: 0, lineweight: crate::lineweight::Lineweight::Default,
            visible: true, locked: false, frozen: false, plottable: true, order: 0 });
        // Unused + used blocks.
        doc.blocks.blocks.push(crate::block::Block {
            name: "UnusedBlock".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let used_block = doc.blocks.blocks.len() as u32;
        doc.blocks.blocks.push(crate::block::Block {
            name: "UsedBlock".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        // A dobject on `used` + a block ref into UsedBlock.
        let mut d = DObject::new(Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(1.0, 0.0) }));
        d.style.layer = used;
        doc.push(d);
        let mut br = DObject::new(Geom::BlockRef(crate::block::BlockRef {
            block: used_block,
            insert: Vec2::new(5.0, 5.0),
            scale: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            mirror_x: false,
            attr_values: Vec::new(),
            param_values: [0.0; 8],
        }));
        br.style.layer = used;
        doc.push(br);

        let r = purge(&mut doc);
        assert!(r.layers.iter().any(|n| n == "UnusedLayer"));
        assert!(r.blocks.iter().any(|n| n == "UnusedBlock"));
        assert!(r.layers.iter().all(|n| n != "UsedLayer"));
        assert!(r.blocks.iter().all(|n| n != "UsedBlock"));
        // Used layer survives (id may shift — references were remapped).
        let kept = doc.layers.find("UsedLayer").expect("used layer kept");
        // The dobject's layer reference still resolves to it.
        let d = doc.dobjects.iter().find(|d| matches!(d.geom, Geom::Line(_))).unwrap();
        assert_eq!(d.style.layer, kept, "reference remapped");
        // Used block still resolves (by name — its id may have shifted).
        assert!(doc.blocks.blocks.iter().any(|b| b.name == "UsedBlock"));
        // The BlockRef was remapped to the surviving id.
        let br = doc.dobjects.iter().find(|d| matches!(d.geom, Geom::BlockRef(_))).unwrap();
        if let Geom::BlockRef(b) = &br.geom {
            assert_eq!(doc.blocks.get(b.block).map(|x| x.name.as_str()), Some("UsedBlock"));
        } else { unreachable!(); }
        // Layer 0 stays.
        assert!(doc.layers.get(0).is_some());
    }

    #[test]
    fn keeps_reserved_styles_and_continuous() {
        let mut doc = Document::default();
        let unused_ts = doc.text_styles.add(crate::text::TextStyle {
            name: "UnusedStyle".into(), font_name: String::new(),
            width_factor: 1.0, oblique: 0.0, default_height: 0.0,
            bold: false, underline: false, outline_only: false,
            outline_width: 0.0,
        });
        assert!(unused_ts > 0);
        let r = purge(&mut doc);
        assert!(r.text_styles.iter().any(|n| n == "UnusedStyle"));
        assert!(doc.text_styles.get(0).is_some(), "STANDARD kept");
        assert!(doc.linetypes.get(0).is_some(), "Continuous kept");
    }

    #[test]
    fn used_nested_block_keeps_inner_definition() {
        let mut doc = Document::default();
        // Inner block referenced ONLY by the outer definition.
        doc.blocks.blocks.push(crate::block::Block {
            name: "Inner".into(), base: Vec2::ZERO,
            dobjects: vec![DObject::new(Geom::Circle(Circle {
                center: Vec2::ZERO, radius: 1.0 }))],
            smart: false, params: Vec::new(), cut_edges: Vec::new(),
        });
        let inner = 0u32;
        let outer_id = doc.blocks.blocks.len() as u32;
        doc.blocks.blocks.push(crate::block::Block {
            name: "Outer".into(), base: Vec2::ZERO,
            dobjects: vec![DObject::new(Geom::BlockRef(crate::block::BlockRef {
                block: inner, insert: Vec2::ZERO, scale: 1.0, scale_y: 1.0,
                rotation: 0.0, mirror_x: false,
                attr_values: Vec::new(), param_values: [0.0; 8],
            }))],
            smart: false, params: Vec::new(), cut_edges: Vec::new(),
        });
        // An instance of Outer in model space.
        doc.push(DObject::new(Geom::BlockRef(crate::block::BlockRef {
            block: outer_id, insert: Vec2::ZERO, scale: 1.0, scale_y: 1.0,
            rotation: 0.0, mirror_x: false,
            attr_values: Vec::new(), param_values: [0.0; 8],
        })));
        let r = purge(&mut doc);
        assert!(r.blocks.is_empty(), "both blocks are referenced: {r:?}");
        assert!(doc.blocks.get(inner).is_some() && doc.blocks.get(outer_id).is_some());
    }

    #[test]
    fn empty_doc_purges_nothing_but_removes_nothing() {
        let mut doc = Document::default();
        let r = purge(&mut doc);
        assert!(r.is_empty(), "standard linetypes + reserved ids stay: {r:?}");
        assert_eq!(doc.layers.layers.len(), 1);
        assert!(doc.linetypes.linetypes.len() >= 25, "standard set kept");
    }

    #[test]
    fn unused_style_removal_keeps_dobject_ids_valid() {
        // Style removal must not break ids of STYLES still in use (they
        // reference by index): remove from high to low.
        let mut doc = Document::default();
        let mk = |name: &str| crate::text::TextStyle {
            name: name.into(), font_name: String::new(),
            width_factor: 1.0, oblique: 0.0, default_height: 0.0,
            bold: false, underline: false, outline_only: false,
            outline_width: 0.0,
        };
        let s1 = doc.text_styles.add(mk("Keep"));
        let s2 = doc.text_styles.add(mk("Drop"));
        // A Text using s1.
        let t = crate::text::Text {
            position: Vec2::ZERO, height: 1.0, angle: 0.0,
            text: "hi".into(), h_align: crate::text::HAlign::Left,
            v_align: crate::text::VAlign::Baseline, style: s1,
            ..crate::text::Text::empty()
        };
        doc.push(DObject::new(Geom::Text(t)));
        let r = purge(&mut doc);
        assert!(r.text_styles.iter().any(|n| n == "Drop"));
        assert!(doc.text_styles.get(s1).is_some(), "used style kept at its id");
        assert_eq!(doc.text_styles.get(s1).map(|s| s.name.as_str()), Some("Keep"));
    }
}
