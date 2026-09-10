use super::*;

// ============ GPU render merge (from dokkandar/Auto_RASM): ported items ============

/// Per-frame cap on how many dobjects the CPU (egui painter) render path
/// will draw. egui tessellates every primitive on the CPU each frame, so a
/// heavy drawing (tens/hundreds of thousands of dobjects in the viewport)
/// makes each frame take seconds — the app looks DEAD and you can't even
/// click to switch modes. Above this budget CPU stops drawing the rest and
/// shows a banner telling the user to switch to GPU/APX. The frame stays
/// bounded, so the UI stays responsive and the mode badges remain clickable.
pub(super) const CPU_DRAW_BUDGET: usize = 20_000;

/// Max GENERATED primitives (segments + circles + fill/hole triangles) the
/// hatch cache builds in ONE frame. A big paste/array of hatches builds over
/// a few frames instead of freezing one; the left-corner note shows how many
/// hatches remain. Budgeting by primitives (not hatch count) bounds per-frame
/// work even when individual hatches are very dense.
pub(super) const HATCH_GEN_WORK_BUDGET: usize = 200_000;

/// Cached, COLOUR-LESS render geometry for ONE hatch (world space). Built by
/// `CadApp::build_hatch_cache_entry`, reused every frame until the doc
/// mutates. Pattern hatches populate `segs`/`circs`; solid hatches populate
/// `solid` — one triangle batch per boundary loop, ORDERED by containment
/// depth ascending (outermost loop first, `true` = even depth = hatch
/// colour, `false` = odd depth = bg over-draw). That depth order is what
/// makes nested even-odd paint correctly: a depth-2 island (e.g. a letter
/// counter inside a punched-out letter) is drawn AFTER the depth-1 hole
/// that contains it, so the hole's bg cannot erase it. Colour is applied at
/// draw time so highlights work.
#[derive(Default, Clone)]
pub(super) struct HatchCacheEntry {
    pub(super) segs: Vec<(Vec2, Vec2)>,
    pub(super) circs: Vec<(Vec2, f64)>,
    /// `(is_fill, triangles)` per boundary loop, depth-ascending.
    pub(super) solid: Vec<(bool, Vec<[Vec2; 3]>)>,
}

pub(super) fn gpu_push_seg(
    lines: &mut Vec<LineInstance>,
    a: Vec2,
    b: Vec2,
    ox: f64,
    oy: f64,
    half_w: f32,
    color: u32,
) {
    lines.push(LineInstance {
        ax: (a.x + ox) as f32,
        ay: (a.y + oy) as f32,
        bx: (b.x + ox) as f32,
        by: (b.y + oy) as f32,
        half_w,
        color,
        flags: 0,
    });
}

pub(super) fn dash_world_segments(pl: &[Vec2], pat: &[f32], out: &mut Vec<(Vec2, Vec2)>) {
    let m = pat.len();
    if pl.len() < 2 || m == 0 {
        return;
    }
    let total: f64 = pat.iter().map(|p| (*p as f64).max(0.0)).sum();
    if total < 1e-9 {
        return;
    }
    let mut pi = 0usize;
    let mut rem = (pat[0] as f64).max(0.0);
    for seg in pl.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let d = b - a;
        let seglen = d.len();
        if seglen < 1e-12 {
            continue;
        }
        let dir = d / seglen;
        let mut pos = 0.0_f64;
        while pos < seglen - 1e-12 {
            // Skip exhausted elements; a zero-length pen-down element = a dot.
            let mut guard = 0;
            while rem <= 1e-12 && guard < 2 * m {
                if pi % 2 == 0 {
                    let p = a + dir * pos;
                    out.push((p, p));
                }
                pi = (pi + 1) % m;
                rem = (pat[pi] as f64).max(0.0);
                guard += 1;
            }
            if rem <= 1e-12 {
                break;
            } // all-zero safety
            let step = rem.min(seglen - pos);
            if pi % 2 == 0 {
                out.push((a + dir * pos, a + dir * (pos + step)));
            }
            pos += step;
            rem -= step;
        }
    }
}

pub(super) fn pack_rgba(c: egui::Color32) -> u32 {
    ((c.r() as u32) << 24) | ((c.g() as u32) << 16) | ((c.b() as u32) << 8) | (c.a() as u32)
}

