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
    /// Per-vertex OPACITY (1.0 = opaque), parallel to `positions`. EMPTY when the file
    /// declared no transparency at all — the common case, so an opaque mesh carries no
    /// extra bytes and the renderer keeps its fast all-opaque path. When present, glass
    /// panes (OBJ/MTL `d`/`Tr`, glTF `alphaMode:BLEND` + `baseColorFactor.a`) arrive with
    /// their material's opacity so they can be drawn see-through.
    pub alpha: Vec<f32>,
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

/// Parse a companion `.mtl` for per-material OPACITY: material name → opacity in `0..=1`.
/// Reads `d` (dissolve; 1 = opaque) and, as a fallback, `Tr` (transparency; opacity = 1 − Tr).
/// Only opacity is taken — diffuse colour stays an in-app choice (see the module note). Both
/// `d` and `Tr` appear in real exports (e.g. 3ds Max writes both); `d` wins when present.
fn parse_mtl_opacity(text: &str) -> std::collections::HashMap<String, f32> {
    let mut map = std::collections::HashMap::new();
    let mut cur: Option<String> = None;
    let mut seen_d = false; // `d` for the current material takes priority over `Tr`
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("newmtl") => {
                cur = it.next().map(|s| s.to_string());
                seen_d = false;
            }
            Some("d") => {
                if let (Some(name), Some(v)) = (&cur, it.next().and_then(|s| s.parse::<f32>().ok())) {
                    map.insert(name.clone(), v.clamp(0.0, 1.0));
                    seen_d = true;
                }
            }
            Some("Tr") if !seen_d => {
                if let (Some(name), Some(v)) = (&cur, it.next().and_then(|s| s.parse::<f32>().ok())) {
                    map.insert(name.clone(), (1.0 - v).clamp(0.0, 1.0));
                }
            }
            _ => {}
        }
    }
    map
}

/// Parse OBJ text into a triangle soup. Never fails: malformed lines are skipped, so a
/// slightly-off export still imports what it can rather than importing nothing. Materials
/// (`.mtl`) are NOT resolved — colour is an in-app choice and no base directory is known;
/// use [`parse_obj_dir`] to pick up per-material transparency from a companion `.mtl`.
pub fn parse_obj(text: &str) -> ObjMesh {
    parse_obj_dir(text, None)
}

/// Like [`parse_obj`], but `base_dir` lets it resolve the `mtllib` companion file so glass
/// panes keep their transparency: each face inherits its `usemtl` material's opacity (`d`/`Tr`),
/// recorded per-vertex in [`ObjMesh::alpha`]. When every material is opaque (or no `.mtl` is
/// found) `alpha` is left EMPTY, so an ordinary opaque model is byte-for-byte as before.
pub fn parse_obj_dir(text: &str, base_dir: Option<&std::path::Path>) -> ObjMesh {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut norms: Vec<[f32; 3]> = Vec::new();
    let mut out = ObjMesh::default();

    // Load whatever `.mtl` files the OBJ references (`mtllib`), merged into one opacity map.
    let mut opacity: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    if let Some(dir) = base_dir {
        let mut referenced = false;
        let mut resolved = false;
        for line in text.lines() {
            let mut it = line.trim().split_whitespace();
            if it.next() == Some("mtllib") {
                for name in it {
                    referenced = true;
                    if let Ok(mtl) = std::fs::read_to_string(dir.join(name)) {
                        opacity.extend(parse_mtl_opacity(&mtl));
                        resolved = true;
                    }
                }
            }
        }
        // The `mtllib` name often doesn't match the file that actually ships beside the OBJ
        // (renamed/relocated on export — e.g. the bundled window says `…Casement…V1.mtl` but the
        // file is `window.mtl`). Rather than silently lose the glass materials, fall back to every
        // `.mtl` in the same directory when the reference couldn't be resolved.
        if referenced && !resolved {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    let is_mtl = p.extension().and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("mtl")).unwrap_or(false);
                    if is_mtl {
                        if let Ok(mtl) = std::fs::read_to_string(&p) {
                            opacity.extend(parse_mtl_opacity(&mtl));
                        }
                    }
                }
            }
        }
    }
    let mut cur_alpha = 1.0f32; // opacity of the active `usemtl` material

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
            Some("usemtl") => {
                cur_alpha = it.next().and_then(|n| opacity.get(n).copied()).unwrap_or(1.0);
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
                        out.alpha.push(cur_alpha);
                    }
                }
            }
            _ => {} // comments (#), groups (g/o), smoothing — ignored
        }
    }
    trim_alpha(&mut out);
    out
}

