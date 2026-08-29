// Layout — paper-space definitions (AutoCAD Layout analog).
//
// A Layout is a named paper-space workspace with its own paper size, plot
// config, camera, paper-space entities, viewports into model space, and
// per-layout layer table. Documents start with zero layouts (model-space
// only); the user creates Layout tabs as needed.
//
// Model space lives in `Document.dobjects` + `Document.layers`.
// Paper space lives in `Layout.entities` + `Layout.layers`.
// A Viewport is a paper-space entity (`Geom::Viewport`) that defines a
// rectangular window peering into model space with its own camera.

use crate::dobject::DObject;
use crate::layer::LayerTable;
use crate::math::Vec2;
use crate::plotstyle::{Offset, PaperSize, PlotArea, PlotScale};

/// Saved camera state for a layout's on-screen view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutCamera {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for LayoutCamera {
    fn default() -> Self {
        Self { zoom: 1.0, pan_x: 0.0, pan_y: 0.0 }
    }
}

/// A rectangular window in paper space that peers into model space.
///
/// `shape_id` links to a paper-space `DObject` whose geometry is
/// `Geom::Viewport`. The app syncs the rect / model-camera from that
/// shape on save and restores on load.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewportData {
    /// Handle of the paper-space entity (`Geom::Viewport`) that draws
    /// the viewport frame. None = orphaned (shape was deleted).
    pub shape_handle: Option<u64>,
    /// The viewport rectangle on the paper in mm (bottom-left origin).
    pub rect_min: (f64, f64),
    pub rect_max: (f64, f64),
    /// The point in model space centred in this viewport.
    pub model_center: (f64, f64),
    /// Combined camera zoom for this viewport's view of model space.
    pub model_zoom: f64,
    /// Plot scale: paper_mm per model_unit (e.g. 0.01 for 1:100).
    pub model_scale: f64,
    /// Layer IDs frozen (hidden) in this viewport. Independent of the
    /// layout-level or document-level layer freeze.
    pub frozen_layers: Vec<u32>,
    /// Per-viewport CTB table name override. None = inherit from layout.
    pub ctb_name: Option<String>,
    /// Scale lock. When true, the model view inside the viewport is FROZEN:
    /// wheel-zoom / pan inside it are ignored so the applied plot scale can't
    /// drift. Frame move/resize still work (that's the paper layout, not the
    /// view). Toggled by the corner lock badge / right-click menu.
    pub locked: bool,
}

impl ViewportData {
    pub fn new(
        rect_min: (f64, f64),
        rect_max: (f64, f64),
        model_center: (f64, f64),
        model_zoom: f64,
        model_scale: f64,
    ) -> Self {
        Self {
            shape_handle: None,
            rect_min,
            rect_max,
            model_center,
            model_zoom,
            model_scale,
            frozen_layers: Vec::new(),
            ctb_name: None,
            locked: false,
        }
    }
}

/// A named paper-space layout.
#[derive(Clone, Debug)]
pub struct Layout {
    pub name: String,
    /// Physical page size in mm (before orientation swap).
    pub paper: PaperSize,
    pub orientation: crate::plotstyle::Orientation,
    /// Physical page dims after orientation (mm). Derived from `paper`
    /// and `orientation`; stored directly to avoid recomputing.
    pub page_w_mm: f64,
    pub page_h_mm: f64,
    pub plot_area: PlotArea,
    pub plot_scale: PlotScale,
    pub plot_offset: Offset,
    pub margins_mm: f64,
    /// CTB table name (references a `.pst` file or built-in).
    pub ctb_name: Option<String>,
    pub plot_with_styles: bool,
    pub plot_object_lineweights: bool,
    /// Saved on-screen camera for the paper-space view.
    pub camera: LayoutCamera,
    /// Paper-space entities (title blocks, viewport frames, notes).
    pub entities: Vec<DObject>,
    /// Viewports that peer from this paper into model space.
    pub viewports: Vec<ViewportData>,
    /// Per-layout layer table. Swapped in when this layout is active.
    pub layers: LayerTable,
}

impl Layout {
    pub fn new(name: impl Into<String>, paper: PaperSize, orientation: crate::plotstyle::Orientation) -> Self {
        let (pw0, ph0) = paper.dims_mm();
        let (page_w_mm, page_h_mm) = match orientation {
            crate::plotstyle::Orientation::Portrait => (pw0 as f64, ph0 as f64),
            crate::plotstyle::Orientation::Landscape => (ph0 as f64, pw0 as f64),
        };
        Self {
            name: name.into(),
            paper,
            orientation,
            page_w_mm,
            page_h_mm,
            plot_area: PlotArea::Extents,
            plot_scale: PlotScale::Fit,
            plot_offset: Offset::Center,
            margins_mm: 5.0,
            ctb_name: None,
            plot_with_styles: true,
            plot_object_lineweights: true,
            camera: LayoutCamera::default(),
            entities: Vec::new(),
            viewports: Vec::new(),
            layers: LayerTable::with_defaults(),
        }
    }

