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
        // FURNITURE — its own material, not bucketed by orientation like the building.
        //
        // A desk top is not a floor and a cupboard side is not a wall; giving a shop's stock the
        // ceiling's 0.70 would recreate the empty-box error that including furniture exists to fix.
        // 0.35 is the middle of the range CIE 97 gives for furnished contents, and it is a stated
        // default the user can change rather than a number smuggled in from a texture.
        Material { id: MATERIAL_FURNITURE, name: "Furniture".into(), reflectance: 0.35, color: [0.7, 0.65, 0.6] },
    ]
}

/// Material id furniture is traced under. Named because two crates have to agree on it.
pub const MATERIAL_FURNITURE: MaterialId = 3;

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
    /// CONNECTED LOAD for THIS fitting (W), overriding the profile's.
    ///
    /// "the paramters of the light also need to accessible to the user, like that wattage and
    /// efficacy. the changes should be for the ldt file in the app the parent ldt file shouldnt
    /// change." So the override lives on the FIXTURE and travels in the project sidecar; the
    /// photometric file on disk is never touched — nothing in either crate writes `.ldt` or `.ies`.
    ///
    /// Watts alone do NOT change the light. A fitting rewired to draw less power for the same
    /// output is more efficient, not dimmer; this moves the power density and nothing else.
    #[serde(default)]
    pub watts_override: Option<f64>,
    /// INSTALLED FLUX for this fitting (lm), overriding the profile's.
    ///
    /// This one DOES change the light, and it must: a luminaire's flux scales its whole intensity
    /// distribution, so 3600 lm from a profile measured at 4000 lm is that profile at 0.9. Editing
    /// EFFICACY is editing this — `flux = watts × efficacy` — which is why efficacy is not a third
    /// stored field. Two numbers that must agree are one number and a bug.
    #[serde(default)]
    pub flux_override: Option<f64>,
    /// WHICH BLOCK PUT THIS FITTING HERE, if one did. `None` means placed by hand.
    ///
    /// The Light Editor wires a block definition in the drawing to a photometric file and places
    /// one fitting per instance of it. Re-applying that wiring — or changing which file it points
    /// at — has to replace what it placed LAST time, or pressing Apply twice silently doubles the
    /// installation, and the lux result with it.
    ///
    /// Held ON THE FITTING rather than in a side map beside it, so the two cannot fall out of
    /// step: deleting a luminaire by hand takes its provenance with it, and the project sidecar
    /// carries the pairing without a second thing to serialise. A block id is only a number here
    /// — the engine neither knows nor cares what a block is.
    #[serde(default)]
    pub from_block: Option<u32>,
}

impl Luminaire {
    /// This fitting's connected load (W), given the profile it points at.
    pub fn watts(&self, prof: &crate::IesProfile) -> f64 {
        self.watts_override.unwrap_or(prof.watts)
    }

    /// This fitting's installed flux (lm), given the profile it points at.
    pub fn lumens(&self, prof: &crate::IesProfile) -> f64 {
        self.flux_override.unwrap_or(prof.lumens)
    }

    /// This fitting's efficacy (lm/W), or `None` when either figure is missing.
    pub fn efficacy(&self, prof: &crate::IesProfile) -> Option<f64> {
        let (w, lm) = (self.watts(prof), self.lumens(prof));
        (w > 0.0 && lm > 0.0).then(|| lm / w)
    }

