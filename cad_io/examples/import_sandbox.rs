//! IMPORT SANDBOX — a testbed for "newcomer" file formats (DWG first).
//!
//! RUST_CAD reads DXF natively, so the import path for foreign formats is
//! `DWG → (external converter) → DXF → cad_io::dxf::read_dxf`. This harness
//! lets us trial ANY converter without baking it into the app, and — crucially
//! — reports how much of a real file actually survives the round-trip into the
//! kernel `Document` (which DXF entities RUST_CAD's reader supports vs drops).
//!
//!   cargo run -p cad_io --example import_sandbox -- <file.dwg|file.dxf> \
//!        [--converter "<cmd with {in} {out}>"] [--out drawing.rsm]
//!
//! Examples:
//!   # a DXF directly (no converter needed) — pure reader-coverage report
//!   cargo run -p cad_io --example import_sandbox -- plan.dxf
//!
//!   # a DWG via a converter that takes {in} {out} (e.g. the ACadSharp tool)
//!   cargo run -p cad_io --example import_sandbox -- plan.dwg \
//!        --converter "dwgconv {in} {out}" --out plan.rsm

use cad_kernel::{Document, Geom};

/// DXF entity names RUST_CAD's `read_dxf` currently turns into geometry.
/// (INSERT → BlockRef, with the BLOCKS section parsed into the block table.)
const SUPPORTED: &[&str] = &["LINE", "CIRCLE", "ARC", "ELLIPSE", "POINT", "LWPOLYLINE", "INSERT"];

/// Common DXF entity names we tally when scanning a file (everything not in
/// SUPPORTED is reported as DROPPED, so we can see what coverage is missing).
const ENTITY_VOCAB: &[&str] = &[
    "LINE", "CIRCLE", "ARC", "ELLIPSE", "POINT", "LWPOLYLINE", "POLYLINE",
    "SPLINE", "INSERT", "TEXT", "MTEXT", "ATTRIB", "ATTDEF", "DIMENSION",
    "HATCH", "SOLID", "3DSOLID", "3DFACE", "LEADER", "MLEADER", "MLINE",
    "RAY", "XLINE", "WIPEOUT", "IMAGE", "TABLE", "TOLERANCE", "VIEWPORT",
    "REGION", "BODY", "SHAPE", "TRACE", "ACAD_PROXY_ENTITY",
];

fn geom_kind(g: &Geom) -> &'static str {
    match g {
        Geom::Line(_) => "Line",          Geom::Circle(_) => "Circle",
        Geom::Arc(_) => "Arc",            Geom::Ellipse(_) => "Ellipse",
        Geom::EllipseArc(_) => "EllipseArc", Geom::Point(_) => "Point",
        Geom::Polyline(_) => "Polyline",  Geom::Hatch(_) => "Hatch",
        Geom::Spline(_) => "Spline",      Geom::Wall(_) => "Wall",
        Geom::Text(_) => "Text",          Geom::Dimension(_) => "Dimension",
        Geom::BlockRef(_) => "BlockRef",
    }
}

