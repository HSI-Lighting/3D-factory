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
    let pairs = parse_pairs(text)?;
    let mut doc = Document::default();
    let mut i = 0;
    while i < pairs.len() {
        let (code, value) = &pairs[i];
        if *code == 0 && value == "SECTION" && i + 1 < pairs.len() {
            let (c2, name) = &pairs[i + 1];
            if *c2 == 2 {
                match name.as_str() {
                    "TABLES"   => i = read_tables(&pairs, i + 2, &mut doc),
                    // BLOCKS precedes ENTITIES in the file, so block defs land
                    // in the table before any INSERT in ENTITIES resolves them.
                    "BLOCKS"   => i = read_blocks(&pairs, i + 2, &mut doc),
                    "ENTITIES" => i = read_entities(&pairs, i + 2, &mut doc),
                    _ => i = skip_to_endsec(&pairs, i + 2),
                }
                continue;
            }
        }
        i += 1;
    }
    Ok(doc)
}

/// Tokenize the source into (code, value) pairs. DXF files use CRLF or
/// LF; we tolerate either. The line *after* the code line is the value;
/// trailing whitespace is trimmed.
fn parse_pairs(text: &str) -> Result<Vec<(i32, String)>, String> {
    let mut lines = text.lines();
    let mut out = Vec::new();
    while let Some(code_line) = lines.next() {
        let code_str = code_line.trim();
        if code_str.is_empty() { continue; }
        let value_line = lines.next()
            .ok_or_else(|| "DXF: code line without value line".to_string())?;
        let code: i32 = code_str.parse()
            .map_err(|_| format!("DXF: bad group code '{}'", code_str))?;
        out.push((code, value_line.trim().to_string()));
    }
    Ok(out)
}

fn skip_to_endsec(pairs: &[(i32, String)], start: usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDSEC" { return i + 1; }
        i += 1;
    }
    pairs.len()
}