pub(super) fn point_in_tri(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let d1 = (p.x - b.x) * (a.y - b.y) - (a.x - b.x) * (p.y - b.y);
    let d2 = (p.x - c.x) * (b.y - c.y) - (b.x - c.x) * (p.y - c.y);
    let d3 = (p.x - a.x) * (c.y - a.y) - (c.x - a.x) * (p.y - a.y);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

pub(super) fn ear_clip(poly: &[Vec2]) -> Vec<[Vec2; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    // Signed area → orientation; work on an index ring normalised to CCW.
    let mut area = 0.0;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        area += a.x * b.y - b.x * a.y;
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if area < 0.0 {
        idx.reverse();
    }
    let mut tris: Vec<[Vec2; 3]> = Vec::with_capacity(n.saturating_sub(2));
    let mut guard = 0usize;
    while idx.len() > 3 && guard < n * n + 16 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let i0 = idx[(k + m - 1) % m];
            let i1 = idx[k];
            let i2 = idx[(k + 1) % m];
            let (a, b, c) = (poly[i0], poly[i1], poly[i2]);
            // Convex corner? (CCW cross > 0). Reflex corners can't be ears.
            let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if cross <= 0.0 {
                continue;
            }
            // Ear only if no OTHER vertex lies inside triangle a-b-c.
            let mut is_ear = true;
            for &j in &idx {
                if j == i0 || j == i1 || j == i2 {
                    continue;
                }
                if point_in_tri(poly[j], a, b, c) {
                    is_ear = false;
                    break;
                }
            }
            if is_ear {
                tris.push([a, b, c]);
                idx.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        } // self-intersecting → stop (rare)
    }
    if idx.len() == 3 {
        tris.push([poly[idx[0]], poly[idx[1]], poly[idx[2]]]);
    }
    tris
}

/// WORLD-space wall faces (left, right), each a LIST of pieces. World-space
/// analog of `wall_face_screen_pts` (uses `cad_wall::solve_faces`), so GPU
/// walls render identically to the CPU path. X-junction face splitting is a
/// separate ext feature not ported here.
pub(super) fn wall_face_world_pts(
    app: &CadApp,
    w: &cad_kernel::Wall,
) -> (Vec<Vec<Vec2>>, Vec<Vec<Vec2>>) {
    if w.is_curved() {
        let n = match cad_kernel::bulge_arc(w.start, w.end, w.bulge) {
            Some((_c, r, _a0, sweep)) => {
                let arc_px = ((r + w.thickness * 0.5) * sweep.abs()) as f32 * app.scale;
                (arc_px * 0.25).clamp(12.0, 256.0) as usize
            }
            None => 28,
        };
        match w.face_polylines(n) {
            Some((l, r)) => (vec![l], vec![r]),
            None => (Vec::new(), Vec::new()),
        }
    } else {
        let walls: Vec<cad_kernel::Wall> = app
            .doc
            .dobjects
            .iter()
            .filter_map(|d| {
                if let Geom::Wall(x) = &d.geom {
                    Some(x.clone())
                } else {
                    None
                }
            })
            .collect();
        match cad_wall::solve_faces(w, &walls) {
            Some(f) => (
                vec![vec![f.left.0, f.left.1]],
                vec![vec![f.right.0, f.right.1]],
            ),
            None => match (w.left_line(), w.right_line()) {
                (Some(l), Some(r)) => (vec![vec![l.a, l.b]], vec![vec![r.a, r.b]]),
                _ => (Vec::new(), Vec::new()),
            },
        }
    }
}

impl CadApp {
    /// Floating Recorder window. Controls + live counters + buttons
    /// for Note / Snap-now / Clear / Copy. Inspector / replay UI ships
    /// in Slice C.
    /// Switch the render mode directly (CPU / GPU / APX are mutually
    /// exclusive). No-op if already in that mode. Marks the GPU batch dirty
    /// so the next frame rebuilds for the new path.
    pub(super) fn set_render_mode(&mut self, m: RenderMode) {
        if self.render_mode == m {
            return;
        }
        self.render_mode = m;
        self.touch_view();
        self.history.push(format!("  render mode → {:?}", m));
    }

