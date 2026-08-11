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

/// What the installation costs to run, as distinct from what it delivers.
///
/// Every number here was already sitting in the parsed photometric files — `watts` and `lumens`
/// are read from both IES and EULUMDAT and were never used for anything. Power density is the
/// figure building regulations are written in terms of, and the one a lighting scheme is rejected
/// on after it has passed on lux.
#[derive(Debug, Clone, Copy, Default)]
pub struct Installation {
    /// Fixtures counted (those whose profile resolves).
    pub count: usize,
    /// CONNECTED LOAD (W) — the full installed wattage, not reduced by dimming. Dimming reduces
    /// consumption but not what the circuit must be sized for, and the two are not proportional.
    pub total_watts: f64,
    /// Installed luminous flux (lm), likewise undimmed.
    pub total_lumens: f64,
    /// Floor area the density is taken over (m²).
    pub area_m2: f64,
    /// W/m² — connected load over floor area.
    pub power_density: f64,
    /// Installation efficacy (lm/W).
    pub efficacy: f64,
    /// Fixtures whose profile carries no wattage, and so contribute nothing to the load.
    ///
    /// Reported rather than assumed away: a manufacturer file with no `watts` is common, and a
    /// power density computed from half the fixtures is worse than none at all.
    pub missing_watts: usize,
    /// Fixtures whose profile declares no rated flux.
    pub missing_lumens: usize,
}

/// Summarise the connected load of `luminaires` over a floor area.
pub fn installation_summary(
    luminaires: &[Luminaire],
    profiles: &std::collections::HashMap<String, crate::ies::IesProfile>,
    area_m2: f64,
) -> Installation {
    let mut s = Installation { area_m2, ..Default::default() };
    for l in luminaires {
        let Some(p) = profiles.get(&l.profile) else {
            continue; // an unassigned point is not a fixture yet
        };
        s.count += 1;
        if p.watts > 0.0 {
            s.total_watts += p.watts;
        } else {
            s.missing_watts += 1;
        }
        if p.lumens > 0.0 {
            s.total_lumens += p.lumens;
        } else {
            s.missing_lumens += 1;
        }
    }
    if area_m2 > 0.0 {
        s.power_density = s.total_watts / area_m2;
    }
    if s.total_watts > 0.0 {
        s.efficacy = s.total_lumens / s.total_watts;
    }
    s
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
    /// Five bounces, not one.
    ///
    /// One bounce was never a judgement about accuracy — it was what the old branching solver could
    /// afford, since its cost went up by roughly forty times per bounce. But a room does not stop
    /// reflecting after one pass: in a 4 × 4 × 3 m box at ρ = 0.5 the series runs
    /// 78 → 110 → 130 → 139 → 143 → 145 → 146 lx, so stopping at one bounce reports **three
    /// quarters** of the light that is actually there. That is a bigger error than the maintenance
    /// factor, and in the opposite direction.
    ///
    /// Five lands within about 2% of the converged answer and, now that the cost is linear in
    /// bounces rather than exponential, costs under twice what one bounce did. Existing projects
    /// keep whatever they were saved with — a result already issued should not change because the
    /// default moved.
    fn default() -> Self {
        Self { rays_per_point: 64, max_bounces: 5, shadows: true }
    }
}

