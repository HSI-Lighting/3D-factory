//! Wall styles — the "type" of a wall (Dry Wall, Structural, …), analog of
//! `DimStyle` for dimensions. A `Wall` references a `WallStyle` by id; the
//! style supplies the default thickness, the poché fill, and the face color.
//! Editing a style re-derives every wall of that type (smart-dobject payoff).
//!
//! Mirrors `DimStyleTable`: STANDARD lives at id 0.

/// A named wall type.
#[derive(Clone, Debug, PartialEq)]
pub struct WallStyle {
    pub name:        String,
    /// Default centerline-to-face thickness (full width).
    pub thickness:   f64,
    /// Poché fill color (AutoCAD Color Index). 0 = no fill (hollow wall).
    /// A solid tint for now; true hatch patterns are a follow-up.
    pub fill_color:  u32,
    /// Face-line color (ACI). 0 = ByLayer/ByBlock (use the dobject color).
    pub face_color:  u32,
    /// Draw a batt-INSULATION symbol (sine wave) in the cavity — the
    /// architectural insulation-layer wall. Amplitude auto-fits the thickness.
    /// NOT persisted to RSM yet (reader defaults it false, like the table).
    pub insulation:  bool,
    /// Free-text note shown in the Wall Style Manager.
    pub description: String,
}

impl WallStyle {
    /// The built-in STANDARD style (always id 0).
    pub fn standard() -> Self {
        Self {
            name:        "STANDARD".into(),
            thickness:   0.2,
            fill_color:  0,
            face_color:  0,
            insulation:  false,
            description: String::new(),
        }
    }
}

/// Table of wall styles — STANDARD at id 0. Analog of `DimStyleTable`.
#[derive(Clone, Debug)]
pub struct WallStyleTable {
    pub styles: Vec<WallStyle>,
}

impl WallStyleTable {
    pub const STANDARD: u32 = 0;

    pub fn with_defaults() -> Self {
        Self { styles: vec![WallStyle::standard()] }
    }
    pub fn get(&self, id: u32) -> Option<&WallStyle> {
        self.styles.get(id as usize)
    }
    pub fn add(&mut self, s: WallStyle) -> u32 {
        let id = self.styles.len() as u32;
        self.styles.push(s);
        id
    }
    pub fn find(&self, name: &str) -> Option<u32> {
        self.styles.iter().position(|s| s.name.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }
    pub fn len(&self) -> usize { self.styles.len() }
    pub fn is_empty(&self) -> bool { self.styles.is_empty() }
}

impl Default for WallStyleTable {
    fn default() -> Self { Self::with_defaults() }
}
