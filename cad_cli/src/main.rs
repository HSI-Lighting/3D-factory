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
//
// Python scripting (WP-SCRIPT slice 2): `py <expr>` / `pyfile <path>` run an
// embedded script against THIS document through the same ScriptOp queue as
// the GUI (reads + geometry adds + layers; UI-only ops are rejected loudly).

use cad_kernel::*;
use cad_script::{ScriptEngine, ScriptMeta, ScriptOp, ScriptOpReply, ScriptReply, ViewInfo};
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
            Ok(Command::Python(Some(code))) => {
                writeln!(out, "py> {}", code).ok();
                out.flush().ok();
                run_python(&mut doc, PythonJob::Text(code));
            }
            Ok(Command::PythonFile(path)) => {
                writeln!(out, "pyfile> {}", path).ok();
                out.flush().ok();
                run_python(&mut doc, PythonJob::File(std::path::PathBuf::from(path)));
            }
            Ok(Command::Python(None)) => {
                writeln!(out,
                    "python: `py <expr>` runs inline; `pyfile <path>` runs a file (headless, slice 2)").ok();
            }
            Ok(Command::Script(name, args)) => {
                match name {
                    None => {
                        writeln!(out,
                            "run: scripts available: {}  (usage: run <name> [k=v …] or positional args)",
                            cli_script_names().join(", ")).ok();
                    }
                    Some(name) => {
                        match cli_resolve_script(&name) {
                            Some((stem, path)) => {
                                // All k=v → named inputs; anything else → the
                                // legacy positional form (rasm.args).
                                if args.iter().all(|a| a.contains('=')) {
                                    let params: Vec<(String, String)> = args
                                        .iter()
                                        .map(|a| {
                                            let (k, v) = a.split_once('=').unwrap();
                                            (k.trim().to_string(), v.to_string())
                                        })
                                        .collect();
                                    let shown: Vec<String> = params
                                        .iter()
                                        .map(|(k, v)| format!("{}={}", k, v))
                                        .collect();
                                    writeln!(out, "run> {} {}", stem, shown.join(" ")).ok();
                                    out.flush().ok();
                                    run_python(&mut doc, PythonJob::Script {
                                        path, name: stem, args: Vec::new(), params,
                                    });
                                } else {
                                    writeln!(out, "run> {} {}", stem, args.join(" ")).ok();
                                    out.flush().ok();
                                    run_python(&mut doc, PythonJob::Script {
                                        path, name: stem, args, params: Vec::new(),
                                    });
                                }
                            }
                            None => {
                                writeln!(out,
                                    "! run: no script '{}' in scripts/ (available: {})",
                                    name, cli_script_names().join(", ")).ok();
                            }
                        }
                    }
                }
            }
            Ok(Command::PyApiDoc) => {
                // Print the full scripting API reference (the AI-agent
                // document) — same file the app's `pyhelp` window shows.
                let mut dirs = vec![std::path::PathBuf::from("docs")];
                if let Ok(exe) = std::env::current_exe() {
                    if let Some(d) = exe.parent() { dirs.push(d.join("docs")); }
                }
                let mut shown = false;
                for dir in dirs {
                    let p = dir.join("scripting_api.md");
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        println!("{}", t);
                        shown = true;
                        break;
                    }
                }
                if !shown {
                    println!("! cannot find docs/scripting_api.md (looked in ./docs and <exe dir>/docs)");
                }
            }
            Ok(other) => {
                let s = apply_line(&mut doc, other);
                write!(out, "{}", s).ok();
                out.flush().ok();
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

/// Apply one command line (already parsed) to the document, returning the
/// report text. Shared by the stdin loop and `rasm.command()` in scripts.
fn apply_line(doc: &mut Document, cmd: Command) -> String {
    let mut out = String::new();
    match cmd {
        Command::Add(geom) => {
            let mut d = DObject::new(geom);
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            out.push_str(&format!("+ #{} {}\n", i, describe(&doc.dobjects[i].geom)));
        }
        Command::Delete(i) => {
            if i < doc.dobjects.len() {
                doc.dobjects.remove(i);
                out.push_str(&format!("- removed #{}\n", i));
            } else {
                out.push_str(&format!("! no dobject #{}\n", i));
            }
        }
        Command::Clear => {
            doc.dobjects.clear();
            out.push_str("- cleared\n");
        }
        Command::Help => {
            out.push_str("commands:\n");
            out.push_str("  line  x1,y1 x2,y2\n");
            out.push_str("  circle cx,cy r\n");
            out.push_str("  arc   cx,cy r start_deg end_deg\n");
            out.push_str("  arc3p p1 p2 p3                    [through 3 points]\n");
            out.push_str("  arcse cx,cy start end             [center + start + end]\n");
            out.push_str("  arccr start end r [major|minor]   [chord + radius]\n");
            out.push_str("  arccl start end length [left|right] [chord + arc length]\n");
            out.push_str("  del N / clear / help\n");
            out.push_str("python:\n");
            out.push_str("  py <expr>       run inline python (rasm.* surface)\n");
            out.push_str("  pyfile <path>   run a .py file\n");
            out.push_str("  run <name> [args…]  run scripts/<name>.py (args → rasm.args)\n");
        }
        Command::SnapOverride(k) => {
            out.push_str(&format!(
                "(snap override '{}' ignored — CLI has no interactive draw)\n",
                k.name()));
        }
        Command::GripsToggle => {
            out.push_str("(grips toggle ignored — CLI has no selection / display)\n");
        }
        Command::List => {
            out.push_str("list — all dobjects:\n");
            for (i, d) in doc.dobjects.iter().enumerate() {
                out.push_str(&format!("  #{} {}\n", i, describe(&d.geom)));
            }
        }
        Command::Select => {
            out.push_str("(select ignored — CLI has no interactive selection)\n");
        }
        Command::SelectAll | Command::SelectPrevious
        | Command::SelectNone | Command::SelectRemoveMode
        | Command::SelectAddMode | Command::SelectWindow
        | Command::SelectCrossing | Command::SelectLast
        | Command::SelectWindowPolygon | Command::SelectCrossingPolygon => {
            out.push_str("(selection sub-command ignored — CLI has no selection session)\n");
        }
        Command::Move => {
            out.push_str("(move ignored — CLI has no interactive draw)\n");
        }
        Command::Open(_) | Command::SaveAs(_) => {
            out.push_str("(open/save ignored — CLI is a math REPL, not a doc viewer)\n");
        }
        Command::Copy | Command::Rotate | Command::Scale
        | Command::Mirror | Command::Hatch { .. } | Command::DeleteSelected | Command::Undo
        | Command::Redo | Command::MatchProps | Command::Reverse
        | Command::ChangeLayer | Command::Offset(_) | Command::Wall(_)
        | Command::WallCleanup
        | Command::Linetype(_) | Command::ChProp(_) | Command::Text(_)
        | Command::TextStyle(_) | Command::DbgRecorder
        | Command::Dim | Command::DimContinue | Command::DimBaseline | Command::DimAngular
        | Command::DimArcLen | Command::DimOrdinate | Command::DimJogged | Command::QDim
        | Command::MInsert | Command::LayIso | Command::LayFrz | Command::LayOff | Command::LayOn | Command::LayWalk
        | Command::Publish | Command::ETransmit | Command::MeasureGeom | Command::QuickCalc
        | Command::Find(_) | Command::Replace(_)
        | Command::CenterMark(_) | Command::Xline | Command::Ray | Command::Id | Command::Oops | Command::SetByLayer | Command::Rename(_) | Command::RevCloud | Command::Area
        | Command::Donut | Command::Wipeout | Command::Sketch | Command::Blend
        | Command::Mline | Command::Region
        | Command::Overkill | Command::Purge
        | Command::LayerState(_) | Command::QSelect | Command::Ucs(_)
        | Command::PageSetup | Command::Table | Command::Xref(_)
        | Command::WBlock | Command::Boundary
        | Command::DimStyle(_) | Command::WallStyle(_)
        | Command::BlockDef(_) | Command::Insert(_) | Command::Explode
        | Command::Card(_)
        | Command::Lengthen(_) | Command::Break | Command::Align
        | Command::Stretch | Command::Trim | Command::Extend
        | Command::Fillet(_) | Command::Chamfer(_) | Command::Join
        | Command::Dist | Command::SetTool(_)
        | Command::BlockDiff(_) | Command::BlockTaskRecorder
        | Command::BlockTaskFinish
        | Command::SelectFence | Command::Divide | Command::Measure
        | Command::PlotStyle | Command::Plot
        // 3D-Factory commands (merged in from the SIMLUX line): no headless
        // meaning here — report them like the other editing ops.
        | Command::Units(..) | Command::Diag | Command::StrayLights(_)
        | Command::Dedupe | Command::RepairCuts | Command::Scene => {
            out.push_str("(editing op ignored — CLI has no interactive selection)\n");
        }
        Command::Python(_) | Command::PythonFile(_) | Command::Script(..)
        | Command::PyApiDoc => {
            // reached only via rasm.command() — scripts run through the engine
            out.push_str("(nested python not supported headless)\n");
        }
    }
    out
}

enum PythonJob {
    Text(String),
    File(std::path::PathBuf),
    Script { path: std::path::PathBuf, name: String, args: Vec<String>, params: Vec<(String, String)> },
}

/// Named scripts listed in ./scripts (or the exe's directory).
fn cli_script_names() -> Vec<String> {
    let mut dirs = vec![std::path::PathBuf::from("scripts")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() { dirs.push(d.join("scripts")); }
    }
    let mut names = Vec::new();
    for dir in dirs {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "py") {
                    if let Some(stem) = p.file_stem() {
                        names.push(stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Resolve `run <name>` → (stem, scripts/<name>.py), case-insensitive.
fn cli_resolve_script(name: &str) -> Option<(String, std::path::PathBuf)> {
    let mut dirs = vec![std::path::PathBuf::from("scripts")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() { dirs.push(d.join("scripts")); }
    }
    for dir in dirs {
        let mut p = dir.join(name);
        if p.extension().is_none() { p.set_extension("py"); }
        if p.is_file() {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            return Some((stem, p));
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "py") {
                    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                    if stem.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(name)) {
                        return Some((stem.unwrap_or_default(), path));
                    }
                }
            }
        }
    }
    None
}

/// Run a script job on the engine and service its ops against `doc` until
/// Finished. The UI-less twin of the app's per-frame pump.
fn run_python(doc: &mut Document, job: PythonJob) {
    let eng = ScriptEngine::new();
    match job {
        PythonJob::Text(code) => eng.submit_text(code),
        PythonJob::File(path) => eng.submit_file(path),
        PythonJob::Script { path, name, args, params } => {
            // Slice 5: learn the declaration first (a no-op metadata pass)
            // so LENGTH inputs convert through the document's display unit.
            let mut spec: Option<ScriptMeta> = None;
            eng.request_meta(path.clone());
            loop {
                for msg in eng.drain_ops() {
                    // The meta pass no-ops every rasm op; nothing to do.
                    let _ = msg;
                }
                let mut done = false;
                for r in eng.poll() {
                    match r {
                        ScriptReply::Meta(m) => spec = m,
                        ScriptReply::Finished { .. } => done = true,
                        ScriptReply::Error(e) => {
                            println!("{}", e.trim_end());
                            done = true;
                        }
                        _ => {}
                    }
                }
                if done { break; }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            match spec {
                Some(m) => {
                    // named as typed, positional mapped onto the declared
                    // order; lengths convert display → scene.
                    let raw: Vec<(String, String)> = if params.is_empty() && !args.is_empty() {
                        m.params.iter().zip(args.iter())
                            .map(|(p, a)| (p.name.clone(), a.clone()))
                            .collect()
                    } else {
                        params.clone()
                    };
                    let mut converted = Vec::with_capacity(raw.len());
                    for (k, v) in &raw {
                        let Some(p) = m.params.iter().find(|p| &p.name == k) else {
                            converted.push((k.clone(), v.clone()));
                            continue;
                        };
                        match p.ptype {
                            cad_script::ParamType::Length => {
                                match doc.units.parse_distance(v) {
                                    Some(s) if s.is_finite() => converted.push((k.clone(), format!("{}", s))),
                                    _ => {
                                        println!("! run {}: {}: '{}' is not a valid length", name, k, v);
                                        return;
                                    }
                                }
                            }
                            // Catalog dropdowns validate against the live
                            // catalogs and canonicalize the spelling.
                            cad_script::ParamType::Linetype
                            | cad_script::ParamType::Layer
                            | cad_script::ParamType::Block
                            | cad_script::ParamType::HatchPattern
                            | cad_script::ParamType::Choice => {
                                let choices: Vec<String> = match p.ptype {
                                    cad_script::ParamType::Linetype => doc.linetypes
                                        .linetypes.iter().map(|l| l.name.clone()).collect(),
                                    cad_script::ParamType::Layer => doc.layers
                                        .layers.iter().map(|l| l.name.clone()).collect(),
                                    cad_script::ParamType::Block => doc.blocks
                                        .blocks.iter().map(|b| b.name.clone()).collect(),
                                    cad_script::ParamType::HatchPattern => {
                                        cad_kernel::patterns::PATTERN_NAMES
                                            .iter()
                                            .map(|s| s.to_string())
                                            .collect()
                                    }
                                    _ => p.choices.clone(),
                                };
                                if choices.is_empty()
                                    || !choices.iter().any(|c| c.eq_ignore_ascii_case(v.trim()))
                                {
                                    println!("! run {}: {}: '{}' is not one of [{}]", name, k, v, choices.join(", "));
                                    return;
                                }
                                let canonical = choices
                                    .iter()
                                    .find(|c| c.eq_ignore_ascii_case(v.trim()))
                                    .cloned()
                                    .unwrap_or_else(|| v.clone());
                                converted.push((k.clone(), canonical));
                            }
                            _ => converted.push((k.clone(), v.clone())),
                        }
                    }
                    eng.submit_script_with_params(path, name, Vec::new(), converted);
                }
                None => {
                    if !params.is_empty() {
                        eng.submit_script_with_params(path, name, Vec::new(), params);
                    } else {
                        eng.submit_script(path, name, args);
                    }
                }
            }
        }
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        for msg in eng.drain_ops() {
            let reply = cli_apply_op(doc, &msg.op);
            eng.reply_op(msg.id, reply);
        }
        let mut finished = false;
        for r in eng.poll() {
            match r {
                ScriptReply::Print(s) => { write!(out, "{}", s).ok(); }
                ScriptReply::Value(v) => { writeln!(out, "= {}", v).ok(); }
                ScriptReply::Error(e) => {
                    writeln!(out, "{}", e.trim_end()).ok();
                }
                ScriptReply::Finished { .. } => finished = true,
                // Headless runs never request metadata — ignore.
                ScriptReply::Meta(_) => {}
            }
        }
        out.flush().ok();
        if finished { break; }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Stamp the document's CURRENT style (color / linetype / lineweight) and
/// ACTIVE layer onto a fresh entity — the headless twin of the GUI's
/// `stamp_fresh_style` (cad_app/src/app.rs). Script adds (`rasm.add_*`) and
/// parser adds are "fresh draws": the docs promise new entities land on the
/// active layer with the current style.
fn stamp_fresh_style(doc: &Document, style: &mut Style) {
    let fresh_default = style.layer == LayerTable::LAYER_ZERO
        && style.color == Color::ByLayer
        && style.linetype == LinetypeTable::CONTINUOUS
        && style.linetype_scale == 1.0
        && style.lineweight == Lineweight::ByLayer
        && style.visible;
    if fresh_default {
        style.color = doc.current_color;
        // BYLAYER (u32::MAX) → follow the active layer's linetype at draw
        // time (linetype has no ByLayer variant); else the current linetype.
        style.linetype = if doc.current_linetype == LinetypeTable::BYLAYER {
            doc.layers.get(doc.layers.active)
                .map(|l| l.linetype)
                .unwrap_or(LinetypeTable::CONTINUOUS)
        } else {
            doc.current_linetype
        };
        style.lineweight = doc.current_lineweight;
    }
    if style.layer == LayerTable::LAYER_ZERO
        && doc.layers.active != LayerTable::LAYER_ZERO
    {
        style.layer = doc.layers.active;
    }
}

/// Apply one script op against the headless document. Reads + geometry adds
/// + layers + entity modification work; UI-only surfaces (selection, view,
/// sysvars, blocks, files) answer loudly instead of pretending (rule 10).
fn cli_apply_op(doc: &mut Document, op: &ScriptOp) -> ScriptOpReply {
    use cad_script::{Entity, LayerInfo, LayoutInfo, UnitsInfo};
    let entity = |d: &DObject| {
        let layer = doc.layers.get(d.style.layer);
        let color = match d.style.color {
            Color::ByLayer => layer
                .map(|l| match l.color {
                    Color::Aci(n) => format!("aci {}", n),
                    Color::ByLayer => "bylayer".into(),
                    Color::ByBlock => "byblock".into(),
                    Color::TrueColorRef(idx) => format!("truecolor {}", idx),
                })
                .unwrap_or_else(|| "bylayer".into()),
            Color::ByBlock => "byblock".into(),
            Color::Aci(n) => format!("aci {}", n),
            Color::TrueColorRef(idx) => format!("truecolor {}", idx),
        };
        let linetype = {
            let id = match d.style.linetype {
                LinetypeTable::BYLAYER | LinetypeTable::CONTINUOUS => layer
                    .map(|l| l.linetype)
                    .unwrap_or(LinetypeTable::CONTINUOUS),
                other => other,
            };
            doc.linetypes
                .get(id)
                .map(|lt| lt.name.clone())
                .unwrap_or_else(|| format!("#{}", id))
        };
        let lineweight = lineweight::resolve_lineweight(d.style.lineweight, d.style.layer, &doc.layers);
        Entity {
            handle: d.handle,
            layer: layer.map(|l| l.name.clone()).unwrap_or_default(),
            color,
            linetype,
            lineweight,
            visible: d.style.visible,
            geom: d.geom.clone(),
            style: d.style,
        }
    };
    let cli_transform = |doc: &mut Document, indices: &[usize], f: &dyn Fn(&Geom) -> Geom| -> Result<usize, String> {
        if indices.iter().all(|&i| i >= doc.dobjects.len()) {
            return Err("none of the given entity indices exist".into());
        }
        let mut n = 0;
        for &i in indices {
            if let Some(d) = doc.dobjects.get_mut(i) {
                d.geom = f(&d.geom);
                n += 1;
            }
        }
        Ok(n)
    };
    match op {
        ScriptOp::DocCount => ScriptOpReply::Count(doc.dobjects.len()),
        ScriptOp::DocGet { index } => match doc.dobjects.get(*index) {
            Some(d) => ScriptOpReply::Entity(entity(d)),
            None => ScriptOpReply::Error(format!(
                "no dobject #{} ({} total)", index, doc.dobjects.len())),
        },
        ScriptOp::DocAll => {
            ScriptOpReply::Entities(doc.dobjects.iter().map(entity).collect())
        }
        ScriptOp::SelectionGet => ScriptOpReply::Indices(Vec::new()),
        ScriptOp::LayersGet => ScriptOpReply::Layers(
            doc.layers.layers.iter().enumerate().map(|(i, l)| LayerInfo {
                id: i as u32,
                name: l.name.clone(),
                visible: l.visible,
                locked: l.locked,
                frozen: l.frozen,
                plottable: l.plottable,
                color: format!("{:?}", l.color),
            }).collect(),
        ),
        ScriptOp::LayerActive => ScriptOpReply::LayerActive(doc.layers.active),
        ScriptOp::BlocksGet => ScriptOpReply::Blocks(Vec::new()),
        ScriptOp::SysVarGet { .. } => ScriptOpReply::SysVar(None),
        ScriptOp::ViewGet => ScriptOpReply::View(ViewInfo {
            center: math::Vec2::ZERO, scale: 1.0,
        }),

        ScriptOp::SelectionSet { .. } => ScriptOpReply::Indices(Vec::new()),
        ScriptOp::AddLine { a, b } => {
            let mut d = DObject::new(Geom::Line(Line { a: *a, b: *b }));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::AddCircle { center, radius } => {
            if !(*radius > 0.0) { return ScriptOpReply::Error("circle radius must be > 0".into()); }
            let mut d = DObject::new(Geom::Circle(Circle { center: *center, radius: *radius }));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::AddArc { center, radius, start_deg, sweep_deg } => {
            if !(*radius > 0.0) { return ScriptOpReply::Error("arc radius must be > 0".into()); }
            let mut d = DObject::new(Geom::Arc(Arc {
                center: *center,
                radius: *radius,
                start_angle: start_deg.to_radians(),
                sweep_angle: sweep_deg.to_radians(),
            }));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::AddEllipse { center, major, ratio } => {
            let mut d = DObject::new(Geom::Ellipse(Ellipse {
                center: *center, major: *major, ratio: *ratio,
            }));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::AddPolyline { vertices, closed } => {
            if vertices.len() < 2 {
                return ScriptOpReply::Error("a polyline needs at least 2 points".into());
            }
            let mut d = DObject::new(Geom::Polyline(Polyline {
                vertices: vertices.iter()
                    .map(|p| PolyVertex { pos: *p, bulge: 0.0 }).collect(),
                closed: *closed,
                widths: Vec::new(),
            }));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::AddPoint { at } => {
            let mut d = DObject::new(Geom::Point(Point { location: *at, style: 0, size: 0.0 }));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::AddText { text, at, height, angle_deg } => {
            let mut t = Text::empty();
            t.position = *at;
            t.height = *height;
            t.angle = angle_deg.to_radians();
            t.text = text.clone();
            let mut d = DObject::new(Geom::Text(t));
            stamp_fresh_style(doc, &mut d.style);
            let i = doc.push(d);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::Delete { indices } => {
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let n = sorted.len();
            for &i in sorted.iter().rev() {
                if i < doc.dobjects.len() { doc.dobjects.remove(i); }
            }
            ScriptOpReply::Ok(n)
        }
        ScriptOp::LayerAdd { name } => {
            let name = name.trim().to_string();
            if name.is_empty() { return ScriptOpReply::Error("layer name cannot be empty".into()); }
            if doc.layers.find(&name).is_some() {
                return ScriptOpReply::Error(format!("layer '{}' already exists", name));
            }
            let id = doc.layers.add(Layer {
                name,
                color: Color::Aci(7),
                linetype: 0,
                lineweight: Lineweight::Custom(0.0),
                visible: true, locked: false, frozen: false, plottable: true,
                order: 0,
            });
            ScriptOpReply::Ok(id as usize)
        }
        ScriptOp::LayerSetActive { name } => match doc.layers.find(name) {
            Some(id) => { doc.layers.active = id; ScriptOpReply::OkUnit }
            None => ScriptOpReply::Error(format!("no layer named '{}'", name)),
        },
        ScriptOp::LayerSet { name, visible, locked, frozen, plottable, color_aci } => {
            match doc.layers.find(name) {
                None => ScriptOpReply::Error(format!("no layer named '{}'", name)),
                Some(id) => {
                    if let Some(l) = doc.layers.get_mut(id) {
                        if let Some(v) = visible { l.visible = *v; }
                        if let Some(v) = locked { l.locked = *v; }
                        if let Some(v) = frozen { l.frozen = *v; }
                        if let Some(v) = plottable { l.plottable = *v; }
                        if let Some(a) = color_aci { l.color = Color::Aci(*a); }
                    }
                    ScriptOpReply::OkUnit
                }
            }
        }
        ScriptOp::BlockCreate { .. } =>
            ScriptOpReply::Error("blocks are not available in the headless CLI".into()),
        ScriptOp::BlockInsert { .. } =>
            ScriptOpReply::Error("blocks are not available in the headless CLI".into()),
        ScriptOp::Command { raw } => {
            match parse(raw) {
                Ok(cmd) => {
                    let s = apply_line(doc, cmd);
                    let lines: Vec<String> =
                        s.trim_end().split('\n').map(String::from).collect();
                    ScriptOpReply::CommandOutput(lines)
                }
                Err(e) => ScriptOpReply::Error(format!("parse error: {}", e)),
            }
        }
        ScriptOp::SysVarSet { .. } =>
            ScriptOpReply::Error("sysvars are not available in the headless CLI".into()),
        ScriptOp::ViewSet { .. } => ScriptOpReply::OkUnit,
        ScriptOp::Save { path } => {
            let lower = path.to_ascii_lowercase();
            let bytes: Vec<u8> = if lower.ends_with(".dxf") {
                cad_io::dxf::write_dxf(doc).into_bytes()
            } else if lower.ends_with(".rsm") {
                cad_io::rsm::write_rsm(doc)
            } else {
                return ScriptOpReply::Error(format!(
                    "save '{}': unknown extension (expected .dxf or .rsm)",
                    path
                ));
            };
            match std::fs::write(path, &bytes) {
                Ok(()) => ScriptOpReply::CommandOutput(vec![
                    format!("saved '{}'  ({} bytes)", path, bytes.len())
                ]),
                Err(e) => ScriptOpReply::Error(format!("save '{}': {}", path, e)),
            }
        }
        ScriptOp::Open { .. } =>
            ScriptOpReply::Error("open is not available in the headless CLI".into()),

        // P1 — entity modification on the headless document.
        ScriptOp::ModifyMove { indices, delta } => match cli_transform(doc, indices, &|g| g.translated(*delta)) {
            Ok(n) => ScriptOpReply::Ok(n),
            Err(e) => ScriptOpReply::Error(e),
        },
        ScriptOp::ModifyCopy { indices, delta } => {
            // Validated loudly, like the other transforms (docs §4.2: "if
            // none of them exist the call raises").
            if indices.iter().all(|&i| i >= doc.dobjects.len()) {
                return ScriptOpReply::Error("none of the given entity indices exist".into());
            }
            let n0 = doc.dobjects.len();
            for &i in indices {
                if let Some(src) = doc.dobjects.get(i).cloned() {
                    let mut d = DObject::new(src.geom.translated(*delta));
                    d.style = src.style;
                    doc.push(d);
                }
            }
            ScriptOpReply::Indices((n0..doc.dobjects.len()).collect())
        }
        ScriptOp::ModifyRotate { indices, pivot, angle_deg } => {
            let a = angle_deg.to_radians();
            match cli_transform(doc, indices, &|g| g.rotated(*pivot, a)) {
                Ok(n) => ScriptOpReply::Ok(n),
                Err(e) => ScriptOpReply::Error(e),
            }
        }
        ScriptOp::ModifyScale { indices, pivot, factor } =>
            match cli_transform(doc, indices, &|g| g.scaled(*pivot, *factor)) {
                Ok(n) => ScriptOpReply::Ok(n),
                Err(e) => ScriptOpReply::Error(e),
            },
        ScriptOp::ModifyMirror { indices, a, b } =>
            match cli_transform(doc, indices, &|g| g.mirrored(*a, *b)) {
                Ok(n) => ScriptOpReply::Ok(n),
                Err(e) => ScriptOpReply::Error(e),
            },
        ScriptOp::SetEntityColor { indices, color } => {
            let c = match color {
                -1 => Color::ByLayer,
                -2 => Color::ByBlock,
                n if (0..=255).contains(n) => Color::Aci(*n as u8),
                other => return ScriptOpReply::Error(format!("{} is not a color", other)),
            };
            let mut n = 0;
            for &i in indices {
                if let Some(d) = doc.dobjects.get_mut(i) {
                    d.style.color = c;
                    n += 1;
                }
            }
            ScriptOpReply::Ok(n)
        }
        ScriptOp::SetEntityLinetype { indices, name } => {
            let id = if name.is_empty() || name.eq_ignore_ascii_case("bylayer") {
                LinetypeTable::BYLAYER
            } else {
                match doc.linetypes.find(name) {
                    Some(id) => id,
                    None => return ScriptOpReply::Error(format!("no linetype named '{}'", name)),
                }
            };
            let mut n = 0;
            for &i in indices {
                if let Some(d) = doc.dobjects.get_mut(i) {
                    d.style.linetype = id;
                    n += 1;
                }
            }
            ScriptOpReply::Ok(n)
        }
        ScriptOp::SetEntityLayer { indices, name } => match doc.layers.find(name) {
            None => ScriptOpReply::Error(format!("no layer named '{}'", name)),
            Some(id) => {
                let mut n = 0;
                for &i in indices {
                    if let Some(d) = doc.dobjects.get_mut(i) {
                        d.style.layer = id;
                        n += 1;
                    }
                }
                ScriptOpReply::Ok(n)
            }
        },
        ScriptOp::SetEntityLineweight { indices, mm } => {
            let lw = if *mm < 0.0 {
                Lineweight::ByLayer
            } else {
                Lineweight::Custom(*mm as f32)
            };
            let mut n = 0;
            for &i in indices {
                if let Some(d) = doc.dobjects.get_mut(i) {
                    d.style.lineweight = lw;
                    n += 1;
                }
            }
            ScriptOpReply::Ok(n)
        }
        ScriptOp::SetEntityVisible { indices, visible } => {
            let mut n = 0;
            for &i in indices {
                if let Some(d) = doc.dobjects.get_mut(i) {
                    d.style.visible = *visible;
                    n += 1;
                }
            }
            ScriptOpReply::Ok(n)
        }
        ScriptOp::SetEntityGeom { index, geom } => match doc.dobjects.get_mut(*index) {
            Some(d) => {
                d.geom = geom.clone();
                ScriptOpReply::OkUnit
            }
            None => ScriptOpReply::Error(format!("no dobject #{}", index)),
        },
        ScriptOp::DocUnits => ScriptOpReply::Units(UnitsInfo {
            name: doc.units.name.clone(),
            scene_per_unit: doc.units.scene_per_unit,
        }),
        ScriptOp::DocBounds => {
            let mut min: Option<math::Vec2> = None;
            let mut max: Option<math::Vec2> = None;
            for d in &doc.dobjects {
                if matches!(d.geom, Geom::Hatch(_)) {
                    continue;
                }
                let (lo, hi) = d.bbox();
                min = Some(match min {
                    None => lo,
                    Some(m) => math::Vec2::new(m.x.min(lo.x), m.y.min(lo.y)),
                });
                max = Some(match max {
                    None => hi,
                    Some(m) => math::Vec2::new(m.x.max(hi.x), m.y.max(hi.y)),
                });
            }
            ScriptOpReply::Bounds(min.zip(max))
        }
        ScriptOp::LayoutsGet => ScriptOpReply::Layouts(
            doc.layouts
                .iter()
                .enumerate()
                .map(|(i, l)| LayoutInfo {
                    id: i as u32,
                    name: l.name.clone(),
                    active: doc.active_layout == Some(i),
                })
                .collect(),
        ),
        ScriptOp::LayoutSetActive { name } => {
            match doc.layouts.iter().position(|l| l.name.eq_ignore_ascii_case(name)) {
                Some(i) => {
                    doc.active_layout = Some(i);
                    ScriptOpReply::OkUnit
                }
                None => ScriptOpReply::Error(format!("no layout named '{}'", name)),
            }
        }
        ScriptOp::LinetypesGet => ScriptOpReply::Linetypes(
            doc.linetypes.linetypes.iter().map(|l| l.name.clone()).collect(),
        ),
        ScriptOp::UndoGroup => ScriptOpReply::OkUnit,
        ScriptOp::SetCurrentColor { color } => {
            let c = match color {
                -1 => Color::ByLayer,
                -2 => Color::ByBlock,
                n if (0..=255).contains(n) => Color::Aci(*n as u8),
                other => return ScriptOpReply::Error(format!("{} is not a color", other)),
            };
            doc.current_color = c;
            ScriptOpReply::OkUnit
        }
        ScriptOp::SetCurrentLinetype { name } => match doc.linetypes.find(name) {
            Some(id) => {
                doc.current_linetype = id;
                ScriptOpReply::OkUnit
            }
            None => ScriptOpReply::Error(format!("no linetype named '{}'", name)),
        },
        ScriptOp::SetCurrentLineweight { mm } => {
            if !(*mm >= 0.0 && mm.is_finite()) {
                return ScriptOpReply::Error("lineweight mm must be >= 0".into());
            }
            doc.current_lineweight = Lineweight::Custom(*mm as f32);
            ScriptOpReply::OkUnit
        }
        ScriptOp::ZoomExtents => ScriptOpReply::OkUnit,
        ScriptOp::AddHatch { boundary_indices, pattern } => {
            let pattern = pattern.trim();
            let pat = if pattern.eq_ignore_ascii_case("solid") {
                HatchPattern::Solid
            } else {
                let canonical = cad_kernel::patterns::PATTERN_NAMES
                    .iter()
                    .find(|n| n.eq_ignore_ascii_case(pattern))
                    .map(|s| s.to_string());
                match canonical {
                    Some(name) => HatchPattern::Pattern { name, scale: 1.0, angle_deg: 0.0 },
                    None => {
                        return ScriptOpReply::Error(format!(
                            "no hatch pattern '{}' — available: {}",
                            pattern,
                            cad_kernel::patterns::PATTERN_NAMES.join(", ")
                        ))
                    }
                }
            };
            let mut handles: Vec<Handle> = Vec::new();
            let is_closed = |g: &Geom| -> bool {
                match g {
                    Geom::Polyline(p) => {
                        p.closed
                            || (p.vertices.len() >= 3
                                && p.vertices.first().map(|v| v.pos)
                                    == p.vertices.last().map(|v| v.pos))
                    }
                    Geom::Circle(_) | Geom::Ellipse(_) => true,
                    Geom::Spline(s) => {
                        !s.control_points.is_empty()
                            && s.control_points.first() == s.control_points.last()
                    }
                    _ => false,
                }
            };
            for &i in boundary_indices {
                if let Some(d) = doc.dobjects.get(i) {
                    if is_closed(&d.geom) {
                        handles.push(d.handle);
                    }
                }
            }
            if handles.is_empty() {
                return ScriptOpReply::Error(
                    "add_hatch: none of the given indices is a closed boundary".into(),
                );
            }
            let mut hd: DObject = Hatch { boundary_handles: handles, pattern: pat }.into();
            stamp_fresh_style(doc, &mut hd.style);
            let i = doc.push(hd);
            ScriptOpReply::Ok(i)
        }
        ScriptOp::HatchAt { .. } =>
            ScriptOpReply::Error("hatch boundary tracing is not available in the headless CLI — use add_hatch with explicit boundary indices".into()),
        ScriptOp::HatchPatternsGet => ScriptOpReply::Patterns(
            cad_kernel::patterns::PATTERN_NAMES.iter().map(|s| s.to_string()).collect(),
        ),
    }
}

fn describe(g: &Geom) -> String {
    match g {
        Geom::Line(l)   => format!(
            "line ({:.4},{:.4}) -> ({:.4},{:.4})",
            l.a.x, l.a.y, l.b.x, l.b.y),
        Geom::Xline(x)  => format!(
            "xline base=({:.4},{:.4}) dir=({:.4},{:.4})",
            x.base.x, x.base.y, x.dir.x, x.dir.y),
        Geom::Ray(r)    => format!(
            "ray base=({:.4},{:.4}) dir=({:.4},{:.4})",
            r.base.x, r.base.y, r.dir.x, r.dir.y),
        Geom::Donut(d)  => format!(
            "donut center=({:.4},{:.4}) r={:.4}->{:.4}",
            d.center.x, d.center.y, d.inner_radius, d.outer_radius),
        Geom::Wipeout(wo) => format!("wipeout {} vertices", wo.pts.len()),
        Geom::Region(rg)  => format!("region {} vertices", rg.loop_pts.len()),
        Geom::Table(t)  => format!(
            "table {}×{} at ({:.4},{:.4})", t.n_rows, t.n_cols,
            t.insert.x, t.insert.y),
        Geom::Xref(x)   => format!(
            "xref '{}' -> {} at ({:.4},{:.4}) ({} children)",
            x.name, x.path, x.insert.x, x.insert.y, x.cached.len()),
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
                DimKind::Angular { .. }  => "angular",
                DimKind::ArcLen { .. }   => "arc-length",
                DimKind::Ordinate { .. } => "ordinate",
                DimKind::JoggedRadius { .. } => "jogged radius",
            };
            format!("dim {} value={:.4} style={}",
                kind_name, d.measured_value(), d.style)
        }
        Geom::BlockRef(br) => format!(
            "blockref #{} at ({:.4},{:.4}) scale={:.3} rot={:.4}",
            br.block, br.insert.x, br.insert.y, br.scale, br.rotation),
        Geom::Viewport(vp) => format!(
            "viewport ({:.4},{:.4}) {}x{}",
            vp.center.x, vp.center.y, vp.width, vp.height),
        Geom::Leader(l) => format!(
            "leader ({} pts, text \"{}\")",
            l.pts.len(), l.label.text),
        Geom::AttrDef(a) => format!(
            "attdef \"{}\" at ({:.4},{:.4}) h={:.4}",
            a.tag, a.position.x, a.position.y, a.height),
        Geom::CenterMark(cm) => format!(
            "centermark at ({:.4},{:.4}) size={:.4} rot={:.2}°",
            cm.center.x, cm.center.y, cm.size, cm.rotation.to_degrees()),
    }
}
