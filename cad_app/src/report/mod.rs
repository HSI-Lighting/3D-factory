//! Report generation — the calculation as a document a person can issue.
//!
//! ONE LAYOUT, TWO DESTINATIONS. [`layout`] turns the numbers into pages of positioned items;
//! [`pdf`] writes those pages to a file, and the dialog paints the same pages on screen as its
//! preview. That is why the preview is worth trusting: it is not an impression of the report, it
//! is the report, drawn with a different painter.
//!
//! HTML keeps its own renderer ([`crate::light_report`]) because HTML is a flow format — it
//! reflows to the window and has no pages — and pretending otherwise would give a worse document
//! than letting it do what it is good at.

pub mod layout;
pub mod ui;
pub mod options;
pub mod pdf;

pub use options::{Format, Options, PageSize, ReportImage, Section};
