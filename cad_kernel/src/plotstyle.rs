// Plot-style table — AutoCAD "Color-Dependent Plot Style Table" (CTB) analog.
//
// A table is indexed by ACI (1..=255) — matching AutoCAD's CTB and our
// ACI-primary color picker. Each color carries the FULL set of 12 CTB
// properties (Form-View order), so the table is a faithful CTB and can
// import/export `.ctb` later. The plot pipeline HONOURS a subset (see the
// honour matrix in `PRINT_PLOT_MENTOR.md §1a`) and STORES the rest for fidelity.
//
// This module is PURE DATA + a width resolver + serde. No rendering, no dialog.
// It reuses the kernel's `Color` / `Lineweight` / `resolve_lineweight`: a plot
// pen's `lineweight = Fixed(mm)` is a per-color OVERRIDE that composes on top of
// the normal lineweight chain; `UseObject` falls through to `resolve_lineweight`.
//
// Spec: `kernel mentor MD/PRINT_PLOT_MENTOR.md` §1 (data model), §1a (honour
// matrix), §2 (plot config).

use crate::layer::LayerTable;
use crate::lineweight::{resolve_lineweight, Lineweight};
use crate::math::Vec2;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ===================================================================
// §1 — per-color plot style (the 12 CTB properties)
// ===================================================================

/// Stroke WIDTH pen for a color. `Fixed` = physical mm on paper (scale
/// independent); `0.00` = thinnest renderable hairline.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlotWidth {
    UseObject,
    Fixed(f32),
}
impl Default for PlotWidth {
    fn default() -> Self { PlotWidth::UseObject }
}

/// Plotted COLOR for a color. `UseObject` = the entity's own color; `Black`;
/// `Aci` = a palette index; `Rgb` = a direct truecolor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlotColor {
    UseObject,
    Black,
    Aci(u8),
    Rgb(u8, u8, u8),
}
impl Default for PlotColor {
    fn default() -> Self { PlotColor::UseObject }
}

/// Plotted LINETYPE for a color: keep the object's, or force a table linetype id.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlotLinetype {
    UseObject,
    Id(u32),
}
impl Default for PlotLinetype {
    fn default() -> Self { PlotLinetype::UseObject }
}

/// Legacy pen-plotter pen number (physical or virtual). Stored for CTB fidelity;
/// no effect on a vector PDF.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PenNum {
    Automatic,
    N(u16),
}
impl Default for PenNum {
    fn default() -> Self { PenNum::Automatic }
}

/// Line END-cap style. `UseObject` keeps the renderer default.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EndStyle {
    UseObject,
    Butt,
    Square,
    Round,
    Diamond,
}
impl Default for EndStyle {
    fn default() -> Self { EndStyle::UseObject }
}

/// Line JOIN style.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum JoinStyle {
    UseObject,
    Miter,
    Bevel,
    Round,
    Diamond,
}
impl Default for JoinStyle {
    fn default() -> Self { JoinStyle::UseObject }
}

/// FILL style for wide/filled entities. MVP effective = Solid; the rest store.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FillStyle {
    UseObject,
    Solid,
    Checkerboard,
    Crosshatch,
    Diamonds,
    HorizontalBars,
    SlantLeft,
    SlantRight,
    SquareDots,
    VerticalBars,
}
impl Default for FillStyle {
    fn default() -> Self { FillStyle::UseObject }
}

/// One color's full plot style — the 12 CTB properties, Form-View order.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlotStyle {
    pub plot_color:  PlotColor,      // ✅ honoured
    pub dither:      bool,           // ◻ stored (no PDF effect)
    pub grayscale:   bool,           // ✅ honoured
    pub pen_number:  PenNum,         // ◻ stored (legacy)
    pub virtual_pen: PenNum,         // ◻ stored (legacy)
    pub screening:   u8,             // ✅ honoured (0..=100 % ink)
    pub linetype:    PlotLinetype,   // ✅ honoured
    pub adaptive:    bool,           // ◻ stored
    pub lineweight:  PlotWidth,      // ✅ honoured — THE pen width
    pub end_style:   EndStyle,       // ✅ honoured (PDF line cap)
    pub join_style:  JoinStyle,      // ✅ honoured (PDF line join)
    pub fill_style:  FillStyle,      // ◻ stored (MVP: Solid)
}

