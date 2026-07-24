//! Extract + report hatch patterns from a `.pat` file.
//!
//!   cargo run -p cad_io --example pat_extract -- <file.pat>
//!   cargo run -p cad_io --example pat_extract -- assets/hatch/standard.pat
//!
//! Prints each pattern (name, line-family count, solid/dashed) and how many of
//! the file's patterns are USABLE (have ≥1 line family). Point it at a real
//! acad.pat / acadiso.pat to see exactly how many of a pack come through.

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: pat_extract <file.pat>");
        std::process::exit(1);
    });
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1);
    });

    let r = cad_io::parse_pat(&text);
    println!("== .pat extract: {path} ==");
    for p in &r.patterns {
        let kind = if !p.is_usable() { "EMPTY (no families)" }
                   else if p.is_solid_lines() { "solid lines" }
                   else { "dashed lines" };
        let mark = if p.is_usable() { "✓" } else { "✗" };
        println!("  {mark} {:<14} {} family(ies) · {:<18} {}",
                 p.name, p.lines.len(), kind, p.description);
    }
    println!("\n-- summary --");
    println!("  patterns:        {}", r.patterns.len());
    println!("  usable (.pat):   {}", r.usable_count());
    println!("  empty/headers:   {}", r.patterns.len() - r.usable_count());
    if !r.warnings.is_empty() {
        println!("  warnings:        {}", r.warnings.len());
        for w in r.warnings.iter().take(10) { println!("    ! {w}"); }
        if r.warnings.len() > 10 { println!("    … {} more", r.warnings.len() - 10); }
    }
}
