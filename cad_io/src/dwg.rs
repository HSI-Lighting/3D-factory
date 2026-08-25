//! DWG, read natively — no AutoCAD, no converter, no extra files.
//!
//! Asked for as: *"cant our installation file have just the required files so dwg can open for the
//! user without any hassel."* The answer turned out better than the question: it needs no extra
//! files at all. This compiles into `simlux.exe`.
//!
//! WHY IT USED TO NEED AUTOCAD. DWG is closed, versioned and undocumented, so every package that
//! opens one either licenses Autodesk's RealDWG or the Open Design Alliance's library, or uses a
//! clean-room reader. SIMLUX shelled out to `accoreconsole.exe` — AutoCAD's own headless core,
//! exactly as correct as AutoCAD and exactly as available. On a machine without AutoCAD there was
//! nothing to drive, and the user got an exit code.
//!
//! `acadrust` is a clean-room reader under **MPL-2.0** — file-level copyleft, so it links into a
//! proprietary binary and only its own files carry the obligation. That licence is what makes this
//! possible: LibreDWG, the obvious alternative, is GPL-3.0 and would take the whole application
//! with it.
//!
//! MEASURED ON REAL DRAWINGS BEFORE A LINE OF THIS WAS WRITTEN, because "a pure Rust DWG reader" is
//! a claim and the versions in the wild are the test:
//!
//! ```text
//!   for3dfactorygym.dwg             AC1032    2 336 entities      5 ms    156 KB
//!   FOR LUX - NEW.dwg               AC1032    6 577 entities     11 ms    344 KB
//!   floor plan o2 first floor.dwg   AC1027    6 577 entities     11 ms    341 KB
//!   1 Mashrabiya.dwg                AC1032    2 133 entities     70 ms    1.3 MB
//!   villa mashrabya.dwg             AC1027    5 920 entities     94 ms    3.0 MB
//!   FOR DIALUX.dwg                  AC1032   65 790 entities   6 841 ms     93 MB
//! ```
//!
//! THE CONVERTER STAYS as the fallback. This reader is new and DWG has thirty years of versions;
//! where it cannot read a file AutoCAD still can, and a user who has AutoCAD should not lose that
//! because we added something. The fallback costs nothing when it is not used.

use cad_kernel::{
    geom::{Arc, Circle, Ellipse, Line, Point, PolyVertex, Polyline},
    Color, DObject, Document, Geom, Vec2,
};

/// What a read produced, beside the document: what was kept and what was not.
///
/// Reported rather than inferred from a count, because "2336 entities" and "2336 kept" look
/// identical in a log and mean very different things. The caller prints it on open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub kept: usize,
    pub skipped: usize,
}

/// Read a `.dwg` into a `Document`.
///
/// Entities map to the same `Geom` variants the DXF reader produces, so nothing downstream —
/// promotion, snapping, the lux engine — can tell which door a drawing came in through.
pub fn read_dwg(path: &std::path::Path) -> Result<(Document, Tally), String> {
    let mut rd = acadrust::DwgReader::from_file(path).map_err(|e| format!("open dwg: {e}"))?;
    let src = rd.read().map_err(|e| format!("read dwg: {e}"))?;
    Ok(convert(&src))
}

/// The mapping itself, separated from the file so it can be tested against a document built in
/// memory rather than only against drawings on somebody's disk.
pub fn convert(src: &acadrust::CadDocument) -> (Document, Tally) {
    let mut doc = Document::default();

    // ---- units, from the drawing's own header -----------------------------------------------
    //
    // TAKEN, NOT ASSUMED. A metre drawing read as millimetres puts every fitting at a thousandth
    // of its position — that cost a full session on this project, and the declaration was in the
    // file the whole time. Same mapping and the same two refusals as the DXF side: 0 is UNITLESS,
    // an explicit absence of a claim rather than a claim of metres, and an exotic code is left
    // unmapped because a wrong guess beats no guess only if it is right.
    if let Some(m) = insunits_to_metres(src.header.insertion_units as i32) {
        doc.units = cad_kernel::DocUnits::new(m, cad_kernel::UnitSource::Declared);
    }

    // ---- layers, before the entities that name them -----------------------------------------
    for l in src.layers.iter() {
        let name = l.name.trim();
        if name.is_empty() || doc.layers.find(name).is_some() {
            continue;
        }
        let color = match l.color.index() {
            Some(i) if (1..=255).contains(&i) => Color::Aci(i as u8),
            _ => Color::Aci(7),
        };
        doc.layers.add(cad_kernel::Layer {
            name: name.to_string(),
            color,
            linetype: cad_kernel::LinetypeTable::CONTINUOUS,
            lineweight: cad_kernel::Lineweight::Default,
            visible: !l.is_off(),
            locked: l.is_locked(),
            frozen: l.is_frozen(),
            plottable: true,
        });
    }

    let mut tally = Tally::default();
    for e in src.entities() {
        match build(e, &doc) {
            Some(d) => {
                doc.push(d);
                tally.kept += 1;
            }
            None => tally.skipped += 1,
        }
    }
    (doc, tally)
}

