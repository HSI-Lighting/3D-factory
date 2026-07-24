// Headless CAD kernel REPL — no UI, no GL, no Qt.
//
// Reads commands from stdin (one per line), prints a structured report to
// stdout. Designed for human verification of the math: write a fixture file
// of commands, pipe it in, diff the output against expected values.
//
// Usage:
//   cad_cli < fixtures/two_lines.txt
//   echo -e "line 0,0 10,0\nline 5,-5 5,5" | cad_cli
//
// Lines beginning with '#' or empty lines are ignored.

use cad_kernel::*;
use std::io::{self, BufRead, Write};

fn main() {
    let stdin  = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut doc = Document::default();

    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        match parse(trimmed) {
            Ok(Command::Add(geom)) => {
                let i = doc.push(DObject::new(geom));
                writeln!(out, "+ #{} {}", i, describe(&doc.dobjects[i].geom)).ok();
            }
            Ok(Command::Delete(i)) => {
                if i < doc.dobjects.len() {
                    doc.dobjects.remove(i);
                    writeln!(out, "- removed #{}", i).ok();
                } else {
                    writeln!(out, "! no dobject #{}", i).ok();
                }
            }
            Ok(Command::Clear) => {
                doc.dobjects.clear();
                writeln!(out, "- cleared").ok();
            }
            Ok(Command::Help) => {
                writeln!(out, "commands:").ok();
                writeln!(out, "  line  x1,y1 x2,y2").ok();
                writeln!(out, "  circle cx,cy r").ok();
                writeln!(out, "  arc   cx,cy r start_deg end_deg").ok();
                writeln!(out, "  arc3p p1 p2 p3                    [through 3 points]").ok();
                writeln!(out, "  arcse cx,cy start end             [center + start + end]").ok();
                writeln!(out, "  arccr start end r [major|minor]   [chord + radius]").ok();
                writeln!(out, "  arccl start end length [left|right] [chord + arc length]").ok();
                writeln!(out, "  del N / clear / help").ok();
            }
            Ok(Command::SnapOverride(k)) => {
                // headless CLI doesn't have an in-progress draw, so snap
                // overrides have no effect — just acknowledge.
                writeln!(out, "(snap override '{}' ignored — CLI has no interactive draw)",
                    k.name()).ok();
            }
            Ok(Command::GripsToggle) => {
                writeln!(out, "(grips toggle ignored — CLI has no selection / display)").ok();
            }
            Ok(Command::List) => {
                // Headless: no interactive selection — just dump everything.
                writeln!(out, "list — all dobjects:").ok();
                for (i, d) in doc.dobjects.iter().enumerate() {
                    writeln!(out, "  #{} {}", i, describe(&d.geom)).ok();
                }
            }
            Ok(Command::Select) => {
                writeln!(out, "(select ignored — CLI has no interactive selection)").ok();
            }
            Ok(Command::SelectAll) | Ok(Command::SelectPrevious)
            | Ok(Command::SelectNone) | Ok(Command::SelectRemoveMode)
            | Ok(Command::SelectAddMode) | Ok(Command::SelectWindow)
            | Ok(Command::SelectCrossing) | Ok(Command::SelectLast) => {
                writeln!(out, "(selection sub-command ignored — CLI has no selection session)").ok();
            }
            Ok(Command::Move) => {
                writeln!(out, "(move ignored — CLI has no interactive draw)").ok();
            }
            Ok(Command::Open(_)) | Ok(Command::SaveAs(_)) => {
                writeln!(out, "(open/save ignored — CLI is a math REPL, not a doc viewer)").ok();
            }
            Ok(Command::Copy) | Ok(Command::Rotate) | Ok(Command::Scale)
            | Ok(Command::Mirror) | Ok(Command::Hatch { .. }) | Ok(Command::DeleteSelected) | Ok(Command::Undo)
            | Ok(Command::Redo) | Ok(Command::MatchProps) | Ok(Command::Reverse)
            | Ok(Command::ChangeLayer) | Ok(Command::Offset(_)) | Ok(Command::Wall(_))
            | Ok(Command::Linetype(_)) | Ok(Command::ChProp(_)) | Ok(Command::Text(_))
            | Ok(Command::TextStyle(_)) | Ok(Command::DbgRecorder)
            | Ok(Command::Dim) | Ok(Command::DimStyle(_)) | Ok(Command::WallStyle(_))
            | Ok(Command::BlockDef(_)) | Ok(Command::Insert(_)) | Ok(Command::Explode)
            | Ok(Command::Card(_))
            | Ok(Command::Lengthen(_)) | Ok(Command::Break) | Ok(Command::Align)
            | Ok(Command::Stretch) | Ok(Command::Trim) | Ok(Command::Extend)
            | Ok(Command::Fillet(_)) | Ok(Command::Chamfer(_)) | Ok(Command::Join)
            | Ok(Command::Dist) | Ok(Command::SetTool(_))
            | Ok(Command::BlockDiff(_)) | Ok(Command::BlockTaskRecorder)
            | Ok(Command::BlockTaskFinish) => {
                writeln!(out, "(editing op ignored — CLI has no interactive selection)").ok();
            }
            Err(e) => { writeln!(out, "! parse error: {}", e).ok(); }
        }
    }

    writeln!(out).ok();
    writeln!(out, "=== dobjects ({}) ===", doc.dobjects.len()).ok();
    for (i, d) in doc.dobjects.iter().enumerate() {
        writeln!(out, "  #{} {}", i, describe(&d.geom)).ok();
    }

    writeln!(out).ok();
    writeln!(out, "=== intersections ===").ok();
    let mut count = 0;
    for i in 0..doc.dobjects.len() {
        for j in (i + 1)..doc.dobjects.len() {
            for p in intersect(&doc.dobjects[i].geom, &doc.dobjects[j].geom) {
                writeln!(out,
                    "  ({:>12.6}, {:>12.6})    [dobjects #{} ∩ #{}]",
                    p.x, p.y, i, j).ok();
                count += 1;
            }
        }
    }
    writeln!(out, "total: {}", count).ok();
}