/// Drop the per-vertex `alpha` array when every value is (near-)opaque, restoring the
/// "empty ⇒ all opaque" invariant so opaque meshes stay lean and take the fast render path.
fn trim_alpha(m: &mut ObjMesh) {
    if m.alpha.iter().all(|&a| a >= 0.996) {
        m.alpha.clear();
    }
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
/// Diagnostics from an FBX parse — surfaced in the import recorder event so a broken import
/// ("mutilated" / "not recognized" / a blob) is DEBUGGABLE from a dump instead of guesswork.
#[derive(Clone, Debug, Default)]
pub struct FbxInfo {
    /// True when the file is NOT binary FBX (ASCII or not-FBX). We parse binary only, so an
    /// ASCII file yields no geometry — this makes the "not recognized" case explicit.
    pub ascii: bool,
    pub version: u32,
    /// Number of `Vertices` arrays found (one per geometry). 0 = nothing recognised.
    pub geometries: usize,
    pub total_verts: usize,
    pub total_indices: usize,
}

pub fn parse_fbx(data: &[u8]) -> ObjMesh {
    parse_fbx_ex(data).0
}

/// Binary-FBX parse that also returns [`FbxInfo`] diagnostics.
///
/// KNOWN LIMITATIONS (each a likely cause of a wrong-looking import — report a sample file to
/// fix a specific one): ASCII FBX unsupported; per-node `Lcl Translation/Rotation/Scaling`
/// transforms are NOT applied (multi-part meshes can collapse); the axis conversion is a fixed
/// Y-up→Z-up (a Z-up export comes in rotated); `UnitScaleFactor` is ignored.
pub fn parse_fbx_ex(data: &[u8]) -> (ObjMesh, FbxInfo) {
    parse_fbx_inner(data, false)
}

/// Parse a binary FBX intended as an APERTURE DOOR — like [`parse_fbx_ex`] but drops a solid,
/// full-depth surround shell (see [`fbx_build_scene_opt`]). The bundled door model wraps its leaf
/// in a frame whose solid back is an opaque grey slab from behind; the wall opening is the real
/// frame, so dropping the shell makes the door read correctly from BOTH faces.
pub fn parse_fbx_door(data: &[u8]) -> (ObjMesh, FbxInfo) {
    parse_fbx_inner(data, true)
}

fn parse_fbx_inner(data: &[u8], drop_full_depth_shell: bool) -> (ObjMesh, FbxInfo) {
    let mut out = ObjMesh::default();
    let mut info = FbxInfo::default();
    // Magic: "Kaydara FBX Binary  \x00\x1a\x00" (23 bytes) + u32 version.
    const MAGIC: &[u8] = b"Kaydara FBX Binary  \x00\x1a\x00";
    if data.len() < 27 || &data[..MAGIC.len()] != MAGIC {
        info.ascii = true; // not binary FBX (ASCII unsupported)
        return (out, info);
    }
    let version = u32::from_le_bytes([data[23], data[24], data[25], data[26]]);
    info.version = version;
    let v75 = version >= 7500; // 7.5+ uses 64-bit node offsets
    let mut cur = FbxCursor { buf: data, pos: 27 };
    // Parse the whole node tree, then interpret it (geometries + Model transforms +
    // Connections), so each part lands at its real world pose instead of collapsing at origin.
    let root = fbx_parse_siblings(&mut cur, v75, data.len());
    let (mesh, g, tv, ti) = fbx_build_scene_opt(&root, drop_full_depth_shell);
    out = mesh;
    info.geometries = g;
    info.total_verts = tv;
    info.total_indices = ti;
    (out, info)
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

/// One decoded FBX property value.
enum FbxVal {
    I(i64),
    F(f64),
    S(String),
    /// A raw byte blob (`R`). This is how an FBX carries an EMBEDDED texture: `Video > Content`
    /// holds the whole PNG/JPEG. Skipping it was why binary FBX could never show an image.
    Raw(Vec<u8>),
    Fa(Vec<f64>),
    Ia(Vec<i32>),
    Skip,
}

/// A parsed FBX node: name, its property values, and its child nodes.
struct FbxNode {
    name: String,
    props: Vec<FbxVal>,
    children: Vec<FbxNode>,
}

impl FbxNode {
    fn child(&self, name: &str) -> Option<&FbxNode> {
        self.children.iter().find(|c| c.name == name)
    }
    fn i64_at(&self, i: usize) -> Option<i64> {
        match self.props.get(i)? { FbxVal::I(x) => Some(*x), _ => None }
    }
    fn str_at(&self, i: usize) -> Option<&str> {
        match self.props.get(i)? { FbxVal::S(s) => Some(s.as_str()), _ => None }
    }
    /// The last F64 property value — an FBX `P` scalar (e.g. `Opacity`) puts its number last.
    fn last_f64(&self) -> Option<f64> {
        self.props.iter().rev().find_map(|p| if let FbxVal::F(x) = p { Some(*x) } else { None })
    }
    /// The last three F64 property values (FBX `P` vectors put x,y,z at the tail).
    fn f64_tail3(&self) -> [f64; 3] {
        let fs: Vec<f64> = self.props.iter()
            .filter_map(|p| if let FbxVal::F(x) = p { Some(*x) } else { None })
            .collect();
        let n = fs.len();
        if n >= 3 { [fs[n - 3], fs[n - 2], fs[n - 1]] } else { [0.0; 3] }
    }
    fn first_f64_arr(&self) -> Option<&Vec<f64>> {
        self.props.iter().find_map(|p| if let FbxVal::Fa(a) = p { Some(a) } else { None })
    }
    fn first_i32_arr(&self) -> Option<&Vec<i32>> {
        self.props.iter().find_map(|p| if let FbxVal::Ia(a) = p { Some(a) } else { None })
    }
    /// The first raw blob property — an embedded texture's image bytes (`Video > Content`).
    fn first_raw(&self) -> Option<&[u8]> {
        self.props.iter().find_map(|p| if let FbxVal::Raw(b) = p { Some(b.as_slice()) } else { None })
    }
    /// The string value of the first child named in `names` that carries a non-empty one.
    fn str_child(&self, names: &[&str]) -> Option<&str> {
        names.iter().find_map(|k| self.child(k).and_then(|c| c.str_at(0)).filter(|s| !s.is_empty()))
    }
}

/// Parse a run of sibling nodes until a null-record terminator, EOF, or `end`.
fn fbx_parse_siblings(cur: &mut FbxCursor, v75: bool, end: usize) -> Vec<FbxNode> {
    let mut out = Vec::new();
    while cur.pos < end {
        match fbx_parse_node(cur, v75) {
            Some(Some(n)) => out.push(n),
            _ => break, // null record or malformed → end of this list
        }
    }
    out
}

/// Parse ONE node. `Some(Some(node))` = a real node; `Some(None)` = the null-record
/// terminator; `None` = EOF / malformed (unwinds the parse).
fn fbx_parse_node(cur: &mut FbxCursor, v75: bool) -> Option<Option<FbxNode>> {
    let end_offset = cur.offset(v75)? as usize;
    let num_props = cur.offset(v75)?;
    let _prop_len = cur.offset(v75)?;
    let name_len = cur.u8()?;
    if end_offset == 0 && num_props == 0 && name_len == 0 {
        return Some(None); // null record ends the sibling list
    }
    let name = String::from_utf8_lossy(cur.take(name_len as usize)?).into_owned();
    let mut props = Vec::new();
    for _ in 0..num_props {
        props.push(fbx_value(cur)?);
    }
    let mut children = Vec::new();
    if cur.pos < end_offset {
        children = fbx_parse_siblings(cur, v75, end_offset);
    }
    // Jump to the recorded end so trailing padding / unread bytes never desync the parse.
    if end_offset >= cur.pos && end_offset <= cur.buf.len() {
        cur.pos = end_offset;
    }
    Some(Some(FbxNode { name, props, children }))
}

/// Read one property value, advancing the cursor. Scalars, strings and raw blobs are always
/// decoded (cheap, and the blob is an embedded texture); `d`/`f`/`i` arrays are decoded (verts,
/// indices, UVs, material indices); `l`/`b` arrays are skipped without materialising.
fn fbx_value(cur: &mut FbxCursor) -> Option<FbxVal> {
    let ty = cur.u8()?;
    Some(match ty {
        b'Y' => { let b = cur.take(2)?; FbxVal::I(i16::from_le_bytes([b[0], b[1]]) as i64) }
        b'C' => FbxVal::I(cur.u8()? as i64),
        b'I' => { let b = cur.take(4)?; FbxVal::I(i32::from_le_bytes(b.try_into().unwrap()) as i64) }
        b'L' => { let b = cur.take(8)?; FbxVal::I(i64::from_le_bytes(b.try_into().unwrap())) }
        b'F' => { let b = cur.take(4)?; FbxVal::F(f32::from_le_bytes(b.try_into().unwrap()) as f64) }
        b'D' => { let b = cur.take(8)?; FbxVal::F(f64::from_le_bytes(b.try_into().unwrap())) }
        b'S' | b'R' => {
            let n = cur.u32()? as usize;
            let b = cur.take(n)?;
            if ty == b'S' { FbxVal::S(String::from_utf8_lossy(b).into_owned()) } else { FbxVal::Raw(b.to_vec()) }
        }
        b'f' | b'd' | b'l' | b'i' | b'b' => {
            let len = cur.u32()? as usize;
            let encoding = cur.u32()?;
            let comp_len = cur.u32()? as usize;
            let raw = cur.take(comp_len)?;
            let bytes: Vec<u8> = if encoding == 1 {
                match miniz_oxide::inflate::decompress_to_vec_zlib(raw) {
                    Ok(b) => b,
                    Err(_) => return Some(FbxVal::Skip),
                }
            } else {
                raw.to_vec()
            };
            match ty {
                b'd' if bytes.len() >= len * 8 => FbxVal::Fa(
                    bytes.chunks_exact(8).take(len).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect(),
                ),
                // f32 arrays widen to the same Fa. Blender writes UVs as `d`, but the Autodesk SDK
                // (Max/Maya) writes some layers as `f` — skipping those lost their UVs entirely.
                b'f' if bytes.len() >= len * 4 => FbxVal::Fa(
                    bytes.chunks_exact(4).take(len).map(|c| f32::from_le_bytes(c.try_into().unwrap()) as f64).collect(),
                ),
                b'i' if bytes.len() >= len * 4 => FbxVal::Ia(
                    bytes.chunks_exact(4).take(len).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect(),
                ),
                _ => FbxVal::Skip,
            }
        }
        _ => return None, // unknown property type
    })
}

/// A Euler rotation matrix (degrees) in an FBX rotation order. FBX applies the axes in the
/// listed order about FIXED axes, so for order XYZ the matrix is `Rz·Ry·Rx` (X applied first,
/// i.e. rightmost) — the same convention three.js/Blender use for FBX. Using glam's intrinsic
/// `EulerRot` here instead reverses multi-axis parts (a couch backrest ends up standing up);
/// single-axis rotations (Table & Chairs) are unaffected either way.
fn fbx_euler(order: i64, r_deg: [f64; 3]) -> glam::DMat4 {
    use glam::DMat4 as M;
    let rx = M::from_rotation_x(r_deg[0].to_radians());
    let ry = M::from_rotation_y(r_deg[1].to_radians());
    let rz = M::from_rotation_z(r_deg[2].to_radians());
    match order {
        1 => ry * rz * rx, // XZY
        2 => rx * rz * ry, // YZX
        3 => rz * rx * ry, // YXZ
        4 => ry * rx * rz, // ZXY
        5 => rx * ry * rz, // ZYX
        _ => rz * ry * rx, // XYZ (default)
    }
}

/// All the transform components a Model node can carry. Defaults are identity.
#[derive(Default)]
struct FbxXform {
    t: [f64; 3],
    r: [f64; 3],
    s: [f64; 3],
    pre_r: [f64; 3],
    post_r: [f64; 3],
    r_off: [f64; 3],
    r_piv: [f64; 3],
    s_off: [f64; 3],
    s_piv: [f64; 3],
    order: i64,
}

/// The FBX local transform, per the FBX SDK formula:
///   T · Roff · Rp · Rpre · R · Rpost⁻¹ · Rp⁻¹ · Soff · Sp · S · Sp⁻¹
/// PreRotation is what makes a part "stand up" if dropped — the common furniture case.
fn fbx_local_matrix(x: &FbxXform) -> glam::DMat4 {
    use glam::{DMat4 as M, DVec3 as V};
    let tr = |v: [f64; 3]| M::from_translation(V::from(v));
    let tri = |v: [f64; 3]| M::from_translation(-V::from(v));
    let r = fbx_euler(x.order, x.r);
    let rpre = fbx_euler(0, x.pre_r);          // pre/post rotation are always XYZ order
    let rpost = fbx_euler(0, x.post_r).inverse();
    let s = M::from_scale(V::from(x.s));
    tr(x.t) * tr(x.r_off) * tr(x.r_piv) * rpre * r * rpost * tri(x.r_piv)
        * tr(x.s_off) * tr(x.s_piv) * s * tri(x.s_piv)
}

/// Simple TRS (translate·rotate·scale, XYZ) — used for the geometric transform, which has no
/// pivots/offsets in the FBX spec.
fn fbx_trs(t: [f64; 3], r_deg: [f64; 3], s: [f64; 3]) -> glam::DMat4 {
    glam::DMat4::from_translation(glam::DVec3::from(t)) * fbx_euler(0, r_deg) * glam::DMat4::from_scale(glam::DVec3::from(s))
}

/// Recursively collect the pieces we need from the node tree: every `Geometry`'s
/// verts+indices (+ its id), every `Model`'s local & geometric transforms by id, the
/// `Connections` child→parent map, and the scene up-axis.
#[allow(clippy::type_complexity)]
fn fbx_scan<'a>(
    nodes: &'a [FbxNode],
    geoms: &mut Vec<(Option<i64>, &'a Vec<f64>, &'a Vec<i32>)>,
    models: &mut std::collections::HashMap<i64, (glam::DMat4, glam::DMat4)>,
    parent: &mut std::collections::HashMap<i64, i64>,
    up_axis: &mut i64,
    // Centimetres per file unit, from `GlobalSettings > UnitScaleFactor`. Metres = unit * this / 100.
    unit_cm: &mut f64,
) {
    for n in nodes {
        match n.name.as_str() {
            "Geometry" => {
                if let (Some(v), Some(i)) = (
                    n.child("Vertices").and_then(|c| c.first_f64_arr()),
                    n.child("PolygonVertexIndex").and_then(|c| c.first_i32_arr()),
                ) {
                    geoms.push((n.i64_at(0), v, i));
                }
            }
            "Model" => {
                if let Some(id) = n.i64_at(0) {
                    let mut x = FbxXform { s: [1.0; 3], ..Default::default() };
                    let (mut gt, mut gr, mut gs) = ([0.0; 3], [0.0; 3], [1.0; 3]);
                    if let Some(p70) = n.child("Properties70") {
                        for c in &p70.children {
                            if c.name != "P" { continue; }
                            match c.str_at(0) {
                                Some("Lcl Translation") => x.t = c.f64_tail3(),
                                Some("Lcl Rotation") => x.r = c.f64_tail3(),
                                Some("Lcl Scaling") => x.s = c.f64_tail3(),
                                Some("PreRotation") => x.pre_r = c.f64_tail3(),
                                Some("PostRotation") => x.post_r = c.f64_tail3(),
                                Some("RotationOffset") => x.r_off = c.f64_tail3(),
                                Some("RotationPivot") => x.r_piv = c.f64_tail3(),
                                Some("ScalingOffset") => x.s_off = c.f64_tail3(),
                                Some("ScalingPivot") => x.s_piv = c.f64_tail3(),
                                Some("RotationOrder") => {
                                    if let Some(FbxVal::I(o)) = c.props.last() { x.order = *o; }
                                }
                                Some("GeometricTranslation") => gt = c.f64_tail3(),
                                Some("GeometricRotation") => gr = c.f64_tail3(),
                                Some("GeometricScaling") => gs = c.f64_tail3(),
                                _ => {}
                            }
                        }
                    }
                    if std::env::var("RUSTCAD_FBX_DEBUG").is_ok()
                        && (x.r != [0.0; 3] || x.pre_r != [0.0; 3] || x.post_r != [0.0; 3] || x.order != 0)
                    {
                        eprintln!(
                            "  MODEL id={id} order={} lcl_r=({:.1},{:.1},{:.1}) pre_r=({:.1},{:.1},{:.1}) post_r=({:.1},{:.1},{:.1})",
                            x.order, x.r[0], x.r[1], x.r[2], x.pre_r[0], x.pre_r[1], x.pre_r[2], x.post_r[0], x.post_r[1], x.post_r[2],
                        );
                    }
                    models.insert(id, (fbx_local_matrix(&x), fbx_trs(gt, gr, gs)));
                }
            }
            "C" => {
                // Connection: C: "OO", childId, parentId (source connects to dest).
                if n.str_at(0) == Some("OO") {
                    if let (Some(child), Some(par)) = (n.i64_at(1), n.i64_at(2)) {
                        parent.entry(child).or_insert(par);
                    }
                }
            }
            "P" if n.str_at(0) == Some("UpAxis") => {
                if let Some(FbxVal::I(x)) = n.props.last() { *up_axis = *x; }
            }
            // FBX measures in CENTIMETRES: `UnitScaleFactor` is how many centimetres one file unit
            // is. Ignoring it meant every FBX imported 100x too big — invisible for furniture,
            // which is normalised to prop size, and glaring the moment something has to sit at TRUE
            // scale, like a door handle.
            "P" if n.str_at(0) == Some("UnitScaleFactor") => {
                if let Some(u) = n.last_f64() {
                    if u > 1e-9 { *unit_cm = u; }
                }
            }
            _ => {}
        }
        fbx_scan(&n.children, geoms, models, parent, up_axis, unit_cm);
    }
}

/// World matrix of a Model = its local matrix composed up the Model→Model parent chain.
fn fbx_model_world(
    id: i64,
    models: &std::collections::HashMap<i64, (glam::DMat4, glam::DMat4)>,
    parent: &std::collections::HashMap<i64, i64>,
) -> glam::DMat4 {
    let mut m = models.get(&id).map(|x| x.0).unwrap_or(glam::DMat4::IDENTITY);
    let mut cur = id;
    for _ in 0..256 {
        // Walk up while the parent is ALSO a Model (stop at the scene root).
        match parent.get(&cur) {
            Some(&p) if models.contains_key(&p) => {
                m = models[&p].0 * m;
                cur = p;
            }
            _ => break,
        }
    }
    m
}

/// Interpret the parsed tree into a world-space triangle mesh. Each geometry is placed by the
/// Model it connects to (translation/rotation/scale + geometric offset); a geometry with no
/// Model is left at identity. Coordinates are converted to the app's Z-up per the scene UpAxis.
fn fbx_build_scene(root: &[FbxNode]) -> (ObjMesh, usize, usize, usize) {
    fbx_build_scene_opt(root, false)
}

/// As [`fbx_build_scene`], but when `drop_full_depth_shell` is set, any geometry that spans
/// (nearly) the whole mesh's THINNEST dimension is skipped. That thin dimension is the "depth"
/// of a flat object like a door; the only part spanning all of it is a solid surround/backing
/// shell (the bundled door's frame), which reads as an opaque grey block from behind. The wall
/// opening provides the real frame, so dropping it lets the door read correctly from both sides.
/// ONLY used for the aperture door — ordinary furniture/window import passes `false`.
fn fbx_build_scene_opt(root: &[FbxNode], drop_full_depth_shell: bool) -> (ObjMesh, usize, usize, usize) {
    let mut out = ObjMesh::default();
    let mut geoms: Vec<(Option<i64>, &Vec<f64>, &Vec<i32>)> = Vec::new();
    let mut models = std::collections::HashMap::new();
    let mut parent = std::collections::HashMap::new();
    let mut up_axis: i64 = 1; // FBX default Y-up
    let mut unit_cm: f64 = 1.0; // centimetres per file unit
    fbx_scan(root, &mut geoms, &mut models, &mut parent, &mut up_axis, &mut unit_cm);

    // Resolve each geometry's world transform up front (needed twice: for the shell test and to emit).
    let world_of = |gid: &Option<i64>| -> glam::DMat4 {
        let mid = gid.and_then(|g| parent.get(&g)).copied();
        match mid {
            Some(mid) if models.contains_key(&mid) => fbx_model_world(mid, &models, &parent) * models[&mid].1,
            _ => glam::DMat4::IDENTITY,
        }
    };
    // Per-geometry world AABB, and the overall AABB, so we can find the thin (depth) axis.
    let boxes: Vec<([f64; 3], [f64; 3])> = geoms.iter().map(|(gid, verts, _)| {
        let world = world_of(gid);
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for c in verts.chunks_exact(3) {
            let w = world.transform_point3(glam::DVec3::new(c[0], c[1], c[2]));
            for (k, v) in [w.x, w.y, w.z].into_iter().enumerate() { lo[k] = lo[k].min(v); hi[k] = hi[k].max(v); }
        }
        (lo, hi)
    }).collect();
    let (mut olo, mut ohi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for (lo, hi) in &boxes {
        for k in 0..3 { if lo[k].is_finite() { olo[k] = olo[k].min(lo[k]); ohi[k] = ohi[k].max(hi[k]); } }
    }
    let osize = [ohi[0]-olo[0], ohi[1]-olo[1], ohi[2]-olo[2]];
    let thin = (0..3).min_by(|&a, &b| osize[a].partial_cmp(&osize[b]).unwrap()).unwrap_or(2);
    let shell_cut = osize[thin] * 0.85; // a part spanning ≥85% of the depth is the surround shell

    let n_geoms = geoms.len();
    let mut tv = 0usize;
    let mut ti = 0usize;
    let dbg = std::env::var("RUSTCAD_FBX_DEBUG").is_ok();
    for (i, (gid, verts, indices)) in geoms.iter().enumerate() {
        tv += verts.len() / 3;
        ti += indices.len();
        let world = world_of(gid);
        // Drop the full-depth surround shell for a door aperture.
        if drop_full_depth_shell {
            let (lo, hi) = boxes[i];
            if osize[thin] > 1e-6 && (hi[thin] - lo[thin]) >= shell_cut {
                if dbg { eprintln!("  DROP shell geom id={:?} (spans full depth)", gid); }
                continue;
            }
        }
        if dbg {
            // Local bbox of this part + its placement, to spot a mis-transformed piece.
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for c in verts.chunks_exact(3) {
                for k in 0..3 { lo[k] = lo[k].min(c[k]); hi[k] = hi[k].max(c[k]); }
            }
            let (mut wlo, mut whi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for c in verts.chunks_exact(3) {
                let w = world.transform_point3(glam::DVec3::new(c[0], c[1], c[2]));
                for (k, v) in [w.x, w.y, w.z].into_iter().enumerate() { wlo[k]=wlo[k].min(v); whi[k]=whi[k].max(v); }
            }
            eprintln!(
                "  geom id={:?} verts={} local=({:.0},{:.0},{:.0}) world[X {:.0}..{:.0} Y {:.0}..{:.0} Z {:.0}..{:.0}]",
                gid, verts.len() / 3,
                hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2],
                wlo[0],whi[0], wlo[1],whi[1], wlo[2],whi[2],
            );
        }
        fbx_emit_geometry(verts, indices, &world, up_axis, unit_cm, &mut out);
    }
    (out, n_geoms, tv, ti)
}

/// Fan-triangulate one geometry, transforming each vertex by `world` and converting to Z-up.
/// A negative `PolygonVertexIndex` is the bit-negated LAST vertex of a polygon (`real = !neg`).
/// `unit_cm` is centimetres per file unit (`UnitScaleFactor`); metres = unit * unit_cm / 100.
fn fbx_emit_geometry(verts: &[f64], indices: &[i32], world: &glam::DMat4, up_axis: i64, unit_cm: f64, out: &mut ObjMesh) {
    let to_m = unit_cm / 100.0;
    let vcount = verts.len() / 3;
    let pos = |i: usize| -> [f32; 3] {
        let w = world.transform_point3(glam::DVec3::new(verts[3 * i], verts[3 * i + 1], verts[3 * i + 2]));
        // Scene up-axis → app Z-up. UpAxis 1=Y (default) rotates; 2=Z is already correct.
        let v = if up_axis == 2 { w } else { glam::DVec3::new(w.x, -w.z, w.y) };
        [(v.x * to_m) as f32, (v.y * to_m) as f32, (v.z * to_m) as f32]
    };
    let mut poly: Vec<usize> = Vec::new();
    for &raw in indices {
        let (idx, last) = if raw < 0 { ((!raw) as usize, true) } else { (raw as usize, false) };
        if idx < vcount { poly.push(idx); }
        if last {
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

/// Recursively collect every `Material` node's id → diffuse (base) colour (linear RGB) and its
/// OPACITY in `0..=1` (1 = opaque). Opacity comes from the Principled/BSDF alpha, which Blender's
/// FBX exporter writes as `Opacity` (alpha directly) and/or `TransparencyFactor` (1 − alpha); we
/// prefer `Opacity` when present so glass panes (e.g. villa glass, alpha 0.10) import see-through.
fn fbx_collect_materials(
    nodes: &[FbxNode],
    out: &mut std::collections::HashMap<i64, [f32; 3]>,
    opac: &mut std::collections::HashMap<i64, f32>,
) {
    for n in nodes {
        if n.name == "Material" {
            if let Some(id) = n.i64_at(0) {
                let mut diff = [0.7f32, 0.7, 0.7];
                let mut opacity = 1.0f32;
                let mut have_opacity = false; // `Opacity` wins over `TransparencyFactor`
                if let Some(p70) = n.child("Properties70") {
                    for p in &p70.children {
                        if p.name == "P" {
                            match p.str_at(0) {
                                Some("DiffuseColor") | Some("Diffuse") => {
                                    let d = p.f64_tail3();
                                    diff = [d[0] as f32, d[1] as f32, d[2] as f32];
                                }
                                Some("Opacity") => {
                                    if let Some(o) = p.last_f64() {
                                        opacity = o as f32;
                                        have_opacity = true;
                                    }
                                }
                                Some("TransparencyFactor") => {
                                    if !have_opacity {
                                        if let Some(t) = p.last_f64() {
                                            opacity = 1.0 - t as f32;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                out.insert(id, diff);
                opac.insert(id, opacity.clamp(0.0, 1.0));
            }
        }
        fbx_collect_materials(&n.children, out, opac);
    }
}

/// Everything the textured binary-FBX path needs that [`fbx_scan`] doesn't carry: the Geometry
/// NODES themselves (so their UV and material layers can be read), the Texture/Video image
/// objects, and the FULL connection list.
///
/// The connection list matters because [`fbx_scan`] keeps only `OO` links, and a texture is bound
/// to a material by an `OP` (object→property) link naming the property it drives —
/// `C: "OP", texture, material, "DiffuseColor"`. Without `OP` there is no material→image edge at
/// all, which is the second reason binary FBX never showed a texture.
#[derive(Default)]
struct FbxPbrScan<'a> {
    geoms: Vec<&'a FbxNode>,
    tex_ids: std::collections::HashSet<i64>,
    vid_ids: std::collections::HashSet<i64>,
    tex_file: std::collections::HashMap<i64, String>,
    vid_file: std::collections::HashMap<i64, String>,
    /// Video id → the embedded image bytes (borrowed from the parsed tree, never copied).
    vid_content: std::collections::HashMap<i64, &'a [u8]>,
    /// (kind, source id, destination id, property name) in file order — the order IS the
    /// material-slot order for material→model links.
    conns: Vec<(&'a str, i64, i64, Option<&'a str>)>,
}

/// FBX material properties a texture can be bound to that are NOT the base colour. Lower-case,
/// matched as substrings against the `OP` connection's property name (`"NormalMap"`,
/// `"Maya|specularColor"`, `"3dsMax|Parameters|bump_map"`, …).
const NON_COLOUR_MAPS: &[&str] = &[
    "normal", "bump", "specular", "shininess", "reflect", "displacement", "emissive",
    "transparen", "ambient", "occlusion", "roughness", "metal", "gloss", "opacity", "vector",
];

/// The filename a Texture/Video node carries. `RelativeFilename` is tried FIRST because it is
/// relative to the FBX itself, so it still resolves after the file leaves the machine that wrote
/// it — the absolute `FileName` almost never does.
fn fbx_tex_filename(n: &FbxNode) -> Option<&str> {
    n.str_child(&["RelativeFilename", "FileName", "Filename"])
}

fn fbx_scan_pbr<'a>(nodes: &'a [FbxNode], s: &mut FbxPbrScan<'a>) {
    for n in nodes {
        match n.name.as_str() {
            "Geometry" if n.child("Vertices").is_some() => s.geoms.push(n),
            "Texture" => {
                if let Some(id) = n.i64_at(0) {
                    s.tex_ids.insert(id);
                    if let Some(f) = fbx_tex_filename(n) {
                        s.tex_file.insert(id, f.to_string());
                    }
                }
            }
            "Video" => {
                if let Some(id) = n.i64_at(0) {
                    s.vid_ids.insert(id);
                    if let Some(f) = fbx_tex_filename(n) {
                        s.vid_file.insert(id, f.to_string());
                    }
                    // An un-embedded Video still writes a `Content` node — an EMPTY blob. Requiring
                    // a plausible header length keeps that from masking the on-disk file.
                    if let Some(c) = n.child("Content").and_then(|c| c.first_raw()) {
                        if c.len() > 16 {
                            s.vid_content.insert(id, c);
                        }
                    }
                }
            }
            "C" => {
                if let (Some(k), Some(a), Some(b)) = (n.str_at(0), n.i64_at(1), n.i64_at(2)) {
                    s.conns.push((k, a, b, n.str_at(3)));
                }
            }
            _ => {}
        }
        fbx_scan_pbr(&n.children, s);
    }
}

/// One geometry's UV layer: the coordinates, the index array, and the two mapping flags that say
/// how to read them. Missing layer ⇒ empty coordinates, and the caller emits `[0,0]`.
struct FbxUvLayer<'a> {
    uvs: &'a [f64],
    index: &'a [i32],
    /// `IndexToDirect`: look the coordinate up through `index` rather than reading it in order.
    indexed: bool,
    /// `ByVertice`: one UV per CONTROL POINT, not per polygon-vertex.
    by_vertex: bool,
}

fn fbx_uv_layer(g: &FbxNode) -> FbxUvLayer<'_> {
    const NONE_F: &[f64] = &[];
    const NONE_I: &[i32] = &[];
    let n = match g.child("LayerElementUV") {
        Some(n) => n,
        None => return FbxUvLayer { uvs: NONE_F, index: NONE_I, indexed: false, by_vertex: false },
    };
    let mode = |k: &str| n.child(k).and_then(|c| c.str_at(0)).unwrap_or("");
    FbxUvLayer {
        uvs: n.child("UV").and_then(|c| c.first_f64_arr()).map(|v| v.as_slice()).unwrap_or(NONE_F),
        index: n.child("UVIndex").and_then(|c| c.first_i32_arr()).map(|v| v.as_slice()).unwrap_or(NONE_I),
        indexed: mode("ReferenceInformationType").contains("IndexToDirect"),
        by_vertex: {
            let m = mode("MappingInformationType");
            m.contains("ByVertice") || m.contains("ByVertex")
        },
    }
}

/// One geometry's per-polygon material indices, and whether the whole geometry shares one
/// material (`AllSame`). These index the OWNING MODEL's material slots, not global ids.
fn fbx_material_layer(g: &FbxNode) -> (&[i32], bool) {
    const NONE_I: &[i32] = &[];
    let n = match g.child("LayerElementMaterial") {
        Some(n) => n,
        None => return (NONE_I, true),
    };
    let all_same = n
        .child("MappingInformationType")
        .and_then(|c| c.str_at(0))
        .map(|m| m.contains("AllSame"))
        .unwrap_or(false);
    let m = n.child("Materials").and_then(|c| c.first_i32_arr()).map(|v| v.as_slice()).unwrap_or(NONE_I);
    (m, all_same || m.len() <= 1)
}

/// Parse a BINARY FBX into geometry PLUS its per-material colours, returned through the same
/// [`GltfPbr`] channel the multi-material glTF importer uses — so each material becomes its own
/// selectable/paintable part carrying its own appearance.
///
/// A material's base colour is, in order of preference: its EMBEDDED image (`Video > Content`),
/// the image FILE it names (resolved beside the FBX, hence `base`), or a 1×1 swatch of its diffuse
/// colour. UVs come from `LayerElementUV` and are emitted per vertex, so a textured FBX maps its
/// image the way it was authored instead of falling back to box projection.
///
/// Parts are split per (geometry, material slot) via `LayerElementMaterial`, so one mesh carrying
/// several materials — the normal case for anything modelled as a single object — no longer
/// collapses onto whichever material happened to be connected first.
///
/// Empty `GltfPbr` for ASCII / material-less files.
pub fn parse_fbx_pbr(data: &[u8]) -> (ObjMesh, GltfPbr) {
    parse_fbx_pbr_at(data, None)
}

/// [`parse_fbx_pbr`] with the FBX's own directory, so textures stored BESIDE the file (the usual
/// `model.fbx` + `model.fbm/` or `textures/` layout) resolve. Without it only embedded images work.
pub fn parse_fbx_pbr_at(data: &[u8], base: Option<&std::path::Path>) -> (ObjMesh, GltfPbr) {
    use std::collections::HashMap;
    let mut out = ObjMesh::default();
    let mut pbr = GltfPbr::default();
    if is_ascii_fbx(data) || data.len() < 27 {
        return (out, pbr);
    }
    let version = u32::from_le_bytes([data[23], data[24], data[25], data[26]]);
    let v75 = version >= 7500;
    let mut cur = FbxCursor { buf: data, pos: 27 };
    let root = fbx_parse_siblings(&mut cur, v75, data.len());

    // `fbx_scan` for the transform graph; `fbx_scan_pbr` for the appearance graph.
    let mut geoms_vi: Vec<(Option<i64>, &Vec<f64>, &Vec<i32>)> = Vec::new();
    let mut models = HashMap::new();
    let mut parent = HashMap::new();
    let mut up_axis: i64 = 1;
    let mut unit_cm: f64 = 1.0;
    fbx_scan(&root, &mut geoms_vi, &mut models, &mut parent, &mut up_axis, &mut unit_cm);

    let mut mat_color: HashMap<i64, [f32; 3]> = HashMap::new();
    let mut mat_opac: HashMap<i64, f32> = HashMap::new();
    fbx_collect_materials(&root, &mut mat_color, &mut mat_opac);

    let mut sc = FbxPbrScan::default();
    fbx_scan_pbr(&root, &mut sc);

    // ---- resolve the appearance graph: model → materials → texture → video ----
    let mut model_mats: HashMap<i64, Vec<i64>> = HashMap::new(); // ORDER = material slot order
    let mut mat_tex: HashMap<i64, i64> = HashMap::new(); // named base-colour texture
    let mut mat_tex_alt: HashMap<i64, i64> = HashMap::new(); // plausible fallback (see below)
    let mut tex_video: HashMap<i64, i64> = HashMap::new();
    for &(kind, child, par, prop) in &sc.conns {
        if mat_color.contains_key(&child) && models.contains_key(&par) {
            let slots = model_mats.entry(par).or_default();
            if !slots.contains(&child) {
                slots.push(child);
            }
        } else if sc.tex_ids.contains(&child) && mat_color.contains_key(&par) {
            // An `OP` link names the material property it feeds, and that name is the only
            // evidence of what the image IS. Binding a normal or specular map as base colour is
            // exactly how a model ends up looking like flat lilac or tinfoil, so the non-colour
            // properties are excluded outright — a material wearing only a normal map falls back
            // to its diffuse colour rather than to a picture of its bumps.
            let p = prop.unwrap_or("").to_ascii_lowercase();
            let named_colour =
                p.contains("diffusecolor") || p.contains("basecolor") || p.contains("base_color");
            let not_colour = NON_COLOUR_MAPS.iter().any(|k| p.contains(k));
            if named_colour {
                mat_tex.entry(par).or_insert(child);
            } else if !not_colour {
                // Unknown property, or a plain `OO` link with no property at all: the exporter's
                // only texture link, so it is the best base-colour candidate available.
                let _ = kind;
                mat_tex_alt.entry(par).or_insert(child);
            }
        } else if sc.vid_ids.contains(&child) && sc.tex_ids.contains(&par) {
            tex_video.entry(par).or_insert(child);
        }
    }

    let world_of = |gid: &Option<i64>| -> glam::DMat4 {
        let mid = gid.and_then(|g| parent.get(&g)).copied();
        match mid {
            Some(mid) if models.contains_key(&mid) => fbx_model_world(mid, &models, &parent) * models[&mid].1,
            _ => glam::DMat4::IDENTITY,
        }
    };

    // ---- material → texture slot, decoded once and shared ----
    let mut img_slot: HashMap<String, usize> = HashMap::new(); // "v:<id>" embedded / "f:<name>" file
    let mut col_slot: HashMap<[u8; 4], usize> = HashMap::new();
    let mut mat_slot: HashMap<i64, Option<usize>> = HashMap::new();
    let mut resolve = |mid: Option<i64>, pbr: &mut GltfPbr| -> Option<usize> {
        let key = mid.unwrap_or(-1);
        if let Some(&s) = mat_slot.get(&key) {
            return s;
        }
        let mut slot: Option<usize> = None;
        if let Some(&tid) = mid.and_then(|m| mat_tex.get(&m).or_else(|| mat_tex_alt.get(&m))) {
            let vid = tex_video.get(&tid).copied();
            // 1. the image packed inside the file
            if let Some(v) = vid {
                let k = format!("v:{v}");
                if let Some(&s) = img_slot.get(&k) {
                    slot = Some(s);
                } else if let Some(bytes) = sc.vid_content.get(&v) {
                    if let Ok(dec) = image::load_from_memory(bytes) {
                        let rgba = dec.to_rgba8();
                        let (w, h) = (rgba.width(), rgba.height());
                        slot = Some(pbr.textures.len());
                        pbr.textures.push((w, h, rgba.into_raw()));
                        img_slot.insert(k, slot.unwrap());
                    }
                }
            }
            // 2. the image file it names, beside the FBX
            if slot.is_none() {
                if let Some(name) = sc.tex_file.get(&tid).or_else(|| vid.and_then(|v| sc.vid_file.get(&v))) {
                    let k = format!("f:{name}");
                    if let Some(&s) = img_slot.get(&k) {
                        slot = Some(s);
                    } else if let Some((w, h, rgba)) = fbx_load_texture(name, base) {
                        slot = Some(pbr.textures.len());
                        pbr.textures.push((w, h, rgba));
                        img_slot.insert(k, slot.unwrap());
                    }
                }
            }
        }
        // 3. no image: a 1×1 swatch of the diffuse colour, so the part still reads as itself.
        if slot.is_none() {
            let d = mid.and_then(|m| mat_color.get(&m)).copied().unwrap_or([0.72; 3]);
            let rgba = [
                (d[0].clamp(0.0, 1.0) * 255.0) as u8,
                (d[1].clamp(0.0, 1.0) * 255.0) as u8,
                (d[2].clamp(0.0, 1.0) * 255.0) as u8,
                255,
            ];
            slot = Some(*col_slot.entry(rgba).or_insert_with(|| {
                pbr.textures.push((1, 1, rgba.to_vec()));
                pbr.textures.len() - 1
            }));
        }
        mat_slot.insert(key, slot);
        slot
    };

    // ---- emit ----
    let mut part_of: HashMap<(usize, usize), u32> = HashMap::new(); // (geometry, material slot) → part
    let mut had_uv = false;
    for (gi, g) in sc.geoms.iter().enumerate() {
        let gid = g.i64_at(0);
        let (verts, indices) = match (
            g.child("Vertices").and_then(|c| c.first_f64_arr()),
            g.child("PolygonVertexIndex").and_then(|c| c.first_i32_arr()),
        ) {
            (Some(v), Some(i)) => (v, i),
            _ => continue,
        };
        let uv = fbx_uv_layer(g);
        had_uv |= uv.uvs.len() >= 2;
        let (mat_poly, mat_all_same) = fbx_material_layer(g);
        let mats: &[i64] = gid
            .and_then(|x| parent.get(&x))
            .and_then(|m| model_mats.get(m))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let world = world_of(&gid);
        let vcount = verts.len() / 3;

        let pos = |i: usize| -> [f32; 3] {
            let w = world.transform_point3(glam::DVec3::new(verts[3 * i], verts[3 * i + 1], verts[3 * i + 2]));
            // Metres = file unit x UnitScaleFactor / 100 — FBX measures in centimetres.
            let v = if up_axis == 2 { w } else { glam::DVec3::new(w.x, -w.z, w.y) } * (unit_cm / 100.0);
            [v.x as f32, v.y as f32, v.z as f32]
        };
        let uv_at = |pv: usize, vi: usize| -> [f32; 2] {
            if uv.uvs.len() < 2 {
                return [0.0, 0.0];
            }
            let di = if uv.by_vertex {
                vi
            } else if uv.indexed {
                uv.index.get(pv).map(|v| *v as usize).unwrap_or(0)
            } else {
                pv
            };
            match (uv.uvs.get(di * 2), uv.uvs.get(di * 2 + 1)) {
                (Some(u), Some(v)) => [*u as f32, *v as f32],
                _ => [0.0, 0.0],
            }
        };

        let mut poly: Vec<(usize, [f32; 2])> = Vec::new();
        let mut pv = 0usize;
        let mut poly_i = 0usize;
        for &raw in indices {
            let (vi, last) = if raw < 0 { ((!raw) as usize, true) } else { (raw as usize, false) };
            if vi < vcount {
                poly.push((vi, uv_at(pv, vi)));
            }
            pv += 1;
            if !last {
                continue;
            }
            // Which of the owning model's material slots this polygon wears.
            let local = if mat_all_same {
                mat_poly.first().copied().unwrap_or(0).max(0) as usize
            } else {
                mat_poly.get(poly_i).copied().unwrap_or(0).max(0) as usize
            };
            let mid = mats.get(local).or_else(|| mats.first()).copied();
            let part = *part_of.entry((gi, local)).or_insert_with(|| {
                let slot = resolve(mid, &mut pbr);
                pbr.part_texture.push(slot);
                (pbr.part_texture.len() - 1) as u32
            });
            // Material opacity (1 = opaque) → per-vertex alpha, so glass panes are peeled into the
            // see-through blended pass exactly like OBJ/glTF transparency.
            let opac = mid.and_then(|m| mat_opac.get(&m)).copied().unwrap_or(1.0);
            for k in 1..poly.len().saturating_sub(1) {
                let (a, ua) = poly[0];
                let (b, ub) = poly[k];
                let (c, uc) = poly[k + 1];
                let (pa, pb, pc) = (pos(a), pos(b), pos(c));
                let n = flat_normal(pa, pb, pc);
                out.positions.extend_from_slice(&[pa, pb, pc]);
                out.normals.extend_from_slice(&[n, n, n]);
                out.alpha.extend_from_slice(&[opac; 3]);
                pbr.uvs.extend_from_slice(&[ua, ub, uc]);
                pbr.part_ids.push(part);
            }
            poly.clear();
            poly_i += 1;
        }
    }
    // All-opaque FBX ⇒ drop the alpha array so opaque models keep the fast, byte-identical path.
    trim_alpha(&mut out);
    // No UV layer anywhere ⇒ drop the (all-zero) channel so the app box-projects instead.
    if !had_uv {
        pbr.uvs.clear();
    }
    pbr.texture = pbr.textures.first().cloned();
    (out, pbr)
}

// ============================ ASCII FBX (text export) ============================
//
// SketchUp / the FBX SDK also emit a TEXT ("ASCII") FBX — a brace-nested node tree, no binary
// magic. The binary reader above rejects it (`info.ascii = true`), so those files imported as
// nothing. This reader parses the text tree, pulls each `Geometry`'s Vertices / PolygonVertexIndex
// / UVs, and follows the `Connections` graph (Geometry→Model←Material←Texture←Video) so each part
// gets its own material's IMAGE (or a solid swatch of its diffuse colour) — returned through the
// same `GltfPbr` channel the multi-material glTF path uses, so per-region texturing "just works".

/// True when `data` is not a BINARY FBX — an ASCII FBX text export (or not FBX at all). Only the
/// `.fbx` import arm calls this, so a non-binary `.fbx` is treated as ASCII.
pub fn is_ascii_fbx(data: &[u8]) -> bool {
    const MAGIC: &[u8] = b"Kaydara FBX Binary  \x00\x1a\x00";
    data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC
}

#[derive(Debug, Clone)]
enum AVal {
    Num(f64),
    Str(String),
}
impl AVal {
    fn num(&self) -> Option<f64> {
        if let AVal::Num(n) = self { Some(*n) } else { None }
    }
    fn as_u64(&self) -> Option<u64> {
        self.num().map(|n| n as u64)
    }
    fn str(&self) -> Option<&str> {
        if let AVal::Str(s) = self { Some(s) } else { None }
    }
}

#[derive(Debug)]
struct ANode {
    name: String,
    props: Vec<AVal>,
    children: Vec<ANode>,
}
impl ANode {
    fn child(&self, name: &str) -> Option<&ANode> {
        self.children.iter().find(|c| c.name == name)
    }
    /// The numeric `a: …` array that FBX nests under an array node (Vertices/Normals/UV/…).
    fn arr(&self) -> Vec<f64> {
        self.child("a")
            .map(|a| a.props.iter().filter_map(|v| v.num()).collect())
            .unwrap_or_default()
    }
}

struct AParse<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> AParse<'a> {
    /// Skip whitespace AND `;`-to-end-of-line comments.
    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b';' {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if c.is_ascii_whitespace() {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    /// Skip spaces/tabs/CR only — NOT newlines (a newline without a preceding comma ends a value list).
    fn skip_inline(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b' ' || c == b'\t' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    fn parse_siblings(&mut self) -> Vec<ANode> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            if self.i >= self.b.len() || self.b[self.i] == b'}' {
                break;
            }
            match self.parse_node() {
                Some(n) => out.push(n),
                None => break,
            }
        }
        out
    }
    fn parse_node(&mut self) -> Option<ANode> {
        self.skip_ws();
        let start = self.i;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            // Not an identifier (stray char) — consume it so we don't spin forever.
            self.i += 1;
            return None;
        }
        let name = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
        self.skip_inline();
        if self.i >= self.b.len() || self.b[self.i] != b':' {
            return None;
        }
        self.i += 1; // ':'
        let mut props = Vec::new();
        loop {
            self.skip_inline();
            if self.i >= self.b.len() {
                break;
            }
            let c = self.b[self.i];
            if c == b'{' || c == b'}' || c == b'\n' {
                break;
            }
            match self.read_value() {
                Some(v) => props.push(v),
                None => break,
            }
            self.skip_inline();
            if self.i < self.b.len() && self.b[self.i] == b',' {
                self.i += 1;
                self.skip_ws(); // a value list may wrap onto following lines after a comma
            } else {
                break;
            }
        }
        self.skip_ws();
        let mut children = Vec::new();
        if self.i < self.b.len() && self.b[self.i] == b'{' {
            self.i += 1;
            children = self.parse_siblings();
            self.skip_ws();
            if self.i < self.b.len() && self.b[self.i] == b'}' {
                self.i += 1;
            }
        }
        Some(ANode { name, props, children })
    }
    fn read_value(&mut self) -> Option<AVal> {
        let c = self.b[self.i];
        if c == b'"' {
            self.i += 1;
            let s = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' {
                self.i += 1;
            }
            let out = String::from_utf8_lossy(&self.b[s..self.i]).into_owned();
            if self.i < self.b.len() {
                self.i += 1; // closing quote
            }
            Some(AVal::Str(out))
        } else if c == b'*' {
            self.i += 1; // array-count marker `*N`
            Some(AVal::Num(self.read_number().unwrap_or(0.0)))
        } else if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() {
            self.read_number().map(AVal::Num)
        } else if c.is_ascii_alphabetic() {
            let s = self.i;
            while self.i < self.b.len() {
                let d = self.b[self.i];
                if d.is_ascii_alphanumeric() || d == b'_' {
                    self.i += 1;
                } else {
                    break;
                }
            }
            Some(AVal::Str(String::from_utf8_lossy(&self.b[s..self.i]).into_owned()))
        } else {
            None
        }
    }
    fn read_number(&mut self) -> Option<f64> {
        let s = self.i;
        while self.i < self.b.len() {
            let d = self.b[self.i];
            if d.is_ascii_digit() || d == b'.' || d == b'-' || d == b'+' || d == b'e' || d == b'E' {
                self.i += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.b[s..self.i]).ok()?.parse::<f64>().ok()
    }
}

/// Resolve an FBX texture filename (which may be `modern_stair\stair_wood.png`, a relative path, or
/// bare) against the FBX's own directory and decode it to `(w, h, rgba8)`. Tries the path as-given,
/// then just the basename in the same folder — enough for SketchUp's `folder/name.png` layout.
fn fbx_load_texture(name: &str, base: Option<&std::path::Path>) -> Option<(u32, u32, Vec<u8>)> {
    let base = base?;
    let norm = name.replace('\\', "/");
    let basename = norm.rsplit('/').next().unwrap_or(&norm);
    let mut candidates: Vec<std::path::PathBuf> = vec![base.join(&norm), base.join(basename)];
    // Also strip any leading `../` so an export-machine path still resolves beside the file.
    let trimmed: String = norm.trim_start_matches("../").trim_start_matches("./").to_string();
    if trimmed != norm {
        candidates.push(base.join(&trimmed));
    }
    for cand in candidates {
        if let Ok(bytes) = std::fs::read(&cand) {
            if let Ok(dec) = image::load_from_memory(&bytes) {
                let rgba = dec.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                return Some((w, h, rgba.into_raw()));
            }
        }
    }
    None
}

/// Parse an ASCII (text) FBX into a triangle soup plus a [`GltfPbr`] carrying per-part UVs and each
/// part's material image / colour. `base` is the FBX's directory (for resolving texture files).
pub fn parse_fbx_ascii(text: &[u8], base: Option<&std::path::Path>) -> (ObjMesh, GltfPbr) {
    use std::collections::HashMap;
    let mut out = ObjMesh::default();
    let mut pbr = GltfPbr::default();
    let mut p = AParse { b: text, i: 0 };
    let roots = p.parse_siblings();
    let objects = match roots.iter().find(|n| n.name == "Objects") {
        Some(o) => o,
        None => return (out, pbr),
    };

    // Up axis (default Y): FBX GlobalSettings `UpAxis` 1=Y (rotate to Z-up), 2=Z (already correct).
    let up_axis = roots
        .iter()
        .find(|n| n.name == "GlobalSettings")
        .and_then(|g| g.child("Properties70"))
        .and_then(|p| {
            p.children.iter().find(|c| {
                c.name == "P" && c.props.first().and_then(|v| v.str()) == Some("UpAxis")
            })
        })
        .and_then(|c| c.props.iter().rev().find_map(|v| v.num()))
        .map(|n| n as i64)
        .unwrap_or(1);

    // ---- catalogue materials, textures, videos by id ----
    let mut mat_diffuse: HashMap<u64, [f32; 3]> = HashMap::new();
    let mut tex_file: HashMap<u64, String> = HashMap::new();
    let mut vid_file: HashMap<u64, String> = HashMap::new();
    for n in &objects.children {
        let id = match n.props.first().and_then(|v| v.as_u64()) {
            Some(i) => i,
            None => continue,
        };
        match n.name.as_str() {
            "Material" => {
                let mut diff = [0.8, 0.8, 0.8];
                if let Some(props70) = n.child("Properties70") {
                    for pp in &props70.children {
                        if pp.name == "P"
                            && pp.props.first().and_then(|v| v.str()) == Some("DiffuseColor")
                        {
                            let nums: Vec<f64> = pp.props.iter().filter_map(|v| v.num()).collect();
                            if nums.len() >= 3 {
                                let l = nums.len();
                                diff = [nums[l - 3] as f32, nums[l - 2] as f32, nums[l - 1] as f32];
                            }
                        }
                    }
                }
                mat_diffuse.insert(id, diff);
            }
            "Texture" => {
                if let Some(f) = n
                    .child("FileName")
                    .or_else(|| n.child("RelativeFilename"))
                    .or_else(|| n.child("Filename"))
                    .and_then(|c| c.props.first())
                    .and_then(|v| v.str())
                {
                    tex_file.insert(id, f.to_string());
                }
            }
            "Video" => {
                if let Some(f) = n
                    .child("Filename")
                    .or_else(|| n.child("FileName"))
                    .or_else(|| n.child("RelativeFilename"))
                    .and_then(|c| c.props.first())
                    .and_then(|v| v.str())
                {
                    vid_file.insert(id, f.to_string());
                }
            }
            _ => {}
        }
    }

    // Geometry ids (to classify connection endpoints).
    let geom_ids: std::collections::HashSet<u64> = objects
        .children
        .iter()
        .filter(|n| n.name == "Geometry" && n.child("Vertices").is_some())
        .filter_map(|n| n.props.first().and_then(|v| v.as_u64()))
        .collect();

    // ---- follow the Connections graph ----
    let mut geom_model: HashMap<u64, u64> = HashMap::new(); // geometry → owning model
    let mut model_mats: HashMap<u64, Vec<u64>> = HashMap::new(); // model → materials (in order)
    let mut mat_tex: HashMap<u64, u64> = HashMap::new(); // material → texture
    let mut model_tex: HashMap<u64, u64> = HashMap::new(); // model → texture (some exporters)
    if let Some(conns) = roots.iter().find(|n| n.name == "Connections") {
        for c in &conns.children {
            if c.name != "C" {
                continue;
            }
            let child = c.props.get(1).and_then(|v| v.as_u64());
            let parent = c.props.get(2).and_then(|v| v.as_u64());
            let (child, parent) = match (child, parent) {
                (Some(a), Some(b)) => (a, b),
                _ => continue,
            };
            if geom_ids.contains(&child) {
                geom_model.insert(child, parent);
            } else if mat_diffuse.contains_key(&child) {
                model_mats.entry(parent).or_default().push(child);
            } else if tex_file.contains_key(&child) {
                if mat_diffuse.contains_key(&parent) {
                    mat_tex.insert(parent, child);
                } else {
                    model_tex.insert(parent, child);
                }
            }
        }
    }

    // ---- emit geometry, resolving each part's texture/colour slot ----
    let mut img_cache: HashMap<String, usize> = HashMap::new(); // texture file → `textures` slot
    let mut color_cache: HashMap<[u8; 4], usize> = HashMap::new(); // solid diffuse → slot
    let mut part_id: u32 = 0;
    for n in &objects.children {
        if n.name != "Geometry" {
            continue;
        }
        let gid = match n.props.first().and_then(|v| v.as_u64()) {
            Some(i) => i,
            None => continue,
        };
        let verts = n.child("Vertices").map(|c| c.arr()).unwrap_or_default();
        let indices = n.child("PolygonVertexIndex").map(|c| c.arr()).unwrap_or_default();
        if verts.is_empty() || indices.is_empty() {
            continue;
        }
        let uv_node = n.child("LayerElementUV");
        let uvs = uv_node.map(|u| u.child("UV").map(|c| c.arr()).unwrap_or_default()).unwrap_or_default();
        let uv_index = uv_node.map(|u| u.child("UVIndex").map(|c| c.arr()).unwrap_or_default()).unwrap_or_default();
        let uv_ref = uv_node
            .and_then(|u| u.child("ReferenceInformationType"))
            .and_then(|c| c.props.first())
            .and_then(|v| v.str())
            .unwrap_or("Direct");
        let uv_map = uv_node
            .and_then(|u| u.child("MappingInformationType"))
            .and_then(|c| c.props.first())
            .and_then(|v| v.str())
            .unwrap_or("ByPolygonVertex");
        let uv_i2d = uv_ref.contains("IndexToDirect");
        let uv_by_vertex = uv_map.contains("ByVertice") || uv_map.contains("ByVertex");

        // This geometry's material → texture image (or a swatch of the material's diffuse colour).
        let mat_poly = n
            .child("LayerElementMaterial")
            .and_then(|m| m.child("Materials"))
            .map(|c| c.arr())
            .unwrap_or_default();
        let mi = mat_poly.first().map(|v| *v as usize).unwrap_or(0);
        let model = geom_model.get(&gid).copied();
        let material_id = model
            .and_then(|m| model_mats.get(&m))
            .and_then(|v| v.get(mi).or_else(|| v.first()))
            .copied();
        let tex_id = material_id
            .and_then(|m| mat_tex.get(&m))
            .copied()
            .or_else(|| model.and_then(|m| model_tex.get(&m)).copied());

        let slot: Option<usize> = tex_id
            .and_then(|t| tex_file.get(&t).or_else(|| vid_file.get(&t)))
            .and_then(|fname| {
                if let Some(&s) = img_cache.get(fname) {
                    return Some(s);
                }
                let (w, h, rgba) = fbx_load_texture(fname, base)?;
                let s = pbr.textures.len();
                pbr.textures.push((w, h, rgba));
                img_cache.insert(fname.clone(), s);
                Some(s)
            })
            .or_else(|| {
                // No image: bake the material's diffuse colour into a 1×1 swatch so the part still
                // shows its own colour (metal grey vs wood brown) instead of the neutral default.
                let d = material_id.and_then(|m| mat_diffuse.get(&m)).copied()?;
                let key = [
                    (d[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (d[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (d[2].clamp(0.0, 1.0) * 255.0) as u8,
                    255,
                ];
                if let Some(&s) = color_cache.get(&key) {
                    return Some(s);
                }
                let s = pbr.textures.len();
                pbr.textures.push((1, 1, key.to_vec()));
                color_cache.insert(key, s);
                Some(s)
            });
        pbr.part_texture.push(slot);

        // Transform a vertex to app space (Y-up→Z-up unless the file is already Z-up).
        let tp = |vi: usize| -> [f32; 3] {
            let (x, y, z) = (verts[3 * vi], verts[3 * vi + 1], verts[3 * vi + 2]);
            let v = if up_axis == 2 {
                [x, y, z]
            } else {
                [x, -z, y]
            };
            [v[0] as f32, v[1] as f32, v[2] as f32]
        };
        let vcount = verts.len() / 3;
        let uv_at = |pv: usize, vi: usize| -> [f32; 2] {
            if uvs.len() < 2 {
                return [0.0, 0.0];
            }
            let di = if uv_by_vertex {
                vi
            } else if uv_i2d {
                uv_index.get(pv).map(|v| *v as usize).unwrap_or(0)
            } else {
                pv
            };
            let b = di * 2;
            if b + 1 < uvs.len() {
                [uvs[b] as f32, uvs[b + 1] as f32]
            } else {
                [0.0, 0.0]
            }
        };

        let mut poly: Vec<(usize, [f32; 2])> = Vec::new();
        let mut pv = 0usize;
        for &raw in &indices {
            let r = raw as i64;
            let (vi, last) = if r < 0 { ((!r) as usize, true) } else { (r as usize, false) };
            if vi < vcount {
                poly.push((vi, uv_at(pv, vi)));
            }
            pv += 1;
            if last {
                for k in 1..poly.len().saturating_sub(1) {
                    let (a, ua) = poly[0];
                    let (b, ub) = poly[k];
                    let (cc, uc) = poly[k + 1];
                    let (pa, pb, pcc) = (tp(a), tp(b), tp(cc));
                    let nrm = flat_normal(pa, pb, pcc);
                    out.positions.push(pa);
                    out.positions.push(pb);
                    out.positions.push(pcc);
                    out.normals.push(nrm);
                    out.normals.push(nrm);
                    out.normals.push(nrm);
                    pbr.uvs.push([ua[0], ua[1]]);
                    pbr.uvs.push([ub[0], ub[1]]);
                    pbr.uvs.push([uc[0], uc[1]]);
                    pbr.part_ids.push(part_id);
                }
                poly.clear();
            }
        }
        part_id += 1;
    }

    // If not a single UV survived, drop the (all-zero) UV channel so the app box-projects instead.
    if pbr.uvs.iter().all(|t| t[0] == 0.0 && t[1] == 0.0) {
        pbr.uvs.clear();
    }
    pbr.texture = pbr.textures.first().cloned();
    (out, pbr)
}

// ============================ glTF 2.0 (.glb / .gltf) ============================
//
// The modern interchange format: one file carries geometry (and, in full glTF, PBR
// materials + textures). We read GEOMETRY here — positions, normals, indices — walking the
// node hierarchy so multi-part models assemble correctly, and converting glTF's Y-up frame
// to the app's Z-up. `.glb` (binary, self-contained) and `.gltf` (JSON + data-URI/external
// buffers) are both handled. Materials/UVs are not yet applied (colour via the Textures
// menu, same as OBJ/FBX). Never panics: a malformed file yields an empty mesh.

/// Bounds-checked little-endian u32 read (the file's other `le_u32` panics on OOB; glTF
/// offsets come from untrusted files, so these must fail soft).
fn ck_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Split a `.glb` container into its JSON string and optional BIN chunk. Returns `None` if
/// the magic/version/structure is wrong (caller then tries a text `.gltf` parse).
fn glb_split(data: &[u8]) -> Option<(String, Option<Vec<u8>>)> {
    // GLB magic is the ASCII "glTF" = [0x67,0x6C,0x54,0x46] → little-endian u32 0x4654_6C67.
    // (This was previously 0x4674_6C67 = "gltF" with a lowercase 't', so EVERY real .glb was
    // rejected and silently fell back to a failing text parse — 0 triangles imported.)
    if ck_u32(data, 0)? != 0x4654_6C67 {
        return None; // not GLB → caller tries a text `.gltf` parse
    }
    // le_u32(data,4) = version (2), le_u32(data,8) = total length (unused).
    let mut off = 12usize;
    let mut json: Option<String> = None;
    let mut bin: Option<Vec<u8>> = None;
    while off + 8 <= data.len() {
        let len = ck_u32(data, off)? as usize;
        let kind = ck_u32(data, off + 4)?;
        let start = off + 8;
        let end = start.checked_add(len)?;
        let body = data.get(start..end)?;
        match kind {
            0x4E4F_534A => json = Some(String::from_utf8_lossy(body).into_owned()), // "JSON"
            0x004E_4942 => bin = Some(body.to_vec()),                               // "BIN\0"
            _ => {}
        }
        off = end + (end % 4).wrapping_neg() % 4; // chunks are 4-byte aligned
        if json.is_some() && bin.is_some() {
            break;
        }
    }
    json.map(|j| (j, bin))
}

/// Resolve every `buffers[]` entry to raw bytes: the GLB BIN chunk (uri absent), a
/// `data:...;base64,` URI, or an external file relative to `base_dir`.
fn gltf_buffers(
    doc: &serde_json::Value,
    glb_bin: Option<Vec<u8>>,
    base_dir: Option<&std::path::Path>,
) -> Vec<Vec<u8>> {
    use base64::Engine;
    let arr = doc.get("buffers").and_then(|b| b.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(arr.len());
    for (i, b) in arr.iter().enumerate() {
        match b.get("uri").and_then(|u| u.as_str()) {
            None => out.push(if i == 0 { glb_bin.clone().unwrap_or_default() } else { Vec::new() }),
            Some(uri) if uri.starts_with("data:") => {
                let bytes = uri
                    .find(";base64,")
                    .and_then(|p| base64::engine::general_purpose::STANDARD.decode(&uri[p + 8..]).ok())
                    .unwrap_or_default();
                out.push(bytes);
            }
            Some(uri) => {
                let dec = percent_decode(uri);
                let bytes = base_dir
                    .map(|d| d.join(&dec))
                    .and_then(|p| std::fs::read(p).ok())
                    .unwrap_or_default();
                out.push(bytes);
            }
        }
    }
    out
}

/// Minimal percent-decoding for buffer/image URIs (spaces as `%20`, etc.).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = |c: u8| (c as char).to_digit(16);
            if let (Some(a), Some(c)) = (h(b[i + 1]), h(b[i + 2])) {
                out.push((a * 16 + c) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read one accessor as `comps`-wide FLOAT tuples (POSITION/NORMAL = VEC3). Honours the
/// bufferView's `byteStride` (interleaved buffers) and both offsets. Non-float component
/// types are skipped (returns empty), which just drops that primitive rather than panicking.
fn accessor_floats(
    doc: &serde_json::Value,
    bufs: &[Vec<u8>],
    idx: usize,
    comps: usize,
) -> Vec<f32> {
    let acc = &doc["accessors"][idx];
    if acc.get("componentType").and_then(|v| v.as_u64()) != Some(5126) {
        return Vec::new(); // only FLOAT positions/normals
    }
    let count = acc.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let acc_off = acc.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let bv_idx = match acc.get("bufferView").and_then(|v| v.as_u64()) {
        Some(v) => v as usize,
        None => return Vec::new(),
    };
    let bv = &doc["bufferViews"][bv_idx];
    let buf_idx = bv.get("buffer").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let bv_off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let elem = comps * 4;
    let stride = bv.get("byteStride").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(elem);
    let Some(buf) = bufs.get(buf_idx) else { return Vec::new() };
    let mut out = Vec::with_capacity(count * comps);
    for e in 0..count {
        let base = bv_off + acc_off + e * stride;
        for c in 0..comps {
            match ck_u32(buf, base + c * 4) {
                Some(bits) => out.push(f32::from_bits(bits)),
                None => return out,
            }
        }
    }
    out
}

/// Read an index accessor (SCALAR ubyte/ushort/uint) into u32s. Empty accessor index → a
/// sequential 0..count list (non-indexed primitive), filled by the caller.
fn accessor_indices(doc: &serde_json::Value, bufs: &[Vec<u8>], idx: usize) -> Vec<u32> {
    let acc = &doc["accessors"][idx];
    let count = acc.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let ct = acc.get("componentType").and_then(|v| v.as_u64()).unwrap_or(0);
    let acc_off = acc.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let Some(bv_idx) = acc.get("bufferView").and_then(|v| v.as_u64()) else { return Vec::new() };
    let bv = &doc["bufferViews"][bv_idx as usize];
    let buf_idx = bv.get("buffer").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let bv_off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let Some(buf) = bufs.get(buf_idx) else { return Vec::new() };
    let size = match ct { 5121 => 1, 5123 => 2, 5125 => 4, _ => return Vec::new() };
    let mut out = Vec::with_capacity(count);
    for e in 0..count {
        let o = bv_off + acc_off + e * size;
        let v = match ct {
            5121 => buf.get(o).map(|&b| b as u32),
            5123 => buf.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u32),
            5125 => ck_u32(buf, o),
            _ => None,
        };
        match v { Some(v) => out.push(v), None => return out }
    }
    out
}

/// Local transform of a node: its `matrix` (column-major) if present, else `T * R * S`.
fn gltf_node_matrix(node: &serde_json::Value) -> glam::Mat4 {
    if let Some(m) = node.get("matrix").and_then(|v| v.as_array()) {
        if m.len() == 16 {
            let mut a = [0f32; 16];
            for (i, x) in m.iter().enumerate() { a[i] = x.as_f64().unwrap_or(0.0) as f32; }
            return glam::Mat4::from_cols_array(&a);
        }
    }
    let f3 = |v: &serde_json::Value, def: glam::Vec3| -> glam::Vec3 {
        v.as_array().filter(|a| a.len() == 3).map(|a| {
            glam::Vec3::new(a[0].as_f64().unwrap_or(0.0) as f32,
                a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32)
        }).unwrap_or(def)
    };
    let t = f3(&node["translation"], glam::Vec3::ZERO);
    let s = f3(&node["scale"], glam::Vec3::ONE);
    let r = node.get("rotation").and_then(|v| v.as_array()).filter(|a| a.len() == 4).map(|a| {
        glam::Quat::from_xyzw(a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32,
            a[2].as_f64().unwrap_or(0.0) as f32, a[3].as_f64().unwrap_or(1.0) as f32)
    }).unwrap_or(glam::Quat::IDENTITY);
    glam::Mat4::from_scale_rotation_translation(s, r, t)
}

/// PBR extras a glTF carries beyond raw geometry, so imported furniture shows its OWN materials —
/// including MULTI-MATERIAL models, where each primitive has a different base-colour texture (a
/// staircase whose treads, glass and rails are separate materials).
#[derive(Clone, Debug, Default)]
pub struct GltfPbr {
    /// One `[u,v]` per emitted vertex (same length as `ObjMesh::positions`), or empty when the
    /// model has no TEXCOORD_0 — then the caller falls back to box-projection UVs.
    pub uvs: Vec<[f32; 2]>,
    /// Every distinct base-colour image used by the model, decoded to `(w, h, rgba8)`. A material
    /// with only a `baseColorFactor` (no image) contributes a 1×1 solid-colour swatch here.
    pub textures: Vec<(u32, u32, Vec<u8>)>,
    /// One PART id per TRIANGLE (a glTF primitive = one part = one material), parallel to
    /// `ObjMesh::positions` / 3. Lets the app texture each material region on its own.
    pub part_ids: Vec<u32>,
    /// For each part id, the index into [`Self::textures`] of its base colour (or `None` when the
    /// material had neither an image nor a colour factor).
    pub part_texture: Vec<Option<usize>>,
    /// Back-compat: the FIRST texture (single-material fast path / older callers).
    pub texture: Option<(u32, u32, Vec<u8>)>,
    /// Per part: the material's ROUGHNESS and METALLIC.
    ///
    /// These were being thrown away, and it showed. glTF carries them per material and every
    /// import landed on the app's default 0.5 roughness instead — so a pool whose material is
    /// roughness 0.035 (a mirror) rendered as flat cyan paint with nothing to reflect, and the
    /// whole scene read matte. Empty when the format has no such notion (OBJ, FBX).
    pub part_rough: Vec<f32>,
    pub part_metal: Vec<f32>,
    /// Per part: the material's MAPS — tangent-space normal, plus the roughness / metallic /
    /// occlusion channels of its `metallicRoughnessTexture` and `occlusionTexture`, each already
    /// split into its own single-channel image. Indices into [`Self::textures`], like
    /// `part_texture`, and `None` where the material has no such map.
    ///
    /// The scalars above were read and the maps were not, and that is not the same material: the
    /// villa's stucco, painted wood, roof tiles, granite paving and pool tiles each carry a normal
    /// map AND a packed metallic-roughness map, so all five imported perfectly flat with one
    /// uniform finish across the whole surface.
    pub part_normal: Vec<Option<usize>>,
    pub part_rough_map: Vec<Option<usize>>,
    pub part_metal_map: Vec<Option<usize>>,
    pub part_ao_map: Vec<Option<usize>>,
    /// Per part: `KHR_materials_transmission`, 0 for everything else. See [`gltf_transmission`] —
    /// this is what tells the renderer a surface is a MEDIUM rather than a mesh with holes, and so
    /// which surfaces are volumes whose back faces must not be drawn.
    pub part_transmission: Vec<f32>,
}

/// Which channel of a glTF image an app-side map wants.
///
/// glTF packs occlusion in R, roughness in G and metallic in B of ONE image, while the shader reads
/// `.r` through a dedicated sampler per map. So a packed source is split here, at import, into one
/// single-channel image each. The alternative — a channel selector threaded through the texture
/// asset, the uploader and three uniforms — carries a fact the importer already knows all the way
/// to the GPU for no gain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Chan {
    /// Keep all three channels — a tangent-space normal map.
    Rgb,
    R,
    G,
    B,
}

/// Accumulator threaded through the glTF node walk while building the multi-material mesh.
#[derive(Default)]
struct GltfBuild {
    out: ObjMesh,
    uvs: Vec<[f32; 2]>,
    had_uv: bool,
    part_ids: Vec<u32>,               // per triangle
    part_texture: Vec<Option<usize>>, // per part → slot in `textures`
    part_rough: Vec<f32>,
    part_metal: Vec<f32>,
    part_normal: Vec<Option<usize>>,
    part_rough_map: Vec<Option<usize>>,
    part_metal_map: Vec<Option<usize>>,
    part_ao_map: Vec<Option<usize>>,
    part_transmission: Vec<f32>,
    textures: Vec<(u32, u32, Vec<u8>)>,
    img_slot: std::collections::HashMap<usize, usize>, // image source index → `textures` slot
    /// `(image source, channel)` → slot, for the maps. Keyed on the channel too because one packed
    /// metallic-roughness image legitimately becomes two different app textures.
    aux_slot: std::collections::HashMap<(usize, Chan), usize>,
    color_slot: std::collections::HashMap<[u8; 4], usize>, // solid-colour factor → slot
    next_part: u32,
}

impl GltfBuild {
    /// Resolve a material's base colour to a `textures` slot: its base-colour IMAGE if it has one
    /// (deduped by image source), else a 1×1 swatch of its `baseColorFactor`, else `None`.
    fn material_slot(
        &mut self, doc: &serde_json::Value, bufs: &[Vec<u8>], base_dir: Option<&std::path::Path>,
        material_idx: Option<usize>,
    ) -> Option<usize> {
        let m = material_idx.map(|mi| &doc["materials"][mi]);
        // 1) base-colour texture image
        if let Some(m) = m {
            if let Some(ti) = m.pointer("/pbrMetallicRoughness/baseColorTexture/index").and_then(|v| v.as_u64()) {
                if let Some(src) = doc["textures"][ti as usize].get("source").and_then(|v| v.as_u64()) {
                    let src = src as usize;
                    if let Some(&slot) = self.img_slot.get(&src) {
                        return Some(slot);
                    }
                    if let Some(img) = gltf_image_at(doc, bufs, base_dir, src) {
                        let slot = self.textures.len();
                        self.textures.push(img);
                        self.img_slot.insert(src, slot);
                        return Some(slot);
                    }
                }
            }
        }
        // 2) flat baseColorFactor → 1×1 swatch
        let factor = m
            .and_then(|m| m.pointer("/pbrMetallicRoughness/baseColorFactor"))
            .and_then(|v| v.as_array());
        if let Some(f) = factor {
            // `baseColorFactor` is LINEAR — glTF is a linear-light format — but a swatch is a
            // texture, and every texture in this app is sRGB-encoded bytes that the uploader hands
            // to GL as `SRGB8_ALPHA8` for the sampler to decode. Writing the linear number straight
            // into the byte makes the sampler decode something that was never encoded, and the
            // material lands far darker than it was authored: the villa's pool water, authored
            // (0.055, 0.30, 0.34), arrived as (0.004, 0.065, 0.095) — a thirteenth of its red — so
            // it had almost no colour of its own left, and once it reflected the sky at all it
            // read as pale grey rather than as water.
            //
            // ALPHA is coverage, not colour: it is linear on both sides and must not be encoded.
            let chan = |i: usize| f.get(i).and_then(|v| v.as_f64()).unwrap_or(1.0).clamp(0.0, 1.0) as f32;
            let to8 = |v: f32| (v * 255.0).round() as u8;
            let rgba = [
                to8(crate::color::linear_to_srgb(chan(0))),
                to8(crate::color::linear_to_srgb(chan(1))),
                to8(crate::color::linear_to_srgb(chan(2))),
                to8(chan(3)),
            ];
            if let Some(&slot) = self.color_slot.get(&rgba) {
                return Some(slot);
            }
            let slot = self.textures.len();
            self.textures.push((1, 1, rgba.to_vec()));
            self.color_slot.insert(rgba, slot);
            return Some(slot);
        }
        None
    }

    /// Load the image behind a `textures[]` entry as a MAP, taking `chan` from it, and return its
    /// `textures` slot. Deduped on `(image source, channel)`.
    fn map_slot(
        &mut self, doc: &serde_json::Value, bufs: &[Vec<u8>], base_dir: Option<&std::path::Path>,
        tex_idx: usize, chan: Chan,
    ) -> Option<usize> {
        let src = doc["textures"][tex_idx].get("source").and_then(|v| v.as_u64())? as usize;
        if let Some(&slot) = self.aux_slot.get(&(src, chan)) {
            return Some(slot);
        }
        let (w, h, mut rgba) = gltf_image_at(doc, bufs, base_dir, src)?;
        if chan != Chan::Rgb {
            // Broadcast the wanted channel across RGB — the shader samples `.r`, and keeping the
            // other two would leave a roughness map looking like a colour picture in the library.
            let k = match chan {
                Chan::R => 0,
                Chan::G => 1,
                Chan::B => 2,
                Chan::Rgb => unreachable!(),
            };
            for px in rgba.chunks_exact_mut(4) {
                let v = px[k];
                px[0] = v;
                px[1] = v;
                px[2] = v;
                px[3] = 255;
            }
        }
        let slot = self.textures.len();
        self.textures.push((w, h, rgba));
        self.aux_slot.insert((src, chan), slot);
        Some(slot)
    }

    /// Record every map this material carries, in lockstep with `part_texture`. Called ONCE per
    /// part, so all four vectors stay parallel whether or not the material has any maps at all.
    fn push_material_maps(
        &mut self, doc: &serde_json::Value, bufs: &[Vec<u8>], base_dir: Option<&std::path::Path>,
        material_idx: Option<usize>,
    ) {
        let tex_at = |p: &str| -> Option<usize> {
            material_idx
                .and_then(|mi| doc["materials"][mi].pointer(p))
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
        };
        let nrm = tex_at("/normalTexture/index");
        let mr = tex_at("/pbrMetallicRoughness/metallicRoughnessTexture/index");
        let ao = tex_at("/occlusionTexture/index");
        let n = nrm.and_then(|t| self.map_slot(doc, bufs, base_dir, t, Chan::Rgb));
        let r = mr.and_then(|t| self.map_slot(doc, bufs, base_dir, t, Chan::G));
        let m = mr.and_then(|t| self.map_slot(doc, bufs, base_dir, t, Chan::B));
        let o = ao.and_then(|t| self.map_slot(doc, bufs, base_dir, t, Chan::R));
        self.part_normal.push(n);
        self.part_rough_map.push(r);
        self.part_metal_map.push(m);
        self.part_ao_map.push(o);
    }
}

/// Emit one mesh's primitives (transformed by `world`) as a flat triangle soup. Each primitive
/// becomes its own PART (`build.next_part`) carrying its material's base-colour slot, and UVs are
/// pushed in lockstep with positions.
fn gltf_emit_mesh(
    doc: &serde_json::Value, bufs: &[Vec<u8>], base_dir: Option<&std::path::Path>,
    mesh_idx: usize, world: glam::Mat4, build: &mut GltfBuild,
) {
    let normal_mat = glam::Mat3::from_mat4(world).inverse().transpose();
    let prims = doc["meshes"][mesh_idx].get("primitives").and_then(|p| p.as_array()).cloned().unwrap_or_default();
    for prim in &prims {
        // mode 4 = TRIANGLES (default). Skip lines/points/strips — we only draw solids.
        if prim.get("mode").and_then(|m| m.as_u64()).unwrap_or(4) != 4 {
            continue;
        }
        let attr = &prim["attributes"];
        let Some(pos_acc) = attr.get("POSITION").and_then(|v| v.as_u64()) else { continue };
        let positions = accessor_floats(doc, bufs, pos_acc as usize, 3);
        if positions.is_empty() {
            continue;
        }
        let nverts = positions.len() / 3;
        let normals = attr.get("NORMAL").and_then(|v| v.as_u64())
            .map(|n| accessor_floats(doc, bufs, n as usize, 3)).unwrap_or_default();
        let uv = attr.get("TEXCOORD_0").and_then(|v| v.as_u64())
            .map(|t| accessor_floats(doc, bufs, t as usize, 2)).unwrap_or_default();
        if uv.len() >= nverts * 2 {
            build.had_uv = true;
        }
        // Index list, or 0..nverts for a non-indexed primitive.
        let indices = match prim.get("indices").and_then(|v| v.as_u64()) {
            Some(i) => accessor_indices(doc, bufs, i as usize),
            None => (0..nverts as u32).collect(),
        };
        if indices.len() < 3 {
            continue;
        }
        // This primitive is one PART. Resolve + record its material's base-colour slot ONCE.
        let material_idx = prim.get("material").and_then(|v| v.as_u64()).map(|m| m as usize);
        let slot = build.material_slot(doc, bufs, base_dir, material_idx);
        let part = build.next_part;
        build.next_part += 1;
        build.part_texture.push(slot);
        // The surface properties beside the colour. glTF's defaults are 1.0 for both, but a
        // fully-metal default would turn every untagged material into chrome, so an absent
        // metallic reads as 0 — dielectric — which is what an architectural export means.
        let m = material_idx.map(|mi| &doc["materials"][mi]);
        let num = |p: &str, d: f32| {
            m.and_then(|m| m.pointer(p)).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(d)
        };
        build.part_rough.push(num("/pbrMetallicRoughness/roughnessFactor", 0.5).clamp(0.0, 1.0));
        build.part_metal.push(num("/pbrMetallicRoughness/metallicFactor", 0.0).clamp(0.0, 1.0));
        // …and the MAPS beside those scalars, pushed here so all the per-part vectors advance
        // together even for a material that has none.
        build.push_material_maps(doc, bufs, base_dir, material_idx);
        build.part_transmission.push(gltf_transmission(doc, material_idx));

        let gp = |i: u32| -> glam::Vec3 {
            let k = i as usize * 3;
            world.transform_point3(glam::Vec3::new(positions[k], positions[k + 1], positions[k + 2]))
        };
        let gn = |i: u32| -> Option<glam::Vec3> {
            let k = i as usize * 3;
            (k + 2 < normals.len()).then(|| (normal_mat * glam::Vec3::new(normals[k], normals[k + 1], normals[k + 2])).normalize_or_zero())
        };
        let guv = |i: u32| -> [f32; 2] {
            let k = i as usize * 2;
            if k + 1 < uv.len() { [uv[k], uv[k + 1]] } else { [0.0, 0.0] }
        };
        // glTF is Y-up; the app is Z-up → (x, y, z) → (x, -z, y).
        let z_up = |v: glam::Vec3| [v.x, -v.z, v.y];
        // Material opacity: `alphaMode:BLEND` coverage OR `KHR_materials_transmission`, whichever
        // lets more light through (MASK stays solid — no per-texel cutout yet). See
        // `gltf_material_alpha`.
        let mat_alpha = gltf_material_alpha(doc, prim);
        for t in indices.chunks_exact(3) {
            let (a, b, c) = (gp(t[0]), gp(t[1]), gp(t[2]));
            let face_n = (b - a).cross(c - a).normalize_or_zero();
            for (vi, p) in [(t[0], a), (t[1], b), (t[2], c)] {
                let n = gn(vi).filter(|n| n.length_squared() > 1e-8).unwrap_or(face_n);
                build.out.positions.push(z_up(p));
                build.out.normals.push(z_up(n));
                build.out.alpha.push(mat_alpha);
                build.uvs.push(guv(vi));
            }
            build.part_ids.push(part); // one part id per triangle
        }
    }
}

/// Walk the scene node hierarchy from `node_idx`, composing transforms and emitting meshes.
fn gltf_walk(
    doc: &serde_json::Value, bufs: &[Vec<u8>], base_dir: Option<&std::path::Path>,
    node_idx: usize, parent: glam::Mat4, build: &mut GltfBuild, depth: u32,
) {
    if depth > 256 {
        return; // cycle / pathological nesting guard
    }
    let node = &doc["nodes"][node_idx];
    if node.is_null() {
        return;
    }
    let world = parent * gltf_node_matrix(node);
    if let Some(m) = node.get("mesh").and_then(|v| v.as_u64()) {
        gltf_emit_mesh(doc, bufs, base_dir, m as usize, world, build);
    }
    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for ch in children {
            if let Some(ci) = ch.as_u64() {
                gltf_walk(doc, bufs, base_dir, ci as usize, world, build, depth + 1);
            }
        }
    }
}

/// Decode glTF `images[src]` to `(w, h, rgba8)` — resolving a `uri` (data:/external) OR an
/// embedded `bufferView`.
fn gltf_image_at(
    doc: &serde_json::Value, bufs: &[Vec<u8>], base_dir: Option<&std::path::Path>, src: usize,
) -> Option<(u32, u32, Vec<u8>)> {
    let img = &doc["images"][src];
    let bytes = if img.get("uri").is_some() {
        gltf_image_bytes(img, base_dir)
    } else if let Some(bv_idx) = img.get("bufferView").and_then(|v| v.as_u64()) {
        let bv = &doc["bufferViews"][bv_idx as usize];
        let buf = bufs.get(bv.get("buffer").and_then(|v| v.as_u64()).unwrap_or(0) as usize)?;
        let off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let len = bv.get("byteLength").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        buf.get(off..off + len).map(|s| s.to_vec())
    } else {
        None
    }?;
    let dec = image::load_from_memory(&bytes).ok()?;
    let rgba = dec.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Raw bytes of a glTF `images[]` entry addressed by `uri` — a `data:` URI or an external
/// file relative to `base_dir`. The embedded `bufferView` case is handled by the caller.
fn gltf_image_bytes(img: &serde_json::Value, base_dir: Option<&std::path::Path>) -> Option<Vec<u8>> {
    use base64::Engine;
    let uri = img.get("uri").and_then(|v| v.as_str())?;
    if uri.starts_with("data:") {
        let p = uri.find(";base64,")?;
        return base64::engine::general_purpose::STANDARD.decode(&uri[p + 8..]).ok();
    }
    let dec = percent_decode(uri);
    base_dir.map(|d| d.join(&dec)).and_then(|p| std::fs::read(p).ok())
}

/// A glTF primitive's opacity in `0..=1`.
///
/// TWO independent ways a glTF says "you can see through this", and the file needs both read:
///
///   * `alphaMode:BLEND` — thin coverage, the alpha of `baseColorFactor`. `OPAQUE` (the default)
///     and `MASK` are solid.
///   * **`KHR_materials_transmission`** — real refractive transmission, which is what a physically
///     based authoring tool writes for water, glass and anything else light passes THROUGH rather
///     than around. A transmissive material stays `alphaMode:OPAQUE` by design (the spec is
///     explicit that transmission is not coverage), so reading only `alphaMode` misses it
///     completely — which is how the villa's pool, authored at 0.45 transmission, imported as a
///     solid teal lid with the tiles beneath it invisible.
///
/// Whichever admits more light wins, so a material using both is not double-counted.
fn gltf_material_alpha(doc: &serde_json::Value, prim: &serde_json::Value) -> f32 {
    let Some(mi) = prim.get("material").and_then(|v| v.as_u64()) else { return 1.0 };
    let m = &doc["materials"][mi as usize];
    let blend = if m.get("alphaMode").and_then(|v| v.as_str()) == Some("BLEND") {
        m.pointer("/pbrMetallicRoughness/baseColorFactor/3")
            .and_then(|v| v.as_f64())
            .map(|a| (a as f32).clamp(0.0, 1.0))
            .unwrap_or(1.0)
    } else {
        1.0
    };
    blend.min(1.0 - gltf_transmission(doc, prim.get("material").and_then(|v| v.as_u64()).map(|m| m as usize)))
}

/// A material's `KHR_materials_transmission` factor, 0 when it has none.
///
/// Kept apart from the coverage alpha above because the two are NOT the same physical thing, and
/// collapsing them cost a day: coverage says "this mesh has holes in it", transmission says "this
/// is a MEDIUM light travels through". A leaf card is the first; water is the second. Only the
/// second is a volume — with an entry face and an exit face — and only the second may therefore
/// have its back faces dropped. Cull a leaf card's back face and the tree disappears when you walk
/// round it.
fn gltf_transmission(doc: &serde_json::Value, material_idx: Option<usize>) -> f32 {
    // The extension's own default is 0 (opaque) when the object is present but the factor absent.
    material_idx
        .and_then(|mi| doc["materials"][mi].pointer("/extensions/KHR_materials_transmission"))
        .map(|t| {
            t.get("transmissionFactor")
                .and_then(|v| v.as_f64())
                .map(|v| (v as f32).clamp(0.0, 1.0))
                .unwrap_or(0.0)
        })
        .unwrap_or(0.0)
}

/// Parse a glTF into geometry (Z-up flat soup) PLUS its PBR extras — per-vertex UVs, per-primitive
/// part ids, and EVERY material's base colour, so a MULTI-material model shows all its textures.
pub fn parse_gltf_ex(data: &[u8], base_dir: Option<&std::path::Path>) -> (ObjMesh, GltfPbr) {
    let mut pbr = GltfPbr::default();
    // GLB first; fall back to treating the bytes as text .gltf JSON.
    let (json, glb_bin) = match glb_split(data) {
        Some(v) => v,
        None => (String::from_utf8_lossy(data).into_owned(), None),
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&json) else { return (ObjMesh::default(), pbr) };
    let bufs = gltf_buffers(&doc, glb_bin, base_dir);

    // Root nodes: the default scene's `nodes`, else scene 0, else every node.
    let scene_idx = doc.get("scene").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let roots: Vec<usize> = doc["scenes"][scene_idx]
        .get("nodes")
        .and_then(|n| n.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).collect())
        .unwrap_or_else(|| {
            (0..doc.get("nodes").and_then(|n| n.as_array()).map(|a| a.len()).unwrap_or(0)).collect()
        });
    let mut build = GltfBuild::default();
    for r in roots {
        gltf_walk(&doc, &bufs, base_dir, r, glam::Mat4::IDENTITY, &mut build, 0);
    }
    let mut out = std::mem::take(&mut build.out);
    // Keep UVs only if the model actually supplied them (else box-projection in the app).
    if build.had_uv && build.uvs.len() == out.positions.len() {
        pbr.uvs = std::mem::take(&mut build.uvs);
    }
    // Part ids ride along only when they cover every triangle (they always should).
    if build.part_ids.len() == out.positions.len() / 3 {
        pbr.part_ids = std::mem::take(&mut build.part_ids);
        pbr.part_texture = std::mem::take(&mut build.part_texture);
        pbr.part_rough = std::mem::take(&mut build.part_rough);
        pbr.part_metal = std::mem::take(&mut build.part_metal);
        pbr.part_normal = std::mem::take(&mut build.part_normal);
        pbr.part_rough_map = std::mem::take(&mut build.part_rough_map);
        pbr.part_metal_map = std::mem::take(&mut build.part_metal_map);
        pbr.part_ao_map = std::mem::take(&mut build.part_ao_map);
        pbr.part_transmission = std::mem::take(&mut build.part_transmission);
    }
    pbr.texture = build.textures.first().cloned(); // back-compat single-texture field
    pbr.textures = std::mem::take(&mut build.textures);
    trim_alpha(&mut out); // all-opaque ⇒ drop the per-vertex alpha (fast path)
    (out, pbr)
}

/// Parse a glTF 2.0 model (`.glb` binary or `.gltf` JSON) into a flat triangle soup, in the
/// app's Z-up frame. `base_dir` resolves external `.bin`/buffers for a text `.gltf`.
pub fn parse_gltf(data: &[u8], base_dir: Option<&std::path::Path>) -> ObjMesh {
    parse_gltf_ex(data, base_dir).0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INVESTIGATIVE (ignored): what does the villa FBX carry in its material graph? Counts node
    /// types and lists Materials (with diffuse colour) + any Texture/Video (embedded image) nodes.
    ///   cargo test -p cad_app villa_material_probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn villa_material_probe() {
        let data = std::fs::read(r"G:\blender dev\staircase\villa model\build\villa_v1.fbx").unwrap();
        const MAGIC: &[u8] = b"Kaydara FBX Binary  \x00\x1a\x00";
        assert_eq!(&data[..MAGIC.len()], MAGIC, "binary FBX");
        let version = u32::from_le_bytes([data[23], data[24], data[25], data[26]]);
        let v75 = version >= 7500;
        let mut cur = FbxCursor { buf: &data, pos: 27 };
        let root = fbx_parse_siblings(&mut cur, v75, data.len());

        fn walk<'a>(nodes: &'a [FbxNode], counts: &mut std::collections::BTreeMap<String, usize>, mats: &mut Vec<String>, imgs: &mut Vec<String>) {
            for n in nodes {
                *counts.entry(n.name.clone()).or_default() += 1;
                if n.name == "Material" {
                    let name = n.str_at(1).unwrap_or("?");
                    let mut diff = [0.0; 3];
                    if let Some(p70) = n.child("Properties70") {
                        for p in &p70.children {
                            if let Some(k) = p.str_at(0) {
                                if k == "DiffuseColor" || k == "Diffuse" { diff = p.f64_tail3(); }
                            }
                        }
                    }
                    mats.push(format!("{name}  diffuse=({:.2},{:.2},{:.2})", diff[0], diff[1], diff[2]));
                }
                if n.name == "Video" || n.name == "Texture" {
                    let f = n.child("RelativeFilename").and_then(|c| c.str_at(0))
                        .or_else(|| n.child("FileName").and_then(|c| c.str_at(0)))
                        .or_else(|| n.str_at(1))
                        .unwrap_or("?");
                    let has_content = n.child("Content").is_some();
                    imgs.push(format!("{}  file={f}  embedded_content={has_content}", n.name));
                }
                walk(&n.children, counts, mats, imgs);
            }
        }
        let mut counts = std::collections::BTreeMap::new();
        let mut mats = Vec::new();
        let mut imgs = Vec::new();
        walk(&root, &mut counts, &mut mats, &mut imgs);
        eprintln!("\n=== villa_v1.fbx node counts ===");
        for (k, v) in &counts {
            if ["Geometry", "Model", "Material", "Texture", "Video", "Connections", "C", "Objects"].contains(&k.as_str()) {
                eprintln!("  {k}: {v}");
            }
        }
        eprintln!("--- {} materials ---", mats.len());
        for m in &mats { eprintln!("  {m}"); }
        eprintln!("--- {} texture/video (image) nodes ---", imgs.len());
        for i in &imgs { eprintln!("  {i}"); }

        // Report per-material opacity too (glass should read < 1).
        let mut mcol = std::collections::HashMap::new();
        let mut mop = std::collections::HashMap::new();
        fbx_collect_materials(&root, &mut mcol, &mut mop);
        let translucent_mats = mop.values().filter(|&&o| o < 0.996).count();
        eprintln!("--- materials with opacity<1: {translucent_mats} (of {})", mop.len());
        for (id, o) in &mop { if *o < 0.996 { eprintln!("  mat {id} opacity={o:.3}"); } }

        // Now exercise the actual import path: geometry + per-material colour swatches + alpha.
        let (mesh, pbr) = parse_fbx_pbr(&data);
        let distinct: std::collections::BTreeSet<u32> = pbr.part_ids.iter().copied().collect();
        let translucent_verts = mesh.alpha.iter().filter(|&&a| a < 0.996).count();
        eprintln!(
            "--- parse_fbx_pbr: tris={} part_ids={} swatches={} distinct_parts_used={} alpha_len={} translucent_verts={}",
            mesh.tri_count(), pbr.part_ids.len(), pbr.textures.len(), distinct.len(),
            mesh.alpha.len(), translucent_verts,
        );
        assert_eq!(pbr.part_ids.len(), mesh.tri_count(), "one part id per triangle");
        assert!(pbr.textures.len() >= 10, "villa has many colour materials");
        assert!(translucent_mats >= 1, "villa glass material reads opacity < 1");
        assert!(translucent_verts > 0, "villa glass panes import see-through (per-vertex alpha)");
    }

    /// INVESTIGATIVE (ignored by default): parse the user's real FBX files and print what the
    /// parser sees — geometry count, total verts, and the WORLD bounding box, so we can tell
    /// whether the mesh is empty, collapsed (transforms missing), or mis-scaled. Run with:
    ///   cargo test -p cad_app fbx_probe_real_files -- --ignored --nocapture
    #[test]
    #[ignore]
    fn fbx_probe_real_files() {
        let base = "D:/Dropbox/03--PROJECTS/03-PROJECTS/2026/SAEEDA MAM/SAEEDA MA'AM'S PROJECT-02/NEW LAYOUT/furniture/fbx";
        let files = [
            format!("{base}/2/Koltuk.fbx"),
            format!("{base}/2/koltuk2.fbx"),
            format!("{base}/2/testfurniture.fbx"),
            format!("{base}/4/Table And Chairs.FBX"),
        ];
        for f in &files {
            match std::fs::read(f) {
                Ok(bytes) => {
                    let (mesh, info) = parse_fbx_ex(&bytes);
                    let mut mn = [f32::INFINITY; 3];
                    let mut mx = [f32::NEG_INFINITY; 3];
                    for p in &mesh.positions {
                        for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); }
                    }
                    eprintln!(
                        "\n=== {f}\n  ascii={} ver={} geoms={} verts={} indices={} tris={}\n  bbox min=({:.2},{:.2},{:.2}) max=({:.2},{:.2},{:.2}) size=({:.2},{:.2},{:.2})",
                        info.ascii, info.version, info.geometries, info.total_verts, info.total_indices,
                        mesh.tri_count(),
                        mn[0], mn[1], mn[2], mx[0], mx[1], mx[2],
                        mx[0]-mn[0], mx[1]-mn[1], mx[2]-mn[2],
                    );
                }
                Err(e) => eprintln!("=== {f}\n  (could not read: {e})"),
            }
        }
    }

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

    /// INVESTIGATIVE (ignored): parse the user's real multi-material GLB and print what the
    /// importer now extracts — parts, per-part textures, image count. Run:
    ///   cargo test -p cad_app gltf_probe_multimaterial -- --ignored --nocapture
    #[test]
    #[ignore]
    fn gltf_probe_multimaterial() {
        let path = "C:/Users/hsili/Desktop/stairstest2.glb";
        let Ok(bytes) = std::fs::read(path) else { eprintln!("no file — skip"); return };
        let (mesh, pbr) = parse_gltf_ex(&bytes, None);
        let ntri = mesh.positions.len() / 3;
        let parts = pbr.part_ids.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        let with_tex = pbr.part_texture.iter().filter(|t| t.is_some()).count();
        eprintln!(
            "GLB: {ntri} tris, {} textures, {parts} parts ({with_tex} with a base colour), uvs={}",
            pbr.textures.len(), pbr.uvs.len(),
        );
        assert!(!pbr.textures.is_empty(), "extracts material images");
        assert_eq!(pbr.part_ids.len(), ntri, "one part id per triangle");
        assert!(parts >= 2, "multi-material → multiple parts");
        for (i, (w, h, _)) in pbr.textures.iter().enumerate() {
            eprintln!("  tex {i}: {w}x{h}");
        }
    }

    /// A minimal ASCII FBX (one quad, one material with a diffuse colour, wired through
    /// Connections) parses to two triangles, carries UVs, and gets a per-part colour swatch — the
    /// path that was previously a hard "ASCII FBX unsupported" bail.
    #[test]
    fn ascii_fbx_reads_geometry_uvs_and_material_colour() {
        let src = br#"; FBX 7.7.0 project file
GlobalSettings:  {
    Properties70:  {
        P: "UpAxis", "int", "Integer", "",1
    }
}
Objects:  {
    Geometry: 1001, "Geometry::", "Mesh" {
        Vertices: *12 {
            a: 0,0,0,1,0,0,1,0,1,0,0,1
        }
        PolygonVertexIndex: *4 {
            a: 0,1,2,-4
        }
        LayerElementUV: 0 {
            MappingInformationType: "ByPolygonVertex"
            ReferenceInformationType: "IndexToDirect"
            UV: *8 {
                a: 0,0,1,0,1,1,0,1
            }
            UVIndex: *4 {
                a: 0,1,2,3
            }
        }
        LayerElementMaterial: 0 {
            MappingInformationType: "ByPolygon"
            Materials: *1 {
                a: 0
            }
        }
    }
    Model: 2001, "Model::stair", "Mesh" {
    }
    Material: 3001, "Material::wood", "" {
        Properties70:  {
            P: "DiffuseColor", "Color", "", "A",0.6,0.4,0.2
        }
    }
}
Connections:  {
    C: "OO",1001,2001
    C: "OO",3001,2001
}
"#;
        let (mesh, pbr) = parse_fbx_ascii(src, None);
        assert_eq!(mesh.tri_count(), 2, "a quad fan-triangulates to two tris");
        assert_eq!(pbr.uvs.len(), mesh.positions.len(), "one UV per emitted vertex");
        assert_eq!(pbr.part_ids.len(), mesh.tri_count(), "one part id per triangle");
        assert_eq!(pbr.textures.len(), 1, "the material's diffuse becomes one swatch");
        assert_eq!(pbr.part_texture, vec![Some(0)], "the part points at that swatch");
        // Y-up→Z-up: local (1,0,1) becomes (1,-1,0).
        assert!(mesh.positions.iter().any(|p| (p[1] + 1.0).abs() < 1e-5), "Y-up converted to Z-up");
        // Diffuse 0.6,0.4,0.2 → the 1×1 swatch pixels.
        let (w, h, px) = &pbr.textures[0];
        assert_eq!((*w, *h), (1, 1));
        assert_eq!(px[0], (0.6f32 * 255.0) as u8);
    }

    /// INVESTIGATIVE (ignored): parse the user's real ASCII FBX staircase and print what comes out.
    ///   cargo test -p cad_app ascii_fbx_probe_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ascii_fbx_probe_real() {
        let path = "D:/Dropbox/03--PROJECTS/03-PROJECTS/2026/SAEEDA MAM/SAEEDA MA'AM'S PROJECT-02/NEW LAYOUT/furniture/fbx/sketchup/stairs/modern_stair.fbx";
        let Ok(bytes) = std::fs::read(path) else { eprintln!("no file — skip"); return };
        let base = std::path::Path::new(path).parent();
        let (mesh, pbr) = parse_fbx_ascii(&bytes, base);
        let ntri = mesh.tri_count();
        let parts = pbr.part_ids.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        let img = pbr.textures.iter().filter(|(w, h, _)| *w > 1 || *h > 1).count();
        let with_tex = pbr.part_texture.iter().filter(|t| t.is_some()).count();
        eprintln!(
            "ASCII FBX: {ntri} tris, {parts} parts ({with_tex} bound), textures={} (images={}), uvs={}",
            pbr.textures.len(), img, pbr.uvs.len(),
        );
        for (i, (w, h, _)) in pbr.textures.iter().enumerate() {
            eprintln!("  tex {i}: {w}x{h}");
        }
        assert!(ntri > 0, "produces geometry (was 0 before)");
    }

    #[test]
    fn fbx_binary_reads_one_triangle_uncompressed() {
        let m = parse_fbx(&synth_fbx(false));
        assert_eq!(m.tri_count(), 1, "one polygon → one triangle");
        // Y-up → Z-up: (0,1,0) becomes (0,0,1). The synthetic blob carries no `GlobalSettings`,
        // so it is read as FBX.s default CENTIMETRES — 1 unit lands at 0.01 m.
        assert_eq!(m.positions[2], [0.0, 0.0, 0.01]);
        assert_eq!(m.normals.len(), m.positions.len());
    }

    #[test]
    fn fbx_binary_reads_one_triangle_zlib_compressed() {
        let m = parse_fbx(&synth_fbx(true));
        assert_eq!(m.tri_count(), 1, "compressed arrays inflate to the same triangle");
        // Centimetres, as above — no `GlobalSettings` in the synthetic blob.
        assert_eq!(m.positions[1], [0.01, 0.0, 0.0]);
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

    /// A hand-built single-triangle GLB parses to one triangle, with glTF's Y-up converted to
    /// the app's Z-up: the glTF vertex (0,1,0) must land at (0,0,1).
    #[test]
    fn gltf_glb_single_triangle_y_up_to_z_up() {
        // BIN: 3 positions (VEC3 f32) then 3 indices (u16).
        let pos: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut bin = Vec::new();
        for f in pos { bin.extend_from_slice(&f.to_le_bytes()); }
        for i in [0u16, 1, 2] { bin.extend_from_slice(&i.to_le_bytes()); }
        let json = format!(r#"{{
            "asset":{{"version":"2.0"}},
            "scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],
            "buffers":[{{"byteLength":{}}}],
            "bufferViews":[
                {{"buffer":0,"byteOffset":0,"byteLength":36}},
                {{"buffer":0,"byteOffset":36,"byteLength":6}}],
            "accessors":[
                {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
                {{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}]
        }}"#, bin.len());
        // Assemble the GLB container.
        let mut jb = json.into_bytes();
        while jb.len() % 4 != 0 { jb.push(b' '); }
        while bin.len() % 4 != 0 { bin.push(0); }
        let total = 12 + 8 + jb.len() + 8 + bin.len();
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF"); // magic — the REAL bytes, so this exercises glb_split
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(jb.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
        glb.extend_from_slice(&jb);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        glb.extend_from_slice(&bin);

        let m = parse_gltf(&glb, None);
        assert_eq!(m.tri_count(), 1, "one triangle");
        assert_eq!(m.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
        // (0,1,0) in glTF Y-up → (0,0,1) in app Z-up.
        assert_eq!(m.positions[2], [0.0, 0.0, 1.0]);
    }

    /// Wrap a JSON doc + BIN chunk into a GLB container.
    fn glb_of(json: String, mut bin: Vec<u8>) -> Vec<u8> {
        let mut jb = json.into_bytes();
        while jb.len() % 4 != 0 { jb.push(b' '); }
        while bin.len() % 4 != 0 { bin.push(0); }
        let total = 12 + 8 + jb.len() + 8 + bin.len();
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(jb.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
        glb.extend_from_slice(&jb);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        glb.extend_from_slice(&bin);
        glb
    }

    /// One triangle wearing a material with a normal map, a PACKED metallic-roughness map and
    /// `KHR_materials_transmission` — i.e. the shape of every textured material in the villa.
    ///
    /// The packed map is the point: glTF puts occlusion/roughness/metallic in R/G/B of ONE image
    /// and the shader reads `.r` per sampler, so importing it whole would feed the roughness
    /// sampler a picture whose red channel is the AMBIENT OCCLUSION.
    #[test]
    fn gltf_splits_the_packed_metallic_roughness_map() {
        // A 2×1 image with distinguishable channels: px0 = (10, 20, 30), px1 = (40, 50, 60).
        let img = image::RgbaImage::from_raw(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 255]).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();

        let pos: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut bin = Vec::new();
        for f in pos { bin.extend_from_slice(&f.to_le_bytes()); }
        for i in [0u16, 1, 2] { bin.extend_from_slice(&i.to_le_bytes()); }
        while bin.len() % 4 != 0 { bin.push(0); }
        let png_off = bin.len();
        bin.extend_from_slice(&png);

        let json = format!(r#"{{
            "asset":{{"version":"2.0"}},
            "scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}}],
            "materials":[{{
                "pbrMetallicRoughness":{{
                    "baseColorFactor":[0.055,0.3,0.34,1],
                    "roughnessFactor":0.035,"metallicFactor":0.0,
                    "metallicRoughnessTexture":{{"index":0}}}},
                "normalTexture":{{"index":0}},
                "occlusionTexture":{{"index":0}},
                "extensions":{{"KHR_materials_transmission":{{"transmissionFactor":0.45}}}}}}],
            "textures":[{{"source":0}}],
            "images":[{{"bufferView":2,"mimeType":"image/png"}}],
            "buffers":[{{"byteLength":{}}}],
            "bufferViews":[
                {{"buffer":0,"byteOffset":0,"byteLength":36}},
                {{"buffer":0,"byteOffset":36,"byteLength":6}},
                {{"buffer":0,"byteOffset":{},"byteLength":{}}}],
            "accessors":[
                {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
                {{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}]
        }}"#, bin.len(), png_off, png.len());

        let (mesh, pbr) = parse_gltf_ex(&glb_of(json, bin), None);
        assert_eq!(mesh.tri_count(), 1);

        // The scalar still arrives…
        assert!((pbr.part_rough[0] - 0.035).abs() < 1e-6, "roughnessFactor survives");
        // …and so do the maps, each as its OWN slot — one image became four textures.
        let (n, r, m, o) = (pbr.part_normal[0], pbr.part_rough_map[0],
                            pbr.part_metal_map[0], pbr.part_ao_map[0]);
        for (name, s) in [("normal", n), ("roughness", r), ("metallic", m), ("occlusion", o)] {
            assert!(s.is_some(), "the {name} map must be imported");
        }
        assert_eq!([n, r, m, o].iter().flatten().collect::<std::collections::HashSet<_>>().len(), 4,
            "four distinct channels ⇒ four distinct slots, not one image reused four times");

        // The normal map keeps all three channels; the others are their own channel broadcast.
        let px = |slot: Option<usize>| pbr.textures[slot.unwrap()].2.clone();
        assert_eq!(&px(n)[0..3], &[10, 20, 30], "a normal map is not a single channel");
        assert_eq!(&px(r)[0..3], &[20, 20, 20], "roughness comes from GREEN");
        assert_eq!(&px(m)[0..3], &[30, 30, 30], "metallic comes from BLUE");
        assert_eq!(&px(o)[0..3], &[10, 10, 10], "occlusion comes from RED");
        assert_eq!(&px(r)[4..7], &[50, 50, 50], "…for every texel, not just the first");

        // Transmission 0.45 ⇒ 55% opaque, even though alphaMode is (correctly) absent/OPAQUE.
        assert!((mesh.alpha[0] - 0.55).abs() < 1e-6,
            "KHR_materials_transmission must make the surface see-through");
    }

    /// A flat `baseColorFactor` has to survive the round trip through the swatch and back out of
    /// the sRGB sampler as the SAME linear colour it was authored as.
    ///
    /// It did not. The factor is linear and was written straight into a byte, which the GPU then
    /// decoded as though it had been sRGB-encoded — so every flat-colour glTF material imported
    /// dark, dark colours worst of all. The pool water's red channel came out at a thirteenth of
    /// what Blender had.
    #[test]
    fn gltf_flat_base_colour_survives_the_srgb_round_trip() {
        let pos: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut bin = Vec::new();
        for f in pos { bin.extend_from_slice(&f.to_le_bytes()); }
        for i in [0u16, 1, 2] { bin.extend_from_slice(&i.to_le_bytes()); }
        let json = format!(r#"{{
            "asset":{{"version":"2.0"}},
            "scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}}],
            "materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[0.055,0.3,0.34,1.0]}}}}],
            "buffers":[{{"byteLength":{}}}],
            "bufferViews":[
                {{"buffer":0,"byteOffset":0,"byteLength":36}},
                {{"buffer":0,"byteOffset":36,"byteLength":6}}],
            "accessors":[
                {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
                {{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}]
        }}"#, bin.len());
        let (_m, pbr) = parse_gltf_ex(&glb_of(json, bin), None);
        let px = &pbr.textures[pbr.part_texture[0].expect("a swatch")].2;
        // Decode the swatch the way the GPU's SRGB8_ALPHA8 sampler does.
        let back: Vec<f32> = px[0..3].iter().map(|&b| crate::color::srgb_to_linear(b as f32 / 255.0)).collect();
        for (got, want) in back.iter().zip([0.055f32, 0.30, 0.34]) {
            assert!((got - want).abs() < 0.01,
                "authored {want}, the shader would see {got} — {:?} vs the linear factor", back);
        }
        assert_eq!(px[3], 255, "alpha is coverage, not colour — never sRGB-encoded");
    }

    /// Garbage bytes → empty mesh, no panic (fail-soft on untrusted files).
    #[test]
    fn gltf_rejects_garbage() {
        assert_eq!(parse_gltf(b"glTFnope not a real file", None).tri_count(), 0);
        assert_eq!(parse_gltf(b"{bad json", None).tri_count(), 0);
    }

    /// Probe a REAL glTF (external `.bin`) — set `RUSTCAD_GLTF_PROBE` to a `.gltf`/`.glb` path.
    /// `cargo test -p cad_app gltf_probe_real -- --ignored`.
    #[test]
    #[ignore]
    fn gltf_probe_real() {
        let Ok(p) = std::env::var("RUSTCAD_GLTF_PROBE") else { return };
        let bytes = std::fs::read(&p).expect("read probe file");
        let base = std::path::Path::new(&p).parent();
        let (m, pbr) = parse_gltf_ex(&bytes, base);
        let (mn, mx) = m.bounds().expect("non-empty");
        let tex = pbr.texture.as_ref().map(|(w, h, _)| (*w, *h));
        eprintln!(
            "gltf probe: {} tris, bbox {:?}..{:?}, uvs={} (verts={}), base_color={:?}",
            m.tri_count(), mn, mx, pbr.uvs.len(), m.positions.len(), tex,
        );
        assert!(m.tri_count() > 0, "real glTF must yield triangles");
        assert_eq!(pbr.uvs.len(), m.positions.len(), "one UV per vertex");
        assert!(pbr.texture.is_some(), "base-colour image extracted");
    }

    /// Regression: the bundled aperture DOOR must load with its full-depth surround shell
    /// dropped (the solid frame-backing that reads as a grey block from behind). Self-skips if
    /// the asset isn't found (path depends on the run's working dir), so it never breaks CI.
    #[test]
    fn door_fbx_drops_the_full_depth_shell() {
        let candidates = [
            "assets/apertures/door.fbx",
            "../assets/apertures/door.fbx",
            r"G:\3d factory\assets\apertures\door.fbx",
        ];
        let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
            eprintln!("door.fbx not found from this cwd — skipping");
            return;
        };
        let full = parse_fbx_ex(&bytes).0;
        let door = parse_fbx_door(&bytes).0;
        assert!(full.tri_count() > 0, "the full door parses");
        assert!(door.tri_count() > 0, "the shell-dropped door still has geometry");
        assert!(
            door.tri_count() < full.tri_count(),
            "the surround shell was dropped: {} → {}", full.tri_count(), door.tri_count()
        );
        // The dropped part was the FRAME, so the door is now shallower (thinner overall depth).
        let dep = |m: &ObjMesh| { let (mn, mx) = m.bounds().unwrap(); (mx[1]-mn[1]).min(mx[0]-mn[0]).min(mx[2]-mn[2]) };
        assert!(dep(&door) <= dep(&full) + 1e-3, "no part grew");
    }

    /// A GLB with the REAL "glTF" magic must split into JSON + BIN (regression for the magic
    /// constant that was "gltF" and rejected every real .glb → silent 0-triangle import).
    #[test]
    fn real_glb_magic_is_accepted() {
        let mut data = Vec::new();
        data.extend_from_slice(b"glTF"); // the actual ASCII magic
        data.extend_from_slice(&2u32.to_le_bytes()); // version
        let json = br#"{"asset":{"version":"2.0"}}"#;
        let mut jb = json.to_vec();
        while jb.len() % 4 != 0 { jb.push(b' '); }
        let bin = vec![1u8, 2, 3, 4];
        let total = 12 + 8 + jb.len() + 8 + bin.len();
        data.extend_from_slice(&(total as u32).to_le_bytes());
        data.extend_from_slice(&(jb.len() as u32).to_le_bytes());
        data.extend_from_slice(b"JSON");
        data.extend_from_slice(&jb);
        data.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        data.extend_from_slice(b"BIN\0");
        data.extend_from_slice(&bin);
        let split = glb_split(&data);
        assert!(split.is_some(), "real 'glTF' magic must be recognised as GLB");
        let (_json, glb_bin) = split.unwrap();
        assert_eq!(glb_bin.as_deref(), Some(&[1u8, 2, 3, 4][..]), "BIN chunk extracted");
    }

    /// The bundled casement window must import with see-through glass: its OBJ's `mtllib` names a
    /// file that isn't shipped (renamed to `window.mtl`), so this also exercises the "scan the
    /// directory for any .mtl" fallback. Self-skips if the asset isn't found from this cwd.
    #[test]
    fn bundled_window_glass_is_translucent() {
        let candidates = [
            "assets/apertures/window.obj",
            "../assets/apertures/window.obj",
            r"G:\3d factory\assets\apertures\window.obj",
        ];
        let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) else {
            eprintln!("window.obj not found from this cwd — skipping");
            return;
        };
        let text = std::fs::read_to_string(path).unwrap();
        let dir = std::path::Path::new(path).parent();
        let m = parse_obj_dir(&text, dir);
        assert!(m.tri_count() > 0, "window parses");
        assert_eq!(m.alpha.len(), m.positions.len(), "per-vertex opacity recovered from the .mtl");
        assert!(
            m.alpha.iter().any(|&a| a < 0.5),
            "at least one glass pane is see-through (Window_Mat d=0.2)"
        );
        assert!(m.alpha.iter().any(|&a| a >= 0.996), "the frame stays opaque");
    }

    /// INVESTIGATIVE (ignored): parse the user's REAL window files and report per-vertex alpha,
    /// so we can see whether transparency is actually recovered from each. Run with:
    ///   cargo test -p cad_app probe_real_window_alpha -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_real_window_alpha() {
        let w = "D:/Dropbox/03--PROJECTS/03-PROJECTS/2026/SAEEDA MAM/SAEEDA MA'AM'S PROJECT-02/NEW LAYOUT/furniture/window";
        let files = [
            format!("{w}/12_Pane_Casement_Window-White_V1_L1.123cb116c362-7bf3-406d-aaa9-b0f0d2f52f5c/16639_12_Pane_Casement_Window-White_V1.obj"),
            format!("{w}/Window/window.obj"),
        ];
        for f in &files {
            match std::fs::read_to_string(f) {
                Ok(text) => {
                    let dir = std::path::Path::new(f).parent();
                    let m = parse_obj_dir(&text, dir);
                    let tris = m.tri_count();
                    let translucent = m.alpha.iter().filter(|&&a| a < 0.996).count();
                    let mn = m.alpha.iter().cloned().fold(f32::INFINITY, f32::min);
                    eprintln!("\n=== {f}\n  tris={tris} alpha_len={} translucent_verts={translucent} min_alpha={}",
                        m.alpha.len(), if mn.is_finite() { mn } else { 1.0 });
                }
                Err(e) => eprintln!("=== {f}\n  (could not read: {e})"),
            }
        }
    }

    #[test]
    fn mtl_opacity_reads_d_and_tr() {
        // `d` (dissolve, 1 = opaque) is taken directly; `Tr` (transparency) is inverted.
        let mtl = "\
            newmtl frame\nd 1.0000\n\
            newmtl glass\nd 0.2000\nTr 0.8000\n\
            newmtl fromtr\nTr 0.3000\n";
        let m = parse_mtl_opacity(mtl);
        assert_eq!(m["frame"], 1.0);
        assert_eq!(m["glass"], 0.2, "d wins when present");
        assert!((m["fromtr"] - 0.7).abs() < 1e-6, "opacity = 1 - Tr when only Tr is given");
    }

    #[test]
    fn obj_dir_tags_glass_faces_translucent() {
        // A quad OBJ: one face uses an opaque material, one a glass material. The companion
        // .mtl is resolved from the base dir, so the glass face's vertices carry low opacity.
        let dir = std::env::temp_dir().join("simlux_obj_alpha_test");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("w.mtl"), "newmtl frame\nd 1.0\nnewmtl glass\nd 0.2\n").unwrap();
        let obj = "\
            mtllib w.mtl\n\
            v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
            usemtl frame\nf 1 2 3\n\
            usemtl glass\nf 1 3 4\n";
        std::fs::write(dir.join("w.obj"), obj).unwrap();
        let text = std::fs::read_to_string(dir.join("w.obj")).unwrap();
        let m = parse_obj_dir(&text, Some(dir.as_path()));
        assert_eq!(m.tri_count(), 2);
        assert_eq!(m.alpha.len(), m.positions.len(), "per-vertex alpha present");
        // Tri 0 = frame (opaque), tri 1 = glass (0.2).
        assert_eq!(&m.alpha[0..3], &[1.0, 1.0, 1.0]);
        assert_eq!(&m.alpha[3..6], &[0.2, 0.2, 0.2]);

        // An all-opaque model leaves `alpha` empty (fast path).
        std::fs::write(dir.join("all.mtl"), "newmtl frame\nd 1.0\n").unwrap();
        let obj2 = "mtllib all.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nusemtl frame\nf 1 2 3\n";
        let m2 = parse_obj_dir(obj2, Some(dir.as_path()));
        assert!(m2.alpha.is_empty(), "opaque mesh carries no alpha array");
    }

    #[test]
    fn gltf_blend_material_alpha() {
        // Only alphaMode:BLEND is see-through, taking baseColorFactor's 4th component.
        let doc: serde_json::Value = serde_json::from_str(
            r#"{"materials":[
                {"alphaMode":"OPAQUE","pbrMetallicRoughness":{"baseColorFactor":[1,1,1,0.3]}},
                {"alphaMode":"BLEND","pbrMetallicRoughness":{"baseColorFactor":[1,1,1,0.3]}},
                {"alphaMode":"BLEND"}
            ]}"#,
        ).unwrap();
        let mat = |i: u64| serde_json::json!({ "material": i });
        assert_eq!(gltf_material_alpha(&doc, &mat(0)), 1.0, "OPAQUE stays solid even with a<1");
        assert!((gltf_material_alpha(&doc, &mat(1)) - 0.3).abs() < 1e-6, "BLEND takes baseColorFactor.a");
        assert_eq!(gltf_material_alpha(&doc, &mat(2)), 1.0, "BLEND with no factor defaults to 1");
        assert_eq!(gltf_material_alpha(&doc, &serde_json::json!({})), 1.0, "no material ⇒ opaque");
    }

    /// `KHR_materials_transmission` is the OTHER way a glTF says see-through, and the one a
    /// physically based exporter actually uses for water and glass. A transmissive material is
    /// `alphaMode:OPAQUE` by design — transmission is not coverage — so reading alphaMode alone
    /// left the villa's pool a solid lid.
    #[test]
    fn gltf_transmission_is_see_through_despite_opaque_alpha_mode() {
        let doc: serde_json::Value = serde_json::from_str(
            r#"{"materials":[
                {"alphaMode":"OPAQUE","extensions":{"KHR_materials_transmission":{"transmissionFactor":0.45}}},
                {"alphaMode":"OPAQUE","extensions":{"KHR_materials_transmission":{}}},
                {"alphaMode":"BLEND","pbrMetallicRoughness":{"baseColorFactor":[1,1,1,0.8]},
                 "extensions":{"KHR_materials_transmission":{"transmissionFactor":0.9}}},
                {"alphaMode":"OPAQUE","extensions":{"KHR_materials_specular":{"specularFactor":0.85}}}
            ]}"#,
        ).unwrap();
        let mat = |i: u64| serde_json::json!({ "material": i });
        assert!((gltf_material_alpha(&doc, &mat(0)) - 0.55).abs() < 1e-6, "0.45 transmitted ⇒ 0.55 opaque");
        assert_eq!(gltf_material_alpha(&doc, &mat(1)), 1.0, "the extension's own default is 0 transmission");
        assert!((gltf_material_alpha(&doc, &mat(2)) - 0.1).abs() < 1e-6,
            "coverage and transmission together ⇒ whichever passes MORE light, not the product");
        assert_eq!(gltf_material_alpha(&doc, &mat(3)), 1.0,
            "a different KHR extension must not be mistaken for transmission");
    }

    /// Transmission has to reach the renderer as ITSELF, not folded into the coverage alpha.
    ///
    /// Both make a surface see-through and only one makes it a VOLUME. Water is a closed box whose
    /// faces are coplanar with the pool liner, so its back faces must be dropped; a leaf card is a
    /// single sheet and dropping its back face deletes the tree from behind. One number cannot say
    /// which of those a material is, and while it was one number the water z-fought.
    #[test]
    fn transmission_is_carried_apart_from_coverage() {
        let doc: serde_json::Value = serde_json::from_str(
            r#"{"materials":[
                {"alphaMode":"OPAQUE","extensions":{"KHR_materials_transmission":{"transmissionFactor":0.45}}},
                {"alphaMode":"BLEND","pbrMetallicRoughness":{"baseColorFactor":[1,1,1,0.4]}}
            ]}"#,
        ).unwrap();
        let mat = |i: u64| serde_json::json!({ "material": i });
        // The pool: a medium.
        assert!((gltf_transmission(&doc, Some(0)) - 0.45).abs() < 1e-6);
        assert!((gltf_material_alpha(&doc, &mat(0)) - 0.55).abs() < 1e-6);
        // A leaf card: just as see-through, and not a medium at all.
        assert_eq!(gltf_transmission(&doc, Some(1)), 0.0, "coverage is not transmission");
        assert!((gltf_material_alpha(&doc, &mat(1)) - 0.4).abs() < 1e-6, "…but it is still see-through");
        assert_eq!(gltf_transmission(&doc, None), 0.0, "no material ⇒ not a medium");
    }
}

#[cfg(test)]
mod villa_import_tests {
    /// What SURFACE the villa's materials actually arrive with — the maps and the transmission,
    /// not just the geometry. Every claim here is about the real exported file.
    ///
    /// `cargo test --release -p cad_app --bin simlux villa_surfaces -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the exported villa scene; run explicitly"]
    fn villa_surfaces_arrive_intact() {
        const P: &str = r"G:\blender dev\staircase\villa scene\villa_scene.glb";
        let path = std::path::Path::new(P);
        assert!(path.exists(), "{P} missing — run build/export_factory.py");
        let bytes = std::fs::read(path).expect("read");
        let (mesh, pbr) = super::parse_gltf_ex(&bytes, path.parent());

        let nparts = pbr.part_texture.len();
        let mapped = (0..nparts).filter(|&i| pbr.part_normal[i].is_some()).count();
        let rough_mapped = (0..nparts).filter(|&i| pbr.part_rough_map[i].is_some()).count();
        println!("{nparts} parts, {} textures, {mapped} with a normal map, {rough_mapped} with a roughness map",
            pbr.textures.len());

        // The five textured surfaces (stucco, painted wood, roof tiles, granite paving, pool
        // tiles) each carry a normal map AND a packed metallic-roughness map. Before this they
        // imported with the base colour alone and rendered dead flat.
        assert!(mapped >= 5, "expected at least the five mapped surfaces, got {mapped}");
        assert!(rough_mapped >= 5, "…each of which also carries roughness, got {rough_mapped}");

        // The pool: authored roughness 0.035 (a mirror) and transmission 0.45.
        let water = (0..nparts)
            .find(|&i| (pbr.part_rough[i] - 0.035).abs() < 1e-4)
            .expect("the pool_water part, by its unmistakable roughness");
        println!("pool_water: part {water}, rough {:.3}, metal {:.3}", pbr.part_rough[water], pbr.part_metal[water]);
        // …and the colour the SHADER will see: the swatch decoded the way an sRGB sampler decodes it.
        let sw = &pbr.textures[pbr.part_texture[water].expect("water swatch")].2;
        let lin: Vec<f32> = sw[0..3].iter().map(|&b| crate::color::srgb_to_linear(b as f32 / 255.0)).collect();
        println!("pool_water albedo: bytes {:?} → linear {:.3?} (authored 0.055, 0.300, 0.340)", &sw[0..4], lin);
        for (got, want) in lin.iter().zip([0.055f32, 0.30, 0.34]) {
            assert!((got - want).abs() < 0.01, "the water must keep its own colour: {lin:.3?}");
        }
        assert_eq!(pbr.part_metal[water], 0.0, "water is a dielectric — the case that used to reflect nothing");

        // …and it must reach the mesh see-through, which only the transmission extension says.
        let clear = mesh.alpha.iter().filter(|&&a| (a - 0.55).abs() < 1e-4).count();
        assert!(clear > 0, "the pool surface must import at 55% opacity (0.45 transmitted)");
        println!("{clear} vertices at 0.55 opacity — the water reads as water");
    }

    /// What SHAPE is the pool water, and does any of it sit on the same plane as the pool liner?
    ///
    /// Coincident faces z-fight, and z-fighting between two triangulated quads is exactly the
    /// shifting triangular wedge pattern that appeared on the water — so this is worth knowing as
    /// a fact rather than as a theory about the renderer.
    ///
    /// `cargo test --release -p cad_app --bin simlux villa_water_geometry -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the exported villa scene; run explicitly"]
    fn villa_water_geometry() {
        const P: &str = r"G:\blender dev\staircase\villa scene\villa_scene.glb";
        let path = std::path::Path::new(P);
        assert!(path.exists(), "{P} missing");
        let bytes = std::fs::read(path).expect("read");
        let (mesh, pbr) = super::parse_gltf_ex(&bytes, path.parent());

        // Group each part's triangles by the PLANE they lie in (normal, offset), quantised.
        let mut planes: std::collections::HashMap<(usize, [i64; 4]), usize> = Default::default();
        for (t, part) in pbr.part_ids.iter().enumerate() {
            let p: Vec<glam::Vec3> = (0..3).map(|k| glam::Vec3::from(mesh.positions[t * 3 + k])).collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            if n.length_squared() < 1e-12 {
                continue;
            }
            let n = n.normalize();
            let d = n.dot(p[0]);
            let q = |v: f32| (v * 1000.0).round() as i64;
            *planes.entry((*part as usize, [q(n.x), q(n.y), q(n.z), q(d)])).or_default() += 1;
        }

        let water = (0..pbr.part_rough.len()).find(|&i| (pbr.part_rough[i] - 0.035).abs() < 1e-4).unwrap();
        let mut mine: Vec<_> = planes.iter().filter(|((p, _), _)| *p == water).collect();
        mine.sort_by_key(|((_, k), _)| *k);
        println!("pool_water is part {water}: {} distinct planes", mine.len());
        for ((_, k), n) in &mine {
            println!("   normal ({:.2},{:.2},{:.2})  offset {:.3} m   {n} triangles",
                k[0] as f32 / 1000.0, k[1] as f32 / 1000.0, k[2] as f32 / 1000.0, k[3] as f32 / 1000.0);
        }

        // Does any OTHER part share one of those planes?
        let mut clashes = 0;
        for ((_, k), _) in &mine {
            for ((op, ok), on) in &planes {
                // Same plane, opposite-facing counts too — a lid and a floor meet nose to nose.
                let flipped = [-ok[0], -ok[1], -ok[2], -ok[3]];
                if *op != water && (ok == k || flipped == *k) {
                    println!("   !! part {op} shares that plane ({on} triangles) — these z-fight");
                    clashes += 1;
                }
            }
        }
        println!("\n{clashes} coincident-plane clashes against the water");
    }

    /// Load the exported villa scene through the real importer and report what it costs.
    ///
    /// The app auto-loads this file at startup, so "does it parse" and "how long does it take" are
    /// startup-blocking questions. Ignored because it depends on a 119 MB file outside the repo.
    ///
    /// `cargo test --release -p cad_app --bin simlux villa_scene_imports -- --ignored --nocapture`
    #[test]
    #[ignore = "needs the exported villa scene; run explicitly"]
    fn villa_scene_imports() {
        const P: &str = r"G:\blender dev\staircase\villa scene\villa_scene.glb";
        let path = std::path::Path::new(P);
        assert!(path.exists(), "{P} missing — run build/export_factory.py");
        let t0 = std::time::Instant::now();
        let bytes = std::fs::read(path).expect("read");
        let read_ms = t0.elapsed().as_millis();
        let t1 = std::time::Instant::now();
        let (mesh, pbr) = super::parse_gltf_ex(&bytes, path.parent());
        let parse_ms = t1.elapsed().as_millis();

        let tris = mesh.positions.len() / 3;
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in &mesh.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        let tex_px: usize = pbr.textures.iter().map(|(w, h, _)| *w as usize * *h as usize).sum();
        println!("file      : {:.1} MB, read {read_ms} ms", bytes.len() as f64 / 1e6);
        println!("parse     : {parse_ms} ms");
        println!("triangles : {tris}");
        println!("uvs       : {} (verts {})", pbr.uvs.len(), mesh.positions.len());
        println!("parts     : {} distinct", pbr.part_texture.len());
        println!("textures  : {} images, {:.1} Mpx, {:.0} MB RGBA",
                 pbr.textures.len(), tex_px as f64 / 1e6, tex_px as f64 * 4.0 / 1e6);
        println!("bounds    : {mn:?} .. {mx:?}");
        println!("size      : {:?} m", [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]]);

        assert!(tris > 1_500_000, "expected the full scene, got {tris} triangles");
        assert_eq!(pbr.uvs.len(), mesh.positions.len(), "every vertex must carry a UV");
        assert_eq!(pbr.part_ids.len(), tris, "one part id per triangle");
        // Authored in metres at real-world size — the autoload depends on this.
        let longest = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
        assert!((80.0..95.0).contains(&longest), "expected an ~86 m site, got {longest} m");
        assert!(mx[2] - mn[2] > 8.0 && mx[2] - mn[2] < 14.0, "expected a ~10.6 m tall scene");
    }
}

#[cfg(test)]
mod import_scale_tests {
    use crate::factory::FactoryState;
    use crate::mesh_io::ObjMesh;

    /// A cuboid mesh `sx × sy × sz` metres, as two triangles per face is unnecessary — one
    /// degenerate-free triangle spanning the box's extremes is enough to exercise the bounds path.
    fn box_mesh(sx: f32, sy: f32, sz: f32) -> ObjMesh {
        let mut m = ObjMesh::default();
        m.positions = vec![[0.0, 0.0, 0.0], [sx, sy, 0.0], [sx, sy, sz]];
        m.normals = vec![[0.0, 0.0, 1.0]; 3];
        m
    }

    /// The autoload's whole correctness rests on this: `add_furniture_asset` shrinks a big import
    /// toward 1.5 m, and `import_scale` must record exactly enough to undo it. Getting it wrong is
    /// how the previous autoload ended up carrying a hand-tuned `×35` that was right for one file
    /// and wrong for any other.
    #[test]
    fn import_scale_undoes_the_normalisation_exactly() {
        let mut st = FactoryState::default();
        // An 86 m site — the villa scene. Normalised down, so the factor must be < 1.
        let a = st.add_furniture_asset("villa".into(), box_mesh(86.0, 74.0, 10.6));
        let asset = &st.furniture_lib[a];
        assert!(asset.import_scale < 1.0, "a big model is shrunk: {}", asset.import_scale);
        let longest = |a: &crate::factory::FurnitureAsset| {
            let e = [a.local_max[0] - a.local_min[0], a.local_max[1] - a.local_min[1], a.local_max[2] - a.local_min[2]];
            e[0].max(e[1]).max(e[2])
        };
        assert!((longest(asset) - 1.5).abs() < 1e-3, "normalised to 1.5 m, got {}", longest(asset));
        // Undoing it must restore the real 86 m.
        let world = longest(asset) / asset.import_scale;
        assert!((world - 86.0).abs() < 0.01, "1/import_scale must restore real size, got {world}");

        // A normal-sized piece is left alone, and undoing is then a no-op.
        let b = st.add_furniture_asset("chair".into(), box_mesh(0.6, 0.6, 0.9));
        assert_eq!(st.furniture_lib[b].import_scale, 1.0, "an in-range model is not rescaled");
        assert!((longest(&st.furniture_lib[b]) - 0.9).abs() < 1e-4);

        // …and a millimetre export is scaled UP, so the factor exceeds 1.
        let c = st.add_furniture_asset("tiny".into(), box_mesh(0.02, 0.01, 0.03));
        assert!(st.furniture_lib[c].import_scale > 1.0, "a sub-50 mm model is grown");
        assert!((longest(&st.furniture_lib[c]) - 1.5).abs() < 1e-3);
    }
}

#[cfg(test)]
mod gltf_material_dump {
    /// Dump every material in a GLB: whether it carries a base-colour IMAGE or only a flat factor.
    /// `cargo test --release -p cad_app --bin simlux dump_gltf_materials -- --ignored --nocapture`
    #[test]
    #[ignore = "diagnostic"]
    fn dump_gltf_materials() {
        let p = std::env::var("GLB").unwrap_or_else(|_| r"G:\blender dev\staircase\villa scene\villa_scene.glb".into());
        let bytes = std::fs::read(&p).expect("read glb");
        let (json, _) = super::glb_split(&bytes).expect("glb");
        let doc: serde_json::Value = serde_json::from_str(&json).expect("json");
        let mats = doc["materials"].as_array().cloned().unwrap_or_default();
        println!("{} materials, {} textures, {} images",
            mats.len(),
            doc["textures"].as_array().map(|a| a.len()).unwrap_or(0),
            doc["images"].as_array().map(|a| a.len()).unwrap_or(0));
        let mut with_img = 0;
        for (i, m) in mats.iter().enumerate() {
            let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let ti = m.pointer("/pbrMetallicRoughness/baseColorTexture/index").and_then(|v| v.as_u64());
            let f = m.pointer("/pbrMetallicRoughness/baseColorFactor")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(","))
                .unwrap_or_else(|| "1,1,1,1 (default)".into());
            let nrm = m.pointer("/normalTexture/index").is_some();
            let mr = m.pointer("/pbrMetallicRoughness/metallicRoughnessTexture/index").is_some();
            let img = ti.and_then(|t| doc["textures"][t as usize].get("source").and_then(|v| v.as_u64()))
                .and_then(|s| doc["images"][s as usize].get("name").and_then(|v| v.as_str()).map(|x| x.to_string()));
            if img.is_some() { with_img += 1; }
            println!("{i:>3}  {name:<34} base={:<28} factor=[{f}] {}{}",
                img.unwrap_or_else(|| "— NO IMAGE —".into()),
                if nrm { "+nrm" } else { "" }, if mr { "+mr" } else { "" });
        }
        println!("\n{with_img}/{} materials carry a base-colour image", mats.len());
    }
}