    /// The effective page size in mm (orientation already applied).
    pub fn page_dims(&self) -> (f64, f64) {
        (self.page_w_mm, self.page_h_mm)
    }

    /// Find a viewport by its shape handle.
    pub fn viewport_by_shape(&self, handle: u64) -> Option<&ViewportData> {
        self.viewports.iter().find(|vp| vp.shape_handle == Some(handle))
    }

    /// Find a viewport by its shape handle (mutable).
    pub fn viewport_by_shape_mut(&mut self, handle: u64) -> Option<&mut ViewportData> {
        self.viewports.iter_mut().find(|vp| vp.shape_handle == Some(handle))
    }

    /// Sync viewport rect from its Geom::Viewport entity.
    pub fn sync_viewport_rect(&mut self, vp: &mut ViewportData) {
        if let Some(h) = vp.shape_handle {
            if let Some(d) = self.entities.iter().find(|e| e.handle == h) {
                if let crate::geom::Geom::Viewport(v) = &d.geom {
                    let (mn, mx) = v.bbox_world();
                    vp.rect_min = (mn.x, mn.y);
                    vp.rect_max = (mx.x, mx.y);
                    vp.model_center = (v.model_center.x, v.model_center.y);
                    vp.model_zoom = v.model_zoom;
                    vp.model_scale = v.model_scale;
                }
            }
        }
    }

    /// Sync all viewport rects from their shape entities.
    pub fn sync_all_viewports(&mut self) {
        let _len = self.viewports.len();
        let mut to_update: Vec<(u64, (f64, f64), (f64, f64), (f64, f64), f64, f64)> = Vec::new();
        for vp in &self.viewports {
            if let Some(h) = vp.shape_handle {
                if let Some(d) = self.entities.iter().find(|e| e.handle == h) {
                    if let crate::geom::Geom::Viewport(v) = &d.geom {
                        let (mn, mx) = v.bbox_world();
                        to_update.push((h, (mn.x, mn.y), (mx.x, mx.y),
                            (v.model_center.x, v.model_center.y), v.model_zoom, v.model_scale));
                    }
                }
            }
        }
        for (h, mn, mx, mc, zoom, scale) in to_update {
            if let Some(vp) = self.viewports.iter_mut().find(|v| v.shape_handle == Some(h)) {
                vp.rect_min = mn;
                vp.rect_max = mx;
                vp.model_center = mc;
                vp.model_zoom = zoom;
                vp.model_scale = scale;
            }
        }
    }
}

/// On-screen geometry for a viewport entity in paper space.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewportGeom {
    /// Center of the viewport rectangle on the paper (mm, bottom-left origin).
    pub center: Vec2,
    /// Width on paper (mm).
    pub width: f64,
    /// Height on paper (mm).
    pub height: f64,
    /// The point in model space centred in this viewport.
    pub model_center: Vec2,
    /// Combined zoom factor for the model-space view.
    pub model_zoom: f64,
    /// Plot scale: paper_mm per model_unit (e.g. 0.01 for 1:100).
    pub model_scale: f64,
    /// Is the viewport frame visible (on-screen / plotted)?
    pub frame_visible: bool,
}

impl ViewportGeom {
    /// Axis-aligned bounding box of the viewport rect on paper.
    pub fn bbox_world(&self) -> (Vec2, Vec2) {
        let half_w = self.width * 0.5;
        let half_h = self.height * 0.5;
        (
            Vec2::new(self.center.x - half_w, self.center.y - half_h),
            Vec2::new(self.center.x + half_w, self.center.y + half_h),
        )
    }

    /// Four corners of the viewport rectangle (paper mm, bottom-left origin).
    pub fn corners(&self) -> [Vec2; 4] {
        let (mn, mx) = self.bbox_world();
        [
            mn,
            Vec2::new(mx.x, mn.y),
            mx,
            Vec2::new(mn.x, mx.y),
        ]
    }
}

impl Default for ViewportGeom {
    fn default() -> Self {
        Self {
            center: Vec2::new(150.0, 100.0),
            width: 200.0,
            height: 150.0,
            model_center: Vec2::ZERO,
            model_zoom: 1.0,
            model_scale: 1.0,
            frame_visible: true,
        }
    }
}
