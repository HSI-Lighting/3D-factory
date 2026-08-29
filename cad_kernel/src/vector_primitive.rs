// VectorPrimitive — shape-agnostic intermediate representation for export.
//
// Every Geom variant converts to one or more VectorPrimitive items via
// `Geom::to_vector_primitives()`. Exporters (PDF, SVG, DXF, PNG) consume
// these generically — no per-shape-type dispatch.
//
// Primitives are GEOMETRY ONLY. Color, lineweight, dash, and CTB overrides
// are resolved separately by the caller and applied at emit time.

use crate::Document;
use crate::geom::{Geom, HatchPattern};
use crate::math::Vec2;

/// A single drawing primitive — geometry only, no style.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorPrimitive {
    /// A straight line segment from p0 to p1.
    Segment { p0: Vec2, p1: Vec2 },
    /// A circular arc. `sweep_angle` is CCW in radians.
    Arc { center: Vec2, radius: f64, start_angle: f64, sweep_angle: f64 },
    /// A full circle.
    Circle { center: Vec2, radius: f64 },
    /// An elliptical arc. `major` is the semi-major axis vector.
    EllipseArc { center: Vec2, major: Vec2, ratio: f64, start_param: f64, sweep_param: f64 },
    /// A NURBS spline curve.
    Spline { degree: usize, control_points: Vec<Vec2>, closed: bool },
    /// Single-line text entity.
    Text { position: Vec2, content: String, height: f64, rotation: f64 },
    /// A point marker.
    Point { position: Vec2, size: f64 },
    /// A filled polygon region. `outer` is the boundary; `holes` are cutouts.
    FilledPolygon { outer: Vec<Vec2>, holes: Vec<Vec<Vec2>> },
    /// A viewport frame rectangle on paper.
    ViewportRect { center: Vec2, width: f64, height: f64 },
}

