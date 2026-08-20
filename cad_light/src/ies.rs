//! IES LM-63 photometric file support (Type A/B/C, TILT=NONE).

use serde::{Deserialize, Serialize};

/// Goniometer geometry declared in the IES header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhotometryType {
    A,
    B,
    C,
}

/// A parsed IES luminous-intensity distribution. `candela[h][v]` is indexed by
/// horizontal-angle row then vertical-angle column (LM-63 layout).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IesProfile {
    pub name: String,
    pub photometry: PhotometryType,
    pub lumens: f64,
    pub multiplier: f64,
    pub vertical_angles: Vec<f64>,
    pub horizontal_angles: Vec<f64>,
    pub candela: Vec<Vec<f64>>,
    pub watts: f64,
    pub width: f64,
    pub length: f64,
    pub height: f64,
    /// LUMINOUS AREA — the emitting aperture, in metres. NOT the housing.
    ///
    /// Glare is computed from luminance, `L = I / A`, so a 600 mm fitting with a 300 mm aperture is
    /// four times brighter than its outline suggests. Using the outline under-states luminance and
    /// therefore under-states UGR — the direction that passes a design which should fail. Falls
    /// back to the outline when the file declares no separate aperture.
    ///
    /// `#[serde(default)]` because these arrived AFTER projects were already being saved, and
    /// without it every one of those projects stopped opening: serde refuses a missing field, so a
    /// whole scheme became unreadable because its photometry predated a glare calculation it had
    /// never used. Found by loading the user's own `testfiles.simlux.json` through the real loader
    /// — nothing else exercised that path, because everything else builds its scenes in code.
    ///
    /// Zero is also the honest default. It means "no aperture declared", which excludes the fitting
    /// from UGR rather than inventing an area for it.
    #[serde(default)]
    pub luminous_length: f64,
    #[serde(default)]
    pub luminous_width: f64,
    /// WHO MADE IT, from the file's own header — `[MANUFAC]` in IES, record 1 in EULUMDAT.
    ///
    /// Carried because a report has to say what produced the illuminance it states: a lighting
    /// report without a schedule cannot be checked, ordered from, or handed to an installer.
    /// Empty when the file declares none, which is that file's omission and is shown as one.
    ///
    /// `#[serde(default)]` for the same reason the luminous dimensions have it — projects were
    /// already being saved before these existed, and serde refuses a missing field.
    #[serde(default)]
    pub manufacturer: String,
    /// Catalogue number — `[LUMCAT]` in IES, record 9 in EULUMDAT.
    #[serde(default)]
    pub catalogue: String,
    /// What is in it: lamp type, and the colour data where the file gives it.
    #[serde(default)]
    pub lamp: String,
}

impl IesProfile {
    /// Projected luminous area (m²) seen at `gamma_deg` from nadir.
    ///
    /// A flat aperture foreshortens as `cos γ`; a round one is treated as a disc of the declared
    /// diameter, a rectangular one as `length × width`. A file with no dimensions at all returns
    /// `None` rather than zero — an area of zero is an infinite luminance, and a glare figure built
    /// on it would be nonsense presented as a number.
    pub fn projected_luminous_area(&self, gamma_deg: f64) -> Option<f64> {
        let flat = self.aperture()?.flat_area();
        let cos = gamma_deg.to_radians().cos().abs();
        // Below ~5° of grazing the projection collapses and the luminance runs away; the standard
        // treats such a source as contributing nothing, and so does this.
        (cos > 0.087).then_some(flat * cos)
    }

    /// The EMITTING APERTURE's outline, in metres, or `None` when the file declares none.
    ///
    /// Zero dimensions are the normal case, not an error — this crate's own IES fixture declares
    /// `0 0 0`, and both synthesised profiles in the app hard-code every dimension to zero.
    pub fn aperture(&self) -> Option<Aperture> {
        Aperture::from_pair(self.luminous_length, self.luminous_width)
    }

    /// The HOUSING outline — the physical body, not the light-emitting part. `(length, width,
    /// height)` in metres, or `None` when the file declares none.
    ///
    /// This is what to DRAW. The aperture is what glare is computed from, and the two are
    /// different numbers on purpose: a 600 mm fitting with a 300 mm aperture is four times brighter
    /// than its outline suggests.
    pub fn housing(&self) -> Option<(f64, f64, f64)> {
        (self.length > 0.0).then_some((self.length, self.width, self.height))
    }

