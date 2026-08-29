// Uniform-grid spatial index.
//
// Every dobject is bucketed into all cells its bounding box overlaps.
// Queries take a rectangle (or a point + radius) and return a deduplicated
// list of candidate dobject indices — typically O(visible cells × avg per cell)
// instead of O(N).
//
// Bucketing decision: a fixed cell size, chosen by `auto_cell_size` to target
// ~10 dobjects per cell on average. Memory cost is O(N · avg cells per dobject).
//
// Trade-off: this index assumes a roughly uniform dobject distribution. For
// pathological cases (e.g. one giant dobject covering the whole drawing plus a
// million tiny ones) a quadtree would do better. Uniform grid is fast to build,
// trivially parallel, and matches the array-grid workload we want to stress.

use crate::dobject::DObject;
use crate::math::Vec2;

/// Marks a dobject that is not bucketed (view-independent bbox).
const SKIP: [u32; 4] = [u32::MAX, 0, 0, 0];

/// The cell range a bbox occupies, clamped to the grid.
fn cell_range(bb: &(Vec2, Vec2), origin: Vec2, cs: f64, cols: usize, rows: usize) -> [u32; 4] {
    let (emin, emax) = *bb;
    let xmin = (((emin.x - origin.x) / cs).floor() as isize).max(0) as usize;
    let xmax = ((((emax.x - origin.x) / cs).floor() as isize).max(0) as usize).min(cols - 1);
    let ymin = (((emin.y - origin.y) / cs).floor() as isize).max(0) as usize;
    let ymax = ((((emax.y - origin.y) / cs).floor() as isize).max(0) as usize).min(rows - 1);
    [xmin as u32, xmax as u32, ymin as u32, ymax as u32]
}

/// Does this bbox lie INSIDE the grid? A clamped range would silently mis-bucket a
/// dobject that has moved out of bounds, so `update` must reject it and rebuild.
fn fits(emin: Vec2, emax: Vec2, origin: Vec2, cs: f64, cols: usize, rows: usize) -> bool {
    emin.x >= origin.x
        && emin.y >= origin.y
        && emax.x < origin.x + cols as f64 * cs
        && emax.y < origin.y + rows as f64 * cs
}

pub struct UniformGrid {
    pub cell_size: f64,
    pub origin:    Vec2,   // world coord of the grid's (0,0) corner
    pub cols:      usize,
    pub rows:      usize,
    cells: Vec<Vec<u32>>,  // row-major, cells[row * cols + col]
    /// Per-dobject cell range `[xmin, xmax, ymin, ymax]` — the cells it currently
    /// occupies. Exists so [`Self::update`] can REMOVE a changed dobject from its old
    /// cells without scanning the grid; a moved dobject's old position is otherwise
    /// unrecoverable (the geometry has already changed by the time we're told).
    ///
    /// `[u32::MAX, ..]` marks a view-independent dobject (never bucketed).
    /// Costs 16 B/dobject (~24 MB at 1.5M) — 8% on top of a ~300 MB document, and it
    /// buys O(changed) edits instead of O(n): moving 3,150 of 1.5M dobjects
    /// re-buckets 3,150, not 1,500,000.
    ranges: Vec<[u32; 4]>,
    /// Indices of dobjects whose `Geom::is_view_independent_bbox()`
    /// returned true (Hatch today). They're NOT bucketed into cells
    /// because their bbox is a degenerate placeholder; instead they
    /// get appended to every query result so the render path always
    /// has a chance to draw them.
    view_independent: Vec<u32>,
    /// Total dobjects this index was built from. Used by `query_bbox`
    /// to size the visited bitset for O(1) dedup (vs HashSet, which
    /// becomes pathological when entities cover many cells and the
    /// query hits many cells — at 580 cells/entity, a full-grid query
    /// performed 100M HashSet inserts and tanked FPS to ~2).
    n_entities: usize,
}

impl UniformGrid {
    /// World bounds of the whole grid (origin → origin + cols·cell_size).
    /// `None` for an empty grid (cols or rows == 0).
    pub fn world_bounds(&self) -> Option<(Vec2, Vec2)> {
        if self.cols == 0 || self.rows == 0 { return None; }
        Some((
            self.origin,
            Vec2::new(self.origin.x + self.cols as f64 * self.cell_size,
                      self.origin.y + self.rows as f64 * self.cell_size),
        ))
    }

    pub fn empty() -> Self {
        Self {
            cell_size: 1.0,
            origin:    Vec2::ZERO,
            cols: 0, rows: 0,
            cells: Vec::new(),
            ranges: Vec::new(),
            view_independent: Vec::new(),
            n_entities: 0,
        }
    }

