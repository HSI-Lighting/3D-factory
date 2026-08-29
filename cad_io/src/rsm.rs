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
    HatchPattern, Layer, LayerTable, Layout, ViewportData, ViewportGeom,
    Line, Lineweight, Linetype, LinetypeTable,
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
// v17: + WallStyle.insulation flag (wall-style record), + Block.params /
//     cut_edges (sub-tables after the block's dobjects), + BlockRef
//     param_values (8 f64 after scale_y in the geom-12 record). Older files
//     (v<17) default all of them (false / empty / zeroed).
// v18: + trailing CRC-32 of everything before it (detect bit-rot / bad
//     transfers on open). Older files (v<18) have no checksum and load
//     unvalidated; old readers ignore the trailing bytes.
//
// ── VERSION RENUMBERING: 200+ IS THE MERGED LINE ─────────────────────────────
//
// This repo merges TWO independent lineages that both wrote this format under
// the SAME magic bytes:
//
//   * RUST-AutoRASM (2D CAD): versions 1..=34. v8-v11 text specs, v12 plot
//     styles, v13 layouts, v14 document units, v15 unit formats, v16 viewport
//     lock, v17 wall insulation + block params, v18 CRC, v19 groups, v20 layer
//     draw order, v21 spline knots, v22 MLEADER + block attrs, v23 angular
//     dims, v24 center marks, v25 xlines, v26 layer states, v27 UCS, v28 page
//     setup, v29 tables, v30 xrefs, v31 rays, v32 dim kinds 4-6, v33 donut/
//     wipeout/region, v34 Style.hatch_aux.
//   * 3D-Factory (SIMLUX): version 100 when a document unit was DECLARED
//     (else 7, byte-identical to the shared base). Its v100 file is the v7
//     stream + one trailing units block (f64 metres_per_unit + u8 source).
//
// They collided below: RUST-AutoRASM's v8 ("TextStyle bold") was the factory's
// v8 ("document units") — each reader accepted the other's files and parsed
// the same bytes as different fields. The factory renumbered to 100 to get
// out of the way; THIS merge renumbers the whole stream to 200 so the merged
// writer emits ONE unambiguous layout.
//
// The reader accepts everything either lineage ever wrote:
//   * ver 1..=99  — the RUST-AutoRASM stream (v14 units block, derived metres).
//   * ver == 100  — the 3D-Factory stream (v7 base + trailing units block).
//   * ver 101..=199 — nothing wrote these; refused like a future version.
//   * ver == 200  — the merged stream: base + their sections + the UNIFIED
//     units block (name, scene_per_unit, formats, metres_per_unit, source).
const VERSION: u16  = 200;
/// Version gate for the merged UNIFIED units block (v200). Older merged files
/// do not exist yet — this is the first version of the merged line.
const V_UNIFIED_UNITS: u16 = 200;
/// The 3D-Factory lineage's units-only version (its v7 base + units trailer).
const V_FACTORY_UNITS: u16 = 100;


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
    write_plot_styles(&mut w, &doc.plot_styles);              // v12
    write_layouts(&mut w, &doc.layouts);                      // v13
    write_units(&mut w, &doc.units);                          // v14
    write_groups(&mut w, &doc.groups);                        // v19
    write_layer_states(&mut w, &doc.layer_states);            // v26
    write_ucs(&mut w, &doc.ucs_list, doc.current_ucs);        // v27
    write_page_setup(&mut w, &doc.page_setup);                // v28

    // v18 — trailing CRC-32 of everything written above. Appending (rather
    // than wrapping) keeps old readers working: they simply ignore the
    // trailing bytes. The v<18 reader gate skips validation for older files,
    // which don't carry a checksum.
    w.extend_from_slice(&crc32(&w).to_le_bytes());

    w
}

/// CRC-32 (IEEE 802.3, reflected polynomial 0xEDB88320), table-driven and
/// dependency-free — the RSM format is hand-rolled by design.
fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// v200 — the UNIFIED document unit calibration. One block serving BOTH
/// lineages: the RUST-AutoRASM pair (name + scene-units-per-unit + display
/// formats, v14/v15) PLUS the 3D-Factory pair (metres-per-unit + source,
/// v100). `metres_per_unit` is stored too (not derived) so a file round-trips
/// exactly what the user set; readers derive it for older files.
fn write_units(w: &mut Vec<u8>, u: &cad_kernel::Units) {
    use cad_kernel::{LengthFormat, AngleFormat};
    write_str(w, &u.name);
    write_f64(w, u.scene_per_unit);
    let lf: u8 = match u.length_format {
        LengthFormat::Scientific => 0, LengthFormat::Decimal => 1,
        LengthFormat::Engineering => 2, LengthFormat::Architectural => 3,
        LengthFormat::Fractional => 4,
    };
    let af: u8 = match u.angle_format {
        AngleFormat::DecimalDegrees => 0, AngleFormat::DegMinSec => 1,
        AngleFormat::Grads => 2, AngleFormat::Radians => 3, AngleFormat::Surveyor => 4,
    };
    write_u8(w, lf);
    write_u8(w, u.length_precision);
    write_u8(w, af);
    write_u8(w, u.angle_precision);
    write_u8(w, u.angle_clockwise as u8);
    // v200 — the 3D-Factory pair appended.
    write_f64(w, u.metres_per_unit);
    let src: u8 = match u.source {
        cad_kernel::UnitSource::Assumed => 0,
        cad_kernel::UnitSource::Declared => 1,
        cad_kernel::UnitSource::User => 2,
    };
    write_u8(w, src);
}

/// v100 — the 3D-Factory lineage's units trailer: f64 metres_per_unit + u8
/// source tag (0=Assumed, 1=Declared, 2=User). The factory line never wrote a
/// name or scene calibration — its Units had only the metre factor — so the
/// merged `Units` is reconstructed from the factor (nearest named unit) and
/// the source is kept exactly as the file said.
fn read_factory_units(r: &mut R) -> Result<cad_kernel::Units, String> {
    let metres_per_unit = r.f64()?;
    let source = match r.u8()? {
        1 => cad_kernel::UnitSource::Declared,
        2 => cad_kernel::UnitSource::User,
        _ => cad_kernel::UnitSource::Assumed,
    };
    Ok(cad_kernel::Units::from_metres_per_unit(metres_per_unit, source))
}

/// v12 — the per-document plot style table (AutoCAD CTB analog). Stored as ONE
/// length-prefixed JSON blob reusing the `.pst` serialization: it's a single
/// bounded auxiliary section, so the geometry/core encoding stays hand-rolled
/// binary while this table rides along as text. Older files have no section and
/// load the default (all-`UseObject`) table.
fn write_plot_styles(w: &mut Vec<u8>, t: &cad_kernel::plotstyle::PlotStyleTable) {
    // The default (all-`UseObject`) table is the overwhelmingly common case; skip
    // the ~59 KB JSON blob for it (write a 0-length section → reader uses default)
    // so ordinary drawings stay compact. Only a table with assigned pens rides
    // along as JSON.
    if t == &cad_kernel::plotstyle::PlotStyleTable::default() {
        write_u32(w, 0);
        return;
    }
    let json = serde_json::to_vec(t).unwrap_or_default();
    write_u32(w, json.len() as u32);
    w.extend_from_slice(&json);
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

/// v13 — paper-space layouts. Per layout: name, paper config, plot config,
/// camera, viewports, layers, and paper-space entities.
fn write_layouts(w: &mut Vec<u8>, layouts: &[Layout]) {
    write_u32(w, layouts.len() as u32);
    for layout in layouts {
        write_str(w, &layout.name);
        // Paper + plot config
        use cad_kernel::plotstyle::{PaperSize, Orientation, PlotScale, PlotArea, Offset};
        // paper size: encode as (tag, dim0, dim1)
        match layout.paper {
            PaperSize::A4 => { write_u8(w, 0); }
            PaperSize::A3 => { write_u8(w, 1); }
            PaperSize::A2 => { write_u8(w, 2); }
            PaperSize::A1 => { write_u8(w, 3); }
            PaperSize::A0 => { write_u8(w, 4); }
            PaperSize::Letter => { write_u8(w, 5); }
                PaperSize::Custom { w_mm: pw, h_mm: ph } => { write_u8(w, 6); write_f64(w, pw as f64); write_f64(w, ph as f64); }
        }
        write_u8(w, match layout.orientation {
            Orientation::Portrait => 0,
            Orientation::Landscape => 1,
        });
        write_u8(w, match layout.plot_area {
            PlotArea::Extents => 0,
            PlotArea::Display => 1,
            PlotArea::Window { .. } => 2,
            _ => 0, // Layout falls back to Extents
        });
        if let PlotArea::Window { min, max } = layout.plot_area {
            write_vec2(w, min);
            write_vec2(w, max);
        }
        match layout.plot_scale {
            PlotScale::Fit => { write_u8(w, 0); }
            PlotScale::Ratio { model, paper_mm } => { write_u8(w, 1); write_f64(w, model); write_f64(w, paper_mm); }
        }
        match layout.plot_offset {
            Offset::Center => { write_u8(w, 0); }
            Offset::Xy { x_mm, y_mm } => { write_u8(w, 1); write_f32(w, x_mm); write_f32(w, y_mm); }
        }
        write_f64(w, layout.margins_mm);
        let ctb = layout.ctb_name.as_deref().unwrap_or("");
        write_str(w, ctb);
        write_u8(w, layout.plot_with_styles as u8);
        write_u8(w, layout.plot_object_lineweights as u8);
        // camera
        write_f32(w, layout.camera.zoom);
        write_f32(w, layout.camera.pan_x);
        write_f32(w, layout.camera.pan_y);
        // paper-space entities
        write_dobjects(w, &layout.entities, &cad_kernel::TrueColorTable::new());
        // viewports
        write_u32(w, layout.viewports.len() as u32);
        for vp in &layout.viewports {
            write_u64(w, vp.shape_handle.unwrap_or(0));
            write_f64(w, vp.rect_min.0);
            write_f64(w, vp.rect_min.1);
            write_f64(w, vp.rect_max.0);
            write_f64(w, vp.rect_max.1);
            write_f64(w, vp.model_center.0);
            write_f64(w, vp.model_center.1);
            write_f64(w, vp.model_zoom);
            write_f64(w, vp.model_scale);
            write_u32(w, vp.frozen_layers.len() as u32);
            for &lid in &vp.frozen_layers {
                write_u32(w, lid);
            }
            let vp_ctb = vp.ctb_name.as_deref().unwrap_or("");
            write_str(w, vp_ctb);
            write_u8(w, vp.locked as u8);   // v16
        }
        // per-layout layers
        write_layer_table(w, &layout.layers, &cad_kernel::TrueColorTable::new());
    }
}

/// v2 — block definitions. Per block: name, base point, then the
/// contained dobjects in the SAME framing as the document list (so the
/// dobject reader/writer is reused verbatim, nested blocks included).
/// v17 adds the parametric `params` sub-table and the `cut_edges` list
/// after the dobjects.
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
        // v17 — parametric params + cut edges.
        write_u32(w, b.params.len() as u32);
        for p in &b.params {
            write_str(w, &p.name);
            write_f64(w, p.original);
            write_u32(w, p.vectors.len() as u32);
            for v in &p.vectors {
                write_vec2(w, v.win_min);
                write_vec2(w, v.win_max);
                write_vec2(w, v.dir);
                write_f64(w, v.gain);
            }
        }
        write_u32(w, b.cut_edges.len() as u32);
        for &ci in &b.cut_edges {
            write_u32(w, ci as u32);
        }
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
        // v8+
        write_u8(w, s.bold as u8);
        write_u8(w, s.outline_only as u8);
        write_f64(w, s.outline_width);
        // v10+
        write_u8(w, s.underline as u8);
    }
}

