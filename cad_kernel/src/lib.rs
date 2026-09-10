//! cad_kernel — pure-Rust 2D CAD geometry kernel.
//!
//! Zero UI dependencies. Designed so the math is independently verifiable:
//! every intersection function is a free `fn` with `#[cfg(test)]` coverage,
//! and the `cad_cli` binary lets a human pipe commands in and inspect the
//! intersection output line-by-line.
//!
//! Modules:
//! - [`math`]      — `Vec2`, `EPS`, angle helpers
//! - [`geom`]      — `Line`, `Circle`, `Arc`, `Ellipse`, `EllipseArc`, `Geom` enum
//! - [`color`]     — `Color` enum (ByLayer / ByBlock / Aci / TrueColor) + resolution
//! - [`lineweight`]— `Lineweight` enum + resolution
//! - [`linetype`]  — named dash/gap patterns + `LinetypeTable`
//! - [`layer`]     — `Layer` + `LayerTable` (with reserved layer "0")
//! - [`style`]     — `Style` struct (layer + color + linetype + lineweight + visibility)
//! - [`dobject`]   — `DObject` struct = geometry + style + handle
//! - [`document`]  — `Document` container (Dobjects + tables); RUST_CAD's `AcDbDatabase` analog
//! - [`intersect`] — pairwise intersection on `Geom` + dispatcher
//! - [`spatial`]   — uniform-grid spatial index over `&[DObject]`
//! - [`snap`]      — object-snap engine (END/MID/CEN/QUA/INT/PER/TAN/NEA)
//! - [`parser`]    — command-line grammar
//! - [`construct`] — constructors (arc-from-three-points, ellipse-from-center, …)

pub mod block;
pub mod blockdiff;
pub mod color;
pub mod construct;
pub mod dedupe;
pub mod dim;
pub mod dobject;
pub mod document;
pub mod fillet;
pub mod geom;
pub mod hatch_resolve;
pub mod intersect;
pub mod join;
pub mod layer;
pub mod layout;
pub mod laystate;
pub mod linetype;
pub mod lineweight;
pub mod math;
pub mod modify;
pub mod mtext;
pub mod pagesetup;
pub mod parser;
pub mod pen;
pub mod plotstyle;
pub mod purge;
pub mod snap;
pub mod spatial;
pub mod style;
pub mod table;
pub mod text;
pub mod trim;
pub mod ucs;
pub mod units;
pub mod vector_primitive;
pub mod wallstyle;
pub mod xref;

// Convenience re-exports
pub use block::{AttrDef, Block, BlockParam, BlockRef, BlockTable, ParamVector, MAX_BLOCK_PARAMS};
pub use blockdiff::{diff_blocks, BlockDiff, ParamCluster};
pub use dim::{Dim, DimKind, DimRenderGeometry, DimStyle, DimStyleTable, LinearOrtho};
pub use geom::{
    point_in_polygon, Arc, CenterMark, Circle, Donut, Ellipse, EllipseArc, Geom, Hatch,
    HatchPattern, Line, Point, PolyVertex, Polyline, Ray, Region, Spline, Wall, Wipeout, Xline,
};
pub use hatch_resolve::resolve_hatch_loops;
pub use join::{bulge_arc, bulge_from_arc};
pub use math::{approx_eq, approx_zero, norm_angle, Vec2, EPS};
pub use table::Table;
pub use text::{
    HAlign as TextHAlign, Leader, Text, TextListKind, TextStyle, TextStyleTable,
    VAlign as TextVAlign,
};
pub use vector_primitive::VectorPrimitive;
pub use wallstyle::{WallStyle, WallStyleTable};
pub use xref::Xref;
pub mod patterns;
pub use color::{aci_palette, resolve_color, Color, TrueColorTable};
pub use construct::{
    arc_center_start_end, arc_chord_length, arc_chord_radius, arc_three_points,
    ellipse_center_major_minor, wall_sides,
};
pub use dobject::{next_handle, reserve_handles_above, DObject, Handle};
pub use document::{Document, RasterImage};
pub use fillet::{
    chamfer_geoms, chamfer_polyline_all, chamfer_polyline_corner, fillet_geoms,
    fillet_polyline_all, fillet_polyline_corner, nearest_polyline_segment,
};
pub use geom::GripRole;
pub use intersect::intersect;
pub use join::{join_geoms, JoinOut};
pub use layer::{Layer, LayerId, LayerTable};
pub use layout::{Layout, LayoutCamera, ViewportData, ViewportGeom};
pub use linetype::{Linetype, LinetypeTable};
pub use lineweight::{resolve_lineweight, Lineweight, DEFAULT_LINEWEIGHT_MM};
pub use modify::{chamfer_lines, fillet_lines, ChamferOut, FilletOut};
pub use parser::{parse, Command, ToolKind};
pub use pen::{Pen, PenTable};
pub use plotstyle::{
    plot_width_mm, EndStyle, FillStyle, JoinStyle, Offset, Orientation, PaperSize, PenNum,
    PlotArea, PlotColor, PlotConfig, PlotLinetype, PlotScale, PlotStyle, PlotStyleTable,
    PlotTarget, PlotWidth, AUTOCAD_LADDER,
};
pub use snap::{find_all_snaps, find_snap, snap_to, SnapHit, SnapKind, SnapSet};
pub use spatial::UniformGrid;
pub use style::Style;
pub use trim::join_trim_survivors;
pub use units::{AngleFormat, LengthFormat, UnitSource, Units, INSERT_UNITS};