#[cfg(test)]
mod villa_perf_tests {
    /// Time every stage the app runs when the villa scene loads and then draws.
    ///
    /// "Laggy and unusable" is a symptom with several candidate causes; this separates them so the
    /// fix lands on the one that actually costs the time rather than the one that looks expensive.
    ///
    /// `cargo test --release -p cad_app --bin simlux villa_pipeline_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "profiling; needs the exported villa scene"]
    fn villa_pipeline_timing() {
        use std::time::Instant;
        const P: &str = r"G:\blender dev\staircase\villa scene\villa_scene.glb";
        if !std::path::Path::new(P).exists() {
            eprintln!("{P} missing — skipping");
            return;
        }
        let bytes = std::fs::read(P).unwrap();
        let t = Instant::now();
        let (mesh, pbr) = super::parse_gltf_ex(&bytes, std::path::Path::new(P).parent());
        println!("parse_gltf_ex        {:>8.0} ms   ({} tris)", t.elapsed().as_secs_f64() * 1e3, mesh.positions.len() / 3);

        let mut st = crate::factory::FactoryState::default();
        let t = Instant::now();
        let idx = st.add_furniture_asset("villa".into(), mesh);
        println!("add_furniture_asset  {:>8.0} ms", t.elapsed().as_secs_f64() * 1e3);

        // The app attaches the glTF UVs + part ids after adding the asset.
        let t = Instant::now();
        {
            let a = &mut st.furniture_lib[idx];
            a.uvs = pbr.uvs.clone();
            a.part_ids = pbr.part_ids.clone();
        }
        println!("attach uvs+parts     {:>8.0} ms", t.elapsed().as_secs_f64() * 1e3);

        // THE suspect: a coplanar flood fill + weld over every triangle, run lazily on first draw.
        let t = Instant::now();
        let g = st.furniture_lib[idx].group_geom();
        let el = t.elapsed().as_secs_f64() * 1e3;
        let nface = g.face.iter().max().map(|m| m + 1).unwrap_or(0);
        let nbody = g.body.iter().max().map(|m| m + 1).unwrap_or(0);
        println!("group_geom           {:>8.0} ms   ({nface} faces, {nbody} bodies)  <-- once, on first draw", el);

        let t = Instant::now();
        let _ = st.furniture_lib[idx].group_geom();
        println!("group_geom (cached)  {:>8.3} ms", t.elapsed().as_secs_f64() * 1e3);

        let t = Instant::now();
        let n = st.furniture_lib[idx].is_translucent();
        println!("is_translucent       {:>8.0} ms   ({n})", t.elapsed().as_secs_f64() * 1e3);
    }
}

