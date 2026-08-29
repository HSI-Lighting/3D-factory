// Hatch pattern catalog — hardcoded, no external .pat files.
//
// A pattern is either a list of LINE FAMILIES (infinite parallel lines
// spaced uniformly — the renderer clips them against the resolved hatch
// boundary using even-odd) or a TILE (finite segments laid out on a
// periodic grid). Solid hatches don't go through here.
//
// Pattern names match the industry-standard AutoCAD vocabulary
// (ANSI31, BRICK, NET, EARTH, …) so files exchange cleanly. The
// geometry of each pattern is derived independently — no copy of
// AutoCAD's `acad.pat` or LibreCAD's GPL'd .dxf pattern files. The
// names are not trademarks (ANSI is a real standards body; the rest
// are English words used in CAD vocabulary for decades).
//
// BRICK + TILE were derived from in-house DXF references supplied by
// the user (see `~/workspace/RUST_CAD/Hatch_Patten/`) — running-bond
// brick and a decorative star tile, both expressed as a minimal
// periodic cell whose segments cover every brick / tile edge without
// duplicates when tiled.

#[derive(Clone, Debug)]
pub struct LineFamily {
    /// Direction of the lines, in radians measured CCW from +X.
    pub angle:    f64,
    /// Anchor — one specific line in the family passes through this
    /// point. The rest are stepped from this anchor by `spacing` in
    /// the family's normal direction.
    pub base_x:   f64,
    pub base_y:   f64,
    /// Perpendicular distance between consecutive parallel lines, in
    /// pattern's unit scale (multiplied by the hatch's `scale` field
    /// at render time).
    pub spacing:  f64,
}

/// One finite segment inside a tile's canonical period rectangle.
/// Coordinates are in the pattern's natural units (multiplied by the
/// hatch's `scale` at render time). Endpoints MAY lie outside
/// `[0, period_x) × [0, period_y)` — tile renderer just translates them
/// by every (i·period_x, j·period_y) covering the boundary bbox.
#[derive(Clone, Debug)]
pub struct PatternSegment {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// One circle inside a tile's canonical period. Same scale + tiling
/// rules as `PatternSegment`. Used by patterns like CONCENTRIC where
/// the cell repeats a stack of nested circles.
#[derive(Clone, Debug)]
pub struct PatternCircle {
    pub cx:     f64,
    pub cy:     f64,
    pub radius: f64,
}

/// What `lookup` returns. Either a set of infinite line families
/// (ANSI31, NET, EARTH, …) or a tiled finite-segment cell (BRICK,
/// TILE). The renderer dispatches on this enum.
#[derive(Clone, Debug)]
pub enum Pattern {
    Families(Vec<LineFamily>),
    Tile {
        period_x: f64,
        period_y: f64,
        segments: Vec<PatternSegment>,
        /// Circle primitives in the same tile cell. Most patterns leave
        /// this empty; patterns like CONCENTRIC use it for stacked
        /// rings. Renderer paints each circle (clipped to the hatch
        /// boundary) at every tiled cell origin.
        circles:  Vec<PatternCircle>,
    },
}

impl Pattern {
    /// Empty pattern — renderer draws nothing. Used as the unknown-name
    /// fallback so hatches with stale pattern names don't crash.
    pub fn empty() -> Self { Pattern::Families(Vec::new()) }

