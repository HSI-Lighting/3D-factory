// DXF ASCII reader + writer.
//
// DXF format: alternating lines of (group code, value). Group codes are
// integers; values are strings/ints/floats depending on the code's type.
// Files are organized into SECTIONs (HEADER / TABLES / BLOCKS / ENTITIES
// / OBJECTS), each delimited by `0\nSECTION` ... `0\nENDSEC`.
//
// We implement a minimal subset sufficient to round-trip RUST_CAD's seven
// Geom variants plus layer + linetype tables. AutoCAD-specific niceties
// (handles, extrusion vectors, true-color, lineweight enum, dimstyles,
// blocks) are skipped on read (silently) and emitted on write only when
// the data exists. Files written by RUST_CAD open cleanly in LibreCAD
// and AutoCAD.

use cad_kernel::{
    Arc, Block, BlockRef, Circle, Color, DObject, Document, Ellipse, EllipseArc,
    Geom, Layer, Line, Lineweight, Linetype, LinetypeTable, Point, PolyVertex,
    Polyline, Vec2,
};

// ============================================================================
//   READER
// ============================================================================

/// Parse DXF ASCII text into a fresh `Document`. Errors only on
/// fundamentally unreadable input (broken line pairs); unknown entities
/// and unknown group codes are silently skipped.
pub fn read_dxf(text: &str) -> Result<Document, String> {
    Ok(read_dxf_with_stats(text)?.0)
}

/// Like `read_dxf`, but also returns the number of entities that were
/// SKIPPED because their entity type isn't supported (DIMENSION, MTEXT,
/// old-style POLYLINE, ATTDEF, …). Callers surface this in the open log
/// so a lossy import is at least reported.
pub fn read_dxf_with_stats(text: &str) -> Result<(Document, usize), String> {
    let pairs = parse_pairs(text)?;
    let mut doc = Document::default();
    let mut skipped = 0usize;
    let mut i = 0;
    while i < pairs.len() {
        let (code, value) = pairs[i];
        if code == 0 && value == "SECTION" && i + 1 < pairs.len() {
            let (c2, name) = pairs[i + 1];
            if c2 == 2 {
                match name {
                    "TABLES"   => i = read_tables(&pairs, i + 2, &mut doc),
                    // BLOCKS precedes ENTITIES in the file, so block defs land
                    // in the table before any INSERT in ENTITIES resolves them.
                    "BLOCKS"   => i = read_blocks(&pairs, i + 2, &mut doc, &mut skipped),
                    "ENTITIES" => i = read_entities(&pairs, i + 2, &mut doc, &mut skipped),
                    // HEADER carries $INSUNITS — the file's own statement of what one
                    // drawing unit means. Skipping it (which this reader once did) is
                    // why an architectural plan in millimetres arrived
                    // indistinguishable from one in metres, and so came into the 3D
                    // side 1000x too large.
                    "HEADER"   => i = read_header(&pairs, i + 2, &mut doc),
                    _ => i = skip_to_endsec(&pairs, i + 2),
                }
                continue;
            }
        }
        i += 1;
    }
    Ok((doc, skipped))
}

/// Tokenize the source into (code, value) pairs. DXF files use CRLF or
/// LF; we tolerate either. The line *after* the code line is the value;
/// trailing whitespace is trimmed.
///
/// ZERO-COPY: each value BORROWS a slice of `text` instead of allocating a
/// fresh `String`. On a large drawing this is the whole ball game — tens of
/// millions of tiny heap allocations (measured ≈20 s parsing a 734 MB file)
/// collapse to none; the pairs vec is the only allocation, and it is reserved
/// up front so it barely re-grows. The returned slices live as long as `text`.
fn parse_pairs(text: &str) -> Result<Vec<(i32, &str)>, String> {
    let mut lines = text.lines();
    // ~1 pair per 24 source bytes is a safe under-estimate for typical DXF, so
    // the vec re-grows a couple of times at most rather than dozens.
    let mut out: Vec<(i32, &str)> = Vec::with_capacity(text.len() / 24 + 16);
    while let Some(code_line) = lines.next() {
        let code_str = code_line.trim();
        if code_str.is_empty() { continue; }
        let value_line = lines.next()
            .ok_or_else(|| "DXF: code line without value line".to_string())?;
        let code: i32 = code_str.parse()
            .map_err(|_| format!("DXF: bad group code '{}'", code_str))?;
        out.push((code, value_line.trim()));
    }
    Ok(out)
}

/// Metres in one drawing unit for an AutoCAD `$INSUNITS` code, or `None` when
/// the file declares nothing usable.
///
/// 0 is "unitless" — an explicit *absence* of a claim, not a unit, so it must
/// NOT be taken as metres. The exotic codes (angstroms, parsecs, survey feet…)
/// are deliberately left out: a drawing that really is in parsecs is not a
/// drawing this app can help with, and guessing would be worse than leaving it
/// `Assumed`.
/// Exposed to the DWG reader's test, which keeps its own copy of this table and
/// needs something to hold the two together — a drawing that means one thing as
/// DXF and another as DWG is the same class of bug as reading metres as
/// millimetres.
#[cfg(test)]
pub(crate) fn insunits_to_metres_for_test(code: i32) -> Option<f64> {
    insunits_to_metres(code)
}

fn insunits_to_metres(code: i32) -> Option<f64> {
    Some(match code {
        1  => 0.0254,      // inches
        2  => 0.3048,      // feet
        4  => 0.001,       // millimetres  ← the architectural default
        5  => 0.01,        // centimetres
        6  => 1.0,         // metres
        10 => 0.9144,      // yards
        14 => 0.1,         // decimetres
        _  => return None, // 0 = unitless, and everything exotic
    })
}

/// HEADER section: pick out `$INSUNITS` and record it on the document.
///
/// Shape is `9 / $VARNAME` followed by the value pair, so this walks to the
/// named variable and reads whatever pair comes next. Unknown variables are
/// skipped, exactly as before.
fn read_header(pairs: &[(i32, &str)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (code, value) = pairs[i];
        if code == 0 && value == "ENDSEC" {
            return i + 1;
        }
        if code == 9 && value == "$INSUNITS" && i + 1 < pairs.len() {
            if let Ok(n) = pairs[i + 1].1.parse::<i32>() {
                if let Some(k) = insunits_to_metres(n) {
                    // DECLARED, not User: the FILE said so. The distinction
                    // matters because an assumed unit must never be written
                    // back out as a positive claim.
                    doc.units = cad_kernel::Units::from_metres_per_unit(
                        k, cad_kernel::UnitSource::Declared);
                }
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    i
}

fn skip_to_endsec(pairs: &[(i32, &str)], start: usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDSEC" { return i + 1; }
        i += 1;
    }
    pairs.len()
}

fn read_tables(pairs: &[(i32, &str)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDSEC" { return i + 1; }
        if c == 0 && v == "TABLE" && i + 1 < pairs.len() {
            let (c2, name) = pairs[i + 1];
            if c2 == 2 {
                match name {
                    "LAYER" => i = read_layer_table(pairs, i + 2, doc),
                    "LTYPE" => i = read_ltype_table(pairs, i + 2, doc),
                    _       => i = skip_to_endtab(pairs, i + 2),
                }
                continue;
            }
        }
        i += 1;
    }
    pairs.len()
}

fn skip_to_endtab(pairs: &[(i32, &str)], start: usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDTAB" { return i + 1; }
        i += 1;
    }
    pairs.len()
}

fn read_layer_table(pairs: &[(i32, &str)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDTAB" { return i + 1; }
        if c == 0 && v == "LAYER" {
            // Accumulate this layer's fields until the next 0-group.
            let mut name = String::new();
            let mut color = Color::Aci(7);   // ACI 7 = white default
            let mut lt_name = String::from("Continuous");
            let mut flags: i32 = 0;
            let mut off = false;   // negative 62 = layer OFF (kept apart from frozen)
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    2  => name    = pairs[i].1.to_string(),
                    62 => {
                        let aci: i32 = pairs[i].1.parse().unwrap_or(7);
                        // Negative ACI = layer off; magnitude = the color.
                        off = aci < 0;
                        color = Color::Aci(aci.unsigned_abs() as u8);
                    }
                    6  => lt_name = pairs[i].1.to_string(),
                    70 => flags |= pairs[i].1.parse::<i32>().unwrap_or(0),
                    _ => {}
                }
                i += 1;
            }
            if !name.is_empty() {
                // "0" already exists at id 0; reuse it instead of duplicating.
                // DXF group-70 flags: 0x01 = frozen, 0x04 = locked,
                // 0x10 = not plottable. OFF is carried by the negative 62 —
                // it must NOT be folded into frozen (issue #41); the kernel's
                // `LayerTable::renders` (visible && !frozen) keeps both
                // semantics distinct, exactly like the layer panel does.
                let lt_id = doc.linetypes.find(&lt_name)
                    .unwrap_or(LinetypeTable::CONTINUOUS);
                if let Some(existing) = doc.layers.find(&name) {
                    if let Some(l) = doc.layers.get_mut(existing) {
                        l.color    = color;
                        l.linetype = lt_id;
                        l.visible  = !off;
                        l.frozen   = (flags & 0x01) != 0;
                        l.locked   = (flags & 0x04) != 0;
                        l.plottable = (flags & 0x10) == 0;
                    }
                } else {
                    doc.layers.add(Layer {
                        name,
                        color,
                        linetype:   lt_id,
                        lineweight: Lineweight::Default,
                        visible:    !off,
                        locked:     (flags & 0x04) != 0,
                        frozen:     (flags & 0x01) != 0,
                        plottable:  (flags & 0x10) == 0,
                        order:      0,});
                }
            }
            continue;
        }
        i += 1;
    }
    pairs.len()
}

fn read_ltype_table(pairs: &[(i32, &str)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDTAB" { return i + 1; }
        if c == 0 && v == "LTYPE" {
            let mut name = String::new();
            let mut desc = String::new();
            let mut pattern: Vec<f32> = Vec::new();
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    2  => name = pairs[i].1.to_string(),
                    3  => desc = pairs[i].1.to_string(),
                    49 => {
                        // dash length (positive) or gap (negative) — convert to
                        // alternating positive lengths for our pattern repr
                        if let Ok(v) = pairs[i].1.parse::<f32>() {
                            pattern.push(v.abs());
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            // ByBlock / ByLayer are DXF sentinels, not real linetypes — never
            // add them as table entries (our writer emits them for AutoCAD; the
            // model represents them as Color/Linetype sentinels, not rows).
            let is_sentinel = name.eq_ignore_ascii_case("ByBlock")
                || name.eq_ignore_ascii_case("ByLayer");
            if !name.is_empty() && !is_sentinel && doc.linetypes.find(&name).is_none() {
                doc.linetypes.add(Linetype { name, description: desc, pattern });
            }
            continue;
        }
        i += 1;
    }
    pairs.len()
}

fn read_entities(pairs: &[(i32, &str)], start: usize, doc: &mut Document, skipped: &mut usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDSEC" { return i + 1; }
        if c == 0 {
            let entity_kind = v.to_string();
            // Collect this entity's fields until the next 0-group.
            let mut fields: Vec<(i32, &str)> = Vec::new();
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                fields.push((pairs[i].0, pairs[i].1));
                i += 1;
            }
            let (main, sats) = build_entity(&entity_kind, &fields, doc);
            if let Some(d) = main {
                // Boundary dobjects land FIRST so the hatch (pushed after)
                // can resolve them by handle in the same doc.
                for s in sats { doc.push(s); }
                doc.push(d);
            } else if sats.is_empty() {
                *skipped += 1;   // unsupported entity type
            }
            continue;
        }
        i += 1;
    }
    pairs.len()
}

/// Read the BLOCKS section into `doc.blocks`. Each `BLOCK … ENDBLK` becomes a
/// `Block` (name + base point + contained entities, parsed with `build_entity`,
/// so nested INSERTs and every supported geom work). Special/anonymous records
/// (names starting with `*` — *Model_Space, *Paper_Space, *U### hatch/dim
/// blocks) are skipped: their geometry already lives in ENTITIES, and importing
/// them would duplicate it. INSERTs in ENTITIES resolve to these by name.
fn read_blocks(pairs: &[(i32, &str)], start: usize, doc: &mut Document, skipped: &mut usize) -> usize {
    // TWO passes so nested INSERTs resolve regardless of definition order:
    //   pass 1 registers every real block name (empty placeholder, fixes ids);
    //   pass 2 fills each block's base + entities (build_entity now resolves
    //   nested INSERTs by name — forward references included).
    // ---- pass 1: register names -----------------------------------------
    let end;
    let mut i = start;
    loop {
        if i >= pairs.len() { end = pairs.len(); break; }
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDSEC" { end = i + 1; break; }
        if c == 0 && v == "BLOCK" {
            let name = block_name(pairs, i + 1);
            if is_real_block(&name) && doc.blocks.find(&name).is_none() {
                doc.blocks.add(Block {
                    name, base: Vec2::new(0.0, 0.0), dobjects: Vec::new(),
                    smart: false, params: Vec::new(), cut_edges: Vec::new(),
                });
            }
            i = skip_to_endblk(pairs, i + 1);
            continue;
        }
        i += 1;
    }
    // ---- pass 2: fill base point + contained entities -------------------
    let mut i = start;
    while i < end {
        let (c, v) = pairs[i];
        if c == 0 && v == "BLOCK" {
            i += 1;
            let mut name = String::new();
            let mut base = Vec2::new(0.0, 0.0);
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    2  => name   = pairs[i].1.to_string(),
                    10 => base.x = pairs[i].1.parse().unwrap_or(0.0),
                    20 => base.y = pairs[i].1.parse().unwrap_or(0.0),
                    _  => {}
                }
                i += 1;
            }
            let mut dobjects: Vec<DObject> = Vec::new();
            while i < pairs.len() {
                let (c2, v2) = pairs[i];
                if c2 == 0 && (v2 == "ENDBLK" || v2 == "ENDSEC") {
                    if v2 == "ENDBLK" { i += 1; }
                    break;
                }
                if c2 == 0 {
                    let kind = v2.to_string();
                    i += 1;
                    let mut fields: Vec<(i32, &str)> = Vec::new();
                    while i < pairs.len() && pairs[i].0 != 0 {
                        fields.push((pairs[i].0, pairs[i].1));
                        i += 1;
                    }
                    // build_entity resolves nested INSERTs via doc.blocks (all
                    // names are registered now, so order doesn't matter). Hatch
                    // boundary polylines ride along as satellites into the SAME
                    // block so block-local hatches stay self-contained.
                    let (main, sats) = build_entity(&kind, &fields, doc);
                    if let Some(d) = main {
                        dobjects.extend(sats);
                        dobjects.push(d);
                    } else if sats.is_empty() {
                        *skipped += 1;   // unsupported entity type
                    }
                    continue;
                }
                i += 1;
            }
            if let Some(id) = doc.blocks.find(&name) {
                doc.blocks.blocks[id as usize].base = base;
                doc.blocks.blocks[id as usize].dobjects = dobjects;
            }
            continue;
        }
        i += 1;
    }
    end
}

/// First `2` (name) group of a BLOCK header, scanning until the next 0-group.
fn block_name(pairs: &[(i32, &str)], start: usize) -> String {
    let mut i = start;
    while i < pairs.len() && pairs[i].0 != 0 {
        if pairs[i].0 == 2 { return pairs[i].1.to_string(); }
        i += 1;
    }
    String::new()
}

fn skip_to_endblk(pairs: &[(i32, &str)], start: usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = pairs[i];
        if c == 0 && v == "ENDBLK" { return i + 1; }
        if c == 0 && v == "ENDSEC" { return i; }
        i += 1;
    }
    pairs.len()
}

/// Whether to import a block's geometry. Skip ONLY the model/paper-space layout
/// records (their entities live in the ENTITIES section — importing them would
/// duplicate). Anonymous blocks (`*U##`/`*D##`/`*X##`, used by hatches, dynamic
/// blocks and groups) hold REAL geometry referenced by nested INSERTs, so they
/// MUST be kept — skipping them scatters fixtures into missing parts.
fn is_real_block(name: &str) -> bool {
    if name.is_empty() { return false; }
    let u = name.to_ascii_uppercase();
    !(u.starts_with("*MODEL_SPACE") || u.starts_with("*PAPER_SPACE"))
}

/// Parse one entity into (main dobject, satellite dobjects). Satellites are
/// extra dobjects that must land in the SAME container as the main one, right
/// before it: a HATCH's synthesized boundary polylines (the DXF entity carries
/// its own loop vertices, but our `Hatch` references boundary dobjects by
/// handle). Returns `(None, vec![])` for unsupported entity types — callers
/// count that as "skipped".
/// WHICH WAY ROUND AN ENTITY'S OWN COORDINATE SYSTEM SITS.
///
/// Most DXF entities are written in OBJECT COORDINATES, not world coordinates, and carry a
/// 210/220/230 extrusion vector saying which way that system faces. This reader ignored it and
/// read the numbers as world coordinates — right for the extrusion nearly every entity has, and a
/// silent mirror for the ones that do not.
///
/// It is not a rare case. AutoCAD writes `(0, 0, -1)` whenever a circle, arc or block reference is
/// MIRRORED, and several exporters write it for whole drawings. The symptom is geometry that comes
/// in back to front with no error — and only for the entities that carry it, so a plan can arrive
/// with half its blocks flipped and the walls exactly where they should be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ocs {
    /// Extrusion +Z. Object coordinates ARE world coordinates; nothing to do, which is the path
    /// almost every entity in almost every file takes.
    World,
    /// Extrusion −Z. The arbitrary-axis algorithm gives `Ax = (−1, 0, 0)` and `Ay = (0, 1, 0)`, so
    /// object `(x, y)` is world `(−x, y)`: a mirror in X, and an exact one.
    MirroredX,
    /// Anything else — an entity standing on a tilted plane.
    ///
    /// LEFT ALONE ON PURPOSE. A 2D document has nowhere to put a tilted entity: the honest
    /// transform is a projection, which turns its circles into ellipses and its right angles into
    /// something else. That is a different silent wrong answer, not a fix. A file full of tilted
    /// extrusions is 3D data arriving through a 2D door, and needs saying out loud rather than
    /// quietly flattening.
    Tilted,
}

/// Read an entity's extrusion. Absent means +Z — what the spec says, and what the vast majority
/// of entities carry.
fn entity_ocs(fields: &[(i32, &str)]) -> Ocs {
    let axis = |code: i32, dflt: f64| -> f64 {
        fields
            .iter()
            .find(|&&(c, _)| c == code)
            .and_then(|&(_, v)| v.parse::<f64>().ok())
            .unwrap_or(dflt)
    };
    let (nx, ny, nz) = (axis(210, 0.0), axis(220, 0.0), axis(230, 1.0));
    // The 1/64 threshold is the DXF arbitrary-axis algorithm's own: below it an axis counts as
    // aligned. Using the same number means this agrees with the transform it stands in for.
    if nx.abs() >= 1.0 / 64.0 || ny.abs() >= 1.0 / 64.0 {
        return Ocs::Tilted;
    }
    if nz < 0.0 { Ocs::MirroredX } else { Ocs::World }
}

