// RSM — RUST_CAD's native binary Document format.
//
// Design goals:
//   1. **Fast** — direct field-by-field little-endian write; no JSON, no
//      varint, no compression in the v1 spec. Target: serialize 5M dobjects
//      in well under a second.
//   2. **Lossless** — every field in `Document` round-trips exactly.
//      Where DXF compresses or paraphrases (e.g. ACI color → ByLayer
//      sentinel), RSM stores the original.
//   3. **Versioned** — header carries `(magic, version)` so a v2 reader
//      can refuse a v3 file or upgrade a v1 file. Today's spec is v1.
//   4. **No deps** — hand-rolled; no serde, no bincode, no postcard. The
//      whole format is in this file.
//
// Layout (little-endian throughout):
//
//   magic    [u8;4] = "RSM\x01"     (last byte = format version)
//   version  u16    = 1
//   pad      u16    = 0
//
//   --- LinetypeTable ---
//   count    u32
//   per linetype:
//     name        len: u32, bytes
//     description len: u32, bytes
//     pattern     len: u32, f32 …
//
//   --- LayerTable ---
//   active      u32
//   count       u32
//   per layer:
//     name        len: u32, bytes
//     color       u8 tag + payload (see encode_color)
//     linetype    u32 (LinetypeId)
//     lineweight  u8 tag + payload (see encode_lineweight)
//     flags       u8 — bit 0 visible, bit 1 locked, bit 2 frozen, bit 3 plottable
//
//   --- PenTable ---
//   count       u32
//   per pen:
//     name        len: u32, bytes
//     color       (encoded as above)
//     linetype    u32
//     lineweight  (encoded as above)
//
//   --- DObjects ---
//   count       u32
//   per dobject:
//     handle      u64
//     style: layer u32, color, linetype u32, linetype_scale f32, lineweight, visible u8
//     geom: u8 tag + per-variant payload (see write_geom)
//
// Future versions can add fields by bumping the version byte; the reader
// dispatches on it.

use cad_kernel::{
    Arc, Circle, Color, DObject, Document, Ellipse, EllipseArc, Geom, Hatch,
    HatchPattern, Layer, LayerTable, Line, Lineweight, Linetype, LinetypeTable,
    Pen, PenTable, Point, PolyVertex, Polyline, RasterImage, Spline, Style, Vec2, Wall,
};
use std::sync::Arc as StdArc;

const MAGIC: [u8; 4] = *b"RSM\x01";
// v2: + blocks table (after dobjects) and geom tag 12 = BlockRef. The
// reader accepts ANY version <= VERSION and skips sections newer files
// would have — old drawings keep loading.
// v3: + block `smart` flag, + text/dim/wall style tables (after blocks).
// v4: + embedded raster-image underlays section (after wall styles).
// v5: + BlockRef `mirror_x` flag (after rotation in the geom-12 record).
// v6: + BlockRef `scale_y` (after mirror_x) — per-axis scale / stretched blocks.
// v7: + per-segment polyline widths (in the geom polyline record). NOTE: the
//     HSI windows-ui branch shipped this as "v4"; renumbered to v7 here because
//     our v4/v5/v6 were already taken by raster / mirror_x / scale_y. The width
//     reader is therefore gated on ver >= 7 so v4..v6 files (no widths) load.
const VERSION: u16  = 7;

// =============================================================================
//   WRITER
// =============================================================================

pub fn write_rsm(doc: &Document) -> Vec<u8> {
    let mut w = Vec::with_capacity(1024 + doc.dobjects.len() * 64);
    w.extend_from_slice(&MAGIC);
    write_u16(&mut w, VERSION);
    write_u16(&mut w, 0);

    write_linetype_table(&mut w, &doc.linetypes);
    write_layer_table(&mut w, &doc.layers, &doc.truecolors);
    write_pen_table(&mut w, &doc.pens, &doc.truecolors);
    write_dobjects(&mut w, &doc.dobjects, &doc.truecolors);
    write_block_table(&mut w, &doc.blocks, &doc.truecolors);   // v2
    // v3 — full style tables so a re-opened drawing keeps its wall poché
    // fill, dim styling, and text styles (previously reset to defaults).
    write_text_style_table(&mut w, &doc.text_styles);
    write_dim_style_table(&mut w, &doc.dim_styles);
    write_wall_style_table(&mut w, &doc.wall_styles);
    write_raster_images(&mut w, &doc.raster_images);          // v4

    w
}

/// v4 — embedded raster underlays. Per image: name, placement (insert + world
/// size), then the raw encoded file bytes (PNG/JPEG/…) length-prefixed.
fn write_raster_images(w: &mut Vec<u8>, imgs: &[RasterImage]) {
    write_u32(w, imgs.len() as u32);
    for img in imgs {
        write_str(w, &img.name);
        write_vec2(w, img.insert);
        write_f64(w, img.world_w);
        write_f64(w, img.world_h);
        write_u64(w, img.data.len() as u64);
        w.extend_from_slice(&img.data);
    }
}

/// v2 — block definitions. Per block: name, base point, then the
/// contained dobjects in the SAME framing as the document list (so the
/// dobject reader/writer is reused verbatim, nested blocks included).
fn write_block_table(
    w: &mut Vec<u8>,
    blocks: &cad_kernel::BlockTable,
    tc: &cad_kernel::TrueColorTable,
) {
    write_u32(w, blocks.blocks.len() as u32);
    for b in &blocks.blocks {
        write_str(w, &b.name);
        write_vec2(w, b.base);
        write_u8(w, b.smart as u8);          // v3
        write_dobjects(w, &b.dobjects, tc);
    }
}

// =============================================================================
//   v3 STYLE TABLES — text / dim / wall
//
// DimStyle has ~75 fields, so the field list lives in ONE place (the
// `dim_style_fields!` macro) and drives BOTH the writer and the reader.
// That makes a read/write order mismatch impossible — add a field once and
// both sides pick it up. Type tag legend: str / f64 / bool / i32 / u32 /
// i16 / char.
// =============================================================================

