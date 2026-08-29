//! cad_plot — the plot/print pipeline.
//!
//! `plot(doc, table, cfg)` flattens a `Document` to a PDF, applying a
//! color→thickness plot-style table (AutoCAD CTB analog). The pipeline is
//! READ-ONLY on the document: it never mutates, snapshots, or undoes.
//!
//! Pipeline: `build_scene` (flatten + resolve color/width in paper space) →
//! `emit::scene_to_pdf_bytes` (printpdf). The physical-mm lineweight invariant
//! — 0.25 mm prints 0.25 mm at any plot scale — is enforced in `scene` and
//! guarded by `tests::lineweight_is_physical_across_scales`.

pub mod emit;
pub mod scene;
pub mod svg;
pub mod png;
pub mod xform;

pub use scene::{build_scene, Prim, Scene};
pub use xform::PageXform;

use cad_kernel::plotstyle::{PlotConfig, PlotStyleTable, PlotTarget};
use cad_kernel::Document;
use std::path::PathBuf;

/// What `plot()` produced (for reporting in the command history).
#[derive(Clone, Debug)]
pub struct PlotOutcome {
    pub path:         PathBuf,
    pub bytes:        usize,
    pub prim_count:   usize,
    pub skipped_dims: usize,
}

#[derive(Debug)]
pub enum PlotError {
    /// The chosen output has no file path.
    NoOutputPath,
    /// P2 target (system printer) not implemented in the MVP.
    UnsupportedTarget,
    /// PDF backend error.
    Pdf(String),
    /// SVG backend error.
    Svg(String),
    /// Filesystem error writing the output.
    Io(std::io::Error),
}

impl std::fmt::Display for PlotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlotError::NoOutputPath => write!(f, "no output file path set"),
            PlotError::UnsupportedTarget => write!(f, "system-printer output is not implemented (PDF/SVG only)"),
            PlotError::Pdf(e) => write!(f, "PDF backend error: {e}"),
            PlotError::Svg(e) => write!(f, "SVG backend error: {e}"),
            PlotError::Io(e) => write!(f, "write error: {e}"),
        }
    }
}
impl std::error::Error for PlotError {}

/// Flatten `doc` and write a PDF per `cfg`, applying the plot-style `table`.
/// Read-only on the document.
pub fn plot(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
) -> Result<PlotOutcome, PlotError> {
    let path = match &cfg.output {
        PlotTarget::PdfFile(p) => {
            if p.as_os_str().is_empty() {
                return Err(PlotError::NoOutputPath);
            }
            p.clone()
        }
        PlotTarget::SystemPrinter(_) => return Err(PlotError::UnsupportedTarget),
    };

    let scene = build_scene(doc, table, cfg);
    let bytes = emit::scene_to_pdf_bytes(&scene, "RUST-AutoRASM plot")
        .map_err(|e| PlotError::Pdf(format!("{e:?}")))?;
    std::fs::write(&path, &bytes).map_err(PlotError::Io)?;

    Ok(PlotOutcome {
        path,
        bytes: bytes.len(),
        prim_count: scene.prims.len(),
        skipped_dims: scene.skipped_dims,
    })
}

/// Flatten and render to PDF bytes without touching the filesystem.
pub fn plot_to_bytes(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
) -> Result<Vec<u8>, PlotError> {
    let scene = build_scene(doc, table, cfg);
    emit::scene_to_pdf_bytes(&scene, "RUST-AutoRASM plot")
        .map_err(|e| PlotError::Pdf(format!("{e:?}")))
}

/// Flatten `doc` and write an SVG file per `cfg`, applying the plot-style
/// `table`. Read-only on the document.
pub fn export_svg(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
    path: &std::path::Path,
) -> Result<usize, PlotError> {
    Ok(export_svg_meta(doc, table, cfg, path)?.bytes)
}

/// `export_svg` + a `PlotOutcome` (byte count, skipped dimensions) for
/// reporting — same output, no double flattening.
pub fn export_svg_meta(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
    path: &std::path::Path,
) -> Result<PlotOutcome, PlotError> {
    let scene = build_scene(doc, table, cfg);
    let svg_str = svg::scene_to_svg(&scene, "RUST-AutoRASM plot");
    std::fs::write(path, svg_str.as_bytes()).map_err(PlotError::Io)?;
    Ok(PlotOutcome {
        path: path.to_path_buf(),
        bytes: svg_str.len(),
        prim_count: scene.prims.len(),
        skipped_dims: scene.skipped_dims,
    })
}