impl Geom {
    /// Decompose this geometry into zero or more VectorPrimitive items.
    ///
    /// Curves (arcs, ellipses, splines) are kept INTACT — the caller decides
    /// when to tessellate (PDF uses line segments at a chord tolerance; SVG
    /// can emit native arc commands).
    ///
    /// Takes `doc` for resolving hatches (boundary handles → geometry) and
    /// block references (recursive expansion).
    pub fn to_vector_primitives(&self, doc: &Document) -> Vec<VectorPrimitive> {
        use VectorPrimitive::*;
        match self {
            Geom::Line(l) => vec![Segment { p0: l.a, p1: l.b }],
            // Xline — clip the infinite line to the drawing's overall
            // extents (other entities' bboxes + the xline's own base, grown
            // by the diagonal) so exports stay finite; empty drawings get a
            // ±1e4 segment around the base.
            Geom::Xline(x) => {
                let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
                let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                let mut any = false;
                for d in &doc.dobjects {
                    // Skip Xline's own ±1e6 kernel bbox (it would swallow
                    // the clip rect); its base point still participates.
                    let (a, b) = if matches!(d.geom, Geom::Xline(_)) {
                        match d.geom {
                            Geom::Xline(x2) => (x2.base, x2.base),
                            _ => unreachable!(),
                        }
                    } else {
                        d.bbox()
                    };
                    if b.x < a.x || b.y < a.y { continue; }
                    any = true;
                    mn = Vec2::new(mn.x.min(a.x), mn.y.min(a.y));
                    mx = Vec2::new(mx.x.max(b.x), mx.y.max(b.y));
                }
                if !any {
                    let seg = x.line_segment(1e4);
                    vec![Segment { p0: seg.a, p1: seg.b }]
                } else {
                    let diag = (mx - mn).len().max(1.0);
                    let lo = mn - Vec2::new(diag, diag);
                    let hi = mx + Vec2::new(diag, diag);
                    match x.clip_to_rect(lo, hi) {
                        Some(l) => vec![Segment { p0: l.a, p1: l.b }],
                        None => Vec::new(),
                    }
                }
            }

            // Donut — two closed circles (outer + hole). Exporters that
            // can't fill use the outlines; the fill is a style choice.
            Geom::Donut(d) => {
                let seg = |radius: f64| {
                    let n = 48;
                    let mut pts = Vec::with_capacity(n + 1);
                    for i in 0..=n {
                        let t = std::f64::consts::TAU * (i as f64 / n as f64);
                        pts.push(Vec2::new(
                            d.center.x + radius * t.cos(),
                            d.center.y + radius * t.sin()));
                    }
                    pts
                };
                let mut out = Vec::new();
                for w in seg(d.outer_radius).windows(2) {
                    out.push(Segment { p0: w[0], p1: w[1] });
                }
                if d.inner_radius > 1e-9 {
                    for w in seg(d.inner_radius).windows(2) {
                        out.push(Segment { p0: w[0], p1: w[1] });
                    }
                }
                out
            }
            // Wipeout / Region — the closed loop outline.
            Geom::Wipeout(w) => loop_segments(&w.pts),
            Geom::Region(rg) => loop_segments(&rg.loop_pts),

            // Ray — clip the forward ray to the same extents (base
            // participates; the ±1e6 kernel bbox is skipped like Xline).
            Geom::Ray(r) => {
                let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
                let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                let mut any = false;
                for d in &doc.dobjects {
                    let (a, b) = if matches!(d.geom, Geom::Ray(_)) {
                        (r.base, r.base)
                    } else {
                        d.bbox()
                    };
                    if b.x < a.x || b.y < a.y { continue; }
                    any = true;
                    mn = Vec2::new(mn.x.min(a.x), mn.y.min(a.y));
                    mx = Vec2::new(mx.x.max(b.x), mx.y.max(b.y));
                }
                if !any {
                    let seg = r.ray_segment(1e4);
                    vec![Segment { p0: seg.a, p1: seg.b }]
                } else {
                    let diag = (mx - mn).len().max(1.0);
                    let lo = mn - Vec2::new(diag, diag);
                    let hi = mx + Vec2::new(diag, diag);
                    match r.clip_to_rect(lo, hi) {
                        Some(l) => vec![Segment { p0: l.a, p1: l.b }],
                        None => Vec::new(),
                    }
                }
            }

            Geom::Circle(c) => vec![Circle { center: c.center, radius: c.radius }],

            Geom::Arc(a) => vec![Arc {
                center: a.center, radius: a.radius,
                start_angle: a.start_angle, sweep_angle: a.sweep_angle,
            }],

            Geom::Ellipse(e) => {
                // A full ellipse = elliptical arc spanning 0..2π.
                vec![EllipseArc {
                    center: e.center, major: e.major, ratio: e.ratio,
                    start_param: 0.0,
                    sweep_param: std::f64::consts::TAU,
                }]
            }

            Geom::EllipseArc(ea) => vec![EllipseArc {
                center: ea.ellipse.center,
                major: ea.ellipse.major,
                ratio: ea.ellipse.ratio,
                start_param: ea.start_param,
                sweep_param: ea.sweep_param,
            }],

            Geom::Point(p) => vec![Point { position: p.location, size: p.size as f64 }],

            Geom::Polyline(p) => {
                let n = p.vertices.len();
                if n < 2 {
                    return Vec::new();
                }
                let seg_count = if p.closed { n } else { n - 1 };
                let mut out: Vec<VectorPrimitive> = Vec::with_capacity(seg_count);
                for i in 0..seg_count {
                    let v0 = &p.vertices[i];
                    let v1 = &p.vertices[(i + 1) % n];
                    if v0.bulge.abs() < 1e-9 {
                        out.push(Segment { p0: v0.pos, p1: v1.pos });
                    } else if let Some((center, radius, start_angle, sweep)) =
                        crate::join::bulge_arc(v0.pos, v1.pos, v0.bulge)
                    {
                        out.push(Arc { center, radius, start_angle, sweep_angle: sweep });
                    } else {
                        out.push(Segment { p0: v0.pos, p1: v1.pos });
                    }
                }
                out
            }

            Geom::Hatch(h) => {
                let loops = crate::hatch_resolve::resolve_hatch_loops(h, doc);
                if loops.is_empty() {
                    return Vec::new();
                }
                match &h.pattern {
                    HatchPattern::Solid => {
                        if loops.is_empty() {
                            return Vec::new();
                        }
                        let outer = loops[0].clone();
                        let holes = if loops.len() > 1 { loops[1..].to_vec() } else { Vec::new() };
                        vec![FilledPolygon { outer, holes }]
                    }
                    HatchPattern::Pattern { name, scale, angle_deg } => {
                        // Pattern hatches emit their pattern LINES (and
                        // circles for ring families), NOT a solid fill —
                        // `patterns::hatch_geometry` is the single source
                        // shared with the app canvas/GPU renderers.
                        let pat = crate::patterns::lookup(name.as_str());
                        if pat.is_empty() {
                            return Vec::new();
                        }
                        let (segs, circs) =
                            crate::patterns::hatch_geometry(&loops, &pat, *scale, *angle_deg);
                        let mut out = Vec::with_capacity(segs.len() + circs.len());
                        for (a, b) in segs {
                            out.push(Segment { p0: a, p1: b });
                        }
                        for (c, r) in circs {
                            out.push(Circle { center: c, radius: r });
                        }
                        out
                    }
                }
            }

            Geom::Spline(s) => vec![Spline {
                degree: s.degree,
                control_points: s.control_points.clone(),
                closed: false,
            }],

            Geom::Wall(w) => {
                let Some((left, right)) = w.face_polylines(24) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                let n = left.len();
                if n >= 2 {
                    for i in 0..n - 1 {
                        out.push(Segment { p0: left[i], p1: left[i + 1] });
                    }
                }
                let n = right.len();
                if n >= 2 {
                    for i in 0..n - 1 {
                        out.push(Segment { p0: right[i], p1: right[i + 1] });
                    }
                }
                out
            }

            Geom::Text(t) => vec![Text {
                position: t.position,
                content: t.text.clone(),
                height: t.height,
                rotation: t.angle,
            }],

            // Leader — the chain as segments + the label as Text.
            Geom::Leader(l) => {
                let mut out: Vec<VectorPrimitive> = Vec::new();
                for w in l.pts.windows(2) {
                    out.push(Segment { p0: w[0], p1: w[1] });
                }
                if !l.label.text.is_empty() {
                    out.push(Text {
                        position: l.label.position,
                        content: l.label.text.clone(),
                        height: l.label.height,
                        rotation: l.label.angle,
                    });
                }
                out
            }

            // AttrDef — export the VALUE text if the instance carries one,
            // else the default (mirrors what the renderer shows).
            Geom::AttrDef(a) => vec![Text {
                position: a.position,
                content: if a.default.is_empty() { a.tag.clone() } else { a.default.clone() },
                height: a.height,
                rotation: a.angle,
            }],

            // Xref — resolved children, transformed into world.
            Geom::Xref(x) => {
                let mut out = Vec::new();
                for d in &x.cached {
                    out.extend(x.transform_geom(&d.geom).to_vector_primitives(doc));
                }
                out
            }
            // Table — grid rules + each cell's text (annotations export as
            // the real grid + text, not a placeholder).
            Geom::Table(t) => {
                let mut out = Vec::new();
                for (a, b) in t.grid_lines() {
                    out.push(Segment { p0: a, p1: b });
                }
                for r in 0..t.n_rows {
                    for c in 0..t.n_cols {
                        if let Some(tx) = t.cell_text(r, c) {
                            out.push(Text {
                                position: tx.position,
                                content: tx.text.clone(),
                                height: tx.height,
                                rotation: tx.angle,
                            });
                        }
                    }
                }
                out
            }

            // CenterMark — the two crossing arms.
            Geom::CenterMark(cm) => {
                let [t0, t1, t2, t3] = cm.tips();
                vec![
                    Segment { p0: t0, p1: t2 },
                    Segment { p0: t1, p1: t3 },
                ]
            }

            // Issue #10 — emit the FULL dimension instead of nothing: dim
            // line + extension lines + leaders, arrowheads (filled triangle
            // or oblique tick per the style), and the formatted text label.
            // Uses the same resolved geometry as the on-screen renderer
            // (`Dim::render_geometry`), so exports match the canvas.
            Geom::Dimension(d) => {
                let style = doc.dim_styles.get(d.style)
                    .or_else(|| doc.dim_styles.get(0))
                    .cloned()
                    .unwrap_or_else(crate::dim::DimStyle::standard);
                let geo = d.render_geometry(&style);
                let mut out = Vec::new();
                for (a, b) in &geo.ext_lines {
                    out.push(Segment { p0: *a, p1: *b });
                }
                for (a, b) in &geo.leaders {
                    out.push(Segment { p0: *a, p1: *b });
                }
                if let Some((a, b)) = geo.dim_line {
                    // Gap-trim around centered text, exactly like the renderer.
                    if geo.text_on_dim_line {
                        let text = d.formatted_text(&style);
                        if !text.is_empty() {
                            let u   = (b - a).normalized();
                            let len = (b - a).len();
                            let half_gap = text.len() as f64 * 0.6
                                * (style.text_height * style.overall_scale) * 0.5
                                + style.text_gap * style.overall_scale;
                            let g1 = geo.text_pos - u * half_gap;
                            let g2 = geo.text_pos + u * half_gap;
                            let da = (g1 - a).dot(u);
                            let db = (g2 - a).dot(u);
                            if da > 0.0 && db < len && da < db {
                                out.push(Segment { p0: a, p1: g1 });
                                out.push(Segment { p0: g2, p1: b });
                            } else {
                                out.push(Segment { p0: a, p1: b });
                            }
                        } else {
                            out.push(Segment { p0: a, p1: b });
                        }
                    } else {
                        out.push(Segment { p0: a, p1: b });
                    }
                }
                // Angular dim arc — emit as a native Arc primitive.
                if let Some((c, r, a1, sweep)) = geo.dim_arc {
                    out.push(Arc {
                        center: c, radius: r,
                        start_angle: a1, sweep_angle: sweep,
                    });
                }
                let arrow_size = (style.arrow_size * style.overall_scale).max(1e-6);
                let tick_w = (style.tick_size * style.overall_scale).max(1e-6);
                for (tip, dir) in &geo.arrows {
                    let dn = dir.normalized();
                    if style.tick_size > 0.0 {
                        // Architectural tick — a 45° slash centered on the tip.
                        let c = std::f64::consts::FRAC_1_SQRT_2;
                        let t = Vec2::new(dn.x * c - dn.y * c, dn.x * c + dn.y * c);
                        out.push(Segment { p0: *tip + t * tick_w, p1: *tip - t * tick_w });
                    } else {
                        // Filled arrowhead: tip + two base corners (20° half
                        // angle, AutoCAD default); hollow = outline strokes.
                        let perp = Vec2::new(-dn.y, dn.x);
                        let base = *tip + dn * arrow_size;
                        let b1 = base + perp * (arrow_size * 0.35);
                        let b2 = base - perp * (arrow_size * 0.35);
                        if style.arrow_filled {
                            out.push(FilledPolygon {
                                outer: vec![*tip, b1, b2],
                                holes: Vec::new(),
                            });
                        } else {
                            out.push(Segment { p0: *tip, p1: b1 });
                            out.push(Segment { p0: b1, p1: b2 });
                            out.push(Segment { p0: b2, p1: *tip });
                        }
                    }
                }
                let text = d.formatted_text(&style);
                if !text.is_empty() {
                    out.push(Text {
                        position: geo.text_pos,
                        content: text,
                        height: style.text_height * style.overall_scale,
                        rotation: geo.text_angle,
                    });
                }
                out
            }

            Geom::BlockRef(br) => {
                let Some(block) = doc.blocks.get(br.block) else {
                    return Vec::new();
                };
                let mut out = Vec::new();
                for child in &block.dobjects {
                    let transformed_geom = br.transform_geom(&child.geom, block.base);
                    let prims = transformed_geom.to_vector_primitives(doc);
                    out.extend(prims);
                }
                out
            }

            Geom::Viewport(vp) => vec![ViewportRect {
                center: vp.center,
                width: vp.width,
                height: vp.height,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Line, Polyline, PolyVertex};
    use crate::dobject::DObject;

    #[test]
    fn line_to_segment() {
        let g = Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 5.0) });
        let doc = Document::default();
        let prims = g.to_vector_primitives(&doc);
        assert_eq!(prims.len(), 1);
        match &prims[0] {
            VectorPrimitive::Segment { p0, p1 } => {
                assert!((p0.x - 0.0).abs() < 1e-9);
                assert!((p1.x - 10.0).abs() < 1e-9);
            }
            _ => panic!("expected Segment"),
        }
    }