    /// Build the index from `dobjects`. Panics if `cell_size <= 0`.
    pub fn build(dobjects: &[DObject], cell_size: f64) -> Self {
        assert!(cell_size > 0.0, "cell_size must be positive");
        if dobjects.is_empty() { return Self::empty(); }

        // overall world bbox — only over dobjects whose bbox is
        // view-meaningful. View-independent bboxes (e.g. Hatch's
        // placeholder (0,0)) would otherwise distort the grid origin
        // and force absurd cell counts.
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut view_independent: Vec<u32> = Vec::new();
        for (i, e) in dobjects.iter().enumerate() {
            if e.geom.is_view_independent_bbox() {
                view_independent.push(i as u32);
                continue;
            }
            let (emin, emax) = e.bbox();
            if emin.x < min.x { min.x = emin.x; }
            if emin.y < min.y { min.y = emin.y; }
            if emax.x > max.x { max.x = emax.x; }
            if emax.y > max.y { max.y = emax.y; }
        }
        // If EVERY dobject is view-independent, there's no spatial grid
        // to build — return an empty grid that still carries the
        // global list, so query_bbox keeps returning the hatches.
        if !min.x.is_finite() {
            return Self {
                cell_size, origin: Vec2::ZERO, cols: 0, rows: 0,
                cells: Vec::new(),
                ranges: vec![SKIP; dobjects.len()],
                view_independent,
                n_entities: dobjects.len(),
            };
        }

        let cols = (((max.x - min.x) / cell_size).floor() as isize + 1).max(1) as usize;
        let rows = (((max.y - min.y) / cell_size).floor() as isize + 1).max(1) as usize;
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); cols * rows];
        let mut ranges: Vec<[u32; 4]> = Vec::with_capacity(dobjects.len());

        for (i, e) in dobjects.iter().enumerate() {
            if e.geom.is_view_independent_bbox() {
                ranges.push(SKIP);
                continue;   // already in view_independent
            }
            let r = cell_range(&e.bbox(), min, cell_size, cols, rows);
            ranges.push(r);
            for cy in r[2]..=r[3] {
                let row_start = cy as usize * cols;
                for cx in r[0]..=r[1] {
                    cells[row_start + cx as usize].push(i as u32);
                }
            }
        }

        Self { cell_size, origin: min, cols, rows, cells, ranges, view_independent,
               n_entities: dobjects.len() }
    }

    /// [`Self::build`] with a resolved-bbox resolver: a dobject whose
    /// `is_view_independent_bbox()` is true is bucketed by the resolver's
    /// answer instead of being skipped (the same contract the original repo
    /// uses for Hatch fills whose bbox is a degenerate placeholder).
    pub fn build_with(
        dobjects: &[DObject],
        cell_size: f64,
        bbox_of: &dyn Fn(&DObject) -> Option<(Vec2, Vec2)>,
    ) -> Self {
        assert!(cell_size > 0.0, "cell_size must be positive");
        if dobjects.is_empty() { return Self::empty(); }

        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut view_independent: Vec<u32> = Vec::new();
        let mut boxes: Vec<Option<(Vec2, Vec2)>> = Vec::with_capacity(dobjects.len());
        for (i, e) in dobjects.iter().enumerate() {
            let b = if e.geom.is_view_independent_bbox() {
                bbox_of(e)
            } else {
                Some(e.bbox())
            };
            match b {
                Some((emin, emax)) if emax.x >= emin.x && emax.y >= emin.y => {
                    if emin.x < min.x { min.x = emin.x; }
                    if emin.y < min.y { min.y = emin.y; }
                    if emax.x > max.x { max.x = emax.x; }
                    if emax.y > max.y { max.y = emax.y; }
                    boxes.push(Some((emin, emax)));
                }
                _ => {
                    view_independent.push(i as u32);
                    boxes.push(None);
                }
            }
        }
        if !min.x.is_finite() {
            return Self {
                cell_size, origin: Vec2::ZERO, cols: 0, rows: 0,
                cells: Vec::new(),
                ranges: vec![SKIP; dobjects.len()],
                view_independent,
                n_entities: dobjects.len(),
            };
        }

        let cols = (((max.x - min.x) / cell_size).floor() as isize + 1).max(1) as usize;
        let rows = (((max.y - min.y) / cell_size).floor() as isize + 1).max(1) as usize;
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); cols * rows];
        let mut ranges: Vec<[u32; 4]> = Vec::with_capacity(dobjects.len());

        for (i, b) in boxes.iter().enumerate() {
            let Some((emin, emax)) = b else {
                ranges.push(SKIP);
                continue;   // already in view_independent
            };
            let r = cell_range(&(*emin, *emax), min, cell_size, cols, rows);
            ranges.push(r);
            for cy in r[2]..=r[3] {
                let row_start = cy as usize * cols;
                for cx in r[0]..=r[1] {
                    cells[row_start + cx as usize].push(i as u32);
                }
            }
        }

        Self { cell_size, origin: min, cols, rows, cells, ranges, view_independent,
               n_entities: dobjects.len() }
    }

    /// Absorb dobjects APPENDED to the end of the document — the shape a DRAW makes. **O(added)**.
    ///
    /// [`Self::update`] cannot do this: it refuses any change in COUNT, because an insert or a
    /// delete in the MIDDLE shifts every index after it and invalidates the whole bucket space.
    /// An APPEND shifts nothing — every existing index still means what it meant — so the grid
    /// only has to learn about the objects on the end. Drawing one line into a 1.5 M-object plan
    /// stops re-bucketing 1.5 M objects to record the arrival of one.
    ///
    /// Refuses (and the caller must then rebuild) when:
    ///   * the count did not GROW — this is for appends, not deletes,
    ///   * the grid has no cells: an all-view-independent document has no spatial structure to
    ///     add to, and a rebuild will make one once there is something worth bucketing, or
    ///   * a new bbox falls OUTSIDE the current grid, which would need it to GROW — `origin`,
    ///     `cols` and `rows` are fixed at build time. That is the ordinary case for the first
    ///     object drawn well clear of everything else, and rebuilding is the right answer.
    ///
    /// VERIFIES EVERYTHING BEFORE MUTATING ANYTHING, exactly as `update` does. A refused append
    /// must leave the grid untouched rather than half-populated: the caller's fallback is to
    /// rebuild, and until it does, the grid it still holds gets queried.
    pub fn insert_appended(&mut self, dobjects: &[DObject]) -> bool {
        let old = self.ranges.len();
        if dobjects.len() <= old || self.cells.is_empty() {
            return false;
        }
        // Pass 1 — decide every new object's fate without touching the grid.
        let mut planned: Vec<[u32; 4]> = Vec::with_capacity(dobjects.len() - old);
        for e in &dobjects[old..] {
            if e.geom.is_view_independent_bbox() {
                planned.push(SKIP); // joins `view_independent`; never bucketed
                continue;
            }
            let (emin, emax) = e.bbox();
            if !fits(emin, emax, self.origin, self.cell_size, self.cols, self.rows) {
                return false; // outside the grid → it would have to grow → rebuild
            }
            planned.push(cell_range(&(emin, emax), self.origin, self.cell_size, self.cols, self.rows));
        }
        // Pass 2 — commit.
        for (k, r) in planned.into_iter().enumerate() {
            let i = (old + k) as u32;
            self.ranges.push(r);
            if r == SKIP {
                self.view_independent.push(i);
                continue;
            }
            for cy in r[2]..=r[3] {
                let row = cy as usize * self.cols;
                for cx in r[0]..=r[1] {
                    self.cells[row + cx as usize].push(i);
                }
            }
        }
        self.n_entities = dobjects.len();
        true
    }

    /// Incrementally re-bucket just the `changed` dobjects. **O(changed)**, not O(n).
    ///
    /// Returns `false` when the grid cannot absorb the change — the caller must then
    /// do a full [`Self::build`]. That happens when:
    ///   * the dobject COUNT changed (add/delete shifts every index — the whole
    ///     bucket space is invalidated), or
    ///   * a new bbox falls OUTSIDE the current grid (the grid would have to grow;
    ///     `origin`/`cols`/`rows` are fixed at build time), or
    ///   * a dobject changed to/from view-independent.
    ///
    /// Falling back is always CORRECT, just slow — so a caller can call this
    /// unconditionally and rebuild on `false`.
    pub fn update(&mut self, dobjects: &[DObject], changed: &[usize]) -> bool {
        // Count change ⇒ indices shifted ⇒ every stored range is suspect.
        if dobjects.len() != self.ranges.len() || self.cells.is_empty() {
            return false;
        }
        // Pass 1 — verify EVERY changed dobject fits before mutating anything, so a
        // rejected update leaves the grid untouched (no half-applied state).
        let mut new_ranges: Vec<(usize, [u32; 4])> = Vec::with_capacity(changed.len());
        for &i in changed {
            let e = match dobjects.get(i) { Some(e) => e, None => return false };
            if e.geom.is_view_independent_bbox() {
                return false; // changed KIND — rebuild handles the bookkeeping
            }
            if self.ranges[i] == SKIP {
                return false; // was view-independent, now isn't
            }
            let (emin, emax) = e.bbox();
            if !fits(emin, emax, self.origin, self.cell_size, self.cols, self.rows) {
                return false; // outside the grid → must grow → rebuild
            }
            new_ranges.push((i, cell_range(&(emin, emax), self.origin, self.cell_size, self.cols, self.rows)));
        }
        // Pass 2 — remove from old cells, insert into new.
        for (i, new) in new_ranges {
            let old = self.ranges[i];
            if old == new {
                continue; // same cells → nothing to do (a small move usually lands here)
            }
            let id = i as u32;
            for cy in old[2]..=old[3] {
                let row = cy as usize * self.cols;
                for cx in old[0]..=old[1] {
                    let c = &mut self.cells[row + cx as usize];
                    if let Some(p) = c.iter().position(|&x| x == id) {
                        c.swap_remove(p); // order within a cell is irrelevant
                    }
                }
            }
            for cy in new[2]..=new[3] {
                let row = cy as usize * self.cols;
                for cx in new[0]..=new[1] {
                    self.cells[row + cx as usize].push(id);
                }
            }
            self.ranges[i] = new;
        }
        true
    }

    /// Build the index computing every `bbox()` **exactly ONCE**.
    ///
    /// The `auto_cell_size` + `build` pair sweeps `bbox()` THREE times (cell-size pass,
    /// world-bbox pass, bucketing pass). On real geometry that dominates: measured on a
    /// 1.5M drawing of lines/circles/arcs/ellipses/polylines, one sweep is 106 ms and
    /// the rebuild is 409 ms — **78% of it is bbox()**, because bbox() is O(verts) for
    /// a polyline and trigonometric for an arc/ellipse. Flat-line benchmarks hide this.
    ///
    /// Caches the bboxes (32 B/dobject, freed when this returns) and derives the cell
    /// size from the same pass. Same result as `build(d, auto_cell_size(d, t))`, which
    /// `index_rebuild_matches_build_auto` asserts.
    pub fn build_auto(dobjects: &[DObject], target_per_cell: f64) -> Self {
        if dobjects.is_empty() { return Self::empty(); }

        // ---- the ONE bbox sweep -------------------------------------------
        let mut bbs: Vec<(Vec2, Vec2)> = Vec::with_capacity(dobjects.len());
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut view_independent: Vec<u32> = Vec::new();
        let mut count_real = 0usize;
        for (i, e) in dobjects.iter().enumerate() {
            if e.geom.is_view_independent_bbox() {
                view_independent.push(i as u32);
                bbs.push((Vec2::ZERO, Vec2::ZERO));
                continue;
            }
            let bb = e.bbox();
            count_real += 1;
            if bb.0.x < min.x { min.x = bb.0.x; }
            if bb.0.y < min.y { min.y = bb.0.y; }
            if bb.1.x > max.x { max.x = bb.1.x; }
            if bb.1.y > max.y { max.y = bb.1.y; }
            bbs.push(bb);
        }
        if count_real == 0 || !min.x.is_finite() {
            return Self {
                cell_size: 1.0, origin: Vec2::ZERO, cols: 0, rows: 0,
                cells: Vec::new(), ranges: vec![SKIP; dobjects.len()],
                view_independent, n_entities: dobjects.len(),
            };
        }

        // ---- cell size, from the SAME pass (no second sweep) --------------
        let w = (max.x - min.x).max(1.0);
        let h = (max.y - min.y).max(1.0);
        let cell_size = (w * h * target_per_cell / count_real as f64)
            .sqrt().max(0.001).min(1.0e6);

        // ---- bucket, reusing the cached bboxes (no third sweep) ----------
        let cols = (((max.x - min.x) / cell_size).floor() as isize + 1).max(1) as usize;
        let rows = (((max.y - min.y) / cell_size).floor() as isize + 1).max(1) as usize;
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); cols * rows];
        let mut ranges: Vec<[u32; 4]> = Vec::with_capacity(dobjects.len());
        for (i, e) in dobjects.iter().enumerate() {
            if e.geom.is_view_independent_bbox() {
                ranges.push(SKIP);
                continue;
            }
            let r = cell_range(&bbs[i], min, cell_size, cols, rows);
            ranges.push(r);
            for cy in r[2]..=r[3] {
                let row = cy as usize * cols;
                for cx in r[0]..=r[1] {
                    cells[row + cx as usize].push(i as u32);
                }
            }
        }
        Self { cell_size, origin: min, cols, rows, cells, ranges, view_independent,
               n_entities: dobjects.len() }
    }

    /// Pick a cell size that targets `target_per_cell` dobjects per cell, on
    /// average, given the dobjects' overall bbox area.
    pub fn auto_cell_size(dobjects: &[DObject], target_per_cell: f64) -> f64 {
        if dobjects.is_empty() { return 1.0; }
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut count_real: usize = 0;
        for e in dobjects {
            if e.geom.is_view_independent_bbox() { continue; }
            count_real += 1;
            let (emin, emax) = e.bbox();
            if emin.x < min.x { min.x = emin.x; }
            if emin.y < min.y { min.y = emin.y; }
            if emax.x > max.x { max.x = emax.x; }
            if emax.y > max.y { max.y = emax.y; }
        }
        if count_real == 0 { return 1.0; }
        let w = (max.x - min.x).max(1.0);
        let h = (max.y - min.y).max(1.0);
        let by_density = (w * h * target_per_cell / count_real as f64).sqrt();
        by_density.max(0.001).min(1.0e6)
    }

    /// [`Self::auto_cell_size`] with the resolved-bbox resolver — a dobject
    /// whose `is_view_independent_bbox()` is true is measured by the resolver
    /// instead of skipped (same contract as [`Self::build_with`]).
    pub fn auto_cell_size_with(
        dobjects: &[DObject],
        target_per_cell: f64,
        bbox_of: &dyn Fn(&DObject) -> Option<(Vec2, Vec2)>,
    ) -> f64 {
        if dobjects.is_empty() { return 1.0; }
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut count_real: usize = 0;
        for e in dobjects {
            let b = if e.geom.is_view_independent_bbox() {
                bbox_of(e)
            } else {
                Some(e.bbox())
            };
            let Some((emin, emax)) = b else { continue; };
            if emax.x < emin.x || emax.y < emin.y { continue; }
            count_real += 1;
            if emin.x < min.x { min.x = emin.x; }
            if emin.y < min.y { min.y = emin.y; }
            if emax.x > max.x { max.x = emax.x; }
            if emax.y > max.y { max.y = emax.y; }
        }
        if count_real == 0 { return 1.0; }
        let w = (max.x - min.x).max(1.0);
        let h = (max.y - min.y).max(1.0);
        let by_density = (w * h * target_per_cell / count_real as f64).sqrt();
        by_density.max(0.001).min(1.0e6)
    }

    /// Candidate dobject indices whose stored cells overlap the rectangle.
    /// Deduplicated. DObjects were bucketed by bbox so callers may still need
    /// a tighter test for exact-overlap semantics.
    pub fn query_bbox(&self, q_min: Vec2, q_max: Vec2) -> Vec<u32> {
        // Even an empty cell grid must still return the view-independent
        // dobjects (Hatch etc.) — their bbox is a placeholder, so a
        // bbox-overlap test would always miss them.
        if self.cells.is_empty() {
            return self.view_independent.clone();
        }
        let cs = self.cell_size;
        let to_cell = |v: f64, axis_origin: f64| -> isize {
            ((v - axis_origin) / cs).floor() as isize
        };
        let xmin = to_cell(q_min.x, self.origin.x).max(0) as usize;
        let xmax = (to_cell(q_max.x, self.origin.x).max(0) as usize).min(self.cols - 1);
        let ymin = to_cell(q_min.y, self.origin.y).max(0) as usize;
        let ymax = (to_cell(q_max.y, self.origin.y).max(0) as usize).min(self.rows - 1);
        if xmin > xmax || ymin > ymax {
            return self.view_independent.clone();
        }

        // Fast path: when the query covers ≥80% of the grid we'd be
        // iterating most cells anyway. Skip the cell scan, return
        // every entity. Avoids the worst case (zoomed-to-extents on a
        // drawing where entities span many cells — the per-cell visit
        // count alone becomes the bottleneck).
        let cells_q = (xmax - xmin + 1) * (ymax - ymin + 1);
        let cells_t = self.cols * self.rows;
        if cells_t > 0 && (cells_q * 5) >= (cells_t * 4) {
            let mut out: Vec<u32> = (0..self.n_entities as u32).collect();
            // view_independent indices are already in 0..n_entities
            // since they share the same flat index space.
            let _ = &self.view_independent;
            return out;
        }

        // Dedup with a flat Vec<bool> sized n_entities. ~5 ns per
        // mark-and-test vs ~50 ns per HashSet insert — and crucially,
        // no allocator / hash work per duplicate. At 580 cells/entity
        // (zoomed-out array drawings) this is the difference between
        // ~500 ms and ~20 ms per query.
        let mut seen = vec![false; self.n_entities];
        let mut out: Vec<u32> = Vec::with_capacity(256);
        for cy in ymin..=ymax {
            let row = cy * self.cols;
            for cx in xmin..=xmax {
                for &idx in &self.cells[row + cx] {
                    let u = idx as usize;
                    if u < seen.len() && !seen[u] {
                        seen[u] = true;
                        out.push(idx);
                    }
                }
            }
        }
        // Append view-independent always-include set.
        for &idx in &self.view_independent {
            let u = idx as usize;
            if u < seen.len() && !seen[u] {
                seen[u] = true;
                out.push(idx);
            }
        }
        out
    }

    pub fn query_near(&self, p: Vec2, radius: f64) -> Vec<u32> {
        self.query_bbox(
            Vec2::new(p.x - radius, p.y - radius),
            Vec2::new(p.x + radius, p.y + radius),
        )
    }

    /// (total_cells, total_index_entries, cell_size).
    /// `total_index_entries / N_entities` gives the average cells-per-dobject.
    pub fn stats(&self) -> (usize, usize, f64) {
        let total: usize = self.cells.iter().map(|c| c.len()).sum();
        (self.cols * self.rows, total, self.cell_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Circle;

    fn c(x: f64, y: f64, r: f64) -> DObject {
        Circle { center: Vec2::new(x, y), radius: r }.into()
    }

    /// VIEW-INDEPENDENT ENTITIES MUST REACH EVERY QUERY, and this is load-bearing beyond the
    /// render path.
    ///
    /// A Hatch resolves its real extent through the Document's boundary handles and a BlockRef
    /// through the block table, so the kernel's own bbox for either is a placeholder. They are
    /// therefore not bucketed — they are appended to every result regardless of the rectangle.
    ///
    /// `nearest_entity_under`'s two fallbacks now depend on exactly that. They used to walk the
    /// whole document on every missed click (measured floor 0.95 ms, against 5.8 µs for the
    /// indexed query they back up) and iterate the candidate list instead. That is equivalent only
    /// while this invariant holds — quietly bucketing hatches one day would make clicks inside a
    /// hatch fill stop selecting it, with nothing to say why.
    #[test]
    fn view_independent_entities_reach_every_query() {
        let mut hatch = c(0.0, 0.0, 1.0);
        hatch.geom = crate::geom::Geom::Hatch(crate::geom::Hatch { boundary_handles: Vec::new(), pattern: crate::geom::HatchPattern::Solid });
        assert!(hatch.geom.is_view_independent_bbox(), "a Hatch must be view-independent");

        // The hatch sits at index 1; the query is a pinprick nowhere near anything.
        let ents = vec![c(0.0, 0.0, 1.0), hatch, c(500.0, 500.0, 1.0)];
        let g = UniformGrid::build(&ents, 10.0);
        let hits = g.query_bbox(Vec2::new(-9000.0, -9000.0), Vec2::new(-8999.0, -8999.0));

        assert!(
            hits.contains(&1),
            "the hatch was left out of a query it can never be culled from: {hits:?}",
        );
    }

    #[test]
    fn empty_grid_returns_empty() {
        let g = UniformGrid::build(&[], 1.0);
        assert!(g.query_bbox(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)).is_empty());
    }

    #[test]
    fn basic_query() {
        let ents = vec![
            c(  0.0,  0.0, 1.0),
            c( 10.0, 10.0, 1.0),
            c( 50.0, 50.0, 1.0),
            c(100.0,100.0, 1.0),
        ];
        let g = UniformGrid::build(&ents, 5.0);
        let r = g.query_bbox(Vec2::new(-2.0, -2.0), Vec2::new(12.0, 12.0));
        // Should find indices 0 and 1, not 2 or 3
        assert!(r.contains(&0) && r.contains(&1));
        assert!(!r.contains(&2) && !r.contains(&3));
    }

    #[test]
    fn near_point_query() {
        let ents = vec![
            c( 0.0, 0.0, 1.0),
            c(50.0, 0.0, 1.0),
            c( 0.0,50.0, 1.0),
        ];
        let g = UniformGrid::build(&ents, 5.0);
        let r = g.query_near(Vec2::new(0.0, 0.0), 10.0);
        assert!(r.contains(&0));
        assert!(!r.contains(&1));
        assert!(!r.contains(&2));
    }

    // ---- incremental update (UniformGrid::update) --------------------------

    fn line(x: f64, y: f64) -> DObject {
        crate::geom::Line { a: Vec2::new(x, y), b: Vec2::new(x + 3.0, y + 2.0) }.into()
    }

    fn sorted(mut v: Vec<u32>) -> Vec<u32> { v.sort_unstable(); v }

    /// ⭐ THE ACCEPTANCE TEST. An incrementally-updated grid must answer queries
    /// IDENTICALLY to a full rebuild. A wrong index is a CORRECTNESS bug — you would
    /// click a dobject you can see and not select it — so "faster" is worthless
    /// unless the answers match exactly.
    #[test]
    fn update_matches_a_full_rebuild_everywhere() {
        let mut ents: Vec<DObject> = (0..400)
            .map(|i| line((i % 20) as f64 * 10.0, (i / 20) as f64 * 10.0))
            .collect();
        let cs = 12.0;
        let mut g = UniformGrid::build(&ents, cs);

        // Move a scattered subset — the real "move 3150 of 1.5M" shape. The delta is
        // deliberately small enough to stay INSIDE the grid: build() fits the grid to
        // the drawing's exact bbox, so an OUTWARD move leaves it and is rejected by
        // design (covered by `update_rejects_a_move_outside_the_grid`).
        let changed: Vec<usize> = (0..400).step_by(7).collect();
        for &i in &changed {
            if let crate::geom::Geom::Line(l) = &mut ents[i].geom {
                l.a.x += 5.0; l.a.y += 5.0;
                l.b.x += 5.0; l.b.y += 5.0;
            }
        }
        assert!(g.update(&ents, &changed), "in-bounds move must be absorbed");

        let fresh = UniformGrid::build(&ents, cs);
        // sweep the whole grid with many probes, not one
        for gy in 0..12 {
            for gx in 0..12 {
                let q0 = Vec2::new(gx as f64 * 18.0 - 5.0, gy as f64 * 18.0 - 5.0);
                let q1 = Vec2::new(q0.x + 25.0, q0.y + 25.0);
                assert_eq!(
                    sorted(g.query_bbox(q0, q1)),
                    sorted(fresh.query_bbox(q0, q1)),
                    "incremental != rebuild at probe ({gx},{gy})"
                );
            }
        }
    }

    /// Out-of-bounds must be REJECTED (not silently clamped): the grid's origin/cols/
    /// rows are fixed at build, so a dobject moved outside would be mis-bucketed.
    /// Rejecting is how the caller knows to rebuild.
    #[test]
    fn update_rejects_a_move_outside_the_grid() {
        let mut ents: Vec<DObject> = (0..50).map(|i| line(i as f64 * 5.0, 0.0)).collect();
        let mut g = UniformGrid::build(&ents, 8.0);
        if let crate::geom::Geom::Line(l) = &mut ents[3].geom {
            l.a.x += 100_000.0; l.b.x += 100_000.0; // far outside
        }
        assert!(!g.update(&ents, &[3]), "must reject → caller rebuilds");
    }

    /// A count change shifts every index, invalidating the stored ranges.
    #[test]
    fn update_rejects_add_or_delete() {
        let ents: Vec<DObject> = (0..20).map(|i| line(i as f64 * 5.0, 0.0)).collect();
        let mut g = UniformGrid::build(&ents, 8.0);
        let mut more = ents.clone();
        more.push(line(1.0, 1.0));
        assert!(!g.update(&more, &[0]), "count change → rebuild");
        let fewer = &ents[..19];
        assert!(!g.update(fewer, &[0]), "count change → rebuild");
    }

    /// A rejected update must leave the grid UNTOUCHED — no half-applied state.
    #[test]
    fn a_rejected_update_does_not_corrupt_the_grid() {
        let mut ents: Vec<DObject> = (0..30).map(|i| line(i as f64 * 5.0, 0.0)).collect();
        let mut g = UniformGrid::build(&ents, 8.0);
        let before = sorted(g.query_bbox(Vec2::new(-1.0, -1.0), Vec2::new(200.0, 20.0)));
        // one legal move, one illegal — the batch must be rejected WHOLE
        if let crate::geom::Geom::Line(l) = &mut ents[1].geom { l.a.x += 2.0; l.b.x += 2.0; }
        if let crate::geom::Geom::Line(l) = &mut ents[2].geom { l.a.x += 99_999.0; l.b.x += 99_999.0; }
        assert!(!g.update(&ents, &[1, 2]));
        let after = sorted(g.query_bbox(Vec2::new(-1.0, -1.0), Vec2::new(200.0, 20.0)));
        assert_eq!(before, after, "a rejected update must not mutate the grid");
    }

    /// build_auto must produce an index INDISTINGUISHABLE from the two-call form. It
    /// is a pure speed change; if the answers differ, picking/selection break.
    #[test]
    fn build_auto_matches_auto_cell_size_plus_build() {
        let ents: Vec<DObject> = (0..500)
            .map(|i| line((i % 25) as f64 * 11.0, (i / 25) as f64 * 9.0))
            .collect();
        let a = UniformGrid::build_auto(&ents, 10.0);
        let b = UniformGrid::build(&ents, UniformGrid::auto_cell_size(&ents, 10.0));
        assert!((a.cell_size - b.cell_size).abs() < 1e-9, "same cell size");
        assert_eq!((a.cols, a.rows), (b.cols, b.rows), "same grid shape");
        for gy in 0..14 {
            for gx in 0..14 {
                let q0 = Vec2::new(gx as f64 * 22.0 - 6.0, gy as f64 * 18.0 - 6.0);
                let q1 = Vec2::new(q0.x + 30.0, q0.y + 30.0);
                assert_eq!(
                    sorted(a.query_bbox(q0, q1)),
                    sorted(b.query_bbox(q0, q1)),
                    "build_auto != build at probe ({gx},{gy})"
                );
            }
        }
    }

    /// …and it must still support incremental update afterwards.
    #[test]
    fn build_auto_grid_can_still_absorb_updates() {
        let mut ents: Vec<DObject> = (0..200).map(|i| line((i % 20) as f64 * 10.0, (i / 20) as f64 * 10.0)).collect();
        let mut g = UniformGrid::build_auto(&ents, 10.0);
        let changed: Vec<usize> = (0..200).step_by(5).collect();
        for &i in &changed {
            if let crate::geom::Geom::Line(l) = &mut ents[i].geom { l.a.x += 1.0; l.b.x += 1.0; }
        }
        assert!(g.update(&ents, &changed), "ranges are populated by build_auto too");
    }

    // ---- APPEND ABSORPTION ------------------------------------------------------------------
    //
    // Drawing an object used to invalidate the whole index: a full O(n) rebuild, measured at
    // 13.4 ms per edit at 100k dobjects and 247.9 ms at 1.5M, to record the arrival of one line.
    // `insert_appended` takes it in O(added) instead. The risk this trades for speed is an index
    // that is subtly WRONG, so the governing test is the one that compares it against a rebuild.

    /// THE INVARIANT. An absorbed append must answer every query exactly as a freshly built grid
    /// would — same objects, for every rectangle. A fast index that quietly loses an object is
    /// worse than a slow one, because the object is still on screen and simply stops being
    /// selectable.
    ///
    /// Compared against `build`, which is the independent authority here: it is the code path the
    /// fallback uses, so if the two ever disagree the fallback is the correct one.
    #[test]
    fn an_absorbed_append_answers_exactly_as_a_rebuild_would() {
        let mut objs: Vec<DObject> = (0..40)
            .map(|i| c((i % 8) as f64 * 5.0, (i / 8) as f64 * 5.0, 1.0))
            .collect();
        let mut grid = UniformGrid::build(&objs, 4.0);

        // Three more, inside the existing extent so the grid can take them.
        objs.push(c(11.0, 6.0, 0.5));
        objs.push(c(3.0, 14.0, 2.0));
        objs.push(c(28.0, 2.0, 1.5));
        assert!(grid.insert_appended(&objs), "an in-bounds append must be absorbed");

        let rebuilt = UniformGrid::build(&objs, 4.0);
        // Sweep the whole populated area plus a margin, in windows of several sizes — one
        // rectangle could agree by luck; forty cannot.
        for &w in &[1.0_f64, 3.0, 9.0] {
            let mut x = -5.0_f64;
            while x < 40.0 {
                let mut y = -5.0_f64;
                while y < 25.0 {
                    let (lo, hi) = (Vec2::new(x, y), Vec2::new(x + w, y + w));
                    let mut a = grid.query_bbox(lo, hi);
                    let mut b = rebuilt.query_bbox(lo, hi);
                    a.sort_unstable();
                    b.sort_unstable();
                    assert_eq!(a, b, "absorbed grid disagrees with a rebuild at ({x},{y}) w={w}");
                    y += 3.0;
                }
                x += 3.0;
            }
        }
    }

    /// The new object is actually findable where it was put — the cheap sanity check that the
    /// comparison above would also catch, kept because its failure message says what went wrong.
    #[test]
    fn an_appended_object_is_found_at_its_own_position() {
        let mut objs = vec![c(0.0, 0.0, 1.0), c(10.0, 0.0, 1.0)];
        let mut grid = UniformGrid::build(&objs, 4.0);
        objs.push(c(5.0, 0.0, 0.5));
        assert!(grid.insert_appended(&objs));
        let hit = grid.query_bbox(Vec2::new(4.5, -0.5), Vec2::new(5.5, 0.5));
        assert!(hit.contains(&2), "the appended circle is not in its own cell: {hit:?}");
    }

    /// OUT OF BOUNDS IS REFUSED, because the grid's origin/cols/rows are fixed at build time and
    /// growing them is a rebuild. Drawing the first object far from everything else is the
    /// ordinary way to reach this.
    #[test]
    fn an_append_outside_the_grid_is_refused() {
        let mut objs = vec![c(0.0, 0.0, 1.0), c(5.0, 5.0, 1.0)];
        let mut grid = UniformGrid::build(&objs, 4.0);
        objs.push(c(10_000.0, 10_000.0, 1.0));
        assert!(!grid.insert_appended(&objs), "an out-of-bounds append must be refused");
    }

    /// …AND A REFUSAL LEAVES THE GRID EXACTLY AS IT WAS. The caller's answer to `false` is to
    /// rebuild, and until it does, this grid is still being queried — so a half-applied append
    /// would hand out wrong answers in the window between.
    #[test]
    fn a_refused_append_leaves_the_grid_untouched() {
        let mut objs = vec![c(0.0, 0.0, 1.0), c(5.0, 5.0, 1.0)];
        let mut grid = UniformGrid::build(&objs, 4.0);
        let before: Vec<u32> = grid.query_bbox(Vec2::new(-9.0, -9.0), Vec2::new(9.0, 9.0));

        // One that fits, then one that does not — so a naive implementation that commits as it
        // goes would already have written the first before refusing on the second.
        objs.push(c(2.0, 2.0, 0.5));
        objs.push(c(10_000.0, 10_000.0, 1.0));
        assert!(!grid.insert_appended(&objs), "must refuse the batch");

        let after: Vec<u32> = grid.query_bbox(Vec2::new(-9.0, -9.0), Vec2::new(9.0, 9.0));
        assert_eq!(before, after, "a refused append modified the grid anyway");
        assert_eq!(grid.ranges.len(), 2, "a refused append grew the range table");
    }

    /// A VIEW-INDEPENDENT APPEND JOINS THE GLOBAL LIST rather than a cell. Hatches and block
    /// references resolve their real extent through the Document, so the kernel's bbox for them
    /// is a placeholder — they are returned by every query instead of being bucketed, and an
    /// append has to preserve that or a newly drawn hatch stops being clickable.
    #[test]
    fn an_appended_view_independent_object_still_reaches_every_query() {
        use crate::geom::{Hatch, Geom};
        let mut objs = vec![c(0.0, 0.0, 1.0), c(5.0, 5.0, 1.0)];
        let mut grid = UniformGrid::build(&objs, 4.0);
        let h = Hatch { boundary_handles: Vec::new(), pattern: crate::geom::HatchPattern::Solid };
        objs.push(DObject::new(Geom::Hatch(h)));
        assert!(grid.insert_appended(&objs), "a view-independent append is always absorbable");

        // Far from anything, so only the global list can supply it.
        let far = grid.query_bbox(Vec2::new(500.0, 500.0), Vec2::new(501.0, 501.0));
        assert!(far.contains(&2), "the appended hatch does not reach a distant query: {far:?}");
    }

    /// Appends are for GROWTH only. A delete shifts every index after it, which is the whole
    /// reason `update` refuses a count change, and this must refuse it too rather than corrupt
    /// the bucket space.
    #[test]
    fn a_shrunk_document_is_refused() {
        let mut objs = vec![c(0.0, 0.0, 1.0), c(5.0, 5.0, 1.0), c(2.0, 2.0, 1.0)];
        let mut grid = UniformGrid::build(&objs, 4.0);
        objs.pop();
        assert!(!grid.insert_appended(&objs), "a shrunk document must be refused");
        objs.pop();
        assert!(!grid.insert_appended(&objs), "…and so must a shorter one");
    }
}