/// Flatten and render to SVG string (for preview / tests).
pub fn export_svg_string(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
) -> Result<String, PlotError> {
    let scene = build_scene(doc, table, cfg);
    Ok(svg::scene_to_svg(&scene, "RUST-AutoRASM plot"))
}

/// Flatten `doc` and write a PNG file at `dpi` (default 300), applying the
/// plot-style `table`. Read-only on the document.
pub fn export_png(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
    path: &std::path::Path,
    dpi: f32,
) -> Result<usize, PlotError> {
    Ok(export_png_meta(doc, table, cfg, path, dpi)?.bytes)
}

/// `export_png` + a `PlotOutcome` (byte count, skipped dimensions) for
/// reporting — same output, no double flattening.
pub fn export_png_meta(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
    path: &std::path::Path,
    dpi: f32,
) -> Result<PlotOutcome, PlotError> {
    let scene = build_scene(doc, table, cfg);
    let bytes = png::scene_to_png_bytes(&scene, dpi)
        .map_err(PlotError::Pdf)?; // reuse Pdf variant for raster errors
    std::fs::write(path, &bytes).map_err(PlotError::Io)?;
    Ok(PlotOutcome {
        path: path.to_path_buf(),
        bytes: bytes.len(),
        prim_count: scene.prims.len(),
        skipped_dims: scene.skipped_dims,
    })
}

