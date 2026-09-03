//! Scene builder — flatten a Document into paper-space primitives.
//!
//! `build_scene` is PURE and READ-ONLY on the doc: it resolves each entity's
//! plot color + physical print width, flattens geometry to straight segments /
//! fill loops / text triangles, and maps every coordinate through the
//! `PageXform`. The resulting `Scene` is what `emit` writes to PDF.
//!
//! The physical-mm invariant lives here: `width_mm` on every `Prim::Stroke` is
//! `plot_width_mm(...) × cfg.lw_scale` — a value that never touches the plot
//! scale. Two builds of the same drawing at different scales differ only in
//! point coordinates, not in stroke widths (asserted in `lib::tests`).

use cad_kernel::block::BlockRef;
use cad_kernel::color::{resolve_color, Color};
use cad_kernel::geom::Geom;
use cad_kernel::lineweight::Lineweight;
use cad_kernel::math::Vec2;
use cad_kernel::plotstyle::{
    EndStyle, FillStyle, JoinStyle, PlotConfig, PlotLinetype, PlotStyleTable, PlotWidth,
};
use cad_kernel::VectorPrimitive;
use cad_kernel::{Document, Style};

use crate::xform::PageXform;

/// A single paper-space drawing primitive (coordinates already in page mm).
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    /// A stroked polyline. `width_mm` is physical paper width (scale-independent).
    /// `dash_mm` is the paper-space [dash, gap, …] linetype pattern (empty = solid).
    /// `dash_offset_mm` is the dash-pattern phase — the pattern position at the
    /// path start (SVG `stroke-dashoffset` / PDF dash-phase convention), non-zero
    /// only for adaptive linetypes.
    /// `smooth` marks curve tessellations (circles/arcs/splines): their interior
    /// vertices are sampling points, not real corners — previews skip join
    /// emulation there (the vector emitters apply joins natively).
    Stroke {
        pts: Vec<(f64, f64)>,
        closed: bool,
        width_mm: f32,
        rgb: (u8, u8, u8),
        dash_mm: Vec<f32>,
        dash_offset_mm: f32,
        cap: EndStyle,
        join: JoinStyle,
        dither: bool,
        smooth: bool,
    },
    /// A filled region (even-odd): outer loop first, successive loops are holes.
    /// Non-Solid pen fill styles are expanded into extra pattern geometry at
    /// scene build; the Fill itself stays solid.
    Fill { loops: Vec<Vec<(f64, f64)>>, rgb: (u8, u8, u8), dither: bool },
    /// A triangle soup fill (text glyphs).
    Tris { tris: Vec<[(f64, f64); 3]>, rgb: (u8, u8, u8) },
}

/// The full flattened plot: page size + primitives, ready to emit.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub page_w_mm: f64,
    pub page_h_mm: f64,
    pub prims: Vec<Prim>,
    /// Entities the MVP does not flatten (dimensions) — reported to the caller.
    pub skipped_dims: usize,
}

/// Chord-error tolerance on paper (mm) for curve flattening. Tight enough that
/// facets are imperceptible in print (was 0.1 mm → visibly polygonal circles).
const CHORD_TOL_MM: f64 = 0.02;

/// Minimum segments per FULL turn, so even a small / scaled-down circle still
/// reads as round (the chord-error count can drop very low at tiny paper radii).
const MIN_SEGS_PER_TURN: f64 = 96.0;

/// Plot width for a "0.00" / Default lineweight (owner rule): a thin 0.1 mm
/// line, distinct from an explicit 0.25 mm.
const DEFAULT_PLOT_MM: f32 = 0.1;

/// Number of straight segments to approximate an arc of `sweep_abs` radians at
/// world radius `r_world` — the max of a chord-error count (≤ CHORD_TOL_MM on
/// paper) and a per-turn smoothness floor, so curves read as smooth in print.
fn curve_segments(r_world: f64, sweep_abs: f64, s: f64) -> usize {
    if sweep_abs <= 1e-9 {
        return 2;
    }
    let r_paper = (r_world * s).abs();
    // Chord-error count.
    let by_err = if r_paper <= 1e-6 {
        8
    } else {
        let ratio = (1.0 - CHORD_TOL_MM / r_paper).clamp(-1.0, 1.0);
        let theta_max = 2.0 * ratio.acos(); // max radians per segment
        if theta_max <= 1e-6 { 4096 } else { (sweep_abs / theta_max).ceil() as usize }
    };
    // Smoothness floor: at least MIN_SEGS_PER_TURN over a full circle, scaled
    // to this arc's sweep.
    let by_min = ((sweep_abs / std::f64::consts::TAU) * MIN_SEGS_PER_TURN).ceil() as usize;
    by_err.max(by_min).max(2).min(4096)
}

/// Build the paper-space scene from a document, plot table and config.
///
/// READ-ONLY on `doc`. Never mutates, never snapshots.
pub fn build_scene(doc: &Document, table: &PlotStyleTable, cfg: &PlotConfig) -> Scene {
    // Layout plot (restored from e0fddd1): 1:1 paper — paper border,
    // paper-space entities + per-viewport model content with CTBs.
    if let Some(li) = cfg.plot_layout_index {
        if let Some(layout) = doc.layouts.get(li) {
            return build_layout_scene(doc, table, cfg, layout);
        }
    }
    let (mn, mx) = resolve_extents(doc, cfg);
    let xform = PageXform::build(cfg, mn, mx);
    let bbox_diag = ((mx.x - mn.x).powi(2) + (mx.y - mn.y).powi(2)).sqrt().max(1.0);
    // Window area → clip every primitive to the picked rectangle (only the parts
    // INSIDE the window are plotted). Extents/Display → no clip.
    let clip = if let cad_kernel::plotstyle::PlotArea::Window { min, max } = cfg.area {
        Some((min, max))
    } else {
        None
    };

    let mut b = Builder {
        doc,
        table,
        cfg,
        xform,
        bbox_diag,
        clip,
        cur_dash: Vec::new(),
        cur_cap: EndStyle::Round,
        cur_join: JoinStyle::Round,
        cur_fill: FillStyle::Solid,
        cur_dither: false,
        cur_adaptive: true,
        scene: Scene {
            page_w_mm: xform.page_w_mm,
            page_h_mm: xform.page_h_mm,
            prims: Vec::new(),
            skipped_dims: 0,
        },
        fonts: None,
        layers: &doc.layers,
        ctb_mode: false,
        ctb: "",
    };

    for (i, d) in doc.dobjects.iter().enumerate() {
        if !doc.is_visible(i) {
            continue;
        }
        // AutoCAD plot gating: skip non-plottable layers.
        if doc.layers.get(d.style.layer).map(|l| !l.plottable).unwrap_or(false) {
            continue;
        }
        b.emit_dobject(&d.geom, &d.style, None, 0);
    }
    b.scene
}