fn build_entity(kind: &str, fields: &[(i32, &str)], doc: &mut Document) -> (Option<DObject>, Vec<DObject>) {
    let mut layer_name = String::new();
    let mut color: Option<Color> = None;
    let mut raw_aci: Option<i32> = None;
    let mut linetype_name: Option<String> = None;
    let mut visible = true;
    let mut truecolor_rgb: Option<u32> = None;
    let mut ltscale: Option<f32> = None;
    // helpers — return None when the field is missing or unparseable
    let get_f = |code: i32| -> Option<f64> {
        fields.iter().find(|(c, _)| *c == code).and_then(|(_, v)| v.parse().ok())
    };
    let get_i = |code: i32| -> Option<i32> {
        fields.iter().find(|(c, _)| *c == code).and_then(|(_, v)| v.parse().ok())
    };

    for (c, v) in fields {
        match c {
            8  => layer_name = v.to_string(),
            62 => {
                if let Ok(aci) = v.parse::<i32>() {
                    raw_aci = Some(aci);
                    color = Some(if aci == 256 { Color::ByLayer }
                                 else if aci == 0 { Color::ByBlock }
                                 else if aci < 0 { Color::Aci(aci.unsigned_abs() as u8) }
                                 else { Color::Aci(aci as u8) });
                }
            }
            // TrueColor (RGB packed 0xRRGGBB) — only meaningful when 62 is
            // -1 (our writer) or absent; a real ACI takes precedence.
            420 => {
                if let Ok(packed) = v.parse::<u32>() {
                    truecolor_rgb = Some(packed & 0x00FF_FFFF);
                }
            }
            6  => linetype_name = Some(v.to_string()),
            48 => {
                if let Ok(ls) = v.parse::<f32>() { ltscale = Some(ls); }
            }
            60 => visible = v.parse::<i32>().unwrap_or(0) == 0,
            _  => {}
        }
    }

    // Style resolution is shared by the entity and any satellite boundaries —
    // built before the geom match so the HATCH arm can clone it.
    let mut style = cad_kernel::Style::default();
    if let Some(ls) = ltscale { style.linetype_scale = ls; }
    if let Some(c) = color {
        style.color = match (raw_aci, c, truecolor_rgb) {
            // 62 = -1 (TrueColor marker from our writer) + a 420 value
            // → intern the RGB into the doc's TrueColorTable.
            (Some(-1), _, Some(rgb)) if rgb != 0 =>
                Color::TrueColorRef(doc.truecolors.intern(rgb)),
            (_, c, _) => c,
        };
    } else if let Some(rgb) = truecolor_rgb {
        style.color = Color::TrueColorRef(doc.truecolors.intern(rgb));
    }
    if !layer_name.is_empty() {
        if let Some(lid) = doc.layers.find(&layer_name) {
            style.layer = lid;
        }
        // If the layer wasn't seen in TABLES we'd have to create it here.
        // For now we silently fall back to layer "0" — TODO when needed.
    }
    if let Some(lt) = linetype_name {
        if let Some(ltid) = doc.linetypes.find(&lt) {
            style.linetype = ltid;
        }
    }
    style.visible = visible;

    let mut satellites: Vec<DObject> = Vec::new();
    let main = (|| -> Option<DObject> {
        // WHICH ENTITIES THIS APPLIES TO IS NOT A GUESS. The DXF reference
        // states, per entity, whether its points are world or object
        // coordinates, and the two are NOT the same list:
        //
        //   OBJECT coordinates — CIRCLE, ARC, LWPOLYLINE, POINT, INSERT.
        //                         Transformed below.
        //   WORLD  coordinates — LINE, ELLIPSE, HATCH. Left exactly as they
        //                         are, extrusion or no extrusion.
        //
        // Mirroring a LINE because it happens to carry a 210 would put it
        // somewhere it is not, which is the same defect arriving from the
        // other direction.
        let flip = entity_ocs(fields) == Ocs::MirroredX;
        // Object → world for a −Z extrusion: x negated, y kept.
        let ocs_pt = |p: Vec2| if flip { Vec2::new(-p.x, p.y) } else { p };

        let geom = match kind {
        "LINE" => Geom::Line(Line {
            a: Vec2::new(get_f(10)?, get_f(20)?),
            b: Vec2::new(get_f(11)?, get_f(21)?),
        }),
        "CIRCLE" => Geom::Circle(Circle {
            center: ocs_pt(Vec2::new(get_f(10)?, get_f(20)?)),
            radius: get_f(40)?,
        }),
        "ARC" => {
            let sa = get_f(50)?.to_radians();
            let ea = get_f(51)?.to_radians();
            // A MIRROR REVERSES THE SWEEP. An object angle θ is measured from
            // `Ax` toward `Ay`, and under a −Z extrusion that pair maps to
            // world (−1, 0) and (0, 1) — so θ arrives as π − θ, which runs the
            // other way. Keeping the start and letting the sweep run the
            // original way draws the COMPLEMENT of the arc: everything except
            // the piece that is actually there.
            let (sa, ea) = if flip {
                (std::f64::consts::PI - ea, std::f64::consts::PI - sa)
            } else {
                (sa, ea)
            };
            // `start == end` is ambiguous: the RAW (un-wrapped) difference tells
            // them apart. raw≈0 is a DEGENERATE zero-length arc — AutoCAD emits
            // these from spline-fit / explode and renders nothing, so DROP it.
            // raw≈±360 is a true full circle written as an arc → TAU. The old
            // `sweep<1e-9 → TAU` collapsed BOTH to a full circle, so a degenerate
            // arc imported as a spurious CIRCLE (bug: unwanted circle on block insert).
            let raw = ea - sa;
            let sweep = raw.rem_euclid(std::f64::consts::TAU);
            let sweep = if sweep < 1e-9 {
                if raw.abs() < 1e-6 { return None; }   // degenerate → skip
                std::f64::consts::TAU                    // genuine full circle
            } else { sweep };
            Geom::Arc(Arc {
                center: ocs_pt(Vec2::new(get_f(10)?, get_f(20)?)),
                radius: get_f(40)?,
                start_angle: sa.rem_euclid(std::f64::consts::TAU),
                sweep_angle: sweep,
            })
        }
        "ELLIPSE" => {
            // 10/20 = center, 11/21 = major-axis vector (relative to center),
            // 40 = ratio, 41 = start param, 42 = end param.
            let center = Vec2::new(get_f(10)?, get_f(20)?);
            let major  = Vec2::new(get_f(11)?, get_f(21)?);
            let ratio  = get_f(40)?;
            let el = Ellipse { center, major, ratio };
            let sp = get_f(41).unwrap_or(0.0);
            let ep = get_f(42).unwrap_or(std::f64::consts::TAU);
            // Full ellipse <-> partial: if start ~= 0 and end ~= TAU, treat as full.
            if (sp.abs() < 1e-9) && ((ep - std::f64::consts::TAU).abs() < 1e-9) {
                Geom::Ellipse(el)
            } else {
                let sweep = (ep - sp).rem_euclid(std::f64::consts::TAU);
                let sweep = if sweep < 1e-9 { std::f64::consts::TAU } else { sweep };
                Geom::EllipseArc(EllipseArc {
                    ellipse: el,
                    start_param: sp.rem_euclid(std::f64::consts::TAU),
                    sweep_param: sweep,
                })
            }
        }
        "POINT" => Geom::Point(Point {
            location: ocs_pt(Vec2::new(get_f(10)?, get_f(20)?)),
            style: 0, size: 0.0,
        }),
        "MTEXT" => {
            // FULL MTEXT round-trip: the string may carry \P/\C/\H/\f
            // codes — stored verbatim (the renderer parses runs). \P →
            // \n; DXF-escaped backslashes (\\ in the file) unescape.
            let mut text = fields.iter().find(|(c, _)| *c == 1)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            // MTEXT stores \\ for a literal backslash and \P for breaks;
            // our storage uses plain \n — convert the paragraph breaks back.
            text = text.replace("\\P", "\n").replace("\\\\", "\\");
            let height = get_f(40).unwrap_or(0.18);
            let angle = get_f(50).unwrap_or(0.0).to_radians();
            let bold = false;
            let mut outline_only = false;
            let mut underline = false;
            // AutoRASM XDATA (our own export) restores the exact specs.
            let mut font_name = String::new();
            let mut outline_width = 0.0;
            for (c, v) in fields {
                match c {
                    1001 if *v == "AutoRASM" => {}
                    1070 => {
                        let f = v.parse().unwrap_or(0);
                        outline_only = (f & 2) != 0;
                        underline = (f & 4) != 0;
                    }
                    1040 => outline_width = v.parse().unwrap_or(0.0),
                    1000 => font_name = v.to_string(),
                    _ => {}
                }
            }
            Geom::Text(cad_kernel::Text {
                position: Vec2::new(get_f(10)?, get_f(20)?),
                height,
                angle,
                text,
                h_align: cad_kernel::TextHAlign::Left,
                v_align: cad_kernel::TextVAlign::Baseline,
                style: cad_kernel::TextStyleTable::STANDARD,
                font_name,
                bold,
                oblique: 0.0,
                width_factor: 1.0,
                outline_only,
                outline_width,
                underline,
                list_mode: cad_kernel::TextListKind::None,
                line_spacing: 1.5,
            })
        }
        "DIMENSION" => {
            // REAL DIMENSION entity — mirror of the writer. 70&7 = type,
            // 10/20 = def point, 11/21 = text mid, 13/14 = extension origins,
            // 15 = chord/arc point, 3 = style name, 1 = text override
            // ("<>" = measured). Linear 50 = rotation (0/90 → H/V).
            let ty = get_i(70).unwrap_or(0) & 0x0F;
            let p10 = Vec2::new(get_f(10)?, get_f(20)?);
            let p11 = Vec2::new(get_f(11).unwrap_or(0.0), get_f(21).unwrap_or(0.0));
            let p13 = Vec2::new(get_f(13).unwrap_or(0.0), get_f(23).unwrap_or(0.0));
            let p14 = Vec2::new(get_f(14).unwrap_or(0.0), get_f(24).unwrap_or(0.0));
            let p15 = Vec2::new(get_f(15).unwrap_or(0.0), get_f(25).unwrap_or(0.0));
            let style_name = fields.iter().find(|(c, _)| *c == 3)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            let style = doc.dim_styles.styles.iter()
                .position(|st| st.name.eq_ignore_ascii_case(&style_name))
                .unwrap_or(cad_kernel::DimStyleTable::STANDARD as usize) as u32;
            let text_override = fields.iter().find(|(c, _)| *c == 1)
                .map(|(_, v)| v.to_string())
                .filter(|v| !v.is_empty() && v != "<>");
            let kind = match ty {
                1 => cad_kernel::DimKind::Linear {
                    p1: p13, p2: p14, dimline_pos: p10,
                    ortho: cad_kernel::LinearOrtho::Aligned,
                },
                0 => {
                    let ang = get_f(50).unwrap_or(0.0).to_radians();
                    let ortho = if (ang - std::f64::consts::FRAC_PI_2).abs() < 0.01 {
                        cad_kernel::LinearOrtho::Vertical
                    } else {
                        cad_kernel::LinearOrtho::Horizontal
                    };
                    cad_kernel::DimKind::Linear {
                        p1: p13, p2: p14, dimline_pos: p10, ortho,
                    }
                }
                2 => cad_kernel::DimKind::Angular {
                    vertex: p10, p1: p13, p2: p14, arc_pos: p15,
                },
                3 => cad_kernel::DimKind::Diameter {
                    center: p10, on_circle: p15, leader_end: p11,
                },
                4 => cad_kernel::DimKind::Radius {
                    center: p10, on_circle: p15, leader_end: p11,
                },
                8 => {
                    // Arc-length: 10 = center, 40 = radius, 13/14 = arc
                    // start/end points (start_angle/sweep derived).
                    let center = p10;
                    let radius = get_f(41).or_else(|| get_f(40)).unwrap_or(1.0);
                    let s13 = Vec2::new(get_f(13).unwrap_or(center.x),
                                        get_f(23).unwrap_or(center.y));
                    let s14 = Vec2::new(get_f(14).unwrap_or(s13.x),
                                        get_f(24).unwrap_or(s13.y));
                    let start_angle = (s13 - center).angle();
                    let mut sweep = (s14 - center).angle() - start_angle;
                    if sweep < 0.0 { sweep += std::f64::consts::TAU; }
                    cad_kernel::DimKind::ArcLen {
                        center, radius, start_angle, sweep, leader_end: p11,
                    }
                }
                6 => cad_kernel::DimKind::Ordinate {
                    datum: Vec2::new(get_f(13).unwrap_or(0.0), get_f(23).unwrap_or(0.0)),
                    point: p10, leader_end: p11,
                    is_x: (get_i(70).unwrap_or(0) & 64) != 0,
                },
                _ => return None,
            };
            Geom::Dimension(cad_kernel::Dim {
                kind, style, text_override,
            })
        }
        "TEXT" => {
            // Mirror the TEXT writer (~write side): 10/20 = insertion point,
            // 40 = height, 1 = the string, 50 = rotation DEGREES, 72 = HAlign
            // (0/1/2 = Left/Center/Right). Code 1 is textual, so read it straight
            // off `fields` (get_f/get_i are numeric-only). Text style (code 7) and
            // vertical alignment aren't emitted by the writer → default STANDARD /
            // Baseline. NOTE: an exported DIMENSION is written as a TEXT entity, so
            // it round-trips back here as a Text dobject carrying its label — the
            // intended v1 behaviour (no Dimension reconstruction in this slice).
            let text = fields.iter().find(|(c, _)| *c == 1)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            let h_align = match get_i(72).unwrap_or(0) {
                1 => cad_kernel::TextHAlign::Center,
                2 => cad_kernel::TextHAlign::Right,
                _ => cad_kernel::TextHAlign::Left,
            };
            // Standard 51/41 + AutoRASM XDATA (1070 flags / 1040 width / 1000 font).
            let flags = get_i(1070).unwrap_or(0);
            let font_name = fields.iter().find(|(c, _)| *c == 1000)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            Geom::Text(cad_kernel::Text {
                position: Vec2::new(get_f(10)?, get_f(20)?),
                height:   get_f(40)?,
                angle:    get_f(50).unwrap_or(0.0).to_radians(),
                text,
                h_align,
                v_align:  cad_kernel::TextVAlign::Baseline,
                style:    cad_kernel::TextStyleTable::STANDARD,
                oblique:       get_f(51).unwrap_or(0.0).to_radians(),
                width_factor:  get_f(41).unwrap_or(1.0),
                bold:          flags & 1 != 0,
                outline_only:  flags & 2 != 0,
                outline_width: get_f(1040).unwrap_or(0.0),
                underline:     flags & 4 != 0,
                font_name,
                // A DXF TEXT is a single line — paragraphs export as stacked
                // TEXT records, so imports come back as single-line, no list.
                list_mode:     cad_kernel::TextListKind::None,
                line_spacing:  1.5,
            })
        }
        "SPLINE" => {
            // Round-trip mirror of the writer. Walk fields in order (like
            // LWPOLYLINE): 71 = degree, 10/20 = control points (paired, in order),
            // 41 = weights (in control-point order). Code 40 (knots) is IGNORED —
            // the Spline constructor rebuilds a clamped-uniform knot vector, so a
            // foreign non-uniform spline re-fits to clamped-uniform (v1 interop
            // debt). Defensive: the constructors PANIC on degenerate input.
            let Some(degree) = get_i(71) else { return None; };   // degree missing
            let degree = degree as usize;
            let mut ctrl: Vec<Vec2> = Vec::new();
            let mut weights: Vec<f64> = Vec::new();
            let mut cur: Option<Vec2> = None;
            for (c, v) in fields {
                match c {
                    10 => {
                        if let Some(p) = cur.take() { ctrl.push(p); }
                        cur = Some(Vec2 { x: v.parse().unwrap_or(0.0), y: 0.0 });
                    }
                    20 => { if let Some(p) = cur.as_mut() { p.y = v.parse().unwrap_or(0.0); } }
                    41 => weights.push(v.parse().unwrap_or(1.0)),
                    _  => {}   // 40 (knots) and everything else ignored
                }
            }
            if let Some(p) = cur.take() { ctrl.push(p); }
            // Guard the panicking constructors: need ctrl.len() > degree (and
            // non-empty). A negative/huge degree fails this too.
            if ctrl.is_empty() || ctrl.len() <= degree { return None; }
            let spline = if weights.len() == ctrl.len() {
                cad_kernel::Spline::new(degree, ctrl, weights)          // rational
            } else {
                cad_kernel::Spline::new_bspline(degree, ctrl)           // non-rational
            };
            Geom::Spline(spline)
        }
        "LWPOLYLINE" => {
            let count = get_i(90).unwrap_or(0) as usize;
            let flags = get_i(70).unwrap_or(0);
            let closed = (flags & 0x01) != 0;
            // For LWPOLYLINE, vertex coords are interleaved 10/20 group codes
            // (and 42 for bulge per vertex). We walk fields in order and pair them.
            let mut vertices: Vec<PolyVertex> = Vec::with_capacity(count);
            let mut vwidths: Vec<(f64, f64)> = Vec::with_capacity(count);
            let mut cur: Option<Vec2> = None;
            let mut cur_bulge = 0.0_f64;
            let mut cur_sw = 0.0_f64;   // 40 = start width of segment at this vertex
            let mut cur_ew = 0.0_f64;   // 41 = end width
            let mut const_w = 0.0_f64;  // 43 = constant width for the whole pline
            for (c, v) in fields {
                match c {
                    10 => {
                        if let Some(p) = cur.take() {
                            vertices.push(PolyVertex { pos: ocs_pt(p), bulge: cur_bulge });
                            vwidths.push((cur_sw, cur_ew));
                            cur_bulge = 0.0; cur_sw = 0.0; cur_ew = 0.0;
                        }
                        cur = Some(Vec2 { x: v.parse().unwrap_or(0.0), y: 0.0 });
                    }
                    20 => {
                        if let Some(p) = cur.as_mut() {
                            p.y = v.parse().unwrap_or(0.0);
                        }
                    }
                    40 => cur_sw = v.parse().unwrap_or(0.0),
                    41 => cur_ew = v.parse().unwrap_or(0.0),
                    42 => {
                        let b = v.parse().unwrap_or(0.0);
                        // A MIRROR FLIPS THE ARC'S BOW DIRECTION: bulge is
                        // signed CCW-positive, and under a −Z extrusion the
                        // vertex order reverses, so the same bulge value would
                        // arc into the wall instead of out of it.
                        cur_bulge = if flip { -b } else { b };
                    }
                    43 => const_w = v.parse().unwrap_or(0.0),
                    _ => {}
                }
            }
            if let Some(p) = cur.take() {
                vertices.push(PolyVertex { pos: ocs_pt(p), bulge: cur_bulge });
                vwidths.push((cur_sw, cur_ew));
            }
            if vertices.is_empty() { return None; }
            // Map to per-SEGMENT widths (n-1 open, n closed). Prefer per-vertex
            // 40/41; fall back to constant width 43; empty when all zero.
            let seg_count = if closed { vertices.len() } else { vertices.len().saturating_sub(1) };
            vwidths.truncate(seg_count);
            let widths = if vwidths.iter().any(|&(a, b)| a.abs() > 1e-12 || b.abs() > 1e-12) {
                vwidths
            } else if const_w.abs() > 1e-12 {
                vec![(const_w, const_w); seg_count]
            } else {
                Vec::new()
            };
            Geom::Polyline(Polyline { vertices, closed, widths })
        }
        "INSERT" => {
            // Block reference: 2 = block name, 10/20 = insertion point,
            // 41/42 = x/y scale, 50 = rotation (degrees). MINSERT arrays
            // (70/71) ignored. A negative axis scale = a MIRROR — encode it as
            // a positive magnitude + mirror_x + a rotation adjustment so the
            // |sx|==|sy| (similarity) case (the common furniture mirror) is
            // exact. Non-uniform |sx|≠|sy| isn't modelled (uses |41|).
            let bname = fields.iter().find(|(c, _)| *c == 2).map(|(_, v)| v.to_string())?;
            let block = doc.blocks.find(&bname)?;   // unknown/skipped block → drop
            let sx = get_f(41).unwrap_or(1.0);
            let sy = get_f(42).unwrap_or(1.0);
            // Factor signs out into mirror_x + a π rotation; the per-axis
            // MAGNITUDES go to scale / scale_y (non-uniform → ellipses).
            let mirror_x = (sx < 0.0) != (sy < 0.0);
            let extra = if sy < 0.0 { std::f64::consts::PI } else { 0.0 };
            Geom::BlockRef(BlockRef {
                block,
                insert:   ocs_pt(Vec2::new(get_f(10)?, get_f(20)?)),
                scale:    sx.abs().max(1e-9),
                scale_y:  sy.abs().max(1e-9),
                rotation: get_f(50).unwrap_or(0.0).to_radians() + extra,
                mirror_x,
                param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            })
        }
        "LEADER" => {
            // Mirror of the writer: 71 arrow flag, 72 path type, 73 vertex
            // count, then one 10/20/30 per vertex. The annotation TEXT that
            // follows (a separate TEXT entity) becomes the label position.
            let arrow = get_i(71).unwrap_or(1) != 0;
            let n = get_i(73).unwrap_or(0).max(0) as usize;
            let mut pts: Vec<Vec2> = Vec::with_capacity(n);
            let mut pi = 0usize;
            while pi < n {
                let x = fields.iter().filter(|(c, _)| *c == 10).nth(pi)
                    .and_then(|(_, v)| v.parse::<f64>().ok());
                let y = fields.iter().filter(|(c, _)| *c == 20).nth(pi)
                    .and_then(|(_, v)| v.parse::<f64>().ok());
                match (x, y) {
                    (Some(x), Some(y)) => pts.push(Vec2::new(x, y)),
                    _ => break,
                }
                pi += 1;
            }
            if pts.len() < 2 { return None; }
            let label = cad_kernel::Text {
                position: *pts.last().unwrap(),
                height: 0.25,
                angle: 0.0,
                text: String::new(),
                h_align: cad_kernel::TextHAlign::Left,
                v_align: cad_kernel::TextVAlign::Baseline,
                style: cad_kernel::TextStyleTable::STANDARD,
                font_name: String::new(), bold: false, oblique: 0.0,
                width_factor: 1.0, outline_only: false, outline_width: 0.0,
                underline: false, list_mode: cad_kernel::TextListKind::None,
                line_spacing: 1.5,
            };
            Geom::Leader(cad_kernel::Leader { pts, label, arrow })
        }
        "ATTDEF" => {
            // Mirror of the writer: 2 = tag, 3 = prompt, 1 = default,
            // 10/20 = position, 40 = height, 50 = angle.
            let tag = fields.iter().find(|(c, _)| *c == 2)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            let prompt = fields.iter().find(|(c, _)| *c == 3)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            let default = fields.iter().find(|(c, _)| *c == 1)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            Geom::AttrDef(cad_kernel::AttrDef {
                tag,
                prompt,
                default,
                position: Vec2::new(get_f(10)?, get_f(20)?),
                height: get_f(40).unwrap_or(0.25),
                angle: get_f(50).unwrap_or(0.0).to_radians(),
                style: cad_kernel::TextStyleTable::STANDARD,
                visible: true,
            })
        }
        "XLINE" => {
            // 10/20 base point, 11/21 direction vector.
            let base = Vec2::new(get_f(10)?, get_f(20)?);
            let dir = Vec2::new(
                get_f(11).unwrap_or(1.0),
                get_f(21).unwrap_or(0.0),
            );
            Geom::Xline(cad_kernel::Xline::new(base, dir))
        }
        "RAY" => {
            let base = Vec2::new(get_f(10)?, get_f(20)?);
            let dir = Vec2::new(
                get_f(11).unwrap_or(1.0),
                get_f(21).unwrap_or(0.0),
            );
            Geom::Ray(cad_kernel::Ray::new(base, dir))
        }
        "CENTERMARK" => {
            // Mirror of the writer: 10/20 center, 40 arm size, 50 rotation.
            Geom::CenterMark(cad_kernel::CenterMark {
                center: Vec2::new(get_f(10)?, get_f(20)?),
                size: get_f(40).unwrap_or(0.25),
                rotation: get_f(50).unwrap_or(0.0).to_radians(),
            })
        }
        "HATCH" => {
            // then per path: 92 flags, 72 has_bulge, 73 closed, 93 vertex
            // count, 10/20 (+42 bulge) vertices, 97 source count; then 75
            // style, 76 type, 98 seed count. Our `Hatch` stores boundary
            // HANDLES, not vertices — so each polyline path is materialized
            // as a closed Polyline satellite dobject in the same container,
            // and the hatch references it by handle (matches how the app
            // represents hatches: boundary dobjects + handle refs).
            let pattern_name = fields.iter().find(|(c, _)| *c == 2)
                .map(|(_, v)| v.to_string()).unwrap_or_default();
            let mut paths: Vec<Vec<PolyVertex>> = Vec::new();
            let mut cur: Option<Vec<PolyVertex>> = None;
            let mut accepting = false;   // only polyline paths (92 bit 1)
            let mut past_seed = false;   // 98 seen → 10/20 are seed points
            for (c, v) in fields {
                match c {
                    92 => {
                        if let Some(p) = cur.take() {
                            if accepting && p.len() >= 3 { paths.push(p); }
                        }
                        accepting = (v.parse::<i32>().unwrap_or(0) & 2) != 0;
                        cur = if accepting { Some(Vec::new()) } else { None };
                    }
                    10 => {
                        if accepting && !past_seed {
                            if let Some(p) = cur.as_mut() {
                                p.push(PolyVertex {
                                    pos: Vec2 { x: v.parse().unwrap_or(0.0), y: 0.0 },
                                    bulge: 0.0,
                                });
                            }
                        }
                    }
                    20 => {
                        if accepting && !past_seed {
                            if let Some(p) = cur.as_mut() {
                                if let Some(last) = p.last_mut() {
                                    last.pos.y = v.parse().unwrap_or(0.0);
                                }
                            }
                        }
                    }
                    42 => {
                        if accepting && !past_seed {
                            if let Some(p) = cur.as_mut() {
                                if let Some(last) = p.last_mut() {
                                    last.bulge = v.parse().unwrap_or(0.0);
                                }
                            }
                        }
                    }
                    97 => {
                        if let Some(p) = cur.take() {
                            if accepting && p.len() >= 3 { paths.push(p); }
                        }
                    }
                    98 => past_seed = true,
                    _ => {}
                }
            }
            if let Some(p) = cur.take() {
                if accepting && p.len() >= 3 { paths.push(p); }
            }
            // Materialize each loop as a closed boundary polyline.
            let mut handles: Vec<u64> = Vec::with_capacity(paths.len());
            for path in paths {
                let bd = DObject::with_style(Geom::Polyline(Polyline {
                    vertices: path,
                    closed: true,
                    widths: Vec::new(),
                }), style.clone());
                handles.push(bd.handle);
                satellites.push(bd);
            }
            let pattern = if pattern_name.eq_ignore_ascii_case("SOLID") {
                cad_kernel::HatchPattern::Solid
            } else {
                cad_kernel::HatchPattern::Pattern {
                    name: if pattern_name.is_empty() { "ANSI31".into() } else { pattern_name },
                    scale: get_f(41).unwrap_or(1.0),
                    angle_deg: get_f(52).unwrap_or(0.0),
                }
            };
            Geom::Hatch(cad_kernel::Hatch { boundary_handles: handles, pattern })
        }
        _ => return None,   // unknown entity type — counted as skipped
    };

        Some(DObject::with_style(geom, style))
    })();
    (main, satellites)
}

