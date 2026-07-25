//! Minimal Wavefront **OBJ** reader for furniture import — pure Rust, no dependency.
//!
//! OBJ is the pragmatic furniture format: every 3D tool exports it, it is plain text, and
//! it needs no library. We read only what the 3D view needs: vertex positions, optional
//! vertex normals, and faces (triangulated as a fan). Materials (`.mtl`), texture coords
//! and smoothing groups are ignored — colour is assigned in-app via the Textures menu.
//!
//! Robustness notes: vertex references are 1-based and may be NEGATIVE (relative to the
//! end); both are handled. Faces with more than 3 vertices are fan-triangulated. When a
//! face gives no normals, a flat per-face normal is computed so shading still works.

/// A parsed mesh as a flat triangle soup: `positions.len() == normals.len()`, a multiple
/// of 3 (three vertices per triangle).
#[derive(Clone, Debug, Default)]
pub struct ObjMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// A representative diffuse colour read from the file, if any (3DS material diffuse;
    /// OBJ carries none without its `.mtl`). `None` → the caller uses a neutral default.
    pub color: Option<[f32; 3]>,
}

impl ObjMesh {
    pub fn tri_count(&self) -> usize {
        self.positions.len() / 3
    }

    /// Axis-aligned bounds, or `None` when empty. Used to auto-scale / seat imported
    /// furniture at a sane size.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        mn[0].is_finite().then_some((mn, mx))
    }
}

/// Resolve a possibly-1-based, possibly-negative OBJ index against a table of `len` items.
/// Returns a 0-based index, or `None` if out of range.
fn resolve(idx: i64, len: usize) -> Option<usize> {
    let i = if idx > 0 {
        idx - 1
    } else if idx < 0 {
        len as i64 + idx
    } else {
        return None; // 0 is not a valid OBJ index
    };
    (i >= 0 && (i as usize) < len).then_some(i as usize)
}

/// Parse one face vertex token `v`, `v/vt`, `v//vn`, or `v/vt/vn` → `(v_index, vn_index?)`.
fn parse_ref(tok: &str) -> Option<(i64, Option<i64>)> {
    let mut parts = tok.split('/');
    let v: i64 = parts.next()?.parse().ok()?;
    let _vt = parts.next(); // texture coord — ignored
    let vn = parts.next().and_then(|s| s.parse::<i64>().ok());
    Some((v, vn))
}

/// Parse OBJ text into a triangle soup. Never fails: malformed lines are skipped, so a
/// slightly-off export still imports what it can rather than importing nothing.
pub fn parse_obj(text: &str) -> ObjMesh {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut norms: Vec<[f32; 3]> = Vec::new();
    let mut out = ObjMesh::default();

    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if c.len() == 3 {
                    verts.push([c[0], c[1], c[2]]);
                }
            }
            Some("vn") => {
                let c: Vec<f32> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if c.len() == 3 {
                    norms.push([c[0], c[1], c[2]]);
                }
            }
            Some("f") => {
                let refs: Vec<(i64, Option<i64>)> = it.filter_map(parse_ref).collect();
                if refs.len() < 3 {
                    continue;
                }
                // Fan-triangulate: (0, k, k+1).
                for k in 1..refs.len() - 1 {
                    let tri = [refs[0], refs[k], refs[k + 1]];
                    let mut ps = [[0.0f32; 3]; 3];
                    let mut ns = [None; 3];
                    let mut ok = true;
                    for (j, &(vi, ni)) in tri.iter().enumerate() {
                        match resolve(vi, verts.len()) {
                            Some(i) => ps[j] = verts[i],
                            None => { ok = false; break; }
                        }
                        ns[j] = ni.and_then(|n| resolve(n, norms.len())).map(|i| norms[i]);
                    }
                    if !ok {
                        continue;
                    }
                    // Flat face normal fallback where a vertex had none.
                    let face_n = flat_normal(ps[0], ps[1], ps[2]);
                    for j in 0..3 {
                        out.positions.push(ps[j]);
                        out.normals.push(ns[j].unwrap_or(face_n));
                    }
                }
            }
            _ => {} // comments (#), groups (g/o), mtl refs, smoothing — ignored
        }
    }
    out
}

