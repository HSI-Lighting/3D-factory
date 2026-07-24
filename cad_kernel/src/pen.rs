// Pen presets — named bundles of (color, linetype, lineweight).
//
// Equivalent of LibreCAD's pen palette. A pen is a *style snapshot* the user
// can apply to one or many Dobjects in a single click: "make all selected
// objects RED + DASHED + 0.5 mm" without setting three properties separately.
//
// Pens do NOT live on Dobjects — they're a UI convenience. Applying a pen
// rewrites the Dobject's `Style` fields with the pen's values. The pen
// itself never appears in DXF or the on-disk format.

use crate::color::Color;
use crate::lineweight::Lineweight;
use crate::linetype::LinetypeTable;

#[derive(Clone, Debug)]
pub struct Pen {
    pub name:       String,
    pub color:      Color,
    pub linetype:   u32,        // LinetypeId
    pub lineweight: Lineweight,
}

#[derive(Clone)]
pub struct PenTable {
    pub pens: Vec<Pen>,
}

impl Default for PenTable {
    /// Starter set covering the most-used pen presets a CAD user keeps
    /// at the ready: ByLayer (the no-op pen), three pure ACI colors with
    /// continuous lines, plus two dashed-line presets.
    fn default() -> Self {
        // Default pens use ACI so they cost ZERO entries in the
        // truecolor table. User can switch any pen to a TrueColor via
        // the Pen panel (which interns via doc.truecolors). Matches
        // memo `feedback_rust_cad_color_aci_primary`: ACI is the
        // primary picker.
        Self {
            pens: vec![
                Pen { name: "ByLayer".into(),         color: Color::ByLayer,   linetype: LinetypeTable::CONTINUOUS, lineweight: Lineweight::ByLayer },
                Pen { name: "Red 0.25 mm".into(),     color: Color::Aci(1),    linetype: LinetypeTable::CONTINUOUS, lineweight: Lineweight::Custom(0.25) },
                Pen { name: "Green 0.25 mm".into(),   color: Color::Aci(3),    linetype: LinetypeTable::CONTINUOUS, lineweight: Lineweight::Custom(0.25) },
                Pen { name: "Blue 0.25 mm".into(),    color: Color::Aci(5),    linetype: LinetypeTable::CONTINUOUS, lineweight: Lineweight::Custom(0.25) },
                Pen { name: "Heavy black 0.7".into(), color: Color::Aci(250),  linetype: LinetypeTable::CONTINUOUS, lineweight: Lineweight::Custom(0.7) },
                Pen { name: "Dashed gray".into(),     color: Color::Aci(8),    linetype: 1, lineweight: Lineweight::Default },
                Pen { name: "Dash-dot center".into(), color: Color::Aci(40),   linetype: 2, lineweight: Lineweight::Default },
            ],
        }
    }
}

impl PenTable {
    pub fn get(&self, i: usize) -> Option<&Pen> { self.pens.get(i) }
    pub fn len(&self) -> usize { self.pens.len() }
    pub fn is_empty(&self) -> bool { self.pens.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_bylayer_first() {
        let t = PenTable::default();
        assert_eq!(t.get(0).unwrap().name, "ByLayer");
        matches!(t.get(0).unwrap().color, Color::ByLayer);
    }

    #[test]
    fn defaults_cover_basic_colors() {
        let t = PenTable::default();
        let names: Vec<&str> = t.pens.iter().map(|p| p.name.as_str()).collect();
        assert!(names.iter().any(|n| n.starts_with("Red")));
        assert!(names.iter().any(|n| n.starts_with("Green")));
        assert!(names.iter().any(|n| n.starts_with("Blue")));
    }
}