#[cfg(test)]
mod heavy_group_tests {
    use crate::factory::{FactoryState, COPLANAR_TRI_LIMIT};
    use crate::mesh_io::ObjMesh;

    /// `n` disjoint triangles — enough geometry to cross the heavy-mesh threshold without needing
    /// a real model.
    fn soup(n: usize) -> ObjMesh {
        let mut m = ObjMesh::default();
        m.positions.reserve(n * 3);
        for i in 0..n {
            let x = i as f32 * 0.01;
            m.positions.push([x, 0.0, 0.0]);
            m.positions.push([x + 0.005, 0.0, 0.0]);
            m.positions.push([x, 0.005, 0.0]);
        }
        m.normals = vec![[0.0, 0.0, 1.0]; n * 3];
        m
    }

    /// A heavy import must NOT run the coplanar flood fill. It used to, on the UI thread, at first
    /// draw: 3.7 s in release for the villa scene and far worse in a debug build — which is what
    /// "unresponsive after loading" was. The parts it falls back to are also the better grouping at
    /// that size (25 materials, versus 407,858 coplanar regions nobody can select individually).
    #[test]
    fn a_heavy_import_groups_by_material_part_not_by_coplanar_region() {
        let n = COPLANAR_TRI_LIMIT + 1000;
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("heavy".into(), soup(n));
        // Three source materials, as a real import would carry.
        st.furniture_lib[idx].part_ids = (0..n).map(|i| (i % 3) as u32).collect();

        let t = std::time::Instant::now();
        let g = st.furniture_lib[idx].group_geom();
        let ms = t.elapsed().as_secs_f64() * 1e3;

        assert_eq!(g.face.len(), n, "one group id per triangle");
        let faces: std::collections::BTreeSet<u32> = g.face.iter().copied().collect();
        assert_eq!(faces.len(), 3, "the material parts ARE the faces on a heavy mesh: {faces:?}");
        assert_eq!(g.body, g.face, "and the bodies match them");
        // A generous ceiling: the flood fill on this many triangles takes hundreds of ms even in
        // release, so anything near it means the shortcut stopped working.
        assert!(ms < 200.0, "heavy grouping must be near-free, took {ms:.0} ms");

        // A mesh with no part ids at all degrades to ONE group rather than to a long wait.
        let idx2 = st.add_furniture_asset("heavy_bare".into(), soup(n));
        let g2 = st.furniture_lib[idx2].group_geom();
        assert!(g2.face.iter().all(|&f| f == 0), "no parts ⇒ a single whole-object group");

        // …and a SMALL mesh still gets the real coplanar grouping, which is what makes
        // click-a-face texturing work on ordinary furniture.
        let mut st3 = FactoryState::default();
        let small = st3.add_furniture_asset("small".into(), soup(64));
        let g3 = st3.furniture_lib[small].group_geom();
        let f3: std::collections::BTreeSet<u32> = g3.face.iter().copied().collect();
        assert!(f3.len() > 1, "a small mesh keeps per-face grouping ({} groups)", f3.len());
    }
}

