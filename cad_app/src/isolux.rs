//! ISOLUX LINES — the threshold curves of a calculated field, traced by marching squares.
//!
//! Asked for as a line item on the SIMLUX viewport: *isolux contour lines in the SIMLUX window*.
//!
//! A false-colour field says roughly how much light there is; an isolux line says exactly where a
//! number is. They answer different questions and a lighting drawing normally carries both — "the
//! 300 lx line runs here" is a statement you can hold a tape measure against, which no amount of
//! colour is.
//!
//! This is deliberately NOT the report's band renderer. That one paints filled regions and merges
//! whole cells into runs; a line has no interior to fill and no runs to merge, and trying to reuse
//! it would mean asking a filler to hand back its boundary, which it never computes. What the two
//! DO share is the thresholds — [`crate::report::options::Scale::edges`] — so the lines land on the
//! band edges and the picture stays one picture.

/// A scalar field sampled at the CORNERS of an `nx` × `ny` grid of cells, so `v` is
/// `(nx + 1) * (ny + 1)` long. Corners, not centres, because a contour runs between samples.
///
/// `inside` is per CELL — `nx * ny` — and says which cells are in the room. A cell that is not is
/// never traced, so the contour stops at the room's edge instead of running out across the page.
pub struct Field {
    pub nx: usize,
    pub ny: usize,
    pub v: Vec<f64>,
    pub inside: Vec<bool>,
}

impl Field {
    pub fn at(&self, i: usize, j: usize) -> f64 {
        self.v[j * (self.nx + 1) + i]
    }

    fn cell_in(&self, i: usize, j: usize) -> bool {
        self.inside.get(j * self.nx + i).copied().unwrap_or(true)
    }
}

/// One traced segment, in LATTICE coordinates: x in `0..=nx`, y in `0..=ny`.
///
/// Left in lattice space on purpose. The 3D viewport wants metres, a future 2D overlay would want
/// points, and a tracer that has already committed to one of them is a tracer the other has to
/// undo. Converting is one multiply at the call site.
pub type Seg = [(f64, f64); 2];

/// Resample a calculated grid onto an `nx` × `ny` lattice of cells, bilinearly.
///
/// IN PLAN ORIENTATION: `j` grows with world **+y**, so lattice row 0 is the plane's minimum y.
/// The report's own resampler flips this, because a page's y runs the other way — and getting that
/// backwards is what once shipped a false-colour page mirrored against the layout page beside it.
/// Anything drawing this in world space wants it exactly as it is here.
///
/// The grid's values sit at CELL CENTRES, so a lattice corner at fraction `u` across the plane
/// reads at `u · cols − 0.5` in cell space; the half-cell is why this is not just a scale.
pub fn sample(
    grid: &cad_light::LuxGrid,
    mask: &[bool],
    nx: usize,
    ny: usize,
) -> Field {
    let (gc, gr) = (grid.cols.max(1) as usize, grid.rows.max(1) as usize);
    let at = |cx: usize, cy: usize| -> f64 {
        grid.values.get(cy.min(gr - 1) * gc + cx.min(gc - 1)).copied().unwrap_or(0.0)
    };
    let read = |fx: f64, fy: f64| -> f64 {
        let x = fx.clamp(0.0, (gc - 1) as f64);
        let y = fy.clamp(0.0, (gr - 1) as f64);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (tx, ty) = (x - x0 as f64, y - y0 as f64);
        at(x0, y0) * (1.0 - tx) * (1.0 - ty)
            + at(x0 + 1, y0) * tx * (1.0 - ty)
            + at(x0, y0 + 1) * (1.0 - tx) * ty
            + at(x0 + 1, y0 + 1) * tx * ty
    };
    let mut v = vec![0.0; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            let fx = i as f64 / nx as f64 * gc as f64 - 0.5;
            let fy = j as f64 / ny as f64 * gr as f64 - 0.5;
            v[j * (nx + 1) + i] = read(fx, fy);
        }
    }
    // The mask is per CALCULATED cell, so a lattice cell takes the mask of whichever calculated
    // cell its centre lands in. Nearest, not interpolated: "is this a place a reading was taken"
    // has no in-between value, and softening it would paint colour over the excluded cells this
    // whole exclusion exists to keep out.
    let mut inside = vec![true; nx * ny];
    if !mask.is_empty() {
        for j in 0..ny {
            for i in 0..nx {
                let cx = ((i as f64 + 0.5) / nx as f64 * gc as f64) as usize;
                let cy = ((j as f64 + 0.5) / ny as f64 * gr as f64) as usize;
                inside[j * nx + i] =
                    mask.get(cy.min(gr - 1) * gc + cx.min(gc - 1)).copied().unwrap_or(true);
            }
        }
    }
    Field { nx, ny, v, inside }
}