fn read_tables(pairs: &[(i32, String)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDSEC" { return i + 1; }
        if *c == 0 && v == "TABLE" && i + 1 < pairs.len() {
            let (c2, name) = &pairs[i + 1];
            if *c2 == 2 {
                match name.as_str() {
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

fn skip_to_endtab(pairs: &[(i32, String)], start: usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDTAB" { return i + 1; }
        i += 1;
    }
    pairs.len()
}

fn read_layer_table(pairs: &[(i32, String)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDTAB" { return i + 1; }
        if *c == 0 && v == "LAYER" {
            // Accumulate this layer's fields until the next 0-group.
            let mut name = String::new();
            let mut color = Color::Aci(7);   // ACI 7 = white default
            let mut lt_name = String::from("Continuous");
            let mut flags: i32 = 0;
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    2  => name    = pairs[i].1.clone(),
                    62 => {
                        let aci: i32 = pairs[i].1.parse().unwrap_or(7);
                        // Negative ACI = layer off; magnitude = the color.
                        let off = aci < 0;
                        let abs = aci.unsigned_abs() as u8;
                        color = Color::Aci(abs);
                        if off { flags |= 0x01; }   // mark hidden
                    }
                    6  => lt_name = pairs[i].1.clone(),
                    70 => flags |= pairs[i].1.parse::<i32>().unwrap_or(0),
                    _ => {}
                }
                i += 1;
            }
            if !name.is_empty() {
                // "0" already exists at id 0; reuse it instead of duplicating.
                let lt_id = doc.linetypes.find(&lt_name)
                    .unwrap_or(LinetypeTable::CONTINUOUS);
                if let Some(existing) = doc.layers.find(&name) {
                    if let Some(l) = doc.layers.get_mut(existing) {
                        l.color    = color;
                        l.linetype = lt_id;
                        l.visible  = (flags & 0x01) == 0;
                        l.frozen   = (flags & 0x01) != 0;
                        l.locked   = (flags & 0x04) != 0;
                    }
                } else {
                    doc.layers.add(Layer {
                        name,
                        color,
                        linetype:   lt_id,
                        lineweight: Lineweight::Default,
                        visible:    (flags & 0x01) == 0,
                        locked:     (flags & 0x04) != 0,
                        frozen:     (flags & 0x01) != 0,
                        plottable:  true,
                    });
                }
            }
            continue;
        }
        i += 1;
    }
    pairs.len()
}

fn read_ltype_table(pairs: &[(i32, String)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDTAB" { return i + 1; }
        if *c == 0 && v == "LTYPE" {
            let mut name = String::new();
            let mut desc = String::new();
            let mut pattern: Vec<f32> = Vec::new();
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    2  => name = pairs[i].1.clone(),
                    3  => desc = pairs[i].1.clone(),
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
            if !name.is_empty() && doc.linetypes.find(&name).is_none() {
                doc.linetypes.add(Linetype { name, description: desc, pattern });
            }
            continue;
        }
        i += 1;
    }
    pairs.len()
}

fn read_entities(pairs: &[(i32, String)], start: usize, doc: &mut Document) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDSEC" { return i + 1; }
        if *c == 0 {
            let entity_kind = v.clone();
            // Collect this entity's fields until the next 0-group.
            let mut fields: Vec<(i32, String)> = Vec::new();
            i += 1;
            while i < pairs.len() && pairs[i].0 != 0 {
                fields.push(pairs[i].clone());
                i += 1;
            }
            if let Some(d) = build_entity(&entity_kind, &fields, doc) {
                doc.push(d);
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
fn read_blocks(pairs: &[(i32, String)], start: usize, doc: &mut Document) -> usize {
    // TWO passes so nested INSERTs resolve regardless of definition order:
    //   pass 1 registers every real block name (empty placeholder, fixes ids);
    //   pass 2 fills each block's base + entities (build_entity now resolves
    //   nested INSERTs by name — forward references included).
    // ---- pass 1: register names -----------------------------------------
    let end;
    let mut i = start;
    loop {
        if i >= pairs.len() { end = pairs.len(); break; }
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDSEC" { end = i + 1; break; }
        if *c == 0 && v == "BLOCK" {
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
        let (c, v) = &pairs[i];
        if *c == 0 && v == "BLOCK" {
            i += 1;
            let mut name = String::new();
            let mut base = Vec2::new(0.0, 0.0);
            while i < pairs.len() && pairs[i].0 != 0 {
                match pairs[i].0 {
                    2  => name   = pairs[i].1.clone(),
                    10 => base.x = pairs[i].1.parse().unwrap_or(0.0),
                    20 => base.y = pairs[i].1.parse().unwrap_or(0.0),
                    _  => {}
                }
                i += 1;
            }
            let mut dobjects: Vec<DObject> = Vec::new();
            while i < pairs.len() {
                let (c2, v2) = &pairs[i];
                if *c2 == 0 && (v2 == "ENDBLK" || v2 == "ENDSEC") {
                    if v2 == "ENDBLK" { i += 1; }
                    break;
                }
                if *c2 == 0 {
                    let kind = v2.clone();
                    i += 1;
                    let mut fields: Vec<(i32, String)> = Vec::new();
                    while i < pairs.len() && pairs[i].0 != 0 {
                        fields.push(pairs[i].clone());
                        i += 1;
                    }
                    // build_entity resolves nested INSERTs via doc.blocks (all
                    // names are registered now, so order doesn't matter).
                    if let Some(d) = build_entity(&kind, &fields, doc) { dobjects.push(d); }
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
fn block_name(pairs: &[(i32, String)], start: usize) -> String {
    let mut i = start;
    while i < pairs.len() && pairs[i].0 != 0 {
        if pairs[i].0 == 2 { return pairs[i].1.clone(); }
        i += 1;
    }
    String::new()
}

fn skip_to_endblk(pairs: &[(i32, String)], start: usize) -> usize {
    let mut i = start;
    while i < pairs.len() {
        let (c, v) = &pairs[i];
        if *c == 0 && v == "ENDBLK" { return i + 1; }
        if *c == 0 && v == "ENDSEC" { return i; }
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

fn build_entity(kind: &str, fields: &[(i32, String)], doc: &Document) -> Option<DObject> {
    let mut layer_name = String::new();
    let mut color: Option<Color> = None;
    let mut linetype_name: Option<String> = None;
    let mut visible = true;
    // helpers — return None when the field is missing or unparseable
    let get_f = |code: i32| -> Option<f64> {
        fields.iter().find(|(c, _)| *c == code).and_then(|(_, v)| v.parse().ok())
    };
    let get_i = |code: i32| -> Option<i32> {
        fields.iter().find(|(c, _)| *c == code).and_then(|(_, v)| v.parse().ok())
    };

    for (c, v) in fields {
        match *c {
            8  => layer_name = v.clone(),
            62 => {
                if let Ok(aci) = v.parse::<i32>() {
                    color = Some(if aci == 256 { Color::ByLayer }
                                 else if aci == 0 { Color::ByBlock }
                                 else { Color::Aci(aci.unsigned_abs() as u8) });
                }
            }
            6  => linetype_name = Some(v.clone()),
            60 => visible = v.parse::<i32>().unwrap_or(0) == 0,
            _  => {}
        }
    }

    let geom = match kind {
        "LINE" => Geom::Line(Line {
            a: Vec2::new(get_f(10)?, get_f(20)?),
            b: Vec2::new(get_f(11)?, get_f(21)?),
        }),
        "CIRCLE" => Geom::Circle(Circle {
            center: Vec2::new(get_f(10)?, get_f(20)?),
            radius: get_f(40)?,
        }),
        "ARC" => {
            let sa = get_f(50)?.to_radians();
            let ea = get_f(51)?.to_radians();
            let sweep = (ea - sa).rem_euclid(std::f64::consts::TAU);
            let sweep = if sweep < 1e-9 { std::f64::consts::TAU } else { sweep };
            Geom::Arc(Arc {
                center: Vec2::new(get_f(10)?, get_f(20)?),
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
            location: Vec2::new(get_f(10)?, get_f(20)?),
            style: 0, size: 0.0,
        }),
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
                match *c {
                    10 => {
                        if let Some(p) = cur.take() {
                            vertices.push(PolyVertex { pos: p, bulge: cur_bulge });
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
                    42 => cur_bulge = v.parse().unwrap_or(0.0),
                    43 => const_w = v.parse().unwrap_or(0.0),
                    _ => {}
                }
            }
            if let Some(p) = cur.take() {
                vertices.push(PolyVertex { pos: p, bulge: cur_bulge });
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
            let bname = fields.iter().find(|(c, _)| *c == 2).map(|(_, v)| v.clone())?;
            let block = doc.blocks.find(&bname)?;   // unknown/skipped block → drop
            let sx = get_f(41).unwrap_or(1.0);
            let sy = get_f(42).unwrap_or(1.0);
            // Factor signs out into mirror_x + a π rotation; the per-axis
            // MAGNITUDES go to scale / scale_y (non-uniform → ellipses).
            let mirror_x = (sx < 0.0) != (sy < 0.0);
            let extra = if sy < 0.0 { std::f64::consts::PI } else { 0.0 };
            Geom::BlockRef(BlockRef {
                block,
                insert:   Vec2::new(get_f(10)?, get_f(20)?),
                scale:    sx.abs().max(1e-9),
                scale_y:  sy.abs().max(1e-9),
                rotation: get_f(50).unwrap_or(0.0).to_radians() + extra,
                mirror_x,
                param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            })
        }
        _ => return None,   // unknown entity type — silently skip
    };

    // Build the DObject with the proper style.
    let mut style = cad_kernel::Style::default();
    if let Some(c) = color { style.color = c; }
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

    Some(DObject::with_style(geom, style))
}

// ============================================================================
//   WRITER
// ============================================================================

/// Serialize a `Document` to DXF ASCII text. Writes a minimal HEADER,
/// the LAYER and LTYPE tables (so layer/linetype references in ENTITIES
/// resolve when read back), then every Dobject. The result reads
/// correctly in LibreCAD and AutoCAD.
pub fn write_dxf(doc: &Document) -> String {
    let mut s = String::with_capacity(64 * 1024);
    write_header(&mut s);
    write_tables(&mut s, doc);
    write_entities(&mut s, doc);
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

fn write_header(s: &mut String) {
    pair(s, 0, "SECTION");
    pair(s, 2, "HEADER");
    pair(s, 9, "$ACADVER"); pair(s, 1, "AC1015");   // AutoCAD 2000 ASCII level
    pair(s, 0, "ENDSEC");
}

fn write_tables(s: &mut String, doc: &Document) {
    pair(s, 0, "SECTION");
    pair(s, 2, "TABLES");

    // ---- LTYPE table ----
    pair(s, 0, "TABLE"); pair(s, 2, "LTYPE");
    pair_i(s, 70, doc.linetypes.len() as i32);
    for lt in &doc.linetypes.linetypes {
        pair(s, 0, "LTYPE");
        pair(s, 2, &lt.name);
        pair_i(s, 70, 0);
        pair(s, 3, &lt.description);
        pair_i(s, 72, 65);          // alignment code 'A'
        pair_i(s, 73, lt.pattern.len() as i32);
        let total: f32 = lt.pattern.iter().sum();
        pair_f(s, 40, total as f64);
        for (i, p) in lt.pattern.iter().enumerate() {
            // Alternating dash/gap convention in our model maps to:
            // dash = positive, gap = negative.
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            pair_f(s, 49, (*p as f64) * sign);
        }
    }
    pair(s, 0, "ENDTAB");

    // ---- LAYER table ----
    pair(s, 0, "TABLE"); pair(s, 2, "LAYER");
    pair_i(s, 70, doc.layers.len() as i32);
    for layer in &doc.layers.layers {
        pair(s, 0, "LAYER");
        pair(s, 2, &layer.name);
        // Flags: bit 0 = frozen, bit 2 = locked.
        let mut flags = 0_i32;
        if layer.frozen { flags |= 0x01; }
        if layer.locked { flags |= 0x04; }
        pair_i(s, 70, flags);
        // Color: ACI index (negative = layer off)
        let aci = match layer.color {
            Color::Aci(i) => i as i32,
            // For TrueColor, pick a "close enough" ACI — 7 = white/black, fine fallback.
            _ => 7,
        };
        let aci_signed = if layer.visible { aci } else { -aci.abs().max(1) };
        pair_i(s, 62, aci_signed);
        // Linetype name reference
        let lt_name = doc.linetypes.get(layer.linetype)
            .map(|l| l.name.clone()).unwrap_or_else(|| "Continuous".into());
        pair(s, 6, &lt_name);
    }
    pair(s, 0, "ENDTAB");

    pair(s, 0, "ENDSEC");
}

fn write_entities(s: &mut String, doc: &Document) {
    pair(s, 0, "SECTION");
    pair(s, 2, "ENTITIES");

    for d in &doc.dobjects {
        write_entity(s, d, doc);
    }

    pair(s, 0, "ENDSEC");
}

fn write_entity(s: &mut String, d: &DObject, doc: &Document) {
    let layer_name = doc.layers.get(d.style.layer)
        .map(|l| l.name.clone()).unwrap_or_else(|| "0".into());
    let linetype_name = doc.linetypes.get(d.style.linetype)
        .map(|l| l.name.clone()).unwrap_or_else(|| "Continuous".into());
    let common = |s: &mut String, kind: &str| {
        pair(s, 0, kind);
        pair(s, 8, &layer_name);
        pair(s, 6, &linetype_name);
        if let Color::Aci(i) = d.style.color {
            pair_i(s, 62, i as i32);
        } else if let Color::ByBlock = d.style.color {
            pair_i(s, 62, 0);
        } else {
            // ByLayer (default) and TrueColor both go as 256 (ByLayer) for
            // now — TrueColor would need group code 420 (handled in a follow-up).
            pair_i(s, 62, 256);
        }
        if !d.style.visible { pair_i(s, 60, 1); }
    };

    match &d.geom {
        Geom::Line(l) => {
            common(s, "LINE");
            pair_f(s, 10, l.a.x); pair_f(s, 20, l.a.y); pair_f(s, 30, 0.0);
            pair_f(s, 11, l.b.x); pair_f(s, 21, l.b.y); pair_f(s, 31, 0.0);
        }
        Geom::Circle(c) => {
            common(s, "CIRCLE");
            pair_f(s, 10, c.center.x); pair_f(s, 20, c.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 40, c.radius);
        }
        Geom::Arc(a) => {
            common(s, "ARC");
            pair_f(s, 10, a.center.x); pair_f(s, 20, a.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 40, a.radius);
            pair_f(s, 50, a.start_angle.to_degrees());
            pair_f(s, 51, (a.start_angle + a.sweep_angle).to_degrees());
        }
        Geom::Ellipse(el) => {
            common(s, "ELLIPSE");
            pair_f(s, 10, el.center.x); pair_f(s, 20, el.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 11, el.major.x);  pair_f(s, 21, el.major.y);  pair_f(s, 31, 0.0);
            pair_f(s, 40, el.ratio);
            pair_f(s, 41, 0.0);
            pair_f(s, 42, std::f64::consts::TAU);
        }
        Geom::EllipseArc(ea) => {
            common(s, "ELLIPSE");
            pair_f(s, 10, ea.ellipse.center.x); pair_f(s, 20, ea.ellipse.center.y); pair_f(s, 30, 0.0);
            pair_f(s, 11, ea.ellipse.major.x);  pair_f(s, 21, ea.ellipse.major.y);  pair_f(s, 31, 0.0);
            pair_f(s, 40, ea.ellipse.ratio);
            pair_f(s, 41, ea.start_param);
            pair_f(s, 42, ea.start_param + ea.sweep_param);
        }
        Geom::Point(pt) => {
            common(s, "POINT");
            pair_f(s, 10, pt.location.x); pair_f(s, 20, pt.location.y); pair_f(s, 30, 0.0);
        }
        Geom::Polyline(p) => {
            common(s, "LWPOLYLINE");
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
        // DXF HATCH is a substantial entity with pattern, seed point,
        // boundary edge data, and seed loop topology. MVP does NOT
        // export hatches yet — skip silently so a round-trip with a
        // hatch in the doc just drops the hatch but otherwise succeeds.
        Geom::Hatch(_) => {}
        // DXF SPLINE entity needs: flags / degree / knots / weights /
        // control points / fit points / fit tolerance. v1 doesn't
        // export it yet — skip silently. Round-trip in RSM works.
        Geom::Spline(_) => {}
        // Wall — DXF has no native wall entity. Export the two side
        // lines as LINE entities so the geometry round-trips
        // visually. The "smart" centerline+thickness link is lost on
        // export (recoverable on re-import only via heuristics).
        Geom::Wall(w) => {
            if let (Some(l), Some(r)) = (w.left_line(), w.right_line()) {
                common(s, "LINE");
                pair_f(s, 10, l.a.x); pair_f(s, 20, l.a.y); pair_f(s, 30, 0.0);
                pair_f(s, 11, l.b.x); pair_f(s, 21, l.b.y); pair_f(s, 31, 0.0);
                common(s, "LINE");
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
            common(s, "TEXT");
            pair_f(s, 10, t.position.x);
            pair_f(s, 20, t.position.y);
            pair_f(s, 30, 0.0);
            pair_f(s, 40, t.height);
            pair(s, 1, &t.text);
            if t.angle.abs() > 1e-12 {
                pair_f(s, 50, t.angle.to_degrees());
            }
            let halign_code = match t.h_align {
                cad_kernel::TextHAlign::Left   => 0,
                cad_kernel::TextHAlign::Center => 1,
                cad_kernel::TextHAlign::Right  => 2,
            };
            if halign_code != 0 { pair_i(s, 72, halign_code); }
        }
        Geom::Dimension(d) => {
            // V1 DXF: dimensions are written as exploded geometry —
            // the kernel's def points become a simple polyline + text
            // so other CAD packages can read the document. Full
            // AutoCAD DIMENSION entity round-trip (with all DIMVARs)
            // ships in a follow-up slice.
            use cad_kernel::DimKind;
            common(s, "TEXT");
            let pos = match &d.kind {
                DimKind::Linear { dimline_pos, .. } => *dimline_pos,
                DimKind::Radius { leader_end, .. } |
                DimKind::Diameter { leader_end, .. } => *leader_end,
            };
            pair_f(s, 10, pos.x);
            pair_f(s, 20, pos.y);
            pair_f(s, 30, 0.0);
            pair_f(s, 40, 0.18);  // placeholder text height
            let st = doc.dim_styles.get(d.style)
                .unwrap_or(doc.dim_styles.get(0).unwrap());
            pair(s, 1, &d.formatted_text(st));
        }
        Geom::BlockRef(br) => {
            // V1 DXF POLICY (documented interop debt): block instances
            // are written EXPLODED — each contained dobject transformed
            // into world space and emitted as a plain entity. The block
            // identity is lost in DXF (RSM keeps it); BLOCK/INSERT
            // round-trip is the "DXF parity" roadmap item. Recursion
            // handles nested blocks; cycles can't exist (see block.rs).
            if let Some(blk) = doc.blocks.get(br.block) {
                for cd in &blk.dobjects {
                    let mut inst = cd.clone();
                    inst.geom = br.transform_geom(&cd.geom, blk.base);
                    // ByBlock content takes the instance's color.
                    if matches!(inst.style.color, Color::ByBlock) {
                        inst.style.color = d.style.color;
                    }
                    write_entity(s, &inst, doc);
                }
            }
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
    fn empty_doc_round_trip() {
        let doc = Document::default();
        let back = round_trip(&doc);
        // Layer "0" + 3 default linetypes round-trip.
        assert_eq!(back.layers.len(), doc.layers.len());
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
            ..Layer::layer_zero()
        });
        doc.layers.active = walls;
        doc.push(Circle { center: Vec2::ZERO, radius: 5.0 }.into());
        let back = round_trip(&doc);
        // Layer must round-trip
        let id = back.layers.find("WALLS").expect("WALLS layer not preserved");
        assert!(matches!(back.layers.get(id).unwrap().color, Color::Aci(1)));
        // Dobject's style.layer must point at WALLS post-import
        assert_eq!(back.dobjects[0].style.layer, id);
    }
}