/// The MAINTENANCE FACTOR, as its four sub-factors (EN 12464-1 / CIE 97).
///
/// A lighting scheme is judged on the illuminance it still delivers at the END of the maintenance
/// cycle, not on the day it is switched on. Lamps decay, luminaires collect dust, and room surfaces
/// darken — so every illuminance a designer quotes is a MAINTAINED illuminance, and this factor is
/// how the initial calculation becomes one.
///
/// This engine computed initial illuminance and reported it as though it were maintained, which
/// overstates a design by the whole of this factor — typically 20%. A scheme that passes at initial
/// can fail at maintained, so this is not a refinement: it is the difference between a result that
/// can be submitted and one that cannot.
///
/// Kept as the four sub-factors rather than one number because that is how they are sourced, and
/// because a reviewer will ask which assumption produced the total:
///
/// * `llmf` — **lamp lumen maintenance factor**: the output left after the operating interval.
/// * `lsf`  — **lamp survival factor**: the fraction still lit. 1.0 for LED with spot replacement.
/// * `lmf`  — **luminaire maintenance factor**: dirt on the optic, set by the cleaning interval and
///   the room's cleanliness classification.
/// * `rsmf` — **room surface maintenance factor**: the room's own surfaces darkening, which matters
///   through the interreflected component.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Maintenance {
    pub llmf: f64,
    pub lsf: f64,
    pub lmf: f64,
    pub rsmf: f64,
}

impl Maintenance {
    /// No maintenance allowance — the INITIAL condition. Correct for "what will this look like on
    /// day one", wrong for anything that gets submitted.
    pub const INITIAL: Maintenance = Maintenance { llmf: 1.0, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };

    /// The overall factor: the product of the four.
    pub fn factor(&self) -> f64 {
        (self.llmf * self.lsf * self.lmf * self.rsmf).clamp(0.0, 1.0)
    }
}

impl Default for Maintenance {
    /// A clean interior — an office or similar — on a three-year cleaning interval, giving an
    /// overall factor of **0.80**, which is the figure most schemes are quoted at.
    ///
    /// These are a defensible STARTING POINT, not an answer. The real `llmf` comes from the
    /// luminaire's own data sheet at the chosen operating hours, and `lmf`/`rsmf` from the room's
    /// cleanliness classification and cleaning interval in CIE 97. Anything being submitted should
    /// have all four set from those sources; what must never happen again is the factor silently
    /// being 1.0.
    fn default() -> Self {
        Self { llmf: 0.95, lsf: 1.00, lmf: 0.90, rsmf: 0.94 }
    }
}

/// A computed illuminance grid (lux) over a [`CalcPlane`], row-major.
///
/// `values` are MAINTAINED lux when `maintenance` is below 1.0 — the factor has already been
/// applied to every cell, so nothing downstream needs to remember to apply it, and no two readers
/// can disagree about whether it has been.
#[derive(Debug, Clone)]
pub struct LuxGrid {
    pub cols: u32,
    pub rows: u32,
    pub values: Vec<f64>,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    /// The maintenance factor already applied to `values`. 1.0 = initial illuminance.
    pub maintenance: f64,
    /// Per-cell DIRECT component, same order as `values`; empty when not computed.
    ///
    /// The split answers a question no summary statistic can: whether a room is lit by its
    /// luminaires or by its own walls. A scheme carried by interreflection is the one that collapses
    /// when a client repaints in a darker colour, and nothing in min/avg/max hints at it.
    pub direct: Vec<f64>,
    /// Per-cell INDIRECT (interreflected) component; empty when not computed.
    pub indirect: Vec<f64>,
}

impl LuxGrid {
    /// Build a grid from raw row-major lux values, computing summary stats.
    ///
    /// Values are taken as-is, so this is the INITIAL condition (`maintenance` = 1.0) with no
    /// direct/indirect split. [`LuxGrid::from_parts`] is the full form.
    pub fn from_values(cols: u32, rows: u32, values: Vec<f64>) -> Self {
        Self::from_parts(cols, rows, values, Vec::new(), Vec::new(), 1.0)
    }

