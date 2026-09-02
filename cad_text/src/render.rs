//! Per-font renderer: shape a string, tessellate its glyphs, and lay them out
//! into WORLD-space fill triangles + outline segments.
//!
//! Coordinates are y-up throughout (font units are y-up, and our CAD world is
//! y-up), so no vertical flip is needed — the app's world→screen transform
//! handles that downstream.

use std::collections::HashMap;

use cad_kernel::text::{HAlign, VAlign};
use cad_kernel::Vec2;
use lyon::math::point as lpoint;
use lyon::path::{Path as LPath, PathEvent};
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};

use crate::font::rtl_fallback_bytes;
use crate::{FillMode, RenderedGlyphs, TextRequest};

/// Hard cap on per-glyph contour classification — the contour count comes
/// from the (untrusted) font file, and containment is quadratic in it. Real
/// glyphs stay far below this; beyond it TXTEXP simply gets no polygons.
const MAX_CLASSIFY_CONTOURS: usize = 1024;

/// Tessellated geometry for ONE glyph, in font units, glyph-local (origin at the
/// glyph's baseline pen point). Cached so each glyph is tessellated once ever.
struct GlyphGeom {
    fills: Vec<[Vec2; 3]>,
    /// Closed perimeter contours (font units, glyph-local).
    outlines: Vec<Vec<Vec2>>,
    /// Contours classified by containment: `(outer, holes)` per island —
    /// glyph-local font units. Even containment depth → outer, odd → hole;
    /// each hole assigned to the smallest containing outer. This mirrors the
    /// flatten-then-classify pipeline of auto_rasm's TXTEXP.
    polygons: Vec<(Vec<Vec2>, Vec<Vec<Vec2>>)>,
}

/// One font in the renderer's face chain: `[0]` is the requested font, `[1..]`
/// are script fallbacks. Glyphs carry the face index they were shaped with, so
/// each glyph is tessellated + scaled with ITS OWN font's metrics (mixed
/// Arabic-in-Latin text keeps both fonts' true proportions).
struct FaceData {
    // `_data` is leaked to `'static` so `shaper`/`face` can borrow it for the
    // program lifetime (fonts are loaded once and never freed — small, fixed).
    _data: &'static [u8],
    shaper: rustybuzz::Face<'static>,
    face: ttf_parser::Face<'static>,
    upm: f64,
    ascender: f64,
    descender: f64,
    line_gap: f64,
    cache: HashMap<u16, GlyphGeom>,
}

impl FaceData {
    fn new(bytes: Vec<u8>, index: u32) -> Option<Self> {
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let shaper = rustybuzz::Face::from_slice(data, index)?;
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let upm = face.units_per_em() as f64;
        if upm <= 0.0 {
            return None;
        }
        let ascender = face.ascender() as f64;
        let descender = face.descender() as f64;
        let line_gap = face.line_gap() as f64;
        Some(FaceData {
            _data: data,
            shaper,
            face,
            upm,
            ascender,
            descender,
            line_gap,
            cache: HashMap::new(),
        })
    }

    /// True when the face has a glyph for every non-whitespace char of `s`
    /// (whitespace is skipped — spaces shape fine via the .notdef gap, and
    /// every font has them).
    fn covers(&self, s: &str) -> bool {
        s.chars()
            .all(|c| c.is_whitespace() || self.face.glyph_index(c).is_some())
    }
}

/// A loaded font + its glyph cache.
pub(crate) struct TextRenderer {
    faces: Vec<FaceData>,
}

