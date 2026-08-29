// Units — the document's real-world scale + distance parsing.
//
// A CAD document draws in abstract "scene units". `Units` records what ONE
// named real-world unit (mm/cm/m/in/ft) is worth in scene units, so the plot
// and viewport machinery can produce dimensionally-correct paper output. This
// is the foundation for the plotting spec's Part 5 invariant: a 1 m line shown
// through a 1:100 viewport must print at exactly 1 cm on paper, regardless of
// which unit system the document declares.
//
// **Default = { name: "mm", scene_per_unit: 1.0 }** — i.e. 1 scene unit = 1 mm.
// This exactly matches the app's historical implicit convention: `PlotScale::
// Ratio { model, paper_mm }` maps model-units→paper-mm 1:1, and the plot's
// physical-mm lineweight invariant both assume 1 scene unit = 1 mm. So adding
// this type with its default is a ZERO-behaviour-change addition.
//
// THE 3D SIDE. The 3D Factory, the renderer, the parametric generators and the
// lux engine all work in METRES. `metres_per_unit` is the derived conversion
// (drawing units → metres) and always satisfies
//
// ```text
// metres_per_unit = mm_per_named(name) / 1000.0 / scene_per_unit
// ```
//
// so the two representations can never disagree: setting the named unit and
// calibration (`Units::new`) derives the 3D factor, and setting a metre factor
// directly (`Units::from_metres_per_unit`) derives the nearest named unit. The
// default (mm, 1 scene unit = 1 mm) makes `metres_per_unit = 0.001` — a drawing
// in millimetres builds at true size in the 3D world without declaring anything.


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

/// Length display format (AcadDDUNITS "Length › Type").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthFormat { Scientific, Decimal, Engineering, Architectural, Fractional }

/// Angle display format (AcadDDUNITS "Angle › Type").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AngleFormat { DecimalDegrees, DegMinSec, Grads, Radians, Surveyor }

impl LengthFormat {
    pub const ALL: [LengthFormat; 5] = [
        LengthFormat::Scientific, LengthFormat::Decimal, LengthFormat::Engineering,
        LengthFormat::Architectural, LengthFormat::Fractional,
    ];
    pub fn label(self) -> &'static str {
        match self {
            LengthFormat::Scientific => "Scientific",
            LengthFormat::Decimal => "Decimal",
            LengthFormat::Engineering => "Engineering",
            LengthFormat::Architectural => "Architectural",
            LengthFormat::Fractional => "Fractional",
        }
    }
}
impl AngleFormat {
    pub const ALL: [AngleFormat; 5] = [
        AngleFormat::DecimalDegrees, AngleFormat::DegMinSec, AngleFormat::Grads,
        AngleFormat::Radians, AngleFormat::Surveyor,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AngleFormat::DecimalDegrees => "Decimal Degrees",
            AngleFormat::DegMinSec => "Deg/Min/Sec",
            AngleFormat::Grads => "Grads",
            AngleFormat::Radians => "Radians",
            AngleFormat::Surveyor => "Surveyor's Units",
        }
    }
}

