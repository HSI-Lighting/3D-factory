//! The lux calculation engine: direct illuminance (inverse-square + cosine,
//! shadow-tested) plus Monte-Carlo one-bounce+ indirect (Lambertian reflection).
use std::collections::HashMap;
use std::f64::consts::PI;

use glam::Vec3;
use rayon::prelude::*;

use crate::ies::IesProfile;
use crate::rt::{cosine_sample, Ray, Rng, RtScene, Tri};
use crate::types::{
    CalcPlane, LuxGrid, Luminaire, Maintenance, Material, MaterialId, Mesh, RaySettings, Vertex,
};

const EPS: f32 = 1e-3;

fn v3(v: Vertex) -> Vec3 {
    v.to_vec3()
}

/// A scene prepared for measurement: geometry, luminaires, materials and settings, with the BVH
/// already built.
///
/// This is the engine's one primitive — illuminance at a point, facing a direction, including
/// interreflection. Nearly every quantity a lighting report contains is that same integral asked
/// with a different normal or averaged over a different set of them: vertical illuminance is one
/// direction, cylindrical is the mean over azimuth, scalar is the mean over the whole sphere, and a
/// diffuse surface's luminance is its illuminance times `ρ/π`. None of them needs new physics, and
/// none of them should rebuild the BVH — hence a reusable evaluator rather than a function per
/// metric.
pub struct Evaluator<'a> {
    scene: RtScene,
    luminaires: &'a [Luminaire],
    profiles: &'a HashMap<String, IesProfile>,
    materials: &'a [Material],
    settings: RaySettings,
    /// Applied to every value this evaluator returns, so a maintained scene stays maintained
    /// whichever quantity is asked for.
    maintenance: f64,
}

impl<'a> Evaluator<'a> {
    pub fn new(
        meshes: &[Mesh],
        luminaires: &'a [Luminaire],
        profiles: &'a HashMap<String, IesProfile>,
        materials: &'a [Material],
        settings: RaySettings,
        maintenance: Maintenance,
    ) -> Self {
        Evaluator {
            scene: RtScene::new(build_tris(meshes)),
            luminaires,
            profiles,
            materials,
            settings,
            maintenance: maintenance.factor(),
        }
    }

    fn reflectance(&self, id: MaterialId) -> f64 {
        self.materials.iter().find(|m| m.id == id).map(|m| m.reflectance as f64).unwrap_or(0.5)
    }

    /// A deterministic RNG for a point, so the same query always gives the same answer.
    ///
    /// Monte-Carlo results that wobble between runs are impossible to review: a designer who
    /// re-runs a calculation and gets 497 lx instead of 499 cannot tell a change they made from
    /// sampling noise.
    fn rng_at(&self, p: Vec3, salt: u64) -> Rng {
        let q = |f: f32| (f as f64 * 8192.0) as i64 as u64;
        let h = q(p.x)
            .wrapping_mul(0x9E3779B9_7F4A7C15)
            ^ q(p.y).wrapping_mul(0xC2B2AE3D_27D4EB4F)
            ^ q(p.z).wrapping_mul(0x1656_67B1_9E37_79F9)
            ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03);
        Rng::seeded(h | 1)
    }

    /// Illuminance (lux) at `point` on a plane whose normal is `normal`.
    pub fn illuminance(&self, point: Vec3, normal: Vec3) -> f64 {
        let mut rng = self.rng_at(point, normal.x.to_bits() as u64);
        illuminance_split(self, point, normal, self.settings.max_bounces, &mut rng).total()
            * self.maintenance
    }

    /// Illuminance at `point`, as `(direct, indirect)`.
    pub fn illuminance_parts(&self, point: Vec3, normal: Vec3) -> (f64, f64) {
        let mut rng = self.rng_at(point, normal.x.to_bits() as u64);
        let s = illuminance_split(self, point, normal, self.settings.max_bounces, &mut rng);
        (s.direct * self.maintenance, s.indirect * self.maintenance)
    }

    /// **Vertical illuminance** `E_v` on a vertical plane facing `azimuth_deg` (0° = +X).
    ///
    /// What a wall, a whiteboard or a face turned that way receives. EN 12464 sets requirements on
    /// it for exactly those, and a scheme optimised only for the horizontal work plane routinely
    /// misses them — downlights put light on desks and very little on anything upright.
    pub fn vertical(&self, point: Vec3, azimuth_deg: f64) -> f64 {
        let a = azimuth_deg.to_radians();
        self.illuminance(point, Vec3::new(a.cos() as f32, a.sin() as f32, 0.0))
    }

    /// **Cylindrical illuminance** `E_z` — the mean illuminance on an infinitesimal vertical
    /// cylinder, which is the mean of the vertical illuminance over every azimuth.
    ///
    /// That equivalence is the definition, not an approximation: the average illuminance over the
    /// cylinder's curved surface is by construction the average of the planar illuminance of the
    /// surface elements making it up, and those elements' normals sweep the horizon uniformly. So
    /// it is computed by sampling azimuths rather than by a formula taken on trust.
    ///
    /// It is the standard measure of how well a space renders faces and solid objects — a room can
    /// hold 500 lx on the desks and still feel flat and cave-like, and `E_z` is the number that
    /// says so.
    pub fn cylindrical(&self, point: Vec3) -> f64 {
        const N: u32 = 24;
        let mut sum = 0.0;
        for i in 0..N {
            sum += self.vertical(point, 360.0 * i as f64 / N as f64);
        }
        sum / N as f64
    }

    /// **Semi-cylindrical illuminance** `E_sc` facing `azimuth_deg`.
    ///
    /// The mean over the half of the horizon the surface faces — the light reaching a face looking
    /// that way, which is what facial-recognition criteria in circulation and security areas are
    /// written against. Directional by nature: facing a luminaire and facing away from it give
    /// different answers, which is the whole point of the measure.
    pub fn semi_cylindrical(&self, point: Vec3, azimuth_deg: f64) -> f64 {
        const N: u32 = 16;
        let mut sum = 0.0;
        for i in 0..N {
            // Mid-points of N equal slices spanning the facing half, −90°..+90°.
            let off = -90.0 + 180.0 * (i as f64 + 0.5) / N as f64;
            sum += self.vertical(point, azimuth_deg + off);
        }
        sum / N as f64
    }

    /// **Scalar (spherical) illuminance** `E_s` — the mean planar illuminance over every possible
    /// orientation, i.e. the light density at the point regardless of which way anything faces.
    ///
    /// Sampled over a deterministic Fibonacci sphere rather than randomly, so the answer is stable
    /// between runs and the quadrature error is a fixed small bias instead of per-call noise.
    pub fn scalar(&self, point: Vec3) -> f64 {
        const N: u32 = 64;
        let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt()); // golden angle
        let mut sum = 0.0;
        for i in 0..N {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / N as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let th = ga * i as f64;
            let n = Vec3::new((r * th.cos()) as f32, (r * th.sin()) as f32, z as f32);
            sum += self.illuminance(point, n);
        }
        sum / N as f64
    }

    /// **Luminance** (cd/m²) of a diffuse surface at `point` facing `normal`, with reflectance `ρ`.
    ///
    /// `L = ρ·E/π` — exact for a Lambertian surface, which is what this engine's materials are.
    /// EN 12464-1 puts floors on the room's surfaces, not only on the work plane, and this is the
    /// quantity those clauses are written in.
    pub fn luminance(&self, point: Vec3, normal: Vec3, reflectance: f64) -> f64 {
        reflectance * self.illuminance(point, normal) / PI
    }
}

/// Luminous intensity (candela) a luminaire emits toward `point`. Convention:
/// photometric nadir (vertical 0°) points down −Z; horizontal 0° is +X.
fn intensity_toward(prof: &IesProfile, lum: &Luminaire, point: Vec3) -> f64 {
    let d = point - v3(lum.position);
    let dist = d.length();
    if dist < 1e-6 {
        return 0.0;
    }
    let dir = d / dist;
    let gamma = (-dir.z).clamp(-1.0, 1.0).acos().to_degrees() as f64;
    let phi = (dir.y.atan2(dir.x).to_degrees() as f64) - lum.rotation_deg as f64;
    prof.intensity(gamma, phi) * lum.output_scale(prof)
}