impl Default for PlotStyle {
    /// A neutral pen: honour the object for everything, full ink, pens automatic,
    /// adaptive on. This is the "no pen assigned yet" state.
    fn default() -> Self {
        Self {
            plot_color:  PlotColor::UseObject,
            dither:      false,
            grayscale:   false,
            pen_number:  PenNum::Automatic,
            virtual_pen: PenNum::Automatic,
            screening:   100,
            linetype:    PlotLinetype::UseObject,
            adaptive:    true,
            lineweight:  PlotWidth::UseObject,
            end_style:   EndStyle::UseObject,
            join_style:  JoinStyle::UseObject,
            fill_style:  FillStyle::UseObject,
        }
    }
}

/// AutoCAD's standard lineweight ladder (mm). The editable default set offered by
/// every Lineweight dropdown; a shop can customise it via "Edit Lineweights…".
/// `0.00` = thinnest renderable hairline; a `UseObject` sentinel is added by the UI.
pub const AUTOCAD_LADDER: [f32; 23] = [
    0.00, 0.05, 0.09, 0.13, 0.15, 0.18, 0.20, 0.25, 0.30, 0.35, 0.40, 0.50,
    0.53, 0.60, 0.70, 0.80, 0.90, 1.00, 1.06, 1.20, 1.40, 2.00, 2.11,
];

/// A full color→pen table (255 usable colors) plus the General-tab metadata and
/// the customisable lineweight ladder.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotStyleTable {
    pub name:                 String,
    pub description:          String,
    pub apply_global_ltscale: bool,
    pub ltscale_percent:      f32,
    pub lineweight_ladder:    Vec<f32>,
    /// Index = ACI 1..=255; index 0 is the DXF ByBlock sentinel (unused by the
    /// resolver, kept so `by_aci[aci]` is always in-bounds for a valid ACI).
    pub by_aci:               Box<[PlotStyle; 256]>,
}

impl Default for PlotStyleTable {
    /// The default table: every color `PlotStyle::default()` (all `UseObject`),
    /// ladder = `AUTOCAD_LADDER`, global LT-scale off at 100%.
    fn default() -> Self {
        Self {
            name:                 "Default".into(),
            description:          String::new(),
            apply_global_ltscale: false,
            ltscale_percent:      100.0,
            lineweight_ladder:    AUTOCAD_LADDER.to_vec(),
            by_aci:               Box::new(std::array::from_fn(|_| PlotStyle::default())),
        }
    }
}

impl PlotStyleTable {
    /// A fresh default table with a given name.
    pub fn named(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    /// The pen for an ACI color.
    pub fn style(&self, aci: u8) -> &PlotStyle {
        &self.by_aci[aci as usize]
    }

    /// Mutable pen for an ACI color (the editor writes here).
    pub fn style_mut(&mut self, aci: u8) -> &mut PlotStyle {
        &mut self.by_aci[aci as usize]
    }

    /// Convenience: set a fixed print width (mm) for one ACI color.
    pub fn set_fixed_width(&mut self, aci: u8, mm: f32) {
        self.by_aci[aci as usize].lineweight = PlotWidth::Fixed(mm);
    }

    /// Convenience: revert one ACI color to honour the object lineweight.
    pub fn set_use_object(&mut self, aci: u8) {
        self.by_aci[aci as usize].lineweight = PlotWidth::UseObject;
    }

    /// Apply this table's per-ACI colour rules to an entity's resolved RGB:
    /// plot_color override, then grayscale, then screening (the §1a subset the
    /// plot pipeline honours). `aci` = None (truecolor entity) leaves the
    /// colour unchanged.
    pub fn apply_color(&self, aci: Option<u8>, rgb0: (u8, u8, u8)) -> (u8, u8, u8) {
        let mut rgb = rgb0;
        let mut style_gray = false;
        let mut screening = 100u8;
        if let Some(a) = aci {
            let st = self.style(a);
            rgb = match st.plot_color {
                PlotColor::UseObject => rgb0,
                PlotColor::Black => (0, 0, 0),
                PlotColor::Aci(i) => crate::color::aci_palette(i),
                PlotColor::Rgb(r, g, b) => (r, g, b),
            };
            style_gray = st.grayscale;
            screening = st.screening;
        }
        if style_gray {
            let l = (0.299 * rgb.0 as f32 + 0.587 * rgb.1 as f32 + 0.114 * rgb.2 as f32)
                .round()
                .clamp(0.0, 255.0) as u8;
            rgb = (l, l, l);
        }
        if screening < 100 {
            let t = screening as f32 / 100.0;
            let mix = |c: u8| (255.0 - (255.0 - c as f32) * t).round().clamp(0.0, 255.0) as u8;
            rgb = (mix(rgb.0), mix(rgb.1), mix(rgb.2));
        }
        rgb
    }
}

// serde for the table — the `[PlotStyle; 256]` array isn't serde-derivable, so
// map through a DTO carrying a `Vec<PlotStyle>` and rebuild the array on load
// (missing/extra entries pad/truncate to the default, so old files load clean).
#[derive(Serialize, Deserialize)]
struct PlotStyleTableDto {
    name:                 String,
    #[serde(default)]
    description:          String,
    #[serde(default)]
    apply_global_ltscale: bool,
    #[serde(default = "default_ltscale_percent")]
    ltscale_percent:      f32,
    #[serde(default = "default_ladder")]
    lineweight_ladder:    Vec<f32>,
    by_aci:               Vec<PlotStyle>,
}
fn default_ltscale_percent() -> f32 { 100.0 }
fn default_ladder() -> Vec<f32> { AUTOCAD_LADDER.to_vec() }

impl Serialize for PlotStyleTable {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let dto = PlotStyleTableDto {
            name:                 self.name.clone(),
            description:          self.description.clone(),
            apply_global_ltscale: self.apply_global_ltscale,
            ltscale_percent:      self.ltscale_percent,
            lineweight_ladder:    self.lineweight_ladder.clone(),
            by_aci:               self.by_aci.to_vec(),
        };
        dto.serialize(s)
    }
}