macro_rules! dim_style_fields {
    ($m:ident, $a:expr, $b:expr) => {
        $m!($a, $b, name, str);
        $m!($a, $b, arrow_size, f64);
        $m!($a, $b, arrow_block, str);
        $m!($a, $b, arrow_block_1, str);
        $m!($a, $b, arrow_block_2, str);
        $m!($a, $b, separate_arrows, bool);
        $m!($a, $b, leader_block, str);
        $m!($a, $b, tick_size, f64);
        $m!($a, $b, arrow_filled, bool);
        $m!($a, $b, text_height, f64);
        $m!($a, $b, text_gap, f64);
        $m!($a, $b, text_style_name, str);
        $m!($a, $b, text_vert_pos, i32);
        $m!($a, $b, text_horiz_just, i32);
        $m!($a, $b, text_vert_offset, f64);
        $m!($a, $b, text_inside_horiz, bool);
        $m!($a, $b, text_outside_horiz, bool);
        $m!($a, $b, text_force_inside, bool);
        $m!($a, $b, text_force_dimline, bool);
        $m!($a, $b, text_user_positioned, bool);
        $m!($a, $b, text_move_rule, i32);
        $m!($a, $b, linear_unit_format, i32);
        $m!($a, $b, decimal_places, i32);
        $m!($a, $b, rounding, f64);
        $m!($a, $b, zero_suppress, i32);
        $m!($a, $b, fraction_format, i32);
        $m!($a, $b, decimal_separator, char);
        $m!($a, $b, linear_scale, f64);
        $m!($a, $b, linear_post, str);
        $m!($a, $b, alt_units_enabled, bool);
        $m!($a, $b, alt_unit_format, i32);
        $m!($a, $b, alt_decimal_places, i32);
        $m!($a, $b, alt_scale, f64);
        $m!($a, $b, alt_rounding, f64);
        $m!($a, $b, alt_zero_suppress, i32);
        $m!($a, $b, alt_post, str);
        $m!($a, $b, arc_length_symbol, i32);
        $m!($a, $b, angular_unit_format, i32);
        $m!($a, $b, angular_decimal_places, i32);
        $m!($a, $b, angular_zero_suppress, i32);
        $m!($a, $b, tolerance_enabled, bool);
        $m!($a, $b, tolerance_plus, f64);
        $m!($a, $b, tolerance_minus, f64);
        $m!($a, $b, tolerance_decimal_places, i32);
        $m!($a, $b, tolerance_text_scale, f64);
        $m!($a, $b, tolerance_vert_just, i32);
        $m!($a, $b, tolerance_zero_suppress, i32);
        $m!($a, $b, limits_enabled, bool);
        $m!($a, $b, alt_tolerance_decimal_places, i32);
        $m!($a, $b, alt_tolerance_zero_suppress, i32);
        $m!($a, $b, ext_line_extend, f64);
        $m!($a, $b, ext_line_offset, f64);
        $m!($a, $b, ext_suppress_1, bool);
        $m!($a, $b, ext_suppress_2, bool);
        $m!($a, $b, ext_fixed_length, f64);
        $m!($a, $b, ext_fixed_length_on, bool);
        $m!($a, $b, ext_linetype_1, str);
        $m!($a, $b, ext_linetype_2, str);
        $m!($a, $b, dim_line_extend, f64);
        $m!($a, $b, dim_line_baseline_inc, f64);
        $m!($a, $b, dim_suppress_1, bool);
        $m!($a, $b, dim_suppress_2, bool);
        $m!($a, $b, dim_suppress_outside, bool);
        $m!($a, $b, dim_linetype, str);
        $m!($a, $b, color_dim_line, u32);
        $m!($a, $b, color_ext_line, u32);
        $m!($a, $b, color_text, u32);
        $m!($a, $b, text_fill_mode, i32);
        $m!($a, $b, text_fill_color, u32);
        $m!($a, $b, lineweight_dim_line, i16);
        $m!($a, $b, lineweight_ext_line, i16);
        $m!($a, $b, overall_scale, f64);
        $m!($a, $b, center_mark_size, f64);
        $m!($a, $b, jog_angle, f64);
        $m!($a, $b, arrow_text_fit, i32);
    };
}

fn write_text_style_table(w: &mut Vec<u8>, t: &cad_kernel::TextStyleTable) {
    write_u32(w, t.styles.len() as u32);
    for s in &t.styles {
        write_str(w, &s.name);
        write_str(w, &s.font_name);
        write_f64(w, s.width_factor);
        write_f64(w, s.oblique);
        write_f64(w, s.default_height);
    }
}

fn write_wall_style_table(w: &mut Vec<u8>, t: &cad_kernel::WallStyleTable) {
    write_u32(w, t.styles.len() as u32);
    for s in &t.styles {
        write_str(w, &s.name);
        write_f64(w, s.thickness);
        write_u32(w, s.fill_color);
        write_u32(w, s.face_color);
        write_str(w, &s.description);
    }
}

fn write_dim_style_table(w: &mut Vec<u8>, t: &cad_kernel::DimStyleTable) {
    write_u32(w, t.styles.len() as u32);
    for s in &t.styles {
        macro_rules! wf {
            ($w:expr, $s:expr, $f:ident, str)  => { write_str($w, &$s.$f); };
            ($w:expr, $s:expr, $f:ident, f64)  => { write_f64($w, $s.$f); };
            ($w:expr, $s:expr, $f:ident, bool) => { write_u8($w, $s.$f as u8); };
            ($w:expr, $s:expr, $f:ident, i32)  => { write_u32($w, $s.$f as u32); };
            ($w:expr, $s:expr, $f:ident, u32)  => { write_u32($w, $s.$f); };
            ($w:expr, $s:expr, $f:ident, i16)  => { write_u16($w, $s.$f as u16); };
            ($w:expr, $s:expr, $f:ident, char) => { write_u32($w, $s.$f as u32); };
        }
        dim_style_fields!(wf, w, s);
    }
}

fn write_u16(w: &mut Vec<u8>, v: u16) { w.extend_from_slice(&v.to_le_bytes()); }
fn write_u32(w: &mut Vec<u8>, v: u32) { w.extend_from_slice(&v.to_le_bytes()); }
fn write_u64(w: &mut Vec<u8>, v: u64) { w.extend_from_slice(&v.to_le_bytes()); }
fn write_f32(w: &mut Vec<u8>, v: f32) { w.extend_from_slice(&v.to_le_bytes()); }
fn write_f64(w: &mut Vec<u8>, v: f64) { w.extend_from_slice(&v.to_le_bytes()); }
fn write_u8 (w: &mut Vec<u8>, v: u8)  { w.push(v); }
fn write_str(w: &mut Vec<u8>, s: &str) {
    write_u32(w, s.len() as u32);
    w.extend_from_slice(s.as_bytes());
}
fn write_vec2(w: &mut Vec<u8>, v: Vec2) {
    write_f64(w, v.x);
    write_f64(w, v.y);
}

