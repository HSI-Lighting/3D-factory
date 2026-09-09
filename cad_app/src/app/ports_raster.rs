use super::*;


// ============ raster editor (from dokkandar/Auto_RASM) ============

/// Output geometry a buffer layer converts its marked line-work into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BufferGeom {
    Lines,
    Arcs,
    Nurbs,
}

impl BufferGeom {
    pub fn label(self) -> &'static str {
        match self {
            BufferGeom::Lines => "Straight lines",
            BufferGeom::Arcs => "Arcs",
            BufferGeom::Nurbs => "NURBS",
        }
    }
}

/// A BUFFER LAYER — one conversion job AND a destructive raster carve. Marking
/// an area with the brush *moves ownership* of those pixels (at their original
/// positions) from the source into this layer's `mask` — exclusively, so a
/// pixel belongs to exactly one layer. The source image thus splits apart layer
/// by layer (Photoshop-style). On Convert the owned pixels are traced as `geom`
/// and emitted as DObjects of `dobject_aci` on CAD layer `cad_layer`.
pub(super) struct BufferLayer {
    pub id: u64,
    pub name: String,
    pub geom: BufferGeom,
    pub cad_layer: String,
    pub dobject_aci: u8,
    /// Identity colour — also the tint shown over this layer's carved pixels.
    pub marker: egui::Color32,
    /// 255 = this pixel was carved (moved) into THIS layer. Same size as the
    /// editor preview base; positions follow the original image exactly.
    pub mask: image::GrayImage,
    /// Show this layer's carved pixels in the composite. Hide it → the hole it
    /// left in the source shows through, so you can watch the image peel apart.
    pub visible: bool,
    /// Tint carved pixels with `marker` (see which layer owns them); off = show
    /// the original pixels in place.
    pub tint: bool,
}

impl BufferLayer {
    pub fn new(id: u64, name: String, w: u32, h: u32, marker: egui::Color32) -> Self {
        Self {
            id,
            name,
            geom: BufferGeom::Lines,
            cad_layer: "0".into(),
            dobject_aci: 7,
            marker,
            mask: image::GrayImage::new(w, h),
            visible: true,
            tint: true,
        }
    }
    /// Number of pixels carved into this layer.
    pub fn marked(&self) -> usize {
        self.mask.pixels().filter(|p| p.0[0] > 0).count()
    }
}

pub(super) struct RasterEditor {
    /// Filename (for the title).
    pub name: String,
    /// Downscaled preview base (fast adjust loop); full-res lives in raster_doc.
    pub base: image::DynamicImage,
    pub grayscale: bool,
    pub brightness: i32,
    pub contrast: f32,
    pub invert: bool,
    pub threshold_on: bool,
    pub threshold: u8,
    /// Re-derive the working source + report next frame (adjustment changed).
    pub dirty: bool,
    /// Re-composite the display next frame (a carve / visibility / tint change).
    pub composite_dirty: bool,
    pub tex: Option<egui::TextureHandle>,
    pub report: Option<cad_raster::Report>,
    /// Buffer (conversion) layers + which one the brush paints into.
    pub buffers: Vec<BufferLayer>,
    pub active_buf: Option<usize>,
    pub next_buf_id: u64,
    /// Brush radius in screen pixels.
    pub brush: f32,
}

