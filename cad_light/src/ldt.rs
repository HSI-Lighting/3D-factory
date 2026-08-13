//! EULUMDAT (`.ldt`) photometric file support.
//!
//! The European counterpart to IES LM-63, and what most manufacturers outside North America
//! publish. A fixed-order list of values, one per line, with no keys — so a miscount early on
//! silently shifts every later field, which is why this validates as it goes rather than trusting
//! the file to be well-formed.
//!
//! It parses into [`IesProfile`], the engine's existing type, so a luminaire behaves identically
//! whichever format it came from. Two things have to be reconciled to do that:
//!
//!   * **Units.** EULUMDAT stores candela **per 1000 lumens**; IES stores absolute candela. The
//!     table is therefore scaled by the lamp set's total flux / 1000. Skip that and a 4000 lm
//!     fitting reads as a 1000 lm one — every result 4× too dark, with nothing on screen to say
//!     so.
//!   * **Symmetry.** A symmetric file stores only the C-planes it needs — a quarter, a half, or a
//!     single plane — and the reader is expected to mirror the rest. Expanding here means
//!     [`IesProfile::intensity`] needs no idea any of this happened.

use crate::ies::{IesProfile, PhotometryType};

/// EULUMDAT symmetry indicator (`Isym`, record 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symmetry {
    /// 0 — none; every C-plane is stored.
    None,
    /// 1 — about the vertical axis; one plane stands for all of them.
    Vertical,
    /// 2 — to the C0–C180 plane; the file holds C0..C180.
    C0C180,
    /// 3 — to the C90–C270 plane; the file holds C90..C270.
    C90C270,
    /// 4 — to both; the file holds one quadrant.
    Quadrant,
}

impl Symmetry {
    fn from_code(c: i32) -> Result<Self, String> {
        Ok(match c {
            0 => Symmetry::None,
            1 => Symmetry::Vertical,
            2 => Symmetry::C0C180,
            3 => Symmetry::C90C270,
            4 => Symmetry::Quadrant,
            other => return Err(format!("unknown EULUMDAT symmetry Isym={other} (expected 0..4)")),
        })
    }

    /// How many C-planes the FILE stores, given `mc` planes in the full circle.
    fn stored_planes(self, mc: usize) -> usize {
        match self {
            Symmetry::None => mc,
            Symmetry::Vertical => 1,
            Symmetry::C0C180 | Symmetry::C90C270 => mc / 2 + 1,
            Symmetry::Quadrant => mc / 4 + 1,
        }
    }
}