    /// `true` if this pattern would produce no geometry. Used by tests
    /// + the hatch-debug dump to spot misconfigured entries.
    pub fn is_empty(&self) -> bool {
        match self {
            Pattern::Families(v) => v.is_empty(),
            Pattern::Tile { segments, circles, .. } =>
                segments.is_empty() && circles.is_empty(),
        }
    }
}

/// Resolve a canonical pattern name (case-insensitive) to its pattern
/// definition. Unknown names return `Pattern::empty()` — render produces
/// no lines but doesn't crash.
///
/// Each entry below is documented with a one-line ASCII sketch so the
/// reader can match name → visual at a glance.
pub fn lookup(name: &str) -> Pattern {
    let up = name.to_ascii_uppercase();
    let pi = std::f64::consts::PI;
    match up.as_str() {
        // ANSI31 — 45° diagonals  / / / / /
        "ANSI31" => Pattern::Families(vec![
            LineFamily { angle: pi / 4.0,        base_x: 0.0, base_y: 0.0, spacing: 3.175 },
        ]),
        // ANSI32 — 45° diagonal pairs (close + far spacing alternating)
        //   ||  ||  ||
        // approximated as two interleaved families at the same angle
        "ANSI32" => Pattern::Families(vec![
            LineFamily { angle: pi / 4.0, base_x: 0.0,  base_y: 0.0, spacing: 6.350 },
            LineFamily { angle: pi / 4.0, base_x: 1.59, base_y: 1.59, spacing: 6.350 },
        ]),
        // ANSI33 — 135° diagonals at 3 mm
        "ANSI33" => Pattern::Families(vec![
            LineFamily { angle: 3.0 * pi / 4.0,  base_x: 0.0, base_y: 0.0, spacing: 3.175 },
        ]),
        // ANSI37 — fine 45°/135° crosshatch (cork / fibre)  X X X
        "ANSI37" => Pattern::Families(vec![
            LineFamily { angle: pi / 4.0,        base_x: 0.0, base_y: 0.0, spacing: 3.175 },
            LineFamily { angle: 3.0 * pi / 4.0,  base_x: 0.0, base_y: 0.0, spacing: 3.175 },
        ]),
        // EARTH — horizontal + vertical coarse grid (soil/earth symbol);
        // visually distinct from ANSI37's diagonals so the two thumbnails
        // don't look identical in the picker.
        "EARTH" => Pattern::Families(vec![
            LineFamily { angle: 0.0,             base_x: 0.0, base_y: 0.0, spacing: 8.0 },
            LineFamily { angle: pi / 2.0,        base_x: 0.0, base_y: 0.0, spacing: 8.0 },
        ]),
        // CROSS — fine horizontal + vertical grid (finer than NET).
        "CROSS" => Pattern::Families(vec![
            LineFamily { angle: 0.0,             base_x: 0.0, base_y: 0.0, spacing: 3.0 },
            LineFamily { angle: pi / 2.0,        base_x: 0.0, base_y: 0.0, spacing: 3.0 },
        ]),
        // NET — coarse horizontal + vertical grid.
        "NET" => Pattern::Families(vec![
            LineFamily { angle: 0.0,             base_x: 0.0, base_y: 0.0, spacing: 6.0 },
            LineFamily { angle: pi / 2.0,        base_x: 0.0, base_y: 0.0, spacing: 6.0 },
        ]),
        // ANGLE — horizontal + vertical, coarser than CROSS
        "ANGLE" => Pattern::Families(vec![
            LineFamily { angle: 0.0,             base_x: 0.0, base_y: 0.0, spacing: 6.35 },
            LineFamily { angle: pi / 2.0,        base_x: 0.0, base_y: 0.0, spacing: 6.35 },
        ]),
        // BRICK — running-bond masonry. Derived from
        //   ~/workspace/RUST_CAD/Hatch_Patten/brick pattern.dxf
        // Cell is 3 × 2 (one brick = 3 × 1, two rows stacked with the
        // upper row offset by half-brick). Canonical period segments:
        //   • horizontal at y = 0    (bottom of bottom row / shared
        //                              with top of cell below)
        //   • horizontal at y = 1    (between the two rows)
        //   • vertical   at x = 0,
        //     y ∈ [1, 2]             (left edge of the top-row brick)
        //   • vertical   at x = 1.5,
        //     y ∈ [0, 1]             (left edge of the bottom-row
        //                              offset brick)
        // When tiled, every brick edge in the running-bond pattern is
        // drawn exactly once.
        "BRICK" => Pattern::Tile {
            period_x: 3.0,
            period_y: 2.0,
            segments: vec![
                PatternSegment { x1: 0.0, y1: 0.0, x2: 3.0, y2: 0.0 },
                PatternSegment { x1: 0.0, y1: 1.0, x2: 3.0, y2: 1.0 },
                PatternSegment { x1: 0.0, y1: 1.0, x2: 0.0, y2: 2.0 },
                PatternSegment { x1: 1.5, y1: 0.0, x2: 1.5, y2: 1.0 },
            ],
            circles: vec![],
        },
        // TILE — decorative 4 × 4 star tile. Derived from
        //   ~/workspace/RUST_CAD/Hatch_Patten/tile pattern.dxf
        // Outer square + two full diagonals + four short half-step
        // diagonals forming the inner star. Canonical period omits the
        // top + right outer edges (drawn by the cell above / to the
        // right) to avoid double strokes.
        "TILE" => Pattern::Tile {
            period_x: 4.0,
            period_y: 4.0,
            segments: vec![
                // Outer square — bottom + left only (top + right come
                // from the neighbouring cells)
                PatternSegment { x1: 0.0, y1: 0.0, x2: 4.0, y2: 0.0 },
                PatternSegment { x1: 0.0, y1: 0.0, x2: 0.0, y2: 4.0 },
                // Two full diagonals through the cell centre
                PatternSegment { x1: 0.0, y1: 0.0, x2: 4.0, y2: 4.0 },
                PatternSegment { x1: 4.0, y1: 0.0, x2: 0.0, y2: 4.0 },
                // Four short half-step diagonals — corner triangles
                PatternSegment { x1: 0.0, y1: 2.0, x2: 2.0, y2: 4.0 },
                PatternSegment { x1: 2.0, y1: 0.0, x2: 4.0, y2: 2.0 },
                PatternSegment { x1: 4.0, y1: 2.0, x2: 2.0, y2: 4.0 },
                PatternSegment { x1: 2.0, y1: 0.0, x2: 0.0, y2: 2.0 },
            ],
            circles: vec![],
        },
        // CONCRETE — diagonal hatches both ways, looser spacing
        "CONCRETE" => Pattern::Families(vec![
            LineFamily { angle: pi / 4.0,        base_x: 0.0, base_y: 0.0, spacing: 5.0 },
            LineFamily { angle: 3.0 * pi / 4.0,  base_x: 0.0, base_y: 0.0, spacing: 5.0 },
        ]),
        // LINE — single horizontal-line family (matches AutoCAD's
        // basic "LINE" pattern). Useful as a clean baseline.
        "LINE" | "HORIZONTAL" => Pattern::Families(vec![
            LineFamily { angle: 0.0,             base_x: 0.0, base_y: 0.0, spacing: 3.175 },
        ]),
        // DOTS / GRAVEL approximation — fine perpendicular crosshatch
        // produces a dotted texture at typical zoom.
        "DOTS" => Pattern::Families(vec![
            LineFamily { angle: 0.0,             base_x: 0.0, base_y: 0.0, spacing: 1.0 },
            LineFamily { angle: pi / 2.0,        base_x: 0.0, base_y: 0.0, spacing: 1.0 },
        ]),
        // DOUBLE — two close horizontal stripes, repeating. Derived
        // from `Hatch_Patten/continues line.dxf` (two horizontal lines
        // 0.285 apart in the reference cell, here normalised to 0.5).
        // ANSI32-style: two interleaved horizontal families at the
        // same angle with offset anchors.
        "DOUBLE" => Pattern::Families(vec![
            LineFamily { angle: 0.0, base_x: 0.0, base_y: 0.0, spacing: 3.0 },
            LineFamily { angle: 0.0, base_x: 0.0, base_y: 0.5, spacing: 3.0 },
        ]),
        // DASH — dashed double-stripe pattern. Derived from
        // `Hatch_Patten/dashed line.dxf`. Tile cell: 2.0 × 1.0.
        // Two rows of dashes 1.0 long with a 1.0 gap between dashes;
        // the rows are 1.0 apart.
        "DASH" => Pattern::Tile {
            period_x: 2.0,
            period_y: 2.0,
            segments: vec![
                // Bottom-row dash
                PatternSegment { x1: 0.0, y1: 0.0, x2: 1.0, y2: 0.0 },
                // Top-row dash (one cell up)
                PatternSegment { x1: 0.0, y1: 1.0, x2: 1.0, y2: 1.0 },
            ],
            circles: vec![],
        },
        // SQGRID — a 2 × 2 grid of small squares inside one cell.
        // Derived from `Hatch_Patten/straight tile.dxf`. Period 2 × 2
        // (one big cell = four small squares). Canonical edges only
        // (no duplicate strokes when tiled).
        "SQGRID" => Pattern::Tile {
            period_x: 2.0,
            period_y: 2.0,
            segments: vec![
                // Outer bottom + left
                PatternSegment { x1: 0.0, y1: 0.0, x2: 2.0, y2: 0.0 },
                PatternSegment { x1: 0.0, y1: 0.0, x2: 0.0, y2: 2.0 },
                // Inner cross — vertical + horizontal mid-line
                PatternSegment { x1: 1.0, y1: 0.0, x2: 1.0, y2: 2.0 },
                PatternSegment { x1: 0.0, y1: 1.0, x2: 2.0, y2: 1.0 },
            ],
            circles: vec![],
        },
        // CONCENTRIC — 4 nested circles per cell. Derived from
        // `Hatch_Patten/Concentric circles.dxf`. Radii 0.25/0.5/0.75/1.0
        // (cell 2 × 2, circles centred). When tiled, gives the user's
        // "ripple"/"polka" look.
        "CONCENTRIC" => Pattern::Tile {
            period_x: 2.0,
            period_y: 2.0,
            segments: vec![],
            circles: vec![
                PatternCircle { cx: 1.0, cy: 1.0, radius: 0.25 },
                PatternCircle { cx: 1.0, cy: 1.0, radius: 0.50 },
                PatternCircle { cx: 1.0, cy: 1.0, radius: 0.75 },
                PatternCircle { cx: 1.0, cy: 1.0, radius: 1.00 },
            ],
        },
        // Unknown name → empty pattern; renderer draws nothing for this
        // hatch. The hatch dobject itself remains in the doc and the
        // user can rename to a valid pattern later.
        _ => Pattern::empty(),
    }
}

/// Catalog of every recognised pattern name. Useful for UI listings
/// (dropdown / chooser) and for tests that enumerate patterns to
/// verify every one resolves to a non-empty pattern.
pub const PATTERN_NAMES: &[&str] = &[
    "SOLID",       // sentinel — actually rendered via the Solid arm
    "ANSI31", "ANSI32", "ANSI33", "ANSI37",
    "CROSS", "NET", "ANGLE", "BRICK", "TILE",
    "CONCRETE", "EARTH", "LINE", "DOTS",
    // Added 2026-06-08 from ~/workspace/RUST_CAD/Hatch_Patten/
    "DOUBLE", "DASH", "SQGRID", "CONCENTRIC",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_pattern_resolves() {
        for name in PATTERN_NAMES {
            if *name == "SOLID" { continue; }   // SOLID is the no-line case
            let pat = lookup(name);
            assert!(!pat.is_empty(),
                "pattern '{}' resolved to empty pattern", name);
            match pat {
                Pattern::Families(fams) => {
                    for f in &fams {
                        assert!(f.spacing > 0.0,
                            "pattern '{}' has non-positive spacing", name);
                    }
                }
                Pattern::Tile { period_x, period_y, segments, circles } => {
                    assert!(period_x > 0.0 && period_y > 0.0,
                        "pattern '{}' has non-positive period", name);
                    assert!(!segments.is_empty() || !circles.is_empty(),
                        "pattern '{}' tile has no segments or circles", name);
                }
            }
        }
    }

    #[test]
    fn unknown_pattern_is_empty() {
        assert!(lookup("NO_SUCH_PATTERN").is_empty());
        assert!(lookup("").is_empty());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let a = lookup("ANSI31");
        let b = lookup("ansi31");
        let c = lookup("Ansi31");
        // All three should resolve to the same variant + same family count.
        match (a, b, c) {
            (Pattern::Families(x), Pattern::Families(y), Pattern::Families(z)) => {
                assert_eq!(x.len(), y.len());
                assert_eq!(y.len(), z.len());
            }
            _ => panic!("ANSI31 should resolve to Families"),
        }
    }

    #[test]
    fn brick_is_tile_with_4_segments() {
        match lookup("BRICK") {
            Pattern::Tile { period_x, period_y, segments, .. } => {
                assert!((period_x - 3.0).abs() < 1e-9);
                assert!((period_y - 2.0).abs() < 1e-9);
                assert_eq!(segments.len(), 4);
            }
            _ => panic!("BRICK should be a Tile pattern"),
        }
    }

    #[test]
    fn tile_has_8_segments_in_4x4_period() {
        match lookup("TILE") {
            Pattern::Tile { period_x, period_y, segments, .. } => {
                assert!((period_x - 4.0).abs() < 1e-9);
                assert!((period_y - 4.0).abs() < 1e-9);
                assert_eq!(segments.len(), 8);
            }
            _ => panic!("TILE should be a Tile pattern"),
        }
    }

    #[test]
    fn concentric_has_4_circles_no_segments() {
        match lookup("CONCENTRIC") {
            Pattern::Tile { segments, circles, .. } => {
                assert!(segments.is_empty());
                assert_eq!(circles.len(), 4);
            }
            _ => panic!("CONCENTRIC should be a Tile pattern"),
        }
    }

    // --- hatch_line_intervals (the shared even-odd clipper) ---------------

    /// Two half-disc loops SHARING the chord: the pattern line through the
    /// shared point — exactly (0,0) — must render across BOTH regions as
    /// one full interval. A single global hit list dedupes the chord's
    /// double report (one per loop) and flips the parity, so the segment
    /// across the shared point vanishes.
    #[test]
    fn shared_chord_line_through_origin_spans_full_disc() {
        let mut upper: Vec<Vec2> = Vec::new();
        for k in 0..=16 {
            let a = std::f64::consts::PI * k as f64 / 16.0;
            upper.push(Vec2::new(30.0 * a.cos(), 30.0 * a.sin()));
        }
        upper.push(Vec2::new(-30.0, 0.0));
        upper.push(Vec2::new(30.0, 0.0));   // chord back — shared edge
        let mut lower: Vec<Vec2> = Vec::new();
        for k in 16..=32 {
            let a = std::f64::consts::PI * k as f64 / 16.0;
            lower.push(Vec2::new(30.0 * a.cos(), 30.0 * a.sin()));
        }
        lower.push(Vec2::new(30.0, 0.0));
        lower.push(Vec2::new(-30.0, 0.0));  // chord back — the shared edge
        let loops = vec![upper, lower];
        let u = Vec2::new(std::f64::consts::FRAC_PI_4.cos(), std::f64::consts::FRAC_PI_4.sin());
        let intervals = hatch_line_intervals(&loops, Vec2::ZERO, u);
        assert_eq!(intervals.len(), 1,
            "the origin-crossing line fills the whole disc: {intervals:?}");
        let span = intervals[0].1 - intervals[0].0;
        assert!((span - 60.0).abs() < 1e-6,
            "span {span} — the full diagonal 2r = 60");
    }

    /// A line through a loop VERTEX is reported twice (once per adjacent
    /// edge); the per-loop dedupe must collapse it to one crossing so a
    /// single circle still yields ONE interval, not two zero-length ones.
    #[test]
    fn vertex_double_count_collapses_within_the_loop() {
        // A 64-gon circle: the 45° line passes through the 45°/225°
        // vertices exactly (64 samples → vertex at 45°).
        let circle: Vec<Vec2> = (0..64).map(|k| {
            let a = std::f64::consts::TAU * k as f64 / 64.0;
            Vec2::new(30.0 * a.cos(), 30.0 * a.sin())
        }).collect();
        let loops = vec![circle];
        let u = Vec2::new(std::f64::consts::FRAC_PI_4.cos(), std::f64::consts::FRAC_PI_4.sin());
        let intervals = hatch_line_intervals(&loops, Vec2::ZERO, u);
        assert_eq!(intervals.len(), 1, "full chord through the circle: {intervals:?}");
        let span = intervals[0].1 - intervals[0].0;
        assert!((span - 60.0).abs() < 1e-6,
            "span {span} — the full diagonal 2r = 60");
    }

    /// Nested island — even-odd must XOR the inner loop out (annulus).
    #[test]
    fn nested_island_xors_out_of_the_fill() {
        let outer: Vec<Vec2> = vec![
            Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0),
        ];
        let inner: Vec<Vec2> = (0..64).map(|k| {
            let a = std::f64::consts::TAU * k as f64 / 64.0;
            Vec2::new(5.0 + 2.0 * a.cos(), 5.0 + 2.0 * a.sin())
        }).collect();
        let loops = vec![outer, inner];
        let u = Vec2::new(1.0, 0.0);
        // The line y=5 (origin (0,5), direction +X) — crosses the outer at
        // x=0/10 and the island at x=3/7.
        let intervals = hatch_line_intervals(&loops, Vec2::new(0.0, 5.0), u);
        assert_eq!(intervals.len(), 2, "annulus along y=5: {intervals:?}");
        let (a, b) = (intervals[0], intervals[1]);
        assert!((a.1 - a.0 - 3.0).abs() < 1e-6 && (b.1 - b.0 - 3.0).abs() < 1e-6,
            "outer minus island: {a:?} {b:?}");
    }