/// LAYOUT plot path (restored from e0fddd1). The layout's page defines the
/// paper (its `page_w_mm`/`page_h_mm` are already orientation-applied) and the
/// transform is exactly 1:1 mm — paper-space coordinates map onto the sheet
/// with the SAME origin the layout tab renders (sheet corner = (0,0), no
/// printable-margin shift, so exported viewport positions match the canvas).
/// Paper-space entities resolve against the PAPER layer table (in `doc.layers`
/// while the layout tab is active — layers are swapped on tab change); each
/// viewport's model content resolves against the MODEL table (`layout.layers`)
/// through its own camera. Colours go through the per-layout / per-viewport
/// CTB (empty = monochrome, the historical layout default), NOT the plot-style
/// table.
fn build_layout_scene(doc: &Document, table: &PlotStyleTable, cfg: &PlotConfig,
                      layout: &cad_kernel::Layout) -> Scene {
    let pw = layout.page_w_mm;
    let ph = layout.page_h_mm;
    let xform = PageXform { s: 1.0, tx: 0.0, ty: 0.0, page_w_mm: pw, page_h_mm: ph };
    let mut b = Builder {
        doc,
        table,
        cfg,
        xform,
        bbox_diag: pw.max(ph),
        clip: None,
        cur_dash: Vec::new(),
        cur_cap: EndStyle::Round,
        cur_join: JoinStyle::Round,
        cur_fill: FillStyle::Solid,
        cur_dither: false,
        cur_adaptive: true,
        scene: Scene { page_w_mm: pw, page_h_mm: ph, prims: Vec::new(), skipped_dims: 0 },
        fonts: None,
        layers: &doc.layers,
        ctb_mode: true,
        ctb: layout.ctb_name.as_deref().unwrap_or(""),
    };

    // Paper frame outline (thin border at the sheet edge — same as the canvas).
    let fw = pw.max(1.0);
    let fh = ph.max(1.0);
    b.push_stroke(
        &[Vec2::new(0.0, 0.0), Vec2::new(fw, 0.0), Vec2::new(fw, fh), Vec2::new(0.0, fh)],
        true, false, 0.5, (0, 0, 0));

    // Paper-space entities — layout CTB overrides colours.
    for d in &layout.entities {
        if !d.style.visible { continue; }
        if doc.layers.get(d.style.layer).map(|l| !l.plottable).unwrap_or(false) { continue; }
        b.emit_dobject(&d.geom, &d.style, None, 0);
    }

    // Viewport model content — per-viewport camera + CTB, MODEL layer table.
    for vp in &layout.viewports {
        let ms = vp.model_zoom * vp.model_scale;
        let vp_cx = (vp.rect_min.0 + vp.rect_max.0) * 0.5;
        let vp_cy = (vp.rect_min.1 + vp.rect_max.1) * 0.5;
        // The viewport rect in MODEL units — the clip window. The layout tab
        // clips model content to the frame; the output files must match, or
        // a viewport's content spills across the sheet past its bounds.
        let vp_clip = if ms > 1e-9 {
            let hw = (vp.rect_max.0 - vp.rect_min.0) * 0.5 / ms;
            let hh = (vp.rect_max.1 - vp.rect_min.1) * 0.5 / ms;
            if hw > 0.0 && hh > 0.0 {
                Some((
                    Vec2::new(vp.model_center.0 - hw, vp.model_center.1 - hh),
                    Vec2::new(vp.model_center.0 + hw, vp.model_center.1 + hh),
                ))
            } else { None }
        } else { None };
        let vp_xform = PageXform {
            s: ms,
            tx: vp_cx - vp.model_center.0 * ms,
            ty: vp_cy - vp.model_center.1 * ms,
            page_w_mm: pw,
            page_h_mm: ph,
        };
        let mut vb = Builder {
            doc,
            table,
            cfg,
            xform: vp_xform,
            bbox_diag: pw.max(ph),
            clip: vp_clip,
            cur_dash: Vec::new(),
            cur_cap: EndStyle::Round,
            cur_join: JoinStyle::Round,
            cur_fill: FillStyle::Solid,
            cur_dither: false,
            cur_adaptive: true,
            scene: Scene { page_w_mm: pw, page_h_mm: ph, prims: Vec::new(), skipped_dims: 0 },
            fonts: None,
            layers: &layout.layers,
            ctb_mode: true,
            // Per-viewport CTB, inheriting the LAYOUT's when the viewport has
            // none (ViewportData::ctb_name: "None = inherit from layout").
            // Empty at every level = full colour (matches the canvas).
            ctb: vp.ctb_name.as_deref().or(layout.ctb_name.as_deref()).unwrap_or(""),
        };
        for d in doc.dobjects.iter() {
            if !d.style.visible { continue; }
            if layout.layers.get(d.style.layer).map(|l| !l.plottable).unwrap_or(false) { continue; }
            vb.emit_dobject(&d.geom, &d.style, None, 0);
        }
        b.scene.prims.extend(vb.scene.prims);
    }
    b.scene
}

/// The layout CTB colour override (restored from e0fddd1 / aa95957): monochrome
/// → black, grayscale → Rec.601 luminance (matches `PlotStyleTable::apply_color`),
/// fullcolor / unknown → unchanged. An EMPTY ctb defaults to MONOCHROME (the
/// historical layout-plot default — new layouts default to monochrome, so the
/// sheet plots black-on-white unless a CTB says otherwise). Same semantics as
/// the canvas `apply_vp_ctb`.
fn apply_ctb(rgb: (u8, u8, u8), ctb: &str) -> (u8, u8, u8) {
    let ctb = if ctb.is_empty() { "monochrome" } else { ctb };
    match ctb {
        "monochrome" => (0, 0, 0),
        "grayscale" => {
            let g = (0.299 * rgb.0 as f32 + 0.587 * rgb.1 as f32 + 0.114 * rgb.2 as f32)
                .round()
                .clamp(0.0, 255.0) as u8;
            (g, g, g)
        }
        _ => rgb,
    }
}

