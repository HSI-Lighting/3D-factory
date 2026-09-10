//! PDF emission — turn a paper-space `Scene` into PDF bytes via printpdf 0.7.
//!
//! The only place `width_mm` becomes a device unit: `pt = mm × 72/25.4`, applied
//! at `set_outline_thickness`. printpdf treats thickness `0.0` as a 1-device-px
//! hairline, which is exactly the "0.00 = thinnest renderable" semantics.
//!
//! Cap/join: printpdf 0.7 sets the line cap / join per LAYER, not per stroke, so
//! the emitter creates one `PdfLayerReference` per used (cap, join) combo and
//! routes each stroke into its combo's layer. Mappings: Butt→Butt, Square→
//! ProjectingSquare, Round/Diamond/UseObject→Round; Miter→Miter, Bevel→Limit
//! (printpdf's name for bevel), Round/Diamond/UseObject→Round. Dither has no
//! meaning in vector output — ignored (documented).
//!
//! Known limitation: printpdf paints page layers strictly in creation order,
//! so fills/tris (the base layer) paint beneath ALL strokes, and strokes of
//! different (cap, join) combos paint in combo-first-appearance order rather
//! than scene order. Overlapping entities whose scene order put a fill above
//! an earlier stroke therefore render with the stroke on top in the PDF.

use crate::scene::{Prim, Scene};
use cad_kernel::plotstyle::{EndStyle, JoinStyle};
use printpdf::{
    Color as PdfCol, Line, LineCapStyle, LineDashPattern, LineJoinStyle, Mm, PdfDocument, Point,
    Polygon, Rgb,
};
use std::collections::HashMap;

/// Millimetres → PDF points.
const MM_TO_PT: f32 = 72.0 / 25.4;

/// Minimum visible stroke on paper (mm). A plot width of ~0 (e.g. the
/// "scale lineweights" / hairline path) would emit PDF thickness 0, which many
/// viewers render invisibly. Floor it to a fine-but-visible line. Kept BELOW the
/// 0.1 mm default (scene.rs) so it never lifts a real width — a 0.1 mm default
/// and a 0.25 mm object stay distinct, and explicit widths print exactly.
const MIN_STROKE_MM: f32 = 0.05;

fn rgb_color(rgb: (u8, u8, u8)) -> PdfCol {
    PdfCol::Rgb(Rgb::new(
        rgb.0 as f32 / 255.0,
        rgb.1 as f32 / 255.0,
        rgb.2 as f32 / 255.0,
        None,
    ))
}

fn pt(x: f64, y: f64) -> (Point, bool) {
    (Point::new(Mm(x as f32), Mm(y as f32)), false)
}

/// The printpdf cap for a scene pen cap (Diamond / UseObject → Round).
fn cap_style(cap: EndStyle) -> LineCapStyle {
    match cap {
        EndStyle::Butt => LineCapStyle::Butt,
        EndStyle::Square => LineCapStyle::ProjectingSquare,
        _ => LineCapStyle::Round,
    }
}

/// The printpdf join for a scene pen join (Diamond / UseObject → Round).
fn join_style(join: JoinStyle) -> LineJoinStyle {
    match join {
        JoinStyle::Miter => LineJoinStyle::Miter,
        JoinStyle::Bevel => LineJoinStyle::Limit,
        _ => LineJoinStyle::Round,
    }
}

/// Build a printpdf dash pattern from a paper-mm [dash, gap, …] list (empty =
/// solid) plus the dash-phase offset (paper mm — the pattern position at the
/// path start). PDF dash lengths are integer points; each is floored to ≥1.
fn dash_pattern(dash_mm: &[f32], offset_mm: f32) -> LineDashPattern {
    let to_pt = |i: usize| {
        dash_mm
            .get(i)
            .map(|&v| ((v * MM_TO_PT).round() as i64).max(1))
    };
    LineDashPattern {
        offset: (offset_mm.abs() * MM_TO_PT).round() as i64,
        dash_1: to_pt(0),
        gap_1: to_pt(1),
        dash_2: to_pt(2),
        gap_2: to_pt(3),
        dash_3: to_pt(4),
        gap_3: to_pt(5),
    }
}

/// Render a scene to PDF bytes.
pub fn scene_to_pdf_bytes(scene: &Scene, title: &str) -> Result<Vec<u8>, printpdf::Error> {
    let (doc, page, layer) = PdfDocument::new(
        title,
        Mm(scene.page_w_mm as f32),
        Mm(scene.page_h_mm as f32),
        "plot",
    );
    let page_ref = doc.get_page(page);
    let base = page_ref.get_layer(layer);
    base.set_line_cap_style(LineCapStyle::Round);
    base.set_line_join_style(LineJoinStyle::Round);

    // One layer per used (cap, join) combo — printpdf cap/join are per-layer.
    // Fills/tris stay on `base` (created first, so they paint beneath all
    // strokes; see the module docs for the z-order caveat).
    let mut combo_layers: HashMap<(u8, u8), printpdf::PdfLayerReference> = HashMap::new();

    for prim in &scene.prims {
        match prim {
            Prim::Stroke {
                pts,
                closed,
                width_mm,
                rgb,
                dash_mm,
                dash_offset_mm,
                cap,
                join,
                ..
            } => {
                let key = (*cap as u8, *join as u8);
                let l = if let Some(l) = combo_layers.get(&key) {
                    l.clone()
                } else {
                    let l = page_ref.add_layer(format!("stroke-{}-{}", key.0, key.1));
                    l.set_line_cap_style(cap_style(*cap));
                    l.set_line_join_style(join_style(*join));
                    combo_layers.insert(key, l.clone());
                    l
                };
                l.set_outline_color(rgb_color(*rgb));
                l.set_outline_thickness(width_mm.max(MIN_STROKE_MM) * MM_TO_PT);
                // Per-stroke dash (empty → solid, which also resets any prior dash).
                l.set_line_dash_pattern(dash_pattern(dash_mm, *dash_offset_mm));
                let points = pts.iter().map(|&(x, y)| pt(x, y)).collect();
                l.add_line(Line {
                    points,
                    is_closed: *closed,
                });
            }
            Prim::Fill { loops, rgb, .. } => {
                base.set_fill_color(rgb_color(*rgb));
                let rings = loops
                    .iter()
                    .map(|lp| lp.iter().map(|&(x, y)| pt(x, y)).collect())
                    .collect();
                base.add_polygon(Polygon {
                    rings,
                    mode: printpdf::path::PaintMode::Fill,
                    winding_order: printpdf::path::WindingOrder::EvenOdd,
                });
            }
            Prim::Tris { tris, rgb } => {
                base.set_fill_color(rgb_color(*rgb));
                for tri in tris {
                    let ring: Vec<(Point, bool)> = tri.iter().map(|&(x, y)| pt(x, y)).collect();
                    base.add_polygon(Polygon {
                        rings: vec![ring],
                        mode: printpdf::path::PaintMode::Fill,
                        winding_order: printpdf::path::WindingOrder::NonZero,
                    });
                }
            }
        }
    }

    doc.save_to_bytes()
}
