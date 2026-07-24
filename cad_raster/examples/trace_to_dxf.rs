//! Offline raster→DXF reference tool.
//!
//! Runs the SAME `cad_raster::trace_layer` engine the in-app Convert button
//! uses, but over the WHOLE image (no painted mask), and writes the result as a
//! DXF so it can be compared against the interactive, layer-by-layer result.
//!
//!   cargo run --release -p cad_raster --example trace_to_dxf -- <image> [out.dxf] [lines|arcs|nurbs] [max_dim]
//!
//! `max_dim` optionally downscales the longest image side before tracing
//! (faster, fewer tiny segments). Default: trace at the image's own resolution.

use cad_kernel::{Color, DObject, Document, Layer, Style};
use cad_raster::{trace_layer, FitKind, TraceParams};
use image::GrayImage;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(in_path) = args.get(1).cloned() else {
        eprintln!("usage: trace_to_dxf <image> [out.dxf] [lines|arcs|nurbs] [max_dim]");
        std::process::exit(1);
    };
    let out_path = args.get(2).cloned().unwrap_or_else(|| "trace.dxf".into());
    let fit = match args.get(3).map(|s| s.as_str()) {
        Some("arcs")  => FitKind::Arcs,
        Some("nurbs") => FitKind::Nurbs,
        _             => FitKind::Lines,
    };
    let max_dim: Option<u32> = args.get(4).and_then(|s| s.parse().ok());

    let mut img = image::open(&in_path).unwrap_or_else(|e| {
        eprintln!("cannot open {in_path}: {e}");
        std::process::exit(1);
    });
    if let Some(m) = max_dim {
        if img.width().max(img.height()) > m {
            img = img.thumbnail(m, m);   // preserves aspect
        }
    }
    let (w, h) = (img.width(), img.height());
    println!("tracing {in_path}  {w}×{h}  fit={fit:?} …");

    // Whole-image trace: mask everything in, so the engine keys off ink only.
    let mask = GrayImage::from_pixel(w, h, image::Luma([255]));
    let params = TraceParams { img_height: h, ..Default::default() };
    let t0 = std::time::Instant::now();
    let geoms = trace_layer(&mask, &img, fit, &params);
    println!("  {} geoms in {:.1}s", geoms.len(), t0.elapsed().as_secs_f32());

    let mut doc = Document::default();
    let lid = doc.layers.add(Layer { name: "TRACE".into(), color: Color::Aci(7),
                                     ..Layer::layer_zero() });
    let style = Style { layer: lid, color: Color::ByLayer, ..Style::default() };
    for g in geoms { doc.push(DObject::with_style(g, style)); }

    let dxf = cad_io::dxf::write_dxf(&doc);
    std::fs::write(&out_path, &dxf).unwrap_or_else(|e| {
        eprintln!("cannot write {out_path}: {e}");
        std::process::exit(1);
    });
    println!("wrote {} DObjects → {out_path}  ({} KB)",
             doc.dobjects.len(), dxf.len() / 1024);

    // Sibling PNG preview: rasterise the traced geometry (black on white) at the
    // source resolution, so the vector result can be eyeballed against the image.
    let png_path = out_path.trim_end_matches(".dxf").to_string() + "_preview.png";
    let mut canvas = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
    let to_px = |p: cad_kernel::Vec2| (p.x.round() as i32, (h as f64 - p.y).round() as i32);
    for d in &doc.dobjects {
        if let cad_kernel::Geom::Line(l) = &d.geom {
            let (x0, y0) = to_px(l.a);
            let (x1, y1) = to_px(l.b);
            draw_line(&mut canvas, x0, y0, x1, y1);
        }
    }
    canvas.save(&png_path).ok();
    println!("preview → {png_path}");
}

/// Bresenham line into an RGB canvas (black).
fn draw_line(img: &mut image::RgbImage, mut x0: i32, mut y0: i32, x1: i32, y1: i32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x0 >= 0 && y0 >= 0 && x0 < w && y0 < h {
            img.put_pixel(x0 as u32, y0 as u32, image::Rgb([0, 0, 0]));
        }
        if x0 == x1 && y0 == y1 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x0 += sx; }
        if e2 <= dx { err += dx; y0 += sy; }
    }
}