// ============================================================================
//   WRITER
// ============================================================================

/// Serialize a `Document` to AC1015 (AutoCAD 2000) DXF. Emits the structure
/// AutoCAD's DXFIN needs to OPEN the file: a HEADER with `$ACADVER`/`$HANDSEED`,
/// the required symbol TABLES (VPORT / LTYPE / LAYER / STYLE / VIEW / UCS /
/// APPID / DIMSTYLE / BLOCK_RECORD), the mandatory `*Model_Space`/`*Paper_Space`
/// BLOCK defs plus one per real block, then ENTITIES — every record / block /
/// entity carrying a synthesized hex handle (`5`), an owner back-pointer (`330`)
/// and `100` subclass markers.
///
/// Handles are synthesized at WRITE TIME (a monotonic counter) and NOT persisted
/// on the `Document` — our own `read_dxf` ignores handles / `100` markers / the
/// extra tables, so the DXF still round-trips through the reader unchanged.
///
/// Scope = openability. Real `HATCH` / `DIMENSION` fidelity is out of scope
/// (D2/D3) — those arms are unchanged. A minimal OBJECTS section (root
/// dictionary + `ACAD_PLOTSTYLENAME` → a `Normal` plot-style placeholder) is
/// emitted so AC1015 LAYER records can carry the mandatory `390` plot-style
/// pointer AutoCAD demands ("Did not receive PlotStyleName").
/// Human-readable DXF export degradations (issue #14): the writer degrades
/// some shape classes on purpose (SOLID hatch, exploded dim, wall → 2 lines,
/// viewports dropped, TrueColor → ByLayer). Report them so a DXF save is
/// never silently lossy. Empty when the document exports losslessly.
pub fn dxf_export_degradations(doc: &Document) -> Vec<String> {
    let mut hatches = 0usize;
    let mut dims = 0usize;
    let mut walls = 0usize;
    let mut viewports = 0usize;
    let mut truecolor = 0usize;
    for d in &doc.dobjects {
        match &d.geom {
            Geom::Hatch(_) => hatches += 1,
            Geom::Dimension(_) => dims += 1,
            Geom::Wall(_) => walls += 1,
            Geom::Viewport(_) => viewports += 1,
            _ => {}
        }
        if matches!(d.style.color, Color::TrueColorRef(_)) { truecolor += 1; }
    }
    let mut out = Vec::new();
    if hatches > 0   { out.push(format!("{hatches} hatch(es) exported as SOLID fill (pattern lost)")); }
    if dims > 0      { out.push(format!("{dims} dimension(s) exported as plain TEXT (structure lost)")); }
    if walls > 0     { out.push(format!("{walls} wall(s) exported as 2 LINES (smart wall lost)")); }
    if viewports > 0 { out.push(format!("{viewports} viewport(s) not exported")); }
    if truecolor > 0 { out.push(format!("{truecolor} TrueColor object(s) downgraded to ByLayer")); }
    out
}

pub fn write_dxf(doc: &Document) -> String {
    // Build the body (TABLES → BLOCKS → ENTITIES → OBJECTS) FIRST so every handle
    // is allocated before we emit `$HANDSEED = max+1` in the HEADER.
    let mut h = HandleGen::new();
    // The plot-style placeholder is referenced by every LAYER (`390`) in TABLES
    // yet defined in OBJECTS (written last), so its handle set is reserved up
    // front. Reserving them first just makes them the lowest handles — order in
    // the file doesn't matter, only uniqueness + coherent back-pointers.
    let obj = ObjectHandles::alloc(&mut h);
    let mut body = String::with_capacity(64 * 1024);
    let brt = write_tables(&mut body, doc, &mut h, &obj);
    write_blocks(&mut body, doc, &mut h, &brt);   // MUST precede ENTITIES —
                                                  // read_dxf dispatches sections
                                                  // in file order; an INSERT
                                                  // resolves its block by name.
    write_entities(&mut body, doc, &mut h, &brt);
    write_objects(&mut body, &obj);               // after ENTITIES (standard order)

    let mut s = String::with_capacity(body.len() + 512);
    write_header(&mut s, &h, doc);
    s.push_str(&body);
    s.push_str("0\nEOF\n");
    s
}

fn pair(s: &mut String, code: i32, value: &str) {
    s.push_str(&format!("{}\n{}\n", code, value));
}
fn pair_f(s: &mut String, code: i32, v: f64) {
    s.push_str(&format!("{}\n{}\n", code, v));
}
fn pair_i(s: &mut String, code: i32, v: i32) {
    s.push_str(&format!("{}\n{}\n", code, v));
}

/// Write-time DXF handle allocator. Every table record, BLOCK and entity needs
/// a unique hex handle (group `5`), and `$HANDSEED` must exceed them all.
/// Synthesized here and never persisted (the reader ignores handles), so they
/// need not be stable across round-trips.
struct HandleGen { next: u64 }
impl HandleGen {
    fn new() -> Self { HandleGen { next: 0x100 } }
    /// Next unique handle — uppercase hex, no `0x`.
    fn alloc(&mut self) -> String { let n = self.next; self.next += 1; format!("{:X}", n) }
    /// `$HANDSEED` value — one past the highest handle handed out so far.
    fn seed(&self) -> String { format!("{:X}", self.next) }
}

/// BLOCK_RECORD handles, so entity / BLOCK ownership (`330`) is coherent within
/// the file: model-space entities are owned by `*Model_Space`, a block's own
/// entities by that block's record.
struct BlockRecords {
    model_space: String,
    paper_space: String,
    real:        Vec<(String, String)>,   // (block name, BLOCK_RECORD handle)
}
impl BlockRecords {
    /// Owning BLOCK_RECORD handle for a block name (falls back to model space).
    fn owner_for(&self, name: &str) -> &str {
        self.real.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, hh)| hh.as_str())
            .unwrap_or(&self.model_space)
    }
}

/// Handles for the minimal OBJECTS graph AC1015 needs so LAYER records can
/// reference a plot style (`390`): the root named-object dictionary, the empty
/// `ACAD_GROUP` dictionary, the `ACAD_PLOTSTYLENAME` dictionary, and the single
/// `Normal` plot-style placeholder every layer points at.
struct ObjectHandles {
    root:        String,
    group:       String,
    psns:        String,
    placeholder: String,
}
impl ObjectHandles {
    fn alloc(h: &mut HandleGen) -> Self {
        ObjectHandles {
            root:        h.alloc(),
            group:       h.alloc(),
            psns:        h.alloc(),
            placeholder: h.alloc(),
        }
    }
}

/// OBJECTS section: the root named-object dictionary, an empty `ACAD_GROUP`, and
/// the `ACAD_PLOTSTYLENAME` dictionary whose default + `Normal` entry point at a
/// single `ACDBPLACEHOLDER`. LAYER records reference this placeholder via `390`.
/// (The reader skips the whole section via `skip_to_endsec`.)
fn write_objects(s: &mut String, o: &ObjectHandles) {
    pair(s, 0, "SECTION");
    pair(s, 2, "OBJECTS");

    // Root (named object) dictionary.
    pair(s, 0, "DICTIONARY");
    pair(s, 5, &o.root);
    pair(s, 330, "0");
    pair(s, 100, "AcDbDictionary");
    pair_i(s, 281, 1);
    pair(s, 3, "ACAD_GROUP");         pair(s, 350, &o.group);
    pair(s, 3, "ACAD_PLOTSTYLENAME"); pair(s, 350, &o.psns);

    // ACAD_GROUP — empty dictionary.
    pair(s, 0, "DICTIONARY");
    pair(s, 5, &o.group);
    pair(s, 330, &o.root);
    pair(s, 100, "AcDbDictionary");
    pair_i(s, 281, 1);

    // ACAD_PLOTSTYLENAME — dictionary WITH DEFAULT → the Normal placeholder.
    pair(s, 0, "ACDBDICTIONARYWITHDEFAULT");
    pair(s, 5, &o.psns);
    pair(s, 330, &o.root);
    pair(s, 100, "AcDbDictionary");
    pair_i(s, 281, 1);
    pair(s, 3, "Normal"); pair(s, 350, &o.placeholder);
    pair(s, 100, "AcDbDictionaryWithDefault");
    pair(s, 340, &o.placeholder);

    // The single Normal plot-style placeholder.
    pair(s, 0, "ACDBPLACEHOLDER");
    pair(s, 5, &o.placeholder);
    pair(s, 330, &o.psns);

    pair(s, 0, "ENDSEC");
}

fn write_header(s: &mut String, h: &HandleGen, doc: &Document) {
    pair(s, 0, "SECTION");
    pair(s, 2, "HEADER");
    pair(s, 9, "$ACADVER");     pair(s, 1, "AC1015");   // AutoCAD 2000 ASCII level
    pair(s, 9, "$HANDSEED");    pair(s, 5, &h.seed());  // > every handle in the file
    // $INSUNITS — but ONLY a unit somebody actually stated. A drawing whose
    // unit was merely ASSUMED (every file made before units existed) writes 0
    // = unitless, which is what this writer effectively emitted before by
    // omitting the variable entirely. Stamping such a file "6 = metres" would
    // turn a default nobody chose into a positive claim, and autosave would do
    // it silently to every .dxf the user has open.
    let code = match doc.units.source {
        cad_kernel::UnitSource::Assumed => 0,
        _ => metres_to_insunits(doc.units.metres_per_unit),
    };
    pair(s, 9, "$INSUNITS");    pair_i(s, 70, code);
    pair(s, 9, "$DWGCODEPAGE"); pair(s, 3, "ANSI_1252");
    pair(s, 9, "$PSTYLEMODE");  pair_i(s, 290, 1);      // color-dependent plot styles
    pair(s, 9, "$LTSCALE");     pair_f(s, 40, 1.0);     // global linetype scale
    pair(s, 0, "ENDSEC");
}

/// Inverse of [`insunits_to_metres`]; 0 (unitless) when the scale is not a
/// standard unit, since inventing a nearest match would misstate the drawing.
fn metres_to_insunits(m: f64) -> i32 {
    const NEAR: f64 = 1e-9;
    for (code, metres) in [(1, 0.0254), (2, 0.3048), (4, 0.001), (5, 0.01),
                           (6, 1.0), (10, 0.9144), (14, 0.1)] {
        if (m - metres).abs() < NEAR {
            return code;
        }
    }
    0
}

/// Open a symbol TABLE (`0 TABLE … 100 AcDbSymbolTable / 70 count`). Returns the
/// table's own handle; its records point back to it via `330`.
fn begin_table(s: &mut String, h: &mut HandleGen, name: &str, count: i32) -> String {
    let th = h.alloc();
    pair(s, 0, "TABLE");
    pair(s, 2, name);
    pair(s, 5, &th);
    pair(s, 330, "0");                       // owned by the implicit root
    pair(s, 100, "AcDbSymbolTable");
    pair_i(s, 70, count);
    th
}

/// Open a table RECORD with handle + owner + subclass markers. `handle_code` is
/// `5` for every table EXCEPT DIMSTYLE (historically `105`). Returns the record
/// handle. The caller emits the record's `2 name` / `70 flags` / type fields.
fn begin_record(s: &mut String, h: &mut HandleGen, kind: &str, owner: &str,
                subclass: &str, handle_code: i32) -> String {
    let rh = h.alloc();
    pair(s, 0, kind);
    pair(s, handle_code, &rh);
    pair(s, 330, owner);
    pair(s, 100, "AcDbSymbolTableRecord");
    pair(s, 100, subclass);
    rh
}

/// The real (TrueType) font a text should render with in DXF: the entity's own
/// `font_name`, else its style's font. `None` for the egui built-ins
/// ("standard"/"monospace"/empty) → those fall back to the STANDARD style.
fn effective_text_font(t: &cad_kernel::Text, doc: &Document) -> Option<String> {
    let f = if !t.font_name.is_empty() {
        t.font_name.clone()
    } else {
        doc.text_styles.get(t.style).map(|s| s.font_name.clone()).unwrap_or_default()
    };
    let fl = f.to_lowercase();
    if f.is_empty() || fl == "standard" || fl == "monospace" {
        None
    } else {
        Some(f)
    }
}