/// One entity, or `None` where there is nothing faithful to make of it.
fn build(e: &acadrust::EntityType, doc: &Document) -> Option<DObject> {
    use acadrust::EntityType as E;
    let geom = match e {
        E::Line(l) => {
            Geom::Line(Line { a: Vec2::new(l.start.x, l.start.y), b: Vec2::new(l.end.x, l.end.y) })
        }
        E::Circle(c) => {
            Geom::Circle(Circle { center: Vec2::new(c.center.x, c.center.y), radius: c.radius })
        }
        E::Arc(a) => {
            // DWG STORES THESE IN RADIANS, unlike DXF's degrees — the single easiest thing to get
            // wrong here, and it would produce arcs that look plausible and are not.
            //
            // A sweep of exactly zero means a full circle rather than an empty arc, the same rule
            // the DXF reader follows.
            let sweep = (a.end_angle - a.start_angle).rem_euclid(std::f64::consts::TAU);
            let sweep = if sweep < 1e-9 { std::f64::consts::TAU } else { sweep };
            Geom::Arc(Arc {
                center: Vec2::new(a.center.x, a.center.y),
                radius: a.radius,
                start_angle: a.start_angle.rem_euclid(std::f64::consts::TAU),
                sweep_angle: sweep,
            })
        }
        // The major axis is a VECTOR from the centre and the minor is a ratio of it — which is
        // exactly how `cad_kernel::Ellipse` is shaped, so this is a copy rather than a conversion.
        E::Ellipse(el) => Geom::Ellipse(Ellipse {
            center: Vec2::new(el.center.x, el.center.y),
            major: Vec2::new(el.major_axis.x, el.major_axis.y),
            ratio: el.minor_axis_ratio,
        }),
        E::LwPolyline(p) => Geom::Polyline(Polyline {
            vertices: p
                .vertices
                .iter()
                .map(|v| PolyVertex { pos: Vec2::new(v.location.x, v.location.y), bulge: v.bulge })
                .collect(),
            closed: p.is_closed,
            widths: Vec::new(),
        }),
        E::Polyline2D(p) => Geom::Polyline(Polyline {
            vertices: p
                .vertices
                .iter()
                .map(|v| PolyVertex { pos: Vec2::new(v.location.x, v.location.y), bulge: v.bulge })
                .collect(),
            closed: p.is_closed(),
            widths: Vec::new(),
        }),
        E::Point(p) => Geom::Point(Point {
            location: Vec2::new(p.location.x, p.location.y),
            style: 0,
            size: 0.0,
        }),
        // EVERYTHING ELSE IS SKIPPED, DELIBERATELY, AND COUNTED. Text, dimensions, hatches, 3D
        // solids and block references each have an honest representation here and each needs its
        // own care — a block reference in particular resolves against a table this does not yet
        // carry, and inserting one as loose geometry would silently change what the drawing says.
        // Dropping them is visible in the open log; approximating them would not be.
        _ => return None,
    };

    let mut style = cad_kernel::Style::default();
    if let Some(c) = common_of(e) {
        if let Some(lid) = doc.layers.find(c.layer.trim()) {
            style.layer = lid;
        }
        // BYLAYER IS THE DEFAULT AND IS LEFT ALONE. `Color::index()` gives `None` for a true
        // colour and 256 for ByLayer; resolving either here would bake a colour the layer is
        // supposed to own, so a drawing recoloured by layer afterwards would not follow.
        match c.color.index() {
            Some(i) if (1..=255).contains(&i) => style.color = Color::Aci(i as u8),
            _ => {}
        }
        style.visible = !c.invisible;
    }
    Some(DObject::with_style(geom, style))
}

fn common_of(e: &acadrust::EntityType) -> Option<&acadrust::entities::EntityCommon> {
    use acadrust::EntityType as E;
    Some(match e {
        E::Line(x) => &x.common,
        E::Circle(x) => &x.common,
        E::Arc(x) => &x.common,
        E::Ellipse(x) => &x.common,
        E::LwPolyline(x) => &x.common,
        E::Polyline2D(x) => &x.common,
        E::Point(x) => &x.common,
        _ => return None,
    })
}