    /// The housing's footprint as a SHAPE, so drawing code and glare code cannot disagree about
    /// what "width = 0" means.
    pub fn housing_shape(&self) -> Option<Aperture> {
        Aperture::from_pair(self.length, self.width)
    }
}

/// A luminous or physical outline: rectangular, or round with a diameter.
///
/// EULUMDAT (and LM-63, once its negative-means-circular convention is normalised at parse time)
/// marks a ROUND outline by leaving the width at zero and putting the diameter in the length. That
/// rule used to live inside `projected_luminous_area` alone; anything else that needed to know a
/// fitting's shape — such as drawing it — would have had to re-derive it and could have got it
/// wrong. One definition, used by both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aperture {
    Round { d: f64 },
    Rect { l: f64, w: f64 },
}

impl Aperture {
    /// From a `(length, width)` pair in metres. `None` when the length is not positive.
    pub fn from_pair(l: f64, w: f64) -> Option<Self> {
        if l <= 0.0 {
            return None;
        }
        Some(if w <= 0.0 { Aperture::Round { d: l } } else { Aperture::Rect { l, w } })
    }

    /// Its area seen face-on, m².
    pub fn flat_area(self) -> f64 {
        match self {
            Aperture::Round { d } => std::f64::consts::PI * (d * 0.5).powi(2),
            Aperture::Rect { l, w } => l * w,
        }
    }

    /// Its extent as `(along x, along y)` before rotation, m — what a drawer needs.
    pub fn footprint(self) -> (f64, f64) {
        match self {
            Aperture::Round { d } => (d, d),
            Aperture::Rect { l, w } => (l, w),
        }
    }
}

impl IesProfile {
    /// Peak luminous intensity across the whole table (candela), multiplier applied.
    pub fn peak_candela(&self) -> f64 {
        self.candela.iter().flat_map(|r| r.iter()).cloned().fold(0.0, f64::max) * self.multiplier
    }