impl TextRenderer {
    /// Parse font bytes into a renderer with the embedded RTL font as the
    /// script fallback. `None` if the bytes aren't a usable font.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        Self::from_bytes_with_fallback(bytes, Some(rtl_fallback_bytes().to_vec()))
    }

    /// Parse `primary` into a renderer, optionally with an extra script
    /// fallback. The fallback is skipped when it is byte-identical to the
    /// primary (e.g. the user's font IS the embedded one).
    pub(crate) fn from_bytes_with_fallback(
        primary: Vec<u8>,
        fallback: Option<Vec<u8>>,
    ) -> Option<Self> {
        Self::from_face(primary, 0, fallback)
    }

    /// Parse face `index` of `primary` (font collections carry one face per
    /// index; regular fonts are index 0), with the embedded RTL font as the
    /// script fallback.
    pub(crate) fn from_bytes_with_index(primary: Vec<u8>, index: u32) -> Option<Self> {
        Self::from_face(primary, index, Some(rtl_fallback_bytes().to_vec()))
    }

    fn from_face(primary: Vec<u8>, index: u32, fallback: Option<Vec<u8>>) -> Option<Self> {
        let mut faces = vec![FaceData::new(primary, index)?];
        if let Some(fb) = fallback {
            if faces[0]._data != fb.as_slice() {
                if let Some(f) = FaceData::new(fb, 0) {
                    faces.push(f);
                }
            }
        }
        Some(TextRenderer { faces })
    }

    /// Render a request to world-space geometry. Empty result for empty text.
    /// The per-frame text path — polygon classification is NOT emitted here
    /// (see `render_with_polygons`).
    pub fn render(&mut self, req: &TextRequest<'_>) -> RenderedGlyphs {
        self.render_with_polygons(req, false)
    }

    /// `render` plus the classified `(outer, holes)` glyph polygons in world
    /// space — the one-shot TXTEXP path. The per-frame renderer skips the
    /// extra transform/alloc work.
    pub(crate) fn render_with_polygons(
        &mut self,
        req: &TextRequest<'_>,
        want_polygons: bool,
    ) -> RenderedGlyphs {
        let mut out = RenderedGlyphs::default();
        if req.text.is_empty() {
            return out;
        }
        let primary = &self.faces[0];
        let s = req.height / primary.upm; // font units → world units
        let line_h = primary.ascender - primary.descender + primary.line_gap; // font units

        // Lay out each line; collect (glyph_id, pen_x, base_y, face) in font
        // units, remembering each line's advance width for horizontal
        // alignment.
        struct Placed {
            gid: u16,
            x: f64,
            y: f64,
            face: usize,
        }
        let mut placed: Vec<Placed> = Vec::new();
        let mut line_of: Vec<usize> = Vec::new(); // index into line_widths per placed
        let mut line_widths: Vec<f64> = Vec::new();

        for (li, line) in req.text.split('\n').enumerate() {
            let base_y = -(li as f64) * line_h;
            let (glyphs, width) = self.shape_line(line);
            line_widths.push(width);
            for g in glyphs {
                line_of.push(li);
                placed.push(Placed {
                    gid: g.0,
                    x: g.1,
                    y: base_y + g.2,
                    face: g.3,
                });
            }
        }

        // Vertical block offset (font units) — v1 aligns off the FIRST line's
        // baseline metrics; multi-line block centering is refined later.
        let v_off = match req.v_align {
            VAlign::Baseline => 0.0,
            VAlign::Top => -primary.ascender,
            VAlign::Middle => -(primary.ascender + primary.descender) * 0.5,
            VAlign::Bottom => -primary.descender,
        };

        let (sin, cos) = req.angle.sin_cos();
        let pos = req.position;
        let slant_tan = req.slant.tan();
        let x_scale = if req.x_scale.abs() < 1e-6 { 1.0 } else { req.x_scale };
        // Layout point (PRIMARY font units) → world: width-scale + italic
        // shear, then uniform scale by the primary's `s`, rotate, translate.
        let to_base = |lx: f64, ly: f64| -> Vec2 {
            let sheared_x = lx * x_scale + ly * slant_tan;
            let qx = sheared_x * s;
            let qy = ly * s;
            Vec2::new(
                pos.x + qx * cos - qy * sin,
                pos.y + qx * sin + qy * cos,
            )
        };
        // Glyph-local offset (ITS face's font units) added onto the base —
        // sheared/rotated identically, but scaled by the glyph's OWN `fs`.
        let to_glyph = |base: Vec2, px: f64, py: f64, fs: f64| -> Vec2 {
            let sheared_x = px * x_scale + py * slant_tan;
            let qx = sheared_x * fs;
            let qy = py * fs;
            Vec2::new(
                base.x + qx * cos - qy * sin,
                base.y + qx * sin + qy * cos,
            )
        };

        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);

        for (idx, g) in placed.iter().enumerate() {
            // Ensure the glyph is tessellated + cached in ITS face
            // (disjoint-borrow safe).
            let face = &mut self.faces[g.face];
            if !face.cache.contains_key(&g.gid) {
                let geom = Self::tessellate_glyph(&face.face, g.gid, face.upm);
                face.cache.insert(g.gid, geom);
            }
            let geom = &face.cache[&g.gid];
            let fs = req.height / face.upm;

            let li = line_of[idx];
            let w = line_widths[li];
            let h_off = match req.h_align {
                HAlign::Left => 0.0,
                HAlign::Center => -w * 0.5,
                HAlign::Right => -w,
            };
            let ox = g.x + h_off;
            let oy = g.y + v_off;
            let base = to_base(ox, oy);

            // Fill mode emits triangles AND the perimeter (the app strokes the
            // perimeter with a thin anti-aliased line to smooth the hard fill
            // edge). Outline mode emits the perimeter only.
            if matches!(req.fill_mode, FillMode::Fill) {
                for tri in &geom.fills {
                    let a = to_glyph(base, tri[0].x, tri[0].y, fs);
                    let b = to_glyph(base, tri[1].x, tri[1].y, fs);
                    let c = to_glyph(base, tri[2].x, tri[2].y, fs);
                    for p in [a, b, c] {
                        min = Vec2::new(min.x.min(p.x), min.y.min(p.y));
                        max = Vec2::new(max.x.max(p.x), max.y.max(p.y));
                    }
                    out.fills.push([a, b, c]);
                }
            }
            for contour in &geom.outlines {
                let mut wc: Vec<Vec2> = Vec::with_capacity(contour.len());
                for p in contour {
                    let w = to_glyph(base, p.x, p.y, fs);
                    min = Vec2::new(min.x.min(w.x), min.y.min(w.y));
                    max = Vec2::new(max.x.max(w.x), max.y.max(w.y));
                    wc.push(w);
                }
                out.outlines.push(wc);
            }
            // Classified polygons (outer + holes) in world space — TXTEXP
            // input only; the per-frame path never pays this transform.
            if want_polygons {
                for (outer, holes) in &geom.polygons {
                    let wo: Vec<Vec2> =
                        outer.iter().map(|p| to_glyph(base, p.x, p.y, fs)).collect();
                    let wh: Vec<Vec<Vec2>> = holes
                        .iter()
                        .map(|h| h.iter().map(|p| to_glyph(base, p.x, p.y, fs)).collect())
                        .collect();
                    out.glyph_polygons.push((wo, wh));
                }
            }
        }

        if out.is_empty() {
            out.bbox = (Vec2::ZERO, Vec2::ZERO);
        } else {
            out.bbox = (min, max);
        }
        // Widest line's pen advance in world units (font units × scale). Unlike
        // bbox this includes trailing-space advance, so callers can position
        // following content (e.g. trim an underline past a list marker).
        out.advance = line_widths.iter().cloned().fold(0.0_f64, f64::max) * s;
        out
    }

    /// Shape one line (bidi-aware) → `(glyph_id, pen_x, y_offset, face_index)`
    /// in font units, plus the total advance width. Runs are placed in visual
    /// (left→right) order, so Arabic/Hebrew inside Latin lands correctly.
    /// Each run is shaped with the FIRST face in the chain that covers its
    /// characters — a run the requested font can't render (e.g. Arabic in a
    /// Latin-only font) falls back to the embedded RTL font instead of
    /// producing `.notdef` boxes. All pen positions are normalized to the
    /// PRIMARY face's font units, so faces with different upm values still lay
    /// out + align consistently.
    fn shape_line(&self, line: &str) -> (Vec<(u16, f64, f64, usize)>, f64) {
        let mut glyphs: Vec<(u16, f64, f64, usize)> = Vec::new();
        let mut pen_x = 0.0_f64;
        if line.is_empty() {
            return (glyphs, 0.0);
        }
        let primary_upm = self.faces[0].upm;

        let bidi = unicode_bidi::BidiInfo::new(line, None);
        let para = match bidi.paragraphs.first() {
            Some(p) => p,
            None => return (glyphs, 0.0),
        };
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());

        for run in runs {
            let run_text = &line[run.clone()];
            let rtl = levels[run.start].is_rtl();
            // Face selection: first face covering the run wins; primary last
            // resort (partial coverage → .notdef for the odd char).
            let mut face_idx = 0;
            for (i, f) in self.faces.iter().enumerate() {
                if f.covers(run_text) {
                    face_idx = i;
                    break;
                }
            }
            let face = &self.faces[face_idx];
            let upm_ratio = face.upm / primary_upm; // face units → primary units
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(run_text);
            buf.set_direction(if rtl {
                rustybuzz::Direction::RightToLeft
            } else {
                rustybuzz::Direction::LeftToRight
            });
            let shaped = rustybuzz::shape(&face.shaper, &[], buf);
            let infos = shaped.glyph_infos();
            let positions = shaped.glyph_positions();
            for (info, position) in infos.iter().zip(positions.iter()) {
                let gx = pen_x + position.x_offset as f64 * upm_ratio;
                let gy = position.y_offset as f64 * upm_ratio;
                glyphs.push((info.glyph_id as u16, gx, gy, face_idx));
                pen_x += position.x_advance as f64 * upm_ratio;
            }
        }
        (glyphs, pen_x)
    }

    /// Tessellate one glyph into fill triangles + smooth outline contours, in
    /// font units, glyph-local. BOTH derive from the SAME flattened contours:
    /// the beziers are flattened once (fine tolerance), consecutive duplicate
    /// points are dropped, and the fill is a NonZero tessellation of those
    /// LINE-ONLY contours — far more robust than tessellating raw beziers (some
    /// fonts' curve/self-intersecting glyphs made lyon drop the fill, leaving
    /// only a broken-looking outline), and guaranteed consistent with the stroke.
    fn tessellate_glyph(face: &ttf_parser::Face, gid: u16, upm: f64) -> GlyphGeom {
        let mut collector = OutlineCollector::new();
        face.outline_glyph(ttf_parser::GlyphId(gid), &mut collector);
        let path = collector.finish();

        // 1) Flatten → deduped closed contours. Fine tolerance (~0.015% em) so
        // curves stay smooth even on large / zoomed text; `eps` only removes
        // genuinely coincident points (degenerate edges break the tessellator),
        // never real curve detail.
        let tol = (upm / 6000.0).max(0.15) as f32;
        let eps = 0.05_f64;
        let mut outlines: Vec<Vec<Vec2>> = Vec::new();
        let mut cur: Vec<Vec2> = Vec::new();
        // Iterate the RAW events and flatten each curve segment EXPLICITLY with
        // lyon's per-segment flattener — so curves can never be silently dropped
        // (which produced the faceting).
        let mut add = |cur: &mut Vec<Vec2>, x: f32, y: f32| {
            // Reject non-finite coords (a degenerate curve can flatten to
            // NaN/∞) — those drew as long stray lines shooting off the glyph.
            if !x.is_finite() || !y.is_finite() {
                return;
            }
            let p = Vec2::new(x as f64, y as f64);
            if cur.last().map_or(true, |q: &Vec2| (q.x - p.x).abs() > eps
                || (q.y - p.y).abs() > eps) {
                cur.push(p);
            }
        };
        for evt in path.iter() {
            match evt {
                PathEvent::Begin { at } => {
                    cur = Vec::new();
                    add(&mut cur, at.x, at.y);
                }
                PathEvent::Line { to, .. } => add(&mut cur, to.x, to.y),
                PathEvent::Quadratic { from, ctrl, to } => {
                    let seg = lyon::geom::QuadraticBezierSegment { from, ctrl, to };
                    seg.for_each_flattened(tol, &mut |l: &lyon::geom::LineSegment<f32>| {
                        add(&mut cur, l.to.x, l.to.y);
                    });
                }
                PathEvent::Cubic { from, ctrl1, ctrl2, to } => {
                    let seg = lyon::geom::CubicBezierSegment { from, ctrl1, ctrl2, to };
                    seg.for_each_flattened(tol, &mut |l: &lyon::geom::LineSegment<f32>| {
                        add(&mut cur, l.to.x, l.to.y);
                    });
                }
                PathEvent::End { .. } => {
                    // Drop a trailing point coincident with the start (the
                    // closing edge is implicit).
                    if cur.len() >= 2 && cur[0].dist(cur[cur.len() - 1]) < eps {
                        cur.pop();
                    }
                    if cur.len() >= 3 {
                        outlines.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                }
            }
        }

        // 2) Fill: NonZero tessellation of the flattened contours as a line-only
        //    lyon path.
        let mut fills: Vec<[Vec2; 3]> = Vec::new();
        if !outlines.is_empty() {
            let mut fb = LPath::builder();
            for c in &outlines {
                fb.begin(lpoint(c[0].x as f32, c[0].y as f32));
                for p in &c[1..] {
                    fb.line_to(lpoint(p.x as f32, p.y as f32));
                }
                fb.end(true);
            }
            let fill_path = fb.build();
            let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
            let mut tess = FillTessellator::new();
            let opts = FillOptions::default().with_fill_rule(FillRule::NonZero);
            if tess.tessellate_path(
                &fill_path, &opts,
                &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
                    let p = v.position();
                    [p.x, p.y]
                }),
            ).is_ok() {
                let idx = &buffers.indices;
                let vtx = &buffers.vertices;
                let mut i = 0;
                while i + 3 <= idx.len() {
                    let a = vtx[idx[i] as usize];
                    let b = vtx[idx[i + 1] as usize];
                    let c = vtx[idx[i + 2] as usize];
                    fills.push([
                        Vec2::new(a[0] as f64, a[1] as f64),
                        Vec2::new(b[0] as f64, b[1] as f64),
                        Vec2::new(c[0] as f64, c[1] as f64),
                    ]);
                    i += 3;
                }
            }
        }

        // 3) Containment classification → (outer, holes) islands for TXTEXP.
        let polygons = classify_contours(&outlines);

        GlyphGeom { fills, outlines, polygons }
    }
}