    /// Two DISJOINT regions in one hatch stay separate — no line connects
    /// them.
    #[test]
    fn disjoint_regions_stay_separate() {
        let left: Vec<Vec2> = vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0), Vec2::new(0.0, 4.0),
        ];
        let right: Vec<Vec2> = vec![
            Vec2::new(10.0, 0.0), Vec2::new(14.0, 0.0),
            Vec2::new(14.0, 4.0), Vec2::new(10.0, 4.0),
        ];
        let loops = vec![left, right];
        let u = Vec2::new(1.0, 0.0);
        let intervals = hatch_line_intervals(&loops, Vec2::ZERO, u);
        assert_eq!(intervals.len(), 2, "{intervals:?}");
        assert!((intervals[0].1 - intervals[0].0 - 4.0).abs() < 1e-6);
        assert!((intervals[1].1 - intervals[1].0 - 4.0).abs() < 1e-6);
        assert!(intervals[1].0 - intervals[0].1 > 5.0,
            "the gap between the regions is NOT filled: {intervals:?}");
    }

    /// The GPU/plot geometry generator must produce the full disc for the
    /// divided circle (regression: the applied hatch lost the line through
    /// (0,0) because hatch_geometry used a global hit list).
    #[test]
    fn hatch_geometry_full_disc_for_two_half_discs() {
        let mut upper: Vec<Vec2> = Vec::new();
        for k in 0..=16 {
            let a = std::f64::consts::PI * k as f64 / 16.0;
            upper.push(Vec2::new(30.0 * a.cos(), 30.0 * a.sin()));
        }
        upper.push(Vec2::new(-30.0, 0.0));
        upper.push(Vec2::new(30.0, 0.0));
        let mut lower: Vec<Vec2> = Vec::new();
        for k in 16..=32 {
            let a = std::f64::consts::PI * k as f64 / 16.0;
            lower.push(Vec2::new(30.0 * a.cos(), 30.0 * a.sin()));
        }
        lower.push(Vec2::new(30.0, 0.0));
        lower.push(Vec2::new(-30.0, 0.0));
        let loops = vec![upper, lower];
        let (segs, _circs) = hatch_geometry(&loops, &lookup("ANSI31"), 1.0, 0.0);
        assert!(!segs.is_empty(), "pattern lines exist");
        // The 45° line through (0,0): midpoint at the origin, full-disc
        // length (2r along the diagonal).
        let mid_hit = segs.iter().find(|(a, b)| {
            let mid = (*a + *b) * 0.5;
            mid.len() < 0.5 && (*b - *a).len() > 55.0
        });
        assert!(mid_hit.is_some(),
            "origin-crossing line must span both half-discs (got {} segs)",
            segs.len());
    }
}