fn describe(g: &Geom) -> String {
    match g {
        Geom::Line(l)   => format!(
            "line ({:.4},{:.4}) -> ({:.4},{:.4})",
            l.a.x, l.a.y, l.b.x, l.b.y),
        Geom::Circle(c) => format!(
            "circle c=({:.4},{:.4}) r={:.4}",
            c.center.x, c.center.y, c.radius),
        Geom::Arc(a)    => format!(
            "arc c=({:.4},{:.4}) r={:.4} start={:.4}° sweep={:.4}°",
            a.center.x, a.center.y, a.radius,
            a.start_angle.to_degrees(), a.sweep_angle.to_degrees()),
        Geom::Ellipse(el) => format!(
            "ellipse c=({:.4},{:.4}) a={:.4} ratio={:.4} rot={:.4}°",
            el.center.x, el.center.y, el.semi_major(), el.ratio,
            el.major.angle().to_degrees()),
        Geom::EllipseArc(ea) => format!(
            "ellipsearc c=({:.4},{:.4}) a={:.4} ratio={:.4} start={:.4}° sweep={:.4}°",
            ea.ellipse.center.x, ea.ellipse.center.y,
            ea.ellipse.semi_major(), ea.ellipse.ratio,
            ea.start_param.to_degrees(), ea.sweep_param.to_degrees()),
        Geom::Point(pt) => format!(
            "point ({:.4},{:.4}) style={} size={:.4}",
            pt.location.x, pt.location.y, pt.style, pt.size),
        Geom::Polyline(p) => format!(
            "polyline {} verts{} length={:.4}",
            p.vertices.len(),
            if p.closed { " (closed)" } else { "" },
            p.length()),
        Geom::Hatch(h) => format!(
            "hatch ({} boundary loops, {:?})",
            h.boundary_handles.len(), h.pattern),
        Geom::Spline(s) => format!(
            "spline (degree {}, {} control points)",
            s.degree, s.control_points.len()),
        Geom::Wall(w) => format!(
            "wall ({:.4},{:.4}) -> ({:.4},{:.4}) thk={:.4}",
            w.start.x, w.start.y, w.end.x, w.end.y, w.thickness),
        Geom::Text(t) => format!(
            "text \"{}\" @ ({:.4},{:.4}) h={:.4} ang={:.2}°",
            t.text, t.position.x, t.position.y, t.height,
            t.angle.to_degrees()),
        Geom::Dimension(d) => {
            use cad_kernel::DimKind;
            let kind_name = match &d.kind {
                DimKind::Linear { .. }   => "linear",
                DimKind::Radius { .. }   => "radius",
                DimKind::Diameter { .. } => "diameter",
            };
            format!("dim {} value={:.4} style={}",
                kind_name, d.measured_value(), d.style)
        }
        Geom::BlockRef(br) => format!(
            "blockref #{} at ({:.4},{:.4}) scale={:.3} rot={:.4}",
            br.block, br.insert.x, br.insert.y, br.scale, br.rotation),
    }
}
