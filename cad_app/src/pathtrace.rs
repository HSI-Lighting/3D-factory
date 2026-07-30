//! In-app **path tracer** — the "Cycles" to the viewport's "EEVEE".
//!
//! One shared render core (scene = BVH over [`crate::radiance_export::ExportTri`], Principled-ish
//! materials, the SAME resolved sun/sky as the raster viewport) with pluggable backends selected by
//! the user in the Render dialog:
//!
//! - [`Device::Cpu`] — implemented here: a progressive, multithreaded (std::thread) tracer. Each
//!   pass adds one sample per pixel into a shared accumulation buffer, so the image refines live and
//!   Cancel is instant.
//! - [`Device::Gpu`] — the same scene packed into texture buffers and traced in a fragment shader
//!   (GL 3.3). Wired through the same [`RenderJob`] socket. (Backend lands next; the UI offers it.)
//!
//! Physics kept intentionally compact: Lambert diffuse + metallic mirror (roughness-jittered),
//! thin-pane glass (fresnel reflect / tinted straight-through transmit — right for archviz window
//! panes), emission, next-event sun sampling WITH glass-aware shadow transmission (sunlight actually
//! comes through the windows), hemispheric sky/ground environment matched to the viewport's, and the
//! same `1 − e⁻ˣ` tone-map so a render lines up with what you see.

use crate::radiance_export::ExportTri;
use glam::Vec3;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Which backend traces the rays. The user picks in the Render dialog (like Blender's
/// Cycles CPU/GPU device switch) — both consume the identical [`Scene`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Device {
    #[default]
    Cpu,
    Gpu,
}

/// The environment light — the SAME resolved values the raster viewport uses
/// ([`crate::factory::SunEnv::resolve`]), so the render matches the view.
#[derive(Clone, Copy, Debug)]
pub struct Sky {
    /// Unit direction TO the sun (export frame).
    pub sun_dir: Vec3,
    pub sun_col: [f32; 3],
    pub sky_col: [f32; 3],
    pub ground_col: [f32; 3],
}

/// Pinhole camera (matches the viewport's eye/target; Z-up).
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_deg: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub w: usize,
    pub h: usize,
    /// Total passes (1 sample/pixel each) the job runs.
    pub passes: u32,
    pub max_depth: u32,
}

// ============================ scene / BVH ============================

#[derive(Clone, Copy)]
struct Tri {
    p0: Vec3,
    e1: Vec3,
    e2: Vec3,
    n: Vec3, // geometric unit normal
    mat: Mat,
}

#[derive(Clone, Copy)]
struct Mat {
    albedo: [f32; 3],
    rough: f32,
    metallic: f32,
    opacity: f32,
    emission: [f32; 3],
}

struct BvhNode {
    mn: Vec3,
    mx: Vec3,
    /// Leaf: `start..start+count` into `order`. Inner: `left` = self+1, `right` stored here.
    right_or_start: u32,
    count: u32, // 0 = inner node
}

/// The traceable scene: triangles + a flat median-split BVH.
pub struct Scene {
    tris: Vec<Tri>,
    order: Vec<u32>,
    nodes: Vec<BvhNode>,
}

