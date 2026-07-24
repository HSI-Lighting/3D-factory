// Layer table — the AutoCAD/LibreCAD layer model.
//
// Every Dobject belongs to exactly one layer (its `style.layer` id). Layers
// carry the default color / linetype / lineweight that ByLayer Dobjects
// inherit, plus per-layer visibility / lock / freeze / plottable flags.
//
// Built-in id 0 is layer "0" — always present, cannot be deleted. Matches
// AutoCAD's reserved layer.

use crate::color::Color;
use crate::lineweight::Lineweight;
use crate::linetype::LinetypeTable;

pub type LayerId = u32;

#[derive(Clone, Debug)]
pub struct Layer {
    pub name:       String,
    pub color:      Color,
    pub linetype:   u32,           // LinetypeId into LinetypeTable
    pub lineweight: Lineweight,
    /// Visibility — false hides every Dobject on this layer from rendering
    /// AND from selection/snap.
    pub visible:    bool,
    /// Locked — Dobjects render and pick-highlight, but cannot be modified
    /// or selected for editing operations.
    pub locked:     bool,
    /// Frozen — like `!visible` but stronger: AutoCAD also skips layer-frozen
    /// entities during regen. Modelled here so plotting and panel UI can
    /// distinguish; functionally `visible` is what the renderer reads.
    pub frozen:     bool,
    /// Plottable — false skips this layer on plot/export.
    pub plottable:  bool,
}

impl Layer {
    /// The mandatory built-in BASE layer. Default name "LAYER B"; default
    /// color = ACI 7 (white). All freshly created Dobjects land on this
    /// layer with `Color::ByLayer`, so the default visual is white.
    ///
    /// Kept under the historical `layer_zero` name for backward compat;
    /// internally referenced via `LayerTable::LAYER_BASE` (alias for the
    /// reserved id 0). DXF round-trip preserves whatever name the
    /// imported file used at id 0.
    pub fn layer_zero() -> Self {
        Self {
            name:       "LAYER B".into(),     // Base
            color:      Color::Aci(7),         // white (ACI 7)
            linetype:   LinetypeTable::CONTINUOUS,
            lineweight: Lineweight::Default,
            visible:    true,
            locked:     false,
            frozen:     false,
            plottable:  true,
        }
    }
}

#[derive(Clone)]
pub struct LayerTable {
    pub layers: Vec<Layer>,            // index = LayerId
    /// The layer new Dobjects get assigned to. Index into `layers`.
    pub active: LayerId,
}

impl LayerTable {
    /// Reserved id of the BASE layer — always present, can't be deleted
    /// or renamed (DXF / RSM round-trip would break). Default name
    /// "LAYER B"; default color ACI 7 (white). Kept as
    /// `LAYER_ZERO` for backward compat AND aliased as `LAYER_BASE`
    /// for new code that prefers the descriptive name.
    pub const LAYER_ZERO: LayerId = 0;
    pub const LAYER_BASE: LayerId = 0;

    /// Constructed with layer "0" only. Active layer = 0.
    pub fn with_defaults() -> Self {
        Self {
            layers: vec![Layer::layer_zero()],
            active: Self::LAYER_ZERO,
        }
    }

    pub fn get(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(id as usize)
    }

    pub fn get_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.get_mut(id as usize)
    }

    pub fn add(&mut self, layer: Layer) -> LayerId {
        let id = self.layers.len() as LayerId;
        self.layers.push(layer);
        id
    }

    pub fn find(&self, name: &str) -> Option<LayerId> {
        self.layers.iter().position(|l| l.name.eq_ignore_ascii_case(name))
            .map(|i| i as LayerId)
    }

    /// Remove a layer. Returns false if `id` is 0 (cannot delete layer "0")
    /// or out of range. Callers MUST reassign any Dobjects whose
    /// `style.layer == id` before/after; this method does not touch them.
    pub fn remove(&mut self, id: LayerId) -> bool {
        if id == Self::LAYER_ZERO || (id as usize) >= self.layers.len() {
            return false;
        }
        self.layers.remove(id as usize);
        if self.active == id {
            self.active = Self::LAYER_ZERO;
        } else if self.active > id {
            self.active -= 1;
        }
        true
    }

    pub fn rename(&mut self, id: LayerId, new_name: &str) -> bool {
        if id == Self::LAYER_ZERO { return false; }   // "0" is reserved
        if self.find(new_name).is_some() { return false; }
        if let Some(l) = self.layers.get_mut(id as usize) {
            l.name = new_name.into();
            true
        } else { false }
    }

    pub fn len(&self) -> usize { self.layers.len() }
    pub fn is_empty(&self) -> bool { self.layers.is_empty() }

    /// True iff a Dobject on this layer is allowed to render. Combines
    /// visible AND not-frozen.
    pub fn renders(&self, id: LayerId) -> bool {
        self.get(id).map(|l| l.visible && !l.frozen).unwrap_or(false)
    }

    /// True iff the layer permits selection / editing.
    pub fn selectable(&self, id: LayerId) -> bool {
        self.get(id).map(|l| !l.locked && self.renders(id)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_layer_base() {
        let t = LayerTable::with_defaults();
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(0).unwrap().name, "LAYER B");
        assert_eq!(t.active, 0);
    }

    #[test]
    fn cannot_delete_layer_zero() {
        let mut t = LayerTable::with_defaults();
        assert!(!t.remove(0));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn add_and_remove_shift_active() {
        let mut t = LayerTable::with_defaults();
        let a = t.add(Layer { name: "A".into(), ..Layer::layer_zero() });
        let _b = t.add(Layer { name: "B".into(), ..Layer::layer_zero() });
        t.active = a;
        assert!(t.remove(a));
        // Active fell back to layer "0".
        assert_eq!(t.active, 0);
    }

    #[test]
    fn find_is_case_insensitive() {
        let mut t = LayerTable::with_defaults();
        t.add(Layer { name: "Walls".into(), ..Layer::layer_zero() });
        assert_eq!(t.find("WALLS"), Some(1));
    }

    #[test]
    fn rename_rejects_duplicates() {
        let mut t = LayerTable::with_defaults();
        let a = t.add(Layer { name: "A".into(), ..Layer::layer_zero() });
        t.add(Layer { name: "B".into(), ..Layer::layer_zero() });
        assert!(!t.rename(a, "B"));
        assert!(t.rename(a, "Alpha"));
    }
}