#[cfg(test)]
mod gltf_texture_content {
    /// The average colour of every base-colour image the villa exports, so a "texture is present"
    /// claim can be checked against "the texture is not white". The roof and the lawn both shipped
    /// as present-but-white before the exporter learned to pick the albedo and bake procedurals.
    #[test]
    #[ignore = "diagnostic"]
    fn dump_texture_average_colours() {
        let p = std::env::var("GLB").unwrap_or_else(|_| r"G:\blender dev\staircase\villa scene\villa_scene.glb".into());
        let bytes = std::fs::read(&p).expect("read glb");
        let (_m, pbr) = super::parse_gltf_ex(&bytes, std::path::Path::new(&p).parent());
        println!("{} textures, {} parts", pbr.textures.len(), pbr.part_texture.len());
        for (i, (w, h, rgba)) in pbr.textures.iter().enumerate() {
            let mut s = [0u64; 3];
            let n = (rgba.len() / 4).max(1);
            for px in rgba.chunks_exact(4) {
                for k in 0..3 {
                    s[k] += px[k] as u64;
                }
            }
            let avg = [s[0] / n as u64, s[1] / n as u64, s[2] / n as u64];
            let white = avg.iter().all(|&c| c > 235);
            println!("  tex {i:>2}  {w:>5}x{h:<5} avg=({:>3},{:>3},{:>3}) {}",
                avg[0], avg[1], avg[2], if white { "<-- ESSENTIALLY WHITE" } else { "" });
        }
    }
}