impl<'de> Deserialize<'de> for PlotStyleTable {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let dto = PlotStyleTableDto::deserialize(d)?;
        let mut by_aci: Box<[PlotStyle; 256]> =
            Box::new(std::array::from_fn(|_| PlotStyle::default()));
        for (i, st) in dto.by_aci.into_iter().take(256).enumerate() {
            by_aci[i] = st;
        }
        let ladder = if dto.lineweight_ladder.is_empty() {
            AUTOCAD_LADDER.to_vec()
        } else {
            dto.lineweight_ladder
        };
        Ok(PlotStyleTable {
            name:                 dto.name,
            description:          dto.description,
            apply_global_ltscale: dto.apply_global_ltscale,
            ltscale_percent:      dto.ltscale_percent,
            lineweight_ladder:    ladder,
            by_aci,
        })
    }
}

// ===================================================================
// The core resolver — per-color print width in physical mm.
// ===================================================================

/// Resolve the print WIDTH (mm on paper) for an entity of ACI color `aci`.
///
/// - `Fixed(w)` on the color → `w` (the per-color pen — the owner's ask).
/// - `UseObject` → the object's own resolved lineweight via `resolve_lineweight`.
///
/// The returned value is PHYSICAL mm; the pipeline must NOT scale it by the plot
/// scale (0.25 mm prints 0.25 mm at 1:1 and 1:100). `cfg.lw_scale` is applied by
/// the caller, not here.
pub fn plot_width_mm(
    table:     &PlotStyleTable,
    aci:       u8,
    entity_lw: Lineweight,
    layer:     u32,
    layers:    &LayerTable,
) -> f32 {
    match table.style(aci).lineweight {
        PlotWidth::Fixed(w)  => w,
        PlotWidth::UseObject => resolve_lineweight(entity_lw, layer, layers),
    }
}

// ===================================================================
// §2 — plot configuration (paper, area, scale, output)
// ===================================================================

/// Where the plot goes. MVP implements `PdfFile`; `SystemPrinter` is P2.
#[derive(Clone, Debug, PartialEq)]
pub enum PlotTarget {
    PdfFile(PathBuf),
    SystemPrinter(String),
}

/// Standard paper sizes (mm). `dims_mm` returns the PORTRAIT (w, h).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaperSize {
    A4, A3, A2, A1, A0, Letter,
    Custom { w_mm: f32, h_mm: f32 },
}

impl PaperSize {
    /// Portrait width × height in millimetres.
    pub fn dims_mm(self) -> (f32, f32) {
        match self {
            PaperSize::A4     => (210.0, 297.0),
            PaperSize::A3     => (297.0, 420.0),
            PaperSize::A2     => (420.0, 594.0),
            PaperSize::A1     => (594.0, 841.0),
            PaperSize::A0     => (841.0, 1189.0),
            PaperSize::Letter => (216.0, 279.0),
            PaperSize::Custom { w_mm, h_mm } => (w_mm, h_mm),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Orientation { Portrait, Landscape }

/// Which part of the drawing to plot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlotArea {
    Extents,
    Display,
    Window { min: Vec2, max: Vec2 },
}

/// Plot scale. `Fit` sizes the drawing to the printable area; `Ratio` is a fixed
/// drawing scale (`model` model-units drawn as `paper_mm` on paper → 1:N).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlotScale {
    Fit,
    Ratio { model: f64, paper_mm: f64 },
}

/// How the plotted image is positioned within the printable area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Offset {
    Center,
    Xy { x_mm: f32, y_mm: f32 },
}