fn write_tables(s: &mut String, doc: &Document, h: &mut HandleGen, obj: &ObjectHandles) -> BlockRecords {
    pair(s, 0, "SECTION");
    pair(s, 2, "TABLES");

    // ---- VPORT (AutoCAD needs the *Active viewport) ----
    let vt = begin_table(s, h, "VPORT", 1);
    begin_record(s, h, "VPORT", &vt, "AcDbViewportTableRecord", 5);
    pair(s, 2, "*Active"); pair_i(s, 70, 0);
    pair_f(s, 10, 0.0); pair_f(s, 20, 0.0);       // lower-left corner
    pair_f(s, 11, 1.0); pair_f(s, 21, 1.0);       // upper-right corner
    pair_f(s, 12, 0.0); pair_f(s, 22, 0.0);       // view center
    pair_f(s, 13, 0.0); pair_f(s, 23, 0.0);       // snap base
    pair_f(s, 14, 10.0); pair_f(s, 24, 10.0);     // snap spacing
    pair_f(s, 15, 10.0); pair_f(s, 25, 10.0);     // grid spacing
    pair_f(s, 16, 0.0); pair_f(s, 26, 0.0); pair_f(s, 36, 1.0);   // view direction
    pair_f(s, 17, 0.0); pair_f(s, 27, 0.0); pair_f(s, 37, 0.0);   // view target
    pair_f(s, 40, 297.0); pair_f(s, 41, 1.5); pair_f(s, 42, 50.0);
    pair_f(s, 43, 0.0); pair_f(s, 44, 0.0);
    pair_f(s, 50, 0.0); pair_f(s, 51, 0.0);
    pair_i(s, 71, 0); pair_i(s, 72, 100); pair_i(s, 73, 1); pair_i(s, 74, 3);
    pair_i(s, 75, 0); pair_i(s, 76, 1); pair_i(s, 77, 0); pair_i(s, 78, 0);
    pair(s, 0, "ENDTAB");

    // ---- LTYPE (ByBlock + ByLayer are mandatory; then our named linetypes) ----
    let lt_tbl = begin_table(s, h, "LTYPE", 2 + doc.linetypes.len() as i32);
    for special in ["ByBlock", "ByLayer"] {
        begin_record(s, h, "LTYPE", &lt_tbl, "AcDbLinetypeTableRecord", 5);
        pair(s, 2, special); pair_i(s, 70, 0);
        pair(s, 3, ""); pair_i(s, 72, 65); pair_i(s, 73, 0); pair_f(s, 40, 0.0);
    }
    for lt in &doc.linetypes.linetypes {
        begin_record(s, h, "LTYPE", &lt_tbl, "AcDbLinetypeTableRecord", 5);
        pair(s, 2, &lt.name); pair_i(s, 70, 0);
        pair(s, 3, &lt.description); pair_i(s, 72, 65);   // alignment code 'A'
        pair_i(s, 73, lt.pattern.len() as i32);
        let total: f32 = lt.pattern.iter().sum();
        pair_f(s, 40, total as f64);
        for (i, p) in lt.pattern.iter().enumerate() {
            // dash = positive, gap = negative; 74 = simple element (no shape/text).
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            pair_f(s, 49, (*p as f64) * sign);
            pair_i(s, 74, 0);
        }
    }
    pair(s, 0, "ENDTAB");

    // ---- LAYER (AutoCAD REQUIRES the default layer "0") ----
    // This codebase's default layer is NOT named "0" (e.g. "LAYER B"), and a
    // user can rename/remove "0", so synthesize a canonical "0" whenever the
    // document lacks one — else AutoCAD aborts: "Missing Default entry 0 in
    // SymbolTable:LAYER".
    let has_zero = doc.layers.layers.iter().any(|l| l.name == "0");
    let layer_count = doc.layers.len() as i32 + if has_zero { 0 } else { 1 };
    let ly_tbl = begin_table(s, h, "LAYER", layer_count);
    if !has_zero {
        begin_record(s, h, "LAYER", &ly_tbl, "AcDbLayerTableRecord", 5);
        pair(s, 2, "0");                 // white / Continuous / thawed / unlocked
        pair_i(s, 70, 0);
        pair_i(s, 62, 7);
        pair(s, 6, "Continuous");
        pair_i(s, 370, -3);
        pair(s, 390, &obj.placeholder);
    }
    for layer in &doc.layers.layers {
        begin_record(s, h, "LAYER", &ly_tbl, "AcDbLayerTableRecord", 5);
        pair(s, 2, &layer.name);
        // Flags: bit 0 = frozen, bit 2 = locked, bit 4 = not plottable.
        let mut flags = 0_i32;
        if layer.frozen { flags |= 0x01; }
        if layer.locked { flags |= 0x04; }
        if !layer.plottable { flags |= 0x10; }
        pair_i(s, 70, flags);
        // Color: ACI index (negative = layer off). TrueColor → 7 fallback.
        let aci = match layer.color { Color::Aci(i) => i as i32, _ => 7 };
        let aci_signed = if layer.visible { aci } else { -aci.abs().max(1) };
        pair_i(s, 62, aci_signed);
        let lt_name = doc.linetypes.get(layer.linetype)
            .map(|l| l.name.clone()).unwrap_or_else(|| "Continuous".into());
        pair(s, 6, &lt_name);
        pair_i(s, 370, -3);           // lineweight = default
        // 390 = hard-pointer to this layer's plot-style name object. AutoCAD's
        // AC1015 LAYER reader REQUIRES it ("Did not receive PlotStyleName") — it
        // points at the single Normal placeholder in the OBJECTS section.
        pair(s, 390, &obj.placeholder);
    }
    pair(s, 0, "ENDTAB");

    // ---- STYLE table: STANDARD + one record per unique TEXT font, so AutoCAD
    //      renders the chosen TrueType family (font in DXF comes from the TEXT's
    //      style, code 7 → STYLE → typeface). ----
    let mut fonts: Vec<String> = Vec::new();
    for d in &doc.dobjects {
        if let Geom::Text(t) = &d.geom {
            if let Some(f) = effective_text_font(t, doc) {
                if !fonts.iter().any(|x| x.eq_ignore_ascii_case(&f)) {
                    fonts.push(f);
                }
            }
        }
    }
    let st_tbl = begin_table(s, h, "STYLE", (1 + fonts.len()) as i32);
    begin_record(s, h, "STYLE", &st_tbl, "AcDbTextStyleTableRecord", 5);
    pair(s, 2, "STANDARD"); pair_i(s, 70, 0);
    pair_f(s, 40, 0.0); pair_f(s, 41, 1.0); pair_f(s, 50, 0.0);
    pair_i(s, 71, 0); pair_f(s, 42, 2.5);
    pair(s, 3, "txt"); pair(s, 4, "");
    for f in &fonts {
        begin_record(s, h, "STYLE", &st_tbl, "AcDbTextStyleTableRecord", 5);
        pair(s, 2, f); pair_i(s, 70, 0);
        pair_f(s, 40, 0.0); pair_f(s, 41, 1.0); pair_f(s, 50, 0.0);
        pair_i(s, 71, 0); pair_f(s, 42, 2.5);
        // TrueType: empty SHX file (3/4) + the typeface via the ACAD XDATA.
        pair(s, 3, ""); pair(s, 4, "");
        pair(s, 1001, "ACAD");
        pair(s, 1000, f);
        pair_i(s, 1071, 34);
    }
    pair(s, 0, "ENDTAB");

    // ---- VIEW / UCS (empty, but the tables must be present) ----
    begin_table(s, h, "VIEW", 0); pair(s, 0, "ENDTAB");
    begin_table(s, h, "UCS",  0); pair(s, 0, "ENDTAB");

    // ---- APPID (ACAD + our XDATA app for per-entity text specs) ----
    let ap_tbl = begin_table(s, h, "APPID", 2);
    begin_record(s, h, "APPID", &ap_tbl, "AcDbRegAppTableRecord", 5);
    pair(s, 2, "ACAD"); pair_i(s, 70, 0);
    begin_record(s, h, "APPID", &ap_tbl, "AcDbRegAppTableRecord", 5);
    pair(s, 2, "AutoRASM"); pair_i(s, 70, 0);
    pair(s, 0, "ENDTAB");

    // ---- DIMSTYLE (STANDARD stub — record handle is code 105, not 5) ----
    let dt_tbl = begin_table(s, h, "DIMSTYLE", 1);
    pair(s, 100, "AcDbDimStyleTable"); pair_i(s, 71, 0);   // R2000 DIMSTYLE quirk
    begin_record(s, h, "DIMSTYLE", &dt_tbl, "AcDbDimStyleTableRecord", 105);
    pair(s, 2, "STANDARD"); pair_i(s, 70, 0);
    pair(s, 0, "ENDTAB");

    // ---- BLOCK_RECORD (*Model_Space, *Paper_Space, + one per real block) ----
    let real_names: Vec<String> = doc.blocks.blocks.iter()
        .map(|b| b.name.clone())
        .filter(|n| is_real_block(n))
        .collect();
    let br_tbl = begin_table(s, h, "BLOCK_RECORD", 2 + real_names.len() as i32);
    let model_space = begin_record(s, h, "BLOCK_RECORD", &br_tbl, "AcDbBlockTableRecord", 5);
    pair(s, 2, "*Model_Space"); pair_i(s, 70, 0);
    let paper_space = begin_record(s, h, "BLOCK_RECORD", &br_tbl, "AcDbBlockTableRecord", 5);
    pair(s, 2, "*Paper_Space"); pair_i(s, 70, 0);
    let mut real = Vec::with_capacity(real_names.len());
    for name in &real_names {
        let rh = begin_record(s, h, "BLOCK_RECORD", &br_tbl, "AcDbBlockTableRecord", 5);
        pair(s, 2, name); pair_i(s, 70, 0);
        real.push((name.clone(), rh));
    }
    pair(s, 0, "ENDTAB");

    pair(s, 0, "ENDSEC");

    BlockRecords { model_space, paper_space, real }
}

/// BLOCKS section: one `BLOCK…ENDBLK` per definition, so `read_blocks` can
/// rebuild `doc.blocks` and INSERTs in ENTITIES resolve by name. Each block's
/// contained dobjects go through `write_entity` (a nested `BlockRef` therefore
/// emits a nested INSERT recursively). Gated by `is_real_block` — the SAME
/// predicate the reader uses to drop pseudo-blocks — so we never emit a block
/// the reader would silently discard (write/read symmetry).
fn write_blocks(s: &mut String, doc: &Document, h: &mut HandleGen, brt: &BlockRecords) {
    pair(s, 0, "SECTION");
    pair(s, 2, "BLOCKS");

    // Mandatory layout blocks (empty — their entities live in ENTITIES).
    write_block_shell(s, h, "*Model_Space", Vec2::new(0.0, 0.0), &brt.model_space);
    write_block_shell(s, h, "*Paper_Space", Vec2::new(0.0, 0.0), &brt.paper_space);

    for blk in &doc.blocks.blocks {
        if !is_real_block(&blk.name) { continue; }
        let owner = brt.owner_for(&blk.name);
        write_block_begin(s, h, &blk.name, blk.base, owner);
        for cd in &blk.dobjects {
            write_entity(s, cd, doc, h, owner);   // owned by THIS block's record
        }
        write_block_end(s, h, owner);
    }
    pair(s, 0, "ENDSEC");
}

/// Empty `BLOCK…ENDBLK` shell (used for the *Model_Space / *Paper_Space blocks).
fn write_block_shell(s: &mut String, h: &mut HandleGen, name: &str, base: Vec2, owner: &str) {
    write_block_begin(s, h, name, base, owner);
    write_block_end(s, h, owner);
}

fn write_block_begin(s: &mut String, h: &mut HandleGen, name: &str, base: Vec2, owner: &str) {
    pair(s, 0, "BLOCK");
    pair(s, 5, &h.alloc());
    pair(s, 330, owner);
    pair(s, 100, "AcDbEntity");
    pair(s, 8, "0");
    pair(s, 100, "AcDbBlockBegin");
    pair(s, 2, name);
    pair_i(s, 70, 0);                 // block-type flags: 0 = plain (non-anon/xref)
    pair_f(s, 10, base.x); pair_f(s, 20, base.y); pair_f(s, 30, 0.0);
    pair(s, 3, name);
    pair(s, 1, "");                   // xref path (none)
}

fn write_block_end(s: &mut String, h: &mut HandleGen, owner: &str) {
    pair(s, 0, "ENDBLK");
    pair(s, 5, &h.alloc());
    pair(s, 330, owner);
    pair(s, 100, "AcDbEntity");
    pair(s, 8, "0");
    pair(s, 100, "AcDbBlockEnd");
}

fn write_entities(s: &mut String, doc: &Document, h: &mut HandleGen, brt: &BlockRecords) {
    pair(s, 0, "SECTION");
    pair(s, 2, "ENTITIES");

    for d in &doc.dobjects {
        write_entity(s, d, doc, h, &brt.model_space);   // owned by *Model_Space
    }

    pair(s, 0, "ENDSEC");
}