/// Direct illuminance (lux) at a surface point with the given outward `normal`.
fn direct(ev: &Evaluator, point: Vec3, normal: Vec3) -> f64 {
    let mut e = 0.0;
    for lum in ev.luminaires {
        let Some(prof) = ev.profiles.get(&lum.profile) else {
            continue;
        };
        let lpos = v3(lum.position);
        let to_light = lpos - point;
        let dist = to_light.length();
        if dist < 1e-6 {
            continue;
        }
        let cos_inc = normal.dot(to_light / dist) as f64;
        if cos_inc <= 0.0 {
            continue;
        }
        let intensity = intensity_toward(prof, lum, point);
        if intensity <= 0.0 {
            continue;
        }
        if ev.settings.shadows && ev.scene.occluded(point + normal * EPS, lpos) {
            continue;
        }
        e += intensity * cos_inc / (dist as f64 * dist as f64);
    }
    e
}

/// Illuminance at a point, kept as its two components.
///
/// The split exists only at the point being MEASURED. Deeper in the recursion a reflecting surface
/// contributes its whole illuminance, direct and interreflected alike, because from the receiving
/// point's view every bit of it arrives by reflection.
#[derive(Clone, Copy, Default)]
struct Split {
    direct: f64,
    indirect: f64,
}

impl Split {
    fn total(&self) -> f64 {
        self.direct + self.indirect
    }
}

/// Illuminance at a point, split into the light straight from the luminaires and the light that
/// arrived off the room's own surfaces.
///
/// Worth separating because no summary statistic reveals it: a room carried by interreflection is
/// the one that collapses when the client repaints in a darker colour, and its average lux looks
/// exactly like a directly-lit room's.
///
/// The indirect term is a PATH sum, not a branching tree. The previous form spawned
/// `rays_per_point` fresh rays at every bounce, each of which spawned that many again — so the cost
/// was `rays^bounces`. Measured at the shipped default of 64 rays, each extra bounce multiplied the
/// time by about forty: bounce 3 took 615 ms on a twelve-by-twelve grid in a bare box, which puts
/// bounce 5 at a quarter of an hour and bounce 6 somewhere past half a day — and the UI offers
/// eight. Nobody could have run those settings; the app would simply have stopped responding.
///
/// Continuing ONE ray per sample instead makes the cost `rays × bounces`, and the estimator is the
/// same one: at each vertex the path carries a throughput of the reflectances so far and collects
/// the direct light there, which is the expansion of the recursion above, term by term. For a
/// single bounce the two are algebraically identical.
///
/// `interreflection_matches_the_closed_form_for_an_enclosure` is what checks the result rather
/// than the reasoning: it lands within 0.2% of the radiosity answer for a closed room.
///
/// The `π` that appeared twice — dividing to turn illuminance into Lambertian radiance, multiplying
/// to integrate the cosine-weighted hemisphere — cancels, so it is absent here by cancellation and
/// not by omission.
fn illuminance_split(ev: &Evaluator, point: Vec3, normal: Vec3, bounces: u32, rng: &mut Rng) -> Split {
    let e = direct(ev, point, normal);
    if bounces == 0 {
        return Split { direct: e, indirect: 0.0 };
    }
    let n = ev.settings.rays_per_point.max(1);
    let mut acc = 0.0;
    for _ in 0..n {
        // One path, walked to `bounces` vertices.
        let mut throughput = 1.0;
        let mut o = point;
        let mut nrm = normal;
        for _ in 0..bounces {
            let w = cosine_sample(nrm, rng);
            let Some(hit) = ev.scene.closest_hit(&Ray { o: o + nrm * EPS, d: w }) else {
                break; // the ray left the room: nothing further can come back along it
            };
            let rho = ev.reflectance(hit.material);
            if rho <= 0.0 {
                break; // a perfect absorber ends the path
            }
            let wn = if hit.normal.dot(w) < 0.0 { hit.normal } else { -hit.normal };
            throughput *= rho;
            acc += throughput * direct(ev, hit.point, wn);
            o = hit.point;
            nrm = wn;
        }
    }
    Split { direct: e, indirect: acc / n as f64 }
}

/// Total illuminance (direct + up to `bounces` diffuse reflections) at a point.
#[allow(dead_code)]
fn illuminance(ev: &Evaluator, point: Vec3, normal: Vec3, bounces: u32, rng: &mut Rng) -> f64 {
    illuminance_split(ev, point, normal, bounces, rng).total()
}

fn build_tris(meshes: &[Mesh]) -> Vec<Tri> {
    let mut tris = Vec::new();
    for m in meshes {
        for t in &m.triangles {
            let (Some(a), Some(b), Some(c)) = (
                m.vertices.get(t.a as usize),
                m.vertices.get(t.b as usize),
                m.vertices.get(t.c as usize),
            ) else {
                continue;
            };
            tris.push(Tri { a: v3(*a), b: v3(*b), c: v3(*c), material: m.material });
        }
    }
    tris
}

/// Compute the INITIAL lux grid over `plane` — no maintenance allowance.
///
/// Kept at its original signature so existing callers are unaffected. New work should call
/// [`calculate_maintained`]: what a designer quotes, and what EN 12464-1 sets limits on, is the
/// MAINTAINED illuminance, and this function's answer is the day-one condition.
pub fn calculate(
    meshes: &[Mesh],
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    materials: &[Material],
    plane: &CalcPlane,
    settings: &RaySettings,
) -> LuxGrid {
    calculate_maintained(meshes, luminaires, profiles, materials, plane, settings, Maintenance::INITIAL)
}

/// Compute the MAINTAINED lux grid over `plane`, with the direct/indirect split.
///
/// The maintenance factor is applied to every cell here rather than left to the caller, so the grid
/// is maintained lux by construction and no two readers can disagree about whether it has been
/// applied. `LuxGrid::maintenance` records what was used.
///
/// Applying it as a scale on the result is exact: illuminance is linear in emitted flux, and all
/// four sub-factors reduce flux (or the room's return of it) by a constant. The plane faces up
/// (+Z). rayon-parallel.
pub fn calculate_maintained(
    meshes: &[Mesh],
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    materials: &[Material],
    plane: &CalcPlane,
    settings: &RaySettings,
    maintenance: Maintenance,
) -> LuxGrid {
    let ev = Evaluator::new(meshes, luminaires, profiles, materials, *settings, maintenance);
    calculate_on(&ev, plane, maintenance)
}

