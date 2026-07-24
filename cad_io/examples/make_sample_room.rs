//! Generate a sample room DXF for testing the SIMLUX lux workflow.
//!
//! Two NAMED layers so the per-layer import + per-layer extrude height is
//! demonstrable end-to-end:
//!   * `WALLS`     — a closed 8 × 5 m room  (extrude ~3.0 m → walls + floor + ceiling)
//!   * `PARTITION` — one interior divider   (extrude ~1.2 m → a low wall)
//!
//! Run:  cargo run -p cad_io --example make_sample_room
//! Output: <repo>/sample_room.dxf  (Open it in simLUX, then SIMLUX ▸ Light ▸ Import).

use cad_io::dxf::write_dxf;
use cad_kernel::{Color, DObject, Document, Geom, Layer, Line, PolyVertex, Polyline, Style, Vec2};

fn main() {
    let mut doc = Document::default();

    // --- WALLS: a closed rectangular room (gets floor + ceiling on extrude) ---
    let walls = doc.layers.add(Layer {
        name: "WALLS".into(),
        color: Color::Aci(7),
        ..Layer::layer_zero()
    });
    let rect: Vec<PolyVertex> = [(0.0, 0.0), (8.0, 0.0), (8.0, 5.0), (0.0, 5.0)]
        .iter()
        .map(|&(x, y)| PolyVertex { pos: Vec2::new(x, y), bulge: 0.0 })
        .collect();
    doc.push(DObject::with_style(
        Geom::Polyline(Polyline { vertices: rect, closed: true, widths: Vec::new() }),
        Style::on_layer(walls),
    ));

    // --- PARTITION: one interior divider line (a lower wall) ---
    let part = doc.layers.add(Layer {
        name: "PARTITION".into(),
        color: Color::Aci(3),
        ..Layer::layer_zero()
    });
    doc.push(DObject::with_style(
        Geom::Line(Line { a: Vec2::new(5.0, 0.0), b: Vec2::new(5.0, 3.2) }),
        Style::on_layer(part),
    ));

    let dxf = write_dxf(&doc);
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/../sample_room.dxf");
    std::fs::write(out, &dxf).expect("write sample_room.dxf");
    eprintln!(
        "wrote {out} ({} bytes) — layers: WALLS (closed 8x5 room), PARTITION (divider)",
        dxf.len()
    );
}