/// Color tag space (on-disk format is UNCHANGED for backward compat):
///   0 = ByLayer, 1 = ByBlock, 2 = Aci(u8), 3 = TrueColor (RGB u32 inline)
/// In-memory `Color::TrueColorRef(idx)` is dereferenced via `truecolors`
/// at write time. Reader interns the RGB into the doc's table.
fn write_color(w: &mut Vec<u8>, c: Color, tc: &cad_kernel::TrueColorTable) {
    match c {
        Color::ByLayer            => write_u8(w, 0),
        Color::ByBlock            => write_u8(w, 1),
        Color::Aci(i)             => { write_u8(w, 2); write_u8(w, i); }
        Color::TrueColorRef(idx)  => {
            let rgb = tc.get(idx).unwrap_or(0xFFFFFF);
            write_u8(w, 3);
            write_u32(w, rgb);
        }
    }
}

/// Lineweight tag space:
///   0 = ByLayer, 1 = ByBlock, 2 = Default, 3 = Custom(f32 mm)
fn write_lineweight(w: &mut Vec<u8>, lw: Lineweight) {
    match lw {
        Lineweight::ByLayer    => write_u8(w, 0),
        Lineweight::ByBlock    => write_u8(w, 1),
        Lineweight::Default    => write_u8(w, 2),
        Lineweight::Custom(mm) => { write_u8(w, 3); write_f32(w, mm); }
    }
}

fn write_linetype_table(w: &mut Vec<u8>, t: &LinetypeTable) {
    write_u32(w, t.linetypes.len() as u32);
    for lt in &t.linetypes {
        write_str(w, &lt.name);
        write_str(w, &lt.description);
        write_u32(w, lt.pattern.len() as u32);
        for v in &lt.pattern { write_f32(w, *v); }
    }
}

fn write_layer_table(w: &mut Vec<u8>, t: &LayerTable, tc: &cad_kernel::TrueColorTable) {
    write_u32(w, t.active);
    write_u32(w, t.layers.len() as u32);
    for l in &t.layers {
        write_str(w, &l.name);
        write_color(w, l.color, tc);
        write_u32(w, l.linetype);
        write_lineweight(w, l.lineweight);
        let mut flags = 0_u8;
        if l.visible   { flags |= 0b0001; }
        if l.locked    { flags |= 0b0010; }
        if l.frozen    { flags |= 0b0100; }
        if l.plottable { flags |= 0b1000; }
        write_u8(w, flags);
    }
}

fn write_pen_table(w: &mut Vec<u8>, t: &PenTable, tc: &cad_kernel::TrueColorTable) {
    write_u32(w, t.pens.len() as u32);
    for p in &t.pens {
        write_str(w, &p.name);
        write_color(w, p.color, tc);
        write_u32(w, p.linetype);
        write_lineweight(w, p.lineweight);
    }
}

fn write_dobjects(w: &mut Vec<u8>, ds: &[DObject], tc: &cad_kernel::TrueColorTable) {
    write_u32(w, ds.len() as u32);
    for d in ds {
        write_u64(w, d.handle);
        // Style block
        write_u32(w, d.style.layer);
        write_color(w, d.style.color, tc);
        write_u32(w, d.style.linetype);
        write_f32(w, d.style.linetype_scale);
        write_lineweight(w, d.style.lineweight);
        write_u8 (w, if d.style.visible { 1 } else { 0 });
        // Geometry
        write_geom(w, &d.geom);
    }
}