/// Flatten and render to PNG bytes (for preview / tests).
pub fn export_png_bytes(
    doc: &Document,
    table: &PlotStyleTable,
    cfg: &PlotConfig,
    dpi: f32,
) -> Result<Vec<u8>, PlotError> {
    let scene = build_scene(doc, table, cfg);
    png::scene_to_png_bytes(&scene, dpi)
        .map_err(PlotError::Pdf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::color::Color;
    use cad_kernel::geom::{Geom, Line};
    use cad_kernel::lineweight::Lineweight;
    use cad_kernel::math::Vec2;
    use cad_kernel::plotstyle::{
        Offset, Orientation, PaperSize, PlotArea, PlotConfig, PlotScale, PlotStyleTable, PlotTarget,
    };
    use cad_kernel::{DObject, Document, Style};

    /// A doc with one ACI-1 line carrying a 0.50 mm lineweight, on plottable
    /// visible layer 0.
    fn doc_with_aci1_line(lw: Lineweight) -> Document {
        let mut doc = Document::default();
        let style = Style {
            color: Color::Aci(1),
            lineweight: lw,
            ..Style::default()
        };
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(100.0, 50.0) }),
            style,
            handle: 1,
        });
        doc
    }

    fn cfg(scale: PlotScale) -> PlotConfig {
        PlotConfig {
            output: PlotTarget::PdfFile(std::path::PathBuf::from("mem")),
            paper: PaperSize::A3,
            orientation: Orientation::Landscape,
            area: PlotArea::Extents,
            scale,
            offset: Offset::Center,
            lw_scale: 1.0,
            monochrome: false,
            margins_mm: 5.0,
            plot_layout_index: None,
            ctb_tables: Default::default(),
        }
    }

    fn stroke_widths(scene: &Scene) -> Vec<f32> {
        scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Stroke { width_mm, .. } => Some(*width_mm),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn lineweight_is_physical_across_scales() {
        // THE #1 invariant: a 0.50 mm object prints 0.50 mm at Fit AND at 1:100.
        let doc = doc_with_aci1_line(Lineweight::Custom(0.50));
        let table = PlotStyleTable::default();

        let fit = build_scene(&doc, &table, &cfg(PlotScale::Fit));
        let hundred = build_scene(&doc, &table, &cfg(PlotScale::Ratio { model: 100.0, paper_mm: 1.0 }));

        let wf = stroke_widths(&fit);
        let wh = stroke_widths(&hundred);
        assert_eq!(wf, vec![0.50]);
        assert_eq!(wh, vec![0.50], "lineweight must NOT scale with plot scale");

        // And the coordinates DID scale differently (sanity: the transform works).
        let pf = match &fit.prims[0] { Prim::Stroke { pts, .. } => pts.clone(), _ => panic!() };
        let ph = match &hundred.prims[0] { Prim::Stroke { pts, .. } => pts.clone(), _ => panic!() };
        assert_ne!(pf, ph, "positions should differ between Fit and 1:100");
    }

    #[test]
    fn default_width_plots_thin_and_distinct_from_025() {
        // "0.00"/Default plots at the thin 0.1 mm default; an explicit 0.25 plots
        // exactly — so the two are clearly different on paper (owner survey).
        let table = PlotStyleTable::default();
        let w0 = stroke_widths(&build_scene(
            &doc_with_aci1_line(Lineweight::Default), &table, &cfg(PlotScale::Fit)));
        let w25 = stroke_widths(&build_scene(
            &doc_with_aci1_line(Lineweight::Custom(0.25)), &table, &cfg(PlotScale::Fit)));
        assert_eq!(w0, vec![0.10], "default/0.00 → thin 0.1 mm");
        assert_eq!(w25, vec![0.25], "explicit 0.25 → exactly 0.25 mm");
        assert_ne!(w0, w25, "0.00 and 0.25 must NOT be the same width");
    }

    #[test]
    fn window_clips_geometry_to_the_rect() {
        // Line (0,0)–(100,50). Window (10,10)–(40,40): the visible part is the
        // (20,10)–(40,20) run → exactly one stroke. A window far away → nothing.
        let doc = doc_with_aci1_line(Lineweight::Custom(0.50));
        let table = PlotStyleTable::default();

        let mut c = cfg(PlotScale::Fit);
        c.area = PlotArea::Window { min: Vec2::new(10.0, 10.0), max: Vec2::new(40.0, 40.0) };
        let inside = build_scene(&doc, &table, &c);
        assert_eq!(inside.prims.len(), 1, "a crossing line clips to one stroke");

        let mut c2 = cfg(PlotScale::Fit);
        c2.area = PlotArea::Window { min: Vec2::new(200.0, 200.0), max: Vec2::new(300.0, 300.0) };
        let outside = build_scene(&doc, &table, &c2);
        assert_eq!(outside.prims.len(), 0, "a line outside the window is fully clipped");
    }

    #[test]
    fn fixed_pen_overrides_object_width() {
        // ACI-1 pen = 0.70 mm forces 0.70 regardless of the object's 0.25.
        let doc = doc_with_aci1_line(Lineweight::Custom(0.25));
        let mut table = PlotStyleTable::default();
        table.set_fixed_width(1, 0.70);
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Fit));
        assert_eq!(stroke_widths(&scene), vec![0.70]);
    }

    #[test]
    fn useobject_emits_object_lineweight() {
        let doc = doc_with_aci1_line(Lineweight::Custom(0.13));
        let table = PlotStyleTable::default(); // all UseObject
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Fit));
        assert_eq!(stroke_widths(&scene), vec![0.13]);
    }

    #[test]
    fn lw_scale_multiplies_width() {
        let doc = doc_with_aci1_line(Lineweight::Custom(0.20));
        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.lw_scale = 2.0;
        let scene = build_scene(&doc, &table, &c);
        assert_eq!(stroke_widths(&scene), vec![0.40]);
    }

    #[test]
    fn monochrome_forces_black() {
        let doc = doc_with_aci1_line(Lineweight::Custom(0.25)); // red object
        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.monochrome = true;
        let scene = build_scene(&doc, &table, &c);
        let rgb = match &scene.prims[0] { Prim::Stroke { rgb, .. } => *rgb, _ => panic!() };
        assert_eq!(rgb, (0, 0, 0));
    }

    #[test]
    fn asis_color_is_object_color() {
        let doc = doc_with_aci1_line(Lineweight::Custom(0.25)); // ACI 1 = red
        let table = PlotStyleTable::default();
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Fit));
        let rgb = match &scene.prims[0] { Prim::Stroke { rgb, .. } => *rgb, _ => panic!() };
        assert_eq!(rgb, (255, 0, 0));
    }

    #[test]
    fn plot_writes_a_pdf_file() {
        let doc = doc_with_aci1_line(Lineweight::Custom(0.25));
        let table = PlotStyleTable::default();
        let dir = std::env::temp_dir();
        let path = dir.join("cad_plot_test_out.pdf");
        let mut c = cfg(PlotScale::Fit);
        c.output = PlotTarget::PdfFile(path.clone());
        let out = plot(&doc, &table, &c).expect("plot ok");
        assert!(out.bytes > 200);
        let bytes = std::fs::read(&path).expect("read back");
        assert!(bytes.starts_with(b"%PDF"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn non_plottable_layer_is_skipped() {
        let mut doc = doc_with_aci1_line(Lineweight::Custom(0.25));
        // Mark layer 0 non-plottable → nothing emitted.
        if let Some(l0) = doc.layers.get_mut(0) {
            l0.plottable = false;
        }
        let table = PlotStyleTable::default();
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Fit));
        assert!(stroke_widths(&scene).is_empty(), "non-plottable layer must not plot");
    }

    #[test]
    fn layout_plot_maps_viewports_1to1_matching_layout_tab() {
        // The layout tab renders paper space with the sheet corner as origin.
        // The layout plot must use the SAME mapping — no printable-margin
        // shift — so exported viewport frames / content sit exactly where
        // they appear on screen.
        use cad_kernel::geom::Line;
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};

        let mut doc = Document::default();
        // Model content: a 10-unit horizontal line through the origin.
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
            style: Style {
                color: Color::Aci(1),
                lineweight: Lineweight::Custom(0.25),
                ..Style::default()
            },
            handle: 1,
        });
        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        // Viewport frame at paper rect (30,25)-(70,55), model centre = origin,
        // 1:1 view scale.
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let scene = build_scene(&doc, &table, &c);

        // The viewport FRAME must land at its paper rect exactly (no +margin).
        let frame_corners: Vec<(f64, f64)> =
            vec![(30.0, 25.0), (70.0, 25.0), (70.0, 55.0), (30.0, 55.0)];
        let frame_found = scene.prims.iter().any(|p| {
            if let Prim::Stroke { pts, closed: true, .. } = p {
                pts.len() == frame_corners.len()
                    && pts
                        .iter()
                        .zip(&frame_corners)
                        .all(|(a, b)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9)
            } else {
                false
            }
        });
        assert!(frame_found, "viewport frame must sit at its layout rect (no margin shift)");

        // Model content maps through the viewport camera: the line (0,0)-(10,0)
        // centred on the viewport rect centre (50,40) → (50,40)-(60,40).
        let model_mapped = scene.prims.iter().any(|p| {
            if let Prim::Stroke { pts, closed: false, .. } = p {
                pts.len() == 2
                    && (pts[0].0 - 50.0).abs() < 1e-9
                    && (pts[0].1 - 40.0).abs() < 1e-9
                    && (pts[1].0 - 60.0).abs() < 1e-9
                    && (pts[1].1 - 40.0).abs() < 1e-9
            } else {
                false
            }
        });
        assert!(model_mapped, "viewport model content must be centred on the viewport rect");
    }

    #[test]
    fn layout_plot_applies_saved_ctb_per_aci_rules() {
        // A layout whose CTB is a SAVED table: the table's per-ACI colour rules
        // (plot_color override here) must apply to the viewport's model content.
        use cad_kernel::geom::Line;
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};

        let mut doc = Document::default();
        // Model content: a 10-unit line in ACI 1 (red).
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
            style: Style {
                color: Color::Aci(1),
                lineweight: Lineweight::Custom(0.25),
                ..Style::default()
            },
            handle: 1,
        });
        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        layout.ctb_name = Some("shop".into());
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let mut shop = PlotStyleTable::named("shop");
        shop.style_mut(1).plot_color = cad_kernel::plotstyle::PlotColor::Black;
        c.ctb_tables.insert("shop".into(), shop);

        let scene = build_scene(&doc, &table, &c);
        // The model line (starts at the viewport rect centre (50,40)) must plot
        // BLACK through the saved CTB's ACI-1 rule.
        let model_rgb = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, rgb, .. } if pts.len() == 2
                && (pts[0].0 - 50.0).abs() < 1e-9 && (pts[0].1 - 40.0).abs() < 1e-9
                => Some(*rgb),
            _ => None,
        });
        assert_eq!(model_rgb, Some((0, 0, 0)),
            "saved CTB must apply its per-ACI plot color");
    }

    #[test]
    fn pattern_hatch_emits_lines_not_a_solid_fill() {
        // A PATTERN hatch must expand to its pattern line segments in the
        // scene (plotted like AutoCAD) — never a solid fill.
        let doc = doc_with_hatch(1);
        // Swap the hatch's pattern to ANSI31 (handle 2 = the hatch).
        let mut doc = doc;
        if let cad_kernel::Geom::Hatch(h) = &mut doc.dobjects[1].geom {
            h.pattern = cad_kernel::geom::HatchPattern::Pattern {
                name: "ANSI31".into(),
                scale: 2.0,
                angle_deg: 0.0,
            };
        }
        let table = PlotStyleTable::default();
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Ratio { model: 1.0, paper_mm: 1.0 }));

        // The boundary polyline strokes are there; the pattern must add many
        // line segments and NOT a fill (the doc has no solid hatch).
        let strokes: Vec<&Vec<(f64, f64)>> = scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Stroke { pts, .. } => Some(pts),
                _ => None,
            })
            .collect();
        assert!(strokes.len() >= 3,
            "ANSI31 over a 20×20 square must add pattern lines, got {} strokes", strokes.len());
        assert!(!scene.prims.iter().any(|p| matches!(p, Prim::Fill { .. })),
            "a pattern hatch must not emit a solid Fill");
        // The 4 boundary segments MERGE into one closed stroke (join styles
        // apply across the square's corners) — a 4-point closed loop.
        assert!(scene.prims.iter().any(|p| matches!(
            p, Prim::Stroke { pts, closed: true, .. } if pts.len() == 4)),
            "the boundary must merge into one closed 4-point stroke");
        // The boundary is 4 segments; the pattern lines are the rest. The
        // union bbox of ALL of them must still be the 20×20 square — pattern
        // lines that escaped the boundary would stretch it.
        let mut mn = (f64::INFINITY, f64::INFINITY);
        let mut mx = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for pts in &strokes {
            for &(x, y) in pts.iter() {
                mn.0 = mn.0.min(x); mn.1 = mn.1.min(y);
                mx.0 = mx.0.max(x); mx.1 = mx.1.max(y);
            }
        }
        assert!((mx.0 - mn.0 - 20.0).abs() < 1e-6 && (mx.1 - mn.1 - 20.0).abs() < 1e-6,
            "pattern + boundary bbox must stay 20×20, got {:.1}×{:.1}",
            mx.0 - mn.0, mx.1 - mn.1);
    }

    #[test]
    fn saved_ctb_cap_and_join_reach_the_scene() {
        // A saved CTB's end_style / join_style must reach the scene Stroke
        // prims (they drive the PDF/SVG/PNG cap/join) — and UseObject falls
        // back to Round defaults.
        use cad_kernel::geom::Line;
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};
        use cad_kernel::plotstyle::{EndStyle, JoinStyle};

        let mut doc = Document::default();
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
            style: Style { color: Color::Aci(1), ..Style::default() },
            handle: 1,
        });
        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        layout.ctb_name = Some("shop".into());
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let mut shop = PlotStyleTable::named("shop");
        shop.style_mut(1).end_style = EndStyle::Square;
        shop.style_mut(1).join_style = JoinStyle::Bevel;
        c.ctb_tables.insert("shop".into(), shop);
        let scene = build_scene(&doc, &table, &c);
        let model = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, cap, join, .. } if pts.len() == 2
                && (pts[0].0 - 50.0).abs() < 1e-9 && (pts[0].1 - 40.0).abs() < 1e-9
                => Some((*cap, *join)),
            _ => None,
        }).expect("model line stroke");
        assert_eq!(model, (EndStyle::Square, JoinStyle::Bevel),
            "saved CTB cap/join must reach the scene strokes");

        // Built-in CTB (no saved table) → the Round defaults.
        let mut c2 = cfg(PlotScale::Fit);
        c2.plot_layout_index = Some(0);
        c2.ctb_tables.clear();
        doc.layouts[0].ctb_name = Some("grayscale".into());
        let scene2 = build_scene(&doc, &table, &c2);
        let model2 = scene2.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, cap, join, .. } if pts.len() == 2
                && (pts[0].0 - 50.0).abs() < 1e-9 && (pts[0].1 - 40.0).abs() < 1e-9
                => Some((*cap, *join)),
            _ => None,
        }).expect("model line stroke");
        assert_eq!(model2, (EndStyle::Round, JoinStyle::Round),
            "built-in CTBs resolve to the Round defaults");
    }

    #[test]
    fn layout_plot_applies_saved_ctb_width_and_linetype() {
        // A layout whose CTB is a SAVED table: Fixed pen width wins over the
        // object's lineweight, and a PlotLinetype::Id override dashes the line.
        use cad_kernel::geom::Line;
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};
        use cad_kernel::plotstyle::{PlotLinetype, PlotWidth};

        let mut doc = Document::default();
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
            style: Style {
                color: Color::Aci(1),
                lineweight: Lineweight::Custom(0.25),
                ..Style::default()
            },
            handle: 1,
        });
        // A dashed linetype (id = the table's last index).
        doc.linetypes.linetypes.push(cad_kernel::linetype::Linetype::new("DASH", &[4.0, -2.0]));
        let dash_id = (doc.linetypes.linetypes.len() - 1) as u32;

        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        layout.ctb_name = Some("shop".into());
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let mut shop = PlotStyleTable::named("shop");
        shop.style_mut(1).lineweight = PlotWidth::Fixed(0.70);
        shop.style_mut(1).linetype = PlotLinetype::Id(dash_id);
        c.ctb_tables.insert("shop".into(), shop);

        let scene = build_scene(&doc, &table, &c);
        let model = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, width_mm, dash_mm, .. } if pts.len() == 2
                && (pts[0].0 - 50.0).abs() < 1e-9 && (pts[0].1 - 40.0).abs() < 1e-9
                => Some((*width_mm, dash_mm.clone())),
            _ => None,
        }).expect("model line stroke");
        assert!((model.0 - 0.70).abs() < 1e-6,
            "CTB Fixed pen width must win over the object lineweight: {}", model.0);
        assert_eq!(model.1, vec![4.0_f32, 2.0_f32],
            "CTB linetype override must dash the line at world scale 1:1");
    }

    #[test]
    fn layout_plot_clips_viewport_content_to_the_frame() {
        // Model content must NOT spill past the viewport frame in the output
        // files (the layout tab clips; the scene must too).
        use cad_kernel::geom::Line;
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};

        let mut doc = Document::default();
        // A long model line that would reach far past the viewport frame.
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(-100.0, 0.0), b: Vec2::new(100.0, 0.0) }),
            style: Style {
                color: Color::Aci(1),
                lineweight: Lineweight::Custom(0.25),
                ..Style::default()
            },
            handle: 1,
        });
        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        // Viewport frame at paper rect (30,25)-(70,55), model centre = origin, 1:1.
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let scene = build_scene(&doc, &table, &c);

        // The model line must be clipped to the viewport rect: (30,40)-(70,40).
        let line = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, width_mm, .. }
                if *width_mm == 0.25 && pts.len() == 2
                    && (pts[0].1 - 40.0).abs() < 1e-9
                => Some(pts.clone()),
            _ => None,
        }).expect("clipped model line stroke");
        assert!((line[0].0 - 30.0).abs() < 1e-6 && (line[0].1 - 40.0).abs() < 1e-6,
            "left endpoint must sit on the frame edge: {:?}", line);
        assert!((line[1].0 - 70.0).abs() < 1e-6 && (line[1].1 - 40.0).abs() < 1e-6,
            "right endpoint must sit on the frame edge: {:?}", line);
    }

    #[test]
    fn polyline_merges_into_one_stroke_so_joins_apply() {
        // A 3-vertex polyline must become ONE 3-point stroke (line join styles
        // can only apply within a multi-point path); a closed square becomes
        // ONE 4-point closed stroke (its seam gets the join style, not caps).
        use cad_kernel::geom::{Polyline, PolyVertex};
        let mut doc = Document::default();
        doc.push(DObject {
            geom: Geom::Polyline(Polyline {
                vertices: vec![
                    PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                ],
                closed: false,
                widths: Vec::new(),
            }),
            style: Style { color: Color::Aci(1), ..Style::default() },
            handle: 1,
        });
        doc.push(DObject {
            geom: Geom::Polyline(Polyline {
                vertices: vec![
                    PolyVertex { pos: Vec2::new(0.0, 20.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(10.0, 20.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(10.0, 30.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(0.0, 30.0), bulge: 0.0 },
                ],
                closed: true,
                widths: Vec::new(),
            }),
            style: Style { color: Color::Aci(1), ..Style::default() },
            handle: 2,
        });

        let table = PlotStyleTable::default();
        let scene = build_scene(&doc, &table,
            &cfg(PlotScale::Ratio { model: 1.0, paper_mm: 1.0 }));

        let open = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, .. } if pts.len() == 3 => Some(pts.clone()),
            _ => None,
        }).expect("the open polyline must be one 3-point stroke");
        assert_eq!(open.len(), 3);

        let closed = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: true, .. } if pts.len() == 4 => Some(pts.clone()),
            _ => None,
        }).expect("the closed polyline must be one 4-point closed stroke");
        assert_eq!(closed.len(), 4, "no duplicated closing point on the seam");

        // And the SVG must carry ONE multi-point path per stroke so
        // stroke-linejoin actually applies.
        let svg = export_svg_string(&doc, &table,
            &cfg(PlotScale::Ratio { model: 1.0, paper_mm: 1.0 })).unwrap();
        let multi = svg.matches(" L ").count();
        assert!(multi >= 4, "SVG must emit multi-point paths: {svg}");
    }

    #[test]
    fn builtin_ctb_viewports_do_not_leak_doc_plot_styles_widths() {
        // A viewport whose CTB is a BUILT-IN (no saved .pst) must use the
        // OBJECT's own width/linetype — never the document's plot-style table.
        // The editor holds whatever CTB is being edited in doc.plot_styles, so
        // a fallback to it would make one CTB's 0.8 mm pen change the OTHER
        // viewport's lineweights.
        use cad_kernel::geom::Line;
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};
        use cad_kernel::plotstyle::{PlotLinetype, PlotWidth};

        let mut doc = Document::default();
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
            style: Style {
                color: Color::Aci(1),
                lineweight: Lineweight::Custom(0.25),
                ..Style::default()
            },
            handle: 1,
        });
        doc.linetypes.linetypes.push(cad_kernel::linetype::Linetype::new("DASH", &[4.0, -2.0]));
        let dash_id = (doc.linetypes.linetypes.len() - 1) as u32;

        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        // No layout CTB → monochrome built-in (no saved file).
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        vd.ctb_name = None; // inherit → monochrome built-in
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        // The doc's plot-style table — say the CTB editor currently holds
        // another viewport's CTB with a 0.8 mm ACI-1 pen + a dash override.
        let mut table = PlotStyleTable::default();
        table.style_mut(1).lineweight = PlotWidth::Fixed(0.8);
        table.style_mut(1).linetype = PlotLinetype::Id(dash_id);

        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let scene = build_scene(&doc, &table, &c);

        let model = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, width_mm, dash_mm, .. } if pts.len() == 2
                && (pts[0].0 - 50.0).abs() < 1e-9 && (pts[0].1 - 40.0).abs() < 1e-9
                => Some((*width_mm, dash_mm.clone())),
            _ => None,
        }).expect("model line stroke");
        assert!((model.0 - 0.25).abs() < 1e-6,
            "built-in CTB must keep the OBJECT width, not the doc table's 0.8: {}", model.0);
        assert!(model.1.is_empty(),
            "built-in CTB must keep the OBJECT linetype, not the doc table's dash: {:?}", model.1);
    }

    /// A doc with one closed square polyline (boundary) + one solid Hatch on it.
    fn doc_with_hatch(aci: u8) -> Document {
        use cad_kernel::geom::{Hatch, HatchPattern, Polyline, PolyVertex};
        let mut doc = Document::default();
        let poly = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(20.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(20.0, 20.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 20.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        };
        doc.push(DObject {
            geom: Geom::Polyline(poly),
            style: Style {
                color: Color::Aci(aci),
                lineweight: Lineweight::Custom(0.25), // distinct from the 0.1 bars
                ..Style::default()
            },
            handle: 1,
        });
        doc.push(DObject {
            geom: Geom::Hatch(Hatch {
                boundary_handles: vec![1],
                pattern: HatchPattern::Solid,
            }),
            style: Style { color: Color::Aci(aci), ..Style::default() },
            handle: 2,
        });
        doc
    }

    #[test]
    fn fill_style_horizontal_bars_emits_clipped_bar_strokes() {
        use cad_kernel::plotstyle::FillStyle;
        let doc = doc_with_hatch(1);
        let mut table = PlotStyleTable::default();
        table.style_mut(1).fill_style = FillStyle::HorizontalBars;
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Ratio { model: 1.0, paper_mm: 1.0 }));

        // The hatch's Fill prim, plus the pattern bar strokes.
        let fill_bbox = scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Fill { loops, .. } => Some(loops[0].clone()),
                _ => None,
            })
            .next()
            .expect("hatch must emit its solid Fill");
        let (fx0, fy0) = fill_bbox.iter().fold(
            (f64::INFINITY, f64::INFINITY),
            |(ax, ay), &(x, y)| (ax.min(x), ay.min(y)));
        let (fx1, fy1) = fill_bbox.iter().fold(
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
            |(ax, ay), &(x, y)| (ax.max(x), ay.max(y)));
        let bars: Vec<&(f64, f64)> = scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Stroke { width_mm, pts, dash_mm, .. }
                    if *width_mm == 0.1 && dash_mm.is_empty() && pts.len() == 2 => Some(&pts[0]),
                _ => None,
            })
            .collect();
        assert!(bars.len() >= 10, "expected ~10 horizontal bars (20 mm / 1.5), got {}", bars.len());
        // Every bar is horizontal (equal y) and inside the fill's bbox.
        for prim in scene.prims.iter() {
            if let Prim::Stroke { width_mm, pts, .. } = prim {
                if *width_mm == 0.1 && pts.len() == 2 {
                    assert!((pts[0].1 - pts[1].1).abs() < 1e-9, "bars must be horizontal: {:?}", pts);
                    for &(x, y) in pts.iter() {
                        assert!(x >= fx0 - 1e-6 && x <= fx1 + 1e-6
                            && y >= fy0 - 1e-6 && y <= fy1 + 1e-6,
                            "bars must be clipped to the polygon bbox: {:?}", pts);
                    }
                }
            }
        }
    }

    #[test]
    fn adaptive_dash_phase_rotates_so_the_end_lands_in_a_dash() {
        // A 5-unit line with linetype [4, -2] at 1:1: the end lands in the gap
        // (5 mod 6 = 5), so the adaptive phase must shift the pattern 5 mm.
        let mut doc = Document::default();
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(5.0, 0.0) }),
            style: Style { color: Color::Aci(1), ..Style::default() },
            handle: 1,
        });
        doc.linetypes.linetypes.push(cad_kernel::linetype::Linetype::new("DASH", &[4.0, -2.0]));
        let dash_id = (doc.linetypes.linetypes.len() - 1) as u32;
        doc.dobjects[0].style.linetype = dash_id;

        let table = PlotStyleTable::default(); // adaptive on (default)
        let scene = build_scene(&doc, &table, &cfg(PlotScale::Ratio { model: 1.0, paper_mm: 1.0 }));
        let off = match &scene.prims[0] {
            Prim::Stroke { dash_offset_mm, .. } => *dash_offset_mm,
            _ => panic!("expected a stroke"),
        };
        assert!((off - 5.0).abs() < 1e-3,
            "adaptive phase must rotate so the end lands in a dash: {off}");

        // adaptive OFF → no phase rotation.
        let mut t2 = PlotStyleTable::default();
        t2.style_mut(1).adaptive = false;
        let scene2 = build_scene(&doc, &t2, &cfg(PlotScale::Ratio { model: 1.0, paper_mm: 1.0 }));
        let off2 = match &scene2.prims[0] {
            Prim::Stroke { dash_offset_mm, .. } => *dash_offset_mm,
            _ => panic!("expected a stroke"),
        };
        assert_eq!(off2, 0.0, "adaptive off must not rotate the pattern");
    }

    #[test]
    fn builtin_ctb_grayscale_is_rec601_luminance() {
        // Layout plot with the built-in "grayscale" CTB: a red ACI-1 model line
        // must plot at Rec.601 luminance (0.299·255 ≈ 76), NOT the channel
        // average (85).
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};

        let mut doc = Document::default();
        doc.push(DObject {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
            style: Style { color: Color::Aci(1), ..Style::default() },
            handle: 1,
        });
        let mut layout = Layout::new("L1", PaperSize::A4, Orientation::Portrait);
        layout.ctb_name = Some("grayscale".into());
        let vg = ViewportGeom {
            center: Vec2::new(50.0, 40.0),
            width: 40.0,
            height: 30.0,
            model_center: Vec2::new(0.0, 0.0),
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        };
        let ent = cad_kernel::DObject::new(Geom::Viewport(vg.clone()));
        let h = ent.handle;
        layout.entities.push(ent);
        let mut vd = ViewportData::new((30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let table = PlotStyleTable::default();
        let mut c = cfg(PlotScale::Fit);
        c.plot_layout_index = Some(0);
        let scene = build_scene(&doc, &table, &c);

        let model_rgb = scene.prims.iter().find_map(|p| match p {
            Prim::Stroke { pts, closed: false, rgb, .. } if pts.len() == 2
                && (pts[0].0 - 50.0).abs() < 1e-9 && (pts[0].1 - 40.0).abs() < 1e-9
                => Some(*rgb),
            _ => None,
        }).expect("model line stroke");
        assert_eq!(model_rgb, (76, 76, 76),
            "built-in grayscale must use Rec.601 luminance, not the average");
    }
}