impl Scene {
    pub fn build(input: &[ExportTri]) -> Self {
        let mut tris = Vec::with_capacity(input.len());
        for t in input {
            let p0 = Vec3::from(t.verts[0]);
            let p1 = Vec3::from(t.verts[1]);
            let p2 = Vec3::from(t.verts[2]);
            let e1 = p1 - p0;
            let e2 = p2 - p0;
            let n = e1.cross(e2);
            if !n.is_finite() || n.length_squared() < 1e-16 {
                continue; // degenerate
            }
            tris.push(Tri {
                p0,
                e1,
                e2,
                n: n.normalize(),
                mat: Mat {
                    albedo: t.rgb,
                    rough: t.roughness.clamp(0.0, 1.0),
                    metallic: t.metallic.clamp(0.0, 1.0),
                    opacity: t.opacity.clamp(0.0, 1.0),
                    emission: t.emission,
                },
            });
        }
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len().max(1) * 2);
        if !tris.is_empty() {
            build_node(&tris, &mut order, 0, tris.len(), &mut nodes);
        }
        Self { tris, order, nodes }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    /// Pack the scene for the GPU backend: flat RGBA32F texel streams the fragment tracer reads
    /// with `texelFetch`. Layouts (one texel = 4 floats):
    /// - `tris`: 5 texels/tri — [p0|rough] [e1|metallic] [e2|opacity] [albedo|0] [emission|0]
    /// - `nodes`: 2 texels/node — [mn|right_or_start] [mx|count]
    /// - `order`: 1 texel/entry — [tri_index|0|0|0]
    /// Counts stay far below f32's exact-integer range (2²⁴), so indices survive the float trip.
    pub fn pack_gpu(&self) -> GpuPack {
        let mut tris = Vec::with_capacity(self.tris.len() * 20);
        for t in &self.tris {
            tris.extend_from_slice(&[t.p0.x, t.p0.y, t.p0.z, t.mat.rough]);
            tris.extend_from_slice(&[t.e1.x, t.e1.y, t.e1.z, t.mat.metallic]);
            tris.extend_from_slice(&[t.e2.x, t.e2.y, t.e2.z, t.mat.opacity]);
            tris.extend_from_slice(&[t.mat.albedo[0], t.mat.albedo[1], t.mat.albedo[2], 0.0]);
            tris.extend_from_slice(&[t.mat.emission[0], t.mat.emission[1], t.mat.emission[2], 0.0]);
        }
        let mut nodes = Vec::with_capacity(self.nodes.len() * 8);
        for n in &self.nodes {
            nodes.extend_from_slice(&[n.mn.x, n.mn.y, n.mn.z, n.right_or_start as f32]);
            nodes.extend_from_slice(&[n.mx.x, n.mx.y, n.mx.z, n.count as f32]);
        }
        let order = self.order.iter().flat_map(|&i| [i as f32, 0.0, 0.0, 0.0]).collect();
        GpuPack { tris, nodes, order, tri_count: self.tris.len(), node_count: self.nodes.len() }
    }
}

/// The GPU-ready scene streams (see [`Scene::pack_gpu`]).
pub struct GpuPack {
    pub tris: Vec<f32>,
    pub nodes: Vec<f32>,
    pub order: Vec<f32>,
    pub tri_count: usize,
    pub node_count: usize,
}

fn tri_bounds(t: &Tri) -> (Vec3, Vec3) {
    let (a, b, c) = (t.p0, t.p0 + t.e1, t.p0 + t.e2);
    (a.min(b).min(c), a.max(b).max(c))
}

/// Recursively build; returns this node's index. Children of an inner node: left = idx+1 (depth
/// first), right stored in `right_or_start`.
fn build_node(tris: &[Tri], order: &mut [u32], start: usize, count: usize, nodes: &mut Vec<BvhNode>) -> u32 {
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for &i in &order[start..start + count] {
        let (a, b) = tri_bounds(&tris[i as usize]);
        mn = mn.min(a);
        mx = mx.max(b);
    }
    let idx = nodes.len() as u32;
    nodes.push(BvhNode { mn, mx, right_or_start: start as u32, count: count as u32 });
    if count <= 4 {
        return idx; // leaf
    }
    // Median split on the longest centroid axis.
    let ext = mx - mn;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    let cen = |t: &Tri| (t.p0 + (t.p0 + t.e1) + (t.p0 + t.e2)) / 3.0;
    order[start..start + count].sort_unstable_by(|&a, &b| {
        let ca = cen(&tris[a as usize])[axis];
        let cb = cen(&tris[b as usize])[axis];
        ca.total_cmp(&cb)
    });
    let half = count / 2;
    nodes[idx as usize].count = 0; // inner
    let _left = build_node(tris, order, start, half, nodes);
    let right = build_node(tris, order, start + half, count - half, nodes);
    nodes[idx as usize].right_or_start = right;
    idx
}