/// The document's real-world calibration — ONE type serving both the 3D
/// Factory (metres conversion) and the plot/viewport machinery (named unit +
/// scene calibration + display formats).
///
/// `scene_per_unit` is the stored calibration number; everything physical
/// derives from `name` through the fixed `mm_per_named` table, so the same
/// "1:100" viewport prints identically in an mm, cm, m or inch document (the
/// unit factors cancel — see `viewport_zoom`). `metres_per_unit` is derived
/// from the same pair (`mm_per_named(name)/1000/scene_per_unit`) and is what
/// every 2D→3D boundary multiplies by.
#[derive(Clone, Debug, PartialEq)]
pub struct Units {
    /// Display unit name — a canonical short symbol ("mm", "cm", "m", "in",
    /// "ft", …) OR a custom label. This is the "Insertion scale" unit: what ONE
    /// app/scene unit represents physically. `mm_per_named` resolves it.
    pub name: String,
    /// Scene coordinates per ONE named unit. `1.0` = 1 app unit = 1 named unit
    /// (the common case from the Insertion-scale dropdown). `>1` = a calibrated
    /// document (e.g. 100 units per mm). Must be > 0.
    pub scene_per_unit: f64,
    /// Metres in one drawing unit, DERIVED: `mm_per_named(name)/1000/scene_per_unit`.
    /// 0.001 = millimetres (the default), 1.0 = metres. The 3D Factory, renderer
    /// and lux engine multiply drawing coordinates by this.
    pub metres_per_unit: f64,
    /// How this calibration was arrived at — an assumed fallback must never be
    /// written out as a positive assertion.
    pub source: UnitSource,
    /// Length display format + decimal precision (0..8). Formatting only —
    /// does not change stored geometry.
    pub length_format: LengthFormat,
    pub length_precision: u8,
    /// Angle display format + precision, and whether positive angles run
    /// clockwise (default false = CCW, AutoCAD default).
    pub angle_format: AngleFormat,
    pub angle_precision: u8,
    pub angle_clockwise: bool,
}

impl Default for Units {
    fn default() -> Self {
        // 1 scene unit = 1 mm — the app's historical implicit convention.
        // metres_per_unit = mm_per_named("mm")/1000/1.0 = 0.001: a millimetre
        // drawing builds at true size in the metre-based 3D world.
        Self {
            name: "mm".into(),
            scene_per_unit: 1.0,
            metres_per_unit: 0.001,
            source: UnitSource::Assumed,
            length_format: LengthFormat::Decimal,
            length_precision: 2,
            angle_format: AngleFormat::DecimalDegrees,
            angle_precision: 0,
            angle_clockwise: false,
        }
    }
}

/// The AutoCAD "Units to scale inserted content" list — (display label, canonical
/// short name stored in `Units.name`). `mm_per_named` covers every short name.
pub const INSERT_UNITS: &[(&str, &str)] = &[
    ("Unitless", "unit"),
    ("Inches", "in"),
    ("Feet", "ft"),
    ("US Survey Feet", "usft"),
    ("Miles", "mi"),
    ("Millimeters", "mm"),
    ("Centimeters", "cm"),
    ("Meters", "m"),
    ("Kilometers", "km"),
    ("Microinches", "uin"),
    ("Mils", "mil"),
    ("Yards", "yd"),
    ("Angstroms", "ang"),
    ("Nanometers", "nm"),
    ("Microns", "um"),
    ("Decimeters", "dm"),
    ("Dekameters", "dam"),
    ("Hectometers", "hm"),
    ("Gigameters", "gm"),
    ("Astronomical", "au"),
    ("Light Years", "ly"),
    ("Parsecs", "pc"),
];

impl Units {
    pub const MM: f64 = 0.001;
    pub const CM: f64 = 0.01;
    pub const M: f64 = 1.0;
    pub const INCH: f64 = 0.0254;
    pub const FOOT: f64 = 0.3048;

    /// Build from the named unit + scene calibration (the plot-side form).
    /// `metres_per_unit` is derived, so the two representations can never
    /// disagree. `source` starts `Assumed`.
    pub fn new(name: impl Into<String>, scene_per_unit: f64) -> Self {
        let name = name.into();
        let mpu = Self::mm_per_named(&name) / 1000.0 / scene_per_unit.max(1e-15);
        Self { name, scene_per_unit, metres_per_unit: mpu, ..Self::default() }
    }