    /// If the dobject is a STROKED geom with a non-continuous linetype
    /// (its own, else ByLayer), return the WORLD-unit dash pattern (already
    /// multiplied by the per-dobject linetype scale) so the GPU path can walk
    /// dashes. `None` = solid (continuous, or a non-stroked geom). Mirrors the
    /// resolution in `paint_dobject_with_style`.
    pub(super) fn effective_dash_pattern(&self, e: &DObject) -> Option<Vec<f32>> {
        use cad_kernel::LinetypeTable;
        if !matches!(
            e.geom,
            Geom::Line(_)
                | Geom::Polyline(_)
                | Geom::Circle(_)
                | Geom::Arc(_)
                | Geom::Ellipse(_)
                | Geom::EllipseArc(_)
                | Geom::Spline(_)
        ) {
            return None;
        }
        let lt_id = if e.style.linetype == LinetypeTable::CONTINUOUS {
            self.doc
                .layers
                .get(e.style.layer)
                .map(|l| l.linetype)
                .unwrap_or(LinetypeTable::CONTINUOUS)
        } else {
            e.style.linetype
        };
        let lt = self.doc.linetypes.get(lt_id)?;
        if lt.is_continuous() {
            return None;
        }
        let lt_scale = if e.style.linetype_scale > 1e-6 {
            e.style.linetype_scale
        } else {
            1.0
        };
        Some(lt.pattern.iter().map(|p| *p * lt_scale).collect())
    }

    /// Generate the (expensive) COLOUR-LESS render geometry for the hatch at
    /// dobject index `i`. World space. Pattern → clipped lines/circles; solid
    /// → ear-clipped fill/hole triangles per boundary loop. Non-hatch or a
    /// missing index yields an empty entry. Cached by `hatch_cache`.
    pub(super) fn build_hatch_cache_entry(&self, i: usize) -> HatchCacheEntry {
        let mut out = HatchCacheEntry::default();
        let Some(d) = self.doc.dobjects.get(i) else {
            return out;
        };
        let Geom::Hatch(h) = &d.geom else {
            return out;
        };
        match &h.pattern {
            cad_kernel::HatchPattern::Pattern { .. } => {
                let (segs, circs) = self.hatch_pattern_world_geometry(h);
                out.segs = segs;
                out.circs = circs;
            }
            cad_kernel::HatchPattern::Solid => {
                let loops = self.resolve_hatch_loops(h);
                // Depth-ascending per-loop batches — consumers draw them in
                // this order so nested islands re-fill their parent holes
                // (see HatchCacheEntry::solid).
                let depths = loop_depths(&loops);
                let mut order: Vec<usize> = (0..loops.len()).collect();
                order.sort_by_cached_key(|&li| depths[li]);
                for li in order {
                    let tris = ear_clip(&loops[li]);
                    if !tris.is_empty() {
                        out.solid.push((depths[li] % 2 == 0, tris));
                    }
                }
            }
        }
        out
    }

    /// GPU-mode helper: resolve a hatch's PATTERN geometry (loops + named
    /// pattern) to world-space segments + circles. Empty for Solid hatches
    /// (those stay on the egui fill path). Mirrors the loop resolution the
    /// egui `render_hatch_fill` does for patterns.
    pub(super) fn hatch_pattern_world_geometry(
        &self,
        h: &cad_kernel::Hatch,
    ) -> (Vec<(Vec2, Vec2)>, Vec<(Vec2, f64)>) {
        if let cad_kernel::HatchPattern::Pattern {
            name,
            scale,
            angle_deg,
        } = &h.pattern
        {
            let loops = self.resolve_hatch_loops(h);
            if loops.is_empty() {
                return (Vec::new(), Vec::new());
            }
            self.hatch_pattern_geometry(&loops, name, *scale, *angle_deg)
        } else {
            (Vec::new(), Vec::new())
        }
    }

