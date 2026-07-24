//! Render a DXF/DWG import to PNG — verification that blocks resolve correctly
//! (uses the SAME `BlockRef::transform_geom` the app renders with).
//!
//!   cargo run --release -p cad_raster --example render_dxf -- <file.dxf> [out.png]
//!   cargo run --release -p cad_raster --example render_dxf -- <file.dwg> [out.png] "<conv {in} {out}>"

use cad_kernel::{Document, Geom, Vec2};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let Some(input) = a.get(1).cloned() else { eprintln!("usage: render_dxf <file> [out.png] [conv]"); std::process::exit(1); };
    let out = a.get(2).cloned().unwrap_or_else(|| "render.png".into());
    let dxf_path = if input.to_lowercase().ends_with(".dwg") {
        let conv = a.get(3).cloned().unwrap_or_else(|| {
            format!("{}/.local/bin/dwgconv {{in}} {{out}}", std::env::var("HOME").unwrap_or_default())
        });
        let tmp = std::env::temp_dir().join("render_dxf.dxf");
        let cmd = conv.replace("{in}", &format!("'{}'", input)).replace("{out}", &format!("'{}'", tmp.display()));
        std::process::Command::new("sh").arg("-c").arg(&cmd).status().expect("converter");
        tmp.to_string_lossy().to_string()
    } else { input.clone() };

    let text = std::fs::read_to_string(&dxf_path).expect("read dxf");
    let doc = cad_io::dxf::read_dxf(&text).expect("parse dxf");

    // Resolve every dobject to world polylines (expanding blockrefs recursively).
    let mut polys: Vec<Vec<Vec2>> = Vec::new();
    for d in &doc.dobjects { emit(&d.geom, &doc, &mut polys); }
    println!("{} dobjects → {} polylines", doc.dobjects.len(), polys.len());

    // bbox
    let (mut mn, mut mx) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
    for pl in &polys { for p in pl { mn.x=mn.x.min(p.x); mn.y=mn.y.min(p.y); mx.x=mx.x.max(p.x); mx.y=mx.y.max(p.y); } }
    if !mx.x.is_finite() { eprintln!("nothing to render"); std::process::exit(2); }
    let (w, h) = (1400u32, 900u32);
    let pad = 40.0;
    let sx = (w as f64 - 2.0*pad) / (mx.x - mn.x).max(1e-9);
    let sy = (h as f64 - 2.0*pad) / (mx.y - mn.y).max(1e-9);
    let s = sx.min(sy);
    let to_px = |p: Vec2| ((pad + (p.x - mn.x)*s) as i32, (h as f64 - pad - (p.y - mn.y)*s) as i32);

    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([255,255,255]));
    for pl in &polys {
        for seg in pl.windows(2) {
            let (x0,y0) = to_px(seg[0]); let (x1,y1) = to_px(seg[1]);
            line(&mut img, x0,y0,x1,y1);
        }
    }
    img.save(&out).expect("save png");
    println!("wrote {out}  (bbox {:.0},{:.0} → {:.0},{:.0})", mn.x, mn.y, mx.x, mx.y);
}

fn emit(g: &Geom, doc: &Document, out: &mut Vec<Vec<Vec2>>) {
    use std::f64::consts::TAU;
    let n = 48usize;
    match g {
        Geom::Line(l) => out.push(vec![l.a, l.b]),
        Geom::Polyline(p) => {
            let mut v: Vec<Vec2> = p.vertices.iter().map(|x| x.pos).collect();
            if p.closed { if let Some(&f) = v.first() { v.push(f); } }
            out.push(v);
        }
        Geom::Circle(c) => out.push((0..=n).map(|i| { let t=i as f64/n as f64*TAU; Vec2::new(c.center.x+c.radius*t.cos(), c.center.y+c.radius*t.sin()) }).collect()),
        Geom::Arc(arc) => out.push((0..=n).map(|i| { let t=arc.start_angle+(i as f64/n as f64)*arc.sweep_angle; Vec2::new(arc.center.x+arc.radius*t.cos(), arc.center.y+arc.radius*t.sin()) }).collect()),
        Geom::Ellipse(e) => out.push((0..=n).map(|i| e.point_at(i as f64/n as f64*TAU)).collect()),
        Geom::EllipseArc(ea) => out.push((0..=n).map(|i| ea.ellipse.point_at(ea.start_param+(i as f64/n as f64)*ea.sweep_param)).collect()),
        Geom::Spline(s) => out.push(s.tessellate(64)),
        Geom::BlockRef(br) => {
            if let Some(b) = doc.blocks.get(br.block) {
                for cd in &b.dobjects {
                    let wg = br.transform_geom(&cd.geom, b.base);
                    emit(&wg, doc, out);   // recurse (nested blocks)
                }
            }
        }
        _ => {}
    }
}

fn line(img: &mut image::RgbImage, mut x0:i32, mut y0:i32, x1:i32, y1:i32) {
    let (w,h)=(img.width() as i32, img.height() as i32);
    let dx=(x1-x0).abs(); let dy=-(y1-y0).abs();
    let sx=if x0<x1 {1} else {-1}; let sy=if y0<y1 {1} else {-1};
    let mut err=dx+dy;
    loop {
        if x0>=0&&y0>=0&&x0<w&&y0<h { img.put_pixel(x0 as u32,y0 as u32,image::Rgb([0,0,0])); }
        if x0==x1&&y0==y1 {break;}
        let e2=2*err;
        if e2>=dy {err+=dy; x0+=sx;}
        if e2<=dx {err+=dx; y0+=sy;}
    }
}
