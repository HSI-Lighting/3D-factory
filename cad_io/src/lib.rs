//! cad_io — Document persistence (DXF round-trip + .rsm binary format).
//!
//! Two formats live here:
//!
//! - **DXF** (`dxf::read_dxf` / `dxf::write_dxf`) — ASCII interchange with
//!   AutoCAD / LibreCAD / FreeCAD / etc. Round-trips LINE / CIRCLE / ARC /
//!   ELLIPSE / POINT / LWPOLYLINE; layers and linetypes are populated from
//!   the TABLES section on read and emitted on write.
//! - **RSM** (`rsm::read_rsm` / `rsm::write_rsm`) — RUST_CAD's own binary
//!   format. Faster, smaller, lossless. Versioned header so future schema
//!   changes can refuse mismatched files cleanly.
//!
//! Both APIs take/return a `cad_kernel::Document`. They do NOT touch
//! filesystem, threading, or UI — pass in / receive a `&[u8]` (RSM) or a
//! `&str` (DXF) and let the caller handle I/O.

pub mod dxf;
pub mod pat;
pub mod rsm;

pub use pat::{parse_pat, PatLine, PatParse, PatPattern};
