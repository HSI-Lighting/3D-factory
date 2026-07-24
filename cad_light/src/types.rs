//! Shared 3D + photometric types for the lighting engine.
//!
//! Geometry is `glam` f32 (ample precision at room scale, in metres). Photometric
//! scalars (candela, lux, lumens) are `f64`.
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub type MaterialId = u32;

/// A 3D vertex (metres).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vertex {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    #[inline]
    pub fn to_vec3(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

/// A mesh face, indexing three vertices of its parent [`Mesh`].
#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

/// A triangulated surface with a Lambertian material.
#[derive(Debug, Clone)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<Triangle>,
    pub material: MaterialId,
}

/// A Lambertian surface material.
#[derive(Debug, Clone)]
#[derive(Serialize, Deserialize)]
pub struct Material {
    pub id: MaterialId,
    pub name: String,
    /// Diffuse reflectance, 0.0–1.0.
    pub reflectance: f32,
    /// Linear RGB tint, 0.0–1.0 per channel.
    pub color: [f32; 3],
}

/// Sensible room defaults: floor 0.20, walls 0.50, ceiling 0.70.
pub fn default_materials() -> Vec<Material> {
    vec![
        Material { id: 0, name: "Floor".into(), reflectance: 0.20, color: [0.6, 0.6, 0.6] },
        Material { id: 1, name: "Wall".into(), reflectance: 0.50, color: [0.8, 0.8, 0.8] },
        Material { id: 2, name: "Ceiling".into(), reflectance: 0.70, color: [0.9, 0.9, 0.9] },
    ]
}

/// A luminaire placed in the scene: an IES profile at a pose.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Luminaire {
    pub id: u32,
    /// Key into the profile table.
    pub profile: String,
    pub position: Vertex,
    /// Rotation about the vertical axis (degrees).
    pub rotation_deg: f32,
    /// Dimming / output scale, 0.0–1.0.
    pub dimming: f32,
}

/// The horizontal work plane on which illuminance is sampled. A `rows x cols`
/// grid spans `width` (x) by `depth` (y), anchored at `origin` (min corner), at
/// height `origin.z`. Engine world is Z-up.
#[derive(Debug, Clone, Copy)]
pub struct CalcPlane {
    pub origin: Vertex,
    pub width: f32,
    pub depth: f32,
    pub cols: u32,
    pub rows: u32,
}

impl CalcPlane {
    /// World position of the sensor at cell `(col, row)`, taken at its centre.
    pub fn sample_point(&self, col: u32, row: u32) -> Vertex {
        let dx = self.width / self.cols.max(1) as f32;
        let dy = self.depth / self.rows.max(1) as f32;
        Vertex::new(
            self.origin.x + (col as f32 + 0.5) * dx,
            self.origin.y + (row as f32 + 0.5) * dy,
            self.origin.z,
        )
    }
}

/// Ray-tracing / radiosity controls.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RaySettings {
    /// Hemisphere samples per point for the indirect pass.
    pub rays_per_point: u32,
    /// Maximum number of diffuse bounces (0 = direct only).
    pub max_bounces: u32,
    /// Whether to cast shadow rays against scene geometry.
    pub shadows: bool,
}

impl Default for RaySettings {
    fn default() -> Self {
        Self { rays_per_point: 64, max_bounces: 1, shadows: true }
    }
}

/// A computed illuminance grid (lux) over a [`CalcPlane`], row-major.
#[derive(Debug, Clone)]
pub struct LuxGrid {
    pub cols: u32,
    pub rows: u32,
    pub values: Vec<f64>,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

impl LuxGrid {
    /// Build a grid from raw row-major lux values, computing summary stats.
    pub fn from_values(cols: u32, rows: u32, values: Vec<f64>) -> Self {
        let (mut min, mut max, mut sum) = (f64::INFINITY, f64::NEG_INFINITY, 0.0);
        for &v in &values {
            min = min.min(v);
            max = max.max(v);
            sum += v;
        }
        let n = values.len().max(1) as f64;
        LuxGrid {
            cols,
            rows,
            min: if min.is_finite() { min } else { 0.0 },
            max: if max.is_finite() { max } else { 0.0 },
            avg: sum / n,
            values,
        }
    }
}