fn write_entity(s: &mut String, d: &DObject, doc: &Document, h: &mut HandleGen, owner: &str) {
    let layer_name = doc.layers.get(d.style.layer)
        .map(|l| l.name.clone()).unwrap_or_else(|| "0".into());
    let linetype_name = doc.linetypes.get(d.style.linetype)
        .map(|l| l.name.clone()).unwrap_or_else(|| "Continuous".into());
    // Entity preamble: 0/type, 5/handle, 330/owner, 100 AcDbEntity, then the
    // common entity fields (layer / linetype / color / visibility). Each arm
    // below emits its own `100 AcDb<Subclass>` marker + geometry after this.
    let common = |s: &mut String, h: &mut HandleGen, kind: &str| {
        pair(s, 0, kind);
        pair(s, 5, &h.alloc());
        pair(s, 330, owner);
        pair(s, 100, "AcDbEntity");
        pair(s, 8, &layer_name);
        pair(s, 6, &linetype_name);
        if (d.style.linetype_scale - 1.0).abs() > 1e-6 {
            pair_f(s, 48, d.style.linetype_scale as f64);
        }
        match d.style.color {
            Color::Aci(i) => pair_i(s, 62, i as i32),
            Color::ByBlock => pair_i(s, 62, 0),
            // TrueColor → 62 = -1 (ACI "by layer") + real 420 RGB; the
            // ACI stays -1 so the entity still resolves its layer color
            // when a viewer ignores 420.
            Color::TrueColorRef(idx) => {
                pair_i(s, 62, -1);
                if let Some((r, g, b)) =
                    Color::TrueColorRef(idx).rgb_bytes(&doc.truecolors)
                {
                    let packed = (r as u32) << 16 | (g as u32) << 8 | b as u32;
                    pair_i(s, 420, packed as i32);
                }
            }
            Color::ByLayer => pair_i(s, 62, 256),
        }
        if !d.style.visible { pair_i(s, 60, 1); }
    };

    match &d.geom {
        Geom::Line(l) => {
            common(s, h, "LINE");
            pair(s, 100, "AcDbLine");
            pair_f(s, 10, l.a.x); pair_f(s, 20, l.a.y); pair_f(s, 30, 0.0);
            pair_f(s, 11, l.b.x); pair_f(s, 21, l.b.y); pair_f(s, 31, 0.0);
        }
        Geom::Circle(c) => {
            common(s, h, "CIRCLE");
            pair(s, 100, "AcDbCircle");
            pair_f(s, 10, c.center.x); pair_f(s, 20, c.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 40, c.radius);
        }
        Geom::Arc(a) => {
            common(s, h, "ARC");
            // R2000 ARC = AcDbCircle (center+radius) THEN AcDbArc (angles).
            pair(s, 100, "AcDbCircle");
            pair_f(s, 10, a.center.x); pair_f(s, 20, a.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 40, a.radius);
            pair(s, 100, "AcDbArc");
            pair_f(s, 50, a.start_angle.to_degrees());
            pair_f(s, 51, (a.start_angle + a.sweep_angle).to_degrees());
        }
        Geom::Ellipse(el) => {
            common(s, h, "ELLIPSE");
            pair(s, 100, "AcDbEllipse");
            pair_f(s, 10, el.center.x); pair_f(s, 20, el.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 11, el.major.x);  pair_f(s, 21, el.major.y);  pair_f(s, 31, 0.0);
            pair_f(s, 40, el.ratio);
            pair_f(s, 41, 0.0);
            pair_f(s, 42, std::f64::consts::TAU);
        }
        Geom::EllipseArc(ea) => {
            common(s, h, "ELLIPSE");
            pair(s, 100, "AcDbEllipse");
            pair_f(s, 10, ea.ellipse.center.x); pair_f(s, 20, ea.ellipse.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 11, ea.ellipse.major.x);  pair_f(s, 21, ea.ellipse.major.y);  pair_f(s, 31, 0.0);
            pair_f(s, 40, ea.ellipse.ratio);
            pair_f(s, 41, ea.start_param);
            pair_f(s, 42, ea.start_param + ea.sweep_param);
        }
        Geom::Point(pt) => {
            common(s, h, "POINT");
            pair(s, 100, "AcDbPoint");
            pair_f(s, 10, pt.location.x); pair_f(s, 20, pt.location.y); pair_f(s, 30, 0.0);
        }
        Geom::Polyline(p) => {
            common(s, h, "LWPOLYLINE");
            pair(s, 100, "AcDbPolyline");
            pair_i(s, 90, p.vertices.len() as i32);
            pair_i(s, 70, if p.closed { 1 } else { 0 });
            for (i, v) in p.vertices.iter().enumerate() {
                pair_f(s, 10, v.pos.x);
                pair_f(s, 20, v.pos.y);
                // Per-vertex start/end width (DXF 40/41) = the width of the
                // segment beginning at this vertex. Only emit when non-zero.
                if let Some(&(sw, ew)) = p.widths.get(i) {
                    if sw.abs() > 1e-12 || ew.abs() > 1e-12 {
                        pair_f(s, 40, sw);
                        pair_f(s, 41, ew);
                    }
                }
                if v.bulge.abs() > 1e-12 {
                    pair_f(s, 42, v.bulge);
                }
            }
        }
        // DXF HATCH — SOLID fill v1 (D2). The kernel resolves the hatch's
        // boundary handles into world-space vertex loops (shared with the app
        // renderer, so fill matches on screen and on export); each loop is
        // written as a POLYLINE boundary path. Named/line patterns and a HATCH
        // *reader* are deferred (write-only). An empty resolution → write
        // nothing (as before), so a hatch whose boundaries were deleted just
        // vanishes rather than emitting a degenerate entity.
        Geom::Hatch(hatch) => {
            let loops = cad_kernel::resolve_hatch_loops(hatch, doc);
            // Drop a repeated closing vertex per loop; skip degenerate (<3) loops.
            let loops: Vec<&[cad_kernel::Vec2]> = loops.iter().map(|l| {
                let mut n = l.len();
                if n >= 2 && (l[0] - l[n - 1]).len() < 1e-9 { n -= 1; }
                &l[..n]
            }).filter(|l| l.len() >= 3).collect();
            if !loops.is_empty() {
                common(s, h, "HATCH");
                pair(s, 100, "AcDbHatch");
                pair_f(s, 10, 0.0); pair_f(s, 20, 0.0); pair_f(s, 30, 0.0);  // elevation
                pair_f(s, 210, 0.0); pair_f(s, 220, 0.0); pair_f(s, 230, 1.0); // normal +Z
                pair(s, 2, "SOLID");        // hatch pattern name
                pair_i(s, 70, 1);           // 1 = solid fill
                pair_i(s, 71, 0);           // 0 = non-associative
                pair_i(s, 91, loops.len() as i32);   // number of boundary paths
                for (li, lp) in loops.iter().enumerate() {
                    // path type flag: 2 = polyline; bit 0 (=1) marks external.
                    let ext = if li == 0 { 1 } else { 0 };
                    pair_i(s, 92, 2 | ext);
                    pair_i(s, 72, 0);       // has_bulge = 0 (already tessellated)
                    pair_i(s, 73, 1);       // is_closed = 1
                    pair_i(s, 93, lp.len() as i32);  // number of vertices
                    for v in lp.iter() {
                        pair_f(s, 10, v.x);
                        pair_f(s, 20, v.y);
                    }
                    pair_i(s, 97, 0);       // number of source boundary objects
                }
                pair_i(s, 75, 1);           // hatch style: 1 = outermost
                pair_i(s, 76, 1);           // pattern type: 1 = predefined
                pair_i(s, 98, 0);           // number of seed points
            }
        }
        // DXF SPLINE. Emit degree + control points + weights + a valid
        // clamped-uniform knot vector. The reader IGNORES the 40 knots and
        // rebuilds them (clamped-uniform), so foreign NON-uniform splines re-fit
        // to clamped-uniform on import — documented v1 interop debt. 70 flags:
        // 8 = planar, | 4 = rational (any weight ≠ 1).
        Geom::Spline(sp) => {
            common(s, h, "SPLINE");
            pair(s, 100, "AcDbSpline");
            let n = sp.control_points.len();
            let deg = sp.degree;
            let rational = sp.weights.len() == n
                && sp.weights.iter().any(|w| (w - 1.0).abs() > 1e-12);
            pair_i(s, 70, 8 | if rational { 4 } else { 0 });
            pair_i(s, 71, deg as i32);
            let knot_count = n + deg + 1;
            pair_i(s, 72, knot_count as i32);   // number of knots
            pair_i(s, 73, n as i32);            // number of control points
            pair_i(s, 74, 0);                   // number of fit points
            // Clamped-uniform knot vector normalized to [0,1]: deg+1 leading 0s,
            // deg+1 trailing 1s, interior evenly spaced.
            let interior = knot_count.saturating_sub(2 * (deg + 1)); // = n - deg - 1
            for k in 0..knot_count {
                let v = if k < deg + 1 {
                    0.0
                } else if k >= knot_count - (deg + 1) {
                    1.0
                } else {
                    (k - deg) as f64 / (interior as f64 + 1.0)
                };
                pair_f(s, 40, v);
            }
            // Weights (rational only) — one 41 per control point, in order.
            if rational {
                for w in &sp.weights { pair_f(s, 41, *w); }
            }
            // Control points, in order.
            for p in &sp.control_points {
                pair_f(s, 10, p.x);
                pair_f(s, 20, p.y);
                pair_f(s, 30, 0.0);
            }
        }
        // Wall — DXF has no native wall entity. Export the two side
        // lines as LINE entities so the geometry round-trips
        // visually. The "smart" centerline+thickness link is lost on
        // export (recoverable on re-import only via heuristics).
        Geom::Wall(w) => {
            if let (Some(l), Some(r)) = (w.left_line(), w.right_line()) {
                common(s, h, "LINE");
                pair(s, 100, "AcDbLine");
                pair_f(s, 10, l.a.x); pair_f(s, 20, l.a.y); pair_f(s, 30, 0.0);
                pair_f(s, 11, l.b.x); pair_f(s, 21, l.b.y); pair_f(s, 31, 0.0);
                common(s, h, "LINE");
                pair(s, 100, "AcDbLine");
                pair_f(s, 10, r.a.x); pair_f(s, 20, r.a.y); pair_f(s, 30, 0.0);
                pair_f(s, 11, r.b.x); pair_f(s, 21, r.b.y); pair_f(s, 31, 0.0);
            }
        }
        // DXF TEXT entity. Codes 10/20/30 = insertion point;
        // 40 = height; 1 = text string; 50 = rotation degrees;
        // 72 = HAlign (0/1/2 = Left/Center/Right); 73 not emitted
        // (vertical alignment requires the second alignment point
        // at code 11/21/31 — skip for v1, defaults to Baseline).
        Geom::Text(t) => {
            let flags = (t.bold as i32) | ((t.outline_only as i32) << 1)
                | ((t.underline as i32) << 2);
            let halign_code = match t.h_align {
                cad_kernel::TextHAlign::Left   => 0,
                cad_kernel::TextHAlign::Center => 1,
                cad_kernel::TextHAlign::Right  => 2,
            };
            // FULL MTEXT: text carrying inline \C/\H/\f/\P codes (or any
            // newline) exports as ONE real MTEXT entity with the codes as the
            // native string. Plain single-line text stays a TEXT entity
            // (byte-identical to before).
            let has_codes = t.text.contains('\\');
            let lines = t.text.split('\n').count();
            if has_codes || lines > 1 {
                common(s, h, "MTEXT");
                pair(s, 100, "AcDbMText");
                pair_f(s, 10, t.position.x);
                pair_f(s, 20, t.position.y);
                pair_f(s, 30, 0.0);
                pair_f(s, 40, t.height);
                // Normalize \n → \P (the MTEXT paragraph code). No trailing
                // \P is appended — it would round-trip as a phantom newline.
                let body = t.text.replace('\n', "\\P");
                // The '\' itself must be escaped for DXF's escaping rules:
                // AutoCAD stores "\\" for a literal backslash in the string.
                pair(s, 1, &body);
                pair_f(s, 50, t.angle.to_degrees());
                if t.underline { pair_i(s, 77, 1); }
                pair_f(s, 41, t.width_factor);
                if t.oblique.abs() > 1e-9 {
                    pair_f(s, 51, t.oblique.to_degrees());
                }
                pair_i(s, 71, halign_code);
                let attach = 1;  // top-left
                if let Some(fname) = effective_text_font(t, doc) {
                    pair(s, 7, &fname);
                }
                // AutoRASM XDATA keeps the exact per-entity render specs.
                pair(s, 1001, "AutoRASM");
                pair_i(s, 1070, flags);
                pair_f(s, 1040, t.outline_width);
                if !t.font_name.is_empty() {
                    pair(s, 1000, &t.font_name);
                }
                // Column width (63) can be zero; attach point (71) + rotation.
                let _ = attach;
                return;
            }
            // DXF TEXT is single-line. A paragraph (`text` with '\n') exports as
            // ONE TEXT per visible line, stacked down by line_spacing×height. The
            // list marker (a render PROPERTY, never stored in `text`) is applied
            // here; numbered lists auto-number. Each record gets its own handle
            // via `common`. Single-line no-list text is byte-identical to before.
            let dy = t.height * if t.line_spacing > 1e-6 { t.line_spacing } else { 1.5 };
            let raw_lines: Vec<&str> = if t.text.is_empty() {
                vec![""]
            } else {
                t.text.split('\n').collect()
            };
            let font_ref = effective_text_font(t, doc);
            let mut num = 1usize;
            let mut idx = 0usize;
            for raw in raw_lines {
                let trimmed = raw.trim_end();
                if trimmed.trim().is_empty() { continue; }
                let line_text = match t.list_mode {
                    cad_kernel::TextListKind::None     => trimmed.to_string(),
                    cad_kernel::TextListKind::Bulleted => format!("• {trimmed}"),
                    cad_kernel::TextListKind::Numbered => {
                        let x = format!("{num}. {trimmed}"); num += 1; x
                    }
                };
                let py = t.position.y - dy * (idx as f64);
                idx += 1;
                common(s, h, "TEXT");
                pair(s, 100, "AcDbText");
                pair_f(s, 10, t.position.x);
                pair_f(s, 20, py);
                pair_f(s, 30, 0.0);
                pair_f(s, 40, t.height);
                pair(s, 1, &line_text);
                if t.angle.abs() > 1e-12 {
                    pair_f(s, 50, t.angle.to_degrees());
                }
                // Standard, interoperable: 51 = oblique (italic), 41 = width.
                if t.oblique.abs() > 1e-12 {
                    pair_f(s, 51, t.oblique.to_degrees());
                }
                if (t.width_factor - 1.0).abs() > 1e-9 {
                    pair_f(s, 41, t.width_factor);
                }
                // Style reference → the font (built-ins use STANDARD).
                if let Some(font) = &font_ref {
                    pair(s, 7, font);
                }
                if halign_code != 0 {
                    pair_i(s, 72, halign_code);
                    pair_f(s, 11, t.position.x);
                    pair_f(s, 21, py);
                    pair_f(s, 31, 0.0);
                }
                // AutoCAD needs a 2nd `AcDbText` marker + code 73 before XDATA.
                pair(s, 100, "AcDbText");
                pair_i(s, 73, 0);
                // AutoRASM XDATA: 1070 flags (bit0 bold, bit1 outline, bit2
                // underline), 1040 outline width, 1000 font override.
                if flags != 0 || t.outline_width > 1e-9 || !t.font_name.is_empty() {
                    pair(s, 1001, "AutoRASM");
                    pair_i(s, 1070, flags);
                    pair_f(s, 1040, t.outline_width);
                    if !t.font_name.is_empty() {
                        pair(s, 1000, &t.font_name);
                    }
                }
            }
        }
        Geom::Dimension(d) => {
            // REAL DIMENSION entity (AutoCAD 2000+). Def points drive the
            // geometry; code 1 = "<>" (measured, live) or the override.
            // Import reconstructs the DimKind from the same codes, so our
            // own DXF round-trips losslessly.
            use cad_kernel::DimKind;
            let st = doc.dim_styles.get(d.style)
                .unwrap_or(doc.dim_styles.get(0).unwrap());
            let text_h = st.text_height * st.overall_scale;
            common(s, h, "DIMENSION");
            pair(s, 100, "AcDbDimension");
            pair(s, 2, "");                        // anonymous block: none
            let rg = d.render_geometry(st);        // text mid point
            match &d.kind {
                DimKind::Linear { p1, p2, dimline_pos, ortho } => {
                    let aligned = matches!(ortho, cad_kernel::LinearOrtho::Aligned);
                    pair_f(s, 10, dimline_pos.x); pair_f(s, 20, dimline_pos.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, rg.text_pos.x);
                    pair_f(s, 21, rg.text_pos.y);
                    pair_f(s, 31, 0.0);
                    pair_i(s, 70, if aligned { 1 } else { 0 });
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair(s, 100, if aligned {
                        "AcDbAlignedDimension"
                    } else {
                        "AcDbRotatedDimension"
                    });
                    pair_f(s, 13, p1.x); pair_f(s, 23, p1.y); pair_f(s, 33, 0.0);
                    pair_f(s, 14, p2.x); pair_f(s, 24, p2.y); pair_f(s, 34, 0.0);
                    if !aligned {
                        let angle = match ortho {
                            cad_kernel::LinearOrtho::Horizontal => 0.0,
                            cad_kernel::LinearOrtho::Vertical => 90.0,
                            cad_kernel::LinearOrtho::Aligned => 0.0, // unreachable
                        };
                        pair_f(s, 50, angle);
                    }
                }
                DimKind::Angular { vertex, p1, p2, arc_pos } => {
                    pair_f(s, 10, vertex.x); pair_f(s, 20, vertex.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, rg.text_pos.x);
                    pair_f(s, 21, rg.text_pos.y);
                    pair_f(s, 31, 0.0);
                    pair_i(s, 70, 2);
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair(s, 100, "AcDb2LineAngularDimension");
                    pair_f(s, 13, p1.x); pair_f(s, 23, p1.y); pair_f(s, 33, 0.0);
                    pair_f(s, 14, p2.x); pair_f(s, 24, p2.y); pair_f(s, 34, 0.0);
                    pair_f(s, 15, arc_pos.x); pair_f(s, 25, arc_pos.y); pair_f(s, 35, 0.0);
                }
                DimKind::ArcLen { center, radius, start_angle, sweep, leader_end } => {
                    // Arc-length (AcDbArcDimension, type 8): 10 = center,
                    // 11 = leader end, 40 = radius, 13/14 = arc start/end.
                    pair_f(s, 10, center.x); pair_f(s, 20, center.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, leader_end.x);
                    pair_f(s, 21, leader_end.y);
                    pair_f(s, 31, 0.0);
                    pair_i(s, 70, 8);
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair_f(s, 41, *radius);
                    pair(s, 100, "AcDbArcDimension");
                    let a0 = start_angle;
                    let a1 = start_angle + sweep;
                    pair_f(s, 13, center.x + a0.cos() * radius);
                    pair_f(s, 23, center.y + a0.sin() * radius);
                    pair_f(s, 33, 0.0);
                    pair_f(s, 14, center.x + a1.cos() * radius);
                    pair_f(s, 24, center.y + a1.sin() * radius);
                    pair_f(s, 34, 0.0);
                }
                DimKind::Ordinate { datum, point, leader_end, is_x } => {
                    // Ordinate (AcDbOrdinateDimension, type 6): 10 = the
                    // feature point, 11 = leader end, 13 = datum.
                    pair_f(s, 10, point.x); pair_f(s, 20, point.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, leader_end.x);
                    pair_f(s, 21, leader_end.y);
                    pair_f(s, 31, 0.0);
                    // 70 low bits = type 6 (ordinate); bit 64 = X-type.
                    pair_i(s, 70, 6 | if *is_x { 64 } else { 0 });
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair(s, 100, "AcDbOrdinateDimension");
                    pair_f(s, 13, datum.x); pair_f(s, 23, datum.y);
                    pair_f(s, 33, 0.0);
                }
                DimKind::JoggedRadius { center, on_circle, leader_end, jog_pos } => {
                    // Jogged radius (AcDbRadialDimensionLarge, type 4 with
                    // a jog): 10 = center, 11 = leader end, 15 = on-circle,
                    // 40 = radius, 71 = jog point.
                    pair_f(s, 10, center.x); pair_f(s, 20, center.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, leader_end.x);
                    pair_f(s, 21, leader_end.y);
                    pair_f(s, 31, 0.0);
                    pair_i(s, 70, 4);
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair(s, 100, "AcDbRadialDimensionLarge");
                    pair_f(s, 15, on_circle.x); pair_f(s, 25, on_circle.y);
                    pair_f(s, 35, 0.0);
                    pair_f(s, 71, jog_pos.x); pair_f(s, 21, jog_pos.y);
                }
                DimKind::Radius { center, on_circle, leader_end } => {
                    pair_f(s, 10, center.x); pair_f(s, 20, center.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, leader_end.x);
                    pair_f(s, 21, leader_end.y);
                    pair_f(s, 31, 0.0);
                    pair_i(s, 70, 4);
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair(s, 100, "AcDbRadialDimension");
                    pair_f(s, 15, on_circle.x); pair_f(s, 25, on_circle.y);
                    pair_f(s, 35, 0.0);
                }
                DimKind::Diameter { center, on_circle, leader_end } => {
                    pair_f(s, 10, center.x); pair_f(s, 20, center.y);
                    pair_f(s, 30, 0.0);
                    pair_f(s, 11, leader_end.x);
                    pair_f(s, 21, leader_end.y);
                    pair_f(s, 31, 0.0);
                    pair_i(s, 70, 3);
                    match &d.text_override {
                        Some(o) if !o.is_empty() => pair(s, 1, o),
                        _ => pair(s, 1, "<>"),
                    }
                    pair(s, 3, &st.name);
                    pair_f(s, 42, d.measured_value());
                    pair_f(s, 40, text_h.max(0.05));
                    pair(s, 100, "AcDbDiametricDimension");
                    pair_f(s, 15, on_circle.x); pair_f(s, 25, on_circle.y);
                    pair_f(s, 35, 0.0);
                }
            }
            // Old readers that ignore DIMENSION still see a TEXT label.
            pair(s, 100, "AcDbText");
            pair_i(s, 73, 0);
        }
        Geom::BlockRef(br) => {
            // INSERT — the EXACT inverse of read_dxf's INSERT decode. The
            // reader factors negative axis scales into `mirror_x` + a π
            // rotation adjustment (extra = π when sy < 0), keeping scale
            // MAGNITUDES in 41/42, so we reconstruct signed 41/42/50 here:
            //   reader:  mirror_x = (sx<0) != (sy<0);  extra = (sy<0)?π:0;
            //            scale=|sx|; scale_y=|sy|; rotation = rad(50)+extra
            //   writer:  non-mirror → sx=+scale, sy=+scale_y, rot=rotation
            //            mirror     → sx=+scale, sy=-scale_y, rot=rotation−π
            // Read back: mirror case gives sx>0, sy<0 → mirror_x=true,
            //            extra=π, rotation=(rotation−π)+π=rotation. Exact.
            // A dangling block id → skip the entity (the reader would also
            // drop an INSERT whose block name doesn't resolve).
            if let Some(blk) = doc.blocks.get(br.block) {
                common(s, h, "INSERT");
                pair(s, 100, "AcDbBlockReference");
                pair(s, 2, &blk.name);
                pair_f(s, 10, br.insert.x);
                pair_f(s, 20, br.insert.y);
                pair_f(s, 30, 0.0);
                let sx = br.scale;
                let (sy, rot) = if br.mirror_x {
                    (-br.scale_y, br.rotation - std::f64::consts::PI)
                } else {
                    ( br.scale_y, br.rotation)
                };
                // Emit 41/42/50 UNCONDITIONALLY — the reader's defaults
                // (1.0/1.0/0°) only apply when absent, so writing them
                // explicitly guarantees the exact inverse.
                pair_f(s, 41, sx);
                pair_f(s, 42, sy);
                pair_f(s, 50, rot.to_degrees());
                // v22 — attribute VALUES ride as ATTRIB entities (tag → 2,
                // value → 1, position → 10/20, height → 40, angle → 50).
                // DXF-standard: ATTRIB follows its INSERT, inheriting the
                // transform. Parallel by index to the definition's AttrDefs.
                let mut attr_i = 0usize;
                for child in &blk.dobjects {
                    if let Geom::AttrDef(ad) = &child.geom {
                        let val = br.attr_values.get(attr_i)
                            .cloned()
                            .unwrap_or_else(|| ad.default.clone());
                        let wp = br.transform_geom(
                            &Geom::AttrDef(ad.clone()), blk.base);
                        if let Geom::AttrDef(wad) = wp {
                            let (x, y) = (wad.position.x, wad.position.y);
                            common(s, h, "ATTRIB");
                            pair(s, 100, "AcDbAttribute");
                            pair(s, 2, &ad.tag);
                            pair_f(s, 10, x);
                            pair_f(s, 20, y);
                            pair_f(s, 30, 0.0);
                            pair_f(s, 40, wad.height);
                            if wad.angle.abs() > 1e-12 {
                                pair_f(s, 50, wad.angle.to_degrees());
                            }
                            pair(s, 1, &val);
                            pair_i(s, 70, if ad.visible { 0 } else { 1 });
                        }
                        attr_i += 1;
                    }
                }
            }
        }
        Geom::Leader(l) => {
            // LEADER — the chain vertices (10/20/30 per vertex). The
            // annotation is a separate TEXT entity at the label anchor
            // (DXF LEADER's annotation handle is an optional attachment).
            common(s, h, "LEADER");
            pair(s, 100, "AcDbLeader");
            pair_i(s, 71, if l.arrow { 1 } else { 0 });  // arrowhead flag
            pair_i(s, 72, 3);                            // straight-line path
            pair_i(s, 73, l.pts.len() as i32);
            pair_i(s, 74, 0);                            // no annotation hook
            pair_i(s, 75, 0);                            // no arrowhead hook
            for p in &l.pts {
                pair_f(s, 10, p.x);
                pair_f(s, 20, p.y);
                pair_f(s, 30, 0.0);
            }
            // A matching TEXT for the label (parseable by other tools).
            if !l.label.text.is_empty() {
                common(s, h, "TEXT");
                pair(s, 100, "AcDbText");
                pair_f(s, 10, l.label.position.x);
                pair_f(s, 20, l.label.position.y);
                pair_f(s, 30, 0.0);
                pair_f(s, 40, l.label.height);
                pair(s, 1, &l.label.text);
                if l.label.angle.abs() > 1e-12 {
                    pair_f(s, 50, l.label.angle.to_degrees());
                }
                pair(s, 100, "AcDbText");
                pair_i(s, 73, 0);
            }
        }
        Geom::AttrDef(a) => {
            // ATTDEF — a TEXT record carrying the tag in code 2 (AutoCAD's
            // group 2 = tag name), plus prompt (3) + default (1). Renders
            // as `<tag>` placeholder text in other CADs.
            common(s, h, "ATTDEF");
            pair(s, 100, "AcDbText");
            pair(s, 2, &a.tag);
            pair(s, 3, &a.prompt);
            pair(s, 1, &a.default);
            pair_f(s, 10, a.position.x);
            pair_f(s, 20, a.position.y);
            pair_f(s, 30, 0.0);
            pair_f(s, 40, a.height);
            if a.angle.abs() > 1e-12 {
                pair_f(s, 50, a.angle.to_degrees());
            }
            pair(s, 100, "AcDbText");
            pair_i(s, 73, 0);
        }
        Geom::CenterMark(cm) => {
            // CENTERMARK (AcDbCenterMark, R2018+): 10/20 center, 40 arm
            // size, 50 rotation (degrees). Older readers may skip it; the
            // two crossing LINEs are the fallback for maximum interop.
            common(s, h, "CENTERMARK");
            pair(s, 100, "AcDbCenterMark");
            pair_f(s, 10, cm.center.x);
            pair_f(s, 20, cm.center.y);
            pair_f(s, 30, 0.0);
            pair_f(s, 40, cm.size);
            if cm.rotation.abs() > 1e-12 {
                pair_f(s, 50, cm.rotation.to_degrees());
            }
            // Fallback arms as plain LINEs so pre-2018 readers see them.
            let [t0, t1, t2, t3] = cm.tips();
            common(s, h, "LINE");
            pair(s, 100, "AcDbLine");
            pair_f(s, 10, t0.x);  pair_f(s, 20, t0.y);  pair_f(s, 30, 0.0);
            pair_f(s, 11, t2.x);  pair_f(s, 21, t2.y);  pair_f(s, 31, 0.0);
            common(s, h, "LINE");
            pair(s, 100, "AcDbLine");
            pair_f(s, 10, t1.x);  pair_f(s, 20, t1.y);  pair_f(s, 30, 0.0);
            pair_f(s, 11, t3.x);  pair_f(s, 21, t3.y);  pair_f(s, 31, 0.0);
        }
        Geom::Xline(x) => {
            // XLINE (AcDbXline): 10/20/30 = base point, 11/21/31 =
            // direction vector (normalized). The line itself is infinite;
            // readers clip it to their own view.
            common(s, h, "XLINE");
            pair(s, 100, "AcDbXline");
            pair_f(s, 10, x.base.x);
            pair_f(s, 20, x.base.y);
            pair_f(s, 30, 0.0);
            pair_f(s, 11, x.dir.x);
            pair_f(s, 21, x.dir.y);
            pair_f(s, 31, 0.0);
        }
        Geom::Donut(d) => {
            // DONUT — exported as two closed LWPOLYLINE rings (outer + hole).
            // Import reconstructs them as plain closed polylines.
            let mut ring = |radius: f64, layer: &str| -> String {
                let mut b = String::new();
                let _ = layer;
                common(&mut b, h, "LWPOLYLINE");
                b.push_str(" 90\n 48\n 70\n 1");
                for i in 0..48 {
                    let t = std::f64::consts::TAU * (i as f64 / 48.0);
                    pair_f(&mut b, 10, d.center.x + radius * t.cos());
                    pair_f(&mut b, 20, d.center.y + radius * t.sin());
                }
                b
            };
            s.push_str(&ring(d.outer_radius, ""));
            if d.inner_radius > 1e-9 {
                s.push_str(&ring(d.inner_radius, ""));
            }
        }
        Geom::Wipeout(w) => {
            // WIPEOUT — exported as a closed LWPOLYLINE (the mask's outline;
            // AutoCAD's fill-on-top semantics are viewer-side).
            common(s, h, "LWPOLYLINE");
            let n = w.pts.len();
            pair_i(s, 90, n as i32);
            pair_i(s, 70, 1);
            for p in &w.pts {
                pair_f(s, 10, p.x);
                pair_f(s, 20, p.y);
            }
        }
        Geom::Region(rg) => {
            // REGION — exported as a closed LWPOLYLINE (the filled loop).
            common(s, h, "LWPOLYLINE");
            let n = rg.loop_pts.len();
            pair_i(s, 90, n as i32);
            pair_i(s, 70, 1);
            for p in &rg.loop_pts {
                pair_f(s, 10, p.x);
                pair_f(s, 20, p.y);
            }
        }
        Geom::Ray(r) => {
            // RAY (AcDbRay): 10/20/30 = base point, 11/21/31 = direction
            // vector (normalized). The ray is infinite forward from base.
            common(s, h, "RAY");
            pair(s, 100, "AcDbRay");
            pair_f(s, 10, r.base.x);
            pair_f(s, 20, r.base.y);
            pair_f(s, 30, 0.0);
            pair_f(s, 11, r.dir.x);
            pair_f(s, 21, r.dir.y);
            pair_f(s, 31, 0.0);
        }
        Geom::Xref(_) => {
            // XREF — external references are not exported to DXF v1 (like
            // Viewports). The referenced content lives in the external file.
        }
        Geom::Table(t) => {
            // TABLE — exported as the real grid (LINEs) + cell TEXT records
            // (a native DXF TABLE entity is out of scope; the fallback
            // round-trips as lines + text, which is visually identical).
            for (a, b) in t.grid_lines() {
                common(s, h, "LINE");
                pair(s, 100, "AcDbLine");
                pair_f(s, 10, a.x); pair_f(s, 20, a.y); pair_f(s, 30, 0.0);
                pair_f(s, 11, b.x); pair_f(s, 21, b.y); pair_f(s, 31, 0.0);
            }
            for r in 0..t.n_rows {
                for c in 0..t.n_cols {
                    if let Some(tx) = t.cell_text(r, c) {
                        common(s, h, "TEXT");
                        pair(s, 100, "AcDbText");
                        pair_f(s, 10, tx.position.x);
                        pair_f(s, 20, tx.position.y);
                        pair_f(s, 30, 0.0);
                        pair_f(s, 40, tx.height);
                        pair(s, 1, &tx.text);
                        pair_f(s, 50, tx.angle.to_degrees());
                    }
                }
            }
        }
        Geom::Viewport(_) => {
            // Viewports are paper-space entities not exported to DXF yet.
        }
    }
}