    /// The factor every candela value from `prof` is multiplied by for THIS fitting: dimming, times
    /// the ratio of its installed flux to the flux the profile was measured at.
    ///
    /// WITH NO OVERRIDE THIS IS EXACTLY `dimming`, to the bit. That is the property that keeps the
    /// engine's DIALux agreement intact: the calculation is unchanged for every existing project,
    /// and provably so — the identical-room comparisons still report 200.1 / 337.6 / 337.5 lx.
    pub fn output_scale(&self, prof: &crate::IesProfile) -> f64 {
        let d = self.dimming as f64;
        match self.flux_override {
            Some(lm) if prof.lumens > 0.0 && lm >= 0.0 => d * (lm / prof.lumens),
            _ => d,
        }
    }
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
        // PER-FITTING FIGURES, not the profile's, so a fixture the user has re-rated counts as what
        // it actually is. With no override these are the profile's values unchanged.
        let (w, lm) = (l.watts(p), l.lumens(p));
        if w > 0.0 {
            s.total_watts += w;
        } else {
            s.missing_watts += 1;
        }
        if lm > 0.0 {
            s.total_lumens += lm;
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

/// EN 12464-1's MAXIMUM GRID SPACING for an area whose relevant dimension is `d` metres:
/// `p = 0.2 · 5^log₁₀(d)`, rounded DOWN to the next value in the series the standard tabulates.
///
/// Uniformity is not a property of a room. It is a property of a room AND the grid it was measured
/// on, and a coarse grid always reports it too high. Comparing SIMLUX against DIALux on three fully
/// specified rooms made that concrete: the averages agreed to half a per cent while U₀ disagreed by
/// a third, entirely because of where the minimum was sampled — see
/// `cad_light/tests/identical_dialux.rs`.
///
/// So the grid has to be a STATED CHOICE rather than a side effect of how many cells someone wanted
/// drawn. This is that choice, and it is the one the standard makes.
pub fn en12464_spacing(d: f32) -> f32 {
    if !(d.is_finite() && d > 0.0) {
        return 0.2;
    }
    // The formula is continuous; the standard's table is not. Rounding DOWN to the next tabulated
    // value keeps the grid at least as fine as the formula asks for — erring toward more samples,
    // which is the safe direction for a minimum.
    const SERIES: [f32; 9] = [0.2, 0.25, 0.5, 1.0, 2.0, 3.0, 5.0, 10.0, 20.0];
    let p = 0.2 * 5.0_f32.powf(d.log10());
    let mut out = SERIES[0];
    for s in SERIES {
        if s <= p + 1e-6 {
            out = s;
        }
    }
    out
}

/// Cell counts for a `width × depth` area on the EN 12464-1 grid.
///
/// The standard also asks that a cell stay roughly square — it caps the ratio of the two spacings
/// at 2:1 — so a long thin room is not sampled finely along one axis and coarsely along the other.
pub fn en12464_cells(width: f32, depth: f32) -> (u32, u32) {
    let cells = |extent: f32, spacing: f32| ((extent / spacing).ceil() as u32).max(1);
    // The spacing is set by the LONGER dimension: that is the "relevant dimension" the formula
    // takes, and it keeps a 12 × 2 m corridor on one grid rather than two.
    let p = en12464_spacing(width.max(depth));
    let (mut c, mut r) = (cells(width, p), cells(depth, p));
    // …then refine the coarser axis until the cells are no worse than 2:1.
    let (mut dx, mut dy) = (width / c as f32, depth / r as f32);
    let mut guard = 0;
    while (dx / dy).max(dy / dx) > 2.0 && guard < 16 {
        if dx > dy {
            c += 1;
        } else {
            r += 1;
        }
        dx = width / c as f32;
        dy = depth / r as f32;
        guard += 1;
    }
    (c, r)
}

impl CalcPlane {
    /// The same plane, re-gridded to EN 12464-1 — the basis uniformity should be quoted on.
    ///
    /// Deliberately SEPARATE from the plane you display. A report wants a readable number of cells;
    /// a compliance figure wants the standard's. Tying the two together is what made our U₀
    /// optimistic, and optimism here is the direction that passes an installation which should fail.
    pub fn on_standard_grid(&self) -> CalcPlane {
        let (cols, rows) = en12464_cells(self.width, self.depth);
        CalcPlane { cols, rows, ..*self }
    }

    /// A human note for the report: "8 × 8 cells, 0.50 m spacing".
    ///
    /// A uniformity figure without its grid is not reproducible — which is the whole lesson of the
    /// DIALux comparison, where their U₀ could not be reproduced because the grid behind it is
    /// stated nowhere in the report.
    pub fn grid_note(&self) -> String {
        let dx = self.width / self.cols.max(1) as f32;
        let dy = self.depth / self.rows.max(1) as f32;
        if (dx - dy).abs() < 5e-3 {
            format!("{} × {} cells, {:.2} m spacing", self.cols, self.rows, dx)
        } else {
            format!("{} × {} cells, {:.2} × {:.2} m spacing", self.cols, self.rows, dx, dy)
        }
    }

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
            luminous_length: 0.0,
            luminous_width: 0.0,
        }
    }