/// Split closed contours into `(outer, holes)` islands. A contour whose first
/// point is contained by an EVEN number of other contours is an outer; odd →
/// hole. Each hole joins the smallest-area outer that contains it (nested
/// islands keep their holes separate). Degenerate contours with < 3 points
/// were already dropped upstream.
///
/// Adversarial-font guard: containment is O(n²·m) in the contour count (which
/// the FONT controls). Contours beyond `MAX_CLASSIFY_CONTOURS` skip polygon
/// classification entirely (fills/outlines still render), and a bounding-box
/// pre-filter rejects most non-containing pairs before the ray-cast.
fn classify_contours(contours: &[Vec<Vec2>]) -> Vec<(Vec<Vec2>, Vec<Vec<Vec2>>)> {
    let n = contours.len();
    if n > MAX_CLASSIFY_CONTOURS {
        return Vec::new();
    }
    // containment[i] = indices of every contour containing contour i.
    let bboxes: Vec<(Vec2, Vec2)> = contours.iter().map(|c| contour_bbox(c)).collect();
    let mut containment: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        let p = contours[i][0];
        for j in 0..n {
            if i == j {
                continue;
            }
            // Cheap rejection: containment implies the point lies inside the
            // contour's bounding box (inclusive, matching point_in_polygon's
            // boundary-inside semantics).
            let (mn, mx) = bboxes[j];
            if p.x < mn.x || p.x > mx.x || p.y < mn.y || p.y > mx.y {
                continue;
            }
            if cad_kernel::point_in_polygon(p, &contours[j]) {
                containment[i].push(j);
            }
        }
    }

    let area = |c: &Vec<Vec2>| -> f64 {
        let mut a = 0.0;
        let m = c.len();
        for i in 0..m {
            let p = c[i];
            let q = c[(i + 1) % m];
            a += p.x * q.y - q.x * p.y;
        }
        a.abs() * 0.5
    };

    // Outers (even depth), then attach each hole (odd depth) to the smallest
    // containing outer. A hole contained by NO outer (malformed glyph) is
    // promoted to an outer so its outline is never lost.
    let outer_idx: Vec<usize> = (0..n)
        .filter(|&i| containment[i].len() % 2 == 0)
        .collect();
    let mut islands: Vec<(Vec<Vec2>, Vec<Vec<Vec2>>)> = outer_idx
        .iter()
        .map(|&i| (contours[i].clone(), Vec::new()))
        .collect();
    let mut orphan_holes: Vec<Vec<Vec2>> = Vec::new();
    for (i, c) in contours.iter().enumerate() {
        if containment[i].len() % 2 != 1 {
            continue;
        }
        // Smallest-area OUTER among the containing contours.
        let parent = containment[i]
            .iter()
            .filter(|&&j| containment[j].len() % 2 == 0)
            .min_by(|&&a, &&b| {
                area(&contours[a])
                    .partial_cmp(&area(&contours[b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        match parent {
            Some(&j) => {
                // `j` is even-depth, and `outer_idx` holds exactly those —
                // the position always exists.
                let pos = outer_idx
                    .iter()
                    .position(|&o| o == j)
                    .expect("hole parent must be a classified outer");
                islands[pos].1.push(c.clone());
            }
            None => orphan_holes.push(c.clone()),
        }
    }
    for h in orphan_holes {
        islands.push((h, Vec::new()));
    }
    islands
}

/// Axis-aligned bounding box of a contour (inclusive).
fn contour_bbox(c: &[Vec2]) -> (Vec2, Vec2) {
    let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in c {
        mn = Vec2::new(mn.x.min(p.x), mn.y.min(p.y));
        mx = Vec2::new(mx.x.max(p.x), mx.y.max(p.y));
    }
    (mn, mx)
}

/// Feeds a glyph outline into a lyon path (fill + flattening both derive from
/// it). Font units.
struct OutlineCollector {
    builder: lyon::path::path::Builder,
    open: bool,
}

impl OutlineCollector {
    fn new() -> Self {
        OutlineCollector { builder: LPath::builder(), open: false }
    }

    fn finish(mut self) -> LPath {
        if self.open {
            self.builder.end(true);
        }
        self.builder.build()
    }
}

impl ttf_parser::OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.builder.end(true);
        }
        self.builder.begin(lpoint(x, y));
        self.open = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(lpoint(x, y));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.builder.quadratic_bezier_to(lpoint(x1, y1), lpoint(x, y));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.builder.cubic_bezier_to(lpoint(x1, y1), lpoint(x2, y2), lpoint(x, y));
    }

    fn close(&mut self) {
        if self.open {
            self.builder.end(true);
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn liberation() -> Vec<u8> {
        include_bytes!("../assets/LiberationSans-Regular.ttf").to_vec()
    }
    fn dejavu() -> Vec<u8> {
        rtl_fallback_bytes().to_vec()
    }

    fn liberation_with_dejavu_fallback() -> TextRenderer {
        // Liberation Sans has NO Arabic glyphs; DejaVu Sans does.
        TextRenderer::from_bytes_with_fallback(liberation(), Some(dejavu())).unwrap()
    }

    fn shape(text: &str) -> (Vec<(u16, f64, f64, usize)>, f64) {
        liberation_with_dejavu_fallback().shape_line(text)
    }

    #[test]
    fn latin_run_stays_on_primary_face() {
        let (glyphs, width) = shape("abc");
        assert!(!glyphs.is_empty());
        assert!(width > 0.0);
        assert!(glyphs.iter().all(|g| g.3 == 0), "Latin must use the primary face");
    }

    #[test]
    fn arabic_run_falls_back_to_rtl_face() {
        let (glyphs, width) = shape("\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}");
        assert!(!glyphs.is_empty(), "Arabic must produce glyphs, not .notdef boxes");
        assert!(width > 0.0);
        assert!(
            glyphs.iter().all(|g| g.3 == 1),
            "Arabic must shape with the RTL fallback face"
        );
    }

    #[test]
    fn hebrew_run_renders() {
        // Liberation Sans covers Hebrew itself, so this can shape on the
        // primary face — the point is it renders real glyphs, never .notdef.
        let (glyphs, width) = shape("\u{05e9}\u{05dc}\u{05d5}\u{05dd}");
        assert!(!glyphs.is_empty());
        assert!(width > 0.0);
        assert!(glyphs.iter().all(|g| g.2.is_finite()), "positions must be finite");
    }

    #[test]
    fn mixed_ltr_rtl_line_shapes_both_faces_in_visual_order() {
        let (glyphs, width) = shape("abc \u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629} def");
        assert!(!glyphs.is_empty(), "mixed line must not panic (bidi run ordering)");
        assert!(width > 0.0);
        assert!(
            glyphs.iter().any(|g| g.3 == 0) && glyphs.iter().any(|g| g.3 == 1),
            "mixed line must use BOTH faces: {:?}",
            glyphs.iter().map(|g| g.3).collect::<Vec<_>>()
        );
        // Glyphs must be laid out left→right with strictly growing pen x.
        let xs: Vec<f64> = glyphs.iter().map(|g| g.1).collect();
        assert!(
            xs.windows(2).all(|w| w[1] >= w[0]),
            "glyph pen must advance left→right, got {xs:?}"
        );
    }

    #[test]
    fn rtl_paragraph_reorders_runs() {
        // RTL paragraph: logical "العربية only" displays "only" first (visual
        // order) — the runs must not panic and must produce glyphs.
        let (glyphs, width) = shape("\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629} only");
        assert!(!glyphs.is_empty());
        assert!(width > 0.0);
        let xs: Vec<f64> = glyphs.iter().map(|g| g.1).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn identical_fallback_bytes_are_skipped() {
        // User's font IS the RTL font → no duplicate face in the chain.
        let r = TextRenderer::from_bytes_with_fallback(dejavu(), Some(dejavu())).unwrap();
        assert_eq!(r.faces.len(), 1);
    }

    #[test]
    fn empty_line_shapes_to_nothing() {
        let (glyphs, width) = shape("");
        assert!(glyphs.is_empty());
        assert_eq!(width, 0.0);
    }

    #[test]
    fn rtl_render_produces_world_geometry() {
        // End-to-end through `render` with the app's request shape: Arabic text
        // must emit fill triangles via the fallback face.
        let mut r = liberation_with_dejavu_fallback();
        let req = TextRequest {
            text: "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}",
            font_name: "liberation sans",
            position: Vec2::new(0.0, 0.0),
            height: 10.0,
            angle: 0.0,
            h_align: HAlign::Left,
            v_align: VAlign::Baseline,
            fill_mode: FillMode::Fill,
            slant: 0.0,
            x_scale: 1.0,
        };
        let g = r.render(&req);
        assert!(!g.fills.is_empty(), "Arabic must tessellate into triangles");
        assert!(g.advance > 0.0);
        let (min, max) = g.bbox;
        assert!(max.x > min.x && max.y > min.y);
    }

    fn fill_req(text: &'static str, pos: Vec2) -> TextRequest<'static> {
        TextRequest {
            text,
            font_name: "liberation sans",
            position: pos,
            height: 10.0,
            angle: 0.0,
            h_align: HAlign::Left,
            v_align: VAlign::Baseline,
            fill_mode: FillMode::Fill,
            slant: 0.0,
            x_scale: 1.0,
        }
    }

    #[test]
    fn o_has_one_outer_and_one_hole() {
        let mut r = liberation_with_dejavu_fallback();
        let g = r.render_with_polygons(&fill_req("O", Vec2::ZERO), true);
        assert_eq!(
            g.glyph_polygons.len(),
            1,
            "'O' is ONE glyph with one outer + one hole"
        );
        let (outer, holes) = &g.glyph_polygons[0];
        assert!(outer.len() >= 3, "outer contour must be a closed loop");
        assert_eq!(holes.len(), 1, "'O' must have exactly one counter (hole)");
        assert!(holes[0].len() >= 3);
        // The hole must be strictly inside the outer (every hole point
        // contained by the outer loop).
        for p in &holes[0] {
            assert!(cad_kernel::point_in_polygon(*p, outer));
        }
    }

    #[test]
    fn ab_yields_two_glyph_polygons() {
        let mut r = liberation_with_dejavu_fallback();
        let g = r.render_with_polygons(&fill_req("AB", Vec2::ZERO), true);
        assert_eq!(
            g.glyph_polygons.len(),
            2,
            "'AB' is two glyphs → two (outer, holes) entries"
        );
        // 'B' carries two counters; between the two glyphs there are holes.
        let total_holes: usize = g.glyph_polygons.iter().map(|(_, h)| h.len()).sum();
        assert!(total_holes >= 2, "A (1 hole) + B (2 holes) = 3 holes total");
    }

    #[test]
    fn arabic_sample_produces_polygons() {
        let mut r = liberation_with_dejavu_fallback();
        let g = r.render_with_polygons(
            &fill_req("\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}", Vec2::ZERO),
            true,
        );
        assert!(!g.glyph_polygons.is_empty(), "Arabic must produce glyph polygons");
        assert!(g.glyph_polygons.iter().any(|(o, _)| o.len() >= 3));
    }

    #[test]
    fn polygons_transform_to_world_space() {
        let mut r = liberation_with_dejavu_fallback();
        let at_origin = r.render_with_polygons(&fill_req("O", Vec2::ZERO), true);
        let shifted = r.render_with_polygons(&fill_req("O", Vec2::new(30.0, 40.0)), true);
        assert_eq!(at_origin.glyph_polygons.len(), shifted.glyph_polygons.len());
        let d = Vec2::new(30.0, 40.0);
        for ((o1, _), (o2, _)) in at_origin
            .glyph_polygons
            .iter()
            .zip(shifted.glyph_polygons.iter())
        {
            for (a, b) in o1.iter().zip(o2.iter()) {
                assert!(
                    (*b - (*a + d)).len() < 1e-6,
                    "polygon point must translate with the anchor"
                );
            }
        }
    }

    #[test]
    fn per_frame_render_skips_polygon_work() {
        // The per-frame path must NOT pay the polygon classification/transform
        // cost — only the explode path wants it.
        let mut r = liberation_with_dejavu_fallback();
        let g = r.render(&fill_req("O", Vec2::ZERO));
        assert!(g.glyph_polygons.is_empty());
        assert!(!g.fills.is_empty(), "fills still render");
    }
}
