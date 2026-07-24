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
        if let Some(rest) = line.strip_prefix("[LUMINAIRE]") {
            name = rest.trim().to_string();
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

    Ok(IesProfile {
        name,
        photometry,
        lumens,
        multiplier,
        vertical_angles,
        horizontal_angles,
        candela,
        watts,
        width: width * to_m,
        length: length * to_m,
        height: height * to_m,
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