/// Resolve the world bbox for the configured area. Extents/Display union each
/// entity's own bbox; Window uses the two picked points.
fn resolve_extents(doc: &Document, cfg: &PlotConfig) -> (Vec2, Vec2) {
    use cad_kernel::plotstyle::PlotArea;
    if let PlotArea::Window { min, max } = cfg.area {
        return (min, max);
    }
    // Extents / Display (Display has no view rect at this layer → treat as Extents).
    let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut any = false;
    for (i, d) in doc.dobjects.iter().enumerate() {
        if !doc.is_visible(i) {
            continue;
        }
        if doc.layers.get(d.style.layer).map(|l| !l.plottable).unwrap_or(false) {
            continue;
        }
        let (lo, hi) = d.bbox();
        if !lo.x.is_finite() || !hi.x.is_finite() {
            continue;
        }
        mn.x = mn.x.min(lo.x); mn.y = mn.y.min(lo.y);
        mx.x = mx.x.max(hi.x); mx.y = mx.y.max(hi.y);
        any = true;
    }
    if !any {
        return (Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0));
    }
    (mn, mx)
}

struct Builder<'a> {
    doc:   &'a Document,
    table: &'a PlotStyleTable,
    cfg:   &'a PlotConfig,
    xform: PageXform,
    bbox_diag: f64,
    /// Window clip rectangle (world space), or None for Extents/Display. Layout
    /// plots use it per-viewport too: model content clips to the viewport rect.
    clip: Option<(Vec2, Vec2)>,
    /// The current entity's linetype dash pattern in PAPER mm (empty = solid).
    cur_dash: Vec<f32>,
    /// The current entity's resolved pen properties (UseObject → defaults):
    /// consumed by `push_stroke` and the `FilledPolygon` arm.
    cur_cap: EndStyle,
    cur_join: JoinStyle,
    cur_fill: FillStyle,
    cur_dither: bool,
    cur_adaptive: bool,
    scene: Scene,
    fonts: Option<cad_text::FontManager>,
    /// The layer table colours/lineweights resolve against. Model path =
    /// `&doc.layers`; a layout's viewport content = `&layout.layers` (the
    /// swapped-in MODEL table while the layout tab is active).
    layers: &'a cad_kernel::layer::LayerTable,
    /// Layout-plot CTB mode: when true, colours go through `ctb` (empty =
    /// monochrome) instead of the plot-style table.
    ctb_mode: bool,
    ctb: &'a str,
}

impl<'a> Builder<'a> {
    /// The effective ACI of a color, if it maps to one (None for truecolor).
    fn effective_aci(&self, color: Color, layer: u32) -> Option<u8> {
        match color {
            Color::Aci(i) => Some(i),
            Color::ByLayer | Color::ByBlock => match self.layers.get(layer).map(|l| l.color) {
                Some(Color::Aci(i)) => Some(i),
                _ => None,
            },
            Color::TrueColorRef(_) => None,
        }
    }

    /// Resolve (rgb, aci) for a style, honouring a ByBlock substitution when
    /// inside a block instance.
    fn resolve_color_aci(
        &self,
        style: &Style,
        byblock: Option<((u8, u8, u8), Option<u8>)>,
    ) -> ((u8, u8, u8), Option<u8>) {
        if let (Color::ByBlock, Some(sub)) = (style.color, byblock) {
            return sub;
        }
        let rgb = resolve_color(style.color, style.layer, self.layers, &self.doc.truecolors);
        let aci = self.effective_aci(style.color, style.layer);
        (rgb, aci)
    }

    /// The effective pen table for width/linetype resolution: a saved CTB by
    /// name (layout plots), or the plot-style table the pipeline was given
    /// (model plots). `None` for built-in / unknown CTB names — width and
    /// linetype then fall back to the OBJECT's own properties, so the document's
    /// plot-style table can never leak into a layout plot: a viewport whose CTB
    /// is a built-in must not adopt the widths/linetypes of whatever CTB the
    /// editor currently holds in `doc.plot_styles`.
    fn pen_table(&self) -> Option<&PlotStyleTable> {
        if self.ctb_mode {
            return self.cfg.ctb_tables.get(self.ctb);
        }
        Some(self.table)
    }

    /// The physical print width (mm) for this style/aci — `Fixed` pen wins, else
    /// the object's own lineweight. A "0.00"/Default width (the UI shows Default /
    /// ByLayer / ByBlock as 0.00) plots as a thin **0.1 mm** line (owner rule) so
    /// it is distinct from an explicit 0.25 mm; explicit Custom widths print
    /// exactly. Multiplied by the global `lw_scale` only.
    fn width_mm(&self, style: &Style, aci: Option<u8>) -> f32 {
        let pen_lw = self.pen_table().and_then(|t| aci.map(|a| t.style(a).lineweight));
        let base = match pen_lw {
            Some(PlotWidth::Fixed(w)) => w,
            // UseObject — or no pen table at all (built-in/unknown CTB in a
            // layout plot) → the object's own width chain.
            _ => self.object_width_mm(style),
        };
        let base = if base > 0.0 { base } else { DEFAULT_PLOT_MM };
        base * self.cfg.lw_scale
    }

    /// The object's own lineweight in mm through the ByLayer/ByBlock chain.
    /// Default / a non-Custom layer width return 0.0 (→ the 0.1 mm default above).
    fn object_width_mm(&self, style: &Style) -> f32 {
        match style.lineweight {
            Lineweight::Custom(mm) => mm,
            Lineweight::Default => 0.0,
            Lineweight::ByLayer | Lineweight::ByBlock => {
                match self.layers.get(style.layer).map(|l| l.lineweight) {
                    Some(Lineweight::Custom(mm)) => mm,
                    _ => 0.0,
                }
            }
        }
    }