/// The full plot job configuration. The `PlotStyleTable` travels on the Document
/// (per-doc) and is passed to `plot()` separately — not owned here.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotConfig {
    pub output:      PlotTarget,
    pub paper:       PaperSize,
    pub orientation: Orientation,
    pub area:        PlotArea,
    pub scale:       PlotScale,
    pub offset:      Offset,
    /// Global thickness multiplier (default 1.0). Deliberate — distinct from the
    /// plot scale, which must NOT affect physical lineweight.
    pub lw_scale:    f32,
    pub monochrome:  bool,
    /// Printable inset from each paper edge (mm). Default ~5.
    pub margins_mm:  f32,
    /// When `Some(i)`, plot the LAYOUT at index `i` instead of model space:
    /// 1:1 paper-mm — paper border + paper-space entities + every viewport's
    /// model content through its own camera, with the layout's and each
    /// viewport's CTB applied. The layout's OWN page size/orientation are used;
    /// `paper`/`orientation`/`area`/`scale`/`offset`/`margins_mm` above are
    /// ignored for layout plots (paper-space coords map 1:1, matching the
    /// layout tab). Restored from commit e0fddd1 (the layout plot path).
    pub plot_layout_index: Option<usize>,
    /// Resolved saved-CTB tables by name (the caller loads them from the app's
    /// CTB folder). Layout plots look these up so a saved CTB's per-ACI colour
    /// rules apply; names absent here fall back to the built-in name-based
    /// transforms (monochrome / grayscale / full color).
    pub ctb_tables: std::collections::BTreeMap<String, PlotStyleTable>,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            output:      PlotTarget::PdfFile(PathBuf::new()),
            paper:       PaperSize::A3,
            orientation: Orientation::Landscape,
            area:        PlotArea::Extents,
            scale:       PlotScale::Fit,
            offset:      Offset::Center,
            lw_scale:    1.0,
            monochrome:  false,
            margins_mm:  5.0,
            plot_layout_index: None,
            ctb_tables:  std::collections::BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::layer::{Layer, LayerTable};
    use crate::lineweight::{Lineweight, DEFAULT_LINEWEIGHT_MM};

    #[test]
    fn default_table_is_all_use_object() {
        let t = PlotStyleTable::default();
        assert_eq!(t.name, "Default");
        assert_eq!(t.lineweight_ladder, AUTOCAD_LADDER.to_vec());
        assert!(!t.apply_global_ltscale);
        assert_eq!(t.ltscale_percent, 100.0);
        for aci in 0..=255u16 {
            let s = t.style(aci as u8);
            assert_eq!(s.lineweight, PlotWidth::UseObject);
            assert_eq!(s.plot_color, PlotColor::UseObject);
            assert_eq!(s.linetype, PlotLinetype::UseObject);
            assert_eq!(s.screening, 100);
            assert!(!s.dither);
            assert!(!s.grayscale);
            assert!(s.adaptive);
            assert_eq!(s.pen_number, PenNum::Automatic);
            assert_eq!(s.end_style, EndStyle::UseObject);
            assert_eq!(s.fill_style, FillStyle::UseObject);
        }
    }

    #[test]
    fn fixed_width_round_trips() {
        let mut t = PlotStyleTable::default();
        t.set_fixed_width(1, 0.70);
        t.set_fixed_width(3, 0.13);
        assert_eq!(t.style(1).lineweight, PlotWidth::Fixed(0.70));
        assert_eq!(t.style(3).lineweight, PlotWidth::Fixed(0.13));
        t.set_use_object(1);
        assert_eq!(t.style(1).lineweight, PlotWidth::UseObject);
    }

    #[test]
    fn fixed_overrides_object() {
        let layers = LayerTable::with_defaults();
        let mut t = PlotStyleTable::default();
        t.set_fixed_width(1, 0.70);
        assert_eq!(plot_width_mm(&t, 1, Lineweight::Custom(0.25), 0, &layers), 0.70);
    }

    #[test]
    fn useobject_falls_through_to_lineweight() {
        let layers = LayerTable::with_defaults();
        let t = PlotStyleTable::default();
        assert_eq!(plot_width_mm(&t, 3, Lineweight::Custom(0.13), 0, &layers), 0.13);
        assert_eq!(plot_width_mm(&t, 3, Lineweight::Default, 0, &layers), DEFAULT_LINEWEIGHT_MM);
    }

    #[test]
    fn useobject_reads_layer_lineweight_via_bylayer() {
        let mut layers = LayerTable::with_defaults();
        let id = layers.add(Layer {
            name: "HEAVY".into(), color: Color::ByLayer, linetype: 0,
            lineweight: Lineweight::Custom(1.0),
            visible: true, locked: false, frozen: false, plottable: true,
            order:      0,});
        let t = PlotStyleTable::default();
        assert_eq!(plot_width_mm(&t, 7, Lineweight::ByLayer, id, &layers), 1.0);
    }

    #[test]
    fn ladder_is_sorted_and_starts_at_zero() {
        assert_eq!(AUTOCAD_LADDER[0], 0.00);
        for w in AUTOCAD_LADDER.windows(2) {
            assert!(w[1] > w[0], "ladder must be strictly increasing: {:?}", w);
        }
    }

    #[test]
    fn apply_color_honours_plot_color_grayscale_screening() {
        let mut t = PlotStyleTable::default();
        t.style_mut(1).plot_color = PlotColor::Black;
        t.style_mut(2).grayscale = true;
        t.style_mut(3).screening = 50;
        // plot_color override.
        assert_eq!(t.apply_color(Some(1), (10, 20, 30)), (0, 0, 0));
        // grayscale → equal channels.
        let g = t.apply_color(Some(2), (10, 20, 30));
        assert_eq!(g.0, g.1);
        assert_eq!(g.1, g.2);
        // screening tints toward white.
        let s = t.apply_color(Some(3), (200, 100, 50));
        assert!(s.0 > 200 && s.1 > 100 && s.2 > 50, "screening must lighten: {:?}", s);
        // truecolor (aci None) passes through unchanged.
        assert_eq!(t.apply_color(None, (10, 20, 30)), (10, 20, 30));
    }

    #[test]
    fn each_enum_round_trips_json() {
        // A representative style exercising every enum + flag.
        let s = PlotStyle {
            plot_color:  PlotColor::Rgb(10, 20, 30),
            dither:      true,
            grayscale:   true,
            pen_number:  PenNum::N(7),
            virtual_pen: PenNum::N(42),
            screening:   50,
            linetype:    PlotLinetype::Id(3),
            adaptive:    false,
            lineweight:  PlotWidth::Fixed(0.53),
            end_style:   EndStyle::Round,
            join_style:  JoinStyle::Bevel,
            fill_style:  FillStyle::Crosshatch,
        };
        let js = serde_json::to_string(&s).unwrap();
        let back: PlotStyle = serde_json::from_str(&js).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn table_round_trips_json_with_ladder_and_pens() {
        let mut t = PlotStyleTable::named("shop.pst");
        t.description = "house pens".into();
        t.apply_global_ltscale = true;
        t.ltscale_percent = 80.0;
        t.set_fixed_width(1, 0.70);
        t.style_mut(1).plot_color = PlotColor::Black;
        t.set_fixed_width(3, 0.13);
        t.lineweight_ladder.push(0.65);

        let js = serde_json::to_string(&t).unwrap();
        let back: PlotStyleTable = serde_json::from_str(&js).unwrap();
        assert_eq!(t, back);
        assert_eq!(back.style(1).lineweight, PlotWidth::Fixed(0.70));
        assert_eq!(back.style(1).plot_color, PlotColor::Black);
        assert_eq!(back.style(3).lineweight, PlotWidth::Fixed(0.13));
        assert!(back.lineweight_ladder.contains(&0.65));
        assert_eq!(back.by_aci.len(), 256);
    }

    #[test]
    fn ladder_edit_persists() {
        let mut t = PlotStyleTable::default();
        t.lineweight_ladder = vec![0.00, 0.10, 0.25, 0.50];
        let js = serde_json::to_string(&t).unwrap();
        let back: PlotStyleTable = serde_json::from_str(&js).unwrap();
        assert_eq!(back.lineweight_ladder, vec![0.00, 0.10, 0.25, 0.50]);
    }

    #[test]
    fn paper_dims_portrait() {
        assert_eq!(PaperSize::A4.dims_mm(), (210.0, 297.0));
        assert_eq!(PaperSize::A0.dims_mm(), (841.0, 1189.0));
    }

    #[test]
    fn config_default_is_pdf_fit_extents() {
        let c = PlotConfig::default();
        assert!(matches!(c.output, PlotTarget::PdfFile(_)));
        assert_eq!(c.scale, PlotScale::Fit);
        assert_eq!(c.area, PlotArea::Extents);
        assert_eq!(c.lw_scale, 1.0);
    }
}