#[inline]
fn ray_box(ro: Vec3, inv_rd: Vec3, mn: Vec3, mx: Vec3, tmax: f32) -> bool {
    let t0 = (mn - ro) * inv_rd;
    let t1 = (mx - ro) * inv_rd;
    let tsm = t0.min(t1);
    let tbg = t0.max(t1);
    let near = tsm.x.max(tsm.y).max(tsm.z).max(0.0);
    let far = tbg.x.min(tbg.y).min(tbg.z).min(tmax);
    near <= far
}

/// Möller–Trumbore. Returns `t` (> eps) or None.
#[inline]
fn ray_tri(ro: Vec3, rd: Vec3, t: &Tri) -> Option<f32> {
    let p = rd.cross(t.e2);
    let det = t.e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ro - t.p0;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(t.e1);
    let v = rd.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let tt = t.e2.dot(q) * inv;
    (tt > 1e-4).then_some(tt)
}

struct Hit {
    t: f32,
    tri: u32,
}

impl Scene {
    /// Nearest hit along the ray.
    fn intersect(&self, ro: Vec3, rd: Vec3) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / rd.x, 1.0 / rd.y, 1.0 / rd.z);
        let mut best: Option<Hit> = None;
        let mut tmax = f32::INFINITY;
        let mut stack = [0u32; 64];
        let mut sp = 0usize;
        stack[sp] = 0;
        sp += 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp];
            let n = &self.nodes[ni as usize];
            if !ray_box(ro, inv, n.mn, n.mx, tmax) {
                continue;
            }
            if n.count > 0 {
                for k in n.right_or_start..n.right_or_start + n.count {
                    let ti = self.order[k as usize];
                    if let Some(t) = ray_tri(ro, rd, &self.tris[ti as usize]) {
                        if t < tmax {
                            tmax = t;
                            best = Some(Hit { t, tri: ti });
                        }
                    }
                }
            } else if sp + 2 <= stack.len() {
                // Depth-first layout: the left child is `ni + 1`, the right child's index is stored.
                stack[sp] = ni + 1;
                sp += 1;
                stack[sp] = n.right_or_start;
                sp += 1;
            }
        }
        best
    }

    /// Transmission along a shadow ray toward the sun: 1 = clear, 0 = blocked. Glass panes
    /// (opacity < 1) attenuate by their transparency and tint instead of blocking — sun through
    /// windows. Walks at most 16 surfaces.
    fn transmission(&self, mut ro: Vec3, rd: Vec3) -> [f32; 3] {
        let mut trans = [1.0f32; 3];
        for _ in 0..16 {
            let Some(h) = self.intersect(ro, rd) else { return trans };
            let m = &self.tris[h.tri as usize].mat;
            if m.opacity >= 0.99 {
                return [0.0; 3]; // opaque blocker
            }
            let k = 1.0 - m.opacity;
            trans[0] *= k * (0.5 + 0.5 * m.albedo[0]);
            trans[1] *= k * (0.5 + 0.5 * m.albedo[1]);
            trans[2] *= k * (0.5 + 0.5 * m.albedo[2]);
            if trans[0].max(trans[1]).max(trans[2]) < 0.01 {
                return [0.0; 3];
            }
            ro = ro + rd * (h.t + 1e-3);
        }
        trans
    }
}

// ============================ tracing ============================

/// xorshift32 — deterministic per (pixel, pass) so passes accumulate independent samples.
#[derive(Clone, Copy)]
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(seed.wrapping_mul(747796405).wrapping_add(2891336453) | 1)
    }
    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / 16777216.0
    }
}

/// Cosine-weighted hemisphere sample about `n`.
fn cosine_dir(n: Vec3, rng: &mut Rng) -> Vec3 {
    let r1 = rng.next_f32() * std::f32::consts::TAU;
    let r2 = rng.next_f32();
    let r2s = r2.sqrt();
    let w = n;
    let a = if w.x.abs() > 0.5 { Vec3::Y } else { Vec3::X };
    let u = a.cross(w).normalize();
    let v = w.cross(u);
    (u * (r1.cos() * r2s) + v * (r1.sin() * r2s) + w * (1.0 - r2).sqrt()).normalize()
}