impl RasterEditor {
    pub fn new(name: String, base: image::DynamicImage) -> Self {
        Self {
            name,
            base,
            grayscale: false,
            brightness: 0,
            contrast: 1.0,
            invert: false,
            threshold_on: false,
            threshold: 128,
            dirty: true,
            composite_dirty: false,
            tex: None,
            report: None,
            buffers: Vec::new(),
            active_buf: None,
            next_buf_id: 1,
            brush: 14.0,
        }
    }
    /// Add a buffer layer (sized to the preview base) with a fresh marker colour.
    pub fn add_buffer(&mut self) {
        use image::GenericImageView;
        let (w, h) = self.base.dimensions();
        let id = self.next_buf_id;
        self.next_buf_id += 1;
        let marker = editor_param_color((id as usize).wrapping_sub(1));
        self.buffers
            .push(BufferLayer::new(id, format!("buffer {}", id), w, h, marker));
        self.active_buf = Some(self.buffers.len() - 1);
    }
    /// CARVE: move a filled disc (radius `r` px, centre image pixel cx,cy) into
    /// buffer layer `ai`. Ownership is EXCLUSIVE — every pixel touched is set in
    /// `ai`'s mask and cleared from every other layer, so the source splits into
    /// disjoint layers. Positions are preserved (the layer mask is base-sized).
    pub fn carve(&mut self, ai: usize, cx: i32, cy: i32, r: i32) {
        use image::GenericImageView;
        let (w, h) = self.base.dimensions();
        let (w, h) = (w as i32, h as i32);
        let r2 = r * r;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let (ux, uy) = (x as u32, y as u32);
                for (i, b) in self.buffers.iter_mut().enumerate() {
                    if i == ai {
                        b.mask.put_pixel(ux, uy, image::Luma([255]));
                    } else if b.mask.get_pixel(ux, uy).0[0] > 0 {
                        b.mask.put_pixel(ux, uy, image::Luma([0]));
                    }
                }
            }
        }
        self.composite_dirty = true;
    }
    /// Build the display composite from the adjusted source `work`: source pixels
    /// stay in place; each carved pixel is tinted by its layer's marker (if
    /// `tint`), shown as-is (if visible & not tinted), or punched transparent
    /// (if the layer is hidden — revealing the peel). Transparent holes show the
    /// dark canvas behind, so hiding a layer reads as "those pixels left here".
    pub fn composite_image(&self, work: &image::DynamicImage) -> image::RgbaImage {
        let mut out = work.to_rgba8();
        let (w, h) = (out.width(), out.height());
        for b in &self.buffers {
            if b.mask.width() != w || b.mask.height() != h {
                continue;
            }
            let (mr, mg, mb) = (
                b.marker.r() as u32,
                b.marker.g() as u32,
                b.marker.b() as u32,
            );
            for y in 0..h {
                for x in 0..w {
                    if b.mask.get_pixel(x, y).0[0] == 0 {
                        continue;
                    }
                    if !b.visible {
                        out.put_pixel(x, y, image::Rgba([0, 0, 0, 0])); // hole
                    } else if b.tint {
                        let o = out.get_pixel(x, y).0;
                        // 55% marker over the original pixel
                        let mix = |c: u32, m: u32| ((c * 45 + m * 55) / 100) as u8;
                        out.put_pixel(
                            x,
                            y,
                            image::Rgba([
                                mix(o[0] as u32, mr),
                                mix(o[1] as u32, mg),
                                mix(o[2] as u32, mb),
                                255,
                            ]),
                        );
                    }
                    // visible & not tinted → leave the original pixel in place
                }
            }
        }
        out
    }
    /// Apply the current adjustments to the preview base.
    pub fn working(&self) -> image::DynamicImage {
        use cad_raster::AdjustKind;
        let mut img = self.base.clone();
        if self.grayscale {
            img = AdjustKind::Grayscale.apply(&img);
        }
        if self.brightness != 0 || (self.contrast - 1.0).abs() > 1e-3 {
            img = AdjustKind::BrightnessContrast {
                brightness: self.brightness,
                contrast: self.contrast,
            }
            .apply(&img);
        }
        if self.invert {
            img = AdjustKind::Invert.apply(&img);
        }
        if self.threshold_on {
            img = AdjustKind::Threshold(self.threshold).apply(&img);
        }
        img
    }
    pub fn reset(&mut self) {
        self.grayscale = false;
        self.brightness = 0;
        self.contrast = 1.0;
        self.invert = false;
        self.threshold_on = false;
        self.threshold = 128;
        self.dirty = true;
    }
}

/// Which kind of thing an overlay outline came from — so a wall, a room boundary and a chair are
/// tellable apart at a glance on a busy plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PlanLayer {
    Solid,
    Room,
    Furniture,
}

impl PlanLayer {
    fn color(self) -> egui::Color32 {
        match self {
            PlanLayer::Solid => egui::Color32::from_rgba_unmultiplied(150, 190, 230, 170),
            PlanLayer::Room => egui::Color32::from_rgba_unmultiplied(140, 215, 170, 165),
            PlanLayer::Furniture => egui::Color32::from_rgba_unmultiplied(230, 180, 90, 190),
        }
    }
}