// ============================================================================
//   TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(doc: &Document) -> Document {
        let text = write_dxf(doc);
        read_dxf(&text).expect("round-trip parse")
    }

    #[test]
    fn insunits_millimetres_is_read_from_the_header() {
        let text = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
                    0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(text).expect("parse");
        assert_eq!(doc.units.metres_per_unit, 0.001);
        assert_eq!(doc.units.source, cad_kernel::UnitSource::Declared);
    }

    /// `$INSUNITS = 0` is "unitless" — an explicit absence of a claim. Reading it as metres
    /// would silently promote a non-statement into a statement.
    #[test]
    fn insunits_zero_leaves_the_unit_assumed() {
        let text = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n0\n0\nENDSEC\n\
                    0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(text).expect("parse");
        assert_eq!(doc.units.source, cad_kernel::UnitSource::Assumed);
    }

    /// A file with no HEADER at all (and every file written before units existed) must still
    /// load, unchanged, as the default (1 scene unit = 1 mm, Assumed).
    #[test]
    fn a_header_less_file_still_loads_as_assumed() {
        let text = "0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(text).expect("parse");
        assert_eq!(doc.units.source, cad_kernel::UnitSource::Assumed);
    }

    /// Export must not turn a default nobody chose into a positive claim — otherwise autosave
    /// silently stamps "this drawing is metres" on every .dxf the user has open.
    #[test]
    fn an_assumed_unit_is_exported_as_unitless() {
        let doc = Document::default();
        assert_eq!(doc.units.source, cad_kernel::UnitSource::Assumed);
        let text = write_dxf(&doc);
        assert!(text.contains("$INSUNITS"), "the variable is written");
        assert_eq!(round_trip(&doc).units.source, cad_kernel::UnitSource::Assumed);
    }

    /// A unit somebody DID state survives a full write → read cycle.
    #[test]
    fn a_declared_unit_survives_export_and_reimport() {
        for (metres, _name) in [(0.001, "mm"), (0.01, "cm"), (1.0, "m"), (0.0254, "in"), (0.3048, "ft")] {
            let mut doc = Document::default();
            doc.units = cad_kernel::Units::from_metres_per_unit(
                metres, cad_kernel::UnitSource::User);
            let back = round_trip(&doc);
            assert_eq!(back.units.metres_per_unit, metres, "{metres} m/unit round-trips");
            // It comes back DECLARED (the file said so) rather than User — the app cannot
            // know a file's unit was originally typed by a person.
            assert_eq!(back.units.source, cad_kernel::UnitSource::Declared);
        }
    }

    #[test]
    fn a_mirrored_circle_lands_on_the_other_side() {
        // A CIRCLE with extrusion (0,0,-1) is a MIRROR: object (x,y) is world (−x,y).
        let dxf = "0\nSECTION\n2\nENTITIES\n\
            0\nCIRCLE\n8\n0\n10\n5.0\n20\n3.0\n30\n0.0\n40\n2.0\n\
            210\n0.0\n220\n0.0\n230\n-1.0\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let Geom::Circle(c) = &doc.dobjects[0].geom else { panic!("circle") };
        assert_eq!(c.center.x, -5.0, "x is mirrored");
        assert_eq!(c.center.y, 3.0, "y is untouched");
    }

    #[test]
    fn a_mirrored_arc_keeps_its_own_two_ends() {
        // A mirrored ARC must keep the same two endpoints in world space — the sweep
        // reverses under the mirror so the drawn piece is the same one.
        let dxf = "0\nSECTION\n2\nENTITIES\n\
            0\nARC\n8\n0\n10\n0.0\n20\n0.0\n40\n5.0\n\
            50\n30.0\n51\n120.0\n210\n0.0\n220\n0.0\n230\n-1.0\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let Geom::Arc(a) = &doc.dobjects[0].geom else { panic!("arc") };
        // Under the mirror the endpoints are (x,y) → (−x,y) of the original 30°…120° arc.
        // The sweep REVERSES under a mirror, so the stored (CCW, positive-sweep) arc
        // necessarily STARTS at the mirrored original END and ENDS at the mirrored
        // original START — the endpoint PAIR is what the mirror preserves (asserted as
        // a set; which end is "start" swaps with the sweep).
        let arc_pt = |deg: f64| -> Vec2 {
            let t = deg.to_radians();
            a.center + Vec2::new(a.radius * t.cos(), a.radius * t.sin())
        };
        let p1 = arc_pt(a.start_angle.to_degrees());
        let p2 = arc_pt((a.start_angle + a.sweep_angle).to_degrees());
        let expect = |deg: f64| -> Vec2 {
            let t = deg.to_radians();
            Vec2::new(-5.0 * t.cos(), 5.0 * t.sin())
        };
        let e1 = expect(30.0);
        let e2 = expect(120.0);
        let same = |p: Vec2, q: Vec2| (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9;
        assert!(
            (same(p1, e1) && same(p2, e2)) || (same(p1, e2) && same(p2, e1)),
            "the mirrored arc does not span the same ground: ends {p1:?}..{p2:?}, \
             expected the pair {{{e1:?}, {e2:?}}} (either order)",
        );
        assert!(
            (a.sweep_angle.to_degrees() - 90.0).abs() < 1e-6,
            "the sweep must keep its magnitude (90°) — a different one is the complement, \
             not the arc: {}",
            a.sweep_angle.to_degrees(),
        );
    }

    #[test]
    fn a_mirrored_polyline_bows_the_other_way() {
        // A mirrored LWPOLYLINE (extrusion −Z) with a bulge flips x per vertex; the
        // bulge SIGN must also flip so the arc bows the mirrored way.
        let dxf = "0\nSECTION\n2\nENTITIES\n\
            0\nLWPOLYLINE\n8\n0\n90\n3\n70\n0\n\
            10\n1.0\n20\n0.0\n42\n1.0\n\
            10\n3.0\n20\n0.0\n42\n0.0\n\
            10\n5.0\n20\n0.0\n42\n0.0\n\
            210\n0.0\n220\n0.0\n230\n-1.0\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let Geom::Polyline(pl) = &doc.dobjects[0].geom else { panic!("polyline") };
        assert_eq!(pl.vertices[0].pos.x, -1.0, "first vertex mirrored");
        assert_eq!(pl.vertices[1].pos.x, -3.0, "second vertex mirrored");
        assert_eq!(pl.vertices[2].pos.x, -5.0, "third vertex mirrored");
    }

    #[test]
    fn a_line_with_an_extrusion_stays_exactly_where_it_is() {
        // LINE is a WORLD-coordinate entity — its 10/11 are already world; a −Z
        // extrusion must NOT move it (the reader leaves world entities alone).
        let dxf = "0\nSECTION\n2\nENTITIES\n\
            0\nLINE\n8\n0\n10\n1.0\n20\n2.0\n11\n3.0\n21\n4.0\n\
            210\n0.0\n220\n0.0\n230\n-1.0\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let Geom::Line(l) = &doc.dobjects[0].geom else { panic!("line") };
        assert_eq!(l.a, Vec2::new(1.0, 2.0));
        assert_eq!(l.b, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn a_tilted_extrusion_is_not_guessed_at() {
        // An extrusion on a tilted plane (210/220 ≠ 0) is left alone — the honest
        // transform is a projection the reader refuses to guess at.
        let dxf = "0\nSECTION\n2\nENTITIES\n\
            0\nCIRCLE\n8\n0\n10\n5.0\n20\n3.0\n40\n2.0\n\
            210\n0.5\n220\n0.0\n230\n0.866\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let Geom::Circle(c) = &doc.dobjects[0].geom else { panic!("circle") };
        assert_eq!(c.center.x, 5.0, "tilted extrusion is not mirrored");
    }

    #[test]
    fn an_explicit_plus_z_extrusion_is_the_default() {
        let dxf = "0\nSECTION\n2\nENTITIES\n\
            0\nCIRCLE\n8\n0\n10\n5.0\n20\n3.0\n40\n2.0\n\
            210\n0.0\n220\n0.0\n230\n1.0\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let Geom::Circle(c) = &doc.dobjects[0].geom else { panic!("circle") };
        assert_eq!(c.center.x, 5.0);
    }

    #[test]
    fn empty_doc_round_trip() {
        let doc = Document::default();
        let back = round_trip(&doc);
        // The writer synthesizes the AutoCAD-required default layer "0" when the
        // document lacks one (this codebase's default layer is NOT "0"), so a
        // round-trip gains exactly that layer and then stays stable.
        let had_zero = doc.layers.layers.iter().any(|l| l.name == "0");
        let expected = doc.layers.len() + if had_zero { 0 } else { 1 };
        assert_eq!(back.layers.len(), expected);
        assert!(back.layers.layers.iter().any(|l| l.name == "0"),
            "round-trip must contain the default layer \"0\"");
        assert!(back.dobjects.is_empty());
    }

    #[test]
    fn block_and_insert_are_read() {
        // BLOCKS section defines CHAIR (one line); ENTITIES has an INSERT of it.
        let dxf = "\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n2\nCHAIR\n10\n0.0\n20\n0.0\n\
0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n4.0\n21\n0.0\n\
0\nENDBLK\n\
0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n\
0\nINSERT\n2\nCHAIR\n8\n0\n10\n10.0\n20\n5.0\n41\n2.0\n50\n90.0\n\
0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        // The block definition landed in the table with its one line.
        assert_eq!(doc.blocks.blocks.len(), 1);
        let bid = doc.blocks.find("CHAIR").expect("CHAIR block");
        assert_eq!(doc.blocks.blocks[bid as usize].dobjects.len(), 1);
        // The INSERT became a BlockRef with the right transform.
        assert_eq!(doc.dobjects.len(), 1);
        match &doc.dobjects[0].geom {
            Geom::BlockRef(br) => {
                assert_eq!(br.block, bid);
                assert_eq!(br.insert, Vec2::new(10.0, 5.0));
                assert_eq!(br.scale, 2.0);
                assert!((br.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
            }
            other => panic!("expected BlockRef, got {other:?}"),
        }
    }

    #[test]
    fn degenerate_arc_is_dropped_full_circle_arc_kept() {
        // Three ARCs: start==end (degenerate → drop), 0..360 (true full circle
        // → keep as TAU), 0..90 (normal quarter → keep). Regression for the
        // "unwanted circle on block import" bug.
        let dxf = "\
0\nSECTION\n2\nENTITIES\n\
0\nARC\n8\n0\n10\n0.0\n20\n0.0\n40\n1.0\n50\n45.0\n51\n45.0\n\
0\nARC\n8\n0\n10\n5.0\n20\n0.0\n40\n1.0\n50\n0.0\n51\n360.0\n\
0\nARC\n8\n0\n10\n10.0\n20\n0.0\n40\n1.0\n50\n0.0\n51\n90.0\n\
0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let arcs: Vec<&Arc> = doc.dobjects.iter()
            .filter_map(|d| if let Geom::Arc(a) = &d.geom { Some(a) } else { None })
            .collect();
        assert_eq!(arcs.len(), 2, "degenerate start==end arc must be dropped");
        assert!(arcs.iter().any(|a| (a.sweep_angle - std::f64::consts::TAU).abs() < 1e-9),
            "0..360 arc kept as a full circle");
        assert!(arcs.iter().any(|a| (a.sweep_angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9),
            "0..90 arc kept as a quarter");
    }

    #[test]
    fn special_blocks_are_skipped() {
        // *Model_Space etc. must NOT be imported as blocks (would duplicate).
        let dxf = "\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n2\n*Model_Space\n10\n0.0\n20\n0.0\n0\nENDBLK\n\
0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        assert_eq!(doc.blocks.blocks.len(), 0);
    }

    #[test]
    fn anonymous_blocks_are_kept_and_nested_insert_resolves() {
        // *U1 is an anonymous block holding real geometry; FRAME nests an
        // INSERT of it. Both must import and the nested ref must resolve.
        let dxf = "\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n2\n*U1\n10\n0.0\n20\n0.0\n\
0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n1.0\n21\n0.0\n0\nENDBLK\n\
0\nBLOCK\n2\nFRAME\n10\n0.0\n20\n0.0\n\
0\nINSERT\n2\n*U1\n10\n5.0\n20\n0.0\n0\nENDBLK\n\
0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n0\nINSERT\n2\nFRAME\n10\n0.0\n20\n0.0\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        // both *U1 and FRAME imported
        assert_eq!(doc.blocks.blocks.len(), 2);
        let frame = doc.blocks.find("FRAME").expect("FRAME");
        // FRAME's single contained entity is a BlockRef resolving to *U1
        let inner = &doc.blocks.blocks[frame as usize].dobjects;
        assert_eq!(inner.len(), 1);
        match &inner[0].geom {
            Geom::BlockRef(br) => assert_eq!(br.block, doc.blocks.find("*U1").unwrap()),
            other => panic!("expected nested BlockRef, got {other:?}"),
        }
    }

    #[test]
    fn forward_referenced_nested_block_resolves() {
        // Block A (defined FIRST) inserts B, which is defined AFTER it.
        // Two-pass reading must still resolve A's nested insert to B.
        let dxf = "\
0\nSECTION\n2\nBLOCKS\n\
0\nBLOCK\n2\nA\n10\n0.0\n20\n0.0\n\
0\nINSERT\n2\nB\n10\n0.0\n20\n0.0\n0\nENDBLK\n\
0\nBLOCK\n2\nB\n10\n0.0\n20\n0.0\n\
0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n1.0\n21\n0.0\n0\nENDBLK\n\
0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let a = doc.blocks.find("A").expect("A");
        let inner = &doc.blocks.blocks[a as usize].dobjects;
        assert_eq!(inner.len(), 1, "A's forward INSERT of B should resolve");
        match &inner[0].geom {
            Geom::BlockRef(br) => assert_eq!(br.block, doc.blocks.find("B").unwrap()),
            other => panic!("expected BlockRef to B, got {other:?}"),
        }
    }

    // ---- BLOCK / INSERT write→read round-trip (the writer is the inverse of
    //      the reader's INSERT decode; these pin the mirror/rotation math) ----

    /// A CHAIR block (one line (0,0)→(4,0), base at origin) plus its id.
    fn doc_with_chair() -> (Document, u32) {
        let mut doc = Document::default();
        let line = DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 0.0), b: Vec2::new(4.0, 0.0),
        }));
        let bid = doc.blocks.add(Block {
            name: "CHAIR".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![line],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        (doc, bid)
    }

    fn only_blockref(doc: &Document) -> &BlockRef {
        match &doc.dobjects[0].geom {
            Geom::BlockRef(br) => br,
            other => panic!("expected BlockRef, got {other:?}"),
        }
    }

    #[test]
    fn block_insert_round_trip_preserves_ref() {
        // A plain (non-mirrored) instance: rotation 30°, uniform scale 2.0.
        let (mut doc, bid) = doc_with_chair();
        let rot = 30.0_f64.to_radians();
        doc.push(DObject::new(Geom::BlockRef(BlockRef {
            block: bid,
            insert: Vec2::new(10.0, 5.0),
            scale: 2.0, scale_y: 2.0,
            rotation: rot,
            mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            })));
        let back = round_trip(&doc);

        // Block definition survives (identity NOT exploded away).
        assert_eq!(back.blocks.blocks.len(), 1, "CHAIR block must round-trip");
        let bid2 = back.blocks.find("CHAIR").expect("CHAIR");
        assert_eq!(back.blocks.blocks[bid2 as usize].dobjects.len(), 1,
            "block's line must survive");
        // The instance stayed a single BlockRef (not exploded into a line).
        assert_eq!(back.dobjects.len(), 1, "instance must stay ONE BlockRef");
        let br = only_blockref(&back);
        assert_eq!(br.block, bid2);
        assert_eq!(br.insert, Vec2::new(10.0, 5.0));
        assert!((br.scale   - 2.0).abs() < 1e-9);
        assert!((br.scale_y - 2.0).abs() < 1e-9);
        assert!((br.rotation - rot).abs() < 1e-9, "rotation must round-trip");
        assert!(!br.mirror_x, "non-mirrored must stay non-mirrored");
    }

    #[test]
    fn mirrored_block_insert_round_trip() {
        // The MIRROR case — catches a wrong inverse (sign/π-rotation handling).
        let (mut doc, bid) = doc_with_chair();
        let rot = 40.0_f64.to_radians();
        doc.push(DObject::new(Geom::BlockRef(BlockRef {
            block: bid,
            insert: Vec2::new(-3.0, 7.0),
            scale: 1.5, scale_y: 1.5,
            rotation: rot,
            mirror_x: true,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            })));
        let back = round_trip(&doc);

        assert_eq!(back.dobjects.len(), 1, "mirrored instance must stay ONE BlockRef");
        let br = only_blockref(&back);
        assert_eq!(br.block, back.blocks.find("CHAIR").unwrap());
        assert_eq!(br.insert, Vec2::new(-3.0, 7.0));
        assert!((br.scale   - 1.5).abs() < 1e-9);
        assert!((br.scale_y - 1.5).abs() < 1e-9);
        assert!(br.mirror_x, "mirror flag must round-trip");
        assert!((br.rotation - rot).abs() < 1e-9,
            "mirrored rotation must round-trip exactly (writer −π ↔ reader +π)");
    }

    #[test]
    fn nested_block_insert_round_trip() {
        // FRAME contains a BlockRef of CHAIR → proves recursive INSERT emission
        // inside the BLOCKS section (a nested INSERT written for a block's own
        // contained BlockRef) round-trips.
        let (mut doc, chair) = doc_with_chair();
        let nested = DObject::new(Geom::BlockRef(BlockRef {
            block: chair,
            insert: Vec2::new(1.0, 0.0),
            scale: 1.0, scale_y: 1.0,
            rotation: 0.0,
            mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            }));
        let frame = doc.blocks.add(Block {
            name: "FRAME".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![nested],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        doc.push(DObject::new(Geom::BlockRef(BlockRef {
            block: frame,
            insert: Vec2::new(0.0, 0.0),
            scale: 1.0, scale_y: 1.0,
            rotation: 0.0,
            mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            })));
        let back = round_trip(&doc);

        assert_eq!(back.blocks.blocks.len(), 2, "CHAIR + FRAME must both round-trip");
        let frame2 = back.blocks.find("FRAME").expect("FRAME");
        let chair2 = back.blocks.find("CHAIR").expect("CHAIR");
        let inner = &back.blocks.blocks[frame2 as usize].dobjects;
        assert_eq!(inner.len(), 1, "FRAME's nested BlockRef must survive");
        match &inner[0].geom {
            Geom::BlockRef(br) => assert_eq!(br.block, chair2,
                "nested ref must resolve back to CHAIR"),
            other => panic!("expected nested BlockRef, got {other:?}"),
        }
    }

    // AC1015 openability structure: the writer must emit the tables / handles /
    // subclass markers AutoCAD's DXFIN needs. We can't run AutoCAD in CI, so this
    // asserts the STRUCTURE is present (not that AutoCAD accepts it) and that our
    // own reader still round-trips the enriched file.
    #[test]
    fn dim_ext_kinds_dxf_round_trip() {
        // ArcLen (type 8) + Ordinate (type 6) survive a DXF round trip.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::ArcLen {
                center: Vec2::new(2.0, 1.0), radius: 4.0,
                start_angle: 0.0, sweep: std::f64::consts::FRAC_PI_2,
                leader_end: Vec2::new(8.0, 5.0),
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
        let back = round_trip(&doc);
        let Geom::Dimension(a) = &back.dobjects[0].geom else { panic!("arc-len lost") };
        let cad_kernel::DimKind::ArcLen { radius, sweep, .. } = a.kind else { panic!() };
        assert!((radius - 4.0).abs() < 1e-9);
        assert!((sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        let Geom::Dimension(o) = &back.dobjects[1].geom else { panic!("ordinate lost") };
        let cad_kernel::DimKind::Ordinate { is_x, .. } = o.kind else { panic!("kind mismatch") };
        assert!(is_x);
    }

    #[test]
    fn truecolor_and_ltscale_round_trip() {
        let mut doc = Document::default();
        let rgb = doc.truecolors.intern(0x12_34_56);
        let mut d = DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0), b: Vec2::new(5.0, 0.0) }));
        d.style.color = Color::TrueColorRef(rgb);
        d.style.linetype_scale = 2.5;
        doc.push(d);
        let back = round_trip(&doc);
        let b = &back.dobjects[0];
        let Color::TrueColorRef(idx) = b.style.color else {
            panic!("truecolor lost: {:?}", b.style.color);
        };
        assert_eq!(back.truecolors.get(idx), Some(0x12_34_56));
        assert!((b.style.linetype_scale - 2.5).abs() < 1e-5,
            "ltscale survived: {}", b.style.linetype_scale);
    }

    #[test]
    fn ray_dxf_round_trip() {
        // RAY (AcDbRay) — base 10/20, dir 11/21.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Ray(cad_kernel::Ray::new(
            Vec2::new(4.0, -1.0),
            Vec2::new(0.0, 2.0),
        ))));
        let back = round_trip(&doc);
        let Geom::Ray(r) = &back.dobjects[0].geom else { panic!("ray lost") };
        assert_eq!((r.base.x, r.base.y), (4.0, -1.0));
        assert!((r.dir - Vec2::new(0.0, 1.0)).len() < 1e-9, "dir normalized");
    }

    #[test]
    fn ac1015_openability_structure_smoke() {
        let (mut doc, bid) = doc_with_chair();
        doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 0.0), b: Vec2::new(1.0, 1.0),
        })));
        doc.push(DObject::new(Geom::BlockRef(BlockRef {
            block: bid, insert: Vec2::new(2.0, 2.0),
            scale: 1.0, scale_y: 1.0, rotation: 0.0, mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            })));
        let dxf = write_dxf(&doc);

        for needle in [
            "\n$HANDSEED\n", "\nAPPID\n", "\nACAD\n", "\nBLOCK_RECORD\n",
            "\n*Model_Space\n", "\n*Paper_Space\n", "\nVPORT\n", "\n*Active\n",
            "\nDIMSTYLE\n", "\nSTANDARD\n",
            "\nAcDbEntity\n", "\nAcDbSymbolTableRecord\n",
            "\nAcDbLine\n", "\nAcDbBlockReference\n",
            // OBJECTS graph so LAYER 390 plot-style pointers resolve.
            "\nOBJECTS\n", "\nACAD_PLOTSTYLENAME\n", "\nACDBPLACEHOLDER\n",
            "\n390\n",
        ] {
            assert!(dxf.contains(needle), "written DXF missing {needle:?}");
        }
        // AutoCAD requires the default layer "0"; the default doc has no such
        // layer, so the writer must synthesize it.
        assert!(dxf.contains("AcDbLayerTableRecord\n2\n0\n"),
            "LAYER table must contain the default layer \"0\"");
        // A hex handle (group code 5) is present somewhere.
        assert!(dxf.contains("\n5\n"), "no group-5 handle present");
        // Section order must be HEADER < TABLES < BLOCKS < ENTITIES.
        let (hh, tt, bb, ee) = (
            dxf.find("\nHEADER\n").unwrap(),
            dxf.find("\nTABLES\n").unwrap(),
            dxf.find("\nBLOCKS\n").unwrap(),
            dxf.find("\nENTITIES\n").unwrap(),
        );
        assert!(hh < tt && tt < bb && bb < ee,
            "section order must be HEADER < TABLES < BLOCKS < ENTITIES");

        // The enriched file still round-trips through our own reader.
        let back = read_dxf(&dxf).expect("re-read enriched DXF");
        assert_eq!(back.blocks.blocks.len(), 1, "CHAIR must survive");
        assert!(back.dobjects.iter().any(|d| matches!(d.geom, Geom::BlockRef(_))),
            "the INSERT must survive as a BlockRef");
        assert!(back.dobjects.iter().any(|d| matches!(d.geom, Geom::Line(_))),
            "the model-space LINE must survive");
    }

    // D2: SOLID HATCH is written (write-only — the reader has no HATCH arm yet,
    // by design, so these assert the emitted group codes, not a round-trip).
    fn closed_rect_poly(x0: f64, y0: f64, x1: f64, y1: f64) -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(x0, y0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(x1, y0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(x1, y1), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(x0, y1), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        }
    }

    #[test]
    fn hatch_solid_writes_boundary_path() {
        let mut doc = Document::default();
        let rect = DObject::new(Geom::Polyline(closed_rect_poly(0.0, 0.0, 4.0, 3.0)));
        let handle = rect.handle;
        doc.push(rect);
        doc.push(DObject::new(Geom::Hatch(cad_kernel::Hatch {
            boundary_handles: vec![handle],
            pattern: cad_kernel::HatchPattern::Solid,
        })));
        let dxf = write_dxf(&doc);
        assert!(dxf.contains("\nHATCH\n"), "HATCH entity emitted");
        assert!(dxf.contains("\nAcDbHatch\n"), "AcDbHatch subclass marker");
        assert!(dxf.contains("\n2\nSOLID\n"), "SOLID pattern name");
        assert!(dxf.contains("\n91\n1\n"), "exactly one boundary loop");
        // 4-corner rect → duplicate close dropped → 93 = 4 boundary vertices.
        assert!(dxf.contains("\n93\n4\n"), "boundary loop has 4 vertices");
        // A boundary vertex (4,3) is present.
        assert!(dxf.contains("\n10\n4\n20\n3\n"), "boundary vertex (4,3) written");
    }

    #[test]
    fn hatch_two_loops_outer_and_hole() {
        let mut doc = Document::default();
        let outer = DObject::new(Geom::Polyline(closed_rect_poly(0.0, 0.0, 10.0, 10.0)));
        let outer_h = outer.handle;
        doc.push(outer);
        let hole = DObject::new(Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 2.0 }));
        let hole_h = hole.handle;
        doc.push(hole);
        doc.push(DObject::new(Geom::Hatch(cad_kernel::Hatch {
            boundary_handles: vec![outer_h, hole_h],
            pattern: cad_kernel::HatchPattern::Solid,
        })));
        let dxf = write_dxf(&doc);
        assert!(dxf.contains("\n91\n2\n"), "two boundary loops (outer + hole)");
        assert!(dxf.contains("\n93\n64\n"), "circle hole loop has 64 vertices");
    }

    #[test]
    fn hatch_with_no_resolvable_boundary_writes_nothing() {
        let mut doc = Document::default();
        // Hatch referencing a handle that isn't in the doc → resolves empty.
        doc.push(DObject::new(Geom::Hatch(cad_kernel::Hatch {
            boundary_handles: vec![u64::MAX],
            pattern: cad_kernel::HatchPattern::Solid,
        })));
        let dxf = write_dxf(&doc);
        assert!(!dxf.contains("\nHATCH\n"), "empty hatch emits no entity");
    }

    #[test]
    fn hatch_round_trip_rebuilds_boundary_loops() {
        // The app's own hatch file must re-open with the fill intact: the
        // HATCH entity carries its loop vertices, and the reader rebuilds
        // each loop as a closed boundary polyline + handle reference.
        let mut doc = Document::default();
        let rect = DObject::new(Geom::Polyline(closed_rect_poly(0.0, 0.0, 4.0, 3.0)));
        let handle = rect.handle;
        doc.push(rect);
        doc.push(DObject::new(Geom::Hatch(cad_kernel::Hatch {
            boundary_handles: vec![handle],
            pattern: cad_kernel::HatchPattern::Solid,
        })));

        let back = round_trip(&doc);

        // Boundary polyline + hatch (the original boundary polyline is also
        // written as its own LWPOLYLINE entity, so the reader re-imports BOTH).
        let hatch = back.dobjects.iter().find_map(|d| match &d.geom {
            Geom::Hatch(h) => Some(h), _ => None })
            .expect("hatch must survive the round-trip");
        assert_eq!(hatch.boundary_handles.len(), 1,
            "one boundary loop re-linked by handle");
        let loops = cad_kernel::resolve_hatch_loops(hatch, &back);
        assert_eq!(loops.len(), 1, "boundary resolves to one loop");
        assert_eq!(loops[0].len(), 5, "4-corner rect → 4 corners + duplicated close");
        // The synthetic boundary dobject is a CLOSED polyline with the loop's
        // 4 vertices, and the hatch references its handle.
        let bd = back.dobjects.iter().find(|d| d.handle == hatch.boundary_handles[0])
            .expect("synthetic boundary dobject present");
        match &bd.geom {
            Geom::Polyline(p) => {
                assert!(p.closed);
                assert_eq!(p.vertices.len(), 4);
                assert_eq!(p.vertices[0].pos, Vec2::new(0.0, 0.0));
                assert_eq!(p.vertices[3].pos, Vec2::new(0.0, 3.0));
            }
            _ => panic!("boundary must be a polyline"),
        }
    }

    #[test]
    fn hatch_round_trip_outer_and_hole() {
        let mut doc = Document::default();
        let outer = DObject::new(Geom::Polyline(closed_rect_poly(0.0, 0.0, 10.0, 10.0)));
        let outer_h = outer.handle;
        doc.push(outer);
        let hole = DObject::new(Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 2.0 }));
        let hole_h = hole.handle;
        doc.push(hole);
        doc.push(DObject::new(Geom::Hatch(cad_kernel::Hatch {
            boundary_handles: vec![outer_h, hole_h],
            pattern: cad_kernel::HatchPattern::Solid,
        })));

        let back = round_trip(&doc);
        let hatch = back.dobjects.iter().find_map(|d| match &d.geom {
            Geom::Hatch(h) => Some(h), _ => None })
            .expect("hatch must survive");
        let loops = cad_kernel::resolve_hatch_loops(hatch, &back);
        assert_eq!(loops.len(), 2, "outer + hole both re-linked");
    }

    #[test]
    fn hatch_reader_parses_named_pattern_and_bulged_paths() {
        // Foreign DXF with a NAMED pattern (ANSI31, scale 2, angle 30) and a
        // bulged polyline path — the reader must keep the pattern identity and
        // carry the bulge into the synthetic boundary polyline.
        let dxf = "\
0\nSECTION\n2\nENTITIES\n\
0\nHATCH\n8\nWALLS\n62\n3\n2\nANSI31\n\
10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n\
70\n0\n71\n0\n91\n1\n\
92\n3\n72\n1\n73\n1\n93\n4\n\
10\n0\n20\n0\n42\n0\n\
10\n10\n20\n0\n42\n1\n\
10\n10\n20\n10\n42\n0\n\
10\n0\n20\n10\n42\n0\n\
97\n0\n75\n0\n76\n1\n41\n2\n52\n30\n98\n0\n\
0\nENDSEC\n0\nEOF\n";
        let (doc, skipped) = read_dxf_with_stats(dxf).expect("parse");
        assert_eq!(skipped, 0);
        let hatch = doc.dobjects.iter().find_map(|d| match &d.geom {
            Geom::Hatch(h) => Some(h), _ => None })
            .expect("hatch parsed");
        match &hatch.pattern {
            cad_kernel::HatchPattern::Pattern { name, scale, angle_deg } => {
                assert_eq!(name, "ANSI31");
                assert!((scale - 2.0).abs() < 1e-9);
                assert!((angle_deg - 30.0).abs() < 1e-9);
            }
            _ => panic!("named pattern must not collapse to SOLID"),
        }
        assert_eq!(hatch.boundary_handles.len(), 1);
        let bd = doc.dobjects.iter().find(|d| d.handle == hatch.boundary_handles[0])
            .expect("boundary dobject");
        match &bd.geom {
            Geom::Polyline(p) => {
                assert_eq!(p.vertices.len(), 4);
                assert!((p.vertices[1].bulge - 1.0).abs() < 1e-9,
                    "bulge must survive into the boundary polyline");
                assert_eq!(p.vertices[0].pos, Vec2::new(0.0, 0.0));
            }
            _ => panic!("boundary must be a polyline"),
        }
    }

    #[test]
    fn unsupported_entities_are_counted_as_skipped() {
        let dxf = "\
0\nSECTION\n2\nENTITIES\n\
0\nDIMENSION\n8\nA\n1\n12\n\
0\nMTEXT\n8\nA\n1\nhello\n\
0\nLINE\n8\n0\n10\n0\n20\n0\n11\n1\n21\n1\n\
0\nENDSEC\n0\nEOF\n";
        let (doc, skipped) = read_dxf_with_stats(dxf).expect("parse");
        assert_eq!(skipped, 2, "DIMENSION + MTEXT counted, LINE imported");
        assert_eq!(doc.dobjects.len(), 1);
        // Plain read_dxf (no stats) still imports the same content.
        let doc2 = read_dxf(dxf).expect("parse");
        assert_eq!(doc2.dobjects.len(), 1);
    }

    #[test]
    fn line_round_trip() {
        let mut doc = Document::default();
        doc.push(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 5.0) }.into());
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 1);
        match &back.dobjects[0].geom {
            Geom::Line(l) => {
                assert!((l.a.x - 0.0).abs() < 1e-9);
                assert!((l.b.x - 10.0).abs() < 1e-9);
                assert!((l.b.y - 5.0).abs() < 1e-9);
            }
            _ => panic!("expected Line"),
        }
    }

    #[test]
    fn circle_round_trip() {
        let mut doc = Document::default();
        doc.push(Circle { center: Vec2::new(3.0, 4.0), radius: 7.0 }.into());
        let back = round_trip(&doc);
        if let Geom::Circle(c) = &back.dobjects[0].geom {
            assert!((c.center.x - 3.0).abs() < 1e-9);
            assert!((c.radius - 7.0).abs() < 1e-9);
        } else { panic!(); }
    }

    #[test]
    fn arc_round_trip_preserves_sweep() {
        let mut doc = Document::default();
        doc.push(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.5_f64,
            sweep_angle: 1.2_f64,
        }.into());
        let back = round_trip(&doc);
        if let Geom::Arc(a) = &back.dobjects[0].geom {
            assert!((a.start_angle - 0.5).abs() < 1e-6);
            assert!((a.sweep_angle - 1.2).abs() < 1e-6);
        } else { panic!(); }
    }

    #[test]
    fn point_round_trip() {
        let mut doc = Document::default();
        doc.push(Point { location: Vec2::new(1.0, 2.0), style: 0, size: 0.0 }.into());
        let back = round_trip(&doc);
        if let Geom::Point(p) = &back.dobjects[0].geom {
            assert!((p.location.x - 1.0).abs() < 1e-9);
        } else { panic!(); }
    }

    #[test]
    fn text_round_trip() {
        // WP1.8: TEXT must survive export→import (was dropped — no reader arm).
        // Non-zero angle + non-Left alignment exercise codes 50 and 72.
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Text(cad_kernel::Text {
            position: Vec2::new(3.0, -1.5),
            height:   0.42,
            angle:    30.0_f64.to_radians(),
            text:     "Hello DXF".into(),
            h_align:  cad_kernel::TextHAlign::Right,
            v_align:  cad_kernel::TextVAlign::Baseline,
            style:    cad_kernel::TextStyleTable::STANDARD,
            // Exercise the per-entity spec round-trip (51/41 + AutoRASM XDATA).
            oblique:       12.0_f64.to_radians(),
            width_factor:  0.85,
            bold:          true,
            outline_only:  true,
            outline_width: 0.3,
            underline:     true,
            font_name:     "Arial".into(),
            list_mode:     cad_kernel::TextListKind::None,
            line_spacing:  1.5,
        })));
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 1, "text was dropped on import");
        if let Geom::Text(t) = &back.dobjects[0].geom {
            assert_eq!(t.text, "Hello DXF");
            assert!((t.height - 0.42).abs() < 1e-9);
            assert!((t.position.x - 3.0).abs() < 1e-9);
            assert!((t.position.y + 1.5).abs() < 1e-9);
            assert!((t.angle - 30.0_f64.to_radians()).abs() < 1e-6);
            assert_eq!(t.h_align, cad_kernel::TextHAlign::Right);
            assert!((t.oblique - 12.0_f64.to_radians()).abs() < 1e-6, "oblique");
            assert!((t.width_factor - 0.85).abs() < 1e-9, "width");
            assert!(t.bold && t.outline_only, "bold/outline flags");
            assert!(t.underline, "underline flag");
            assert!((t.outline_width - 0.3).abs() < 1e-9, "outline width");
            assert_eq!(t.font_name, "Arial");
        } else { panic!("expected Text, got a different geom"); }
    }

    #[test]
    fn spline_rational_round_trip() {
        // WP1.8 / B3: degree-3 rational spline, 5 ctrl pts, one weight ≠ 1.
        let ctrl = vec![
            Vec2::new(0.0, 0.0), Vec2::new(1.0, 2.0), Vec2::new(3.0, 3.0),
            Vec2::new(5.0, 1.0), Vec2::new(6.0, -1.0),
        ];
        let weights = vec![1.0, 1.0, 2.5, 1.0, 1.0];
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Spline(
            cad_kernel::Spline::new(3, ctrl.clone(), weights.clone()))));
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 1, "spline was dropped on import");
        if let Geom::Spline(s) = &back.dobjects[0].geom {
            assert_eq!(s.degree, 3);
            assert_eq!(s.control_points.len(), ctrl.len());
            for (a, b) in s.control_points.iter().zip(&ctrl) {
                assert!((a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
            }
            assert_eq!(s.weights.len(), weights.len());
            for (a, b) in s.weights.iter().zip(&weights) {
                assert!((a - b).abs() < 1e-9, "weight {} != {}", a, b);
            }
        } else { panic!("expected Spline"); }
    }

    #[test]
    fn spline_bspline_round_trip() {
        // Non-rational path: all weights 1.0 → writer omits 41 → reader rebuilds
        // via new_bspline. degree 3, 5 ctrl pts.
        let ctrl = vec![
            Vec2::new(0.0, 0.0), Vec2::new(2.0, 4.0), Vec2::new(4.0, 0.0),
            Vec2::new(6.0, 4.0), Vec2::new(8.0, 0.0),
        ];
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Spline(
            cad_kernel::Spline::new_bspline(3, ctrl.clone()))));
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 1, "b-spline was dropped on import");
        if let Geom::Spline(s) = &back.dobjects[0].geom {
            assert_eq!(s.degree, 3);
            assert_eq!(s.control_points.len(), ctrl.len());
            assert!(s.weights.iter().all(|w| (w - 1.0).abs() < 1e-9),
                "non-rational weights must all be 1.0");
        } else { panic!("expected Spline"); }
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
            // 2 segments → 2 width pairs preserved via DXF 40/41.
            assert_eq!(p.widths.len(), 2);
            assert!((p.widths[0].0 - 2.0).abs() < 1e-9 && (p.widths[0].1 - 2.0).abs() < 1e-9);
            assert!((p.widths[1].0 - 1.0).abs() < 1e-9 && (p.widths[1].1 - 3.0).abs() < 1e-9);
        } else { panic!(); }
    }

    #[test]
    fn polyline_round_trip_open() {
        let mut doc = Document::default();
        doc.push(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(5.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(5.0, 5.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        }.into());
        let back = round_trip(&doc);
        if let Geom::Polyline(p) = &back.dobjects[0].geom {
            assert_eq!(p.vertices.len(), 3);
            assert!(!p.closed);
        } else { panic!(); }
    }

    #[test]
    fn polyline_round_trip_closed() {
        let mut doc = Document::default();
        doc.push(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(5.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(5.0, 5.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 5.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        }.into());
        let back = round_trip(&doc);
        if let Geom::Polyline(p) = &back.dobjects[0].geom {
            assert_eq!(p.vertices.len(), 4);
            assert!(p.closed);
        } else { panic!(); }
    }

    #[test]
    fn ellipse_round_trip() {
        let mut doc = Document::default();
        doc.push(Ellipse {
            center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4,
        }.into());
        let back = round_trip(&doc);
        if let Geom::Ellipse(e) = &back.dobjects[0].geom {
            assert!((e.semi_major() - 5.0).abs() < 1e-9);
            assert!((e.ratio - 0.4).abs() < 1e-9);
        } else { panic!(); }
    }

    #[test]
    fn layer_round_trip_preserves_name_and_color() {
        let mut doc = Document::default();
        let walls = doc.layers.add(Layer {
            name: "WALLS".into(),
            color: Color::Aci(1),
            order:      0,
            ..Layer::layer_zero()});
        doc.layers.active = walls;
        // WP6.1: push is a pure append now — it no longer inherits the active
        // layer, so the dobject must be placed on WALLS explicitly (was relying
        // on the removed inheritance).
        let mut circ: DObject = Circle { center: Vec2::ZERO, radius: 5.0 }.into();
        circ.style.layer = walls;
        doc.push(circ);
        let back = round_trip(&doc);
        // Layer must round-trip
        let id = back.layers.find("WALLS").expect("WALLS layer not preserved");
        assert!(matches!(back.layers.get(id).unwrap().color, Color::Aci(1)));
        // Dobject's style.layer must point at WALLS post-import
        assert_eq!(back.dobjects[0].style.layer, id);
    }

    /// #41 — a negative 62 (layer OFF) must NOT be folded into the frozen
    /// flag: off, frozen, locked and not-plottable import as distinct states.
    #[test]
    fn layer_off_is_not_frozen() {
        let dxf = "\
0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n70\n4\n\
0\nLAYER\n2\nOFFL\n70\n0\n62\n-3\n6\nContinuous\n\
0\nLAYER\n2\nFROZENL\n70\n1\n62\n4\n6\nContinuous\n\
0\nLAYER\n2\nLOCKEDL\n70\n4\n62\n5\n6\nContinuous\n\
0\nLAYER\n2\nNOPLOTL\n70\n16\n62\n6\n6\nContinuous\n\
0\nLAYER\n2\nOFFFROZENL\n70\n1\n62\n-2\n6\nContinuous\n\
0\nENDTAB\n0\nENDSEC\n0\nEOF\n";
        let doc = read_dxf(dxf).expect("parse");
        let layer = |name: &str| {
            let id = doc.layers.find(name).expect(name);
            doc.layers.get(id).unwrap().clone()
        };
        let off = layer("OFFL");
        assert!(!off.visible, "negative 62 must mean OFF, not frozen");
        assert!(!off.frozen, "OFF layer must not be frozen");
        let fro = layer("FROZENL");
        assert!(fro.visible, "frozen flag alone must keep the layer visible");
        assert!(fro.frozen);
        assert!(!fro.locked, "frozen must not imply locked");
        assert!(layer("LOCKEDL").locked);
        assert!(layer("LOCKEDL").visible, "locked layer stays visible");
        let noplot = layer("NOPLOTL");
        assert!(!noplot.plottable, "bit 0x10 = not plottable");
        assert!(noplot.visible, "not-plottable stays visible");
        // Off AND frozen simultaneously — both states must survive.
        let both = layer("OFFFROZENL");
        assert!(!both.visible);
        assert!(both.frozen);
    }

    /// A non-plottable layer must round-trip through DXF (writer emits 0x10,
    /// reader restores plottable = false).
    #[test]
    fn layer_plottable_round_trip() {
        let mut doc = Document::default();
        doc.layers.add(Layer {
            name: "NOPLOT".into(),
            color: Color::Aci(6),
            plottable: false,
            order:      0,
            ..Layer::layer_zero()});
        let back = round_trip(&doc);
        let id = back.layers.find("NOPLOT").expect("NOPLOT layer preserved");
        assert!(!back.layers.get(id).unwrap().plottable,
            "plottable=false must survive a DXF round-trip");
        // Sanity: a normal layer stays plottable.
        let l0 = back.layers.get(0).unwrap();
        assert!(l0.plottable, "default plottable layer must stay plottable");
    }
}

#[cfg(test)]
mod centermark_dxf_tests {
    use super::*;

    #[test]
    fn centermark_round_trip_via_dxf() {
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::CenterMark(cad_kernel::CenterMark {
            center: cad_kernel::Vec2::new(2.0, -3.0),
            size: 1.25,
            rotation: 0.5,
        })));
        let text = write_dxf(&doc);
        let back = read_dxf(&text).expect("read back");
        // The CENTERMARK entity itself imports; the two fallback LINEs
        // also import as real lines — so we see the mark + 2 arms.
        let marks = back.dobjects.iter()
            .filter(|d| matches!(d.geom, Geom::CenterMark(_)))
            .count();
        assert_eq!(marks, 1, "CENTERMARK entity survives the round trip");
        if let Some(d) = back.dobjects.iter()
            .find(|d| matches!(d.geom, Geom::CenterMark(_)))
        {
            let Geom::CenterMark(cm) = &d.geom else { unreachable!() };
            assert!((cm.center.x - 2.0).abs() < 1e-9);
            assert!((cm.center.y + 3.0).abs() < 1e-9);
            assert!((cm.size - 1.25).abs() < 1e-9);
            assert!((cm.rotation - 0.5).abs() < 1e-9);
        }
    }
}