/// The value at the middle of lattice cell `(i, j)` — what colours it.
pub fn cell_value(f: &Field, i: usize, j: usize) -> f64 {
    (f.at(i, j) + f.at(i + 1, j) + f.at(i, j + 1) + f.at(i + 1, j + 1)) * 0.25
}

/// Where the contour crosses the edge between two corner values.
fn cross(a: f64, b: f64, t: f64) -> f64 {
    let d = b - a;
    if d.abs() < 1e-12 {
        // A FLAT EDGE HAS NO CROSSING POINT. Both corners sit on the threshold, so any position
        // along the edge is as good as any other and the midpoint is the one that does not bias
        // the line towards a corner. Returning 0 or 1 here — which the naive division does when it
        // divides by a denormal — puts a kink in an otherwise straight contour.
        return 0.5;
    }
    ((t - a) / d).clamp(0.0, 1.0)
}

/// Trace the `t` lx contour of `f`.
///
/// Standard marching squares. The two ambiguous cases — the field high on one diagonal and low on
/// the other — are resolved by the cell's own average rather than by picking a convention, which is
/// what stops a saddle from joining two pools that are not joined.
pub fn trace(f: &Field, t: f64) -> Vec<Seg> {
    let mut out = Vec::new();
    if f.nx == 0 || f.ny == 0 || f.v.len() != (f.nx + 1) * (f.ny + 1) {
        return out;
    }
    for j in 0..f.ny {
        for i in 0..f.nx {
            if !f.cell_in(i, j) {
                continue;
            }
            let (a, b, c, d) = (f.at(i, j), f.at(i + 1, j), f.at(i + 1, j + 1), f.at(i, j + 1));
            let code = (a >= t) as u8 | ((b >= t) as u8) << 1 | ((c >= t) as u8) << 2 | ((d >= t) as u8) << 3;
            if code == 0 || code == 15 {
                continue;
            }
            let (x, y) = (i as f64, j as f64);
            let bottom = (x + cross(a, b, t), y);
            let right = (x + 1.0, y + cross(b, c, t));
            let top = (x + cross(d, c, t), y + 1.0);
            let left = (x, y + cross(a, d, t));
            // The centre decides the saddles. `(a + b + c + d) / 4` is the bilinear value at the
            // middle of the cell, which is the same surface the corners came from.
            let centre_high = (a + b + c + d) * 0.25 >= t;
            match code {
                1 | 14 => out.push([left, bottom]),
                2 | 13 => out.push([bottom, right]),
                3 | 12 => out.push([left, right]),
                4 | 11 => out.push([right, top]),
                6 | 9 => out.push([bottom, top]),
                7 | 8 => out.push([left, top]),
                5 => {
                    // a and c high. If the middle is high too they are joined, so the line goes
                    // around b and around d separately; if it is not, a and c are islands.
                    if centre_high {
                        out.push([bottom, right]);
                        out.push([left, top]);
                    } else {
                        out.push([left, bottom]);
                        out.push([right, top]);
                    }
                }
                10 => {
                    // b and d high — the same argument with the diagonal the other way.
                    if centre_high {
                        out.push([left, bottom]);
                        out.push([right, top]);
                    } else {
                        out.push([bottom, right]);
                        out.push([left, top]);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field rising linearly in x, so the `t` contour is a straight vertical line at a KNOWN x.
    fn ramp_field(nx: usize, ny: usize, lo: f64, hi: f64) -> Field {
        let mut v = vec![0.0; (nx + 1) * (ny + 1)];
        for j in 0..=ny {
            for i in 0..=nx {
                v[j * (nx + 1) + i] = lo + (hi - lo) * i as f64 / nx as f64;
            }
        }
        Field { nx, ny, v, inside: vec![true; nx * ny] }
    }

    /// THE LINE LANDS WHERE THE VALUE IS, not on the nearest sample.
    ///
    /// The whole point of tracing rather than colouring: on a 10-cell field running 0→500 lx, the
    /// 275 lx contour sits at 55 % of the width, which is not a grid line. A nearest-sample tracer
    /// would put it at 60 % and be wrong by half a cell everywhere.
    #[test]
    fn the_contour_is_interpolated_not_snapped() {
        let f = ramp_field(10, 4, 0.0, 500.0);
        let segs = trace(&f, 275.0);
        assert!(!segs.is_empty(), "a field crossing 275 lx produced no contour at all");
        let want = 0.55 * 10.0;
        for s in &segs {
            for p in s {
                assert!(
                    (p.0 - want).abs() < 1e-6,
                    "a contour point at x = {:.4}, expected {want:.4}",
                    p.0,
                );
            }
        }
    }

    /// THE SADDLE IS RESOLVED BY THE FIELD, not by a convention.
    ///
    /// This test exists because the obvious one does not work. `every_traced_point_actually_sits_on
    /// _the_threshold` was written to cover the saddle and DOES NOT: deleting the resolution
    /// entirely and always pairing one way leaves every traced point exactly on the threshold, so
    /// it passed unchanged. Both pairings put their endpoints on the same four edge crossings —
    /// what differs is which corner each segment wraps around, and that is the only thing worth
    /// asserting.
    ///
    /// One cell, high on the a–c diagonal. When the middle is LOW the two high corners are islands
    /// and each segment must wrap one of them; when the middle is HIGH they are joined and the
    /// segments must instead wrap the two LOW corners. A fixed convention gets exactly one of these
    /// two cases right, which is why both are here.
    #[test]
    fn a_saddle_wraps_the_corners_the_field_says_it_should() {
        // Corner order in the lattice: a = (0,0), b = (1,0), c = (1,1), d = (0,1).
        let cell = |a: f64, b: f64, c: f64, d: f64| Field {
            nx: 1,
            ny: 1,
            v: vec![a, b, d, c],
            inside: vec![true],
        };
        // Which corner a segment wraps: both its endpoints lie on the two edges meeting there.
        let wraps = |s: &Seg| -> Option<(u8, u8)> {
            let on = |p: &(f64, f64)| -> (bool, bool, bool, bool) {
                // (bottom, right, top, left)
                (p.1 < 1e-9, p.0 > 1.0 - 1e-9, p.1 > 1.0 - 1e-9, p.0 < 1e-9)
            };
            let (e0, e1) = (on(&s[0]), on(&s[1]));
            let edges = |e: (bool, bool, bool, bool)| {
                [e.0, e.1, e.2, e.3].iter().position(|b| *b).map(|i| i as u8)
            };
            Some((edges(e0)?, edges(e1)?))
        };
        // Edge indices: 0 bottom, 1 right, 2 top, 3 left. Corner a is bottom+left = {0,3};
        // b is bottom+right {0,1}; c is right+top {1,2}; d is top+left {2,3}.
        let pair = |s: &Seg| {
            let (x, y) = wraps(s).expect("a segment endpoint was not on a cell edge");
            let mut v = [x, y];
            v.sort();
            v
        };

        // MIDDLE LOW: a and c are islands, so the segments wrap a {0,3} and c {1,2}.
        let low = cell(300.0, 50.0, 300.0, 50.0);
        assert!((300.0 + 50.0 + 300.0 + 50.0) / 4.0 < 200.0, "fixture's centre is not low");
        let segs = trace(&low, 200.0);
        assert_eq!(segs.len(), 2, "a saddle traces two segments");
        let mut got: Vec<[u8; 2]> = segs.iter().map(pair).collect();
        got.sort();
        assert_eq!(
            got,
            vec![[0, 3], [1, 2]],
            "with a low middle the two high corners are separate pools and each segment must wrap \
             one of them",
        );

        // MIDDLE HIGH: a and c are joined, so the segments wrap the LOW corners b {0,1} and d {2,3}.
        let high = cell(300.0, 150.0, 300.0, 150.0);
        assert!((300.0 + 150.0 + 300.0 + 150.0) / 4.0 >= 200.0, "fixture's centre is not high");
        let segs = trace(&high, 200.0);
        assert_eq!(segs.len(), 2, "a saddle traces two segments");
        let mut got: Vec<[u8; 2]> = segs.iter().map(pair).collect();
        got.sort();
        assert_eq!(
            got,
            vec![[0, 1], [2, 3]],
            "with a high middle the high corners are joined and the segments must wrap the low ones",
        );
    }

    /// EVERY SEGMENT IS ON THE LINE. Whatever the case table does, a traced point must be a point
    /// where the field really is `t` — which is checkable directly, by interpolating the field
    /// back at that point.
    ///
    /// NOTE what this does and does not cover: it pins where the endpoints ARE, and says nothing
    /// about how they are paired. See `a_saddle_wraps_the_corners_the_field_says_it_should` for the
    /// pairing, which this test was originally — and wrongly — assumed to cover.
    #[test]
    fn every_traced_point_actually_sits_on_the_threshold() {
        // A saddle: high on one diagonal, low on the other, which is the case a convention gets
        // wrong quietly.
        let (nx, ny) = (8, 8);
        let mut v = vec![0.0; (nx + 1) * (ny + 1)];
        for j in 0..=ny {
            for i in 0..=nx {
                let (x, y) = (i as f64 / nx as f64 - 0.5, j as f64 / ny as f64 - 0.5);
                v[j * (nx + 1) + i] = 200.0 + 400.0 * x * y;
            }
        }
        let f = Field { nx, ny, v, inside: vec![true; nx * ny] };
        let segs = trace(&f, 200.0);
        assert!(!segs.is_empty(), "the saddle produced no contour");
        for s in &segs {
            for p in s {
                // Bilinear read-back at the traced point.
                let (i, j) = (p.0.floor().min((nx - 1) as f64) as usize, p.1.floor().min((ny - 1) as f64) as usize);
                let (tx, ty) = (p.0 - i as f64, p.1 - j as f64);
                let got = f.at(i, j) * (1.0 - tx) * (1.0 - ty)
                    + f.at(i + 1, j) * tx * (1.0 - ty)
                    + f.at(i, j + 1) * (1.0 - tx) * ty
                    + f.at(i + 1, j + 1) * tx * ty;
                assert!(
                    (got - 200.0).abs() < 1e-6,
                    "a point traced as the 200 lx line reads {got:.4} lx",
                );
            }
        }
    }

    /// THE ROOM'S EDGE STOPS THE LINE. A cell outside the room is never traced, so an isolux line
    /// cannot run out across ground the calculation does not cover.
    #[test]
    fn a_cell_outside_the_room_is_not_traced() {
        let mut f = ramp_field(10, 4, 0.0, 500.0);
        let all = trace(&f, 275.0).len();
        assert!(all > 0);
        // Shut off the bottom row.
        for i in 0..f.nx {
            f.inside[i] = false;
        }
        let fewer = trace(&f, 275.0).len();
        assert!(
            fewer < all,
            "masking a row changed nothing — {all} segments before, {fewer} after",
        );
        for s in trace(&f, 275.0) {
            for p in s {
                assert!(p.1 >= 1.0 - 1e-9, "a segment was traced at y = {:.3}, inside the masked row", p.1);
            }
        }
    }

    /// A FLAT FIELD HAS NO CONTOUR — and does not divide by zero producing one.
    #[test]
    fn a_field_that_never_crosses_produces_nothing() {
        let f = ramp_field(6, 6, 300.0, 300.0);
        assert!(trace(&f, 100.0).is_empty(), "a flat 300 lx field crossed 100 lx");
        assert!(trace(&f, 500.0).is_empty(), "a flat 300 lx field crossed 500 lx");
    }
}

/// RESAMPLING A CALCULATED GRID — the step between the engine's cells and anything that draws them.
#[cfg(test)]
mod resampling {
    use super::*;

    /// A grid whose value IS its row, so orientation is readable straight off the numbers.
    fn rows_grid(cols: u32, rows: u32) -> cad_light::LuxGrid {
        let mut values = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            for _ in 0..cols {
                values.push(r as f64 * 100.0);
            }
        }
        cad_light::LuxGrid::from_values(cols, rows, values)
    }

    /// PLAN ORIENTATION, NOT PAGE ORIENTATION. Lattice row 0 is the plane's MINIMUM y, the same way
    /// the calculated grid's row 0 is.
    ///
    /// The report's own resampler flips, because a page's y runs downward — and the two being
    /// confused is precisely what once put the false-colour page and the layout page beside each
    /// other showing the same room mirrored. Anything drawing in world space wants no flip at all,
    /// and that is worth an assertion rather than a comment.
    #[test]
    fn row_zero_is_the_bottom_of_the_room_not_the_top() {
        let g = rows_grid(4, 5);
        let f = sample(&g, &[], 8, 10);
        let bottom = f.at(4, 0);
        let top = f.at(4, f.ny);
        assert!(
            bottom < top,
            "row 0 reads {bottom:.1} lx and the last row {top:.1} — the field came out flipped",
        );
        // And it spans the right RANGE: the grid runs 0 → 400 lx.
        assert!(bottom < 50.0 && top > 350.0, "sampled {bottom:.1} … {top:.1}, expected ~0 … ~400");
    }

    /// THE HALF-CELL IS NOT FORGOTTEN. Values sit at cell CENTRES, so the lattice corner at the
    /// very edge of the plane is half a cell OUTSIDE the outermost centre — it clamps to it rather
    /// than reading the next row along. Dropping the half-cell shifts the whole field by half a
    /// cell, which on a 0.25 m grid is 125 mm of drawing.
    #[test]
    fn the_values_sit_at_cell_centres() {
        let g = rows_grid(4, 4); // rows read 0, 100, 200, 300
        let f = sample(&g, &[], 4, 4);
        // The lattice corner at j = 2 sits at the plane's midpoint, which is the boundary BETWEEN
        // rows 1 and 2 — so it reads their average, 150, not either of them.
        assert!(
            (f.at(2, 2) - 150.0).abs() < 1e-6,
            "the middle corner reads {:.2} lx, expected the 150 lx boundary between rows 1 and 2",
            f.at(2, 2),
        );
        // The bottom corner clamps to row 0's own value rather than extrapolating below it.
        assert!((f.at(2, 0) - 0.0).abs() < 1e-6, "the bottom edge reads {:.2}", f.at(2, 0));
        assert!((f.at(2, 4) - 300.0).abs() < 1e-6, "the top edge reads {:.2}", f.at(2, 4));
    }

    /// AN EXCLUDED CELL STAYS EXCLUDED THROUGH THE RESAMPLE. The buried cells are kept out of the
    /// statistics; a resampler that smoothed the mask would put them back into the picture.
    #[test]
    fn the_mask_survives_the_resample() {
        let g = rows_grid(4, 4);
        // Knock out the bottom-left calculated cell.
        let mut mask = vec![true; 16];
        mask[0] = false;
        let f = sample(&g, &mask, 8, 8);
        // That cell is the bottom-left quarter-of-a-quarter of the plane: lattice cells 0..2 in
        // both axes at a x2 supersample.
        assert!(!f.inside[0], "the excluded cell came back");
        assert!(!f.inside[1], "only part of the excluded cell was excluded");
        assert!(!f.inside[8], "only part of the excluded cell was excluded");
        assert!(f.inside[2], "the exclusion spread into the cell beside it");
        assert!(f.inside[16], "the exclusion spread into the cell above it");
        assert_eq!(f.inside.iter().filter(|k| !**k).count(), 4, "one calculated cell is four here");
    }

    /// NO MASK MEANS EVERYTHING. A plane that IS the room carries an empty mask, and reading that
    /// as "nothing is inside" would draw an empty room.
    #[test]
    fn an_empty_mask_excludes_nothing() {
        let f = sample(&rows_grid(4, 4), &[], 8, 8);
        assert!(f.inside.iter().all(|k| *k), "an empty mask threw cells away");
    }
}