/// The hemispheric environment (matches the raster's `hemi_ambient` + a soft sun-disc glow for
/// primary rays so the sky looks right behind the building).
fn sky_radiance(sky: &Sky, dir: Vec3, primary: bool) -> [f32; 3] {
    let up = (0.5 + 0.5 * dir.z).clamp(0.0, 1.0);
    let mut c = [
        sky.ground_col[0] + (sky.sky_col[0] - sky.ground_col[0]) * up,
        sky.ground_col[1] + (sky.sky_col[1] - sky.ground_col[1]) * up,
        sky.ground_col[2] + (sky.sky_col[2] - sky.ground_col[2]) * up,
    ];
    if primary {
        let d = dir.dot(sky.sun_dir);
        if d > 0.9995 {
            let k = ((d - 0.9995) / 0.0005).clamp(0.0, 1.0);
            for i in 0..3 {
                c[i] += sky.sun_col[i] * k * 2.0;
            }
        }
    }
    c
}

/// Trace one path. Returns linear HDR radiance.
fn trace(scene: &Scene, sky: &Sky, mut ro: Vec3, mut rd: Vec3, max_depth: u32, rng: &mut Rng) -> [f32; 3] {
    let mut radiance = [0.0f32; 3];
    let mut through = [1.0f32; 3];
    let mut primary = true;
    for _depth in 0..max_depth {
        let Some(h) = scene.intersect(ro, rd) else {
            let s = sky_radiance(sky, rd, primary);
            for i in 0..3 {
                radiance[i] += through[i] * s[i];
            }
            break;
        };
        let tri = &scene.tris[h.tri as usize];
        let m = tri.mat;
        let hit = ro + rd * h.t;
        // Face the normal toward the ray.
        let n = if tri.n.dot(rd) > 0.0 { -tri.n } else { tri.n };

        // Emission.
        for i in 0..3 {
            radiance[i] += through[i] * m.emission[i];
        }

        // Thin-pane glass: fresnel reflect, else tinted straight-through transmit.
        if m.opacity < 0.99 {
            let cosi = (-rd).dot(n).clamp(0.0, 1.0);
            let f0 = 0.04;
            let fres = f0 + (1.0 - f0) * (1.0 - cosi).powi(5);
            if rng.next_f32() < fres {
                rd = (rd - n * 2.0 * rd.dot(n)).normalize();
                ro = hit + rd * 1e-3;
            } else {
                let k = 1.0 - m.opacity;
                for i in 0..3 {
                    through[i] *= k * (0.5 + 0.5 * m.albedo[i]) + m.opacity * m.albedo[i] * 0.5;
                }
                ro = hit + rd * 1e-3; // continue straight through (thin pane)
            }
            primary = false;
            continue;
        }

        // Direct sun (next-event estimation) with glass-aware shadow transmission. The sun colour
        // is treated as pre-integrated irradiance (matches the raster's `sun*ndl`), so brightness
        // lines up with the viewport.
        let ndl = n.dot(sky.sun_dir);
        if ndl > 0.0 {
            // Jitter within a small cone for slightly-soft sun edges.
            let jd = (sky.sun_dir + cosine_dir(sky.sun_dir, rng) * 0.012).normalize();
            let tr = scene.transmission(hit + n * 1e-3, jd);
            if tr[0] + tr[1] + tr[2] > 0.0 {
                for i in 0..3 {
                    radiance[i] += through[i] * m.albedo[i] * sky.sun_col[i] * ndl * tr[i];
                }
            }
        }

        // Bounce: metallic mirror vs diffuse.
        let spec_p = m.metallic;
        if rng.next_f32() < spec_p {
            let mirror = (rd - n * 2.0 * rd.dot(n)).normalize();
            let lobe = cosine_dir(mirror, rng);
            rd = mirror.lerp(lobe, m.rough * m.rough).normalize();
            if rd.dot(n) <= 0.0 {
                rd = mirror;
            }
            for i in 0..3 {
                through[i] *= m.albedo[i];
            }
        } else {
            rd = cosine_dir(n, rng);
            for i in 0..3 {
                through[i] *= m.albedo[i];
            }
        }
        ro = hit + n * 1e-3;
        primary = false;

        // Russian roulette after a few bounces.
        let p = through[0].max(through[1]).max(through[2]).clamp(0.05, 1.0);
        if _depth >= 3 {
            if rng.next_f32() > p {
                break;
            }
            for t in &mut through {
                *t /= p;
            }
        }
    }
    radiance
}