/// `$INSUNITS` → metres per drawing unit.
///
/// Mirrors `dxf::insunits_to_metres` exactly, including its two refusals. Kept as its own copy
/// rather than shared, because the two readers are allowed to diverge on a format quirk and a
/// shared helper would hide that; if they ever disagree, the test below says so.
fn insunits_to_metres(code: i32) -> Option<f64> {
    Some(match code {
        1 => 0.0254,  // inches
        2 => 0.3048,  // feet
        4 => 0.001,   // millimetres
        5 => 0.01,    // centimetres
        6 => 1.0,     // metres
        10 => 0.9144, // yards
        14 => 0.1,    // decimetres
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE TWO READERS MUST AGREE ON UNITS. A drawing that means one thing as DXF and another as
    /// DWG is the same class of bug as the declaration that read metres as millimetres — and this
    /// file deliberately keeps its own copy of the table, so something has to hold them together.
    #[test]
    fn the_unit_table_matches_the_dxf_readers() {
        for code in [0, 1, 2, 3, 4, 5, 6, 7, 10, 14, 20, 100] {
            assert_eq!(
                insunits_to_metres(code),
                crate::dxf::insunits_to_metres_for_test(code),
                "code {code} means different things to the DXF and DWG readers",
            );
        }
    }

    /// UNITLESS IS NOT METRES. `$INSUNITS = 0` is an explicit absence of a claim; adopting 1.0 for
    /// it would turn "this drawing does not say" into "this drawing says metres".
    #[test]
    fn unitless_stays_unclaimed() {
        assert_eq!(insunits_to_metres(0), None);
        assert_eq!(insunits_to_metres(6), Some(1.0), "…but a real claim of metres is taken");
    }
}

/// THE OWNER'S OWN DRAWINGS, read through the integrated path.
///
/// `#[ignore]`d because they live on a Dropbox share this repo cannot assume, but kept in the tree
/// because they are the only test that matters for a format reader: DWG has thirty years of
/// versions and a synthetic file proves nothing about the ones people actually have.
///
///     cargo test -p cad_io the_real_drawings -- --ignored --nocapture
#[cfg(test)]
mod the_real_drawings {
    const DIR: &str =
        r"D:\Dropbox\03--PROJECTS\03-PROJECTS\2026\SAEEDA MAM\SAEEDA MA'AM'S PROJECT-02\02";

    /// Every one of them opens, and every one yields geometry. A reader that returns an empty
    /// document without erroring is the worst outcome: the app would show a blank canvas and
    /// report success.
    #[test]
    #[ignore]
    fn every_drawing_opens_with_geometry() {
        let files = [
            "for3dfactorygym.dwg",
            "FOR LUX - NEW.dwg",
            "floor plan o2 first floor.dwg",
            "1 Mashrabiya.dwg",
            "villa mashrabya.dwg",
            "FOR DIALUX.dwg",
        ];
        for f in files {
            let p = std::path::Path::new(DIR).join(f);
            if !p.exists() {
                eprintln!("{f:32} SKIPPED (not on this machine)");
                continue;
            }
            let t = std::time::Instant::now();
            let (doc, tally) = super::read_dwg(&p).unwrap_or_else(|e| panic!("{f}: {e}"));
            eprintln!(
                "{f:32} {:>6} kept {:>5} skipped {:>4} layers  {:>5} ms  unit={:?}",
                tally.kept,
                tally.skipped,
                doc.layers.len(),
                t.elapsed().as_millis(),
                doc.units.metres_per_unit,
            );
            assert!(tally.kept > 0, "{f} produced no geometry — a blank canvas reported as success");
            assert_eq!(tally.kept, doc.dobjects.len(), "{f}: the tally must match what was pushed");
        }
    }

    /// THE DRAWING IS BUILDING-SIZED IN ITS OWN DECLARED UNIT — which is the invariant that
    /// actually matters, and the one a reader with the axes or the scale wrong would fail while
    /// still "opening" and still reporting entities.
    ///
    /// The first version of this asserted the gym plan sits at x ≈ 3500 on survey coordinates,
    /// which is where the DXF sits — the DWG is a different file: millimetres, near the origin,
    /// 36 x 25 m. Asserting one file's coordinates of another is how a test ends up describing the
    /// author's assumption rather than the software.
    #[test]
    #[ignore]
    fn every_drawing_is_building_sized_in_its_own_unit() {
        for f in ["for3dfactorygym.dwg", "villa mashrabya.dwg", "1 Mashrabiya.dwg"] {
        let p = std::path::Path::new(DIR).join(f);
        if !p.exists() {
            continue;
        }
        let (doc, _) = super::read_dwg(&p).expect("opens");
        let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for d in &doc.dobjects {
            let (a, b) = d.geom.bbox();
            lo = (lo.0.min(a.x), lo.1.min(a.y));
            hi = (hi.0.max(b.x), hi.1.max(b.y));
        }
        let k = doc.units.metres_per_unit;
        let (w, d) = ((hi.0 - lo.0) * k, (hi.1 - lo.1) * k);
        eprintln!("{f:32} {w:.1} x {d:.1} m  (unit {k})");
        assert!(
            (1.0..5_000.0).contains(&w) && (1.0..5_000.0).contains(&d),
            "{f} reads as {w:.1} x {d:.1} m, which is not a building -- the unit or the \n             coordinates came through wrong",
        );
        }
    }
}