// ── 3DS (Autodesk) binary import ────────────────────────────────────────────────────
//
// 3DS is a chunk tree: each chunk is a 2-byte id + 4-byte length (both little-endian,
// length includes the 6-byte header) followed by its body. We walk to the triangular
// meshes and read their vertex list (0x4110) and face list (0x4120). 3DS stores no
// normals, so a flat per-face normal is computed. Axis convention is Z-up, matching this
// engine, so no conversion is needed.

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Skip a null-terminated ASCII name, returning the offset just past the terminator.
fn skip_cstr(b: &[u8], mut o: usize, end: usize) -> usize {
    while o < end && b[o] != 0 {
        o += 1;
    }
    (o + 1).min(end) // step over the NUL
}

/// Read a `0x4100` triangular-mesh chunk into (vertices, triangle index triples).
fn read_mesh(b: &[u8], mut pos: usize, end: usize) -> (Vec<[f32; 3]>, Vec<[u16; 3]>) {
    let mut verts = Vec::new();
    let mut faces = Vec::new();
    while pos + 6 <= end {
        let id = le_u16(b, pos);
        let len = le_u32(b, pos + 2) as usize;
        if len < 6 {
            break;
        }
        let ce = (pos + len).min(end);
        let body = pos + 6;
        match id {
            0x4110 => {
                // vertex list: u16 count, then count × 3 f32
                if body + 2 <= ce {
                    let n = le_u16(b, body) as usize;
                    let mut o = body + 2;
                    for _ in 0..n {
                        if o + 12 > ce {
                            break;
                        }
                        verts.push([le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]);
                        o += 12;
                    }
                }
            }
            0x4120 => {
                // face list: u16 count, then count × (3 index u16 + 1 flags u16)
                if body + 2 <= ce {
                    let n = le_u16(b, body) as usize;
                    let mut o = body + 2;
                    for _ in 0..n {
                        if o + 8 > ce {
                            break;
                        }
                        faces.push([le_u16(b, o), le_u16(b, o + 2), le_u16(b, o + 4)]);
                        o += 8;
                    }
                }
            }
            _ => {}
        }
        pos = ce;
    }
    (verts, faces)
}

/// Read the first colour sub-chunk (`0x0010` float / `0x0011` byte, and their linear
/// variants) found within `[pos, end)`.
fn read_color(b: &[u8], mut pos: usize, end: usize) -> Option<[f32; 3]> {
    while pos + 6 <= end {
        let id = le_u16(b, pos);
        let len = le_u32(b, pos + 2) as usize;
        if len < 6 {
            break;
        }
        let ce = (pos + len).min(end);
        let body = pos + 6;
        match id {
            0x0011 | 0x0012 if body + 3 <= ce => {
                return Some([b[body] as f32 / 255.0, b[body + 1] as f32 / 255.0, b[body + 2] as f32 / 255.0]);
            }
            0x0010 | 0x0013 if body + 12 <= ce => {
                return Some([le_f32(b, body), le_f32(b, body + 4), le_f32(b, body + 8)]);
            }
            _ => {}
        }
        pos = ce;
    }
    None
}

/// Walk container chunks, emitting triangles for every mesh found, and capturing the first
/// material's diffuse colour.
fn walk_3ds(b: &[u8], mut pos: usize, end: usize, out: &mut ObjMesh) {
    while pos + 6 <= end {
        let id = le_u16(b, pos);
        let len = le_u32(b, pos + 2) as usize;
        if len < 6 {
            break;
        }
        let ce = (pos + len).min(end);
        let body = pos + 6;
        match id {
            0x4d4d | 0x3d3d => walk_3ds(b, body, ce, out), // MAIN / EDITOR containers
            0x4000 => walk_3ds(b, skip_cstr(b, body, ce), ce, out), // OBJECT — name first
            0xafff => {
                // MATERIAL block — capture the first diffuse (0xA020) colour.
                if out.color.is_none() {
                    let mut p = body;
                    while p + 6 <= ce {
                        let sid = le_u16(b, p);
                        let slen = le_u32(b, p + 2) as usize;
                        if slen < 6 { break; }
                        let sce = (p + slen).min(ce);
                        if sid == 0xa020 {
                            out.color = read_color(b, p + 6, sce);
                            break;
                        }
                        p = sce;
                    }
                }
            }
            0x4100 => {
                // TRIANGULAR MESH — read verts + faces and emit triangles.
                let (verts, faces) = read_mesh(b, body, ce);
                for f in faces {
                    let idx = [f[0] as usize, f[1] as usize, f[2] as usize];
                    if idx.iter().all(|&i| i < verts.len()) {
                        let ps = [verts[idx[0]], verts[idx[1]], verts[idx[2]]];
                        let n = flat_normal(ps[0], ps[1], ps[2]);
                        for p in ps {
                            out.positions.push(p);
                            out.normals.push(n);
                        }
                    }
                }
            }
            _ => {}
        }
        pos = ce;
    }
}