    fn fixture(id: u32, profile: &str) -> Luminaire {
        Luminaire {
            id,
            profile: profile.into(),
            position: Vertex::new(0.0, 0.0, 3.0),
            rotation_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
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

/// THE UNIFORMITY GRID.
///
/// Comparing against DIALux on three fully specified rooms showed the averages agreeing to half a
/// per cent while U₀ disagreed by a third — entirely because of where the minimum was sampled. A
/// coarse grid never samples a corner, always reports the minimum too high, and therefore always
/// reports uniformity too GOOD: the direction that passes an installation which should fail.
///
/// The fix is not to tune a refinement until it matches DIALux (whose grid is stated nowhere, and
/// which no single refinement reproduces across the three cases). It is to sample on a DOCUMENTED
/// grid and say which one.
#[cfg(test)]
mod uniformity_grid {
    use super::*;

    /// The standard's formula, p = 0.2 · 5^log₁₀(d), snapped down to its tabulated series.
    #[test]
    fn spacing_follows_the_standards_formula() {
        // 4 m room: 0.2 · 5^0.602 = 0.527 -> the 0.5 m step.
        assert!((en12464_spacing(4.0) - 0.5).abs() < 1e-6, "got {}", en12464_spacing(4.0));
        // 1 m: 0.2 · 5^0 = 0.2 exactly, the finest step in the table.
        assert!((en12464_spacing(1.0) - 0.2).abs() < 1e-6);
        // 10 m: 0.2 · 5 = 1.0.
        assert!((en12464_spacing(10.0) - 1.0).abs() < 1e-6, "got {}", en12464_spacing(10.0));
        // 100 m: 0.2 · 25 = 5.0.
        assert!((en12464_spacing(100.0) - 5.0).abs() < 1e-6, "got {}", en12464_spacing(100.0));
        // It only ever rounds DOWN — a finer grid than asked for is safe, a coarser one is not.
        for d in [2.0_f32, 3.7, 7.5, 12.0, 45.0, 200.0] {
            let p = en12464_spacing(d);
            assert!(p <= 0.2 * 5.0_f32.powf(d.log10()) + 1e-6, "d = {d} rounded UP to {p}");
        }
        // …except below about 1 m, where the formula asks for finer than the standard tabulates
        // (0.5 m wants 0.115) and 0.2 m is the floor. Sampling a half-metre task area on 0.2 m is
        // 3 cells, which is all the standard offers; going finer than its table is inventing one.
        assert!((en12464_spacing(0.5) - 0.2).abs() < 1e-6);
        assert!((en12464_spacing(0.1) - 0.2).abs() < 1e-6);
    }

    /// Nonsense in, something usable out — a zero or NaN extent must not divide by zero downstream.
    #[test]
    fn a_degenerate_extent_is_survivable() {
        for d in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
            let p = en12464_spacing(d);
            assert!(p.is_finite() && p > 0.0, "d = {d} gave spacing {p}");
        }
        let (c, r) = en12464_cells(0.0, 0.0);
        assert!(c >= 1 && r >= 1);
    }

    /// THE ROOM FROM THE DIALUX COMPARISON. 4 × 4 m on 0.5 m spacing is 8 × 8 — which is exactly
    /// the grid DIALux printed, so the standard and DIALux agree about the DISPLAY grid even though
    /// they disagree about the minimum.
    #[test]
    fn the_dialux_test_room_lands_on_eight_by_eight() {
        assert_eq!(en12464_cells(4.0, 4.0), (8, 8));
    }

    /// A long thin space keeps roughly square cells: the standard caps the two spacings at 2:1, so
    /// a corridor is not sampled finely along its length and coarsely across it.
    #[test]
    fn cells_stay_within_two_to_one() {
        for (w, d) in [(12.0_f32, 2.0_f32), (11.74, 5.644), (30.0, 3.0), (2.0, 9.0)] {
            let (c, r) = en12464_cells(w, d);
            let (dx, dy) = (w / c as f32, d / r as f32);
            let aspect = (dx / dy).max(dy / dx);
            assert!(aspect <= 2.0 + 1e-3, "{w} x {d} gave cells {dx:.3} x {dy:.3} — {aspect:.2}:1");
        }
    }

    /// The spacing comes from the LONGER dimension, so one grid covers the whole area rather than
    /// each axis choosing its own.
    #[test]
    fn the_longer_dimension_sets_the_spacing() {
        // 12 m sets p = 1.0; the 2 m axis then refines only to satisfy the aspect cap.
        let (c, _) = en12464_cells(12.0, 2.0);
        assert_eq!(c, 12, "the long axis should be on 1.0 m spacing, got {c} cells");
    }

    /// THE POINT OF ALL THIS: the grid uniformity is quoted on is INDEPENDENT of the grid drawn.
    #[test]
    fn the_standard_grid_is_independent_of_the_display_grid() {
        let base = CalcPlane {
            origin: Vertex::new(0.0, 0.0, 0.8),
            width: 4.0,
            depth: 4.0,
            cols: 3, // …someone wanted a coarse picture
            rows: 40, // …or a fine one
        };
        let std = base.on_standard_grid();
        assert_eq!((std.cols, std.rows), (8, 8), "the display grid must not leak into the standard");
        // …and everything else about the plane is untouched: same place, same size, same height.
        assert_eq!(std.width, base.width);
        assert_eq!(std.depth, base.depth);
        assert_eq!(std.origin.z, base.origin.z);
    }

    /// A uniformity figure without its grid is not reproducible — the whole lesson of the DIALux
    /// comparison, where their U₀ could not be reproduced because the grid behind it is stated
    /// nowhere.
    #[test]
    fn the_grid_reports_itself() {
        let p = CalcPlane {
            origin: Vertex::new(0.0, 0.0, 0.8),
            width: 4.0,
            depth: 4.0,
            cols: 8,
            rows: 8,
        };
        let note = p.grid_note();
        assert!(note.contains('8'), "got {note:?}");
        assert!(note.contains("0.50"), "the spacing has to be in it: {note:?}");
        // Non-square cells say so rather than quietly quoting one number.
        let oblong = CalcPlane { width: 12.0, depth: 2.0, cols: 12, rows: 2, ..p };
        assert!(oblong.grid_note().contains('×'), "got {:?}", oblong.grid_note());
    }
}

/// PER-FITTING OVERRIDES, AND WHAT EACH ONE IS ALLOWED TO CHANGE.
///
/// "the paramters of the light also need to accessible to the user, like that wattage and efficacy
/// … make sure the changes work" — alongside "make sure it doesnt break any thing with calculation
/// since we already have it working well."
///
/// The two are not the same kind of number and must not behave the same way. Watts is what the
/// fitting COSTS; flux is what it DELIVERS. Re-rating the wattage of a fitting that emits the same
/// light makes it more efficient, not brighter.
#[cfg(test)]
mod a_fitting_may_be_re_rated {
    use super::*;
    use std::collections::HashMap;

    fn profile(lumens: f64, watts: f64) -> crate::IesProfile {
        crate::IesProfile {
            name: "p".into(),
            photometry: crate::PhotometryType::C,
            lumens,
            multiplier: 1.0,
            vertical_angles: vec![0.0, 90.0],
            horizontal_angles: vec![0.0],
            candela: vec![vec![1000.0, 0.0]],
            watts,
            width: 0.0,
            length: 0.0,
            height: 0.0,
            luminous_length: 0.0,
            luminous_width: 0.0,
        }
    }

    fn lum() -> Luminaire {
        Luminaire {
            id: 1,
            profile: "p".into(),
            position: Vertex::new(0.0, 0.0, 3.0),
            rotation_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }
    }

    /// THE SAFETY PROPERTY. With no override the output scale is EXACTLY `dimming` — the same bits,
    /// not merely a close number — so every existing project computes what it always did.
    #[test]
    fn no_override_is_bit_identical_to_dimming() {
        let p = profile(4000.0, 40.0);
        for d in [1.0_f32, 0.75, 0.5, 0.331, 0.0] {
            let mut l = lum();
            l.dimming = d;
            assert_eq!(
                l.output_scale(&p),
                d as f64,
                "an un-overridden fitting must scale by exactly its dimming",
            );
        }
    }

    /// WATTS DO NOT CHANGE THE LIGHT. Re-rating the load must move the power density and nothing
    /// else — a fitting that draws less for the same output is more efficient, not dimmer.
    #[test]
    fn re_rating_the_wattage_does_not_change_the_output() {
        let p = profile(4000.0, 40.0);
        let mut l = lum();
        let before = l.output_scale(&p);
        l.watts_override = Some(32.0);
        assert_eq!(l.output_scale(&p), before, "watts must not touch the light");
        assert_eq!(l.watts(&p), 32.0);
        assert_eq!(l.lumens(&p), 4000.0, "flux is unchanged");
        assert_eq!(l.efficacy(&p), Some(125.0), "…so it is now 125 lm/W, which is the point");
    }

    /// FLUX DOES CHANGE THE LIGHT, and it must: a luminaire's flux scales its whole distribution.
    #[test]
    fn re_rating_the_flux_scales_the_output() {
        let p = profile(4000.0, 40.0);
        let mut l = lum();
        l.flux_override = Some(3600.0);
        assert!((l.output_scale(&p) - 0.9).abs() < 1e-12, "3600 of 4000 is the profile at 0.9");
        // …and it compounds with dimming rather than replacing it.
        l.dimming = 0.5;
        assert!((l.output_scale(&p) - 0.45).abs() < 1e-12);
    }

    /// A profile with no rated flux cannot be scaled against — there is nothing to take a ratio of,
    /// and inventing one would silently rescale a whole scheme.
    #[test]
    fn a_profile_with_no_rated_flux_is_left_alone() {
        let p = profile(0.0, 40.0);
        let mut l = lum();
        l.flux_override = Some(3600.0);
        assert_eq!(l.output_scale(&p), 1.0, "no ratio is possible, so no scaling is applied");
    }

    /// The connected load counts the FITTING's figures, so a re-rated fixture shows up in W/m².
    #[test]
    fn the_power_density_follows_the_override() {
        let mut profs = HashMap::new();
        profs.insert("p".to_string(), profile(4000.0, 40.0));
        let plain = installation_summary(&[lum(), lum()], &profs, 10.0);
        assert!((plain.total_watts - 80.0).abs() < 1e-9);
        assert!((plain.power_density - 8.0).abs() < 1e-9);

        let mut a = lum();
        a.watts_override = Some(32.0);
        let rerated = installation_summary(&[a, lum()], &profs, 10.0);
        assert!((rerated.total_watts - 72.0).abs() < 1e-9, "32 + 40, not 80");
        assert!((rerated.total_lumens - 8000.0).abs() < 1e-9, "flux untouched");
    }

    /// The override survives a save. It lives on the FIXTURE, in the project — the photometric file
    /// on disk is never written, which is the whole of "the parent ldt file shouldnt change".
    #[test]
    fn overrides_round_trip_through_serde() {
        let mut l = lum();
        l.watts_override = Some(32.0);
        l.flux_override = Some(3600.0);
        let json = serde_json::to_string(&l).expect("serialise");
        let back: Luminaire = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.watts_override, Some(32.0));
        assert_eq!(back.flux_override, Some(3600.0));
    }

    /// …and a project saved BEFORE these fields existed still opens, with no override.
    #[test]
    fn an_older_project_opens_without_them() {
        let json = r#"{"id":1,"profile":"p","position":{"x":0.0,"y":0.0,"z":3.0},
                       "rotation_deg":0.0,"dimming":1.0}"#;
        let back: Luminaire = serde_json::from_str(json).expect("older sidecar must still load");
        assert_eq!(back.watts_override, None);
        assert_eq!(back.flux_override, None);
    }
}