// ---------------------------------------------------------------------------
// Pattern geometry generation (shared by the plot pipeline and the app
// canvas/GPU renderers — single source so they can never drift).
// ---------------------------------------------------------------------------

use crate::math::Vec2;

/// Parametric line `o + t*u` against segment `a→b`: returns `t` when the
/// segment crosses the line within [0,1] (half-open at the end so shared
/// vertices aren't double-counted).
fn line_segment_intersect_t(o: Vec2, u: Vec2, a: Vec2, b: Vec2) -> Option<f64> {
    let d = b - a;
    let det = u.x * (-d.y) - (-d.x) * u.y;
    if det.abs() < 1e-12 { return None; }
    let rhs_x = a.x - o.x;
    let rhs_y = a.y - o.y;
    let t = (rhs_x * (-d.y) - (-d.x) * rhs_y) / det;
    let s = (u.x * rhs_y - u.y * rhs_x) / det;
    if s < -1e-9 || s > 1.0 + 1e-9 { return None; }
    Some(t)
}

/// The intervals of the pattern line `line_origin + t*u` that lie INSIDE
/// the hatch — the even-odd fill of `loops` along the line.
///
/// Pairing is done PER LOOP first (each loop's sorted hits pair
/// consecutively, after collapsing the loop's OWN vertex double-counts),
/// then the loops' intervals are XOR-combined with an odd-parity sweep.
/// A single GLOBAL hit list can't get this right:
///   * a shared edge between two loops (e.g. the chord of a circle
///     hatched as two half-discs) reports TWO hits at the same t — one
///     per loop — and collapsing them to one flips the parity, so the
///     segment across the shared point vanishes or spills into the
///     neighbour region;
///   * a line grazing a loop VERTEX reports two identical hits from the
///     adjacent edges — those must collapse to a single crossing WITHIN
///     that loop only (hits at the same t from OTHER loops — a shared
///     edge — are untouched and cancel in the XOR);
///   * nested islands XOR out (even-odd: outer minus inner).
/// The parity sweep processes interval ENDs before STARTs at the same t,
/// so touching intervals (shared edges) close and reopen — then merge.
pub fn hatch_line_intervals(loops: &[Vec<Vec2>], line_origin: Vec2, u: Vec2) -> Vec<(f64, f64)> {
    let mut intervals: Vec<(f64, f64)> = Vec::new();
    for l in loops {
        let m = l.len();
        if m < 2 { continue; }
        let mut hits: Vec<f64> = Vec::new();
        for i in 0..m {
            if let Some(t) = line_segment_intersect_t(line_origin, u, l[i], l[(i + 1) % m]) {
                hits.push(t);
            }
        }
        hits.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        // Per-loop dedupe — collapse the loop's own vertex double-counts
        // so the pairing stays in/out-alternating.
        let scale = hits.last().map(|v| v.abs()).unwrap_or(1.0).max(1.0);
        let eps = 1e-9 * scale;
        let mut w = 1;
        for i in 1..hits.len() {
            if (hits[i] - hits[w - 1]).abs() > eps {
                hits[w] = hits[i];
                w += 1;
            }
        }
        hits.truncate(w);
        let mut i = 0;
        while i + 1 < hits.len() {
            let (t0, t1) = (hits[i], hits[i + 1]);
            if (t1 - t0).abs() > 1e-6 { intervals.push((t0, t1)); }
            i += 2;
        }
    }
    if intervals.is_empty() { return intervals; }
    // XOR across loops: +1 enters a loop, -1 leaves. At equal t, ENDS
    // sort before STARTS so a shared edge closes one interval and opens
    // the next instead of nesting them. Inside iff the crossing count is
    // ODD — nested intervals (islands) push it to even, closing the fill.
    let mut events: Vec<(f64, i32)> = Vec::with_capacity(intervals.len() * 2);
    for &(s, e) in &intervals {
        events.push((s, 1));
        events.push((e, -1));
    }
    events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.1.cmp(&b.1)));
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut parity = 0i32;
    let mut start: Option<f64> = None;
    for (t, d) in events {
        parity += d;
        if parity % 2 == 1 && start.is_none() { start = Some(t); }
        else if parity % 2 == 0 {
            if let Some(s) = start.take() { out.push((s, t)); }
        }
    }
    // Merge intervals that touch exactly (shared-edge pairs produce
    // adjacent intervals meeting at the shared point).
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (s, e) in out {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 + 1e-6 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    merged
}

