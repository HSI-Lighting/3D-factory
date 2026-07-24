//! The lux calculation engine: direct illuminance (inverse-square + cosine,
//! shadow-tested) plus Monte-Carlo one-bounce+ indirect (Lambertian reflection).
use std::collections::HashMap;
use std::f64::consts::PI;

use glam::Vec3;
use rayon::prelude::*;

use crate::ies::IesProfile;
use crate::rt::{cosine_sample, Ray, Rng, RtScene, Tri};
use crate::types::{CalcPlane, LuxGrid, Luminaire, Material, MaterialId, Mesh, RaySettings, Vertex};

const EPS: f32 = 1e-3;

fn v3(v: Vertex) -> Vec3 {
    v.to_vec3()
}

struct Ctx<'a> {
    scene: &'a RtScene,
    luminaires: &'a [Luminaire],
    profiles: &'a HashMap<String, IesProfile>,
    materials: &'a [Material],
    settings: &'a RaySettings,
}

impl Ctx<'_> {
    fn reflectance(&self, id: MaterialId) -> f64 {
        self.materials.iter().find(|m| m.id == id).map(|m| m.reflectance as f64).unwrap_or(0.5)
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
    prof.intensity(gamma, phi) * lum.dimming as f64
}

/// Direct illuminance (lux) at a surface point with the given outward `normal`.
fn direct(ctx: &Ctx, point: Vec3, normal: Vec3) -> f64 {
    let mut e = 0.0;
    for lum in ctx.luminaires {
        let Some(prof) = ctx.profiles.get(&lum.profile) else {
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
        if ctx.settings.shadows && ctx.scene.occluded(point + normal * EPS, lpos) {
            continue;
        }
        e += intensity * cos_inc / (dist as f64 * dist as f64);
    }
    e
}

/// Total illuminance (direct + up to `bounces` diffuse reflections) at a point.
fn illuminance(ctx: &Ctx, point: Vec3, normal: Vec3, bounces: u32, rng: &mut Rng) -> f64 {
    let e = direct(ctx, point, normal);
    if bounces == 0 {
        return e;
    }
    let n = ctx.settings.rays_per_point.max(1);
    let mut acc = 0.0;
    for _ in 0..n {
        let w = cosine_sample(normal, rng);
        let Some(hit) = ctx.scene.closest_hit(&Ray { o: point + normal * EPS, d: w }) else {
            continue;
        };
        let rho = ctx.reflectance(hit.material);
        if rho <= 0.0 {
            continue;
        }
        let wn = if hit.normal.dot(w) < 0.0 { hit.normal } else { -hit.normal };
        let e_surface = illuminance(ctx, hit.point, wn, bounces - 1, rng);
        acc += rho * e_surface / PI;
    }
    e + acc * PI / n as f64
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

/// Compute the lux grid over `plane`, given the scene meshes, luminaires, their
/// IES profiles, and materials. The plane faces up (+Z). rayon-parallel.
pub fn calculate(
    meshes: &[Mesh],
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    materials: &[Material],
    plane: &CalcPlane,
    settings: &RaySettings,
) -> LuxGrid {
    let scene = RtScene::new(build_tris(meshes));
    let ctx = Ctx { scene: &scene, luminaires, profiles, materials, settings };
    let cols = plane.cols.max(1);
    let rows = plane.rows.max(1);
    let bounces = settings.max_bounces;
    let normal = Vec3::Z;
    let count = (cols * rows) as usize;

    let values: Vec<f64> = (0..count)
        .into_par_iter()
        .map(|i| {
            let (col, row) = (i as u32 % cols, i as u32 / cols);
            let p = v3(plane.sample_point(col, row));
            let mut rng = Rng::seeded((i as u64).wrapping_mul(0x9E3779B9_7F4A7C15) ^ 0xD1B54A3);
            illuminance(&ctx, p, normal, bounces, &mut rng)
        })
        .collect();

    LuxGrid::from_values(cols, rows, values)
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
        }
    }

    fn scene(bounces: u32) -> (Vec<Mesh>, HashMap<String, IesProfile>, Vec<Luminaire>, CalcPlane, RaySettings) {
        let (w, d, h) = (4.0f32, 4.0f32, 3.0f32);
        let meshes = extrude::box_room(w, d, h);
        let mut profiles = HashMap::new();
        profiles.insert("flat".into(), flat_1000cd());
        let lums = vec![Luminaire { id: 1, profile: "flat".into(), position: Vertex::new(w / 2.0, d / 2.0, h), rotation_deg: 0.0, dimming: 1.0 }];
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
}
