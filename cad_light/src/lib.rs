//! cad_light — a physically-based lighting (lux) engine for the SIMLUX app.
//!
//! UI-agnostic pure Rust: IES LM-63 photometry, a ray-traced radiosity engine,
//! and an extruder that turns a `cad_kernel::Document` into 3D surfaces. The
//! egui app drives it; nothing here depends on egui/eframe.
//!
//! Pipeline: `Document` --extrude(height)--> `Vec<Mesh>`; place `Luminaire`s with
//! `IesProfile`s; `calc::calculate(...)` --> `LuxGrid` (paint it on the plan).
pub mod calc;
pub mod extrude;
pub mod ies;
pub mod rt;
pub mod types;

pub use calc::calculate;
pub use extrude::{bbox, box_room, extrude, extrude_handles, triangulate};
pub use ies::{parse as parse_ies, IesProfile, PhotometryType};
pub use types::{
    default_materials, CalcPlane, LuxGrid, Luminaire, Material, MaterialId, Mesh, RaySettings,
    Triangle, Vertex,
};

use std::collections::HashMap;

/// Convenience one-shot: extrude the document, size a work-plane grid over its
/// footprint at `plane_height`, and compute the lux grid. Returns `None` if the
/// document has no geometry.
pub fn calculate_document(
    doc: &cad_kernel::Document,
    height: f32,
    plane_height: f32,
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    materials: &[Material],
    settings: &RaySettings,
) -> Option<(Vec<Mesh>, CalcPlane, LuxGrid)> {
    let (min_x, min_y, max_x, max_y) = bbox(doc)?;
    let (w, d) = (max_x - min_x, max_y - min_y);
    let cols = ((w / 0.2).round() as u32).clamp(8, 48);
    let rows = ((d / 0.2).round() as u32).clamp(8, 48);
    let plane = CalcPlane { origin: Vertex::new(min_x, min_y, plane_height), width: w, depth: d, cols, rows };
    let meshes = extrude(doc, height);
    let grid = calculate(&meshes, luminaires, profiles, materials, &plane, settings);
    Some((meshes, plane, grid))
}
