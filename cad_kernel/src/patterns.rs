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
}