/// Binary FBX carrying real textures. The fixtures are a cube whose sides wear a four-quadrant
/// image and whose top wears a plain red material — two materials on ONE mesh, so the reader has
/// to honour `LayerElementMaterial` rather than assume one material per geometry. Built by
/// `assets/test/fbx/make_fbx_fixtures.py` (there was no textured FBX on the machine to test with).
#[cfg(test)]
mod fbx_textures {
    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/test/fbx");

    fn load(name: &str) -> (super::ObjMesh, super::GltfPbr) {
        let p = std::path::Path::new(DIR).join(name);
        let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("fixture {}: {e}", p.display()));
        super::parse_fbx_pbr_at(&bytes, p.parent())
    }

    /// The distinct opaque colours in an RGBA image, rounded hard so JPEG-ish drift can't matter.
    fn palette(rgba: &[u8]) -> std::collections::BTreeSet<[u8; 3]> {
        rgba.chunks_exact(4)
            .map(|p| [p[0] & 0xE0, p[1] & 0xE0, p[2] & 0xE0])
            .collect()
    }

    /// The headline claim: an FBX with an image inside it now imports WEARING that image.
    #[test]
    fn an_embedded_texture_is_read_out_of_the_fbx_itself() {
        let (mesh, pbr) = load("tex_cube_embedded.fbx");
        assert_eq!(mesh.tri_count(), 12, "a cube is 12 triangles");
        let images: Vec<_> = pbr.textures.iter().filter(|(w, h, _)| *w > 1 || *h > 1).collect();
        assert_eq!(images.len(), 1, "one real image, got textures {:?}",
            pbr.textures.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>());
        let (w, h, rgba) = images[0];
        assert_eq!((*w, *h), (64, 64));
        // The four quadrant colours must all survive — a wrong decode gives grey or one colour.
        let pal = palette(rgba);
        for want in [[0xE0, 0, 0], [0, 0xE0, 0], [0, 0, 0xE0], [0xE0, 0xE0, 0]] {
            assert!(pal.contains(&want), "quadrant {want:?} missing from {pal:?}");
        }
    }

    /// The same model with the texture left on disk beside it must import identically — that is
    /// the far more common layout (`model.fbx` + a `textures/` folder).
    #[test]
    fn a_texture_file_beside_the_fbx_resolves_the_same_way() {
        let (_, emb) = load("tex_cube_embedded.fbx");
        let (_, ext) = load("tex_cube_external.fbx");
        let img = |p: &super::GltfPbr| p.textures.iter().find(|(w, h, _)| *w > 1 || *h > 1).cloned();
        let (a, b) = (img(&emb).expect("embedded image"), img(&ext).expect("external image"));
        assert_eq!((a.0, a.1), (b.0, b.1), "same image dimensions either way");
        assert_eq!(palette(&a.2), palette(&b.2), "same pixels either way");
    }

    /// Two materials on one mesh: the reader must split the mesh by per-polygon material index.
    /// Before this, whichever material connected first won and the whole cube wore one appearance.
    #[test]
    fn one_mesh_with_two_materials_splits_into_two_parts() {
        let (mesh, pbr) = load("tex_cube_embedded.fbx");
        assert_eq!(pbr.part_ids.len(), mesh.tri_count(), "one part id per triangle");
        let parts: std::collections::BTreeSet<u32> = pbr.part_ids.iter().copied().collect();
        assert_eq!(parts.len(), 2, "textured sides and the red top are separate parts");

        // The red top is exactly one quad = 2 triangles; the textured sides are the other 10.
        let mut count = std::collections::BTreeMap::new();
        for p in &pbr.part_ids {
            *count.entry(*p).or_insert(0usize) += 1;
        }
        let mut sizes: Vec<usize> = count.values().copied().collect();
        sizes.sort();
        assert_eq!(sizes, vec![2, 10], "the +Z face is the two-triangle part");

        // …and they must point at DIFFERENT appearances: one image, one red swatch.
        let of = |p: u32| pbr.part_texture[p as usize].map(|s| &pbr.textures[s]);
        let big = count.iter().max_by_key(|(_, n)| **n).map(|(p, _)| *p).unwrap();
        let small = count.iter().min_by_key(|(_, n)| **n).map(|(p, _)| *p).unwrap();
        let (bw, bh, _) = of(big).expect("sides have an appearance");
        assert!(*bw > 1 && *bh > 1, "the sides wear the image");
        let (sw, sh, srgba) = of(small).expect("top has an appearance");
        assert_eq!((*sw, *sh), (1, 1), "the top has no image, just its colour");
        assert!(srgba[0] > 150 && srgba[1] < 80 && srgba[2] < 80, "the top is red, got {srgba:?}");
    }

    /// Two images on one material — the quadrant colour map and a normal map. The reader must
    /// choose by what each image FEEDS (`OP … "DiffuseColor"`), not by which it meets first.
    /// Getting this wrong is not hypothetical: the villa roof rendered WHITE because a curvature
    /// mask was picked as its colour.
    #[test]
    fn a_normal_map_is_not_mistaken_for_the_base_colour() {
        let (_, pbr) = load("tex_cube_normalmap.fbx");
        let sides = pbr
            .part_ids
            .iter()
            .copied()
            .max_by_key(|p| pbr.part_ids.iter().filter(|q| *q == p).count())
            .expect("parts");
        let (w, h, rgba) = pbr.part_texture[sides as usize]
            .map(|s| &pbr.textures[s])
            .expect("the sides have an appearance");
        assert_eq!((*w, *h), (64, 64), "the 64² colour map, not the 32² normal map");
        // A normal map is flat lilac everywhere; the colour map has four saturated quadrants.
        let pal = palette(rgba);
        assert!(pal.contains(&[0xE0, 0, 0]), "expected the colour map's red, got {pal:?}");
    }

    /// The discriminating half of the case above: a material wearing ONLY a normal map. There is
    /// no colour image to prefer, so the reader must fall back to the material's diffuse colour —
    /// binding the normal map would paint the cube flat lilac, a picture of its own bumps.
    #[test]
    fn a_material_with_only_a_normal_map_falls_back_to_its_colour() {
        let (_, pbr) = load("tex_cube_normalonly.fbx");
        let images: Vec<_> = pbr.textures.iter().filter(|(w, h, _)| *w > 1 || *h > 1).collect();
        assert!(images.is_empty(), "no image should be bound, got {:?}",
            images.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>());
        // The sides must carry the material's own blue, not the normal map's lilac.
        let sides = pbr
            .part_ids
            .iter()
            .copied()
            .max_by_key(|p| pbr.part_ids.iter().filter(|q| *q == p).count())
            .expect("parts");
        let (_, _, rgba) = pbr.part_texture[sides as usize]
            .map(|s| &pbr.textures[s])
            .expect("the sides have an appearance");
        assert!(rgba[2] > rgba[0] + 60, "expected the material's blue, got {rgba:?}");
    }

    /// UVs are the other half of "textures work" — an image with no UVs is box-projected, which
    /// looks nothing like what was authored. Cube-projected faces span the full 0..1 square.
    #[test]
    fn uvs_come_through_per_vertex_and_span_the_map() {
        let (mesh, pbr) = load("tex_cube_embedded.fbx");
        assert_eq!(pbr.uvs.len(), mesh.positions.len(), "one UV per emitted vertex");
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for uv in &pbr.uvs {
            for k in 0..2 {
                lo[k] = lo[k].min(uv[k]);
                hi[k] = hi[k].max(uv[k]);
            }
        }
        assert!(lo[0] <= 0.01 && lo[1] <= 0.01, "UVs start at the map origin, got {lo:?}");
        assert!(hi[0] >= 0.99 && hi[1] >= 0.99, "UVs reach the far corner, got {hi:?}");
        // Not all one value — a collapsed UV set would pass the span test on a lucky pair.
        let distinct: std::collections::BTreeSet<[u32; 2]> =
            pbr.uvs.iter().map(|t| [t[0].to_bits(), t[1].to_bits()]).collect();
        assert!(distinct.len() >= 4, "expected varied UVs, got {} distinct", distinct.len());
    }

    /// A geometry-only regression guard: the appearance work must not disturb the mesh. The cube
    /// must come back cubic and centred.
    ///
    /// It comes back at 2 m, not 200: FBX measures in CENTIMETRES and the reader now applies
    /// `GlobalSettings > UnitScaleFactor` — metres = unit x factor / 100. Before that every FBX
    /// imported 100x too big, which furniture normalisation hid and a door handle exposed the
    /// moment it had to sit at TRUE scale.
    #[test]
    fn the_textured_reader_still_places_geometry_correctly() {
        let (mesh, _) = load("tex_cube_embedded.fbx");
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &mesh.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        for k in 0..3 {
            assert!((hi[k] - lo[k] - 2.0).abs() < 1e-4, "axis {k} spans {} m, want 2", hi[k] - lo[k]);
            assert!((hi[k] + lo[k]).abs() < 1e-4, "axis {k} is off centre");
        }
    }
}

