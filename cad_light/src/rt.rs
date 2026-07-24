//! A small, dependency-light ray tracer: triangle intersection + a median-split
//! BVH, plus a deterministic RNG and cosine-weighted hemisphere sampling. Pure
//! `glam` f32 — scenes are rooms, not film assets, so a compact BVH suffices.
use glam::Vec3;
use std::f32::consts::TAU;

pub struct Ray {
    pub o: Vec3,
    pub d: Vec3,
}

#[derive(Clone, Copy)]
pub struct Tri {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    pub material: u32,
}

impl Tri {
    pub fn normal(&self) -> Vec3 {
        (self.b - self.a).cross(self.c - self.a).normalize()
    }
    pub fn centroid(&self) -> Vec3 {
        (self.a + self.b + self.c) / 3.0
    }
    /// Möller–Trumbore; returns the hit distance if within `(tmin, tmax)`.
    pub fn intersect(&self, r: &Ray, tmin: f32, tmax: f32) -> Option<f32> {
        let (e1, e2) = (self.b - self.a, self.c - self.a);
        let p = r.d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-8 {
            return None;
        }
        let inv = 1.0 / det;
        let tv = r.o - self.a;
        let u = tv.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = tv.cross(e1);
        let v = r.d.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = e2.dot(q) * inv;
        (t > tmin && t < tmax).then_some(t)
    }
}

#[derive(Clone, Copy)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

impl Aabb {
    fn empty() -> Self {
        Self { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) }
    }
    fn union_pt(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    fn union(&mut self, o: &Aabb) {
        self.min = self.min.min(o.min);
        self.max = self.max.max(o.max);
    }
    fn of(t: &Tri) -> Self {
        let mut a = Self::empty();
        a.union_pt(t.a);
        a.union_pt(t.b);
        a.union_pt(t.c);
        a
    }
    fn hit(&self, r: &Ray, inv_d: Vec3, tmin: f32, tmax: f32) -> bool {
        let t0 = (self.min - r.o) * inv_d;
        let t1 = (self.max - r.o) * inv_d;
        let lo = t0.min(t1).max_element().max(tmin);
        let hi = t0.max(t1).min_element().min(tmax);
        lo <= hi
    }
}

struct Node {
    aabb: Aabb,
    left: u32,
    right: u32,
    start: u32,
    count: u32,
}

pub struct RtScene {
    tris: Vec<Tri>,
    idx: Vec<u32>,
    nodes: Vec<Node>,
}

pub struct Hit {
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    pub material: u32,
}

const LEAF: u32 = 4;

