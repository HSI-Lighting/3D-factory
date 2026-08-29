// SVG export — render a plot Scene to SVG XML.
//
// Reuses the existing `build_scene` pipeline for color/width/CTB/transform
// resolution, then converts Scene primitives to SVG elements. SVG uses
// top-left origin (Y-down), so coordinates are Y-flipped from the Scene's
// bottom-left convention.
//
// Output units: mm with explicit viewBox matching the page size.

use crate::scene::{Prim, Scene};
use cad_kernel::plotstyle::{EndStyle, JoinStyle};

/// SVG cap keyword for a scene pen cap (Diamond / UseObject → round).
fn cap_keyword(cap: EndStyle) -> &'static str {
    match cap {
        EndStyle::Butt => "butt",
        EndStyle::Square => "square",
        _ => "round",
    }
}

/// SVG join keyword for a scene pen join (Diamond / UseObject → round).
fn join_keyword(join: JoinStyle) -> &'static str {
    match join {
        JoinStyle::Miter => "miter",
        JoinStyle::Bevel => "bevel",
        _ => "round",
    }
}

/// Convert a plot Scene to an SVG string.
pub fn scene_to_svg(scene: &Scene, title: &str) -> String {
    let mut s = String::with_capacity(16 * 1024);
    let w = scene.page_w_mm;
    let h = scene.page_h_mm;

    s.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 {:.3} {:.3}" width="{:.3}mm" height="{:.3}mm">"#,
        w, h, w, h
    ));
    s.push('\n');
    if !title.is_empty() {
        s.push_str("  <title>");
        s.push_str(&xml_escape(title));
        s.push_str("</title>\n");
    }
    s.push_str(&format!(
        r#"  <rect x="0" y="0" width="{:.3}" height="{:.3}" fill="white" />"#,
        w, h
    ));
    s.push('\n');
    s.push_str(r#"  <g shape-rendering="geometricPrecision">"#);
    s.push('\n');

    for prim in &scene.prims {
        match prim {
            Prim::Stroke {
                pts, closed, width_mm, rgb, dash_mm, dash_offset_mm, cap, join, ..
            } => {
                if pts.len() < 2 {
                    continue;
                }
                let (r, g, b) = *rgb;
                s.push_str(&format!(
                    r##"    <path d="{}" stroke="rgb({},{},{})" stroke-width="{:.3}" fill="none" stroke-linecap="{}" stroke-linejoin="{}""##,
                    svg_path_d(pts, *closed, h),
                    r, g, b,
                    width_mm.max(0.01),
                    cap_keyword(*cap),
                    join_keyword(*join),
                ));
                if !dash_mm.is_empty() {
                    let dashes: Vec<String> = dash_mm.iter()
                        .map(|v| format!("{:.3}", v))
                        .collect();
                    s.push_str(&format!(" stroke-dasharray=\"{}\"", dashes.join(", ")));
                }
                if dash_offset_mm.abs() > 1e-6 {
                    s.push_str(&format!(" stroke-dashoffset=\"{:.3}\"", dash_offset_mm));
                }
                s.push_str(" />\n");
            }

            Prim::Fill { loops, rgb, .. } => {
                let (r, g, b) = *rgb;
                for lp in loops {
                    if lp.len() < 3 {
                        continue;
                    }
                    s.push_str(&format!(
                        r##"    <path d="{}" fill="rgb({},{},{})" fill-rule="evenodd" stroke="none" />"##,
                        svg_path_d(lp, true, h),
                        r, g, b,
                    ));
                    s.push('\n');
                }
            }

            Prim::Tris { tris, rgb } => {
                let (r, g, b) = *rgb;
                for tri in tris {
                    let y0 = h - tri[0].1;
                    let y1 = h - tri[1].1;
                    let y2 = h - tri[2].1;
                    s.push_str(&format!(
                        r##"    <polygon points="{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}" fill="rgb({},{},{})" stroke="none" />"##,
                        tri[0].0, y0, tri[1].0, y1, tri[2].0, y2,
                        r, g, b,
                    ));
                    s.push('\n');
                }
            }
        }
    }

    s.push_str("  </g>\n</svg>\n");
    s
}