impl CadApp {
    /// File ▸ Import ▸ Image — load a raster into a `RasterDoc` and run the
    /// convertibility analyzer. The interactive raster→vector editor is a
    /// later slice; for now the doc is held in memory and the report logged.
    pub(super) fn do_import_image(&mut self, path: &str) {
        match cad_raster::RasterDoc::load(path) {
            Ok(doc) => {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());
                self.history.push(format!(
                    "  ✔ imported '{}'  {}×{} — opening raster editor",
                    name,
                    doc.width(),
                    doc.height()
                ));
                // Downscaled preview base for a fast adjust loop; the analyzer
                // verdict is ADVISORY — the editor opens regardless of class.
                let preview_base = doc.base.thumbnail(900, 900);
                self.raster_editor = Some(RasterEditor::new(name, preview_base));
                self.raster_doc = Some(doc);
            }
            Err(e) => self
                .history
                .push(format!("  ! import image '{}': {}", path, e)),
        }
    }

    /// File ▸ Import ▸ Image as raster — embed the image in the DOCUMENT as a
    /// reference UNDERLAY (no vectorization; persisted in RSM). Stores the raw
    /// encoded bytes + placement at 1 unit/px with its bottom-left corner at the
    /// world origin, then fits the view to it. The GPU texture is built lazily
    /// by `sync_underlay_textures`.
    pub(super) fn import_raster_underlay(&mut self, path: &str) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.history
                    .push(format!("  ! import raster '{}': {}", path, e));
                return;
            }
        };
        // Decode once just to learn the pixel dimensions for placement.
        let (w, h) = match image::load_from_memory(&bytes) {
            Ok(im) => (im.width(), im.height()),
            Err(e) => {
                self.history
                    .push(format!("  ! import raster '{}': {}", path, e));
                return;
            }
        };
        let name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        let (world_w, world_h) = (w as f64, h as f64);
        self.snapshot_doc();
        self.doc.raster_images.push(cad_kernel::RasterImage {
            name: name.clone(),
            data: std::sync::Arc::new(bytes),
            insert: Vec2::new(0.0, world_h),
            world_w,
            world_h,
        });
        // Fit the view to the freshly placed image (centre + scale to canvas).
        if let Some(rect) = self.canvas_screen_rect {
            self.world_offset = egui::vec2(-(world_w as f32) / 2.0, -(world_h as f32) / 2.0);
            let s = ((rect.width() / world_w as f32).min(rect.height() / world_h as f32)) * 0.9;
            if s.is_finite() && s > 0.0 {
                self.scale = s;
            }
        }
        self.touch_view();
        self.history.push(format!(
            "  ✔ embedded raster underlay '{}'  {}×{}  (saved in RSM; drafted over, not vectorized)",
            name, w, h));
    }

    /// Rebuild the underlay texture cache from `doc.raster_images` when the set
    /// changes (import / detach / file-open / undo / redo). A cheap signature
    /// (count + each Arc identity + insert) detects the change without flags.
    pub(super) fn sync_underlay_textures(&mut self, ctx: &egui::Context) {
        let mut sig = self.doc.raster_images.len() as u64;
        for img in &self.doc.raster_images {
            sig = sig
                .wrapping_mul(1099511628211)
                .wrapping_add(std::sync::Arc::as_ptr(&img.data) as u64);
            sig = sig.wrapping_add(img.insert.x.to_bits());
        }
        if sig == self.underlay_sig && self.underlay_tex.len() == self.doc.raster_images.len() {
            return;
        }
        self.underlay_tex.clear();
        for (i, img) in self.doc.raster_images.iter().enumerate() {
            let tex = match image::load_from_memory(&img.data) {
                Ok(im) => {
                    // Cap the texture so huge scans don't blow the GPU budget.
                    let im = if im.width().max(im.height()) > 4000 {
                        im.thumbnail(4000, 4000)
                    } else {
                        im
                    };
                    let rgba = im.to_rgba8();
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    );
                    ctx.load_texture(format!("underlay_{}", i), ci, egui::TextureOptions::LINEAR)
                }
                Err(_) => {
                    // Keep index alignment with a 1×1 transparent placeholder.
                    let ci = egui::ColorImage::new([1, 1], egui::Color32::TRANSPARENT);
                    ctx.load_texture(
                        format!("underlay_bad_{}", i),
                        ci,
                        egui::TextureOptions::NEAREST,
                    )
                }
            };
            self.underlay_tex.push(tex);
        }
        self.underlay_sig = sig;
    }

    /// When sketching ON a solid's face, draw the 3D object's feature edges (projected onto
    /// the sketch plane) faintly, so the 2D canvas is a true 2D VIEW of the object being
    /// drawn on — not a blank canvas. The projected coords ARE the sketch's (u,v), which is
    /// exactly the coordinate system the 2D canvas draws in, so `w2s` places them correctly.
    pub(super) fn draw_factory_sketch_reference(&self, painter: &egui::Painter, rect: egui::Rect) {
        if self.factory.session.is_none() || self.factory.sketch_ref.is_empty() {
            return;
        }
        let stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 175, 225, 150),
        );
        for [a, b] in &self.factory.sketch_ref {
            // The edges come off the SOLID, so they are metres; the canvas is the sketch's own
            // document. Those coincide today (a sketch is metre-space, k = 1) and `w2s_m` is
            // the identity there — but going through it keeps the two independent.
            let pa = self.w2s_m(Vec2::new(a.x as f64, a.y as f64), rect);
            let pb = self.w2s_m(Vec2::new(b.x as f64, b.y as f64), rect);
            painter.line_segment([pa, pb], stroke);
        }
    }

    /// Draw every FINISHED face-sketch onto the 2D plan, projected to XY — so all the
    /// sketches you've made stay visible in the plan all the time (2D↔3D linked), not only in
    /// 3D. The sketch you are ACTIVELY drafting is skipped here: it already IS the live 2D
    /// canvas. A wall sketch projects to its plan footprint (edge-on), which is what a plan
    /// view should show.
    pub(super) fn draw_factory_sketches_2d(&self, painter: &egui::Painter, rect: egui::Rect) {
        if !self.factory.open {
            return;
        }
        let stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(150, 170, 205, 130),
        );
        // GLOBAL view shows only what is drawn on the GROUND PLANE.
        //
        // Every finished sketch used to be flattened onto the plan at once, so a wall elevation
        // and a ceiling layout landed on top of the floor plan as one unreadable overlay — and
        // nothing said which lines belonged to which plane. The plan is a horizontal cut; a sketch
        // standing vertically in the model is not part of it.
        //
        // In any other view the canvas IS a sketch on that plane, so the same rule holds by
        // construction: you see the plane you are on.
        // The GROUND PLAN shows only what is drawn on it. A face plane is its own drawing, and
        // stacking every plane on the plan is what "global view" was invented to stop.
        let ground_only = self.factory.session.is_none();
        // ONE PLANE AT A TIME, unless asked otherwise. While a face sketch is open the other
        // planes were projected onto this canvas — reference at best, unselectable clutter at
        // worst: "i need a toggle to turn it off so i can only see the what ever is on the plane
        // i am drawing". The ground plan is a separate rule, handled below, and is unaffected.
        if !ground_only && !self.factory.show_other_planes {
            return;
        }
        for (i, sk) in self.factory.model.sketches.iter().enumerate() {
            if self
                .factory
                .session
                .as_ref()
                .is_some_and(|s| s.plane == sk.id)
            {
                continue; // the active sketch is the live canvas — don't double-draw it
            }
            if ground_only && sk.frame.normal().z.abs() < 0.999 {
                continue; // not a horizontal plane, so not part of the plan
            }
            for d in &sk.doc.dobjects {
                // By the SKETCH's own unit (metre-space, so k = 1 today) — `from_uv` below
                // expects the sketch's (u,v) already in metres.
                for path in cad_solid::geom_outlines_scaled(&d.geom, sk.doc.units.metres_per_unit) {
                    for w in path.windows(2) {
                        // `from_uv` lifts to WORLD metres, so the projection back onto the
                        // plan must divide by the drawing's unit.
                        let a = sk.frame.from_uv(glam::Vec2::new(w[0].x, w[0].y));
                        let b = sk.frame.from_uv(glam::Vec2::new(w[1].x, w[1].y));
                        let pa = self.w2s_m(Vec2::new(a.x as f64, a.y as f64), rect);
                        let pb = self.w2s_m(Vec2::new(b.x as f64, b.y as f64), rect);
                        painter.line_segment([pa, pb], stroke);
                    }
                }
            }
        }
    }

    /// WHAT the 2D overlay of the 3D model consists of — closed polygons in the canvas's own
    /// coordinates, tagged by layer so the painter can colour them apart.
    ///
    /// Split out of the painter so it is testable: the bug this replaced ("the FURN toggle does
    /// nothing") was in the *selection* of what to draw, not in the drawing, and there was no way
    /// to assert on it without a live egui pointer.
    fn plan_overlay_shapes(&self) -> Vec<(PlanLayer, Vec<Vec2>)> {
        let mut out = Vec::new();
        if !self.factory.show_furniture_outlines_2d {
            return out;
        }

        // WHICH PLANE AM I DRAWING ON? In the global view the canvas IS world XY, so a top-down
        // footprint lands where it belongs. Inside a sketch session — every standard view, and any
        // face sketch — the canvas is that plane's (u,v), and pushing world XY through `w2s_m`
        // would lay the plan on its side over a correct elevation.
        let frame = self
            .factory
            .session
            .as_ref()
            .and_then(|s| self.factory.model.sketch_by_id(s.plane))
            .map(|sk| sk.frame);

        // SOLIDS — buildings, walls, slabs, and every architecture generator.
        //
        // This overlay used to draw furniture and nothing else, and RETURNED EARLY when there was
        // none. So a model of a building with rooms in it put nothing at all on the plan and the
        // badge appeared to do nothing — which is how it was reported. What is in the model belongs
        // on the plan, whatever kind of thing it is.
        //
        // Union features only: a Difference is a VOID, and drawing the shape of a hole among the
        // walls reads as another wall.
        //
        // Global view only: on a sketch plane the solids are already drawn, properly projected as
        // an elevation, by `sketch_ref` (see `factory_enter_sketch`). Plan footprints on top of
        // that would be a second, wrong copy.
        if frame.is_none() {
            for f in &self.factory.model.features {
                if f.op != cad_solid::BoolOp::Union {
                    continue;
                }
                let outline = self.factory.feature_plan_footprint(f);
                if outline.len() >= 2 {
                    out.push((
                        PlanLayer::Solid,
                        outline
                            .iter()
                            .map(|p| Vec2::new(p.x as f64, p.y as f64))
                            .collect(),
                    ));
                }
            }

            // ROOMS. A carved room is a Difference — skipped with the other cutters above — and its
            // own walls were removed when it was carved. Without this a room is in the model, named
            // in the Rooms menu, and nowhere at all on the plan, even though its footprint is the
            // single most useful line on an architectural drawing.
            for room in &self.factory.rooms {
                if room.footprint.len() >= 2 {
                    out.push((
                        PlanLayer::Room,
                        room.footprint
                            .iter()
                            .map(|p| Vec2::new(p.x as f64, p.y as f64))
                            .collect(),
                    ));
                }
            }
        }

        // FURNITURE and APERTURES, in every view — global and sketch alike. `sketch_ref` is built
        // from the evaluated SOLID mesh and furniture is not in it, so on a plane this is the only
        // thing that puts a chair, a door or a window on the drawing.
        //
        // The footprint is the local bounding box's 8 corners, posed and hulled — O(8) per
        // instance, the same cheap corner transform `furniture_aabb` uses, so it costs nothing even
        // with heavy imported meshes (see the perf note on that function).
        for inst in &self.factory.furniture {
            let Some(asset) = self.factory.furniture_lib.get(inst.asset) else {
                continue;
            };
            let rm = inst.rot_mat();
            let s = inst.scale_vec();
            let pos = glam::Vec3::from(inst.pos);
            let (lmn, lmx) = (asset.local_min, asset.local_max);
            let mut corners = Vec::with_capacity(8);
            for cx in [lmn[0], lmx[0]] {
                for cy in [lmn[1], lmx[1]] {
                    for cz in [lmn[2], lmx[2]] {
                        let w = rm * (glam::Vec3::new(cx, cy, cz) * s) + pos;
                        let p = match &frame {
                            Some(fr) => fr.to_uv(w),
                            None => glam::Vec2::new(w.x, w.y),
                        };
                        corners.push(Vec2::new(p.x as f64, p.y as f64));
                    }
                }
            }
            let hull = convex_hull(&corners);
            if hull.len() >= 2 {
                out.push((PlanLayer::Furniture, hull));
            }
        }
        out
    }

    /// Draw the 3D model on the 2D canvas — buildings, rooms, furniture, apertures and
    /// architecture — as a tracing and snapping reference. Geometry comes from
    /// [`Self::plan_overlay_shapes`]; this only colours and strokes it.
    pub(super) fn draw_furniture_outlines_2d(&self, painter: &egui::Painter, rect: egui::Rect) {
        let shapes = self.plan_overlay_shapes();
        if shapes.is_empty() {
            return;
        }
        let clip = painter.with_clip_rect(rect);
        for (layer, poly) in shapes {
            // Everything here is posed in WORLD METRES, so it divides by the drawing's unit on the
            // way onto the plan — `w2s_m`, not `w2s`.
            let stroke = egui::Stroke::new(1.0, layer.color());
            for i in 0..poly.len() {
                let a = self.w2s_m(poly[i], rect);
                let b = self.w2s_m(poly[(i + 1) % poly.len()], rect);
                clip.line_segment([a, b], stroke);
            }
        }
    }

    /// Draw the reference raster underlays behind the geometry.
    pub(super) fn draw_raster_underlays(&self, painter: &egui::Painter, rect: egui::Rect) {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for (img, tex) in self.doc.raster_images.iter().zip(&self.underlay_tex) {
            let tl = self.w2s(img.insert, rect); // top-left
            let br = self.w2s(
                Vec2::new(img.insert.x + img.world_w, img.insert.y - img.world_h),
                rect,
            ); // bottom-right
            painter.image(
                tex.id(),
                egui::Rect::from_two_pos(tl, br),
                uv,
                egui::Color32::WHITE,
            );
        }
    }

    /// Render the file browser window. Pure `std::fs` — lists sub-dirs and
    /// .dxf/.rsm files, supports navigation (`..`, click a folder), name
    /// entry, and a format toggle for Save. Confirm routes to do_open /
    /// do_save.
    /// Raster→vector EDITOR window (Slice 4 v1): examine + adjust an imported
    /// scan. Analyzer verdict is advisory; the app never refuses an image.
    pub(super) fn render_raster_editor(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.raster_editor.take() else {
            return;
        };
        let mut open = true;
        let mut do_convert = false;

        // Rebuild the display when an adjustment (dirty) OR a carve / layer
        // visibility change (composite_dirty) happened. The displayed texture is
        // the COMPOSITE: adjusted source with each layer's carved pixels tinted /
        // shown / hidden in place.
        if ed.dirty || ed.composite_dirty {
            let work = ed.working();
            if ed.dirty {
                ed.report = Some(cad_raster::analyze(&work));
            }
            let comp = ed.composite_image(&work);
            let size = [comp.width() as usize, comp.height() as usize];
            let ci = egui::ColorImage::from_rgba_unmultiplied(size, comp.as_raw());
            ed.tex = Some(ctx.load_texture("raster_editor", ci, egui::TextureOptions::LINEAR));
            ed.dirty = false;
            ed.composite_dirty = false;
        }

        egui::Window::new(format!("Raster → Vector — {}", ed.name))
            .id(egui::Id::new("raster_editor"))
            .open(&mut open)
            .resizable(true)
            .default_size(egui::vec2(900.0, 640.0))
            .default_pos(egui::pos2(120.0, 60.0))
            .show(ctx, |ui| {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.horizontal_top(|ui| {
                    // LEFT — adjustments + buffer layers (scroll, fixed width).
                    ui.allocate_ui_with_layout(
                        egui::vec2(268.0, 580.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("rast_left")
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("Adjustments").strong());
                                    let mut changed = false;
                                    changed |=
                                        ui.checkbox(&mut ed.grayscale, "Grayscale").changed();
                                    changed |= ui
                                        .add(
                                            egui::Slider::new(&mut ed.brightness, -128..=128)
                                                .text("brightness"),
                                        )
                                        .changed();
                                    changed |= ui
                                        .add(
                                            egui::Slider::new(&mut ed.contrast, 0.0..=4.0)
                                                .text("contrast"),
                                        )
                                        .changed();
                                    changed |= ui.checkbox(&mut ed.invert, "Invert").changed();
                                    changed |= ui
                                        .checkbox(&mut ed.threshold_on, "Threshold (B/W)")
                                        .changed();
                                    if ed.threshold_on {
                                        changed |= ui
                                            .add(
                                                egui::Slider::new(&mut ed.threshold, 0..=255)
                                                    .text("level"),
                                            )
                                            .changed();
                                    }
                                    if ui.button("Reset").clicked() {
                                        ed.reset();
                                    }
                                    if changed {
                                        ed.dirty = true;
                                    }

                                    ui.add_space(6.0);
                                    if let Some(r) = &ed.report {
                                        ui.small(format!(
                                            "Looks like: {:?} · ~{} colours · edge {:.3}",
                                            r.class, r.approx_colors, r.edge_density
                                        ));
                                    }
                                    ui.add_space(6.0);
                                    ui.separator();

                                    // ---- Buffer layers ----------------------------------
                                    ui.label(egui::RichText::new("Buffer layers").strong());
                                    ui.small(
                                        "Brush-mark line-work → those pixels MOVE into the \
                                  active layer (👁 hide a layer to watch the source \
                                  peel apart; 🎨 toggles the colour tint).",
                                    );
                                    ui.add(
                                        egui::Slider::new(&mut ed.brush, 2.0..=60.0).text("brush"),
                                    );
                                    if ui.button("➕ New buffer layer").clicked() {
                                        ed.add_buffer();
                                    }
                                    ui.add_space(2.0);

                                    let mut del: Option<usize> = None;
                                    let mut recomposite = false;
                                    for i in 0..ed.buffers.len() {
                                        let active = ed.active_buf == Some(i);
                                        ui.horizontal(|ui| {
                                            // marker swatch = active selector.
                                            let (rect, sresp) = ui.allocate_exact_size(
                                                egui::vec2(16.0, 16.0),
                                                egui::Sense::click(),
                                            );
                                            ui.painter().rect_filled(
                                                rect,
                                                2.0,
                                                ed.buffers[i].marker,
                                            );
                                            if active {
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    2.0,
                                                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                                                );
                                            }
                                            if sresp.clicked() {
                                                ed.active_buf = Some(i);
                                            }
                                            let b = &mut ed.buffers[i];
                                            ui.add(
                                                egui::TextEdit::singleline(&mut b.name)
                                                    .desired_width(56.0),
                                            );
                                            if ui.color_edit_button_srgba(&mut b.marker).changed() {
                                                recomposite = true;
                                            }
                                            if ui
                                                .selectable_label(b.visible, "👁")
                                                .on_hover_text("show / hide this layer")
                                                .clicked()
                                            {
                                                b.visible = !b.visible;
                                                recomposite = true;
                                            }
                                            if ui
                                                .selectable_label(b.tint, "🎨")
                                                .on_hover_text("tint carved pixels by marker")
                                                .clicked()
                                            {
                                                b.tint = !b.tint;
                                                recomposite = true;
                                            }
                                            if ui.small_button("✕").clicked() {
                                                del = Some(i);
                                            }
                                        });
                                        let b = &mut ed.buffers[i];
                                        ui.horizontal(|ui| {
                                            egui::ComboBox::from_id_salt(("geom", b.id))
                                                .selected_text(b.geom.label())
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(
                                                        &mut b.geom,
                                                        BufferGeom::Lines,
                                                        "Straight lines",
                                                    );
                                                    ui.selectable_value(
                                                        &mut b.geom,
                                                        BufferGeom::Arcs,
                                                        "Arcs",
                                                    );
                                                    ui.selectable_value(
                                                        &mut b.geom,
                                                        BufferGeom::Nurbs,
                                                        "NURBS",
                                                    );
                                                });
                                            ui.label("ACI");
                                            ui.add(
                                                egui::DragValue::new(&mut b.dobject_aci)
                                                    .update_while_editing(false)
                                                    .range(0..=255),
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("CAD layer");
                                            ui.add(
                                                egui::TextEdit::singleline(&mut b.cad_layer)
                                                    .desired_width(90.0),
                                            );
                                        });
                                        ui.separator();
                                    }
                                    if let Some(i) = del {
                                        ed.buffers.remove(i);
                                        ed.active_buf = match ed.active_buf {
                                            Some(a) if a == i => None,
                                            Some(a) if a > i => Some(a - 1),
                                            other => other,
                                        };
                                        recomposite = true;
                                    }
                                    if recomposite {
                                        ed.composite_dirty = true;
                                    }
                                    if ui
                                        .add(egui::Button::new(
                                            egui::RichText::new("Convert to DObjects ▶").strong(),
                                        ))
                                        .clicked()
                                    {
                                        do_convert = true;
                                    }
                                    ui.small(
                                        "Trace engines (line/arc/NURBS fit) land next slice; \
                                  strokes + settings are captured now.",
                                    );
                                });
                        },
                    );

                    ui.separator();
                    // RIGHT — the composite image; the brush CARVES into the
                    // active layer (the displayed texture already reflects each
                    // layer's tint / visibility, so there are no overlays to draw).
                    let avail = ui.available_size();
                    let (resp, painter) = ui.allocate_painter(avail, egui::Sense::click_and_drag());
                    let pr = resp.rect;
                    painter.rect_filled(pr, 0.0, egui::Color32::from_rgb(16, 20, 26));
                    // Fit the image into the canvas; capture its rect + pixel size.
                    let img_rect = ed.tex.as_ref().map(|tex| {
                        let ts = tex.size_vec2();
                        let s = ((pr.width() - 8.0) / ts.x)
                            .min((pr.height() - 8.0) / ts.y)
                            .max(0.01);
                        let r = egui::Rect::from_center_size(
                            pr.center(),
                            egui::vec2(ts.x * s, ts.y * s),
                        );
                        painter.image(tex.id(), r, uv, egui::Color32::WHITE);
                        (r, ts)
                    });
                    if let Some((r, ts)) = img_rect {
                        if let Some(ai) = ed.active_buf {
                            // CARVE the active layer where the brush drags.
                            if resp.dragged() || resp.clicked() {
                                if let Some(cur) = resp.interact_pointer_pos() {
                                    if r.contains(cur) {
                                        let u = (cur.x - r.min.x) / r.width();
                                        let v = (cur.y - r.min.y) / r.height();
                                        let px = (u * ts.x) as i32;
                                        let py = (v * ts.y) as i32;
                                        let rad = (ed.brush * ts.x / r.width()).max(1.0) as i32;
                                        ed.carve(ai, px, py, rad);
                                    }
                                }
                            }
                            // Brush cursor (marker colour) for feedback.
                            if let Some(cur) = resp.hover_pos().or(resp.interact_pointer_pos()) {
                                if r.contains(cur) {
                                    let col = ed
                                        .buffers
                                        .get(ai)
                                        .map(|b| b.marker)
                                        .unwrap_or(egui::Color32::WHITE);
                                    painter.circle_stroke(
                                        cur,
                                        ed.brush,
                                        egui::Stroke::new(1.5, col),
                                    );
                                }
                            }
                        }
                    }
                });
            });
        // A carve / toggle done this frame needs one more frame to re-composite.
        if ed.composite_dirty {
            ctx.request_repaint();
        }

        if do_convert {
            if !ed.buffers.iter().any(|b| b.marked() > 0) {
                self.history
                    .push("  raster→vector: paint line-work into a buffer layer first".into());
            } else {
                self.snapshot_doc();
                let work = ed.working();
                let img_h = work.height();
                let mut grand = 0usize;
                self.history.push(format!(
                    "  raster→vector: converting {} buffer layer(s) —",
                    ed.buffers.len()
                ));
                for b in &ed.buffers {
                    if b.marked() == 0 {
                        continue;
                    }
                    // Ensure the target CAD layer exists — if the drawing has no
                    // such layer, CREATE it from this buffer layer (name + ACI).
                    let lname = {
                        let t = b.cad_layer.trim();
                        if t.is_empty() {
                            format!("RASTER_{}", b.id)
                        } else {
                            t.to_string()
                        }
                    };
                    let lid = match self.doc.layers.find(&lname) {
                        Some(id) => id,
                        None => {
                            let id = self.doc.layers.add(Layer {
                                name: lname.clone(),
                                color: Color::Aci(b.dobject_aci),
                                ..Layer::layer_zero()
                            });
                            self.history.push(format!(
                                "    + new CAD layer '{}' (ACI {})",
                                lname, b.dobject_aci
                            ));
                            id
                        }
                    };
                    let fit = match b.geom {
                        BufferGeom::Lines => cad_raster::FitKind::Lines,
                        BufferGeom::Arcs => cad_raster::FitKind::Arcs,
                        BufferGeom::Nurbs => cad_raster::FitKind::Nurbs,
                    };
                    let params = cad_raster::TraceParams {
                        img_height: img_h,
                        ..Default::default()
                    };
                    let geoms = cad_raster::trace_layer(&b.mask, &work, fit, &params);
                    // DObjects inherit the layer colour (ByLayer) so the CAD layer
                    // drives their appearance.
                    let style = Style {
                        layer: lid,
                        color: Color::ByLayer,
                        ..Style::default()
                    };
                    let n = geoms.len();
                    for g in geoms {
                        self.doc.push(DObject::with_style(g, style));
                    }
                    grand += n;
                    self.history.push(format!(
                        "    • {} → {} {} on '{}'  ({} px)",
                        b.name,
                        n,
                        b.geom.label(),
                        lname,
                        b.marked()
                    ));
                }
                self.index_dirty = true;
                self.history.push(format!(
                    "  raster→vector: added {} DObject(s) — close this window to see them \
                     (geometry is at ~1 unit/px near the origin).",
                    grand
                ));
            }
        }
        if !open {
            self.history.push("  raster editor closed".into());
            return;
        }
        self.raster_editor = Some(ed);
    }
}