/// Parse EULUMDAT text into the engine's profile type.
pub fn parse(contents: &str) -> Result<IesProfile, String> {
    // A fixed-order format: the line INDEX is the field name. Blank trailing lines are common and
    // harmless; blank lines in the middle are not, and would shift everything after them.
    let lines: Vec<&str> = contents.lines().map(|l| l.trim()).collect();
    if lines.len() < 42 {
        return Err(format!("not EULUMDAT: {} lines, expected at least 42", lines.len()));
    }

    let at = |i: usize| -> &str { lines.get(i).copied().unwrap_or("") };
    let num = |i: usize, what: &str| -> Result<f64, String> {
        let s = at(i);
        // Some writers use a comma decimal separator.
        s.replace(',', ".")
            .parse::<f64>()
            .map_err(|_| format!("line {} ({what}): expected a number, found {s:?}", i + 1))
    };
    let int = |i: usize, what: &str| -> Result<i32, String> {
        num(i, what).map(|v| v.round() as i32)
    };

    let isym = Symmetry::from_code(int(2, "Isym")?)?;
    let mc = int(3, "Mc — number of C-planes")?;
    let ng = int(5, "Ng — values per C-plane")?;
    if mc <= 0 || ng <= 0 {
        return Err(format!("Mc={mc}, Ng={ng}: both must be positive"));
    }
    let (mc, ng) = (mc as usize, ng as usize);

    let name = {
        let n = at(8);
        if n.is_empty() { at(7).to_string() } else { n.to_string() }
    };
    // Records 13–15: overall size in MILLIMETRES. The engine works in metres.
    let width_mm = num(13, "luminaire width").unwrap_or(0.0);
    let length_mm = num(12, "luminaire length/diameter").unwrap_or(0.0);
    let height_mm = num(14, "luminaire height").unwrap_or(0.0);

    // Records 16–17: the LUMINOUS AREA — the emitting aperture, which is NOT the housing.
    //
    // Glare is computed from luminance, L = I / A, so this is the area that matters. A 600 mm
    // housing with a 300 mm aperture is four times brighter than its outline suggests, and using
    // the outline UNDER-states luminance and therefore under-states UGR — the direction that
    // passes a design which should fail.
    //
    // A file leaving these at zero (common) means "same as the outline", so fall back to it rather
    // than to nothing: an area of zero would divide by zero downstream.
    let lum_len_mm = num(15, "luminous area length/diameter").unwrap_or(0.0);
    let lum_wid_mm = num(16, "luminous area width").unwrap_or(0.0);
    let (lum_len_mm, lum_wid_mm) =
        if lum_len_mm > 0.0 { (lum_len_mm, lum_wid_mm) } else { (length_mm, width_mm) };

    // Record 26 is the number of lamp sets; each set then occupies 6 lines, and record 27's direct
    // ratios (10 values) follow. Everything after that is angles and intensities, so getting this
    // count wrong shifts the entire distribution.
    let n_lamp_sets = int(25, "number of lamp sets")?.max(1) as usize;
    // The sets are ALTERNATIVES, not simultaneous lamps.
    //
    // A EULUMDAT file describes one luminaire that can be supplied with any of `n` standard lamp
    // configurations; the buyer picks one. They must not be added together — and this reader did
    // add them, which inflated both the flux and, because the candela table is stored per
    // kilolumen, every intensity in the distribution along with it.
    //
    // Caught by comparison with a DIALux report on a real project: `FONDO.ldt` carries three sets —
    // 4000 lm/40 W, 3000 lm/30 W, 4000 lm/40 W. This reader reported 11000 lm and 110 W; DIALux
    // reported 4000 lm and 40 W, and DIALux is right. The scheme's light from that fitting was
    // therefore overstated by 2.75x.
    //
    // The first set is taken, which is what DIALux does and what a file's author means by listing
    // it first. Offering the choice is the fuller answer and is worth doing once anything needs it.
    let base = 26;
    let flux_lm = num(base + 2, "total luminous flux").unwrap_or(0.0).abs();
    let watts = num(base + 5, "wattage").unwrap_or(0.0);
    if flux_lm <= 0.0 {
        return Err("total luminous flux is zero — cannot convert cd/klm to candela".to_string());
    }

    // 10 direct-ratio values, then the C angles, then the G angles, then the intensities.
    let angles_start = 26 + n_lamp_sets * 6 + 10;
    let read_block = |from: usize, count: usize, what: &str| -> Result<Vec<f64>, String> {
        let mut v = Vec::with_capacity(count);
        for k in 0..count {
            v.push(num(from + k, what)?);
        }
        Ok(v)
    };

    let c_stored = isym.stored_planes(mc);
    let c_angles = read_block(angles_start, mc, "C-plane angle")?;
    let g_angles = read_block(angles_start + mc, ng, "gamma angle")?;
    let i_start = angles_start + mc + ng;
    let need = c_stored * ng;
    if lines.len() < i_start + need {
        return Err(format!(
            "file ends early: {} intensity values needed for {c_stored} stored C-plane(s) x {ng} \
             gamma angles, only {} lines left",
            need,
            lines.len().saturating_sub(i_start)
        ));
    }
    let raw = read_block(i_start, need, "luminous intensity")?;

    // cd/klm -> cd. The one conversion that silently scales every result if missed.
    let scale = flux_lm / 1000.0;

    // Expand symmetry into the full set of C-planes, so nothing downstream needs to know.
    let mut candela: Vec<Vec<f64>> = Vec::with_capacity(mc);
    for (ci, &c) in c_angles.iter().enumerate() {
        let src = mirror_index(isym, ci, mc, c_stored, c);
        let row: Vec<f64> = (0..ng).map(|g| raw[src * ng + g] * scale).collect();
        candela.push(row);
    }

    Ok(IesProfile {
        name,
        // EULUMDAT's C/gamma goniometry is IES type C.
        photometry: PhotometryType::C,
        lumens: flux_lm,
        multiplier: 1.0,
        vertical_angles: g_angles,
        horizontal_angles: c_angles,
        candela,
        watts,
        width: width_mm / 1000.0,
        luminous_length: lum_len_mm / 1000.0,
        luminous_width: lum_wid_mm / 1000.0,
        length: length_mm / 1000.0,
        height: height_mm / 1000.0,
    })
}