/// Tally DXF entity names in the model/paper-space ENTITIES section ONLY.
/// Entities inside BLOCK definitions (the BLOCKS section) are excluded because
/// `read_dxf` does not expand block inserts — counting them would overstate what
/// actually lands in the Document (a block-heavy drawing has thousands of
/// block-internal lines but few top-level entities).
fn tally_entities(dxf: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = dxf.lines().collect();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut section = String::new();
    let mut expect_section_name = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        let code = lines[i].trim();
        let value = lines[i + 1].trim();
        if code == "0" {
            let v = value.to_ascii_uppercase();
            match v.as_str() {
                "SECTION" => expect_section_name = true,
                "ENDSEC"  => section.clear(),
                _ if section == "ENTITIES" && ENTITY_VOCAB.contains(&v.as_str()) => {
                    *counts.entry(v).or_insert(0) += 1;
                }
                _ => {}
            }
        } else if code == "2" && expect_section_name {
            section = value.to_ascii_uppercase();
            expect_section_name = false;
        }
        i += 2;
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(input) = args.get(1).filter(|s| !s.starts_with("--")).cloned() else {
        eprintln!("usage: import_sandbox <file.dwg|file.dxf> [--converter \"cmd {{in}} {{out}}\"] [--out out.rsm]");
        std::process::exit(1);
    };
    let converter = arg_value(&args, "--converter");
    let out_rsm = arg_value(&args, "--out");
    let is_dwg = input.to_ascii_lowercase().ends_with(".dwg");

    println!("== IMPORT SANDBOX ==");
    println!("input: {input}");

    // ---- 1. obtain a DXF (convert if DWG) -----------------------------
    let dxf_path = if is_dwg {
        let Some(tmpl) = converter else {
            eprintln!("\n! '{input}' is DWG — RUST_CAD has no native DWG reader.");
            eprintln!("  Provide a converter, e.g.:");
            eprintln!("    --converter \"dwgconv {{in}} {{out}}\"   (the ACadSharp NativeAOT tool)");
            eprintln!("    --converter \"ODAFileConverter ...\"     (ODA File Converter)");
            std::process::exit(2);
        };
        let out = std::env::temp_dir().join("rustcad_import_sandbox.dxf");
        let cmd = tmpl.replace("{in}", &shell_quote(&input))
                      .replace("{out}", &shell_quote(&out.to_string_lossy()));
        println!("converting via: {cmd}");
        let status = std::process::Command::new("sh").arg("-c").arg(&cmd).status();
        match status {
            Ok(s) if s.success() && out.exists() => out.to_string_lossy().to_string(),
            Ok(s) => { eprintln!("! converter exited {s} (no DXF produced)"); std::process::exit(3); }
            Err(e) => { eprintln!("! could not run converter: {e}"); std::process::exit(3); }
        }
    } else {
        input.clone()
    };

    // ---- 2. read the DXF text -----------------------------------------
    let dxf = match std::fs::read_to_string(&dxf_path) {
        Ok(t) => t,
        Err(e) => { eprintln!("! cannot read DXF '{dxf_path}': {e}"); std::process::exit(4); }
    };
    println!("dxf:   {dxf_path}  ({} KB)", dxf.len() / 1024);

    // ---- 3. what entities are IN the file -----------------------------
    let present = tally_entities(&dxf);
    println!("\n-- entities in file --");
    let mut dropped: Vec<(String, usize)> = Vec::new();
    for (name, n) in &present {
        let ok = SUPPORTED.contains(&name.as_str());
        println!("  {:<14} {:>7}   {}", name, n, if ok { "✓ imported" } else { "✗ DROPPED (reader gap)" });
        if !ok { dropped.push((name.clone(), *n)); }
    }
    if present.is_empty() { println!("  (none recognised — is this an ENTITIES-bearing DXF?)"); }

    // ---- 4. parse into the kernel Document ----------------------------
    match cad_io::dxf::read_dxf(&dxf) {
        Ok(doc) => report_doc(&doc, &out_rsm),
        Err(e)  => eprintln!("\n! read_dxf failed: {e}"),
    }

    // ---- 5. verdict ---------------------------------------------------
    println!("\n-- verdict --");
    if dropped.is_empty() {
        println!("  all recognised entities are supported by the reader.");
    } else {
        let lost: usize = dropped.iter().map(|(_, n)| n).sum();
        println!("  {} entit{} across {} type(s) were DROPPED:",
                 lost, if lost == 1 { "y" } else { "ies" }, dropped.len());
        println!("  {}", dropped.iter().map(|(n, c)| format!("{n}×{c}"))
                 .collect::<Vec<_>>().join(", "));
        println!("  → these are the cad_io DXF-reader gaps to close for real files.");
    }
}

fn report_doc(doc: &Document, out_rsm: &Option<String>) {
    println!("\n-- imported into Document --");
    let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
    for d in &doc.dobjects { *kinds.entry(geom_kind(&d.geom)).or_insert(0) += 1; }
    println!("  dobjects: {}", doc.dobjects.len());
    for (k, n) in &kinds { println!("    {:<12} {}", k, n); }
    println!("  layers:   {} ({})", doc.layers.layers.len(),
             doc.layers.layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>().join(", "));
    println!("  blocks:   {}", doc.blocks.blocks.len());

    if let Some(path) = out_rsm {
        let bytes = cad_io::rsm::write_rsm(doc);
        match std::fs::write(path, &bytes) {
            Ok(_)  => println!("\n  → wrote {} ({} KB) — open it in RUST_CAD", path, bytes.len() / 1024),
            Err(e) => eprintln!("\n! could not write {path}: {e}"),
        }
    }
}

/// Minimal shell-quote for paths with spaces (the lighting-layout files have them).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
