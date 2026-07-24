// cad_snap basic usage example.
//
// Demonstrates the public API for every snap kind, the priority cascade,
// the typed override, and the Tab-cycling list. No UI, no rendering —
// just calls into the engine and prints the SnapHit values so you can
// audit the math line-by-line.
//
//     cargo run --example basic -p cad_snap

use cad_snap::{
    find_all_snaps, find_snap, Arc, Circle, Ellipse, DObject, Line, SnapHit,
    SnapKind, SnapSet, Vec2,
};

fn header(title: &str) {
    println!("\n=== {title} ===");
}

fn show(label: &str, hit: Option<SnapHit>) {
    match hit {
        Some(h) => {
            let ext = h.extension_anchor
                .map(|a| format!(" ext=({:.3},{:.3})", a.x, a.y))
                .unwrap_or_default();
            println!(
                "  {label:<32} → {:>3} at ({:>7.3}, {:>7.3}){ext}",
                h.kind.name(), h.point.x, h.point.y
            );
        }
        None => println!("  {label:<32} → (no snap)"),
    }
}

fn main() {
    // -----------------------------------------------------------------
    // Build a small scene: a horizontal line, a circle, and a quarter-arc.
    // -----------------------------------------------------------------
    // Each `<shape> { … }.into()` wraps the geometry into a DObject with
    // default style — exactly what an ad-hoc test needs.
    let dobjects: Vec<DObject> = vec![
        Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(10.0, 0.0),
        }.into(),
        Circle {
            center: Vec2::new(20.0, 0.0),
            radius: 5.0,
        }.into(),
        Arc {
            center: Vec2::new(0.0, 10.0),
            radius: 5.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        }.into(),
        // Axis-aligned ellipse: a=6, b=3, centred at (40, 0).
        Ellipse {
            center: Vec2::new(40.0, 0.0),
            major:  Vec2::new(6.0, 0.0),
            ratio:  0.5,
        }.into(),
    ];

    // A "running osnaps" set — like ticking checkboxes in a UI panel.
    // `SnapSet::defaults()` is the AutoCAD-style starter pack (END+MID+CEN+QUA);
    // `SnapSet::default()` gives you everything off so you can build up.
    let mut osnaps = SnapSet::defaults();
    println!("Running osnaps: END={}  MID={}  CEN={}  QUA={}  INT={}  PER={}  TAN={}  NEA={}",
        osnaps.end, osnaps.mid, osnaps.cen, osnaps.qua,
        osnaps.int, osnaps.per, osnaps.tan, osnaps.nea);

    // -----------------------------------------------------------------
    // Default-priority hits — END/MID at exact targets, CEN whenever the
    // cursor is on the circle's curve (because CEN's activation rule is
    // "cursor on dobject", it fires from anywhere on the rim).
    // -----------------------------------------------------------------
    header("Default priority cascade");
    show("near left endpoint of line",
        find_snap(Vec2::new(0.05, 0.05), 1.0, osnaps, None, None, &dobjects, None));
    show("near midpoint of line",
        find_snap(Vec2::new(5.05, 0.05), 1.0, osnaps, None, None, &dobjects, None));
    show("on circle's curve",
        find_snap(Vec2::new(23.535, 3.535), 1.0, osnaps, None, None, &dobjects, None));

    // -----------------------------------------------------------------
    // QUA on its own (CEN disabled so it doesn't outprioritise QUA).
    // -----------------------------------------------------------------
    header("QUA in isolation");
    let mut qua_only = SnapSet::default();
    qua_only.qua = true;
    show("right at the east quadrant (25, 0)",
        find_snap(Vec2::new(25.05, 0.05), 1.0, qua_only, None, None, &dobjects, None));
    show("on the rim but not at a quadrant",
        find_snap(Vec2::new(23.535, 3.535), 1.0, qua_only, None, None, &dobjects, None));

    // -----------------------------------------------------------------
    // Typed override: user typed `cen` — only CEN considered for next click.
    // -----------------------------------------------------------------
    header("Typed override (`cen`)");
    show("hover circle's curve, forced CEN",
        find_snap(Vec2::new(25.0, 0.0), 1.0, osnaps,
            Some(SnapKind::Cen), None, &dobjects, None));

    // -----------------------------------------------------------------
    // PER snap — requires the "from" anchor (the first click of a line draw).
    // The foot lands past the segment's right endpoint → imaginary extension.
    // Cursor placed at the foot location, well away from the circle so PER
    // picks the LINE dobject (not the circle which is at x=20).
    // -----------------------------------------------------------------
    osnaps.per = true;
    osnaps.cen = false;   // turn off CEN so PER doesn't compete on the circle
    osnaps.qua = false;
    header("PER with anchor → imaginary extension on the line");
    let anchor = Vec2::new(15.0, 5.0);
    show("cursor at the extension foot (15, 0)",
        find_snap(Vec2::new(15.0, 0.05), 1.0, osnaps,
            None, Some(anchor), &dobjects, None));

    // -----------------------------------------------------------------
    // Ellipse: QUA snaps to the four axis-end points, CEN to the centre.
    // For an axis-aligned ellipse with a=6 these are at (±6, 0) and (0, ±3).
    // -----------------------------------------------------------------
    header("Ellipse snaps");
    let mut ellipse_set = SnapSet::default();
    ellipse_set.qua = true;
    show("near +major end at (46, 0)",
        find_snap(Vec2::new(46.05, 0.05), 1.0, ellipse_set, None, None, &dobjects, None));
    show("near +minor end at (40, 3)",
        find_snap(Vec2::new(40.05, 3.05), 1.0, ellipse_set, None, None, &dobjects, None));
    let mut cen_set = SnapSet::default();
    cen_set.cen = true;
    show("CEN — hover the ellipse curve",
        find_snap(Vec2::new(43.5, 1.5), 1.0, cen_set, None, None, &dobjects, None));

    // PER on ellipse from an external anchor — Newton finds the perpendicular
    // foot on the curve. The cursor at (46.5, 0.05) is on the rim near the
    // +major end, and the foot from anchor (50, 0) on the +x axis lands at
    // (46, 0) — i.e. exactly the +major end.
    let mut per_set = SnapSet::default();
    per_set.per = true;
    show("PER on ellipse from (50, 0)",
        find_snap(Vec2::new(46.5, 0.05), 1.0, per_set,
            None, Some(Vec2::new(50.0, 0.0)), &dobjects, None));

    // -----------------------------------------------------------------
    // Tab cycling: list all candidates at one cursor position.
    // -----------------------------------------------------------------
    header("Tab cycling (find_all_snaps on the arc's curve)");
    let r45 = 5.0 / std::f64::consts::SQRT_2;
    let cursor_on_arc = Vec2::new(r45, 10.0 + r45);
    let mut snaps_for_arc = SnapSet::default();
    snaps_for_arc.cen = true;
    snaps_for_arc.nea = true;
    let hits = find_all_snaps(cursor_on_arc, 1.0, snaps_for_arc,
        None, None, &dobjects, None);
    for (i, h) in hits.iter().enumerate() {
        println!("  [{i}]  {} at ({:.3}, {:.3})", h.kind.name(), h.point.x, h.point.y);
    }
    println!("  → default is [0]; Tab walks 1, 2, … wrapping.");

    println!("\nDone. Tweak dobjects / cursor / osnaps to verify other scenarios.");
}