#[cfg(test)]
mod dimension_entity_tests {
    use super::*;

    fn round_trip(doc: &Document) -> Document {
        let dxf = write_dxf(doc);
        read_dxf(&dxf).expect("dxf parse")
    }

    fn linear_doc() -> Document {
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(10.0, 0.0),
                dimline_pos: Vec2::new(5.0, -5.0),
                ortho: cad_kernel::LinearOrtho::Horizontal,
            },
            style: cad_kernel::DimStyleTable::STANDARD,
            text_override: Some("<> mm".into()),
        })));
        doc
    }

    #[test]
    fn linear_dimension_round_trips_as_real_dimension() {
        let back = round_trip(&linear_doc());
        assert_eq!(back.dobjects.len(), 1, "dimension must import");
        if let Geom::Dimension(d) = &back.dobjects[0].geom {
            match d.kind {
                cad_kernel::DimKind::Linear { p1, p2, dimline_pos, ortho } => {
                    assert!((p1 - Vec2::new(0.0, 0.0)).len() < 1e-6);
                    assert!((p2 - Vec2::new(10.0, 0.0)).len() < 1e-6);
                    assert!((dimline_pos - Vec2::new(5.0, -5.0)).len() < 1e-6);
                    assert_eq!(ortho, cad_kernel::LinearOrtho::Horizontal);
                }
                _ => panic!("kind lost: {:?}", d.kind),
            }
            assert_eq!(d.text_override.as_deref(), Some("<> mm"));
        } else { panic!("not a dimension"); }
    }

    #[test]
    fn angular_dimension_round_trips() {
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
            kind: cad_kernel::DimKind::Angular {
                vertex: Vec2::new(0.0, 0.0),
                p1: Vec2::new(10.0, 0.0),
                p2: Vec2::new(0.0, 10.0),
                arc_pos: Vec2::new(5.0, 5.0),
            },
            style: cad_kernel::DimStyleTable::STANDARD,
            text_override: None,
        })));
        let back = round_trip(&doc);
        if let Geom::Dimension(d) = &back.dobjects[0].geom {
            match d.kind {
                cad_kernel::DimKind::Angular { vertex, p1, p2, arc_pos } => {
                    assert!((vertex - Vec2::new(0.0, 0.0)).len() < 1e-6);
                    assert!((p1 - Vec2::new(10.0, 0.0)).len() < 1e-6);
                    assert!((p2 - Vec2::new(0.0, 10.0)).len() < 1e-6);
                    assert!((arc_pos - Vec2::new(5.0, 5.0)).len() < 1e-6);
                }
                _ => panic!("kind lost"),
            }
            assert!(d.text_override.is_none(), "<> means measured");
        } else { panic!("not a dimension"); }
    }

    #[test]
    fn radius_and_diameter_round_trip() {
        for (kind, typ) in [
            (cad_kernel::DimKind::Radius {
                center: Vec2::new(0.0, 0.0),
                on_circle: Vec2::new(3.0, 0.0),
                leader_end: Vec2::new(6.0, 4.0),
            }, 4),
            (cad_kernel::DimKind::Diameter {
                center: Vec2::new(0.0, 0.0),
                on_circle: Vec2::new(3.0, 0.0),
                leader_end: Vec2::new(6.0, 4.0),
            }, 3),
        ] {
            let mut doc = Document::default();
            doc.push(DObject::new(Geom::Dimension(cad_kernel::Dim {
                kind, style: 0, text_override: None,
            })));
            let back = round_trip(&doc);
            if let Geom::Dimension(d) = &back.dobjects[0].geom {
                match d.kind {
                    cad_kernel::DimKind::Radius { center, on_circle, leader_end } => {
                        assert_eq!(typ, 4);
                        assert!((center - Vec2::new(0.0, 0.0)).len() < 1e-6);
                        assert!((on_circle - Vec2::new(3.0, 0.0)).len() < 1e-6);
                        assert!((leader_end - Vec2::new(6.0, 4.0)).len() < 1e-6);
                    }
                    cad_kernel::DimKind::Diameter { center, on_circle, leader_end } => {
                        assert_eq!(typ, 3);
                        assert!((center - Vec2::new(0.0, 0.0)).len() < 1e-6);
                        assert!((on_circle - Vec2::new(3.0, 0.0)).len() < 1e-6);
                        assert!((leader_end - Vec2::new(6.0, 4.0)).len() < 1e-6);
                    }
                    _ => panic!("kind lost for type {typ}"),
                }
            } else { panic!("not a dimension"); }
        }
    }

    #[test]
    fn dimension_text_contains_measurement() {
        let dxf = write_dxf(&linear_doc());
        assert!(dxf.contains("0\nDIMENSION"), "real DIMENSION entity");
        assert!(dxf.contains("1\n<> mm"), "override verbatim");
        assert!(dxf.contains("42\n10"), "measurement code");
        assert!(dxf.contains("AcDbRotatedDimension"));
    }
}