    /// The plot color. Layout-plot CTB mode short-circuits the whole plot-style
    /// table: the per-layout / per-viewport CTB decides. A SAVED CTB (resolved
    /// in `cfg.ctb_tables` by name) applies its per-ACI colour rules (the same
    /// §1a subset as the plot-style table); built-in names keep the simple
    /// transform (empty = monochrome — the historical layout-plot default).
    /// Otherwise honour the §1a subset: per-color plot_color override, then
    /// grayscale, then screening, then global monochrome.
    fn plot_rgb(&self, aci: Option<u8>, rgb0: (u8, u8, u8)) -> (u8, u8, u8) {
        if self.ctb_mode {
            if let Some(t) = self.cfg.ctb_tables.get(self.ctb) {
                return t.apply_color(aci, rgb0);
            }
            return apply_ctb(rgb0, self.ctb);
        }
        // 1) per-color plot_color override (UseObject keeps the entity color).
        let rgb = self.table.apply_color(aci, rgb0);
        // 4) global monochrome wins last (plot-all-black option).
        if self.cfg.monochrome {
            return (0, 0, 0);
        }
        rgb
    }

    fn s(&self) -> f64 { self.xform.s }

    fn tf(&self, p: Vec2) -> (f64, f64) { self.xform.apply(p.x, p.y) }

    fn push_stroke(
        &mut self,
        world: &[Vec2],
        closed: bool,
        smooth: bool,
        width_mm: f32,
        rgb: (u8, u8, u8),
    ) {
        if world.len() < 2 {
            return;
        }
        let dash = self.cur_dash.clone();
        // Adaptive linetype: rotate the pattern so the run ends inside a dash.
        // Computed from the FULL world polyline once per primitive; clipped
        // multi-runs all continue the same phase (acceptable approximation).
        let dash_offset = self.adaptive_dash_offset_mm(world, closed);
        let (cap, join, dither) = (self.cur_cap, self.cur_join, self.cur_dither);
        match self.clip {
            Some((mn, mx)) => {
                // Clip to the window; each inside run becomes its own open stroke.
                for run in clip_polyline_rect(world, closed, mn, mx) {
                    if run.len() >= 2 {
                        let pts: Vec<(f64, f64)> = run.iter().map(|&p| self.tf(p)).collect();
                        self.scene.prims.push(Prim::Stroke {
                            pts, closed: false, width_mm, rgb, dash_mm: dash.clone(),
                            dash_offset_mm: dash_offset, cap, join, dither, smooth,
                        });
                    }
                }
            }
            None => {
                let pts: Vec<(f64, f64)> = world.iter().map(|&p| self.tf(p)).collect();
                self.scene.prims.push(Prim::Stroke {
                    pts, closed, width_mm, rgb, dash_mm: dash,
                    dash_offset_mm: dash_offset, cap, join, dither, smooth,
                });
            }
        }
    }

    /// The adaptive-linetype dash phase for a stroked polyline: when the pen has
    /// `adaptive` on (default) and the entity is dashed, phase-rotate the pattern
    /// so the polyline END lands inside a dash. Returns the pattern offset in
    /// PAPER mm (world length × the plot scale) — the pattern position at the
    /// path start (the SVG `stroke-dashoffset` / PDF dash-phase convention).
    fn adaptive_dash_offset_mm(&self, world: &[Vec2], closed: bool) -> f32 {
        if !self.cur_adaptive || self.cur_dash.is_empty() {
            return 0.0;
        }
        let mut len = 0.0;
        for w in world.windows(2) {
            len += (w[1] - w[0]).len();
        }
        if closed && world.len() > 2 {
            len += (world[0] - world[world.len() - 1]).len();
        }
        adaptive_dash_offset(&self.cur_dash, len * self.s())
    }

    /// The entity's linetype dash pattern converted to PAPER millimetres:
    /// world dash/gap lengths × the plot scale × the object's linetype scale.
    /// A CTB / plot-style linetype override (`PlotLinetype::Id`) wins; with
    /// `UseObject` — or no pen table at all (built-in/unknown CTB in a layout
    /// plot) — the entity's own chain resolves (style, then layer). Empty =
    /// solid (continuous).
    fn linetype_dash_mm(&self, style: &Style, aci: Option<u8>) -> Vec<f32> {
        let override_id = self.pen_table()
            .and_then(|t| aci.map(|a| t.style(a).linetype))
            .and_then(|lt| match lt {
                PlotLinetype::UseObject => None,
                PlotLinetype::Id(id) => Some(id),
            });
        let lt = if let Some(id) = override_id {
            self.doc.linetypes.get(id)
        } else {
            self.doc.linetypes.get(style.linetype).or_else(|| {
                self.layers.get(style.layer).and_then(|l| self.doc.linetypes.get(l.linetype))
            })
        };
        let Some(lt) = lt else { return Vec::new() };
        if lt.pattern.is_empty() {
            return Vec::new();
        }
        let sc = (style.linetype_scale.max(1e-4)) as f64 * self.xform.s;
        lt.pattern.iter().map(|&seg| ((seg.abs() as f64) * sc) as f32).collect()
    }

    /// Flatten + emit one entity. `byblock` supplies the ByBlock color substitute
    /// while recursing into a block; `depth` caps block nesting.
    fn emit_dobject(
        &mut self,
        g: &Geom,
        style: &Style,
        byblock: Option<((u8, u8, u8), Option<u8>)>,
        depth: u32,
    ) {
        let (rgb0, aci) = self.resolve_color_aci(style, byblock);
        let rgb = self.plot_rgb(aci, rgb0);
        let w = self.width_mm(style, aci);
        let s = self.s();
        let bbox_diag = self.bbox_diag;
        self.cur_dash = self.linetype_dash_mm(style, aci);

        // Resolve the pen's cap/join/fill/dither/adaptive through the SAME
        // mechanism as width/linetype (`pen_table()`). UseObject — or no pen
        // table at all (built-in/unknown CTB in a layout plot) — falls back to
        // the defaults: cap Round, join Round, fill Solid, dither off,
        // adaptive on. Copy out before mutating `self`.
        let (pen_end, pen_join, pen_fill, pen_dither, pen_adaptive) = match self
            .pen_table()
            .and_then(|t| aci.map(|a| t.style(a)))
        {
            Some(p) => (p.end_style, p.join_style, p.fill_style, p.dither, p.adaptive),
            None => (EndStyle::UseObject, JoinStyle::UseObject, FillStyle::UseObject, false, true),
        };
        self.cur_cap = match pen_end {
            EndStyle::UseObject => EndStyle::Round,
            other => other,
        };
        self.cur_join = match pen_join {
            JoinStyle::UseObject => JoinStyle::Round,
            other => other,
        };
        self.cur_fill = match pen_fill {
            FillStyle::UseObject => FillStyle::Solid,
            other => other,
        };
        self.cur_dither = pen_dither;
        self.cur_adaptive = pen_adaptive;

        if let Geom::BlockRef(br) = g {
            if depth < 8 {
                self.emit_blockref(br, (rgb0, aci), depth + 1);
            }
            return;
        }

        let prims = g.to_vector_primitives(self.doc);
        self.emit_primitives(&prims, w, rgb, s, bbox_diag);
    }