    /// Build a grid from values that already carry `maintenance`, plus the optional split.
    pub fn from_parts(
        cols: u32,
        rows: u32,
        values: Vec<f64>,
        direct: Vec<f64>,
        indirect: Vec<f64>,
        maintenance: f64,
    ) -> Self {
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
            maintenance,
            direct,
            indirect,
            values,
        }
    }

    /// Overall uniformity `U₀ = Emin / Eavg` — the ratio EN 12464-1 sets a limit on.
    pub fn u0(&self) -> f64 {
        if self.avg > 0.0 { self.min / self.avg } else { 0.0 }
    }

    /// Diversity `U₁ = Emin / Emax`, the stricter ratio used for sports and road lighting.
    ///
    /// Always ≤ `u0`, since the average cannot exceed the maximum. Reported alongside it because a
    /// room can hold a respectable U₀ while still having one corner far brighter than the rest.
    pub fn u1(&self) -> f64 {
        if self.max > 0.0 { self.min / self.max } else { 0.0 }
    }

    /// The `p`-th percentile of the cell values, `p` in 0..=100 (linear interpolation).
    ///
    /// The average hides the shape of a distribution: 500 lx average is a different room when the
    /// 10th percentile is 450 than when it is 150. Percentiles are free here — the grid has kept
    /// every cell all along.
    pub fn percentile(&self, p: f64) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        let mut v = self.values.clone();
        v.sort_by(f64::total_cmp);
        let rank = (p.clamp(0.0, 100.0) / 100.0) * (v.len() - 1) as f64;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        if lo == hi {
            v[lo]
        } else {
            v[lo] + (v[hi] - v[lo]) * (rank - lo as f64)
        }
    }

    /// The median cell value.
    pub fn median(&self) -> f64 {
        self.percentile(50.0)
    }

    /// Share of the average illuminance that arrives DIRECTLY from the luminaires, 0..1.
    ///
    /// `None` when the split was not computed.
    pub fn direct_fraction(&self) -> Option<f64> {
        if self.direct.is_empty() || self.direct.len() != self.values.len() {
            return None;
        }
        let d: f64 = self.direct.iter().sum();
        let t: f64 = self.values.iter().sum();
        (t > 0.0).then(|| (d / t).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod metric_tests {
    use super::*;
    use crate::ies::{IesProfile, PhotometryType};
    use std::collections::HashMap;

    fn grid(values: Vec<f64>) -> LuxGrid {
        LuxGrid::from_values(values.len() as u32, 1, values)
    }

    /// The four sub-factors multiply, and the initial condition is exactly 1.0 — not 0.999 from
    /// some default creeping in, which would make "initial" a subtly different number every time.
    #[test]
    fn the_maintenance_factor_is_the_product_of_its_parts() {
        assert_eq!(Maintenance::INITIAL.factor(), 1.0);
        let m = Maintenance { llmf: 0.9, lsf: 0.95, lmf: 0.8, rsmf: 0.5 };
        assert!((m.factor() - 0.9 * 0.95 * 0.8 * 0.5).abs() < 1e-12);
        // The shipped default is the ~0.80 a clean interior is normally quoted at.
        let d = Maintenance::default().factor();
        assert!((0.78..=0.82).contains(&d), "default MF should be about 0.80, got {d}");
    }

    /// Percentiles interpolate, and the ends are the extremes. Worth pinning because an
    /// off-by-one in the rank is invisible: every answer still lands inside the data.
    #[test]
    fn percentiles_span_the_data_and_interpolate() {
        let g = grid(vec![100.0, 200.0, 300.0, 400.0, 500.0]);
        assert_eq!(g.percentile(0.0), 100.0);
        assert_eq!(g.percentile(100.0), 500.0);
        assert_eq!(g.median(), 300.0);
        // Rank 0.5 of the way between cells 1 and 2 -> halfway between 200 and 300.
        assert!((g.percentile(37.5) - 250.0).abs() < 1e-9, "got {}", g.percentile(37.5));
    }

    /// Percentiles do not care what order the cells arrived in — a grid is a spatial layout, and
    /// sorting must not be confused with reading it row-major.
    #[test]
    fn percentiles_ignore_cell_order() {
        let a = grid(vec![100.0, 200.0, 300.0, 400.0]);
        let b = grid(vec![300.0, 100.0, 400.0, 200.0]);
        assert_eq!(a.median(), b.median());
        assert_eq!(a.percentile(25.0), b.percentile(25.0));
    }

    /// An empty grid answers 0 rather than dividing by zero or panicking on an empty sort.
    #[test]
    fn an_empty_grid_has_no_statistics_rather_than_a_panic() {
        let g = grid(Vec::new());
        assert_eq!(g.median(), 0.0);
        assert_eq!(g.u0(), 0.0);
        assert_eq!(g.u1(), 0.0);
        assert_eq!(g.direct_fraction(), None);
    }

    fn profile(name: &str, watts: f64, lumens: f64) -> IesProfile {
        IesProfile {
            name: name.into(),
            photometry: PhotometryType::C,
            lumens,
            multiplier: 1.0,
            vertical_angles: vec![0.0],
            horizontal_angles: vec![0.0],
            candela: vec![vec![100.0]],
            watts,
            width: 0.0,
            length: 0.0,
            height: 0.0,
        }
    }

    fn fixture(id: u32, profile: &str) -> Luminaire {
        Luminaire {
            id,
            profile: profile.into(),
            position: Vertex::new(0.0, 0.0, 3.0),
            rotation_deg: 0.0,
            dimming: 1.0,
        }
    }

    /// Connected load, density and efficacy, from data the photometric files already carried.
    #[test]
    fn the_installation_summary_adds_up() {
        let mut profiles = HashMap::new();
        profiles.insert("A".to_string(), profile("A", 36.0, 3600.0));
        let lums: Vec<Luminaire> = (1..=10).map(|i| fixture(i, "A")).collect();

        let s = installation_summary(&lums, &profiles, 100.0);
        assert_eq!(s.count, 10);
        assert!((s.total_watts - 360.0).abs() < 1e-9);
        assert!((s.total_lumens - 36_000.0).abs() < 1e-9);
        assert!((s.power_density - 3.6).abs() < 1e-9, "360 W over 100 m2");
        assert!((s.efficacy - 100.0).abs() < 1e-9, "3600 lm / 36 W");
    }

    /// DIMMING does not reduce the connected load. The circuit is sized for the installed
    /// wattage whatever the control system is doing, and quoting a dimmed figure as installed
    /// power would understate the design.
    #[test]
    fn dimming_does_not_reduce_the_connected_load() {
        let mut profiles = HashMap::new();
        profiles.insert("A".to_string(), profile("A", 50.0, 5000.0));
        let mut lums = vec![fixture(1, "A"), fixture(2, "A")];
        lums[0].dimming = 0.1;
        let s = installation_summary(&lums, &profiles, 10.0);
        assert!((s.total_watts - 100.0).abs() < 1e-9, "both fixtures are still installed");
    }

    /// A file with no wattage is COUNTED and REPORTED, not silently treated as 0 W. A power
    /// density computed from half the fixtures is worse than none, because it looks like a result.
    #[test]
    fn fixtures_with_no_rated_power_are_reported_not_assumed() {
        let mut profiles = HashMap::new();
        profiles.insert("known".to_string(), profile("known", 40.0, 4000.0));
        profiles.insert("blank".to_string(), profile("blank", 0.0, -1.0));
        let lums = vec![fixture(1, "known"), fixture(2, "blank")];

        let s = installation_summary(&lums, &profiles, 20.0);
        assert_eq!(s.count, 2);
        assert_eq!(s.missing_watts, 1);
        assert_eq!(s.missing_lumens, 1);
        assert!((s.total_watts - 40.0).abs() < 1e-9, "only the fixture that declares a load");
    }

    /// A point with no fitting on it is not a fixture and must not appear in the load.
    #[test]
    fn unassigned_points_are_not_part_of_the_installation() {
        let mut profiles = HashMap::new();
        profiles.insert("A".to_string(), profile("A", 30.0, 3000.0));
        let lums = vec![fixture(1, "A"), fixture(2, "")];
        let s = installation_summary(&lums, &profiles, 10.0);
        assert_eq!(s.count, 1);
        assert!((s.total_watts - 30.0).abs() < 1e-9);
    }
}