#[cfg(test)]
mod fbx_survey {
    /// Dump the node tree of a binary FBX — the ground truth a reader has to be written against.
    /// Run with: cargo test -p cad_app dump_binary_fbx_tree -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic"]
    fn dump_binary_fbx_tree() {
        // Override with SIMLUX_FBX=<path> to dump any other file.
        let path = std::env::var("SIMLUX_FBX").unwrap_or_else(|_| {
            concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/test/fbx/tex_cube_embedded.fbx").into()
        });
        let data = std::fs::read(&path).expect("fixture");
        let v75 = u32::from_le_bytes([data[23], data[24], data[25], data[26]]) >= 7500;
        let mut cur = super::FbxCursor { buf: &data, pos: 27 };
        let root = super::fbx_parse_siblings(&mut cur, v75, data.len());
        fn walk(ns: &[super::FbxNode], d: usize) {
            for n in ns {
                let p: Vec<String> = n.props.iter().map(|v| match v {
                    super::FbxVal::I(x) => format!("I({x})"),
                    super::FbxVal::F(x) => format!("F({x:.3})"),
                    super::FbxVal::S(s) => format!("S({:?})", &s[..s.len().min(60)]),
                    super::FbxVal::Fa(a) => format!("d[{}]", a.len()),
                    super::FbxVal::Ia(a) => format!("i[{}]", a.len()),
                    super::FbxVal::Raw(b) => format!("raw[{}]", b.len()),
                    super::FbxVal::Skip => "SKIP".into(),
                }).collect();
                println!("{:indent$}{} {}", "", n.name, p.join(" "), indent = d * 2);
                if d < 5 { walk(&n.children, d + 1); }
            }
        }
        walk(&root, 0);
    }