    /// Flatten + emit one entity's primitives, MERGING consecutive
    /// Segment/Arc/EllipseArc/Spline prims whose endpoints touch into ONE
    /// stroked polyline. Without the merge every primitive becomes its own
    /// 2-point stroke: PDF/SVG never see a multi-point path, so JOIN styles
    /// can't apply, the closing segment of a closed polyline shows two caps
    /// instead of a joined seam, and the adaptive dash phase is computed per
    /// 2-point segment instead of over the whole polyline.
    fn emit_primitives(
        &mut self,
        prims: &[VectorPrimitive],
        w: f32,
        rgb: (u8, u8, u8),
        s: f64,
        bbox_diag: f64,
    ) {
        // Points merged so far (world space). `smooth` tracks whether the
        // chain so far is pure curve tessellation (no real corners).
        let mut chain: Vec<Vec2> = Vec::new();
        let mut smooth = true;
        for prim in prims {
            match prim {
                VectorPrimitive::Segment { p0, p1 } => {
                    self.chain_append(&mut chain, &mut smooth, &[*p0, *p1], false, w, rgb);
                }
                VectorPrimitive::Arc { center, radius, start_angle, sweep_angle } => {
                    let n = curve_segments(*radius, sweep_angle.abs(), s);
                    let pts = sample_arc(*center, *radius, *start_angle, *sweep_angle, n);
                    self.chain_append(&mut chain, &mut smooth, &pts, true, w, rgb);
                }
                VectorPrimitive::EllipseArc { center, major, ratio, start_param, sweep_param } => {
                    let a = major.len();
                    let el = cad_kernel::Ellipse { center: *center, major: *major, ratio: *ratio };
                    let n = curve_segments(a, sweep_param.abs(), s).max(4);
                    let pts: Vec<Vec2> = (0..=n)
                        .map(|i| {
                            let t = start_param + sweep_param * (i as f64 / n as f64);
                            el.point_at(t)
                        })
                        .collect();
                    self.chain_append(&mut chain, &mut smooth, &pts, true, w, rgb);
                }
                VectorPrimitive::Spline { degree, control_points, .. } => {
                    let sp = cad_kernel::Spline {
                        degree: *degree,
                        control_points: control_points.clone(),
                        weights: vec![1.0; control_points.len()],
                        knots: None,
                        width: 0.0,   // no width channel in VectorPrimitive
                    };
                    let n = curve_segments(bbox_diag * 0.5, std::f64::consts::TAU, s)
                        .clamp(32, 512);
                    let pts = sp.tessellate(n);
                    self.chain_append(&mut chain, &mut smooth, &pts, true, w, rgb);
                }
                // Standalone prims — flush the pending chain first.
                VectorPrimitive::Circle { center, radius } => {
                    self.flush_chain(&mut chain, smooth, w, rgb);
                    smooth = true;
                    let n = curve_segments(*radius, std::f64::consts::TAU, s);
                    let pts = sample_circle(*center, *radius, n);
                    self.push_stroke(&pts, true, true, w, rgb);
                }
                VectorPrimitive::Point { position, size } => {
                    self.flush_chain(&mut chain, smooth, w, rgb);
                    smooth = true;
                    let hs = if *size > 0.0 { *size } else { bbox_diag * 0.004 };
                    let c = *position;
                    self.push_stroke(&[Vec2::new(c.x - hs, c.y), Vec2::new(c.x + hs, c.y)],
                        false, false, w, rgb);
                    self.push_stroke(&[Vec2::new(c.x, c.y - hs), Vec2::new(c.x, c.y + hs)],
                        false, false, w, rgb);
                }
                VectorPrimitive::FilledPolygon { outer, holes } => {
                    self.flush_chain(&mut chain, smooth, w, rgb);
                    smooth = true;
                    let mapped: Vec<Vec<(f64, f64)>> = std::iter::once(outer)
                        .chain(holes.iter())
                        .filter_map(|lp| {
                            let lp2 = match self.clip {
                                Some((mn, mx)) => {
                                    let c = clip_polygon_rect(lp, mn, mx);
                                    if c.len() < 3 { return None; }
                                    c
                                }
                                None => lp.clone(),
                            };
                            Some(lp2.iter().map(|&p| self.tf(p)).collect())
                        })
                        .collect();
                    if !mapped.is_empty() {
                        self.scene.prims.push(Prim::Fill {
                            loops: mapped.clone(),
                            rgb,
                            dither: self.cur_dither,
                        });
                        // Non-Solid pen fill styles expand into pattern geometry at
                        // scene level (paper space), so every emitter gets them for
                        // free as strokes / small fills.
                        self.emit_fill_pattern(&mapped, rgb);
                    }
                }
                VectorPrimitive::Text { position, content, height, rotation } => {
                    self.flush_chain(&mut chain, smooth, w, rgb);
                    smooth = true;
                    let t = cad_kernel::Text {
                        position: *position, text: content.clone(),
                        height: *height, angle: *rotation,
                        h_align: cad_kernel::TextHAlign::Left,
                        v_align: cad_kernel::TextVAlign::Baseline,
                        style: 0, font_name: String::new(),
                        bold: false, oblique: 0.0, width_factor: 1.0,
                        outline_only: false, outline_width: 0.0,
                        underline: false,
                        list_mode: cad_kernel::TextListKind::None,
                        line_spacing: 1.5,
                    };
                    self.emit_text(&t, rgb);
                }
                VectorPrimitive::ViewportRect { center, width, height } => {
                    self.flush_chain(&mut chain, smooth, w, rgb);
                    smooth = true;
                    // The viewport FRAME prints on the paper (restored from
                    // e0fddd1 / 6c5eb12): a closed rectangle at the paper
                    // entity's bounds, same stroke/CTB as any other paper entity.
                    let hw = *width * 0.5;
                    let hh = *height * 0.5;
                    let pts = vec![
                        Vec2::new(center.x - hw, center.y - hh),
                        Vec2::new(center.x + hw, center.y - hh),
                        Vec2::new(center.x + hw, center.y + hh),
                        Vec2::new(center.x - hw, center.y + hh),
                    ];
                    self.push_stroke(&pts, true, false, w, rgb);
                }
            }
        }
        self.flush_chain(&mut chain, smooth, w, rgb);
    }