/// Geom tag space:
///   0=Line, 1=Circle, 2=Arc, 3=Ellipse, 4=EllipseArc, 5=Point, 6=Polyline,
///   7=Hatch (MVP — boundary handles + pattern code; 0=Solid)
///   8=Spline (NURBS — degree + control points + weights)
fn write_geom(w: &mut Vec<u8>, g: &Geom) {
    match g {
        Geom::Line(l) => {
            write_u8(w, 0);
            write_vec2(w, l.a);
            write_vec2(w, l.b);
        }
        Geom::Circle(c) => {
            write_u8(w, 1);
            write_vec2(w, c.center);
            write_f64(w, c.radius);
        }
        Geom::Arc(a) => {
            write_u8(w, 2);
            write_vec2(w, a.center);
            write_f64(w, a.radius);
            write_f64(w, a.start_angle);
            write_f64(w, a.sweep_angle);
        }
        Geom::Ellipse(e) => {
            write_u8(w, 3);
            write_vec2(w, e.center);
            write_vec2(w, e.major);
            write_f64(w, e.ratio);
        }
        Geom::EllipseArc(ea) => {
            write_u8(w, 4);
            write_vec2(w, ea.ellipse.center);
            write_vec2(w, ea.ellipse.major);
            write_f64(w, ea.ellipse.ratio);
            write_f64(w, ea.start_param);
            write_f64(w, ea.sweep_param);
        }
        Geom::Point(pt) => {
            write_u8(w, 5);
            write_vec2(w, pt.location);
            write_u8 (w, pt.style);
            write_f32(w, pt.size);
        }
        Geom::Polyline(p) => {
            write_u8(w, 6);
            write_u8(w, if p.closed { 1 } else { 0 });
            write_u32(w, p.vertices.len() as u32);
            for v in &p.vertices {
                write_vec2(w, v.pos);
                write_f64(w, v.bulge);
            }
            // v7: per-segment (start,end) widths. Empty = thin (count 0).
            write_u32(w, p.widths.len() as u32);
            for &(sw, ew) in &p.widths {
                write_f64(w, sw);
                write_f64(w, ew);
            }
        }
        Geom::Hatch(h) => {
            write_u8(w, 7);
            // Pattern encoding:
            //   0 = Solid                              (no extra payload)
            //   1 = Pattern { name, scale, angle_deg } (utf-8 name + 2 f64)
            match &h.pattern {
                HatchPattern::Solid => {
                    write_u8(w, 0);
                }
                HatchPattern::Pattern { name, scale, angle_deg } => {
                    write_u8(w, 1);
                    let bytes = name.as_bytes();
                    write_u32(w, bytes.len() as u32);
                    w.extend_from_slice(bytes);
                    write_f64(w, *scale);
                    write_f64(w, *angle_deg);
                }
            }
            write_u32(w, h.boundary_handles.len() as u32);
            for handle in &h.boundary_handles {
                write_u64(w, *handle);
            }
        }
        Geom::Spline(s) => {
            write_u8(w, 8);
            write_u8(w, s.degree as u8);
            write_u32(w, s.control_points.len() as u32);
            for p in &s.control_points {
                write_vec2(w, *p);
            }
            // weights.len() == control_points.len() by Spline invariant.
            for wt in &s.weights {
                write_f64(w, *wt);
            }
        }
        Geom::Wall(wall) => {
            // tag 9 = Wall; centerline + thickness + (v3) style + bulge.
            // Without style the poché-fill wall-style link was lost on
            // reopen; without bulge curved walls reopened straight.
            write_u8(w, 9);
            write_vec2(w, wall.start);
            write_vec2(w, wall.end);
            write_f64(w, wall.thickness);
            write_u32(w, wall.style);     // v3
            write_f64(w, wall.bulge);     // v3
        }
        Geom::Text(t) => {
            // tag 10 = Text.
            write_u8(w, 10);
            write_vec2(w, t.position);
            write_f64(w, t.height);
            write_f64(w, t.angle);
            write_str(w, &t.text);
            write_u8(w, match t.h_align {
                cad_kernel::TextHAlign::Left   => 0,
                cad_kernel::TextHAlign::Center => 1,
                cad_kernel::TextHAlign::Right  => 2,
            });
            write_u8(w, match t.v_align {
                cad_kernel::TextVAlign::Baseline => 0,
                cad_kernel::TextVAlign::Bottom   => 1,
                cad_kernel::TextVAlign::Middle   => 2,
                cad_kernel::TextVAlign::Top      => 3,
            });
            write_u32(w, t.style);
        }
        Geom::Dimension(d) => {
            // tag 11 = Dimension. Encoding:
            //   u8   kind (0=Linear, 1=Radius, 2=Diameter)
            //   per-kind def points
            //   u32  style id
            //   str  text_override ("" = None)
            use cad_kernel::DimKind;
            write_u8(w, 11);
            match &d.kind {
                DimKind::Linear { p1, p2, dimline_pos, ortho } => {
                    write_u8(w, 0);
                    write_vec2(w, *p1);
                    write_vec2(w, *p2);
                    write_vec2(w, *dimline_pos);
                    write_u8(w, match ortho {
                        cad_kernel::LinearOrtho::Horizontal => 0,
                        cad_kernel::LinearOrtho::Vertical   => 1,
                        cad_kernel::LinearOrtho::Aligned    => 2,
                    });
                }
                DimKind::Radius { center, on_circle, leader_end } => {
                    write_u8(w, 1);
                    write_vec2(w, *center);
                    write_vec2(w, *on_circle);
                    write_vec2(w, *leader_end);
                }
                DimKind::Diameter { center, on_circle, leader_end } => {
                    write_u8(w, 2);
                    write_vec2(w, *center);
                    write_vec2(w, *on_circle);
                    write_vec2(w, *leader_end);
                }
            }
            write_u32(w, d.style);
            write_str(w, d.text_override.as_deref().unwrap_or(""));
        }
        Geom::BlockRef(br) => {
            // tag 12 = BlockRef (v2): block id + insert + uniform scale
            // + rotation; v5 adds the mirror_x flag.
            write_u8(w, 12);
            write_u32(w, br.block);
            write_vec2(w, br.insert);
            write_f64(w, br.scale);
            write_f64(w, br.rotation);
            write_u8(w, br.mirror_x as u8);          // v5
            write_f64(w, br.scale_y);                // v6
        }
    }
}

// =============================================================================
//   READER
// =============================================================================

struct R<'a> { bytes: &'a [u8], pos: usize }