/// Generate the camera ray for pixel `(x, y)` (with in-pixel jitter).
fn cam_ray(cam: &Camera, set: &Settings, x: usize, y: usize, rng: &mut Rng) -> (Vec3, Vec3) {
    let fwd = (cam.target - cam.eye).normalize();
    let right = fwd.cross(Vec3::Z).normalize_or_zero();
    let right = if right.length_squared() < 0.5 { Vec3::X } else { right };
    let up = right.cross(fwd);
    let half = (cam.fov_deg.to_radians() * 0.5).tan();
    let aspect = set.w as f32 / set.h.max(1) as f32;
    let px = ((x as f32 + rng.next_f32()) / set.w as f32 * 2.0 - 1.0) * half * aspect;
    let py = (1.0 - (y as f32 + rng.next_f32()) / set.h as f32 * 2.0) * half;
    (cam.eye, (fwd + right * px + up * py).normalize())
}

/// The raster viewport's tone-map (`1 − e⁻ˣ`) — applied when converting the accumulation to sRGB.
#[inline]
pub fn tonemap8(c: f32) -> u8 {
    ((1.0 - (-c.max(0.0)).exp()) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

// ============================ progressive job ============================

/// Shared state between the render worker and the UI.
pub struct Shared {
    /// RGB32F accumulation, `w*h*3`, sum over completed passes.
    pub accum: Mutex<Vec<f32>>,
    pub passes_done: AtomicU32,
    pub cancel: AtomicBool,
    pub done: AtomicBool,
}

/// A running (or finished) render.
pub struct RenderJob {
    pub shared: Arc<Shared>,
    pub settings: Settings,
    pub device: Device,
    pub started: std::time::Instant,
    /// Triangle count of the scene being traced (for the progress readout).
    pub scene_tris: usize,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RenderJob {
    /// Start a progressive CPU render in the background. (The GPU device is served by
    /// [`crate::pathtrace_gpu::GpuTracer`], which consumes the same [`Scene`] via [`Scene::pack_gpu`].)
    pub fn start(scene: Scene, cam: Camera, sky: Sky, settings: Settings, device: Device) -> Self {
        let shared = Arc::new(Shared {
            accum: Mutex::new(vec![0.0f32; settings.w * settings.h * 3]),
            passes_done: AtomicU32::new(0),
            cancel: AtomicBool::new(false),
            done: AtomicBool::new(false),
        });
        let scene_tris = scene.tri_count();
        let sh = shared.clone();
        let handle = std::thread::spawn(move || {
            let scene = &scene;
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).max(1);
            for pass in 0..settings.passes {
                if sh.cancel.load(Ordering::Relaxed) {
                    break;
                }
                // One sample/pixel this pass, split into row bands.
                let mut pass_buf = vec![0.0f32; settings.w * settings.h * 3];
                let band = settings.h.div_ceil(threads);
                std::thread::scope(|s| {
                    for (bi, chunk) in pass_buf.chunks_mut(band * settings.w * 3).enumerate() {
                        let sh = &sh;
                        s.spawn(move || {
                            let y0 = bi * band;
                            for (dy, row) in chunk.chunks_mut(settings.w * 3).enumerate() {
                                if sh.cancel.load(Ordering::Relaxed) {
                                    return;
                                }
                                let y = y0 + dy;
                                for x in 0..settings.w {
                                    let seed = (y as u32)
                                        .wrapping_mul(9781)
                                        .wrapping_add(x as u32)
                                        .wrapping_mul(6271)
                                        .wrapping_add(pass.wrapping_mul(26699));
                                    let mut rng = Rng::new(seed);
                                    let (ro, rd) = cam_ray(&cam, &settings, x, y, &mut rng);
                                    let c = trace(scene, &sky, ro, rd, settings.max_depth, &mut rng);
                                    let o = x * 3;
                                    row[o] += c[0].min(50.0);
                                    row[o + 1] += c[1].min(50.0);
                                    row[o + 2] += c[2].min(50.0);
                                }
                            }
                        });
                    }
                });
                if sh.cancel.load(Ordering::Relaxed) {
                    break;
                }
                {
                    let mut acc = sh.accum.lock().unwrap();
                    for (a, p) in acc.iter_mut().zip(&pass_buf) {
                        *a += *p;
                    }
                }
                sh.passes_done.fetch_add(1, Ordering::Relaxed);
            }
            sh.done.store(true, Ordering::Relaxed);
        });
        Self { shared, settings, device, started: std::time::Instant::now(), scene_tris, handle: Some(handle) }
    }

    /// Snapshot the accumulation as RGBA8 (tone-mapped), or None before the first pass lands.
    pub fn snapshot_rgba(&self) -> Option<(usize, usize, Vec<u8>)> {
        let n = self.shared.passes_done.load(Ordering::Relaxed);
        if n == 0 {
            return None;
        }
        let acc = self.shared.accum.lock().unwrap();
        let inv = 1.0 / n as f32;
        let mut out = Vec::with_capacity(self.settings.w * self.settings.h * 4);
        for px in acc.chunks_exact(3) {
            out.push(tonemap8(px[0] * inv));
            out.push(tonemap8(px[1] * inv));
            out.push(tonemap8(px[2] * inv));
            out.push(255);
        }
        Some((self.settings.w, self.settings.h, out))
    }

    pub fn passes_done(&self) -> u32 {
        self.shared.passes_done.load(Ordering::Relaxed)
    }
    pub fn is_done(&self) -> bool {
        self.shared.done.load(Ordering::Relaxed)
    }
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for RenderJob {
    fn drop(&mut self) {
        self.cancel();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ============================ tests ============================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radiance_export::ExportTri;

    fn quad(z: f32, half: f32, rgb: [f32; 3], opacity: f32) -> [ExportTri; 2] {
        let v = |x: f32, y: f32| [x, y, z];
        let mut a = ExportTri::plain([v(-half, -half), v(half, -half), v(half, half)], rgb, 0.5, opacity);
        let mut b = ExportTri::plain([v(-half, -half), v(half, half), v(-half, half)], rgb, 0.5, opacity);
        a.opacity = opacity;
        b.opacity = opacity;
        [a, b]
    }

    fn sky() -> Sky {
        Sky {
            sun_dir: Vec3::new(0.0, 0.0, 1.0),
            sun_col: [2.0, 2.0, 2.0],
            sky_col: [0.5, 0.5, 0.6],
            ground_col: [0.2, 0.2, 0.2],
        }
    }

    #[test]
    fn bvh_matches_brute_force() {
        // A grid of small quads; random rays must hit the same nearest triangle as brute force.
        let mut tris = Vec::new();
        for i in 0..6 {
            for j in 0..6 {
                let (x, y) = (i as f32, j as f32);
                tris.push(ExportTri::plain([[x, y, 0.0], [x + 0.9, y, 0.0], [x, y + 0.9, 0.0]], [0.5; 3], 0.5, 1.0));
            }
        }
        let scene = Scene::build(&tris);
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let ro = Vec3::new(rng.next_f32() * 6.0, rng.next_f32() * 6.0, 3.0);
            let rd = Vec3::new(rng.next_f32() - 0.5, rng.next_f32() - 0.5, -1.0).normalize();
            let bvh = scene.intersect(ro, rd).map(|h| (h.t * 1e4) as i64);
            let brute = scene
                .tris
                .iter()
                .filter_map(|t| ray_tri(ro, rd, t))
                .min_by(|a, b| a.total_cmp(b))
                .map(|t| (t * 1e4) as i64);
            assert_eq!(bvh, brute, "BVH nearest-hit must equal brute force");
        }
    }

    #[test]
    fn sun_lights_the_open_side_and_shadows_the_covered() {
        // A ground plane, with a solid roof over x<0 only. Up-facing ground under open sky must
        // come out brighter than under the roof.
        let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [0.7; 3], 1.0).into();
        tris.extend([
            ExportTri::plain([[-30.0, -30.0, 3.0], [0.0, -30.0, 3.0], [0.0, 30.0, 3.0]], [0.7; 3], 0.5, 1.0),
            ExportTri::plain([[-30.0, -30.0, 3.0], [0.0, 30.0, 3.0], [-30.0, 30.0, 3.0]], [0.7; 3], 0.5, 1.0),
        ]);
        let scene = Scene::build(&tris);
        let sky = sky();
        let mut rng = Rng::new(42);
        let sample = |x: f32, rng: &mut Rng| -> f32 {
            let mut sum = 0.0;
            for _ in 0..64 {
                let c = trace(&scene, &sky, Vec3::new(x, 0.0, 10.0), Vec3::NEG_Z, 4, rng);
                sum += c[0];
            }
            sum / 64.0
        };
        // Camera above the roof for the covered side: trace from just under the roof instead.
        let open = sample(10.0, &mut rng);
        let mut sum = 0.0;
        for _ in 0..64 {
            let c = trace(&scene, &sky, Vec3::new(-10.0, 0.0, 2.0), Vec3::NEG_Z, 4, &mut rng);
            sum += c[0];
        }
        let covered = sum / 64.0;
        assert!(open > covered * 1.5, "open {open} should be well brighter than covered {covered}");
    }

    #[test]
    fn glass_transmits_sunlight_partially() {
        // A glass pane (opacity 0.1) over the ground: transmission should be well above zero and
        // below clear-sky.
        let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [0.7; 3], 1.0).into();
        tris.extend(quad(3.0, 50.0, [0.85, 0.9, 0.92], 0.1));
        let scene = Scene::build(&tris);
        let tr = scene.transmission(Vec3::new(0.0, 0.0, 0.1), Vec3::Z);
        assert!(tr[0] > 0.4 && tr[0] < 1.0, "glass passes most sun: {tr:?}");
        // An OPAQUE roof blocks fully.
        let mut tris2: Vec<ExportTri> = quad(0.0, 50.0, [0.7; 3], 1.0).into();
        tris2.extend(quad(3.0, 50.0, [0.5; 3], 1.0));
        let scene2 = Scene::build(&tris2);
        assert_eq!(scene2.transmission(Vec3::new(0.0, 0.0, 0.1), Vec3::Z), [0.0; 3]);
    }

    #[test]
    fn emissive_surface_glows() {
        let mut t = ExportTri::plain([[-1.0, -1.0, 2.0], [1.0, -1.0, 2.0], [0.0, 1.0, 2.0]], [1.0; 3], 0.5, 1.0);
        t.emission = [5.0, 1.0, 1.0];
        let scene = Scene::build(&[t]);
        let sky = sky();
        let mut rng = Rng::new(3);
        let c = trace(&scene, &sky, Vec3::new(0.0, 0.0, 0.0), Vec3::Z, 2, &mut rng);
        assert!(c[0] >= 5.0, "looking at the emitter sees its radiance: {c:?}");
    }

    #[test]
    fn render_job_runs_to_completion_and_snapshots() {
        let tris: Vec<ExportTri> = quad(0.0, 50.0, [0.6, 0.4, 0.3], 1.0).into();
        let scene = Scene::build(&tris);
        let cam = Camera { eye: Vec3::new(0.0, -8.0, 5.0), target: Vec3::ZERO, fov_deg: 45.0 };
        let set = Settings { w: 32, h: 24, passes: 2, max_depth: 3 };
        let job = RenderJob::start(scene, cam, sky(), set, Device::Cpu);
        // Wait for completion (small image — fast).
        let t0 = std::time::Instant::now();
        while !job.is_done() && t0.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(job.is_done(), "render finishes");
        assert_eq!(job.passes_done(), 2);
        let (w, h, rgba) = job.snapshot_rgba().expect("snapshot after passes");
        assert_eq!((w, h), (32, 24));
        assert_eq!(rgba.len(), 32 * 24 * 4);
        // The ground fills the lower half — some pixel must be non-black and alpha opaque.
        assert!(rgba.chunks_exact(4).any(|p| p[0] > 10), "image has content");
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    }
}