    /// Append a run of points to the merged stroke chain. A run whose first
    /// point touches the chain's last point extends it; anything else flushes
    /// the current chain and starts a new one. `smooth=false` runs mark the
    /// chain as carrying real corners (polyline vertices).
    fn chain_append(
        &mut self,
        chain: &mut Vec<Vec2>,
        smooth: &mut bool,
        pts: &[Vec2],
        run_smooth: bool,
        w: f32,
        rgb: (u8, u8, u8),
    ) {
        if pts.len() < 2 {
            return;
        }
        if let Some(last) = chain.last() {
            if (*last - pts[0]).len() < 1e-7 {
                *smooth &= run_smooth;
                chain.extend_from_slice(&pts[1..]);
                return;
            }
        }
        self.flush_chain(chain, *smooth, w, rgb);
        *smooth = run_smooth;
        chain.extend_from_slice(pts);
    }

    /// Push the merged chain as one stroke. A chain whose endpoints coincide
    /// (≥3 points) is closed — the seam then gets the pen's JOIN style instead
    /// of two caps. The duplicated closing point (a closed polyline's last
    /// segment ends back at the first vertex) is dropped so the seam vertex
    /// appears once.
    fn flush_chain(&mut self, chain: &mut Vec<Vec2>, smooth: bool, w: f32, rgb: (u8, u8, u8)) {
        if chain.len() < 2 {
            chain.clear();
            return;
        }
        let mut closed = chain.len() > 2
            && (chain[0] - chain[chain.len() - 1]).len() < 1e-7;
        if closed {
            chain.pop();
            if chain.len() < 3 {
                closed = false;
            }
        }
        self.push_stroke(chain, closed, smooth, w, rgb);
        chain.clear();
    }

    /// Expand a non-Solid pen fill style into scene pattern geometry. The loops
    /// are already in PAPER space (post-transform), so the pattern lines / dots
    /// are generated at fixed paper-mm spacings (AutoCAD CTB analog) and
    /// clipped to the polygon's bbox. The Fill itself is emitted solid; these
    /// extra prims layer on top in the fill's rgb.
    fn emit_fill_pattern(&mut self, loops: &[Vec<(f64, f64)>], rgb: (u8, u8, u8)) {
        let fill = match self.cur_fill {
            FillStyle::UseObject | FillStyle::Solid => return,
            f => f,
        };
        if loops.is_empty() {
            return;
        }
        let (mut mnx, mut mny) = (f64::INFINITY, f64::INFINITY);
        let (mut mxx, mut mxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for lp in loops {
            for &(x, y) in lp {
                mnx = mnx.min(x); mny = mny.min(y);
                mxx = mxx.max(x); mxy = mxy.max(y);
            }
        }
        if !mnx.is_finite() || !mxx.is_finite() {
            return;
        }
        let mn = Vec2::new(mnx, mny);
        let mx = Vec2::new(mxx, mxy);
        let dither = self.cur_dither;
        let mut extra: Vec<Prim> = Vec::new();
        match fill {
            FillStyle::HorizontalBars => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(1.0, 0.0), 1.5, rgb, dither));
            }
            FillStyle::VerticalBars => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(0.0, 1.0), 1.5, rgb, dither));
            }
            FillStyle::SlantRight => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(1.0, 1.0), 2.0, rgb, dither));
            }
            FillStyle::SlantLeft => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(-1.0, 1.0), 2.0, rgb, dither));
            }
            FillStyle::Diamonds => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(1.0, 1.0), 2.0, rgb, dither));
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(-1.0, 1.0), 2.0, rgb, dither));
            }
            FillStyle::Crosshatch => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(1.0, 0.0), 2.0, rgb, dither));
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(0.0, 1.0), 2.0, rgb, dither));
            }
            FillStyle::Checkerboard => {
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(1.0, 0.0), 2.0, rgb, dither));
                extra.extend(pattern_line_strokes(mn, mx, Vec2::new(0.0, 1.0), 2.0, rgb, dither));
                extra.extend(checkerboard_squares(mn, mx, loops, rgb, dither));
            }
            FillStyle::SquareDots => {
                extra.extend(square_dots(mn, mx, loops, rgb, dither));
            }
            FillStyle::UseObject | FillStyle::Solid => {}
        }
        self.scene.prims.extend(extra);
    }

    fn emit_blockref(&mut self, br: &BlockRef, parent: ((u8, u8, u8), Option<u8>), depth: u32) {
        let Some(block) = self.doc.blocks.get(br.block) else { return };
        // Clone the child geoms + styles out so we don't hold a borrow of the
        // block table across the recursive &mut self calls.
        let base = block.base;
        let children: Vec<(Geom, Style)> = block
            .dobjects
            .iter()
            .map(|d| (br.transform_geom(&d.geom, base), d.style))
            .collect();
        for (g, style) in &children {
            // ByBlock children inherit the instance's resolved color.
            self.emit_dobject(g, style, Some(parent), depth);
        }
    }

    fn emit_text(&mut self, t: &cad_kernel::text::Text, rgb: (u8, u8, u8)) {
        // Lazily build the font manager on first text; skip text if fonts aren't
        // ready (e.g. headless with no system fonts).
        if self.fonts.is_none() {
            self.fonts = Some(cad_text::FontManager::new());
        }
        let ready = self.fonts.as_ref().map(|f| f.is_ready()).unwrap_or(false);
        if !ready {
            return;
        }
        // Resolve the font: entity override else the text style's font.
        let font_name: String = if !t.font_name.is_empty() {
            t.font_name.clone()
        } else {
            self.doc.text_styles.get(t.style).map(|s| s.font_name.clone()).unwrap_or_default()
        };
        let req = cad_text::TextRequest {
            text: &t.text,
            font_name: &font_name,
            position: t.position,
            height: t.height,
            angle: t.angle,
            h_align: t.h_align,
            v_align: t.v_align,
            fill_mode: cad_text::FillMode::Fill,
            slant: t.oblique,
            x_scale: t.width_factor,
        };
        let glyphs = self.fonts.as_mut().unwrap().render(&req);
        if glyphs.is_empty() {
            return;
        }
        let tris: Vec<[(f64, f64); 3]> = glyphs
            .fills
            .iter()
            // Window clip: keep glyph triangles fully inside the rect (approx).
            .filter(|tri| match self.clip {
                Some((mn, mx)) => tri.iter().all(|p| {
                    p.x >= mn.x && p.x <= mx.x && p.y >= mn.y && p.y <= mx.y
                }),
                None => true,
            })
            .map(|tri| [self.tf(tri[0]), self.tf(tri[1]), self.tf(tri[2])])
            .collect();
        if !tris.is_empty() {
            self.scene.prims.push(Prim::Tris { tris, rgb });
        }
    }
}