impl<'a> R<'a> {
    fn need(&self, n: usize) -> Result<(), String> {
        if self.pos + n > self.bytes.len() {
            Err(format!("RSM: read past end (at {} need {} have {})",
                self.pos, n, self.bytes.len()))
        } else { Ok(()) }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)?;
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn u8 (&mut self) -> Result<u8,  String> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?; Ok(u16::from_le_bytes([b[0],b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?; Ok(u32::from_le_bytes([b[0],b[1],b[2],b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        let b = self.take(4)?; Ok(f32::from_le_bytes([b[0],b[1],b[2],b[3]]))
    }
    fn f64(&mut self) -> Result<f64, String> {
        let b = self.take(8)?;
        Ok(f64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let raw = self.take(n)?;
        String::from_utf8(raw.to_vec())
            .map_err(|e| format!("RSM: bad utf-8 string: {}", e))
    }
    fn vec2(&mut self) -> Result<Vec2, String> {
        Ok(Vec2 { x: self.f64()?, y: self.f64()? })
    }
}

pub fn read_rsm(bytes: &[u8]) -> Result<Document, String> {
    let mut r = R { bytes, pos: 0 };
    let magic = r.take(4)?;
    if magic[..3] != MAGIC[..3] {
        return Err(format!("RSM: bad magic {:?}", &magic[..3]));
    }
    let _embedded_ver = magic[3];   // historic; today we read VERSION below
    let ver = r.u16()?;
    let _pad = r.u16()?;
    if ver > VERSION {
        return Err(format!(
            "RSM: file version {} is newer than this build reads (v{})",
            ver, VERSION));
    }

    let linetypes  = read_linetype_table(&mut r)?;
    let mut truecolors = cad_kernel::TrueColorTable::new();
    let layers    = read_layer_table(&mut r, &mut truecolors)?;
    let pens      = read_pen_table(&mut r, &mut truecolors)?;
    let dobjects  = read_dobjects(&mut r, &mut truecolors, ver)?;
    // v2 — block definitions. v1 files simply have no blocks section.
    let blocks = if ver >= 2 {
        read_block_table(&mut r, &mut truecolors, ver)?
    } else {
        cad_kernel::BlockTable::default()
    };

    // v3 — full style tables. Older files (v<3) had no style sections, so
    // synthesize the default tables (which is what those files relied on).
    let (text_styles, dim_styles, wall_styles) = if ver >= 3 {
        let t = read_text_style_table(&mut r)?;
        let d = read_dim_style_table(&mut r)?;
        let wl = read_wall_style_table(&mut r)?;
        (t, d, wl)
    } else {
        (cad_kernel::TextStyleTable::with_defaults(),
         cad_kernel::DimStyleTable::default(),
         cad_kernel::WallStyleTable::default())
    };
    // v4 — embedded raster underlays. Older files have no section.
    let raster_images = if ver >= 4 { read_raster_images(&mut r)? } else { Vec::new() };
    Ok(Document {
        dobjects, layers, linetypes, pens, truecolors,
        text_styles, dim_styles, wall_styles, blocks, raster_images,
    })
}

/// v4 — embedded raster underlays (mirror of `write_raster_images`).
fn read_raster_images(r: &mut R) -> Result<Vec<RasterImage>, String> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let name    = r.str()?;
        let insert  = r.vec2()?;
        let world_w = r.f64()?;
        let world_h = r.f64()?;
        let len     = r.u64()? as usize;
        let data    = r.take(len)?.to_vec();
        out.push(RasterImage { name, data: StdArc::new(data), insert, world_w, world_h });
    }
    Ok(out)
}

fn read_color(r: &mut R, tc: &mut cad_kernel::TrueColorTable) -> Result<Color, String> {
    Ok(match r.u8()? {
        0 => Color::ByLayer,
        1 => Color::ByBlock,
        2 => Color::Aci(r.u8()?),
        3 => Color::TrueColorRef(tc.intern(r.u32()?)),
        t => return Err(format!("RSM: unknown color tag {}", t)),
    })
}

fn read_lineweight(r: &mut R) -> Result<Lineweight, String> {
    Ok(match r.u8()? {
        0 => Lineweight::ByLayer,
        1 => Lineweight::ByBlock,
        2 => Lineweight::Default,
        3 => Lineweight::Custom(r.f32()?),
        t => return Err(format!("RSM: unknown lineweight tag {}", t)),
    })
}

fn read_linetype_table(r: &mut R) -> Result<LinetypeTable, String> {
    let n = r.u32()? as usize;
    let mut linetypes = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let desc = r.str()?;
        let plen = r.u32()? as usize;
        let mut pattern = Vec::with_capacity(plen);
        for _ in 0..plen { pattern.push(r.f32()?); }
        linetypes.push(Linetype { name, description: desc, pattern });
    }
    Ok(LinetypeTable { linetypes })
}

fn read_layer_table(r: &mut R, tc: &mut cad_kernel::TrueColorTable) -> Result<LayerTable, String> {
    let active = r.u32()?;
    let n = r.u32()? as usize;
    let mut layers = Vec::with_capacity(n);
    for _ in 0..n {
        let name       = r.str()?;
        let color      = read_color(r, tc)?;
        let linetype   = r.u32()?;
        let lineweight = read_lineweight(r)?;
        let flags      = r.u8()?;
        layers.push(Layer {
            name, color, linetype, lineweight,
            visible:   (flags & 0b0001) != 0,
            locked:    (flags & 0b0010) != 0,
            frozen:    (flags & 0b0100) != 0,
            plottable: (flags & 0b1000) != 0,
        });
    }
    Ok(LayerTable { layers, active })
}

fn read_pen_table(r: &mut R, tc: &mut cad_kernel::TrueColorTable) -> Result<PenTable, String> {
    let n = r.u32()? as usize;
    let mut pens = Vec::with_capacity(n);
    for _ in 0..n {
        let name       = r.str()?;
        let color      = read_color(r, tc)?;
        let linetype   = r.u32()?;
        let lineweight = read_lineweight(r)?;
        pens.push(Pen { name, color, linetype, lineweight });
    }
    Ok(PenTable { pens })
}

fn read_dobjects(r: &mut R, tc: &mut cad_kernel::TrueColorTable, ver: u16) -> Result<Vec<DObject>, String> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let handle    = r.u64()?;
        let layer     = r.u32()?;
        let color     = read_color(r, tc)?;
        let linetype  = r.u32()?;
        let lt_scale  = r.f32()?;
        let lineweight= read_lineweight(r)?;
        let visible   = r.u8()? != 0;
        let geom      = read_geom(r, ver)?;
        out.push(DObject {
            handle,
            style: Style {
                layer, color, linetype,
                linetype_scale: lt_scale, lineweight, visible,
            },
            geom,
        });
    }
    Ok(out)
}

/// v2 — block definitions (mirror of `write_block_table`). `ver` selects
/// the per-block layout: v3 carries the `smart` flag, v2 doesn't.
fn read_block_table(
    r: &mut R,
    tc: &mut cad_kernel::TrueColorTable,
    ver: u16,
) -> Result<cad_kernel::BlockTable, String> {
    let n = r.u32()? as usize;
    let mut blocks = Vec::with_capacity(n);
    for _ in 0..n {
        let name     = r.str()?;
        let base     = r.vec2()?;
        let smart    = if ver >= 3 { r.u8()? != 0 } else { false };
        let dobjects = read_dobjects(r, tc, ver)?;
        blocks.push(cad_kernel::Block { name, base, dobjects, smart, params: Vec::new(), cut_edges: Vec::new() });
    }
    Ok(cad_kernel::BlockTable { blocks })
}

/// v3 — text style table (mirror of `write_text_style_table`).
fn read_text_style_table(r: &mut R) -> Result<cad_kernel::TextStyleTable, String> {
    let n = r.u32()? as usize;
    let mut styles = Vec::with_capacity(n);
    for _ in 0..n {
        styles.push(cad_kernel::TextStyle {
            name:           r.str()?,
            font_name:      r.str()?,
            width_factor:   r.f64()?,
            oblique:        r.f64()?,
            default_height: r.f64()?,
        });
    }
    Ok(cad_kernel::TextStyleTable { styles })
}

/// v3 — wall style table (mirror of `write_wall_style_table`).
fn read_wall_style_table(r: &mut R) -> Result<cad_kernel::WallStyleTable, String> {
    let n = r.u32()? as usize;
    let mut styles = Vec::with_capacity(n);
    for _ in 0..n {
        styles.push(cad_kernel::WallStyle {
            name:        r.str()?,
            thickness:   r.f64()?,
            fill_color:  r.u32()?,
            face_color:  r.u32()?,
            insulation:  false,   // not persisted yet
            description: r.str()?,
        });
    }
    Ok(cad_kernel::WallStyleTable { styles })
}

/// v3 — dim style table (mirror of `write_dim_style_table`; same field
/// order via the shared `dim_style_fields!` macro). Reads into a STANDARD
/// base then overwrites every field.
fn read_dim_style_table(r: &mut R) -> Result<cad_kernel::DimStyleTable, String> {
    let n = r.u32()? as usize;
    let mut styles = Vec::with_capacity(n);
    for _ in 0..n {
        let mut s = cad_kernel::DimStyle::standard();
        macro_rules! rf {
            ($r:expr, $s:expr, $f:ident, str)  => { $s.$f = $r.str()?; };
            ($r:expr, $s:expr, $f:ident, f64)  => { $s.$f = $r.f64()?; };
            ($r:expr, $s:expr, $f:ident, bool) => { $s.$f = $r.u8()? != 0; };
            ($r:expr, $s:expr, $f:ident, i32)  => { $s.$f = $r.u32()? as i32; };
            ($r:expr, $s:expr, $f:ident, u32)  => { $s.$f = $r.u32()?; };
            ($r:expr, $s:expr, $f:ident, i16)  => { $s.$f = $r.u16()? as i16; };
            ($r:expr, $s:expr, $f:ident, char) => {
                $s.$f = char::from_u32($r.u32()?).unwrap_or('.');
            };
        }
        dim_style_fields!(rf, r, s);
        styles.push(s);
    }
    Ok(cad_kernel::DimStyleTable { styles })
}

fn read_geom(r: &mut R, ver: u16) -> Result<Geom, String> {
    Ok(match r.u8()? {
        0 => Geom::Line(Line { a: r.vec2()?, b: r.vec2()? }),
        1 => Geom::Circle(Circle { center: r.vec2()?, radius: r.f64()? }),
        2 => Geom::Arc(Arc {
            center: r.vec2()?, radius: r.f64()?,
            start_angle: r.f64()?, sweep_angle: r.f64()?,
        }),
        3 => Geom::Ellipse(Ellipse {
            center: r.vec2()?, major: r.vec2()?, ratio: r.f64()?,
        }),
        4 => {
            let el = Ellipse { center: r.vec2()?, major: r.vec2()?, ratio: r.f64()? };
            Geom::EllipseArc(EllipseArc {
                ellipse: el, start_param: r.f64()?, sweep_param: r.f64()?,
            })
        }
        5 => Geom::Point(Point { location: r.vec2()?, style: r.u8()?, size: r.f32()? }),
        6 => {
            let closed = r.u8()? != 0;
            let n = r.u32()? as usize;
            let mut vertices = Vec::with_capacity(n);
            for _ in 0..n {
                vertices.push(PolyVertex { pos: r.vec2()?, bulge: r.f64()? });
            }
            // v7: per-segment (start,end) widths (absent / empty in v4..v6 and
            // older files — see the VERSION note about the renumber from HSI v4).
            let widths = if ver >= 7 {
                let wn = r.u32()? as usize;
                let mut ws = Vec::with_capacity(wn);
                for _ in 0..wn { ws.push((r.f64()?, r.f64()?)); }
                ws
            } else {
                Vec::new()
            };
            Geom::Polyline(Polyline { vertices, closed, widths })
        }
        7 => {
            let pattern = match r.u8()? {
                0 => HatchPattern::Solid,
                1 => {
                    let name_len = r.u32()? as usize;
                    let bytes = r.take(name_len)?.to_vec();
                    let name = String::from_utf8(bytes)
                        .map_err(|e| format!("RSM: hatch pattern name not UTF-8: {}", e))?;
                    let scale     = r.f64()?;
                    let angle_deg = r.f64()?;
                    HatchPattern::Pattern { name, scale, angle_deg }
                }
                other => return Err(format!("RSM: unknown hatch pattern code {}", other)),
            };
            let n = r.u32()? as usize;
            let mut boundary_handles = Vec::with_capacity(n);
            for _ in 0..n {
                boundary_handles.push(r.u64()?);
            }
            Geom::Hatch(Hatch { boundary_handles, pattern })
        }
        8 => {
            let degree = r.u8()? as usize;
            let n = r.u32()? as usize;
            let mut control_points = Vec::with_capacity(n);
            for _ in 0..n { control_points.push(r.vec2()?); }
            let mut weights = Vec::with_capacity(n);
            for _ in 0..n { weights.push(r.f64()?); }
            Geom::Spline(Spline { degree, control_points, weights })
        }
        9 => {
            let start = r.vec2()?;
            let end   = r.vec2()?;
            let thickness = r.f64()?;
            // v3 added style (wall-style link → poché fill) + bulge
            // (curved walls). v2 files have neither — default them.
            let (style, bulge) = if ver >= 3 {
                (r.u32()?, r.f64()?)
            } else {
                (0, 0.0)
            };
            Geom::Wall(Wall { start, end, thickness, style, bulge })
        }
        10 => {
            let position = r.vec2()?;
            let height   = r.f64()?;
            let angle    = r.f64()?;
            let text     = r.str()?;
            let h_align  = match r.u8()? {
                1 => cad_kernel::TextHAlign::Center,
                2 => cad_kernel::TextHAlign::Right,
                _ => cad_kernel::TextHAlign::Left,
            };
            let v_align  = match r.u8()? {
                1 => cad_kernel::TextVAlign::Bottom,
                2 => cad_kernel::TextVAlign::Middle,
                3 => cad_kernel::TextVAlign::Top,
                _ => cad_kernel::TextVAlign::Baseline,
            };
            let style    = r.u32()?;
            Geom::Text(cad_kernel::Text {
                position, height, angle, text, h_align, v_align, style,
            })
        }
        11 => {
            use cad_kernel::{Dim, DimKind, LinearOrtho};
            let kind = match r.u8()? {
                0 => {
                    let p1          = r.vec2()?;
                    let p2          = r.vec2()?;
                    let dimline_pos = r.vec2()?;
                    let ortho = match r.u8()? {
                        0 => LinearOrtho::Horizontal,
                        1 => LinearOrtho::Vertical,
                        _ => LinearOrtho::Aligned,
                    };
                    DimKind::Linear { p1, p2, dimline_pos, ortho }
                }
                1 => {
                    let center     = r.vec2()?;
                    let on_circle  = r.vec2()?;
                    let leader_end = r.vec2()?;
                    DimKind::Radius { center, on_circle, leader_end }
                }
                _ => {
                    let center     = r.vec2()?;
                    let on_circle  = r.vec2()?;
                    let leader_end = r.vec2()?;
                    DimKind::Diameter { center, on_circle, leader_end }
                }
            };
            let style    = r.u32()?;
            let override_s = r.str()?;
            let text_override = if override_s.is_empty() { None } else { Some(override_s) };
            Geom::Dimension(Dim { kind, style, text_override })
        }
        12 => {
            let block    = r.u32()?;
            let insert   = r.vec2()?;
            let scale    = r.f64()?;
            let rotation = r.f64()?;
            let mirror_x = if ver >= 5 { r.u8()? != 0 } else { false };
            let scale_y  = if ver >= 6 { r.f64()? } else { scale };
            Geom::BlockRef(cad_kernel::BlockRef { block, insert, scale, scale_y, rotation,
                mirror_x, param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS] })
        }
        t => return Err(format!("RSM: unknown geom tag {}", t)),
    })
}