impl RtScene {
    pub fn new(tris: Vec<Tri>) -> Self {
        let n = tris.len() as u32;
        let mut idx: Vec<u32> = (0..n).collect();
        let mut nodes = Vec::new();
        if n > 0 {
            build(&tris, &mut idx, &mut nodes, 0, n);
        }
        Self { tris, idx, nodes }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    pub fn closest_hit(&self, r: &Ray) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv_d = r.d.recip();
        let mut best_t = f32::INFINITY;
        let mut best: Option<u32> = None;
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        stack[0] = 0;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if !node.aabb.hit(r, inv_d, 1e-4, best_t) {
                continue;
            }
            if node.count > 0 {
                for k in node.start..node.start + node.count {
                    let ti = self.idx[k as usize];
                    if let Some(t) = self.tris[ti as usize].intersect(r, 1e-4, best_t) {
                        best_t = t;
                        best = Some(ti);
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = node.left;
                stack[sp + 1] = node.right;
                sp += 2;
            }
        }
        best.map(|ti| {
            let tri = &self.tris[ti as usize];
            Hit { t: best_t, point: r.o + r.d * best_t, normal: tri.normal(), material: tri.material }
        })
    }

    pub fn occluded(&self, from: Vec3, to: Vec3) -> bool {
        let d = to - from;
        let dist = d.length();
        if dist < 1e-5 || self.nodes.is_empty() {
            return false;
        }
        let r = Ray { o: from, d: d / dist };
        let inv_d = r.d.recip();
        let tmax = dist - 1e-3;
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        stack[0] = 0;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if !node.aabb.hit(&r, inv_d, 1e-3, tmax) {
                continue;
            }
            if node.count > 0 {
                for k in node.start..node.start + node.count {
                    let ti = self.idx[k as usize] as usize;
                    if self.tris[ti].intersect(&r, 1e-3, tmax).is_some() {
                        return true;
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = node.left;
                stack[sp + 1] = node.right;
                sp += 2;
            }
        }
        false
    }
}

fn build(tris: &[Tri], idx: &mut [u32], nodes: &mut Vec<Node>, start: u32, count: u32) -> u32 {
    let self_i = nodes.len() as u32;
    nodes.push(Node { aabb: Aabb::empty(), left: 0, right: 0, start, count });

    let (mut bb, mut cb) = (Aabb::empty(), Aabb::empty());
    for &t in idx[start as usize..(start + count) as usize].iter() {
        bb.union(&Aabb::of(&tris[t as usize]));
        cb.union_pt(tris[t as usize].centroid());
    }
    nodes[self_i as usize].aabb = bb;

    if count <= LEAF {
        return self_i;
    }

    let ext = cb.max - cb.min;
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    let mid = 0.5 * (cb.min[axis] + cb.max[axis]);

    let (s, c) = (start as usize, count as usize);
    let slice = &mut idx[s..s + c];
    let mut i = 0usize;
    let mut j = c;
    while i < j {
        if tris[slice[i] as usize].centroid()[axis] < mid {
            i += 1;
        } else {
            j -= 1;
            slice.swap(i, j);
        }
    }
    let mut m = i as u32;
    if m == 0 || m == count {
        m = count / 2;
    }

    let left = build(tris, idx, nodes, start, m);
    let right = build(tris, idx, nodes, start + m, count - m);
    let node = &mut nodes[self_i as usize];
    node.left = left;
    node.right = right;
    node.count = 0;
    self_i
}

/// A tiny deterministic PRNG (xorshift64*). Seed per work item for reproducibility.
pub struct Rng(u64);

impl Rng {
    pub fn seeded(seed: u64) -> Self {
        Rng(seed | 1)
    }
    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F491_4F6CDD1D)
    }
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) * (1.0 / (1u32 << 24) as f32)
    }
}

fn onb(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.z.abs() < 0.999 { Vec3::Z } else { Vec3::X };
    let t = a.cross(n).normalize();
    (t, n.cross(t))
}

/// Cosine-weighted hemisphere direction around `normal` (Malley's method).
pub fn cosine_sample(normal: Vec3, rng: &mut Rng) -> Vec3 {
    let (u1, u2) = (rng.next_f32(), rng.next_f32());
    let r = u1.sqrt();
    let (st, ct) = (TAU * u2).sin_cos();
    let (x, y, z) = (r * ct, r * st, (1.0 - u1).max(0.0).sqrt());
    let (t, b) = onb(normal);
    (t * x + b * y + normal * z).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: [f32; 3], b: [f32; 3], c: [f32; 3], m: u32) -> Tri {
        Tri { a: a.into(), b: b.into(), c: c.into(), material: m }
    }

    #[test]
    fn occlusion_and_bvh_match_brute_force() {
        let blocker = tri([-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [0.0, 2.0, 1.0], 0);
        let scene = RtScene::new(vec![blocker]);
        assert!(scene.occluded(Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0)));
        assert!(!scene.occluded(Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0)));

        let mut rng = Rng::seeded(42);
        let mut tris = Vec::new();
        for i in 0..200u32 {
            let base = Vec3::new(rng.next_f32() * 10.0, rng.next_f32() * 10.0, rng.next_f32() * 10.0);
            let o = |r: &mut Rng| Vec3::new(r.next_f32(), r.next_f32(), r.next_f32()) * 0.5;
            tris.push(Tri { a: base, b: base + o(&mut rng), c: base + o(&mut rng), material: i });
        }
        let scene = RtScene::new(tris.clone());
        for _ in 0..64 {
            let o = Vec3::new(rng.next_f32() * 10.0, rng.next_f32() * 10.0, -5.0);
            let d = (Vec3::new(rng.next_f32() * 10.0, rng.next_f32() * 10.0, 10.0) - o).normalize();
            let ray = Ray { o, d };
            let mut best = f32::INFINITY;
            for t in &tris {
                if let Some(h) = t.intersect(&ray, 1e-4, f32::INFINITY) {
                    best = best.min(h);
                }
            }
            let bvh = scene.closest_hit(&ray).map(|h| h.t).unwrap_or(f32::INFINITY);
            assert!((bvh - best).abs() < 1e-3 || (bvh.is_infinite() && best.is_infinite()));
        }
    }
}
