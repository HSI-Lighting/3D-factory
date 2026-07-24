//! cad_raster — raster → vector conversion subsystem for RUST_CAD.
//!
//! NOT a one-shot auto-vectorizer. A **human-in-the-loop midware**: the app
//! proposes, the human confirms, and the raster is **peeled layer by layer**
//! (text → dimensions → furniture → structure → user) so each step is easy.
//! See `RASTER_TO_VECTOR.md` at the project root for the full design.
//!
//! Layering: this crate is the PURE logic (raster doc + adjustments + analyzer
//! + detection/trace engines) — no UI. The interactive editor (raster layers
//! panel, mask brush, convert→CAD-layer action) lives in `cad_app`, isolated
//! like the Block Editor. Data types come from `cad_kernel`.
//!
//! Brought-in library: `image` (pure-Rust, MIT/Apache) for decode + grayscale
//! + pixel access. The high-level analysis / tracing is ours.

pub mod adjust;
pub mod analyze;
pub mod doc;
pub mod fit;
pub mod trace;

pub use adjust::{AdjustKind, Adjustment};
pub use analyze::{analyze, RasterClass, Report};
pub use doc::{LayerKind, Mask, RasterDoc, RasterLayer};
pub use fit::{trace_layer, FitKind, TraceParams};
pub use trace::{convert, AssetKind};