fn write_wall_style_table(w: &mut Vec<u8>, t: &cad_kernel::WallStyleTable) {
    write_u32(w, t.styles.len() as u32);
    for s in &t.styles {
        write_str(w, &s.name);
        write_f64(w, s.thickness);
        write_u32(w, s.fill_color);
        write_u32(w, s.face_color);
        write_u8(w, s.insulation as u8);     // v17
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
        write_u32(w, l.order);   // v20 — draw-order priority (issue #35)
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
        write_u8 (w, if d.style.hatch_aux { 1 } else { 0 });   // v34
        // Geometry
        write_geom(w, &d.geom, tc);
    }
}

/// Geom tag space:
///   0=Line, 1=Circle, 2=Arc, 3=Ellipse, 4=EllipseArc, 5=Point, 6=Polyline,
///   7=Hatch (MVP — boundary handles + pattern code; 0=Solid)
///   8=Spline (NURBS — degree + control points + weights)
/// Shared Text payload (used by tag 10 Text and tag 14 Leader labels).
fn write_text_payload(w: &mut Vec<u8>, t: &cad_kernel::Text) {
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
    // v9+ per-entity render specs (snapshot).
    write_str(w, &t.font_name);
    write_u8(w, t.bold as u8);
    write_f64(w, t.oblique);
    write_f64(w, t.width_factor);
    write_u8(w, t.outline_only as u8);
    write_f64(w, t.outline_width);
    // v10+
    write_u8(w, t.underline as u8);
    // v11+ paragraph list decoration + line spacing.
    write_u8(w, match t.list_mode {
        cad_kernel::TextListKind::None     => 0,
        cad_kernel::TextListKind::Bulleted => 1,
        cad_kernel::TextListKind::Numbered => 2,
    });
    write_f64(w, t.line_spacing);
}