// ---- geometry samplers (world space) ----

fn sample_circle(center: Vec2, radius: f64, n: usize) -> Vec<Vec2> {
    (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            Vec2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect()
}

fn sample_arc(center: Vec2, radius: f64, start: f64, sweep: f64, n: usize) -> Vec<Vec2> {
    let n = n.max(1);
    (0..=n)
        .map(|i| {
            let a = start + sweep * (i as f64 / n as f64);
            Vec2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect()
}

// ---- window clipping (world space) ----

/// Clip a segment to an axis-aligned rect (Liang-Barsky). Returns the visible
/// sub-segment, or None if fully outside.
fn clip_seg(a: Vec2, b: Vec2, mn: Vec2, mx: Vec2) -> Option<(Vec2, Vec2)> {
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let p = [-dx, dx, -dy, dy];
    let q = [a.x - mn.x, mx.x - a.x, a.y - mn.y, mx.y - a.y];
    for i in 0..4 {
        if p[i].abs() < 1e-12 {
            if q[i] < 0.0 {
                return None; // parallel to this edge and outside it
            }
        } else {
            let r = q[i] / p[i];
            if p[i] < 0.0 {
                if r > t1 { return None; }
                if r > t0 { t0 = r; }
            } else {
                if r < t0 { return None; }
                if r < t1 { t1 = r; }
            }
        }
    }
    Some((
        Vec2::new(a.x + t0 * dx, a.y + t0 * dy),
        Vec2::new(a.x + t1 * dx, a.y + t1 * dy),
    ))
}

/// Clip a polyline to a rect, returning the visible runs (each an open polyline).
fn clip_polyline_rect(pts: &[Vec2], closed: bool, mn: Vec2, mx: Vec2) -> Vec<Vec<Vec2>> {
    let n = pts.len();
    if n < 2 {
        return Vec::new();
    }
    let mut segs: Vec<(Vec2, Vec2)> = Vec::with_capacity(n);
    for i in 0..n - 1 {
        segs.push((pts[i], pts[i + 1]));
    }
    if closed {
        segs.push((pts[n - 1], pts[0]));
    }
    let mut runs: Vec<Vec<Vec2>> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    for (a, b) in segs {
        match clip_seg(a, b, mn, mx) {
            Some((ca, cb)) => {
                if cur.is_empty() {
                    cur.push(ca);
                    cur.push(cb);
                } else if (*cur.last().unwrap() - ca).len() < 1e-7 {
                    cur.push(cb); // contiguous → extend the run
                } else {
                    runs.push(std::mem::take(&mut cur));
                    cur.push(ca);
                    cur.push(cb);
                }
            }
            None => {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs
}

/// Clip a polygon to an axis-aligned rect (Sutherland-Hodgman).
fn clip_polygon_rect(poly: &[Vec2], mn: Vec2, mx: Vec2) -> Vec<Vec2> {
    if poly.len() < 3 {
        return Vec::new();
    }
    // (axis-kind, boundary value): 0 = x≥mn.x, 1 = x≤mx.x, 2 = y≥mn.y, 3 = y≤mx.y.
    let planes: [(u8, f64); 4] = [(0, mn.x), (1, mx.x), (2, mn.y), (3, mx.y)];
    let mut out = poly.to_vec();
    for (kind, val) in planes {
        if out.is_empty() {
            break;
        }
        let inp = std::mem::take(&mut out);
        let inside = |p: Vec2| match kind {
            0 => p.x >= val,
            1 => p.x <= val,
            2 => p.y >= val,
            _ => p.y <= val,
        };
        let intersect = |a: Vec2, b: Vec2| {
            let t = match kind {
                0 | 1 => {
                    let d = b.x - a.x;
                    if d.abs() < 1e-12 { 0.0 } else { (val - a.x) / d }
                }
                _ => {
                    let d = b.y - a.y;
                    if d.abs() < 1e-12 { 0.0 } else { (val - a.y) / d }
                }
            };
            Vec2::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y))
        };
        let m = inp.len();
        for i in 0..m {
            let cur = inp[i];
            let prev = inp[(i + m - 1) % m];
            let (ci, pi) = (inside(cur), inside(prev));
            if ci {
                if !pi {
                    out.push(intersect(prev, cur));
                }
                out.push(cur);
            } else if pi {
                out.push(intersect(prev, cur));
            }
        }
    }
    out
}

// ---- fill-style pattern geometry (paper space) ----

/// Phase-rotate a dash pattern (dash/gap alternating, starting with a dash) so
/// a polyline of paper length `len_paper` ENDS inside a dash. Returns the
/// pattern offset (paper mm) — the pattern position at the path start (the SVG
/// `stroke-dashoffset` / PDF dash-phase convention). 0 = already ends in a dash.
fn adaptive_dash_offset(pattern: &[f32], len_paper: f64) -> f32 {
    let total: f64 = pattern.iter().map(|&v| v.abs() as f64).sum();
    if total <= 1e-9 {
        return 0.0;
    }
    let ph = len_paper.rem_euclid(total);
    let mut cum = 0.0_f64;
    for (i, &v) in pattern.iter().enumerate() {
        let seg = v.abs() as f64;
        let end = cum + seg;
        if i % 2 == 0 {
            // dash: the end already lands inside a dash.
            if ph >= cum - 1e-9 && ph <= end + 1e-9 {
                return 0.0;
            }
        } else if ph > cum + 1e-9 && ph < end - 1e-9 {
            // gap: rotate so the end lands at the END of the preceding dash.
            return (total - (ph - cum)) as f32;
        }
        cum = end;
    }
    0.0
}

/// One parallel-line family for a fill style: lines along `dir` (paper mm) at
/// `spacing`, covering the bbox `mn..mx`, each clipped to the bbox rect via
/// `clip_polyline_rect` and emitted as a thin 0.1 mm stroke in the fill's rgb.
fn pattern_line_strokes(
    mn: Vec2,
    mx: Vec2,
    dir: Vec2,
    spacing: f64,
    rgb: (u8, u8, u8),
    dither: bool,
) -> Vec<Prim> {
    let u = dir.normalized();
    let n = u.perp();
    let corners = [mn, Vec2::new(mx.x, mn.y), Vec2::new(mn.x, mx.y), mx];
    let (mut c_min, mut c_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut t_min, mut t_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in &corners {
        c_min = c_min.min(c.dot(n));
        c_max = c_max.max(c.dot(n));
        t_min = t_min.min(c.dot(u));
        t_max = t_max.max(c.dot(u));
    }
    if !c_min.is_finite() || !c_max.is_finite() || spacing <= 1e-9 {
        return Vec::new();
    }
    // Runaway guard: a huge region at fixed spacing must not freeze the build.
    if ((c_max - c_min) / spacing).ceil() > 10_000.0 {
        return Vec::new();
    }
    let mut out: Vec<Prim> = Vec::new();
    let mut c = (c_min / spacing).ceil() * spacing;
    while c <= c_max + 1e-9 {
        let p0 = n * c + u * t_min;
        let p1 = n * c + u * t_max;
        for run in clip_polyline_rect(&[p0, p1], false, mn, mx) {
            if run.len() >= 2 {
                out.push(Prim::Stroke {
                    pts: run.iter().map(|p| (p.x, p.y)).collect(),
                    closed: false,
                    width_mm: 0.1,
                    rgb,
                    dash_mm: Vec::new(),
                    dash_offset_mm: 0.0,
                    cap: EndStyle::Round,
                    join: JoinStyle::Round,
                    dither,
                    smooth: false,
                });
            }
        }
        c += spacing;
    }
    out
}

/// Even-odd point-in-loops test (outer loops fill; successive loops are holes).
fn point_in_loops(p: (f64, f64), loops: &[Vec<(f64, f64)>]) -> bool {
    let mut inside = false;
    for lp in loops {
        let n = lp.len();
        for i in 0..n {
            let (x0, y0) = lp[i];
            let (x1, y1) = lp[(i + 1) % n];
            if (y0 > p.1) != (y1 > p.1) {
                let x = x0 + (p.1 - y0) / (y1 - y0) * (x1 - x0);
                if p.0 < x {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

/// The integer cell range for one bbox axis, or None when degenerate / too big.
/// The span is bounded in f64 FIRST: f64→i64 casts saturate for extreme
/// coordinates, and the saturated values can overflow the cell-count product
/// (debug panic / release wrap), which would defeat the runaway guards and
/// let the grid loops run ~2^64 iterations.
fn grid_range(lo: f64, hi: f64, cell: f64, max_cells: f64) -> Option<(i64, i64)> {
    let span = hi - lo;
    if !span.is_finite() || span <= 0.0 {
        return None;
    }
    let cells = (span / cell).ceil();
    if cells < 0.0 || cells > max_cells {
        return None;
    }
    Some((lo.div_euclid(cell) as i64, hi.div_euclid(cell) as i64))
}

/// Checkerboard: 2.0 mm grid with a full-cell square fill on alternate cells
/// (grid anchored at the origin, so adjacent fills align across entities).
fn checkerboard_squares(
    mn: Vec2,
    mx: Vec2,
    loops: &[Vec<(f64, f64)>],
    rgb: (u8, u8, u8),
    dither: bool,
) -> Vec<Prim> {
    const CELL: f64 = 2.0;
    let Some((i0, i1)) = grid_range(mn.x, mx.x, CELL, 100_000.0) else { return Vec::new() };
    let Some((j0, j1)) = grid_range(mn.y, mx.y, CELL, 100_000.0) else { return Vec::new() };
    if (i1 - i0 + 1).max(0) * (j1 - j0 + 1).max(0) > 200_000 {
        return Vec::new();
    }
    let mut out: Vec<Prim> = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            if (i + j).rem_euclid(2) != 0 {
                continue;
            }
            let (x0, y0) = (i as f64 * CELL, j as f64 * CELL);
            let (cx, cy) = (x0 + CELL * 0.5, y0 + CELL * 0.5);
            if !point_in_loops((cx, cy), loops) {
                continue;
            }
            out.push(Prim::Fill {
                loops: vec![vec![
                    (x0, y0),
                    (x0 + CELL, y0),
                    (x0 + CELL, y0 + CELL),
                    (x0, y0 + CELL),
                ]],
                rgb,
                dither,
            });
        }
    }
    out
}

/// SquareDots: a 2.0 mm grid of 0.2 mm square fills (centres inside the loops).
fn square_dots(
    mn: Vec2,
    mx: Vec2,
    loops: &[Vec<(f64, f64)>],
    rgb: (u8, u8, u8),
    dither: bool,
) -> Vec<Prim> {
    const SPACING: f64 = 2.0;
    const SIZE: f64 = 0.2;
    let Some((i0, i1)) = grid_range(mn.x, mx.x, SPACING, 100_000.0) else { return Vec::new() };
    let Some((j0, j1)) = grid_range(mn.y, mx.y, SPACING, 100_000.0) else { return Vec::new() };
    if (i1 - i0 + 1).max(0) * (j1 - j0 + 1).max(0) > 200_000 {
        return Vec::new();
    }
    let h = SIZE * 0.5;
    let mut out: Vec<Prim> = Vec::new();
    for j in j0..=j1 {
        for i in i0..=i1 {
            let (cx, cy) = (i as f64 * SPACING, j as f64 * SPACING);
            if !point_in_loops((cx, cy), loops) {
                continue;
            }
            out.push(Prim::Fill {
                loops: vec![vec![
                    (cx - h, cy - h),
                    (cx + h, cy - h),
                    (cx + h, cy + h),
                    (cx - h, cy + h),
                ]],
                rgb,
                dither,
            });
        }
    }
    out
}