    /// Resolve a named PATTERN hatch to WORLD-space line segments +
    /// (centre, world_radius) circles. Single source of truth for pattern
    /// geometry: the egui painter path (CPU mode) and the GPU line/circle
    /// pipelines (GPU mode) both consume it, so they can never drift.
    /// Solid fills are NOT handled here (see `render_hatch_solid`). Returns
    /// empties for an unknown pattern name or a degenerate bbox.
    fn hatch_pattern_geometry(
        &self,
        loops: &[Vec<Vec2>],
        name: &str,
        user_scale: f64,
        user_angle_deg: f64,
    ) -> (Vec<(Vec2, Vec2)>, Vec<(Vec2, f64)>) {
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        let mut circs: Vec<(Vec2, f64)> = Vec::new();
        let pat = cad_kernel::patterns::lookup(name);
        if pat.is_empty() {
            return (segs, circs);
        }
        // Union bbox of all loops in world coords.
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for l in loops {
            for v in l {
                if v.x < min.x {
                    min.x = v.x;
                }
                if v.y < min.y {
                    min.y = v.y;
                }
                if v.x > max.x {
                    max.x = v.x;
                }
                if v.y > max.y {
                    max.y = v.y;
                }
            }
        }
        if !min.x.is_finite() || !max.x.is_finite() {
            return (segs, circs);
        }
        let user_angle = user_angle_deg.to_radians();
        let families = match &pat {
            cad_kernel::patterns::Pattern::Families(fs) => fs.as_slice(),
            cad_kernel::patterns::Pattern::Tile {
                period_x,
                period_y,
                segments,
                circles,
            } => {
                self.hatch_tile_geometry(
                    loops, *period_x, *period_y, segments, circles, user_scale, user_angle, min,
                    max, &mut segs, &mut circs,
                );
                return (segs, circs);
            }
        };
        for fam in families {
            // Effective angle + spacing after user transform.
            let theta = fam.angle + user_angle;
            let spacing = fam.spacing * user_scale.abs().max(1e-9);
            // Line direction u, normal n (CCW perp of u).
            let cos = theta.cos();
            let sin = theta.sin();
            let u = Vec2::new(cos, sin);
            let n = Vec2::new(-sin, cos);
            // Project bbox corners onto n to find the range of
            // s-values (offset along n) that the pattern needs to
            // cover. The bbox of a rotated axis-aligned rect is
            // bounded by the projection of its 4 corners.
            let corners = [
                Vec2::new(min.x, min.y),
                Vec2::new(max.x, min.y),
                Vec2::new(min.x, max.y),
                Vec2::new(max.x, max.y),
            ];
            let base = Vec2::new(fam.base_x, fam.base_y);
            let mut s_min = f64::INFINITY;
            let mut s_max = f64::NEG_INFINITY;
            for c in &corners {
                let s = (*c - base).dot(n);
                if s < s_min {
                    s_min = s;
                }
                if s > s_max {
                    s_max = s;
                }
            }
            // First line at s = ceil(s_min / spacing) * spacing.
            let mut s = (s_min / spacing).ceil() * spacing;
            // Safety cap: spacing too small for the world bbox would
            // generate millions of lines and freeze. Bail out if the
            // family would produce > 10 000 lines for this hatch.
            let line_count_estimate = ((s_max - s_min) / spacing).ceil();
            if line_count_estimate > 10_000.0 {
                continue;
            }
            while s <= s_max + 1e-9 {
                let line_origin = base + n * s;
                // Clip this infinite line against the loops via the shared
                // kernel even-odd clipper — per-loop pairing + XOR across
                // loops handles shared edges (a chord hatched as two
                // half-discs) and nested islands correctly.
                let intervals = cad_kernel::patterns::hatch_line_intervals(loops, line_origin, u);
                for (t0, t1) in intervals {
                    let p0 = line_origin + u * t0;
                    let p1 = line_origin + u * t1;
                    segs.push((p0, p1));
                }
                s += spacing;
            }
        }
        (segs, circs)
    }