fn write_geom(w: &mut Vec<u8>, g: &Geom, tc: &cad_kernel::TrueColorTable) {    match g {
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
            // v21 — explicit knot vector (trimmed splines carry non-uniform
            // knots). 0 = none (clamped-uniform default).
            match &s.knots {
                Some(k) => {
                    write_u32(w, k.len() as u32);
                    for &kv in k { write_f64(w, kv); }
                }
                None => write_u32(w, 0),
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
            write_text_payload(w, t);
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
                // v23 — angular.
                DimKind::Angular { vertex, p1, p2, arc_pos } => {
                    write_u8(w, 3);
                    write_vec2(w, *vertex);
                    write_vec2(w, *p1);
                    write_vec2(w, *p2);
                    write_vec2(w, *arc_pos);
                }
                // v32 — arc-length.
                DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } => {
                    write_u8(w, 4);
                    write_vec2(w, *center);
                    write_f64(w, *radius);
                    write_f64(w, *start_angle);
                    write_f64(w, *sweep);
                    write_vec2(w, *leader_end);
                }
                // v32 — ordinate.
                DimKind::Ordinate { datum, point, leader_end, is_x } => {
                    write_u8(w, 5);
                    write_vec2(w, *datum);
                    write_vec2(w, *point);
                    write_vec2(w, *leader_end);
                    write_u8(w, *is_x as u8);
                }
                // v32 — jogged radius.
                DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos } => {
                    write_u8(w, 6);
                    write_vec2(w, *center);
                    write_vec2(w, *on_circle);
                    write_vec2(w, *leader_end);
                    write_vec2(w, *jog_pos);
                }
            }
            write_u32(w, d.style);
            write_str(w, d.text_override.as_deref().unwrap_or(""));
        }
        Geom::BlockRef(br) => {
            write_u8(w, 12);
            write_u32(w, br.block);
            write_vec2(w, br.insert);
            write_f64(w, br.scale);
            write_f64(w, br.rotation);
            write_u8(w, br.mirror_x as u8);
            write_f64(w, br.scale_y);
            // v17 — per-instance parametric values.
            for pv in &br.param_values {
                write_f64(w, *pv);
            }
            // v22 — per-instance attribute values (parallel to the
            // definition's AttrDef dobjects). Empty for attr-less blocks.
            write_u32(w, br.attr_values.len() as u32);
            for av in &br.attr_values {
                write_str(w, av);
            }
        }
        Geom::Leader(l) => {
            write_u8(w, 14);
            write_u32(w, l.pts.len() as u32);
            for p in &l.pts {
                write_vec2(w, *p);
            }
            write_u8(w, l.arrow as u8);
            // The label serializes as a full Text (tag 10 payload
            // re-used; keeps one encoding for both).
            write_text_payload(w, &l.label);
        }
        Geom::AttrDef(a) => {
            write_u8(w, 15);
            write_str(w, &a.tag);
            write_str(w, &a.prompt);
            write_str(w, &a.default);
            write_vec2(w, a.position);
            write_f64(w, a.height);
            write_f64(w, a.angle);
            write_u32(w, a.style);
            write_u8(w, a.visible as u8);
        }
        Geom::CenterMark(cm) => {
            // v24 — CENTERMARK.
            write_u8(w, 16);
            write_vec2(w, cm.center);
            write_f64(w, cm.size);
            write_f64(w, cm.rotation);
        }
        Geom::Xline(x) => {
            // v25 — XLINE.
            write_u8(w, 17);
            write_vec2(w, x.base);
            write_vec2(w, x.dir);
        }
        Geom::Ray(r) => {
            // v31 — RAY.
            write_u8(w, 20);
            write_vec2(w, r.base);
            write_vec2(w, r.dir);
        }
        Geom::Donut(d) => {
            // v33 — DONUT.
            write_u8(w, 21);
            write_vec2(w, d.center);
            write_f64(w, d.inner_radius);
            write_f64(w, d.outer_radius);
        }
        Geom::Wipeout(wo) => {
            // v33 — WIPEOUT.
            write_u8(w, 22);
            write_u32(w, wo.pts.len() as u32);
            for p in &wo.pts { write_vec2(w, *p); }
        }
        Geom::Region(rg) => {
            // v33 — REGION.
            write_u8(w, 23);
            write_u32(w, rg.loop_pts.len() as u32);
            for p in &rg.loop_pts { write_vec2(w, *p); }
        }
        Geom::Xref(x) => {
            // v30 — XREF (tag 19): instance + snapshot of the file's
            // dobjects (re-resolved by `xref reload` / on open).
            write_u8(w, 19);
            write_str(w, &x.name);
            write_str(w, &x.path);
            write_vec2(w, x.insert);
            write_f64(w, x.scale);
            write_f64(w, x.rotation);
            write_u32(w, x.cached.len() as u32);
            for d in &x.cached {
                // Same record layout as write_dobjects (handle, style, geom).
                write_u64(w, d.handle);
                write_u32(w, d.style.layer);
                write_color(w, d.style.color, tc);
                write_u32(w, d.style.linetype);
                write_f32(w, d.style.linetype_scale);
                write_lineweight(w, d.style.lineweight);
                write_u8(w, d.style.visible as u8);
                write_u8(w, d.style.hatch_aux as u8);   // v34
                write_geom(w, &d.geom, tc);
            }
        }
        Geom::Table(t) => {
            // v29 — TABLE (tag 18): grid + cell text.
            write_u8(w, 18);
            write_vec2(w, t.insert);
            write_u32(w, t.n_rows as u32);
            write_u32(w, t.n_cols as u32);
            write_f64(w, t.row_h);
            write_f64(w, t.col_w);
            write_f64(w, t.rotation);
            write_u32(w, t.style);
            write_f64(w, t.font_height);
            write_u32(w, t.cells.len() as u32);
            for c in &t.cells {
                write_str(w, c);
            }
        }
        Geom::Viewport(vp) => {
            write_u8(w, 13);
            write_vec2(w, vp.center);
            write_f64(w, vp.width);
            write_f64(w, vp.height);
            write_vec2(w, vp.model_center);
            write_f64(w, vp.model_zoom);
            write_f64(w, vp.model_scale);
            write_u8(w, vp.frame_visible as u8);
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

/// Issue #5 — validate every cross-reference index once all tables are
/// loaded. Layer/linetype/active-layer indices, text/dim/wall style ids,
/// block ids, block cut_edges and layout frozen-layers are raw `u32`s in
/// the file; out-of-bounds values must reject the OPEN here instead of
/// panicking later at a usage site.
#[allow(clippy::too_many_arguments)]
fn validate_doc_indices(
    layers: &cad_kernel::LayerTable,
    linetypes: &cad_kernel::LinetypeTable,
    pens: &cad_kernel::PenTable,
    dobjects: &[DObject],
    blocks: &cad_kernel::BlockTable,
    text_styles: &cad_kernel::TextStyleTable,
    dim_styles: &cad_kernel::DimStyleTable,
    wall_styles: &cad_kernel::WallStyleTable,
    layouts: &[Layout],
) -> Result<(), String> {
    let n_layers = layers.layers.len() as u32;
    let n_lts = linetypes.linetypes.len() as u32;
    let n_text = text_styles.styles.len() as u32;
    let n_dim = dim_styles.styles.len() as u32;
    let n_wall = wall_styles.styles.len() as u32;
    let n_blocks = blocks.blocks.len() as u32;
    let bad = |what: &str, idx: u32, len: u32| Err(format!(
        "RSM: invalid {what} index {idx} (table has {len} entries) — corrupt file"));

    if layers.active >= n_layers {
        return bad("active-layer", layers.active, n_layers);
    }
    for l in &layers.layers {
        if l.linetype >= n_lts {
            return bad(&format!("layer '{}' linetype", l.name), l.linetype, n_lts);
        }
    }
    for p in &pens.pens {
        if p.linetype >= n_lts {
            return bad(&format!("pen '{}' linetype", p.name), p.linetype, n_lts);
        }
    }
    // Style/block ids hang off the GEOM, so a dobject check walks both the
    // style block and the geom. `kind` names the container for the error.
    let check_dobject = |d: &DObject, kind: &str| -> Result<(), String> {
        if d.style.layer >= n_layers {
            return bad(&format!("{kind} layer"), d.style.layer, n_layers);
        }
        if d.style.linetype >= n_lts {
            return bad(&format!("{kind} linetype"), d.style.linetype, n_lts);
        }
        match &d.geom {
            Geom::Wall(w) if w.style >= n_wall => {
                bad(&format!("{kind} wall-style"), w.style, n_wall)
            }
            Geom::Text(t) if t.style >= n_text => {
                bad(&format!("{kind} text-style"), t.style, n_text)
            }
            Geom::Dimension(dm) if dm.style >= n_dim => {
                bad(&format!("{kind} dim-style"), dm.style, n_dim)
            }
            Geom::BlockRef(br) if br.block >= n_blocks => {
                bad(&format!("{kind} block id"), br.block, n_blocks)
            }
            _ => Ok(()),
        }
    };
    for (i, d) in dobjects.iter().enumerate() {
        check_dobject(d, &format!("dobject #{i}"))?;
    }
    for b in &blocks.blocks {
        for d in &b.dobjects {
            check_dobject(d, &format!("block '{}' dobject", b.name))?;
        }
        for &ci in &b.cut_edges {
            if ci >= b.dobjects.len() {
                return bad(&format!("block '{}' cut edge", b.name), ci as u32, b.dobjects.len() as u32);
            }
        }
    }
    for layout in layouts {
        // Layout entities index into the LAYOUT's own layer table (it swaps
        // in as the doc table while the layout is active).
        let nl = layout.layers.layers.len() as u32;
        for d in &layout.entities {
            if d.style.layer >= nl {
                return bad(&format!("layout '{}' entity layer", layout.name),
                    d.style.layer, nl);
            }
            if d.style.linetype >= n_lts {
                return bad(&format!("layout '{}' entity linetype", layout.name),
                    d.style.linetype, n_lts);
            }
        }
        for (vi, vp) in layout.viewports.iter().enumerate() {
            for &fl in &vp.frozen_layers {
                if fl >= nl {
                    return bad(&format!("layout '{}' viewport #{vi} frozen layer", layout.name),
                        fl, nl);
                }
            }
        }
    }
    Ok(())
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
    // FIELD GATES. The 3D-Factory lineage (ver == 100) wrote the v7 stream —
    // no v8+ text specs, no v17 params, no v20 layer order, no v34 hatch_aux —
    // so its field gates are capped at 7 while the SECTION stream still
    // branches below. Everything else reads with its own version's gates.
    let gates = if ver == V_FACTORY_UNITS { 7 } else { ver };
    let layers    = read_layer_table(&mut r, &mut truecolors, gates)?;
    let pens      = read_pen_table(&mut r, &mut truecolors)?;
    let dobjects  = read_dobjects(&mut r, &mut truecolors, gates)?;
    // v2 — block definitions. v1 files simply have no blocks section.
    let blocks = if ver >= 2 {
        read_block_table(&mut r, &mut truecolors, gates)?
    } else {
        cad_kernel::BlockTable::default()
    };

    // v3 — full style tables. Older files (v<3) had no style sections, so
    // synthesize the default tables (which is what those files relied on).
    let (text_styles, dim_styles, wall_styles) = if ver >= 3 {
        let t = read_text_style_table(&mut r, gates)?;
        let d = read_dim_style_table(&mut r)?;
        let wl = read_wall_style_table(&mut r, gates)?;
        (t, d, wl)
    } else {
        (cad_kernel::TextStyleTable::with_defaults(),
         cad_kernel::DimStyleTable::default(),
         cad_kernel::WallStyleTable::default())
    };
    // v4 — embedded raster underlays. Older files have no section.
    let raster_images = if ver >= 4 { read_raster_images(&mut r)? } else { Vec::new() };
    // ── LINEAGE BRANCH ──────────────────────────────────────────────────────
    // The 3D-Factory line (ver == 100) wrote the v7 stream + ONE trailing
    // units block — it never wrote plot styles, layouts, groups, layer states,
    // UCS or page setup. Everything else (ver 1..=99 and the merged 200) uses
    // the RUST-AutoRASM section stream below.
    if ver == V_FACTORY_UNITS {
        let units = read_factory_units(&mut r)?;
        cad_kernel::reserve_handles_above(
            dobjects.iter().chain(blocks.blocks.iter().flat_map(|b| b.dobjects.iter()))
                .map(|d| d.handle).max().unwrap_or(0));
        validate_doc_indices(&layers, &linetypes, &pens, &dobjects, &blocks,
            &text_styles, &dim_styles, &wall_styles, &[])?;
        return Ok(Document {
            dobjects, layers, linetypes, pens, truecolors,
            text_styles, dim_styles, wall_styles, blocks, raster_images,
            units,
            ..Document::default()
        });
    }
    // v12 — plot style table. Older files (v<12) have no section → default table.
    let plot_styles = if ver >= 12 {
        read_plot_styles(&mut r)?
    } else {
        cad_kernel::plotstyle::PlotStyleTable::default()
    };
    // v13 — paper-space layouts. Older files have no layouts section.
    let layouts = if ver >= 13 {
        read_layouts(&mut r, ver)?
    } else {
        Vec::new()
    };
    // v14 — document unit calibration. Older files → default (1 scene unit = 1 mm).
    // v15 adds length/angle display format. v200 (merged) appends the 3D-Factory
    // metres_per_unit + source pair, which v14/v15 files derive.
    let units = if ver >= 14 {
        use cad_kernel::{LengthFormat, AngleFormat};
        let name = r.str()?;
        let scene_per_unit = r.f64()?;
        let mut u = cad_kernel::Units::new(name, scene_per_unit);
        if ver >= 15 {
            u.length_format = match r.u8()? {
                0 => LengthFormat::Scientific, 2 => LengthFormat::Engineering,
                3 => LengthFormat::Architectural, 4 => LengthFormat::Fractional,
                _ => LengthFormat::Decimal,
            };
            u.length_precision = r.u8()?;
            u.angle_format = match r.u8()? {
                1 => AngleFormat::DegMinSec, 2 => AngleFormat::Grads,
                3 => AngleFormat::Radians, 4 => AngleFormat::Surveyor,
                _ => AngleFormat::DecimalDegrees,
            };
            u.angle_precision = r.u8()?;
            u.angle_clockwise = r.u8()? != 0;
        }
        if ver >= V_UNIFIED_UNITS {
            u.metres_per_unit = r.f64()?;
            u.source = match r.u8()? {
                1 => cad_kernel::UnitSource::Declared,
                2 => cad_kernel::UnitSource::User,
                _ => cad_kernel::UnitSource::Assumed,
            };
        } else {
            // Pre-merge files: the unit was written by the RUST-AutoRASM
            // writer, so the file itself declared it.
            u.source = cad_kernel::UnitSource::Declared;
        }
        u
    } else {
        cad_kernel::Units::default()
    };
    // v19 — object groups (member dobject handles). Older files have no
    // section and load with no groups. Handles that don't exist in the
    // loaded document (member was erased) are dropped per group; groups
    // left with fewer than 2 members are dropped too — a stale group must
    // never reference dead handles (issue #12).
    let mut groups = if ver >= 19 { read_groups(&mut r)? } else { Vec::new() };
    let layer_states = if ver >= 26 {
        read_layer_states(&mut r)?
    } else {
        Vec::new()
    };
    let (ucs_list, current_ucs) = if ver >= 27 {
        read_ucs(&mut r)?
    } else {
        (Vec::new(), 0)
    };
    let page_setup = if ver >= 28 {
        read_page_setup(&mut r)?
    } else {
        cad_kernel::pagesetup::PageSetup::default()
    };
    if !groups.is_empty() {
        let mut live: std::collections::HashSet<u64> = std::collections::HashSet::new();
        live.extend(dobjects.iter().map(|d| d.handle));
        live.extend(blocks.blocks.iter().flat_map(|b| b.dobjects.iter()).map(|d| d.handle));
        live.extend(layouts.iter().flat_map(|l| l.entities.iter()).map(|d| d.handle));
        groups = groups.into_iter().filter_map(|g| {
            let kept: Vec<u64> = g.into_iter().filter(|h| live.contains(h)).collect();
            (kept.len() >= 2).then_some(kept)
        }).collect();
    }
    // RSM PRESERVES handles, but `HANDLE_COUNTER` restarts at 1 every session —
    // so without this, the next dobject drawn after opening a file is handed a
    // handle a loaded dobject already owns. `Hatch.boundary_handles` resolves BY
    // HANDLE, so a collision can bind a hatch to the WRONG geometry. Raise the
    // counter past everything we just loaded — MODEL dobjects, block definitions'
    // dobjects AND every layout's paper-space entities (a second viewport's
    // ViewportData.shape_handle could otherwise bind to the FIRST viewport's
    // entity: its content rendered in the wrong viewport and its lock badge
    // missing). (3D_Factory §5.1.)
    if let Some(max) = dobjects.iter()
        .chain(blocks.blocks.iter().flat_map(|b| b.dobjects.iter()))
        .chain(layouts.iter().flat_map(|l| l.entities.iter()))
        .map(|d| d.handle).max()
    {
        cad_kernel::reserve_handles_above(max);
    }
    // v18 — trailing CRC-32 integrity check. `r.pos` is the start of the
    // checksum for a well-formed file; a truncated one fails the need() check
    // below. Older files (v<18) carry no checksum and load unvalidated.
    if ver >= 18 {
        let stored = u32::from_le_bytes(r.take(4)?.try_into().unwrap());
        let actual = crc32(&r.bytes[..r.pos - 4]);
        if stored != actual {
            return Err(format!(
                "RSM: checksum mismatch — the file is corrupt or was modified \
                 (stored {stored:#010x}, computed {actual:#010x})"));
        }
    }
    // Issue #5 — cross-reference index validation, now that every table is
    // loaded. A corrupt/crafted file must fail the OPEN here with a clear
    // error, not survive and panic later at a usage site.
    validate_doc_indices(&layers, &linetypes, &pens, &dobjects, &blocks,
        &text_styles, &dim_styles, &wall_styles, &layouts)?;
    Ok(Document {
        dobjects, layers, linetypes, pens, truecolors,
        text_styles, dim_styles, wall_styles, blocks, raster_images, plot_styles, layouts,
        active_layout: None,
        units,
        groups,
        layer_states,
        ucs_list,
        current_ucs,
        page_setup,
        ..Document::default()
    })
}

/// v19 — write object groups (member dobject handles).
fn write_groups(w: &mut Vec<u8>, groups: &[Vec<u64>]) {
    write_u32(w, groups.len() as u32);
    for g in groups {
        write_u32(w, g.len() as u32);
        for &h in g { write_u64(w, h); }
    }
}

/// v19 — read object groups (member dobject handles), raw. Dead-handle
/// filtering happens in `read_rsm` once every table is loaded.
fn read_groups(r: &mut R) -> Result<Vec<Vec<u64>>, String> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let m = r.u32()? as usize;
        let mut g = Vec::with_capacity(m);
        for _ in 0..m {
            g.push(r.u64()?);
        }
        out.push(g);
    }
    Ok(out)
}

/// v26 — layer-state snapshots (LAYERSTATE): name, then per-layer entries
/// keyed by layer NAME (ids shift), with visible/frozen/locked flags, ACI
/// color, lineweight index, and linetype name.
fn write_layer_states(w: &mut Vec<u8>, states: &[cad_kernel::laystate::LayerState]) {
    write_u32(w, states.len() as u32);
    for st in states {
        write_str(w, &st.name);
        write_u32(w, st.entries.len() as u32);
        for e in &st.entries {
            write_str(w, &e.layer);
            write_u8(w, e.visible as u8);
            write_u8(w, e.frozen as u8);
            write_u8(w, e.locked as u8);
            // Color as ACI (layer colors are ACI in practice); lineweight by
            // discriminant (0 ByLayer, 1 ByBlock, 2 Default, 3 Custom mm).
            let aci = match e.color {
                cad_kernel::Color::Aci(i) => i,
                _ => 7,
            };
            write_u8(w, aci);
            match e.lineweight {
                cad_kernel::Lineweight::ByLayer => write_u8(w, 0),
                cad_kernel::Lineweight::ByBlock => write_u8(w, 1),
                cad_kernel::Lineweight::Default  => write_u8(w, 2),
                cad_kernel::Lineweight::Custom(mm) => {
                    write_u8(w, 3);
                    write_f32(w, mm);
                }
            }
            write_str(w, &e.linetype);
        }
    }
}