// =============================================================================
//   TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(doc: &Document) -> Document {
        let bytes = write_rsm(doc);
        read_rsm(&bytes).expect("rsm round-trip")
    }

    #[test]
    fn raster_images_round_trip() {
        // v4 — an embedded raster underlay: name, placement and raw bytes must
        // survive the save/load byte-for-byte.
        let mut doc = Document::default();
        let data = vec![137u8, 80, 78, 71, 1, 2, 3, 4, 250, 0, 99];   // fake PNG-ish bytes
        doc.raster_images.push(RasterImage {
            name: "site_scan.png".into(),
            data: StdArc::new(data.clone()),
            insert: Vec2::new(-5.0, 42.0),
            world_w: 1408.0, world_h: 768.0,
        });
        let back = round_trip(&doc);
        assert_eq!(back.raster_images.len(), 1);
        let r = &back.raster_images[0];
        assert_eq!(r.name, "site_scan.png");
        assert_eq!(&*r.data, &data);
        assert_eq!(r.insert, Vec2::new(-5.0, 42.0));
        assert_eq!(r.world_w, 1408.0);
        assert_eq!(r.world_h, 768.0);
    }

    #[test]
    fn blocks_round_trip() {
        // A block definition (line + circle), one instance with a full
        // similarity transform, plus a NESTED instance inside a second
        // block — name/base/contents and every BlockRef field must
        // survive the v2 save/load.
        let mut doc = Document::default();
        let contents = vec![
            cad_kernel::DObject::new(Geom::Line(Line {
                a: Vec2::new(0.0, 0.0), b: Vec2::new(4.0, 0.0) })),
            cad_kernel::DObject::new(Geom::Circle(Circle {
                center: Vec2::new(2.0, 1.0), radius: 0.5 })),
        ];
        let id = doc.blocks.add(cad_kernel::Block {
            name: "CHAIR".into(), base: Vec2::new(2.0, 0.0),
            dobjects: contents, smart: false, params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let inner = vec![cad_kernel::DObject::new(Geom::BlockRef(
            cad_kernel::BlockRef {
                block: id, insert: Vec2::new(1.0, 1.0),
                scale: 0.5, scale_y: 0.5, rotation: 0.25, mirror_x: false,
                param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            }))];
        doc.blocks.add(cad_kernel::Block {
            name: "DESK_SET".into(), base: Vec2::ZERO, dobjects: inner, smart: false,
            params: Vec::new(), cut_edges: Vec::new(),
        });
        doc.push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
            block: id, insert: Vec2::new(10.0, -3.0),
            scale: 2.0, scale_y: 1.5, rotation: std::f64::consts::FRAC_PI_4, mirror_x: true,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
        })));

        let back = round_trip(&doc);
        assert_eq!(back.blocks.len(), 2);
        let blk = back.blocks.get(0).expect("block 0");
        assert_eq!(blk.name, "CHAIR");
        assert!((blk.base - Vec2::new(2.0, 0.0)).len() < 1e-12);
        assert_eq!(blk.dobjects.len(), 2);
        let nested = back.blocks.get(1).expect("block 1");
        let Geom::BlockRef(nb) = &nested.dobjects[0].geom else {
            panic!("nested blockref lost") };
        assert_eq!(nb.block, id);
        assert!((nb.scale - 0.5).abs() < 1e-12);
        let Geom::BlockRef(br) = &back.dobjects[0].geom else {
            panic!("instance lost") };
        assert_eq!(br.block, id);
        assert!((br.insert - Vec2::new(10.0, -3.0)).len() < 1e-12);
        assert!((br.scale - 2.0).abs() < 1e-12);
        assert!((br.scale_y - 1.5).abs() < 1e-12, "scale_y must survive the v6 round-trip");
        assert!((br.rotation - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert!(br.mirror_x, "mirror_x must survive the v5 round-trip");
        assert!(!nb.mirror_x, "nested instance was not mirrored");
    }

    #[test]
    fn style_tables_round_trip() {
        // Regression for "saved wall poché fill lost on reopen": the wall
        // style table (incl. fill_color), dim styles, text styles, and the
        // block `smart` flag must all survive a v3 save/load.
        let mut doc = Document::default();

        // Wall style WITH a solid fill (the reported bug), + a wall on it.
        let ws_id = doc.wall_styles.add(cad_kernel::WallStyle {
            name: "STRUCTURAL".into(), thickness: 0.35,
            fill_color: 8, face_color: 7, insulation: false,
            description: "load-bearing".into(),
        });
        doc.push(DObject::new(Geom::Wall(cad_kernel::Wall {
            start: Vec2::new(0.0, 0.0), end: Vec2::new(5.0, 0.0),
            thickness: 0.35, style: ws_id, bulge: 0.0,
        })));
        // A CURVED wall (bulge ≠ 0) — must reopen curved, not straight.
        doc.push(DObject::new(Geom::Wall(cad_kernel::Wall {
            start: Vec2::new(0.0, 5.0), end: Vec2::new(5.0, 5.0),
            thickness: 0.2, style: ws_id, bulge: 0.55,
        })));

        // Text style with distinct fields.
        doc.text_styles.styles.push(cad_kernel::TextStyle {
            name: "NOTES".into(), font_name: "romans".into(),
            width_factor: 0.8, oblique: 0.15, default_height: 2.5,
        });

        // Dim style: STANDARD + a spread of distinct values across types so
        // a read/write ORDER mismatch can't slip through (whole-struct eq).
        let mut ds = cad_kernel::DimStyle::standard();
        ds.name = "ARCH".into();
        ds.arrow_size = 1.23;
        ds.tick_size = 0.45;
        ds.arrow_filled = false;
        ds.text_height = 2.75;
        ds.decimal_separator = ',';
        ds.color_dim_line = 5;
        ds.color_ext_line = 6;
        ds.color_text = 7;
        ds.lineweight_dim_line = -2;
        ds.lineweight_ext_line = 35;
        ds.ext_suppress_1 = true;
        ds.linear_post = " mm".into();
        ds.overall_scale = 50.0;
        ds.text_move_rule = 2;
        let ds_clone = ds.clone();
        doc.dim_styles.add(ds);

        // A smart block (v3 flag).
        doc.blocks.add(cad_kernel::Block {
            name: "SMART1".into(), base: Vec2::ZERO,
            dobjects: vec![DObject::new(Geom::Line(Line {
                a: Vec2::ZERO, b: Vec2::new(1.0, 0.0) }))],
            smart: true, params: Vec::new(), cut_edges: Vec::new(),
        });

        let back = round_trip(&doc);

        // Wall style + fill survived; the wall still points at it.
        let wb = back.wall_styles.get(ws_id).expect("wall style");
        assert_eq!(wb.name, "STRUCTURAL");
        assert_eq!(wb.fill_color, 8);
        assert_eq!(wb.face_color, 7);
        assert!((wb.thickness - 0.35).abs() < 1e-12);
        let Geom::Wall(w) = &back.dobjects[0].geom else { panic!("wall lost") };
        assert_eq!(w.style, ws_id);
        // Curved wall kept its bulge + style.
        let Geom::Wall(cw) = &back.dobjects[1].geom else { panic!("curved wall lost") };
        assert!((cw.bulge - 0.55).abs() < 1e-12, "wall bulge lost");
        assert_eq!(cw.style, ws_id);

        // Text style survived.
        let ts = back.text_styles.styles.iter()
            .find(|s| s.name == "NOTES").expect("text style");
        assert!((ts.width_factor - 0.8).abs() < 1e-12);
        assert!((ts.default_height - 2.5).abs() < 1e-12);

        // Dim style survived field-for-field (PartialEq catches order bugs).
        let db = back.dim_styles.styles.iter()
            .find(|s| s.name == "ARCH").expect("dim style");
        assert_eq!(*db, ds_clone);

        // Smart-block flag survived.
        let sb = back.blocks.blocks.iter()
            .find(|b| b.name == "SMART1").expect("smart block");
        assert!(sb.smart);
    }

    #[test]
    fn empty_doc_round_trip() {
        let doc = Document::default();
        let back = round_trip(&doc);
        assert_eq!(back.layers.len(), doc.layers.len());
        assert_eq!(back.linetypes.len(), doc.linetypes.len());
        assert_eq!(back.pens.len(), doc.pens.len());
        assert!(back.dobjects.is_empty());
    }

    #[test]
    fn every_geom_round_trips() {
        let mut doc = Document::default();
        doc.push(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 5.0) }.into());
        doc.push(Circle { center: Vec2::new(1.0, 2.0), radius: 3.0 }.into());
        doc.push(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.5, sweep_angle: 1.0,
        }.into());
        doc.push(Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 }.into());
        doc.push(EllipseArc {
            ellipse: Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 },
            start_param: 0.1, sweep_param: 2.0,
        }.into());
        doc.push(Point { location: Vec2::new(3.0, 4.0), style: 2, size: 0.5 }.into());
        doc.push(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(1.0, 0.0), bulge: 0.5 },
                PolyVertex { pos: Vec2::new(1.0, 1.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        }.into());
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 7);
    }

    #[test]
    fn polyline_widths_round_trip() {
        let mut doc = Document::default();
        doc.push(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
            ],
            closed: false,
            widths: vec![(2.0, 2.0), (1.0, 3.0)],
        }.into());
        let back = round_trip(&doc);
        if let Geom::Polyline(p) = &back.dobjects[0].geom {
            assert_eq!(p.widths, vec![(2.0, 2.0), (1.0, 3.0)]);
        } else { panic!("not a polyline"); }
    }

    #[test]
    fn handles_are_preserved() {
        let mut doc = Document::default();
        let i = doc.push(Line { a: Vec2::ZERO, b: Vec2::new(1.0, 1.0) }.into());
        let h = doc.dobjects[i].handle;
        let back = round_trip(&doc);
        assert_eq!(back.dobjects[0].handle, h);
    }

    #[test]
    fn layer_table_round_trip_preserves_active_and_flags() {
        let mut doc = Document::default();
        let id = doc.layers.add(Layer {
            name: "HIDDEN".into(),
            color: Color::Aci(3),
            linetype: 0,
            lineweight: Lineweight::Custom(0.5),
            visible: false, locked: true, frozen: false, plottable: true,
        });
        doc.layers.active = id;
        let back = round_trip(&doc);
        let lid = back.layers.find("HIDDEN").unwrap();
        let l = back.layers.get(lid).unwrap();
        assert!(!l.visible);
        assert!(l.locked);
        assert!(matches!(l.lineweight, Lineweight::Custom(x) if (x - 0.5).abs() < 1e-9));
        assert_eq!(back.layers.active, lid);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let r = read_rsm(b"NOPE");
        assert!(r.is_err());
    }

    #[test]
    fn bytes_are_compact() {
        // Sanity check: 1000 lines should encode in well under 100 KB
        // (~64 bytes per dobject is the expected scale: handle 8 + style ~20
        // + Line geom 33 = ~61).
        let mut doc = Document::default();
        for i in 0..1000 {
            doc.push(Line {
                a: Vec2::new(i as f64, 0.0),
                b: Vec2::new(i as f64, 10.0),
            }.into());
        }
        let bytes = write_rsm(&doc);
        // < 100 KB headroom — typical is ~70 KB for 1000 lines + table overhead.
        assert!(bytes.len() < 100_000, "1000 lines → {} bytes (expected < 100k)", bytes.len());
    }
}