/// Build an SVG path `d` attribute from points (bottom-left mm coords),
/// Y-flipped to SVG's top-left convention.
fn svg_path_d(pts: &[(f64, f64)], closed: bool, page_h_mm: f64) -> String {
    use std::fmt::Write;
    let mut d = String::with_capacity(pts.len() * 20);
    let _ = write!(d, "M {:.3} {:.3}", pts[0].0, page_h_mm - pts[0].1);
    for pt in &pts[1..] {
        let _ = write!(d, " L {:.3} {:.3}", pt.0, page_h_mm - pt.1);
    }
    if closed {
        d.push_str(" Z");
    }
    d
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;
    use cad_kernel::plotstyle::{EndStyle, JoinStyle};

    fn stroke(pts: Vec<(f64, f64)>, closed: bool, width_mm: f32, rgb: (u8, u8, u8), dash_mm: Vec<f32>) -> Prim {
        Prim::Stroke {
            pts, closed, width_mm, rgb, dash_mm,
            dash_offset_mm: 0.0,
            cap: EndStyle::Round,
            join: JoinStyle::Round,
            dither: false,
            smooth: false,
        }
    }

    #[test]
    fn empty_scene_produces_valid_svg() {
        let scene = Scene {
            page_w_mm: 297.0, page_h_mm: 210.0,
            prims: Vec::new(), skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("viewBox=\"0 0 297.000 210.000\""));
    }

    #[test]
    fn stroke_becomes_path() {
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![stroke(vec![(10.0, 10.0), (90.0, 50.0)], false, 0.25, (255, 0, 0), Vec::new())],
            skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.contains("<path"));
        assert!(svg.contains("stroke=\"rgb(255,0,0)\""));
        assert!(svg.contains("stroke-width=\"0.250\""));
        assert!(svg.contains("fill=\"none\""));
    }

    #[test]
    fn closed_path_has_z() {
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![stroke(vec![(10.0, 10.0), (90.0, 10.0), (90.0, 90.0)], true, 0.5, (0, 0, 255), vec![5.0, 2.0])],
            skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.contains(" Z\""));
        assert!(svg.contains("stroke-dasharray=\"5.000, 2.000\""));
    }

    #[test]
    fn y_coordinates_are_flipped() {
        // In Scene (bottom-left), a point at y=10 is near the bottom.
        // In SVG (top-left), it should be near the bottom: y_svg = page_h - 10.
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![stroke(vec![(50.0, 10.0), (50.0, 90.0)], false, 0.1, (0, 0, 0), Vec::new())],
            skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        // y=10 → svg y = 90.0; y=90 → svg y = 10.0
        assert!(svg.contains("M 50.000 90.000 L 50.000 10.000"));
    }

    #[test]
    fn cap_join_attributes_follow_the_pen() {
        let mut s = stroke(vec![(10.0, 10.0), (90.0, 50.0)], false, 0.25, (0, 0, 0), Vec::new());
        if let Prim::Stroke { cap, join, .. } = &mut s {
            *cap = EndStyle::Square;
            *join = JoinStyle::Bevel;
        }
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![s], skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.contains("stroke-linecap=\"square\""), "svg: {svg}");
        assert!(svg.contains("stroke-linejoin=\"bevel\""), "svg: {svg}");

        // Diamond maps to round (not expressible natively).
        let mut d = stroke(vec![(10.0, 10.0), (90.0, 50.0)], false, 0.25, (0, 0, 0), Vec::new());
        if let Prim::Stroke { cap, join, .. } = &mut d {
            *cap = EndStyle::Diamond;
            *join = JoinStyle::Diamond;
        }
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![d], skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.contains("stroke-linecap=\"round\""));
        assert!(svg.contains("stroke-linejoin=\"round\""));
    }

    #[test]
    fn dash_offset_emits_stroke_dashoffset() {
        let mut s = stroke(vec![(10.0, 50.0), (90.0, 50.0)], false, 0.25, (0, 0, 0), vec![4.0, 2.0]);
        if let Prim::Stroke { dash_offset_mm, .. } = &mut s { *dash_offset_mm = 2.5; }
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![s], skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.contains("stroke-dashoffset=\"2.500\""), "svg: {svg}");
    }

    #[test]
    fn fill_with_pattern_style_still_emits_solid_fill() {
        // A non-Solid pen fill style is expanded to pattern geometry at scene
        // build; the Fill itself still emits a solid region.
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![Prim::Fill {
                loops: vec![vec![(10.0, 10.0), (90.0, 10.0), (90.0, 90.0), (10.0, 90.0)]],
                rgb: (0, 128, 0),
                dither: false,
            }],
            skipped_dims: 0,
        };
        let svg = scene_to_svg(&scene, "test");
        assert!(svg.contains("fill=\"rgb(0,128,0)\""));
        assert!(svg.contains("fill-rule=\"evenodd\""));
    }
}