    /// Tile-pattern GEOMETRY — tiles a finite-segment cell across the
    /// boundary bbox and clips each segment against the loops using the
    /// same infinite-line + even-odd machinery as families, then clamps
    /// the resulting visible intervals to the segment's own length so
    /// only its in-cell portion is emitted. Appends world-space segments
    /// to `segs` and (centre, world_radius) circles to `circs` — the egui
    /// painter path and the GPU pipelines both consume the same output.
    ///
    /// `user_scale` multiplies the period AND the segment coords; the
    /// user_angle rotates the whole pattern about the origin.
    #[allow(clippy::too_many_arguments)]
    fn hatch_tile_geometry(
        &self,
        loops: &[Vec<Vec2>],
        period_x: f64,
        period_y: f64,
        segments: &[cad_kernel::patterns::PatternSegment],
        circles: &[cad_kernel::patterns::PatternCircle],
        user_scale: f64,
        user_angle: f64,
        min: Vec2,
        max: Vec2,
        segs: &mut Vec<(Vec2, Vec2)>,
        circs: &mut Vec<(Vec2, f64)>,
    ) {
        let s = user_scale.abs().max(1e-9);
        let px = period_x * s;
        let py = period_y * s;
        if px < 1e-9 || py < 1e-9 {
            return;
        }
        let cos = user_angle.cos();
        let sin = user_angle.sin();
        // Cells in the AXIS-ALIGNED pattern frame that need rendering.
        // After user_angle rotation the cell axes no longer align with
        // the world bbox — invert the rotation on each world-bbox corner
        // to find the bbox in pattern frame, then iterate cells.
        let corners_world = [
            Vec2::new(min.x, min.y),
            Vec2::new(max.x, min.y),
            Vec2::new(max.x, max.y),
            Vec2::new(min.x, max.y),
        ];
        let mut pmin = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut pmax = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for c in &corners_world {
            // inverse rotation: (cos, sin) → (cos, -sin)
            let px_w = c.x * cos + c.y * sin;
            let py_w = -c.x * sin + c.y * cos;
            if px_w < pmin.x {
                pmin.x = px_w;
            }
            if py_w < pmin.y {
                pmin.y = py_w;
            }
            if px_w > pmax.x {
                pmax.x = px_w;
            }
            if py_w > pmax.y {
                pmax.y = py_w;
            }
        }
        let i0 = (pmin.x / px).floor() as i64 - 1;
        let i1 = (pmax.x / px).ceil() as i64 + 1;
        let j0 = (pmin.y / py).floor() as i64 - 1;
        let j1 = (pmax.y / py).ceil() as i64 + 1;
        // Safety cap — millions of cells would freeze the UI.
        let tile_count = (i1 - i0).max(0) * (j1 - j0).max(0);
        if tile_count > 200_000 {
            return;
        }
        for j in j0..=j1 {
            for i in i0..=i1 {
                let ox = (i as f64) * px;
                let oy = (j as f64) * py;
                for seg in segments {
                    // Segment endpoints in PATTERN frame (with user scale).
                    let ax_p = ox + seg.x1 * s;
                    let ay_p = oy + seg.y1 * s;
                    let bx_p = ox + seg.x2 * s;
                    let by_p = oy + seg.y2 * s;
                    // Rotate to WORLD frame by user_angle.
                    let ax = ax_p * cos - ay_p * sin;
                    let ay = ax_p * sin + ay_p * cos;
                    let bx = bx_p * cos - by_p * sin;
                    let by = bx_p * sin + by_p * cos;
                    let a = Vec2::new(ax, ay);
                    let b = Vec2::new(bx, by);
                    let dvec = b - a;
                    let seg_len2 = dvec.x * dvec.x + dvec.y * dvec.y;
                    if seg_len2 < 1e-18 {
                        continue;
                    }
                    // Clip against the loops with the same per-loop + XOR
                    // machinery as the family lines (kernel
                    // hatch_line_intervals), then clamp the resulting
                    // intervals to the segment's own t-range [0, 1]. The
                    // XOR runs over the infinite line, so a segment
                    // starting/ending INSIDE a loop is handled by intervals
                    // that span past the clamp window.
                    let intervals = cad_kernel::patterns::hatch_line_intervals(&loops, a, dvec);
                    for (t0, t1) in intervals {
                        let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
                        if t1 - t0 > 1e-6 {
                            let p0 = a + dvec * t0;
                            let p1 = a + dvec * t1;
                            segs.push((p0, p1));
                        }
                    }
                }
                // Circles in the cell — paint each, but only when its
                // CENTRE lies inside the resolved hatch boundary loops
                // (even-odd). v1 simplification: a circle is "in or out"
                // as a whole. Boundary-intersecting circles render with
                // their full ring beyond the boundary; refine later by
                // arc-clipping when a circle straddles the boundary.
                for c in circles {
                    let cx_p = ox + c.cx * s;
                    let cy_p = oy + c.cy * s;
                    let cx = cx_p * cos - cy_p * sin;
                    let cy = cx_p * sin + cy_p * cos;
                    let centre = Vec2::new(cx, cy);
                    let r_world = c.radius * s;
                    if r_world < 1e-9 {
                        continue;
                    }
                    let inside = loops.iter().fold(false, |acc, l| {
                        acc ^ point_in_polygon(centre, l.iter().copied())
                    });
                    if !inside {
                        continue;
                    }
                    circs.push((centre, r_world));
                }
            }
        }
    }
}