#[cfg(test)]
mod handle_collision_probe {
    use super::*;
    use cad_kernel::{DObject, Document, Geom, Line, Vec2};

    fn rt(doc: &Document) -> Document {
        read_rsm(&write_rsm(doc)).expect("rsm round-trip")
    }

    /// PROBE: RSM preserves handles on load, but nothing bumps `HANDLE_COUNTER` past
    /// the loaded maximum — so the next dobject drawn after opening a file can be
    /// handed a handle a loaded dobject already owns.
    /// Beyond planes this matters because `Hatch.boundary_handles` resolves its
    /// boundary BY HANDLE: a collision binds a hatch to the wrong geometry.
    #[test]
    #[ignore = "KNOWN BUG (documented, not yet fixed): HANDLE_COUNTER is not bumped on load. \
                Run with `cargo test -p cad_io -- --ignored` to see it. Fix = an additive \
                `cad_kernel::reserve_handles_above(max)` called at the end of read_rsm."]
    fn known_bug_next_handle_collides_with_a_loaded_handle() {
        let mut doc = Document::default();
        let i = doc.push(DObject::new(Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(1.0, 1.0) })));
        doc.dobjects[i].handle = 1_000_000; // a file saved in an earlier, long session
        let back = rt(&doc);
        assert_eq!(back.dobjects[0].handle, 1_000_000, "handle preserved on load");

        let fresh = cad_kernel::next_handle();
        println!("PROBE: loaded handle = 1000000, next_handle() = {fresh}");
        assert!(fresh > 1_000_000, "BUG CONFIRMED: next_handle() = {fresh} collides with loaded handles");
    }
}