/// [`calculate_maintained`] against an EVALUATOR THAT ALREADY EXISTS.
///
/// Building an `Evaluator` builds a BVH, and on a real project that dominates the whole
/// calculation: measured at 1.9 s over seven million triangles, against 1.9 s for the grid it was
/// built to sample. A report asks four questions of one scene — the work plane, the EN 12464-1
/// plane, cylindrical illuminance, the room surfaces — and each built its own, so most of the time
/// went on constructing the same tree four times over.
///
/// `Evaluator`'s own note has said so since it was written: "none of them should rebuild the BVH —
/// hence a reusable evaluator rather than a function per metric". This is the door that lets a
/// caller outside this module act on it.
pub fn calculate_on(ev: &Evaluator, plane: &CalcPlane, maintenance: Maintenance) -> LuxGrid {
    let cols = plane.cols.max(1);
    let rows = plane.rows.max(1);
    let normal = Vec3::Z;
    let count = (cols * rows) as usize;
    let mf = maintenance.factor();

    let parts: Vec<(f64, f64)> = (0..count)
        .into_par_iter()
        .map(|i| {
            let (col, row) = (i as u32 % cols, i as u32 / cols);
            ev.illuminance_parts(v3(plane.sample_point(col, row)), normal)
        })
        .collect();

    let direct: Vec<f64> = parts.iter().map(|(d, _)| *d).collect();
    let indirect: Vec<f64> = parts.iter().map(|(_, i)| *i).collect();
    let values: Vec<f64> = parts.iter().map(|(d, i)| d + i).collect();
    LuxGrid::from_parts(cols, rows, values, direct, indirect, mf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extrude;
    use crate::ies::PhotometryType;
    use crate::types::default_materials;

    fn flat_1000cd() -> IesProfile {
        let va: Vec<f64> = (0..=90).map(|d| d as f64).collect();
        IesProfile {
            name: "flat".into(),
            photometry: PhotometryType::C,
            lumens: -1.0,
            multiplier: 1.0,
            vertical_angles: va.clone(),
            horizontal_angles: vec![0.0],
            candela: vec![vec![1000.0; va.len()]],
            watts: 0.0,
            width: 0.0,
            length: 0.0,
            height: 0.0,
            luminous_length: 0.0,
            luminous_width: 0.0,
        }
    }

    fn scene(bounces: u32) -> (Vec<Mesh>, HashMap<String, IesProfile>, Vec<Luminaire>, CalcPlane, RaySettings) {
        let (w, d, h) = (4.0f32, 4.0f32, 3.0f32);
        let meshes = extrude::box_room(w, d, h);
        let mut profiles = HashMap::new();
        profiles.insert("flat".into(), flat_1000cd());
        let lums = vec![Luminaire { id: 1, profile: "flat".into(), position: Vertex::new(w / 2.0, d / 2.0, h), rotation_deg: 0.0, dimming: 1.0, watts_override: None, flux_override: None, from_block: None }];
        let plane = CalcPlane { origin: Vertex::new(0.0, 0.0, 0.0), width: w, depth: d, cols: 24, rows: 24 };
        (meshes, profiles, lums, plane, RaySettings { rays_per_point: 64, max_bounces: bounces, shadows: true })
    }

    #[test]
    fn direct_center_matches_inverse_square() {
        let (m, pr, l, pl, s) = scene(0);
        let g = calculate(&m, &l, &pr, &default_materials(), &pl, &s);
        assert!((g.max - 1000.0 / 9.0).abs() < 6.0, "peak {} ~ 111", g.max);
    }

    #[test]
    fn indirect_adds_reflected_light() {
        let (m, pr, l, pl, s0) = scene(0);
        let d = calculate(&m, &l, &pr, &default_materials(), &pl, &s0);
        let (m1, pr1, l1, pl1, s1) = scene(1);
        let b = calculate(&m1, &l1, &pr1, &default_materials(), &pl1, &s1);
        assert!(b.avg > d.avg * 1.02, "indirect {} > direct {}", b.avg, d.avg);
    }

    /// The maintenance factor scales EVERY cell by exactly its own product, and the grid records
    /// what was applied — so a reader can never be in doubt whether a figure is maintained.
    ///
    /// Illuminance is linear in emitted flux, so this is an identity and not an approximation;
    /// asserting it exactly is what makes the shortcut of scaling the result legitimate.
    #[test]
    fn maintenance_scales_every_cell_by_its_factor() {
        let (m, pr, l, pl, s) = scene(1);
        let initial = calculate(&m, &l, &pr, &default_materials(), &pl, &s);
        let mf = Maintenance { llmf: 0.95, lsf: 1.0, lmf: 0.90, rsmf: 0.94 };
        let kept = calculate_maintained(&m, &l, &pr, &default_materials(), &pl, &s, mf);

        assert!((initial.maintenance - 1.0).abs() < 1e-12, "calculate() is the INITIAL condition");
        assert!((kept.maintenance - mf.factor()).abs() < 1e-12);
        assert!((mf.factor() - 0.8037).abs() < 1e-3, "0.95 x 1.0 x 0.90 x 0.94, got {}", mf.factor());

        assert_eq!(initial.values.len(), kept.values.len());
        for (i, (a, b)) in initial.values.iter().zip(kept.values.iter()).enumerate() {
            assert!(
                (b - a * mf.factor()).abs() < 1e-9,
                "cell {i}: maintained {b} should be initial {a} x {}",
                mf.factor()
            );
        }
        // …and therefore the design is DIMmer, which is the entire point.
        assert!(kept.avg < initial.avg, "maintained {} < initial {}", kept.avg, initial.avg);
    }

    /// The split is exhaustive: direct + indirect is the value, cell for cell. If it were not, one
    /// of the two would be quietly wrong and the ratio would still look plausible.
    #[test]
    fn direct_and_indirect_sum_to_the_total() {
        let (m, pr, l, pl, s) = scene(2);
        let g = calculate_maintained(&m, &l, &pr, &default_materials(), &pl, &s, Maintenance::INITIAL);
        assert_eq!(g.direct.len(), g.values.len());
        assert_eq!(g.indirect.len(), g.values.len());
        for i in 0..g.values.len() {
            assert!(
                (g.direct[i] + g.indirect[i] - g.values[i]).abs() < 1e-9,
                "cell {i}: {} + {} != {}",
                g.direct[i], g.indirect[i], g.values[i]
            );
        }
        let f = g.direct_fraction().expect("the split was computed");
        assert!(f > 0.0 && f < 1.0, "a bounced room is lit by both, got direct fraction {f}");
    }

    /// With no bounces there is nothing indirect — the split's zero point, and the check that it
    /// is measuring reflection rather than manufacturing a ratio.
    #[test]
    fn without_bounces_every_lux_is_direct() {
        let (m, pr, l, pl, s) = scene(0);
        let g = calculate_maintained(&m, &l, &pr, &default_materials(), &pl, &s, Maintenance::INITIAL);
        assert!(g.indirect.iter().all(|&v| v == 0.0));
        assert!((g.direct_fraction().unwrap() - 1.0).abs() < 1e-12);
    }

    /// An ISOTROPIC point source: `I` candela in every direction, so its flux is exactly `4πI`.
    ///
    /// Needed for the closed-form check below, where the answer depends on knowing the emitted
    /// lumens exactly rather than to within a quadrature error.
    fn isotropic(i: f64) -> IesProfile {
        let va: Vec<f64> = (0..=36).map(|k| k as f64 * 5.0).collect(); // 0..180
        let n = va.len();
        IesProfile {
            name: "isotropic".into(),
            photometry: PhotometryType::C,
            lumens: 4.0 * PI * i,
            multiplier: 1.0,
            vertical_angles: va,
            horizontal_angles: vec![0.0],
            candela: vec![vec![i; n]],
            watts: 0.0,
            width: 0.0,
            length: 0.0,
            height: 0.0,
            luminous_length: 0.0,
            luminous_width: 0.0,
        }
    }

    /// **The engine against physics, not against itself.**
    ///
    /// Every other test here checks that the tracer agrees with its own assumptions. This one
    /// checks it against a result derived independently of any of the code: for a CLOSED enclosure
    /// of area `A` and uniform reflectance `ρ`, containing a source of flux `Φ`, radiosity theory
    /// gives the average illuminance over the enclosure's surfaces in closed form.
    ///
    /// At equilibrium the flux arriving on the walls is the flux emitted plus the fraction the
    /// walls send back — `Φᵢ = Φ + ρΦᵢ` — so `Φᵢ = Φ/(1−ρ)` and
    ///
    /// ```text
    ///     E_avg = Φ / (A · (1 − ρ))
    /// ```
    ///
    /// This is the integrating-sphere relation, and it holds for any closed shape, not just a
    /// sphere. If the Monte-Carlo interreflection has a wrong constant, a missing `π`, a
    /// double-counted cosine or a mis-normalised sample weight, the number will not land here — and
    /// none of those errors is visible in a picture or in "the average went up when I added a
    /// bounce", which is all the previous tests could show.
    ///
    /// It is only affordable to run because the path rewrite above made ten bounces cheap; with the
    /// old branching form this test would have taken longer than the age of the universe.
    #[test]
    fn interreflection_matches_the_closed_form_for_an_enclosure() {
        const RHO: f32 = 0.5;
        const I: f64 = 1000.0;
        let (w, d, h) = (4.0f32, 4.0f32, 3.0f32);

        let meshes = extrude::box_room(w, d, h);
        // ONE reflectance everywhere — the closed form assumes a uniform enclosure.
        let mats: Vec<Material> = (0..3)
            .map(|id| Material { id, name: format!("s{id}"), reflectance: RHO, color: [1.0; 3] })
            .collect();
        let mut profiles = HashMap::new();
        profiles.insert("iso".to_string(), isotropic(I));
        let lums = vec![Luminaire {
            id: 1,
            profile: "iso".into(),
            position: Vertex::new(w / 2.0, d / 2.0, h / 2.0), // centred, so nothing self-shadows
            rotation_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];

        let flux = 4.0 * PI * I;
        let area = 2.0 * (w * d) as f64 + 2.0 * (w * h) as f64 + 2.0 * (d * h) as f64;
        let expected = flux / (area * (1.0 - RHO as f64));

        // Ten bounces: the truncated series leaves ρ¹¹ ≈ 0.05% on the table, far inside tolerance.
        let settings = RaySettings { rays_per_point: 256, max_bounces: 10, shadows: true };
        let ev = Evaluator::new(&meshes, &lums, &profiles, &mats, settings, Maintenance::INITIAL);

        // Area-weighted average over all six faces, each sampled on its own grid. The faces have
        // different areas, so an unweighted mean would quietly answer a different question.
        let faces: [(Vec3, Vec3, Vec3, Vec3); 6] = [
            // (corner, edge u, edge v, INWARD normal)
            (Vec3::ZERO, Vec3::X * w, Vec3::Y * d, Vec3::Z),                       // floor
            (Vec3::new(0.0, 0.0, h), Vec3::X * w, Vec3::Y * d, -Vec3::Z),           // ceiling
            (Vec3::ZERO, Vec3::X * w, Vec3::Z * h, Vec3::Y),                        // y = 0
            (Vec3::new(0.0, d, 0.0), Vec3::X * w, Vec3::Z * h, -Vec3::Y),           // y = d
            (Vec3::ZERO, Vec3::Y * d, Vec3::Z * h, Vec3::X),                        // x = 0
            (Vec3::new(w, 0.0, 0.0), Vec3::Y * d, Vec3::Z * h, -Vec3::X),           // x = w
        ];
        const N: u32 = 12;
        let (mut flux_sum, mut area_sum) = (0.0, 0.0);
        for (corner, edge_u, edge_v, normal) in faces {
            let face_area = (edge_u.length() * edge_v.length()) as f64;
            let mut e_sum = 0.0;
            for iu in 0..N {
                for iv in 0..N {
                    let fu = (iu as f32 + 0.5) / N as f32;
                    let fv = (iv as f32 + 0.5) / N as f32;
                    let p = corner + edge_u * fu + edge_v * fv;
                    e_sum += ev.illuminance(p, normal);
                }
            }
            // Mean illuminance on this face, times its area = flux it receives.
            flux_sum += (e_sum / (N * N) as f64) * face_area;
            area_sum += face_area;
        }
        let measured = flux_sum / area_sum;

        let err = (measured - expected).abs() / expected;
        assert!(
            err < 0.01,
            "enclosure average {measured:.1} lx vs closed form {expected:.1} lx ({:.1}% off).\n\
             Φ = {flux:.0} lm, A = {area:.0} m², ρ = {RHO}",
            err * 100.0
        );
    }

    /// The DEFAULT bounce count is high enough to have converged.
    ///
    /// A default that stops early does not look wrong — it just reports less light than the room
    /// has, consistently, in every project. Doubling it must therefore barely move the answer; if
    /// it moves a lot, the default is truncating the series and every result is low.
    #[test]
    fn the_default_bounce_count_has_converged() {
        let (m, pr, l, mut pl, _) = scene(0);
        pl.cols = 16;
        pl.rows = 16;
        let mats = default_materials();
        let base = RaySettings::default();
        let deeper = RaySettings { max_bounces: base.max_bounces * 2, ..base };

        let a = calculate(&m, &l, &pr, &mats, &pl, &base);
        let b = calculate(&m, &l, &pr, &mats, &pl, &deeper);
        let gap = (b.avg - a.avg).abs() / b.avg;
        assert!(
            gap < 0.03,
            "the default ({} bounces, {:.1} lx) should be within 3% of {} bounces ({:.1} lx); \
             it is {:.1}% out",
            base.max_bounces, a.avg, deeper.max_bounces, b.avg, gap * 100.0
        );
        // …and one bounce is NOT enough, which is why the default moved.
        let one = calculate(&m, &l, &pr, &mats, &pl, &RaySettings { max_bounces: 1, ..base });
        assert!(
            one.avg < a.avg * 0.9,
            "one bounce ({:.1} lx) materially under-reads the converged room ({:.1} lx)",
            one.avg, a.avg
        );
    }

    /// A bare isotropic source in empty space, so every metric has a closed form to be checked
    /// against. No geometry at all: nothing to reflect off, nothing to occlude.
    fn free_field(i: f64) -> (Vec<Mesh>, HashMap<String, IesProfile>, Vec<Luminaire>, RaySettings) {
        let mut profiles = HashMap::new();
        profiles.insert("iso".to_string(), isotropic(i));
        let lums = vec![Luminaire {
            id: 1,
            profile: "iso".into(),
            position: Vertex::new(0.0, 0.0, 0.0),
            rotation_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];
        (Vec::new(), profiles, lums, RaySettings { rays_per_point: 1, max_bounces: 0, shadows: false })
    }

    /// **Each directional quantity against its closed form**, for a point source at distance `d`
    /// with intensity `I`. Every one of these is derived from the definition, not copied from a
    /// table, and they are the checks that distinguish a correct implementation from a plausible
    /// one — a swapped sine, a missing `π` or a half-range integrated over the wrong half all give
    /// answers that look perfectly reasonable on screen.
    ///
    /// * planar, facing the source:      `E = I/d²`
    /// * scalar (spherical):             `E_s = I/(4d²)`   — a quarter, because averaging
    ///   `max(n·ω, 0)` over all orientations of `n` gives ¼
    /// * cylindrical, source on the horizon: `E_z = I/(πd²)` — averaging `max(cos φ, 0)` over
    ///   azimuth gives `1/π`
    /// * cylindrical, source overhead:   `E_z = 0` — a vertical cylinder presents no projected
    ///   area to a source directly above it
    #[test]
    fn the_directional_quantities_match_their_closed_forms() {
        const I: f64 = 1000.0;
        const D: f32 = 2.0;
        let (m, pr, l, s) = free_field(I);
        let ev = Evaluator::new(&m, &l, &pr, &[], s, Maintenance::INITIAL);
        let e_perp = I / (D as f64 * D as f64); // 250 lx

        // The source sits at the origin; measure at distance D along +X.
        let p = Vec3::new(D, 0.0, 0.0);
        let close = |got: f64, want: f64, what: &str| {
            let err = (got - want).abs() / want.max(1e-9);
            assert!(err < 0.02, "{what}: got {got:.2}, expected {want:.2} ({:.1}% out)", err * 100.0);
        };

        // Planar, facing straight back at the source.
        close(ev.illuminance(p, -Vec3::X), e_perp, "planar facing the source");
        // …and facing away from it: nothing, since the source is behind the plane.
        assert!(ev.illuminance(p, Vec3::X) < 1e-9, "a plane facing away receives nothing");

        // Scalar: a quarter of the perpendicular illuminance.
        close(ev.scalar(p), e_perp / 4.0, "scalar illuminance");

        // Cylindrical with the source on the horizon: 1/π of it.
        close(ev.cylindrical(p), e_perp / PI, "cylindrical, source on the horizon");

        // Cylindrical with the source directly overhead: zero, at every height.
        let below = Vec3::new(0.0, 0.0, -D);
        assert!(
            ev.cylindrical(below) < 1e-6,
            "a vertical cylinder under a source presents no area to it, got {}",
            ev.cylindrical(below)
        );
        // …while the horizontal plane there gets the full I/d².
        close(ev.illuminance(below, Vec3::Z), e_perp, "horizontal under the source");
    }

    /// Semi-cylindrical illuminance is DIRECTIONAL: facing the source and facing away give
    /// different answers, and their mean is the full cylindrical value.
    ///
    /// The mean is the sharp part. Two halves of the horizon average to the whole, so if the
    /// half-range were integrated over the wrong span or normalised by the wrong constant, this
    /// identity would break while each individual number still looked sensible.
    #[test]
    fn semi_cylindrical_faces_a_direction_and_averages_to_cylindrical() {
        const I: f64 = 1000.0;
        let (m, pr, l, s) = free_field(I);
        let ev = Evaluator::new(&m, &l, &pr, &[], s, Maintenance::INITIAL);
        let p = Vec3::new(2.0, 0.0, 0.0); // source is at the origin, i.e. toward azimuth 180°

        let toward = ev.semi_cylindrical(p, 180.0);
        let away = ev.semi_cylindrical(p, 0.0);
        assert!(toward > away, "facing the source ({toward:.1}) must beat facing away ({away:.1})");
        assert!(away < 1e-6, "facing away from the only source, a half-cylinder sees nothing");

        let mean = 0.5 * (toward + away);
        let cyl = ev.cylindrical(p);
        let err = (mean - cyl).abs() / cyl;
        assert!(err < 0.02, "the two halves ({mean:.2}) should average to cylindrical ({cyl:.2})");
    }

    /// Luminance of a diffuse surface is `ρE/π`, and it tracks the illuminance that produced it.
    #[test]
    fn diffuse_luminance_is_reflectance_times_illuminance_over_pi() {
        const I: f64 = 1000.0;
        let (m, pr, l, s) = free_field(I);
        let ev = Evaluator::new(&m, &l, &pr, &[], s, Maintenance::INITIAL);
        let p = Vec3::new(0.0, 0.0, -2.0);
        let e = ev.illuminance(p, Vec3::Z);
        let l70 = ev.luminance(p, Vec3::Z, 0.70);
        assert!((l70 - 0.70 * e / PI).abs() < 1e-9);
        // A darker surface returns proportionally less.
        let l20 = ev.luminance(p, Vec3::Z, 0.20);
        assert!((l20 / l70 - 0.20 / 0.70).abs() < 1e-9);
    }

    /// The maintenance factor reaches EVERY quantity, not just the horizontal grid.
    ///
    /// It would be easy to apply it in `calculate` alone and leave the wall and face measures at
    /// the initial condition — and a report mixing the two would be wrong in a way nothing on the
    /// page would reveal.
    #[test]
    fn maintenance_reaches_every_quantity_not_only_the_work_plane() {
        const I: f64 = 1000.0;
        let (m, pr, l, s) = free_field(I);
        let mf = Maintenance { llmf: 0.9, lsf: 1.0, lmf: 0.9, rsmf: 1.0 };
        let initial = Evaluator::new(&m, &l, &pr, &[], s, Maintenance::INITIAL);
        let kept = Evaluator::new(&m, &l, &pr, &[], s, mf);
        let p = Vec3::new(2.0, 0.0, 0.0);
        let f = mf.factor();
        for (a, b, what) in [
            (initial.illuminance(p, -Vec3::X), kept.illuminance(p, -Vec3::X), "planar"),
            (initial.cylindrical(p), kept.cylindrical(p), "cylindrical"),
            (initial.semi_cylindrical(p, 180.0), kept.semi_cylindrical(p, 180.0), "semi-cylindrical"),
            (initial.scalar(p), kept.scalar(p), "scalar"),
            (initial.luminance(p, -Vec3::X, 0.5), kept.luminance(p, -Vec3::X, 0.5), "luminance"),
        ] {
            assert!((b - a * f).abs() < 1e-9, "{what}: {b} should be {a} x {f}");
        }
    }

    /// Asking twice gives the same answer. Monte-Carlo results that wobble between runs cannot be
    /// reviewed — a designer re-running a calculation must be able to tell a change they made from
    /// sampling noise.
    #[test]
    fn the_same_query_twice_gives_the_same_answer() {
        let (m, pr, l, mut pl, _) = scene(0);
        pl.cols = 6;
        pl.rows = 6;
        let mats = default_materials();
        let s = RaySettings { rays_per_point: 32, max_bounces: 3, shadows: true };
        let ev = Evaluator::new(&m, &l, &pr, &mats, s, Maintenance::INITIAL);
        let p = Vec3::new(2.0, 2.0, 0.8);
        assert_eq!(ev.illuminance(p, Vec3::Z), ev.illuminance(p, Vec3::Z));
        assert_eq!(ev.cylindrical(p), ev.cylindrical(p));
        let a = calculate(&m, &l, &pr, &mats, &pl, &s);
        let b = calculate(&m, &l, &pr, &mats, &pl, &s);
        assert_eq!(a.values, b.values, "the whole grid is reproducible, not just one point");
    }

    /// How the cost of a bounce actually scales. Run with:
    ///   `cargo test -p cad_light bounce_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "measurement: --ignored --nocapture"]
    fn bounce_cost_scaling() {
        let (m, pr, l, mut pl, mut s) = scene(0);
        pl.cols = 12;
        pl.rows = 12;
        s.rays_per_point = 64;
        let mats = default_materials();
        let mut prev: Option<f64> = None;
        for b in 0..=6u32 {
            s.max_bounces = b;
            let t = std::time::Instant::now();
            let g = calculate(&m, &l, &pr, &mats, &pl, &s);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let ratio = prev.map(|p| format!("{:.1}x", ms / p)).unwrap_or_else(|| "-".into());
            println!("bounces {b}: {ms:9.1} ms  ({ratio:>6})  avg {:.1} lx", g.avg);
            prev = Some(ms.max(0.01));
        }
    }

    /// U₁ can never exceed U₀, because the average can never exceed the maximum. A cheap invariant
    /// that catches the two being swapped — which is easy to do and hard to spot, since both are
    /// small numbers between 0 and 1.
    #[test]
    fn diversity_is_never_kinder_than_uniformity() {
        let (m, pr, l, pl, s) = scene(1);
        let g = calculate(&m, &l, &pr, &default_materials(), &pl, &s);
        assert!(g.u1() <= g.u0() + 1e-12, "U1 {} must be <= U0 {}", g.u1(), g.u0());
        assert!(g.u0() > 0.0 && g.u0() <= 1.0);
    }

    // ── ONE EVALUATOR, FOUR QUESTIONS ──────────────────────────────────────────────────────
    //
    // Building an `Evaluator` builds a BVH, and on a real project that is the dominant cost of the
    // whole calculation: 1.9 s over seven million triangles, against 31 ms for the grid it exists
    // to sample. A report asks four questions of ONE scene — the work plane, the EN 12464-1 plane,
    // cylindrical illuminance, the room surfaces — and each used to build its own tree.
    //
    //     before   scene 205   grid 1959   grid_en 1840   cyl 2202   surf 2304   = 8.5 s
    //     after    scene 205   evaluator 1889   grid 31   grid_en 1   cyl 296   surf 476   = 2.9 s

    /// THE WRAPPER DELEGATES, AND KEEPS DELEGATING.
    ///
    /// BE CLEAR ABOUT WHAT THIS CANNOT DO. `calculate_maintained` now calls `calculate_on`, so the
    /// two agree by construction and a fault inside the shared implementation moves both — a
    /// mutation there does NOT fail this test, which was tried rather than assumed. What it
    /// catches is the two paths DIVERGING: the moment somebody gives the wrapper an implementation
    /// of its own, this pins them back together.
    ///
    /// The claim that sharing the evaluator did not change the ANSWER rests on
    /// `validate-lighting.ps1`, which compares against DIALux on three fully specified rooms and
    /// is independent of both routes. That is the check which means something here.
    ///
    /// Agreement is asserted to the BIT: the evaluator is deterministic by design, so anything
    /// short of exact equality would be a real difference hiding behind a tolerance.
    #[test]
    fn sharing_the_evaluator_does_not_change_the_grid() {
        let (meshes, profiles, lums, plane, settings) = scene(1);
        let mats = default_materials();
        let own = calculate_maintained(
            &meshes, &lums, &profiles, &mats, &plane, &settings, Maintenance::INITIAL,
        );
        let ev = Evaluator::new(&meshes, &lums, &profiles, &mats, settings, Maintenance::INITIAL);
        let shared = calculate_on(&ev, &plane, Maintenance::INITIAL);

        assert_eq!((own.cols, own.rows), (shared.cols, shared.rows));
        assert_eq!(
            own.values, shared.values,
            "the shared evaluator produced a different grid, avg {:.4} against {:.4}",
            shared.avg, own.avg,
        );
    }

    /// AND THE SAME SURFACE REPORT — with the same caveat as above: `surface_report` delegates to
    /// `surface_report_on`, so this pins that they stay one implementation rather than proving
    /// that implementation right.
    #[test]
    fn sharing_the_evaluator_does_not_change_the_surface_report() {
        let (meshes, profiles, lums, _plane, settings) = scene(1);
        let mats = default_materials();
        let own = surface_report(
            &meshes, &lums, &profiles, &mats, &settings, Maintenance::INITIAL, 1.0,
        );
        let ev = Evaluator::new(&meshes, &lums, &profiles, &mats, settings, Maintenance::INITIAL);
        let shared = surface_report_on(&ev, &meshes, &lums, &mats, 1.0);

        assert_eq!(own.len(), shared.len(), "a surface went missing");
        for (a, b) in own.iter().zip(shared.iter()) {
            assert_eq!(a.material, b.material);
            assert_eq!(a.samples, b.samples, "{}: sample count differs", a.name);
            assert!(
                (a.e_avg - b.e_avg).abs() < 1e-9,
                "{}: {:.6} lx against {:.6} lx",
                a.name, a.e_avg, b.e_avg,
            );
        }
    }

    /// MAINTENANCE STILL REACHES THE ANSWER by the shared route. It is applied inside the
    /// evaluator, so a caller passing it to the wrapper and not to `calculate_on` would get an
    /// initial-condition grid labelled as maintained — and every number on it would be too high.
    #[test]
    fn the_maintenance_factor_survives_the_shared_route() {
        let (meshes, profiles, lums, plane, settings) = scene(1);
        let mats = default_materials();
        let mf = Maintenance { llmf: 0.9, lsf: 1.0, lmf: 0.9, rsmf: 1.0 };
        assert!(mf.factor() < 0.9, "the fixture must actually derate: {}", mf.factor());

        let ev = Evaluator::new(&meshes, &lums, &profiles, &mats, settings, mf);
        let maintained = calculate_on(&ev, &plane, mf);
        let ev0 = Evaluator::new(&meshes, &lums, &profiles, &mats, settings, Maintenance::INITIAL);
        let initial = calculate_on(&ev0, &plane, Maintenance::INITIAL);

        assert!(
            (maintained.avg / initial.avg - mf.factor()).abs() < 1e-6,
            "maintained/initial came to {:.4}, not the factor {:.4}",
            maintained.avg / initial.avg, mf.factor(),
        );
        assert!((maintained.maintenance - mf.factor()).abs() < 1e-9, "the grid mislabels its MF");
    }
}

/// What one room surface receives, and gives back.
///
/// EN 12464-1 does not stop at the work plane: it sets maintained illuminance and uniformity for
/// WALLS and CEILINGS too — an office wants roughly 50 lx on walls and 30 lx on the ceiling, each
/// at U₀ ≥ 0.10 — and a scheme that passes on the desk can still fail on those. Luminance is the
/// quantity the appearance clauses are written in, and for a Lambertian surface it is `ρE/π`.
#[derive(Debug, Clone)]
pub struct SurfaceResult {
    pub material: crate::types::MaterialId,
    pub name: String,
    /// Total area of the surfaces sharing this material (m²).
    pub area_m2: f64,
    /// Area-weighted mean illuminance ON the surface (lx).
    pub e_avg: f64,
    pub e_min: f64,
    pub e_max: f64,
    /// Area-weighted mean luminance, `ρ·E/π` (cd/m²).
    pub l_avg: f64,
    /// `E_min / E_avg` over the samples taken on this surface.
    pub u0: f64,
    /// Samples taken. Reported because a coarse sample makes a minimum look too high here for
    /// exactly the reason it does on the work plane.
    pub samples: usize,
}

/// The most points any ONE material's surfaces are sampled at, however much of it there is.
///
/// These are four room-average figures quoted to the nearest lux. A thousand stratified,
/// area-weighted samples settle such a number long before the tracing cost stops mattering, and
/// without a cap a site plan would ask for one per square metre of a hundred thousand.
pub const MAX_SURFACE_SAMPLES: usize = 1_000;

/// The `n`-th term of the van der Corput sequence in `base` — `n` written in that base, reflected
/// about the point. A cheap, deterministic, low-discrepancy source: consecutive terms fall in the
/// gaps the earlier ones left, so a handful of samples covers a surface far more evenly than the
/// same number drawn at random, and two runs of the same scene agree exactly.
fn radical_inverse(mut n: u64, base: u64) -> f64 {
    let (mut out, mut inv) = (0.0_f64, 1.0_f64 / base as f64);
    while n > 0 {
        out += (n % base) as f64 * inv;
        n /= base;
        inv /= base as f64;
    }
    out
}

/// Illuminance and luminance on every room surface, grouped by material.
///
/// `samples_per_m2` sets the density. Points are placed on a stratified barycentric pattern rather
/// than at centroids — a centroid-only sample of a large wall reports the middle of it and calls
/// that the minimum.
///
/// THE COST IS SET BY AREA, NOT BY TRIANGLE COUNT, and it is worth saying because it used to be
/// the other way round while this comment claimed otherwise. Every triangle got AT LEAST ONE
/// sample — `clamp(ceil(area * samples_per_m2), 1, 64)`, and the floor of 1 was the whole defect.
/// A 450,000-triangle chair covering 2 m² took 450,000 ray-traced evaluations instead of 2.
///
/// On the owner's real project that was 7,036,129 evaluations, each traced against a
/// seven-million-triangle BVH, and the calculation simply never came back: fifteen minutes in a
/// release build, which Windows greys out and a person reports as a crash.
///
/// So the triangles of a material are now sampled IN PROPORTION TO THEIR AREA, up to
/// [`MAX_SURFACE_SAMPLES`]. Selection walks the cumulative area at evenly spaced offsets — a
/// systematic area-weighted sample, deterministic and low-discrepancy, with no RNG to make one run
/// disagree with the next. A triangle a thousand times bigger than its neighbour gets a thousand
/// times the samples; one too small to earn a whole sample contributes its area and waits its turn
/// rather than costing a full trace.
pub fn surface_report(
    meshes: &[Mesh],
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    materials: &[Material],
    settings: &RaySettings,
    maintenance: Maintenance,
    samples_per_m2: f64,
) -> Vec<SurfaceResult> {
    let ev = Evaluator::new(meshes, luminaires, profiles, materials, *settings, maintenance);
    surface_report_on(&ev, meshes, luminaires, materials, samples_per_m2)
}

/// [`surface_report`] against an EVALUATOR THAT ALREADY EXISTS — see [`calculate_on`] for why that
/// matters more than it looks.
///
/// `meshes` is still needed, and not as a duplicate of what the evaluator holds: this walks the
/// triangles for their AREAS, which is a question about the geometry rather than about the light.
pub fn surface_report_on(
    ev: &Evaluator,
    meshes: &[Mesh],
    luminaires: &[Luminaire],
    materials: &[Material],
    samples_per_m2: f64,
) -> Vec<SurfaceResult> {
    // PASS ONE: the areas, with no tracing at all. Cheap even at seven million triangles, and it
    // is what makes an area-proportional sample possible — you cannot weight by a total you have
    // not got yet.
    //
    // `(material, mesh index, triangle index, area, cumulative area within the material)`.
    let mut by_mat: std::collections::BTreeMap<u32, (f64, Vec<(usize, usize, f64, f64)>)> =
        Default::default();
    for (mi, mesh) in meshes.iter().enumerate() {
        for (ti, tri) in mesh.triangles.iter().enumerate() {
            let (a, b, c) = (
                v3(mesh.vertices[tri.a as usize]),
                v3(mesh.vertices[tri.b as usize]),
                v3(mesh.vertices[tri.c as usize]),
            );
            let area = 0.5 * (b - a).cross(c - a).length() as f64;
            if area <= 1e-9 {
                continue;
            }
            let e = by_mat.entry(mesh.material).or_default();
            e.0 += area;
            e.1.push((mi, ti, area, e.0));
        }
    }

    // material id -> (area, sum of E*area, min, max, samples)
    let mut acc: std::collections::BTreeMap<u32, (f64, f64, f64, f64, usize)> = Default::default();

    for (mat, (total_area, tris)) in &by_mat {
        if tris.is_empty() || *total_area <= 0.0 {
            continue;
        }
        let want = ((total_area * samples_per_m2).ceil() as usize).clamp(1, MAX_SURFACE_SAMPLES);
        let e = acc.entry(*mat).or_insert((0.0, 0.0, f64::MAX, 0.0, 0));
        // Each sample stands for the same slice of area, because selection is area-weighted — so
        // the weighted average is the plain mean of the samples, and the area reported is the
        // material's REAL total rather than the part that happened to be sampled.
        let share = total_area / want as f64;
        for k in 0..want {
            // Systematic sampling: one offset per stratum, at the stratum's midpoint. The k-th
            // sample lands in whichever triangle spans that much cumulative area.
            let target = total_area * (k as f64 + 0.5) / want as f64;
            let idx = tris
                .partition_point(|(_, _, _, cum)| *cum < target)
                .min(tris.len() - 1);
            let (mi, ti, _area, _) = tris[idx];
            let mesh = &meshes[mi];
            let tri = &mesh.triangles[ti];
            let (a, b, c) = (
                v3(mesh.vertices[tri.a as usize]),
                v3(mesh.vertices[tri.b as usize]),
                v3(mesh.vertices[tri.c as usize]),
            );
            // The INWARD normal — the side facing the room, and so the side that receives light. A
            // surface sampled on its back reads zero and would quietly drag a wall's average down.
            let mut n = (b - a).cross(c - a).normalize_or_zero();
            if let Some(l) = luminaires.first() {
                let centroid = (a + b + c) / 3.0;
                if (v3(l.position) - centroid).dot(n) < 0.0 {
                    n = -n;
                }
            }
            // WHERE IN THE TRIANGLE, from a 2D low-discrepancy sequence in bases 2 and 3.
            //
            // NOT derived from `k / want`, which is what the area walk above already uses. Tying
            // the two together correlates a point's position WITHIN its triangle to that
            // triangle's position ALONG the surface, and the estimator stops being uniform over
            // the area. Measured: a 4 × 4 m floor reported 197 lx, and 256 lx when the very same
            // surface was subdivided — the tessellation changing the answer, which is the defect
            // this function was being fixed for, arriving through a different door.
            //
            // The mapping is the standard one for a uniform point in a triangle. The `sqrt` on the
            // first coordinate is what corrects for the triangle narrowing towards `a`.
            let (u1, u2) = (radical_inverse(k as u64 + 1, 2), radical_inverse(k as u64 + 1, 3));
            let su = u1.sqrt();
            let (w0, w1, w2) = (1.0 - su, su * (1.0 - u2), su * u2);
            let p = a * w0 as f32 + b * w1 as f32 + c * w2 as f32;
            // Lifted off the surface, or the point is shadowed by the triangle it sits on.
            let lx = ev.illuminance(p + n * 1.0e-3, n);
            e.0 += share;
            e.1 += lx * share;
            e.2 = e.2.min(lx);
            e.3 = e.3.max(lx);
            e.4 += 1;
        }
    }


    acc.into_iter()
        .map(|(id, (area, sum, min, max, samples))| {
            let mat = materials.iter().find(|m| m.id == id);
            let rho = mat.map(|m| m.reflectance as f64).unwrap_or(0.0);
            let e_avg = if area > 0.0 { sum / area } else { 0.0 };
            SurfaceResult {
                material: id,
                name: mat.map(|m| m.name.clone()).unwrap_or_else(|| format!("material {id}")),
                area_m2: area,
                e_avg,
                e_min: if min.is_finite() { min } else { 0.0 },
                e_max: max,
                l_avg: rho * e_avg / PI,
                u0: if e_avg > 0.0 { min / e_avg } else { 0.0 },
                samples,
            }
        })
        .collect()
}

/// SURFACE ILLUMINANCE AND LUMINANCE.
///
/// EN 12464-1 sets maintained levels for walls and ceilings, not only the work plane, and a scheme
/// that passes on the desk can still fail on those. The engine could already answer this one point
/// at a time; a report needs the surface.
#[cfg(test)]
mod surface_tests {
    use super::*;
    use crate::extrude::box_room;
    use crate::types::default_materials;

    fn isotropic(cd: f64) -> IesProfile {
        IesProfile {
            name: "iso".into(),
            photometry: crate::ies::PhotometryType::C,
            lumens: 4.0 * PI * cd,
            multiplier: 1.0,
            vertical_angles: vec![0.0, 90.0, 180.0],
            horizontal_angles: vec![0.0, 360.0],
            candela: vec![vec![cd, cd, cd], vec![cd, cd, cd]],
            watts: 1.0,
            width: 0.0,
            length: 0.0,
            height: 0.0,
            luminous_length: 0.0,
            luminous_width: 0.0,
        }
    }

    fn lamp(x: f32, y: f32, z: f32) -> Vec<Luminaire> {
        vec![Luminaire {
            id: 1,
            profile: "iso".into(),
            position: Vertex::new(x, y, z),
            rotation_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }]
    }

    fn prof(cd: f64) -> HashMap<String, IesProfile> {
        let mut m = HashMap::new();
        m.insert("iso".to_string(), isotropic(cd));
        m
    }

    /// THE CLOSED-FORM CHECK. In a closed enclosure of total area `A` and UNIFORM reflectance `ρ`,
    /// a flux `Φ` produces an area-weighted mean surface illuminance of `Φ / (A(1−ρ))` — every
    /// photon lands eventually, having bounced `1/(1−ρ)` times on average.
    ///
    /// Independent of the engine: it follows from conservation of energy, not from any part of the
    /// code under test. Agreement means the sampling, the areas and the interreflection are all
    /// right together.
    #[test]
    fn surface_illuminance_matches_the_radiosity_closed_form() {
        const S: f32 = 4.0;
        const RHO: f32 = 0.5;
        let cd = 100.0;
        let flux = 4.0 * PI * cd; // isotropic
        let area = 6.0 * (S as f64).powi(2);
        let expected = flux / (area * (1.0 - RHO as f64));

        let meshes = box_room(S, S, S);
        let materials: Vec<Material> =
            default_materials().into_iter().map(|m| Material { reflectance: RHO, ..m }).collect();
        // Enough bounces for a rho = 0.5 room to converge (mean path is 1/(1-rho) = 2), and enough
        // rays to average out; more of either only costs seconds.
        let settings = RaySettings { rays_per_point: 384, max_bounces: 14, shadows: true };
        let rows = surface_report(
            &meshes,
            &lamp(S * 0.5, S * 0.5, S * 0.5),
            &prof(cd),
            &materials,
            &settings,
            Maintenance::INITIAL,
            2.0,
        );

        let total_area: f64 = rows.iter().map(|r| r.area_m2).sum();
        let weighted =
            rows.iter().map(|r| r.e_avg * r.area_m2).sum::<f64>() / total_area;
        assert!(
            (total_area - area).abs() < 1e-3,
            "the six faces should total {area} m2, got {total_area}",
        );
        assert!(
            (weighted - expected).abs() / expected < 0.05,
            "mean surface illuminance {weighted:.1} lx against the closed form {expected:.1}",
        );
    }

    /// Luminance is `ρE/π`, and surfaces differ in ρ — so the BRIGHTEST surface need not be the
    /// most lit one. A ceiling at 0.70 outshines a floor at 0.20 receiving comparable light.
    #[test]
    fn luminance_follows_reflectance_not_just_illuminance() {
        let meshes = box_room(4.0, 4.0, 3.0);
        let materials = default_materials(); // floor 0.20, wall 0.50, ceiling 0.70
        let settings = RaySettings { rays_per_point: 512, max_bounces: 6, shadows: true };
        let rows = surface_report(
            &meshes,
            &lamp(2.0, 2.0, 1.5),
            &prof(100.0),
            &materials,
            &settings,
            Maintenance::INITIAL,
            2.0,
        );

        let get = |n: &str| rows.iter().find(|r| r.name == n).expect(n).clone();
        let (floor, ceil) = (get("Floor"), get("Ceiling"));
        // The source is centred, so floor and ceiling receive comparable illuminance...
        assert!(
            (floor.e_avg - ceil.e_avg).abs() / floor.e_avg < 0.35,
            "floor {:.0} lx vs ceiling {:.0} lx",
            floor.e_avg,
            ceil.e_avg,
        );
        // ...but the ceiling is far brighter to look at.
        assert!(
            ceil.l_avg > floor.l_avg * 2.0,
            "ceiling {:.1} cd/m2 should outshine floor {:.1}",
            ceil.l_avg,
            floor.l_avg,
        );
        // And every row carries the identity it claims.
        for r in &rows {
            let rho = materials.iter().find(|m| m.id == r.material).unwrap().reflectance as f64;
            assert!((r.l_avg - rho * r.e_avg / PI).abs() < 1e-9, "{} broke L = rho E / pi", r.name);
        }
    }

    /// Every surface is reported, with its real area. A wall silently missing would look like a
    /// compliant room.
    #[test]
    fn every_surface_is_accounted_for() {
        let meshes = box_room(4.0, 5.0, 3.0);
        let materials = default_materials();
        let settings = RaySettings { rays_per_point: 128, max_bounces: 2, shadows: true };
        let rows = surface_report(
            &meshes,
            &lamp(2.0, 2.5, 1.5),
            &prof(50.0),
            &materials,
            &settings,
            Maintenance::INITIAL,
            1.0,
        );
        assert_eq!(rows.len(), 3, "floor, walls and ceiling");
        let get = |n: &str| rows.iter().find(|r| r.name == n).expect(n).clone();
        assert!((get("Floor").area_m2 - 20.0).abs() < 1e-3);
        assert!((get("Ceiling").area_m2 - 20.0).abs() < 1e-3);
        // Four walls: 2*(4*3) + 2*(5*3) = 54 m2.
        assert!((get("Wall").area_m2 - 54.0).abs() < 1e-3, "got {}", get("Wall").area_m2);
        for r in &rows {
            assert!(r.samples > 0 && r.e_avg > 0.0, "{} was never sampled", r.name);
            assert!(
                r.e_min <= r.e_avg && r.e_avg <= r.e_max,
                "{}: min/avg/max out of order",
                r.name
            );
        }
    }

    // ── THE COST IS THE AREA, NOT THE TRIANGLE COUNT ───────────────────────────────────────
    //
    // Every triangle used to get AT LEAST ONE ray-traced sample — `clamp(…, 1, 64)`, and the floor
    // of 1 was the defect. A 450,000-triangle chair covering 2 m² took 450,000 evaluations instead
    // of 2, and on the owner's real 7,036,129-triangle project the calculation never came back:
    // fifteen minutes in a release build, which Windows greys out and a person reports as a crash.

    /// Split every triangle of `meshes` into 4 by mid-edge subdivision, `times` over — so the same
    /// SURFACE arrives as 4^times as many triangles. The geometry is identical; only the
    /// tessellation changes, which is exactly the thing the cost must not follow.
    fn subdivide(meshes: &[Mesh], times: u32) -> Vec<Mesh> {
        let mut out = meshes.to_vec();
        for _ in 0..times {
            out = out
                .iter()
                .map(|m| {
                    let mut verts = m.vertices.clone();
                    let mut tris = Vec::with_capacity(m.triangles.len() * 4);
                    for t in &m.triangles {
                        let (ia, ib, ic) = (t.a as usize, t.b as usize, t.c as usize);
                        let mid = |p: Vertex, q: Vertex| {
                            Vertex::new(
                                0.5 * (p.x + q.x),
                                0.5 * (p.y + q.y),
                                0.5 * (p.z + q.z),
                            )
                        };
                        let base = verts.len() as u32;
                        verts.push(mid(m.vertices[ia], m.vertices[ib]));
                        verts.push(mid(m.vertices[ib], m.vertices[ic]));
                        verts.push(mid(m.vertices[ic], m.vertices[ia]));
                        let (ab, bc, ca) = (base, base + 1, base + 2);
                        for (a, b, c) in [
                            (t.a, ab, ca), (ab, t.b, bc), (ca, bc, t.c), (ab, bc, ca),
                        ] {
                            tris.push(crate::types::Triangle { a, b, c });
                        }
                    }
                    Mesh { vertices: verts, triangles: tris, material: m.material }
                })
                .collect();
        }
        out
    }

    fn report_at(meshes: &[Mesh], per_m2: f64) -> Vec<SurfaceResult> {
        let mut profiles = HashMap::new();
        profiles.insert("iso".to_string(), isotropic(1000.0));
        surface_report(
            meshes,
            &lamp(2.0, 2.0, 2.5),
            &profiles,
            &default_materials(),
            &RaySettings { rays_per_point: 16, max_bounces: 1, shadows: true },
            Maintenance::INITIAL,
            per_m2,
        )
    }

    fn report_of(meshes: &[Mesh]) -> Vec<SurfaceResult> {
        report_at(meshes, 1.0)
    }

    /// THE HEADLINE. The same room, tessellated 64× more finely, must not cost 64× more.
    #[test]
    fn subdividing_a_surface_does_not_multiply_the_work() {
        let coarse = box_room(4.0, 4.0, 3.0);
        let fine = subdivide(&coarse, 3); // 4^3 = 64x the triangles, the same room
        let coarse_tris: usize = coarse.iter().map(|m| m.triangles.len()).sum();
        let fine_tris: usize = fine.iter().map(|m| m.triangles.len()).sum();
        assert_eq!(fine_tris, coarse_tris * 64, "the fixture must really subdivide");

        let a: usize = report_of(&coarse).iter().map(|s| s.samples).sum();
        let b: usize = report_of(&fine).iter().map(|s| s.samples).sum();
        assert_eq!(
            a, b,
            "{coarse_tris} triangles took {a} samples and the SAME SURFACE at {fine_tris} \
             triangles took {b} — the cost is following the tessellation, not the area",
        );
    }

    /// AND IT DOES NOT CHANGE THE ANSWER EITHER. Cheap and wrong is not the goal: the same room
    /// must report the same illuminance however finely it happens to be cut up.
    ///
    /// AT A DENSITY WHERE THE COMPARISON MEANS SOMETHING. At the default 1 sample/m² a 16 m² floor
    /// gets sixteen points, and two sixteen-point estimates of a stochastic quantity differ by
    /// more than any tolerance worth writing — so a loose tolerance here would pass whatever the
    /// sampling did. Twenty per square metre puts 320 points on that floor, where a 2% band is a
    /// real constraint. This is the tessellation-independence of the ESTIMATOR, not a statement
    /// about how many samples the app happens to ask for.
    #[test]
    fn subdividing_a_surface_does_not_change_what_it_reports() {
        let coarse = box_room(4.0, 4.0, 3.0);
        let fine = subdivide(&coarse, 3);
        let (a, b) = (report_at(&coarse, 20.0), report_at(&fine, 20.0));
        assert_eq!(a.len(), b.len(), "the same materials must be reported");
        for (x, y) in a.iter().zip(b.iter()) {
            assert!(
                (x.area_m2 - y.area_m2).abs() < 1e-3,
                "{}: area {:.3} m² became {:.3} m² under subdivision",
                x.name, x.area_m2, y.area_m2,
            );
            assert!(
                (x.e_avg - y.e_avg).abs() <= 0.02 * x.e_avg.max(1.0),
                "{}: {:.1} lx became {:.1} lx under subdivision",
                x.name, x.e_avg, y.e_avg,
            );
        }
    }

    /// THE BUDGET IS A CEILING. Without one, a site plan asks for a sample per square metre of a
    /// hundred thousand, and the cap is what stops the fix having its own runaway.
    #[test]
    fn no_material_is_sampled_past_the_budget() {
        // A 200 x 200 m "room": 40,000 m² of floor alone, at 1 sample/m².
        let huge = box_room(200.0, 200.0, 4.0);
        for s in report_of(&huge) {
            assert!(
                s.samples <= MAX_SURFACE_SAMPLES,
                "{} took {} samples against a budget of {}",
                s.name, s.samples, MAX_SURFACE_SAMPLES,
            );
        }
    }

    /// THE AREA REPORTED IS THE REAL AREA, not the part that happened to be sampled. It is what
    /// the average is weighted by and what the report prints, so a budget that quietly shrank it
    /// would understate every surface in the building.
    #[test]
    fn the_area_reported_is_the_whole_surface_even_when_sampling_is_capped() {
        let huge = box_room(200.0, 200.0, 4.0);
        let floor = report_of(&huge)
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case("floor"))
            .expect("a floor");
        assert!(
            (floor.area_m2 - 40_000.0).abs() < 1.0,
            "a 200 x 200 m floor reported {:.0} m²",
            floor.area_m2,
        );
        assert!(floor.samples <= MAX_SURFACE_SAMPLES, "…and was still capped");
    }

    /// SAMPLES FOLLOW AREA. A wall ten times the size of its neighbour must get about ten times
    /// the samples — otherwise "area-weighted" is a claim rather than a behaviour, and a big dark
    /// surface would count for as little as a small bright one.
    #[test]
    fn a_bigger_surface_gets_proportionally_more_samples() {
        let small = report_of(&box_room(2.0, 2.0, 3.0));
        let big = report_of(&box_room(20.0, 20.0, 3.0));
        let floor = |v: &[SurfaceResult]| {
            v.iter().find(|s| s.name.eq_ignore_ascii_case("floor")).map(|s| (s.area_m2, s.samples))
        };
        let (sa, ss) = floor(&small).expect("small floor");
        let (ba, bs) = floor(&big).expect("big floor");
        assert!(ba > sa * 50.0, "precondition: {ba} m² against {sa} m²");
        assert!(
            bs > ss * 10,
            "a {ba:.0} m² floor took {bs} samples and a {sa:.0} m² one took {ss} — samples are \
             not following area",
        );
    }

    /// DETERMINISTIC. Selection walks the cumulative area at fixed offsets rather than using an
    /// RNG, so a designer who re-runs a calculation and sees a different number has changed
    /// something — they are not looking at sampling noise.
    #[test]
    fn the_same_scene_reports_the_same_numbers_twice() {
        let room = box_room(6.0, 4.0, 3.0);
        let (a, b) = (report_of(&room), report_of(&room));
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.samples, y.samples, "{}: sample count wobbled", x.name);
            assert!((x.e_avg - y.e_avg).abs() < 1e-9, "{}: {} vs {}", x.name, x.e_avg, y.e_avg);
        }
    }
}