#[cfg(test)]
mod mtext_entity_tests {
    use super::*;

    fn round_trip(doc: &Document) -> Document {
        let dxf = write_dxf(doc);
        read_dxf(&dxf).expect("dxf parse")
    }

    #[test]
    fn coded_text_round_trips_as_mtext() {
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Text(cad_kernel::Text {
            position: Vec2::new(2.0, 3.0),
            height: 0.5,
            angle: 0.0,
            text: "plain \\C1;red\\C0; end".into(),
            h_align: cad_kernel::TextHAlign::Left,
            v_align: cad_kernel::TextVAlign::Baseline,
            style: cad_kernel::TextStyleTable::STANDARD,
            ..cad_kernel::Text::empty()
        })));
        let dxf = write_dxf(&doc);
        assert!(dxf.contains("0\nMTEXT"), "coded text exports as MTEXT");
        let back = round_trip(&doc);
        assert_eq!(back.dobjects.len(), 1);
        if let Geom::Text(t) = &back.dobjects[0].geom {
            assert_eq!(t.text, "plain \\C1;red\\C0; end", "codes preserved");
            assert_eq!((t.position.x, t.position.y), (2.0, 3.0));
        } else { panic!("mtext lost"); }
    }

    #[test]
    fn multiline_text_round_trips_as_mtext_with_breaks() {
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Text(cad_kernel::Text {
            position: Vec2::ZERO,
            height: 0.4,
            angle: 0.0,
            text: "line one\nline two".into(),
            h_align: cad_kernel::TextHAlign::Left,
            v_align: cad_kernel::TextVAlign::Baseline,
            style: cad_kernel::TextStyleTable::STANDARD,
            ..cad_kernel::Text::empty()
        })));
        let dxf = write_dxf(&doc);
        assert!(dxf.contains("0\nMTEXT"));
        assert!(dxf.contains("line one\\Pline two"), "\\P paragraph codes");
        let back = round_trip(&doc);
        if let Geom::Text(t) = &back.dobjects[0].geom {
            assert_eq!(t.text, "line one\nline two", "\\P back to newline");
        } else { panic!("mtext lost"); }
    }

    #[test]
    fn plain_text_stays_text_entity() {
        let mut doc = Document::default();
        doc.push(DObject::new(Geom::Text(cad_kernel::Text {
            position: Vec2::ZERO,
            height: 0.4,
            angle: 0.0,
            text: "hello".into(),
            h_align: cad_kernel::TextHAlign::Left,
            v_align: cad_kernel::TextVAlign::Baseline,
            style: cad_kernel::TextStyleTable::STANDARD,
            ..cad_kernel::Text::empty()
        })));
        let dxf = write_dxf(&doc);
        assert!(dxf.contains("0\nTEXT"));
        assert!(!dxf.contains("0\nMTEXT"));
        let back = round_trip(&doc);
        if let Geom::Text(t) = &back.dobjects[0].geom {
            assert_eq!(t.text, "hello");
        } else { panic!(); }
    }
}