/// Which STORED plane supplies C-plane `ci`, mirroring per the symmetry.
///
/// Worked in plane INDICES rather than angles: the C angles are evenly spaced by construction
/// (`Dc` = 360/Mc), and indices make the reflection exact instead of a float comparison that has
/// to guess a tolerance.
fn mirror_index(isym: Symmetry, ci: usize, mc: usize, stored: usize, _angle: f64) -> usize {
    let idx = match isym {
        Symmetry::None => ci,
        Symmetry::Vertical => 0,
        // Stored C0..C180; C180..C360 mirrors back down.
        Symmetry::C0C180 => {
            if ci < stored { ci } else { mc - ci }
        }
        // Stored C90..C270. Shift so the stored block starts at 0, then mirror.
        Symmetry::C90C270 => {
            let q = mc / 4; // index of C90
            let rel = (ci + mc - q) % mc;
            if rel < stored { rel } else { mc - rel }
        }
        // Stored one quadrant, C0..C90: mirror into each of the other three.
        Symmetry::Quadrant => {
            let q = mc / 4;
            let r = ci % mc;
            if r <= q { r }
            else if r <= 2 * q { 2 * q - r }
            else if r <= 3 * q { r - 2 * q }
            else { mc - r }
        }
    };
    idx.min(stored.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but complete EULUMDAT file: axially symmetric, 24 C-planes, 3 gamma angles.
    fn minimal_ldt(isym: i32, mc: usize, ng: usize, flux: f64, intensities: &[f64]) -> String {
        let mut l: Vec<String> = Vec::new();
        l.push("Test Manufacturer".into());   // 1  company
        l.push("1".into());                   // 2  Ityp
        l.push(isym.to_string());             // 3  Isym
        l.push(mc.to_string());               // 4  Mc
        l.push((360.0 / mc as f64).to_string()); // 5 Dc
        l.push(ng.to_string());               // 6  Ng
        l.push((90.0 / (ng - 1) as f64).to_string()); // 7 Dg
        l.push("REPORT-1".into());            // 8  measurement report
        l.push("Test Luminaire".into());      // 9  luminaire name
        l.push("LUM-001".into());             // 10 luminaire number
        l.push("test.ldt".into());            // 11 file name
        l.push("2026-08-10".into());          // 12 date/user
        l.push("600".into());                 // 13 length/diameter mm
        l.push("300".into());                 // 14 width mm
        l.push("80".into());                  // 15 height mm
        for _ in 0..6 { l.push("0".into()); } // 16-21 luminous area
        l.push("100".into());                 // 22 DFF %
        l.push("100".into());                 // 23 LORL %
        l.push("1".into());                   // 24 conversion factor
        l.push("0".into());                   // 25 tilt
        l.push("1".into());                   // 26 number of lamp sets
        l.push("1".into());                   // 26a number of lamps
        l.push("LED".into());                 // 26b type
        l.push(flux.to_string());             // 26c total flux lm
        l.push("4000".into());                // 26d colour temp
        l.push("80".into());                  // 26e CRI
        l.push("36".into());                  // 26f wattage
        for _ in 0..10 { l.push("0".into()); } // 27 direct ratios
        for i in 0..mc { l.push((i as f64 * 360.0 / mc as f64).to_string()); }
        for i in 0..ng { l.push((i as f64 * 90.0 / (ng - 1) as f64).to_string()); }
        for v in intensities { l.push(v.to_string()); }
        l.join("\r\n")
    }

    /// cd/klm becomes candela. This is THE conversion: miss it and a 4000 lm fitting is read as a
    /// 1000 lm one, so every lux figure comes out 4x too low with nothing on screen to say so.
    #[test]
    fn intensities_are_scaled_from_per_kilolumen_to_candela() {
        // Axially symmetric: one stored plane of 3 values.
        let text = minimal_ldt(1, 24, 3, 4000.0, &[100.0, 50.0, 0.0]);
        let p = parse(&text).expect("parses");
        assert_eq!(p.lumens, 4000.0);
        // 100 cd/klm at 4000 lm = 400 cd.
        assert!((p.intensity(0.0, 0.0) - 400.0).abs() < 1e-6,
            "nadir should be 400 cd, got {}", p.intensity(0.0, 0.0));
        assert!((p.intensity(45.0, 0.0) - 200.0).abs() < 1e-6,
            "45 deg should be 200 cd, got {}", p.intensity(45.0, 0.0));
    }

    /// Axial symmetry stores ONE plane and it must apply in every direction.
    #[test]
    fn axial_symmetry_expands_to_every_c_plane() {
        let text = minimal_ldt(1, 24, 3, 1000.0, &[100.0, 50.0, 0.0]);
        let p = parse(&text).expect("parses");
        assert_eq!(p.horizontal_angles.len(), 24, "all 24 C-planes must be present");
        for c in [0.0, 90.0, 180.0, 270.0] {
            assert!((p.intensity(0.0, c) - 100.0).abs() < 1e-6,
                "C{c} should match C0 under axial symmetry, got {}", p.intensity(0.0, c));
        }
    }

    /// C0-C180 symmetry: the file holds half, and the other half is its mirror.
    #[test]
    fn half_symmetry_mirrors_the_stored_planes() {
        // 4 C-planes (0/90/180/270) => stored = 4/2 + 1 = 3 planes, 2 gamma angles each.
        let text = minimal_ldt(2, 4, 2, 1000.0, &[
            10.0, 1.0, // C0
            20.0, 2.0, // C90
            30.0, 3.0, // C180
        ]);
        let p = parse(&text).expect("parses");
        assert!((p.intensity(0.0, 0.0) - 10.0).abs() < 1e-6);
        assert!((p.intensity(0.0, 90.0) - 20.0).abs() < 1e-6);
        assert!((p.intensity(0.0, 180.0) - 30.0).abs() < 1e-6);
        // C270 is C90 mirrored across C0-C180.
        assert!((p.intensity(0.0, 270.0) - 20.0).abs() < 1e-6,
            "C270 must mirror C90, got {}", p.intensity(0.0, 270.0));
    }

    /// Millimetres become metres — the engine measures the room in metres, and an 0.6 m fitting
    /// entered as 600 would be a luminaire the size of the building.
    #[test]
    fn dimensions_convert_to_metres() {
        let p = parse(&minimal_ldt(1, 24, 3, 1000.0, &[100.0, 50.0, 0.0])).expect("parses");
        assert!((p.length - 0.6).abs() < 1e-6, "600 mm should be 0.6 m, got {}", p.length);
        assert!((p.width - 0.3).abs() < 1e-6, "300 mm should be 0.3 m, got {}", p.width);
        assert!((p.height - 0.08).abs() < 1e-6, "80 mm should be 0.08 m, got {}", p.height);
    }

    /// A file that is too short is REJECTED, not read into nonsense. A fixed-order format has no
    /// keys, so a truncated file otherwise yields a plausible-looking profile built from whatever
    /// happened to be on those lines.
    #[test]
    fn a_truncated_file_is_rejected() {
        assert!(parse("Company\n1\n0\n24\n15\n").is_err(), "5 lines is not a EULUMDAT file");
        // Complete header, but the intensity block is cut short.
        let full = minimal_ldt(1, 24, 3, 1000.0, &[100.0, 50.0, 0.0]);
        let cut = full.lines().take(full.lines().count() - 2).collect::<Vec<_>>().join("\r\n");
        let err = parse(&cut).unwrap_err();
        assert!(err.contains("ends early"), "expected a truncation error, got: {err}");
    }

    /// Zero flux cannot be converted, and saying so beats returning a profile of all zeros that
    /// looks like a luminaire which emits nothing.
    #[test]
    fn zero_flux_is_an_error_not_a_dark_luminaire() {
        let text = minimal_ldt(1, 24, 3, 0.0, &[100.0, 50.0, 0.0]);
        let err = parse(&text).unwrap_err();
        assert!(err.contains("flux"), "expected a flux error, got: {err}");
    }
}

/// Parse every `.ldt` in a folder and print what came out. Set `LDT_DIR`.
///
///   LDT_DIR="D:/.../LDT" cargo test -p cad_light ldt_probe_real_files -- --ignored --nocapture
///
/// Synthetic fixtures only prove the parser agrees with the fixture writer — which is the same
/// person, holding the same misunderstanding. Real manufacturer files are the independent check:
/// they carry symmetries, lamp-set counts and decimal conventions nobody would think to invent.
#[test]
#[ignore = "needs LDT_DIR=<folder of .ldt files>"]
fn ldt_probe_real_files() {
    let Ok(dir) = std::env::var("LDT_DIR") else {
        println!("set LDT_DIR to a folder of .ldt files");
        return;
    };
    let mut n_ok = 0;
    let mut n_bad = 0;
    for e in std::fs::read_dir(&dir).expect("read LDT_DIR").flatten() {
        let p = e.path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ldt")) {
            continue;
        }
        // Manufacturer files are frequently Latin-1, not UTF-8 (degree signs, umlauts).
        let bytes = std::fs::read(&p).expect("read file");
        let text: String = bytes.iter().map(|&b| b as char).collect();
        let stem = p.file_name().unwrap().to_string_lossy().to_string();
        match parse(&text) {
            Ok(prof) => {
                n_ok += 1;
                println!(
                    "OK   {stem}\n     {} lm, {:.0} W, peak {:.0} cd, {}x{} angles, {:.2}x{:.2}x{:.2} m",
                    prof.lumens, prof.watts, prof.peak_candela(),
                    prof.horizontal_angles.len(), prof.vertical_angles.len(),
                    prof.length, prof.width, prof.height,
                );
                // A downlight's peak should be at or near nadir, and the numbers should be
                // physically sane — this is what catches a symmetry or scaling mistake that a
                // clean parse would otherwise hide.
                let nadir = prof.intensity(0.0, 0.0);
                println!("     nadir {nadir:.0} cd");
            }
            Err(e) => {
                n_bad += 1;
                println!("FAIL {stem}\n     {e}");
            }
        }
    }
    println!("\n{n_ok} parsed, {n_bad} failed");
}
