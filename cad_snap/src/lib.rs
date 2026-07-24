//! # cad_snap — Object-snap engine for 2D CAD
//!
//! Pure-Rust, UI-agnostic engine that finds CAD snap points from a cursor
//! position and a slice of dobjects. Built around the same primitives a CAD
//! user already knows: END, MID, CEN, QUA, INT, PER, TAN, NEA.
//!
//! This crate is a thin facade over [`cad_kernel::snap`] — re-exported here
//! with all the geometry types snap consumers need, so you only have to
//! depend on a single crate.
//!
//! ## Quick start
//!
//! ```
//! use cad_snap::{find_snap, DObject, Line, SnapKind, SnapSet, Vec2};
//!
//! // Each Dobject is a geometry + style + handle. `Line { … }.into()` builds
//! // one with default style — exactly what you want for ad-hoc tests.
//! let dobjects: Vec<DObject> = vec![
//!     Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }.into(),
//! ];
//!
//! let mut snaps = SnapSet::default();
//! snaps.end = true;
//!
//! // Cursor near the left endpoint, with a 1.0 world-unit search radius.
//! let hit = find_snap(
//!     /* cursor   */ Vec2::new(0.1, 0.1),
//!     /* radius   */ 1.0,
//!     /* enabled  */ snaps,
//!     /* forced   */ None,    // typed override; supersedes `enabled`
//!     /* anchor   */ None,    // first click of a draw (for PER/TAN)
//!     /* dobjects */ &dobjects,
//!     /* grid     */ None,    // optional spatial index — pass None for small drawings
//! ).unwrap();
//!
//! assert_eq!(hit.kind, SnapKind::End);
//! assert_eq!(hit.point, Vec2::new(0.0, 0.0));
//! ```
//!
//! ## DObject vs Geom
//!
//! `DObject` is the full drafting object — geometry + style (layer, color,
//! linetype, lineweight, visibility) + a stable handle. `Geom` is the inner
//! enum that holds only the geometric shape (`Line`, `Circle`, `Arc`,
//! `Ellipse`, `EllipseArc`, and future variants).
//!
//! `find_snap` takes `&[DObject]` so the returned hit can carry an index back
//! into your storage. Pure-geometry helpers ([`perpendicular_extended`],
//! [`tangent_points_extended`], [`snap_to`]) take `&Geom` directly — when
//! calling them on a Dobject, pass `&dobj.geom`.
//!
//! ## What the engine returns
//!
//! Every successful lookup returns a [`SnapHit`]:
//!
//! ```text
//! SnapHit {
//!     kind:             SnapKind,        // which snap matched
//!     point:            Vec2,            // where to commit the click
//!     dobject:          Option<usize>,   // index into `dobjects`
//!     extension_anchor: Option<Vec2>,    // for PER/TAN on extensions
//! }
//! ```
//!
//! - `point` is where your click should land — render the snap marker there.
//! - `dobject` tells you which dobject the snap is reading from (useful for
//!   highlighting it in your UI).
//! - `extension_anchor` is set when the snap point lies *outside* the
//!   dobject's visible range — e.g. a PER foot past a segment endpoint, or
//!   past an arc's swept-angle boundary. Draw a dashed indicator from this
//!   anchor to `point` to show the "imaginary extension" cue every CAD user
//!   expects.
//!
//! ## The eight snap kinds
//!
//! | Kind | Line | Arc | Circle | Activation |
//! |---|---|---|---|---|
//! | **END** | endpoints | endpoints | — | cursor near point |
//! | **MID** | midpoint  | angular midpoint | — | cursor near point |
//! | **CEN** | — | centre | centre | cursor on curve |
//! | **QUA** | — | quadrants in swept range | 4 cardinal points | cursor near point |
//! | **INT** | pairwise intersections | pairwise | pairwise | cursor near point |
//! | **PER** | perp foot, real or extension | 2 feet on circle, real or extension | 2 feet, both real | cursor on curve |
//! | **TAN** | (perp foot fallback) | 2 tangents | 2 tangents | cursor on curve |
//! | **NEA** | projection on segment | clamped to swept range | projection on circle | cursor on curve |
//!
//! Priority when multiple kinds match: **END > MID > CEN > QUA > INT > PER > TAN > NEA**.
//!
//! ## Plugging into your UI
//!
//! Each frame:
//!
//! 1. Convert your cursor's screen position to world coordinates.
//! 2. Compute `world_radius = search_pixels / scale`.
//! 3. Call [`find_snap`] for the default hit, or [`find_all_snaps`] if you
//!    want **Tab cycling** between alternatives.
//! 4. Render the marker at `hit.point`. If `hit.extension_anchor.is_some()`,
//!    draw a dashed line/arc from that anchor to the point.
//! 5. On click, commit `hit.point` instead of the raw cursor position.
//!
//! Typed override (the user types `per`, `end`, etc. in your command line):
//! pass `Some(SnapKind::Per)` as the `forced` argument — it bypasses the
//! `enabled` set for one click.
//!
//! ## Scaling
//!
//! For drawings with millions of dobjects, pass a [`UniformGrid`] as the
//! optional spatial index. It bounds candidate-set size to "dobjects near
//! the cursor" instead of all dobjects. Build it once after each edit:
//!
//! ```
//! use cad_snap::{UniformGrid, DObject, Line, Vec2};
//!
//! let dobjects: Vec<DObject> = vec![
//!     Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }.into(),
//! ];
//! let cell_size = UniformGrid::auto_cell_size(&dobjects, 10.0);
//! let grid = UniformGrid::build(&dobjects, cell_size);
//! // pass `Some(&grid)` to find_snap / find_all_snaps thereafter
//! ```
//!
//! ## What this crate is NOT
//!
//! - **Not a renderer.** It returns geometry; you draw.
//! - **Not opinionated about input.** Pass world-space cursor positions; the
//!   crate has no notion of screen pixels or mouse devices.
//! - **Not a parser.** If you want to accept typed snap keywords from a
//!   command line, call [`SnapKind::parse`] on the token and pass the
//!   result as `forced`.

// Re-exports — single import surface for consumers.
pub use cad_kernel::snap::{
    find_all_snaps,
    find_snap,
    perpendicular_extended,
    perpendicular_from,
    snap_to,
    tangent_points_extended,
    SnapHit,
    SnapKind,
    SnapSet,
};
pub use cad_kernel::{
    Arc,
    Circle,
    DObject,
    Ellipse,
    EllipseArc,
    Geom,
    Line,
    UniformGrid,
    Vec2,
    EPS,
};