/// v28 — model-space page setup (PAGESETUP).
fn write_page_setup(w: &mut Vec<u8>, ps: &cad_kernel::pagesetup::PageSetup) {
    use cad_kernel::plotstyle::{Orientation, PaperSize};
    // Paper: 0..5 = A4/A3/A2/A1/A0/Letter, 6 = custom + w/h.
    let (paper_tag, pw, ph) = match ps.paper {
        PaperSize::A4 => (0u8, 0.0f32, 0.0f32),
        PaperSize::A3 => (1, 0.0, 0.0),
        PaperSize::A2 => (2, 0.0, 0.0),
        PaperSize::A1 => (3, 0.0, 0.0),
        PaperSize::A0 => (4, 0.0, 0.0),
        PaperSize::Letter => (5, 0.0, 0.0),
        PaperSize::Custom { w_mm, h_mm } => (6, w_mm, h_mm),
    };
    write_u8(w, paper_tag);
    write_f32(w, pw);
    write_f32(w, ph);
    write_u8(w, match ps.orientation {
        Orientation::Portrait => 0,
        Orientation::Landscape => 1,
    });
    write_f64(w, ps.margins_mm);
    write_u8(w, ps.scale_fit as u8);
    write_f64(w, ps.scale_n);
    write_u8(w, ps.unit_inch as u8);
    write_u8(w, ps.ctb_name.is_some() as u8);
    if let Some(n) = &ps.ctb_name {
        write_str(w, n);
    }
}

fn read_page_setup(r: &mut R) -> Result<cad_kernel::pagesetup::PageSetup, String> {
    use cad_kernel::plotstyle::{Orientation, PaperSize};
    let paper_tag = r.u8()?;
    let pw = r.f32()?;
    let ph = r.f32()?;
    let paper = match paper_tag {
        0 => PaperSize::A4,
        1 => PaperSize::A3,
        2 => PaperSize::A2,
        3 => PaperSize::A1,
        4 => PaperSize::A0,
        5 => PaperSize::Letter,
        6 => PaperSize::Custom { w_mm: pw, h_mm: ph },
        _ => PaperSize::A4,
    };
    let orientation = if r.u8()? == 1 { Orientation::Landscape } else { Orientation::Portrait };
    let margins_mm = r.f64()?;
    let scale_fit = r.u8()? != 0;
    let scale_n = r.f64()?;
    let unit_inch = r.u8()? != 0;
    let ctb_name = if r.u8()? != 0 { Some(r.str()?) } else { None };
    Ok(cad_kernel::pagesetup::PageSetup {
        paper, orientation, margins_mm, scale_fit, scale_n, unit_inch, ctb_name,
    })
}

/// v27 — UCS list + current index (0 = World).
fn write_ucs(w: &mut Vec<u8>, list: &[cad_kernel::ucs::Ucs], current: usize) {
    write_u32(w, list.len() as u32);
    for u in list {
        write_str(w, &u.name);
        write_vec2(w, u.origin);
        write_f64(w, u.rotation);
    }
    write_u32(w, current as u32);
}

fn read_ucs(r: &mut R) -> Result<(Vec<cad_kernel::ucs::Ucs>, usize), String> {
    let n = r.u32()? as usize;
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let origin = r.vec2()?;
        let rotation = r.f64()?;
        list.push(cad_kernel::ucs::Ucs { name, origin, rotation });
    }
    let current = r.u32()? as usize;
    Ok((list, current))
}

fn read_layer_states(r: &mut R) -> Result<Vec<cad_kernel::laystate::LayerState>, String> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let m = r.u32()? as usize;
        let mut entries = Vec::with_capacity(m);
        for _ in 0..m {
            let layer = r.str()?;
            let visible = r.u8()? != 0;
            let frozen = r.u8()? != 0;
            let locked = r.u8()? != 0;
            let aci = r.u8()?;
            let lw = match r.u8()? {
                0 => cad_kernel::Lineweight::ByLayer,
                1 => cad_kernel::Lineweight::ByBlock,
                2 => cad_kernel::Lineweight::Default,
                3 => cad_kernel::Lineweight::Custom(r.f32()?),
                _ => cad_kernel::Lineweight::Default,
            };
            let linetype = r.str()?;
            entries.push(cad_kernel::laystate::LayerStateEntry {
                layer, visible, frozen, locked,
                color: cad_kernel::Color::Aci(aci),
                lineweight: lw,
                linetype,
            });
        }
        out.push(cad_kernel::laystate::LayerState { name, entries });
    }
    Ok(out)
}

/// v12 — read the plot style table's length-prefixed JSON blob (mirror of
/// `write_plot_styles`). A malformed/empty blob falls back to the default table
/// rather than failing the whole load.
fn read_plot_styles(r: &mut R) -> Result<cad_kernel::plotstyle::PlotStyleTable, String> {
    let n = r.u32()? as usize;
    if n == 0 {
        return Ok(cad_kernel::plotstyle::PlotStyleTable::default());
    }
    let bytes = r.take(n)?;
    Ok(serde_json::from_slice(bytes)
        .unwrap_or_else(|_| cad_kernel::plotstyle::PlotStyleTable::default()))
}

/// v13 — paper-space layouts. `ver` gates later-added per-viewport fields
/// (v16 = locked).
fn read_layouts(r: &mut R, ver: u16) -> Result<Vec<Layout>, String> {
    use cad_kernel::plotstyle::{PaperSize, Orientation, PlotScale, PlotArea, Offset};
    use cad_kernel::layout::LayoutCamera;
    let n = r.u32()? as usize;
    let mut layouts = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let paper = match r.u8()? {
            0 => PaperSize::A4,
            1 => PaperSize::A3,
            2 => PaperSize::A2,
            3 => PaperSize::A1,
            4 => PaperSize::A0,
            5 => PaperSize::Letter,
            6 => { let w = r.f64()? as f32; let h = r.f64()? as f32; PaperSize::Custom { w_mm: w, h_mm: h } }
            _ => return Err(format!("RSM: unknown paper size tag in layout '{}'", name)),
        };
        let orientation = match r.u8()? {
            1 => Orientation::Landscape,
            _ => Orientation::Portrait,
        };
        let plot_area = match r.u8()? {
            0 => PlotArea::Extents,
            1 => PlotArea::Display,
            2 => {
                let min = r.vec2()?;
                let max = r.vec2()?;
                PlotArea::Window { min, max }
            }
            _ => PlotArea::Extents,
        };
        let plot_scale = match r.u8()? {
            1 => PlotScale::Ratio { model: r.f64()?, paper_mm: r.f64()? },
            _ => PlotScale::Fit,
        };
        let plot_offset = match r.u8()? {
            1 => Offset::Xy { x_mm: r.f32()?, y_mm: r.f32()? },
            _ => Offset::Center,
        };
        let margins_mm = r.f64()?;
        let ctb_name = r.str()?;
        let ctb_name = if ctb_name.is_empty() { None } else { Some(ctb_name) };
        let plot_with_styles = r.u8()? != 0;
        let plot_object_lineweights = r.u8()? != 0;
        let camera = LayoutCamera {
            zoom: r.f32()?, pan_x: r.f32()?, pan_y: r.f32()?,
        };
        // paper-space entities (reuse the dobject reader) — the sub-stream
        // is written with the CURRENT writer layout, so pass the real
        // version (a hardcoded 13 here would misread any dobject field
        // added after v13).
        let mut tc = cad_kernel::TrueColorTable::new();
        let entities = read_dobjects(r, &mut tc, ver)?;
        // viewports
        let vp_count = r.u32()? as usize;
        let mut viewports = Vec::with_capacity(vp_count);
        for _ in 0..vp_count {
            let shape_handle = r.u64()?;
            let shape_handle = if shape_handle == 0 { None } else { Some(shape_handle) };
            let vp = ViewportData {
                shape_handle,
                rect_min: (r.f64()?, r.f64()?),
                rect_max: (r.f64()?, r.f64()?),
                model_center: (r.f64()?, r.f64()?),
                model_zoom: r.f64()?,
                model_scale: r.f64()?,
                frozen_layers: {
                    let fl = r.u32()? as usize;
                    let mut flayers = Vec::with_capacity(fl);
                    for _ in 0..fl { flayers.push(r.u32()?); }
                    flayers
                },
                ctb_name: {
                    let s = r.str()?;
                    if s.is_empty() { None } else { Some(s) }
                },
                locked: if ver >= 16 { r.u8()? != 0 } else { false },   // v16
            };
            viewports.push(vp);
        }
        // per-layout layers
        let mut tc2 = cad_kernel::TrueColorTable::new();
        let layers = read_layer_table(r, &mut tc2, ver)?;

        let (pw0, ph0) = paper.dims_mm();
        let (page_w_mm, page_h_mm) = match orientation {
            Orientation::Portrait => (pw0 as f64, ph0 as f64),
            Orientation::Landscape => (ph0 as f64, pw0 as f64),
        };

        layouts.push(Layout {
            name, paper, orientation, page_w_mm, page_h_mm,
            plot_area, plot_scale, plot_offset, margins_mm,
            ctb_name, plot_with_styles, plot_object_lineweights,
            camera, entities, viewports, layers,
        });
    }
    Ok(layouts)
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

fn read_layer_table(
    r: &mut R,
    tc: &mut cad_kernel::TrueColorTable,
    ver: u16,
) -> Result<LayerTable, String> {
    let active = r.u32()?;
    let n = r.u32()? as usize;
    let mut layers = Vec::with_capacity(n);
    for idx in 0..n {
        let name       = r.str()?;
        let color      = read_color(r, tc)?;
        let linetype   = r.u32()?;
        let lineweight = read_lineweight(r)?;
        let flags      = r.u8()?;
        // v20 — per-layer draw order; older files default to table index.
        let order = if ver >= 20 { r.u32()? } else { idx as u32 };
        layers.push(Layer {
            name, color, linetype, lineweight,
            visible:   (flags & 0b0001) != 0,
            locked:    (flags & 0b0010) != 0,
            frozen:    (flags & 0b0100) != 0,
            plottable: (flags & 0b1000) != 0,
            order,
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
        let hatch_aux = if ver >= 34 { r.u8()? != 0 } else { false };
        let geom      = read_geom(r, tc, ver)?;
        out.push(DObject {
            handle,
            style: Style {
                layer, color, linetype,
                linetype_scale: lt_scale, lineweight, visible, hatch_aux,
            },
            geom,
        });
    }
    Ok(out)
}

/// v2 — block definitions (mirror of `write_block_table`). `ver` selects
/// the per-block layout: v3 carries the `smart` flag, v2 doesn't; v17
/// carries the parametric `params` sub-table and `cut_edges`.
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
        let (params, cut_edges) = if ver >= 17 {
            let nparams = r.u32()? as usize;
            let mut params = Vec::with_capacity(nparams);
            for _ in 0..nparams {
                let name = r.str()?;
                let original = r.f64()?;
                let nvec = r.u32()? as usize;
                let mut vectors = Vec::with_capacity(nvec);
                for _ in 0..nvec {
                    vectors.push(cad_kernel::ParamVector {
                        win_min: r.vec2()?,
                        win_max: r.vec2()?,
                        dir:     r.vec2()?,
                        gain:    r.f64()?,
                    });
                }
                params.push(cad_kernel::BlockParam { name, original, vectors });
            }
            let ncut = r.u32()? as usize;
            let mut cut_edges = Vec::with_capacity(ncut);
            for _ in 0..ncut {
                cut_edges.push(r.u32()? as usize);
            }
            (params, cut_edges)
        } else {
            (Vec::new(), Vec::new())
        };
        blocks.push(cad_kernel::Block { name, base, dobjects, smart, params, cut_edges });
    }
    Ok(cad_kernel::BlockTable { blocks })
}