    /// Build from a direct metre factor (the 3D-side form): derives the nearest
    /// named unit + the scene calibration that reproduces the factor. `source`
    /// is stored as given — use `UnitSource::User` when a person typed it,
    /// `Declared` when a file said it.
    pub fn from_metres_per_unit(metres_per_unit: f64, source: UnitSource) -> Self {
        // Nearest standard name by comparing mm_per_named/1000 to the factor.
        let standards: [(f64, &str); 5] = [
            (0.001, "mm"), (0.01, "cm"), (1.0, "m"),
            (0.0254, "in"), (0.3048, "ft"),
        ];
        let name = standards.iter()
            .min_by(|a, b| {
                (a.0 - metres_per_unit).abs().partial_cmp(&(b.0 - metres_per_unit).abs()).unwrap()
            })
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| "mm".into());
        let mpu = metres_per_unit.abs().max(1e-15);
        // scene_per_unit that makes mm_per_named(name)/1000/scene_per_unit == mpu.
        let scene_per_unit = Self::mm_per_named(&name) / 1000.0 / mpu;
        Self {
            name, scene_per_unit, metres_per_unit: mpu, source,
            ..Self::default()
        }
    }

    /// Drawing units → metres. The direction every 2D→3D boundary needs.
    #[inline]
    pub fn to_metres(&self, v: f64) -> f64 {
        v * self.metres_per_unit
    }

    /// Metres → drawing units. The direction every 3D→2D boundary needs, and the one that
    /// keeps a metre-shaped default honest when it is stored as a document-unit length.
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

    /// The AutoCAD display label for the current `name` (falls back to the raw
    /// name for a custom label).
    pub fn insert_label(&self) -> String {
        let key = self.name.trim().to_ascii_lowercase();
        INSERT_UNITS.iter()
            .find(|(_, sym)| *sym == key)
            .map(|(disp, _)| disp.to_string())
            .unwrap_or_else(|| self.name.clone())
    }

    /// Millimetres per ONE named unit — the fixed physical table. Unknown /
    /// custom names fall back to 1.0 (treated as mm-equivalent, never panics).
    pub fn mm_per_named(name: &str) -> f64 {
        match name.trim().to_ascii_lowercase().as_str() {
            // Metric
            "mm" | "millimeter" | "millimeters" | "millimetre" => 1.0,
            "cm" | "centimeter" | "centimeters" | "centimetre" => 10.0,
            "dm" | "decimeter" | "decimeters"                  => 100.0,
            "m"  | "meter" | "meters" | "metre"                => 1000.0,
            "dam" | "dekameter" | "dekameters"                 => 10_000.0,
            "hm" | "hectometer" | "hectometers"                => 100_000.0,
            "km" | "kilometer" | "kilometers"                  => 1_000_000.0,
            "um" | "micron" | "microns" | "micrometer"         => 0.001,
            "nm" | "nanometer" | "nanometers"                  => 1e-6,
            "ang" | "angstrom" | "angstroms"                   => 1e-7,
            "gm" | "gigameter" | "gigameters"                  => 1e12,
            // Imperial
            "in" | "inch" | "inches" | "\""                    => 25.4,
            "ft" | "foot" | "feet" | "'"                       => 304.8,
            "usft" | "us survey feet"                          => 304.800_6096,
            "yd" | "yard" | "yards"                            => 914.4,
            "mi" | "mile" | "miles"                            => 1_609_344.0,
            "mil" | "mils"                                     => 0.0254,
            "uin" | "microinch" | "microinches"                => 2.54e-5,
            // Astronomical
            "au" | "astronomical"                              => 1.495_978_707e14,
            "ly" | "light year" | "light years"               => 9.460_730_472e18,
            "pc" | "parsec" | "parsecs"                        => 3.085_677_581e19,
            // Unitless / unknown → mm-equivalent (never panics).
            _ => 1.0,
        }
    }

    /// Metres per one named unit (physical). Convenience over `mm_per_named`.
    pub fn meters_per_unit(name: &str) -> f64 {
        Self::mm_per_named(name) / 1000.0
    }

    /// Millimetres per one named unit for THIS document's unit.
    pub fn mm_per_unit(&self) -> f64 {
        Self::mm_per_named(&self.name)
    }

    /// Scene units per physical metre: `scene_per_unit / meters_per_unit(name)`.
    /// Used by the CTB lineweight override math (`mm_to_scene = scene_per_meter *
    /// 0.001`) so physical-mm pen widths convert to scene units correctly.
    pub fn scene_per_meter(&self) -> f64 {
        let mpu = Self::meters_per_unit(&self.name);
        if mpu.abs() < 1e-15 { self.scene_per_unit } else { self.scene_per_unit / mpu }
    }

    /// Scene units per physical millimetre.
    pub fn scene_per_mm(&self) -> f64 {
        let mm = self.mm_per_unit();
        if mm.abs() < 1e-15 { self.scene_per_unit } else { self.scene_per_unit / mm }
    }

    /// The units-derived factor for a viewport at nominal ratio `desired_scale`
    /// (1:100 → 0.01). Returns paper-mm PER scene-unit for that scale:
    ///
    /// ```text
    /// paper_mm_per_scene = desired_scale × mm_per_named / scene_per_unit
    /// ```
    ///
    /// A model length of `L_scene` scene units therefore lands on paper at
    /// `L_scene × this` millimetres. The `mm_per_named / scene_per_unit` factor
    /// cancels the model's own physical calibration, so a 1:100 viewport prints
    /// the same paper length in mm, cm, m and inch documents (Part 5).
    pub fn viewport_paper_mm_per_scene(&self, desired_scale: f64) -> f64 {
        desired_scale * self.mm_per_unit() / self.scene_per_unit
    }

    /// Inverse of `viewport_paper_mm_per_scene`: recover the nominal ratio
    /// (1:100 → 0.01) from a viewport's stored paper-mm-per-scene factor, so a
    /// Properties dialog can display "1:N" independent of the document unit.
    pub fn viewport_nominal_scale(&self, paper_mm_per_scene: f64) -> f64 {
        let mm = self.mm_per_unit();
        if mm.abs() < 1e-15 { paper_mm_per_scene } else { paper_mm_per_scene * self.scene_per_unit / mm }
    }

    /// Parse a distance string into SCENE units. A bare number is in the
    /// document's DISPLAY unit (`"25"` in an mm-doc = 25 mm = 25 scene units when
    /// `scene_per_unit=1`); an explicit suffix converts through the physical
    /// table (`"25cm"` = 250 mm regardless of the document unit). Returns None on
    /// unparseable input or a non-finite / non-positive-denominator result.
    /// Accepts an optional leading sign and a trailing unit suffix, with or
    /// without a space: "25", "25mm", "25 cm", "-3.5in", `12"`, `6'`.
    pub fn parse_distance(&self, s: &str) -> Option<f64> {
        let t = s.trim().to_ascii_lowercase();
        if t.is_empty() { return None; }
        // Find where the numeric head ends and an alphabetic / quote suffix begins.
        let split = t.find(|c: char| c.is_ascii_alphabetic() || c == '"' || c == '\'');
        let (num_part, suffix) = match split {
            Some(i) => (t[..i].trim(), t[i..].trim()),
            None => (t.as_str(), ""),
        };
        let value: f64 = num_part.parse().ok()?;
        if !value.is_finite() { return None; }
        if suffix.is_empty() {
            // Display units → scene units.
            Some(value * self.scene_per_unit)
        } else {
            // Explicit physical unit → mm → scene units via this doc's calibration.
            let mm = value * Self::mm_per_named(suffix);
            Some(mm * self.scene_per_mm())
        }
    }

    /// Convert a scene-units distance back into the display unit (no suffix) —
    /// the inverse of a bare `parse_distance`. For readouts / dimension text.
    pub fn to_display(&self, scene: f64) -> f64 {
        if self.scene_per_unit.abs() < 1e-15 { scene } else { scene / self.scene_per_unit }
    }

    // -- Display formatting (AcadDDUNITS Length / Angle) ---------------------

    /// Format a SCENE-units length for display per `length_format` +
    /// `length_precision`. Decimal / Scientific are exact; the imperial
    /// feet-inch formats (Engineering / Architectural / Fractional) treat the
    /// display value as inches (they're only meaningful for inch drawings).
    pub fn format_length(&self, scene: f64) -> String {
        let v = self.to_display(scene);
        let p = self.length_precision as usize;
        match self.length_format {
            LengthFormat::Decimal => format!("{:.*}", p, v),
            LengthFormat::Scientific => format!("{:.*E}", p, v),
            LengthFormat::Engineering => {
                // value in inches → F'-I.d"
                let neg = v < 0.0; let a = v.abs();
                let feet = (a / 12.0).floor();
                let inch = a - feet * 12.0;
                let s = format!("{}'-{:.*}\"", feet as i64, p, inch);
                if neg { format!("-{s}") } else { s }
            }
            LengthFormat::Architectural => {
                // value in inches → F'-I n/d" (fractional inch, denom = 2^prec)
                let neg = v < 0.0; let a = v.abs();
                let feet = (a / 12.0).floor();
                let inch_total = a - feet * 12.0;
                let (whole, frac) = fmt_fraction(inch_total, self.length_precision);
                let s = if frac.is_empty() {
                    format!("{}'-{}\"", feet as i64, whole)
                } else {
                    format!("{}'-{} {}\"", feet as i64, whole, frac)
                };
                if neg { format!("-{s}") } else { s }
            }
            LengthFormat::Fractional => {
                let neg = v < 0.0; let a = v.abs();
                let (whole, frac) = fmt_fraction(a, self.length_precision);
                let s = if frac.is_empty() { whole.to_string() } else { format!("{whole} {frac}") };
                if neg { format!("-{s}") } else { s }
            }
        }
    }

    /// Format an angle (radians, CCW) per `angle_format` + `angle_precision`,
    /// honouring `angle_clockwise`.
    pub fn format_angle(&self, rad: f64) -> String {
        let sign = if self.angle_clockwise { -1.0 } else { 1.0 };
        let deg = (rad.to_degrees() * sign).rem_euclid(360.0);
        let p = self.angle_precision as usize;
        match self.angle_format {
            AngleFormat::DecimalDegrees => format!("{:.*}\u{00B0}", p, deg),
            AngleFormat::Radians => format!("{:.*}r", p, deg.to_radians()),
            AngleFormat::Grads => format!("{:.*}g", p, deg * 10.0 / 9.0),
            AngleFormat::DegMinSec => {
                let d = deg.floor();
                let mfull = (deg - d) * 60.0;
                let m = mfull.floor();
                let s = (mfull - m) * 60.0;
                format!("{}\u{00B0}{}'{:.*}\"", d as i64, m as i64, p, s)
            }
            AngleFormat::Surveyor => {
                // Bearing from North/South toward East/West.
                let a = deg.rem_euclid(360.0);
                let (ns, ew, bearing) = if a <= 90.0 { ("N", "E", a) }
                    else if a <= 180.0 { ("N", "W", 180.0 - a) }
                    else if a <= 270.0 { ("S", "W", a - 180.0) }
                    else { ("S", "E", 360.0 - a) };
                format!("{ns}{:.*}\u{00B0}{ew}", p, bearing)
            }
        }
    }

    /// AutoCAD-style "Sample Output" preview: a formatted point + a polar pair,
    /// reflecting the current Length/Angle settings. Two short lines.
    pub fn sample_output(&self) -> (String, String) {
        let x = 1.5 * self.scene_per_unit;
        let y = 2.003_906_25 * self.scene_per_unit;
        let line1 = format!("{}, {}, 0", self.format_length(x), self.format_length(y));
        let line2 = format!("{} < {}", self.format_length(x),
            self.format_angle(45.0_f64.to_radians()));
        (line1, line2)
    }
}

