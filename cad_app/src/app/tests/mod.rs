//! The app's test modules, evacuated out of `app/mod.rs` (2026-09 — app.rs
//! split) so the working file stops carrying ~24k lines of tests. Each file is
//! a child of `app` (`use super::super::*` at its top), so the moved modules
//! still read `CadApp`'s private fields exactly as they did as direct children
//! of `app`.
//!
//! Grouping is by the domain under test, mirroring the `app/*` module map:
//! factory/3D, SIMLUX/lighting, UI, io, modify, commands, scripting, and misc.

#[cfg(test)]
mod tests_scripts;

#[cfg(test)]
mod tests_commands;

#[cfg(test)]
mod tests_ui;