/// Parse a 3DS binary into a triangle soup. Malformed / truncated files yield whatever
/// meshes could be read rather than failing outright.
pub fn parse_3ds(data: &[u8]) -> ObjMesh {
    let mut out = ObjMesh::default();
    if data.len() >= 6 && le_u16(data, 0) == 0x4d4d {
        let len = le_u32(data, 2) as usize;
        walk_3ds(data, 6, len.min(data.len()), &mut out);
    }
    out
}

/// Newell-ish flat normal of a triangle; falls back to +Z for a degenerate face.
fn flat_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-9 {
        [0.0, 0.0, 1.0]
    } else {
        [n[0] / len, n[1] / len, n[2] / len]
    }
}

// ============================================================================
//   FBX (binary) reader
// ============================================================================
//
// FBX is the format most 3D tools (Blender, Maya, 3ds Max, SketchUp) export for
// furniture. The common on-disk form is BINARY: a magic header, a `u32` version, then a
// tree of NODE records. Each node = (EndOffset, NumProps, PropsLen, NameLen, Name, props…,
// child nodes…, null-terminator). We walk the tree, pull every `Vertices` (f64 array) and
// `PolygonVertexIndex` (i32 array, polygons terminated by a bit-negated last index), pair
// them in document order, and fan-triangulate into the same triangle soup OBJ/3DS produce.
//
// Array properties may be zlib-DEFLATE compressed (Encoding==1); we inflate with
// `miniz_oxide` (already in the build via image/png). Normals in the file are ignored — we
// compute a flat per-triangle normal, exactly like the OBJ path, so shading always works.
//
// AXIS: FBX defaults to Y-up; the 3D Factory is Z-up. We rotate Y-up→Z-up ((x,y,z)→(x,-z,y))
// so imported furniture stands upright. (UpAxis override in GlobalSettings isn't read yet —
// if a model imports lying down, that's the case to add.)
//
// Robustness: never panics. Any truncation / unknown structure just stops the walk and
// returns whatever triangles were recovered (0 → the caller reports "no triangles").

/// Parse a binary FBX into a triangle soup. ASCII FBX and truncated files yield an empty
/// mesh (the caller then reports "no triangles found").
pub fn parse_fbx(data: &[u8]) -> ObjMesh {
    let mut out = ObjMesh::default();
    // Magic: "Kaydara FBX Binary  \x00\x1a\x00" (23 bytes) + u32 version.
    const MAGIC: &[u8] = b"Kaydara FBX Binary  \x00\x1a\x00";
    if data.len() < 27 || &data[..MAGIC.len()] != MAGIC {
        return out; // not binary FBX (ASCII unsupported)
    }
    let version = u32::from_le_bytes([data[23], data[24], data[25], data[26]]);
    let v75 = version >= 7500; // 7.5+ uses 64-bit node offsets
    let mut verts_list: Vec<Vec<f64>> = Vec::new();
    let mut idx_list: Vec<Vec<i32>> = Vec::new();
    let mut cur = FbxCursor { buf: data, pos: 27 };
    // Top level: sibling nodes until a null record / EOF.
    while let Some(more) = fbx_walk(&mut cur, v75, &mut verts_list, &mut idx_list) {
        if !more { break; }
    }
    // Pair each geometry's vertices with its polygon indices (document order).
    for (verts, indices) in verts_list.iter().zip(idx_list.iter()) {
        fbx_triangulate(verts, indices, &mut out);
    }
    out
}

/// Little-endian byte cursor over the FBX buffer. Every read is bounds-checked; a short
/// read returns `None` so the walk unwinds cleanly instead of panicking.
struct FbxCursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> FbxCursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> { self.take(1).map(|b| b[0]) }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    /// A node offset field: `u64` on FBX 7.5+, else `u32`.
    fn offset(&mut self, v75: bool) -> Option<u64> {
        if v75 { self.u64() } else { self.u32().map(|x| x as u64) }
    }
}