    /// The real-world check on the binary-FBX texture reader: point `SIMLUX_FBX` at any textured
    /// FBX and see what comes back — parts, UVs, and the average colour of every image bound, so a
    /// map that decoded to white/grey (the failure that made the villa roof white in glTF) is
    /// visible rather than merely absent.
    ///   SIMLUX_FBX=<file> cargo test -p cad_app --release report_fbx_textures -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic"]
    fn report_fbx_textures() {
        let path = match std::env::var("SIMLUX_FBX") {
            Ok(p) => p,
            Err(_) => {
                println!("set SIMLUX_FBX=<path to a textured .fbx>");
                return;
            }
        };
        let p = std::path::Path::new(&path);
        let bytes = std::fs::read(p).expect("readable fbx");
        let t = std::time::Instant::now();
        let (mesh, pbr) = super::parse_fbx_pbr_at(&bytes, p.parent());
        let ms = t.elapsed().as_millis();
        println!("{}  {:.1} MB, parsed in {ms} ms", p.display(), bytes.len() as f64 / 1e6);
        println!("  triangles {}   parts {}   uvs {}",
            mesh.tri_count(), pbr.part_texture.len(), pbr.uvs.len());
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &mesh.positions {
            for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); }
        }
        println!("  bounds    {:?} .. {:?}  size {:?}", lo, hi,
            [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]]);
        let mut images = 0;
        for (i, (w, h, rgba)) in pbr.textures.iter().enumerate() {
            let n = (rgba.len() / 4).max(1) as u64;
            let mut s = [0u64; 3];
            for px in rgba.chunks_exact(4) {
                for k in 0..3 {
                    s[k] += px[k] as u64;
                }
            }
            let avg = [s[0] / n, s[1] / n, s[2] / n];
            if *w > 1 || *h > 1 {
                images += 1;
                println!("  tex {i:>3}  {w:>5}x{h:<5} avg=({:>3},{:>3},{:>3}){}",
                    avg[0], avg[1], avg[2],
                    if avg.iter().all(|&c| c > 235) { "   <-- ESSENTIALLY WHITE" } else { "" });
            }
        }
        println!("  images {images} of {} appearances", pbr.textures.len());
    }

    /// What does a BINARY FBX actually contain? The importer currently gives these files a 1×1
    /// colour swatch per material and no UVs at all, so "textures don't work on FBX" needs to be
    /// grounded in what is in the files before anything is written to read it.
    #[test]
    #[ignore = "diagnostic"]
    fn survey_binary_fbx_contents() {
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for d in ["assets/cc0/furniture", "assets/apertures"] {
            if let Ok(rd) = std::fs::read_dir(d) {
                paths.extend(rd.flatten().map(|e| e.path()).filter(|p| {
                    p.extension().map(|e| e.eq_ignore_ascii_case("fbx")).unwrap_or(false)
                }));
            }
        }
        for extra in [r"G:\blender dev\staircase\villa model\build\villa_v1.fbx",
                      r"G:\blender dev\staircase\cabinet\build\kitchen_v1.fbx"] {
            let p = std::path::PathBuf::from(extra);
            if p.exists() { paths.push(p); }
        }
        for p in paths {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            let ascii = super::is_ascii_fbx(&bytes);
            let (mesh, pbr) = if ascii {
                super::parse_fbx_ascii(&bytes, p.parent())
            } else {
                super::parse_fbx_pbr_at(&bytes, p.parent())
            };
            let real_imgs = pbr.textures.iter().filter(|(w, h, _)| *w > 1 || *h > 1).count();
            println!("{:<44} {:<7} {:>8} tris  uv={:<5} parts={:<4} tex={} (real images {})",
                p.file_name().unwrap().to_string_lossy(),
                if ascii { "ascii" } else { "binary" },
                mesh.positions.len() / 3,
                !pbr.uvs.is_empty(),
                pbr.part_texture.len(),
                pbr.textures.len(),
                real_imgs);
        }
    }
}
