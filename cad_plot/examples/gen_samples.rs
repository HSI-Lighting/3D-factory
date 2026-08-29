//! Generate the owner-facing verification PDFs for the plot pipeline.
//!
//! Usage:  cargo run -p cad_plot --example gen_samples -- <output_dir>
//!
//! Produces, from one demo drawing (ACI 1/3/5 objects at 0.25 mm):
//!   1_default.pdf      — default table: strokes at object lineweights
//!   2_pens.pdf         — ACI-1 → 0.70 mm, ACI-3 → 0.13 mm (per-color pen)
//!   3a_fit.pdf         — default, Fit
//!   3b_1to100.pdf      — default, 1:100 (identical stroke mm to 3a)
//!   4_mono.pdf         — monochrome (all black)

use cad_kernel::color::Color;
use cad_kernel::geom::{Circle, Geom, Line, PolyVertex, Polyline};
use cad_kernel::lineweight::Lineweight;
use cad_kernel::math::Vec2;
use cad_kernel::plotstyle::{
    Offset, Orientation, PaperSize, PlotArea, PlotConfig, PlotScale, PlotStyleTable, PlotTarget,
};
use cad_kernel::{DObject, Document, Style};
use std::path::PathBuf;

fn line(a: (f64, f64), b: (f64, f64), aci: u8, mm: f32, h: u64) -> DObject {
    DObject {
        geom: Geom::Line(Line { a: Vec2::new(a.0, a.1), b: Vec2::new(b.0, b.1) }),
        style: Style { color: Color::Aci(aci), lineweight: Lineweight::Custom(mm), ..Style::default() },
        handle: h,
    }
}

fn demo_doc() -> Document {
    let mut doc = Document::default();
    // ACI 1 (red) rectangle outline via a closed polyline.
    let rect = Polyline {
        vertices: vec![
            PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
            PolyVertex { pos: Vec2::new(120.0, 0.0), bulge: 0.0 },
            PolyVertex { pos: Vec2::new(120.0, 80.0), bulge: 0.0 },
            PolyVertex { pos: Vec2::new(0.0, 80.0), bulge: 0.0 },
        ],
        closed: true,
        widths: Vec::new(),
    };
    doc.push(DObject {
        geom: Geom::Polyline(rect),
        style: Style { color: Color::Aci(1), lineweight: Lineweight::Custom(0.25), ..Style::default() },
        handle: 1,
    });
    // ACI 3 (green) diagonal.
    doc.push(line((0.0, 0.0), (120.0, 80.0), 3, 0.25, 2));
    // ACI 3 (green) second diagonal.
    doc.push(line((0.0, 80.0), (120.0, 0.0), 3, 0.25, 3));
    // ACI 5 (blue) circle in the middle.
    doc.push(DObject {
        geom: Geom::Circle(Circle { center: Vec2::new(60.0, 40.0), radius: 28.0 }),
        style: Style { color: Color::Aci(5), lineweight: Lineweight::Custom(0.25), ..Style::default() },
        handle: 4,
    });
    doc
}

fn base_cfg(path: PathBuf, scale: PlotScale) -> PlotConfig {
    PlotConfig {
        output: PlotTarget::PdfFile(path),
        paper: PaperSize::A4,
        orientation: Orientation::Landscape,
        area: PlotArea::Extents,
        scale,
        offset: Offset::Center,
        lw_scale: 1.0,
        monochrome: false,
        margins_mm: 10.0,
        plot_layout_index: None,
        ctb_tables: Default::default(),
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).ok();
    let doc = demo_doc();

    // 1 — default table, Fit.
    let out = cad_plot::plot(&doc, &PlotStyleTable::default(), &base_cfg(dir.join("1_default.pdf"), PlotScale::Fit)).unwrap();
    println!("1_default.pdf   {} bytes, {} prims", out.bytes, out.prim_count);

    // 2 — per-color pens: ACI-1 → 0.70, ACI-3 → 0.13.
    let mut table = PlotStyleTable::default();
    table.set_fixed_width(1, 0.70);
    table.set_fixed_width(3, 0.13);
    let out = cad_plot::plot(&doc, &table, &base_cfg(dir.join("2_pens.pdf"), PlotScale::Fit)).unwrap();
    println!("2_pens.pdf      {} bytes, {} prims", out.bytes, out.prim_count);

    // 3a / 3b — Fit vs 1:100 (identical stroke mm).
    cad_plot::plot(&doc, &PlotStyleTable::default(), &base_cfg(dir.join("3a_fit.pdf"), PlotScale::Fit)).unwrap();
    cad_plot::plot(&doc, &PlotStyleTable::default(), &base_cfg(dir.join("3b_1to100.pdf"), PlotScale::Ratio { model: 100.0, paper_mm: 1.0 })).unwrap();
    println!("3a_fit.pdf / 3b_1to100.pdf written (same stroke mm, different size)");

    // 4 — monochrome.
    let mut c = base_cfg(dir.join("4_mono.pdf"), PlotScale::Fit);
    c.monochrome = true;
    cad_plot::plot(&doc, &PlotStyleTable::default(), &c).unwrap();
    println!("4_mono.pdf      written (all black)");

    println!("\nAll samples in {}", dir.display());
}