/// Format the fractional part of `v` as "n/d" with denominator 2^prec (clamped
/// 1..=8 → 2..256), returning (whole_part, "n/d" or ""). Reduces the fraction.
fn fmt_fraction(v: f64, prec: u8) -> (i64, String) {
    let denom = 1i64 << prec.clamp(0, 8).max(1); // prec 0 → still /2 min? use 1<<max(1)
    let whole = v.floor() as i64;
    let frac = v - whole as f64;
    let mut num = (frac * denom as f64).round() as i64;
    let mut den = denom;
    if num == 0 { return (whole, String::new()); }
    if num >= den { return (whole + 1, String::new()); }
    // reduce
    let g = gcd(num, den);
    num /= g; den /= g;
    (whole, format!("{num}/{den}"))
}

fn gcd(a: i64, b: i64) -> i64 { if b == 0 { a } else { gcd(b, a % b) } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_one_scene_unit_per_mm() {
        let u = Units::default();
        assert_eq!(u.name, "mm");
        assert_eq!(u.scene_per_unit, 1.0);
        assert_eq!(u.metres_per_unit, 0.001);
        assert_eq!(u.source, UnitSource::Assumed);
        // 25 (bare) in an mm-doc = 25 scene units.
        assert_eq!(u.parse_distance("25"), Some(25.0));
    }

    #[test]
    fn suffix_converts_through_physical_table() {
        let u = Units::default(); // mm, 1 scene = 1 mm
        assert_eq!(u.parse_distance("25cm"), Some(250.0)); // 25 cm = 250 mm = 250 scene
        assert_eq!(u.parse_distance("1m"), Some(1000.0));  // 1 m = 1000 mm
        assert_eq!(u.parse_distance("1in").unwrap(), 25.4);
        assert_eq!(u.parse_distance("2 cm"), Some(20.0));  // space allowed
        assert_eq!(u.parse_distance("12\"").unwrap(), 12.0 * 25.4);
    }

    #[test]
    fn bare_number_is_display_unit() {
        let cm = Units::new("cm", 1.0); // 1 scene unit = 1 cm
        assert_eq!(cm.parse_distance("25"), Some(25.0));   // 25 cm = 25 scene
        assert_eq!(cm.parse_distance("25cm"), Some(25.0)); // same, explicit
        assert_eq!(cm.parse_distance("250mm"), Some(25.0)); // 250 mm = 25 cm = 25 scene
    }

    #[test]
    fn calibrated_document_100_units_per_mm() {
        // Spec Part 5 worked example: 1 mm = 100 units.
        let u = Units::new("mm", 100.0);
        // 1 m = 1000 mm = 100 000 scene units, via a suffix.
        assert_eq!(u.parse_distance("1m"), Some(100_000.0));
        // Bare 1000 = 1000 mm (display units) = 100 000 scene.
        assert_eq!(u.parse_distance("1000"), Some(100_000.0));
    }

    #[test]
    fn viewport_zoom_is_unit_independent() {
        // The single most important property: a 1 m line in a 1:100 viewport must
        // print at 10 mm on paper, regardless of the document's unit system.
        let one_m_paper_mm = |u: &Units, one_m_scene: f64| {
            u.viewport_paper_mm_per_scene(0.01) * one_m_scene
        };
        // mm-doc, 1 unit = 1 mm → 1 m = 1000 scene.
        let mm = Units::new("mm", 1.0);
        assert!((one_m_paper_mm(&mm, 1000.0) - 10.0).abs() < 1e-9);
        // cm-doc, 1 unit = 1 cm → 1 m = 100 scene.
        let cm = Units::new("cm", 1.0);
        assert!((one_m_paper_mm(&cm, 100.0) - 10.0).abs() < 1e-9);
        // m-doc, 1 unit = 1 m → 1 m = 1 scene.
        let m = Units::new("m", 1.0);
        assert!((one_m_paper_mm(&m, 1.0) - 10.0).abs() < 1e-9);
        // calibrated mm-doc, 100 units = 1 mm → 1 m = 100 000 scene.
        let mm100 = Units::new("mm", 100.0);
        assert!((one_m_paper_mm(&mm100, 100_000.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn nominal_scale_round_trips_in_any_unit() {
        // Create a 1:100 viewport (store paper-mm-per-scene), then read it back:
        // the Properties dialog must show "1:100" regardless of the doc unit.
        for u in [Units::new("mm", 1.0), Units::new("cm", 1.0),
                  Units::new("m", 1.0), Units::new("mm", 100.0)] {
            let stored = u.viewport_paper_mm_per_scene(0.01);      // create at 1:100
            let nominal = u.viewport_nominal_scale(stored);        // readout
            assert!((nominal - 0.01).abs() < 1e-12,
                "unit {:?}/{}: nominal {} != 0.01", u.name, u.scene_per_unit, nominal);
        }
        // Default doc: the stored factor is literally 1/N (old behaviour).
        assert!((Units::default().viewport_paper_mm_per_scene(0.01) - 0.01).abs() < 1e-15);
    }

    #[test]
    fn scene_per_meter_matches_calibration() {
        assert!((Units::new("mm", 1.0).scene_per_meter() - 1000.0).abs() < 1e-9);
        assert!((Units::new("cm", 1.0).scene_per_meter() - 100.0).abs() < 1e-9);
        assert!((Units::new("m", 1.0).scene_per_meter() - 1.0).abs() < 1e-9);
        assert!((Units::new("mm", 100.0).scene_per_meter() - 100_000.0).abs() < 1e-9);
    }

    #[test]
    fn junk_and_signs() {
        let u = Units::default();
        assert_eq!(u.parse_distance(""), None);
        assert_eq!(u.parse_distance("abc"), None);
        assert_eq!(u.parse_distance("-3.5"), Some(-3.5));
        assert_eq!(u.to_display(250.0), 250.0);
        assert_eq!(Units::new("cm", 1.0).to_display(25.0), 25.0);
    }

    // -- the 3D side (merged from the fork's DocUnits) ----------------------

    #[test]
    fn derived_metres_per_unit_matches_named_unit() {
        assert_eq!(Units::new("mm", 1.0).metres_per_unit, 0.001);
        assert_eq!(Units::new("cm", 1.0).metres_per_unit, 0.01);
        assert_eq!(Units::new("m", 1.0).metres_per_unit, 1.0);
        assert_eq!(Units::new("in", 1.0).metres_per_unit, 0.0254);
        assert_eq!(Units::new("ft", 1.0).metres_per_unit, 0.3048);
    }

    #[test]
    fn from_metres_per_unit_derives_nearest_named_unit() {
        let u = Units::from_metres_per_unit(0.001, UnitSource::User);
        assert_eq!(u.name, "mm");
        assert_eq!(u.metres_per_unit, 0.001);
        assert_eq!(u.source, UnitSource::User);
        let m = Units::from_metres_per_unit(1.0, UnitSource::Declared);
        assert_eq!(m.name, "m");
        assert_eq!(m.metres_per_unit, 1.0);
        // round-trip: building from the derived pair reproduces the factor
        let back = Units::new(&u.name, u.scene_per_unit);
        assert!((back.metres_per_unit - u.metres_per_unit).abs() < 1e-12);
    }

    #[test]
    fn to_from_metres_round_trip() {
        let u = Units::new("mm", 1.0);
        assert!((u.from_metres(u.to_metres(3000.0)) - 3000.0).abs() < 1e-9);
        let m = Units::new("m", 1.0);
        assert!((m.from_metres(m.to_metres(3.0)) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn label_matches_metre_factor() {
        assert_eq!(Units::default().label(), "mm");
        assert_eq!(Units::new("m", 1.0).label(), "m");
        assert_eq!(Units::from_metres_per_unit(0.0254, UnitSource::Assumed).label(), "in");
    }
}