/// v3 — text style table (mirror of `write_text_style_table`).
fn read_text_style_table(r: &mut R, ver: u16) -> Result<cad_kernel::TextStyleTable, String> {
    let n = r.u32()? as usize;
    let mut styles = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let font_name = r.str()?;
        let width_factor = r.f64()?;
        let oblique = r.f64()?;
        let default_height = r.f64()?;
        // v8+ extended fields; pre-v8 files default them.
        let (bold, outline_only, outline_width) = if ver >= 8 {
            (r.u8()? != 0, r.u8()? != 0, r.f64()?)
        } else {
            (false, false, 0.0)
        };
        // v10+ underline.
        let underline = if ver >= 10 { r.u8()? != 0 } else { false };
        styles.push(cad_kernel::TextStyle {
            name, font_name, width_factor, oblique, default_height,
            bold, outline_only, outline_width, underline,
        });
    }
    Ok(cad_kernel::TextStyleTable { styles })
}

/// v3 — wall style table (mirror of `write_wall_style_table`). v17 files
/// carry the `insulation` flag; older ones default it false.
fn read_wall_style_table(r: &mut R, ver: u16) -> Result<cad_kernel::WallStyleTable, String> {
    let n = r.u32()? as usize;
    let mut styles = Vec::with_capacity(n);
    for _ in 0..n {
        styles.push(cad_kernel::WallStyle {
            name:        r.str()?,
            thickness:   r.f64()?,
            fill_color:  r.u32()?,
            face_color:  r.u32()?,
            insulation:  if ver >= 17 { r.u8()? != 0 } else { false },
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

fn read_geom(r: &mut R, tc: &mut cad_kernel::TrueColorTable, ver: u16) -> Result<Geom, String> {
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
            // v21 — explicit knots; older files default to clamped-uniform.
            let knots = if ver >= 21 {
                let kn = r.u32()? as usize;
                let mut k = Vec::with_capacity(kn);
                for _ in 0..kn { k.push(r.f64()?); }
                if k.is_empty() { None } else { Some(k) }
            } else { None };
            Geom::Spline(Spline { degree, control_points, weights, knots })
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
            let (position, height, angle, text, h_align, v_align, style,
             font_name, bold, oblique, width_factor, outline_only, outline_width,
             underline, list_mode, line_spacing) = read_text_payload(r, ver)?;
            Geom::Text(cad_kernel::Text {
                position, height, angle, text, h_align, v_align, style,
                font_name, bold, oblique, width_factor, outline_only, outline_width,
                underline, list_mode, line_spacing,
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
                // v23 — angular dimensions.
                3 => {
                    let vertex  = r.vec2()?;
                    let p1      = r.vec2()?;
                    let p2      = r.vec2()?;
                    let arc_pos = r.vec2()?;
                    DimKind::Angular { vertex, p1, p2, arc_pos }
                }
                // v32 — arc-length.
                4 => {
                    let center      = r.vec2()?;
                    let radius      = r.f64()?;
                    let start_angle = r.f64()?;
                    let sweep       = r.f64()?;
                    let leader_end  = r.vec2()?;
                    DimKind::ArcLen { center, radius, start_angle, sweep, leader_end }
                }
                // v32 — ordinate.
                5 => {
                    let datum      = r.vec2()?;
                    let point      = r.vec2()?;
                    let leader_end = r.vec2()?;
                    let is_x       = r.u8()? != 0;
                    DimKind::Ordinate { datum, point, leader_end, is_x }
                }
                // v32 — jogged radius.
                6 => {
                    let center     = r.vec2()?;
                    let on_circle  = r.vec2()?;
                    let leader_end = r.vec2()?;
                    let jog_pos    = r.vec2()?;
                    DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos }
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
            // v17 — per-instance parametric values.
            let param_values = if ver >= 17 {
                let mut pv = [0.0; cad_kernel::MAX_BLOCK_PARAMS];
                for p in pv.iter_mut() { *p = r.f64()?; }
                pv
            } else {
                [0.0; cad_kernel::MAX_BLOCK_PARAMS]
            };
            // v22 — per-instance attribute values.
            let attr_values = if ver >= 22 {
                let n = r.u32()? as usize;
                let mut av = Vec::with_capacity(n.min(4096));
                for _ in 0..n { av.push(r.str()?); }
                av
            } else {
                Vec::new()
            };
            Geom::BlockRef(cad_kernel::BlockRef { block, insert, scale, scale_y, rotation,
                mirror_x, param_values, attr_values })
        }
        14 => {
            let n = r.u32()? as usize;
            let mut pts = Vec::with_capacity(n.min(65536));
            for _ in 0..n { pts.push(r.vec2()?); }
            let arrow = r.u8()? != 0;
            let (position, height, angle, text, h_align, v_align, style,
             font_name, bold, oblique, width_factor, outline_only, outline_width,
             underline, list_mode, line_spacing) = read_text_payload(r, ver)?;
            let label = cad_kernel::Text {
                position, height, angle, text, h_align, v_align, style,
                font_name, bold, oblique, width_factor, outline_only, outline_width,
                underline, list_mode, line_spacing,
            };
            Geom::Leader(cad_kernel::Leader { pts, label, arrow })
        }
        15 => {
            let tag      = r.str()?;
            let prompt   = r.str()?;
            let default  = r.str()?;
            let position = r.vec2()?;
            let height   = r.f64()?;
            let angle    = r.f64()?;
            let style    = r.u32()?;
            let visible  = r.u8()? != 0;
            Geom::AttrDef(cad_kernel::AttrDef { tag, prompt, default, position,
                height, angle, style, visible })
        }
        // v24 — CENTERMARK.
        16 => {
            let center   = r.vec2()?;
            let size     = r.f64()?;
            let rotation = r.f64()?;
            Geom::CenterMark(cad_kernel::CenterMark { center, size, rotation })
        }
        // v25 — XLINE.
        17 => {
            let base = r.vec2()?;
            let dir  = r.vec2()?;
            Geom::Xline(cad_kernel::Xline::new(base, dir))
        }
        // v31 — RAY.
        20 => {
            let base = r.vec2()?;
            let dir  = r.vec2()?;
            Geom::Ray(cad_kernel::Ray::new(base, dir))
        }
        // v33 — DONUT.
        21 => {
            let center = r.vec2()?;
            let inner  = r.f64()?;
            let outer  = r.f64()?;
            Geom::Donut(cad_kernel::Donut::new(center, inner, outer))
        }
        // v33 — WIPEOUT.
        22 => {
            let n = r.u32()? as usize;
            let mut pts = Vec::with_capacity(n);
            for _ in 0..n { pts.push(r.vec2()?); }
            Geom::Wipeout(cad_kernel::Wipeout { pts })
        }
        // v33 — REGION.
        23 => {
            let n = r.u32()? as usize;
            let mut loop_pts = Vec::with_capacity(n);
            for _ in 0..n { loop_pts.push(r.vec2()?); }
            Geom::Region(cad_kernel::Region { loop_pts })
        }
        // v30 — XREF.
        19 => {
            let name = r.str()?;
            let path = r.str()?;
            let insert = r.vec2()?;
            let scale = r.f64()?;
            let rotation = r.f64()?;
            let nc = r.u32()? as usize;
            let mut cached = Vec::with_capacity(nc);
            for _ in 0..nc {
                let handle     = r.u64()?;
                let layer      = r.u32()?;
                let color      = read_color(r, tc)?;
                let linetype   = r.u32()?;
                let lt_scale   = r.f32()?;
                let lineweight = read_lineweight(r)?;
                let visible    = r.u8()? != 0;
                let hatch_aux  = if ver >= 34 { r.u8()? != 0 } else { false };
                let geom       = read_geom(r, tc, ver)?;
                cached.push(DObject {
                    geom,
                    style: cad_kernel::Style {
                        layer, color, linetype, linetype_scale: lt_scale,
                        lineweight, visible, hatch_aux,
                    },
                    handle,
                });
            }
            Geom::Xref(cad_kernel::Xref {
                name, path, insert, scale, rotation, cached,
            })
        }
        // v29 — TABLE.
        18 => {
            let insert = r.vec2()?;
            let n_rows = r.u32()? as usize;
            let n_cols = r.u32()? as usize;
            let row_h = r.f64()?;
            let col_w = r.f64()?;
            let rotation = r.f64()?;
            let style = r.u32()?;
            let font_height = r.f64()?;
            let nc = r.u32()? as usize;
            let mut cells = Vec::with_capacity(nc);
            for _ in 0..nc {
                cells.push(r.str()?);
            }
            Geom::Table(cad_kernel::Table { insert, n_rows, n_cols, row_h,
                col_w, rotation, style, font_height, cells })
        }
        13 => {
            let center       = r.vec2()?;
            let width        = r.f64()?;
            let height       = r.f64()?;
            let model_center = r.vec2()?;
            let model_zoom   = r.f64()?;
            let model_scale  = r.f64()?;
            let frame_visible = r.u8()? != 0;
            Geom::Viewport(ViewportGeom { center, width, height, model_center,
                model_zoom, model_scale, frame_visible })
        }
        t => return Err(format!("RSM: unknown geom tag {}", t)),
    })
}

/// Shared Text payload reader (mirror of `write_text_payload`).
#[allow(clippy::type_complexity)]
fn read_text_payload(
    r: &mut R,
    ver: u16,
) -> Result<(
    cad_kernel::Vec2, f64, f64, String,
    cad_kernel::TextHAlign, cad_kernel::TextVAlign, u32,
    String, bool, f64, f64, bool, f64, bool, cad_kernel::TextListKind, f64,
), String> {
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
    // v9+ per-entity render specs; older files inherit (font_name "").
    let (font_name, bold, oblique, width_factor, outline_only, outline_width) =
        if ver >= 9 {
            (r.str()?, r.u8()? != 0, r.f64()?, r.f64()?, r.u8()? != 0, r.f64()?)
        } else {
            (String::new(), false, 0.0, 1.0, false, 0.0)
        };
    // v10+ underline.
    let underline = if ver >= 10 { r.u8()? != 0 } else { false };
    // v11+ paragraph list decoration + line spacing.
    let (list_mode, line_spacing) = if ver >= 11 {
        let lm = match r.u8()? {
            1 => cad_kernel::TextListKind::Bulleted,
            2 => cad_kernel::TextListKind::Numbered,
            _ => cad_kernel::TextListKind::None,
        };
        (lm, r.f64()?)
    } else {
        (cad_kernel::TextListKind::None, 1.5)
    };
    Ok((position, height, angle, text, h_align, v_align, style,
        font_name, bold, oblique, width_factor, outline_only, outline_width,
        underline, list_mode, line_spacing))
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

    /// Issue #21 — a TRIMMED spline (explicit non-uniform knots) must
    /// round-trip: the reopened spline evaluates to the same curve.
    #[test]
    fn trimmed_spline_knots_round_trip() {
        let mut doc = Document::default();
        let sp = cad_kernel::Spline::new_bspline(3, vec![
            cad_kernel::Vec2::new(0.0, 0.0), cad_kernel::Vec2::new(3.0, 6.0),
            cad_kernel::Vec2::new(7.0, -4.0), cad_kernel::Vec2::new(10.0, 2.0),
        ]);
        let (left, _right) = sp.split_at(0.4);
        assert!(left.knots.is_some(), "split halves carry explicit knots");
        doc.push(DObject::new(cad_kernel::geom::Geom::Spline(left)));
        let back = round_trip(&doc);
        let Geom::Spline(s2) = &back.dobjects[0].geom else { panic!("spline preserved") };
        assert!(s2.knots.is_some(), "knots survive the round-trip");
        // Same curve: compare dense tessellations.
        let Geom::Spline(sa) = &back.dobjects[0].geom else { unreachable!() };
        let a = sa.tessellate(128);
        let b = sp.split_at(0.4).0.tessellate(128);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((*x - *y).len() < 1e-9, "trimmed spline shape preserved");
        }
    }

    /// #35 — per-layer draw order must survive an RSM round-trip.
    #[test]
    fn layer_draw_order_round_trip() {
        let mut doc = Document::default();
        let a = doc.layers.add(Layer { name: "A".into(), ..Layer::layer_zero() });
        let b = doc.layers.add(Layer { name: "B".into(), ..Layer::layer_zero() });
        let c = doc.layers.add(Layer { name: "C".into(), ..Layer::layer_zero() });
        // Re-stack: A front, C back (front = highest order).
        doc.layers.get_mut(a).unwrap().order = 2;
        doc.layers.get_mut(b).unwrap().order = 1;
        doc.layers.get_mut(c).unwrap().order = 0;
        let back = round_trip(&doc);
        assert_eq!(back.layers.get(a).unwrap().order, 2, "A stays front");
        assert_eq!(back.layers.get(b).unwrap().order, 1);
        assert_eq!(back.layers.get(c).unwrap().order, 0, "C stays back");
        assert_eq!(back.layers.get(0).unwrap().order, 0, "layer 0 order preserved");
    }

    /// #12 — object groups must survive an RSM round-trip, and members whose
    /// handles no longer exist are dropped (never a dangling reference).
    #[test]
    fn groups_round_trip_and_dead_members_dropped() {
        let mut doc = Document::default();
        let mut line = |x: f64| {
            let d: DObject = cad_kernel::geom::Line {
                a: cad_kernel::Vec2::new(x, 0.0),
                b: cad_kernel::Vec2::new(x, 5.0),
            }.into();
            doc.push(d)
        };
        line(0.0);
        line(10.0);
        line(20.0);
        let a = doc.dobjects[0].handle;
        let b = doc.dobjects[1].handle;
        let c = doc.dobjects[2].handle;
        doc.groups.push(vec![a, b]);
        doc.groups.push(vec![a, 123_456_789, c]);   // one dead member
        doc.groups.push(vec![a]);                    // too small
        let back = round_trip(&doc);
        assert_eq!(back.groups.len(), 2, "dead-member and <2 groups dropped");
        let g0 = back.groups.iter().find(|g| g.contains(&a) && g.contains(&b)).expect("g0");
        assert_eq!(g0.len(), 2);
        let g1 = back.groups.iter().find(|g| g.contains(&c)).expect("g1");
        assert!(!g1.contains(&123_456_789), "dead handle must be dropped");
        assert_eq!(g1.len(), 2);
    }

    /// RSM preserves handles on load, but `HANDLE_COUNTER` restarts at 1 every
    /// session — so before `reserve_handles_above`, the next dobject drawn after
    /// opening a file was handed a handle a loaded dobject already owned.
    /// It matters beyond duplicate IDs because `Hatch.boundary_handles` resolves
    /// its boundary BY HANDLE: a collision binds a hatch to the wrong geometry.
    ///
    /// Ported from 3D_Factory's `#[ignore]`d probe
    /// (`known_bug_next_handle_collides_with_a_loaded_handle`, §5.1) — they
    /// reported it as a known bug; here it is FIXED, so it runs in CI.
    ///
    /// NOTE: `HANDLE_COUNTER` is a process-global and tests share it, so the
    /// probe handle is deliberately HIGHER than any other test's reserve. Were
    /// it low, a sibling test that raised the counter first would make this pass
    /// VACUOUSLY — green while proving nothing. This way only `read_rsm`'s own
    /// `reserve_handles_above` can satisfy the assert, in any run order.
    #[test]
    fn next_handle_does_not_collide_with_a_loaded_handle() {
        const LOADED: cad_kernel::Handle = 9_000_000_000;
        let mut doc = Document::default();
        let i = doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::ZERO, b: Vec2::new(1.0, 1.0) })));
        doc.dobjects[i].handle = LOADED; // a file saved in an earlier, long session
        let back = round_trip(&doc);
        assert_eq!(back.dobjects[0].handle, LOADED, "handle preserved on load");

        let fresh = cad_kernel::next_handle();
        assert!(fresh > LOADED,
            "next_handle() = {fresh} must not collide with the loaded handle {LOADED}");
    }

    /// Batch D — DONUT / WIPEOUT / REGION (RSM v33, geom tags 21–23) must
    /// round-trip losslessly: centers/radii for the donut, every vertex for
    /// the wipeout/region loops.
    #[test]
    fn donut_wipeout_region_round_trip() {
        use cad_kernel::geom::{Donut, Region, Wipeout};
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Donut(Donut::new(
            cad_kernel::Vec2::new(3.0, -2.0), 1.25, 4.5))));
        doc.push(DObject::new(Geom::Wipeout(Wipeout {
            pts: vec![
                cad_kernel::Vec2::new(0.0, 0.0),
                cad_kernel::Vec2::new(8.0, 0.0),
                cad_kernel::Vec2::new(8.0, 5.0),
                cad_kernel::Vec2::new(0.0, 5.0),
            ],
        })));
        doc.push(DObject::new(Geom::Region(Region {
            loop_pts: vec![
                cad_kernel::Vec2::new(1.0, 1.0),
                cad_kernel::Vec2::new(3.0, 1.0),
                cad_kernel::Vec2::new(2.0, 4.0),
            ],
        })));
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 3);
        let Geom::Donut(d) = &back.dobjects[0].geom else { panic!("donut preserved") };
        assert!((d.center - cad_kernel::Vec2::new(3.0, -2.0)).len() < 1e-9);
        assert!((d.inner_radius - 1.25).abs() < 1e-9);
        assert!((d.outer_radius - 4.5).abs() < 1e-9);
        let Geom::Wipeout(w) = &back.dobjects[1].geom else { panic!("wipeout preserved") };
        assert_eq!(w.pts.len(), 4);
        assert!((w.pts[3] - cad_kernel::Vec2::new(0.0, 5.0)).len() < 1e-9);
        let Geom::Region(rg) = &back.dobjects[2].geom else { panic!("region preserved") };
        assert_eq!(rg.loop_pts.len(), 3);
        assert!((rg.loop_pts[2] - cad_kernel::Vec2::new(2.0, 4.0)).len() < 1e-9);
    }

    /// Layout paper-space entities (viewport frames) also carry preserved
    /// handles, but the counter used to reserve only above MODEL dobjects —
    /// after reopening a file, the next entity drawn (e.g. a second viewport)
    /// collided with the first viewport's entity handle, so its ViewportData
    /// bound to the WRONG entity: content rendered in the first viewport and
    /// the lock badge never showed. (3D_Factory §5.1 follow-up.)
    #[test]
    fn next_handle_does_not_collide_with_a_loaded_layout_entity_handle() {
        const LOADED: cad_kernel::Handle = 9_100_000_000;
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::ZERO, b: Vec2::new(1.0, 1.0) })));
        let mut layout = cad_kernel::Layout::new("L1",
            cad_kernel::plotstyle::PaperSize::A4,
            cad_kernel::plotstyle::Orientation::Portrait);
        let mut e = DObject::new(Geom::Viewport(cad_kernel::ViewportGeom {
            center: Vec2::new(50.0, 40.0), width: 40.0, height: 30.0,
            model_center: Vec2::ZERO, model_zoom: 1.0, model_scale: 1.0,
            frame_visible: true }));
        e.handle = LOADED;
        let h = e.handle;
        layout.entities.push(e);
        let mut vd = cad_kernel::ViewportData::new(
            (30.0, 25.0), (70.0, 55.0), (0.0, 0.0), 1.0, 1.0);
        vd.shape_handle = Some(h);
        layout.viewports.push(vd);
        doc.layouts.push(layout);

        let back = round_trip(&doc);
        assert_eq!(back.layouts[0].entities[0].handle, LOADED,
            "layout entity handle preserved on load");

        let fresh = cad_kernel::next_handle();
        assert!(fresh > LOADED,
            "next_handle() = {fresh} must not collide with the loaded LAYOUT entity handle {LOADED}");
    }

    /// `reserve_handles_above` is monotonic — a LOWER value must not lower the
    /// counter. Loading a small file after a big one must not re-open the
    /// collision window.
    #[test]
    fn reserve_handles_above_never_lowers_the_counter() {
        cad_kernel::reserve_handles_above(5_000_000);
        let a = cad_kernel::next_handle();
        cad_kernel::reserve_handles_above(10);          // must be a no-op
        let b = cad_kernel::next_handle();
        assert!(b > a && b > 5_000_000,
            "counter went backwards: a={a} b={b}");
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
    fn plot_styles_round_trip() {
        // v12 — the per-document plot style table must survive save/load.
        use cad_kernel::plotstyle::{PlotColor, PlotWidth};
        let mut doc = Document::default();
        doc.plot_styles.set_fixed_width(1, 0.70);
        doc.plot_styles.style_mut(1).plot_color = PlotColor::Black;
        doc.plot_styles.set_fixed_width(3, 0.13);
        doc.plot_styles.description = "job pens".into();
        doc.plot_styles.apply_global_ltscale = true;
        doc.plot_styles.ltscale_percent = 75.0;
        doc.plot_styles.lineweight_ladder.push(0.66);

        let back = round_trip(&doc);
        assert_eq!(back.plot_styles, doc.plot_styles);
        assert_eq!(back.plot_styles.style(1).lineweight, PlotWidth::Fixed(0.70));
        assert_eq!(back.plot_styles.style(1).plot_color, PlotColor::Black);
        assert_eq!(back.plot_styles.style(3).lineweight, PlotWidth::Fixed(0.13));
        assert!(back.plot_styles.lineweight_ladder.contains(&0.66));
    }

    #[test]
    fn old_rsm_without_plot_styles_loads_default() {
        // A pre-v12 file has no plot-style section. Downgrade the version byte to
        // 11 so the reader skips the (trailing) section and synthesizes the
        // default table — and must NOT error.
        use cad_kernel::plotstyle::{PlotStyleTable, PlotWidth};
        let mut doc = Document::default();
        doc.plot_styles.set_fixed_width(1, 2.0);   // present in the bytes, must be ignored
        let bytes = write_rsm(&doc);
        // v20's per-layer order u32s post-date v11 — strip them so the
        // version-patched stream stays byte-aligned.
        let mut bytes = strip_v20_layer_orders(&bytes);
        bytes[4] = 11;   // u16 version (little-endian low byte) → pre-plot-styles
        let back = read_rsm(&bytes).expect("old-version file loads without error");
        assert_eq!(back.plot_styles, PlotStyleTable::default());
        assert_eq!(back.plot_styles.style(1).lineweight, PlotWidth::UseObject);
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
                attr_values: Vec::new(),
            }))];
        doc.blocks.add(cad_kernel::Block {
            name: "DESK_SET".into(), base: Vec2::ZERO, dobjects: inner, smart: false,
            params: Vec::new(), cut_edges: Vec::new(),
        });
        doc.push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
            block: id, insert: Vec2::new(10.0, -3.0),
            scale: 2.0, scale_y: 1.5, rotation: std::f64::consts::FRAC_PI_4, mirror_x: true,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            attr_values: Vec::new(),
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
    fn v17_persists_block_params_cut_edges_wall_insulation_and_param_values() {
        // The four field families that used to be dropped on save→load:
        // WallStyle.insulation, Block.params, Block.cut_edges and
        // BlockRef.param_values must all round-trip (v17).
        let mut doc = Document::default();

        // Wall style with insulation ON (STANDARD lives at id 0).
        doc.wall_styles.styles.push(cad_kernel::WallStyle {
            name: "INSULATED".into(), thickness: 0.3,
            fill_color: 2, face_color: 7, insulation: true,
            description: "cavity".into(),
        });

        // Parametric block: two dobjects, one named param with a modifier
        // vector, and cut edges 0/1 (the door/window jamb convention).
        let contents = vec![
            cad_kernel::DObject::new(Geom::Line(Line {
                a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) })),
            cad_kernel::DObject::new(Geom::Line(Line {
                a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 20.0) })),
        ];
        let id = doc.blocks.add(cad_kernel::Block {
            name: "DOOR".into(), base: Vec2::new(5.0, 0.0),
            dobjects: contents, smart: true,
            params: vec![cad_kernel::BlockParam {
                name: "width".into(),
                original: 10.0,
                vectors: vec![cad_kernel::ParamVector {
                    win_min: Vec2::new(1.0, -1.0),
                    win_max: Vec2::new(9.0, 1.0),
                    dir:     Vec2::new(1.0, 0.0),
                    gain:    0.5,
                }],
            }],
            cut_edges: vec![0, 1],
        });

        // Instance carrying non-default parameter values.
        doc.push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
            block: id, insert: Vec2::new(50.0, 50.0),
            scale: 2.0, scale_y: 2.0, rotation: 0.0, mirror_x: false,
            param_values: [1.25, 2.5, 0.0, 0.0, 0.0, 0.0, 0.0, 3.75],
            attr_values: Vec::new(),
        })));

        let back = round_trip(&doc);

        assert!(back.wall_styles.styles[1].insulation,
            "wall insulation must survive the v17 round-trip");
        assert_eq!(back.wall_styles.styles[1].name, "INSULATED");
        let blk = back.blocks.get(0).expect("block 0");
        assert!(blk.smart, "smart flag still round-trips");
        assert_eq!(blk.params.len(), 1, "block params must survive");
        assert_eq!(blk.params[0].name, "width");
        assert_eq!(blk.params[0].original, 10.0);
        assert_eq!(blk.params[0].vectors.len(), 1);
        assert_eq!(blk.params[0].vectors[0].gain, 0.5);
        assert_eq!(blk.params[0].vectors[0].dir, Vec2::new(1.0, 0.0));
        assert_eq!(blk.cut_edges, vec![0, 1], "cut edges must survive");
        let Geom::BlockRef(br) = &back.dobjects[0].geom else {
            panic!("instance lost") };
        assert_eq!(br.param_values, [1.25, 2.5, 0.0, 0.0, 0.0, 0.0, 0.0, 3.75],
            "per-instance param_values must survive");
    }

    #[test]
    fn leader_and_attdef_round_trip() {
        // v22 — MLEADER (tag 14) + block attributes (tag 15 + attr_values).
        let mut doc = Document::default();
        let label = cad_kernel::Text {
            position: cad_kernel::Vec2::new(12.0, 5.0),
            height: 0.4, angle: 0.3,
            text: "Note".into(),
            h_align: cad_kernel::TextHAlign::Left,
            v_align: cad_kernel::TextVAlign::Baseline,
            style: 0,
            font_name: "Inter".into(), bold: true, oblique: 0.1,
            width_factor: 0.9, outline_only: false, outline_width: 0.0,
            underline: true, list_mode: cad_kernel::TextListKind::None,
            line_spacing: 1.5,
        };
        doc.push(DObject::new(Geom::Leader(cad_kernel::Leader {
            pts: vec![cad_kernel::Vec2::new(1.0, 1.0),
                      cad_kernel::Vec2::new(6.0, 4.0),
                      cad_kernel::Vec2::new(12.0, 5.0)],
            label: label.clone(),
            arrow: true,
        })));
        // A block definition with one AttrDef + an instance carrying a value.
        let def = cad_kernel::AttrDef {
            tag: "WIDTH".into(), prompt: "Width?".into(),
            default: "100".into(),
            position: cad_kernel::Vec2::new(2.0, 2.0),
            height: 0.3, angle: 0.0, style: 0, visible: true,
        };
        let id = doc.blocks.add(cad_kernel::Block {
            name: "DOOR".into(), base: cad_kernel::Vec2::ZERO,
            dobjects: vec![DObject::new(Geom::AttrDef(def))],
            smart: false, params: Vec::new(), cut_edges: Vec::new(),
        });
        doc.push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
            block: id, insert: cad_kernel::Vec2::new(20.0, 20.0),
            scale: 1.0, scale_y: 1.0, rotation: 0.0, mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            attr_values: vec!["900".into()],
        })));
        let back = round_trip(&doc);
        // Leader survives with its chain + label specs.
        let Geom::Leader(l) = &back.dobjects[0].geom else { panic!("leader lost") };
        assert_eq!(l.pts.len(), 3);
        assert_eq!(l.label.text, "Note");
        assert!(l.label.bold);
        assert!(l.label.underline);
        // The definition's AttrDef survives.
        let blk = back.blocks.get(0).expect("block");
        let Geom::AttrDef(ad) = &blk.dobjects[0].geom else { panic!("attdef lost") };
        assert_eq!(ad.tag, "WIDTH");
        assert_eq!(ad.prompt, "Width?");
        assert_eq!(ad.default, "100");
        // The instance's value survives.
        let Geom::BlockRef(br) = &back.dobjects[1].geom else { panic!("blockref lost") };
        assert_eq!(br.attr_values, vec!["900"]);
    }

    #[test]
    fn angular_dim_round_trip() {
        // v23 — DimKind::Angular (kind code 3) survives write/read.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::Angular {
                vertex:  cad_kernel::Vec2::new(0.0, 0.0),
                p1:      cad_kernel::Vec2::new(10.0, 0.0),
                p2:      cad_kernel::Vec2::new(0.0, 10.0),
                arc_pos: cad_kernel::Vec2::new(5.0, 5.0),
            },
            style: 0,
            text_override: Some("<> °".into()),
        })));
        let back = round_trip(&doc);
        let Geom::Dimension(d) = &back.dobjects[0].geom else { panic!("dim lost") };
        match d.kind {
            cad_kernel::DimKind::Angular { vertex, p1, p2, arc_pos } => {
                assert_eq!((vertex.x, vertex.y), (0.0, 0.0));
                assert_eq!((p1.x, p1.y), (10.0, 0.0));
                assert_eq!((p2.x, p2.y), (0.0, 10.0));
                assert_eq!((arc_pos.x, arc_pos.y), (5.0, 5.0));
            }
            _ => panic!("kind lost"),
        }
        assert_eq!(d.style, 0);
        assert_eq!(d.text_override.as_deref(), Some("<> °"));
        assert!((d.measured_value() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn centermark_round_trip() {
        // v24 — geom tag 16.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::CenterMark(cad_kernel::CenterMark {
            center: Vec2::new(3.5, -2.0),
            size: 0.75,
            rotation: 0.4,
        })));
        let back = round_trip(&doc);
        let Geom::CenterMark(cm) = &back.dobjects[0].geom else { panic!("centermark lost") };
        assert_eq!((cm.center.x, cm.center.y), (3.5, -2.0));
        assert_eq!(cm.size, 0.75);
        assert_eq!(cm.rotation, 0.4);
    }

    #[test]
    fn xline_round_trip() {
        // v25 — geom tag 17.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Xline(cad_kernel::Xline::new(
            Vec2::new(1.0, 2.0),
            Vec2::new(3.0, 4.0),
        ))));
        let back = round_trip(&doc);
        let Geom::Xline(x) = &back.dobjects[0].geom else { panic!("xline lost") };
        assert_eq!((x.base.x, x.base.y), (1.0, 2.0));
        let dl = (x.dir - Vec2::new(3.0, 4.0).normalized()).len();
        assert!(dl < 1e-9, "direction must be normalized + preserved");
    }

    #[test]
    fn dim_ext_kinds_round_trip() {
        // v32 — DimKind codes 4 (ArcLen), 5 (Ordinate), 6 (JoggedRadius).
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::ArcLen {
                center: Vec2::new(1.0, 2.0), radius: 3.5,
                start_angle: 0.25, sweep: 1.4, leader_end: Vec2::new(9.0, 4.0),
            },
            style: 0, text_override: None,
        })));
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::Ordinate {
                datum: Vec2::new(0.0, 0.0), point: Vec2::new(7.0, 2.0),
                leader_end: Vec2::new(9.0, 2.0), is_x: true,
            },
            style: 0, text_override: None,
        })));
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::JoggedRadius {
                center: Vec2::new(5.0, 5.0), on_circle: Vec2::new(10.0, 5.0),
                leader_end: Vec2::new(14.0, 8.0), jog_pos: Vec2::new(11.0, 3.0),
            },
            style: 0, text_override: None,
        })));
        let back = round_trip(&doc);
        let Geom::Dimension(a) = &back.dobjects[0].geom else { panic!("arc-len lost") };
        let cad_kernel::DimKind::ArcLen { radius, sweep, .. } = a.kind else { panic!() };
        assert!((radius - 3.5).abs() < 1e-9 && (sweep - 1.4).abs() < 1e-9);
        let Geom::Dimension(o) = &back.dobjects[1].geom else { panic!("ordinate lost") };
        let cad_kernel::DimKind::Ordinate { is_x, .. } = o.kind else { panic!() };
        assert!(is_x);
        let Geom::Dimension(j) = &back.dobjects[2].geom else { panic!("jogged lost") };
        assert!(matches!(j.kind, cad_kernel::DimKind::JoggedRadius { .. }));
    }

    #[test]
    fn ray_round_trip() {
        // v31 — geom tag 20.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Ray(cad_kernel::Ray::new(
            Vec2::new(1.0, 2.0),
            Vec2::new(3.0, 4.0),
        ))));
        let back = round_trip(&doc);
        let Geom::Ray(r) = &back.dobjects[0].geom else { panic!("ray lost") };
        assert_eq!((r.base.x, r.base.y), (1.0, 2.0));
        let dl = (r.dir - Vec2::new(3.0, 4.0).normalized()).len();
        assert!(dl < 1e-9, "direction must be normalized + preserved");
    }

    #[test]
    fn page_setup_round_trip() {
        // v28 — PAGESETUP section.
        let mut doc = Document::default();
        doc.page_setup = cad_kernel::pagesetup::PageSetup {
            paper: cad_kernel::plotstyle::PaperSize::A3,
            orientation: cad_kernel::plotstyle::Orientation::Landscape,
            margins_mm: 5.0,
            scale_fit: false,
            scale_n: 50.0,
            unit_inch: false,
            ctb_name: Some("monochrome.pst".into()),
        };
        let back = round_trip(&doc);
        assert_eq!(back.page_setup.paper, cad_kernel::plotstyle::PaperSize::A3);
        assert_eq!(back.page_setup.orientation,
            cad_kernel::plotstyle::Orientation::Landscape);
        assert_eq!(back.page_setup.margins_mm, 5.0);
        assert!(!back.page_setup.scale_fit);
        assert_eq!(back.page_setup.scale_n, 50.0);
        assert_eq!(back.page_setup.ctb_name.as_deref(), Some("monochrome.pst"));
    }

    #[test]
    fn table_round_trip() {
        // v29 — geom tag 18.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Table(cad_kernel::Table {
            insert: Vec2::new(1.0, 2.0),
            n_rows: 2,
            n_cols: 2,
            row_h: 5.0,
            col_w: 10.0,
            rotation: 0.25,
            style: 0,
            font_height: 1.2,
            cells: vec!["A".into(), "B".into(), "C".into(), "D".into()],
        })));
        let back = round_trip(&doc);
        let Geom::Table(t) = &back.dobjects[0].geom else { panic!("table lost") };
        assert_eq!((t.insert.x, t.insert.y), (1.0, 2.0));
        assert_eq!((t.n_rows, t.n_cols), (2, 2));
        assert_eq!(t.row_h, 5.0);
        assert_eq!(t.rotation, 0.25);
        assert_eq!(t.cells, vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn xref_round_trip() {
        // v30 — geom tag 19 (instance + cached snapshot).
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Xref(cad_kernel::Xref {
            name: "Site".into(),
            path: "/tmp/site.rsm".into(),
            insert: Vec2::new(10.0, 20.0),
            scale: 2.0,
            rotation: 0.5,
            cached: vec![
                DObject::new(Geom::Line(Line {
                    a: Vec2::new(0.0, 0.0), b: Vec2::new(5.0, 0.0) })),
                DObject::new(Geom::Circle(Circle {
                    center: Vec2::new(2.5, 2.5), radius: 1.0 })),
            ],
        })));
        let back = round_trip(&doc);
        let Geom::Xref(x) = &back.dobjects[0].geom else { panic!("xref lost") };
        assert_eq!(x.name, "Site");
        assert_eq!(x.path, "/tmp/site.rsm");
        assert_eq!((x.insert.x, x.insert.y), (10.0, 20.0));
        assert_eq!(x.scale, 2.0);
        assert_eq!(x.rotation, 0.5);
        assert_eq!(x.cached.len(), 2, "snapshot preserved");
        if let Geom::Circle(c) = &x.cached[1].geom {
            assert_eq!(c.radius, 1.0);
        } else { panic!("child lost"); }
    }

    #[test]
    fn ucs_round_trip() {
        // v27 — UCS list + current index.
        let mut doc = Document::default();
        doc.ucs_list.push(cad_kernel::ucs::Ucs {
            name: "Site".into(),
            origin: Vec2::new(100.0, -50.0),
            rotation: 0.3,
        });
        doc.current_ucs = 1;
        let back = round_trip(&doc);
        assert_eq!(back.ucs_list.len(), 1);
        assert_eq!(back.current_ucs, 1);
        assert_eq!(back.ucs_list[0].name, "Site");
        assert_eq!((back.ucs_list[0].origin.x, back.ucs_list[0].origin.y), (100.0, -50.0));
        assert_eq!(back.ucs_list[0].rotation, 0.3);
    }

    #[test]
    fn layer_states_round_trip() {
        // v26 — LAYERSTATE section.
        let mut doc = Document::default();
        doc.layers.add(cad_kernel::layer::Layer {
            name: "Walls".into(), color: cad_kernel::Color::Aci(1),
            linetype: 0, lineweight: cad_kernel::Lineweight::Custom(0.5),
            visible: false, locked: true, frozen: false,
            plottable: true, order: 0,
        });
        cad_kernel::laystate::save(&mut doc, "PlanView").unwrap();
        let back = round_trip(&doc);
        assert_eq!(back.layer_states.len(), 1);
        let st = &back.layer_states[0];
        assert_eq!(st.name, "PlanView");
        let walls = st.entries.iter().find(|e| e.layer == "Walls").expect("entry");
        assert!(!walls.visible);
        assert!(walls.locked);
        assert_eq!(walls.color, cad_kernel::Color::Aci(1));
        assert_eq!(walls.lineweight, cad_kernel::Lineweight::Custom(0.5));
    }

    #[test]
    fn checksum_detects_corruption() {
        // v18 files carry a trailing CRC-32; a single flipped byte in the
        // payload must fail the open with a clear error, not load silently.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 5.0) })));
        let bytes = write_rsm(&doc);

        // Sanity: the intact file loads.
        read_rsm(&bytes).expect("intact file loads");

        // Flip one byte in the middle of the payload (past the header).
        let mut bad = bytes.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 0x5A;
        match read_rsm(&bad) {
            Err(e) => assert!(e.contains("checksum"), "clear checksum error, got: {e}"),
            Ok(_) => panic!("corrupt file must fail"),
        }

        // Truncating the file (dropping the checksum) must fail too.
        let truncated = &bytes[..bytes.len() - 4];
        match read_rsm(truncated) {
            Err(e) => assert!(e.contains("past end"), "truncation error, got: {e}"),
            Ok(_) => panic!("truncated file must fail"),
        }
    }


    /// Downgrade a v20 stream to the v19 layout: walk the stream exactly like
    /// `read_rsm` up to the layer table and drop each layer's trailing order
    /// u32 (v20). Version-patched old-file tests stay byte-aligned.
    fn strip_v20_layer_orders(bytes: &[u8]) -> Vec<u8> {
        let mut r = R { bytes, pos: 0 };
        let magic = r.take(4).unwrap();
        assert_eq!(&magic[..3], MAGIC[..3].to_vec().as_slice());
        let ver = r.u16().unwrap();
        assert!(ver >= 20, "strip helper expects a v20 stream (got {ver})");
        let _pad = r.u16().unwrap();
        let mut tc = cad_kernel::TrueColorTable::new();
        read_linetype_table(&mut r).unwrap();
        let _active = r.u32().unwrap();
        let n = r.u32().unwrap() as usize;
        let table_start = r.pos;   // first layer's first byte
        let mut slices: Vec<(usize, usize)> = Vec::with_capacity(n);
        for _ in 0..n {
            let start = r.pos;
            let _name = r.str().unwrap();
            let _color = read_color(&mut r, &mut tc).unwrap();
            let _lt = r.u32().unwrap();
            let _lw = read_lineweight(&mut r).unwrap();
            let _flags = r.u8().unwrap();
            let end = r.pos;                    // after flags, BEFORE order
            r.take(4).unwrap();                 // the v20 order u32
            slices.push((start, end));
        }
        let tail = r.pos;                       // everything after the layer table
        let mut out = Vec::with_capacity(bytes.len() - 4 * n);
        out.extend_from_slice(&bytes[..table_start]);
        for (s, e) in slices { out.extend_from_slice(&bytes[s..e]); }
        out.extend_from_slice(&bytes[tail..]);
        out
    }

    #[test]
    fn v17_file_without_checksum_still_loads() {
        // Files saved by the pre-checksum build (v17 layout, no trailing
        // CRC) must keep loading unvalidated. The v17 payload layout is
        // byte-identical to v18 minus the checksum — but v20 added the
        // per-layer draw-order u32 mid-file and v34 added a Style.hatch_aux
        // byte to every dobject record, so strip both, then patch the
        // header version to 17.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::new(1.0, 2.0), b: Vec2::new(3.0, 4.0) })));
        let bytes = write_rsm(&doc);
        let mut patched = strip_v34_dobject_aux(&bytes);
        patched = strip_v20_layer_orders(&patched);
        patched.truncate(patched.len() - 4);    // no checksum in v17
        patched[4] = 17;
        patched[5] = 0;   // u16 LE version = 17
        let back = read_rsm(&patched).expect("v17 file (no checksum) loads");
        assert_eq!(back.dobjects.len(), 1);
    }

    /// v34 added a `Style.hatch_aux` byte to every dobject record. Old-file
    /// fixtures (e.g. the v17 test) re-encode a v34 stream as a v33 one, so
    /// this walks the doc.dobjects section with the real readers and copies
    /// each record byte-for-byte minus that one byte. (Blocks / layouts are
    /// empty in the fixture, so only the top-level dobjects section needs
    /// the strip.)
    fn strip_v34_dobject_aux(bytes: &[u8]) -> Vec<u8> {
        let mut r = R { bytes, pos: 0 };
        let magic = r.take(4).unwrap();
        assert_eq!(&magic[..3], MAGIC[..3].to_vec().as_slice());
        let ver = r.u16().unwrap();
        assert!(ver >= 34, "strip helper expects a v34 stream (got {ver})");
        let _pad = r.u16().unwrap();
        let mut tc = cad_kernel::TrueColorTable::new();
        read_linetype_table(&mut r).unwrap();
        read_layer_table(&mut r, &mut tc, ver).unwrap();
        read_pen_table(&mut r, &mut tc).unwrap();
        let n = r.u32().unwrap() as usize;
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len() - n);
        out.extend_from_slice(&bytes[..r.pos]);   // header + section prefix + count
        for _ in 0..n {
            let rec_start = r.pos;
            let _handle    = r.u64().unwrap();
            let _layer     = r.u32().unwrap();
            let _color     = read_color(&mut r, &mut tc).unwrap();
            let _linetype  = r.u32().unwrap();
            let _ltscale   = r.f32().unwrap();
            let _lineweight= read_lineweight(&mut r).unwrap();
            let _visible   = r.u8().unwrap();
            let aux_at = r.pos;                  // the v34 byte — dropped
            let _aux = r.u8().unwrap();
            let geom_at = r.pos;
            let _geom = read_geom(&mut r, &mut tc, ver).unwrap();
            out.extend_from_slice(&bytes[rec_start..aux_at]);
            out.extend_from_slice(&bytes[geom_at..r.pos]);
        }
        out.extend_from_slice(&bytes[r.pos..]);
        out
    }

    #[test]
    fn out_of_bounds_cross_reference_indices_fail_open() {
        // A document with an EMPTY layer table but a dobject still pointing
        // at layer 0 (and active = 0): the writer emits it verbatim, and the
        // reader must REJECT it at open instead of loading an out-of-bounds
        // layer id that panics later at a usage site.
        let doc = Document {
            layers: cad_kernel::LayerTable { layers: Vec::new(), active: 0 },
            ..Document::default()
        };
        let mut doc = doc;
        doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 0.0), b: Vec2::new(1.0, 1.0) })));
        let bytes = write_rsm(&doc);
        match read_rsm(&bytes) {
            Err(e) => assert!(e.contains("invalid"), "clear index error, got: {e}"),
            Ok(_) => panic!("out-of-bounds layer index must fail the open"),
        }
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
            bold: true, outline_only: true, outline_width: 0.5, underline: true,
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
            order: 0,
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