/// Read ONE node record at the cursor. Returns `Some(true)` if a real node was read,
/// `Some(false)` on the null-record terminator (end of a sibling list), `None` on EOF /
/// malformed. Named `Vertices` / `PolygonVertexIndex` arrays are captured into the lists;
/// children are walked recursively.
fn fbx_walk(
    cur: &mut FbxCursor,
    v75: bool,
    verts: &mut Vec<Vec<f64>>,
    idxs: &mut Vec<Vec<i32>>,
) -> Option<bool> {
    let end_offset = cur.offset(v75)?;
    let num_props = cur.offset(v75)?;
    let _prop_len = cur.offset(v75)?;
    let name_len = cur.u8()?;
    // Null record: all-zero header terminates the current sibling list.
    if end_offset == 0 && num_props == 0 && name_len == 0 {
        return Some(false);
    }
    let name = cur.take(name_len as usize)?.to_vec();
    let want = matches!(name.as_slice(), b"Vertices" | b"PolygonVertexIndex");
    // Read every property (must advance the cursor correctly even for ones we ignore).
    let mut got_f64: Option<Vec<f64>> = None;
    let mut got_i32: Option<Vec<i32>> = None;
    for _ in 0..num_props {
        match fbx_property(cur, want)? {
            FbxProp::F64Arr(a) if got_f64.is_none() => got_f64 = Some(a),
            FbxProp::I32Arr(a) if got_i32.is_none() => got_i32 = Some(a),
            _ => {}
        }
    }
    if name.as_slice() == b"Vertices" {
        if let Some(a) = got_f64 { verts.push(a); }
    } else if name.as_slice() == b"PolygonVertexIndex" {
        if let Some(a) = got_i32 { idxs.push(a); }
    }
    // Nested child nodes occupy [cur.pos, end_offset). A null record ends them.
    let end = end_offset as usize;
    while cur.pos < end {
        match fbx_walk(cur, v75, verts, idxs) {
            Some(true) => {}
            _ => break,
        }
    }
    // Jump to the recorded end so trailing padding / unread props never desync the walk.
    if end >= cur.pos && end <= cur.buf.len() {
        cur.pos = end;
    }
    Some(true)
}

/// A decoded FBX property value — we only materialise the array kinds we use.
enum FbxProp {
    F64Arr(Vec<f64>),
    I32Arr(Vec<i32>),
    Skip,
}

/// Read one property, advancing the cursor. When `decode` is false, array payloads are
/// skipped without decoding (cheaper for the arrays we don't want, e.g. Normals/UVs).
fn fbx_property(cur: &mut FbxCursor, decode: bool) -> Option<FbxProp> {
    let ty = cur.u8()?;
    match ty {
        b'Y' => { cur.take(2)?; Some(FbxProp::Skip) }               // i16
        b'C' => { cur.take(1)?; Some(FbxProp::Skip) }               // bool
        b'I' => { cur.take(4)?; Some(FbxProp::Skip) }               // i32
        b'F' => { cur.take(4)?; Some(FbxProp::Skip) }               // f32
        b'D' => { cur.take(8)?; Some(FbxProp::Skip) }               // f64
        b'L' => { cur.take(8)?; Some(FbxProp::Skip) }               // i64
        b'S' | b'R' => { let n = cur.u32()? as usize; cur.take(n)?; Some(FbxProp::Skip) }
        b'f' | b'd' | b'l' | b'i' | b'b' => {
            let len = cur.u32()? as usize;      // element count
            let encoding = cur.u32()?;          // 0 = raw, 1 = zlib
            let comp_len = cur.u32()? as usize; // bytes on disk
            let raw = cur.take(comp_len)?;
            let elem = match ty { b'd' | b'l' => 8, b'f' | b'i' => 4, _ => 1 };
            if !decode || (ty != b'd' && ty != b'i') {
                return Some(FbxProp::Skip); // wrong type for us, or skipping
            }
            let bytes: Vec<u8> = if encoding == 1 {
                match miniz_oxide::inflate::decompress_to_vec_zlib(raw) {
                    Ok(b) => b,
                    Err(_) => return Some(FbxProp::Skip),
                }
            } else {
                raw.to_vec()
            };
            if bytes.len() < len * elem { return Some(FbxProp::Skip); }
            match ty {
                b'd' => Some(FbxProp::F64Arr(
                    bytes.chunks_exact(8).take(len)
                        .map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect(),
                )),
                b'i' => Some(FbxProp::I32Arr(
                    bytes.chunks_exact(4).take(len)
                        .map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect(),
                )),
                _ => Some(FbxProp::Skip),
            }
        }
        _ => None, // unknown property type → bail (walk unwinds)
    }
}