/// Generate the WORLD-space line segments and (centre, radius) circles for
/// a hatch pattern over the given boundary loops. `user_scale` multiplies
/// the pattern's spacings/coordinates; `user_angle_deg` rotates the whole
/// pattern about the origin. Family patterns clip infinite lines against
/// the loops with even-odd; tile patterns clip each in-cell segment and
/// clamp visible intervals to the segment's own length. Runaway guards
/// (10 000 lines per family, 200 000 tiles) bound the work.
pub fn hatch_geometry(
    loops: &[Vec<Vec2>],
    pattern: &Pattern,
    user_scale: f64,
    user_angle_deg: f64,
) -> (Vec<(Vec2, Vec2)>, Vec<(Vec2, f64)>) {
    let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
    let mut circs: Vec<(Vec2, f64)> = Vec::new();
    if pattern.is_empty() { return (segs, circs); }
    // Union bbox of all loops in world coords.
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for l in loops {
        for v in l {
            if v.x < min.x { min.x = v.x; }
            if v.y < min.y { min.y = v.y; }
            if v.x > max.x { max.x = v.x; }
            if v.y > max.y { max.y = v.y; }
        }
    }
    if !min.x.is_finite() || !max.x.is_finite() { return (segs, circs); }
    let user_angle = user_angle_deg.to_radians();
    match pattern {
        Pattern::Families(families) => {
            for fam in families {
                // Effective angle + spacing after the user transform.
                let theta = fam.angle + user_angle;
                let spacing = fam.spacing * user_scale.abs().max(1e-9);
                let cos = theta.cos();
                let sin = theta.sin();
                let u = Vec2::new(cos, sin);
                let n = Vec2::new(-sin, cos);
                let corners = [
                    Vec2::new(min.x, min.y), Vec2::new(max.x, min.y),
                    Vec2::new(min.x, max.y), Vec2::new(max.x, max.y),
                ];
                let base = Vec2::new(fam.base_x, fam.base_y);
                let mut s_min = f64::INFINITY;
                let mut s_max = f64::NEG_INFINITY;
                for c in &corners {
                    let s = (*c - base).dot(n);
                    if s < s_min { s_min = s; }
                    if s > s_max { s_max = s; }
                }
                // First line at s = ceil(s_min / spacing) * spacing.
                let mut s = (s_min / spacing).ceil() * spacing;
                let line_count_estimate = ((s_max - s_min) / spacing).ceil();
                if line_count_estimate > 10_000.0 { continue; }
                while s <= s_max + 1e-9 {
                    let line_origin = base + n * s;
                    // Per-loop pairing + XOR across loops (see
                    // hatch_line_intervals) — a single global hit list
                    // corrupts the even-odd at shared edges (a circle
                    // hatched as two half-discs loses the line across
                    // the shared chord) and at vertex grazes.
                    for (t0, t1) in hatch_line_intervals(loops, line_origin, u) {
                        segs.push((line_origin + u * t0, line_origin + u * t1));
                    }
                    s += spacing;
                }
            }
        }
        Pattern::Tile { period_x, period_y, segments, circles } => {
            let s = user_scale.abs().max(1e-9);
            let px = period_x * s;
            let py = period_y * s;
            if px < 1e-9 || py < 1e-9 { return (segs, circs); }
            let cos = user_angle.cos();
            let sin = user_angle.sin();
            // Cells in the pattern frame covering the world bbox (invert
            // the user rotation on each bbox corner).
            let corners_world = [
                Vec2::new(min.x, min.y), Vec2::new(max.x, min.y),
                Vec2::new(max.x, max.y), Vec2::new(min.x, max.y),
            ];
            let mut pmin = Vec2::new(f64::INFINITY, f64::INFINITY);
            let mut pmax = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            for c in &corners_world {
                let px_w = c.x * cos + c.y * sin;
                let py_w = -c.x * sin + c.y * cos;
                if px_w < pmin.x { pmin.x = px_w; }
                if py_w < pmin.y { pmin.y = py_w; }
                if px_w > pmax.x { pmax.x = px_w; }
                if py_w > pmax.y { pmax.y = py_w; }
            }
            let i0 = (pmin.x / px).floor() as i64 - 1;
            let i1 = (pmax.x / px).ceil()  as i64 + 1;
            let j0 = (pmin.y / py).floor() as i64 - 1;
            let j1 = (pmax.y / py).ceil()  as i64 + 1;
            // Safety cap — millions of cells would freeze the caller.
            let tile_count = (i1 - i0).max(0) * (j1 - j0).max(0);
            if tile_count > 200_000 { return (segs, circs); }
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
                        if seg_len2 < 1e-18 { continue; }
                        // Clip against the loops with the same per-loop +
                        // XOR machinery as the family lines, then clamp the
                        // resulting intervals to the segment's own t-range
                        // [0,1]. The XOR runs over the infinite line, so a
                        // segment starting/ending INSIDE a loop is handled
                        // by intervals that span past the clamp window.
                        let intervals = hatch_line_intervals(loops, a, dvec);
                        for (t0, t1) in intervals {
                            let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
                            if t1 - t0 > 1e-6 {
                                segs.push((a + dvec * t0, a + dvec * t1));
                            }
                        }
                    }
                    for c in circles {
                        let cx_p = ox + c.cx * s;
                        let cy_p = oy + c.cy * s;
                        let cx = cx_p * cos - cy_p * sin;
                        let cy = cx_p * sin + cy_p * cos;
                        circs.push((Vec2::new(cx, cy), c.radius * s));
                    }
                }
            }
        }
    }
    (segs, circs)
}