    /// Bilinearly-interpolated luminous intensity (candela) toward the given
    /// vertical/horizontal angle in degrees. Zero outside the measured vertical
    /// range (a downlight emits nothing above its last angle).
    pub fn intensity(&self, vertical_deg: f64, horizontal_deg: f64) -> f64 {
        let (va, ha) = (&self.vertical_angles, &self.horizontal_angles);
        if va.is_empty() || ha.is_empty() || self.candela.is_empty() {
            return 0.0;
        }
        if vertical_deg < va[0] - 1e-6 || vertical_deg > va[va.len() - 1] + 1e-6 {
            return 0.0;
        }
        let (v0, v1, vt) = bracket(va, vertical_deg);
        let (h0, h1, ht) = if ha.len() == 1 { (0, 0, 0.0) } else { bracket(ha, horizontal_deg.rem_euclid(360.0)) };
        let c0 = lerp(self.candela[h0][v0], self.candela[h0][v1], vt);
        let c1 = lerp(self.candela[h1][v0], self.candela[h1][v1], vt);
        lerp(c0, c1, ht) * self.multiplier
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Locate `x` in ascending `xs`; returns `(i, i+1, frac)` clamped to the ends.
fn bracket(xs: &[f64], x: f64) -> (usize, usize, f64) {
    if x <= xs[0] {
        return (0, 0, 0.0);
    }
    let last = xs.len() - 1;
    if x >= xs[last] {
        return (last, last, 0.0);
    }
    let mut i = 0;
    while i + 1 < xs.len() && xs[i + 1] < x {
        i += 1;
    }
    (i, i + 1, (x - xs[i]) / (xs[i + 1] - xs[i]))
}

/// Parse the contents of an IES LM-63 file (TILT=NONE) into an [`IesProfile`].
pub fn parse(contents: &str) -> Result<IesProfile, String> {
    let err = |m: &str| m.to_string();

    let mut name = String::new();
    // THE HEADER KEYWORDS A SCHEDULE NEEDS. LM-63 puts them before TILT=, one per line, as
    // `[KEYWORD] value` — so they are read in the same pass that finds TILT rather than in a
    // second walk of the file.
    let mut manufacturer = String::new();
    let mut catalogue = String::new();
    let mut lamp = String::new();
    let mut tilt_line: Option<&str> = None;
    let mut body_start = 0usize;
    let lines: Vec<&str> = contents.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("TILT=") {
            tilt_line = Some(rest.trim());
            body_start = i + 1;
            break;
        }
        for (key, slot) in [
            ("[LUMINAIRE]", &mut name),
            ("[MANUFAC]", &mut manufacturer),
            ("[LUMCAT]", &mut catalogue),
            ("[LAMP]", &mut lamp),
        ] {
            if let Some(rest) = line.strip_prefix(key) {
                *slot = rest.trim().to_string();
            }
        }
    }
    let tilt = tilt_line.ok_or_else(|| err("missing TILT= line"))?;
    if !tilt.eq_ignore_ascii_case("NONE") {
        return Err(err("only TILT=NONE is supported for now"));
    }
    if name.is_empty() {
        name = "Luminaire".to_string();
    }

    let nums: Vec<f64> = lines[body_start..]
        .iter()
        .flat_map(|l| l.split_whitespace())
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    let mut it = nums.into_iter();
    let mut next = |what: &str| it.next().ok_or_else(|| err(&format!("unexpected EOF reading {what}")));

    let _num_lamps = next("num_lamps")?;
    let lumens = next("lumens_per_lamp")?;
    let multiplier = next("candela_multiplier")?;
    let n_vert = next("num_vertical_angles")? as usize;
    let n_horiz = next("num_horizontal_angles")? as usize;
    let photo = next("photometric_type")?;
    let units = next("units_type")?;
    let width = next("width")?;
    let length = next("length")?;
    let height = next("height")?;
    let _ballast = next("ballast_factor")?;
    let _future = next("future_use")?;
    let watts = next("input_watts")?;

    if n_vert == 0 || n_horiz == 0 {
        return Err(err("zero vertical or horizontal angles"));
    }

    let mut vertical_angles = Vec::with_capacity(n_vert);
    for _ in 0..n_vert {
        vertical_angles.push(next("vertical angle")?);
    }
    let mut horizontal_angles = Vec::with_capacity(n_horiz);
    for _ in 0..n_horiz {
        horizontal_angles.push(next("horizontal angle")?);
    }
    let mut candela = Vec::with_capacity(n_horiz);
    for _ in 0..n_horiz {
        let mut row = Vec::with_capacity(n_vert);
        for _ in 0..n_vert {
            row.push(next("candela value")?);
        }
        candela.push(row);
    }

    let to_m = if units as i32 == 1 { 0.3048 } else { 1.0 };
    let photometry = match photo as i32 {
        3 => PhotometryType::A,
        2 => PhotometryType::B,
        _ => PhotometryType::C,
    };

    // LM-63 MARKS A CIRCULAR OPENING WITH A NEGATIVE DIMENSION. Taken literally — and it was, there
    // being no `abs()` anywhere on this path — a header of `-0.6` became −0.6 metres: a negative
    // area for glare, and an inside-out box for anything that tried to draw it.
    //
    // This crate already has a convention for round, the one EULUMDAT uses: width zero, diameter in
    // the length (see `Aperture::from_pair`). Normalising here means both formats arrive in the same
    // shape and nothing downstream has to know which file it came from.
    let circular = width < 0.0 || length < 0.0;
    let (w_m, l_m, h_m) = (width.abs() * to_m, length.abs() * to_m, height.abs() * to_m);
    let w_m = if circular { 0.0 } else { w_m };

    Ok(IesProfile {
        name,
        photometry,
        lumens,
        multiplier,
        vertical_angles,
        horizontal_angles,
        candela,
        watts,
        width: w_m,
        length: l_m,
        height: h_m,
        // LM-63's header dimensions ARE the luminous opening — the format calls them "luminous
        // width / length / height", unlike EULUMDAT which carries the housing and the aperture
        // separately. So there is nothing else to read here.
        luminous_length: l_m,
        luminous_width: w_m,
        manufacturer,
        catalogue,
        lamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal LM-63 Type C profile (2 vertical angles, 1 horizontal).
    const SAMPLE: &str = "IESNA:LM-63-1995\nTILT=NONE\n1 -1 1.0 2 1 1 2 0 0 0\n1.0 1.0 100\n0.0 90.0\n0.0\n1000.0 10.0\n";

    #[test]
    fn parses_and_interpolates() {
        let p = parse(SAMPLE).unwrap();
        assert_eq!(p.photometry, PhotometryType::C);
        assert_eq!(p.vertical_angles, vec![0.0, 90.0]);
        assert!((p.intensity(0.0, 0.0) - 1000.0).abs() < 1e-6);
        assert!((p.intensity(45.0, 0.0) - 505.0).abs() < 1e-6); // halfway
        assert_eq!(p.intensity(95.0, 0.0), 0.0); // above range
    }
}

/// Parse every real photometric file in `PHOTOMETRY_DIR` and report what it says.
///
///   PHOTOMETRY_DIR="D:/.../WORKING" cargo test -p cad_light photometry_probe -- --ignored --nocapture
///
/// The counterpart to `ldt_probe_real_files`, and for the same reason: a synthetic fixture only
/// proves the parser agrees with whoever wrote the fixture. Real manufacturer files carry absolute
/// photometry, negative dimensions meaning "circular", multi-lamp sets and unit codes that nobody
/// would think to invent.
///
/// It also reports LUMINOUS EFFICACY, which is the cheapest possible sanity check on a file: white
/// light cannot exceed roughly 250–300 lm/W even in theory, and real LEDs sit near 100–160. A file
/// claiming more than that is wrong, and it will be wrong in whatever tool loads it — which is
/// worth knowing before a calculation is built on it.
#[test]
#[ignore = "needs PHOTOMETRY_DIR=<folder of .ies/.ldt files>"]
fn photometry_probe_real_files() {
    let Ok(dir) = std::env::var("PHOTOMETRY_DIR") else {
        println!("set PHOTOMETRY_DIR to a folder of .ies / .ldt files");
        return;
    };
    let mut rows: Vec<String> = Vec::new();
    let (mut ok, mut bad, mut suspect) = (0, 0, 0);
    for e in std::fs::read_dir(&dir).expect("read PHOTOMETRY_DIR").flatten() {
        let p = e.path();
        let is_ies = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ies"));
        let is_ldt = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ldt"));
        if !is_ies && !is_ldt {
            continue;
        }
        let bytes = std::fs::read(&p).expect("read file");
        let text: String = match String::from_utf8(bytes.clone()) {
            Ok(s) => s,
            Err(_) => bytes.iter().map(|&b| b as char).collect(),
        };
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let parsed = if is_ies { parse(&text) } else { crate::ldt::parse(&text) };
        match parsed {
            Ok(prof) => {
                ok += 1;
                let eff = if prof.watts > 0.0 && prof.lumens > 0.0 {
                    prof.lumens / prof.watts
                } else {
                    f64::NAN
                };
                let flag = if eff.is_finite() && eff > 250.0 {
                    suspect += 1;
                    "  <-- IMPOSSIBLE EFFICACY, the file is wrong"
                } else {
                    ""
                };
                rows.push(format!(
                    "OK   {name}\n     {:.0} lm · {:.0} W · {} · peak {:.0} cd · nadir {:.0} cd\n     \
                     {} C-planes x {} gamma · {:.2} x {:.2} x {:.2} m{flag}",
                    prof.lumens.max(0.0),
                    prof.watts,
                    if eff.is_finite() { format!("{eff:.0} lm/W") } else { "efficacy unknown".into() },
                    prof.peak_candela(),
                    prof.intensity(0.0, 0.0),
                    prof.horizontal_angles.len(),
                    prof.vertical_angles.len(),
                    prof.length, prof.width, prof.height,
                ));
            }
            Err(err) => {
                bad += 1;
                rows.push(format!("FAIL {name}\n     {err}"));
            }
        }
    }
    rows.sort();
    for r in &rows {
        println!("{r}");
    }
    println!("\n{ok} parsed, {bad} failed, {suspect} physically impossible");
    assert_eq!(bad, 0, "every manufacturer file in the folder should parse");
}

/// THE GLARE AREA MUST NOT MOVE.
///
/// `projected_luminous_area` feeds UGR (`L² ω / p²` in CIE 117), and the engine is validated against
/// DIALux. Before this function was refactored to go through the new shape accessors, its outputs
/// were pinned here — so the refactor is provably a refactor and not a change of answer.
///
/// The values are the FORMULA's, computed by hand, not captured from the code: a golden file
/// recorded from the implementation it is meant to check is a circular test.
///   rect 1.2 × 0.3 m: 0.36 m² × cos γ
///   round Ø0.2 m:     π(0.1)² = 0.031415926535897934 m² × cos γ
#[cfg(test)]
mod the_glare_area_is_unchanged {
    use super::*;

    fn prof(luminous_length: f64, luminous_width: f64) -> IesProfile {
        IesProfile {
            manufacturer: String::new(),
            catalogue: String::new(),
            lamp: String::new(),
            name: "t".into(),
            photometry: PhotometryType::C,
            lumens: 1000.0,
            multiplier: 1.0,
            vertical_angles: vec![0.0],
            horizontal_angles: vec![0.0],
            candela: vec![vec![100.0]],
            watts: 10.0,
            width: 0.0,
            length: 0.0,
            height: 0.0,
            luminous_length,
            luminous_width,
        }
    }

    #[test]
    fn a_rectangular_aperture_foreshortens_as_cos_gamma() {
        let p = prof(1.2, 0.3);
        for (g, want) in [
            (0.0_f64, 0.36_f64),
            (30.0, 0.36 * 0.866_025_403_784_438_6),
            (45.0, 0.36 * 0.707_106_781_186_547_6),
            (60.0, 0.36 * 0.5),
            (80.0, 0.36 * 0.173_648_177_666_930_3),
        ] {
            let got = p.projected_luminous_area(g).expect("declared aperture");
            assert!((got - want).abs() < 1e-12, "gamma {g}: {got} != {want}");
        }
    }

    #[test]
    fn a_round_aperture_is_a_disc_of_the_declared_length() {
        let p = prof(0.2, 0.0); // EULUMDAT: width 0 marks it round, length is the diameter
        let flat = std::f64::consts::PI * 0.1_f64 * 0.1;
        for g in [0.0_f64, 25.0, 55.0, 79.0] {
            let want = flat * g.to_radians().cos();
            let got = p.projected_luminous_area(g).expect("declared aperture");
            assert!((got - want).abs() < 1e-12, "gamma {g}: {got} != {want}");
        }
    }

    /// The grazing cut-off and the no-aperture case are part of the contract too: an area of zero
    /// is an infinite luminance, and a glare figure built on it would be nonsense with a number.
    #[test]
    fn the_edges_of_the_contract_hold() {
        assert!(prof(0.0, 0.0).projected_luminous_area(0.0).is_none(), "no aperture declared");
        assert!(prof(-1.0, 0.2).projected_luminous_area(0.0).is_none(), "negative length");
        let p = prof(1.2, 0.3);
        // `cos > 0.087` puts the cut-off at 85.01 deg, not 85: cos(85 deg) = 0.0872, still in.
        // Stated exactly, because "about 5 degrees of grazing" is prose, not a test.
        assert!(p.projected_luminous_area(86.0).is_none(), "past the grazing cut-off: excluded");
        assert!(p.projected_luminous_area(85.0).is_some(), "just inside it: included");
        // Symmetric in gamma — the sign of the angle cannot change an area.
        assert_eq!(p.projected_luminous_area(40.0), p.projected_luminous_area(-40.0));
    }
}

/// LM-63'S NEGATIVE DIMENSIONS MEAN CIRCULAR, NOT NEGATIVE.
///
/// There was no `abs()` anywhere on the IES path, so a header of `-0.6` became −0.6 metres: a
/// negative area for glare and an inside-out box for anything that drew it. Normalised at parse time
/// onto the convention this crate already had — width zero, diameter in the length.
#[cfg(test)]
mod a_negative_ies_dimension_means_round {
    use super::*;

    /// The LM-63 header line is: num_lamps, lumens, multiplier, n_vert, n_horiz, photo, units,
    /// WIDTH, LENGTH, HEIGHT; then ballast, future, watts on the next line.
    fn ies(width: &str, length: &str, height: &str) -> String {
        format!(
            "IESNA:LM-63-1995\nTILT=NONE\n1 1000 1.0 2 1 1 2 {width} {length} {height}\n\
             1.0 1.0 100\n0.0 90.0\n0.0\n1000.0 10.0\n"
        )
    }

    #[test]
    fn a_negative_width_becomes_a_round_aperture() {
        let p = parse(&ies("-0.6", "-0.6", "0.0")).expect("parses");
        assert_eq!(p.width, 0.0, "round is marked by a zero width, not a negative one");
        assert!((p.length - 0.6).abs() < 1e-12, "the diameter is positive: {}", p.length);
        assert_eq!(p.aperture(), Some(Aperture::Round { d: 0.6 }));
        assert_eq!(p.housing_shape(), Some(Aperture::Round { d: 0.6 }));
        let a = p.projected_luminous_area(0.0).expect("declared");
        assert!((a - std::f64::consts::PI * 0.09).abs() < 1e-12, "area {a}");
    }

    /// One negative field is enough to mean circular, which is how real files write it.
    #[test]
    fn a_negative_length_alone_also_means_round() {
        let p = parse(&ies("0.0", "-0.25", "0.0")).expect("parses");
        assert_eq!(p.aperture(), Some(Aperture::Round { d: 0.25 }));
    }

    #[test]
    fn a_positive_header_is_still_rectangular() {
        let p = parse(&ies("0.3", "1.2", "0.08")).expect("parses");
        assert_eq!(p.aperture(), Some(Aperture::Rect { l: 1.2, w: 0.3 }));
        assert!((p.projected_luminous_area(0.0).unwrap() - 0.36).abs() < 1e-12);
        assert_eq!(p.housing(), Some((1.2, 0.3, 0.08)));
    }

    /// Height is normalised too — a negative one would hang the drawn body upwards.
    #[test]
    fn a_negative_height_is_taken_as_a_magnitude() {
        let p = parse(&ies("0.3", "1.2", "-0.08")).expect("parses");
        assert!((p.height - 0.08).abs() < 1e-12, "height must be positive, got {}", p.height);
    }

    /// The all-zero case stays "no dimensions declared" — the common one, and not an error.
    #[test]
    fn zeros_still_mean_nothing_declared() {
        let p = parse(&ies("0", "0", "0")).expect("parses");
        assert!(p.aperture().is_none());
        assert!(p.housing().is_none());
        assert!(p.projected_luminous_area(0.0).is_none());
    }
}

/// THE HEADER A SCHEDULE NEEDS, from LM-63's keywords.
#[cfg(test)]
mod the_keywords_reach_the_schedule {
    use super::*;

    const WITH_KEYWORDS: &str = "IESNA:LM-63-1995\n\
        [TEST] TR-1234\n\
        [MANUFAC] HSI Lighting\n\
        [LUMCAT] OG20-36\n\
        [LUMINAIRE] OCULUS GRANDE 2.0\n\
        [LAMP] LED 3000K CRI90\n\
        TILT=NONE\n\
        1 -1 1.0 2 1 1 2 0 0 0\n\
        1.0 1.0 100\n\
        0.0 90.0\n\
        0.0\n\
        1000.0 10.0\n";

    /// EACH KEYWORD LANDS IN ITS OWN FIELD. They are read in one pass with TILT, and a scan that
    /// matched the wrong prefix would put the test number where the manufacturer goes.
    #[test]
    fn manufacturer_catalogue_and_lamp_are_read() {
        let p = parse(WITH_KEYWORDS).expect("a valid IES file");
        assert_eq!(p.manufacturer, "HSI Lighting");
        assert_eq!(p.catalogue, "OG20-36");
        assert_eq!(p.name, "OCULUS GRANDE 2.0", "the luminaire name is still the name");
        assert_eq!(p.lamp, "LED 3000K CRI90");
        assert!(
            !p.manufacturer.contains("TR-1234"),
            "the test number leaked into the manufacturer",
        );
    }

    /// A FILE WITH NO KEYWORDS still parses, and leaves them empty rather than inventing any — the
    /// report shows a dash, which is that file's omission.
    #[test]
    fn a_file_without_keywords_leaves_them_empty() {
        const BARE: &str = "IESNA:LM-63-1995
TILT=NONE
1 -1 1.0 2 1 1 2 0 0 0
\n            1.0 1.0 100
0.0 90.0
0.0
1000.0 10.0
";
        let p = parse(BARE).expect("the bare sample still parses");
        assert!(p.manufacturer.is_empty());
        assert!(p.catalogue.is_empty());
        assert!(p.lamp.is_empty());
        assert_eq!(p.name, "Luminaire", "and the name still falls back");
    }
}