/// Fan-triangulate one FBX geometry (flat `verts` xyz-triples + `PolygonVertexIndex`) into
/// `out`, converting Y-up→Z-up. A negative index is the bit-negated LAST vertex of a
/// polygon (`real = !neg`), so polygons of any size are handled.
fn fbx_triangulate(verts: &[f64], indices: &[i32], out: &mut ObjMesh) {
    let vcount = verts.len() / 3;
    let pos = |i: usize| -> [f32; 3] {
        let (x, y, z) = (verts[3 * i] as f32, verts[3 * i + 1] as f32, verts[3 * i + 2] as f32);
        [x, -z, y] // Y-up → Z-up
    };
    let mut poly: Vec<usize> = Vec::new();
    for &raw in indices {
        let (idx, last) = if raw < 0 { ((!raw) as usize, true) } else { (raw as usize, false) };
        if idx < vcount { poly.push(idx); }
        if last {
            // fan: (0, k, k+1)
            for k in 1..poly.len().saturating_sub(1) {
                let (a, b, c) = (pos(poly[0]), pos(poly[k]), pos(poly[k + 1]));
                let n = flat_normal(a, b, c);
                out.positions.push(a); out.positions.push(b); out.positions.push(c);
                out.normals.push(n); out.normals.push(n); out.normals.push(n);
            }
            poly.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- FBX synthetic-file helpers -----------------------------------------
    fn fbx_prop_array(ty: u8, raw: &[u8], count: usize, compress: bool) -> Vec<u8> {
        let mut p = vec![ty];
        p.extend((count as u32).to_le_bytes());
        if compress {
            let z = miniz_oxide::deflate::compress_to_vec_zlib(raw, 6);
            p.extend(1u32.to_le_bytes());
            p.extend((z.len() as u32).to_le_bytes());
            p.extend(z);
        } else {
            p.extend(0u32.to_le_bytes());
            p.extend((raw.len() as u32).to_le_bytes());
            p.extend_from_slice(raw);
        }
        p
    }
    fn fbx_prop_d(vals: &[f64], compress: bool) -> Vec<u8> {
        let raw: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        fbx_prop_array(b'd', &raw, vals.len(), compress)
    }
    fn fbx_prop_i(vals: &[i32], compress: bool) -> Vec<u8> {
        let raw: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        fbx_prop_array(b'i', &raw, vals.len(), compress)
    }
    fn fbx_node(start: usize, name: &[u8], num_props: u32, props: &[u8], children: &[u8]) -> Vec<u8> {
        let end = start + 13 + name.len() + props.len() + children.len(); // 13 = 4+4+4+1 (32-bit)
        let mut out = Vec::new();
        out.extend((end as u32).to_le_bytes());
        out.extend(num_props.to_le_bytes());
        out.extend((props.len() as u32).to_le_bytes());
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out.extend_from_slice(props);
        out.extend_from_slice(children);
        out
    }
    /// Build a minimal binary FBX with one Geometry (a single triangle), optionally with
    /// the arrays zlib-compressed — exercising the exact path a real exporter uses.
    fn synth_fbx(compress: bool) -> Vec<u8> {
        let magic_len = 27;
        let geo_header = 13 + b"Geometry".len();
        let children_start = magic_len + geo_header; // Geometry has no props
        let vprops = fbx_prop_d(&[0., 0., 0., 1., 0., 0., 0., 1., 0.], compress);
        let vnode = fbx_node(children_start, b"Vertices", 1, &vprops, &[]);
        let iprops = fbx_prop_i(&[0, 1, !2], compress); // !2 = -3 → closes poly (0,1,2)
        let pnode = fbx_node(children_start + vnode.len(), b"PolygonVertexIndex", 1, &iprops, &[]);
        let mut children = Vec::new();
        children.extend_from_slice(&vnode);
        children.extend_from_slice(&pnode);
        children.extend_from_slice(&[0u8; 13]); // null record ends Geometry's children
        let geo = fbx_node(magic_len, b"Geometry", 0, &[], &children);
        let mut buf = Vec::new();
        buf.extend_from_slice(b"Kaydara FBX Binary  \x00\x1a\x00");
        buf.extend(7400u32.to_le_bytes());
        buf.extend_from_slice(&geo);
        buf
    }

    #[test]
    fn fbx_binary_reads_one_triangle_uncompressed() {
        let m = parse_fbx(&synth_fbx(false));
        assert_eq!(m.tri_count(), 1, "one polygon → one triangle");
        // Y-up → Z-up: (0,1,0) becomes (0,0,1).
        assert_eq!(m.positions[2], [0.0, 0.0, 1.0]);
        assert_eq!(m.normals.len(), m.positions.len());
    }

    #[test]
    fn fbx_binary_reads_one_triangle_zlib_compressed() {
        let m = parse_fbx(&synth_fbx(true));
        assert_eq!(m.tri_count(), 1, "compressed arrays inflate to the same triangle");
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn fbx_rejects_non_fbx_bytes() {
        assert_eq!(parse_fbx(b"not an fbx file at all").tri_count(), 0);
    }

    /// A unit tetrahedron: 4 triangular faces → 12 positions (3 per tri).
    #[test]
    fn parses_a_simple_mesh() {
        let obj = "\
# a tetra
v 0 0 0
v 1 0 0
v 0 1 0
v 0 0 1
f 1 2 3
f 1 2 4
f 1 3 4
f 2 3 4
";
        let m = parse_obj(obj);
        assert_eq!(m.tri_count(), 4);
        assert_eq!(m.positions.len(), 12);
        assert_eq!(m.normals.len(), 12, "a normal per vertex, even without vn");
    }

    /// A quad face is fan-triangulated into two triangles.
    #[test]
    fn triangulates_a_quad() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
        assert_eq!(parse_obj(obj).tri_count(), 2);
    }

    /// Negative (relative) indices resolve against the end of the vertex list.
    #[test]
    fn handles_negative_indices() {
        let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n";
        assert_eq!(parse_obj(obj).tri_count(), 1);
    }

    /// `v/vt/vn` references use the vertex-normal when present.
    #[test]
    fn uses_supplied_normals() {
        let obj = "\
v 0 0 0
v 1 0 0
v 0 1 0
vn 0 0 1
f 1//1 2//1 3//1
";
        let m = parse_obj(obj);
        assert_eq!(m.tri_count(), 1);
        assert_eq!(m.normals[0], [0.0, 0.0, 1.0]);
    }

    /// Garbage in, no panic — a malformed face is skipped, valid ones still import.
    #[test]
    fn skips_malformed_faces() {
        let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2\nf 1 2 3\nf 9 9 9\n";
        assert_eq!(parse_obj(obj).tri_count(), 1, "only the valid, in-range face");
    }

    // Helpers to hand-assemble a tiny valid 3DS file for the parser test.
    fn chunk(id: u16, body: &[u8]) -> Vec<u8> {
        let len = (6 + body.len()) as u32;
        let mut v = Vec::new();
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&len.to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    /// A minimal 3DS with one triangle round-trips through the chunk parser.
    #[test]
    fn parses_a_minimal_3ds() {
        // vertex list (0x4110): 3 verts
        let mut verts = Vec::new();
        verts.extend_from_slice(&3u16.to_le_bytes());
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in p { verts.extend_from_slice(&c.to_le_bytes()); }
        }
        let verts_chunk = chunk(0x4110, &verts);
        // face list (0x4120): 1 face (a,b,c,flags)
        let mut faces = Vec::new();
        faces.extend_from_slice(&1u16.to_le_bytes());
        for x in [0u16, 1, 2, 0] { faces.extend_from_slice(&x.to_le_bytes()); }
        let faces_chunk = chunk(0x4120, &faces);

        let mesh = chunk(0x4100, &[verts_chunk, faces_chunk].concat());
        // OBJECT (0x4000) needs a null-terminated name before its sub-chunks.
        let mut obj_body = b"cube\0".to_vec();
        obj_body.extend_from_slice(&mesh);
        let obj = chunk(0x4000, &obj_body);
        let editor = chunk(0x3d3d, &obj);
        let main = chunk(0x4d4d, &editor);

        let m = parse_3ds(&main);
        assert_eq!(m.tri_count(), 1, "the single triangle must survive the chunk walk");
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
    }

    /// Not a 3DS (wrong magic) → empty, no panic.
    #[test]
    fn rejects_non_3ds() {
        assert_eq!(parse_3ds(b"not a 3ds file").tri_count(), 0);
    }
}
