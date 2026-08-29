//! LAYERSTATE — named snapshots of layer properties, restorable later
//! (AutoCAD LAYERSTATE).
//!
//! Snapshots are keyed by LAYER NAME (ids shift when layers are deleted), so
//! a state survives layer reorder / purge. Restore re-applies visible/frozen/
//! locked/color/lineweight/linetype to layers that exist and CREATES missing
//! layers (with the saved color) so a state always restores fully.

use crate::color::Color;
use crate::document::Document;
use crate::lineweight::Lineweight;

/// One layer's saved properties (by name — ids are not stable).
#[derive(Clone, Debug, PartialEq)]
pub struct LayerStateEntry {
    pub layer:     String,
    pub visible:   bool,
    pub frozen:    bool,
    pub locked:    bool,
    pub color:     Color,
    pub lineweight: Lineweight,
    /// Linetype by NAME (ids shift too).
    pub linetype:  String,
}

/// A named layer-state snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerState {
    pub name:    String,
    pub entries: Vec<LayerStateEntry>,
}

/// Save the CURRENT layer table as the named state. `Err` when the name
/// already exists (AutoCAD asks before overwrite — the app decides).
pub fn save(doc: &mut Document, name: &str) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("layerstate: name is empty");
    }
    if doc.layer_states.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
        return Err("layerstate: that name already exists (delete it first)");
    }
    let mut entries = Vec::with_capacity(doc.layers.layers.len());
    for l in &doc.layers.layers {
        entries.push(LayerStateEntry {
            layer:      l.name.clone(),
            visible:    l.visible,
            frozen:     l.frozen,
            locked:     l.locked,
            color:      l.color,
            lineweight: l.lineweight,
            linetype:   doc.linetypes.get(l.linetype)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "Continuous".into()),
        });
    }
    doc.layer_states.push(LayerState { name: name.to_string(), entries });
    Ok(())
}

/// Restore a named state. Layers missing from the drawing are created (with
/// the saved color + visibility); saved linetype names resolve when present.
/// Returns false when no such state exists.
pub fn restore(doc: &mut Document, name: &str) -> bool {
    let Some(state) = doc.layer_states.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else {
        return false;
    };
    let st = state.clone();
    for e in &st.entries {
        let id = match doc.layers.find(&e.layer) {
            Some(id) => id,
            None => {
                let id = doc.layers.add(crate::layer::Layer {
                    name: e.layer.clone(),
                    color: e.color,
                    linetype: doc.linetypes.find(&e.linetype).unwrap_or(0),
                    lineweight: e.lineweight,
                    visible: e.visible,
                    locked: e.locked,
                    frozen: e.frozen,
                    plottable: true,
                    order: 0,
                });
                id
            }
        };
        if let Some(l) = doc.layers.get_mut(id) {
            l.visible = e.visible;
            l.frozen = e.frozen;
            l.locked = e.locked;
            l.color = e.color;
            l.lineweight = e.lineweight;
            if let Some(lt) = doc.linetypes.find(&e.linetype) {
                l.linetype = lt;
            }
        }
    }
    true
}

/// Delete a named state. False when it doesn't exist.
pub fn delete(doc: &mut Document, name: &str) -> bool {
    let before = doc.layer_states.len();
    doc.layer_states.retain(|s| !s.name.eq_ignore_ascii_case(name));
    doc.layer_states.len() != before
}

/// Rename a state. False when the source doesn't exist or the target is taken.
pub fn rename(doc: &mut Document, from: &str, to: &str) -> bool {
    let to = to.trim();
    if to.is_empty() { return false; }
    if doc.layer_states.iter().any(|s| s.name.eq_ignore_ascii_case(to)) {
        return false;
    }
    let Some(st) = doc.layer_states.iter_mut()
        .find(|s| s.name.eq_ignore_ascii_case(from)) else { return false };
    st.name = to.to_string();
    true
}

/// Names of all saved states (sorted, original case).
pub fn names(doc: &Document) -> Vec<String> {
    let mut v: Vec<String> = doc.layer_states.iter().map(|s| s.name.clone()).collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Layer;

    fn mk_layer(name: &str, color: Color) -> Layer {
        Layer {
            name: name.into(), color,
            linetype: 0, lineweight: Lineweight::Default,
            visible: true, locked: false, frozen: false, plottable: true, order: 0,
        }
    }

    #[test]
    fn save_restore_round_trip() {
        let mut doc = Document::default();
        doc.layers.add(mk_layer("Walls", Color::Aci(1)));
        doc.layers.add(mk_layer("Furn", Color::Aci(3)));
        if let Some(l) = doc.layers.get_mut(1) {
            l.visible = false;
            l.locked = true;
        }
        save(&mut doc, "MyState").unwrap();
        // Mutate the layers, then restore.
        if let Some(l) = doc.layers.get_mut(1) {
            l.visible = true;
            l.locked = false;
            l.color = Color::Aci(7);
        }
        assert!(restore(&mut doc, "mystate"));   // case-insensitive
        let l = doc.layers.get(1).unwrap();
        assert!(!l.visible, "visibility restored");
        assert!(l.locked, "locked restored");
        assert_eq!(l.color, Color::Aci(1), "color restored");
    }

    #[test]
    fn save_rejects_duplicate_names() {
        let mut doc = Document::default();
        save(&mut doc, "S1").unwrap();
        assert!(save(&mut doc, "s1").is_err());
    }

    #[test]
    fn restore_creates_missing_layers() {
        let mut doc = Document::default();
        doc.layers.add(mk_layer("Ghost", Color::Aci(5)));
        save(&mut doc, "S1").unwrap();
        // Delete the layer entirely (leaving its state behind).
        doc.layers.layers.retain(|l| l.name != "Ghost");
        assert!(restore(&mut doc, "S1"));
        assert!(doc.layers.find("Ghost").is_some(), "layer recreated");
    }

    #[test]
    fn delete_and_rename() {
        let mut doc = Document::default();
        save(&mut doc, "A").unwrap();
        save(&mut doc, "B").unwrap();
        assert!(delete(&mut doc, "a"));
        assert!(!delete(&mut doc, "a"), "already gone");
        assert_eq!(names(&doc), vec!["B".to_string()]);
        assert!(rename(&mut doc, "B", "C"));
        assert_eq!(names(&doc), vec!["C".to_string()]);
        assert!(!rename(&mut doc, "C", "C"), "target taken");
        assert!(!rename(&mut doc, "Nope", "X"));
    }

    #[test]
    fn restore_unknown_returns_false() {
        let mut doc = Document::default();
        assert!(!restore(&mut doc, "nope"));
    }
}