    #[test]
    fn circle_is_intact() {
        let g = Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 3.0 });
        let doc = Document::default();
        let prims = g.to_vector_primitives(&doc);
        assert_eq!(prims.len(), 1);
        match &prims[0] {
            VectorPrimitive::Circle { center, radius } => {
                assert!((center.x - 5.0).abs() < 1e-9);
                assert!((radius - 3.0).abs() < 1e-9);
            }
            _ => panic!("expected Circle"),
        }
    }

    #[test]
    fn polyline_bulge_becomes_arc() {
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 1.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let g = Geom::Polyline(pl);
        let doc = Document::default();
        let prims = g.to_vector_primitives(&doc);
        assert_eq!(prims.len(), 2);
        // First segment is a Line (bulge=0)
        assert!(matches!(&prims[0], VectorPrimitive::Segment { .. }));
        // Second segment is an Arc (bulge=1)
        assert!(matches!(&prims[1], VectorPrimitive::Arc { .. }));
    }

    /// Issue #10 — dimensions now export as full geometry: 2 extension
    /// lines + dim line (gap-trimmed for the centered text) + 2 filled
    /// arrowheads + the formatted text label.
    #[test]
    fn dimension_exports_full_geometry() {
        let g = Geom::Dimension(crate::dim::Dim {
            kind: crate::dim::DimKind::Linear {
                p1: Vec2::ZERO, p2: Vec2::new(10.0, 0.0),
                dimline_pos: Vec2::new(5.0, 1.0),
                ortho: crate::dim::LinearOrtho::Aligned,
            },
            style: 0,
            text_override: None,
        });
        let doc = Document::default();
        let prims = g.to_vector_primitives(&doc);
        // 2 extension lines + (gap-trimmed) dim line = 4 segments, 2 filled
        // arrowheads, 1 text label.
        let segs  = prims.iter().filter(|p| matches!(p, VectorPrimitive::Segment { .. })).count();
        let fills = prims.iter().filter(|p| matches!(p, VectorPrimitive::FilledPolygon { .. })).count();
        let texts = prims.iter().filter(|p| matches!(p, VectorPrimitive::Text { .. })).count();
        assert_eq!(segs, 4, "extension×2 + dim-line×2 (gap-trimmed): {prims:?}");
        assert_eq!(fills, 2, "one arrowhead per dim-line end");
        assert_eq!(texts, 1, "the measured-value label");
        // The label is the measured distance (10.0) formatted by the style.
        if let VectorPrimitive::Text { content, .. } = &prims[prims.len() - 1] {
            assert!(content.contains("10"), "label carries the measured value: {content}");
        } else {
            panic!("last primitive must be the text label");
        }
    }

    #[test]
    fn angular_dim_exports_arc_and_extensions() {
        let g = Geom::Dimension(crate::dim::Dim {
            kind: crate::dim::DimKind::Angular {
                vertex:  Vec2::ZERO,
                p1:      Vec2::new(10.0, 0.0),
                p2:      Vec2::new(0.0, 10.0),
                arc_pos: Vec2::new(5.0, 5.0),
            },
            style: 0,
            text_override: None,
        });
        let doc = Document::default();
        let prims = g.to_vector_primitives(&doc);
        let arcs = prims.iter()
            .filter(|p| matches!(p, VectorPrimitive::Arc { .. }))
            .count();
        assert_eq!(arcs, 1, "the dim arc must export as a native Arc: {prims:?}");
        let segs = prims.iter()
            .filter(|p| matches!(p, VectorPrimitive::Segment { .. }))
            .count();
        assert_eq!(segs, 2, "two extension lines");
        if let Some(VectorPrimitive::Arc { radius, sweep_angle, .. }) = prims.iter()
            .find(|p| matches!(p, VectorPrimitive::Arc { .. }))
        {
            assert!((*radius - (5.0f64 * std::f64::consts::SQRT_2)).abs() < 1e-9);
            assert!((sweep_angle.abs() - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        } else {
            panic!("arc missing");
        }
    }
}

/// Closed vertex loop → outline segments (last → first closes it).
fn loop_segments(pts: &[Vec2]) -> Vec<VectorPrimitive> {
    use VectorPrimitive::Segment;
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(Segment { p0: pts[i], p1: pts[(i + 1) % n] });
    }
    out
}
