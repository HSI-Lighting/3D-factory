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
/// ([`crate::factory::SunEnv::resolve_env`]), so the render matches the view.
///
/// `dome` is the analytic Preetham sky from [`crate::env`]; `sky_col`/`ground_col` remain as its
/// zenith and ground radiance so the GPU backend, which still runs the older two-colour
/// approximation in its fragment shader, keeps working unchanged.
#[derive(Clone, Debug)]
pub struct Sky {
    /// Unit direction TO the sun (export frame).
    pub sun_dir: Vec3,
    pub sun_col: [f32; 3],
    pub sky_col: [f32; 3],
    pub ground_col: [f32; 3],
    /// `None` ⇒ no physical sky (daylight off); the two colours above are used as a flat
    /// hemisphere, exactly as before.
    pub dome: Option<crate::env::Sky>,
    /// A loaded HDR environment, which OUTRANKS the dome when present — an offline render has to be
    /// lit by whatever the viewport is lit by, or ⏺ Render stops being a preview of anything. The
    /// map is shared, not copied: it is tens of megabytes and every ray reads it.
    pub env: Option<std::sync::Arc<crate::env_map::EnvMap>>,
    pub env_strength: f32,
    /// Yaw, radians. Applied by rotating the RAY, so the map is never resampled.
    pub env_rot: f32,
}

impl Sky {
    /// Build from the app's resolved environment. `sun_dir` is passed separately because the export
    /// frame is rotated by the building's north offset while the sky's own model is not.
    pub fn from_env(sun_dir: Vec3, sun_col: [f32; 3], env: &crate::env::EnvRender) -> Self {
        let dome = env.sky.map(|mut s| {
            s.sun_dir = sun_dir; // rotate the sun into the export frame with the geometry
            s
        });
        let (sky_col, ground_col) = match &dome {
            Some(d) => (d.radiance(Vec3::Z), d.ground),
            None => ([0.35, 0.37, 0.43], [0.2, 0.19, 0.17]),
        };
        Self {
            sun_dir,
            sun_col,
            sky_col,
            ground_col,
            dome,
            env: None,
            env_strength: env.hdri.map(|h| h.strength).unwrap_or(1.0),
            env_rot: env.hdri.map(|h| h.rot).unwrap_or(0.0),
        }
    }

    /// Attach the loaded HDR environment. Separate from [`Self::from_env`] because the map lives on
    /// the scene, not on the sun's own settings.
    /// The two hemisphere colours are re-derived from the map as well. The CPU tracer never reads
    /// them once `env` is set — it samples the map itself — but the GPU backend's sky is a simple
    /// two-colour mix, so those colours are the ONLY route an HDRI has into it. Setting them means
    /// switching backend takes the sky from photographic to approximate, rather than from
    /// photographic to unrelated.
    pub fn with_env(mut self, map: Option<std::sync::Arc<crate::env_map::EnvMap>>) -> Self {
        if let Some(m) = &map {
            let k = self.env_strength;
            let sh = m.sh9();
            let avg = |d: Vec3| {
                let c = crate::env::sh_ambient(&sh, d);
                [c[0] * k, c[1] * k, c[2] * k]
            };
            self.sky_col = avg(Vec3::Z);
            self.ground_col = avg(-Vec3::Z);
        }
        self.env = map;
        self
    }
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
    /// The display transform applied when the linear accumulation becomes an image — the SAME one
    /// the viewport composites with, so a render matches the view it was framed from.
    pub color: crate::color::ColorPipeline,
}

// ============================ scene / BVH ============================

#[derive(Clone, Copy)]
struct Tri {
    p0: Vec3,
    e1: Vec3,
    e2: Vec3,
    n: Vec3, // geometric unit normal
    /// UV at `p0` and its deltas along `e1`/`e2`, so a hit's UV is one mad per axis. Only
    /// meaningful when the material has an image AND the surface carried a UV layer; otherwise the
    /// sampler projects from world space instead, matching what the viewport shader does.
    uv0: [f32; 2],
    duv1: [f32; 2],
    duv2: [f32; 2],
    has_uv: bool,
    mat: Mat,
}

/// One decoded base-colour image the tracer can sample.
///
/// Kept as the source sRGB bytes with a 256-entry decode table rather than a linear f32 copy: the
/// villa's texture set is 15.6 Mpx, which as `f32` RGB would be 187 MB for no accuracy gain.
pub struct TexImage {
    pub w: u32,
    pub h: u32,
    /// RGBA8, sRGB-encoded — exactly what the app holds and what the GPU uploads as `SRGB8_ALPHA8`.
    pub rgba: std::sync::Arc<Vec<u8>>,
    /// Mirrors the viewport's `u_triplanar`: when the surface has no UVs, project from world space.
    pub triplanar: bool,
    /// Tiles per metre for that projection (the viewport's `u_tpm`).
    pub tiles_per_m: f32,
}

/// sRGB→linear for all 256 byte values, built once. The tracer samples millions of texels per
/// frame and `powf` in that loop is pure waste.
static SRGB_LUT: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    let mut t = [0.0f32; 256];
    for (i, v) in t.iter_mut().enumerate() {
        *v = crate::color::srgb_to_linear(i as f32 / 255.0);
    }
    t
});

impl TexImage {
    /// Bilinear sample with wrap, returning LINEAR RGB. Wrap (rather than clamp) is what tiling a
    /// material across a wall needs, and it matches the viewport's sampler state.
    fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        if self.w == 0 || self.h == 0 {
            return [1.0; 3];
        }
        let lut = &*SRGB_LUT;
        let (fw, fh) = (self.w as f32, self.h as f32);
        // Half-texel offset so a UV of 0 lands on the centre of texel 0, not between texels.
        let x = u * fw - 0.5;
        let y = v * fh - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let wrap = |i: f32, n: u32| -> usize { (i.rem_euclid(n as f32)) as usize % n as usize };
        let (xi0, yi0) = (wrap(x0, self.w), wrap(y0, self.h));
        let (xi1, yi1) = (wrap(x0 + 1.0, self.w), wrap(y0 + 1.0, self.h));
        let px = |xi: usize, yi: usize| -> [f32; 3] {
            let o = (yi * self.w as usize + xi) * 4;
            match self.rgba.get(o..o + 3) {
                Some(s) => [lut[s[0] as usize], lut[s[1] as usize], lut[s[2] as usize]],
                None => [1.0; 3],
            }
        };
        let (a, b, c, d) = (px(xi0, yi0), px(xi1, yi0), px(xi0, yi1), px(xi1, yi1));
        let mut out = [0.0f32; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            out[k] = top + (bot - top) * fy;
        }
        out
    }

    /// World-space triplanar projection, for a surface with no UV layer — the same blend the
    /// viewport's `tri_or_uv` does, so an offline render and the viewport agree.
    fn sample_triplanar(&self, p: Vec3, n: Vec3, tpm: f32) -> [f32; 3] {
        let w = {
            let a = [n.x.abs(), n.y.abs(), n.z.abs()];
            let s = a[0] + a[1] + a[2];
            if s < 1e-6 { [0.0, 0.0, 1.0] } else { [a[0] / s, a[1] / s, a[2] / s] }
        };
        let x = self.sample(p.y * tpm, p.z * tpm);
        let y = self.sample(p.x * tpm, p.z * tpm);
        let z = self.sample(p.x * tpm, p.y * tpm);
        [
            x[0] * w[0] + y[0] * w[1] + z[0] * w[2],
            x[1] * w[0] + y[1] * w[1] + z[1] * w[2],
            x[2] * w[0] + y[2] * w[1] + z[2] * w[2],
        ]
    }
}

#[derive(Clone, Copy)]
struct Mat {
    albedo: [f32; 3],
    rough: f32,
    metallic: f32,
    ior: f32,
    opacity: f32,
    emission: [f32; 3],
    /// CLEARCOAT — a thin varnish with its own smooth specular lobe over the base. Sampled as a
    /// THIRD lobe (see the bounce below) rather than only added to the sun, because most of what a
    /// clearcoat does visually is reflect the room, and the room only reaches a path tracer through
    /// bounce rays.
    clearcoat: f32,
    clearcoat_rough: f32,
    /// SHEEN — the grazing rim of fabric. Carried on the DIFFUSE lobe rather than as a lobe of its
    /// own, which costs nothing and means indirect sheen works: a velvet curtain lit only by sky
    /// bounce still gets its rim.
    sheen: f32,
    sheen_tint: [f32; 3],
    /// The PROCEDURAL definition, resolved once at build time from the scene's material table. When
    /// present it overrides `albedo`/`rough` per hit point — which is what puts the grain in.
    proc: Option<crate::factory::ProcDef>,
    /// Index into [`Scene::textures`] of this material's base-colour IMAGE, when it has one. With
    /// this absent the tracer only ever saw `albedo` — the image's AVERAGE colour — so a villa
    /// rendered offline had flat terracotta roofs and flat green lawn where the viewport showed
    /// tiles and grass.
    tex: Option<u32>,
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
    /// Base-colour images, indexed by [`Mat::tex`]. Empty on the plain path.
    textures: Vec<TexImage>,
    /// The building's north offset, in radians, as applied to the exported geometry. Procedurals
    /// are evaluated in the MODEL's world space, so a hit point has to be rotated back through this
    /// before the pattern is sampled — otherwise the grain runs in a different direction offline
    /// than it does in the viewport.
    proc_rot: f32,
}

impl Scene {
    /// Build with no procedural materials — the plain path, and what the tests use.
    pub fn build(input: &[ExportTri]) -> Self {
        Self::build_with(input, &[], 0.0)
    }

    /// Build with the app's material table: `procs[i]` is the procedural definition of texture `i`,
    /// which [`ExportTri::material`] indexes.
    pub fn build_with(input: &[ExportTri], procs: &[Option<crate::factory::ProcDef>], proc_rot: f32) -> Self {
        Self::build_full(input, procs, proc_rot, Vec::new(), &[])
    }

    /// Build with IMAGES as well as procedurals. `textures` is the pool; `tex_of[i]` is the pool
    /// index of material `i`'s base colour, parallel to `procs` and indexed by
    /// [`ExportTri::material`] — the same index the viewport binds its textures by, so the offline
    /// render and the viewport are looking at the same picture.
    pub fn build_full(
        input: &[ExportTri],
        procs: &[Option<crate::factory::ProcDef>],
        proc_rot: f32,
        textures: Vec<TexImage>,
        tex_of: &[Option<u32>],
    ) -> Self {
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
                uv0: t.uv[0],
                duv1: [t.uv[1][0] - t.uv[0][0], t.uv[1][1] - t.uv[0][1]],
                duv2: [t.uv[2][0] - t.uv[0][0], t.uv[2][1] - t.uv[0][1]],
                has_uv: t.has_uv,
                mat: Mat {
                    albedo: t.rgb,
                    rough: t.roughness.clamp(0.0, 1.0),
                    metallic: t.metallic.clamp(0.0, 1.0),
                    ior: t.ior.clamp(1.0, 3.0),
                    opacity: t.opacity.clamp(0.0, 1.0),
                    emission: t.emission,
                    clearcoat: t.clearcoat.clamp(0.0, 1.0),
                    clearcoat_rough: t.clearcoat_rough.clamp(0.01, 1.0),
                    sheen: t.sheen.clamp(0.0, 1.0),
                    sheen_tint: t.sheen_tint,
                    // A SOLID procedural is a flat colour dressed as a pattern; evaluating it per
                    // hit would cost three octaves of noise to reproduce `albedo` exactly.
                    proc: t
                        .material
                        .and_then(|i| procs.get(i as usize).copied().flatten())
                        .filter(|d| !d.is_solid() || d.varies_roughness() || d.bump > 0.0),
                    tex: t.material.and_then(|i| tex_of.get(i as usize).copied().flatten()),
                },
            });
        }
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len().max(1) * 2);
        if !tris.is_empty() {
            build_node(&tris, &mut order, 0, tris.len(), &mut nodes);
        }
        Self { tris, order, nodes, textures, proc_rot }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    /// Pack the scene for the GPU backend: flat RGBA32F texel streams the fragment tracer reads
    /// with `texelFetch`. Layouts (one texel = 4 floats):
    /// - `tris`: [`TRI_TEXELS`] texels/tri — [p0|rough] [e1|metallic] [e2|opacity]
    ///   [albedo|sheen_tint packed] [emission|sheen] [clearcoat, clearcoat_rough, 0, 0]
    /// - `nodes`: 2 texels/node — [mn|right_or_start] [mx|count]
    /// - `order`: 1 texel/entry — [tri_index|0|0|0]
    /// Counts stay far below f32's exact-integer range (2²⁴), so indices survive the float trip.
    ///
    /// The sheen TINT is packed into one float rather than given a texel of its own: a scene here
    /// reaches millions of triangles, so every texel added costs 16 MB per million. Three 8-bit
    /// channels is 24 bits, which f32's mantissa carries exactly — the unpacked colour is the
    /// authored one to within a 255th, and a tint is the last thing that needs more.
    pub fn pack_gpu(&self) -> GpuPack {
        let mut tris = Vec::with_capacity(self.tris.len() * TRI_TEXELS * 4);
        for t in &self.tris {
            tris.extend_from_slice(&[t.p0.x, t.p0.y, t.p0.z, t.mat.rough]);
            tris.extend_from_slice(&[t.e1.x, t.e1.y, t.e1.z, t.mat.metallic]);
            tris.extend_from_slice(&[t.e2.x, t.e2.y, t.e2.z, t.mat.opacity]);
            tris.extend_from_slice(&[t.mat.albedo[0], t.mat.albedo[1], t.mat.albedo[2], pack_rgb8(t.mat.sheen_tint)]);
            tris.extend_from_slice(&[t.mat.emission[0], t.mat.emission[1], t.mat.emission[2], t.mat.sheen]);
            tris.extend_from_slice(&[t.mat.clearcoat, t.mat.clearcoat_rough, 0.0, 0.0]);
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

/// How many RGBA32F texels each triangle occupies in [`Scene::pack_gpu`]. The GPU tracer's shader
/// strides by this, so the two must agree — a test pins them together.
pub const TRI_TEXELS: usize = 6;

/// Three 0..1 channels into one float as 8 bits each. Exact through f32 (24 bits of mantissa),
/// and the inverse of `unpack_rgb8` in the tracer's GLSL.
fn pack_rgb8(c: [f32; 3]) -> f32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round();
    q(c[0]) * 65536.0 + q(c[1]) * 256.0 + q(c[2])
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

/// This hit's base colour from an image map: the surface's own UVs when it has them, and the same
/// world-space triplanar projection the viewport shader falls back to when it does not. Matching
/// the viewport's choice here is the point — an offline render that projects differently from the
/// preview is a different picture, not a better one.
#[inline]
fn tex_albedo(img: &TexImage, tri: &Tri, bu: f32, bv: f32, hit: Vec3, n: Vec3) -> [f32; 3] {
    if tri.has_uv && !img.triplanar {
        let u = tri.uv0[0] + tri.duv1[0] * bu + tri.duv2[0] * bv;
        let v = tri.uv0[1] + tri.duv1[1] * bu + tri.duv2[1] * bv;
        img.sample(u, v)
    } else {
        img.sample_triplanar(hit, n, img.tiles_per_m.max(1e-4))
    }
}

/// Möller–Trumbore. Returns `(t, u, v)` — the distance and the BARYCENTRICS along `e1`/`e2` — or
/// None. The barycentrics were previously discarded; they are what lets a hit be textured.
#[inline]
fn ray_tri(ro: Vec3, rd: Vec3, t: &Tri) -> Option<(f32, f32, f32)> {
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
    (tt > 1e-4).then_some((tt, u, v))
}

struct Hit {
    t: f32,
    tri: u32,
    /// Barycentrics along `e1`/`e2` at the hit — the UV lookup needs them.
    u: f32,
    v: f32,
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
                    if let Some((t, u, v)) = ray_tri(ro, rd, &self.tris[ti as usize]) {
                        if t < tmax {
                            tmax = t;
                            best = Some(Hit { t, tri: ti, u, v });
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

/// The environment's radiance along `dir`.
///
/// `primary` selects whether the **sun's disc** is included. That flag is not cosmetic: the sun is
/// sampled explicitly at every bounce (next-event estimation below), so a scattered ray that also
/// happened to see the disc would count the same light twice. Only rays that reach the camera
/// without scattering — the ones drawing the sky itself — may see it. This is the standard
/// treatment of a delta light, and it is why no multiple-importance weight is needed anywhere here.
/// [`sky_radiance`], for the environment tests in [`crate::env_map`] — they check that the tracer
/// and the viewport are lit by the same map, turned the same way.
#[cfg(test)]
pub fn sky_radiance_for_test(sky: &Sky, dir: Vec3, primary: bool) -> [f32; 3] {
    sky_radiance(sky, dir, primary)
}

fn sky_radiance(sky: &Sky, dir: Vec3, primary: bool) -> [f32; 3] {
    // An HDR environment answers for the whole sphere, and it answers the SAME whether the ray is
    // primary or not: its sun is a bright patch of image, not a delta light sampled separately, so
    // there is nothing here that could be counted twice.
    if let Some(m) = &sky.env {
        // Rotate the direction by +rot about Z before looking it up. The shader adds `u_env_rot`
        // to the azimuth, and turning a direction by +θ adds exactly θ to its `atan2` — so this is
        // the same rotation, the same way round. It used to negate, which turned the panorama the
        // OPPOSITE way from the viewport: set a rotation, press Render, and the sun came back from
        // the other side of the building.
        let (s, c) = sky.env_rot.sin_cos();
        let d = Vec3::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c, dir.z);
        let r = m.sample(d);
        return [r[0] * sky.env_strength, r[1] * sky.env_strength, r[2] * sky.env_strength];
    }
    match &sky.dome {
        Some(d) => {
            if primary {
                d.radiance_with_sun(dir)
            } else {
                d.radiance(dir)
            }
        }
        // No daylight: the old flat hemisphere, unchanged.
        None => {
            let up = (0.5 + 0.5 * dir.z).clamp(0.0, 1.0);
            [
                sky.ground_col[0] + (sky.sky_col[0] - sky.ground_col[0]) * up,
                sky.ground_col[1] + (sky.sky_col[1] - sky.ground_col[1]) * up,
                sky.ground_col[2] + (sky.sky_col[2] - sky.ground_col[2]) * up,
            ]
        }
    }
}

// ── the microfacet BSDF ──────────────────────────────────────────────────────────────────────
// The same Cook-Torrance GGX the viewport runs, so a render and the view agree about what a
// material is. What this replaced was "mirror direction, lerped toward a cosine lobe by roughness"
// — which is not a BRDF at all: it has no Fresnel, is not normalised, and converges to the wrong
// image no matter how many samples you give it.

fn d_ggx(n_o_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_o_h * n_o_h * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d).max(1e-7)
}

fn v_smith(n_o_v: f32, n_o_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let sv = n_o_l * (n_o_v * n_o_v * (1.0 - a2) + a2).sqrt();
    let sl = n_o_v * (n_o_l * n_o_l * (1.0 - a2) + a2).sqrt();
    0.5 / (sv + sl).max(1e-5)
}

fn f_schlick(f0: [f32; 3], u: f32) -> [f32; 3] {
    let k = (1.0 - u).clamp(0.0, 1.0).powi(5);
    [f0[0] + (1.0 - f0[0]) * k, f0[1] + (1.0 - f0[1]) * k, f0[2] + (1.0 - f0[2]) * k]
}

/// An orthonormal basis about `n` (Duff et al., branchless and stable at the poles).
fn onb(n: Vec3) -> (Vec3, Vec3) {
    let sign = if n.z >= 0.0 { 1.0f32 } else { -1.0 };
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    (
        Vec3::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x),
        Vec3::new(b, sign + n.y * n.y * a, -n.y),
    )
}

/// Sample a GGX half-vector about `n` from the distribution `D(h)·(n·h)`.
fn sample_ggx_h(n: Vec3, a: f32, rng: &mut Rng) -> Vec3 {
    let u1 = rng.next_f32();
    let u2 = rng.next_f32();
    let phi = u1 * std::f32::consts::TAU;
    // cosθ² = (1 − u) / (1 + (a² − 1)u) — the inverse CDF of the GGX NDF.
    let cos2 = ((1.0 - u2) / (1.0 + (a * a - 1.0) * u2)).clamp(0.0, 1.0);
    let cos_t = cos2.sqrt();
    let sin_t = (1.0 - cos2).max(0.0).sqrt();
    let (t, b) = onb(n);
    (t * (phi.cos() * sin_t) + b * (phi.sin() * sin_t) + n * cos_t).normalize()
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

        // ── the surface, evaluated HERE ──
        // A procedural material is a function of position, so the albedo, the roughness and the
        // normal all have to be resolved at the hit point rather than read off the triangle. This
        // is what puts grain in an offline render.
        let (albedo, rough, n) = match m.proc {
            Some(def) => {
                // Undo the export frame's north rotation: the pattern lives in the model's own
                // world space, which is where the viewport evaluates it.
                let (c, s) = (scene.proc_rot.cos(), scene.proc_rot.sin());
                let local = Vec3::new(hit.x * c + hit.y * s, -hit.x * s + hit.y * c, hit.z);
                let smp = crate::proc_tex::sample(&def, local, n, m.rough);
                // The bump was computed in the model frame; rotate it back with the geometry.
                let bn = smp.normal;
                (smp.albedo, smp.roughness, Vec3::new(bn.x * c - bn.y * s, bn.x * s + bn.y * c, bn.z))
            }
            // An IMAGE map, sampled at the hit. Procedurals win when both are present, because a
            // procedural material's "image" is only its average colour swatch.
            None => match m.tex.and_then(|i| scene.textures.get(i as usize)) {
                Some(img) => (tex_albedo(img, tri, h.u, h.v, hit, n), m.rough, n),
                None => (m.albedo, m.rough, n),
            },
        };

        let v = -rd;
        let f0d = {
            let f = (m.ior - 1.0) / (m.ior + 1.0);
            (f * f).clamp(0.0, 0.25)
        };
        let f0 = [
            f0d + (albedo[0] - f0d) * m.metallic,
            f0d + (albedo[1] - f0d) * m.metallic,
            f0d + (albedo[2] - f0d) * m.metallic,
        ];
        let mut diff = [albedo[0] * (1.0 - m.metallic), albedo[1] * (1.0 - m.metallic), albedo[2] * (1.0 - m.metallic)];
        let mut f0 = f0;
        let a = (rough * rough).max(1e-3);
        let n_o_v = n.dot(v).max(1e-4);

        // CLEARCOAT. Everything below it is dimmed by what the coat reflected away — the same
        // Fresnel attenuation the viewport applies, so a lacquered surface does not simply gain a
        // highlight but also loses a little of the material underneath, which is what stops the
        // two adding up to more light than arrived.
        let coat = m.clearcoat;
        let coat_a = (m.clearcoat_rough * m.clearcoat_rough).max(1e-3);
        // The coat is smooth over the grain, so it reflects about the GEOMETRIC normal — `tri.n`,
        // not the procedural's bumped one. That single choice is the difference between varnished
        // timber and timber with a shiny bump map.
        let coat_n = if tri.n.dot(v) > 0.0 { tri.n } else { -tri.n };
        if coat > 0.0 {
            let loss = coat * (0.04 + 0.96 * (1.0 - n_o_v).powi(5));
            let keep = 1.0 - loss;
            for i in 0..3 {
                diff[i] *= keep;
                f0[i] *= keep;
            }
        }

        // Direct sun (next-event estimation) with glass-aware shadow transmission, now with the
        // SPECULAR lobe as well as the diffuse — a sun highlight on a glossy floor used to be
        // absent from a render while being present in the viewport. `sun_col` is pre-integrated
        // irradiance (the app's convention), which is why the specular carries the π back.
        let ndl = n.dot(sky.sun_dir);
        if ndl > 0.0 {
            // Jitter within a small cone for slightly-soft sun edges.
            let jd = (sky.sun_dir + cosine_dir(sky.sun_dir, rng) * 0.012).normalize();
            let tr = scene.transmission(hit + n * 1e-3, jd);
            if tr[0] + tr[1] + tr[2] > 0.0 {
                let h = (sky.sun_dir + v).normalize();
                let f = f_schlick(f0, v.dot(h).max(0.0));
                let spec = d_ggx(n.dot(h).max(0.0), a) * v_smith(n_o_v, ndl, a) * ndl * std::f32::consts::PI;
                // SHEEN, evaluated as an extra BRDF lobe: a Fresnel-shaped rim peaking where the
                // half vector grazes the view, which is where a nap of fibres catches the light.
                let fh = (1.0 - v.dot(h).max(0.0)).clamp(0.0, 1.0).powi(5);
                let sheen = m.sheen * fh * ndl;
                // CLEARCOAT's own sun highlight, about the geometric normal and at its own
                // roughness — a small, hard glint on top of whatever the base is doing.
                let mut coat_spec = 0.0;
                if coat > 0.0 {
                    let ncl = coat_n.dot(sky.sun_dir).max(0.0);
                    let ncv = coat_n.dot(v).max(1e-4);
                    let fc = (0.04 + 0.96 * (1.0 - v.dot(h).max(0.0)).powi(5)) * coat;
                    coat_spec = d_ggx(coat_n.dot(h).max(0.0), coat_a)
                        * v_smith(ncv, ncl, coat_a) * ncl * std::f32::consts::PI * fc;
                }
                for i in 0..3 {
                    let s = diff[i] * ndl + f[i] * spec + m.sheen_tint[i] * sheen + coat_spec;
                    radiance[i] += through[i] * s * sky.sun_col[i] * tr[i];
                }
            }
        }

        // ── the bounce: one lobe, importance-sampled ──
        // Pick specular or diffuse in proportion to how much light each is likely to carry, then
        // divide by that probability so the estimator stays unbiased. A rough metal and a polished
        // dielectric therefore both converge — the old "mirror lerped toward a cosine lobe" did not
        // converge to anything in particular.
        // THREE lobes now: the coat, the base specular and the diffuse. The coat has to be sampled
        // rather than only added to the sun, because most of what a clearcoat does visually is
        // reflect the ROOM — and the room only reaches a path tracer through bounce rays. Only the
        // selection probabilities need to be positive for the estimator to stay unbiased; a poor
        // split costs variance, not correctness.
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let (ls, ld, lc) = (lum(f0), lum(diff), 0.25 * coat);
        let tot = ls + ld + lc;
        let (p_coat, p_spec) = if tot > 1e-6 {
            // Floors so no lobe is ever starved of samples: a lobe picked one time in a thousand
            // returns a thousand times the radiance when it IS picked, which is a firefly.
            let pc = (lc / tot).clamp(0.0, 0.8);
            let pc = if coat > 0.0 { pc.max(0.05) } else { 0.0 };
            (pc, ((ls / tot) * (1.0 - pc)).clamp(0.05, 0.9 - pc))
        } else {
            (0.0, 0.5)
        };
        let pick = rng.next_f32();
        if pick < p_coat {
            let h = sample_ggx_h(coat_n, coat_a, rng);
            let l = (2.0 * v.dot(h) * h - v).normalize();
            if l.dot(coat_n) <= 0.0 {
                break;
            }
            let n_o_l = l.dot(coat_n).max(1e-4);
            let n_o_h = coat_n.dot(h).max(1e-4);
            let v_o_h = v.dot(h).max(1e-4);
            let fc = (0.04 + 0.96 * (1.0 - v_o_h).powi(5)) * coat;
            let w = fc * 4.0 * v_smith(coat_n.dot(v).max(1e-4), n_o_l, coat_a) * n_o_l * v_o_h / n_o_h / p_coat;
            for t in &mut through {
                *t *= w;
            }
            rd = l;
        } else if pick < p_coat + p_spec {
            let h = sample_ggx_h(n, a, rng);
            let l = (2.0 * v.dot(h) * h - v).normalize();
            if l.dot(n) <= 0.0 {
                break; // sampled below the horizon — this path carries nothing
            }
            // With h drawn from D·(n·h), the estimator collapses to 4·F·Vis·(n·l)·(v·h)/(n·h).
            let n_o_l = l.dot(n).max(1e-4);
            let n_o_h = n.dot(h).max(1e-4);
            let v_o_h = v.dot(h).max(1e-4);
            let f = f_schlick(f0, v_o_h);
            let w = 4.0 * v_smith(n_o_v, n_o_l, a) * n_o_l * v_o_h / n_o_h / p_spec;
            for i in 0..3 {
                through[i] *= f[i] * w;
            }
            rd = l;
        } else {
            // Cosine-weighted diffuse: the pdf cancels the cosine and the 1/π exactly. SHEEN rides
            // here rather than on a lobe of its own — it costs nothing and it means INDIRECT sheen
            // works, so a velvet curtain lit only by sky bounce still gets its rim. The π is the
            // cosine pdf's, which the plain diffuse term cancels but a non-1/π BRDF does not.
            let l = cosine_dir(n, rng);
            let h = (l + v).normalize();
            let fh = (1.0 - v.dot(h).max(0.0)).clamp(0.0, 1.0).powi(5);
            let p_diff = (1.0 - p_coat - p_spec).max(1e-3);
            for i in 0..3 {
                let sheen = m.sheen_tint[i] * m.sheen * fh * std::f32::consts::PI;
                through[i] *= (diff[i] + sheen) / p_diff;
            }
            rd = l;
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

/// The camera ray through the exact CENTRE of pixel `(x, y)` — no jitter. Used for the denoiser's
/// guide buffers, which must describe one definite surface per pixel rather than an average of
/// whatever fell inside it.
fn cam_ray_centred(cam: &Camera, set: &Settings, x: usize, y: usize) -> (Vec3, Vec3) {
    let fwd = (cam.target - cam.eye).normalize();
    let right = fwd.cross(Vec3::Z).normalize_or_zero();
    let right = if right.length_squared() < 0.5 { Vec3::X } else { right };
    let up = right.cross(fwd);
    let half = (cam.fov_deg.to_radians() * 0.5).tan();
    let aspect = set.w as f32 / set.h.max(1) as f32;
    let px = ((x as f32 + 0.5) / set.w as f32 * 2.0 - 1.0) * half * aspect;
    let py = (1.0 - (y as f32 + 0.5) / set.h as f32 * 2.0) * half;
    (cam.eye, (fwd + right * px + up * py).normalize())
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

/// Linear accumulation → display bytes, through the viewport's own colour pipeline (see
/// [`crate::color`]). This used to be a bare `1 − e⁻ˣ` per channel, which both clipped early and
/// disagreed with what the viewport showed.
#[inline]
pub fn tonemap8(p: crate::color::ColorPipeline, c: [f32; 3]) -> [u8; 3] {
    crate::color::tonemap8(p, c)
}

// ============================ denoising ============================

/// The first-hit surface at each pixel — the guides an edge-aware filter needs to know where it is
/// allowed to blur. Deterministic (no jitter, no random bounce), so it is traced once.
#[derive(Default)]
pub struct Guides {
    /// Albedo at the first hit. Used to **demodulate**: the noise is in the lighting, not in the
    /// material, so dividing it out before filtering and multiplying it back after lets a texture
    /// survive a filter wide enough to actually clean the light.
    pub albedo: Vec<f32>, // w*h*3
    pub normal: Vec<f32>, // w*h*3
    pub depth: Vec<f32>,  // w*h — distance from the eye; f32::MAX for the background
}

/// Edge-aware **à-trous wavelet** denoise (Dammertz et al. 2010), the filter behind every
/// real-time-ish path tracer's "denoise" button.
///
/// Five passes of a 5×5 B-spline kernel with the step doubling each time, so it reaches a 65-pixel
/// neighbourhood at the cost of five 25-tap passes rather than one 4225-tap one. Each tap is
/// weighted by how similar its **colour, normal and depth** are to the centre, which is what stops
/// it smearing across a silhouette or a material boundary.
///
/// Albedo is demodulated first: with it left in, the filter has to preserve texture and remove
/// noise at the same time, and it cannot do both.
pub fn atrous(color: &[f32], g: &Guides, w: usize, h: usize, strength: f32) -> Vec<f32> {
    if w == 0 || h == 0 || color.len() < w * h * 3 || g.depth.len() < w * h {
        return color.to_vec();
    }
    // Demodulate. The floor keeps a near-black material from dividing the noise up to infinity.
    let mut cur = vec![0.0f32; w * h * 3];
    for i in 0..w * h {
        for c in 0..3 {
            let a = g.albedo.get(i * 3 + c).copied().unwrap_or(1.0).max(0.05);
            cur[i * 3 + c] = color[i * 3 + c] / a;
        }
    }

    // FIREFLY REJECTION, before the filter and not instead of it. A single path that found the sun
    // through a near-mirror lobe lands one pixel hundreds of times brighter than its neighbours;
    // the à-trous colour edge-stop then reads it as a genuine feature and *protects* it, so the one
    // artefact the filter cannot remove is the one it is most needed for. Clamping each pixel into
    // its own 3×3 neighbourhood's range first costs nothing on well-sampled pixels — they are
    // already inside it — and removes the outlier before the edge test ever sees it.
    {
        let src = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                for c in 0..3 {
                    let (mut mean, mut m2, mut n) = (0.0f32, 0.0f32, 0.0f32);
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                            if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 || (dx == 0 && dy == 0) {
                                continue;
                            }
                            let v = src[(sy as usize * w + sx as usize) * 3 + c];
                            mean += v;
                            m2 += v * v;
                            n += 1.0;
                        }
                    }
                    if n < 1.0 {
                        continue;
                    }
                    mean /= n;
                    let sd = (m2 / n - mean * mean).max(0.0).sqrt();
                    // ±3σ, plus a floor so a perfectly uniform neighbourhood does not clamp to a
                    // hard constant and erase real gradients.
                    let hi = mean + 3.0 * sd + 0.05;
                    if cur[i * 3 + c] > hi {
                        cur[i * 3 + c] = hi;
                    }
                }
            }
        }
    }

    const KERNEL: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
    // The three edge-stopping widths. Colour loosens as the filter widens (later passes work on
    // already-smoothed data, so a tight colour test would reject everything and do nothing).
    let sigma_n = 0.20f32 / strength.max(0.05);
    let sigma_z = 0.05f32 / strength.max(0.05);
    let mut next = cur.clone();
    for pass in 0..5 {
        let step = 1usize << pass;
        let sigma_c = (0.6f32 * strength) * (1 << pass) as f32;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if g.depth[i] >= f32::MAX * 0.5 {
                    next[i * 3..i * 3 + 3].copy_from_slice(&cur[i * 3..i * 3 + 3]);
                    continue;
                }
                let cn = Vec3::new(g.normal[i * 3], g.normal[i * 3 + 1], g.normal[i * 3 + 2]);
                let cz = g.depth[i];
                let cc = [cur[i * 3], cur[i * 3 + 1], cur[i * 3 + 2]];
                let mut sum = [0.0f32; 3];
                let mut wsum = 0.0f32;
                for (ky, kv) in KERNEL.iter().enumerate() {
                    let sy = y as isize + (ky as isize - 2) * step as isize;
                    if sy < 0 || sy >= h as isize {
                        continue;
                    }
                    for (kx, ku) in KERNEL.iter().enumerate() {
                        let sx = x as isize + (kx as isize - 2) * step as isize;
                        if sx < 0 || sx >= w as isize {
                            continue;
                        }
                        let j = sy as usize * w + sx as usize;
                        if g.depth[j] >= f32::MAX * 0.5 {
                            continue;
                        }
                        let sn = Vec3::new(g.normal[j * 3], g.normal[j * 3 + 1], g.normal[j * 3 + 2]);
                        // Normal: reject anything facing meaningfully differently.
                        let dn = (1.0 - cn.dot(sn).clamp(-1.0, 1.0)).max(0.0);
                        // Depth: RELATIVE, so the same filter works at 2 m and at 200 m.
                        let dz = (g.depth[j] - cz).abs() / cz.abs().max(1e-3);
                        let dc = (0..3).map(|c| (cur[j * 3 + c] - cc[c]).powi(2)).sum::<f32>();
                        let wgt = kv * ku
                            * (-dn / (sigma_n * sigma_n) - dz / (sigma_z * sigma_z) - dc / (sigma_c * sigma_c)).exp();
                        if wgt <= 0.0 {
                            continue;
                        }
                        for c in 0..3 {
                            sum[c] += cur[j * 3 + c] * wgt;
                        }
                        wsum += wgt;
                    }
                }
                if wsum > 1e-8 {
                    for c in 0..3 {
                        next[i * 3 + c] = sum[c] / wsum;
                    }
                } else {
                    next[i * 3..i * 3 + 3].copy_from_slice(&cur[i * 3..i * 3 + 3]);
                }
            }
        }
        std::mem::swap(&mut cur, &mut next);
    }

    // Re-modulate: the texture comes back exactly as it was, having never been filtered.
    for i in 0..w * h {
        for c in 0..3 {
            let a = g.albedo.get(i * 3 + c).copied().unwrap_or(1.0).max(0.05);
            cur[i * 3 + c] *= a;
        }
    }
    cur
}

// ============================ progressive job ============================

/// Shared state between the render worker and the UI.
pub struct Shared {
    /// RGB32F accumulation, `w*h*3`, sum over completed passes.
    pub accum: Mutex<Vec<f32>>,
    /// First-hit surface data for the denoiser, traced once before the first pass.
    pub guides: Mutex<Guides>,
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
            guides: Mutex::new(Guides::default()),
            passes_done: AtomicU32::new(0),
            cancel: AtomicBool::new(false),
            done: AtomicBool::new(false),
        });
        let scene_tris = scene.tri_count();
        let sh = shared.clone();
        let handle = std::thread::spawn(move || {
            let scene = &scene;
            // Trace the first hit once, un-jittered, for the denoiser's guides. One extra ray per
            // pixel with no bounces — negligible beside even a single sample pass.
            {
                let mut g = Guides {
                    albedo: vec![1.0; settings.w * settings.h * 3],
                    normal: vec![0.0; settings.w * settings.h * 3],
                    depth: vec![f32::MAX; settings.w * settings.h],
                };
                for y in 0..settings.h {
                    for x in 0..settings.w {
                        let i = y * settings.w + x;
                        let mut rng = Rng::new(1);
                        let (ro, rd) = cam_ray_centred(&cam, &settings, x, y);
                        let _ = &mut rng;
                        if let Some(h) = scene.intersect(ro, rd) {
                            let tri = &scene.tris[h.tri as usize];
                            let p = ro + rd * h.t;
                            let n = if tri.n.dot(rd) > 0.0 { -tri.n } else { tri.n };
                            let alb = match tri.mat.proc {
                                Some(def) => {
                                    let (c, s) = (scene.proc_rot.cos(), scene.proc_rot.sin());
                                    let local = Vec3::new(p.x * c + p.y * s, -p.x * s + p.y * c, p.z);
                                    crate::proc_tex::sample(&def, local, n, tri.mat.rough).albedo
                                }
                                None => tri.mat.albedo,
                            };
                            g.albedo[i * 3..i * 3 + 3].copy_from_slice(&alb);
                            g.normal[i * 3..i * 3 + 3].copy_from_slice(&n.to_array());
                            g.depth[i] = h.t;
                        }
                    }
                }
                *sh.guides.lock().unwrap() = g;
            }
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
                        let sky = &sky;
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
                                    let c = trace(scene, sky, ro, rd, settings.max_depth, &mut rng);
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
    ///
    /// With `denoise`, the linear image goes through the à-trous filter first — the guides were
    /// traced before pass 0, so this works at any sample count. It is applied to the LINEAR image,
    /// before the view transform: filtering display-encoded pixels averages them in the wrong space
    /// and darkens every edge it touches.
    pub fn snapshot_rgba_opt(&self, denoise: bool) -> Option<(usize, usize, Vec<u8>)> {
        let n = self.shared.passes_done.load(Ordering::Relaxed);
        if n == 0 {
            return None;
        }
        let (w, h) = (self.settings.w, self.settings.h);
        let lin: Vec<f32> = {
            let acc = self.shared.accum.lock().unwrap();
            let inv = 1.0 / n as f32;
            acc.iter().map(|v| v * inv).collect()
        };
        let lin = if denoise {
            let g = self.shared.guides.lock().unwrap();
            // Filter harder when there are few samples and back off as the render converges —
            // past a few hundred samples the noise is already below what the filter would cost in
            // detail.
            let strength = (24.0 / (n as f32).max(1.0)).clamp(0.15, 1.5);
            atrous(&lin, &g, w, h, strength)
        } else {
            lin
        };
        let mut out = Vec::with_capacity(w * h * 4);
        for px in lin.chunks_exact(3) {
            let [r, g, b] = tonemap8(self.settings.color, [px[0], px[1], px[2]]);
            out.extend_from_slice(&[r, g, b, 255]);
        }
        Some((w, h, out))
    }

    /// The un-denoised snapshot — what every existing caller wants.
    pub fn snapshot_rgba(&self) -> Option<(usize, usize, Vec<u8>)> {
        self.snapshot_rgba_opt(false)
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

    /// A flat two-colour environment (`dome: None`) — the simplest thing to reason about, so the
    /// tests measure the tracer rather than the sky model.
    fn sky() -> Sky {
        Sky {
            sun_dir: Vec3::new(0.0, 0.0, 1.0),
            sun_col: [2.0, 2.0, 2.0],
            sky_col: [0.5, 0.5, 0.6],
            ground_col: [0.2, 0.2, 0.2],
            dome: None,
            env: None,
            env_strength: 1.0,
            env_rot: 0.0,
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
                .filter_map(|t| ray_tri(ro, rd, t).map(|(d, _, _)| d))
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

    /// A uniform environment of radiance `L` around a white surface must come back as `L`.
    ///
    /// This is the **furnace test**, and it is the only way to catch a wrong importance-sampling
    /// weight: a bad weight still produces a picture, still converges, and converges to the wrong
    /// brightness. The old bounce ("mirror lerped toward a cosine lobe") failed it by a factor of
    /// several at high roughness, which is why glossy materials never looked right.
    ///
    /// The band is deliberately not tight. This BSDF is single-scattering GGX plus a full Lambert
    /// diffuse, so it LOSES energy at high roughness (no multiple scattering between microfacets)
    /// and GAINS a little on dielectrics (the diffuse is not reduced by the Fresnel that was
    /// reflected). Both are properties of the model, shared with the viewport, and both are far
    /// smaller than the errors a sampling mistake makes.
    #[test]
    fn a_white_surface_in_a_uniform_furnace_returns_what_it_receives() {
        const L: f32 = 0.6;
        let furnace = Sky { sun_dir: Vec3::Z, sun_col: [0.0; 3], sky_col: [L; 3], ground_col: [L; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        // A single large white quad; rays that miss it see the furnace directly.
        let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [1.0; 3], 1.0).into();
        for t in &mut tris {
            t.roughness = 0.0;
        }
        for (rough, metallic, lo, hi) in [
            (1.0f32, 0.0f32, 0.85f32, 1.30f32), // rough dielectric
            (0.3, 0.0, 0.85, 1.30),             // semi-gloss dielectric
            (0.1, 1.0, 0.70, 1.15),             // near-mirror metal (white F0)
            (0.7, 1.0, 0.55, 1.10),             // rough metal — single-scattering GGX loses most here
        ] {
            let mut src = tris.clone();
            for t in &mut src {
                t.roughness = rough;
                t.metallic = metallic;
            }
            let scene = Scene::build(&src);
            let mut rng = Rng::new(11);
            let mut sum = 0.0f64;
            const N: usize = 3000;
            for _ in 0..N {
                // Straight down onto the quad, so every path starts on the surface.
                let c = trace(&scene, &furnace, Vec3::new(0.0, 0.0, 4.0), Vec3::NEG_Z, 6, &mut rng);
                sum += c[0] as f64;
            }
            let got = (sum / N as f64) as f32 / L;
            assert!(
                (lo..=hi).contains(&got),
                "rough {rough} metallic {metallic}: returned {got:.3}× the furnace radiance (want {lo}..{hi})"
            );
        }
    }

    /// The packed stream must hold exactly what the GPU shader expects to read.
    #[test]
    fn the_gpu_pack_carries_every_material_field() {
        let mut t = ExportTri::plain([[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], [0.5; 3], 0.4, 1.0);
        t.clearcoat = 0.7;
        t.clearcoat_rough = 0.25;
        t.sheen = 0.6;
        t.sheen_tint = [0.2, 0.4, 0.8];
        let pack = Scene::build(&[t]).pack_gpu();
        assert_eq!(pack.tris.len(), TRI_TEXELS * 4, "one triangle occupies exactly TRI_TEXELS texels");
        assert!((pack.tris[4 * 4 + 3] - 0.6).abs() < 1e-6, "sheen rides in the emission texel's w");
        assert!((pack.tris[5 * 4] - 0.7).abs() < 1e-6, "clearcoat");
        assert!((pack.tris[5 * 4 + 1] - 0.25).abs() < 1e-6, "clearcoat roughness");
        // The tint packs and unpacks to within a 255th — the same arithmetic the shader does.
        let p = pack.tris[3 * 4 + 3];
        let r = (p / 65536.0).floor();
        let g = ((p - r * 65536.0) / 256.0).floor();
        let got = [r / 255.0, g / 255.0, (p - r * 65536.0 - g * 256.0) / 255.0];
        for i in 0..3 {
            assert!((got[i] - [0.2, 0.4, 0.8][i]).abs() < 1.0 / 255.0, "tint channel {i}: {got:?}");
        }
    }

    /// A CLEARCOAT must not create light. This is the test a bolted-on lobe fails.
    ///
    /// Adding a second specular lobe on top of a finished BSDF is the single easiest way to break
    /// energy conservation: the coat reflects the room AND the material underneath still reflects
    /// everything it did before, so a varnished white wall comes back brighter than the furnace
    /// around it. The fix is that the base has to LOSE what the coat took, and this is what proves
    /// it did.
    #[test]
    fn a_clearcoat_adds_a_reflection_without_adding_energy() {
        const L: f32 = 0.6;
        let furnace = Sky { sun_dir: Vec3::Z, sun_col: [0.0; 3], sky_col: [L; 3], ground_col: [L; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        let measure = |coat: f32| -> f32 {
            let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [1.0; 3], 1.0).into();
            for t in &mut tris {
                t.roughness = 0.6;
                t.clearcoat = coat;
                t.clearcoat_rough = 0.05;
            }
            let scene = Scene::build(&tris);
            let mut rng = Rng::new(17);
            let mut sum = 0.0f64;
            const N: usize = 4000;
            for _ in 0..N {
                sum += trace(&scene, &furnace, Vec3::new(0.0, 0.0, 4.0), Vec3::NEG_Z, 6, &mut rng)[0] as f64;
            }
            (sum / N as f64) as f32 / L
        };
        let bare = measure(0.0);
        let lacquered = measure(1.0);
        // The same band the furnace test allows for the base model, no wider: the coat must not
        // push a white surface past what a white surface can return.
        assert!(
            (0.85..=1.30).contains(&lacquered),
            "a fully lacquered white surface returned {lacquered:.3}× the furnace (bare: {bare:.3}×)"
        );
        // NOTE: no assertion that the two DIFFER. A furnace is blind to the shape of a BRDF by
        // construction — reflecting L back is indistinguishable from diffusing L back — so the
        // coat legitimately changes nothing here. That it does something is the next test's job.
        let _ = bare;
    }

    /// …and it must put a glint where the base material could not.
    ///
    /// The base here is black and fully rough: its own specular is the 4% every dielectric has,
    /// smeared across the whole hemisphere. A tight bright highlight can therefore only be the
    /// coat, which is exactly the claim — a clearcoat is a SMOOTH layer over a ROUGH one, and that
    /// combination is not expressible by a roughness slider at all.
    #[test]
    fn a_clearcoat_puts_a_glint_where_the_base_could_not() {
        let make = |coat: f32| {
            let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [0.0; 3], 1.0).into();
            for t in &mut tris {
                t.roughness = 1.0;
                t.clearcoat = coat;
                t.clearcoat_rough = 0.03;
            }
            Scene::build(&tris)
        };
        // The mirror configuration: eye and sun placed so the view reflects straight into the sun.
        let sky = Sky { sun_dir: Vec3::new(0.0, 0.6, 0.8).normalize(), sun_col: [4.0; 3], sky_col: [0.0; 3], ground_col: [0.0; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        let eye = Vec3::new(0.0, -3.0, 4.0);
        let dir = (Vec3::ZERO - eye).normalize();
        let look = |scene: &Scene| {
            let mut rng = Rng::new(29);
            let mut sum = 0.0f64;
            for _ in 0..1200 {
                sum += trace(scene, &sky, eye, dir, 3, &mut rng)[0] as f64;
            }
            (sum / 1200.0) as f32
        };
        let bare = look(&make(0.0));
        let coated = look(&make(1.0));
        assert!(coated > bare * 3.0, "lacquered {coated:.4} vs bare {bare:.4} — no glint");
    }

    /// SHEEN must brighten a fabric at GRAZING angles and leave it alone head-on.
    ///
    /// That angular dependence is the whole effect — a term that lifted the surface uniformly
    /// would just be a brighter albedo, and velvet would still look like plastic.
    #[test]
    fn sheen_shows_at_grazing_angles_and_not_head_on() {
        // A black cloth, so anything returned can only be sheen. Sun overhead.
        let make = |sheen: f32| {
            let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [0.0; 3], 1.0).into();
            for t in &mut tris {
                t.roughness = 1.0;
                t.sheen = sheen;
                t.sheen_tint = [1.0; 3];
            }
            Scene::build(&tris)
        };
        let sky = Sky { sun_dir: Vec3::new(0.0, 0.35, 0.94).normalize(), sun_col: [4.0; 3], sky_col: [0.0; 3], ground_col: [0.0; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        let look = |scene: &Scene, eye: Vec3| {
            let dir = (Vec3::ZERO - eye).normalize();
            let mut rng = Rng::new(23);
            let mut sum = 0.0f64;
            for _ in 0..1500 {
                sum += trace(scene, &sky, eye, dir, 2, &mut rng)[0] as f64;
            }
            (sum / 1500.0) as f32
        };
        let fabric = make(1.0);
        let plain = make(0.0);
        let head_eye = Vec3::new(0.0, 0.0, 6.0);
        let graze_eye = Vec3::new(0.0, -12.0, 0.9);
        // Measured as a DIFFERENCE against the same surface without sheen. A black dielectric
        // already brightens at grazing angles all on its own — that is Fresnel, and every surface
        // does it — so comparing the fabric against nothing would credit the sheen with an effect
        // that was there before it.
        let d_head = look(&fabric, head_eye) - look(&plain, head_eye);
        let d_graze = look(&fabric, graze_eye) - look(&plain, graze_eye);
        assert!(d_graze > 0.0, "sheen added nothing at all at a grazing angle");
        assert!(
            d_graze > d_head * 3.0,
            "sheen added {d_graze:.4} grazing and {d_head:.4} head-on — that is a brighter albedo, not a rim"
        );
    }

    /// The sun must reach a glossy surface as a HIGHLIGHT, not only as a diffuse term. Before this,
    /// next-event estimation multiplied by albedo alone, so a polished floor had a sun highlight in
    /// the viewport and none at all in the render.
    #[test]
    fn the_sun_makes_a_specular_highlight() {
        // A mirror-ish floor with a BLACK albedo: any light returned can only be specular.
        let mut tris: Vec<ExportTri> = quad(0.0, 50.0, [0.0; 3], 1.0).into();
        for t in &mut tris {
            t.roughness = 0.08;
            t.metallic = 1.0;
        }
        // A black metal has F0 = 0, so give it a white one to reflect with.
        for t in &mut tris {
            t.rgb = [1.0; 3];
        }
        let scene = Scene::build(&tris);
        // Sun low in +y so its mirror direction leaves along −y; look straight into that.
        let sky = Sky { sun_dir: Vec3::new(0.0, 0.6, 0.8).normalize(), sun_col: [4.0; 3], sky_col: [0.0; 3], ground_col: [0.0; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        let mut rng = Rng::new(5);
        let eye = Vec3::new(0.0, -3.0, 4.0);
        let dir = (Vec3::ZERO - eye).normalize();
        let mut sum = 0.0f64;
        for _ in 0..800 {
            sum += trace(&scene, &sky, eye, dir, 3, &mut rng)[0] as f64;
        }
        let lit = sum / 800.0;
        // …and with the sun switched off there is nothing at all to see.
        let dark = Sky { sun_col: [0.0; 3], ..sky };
        let mut sum2 = 0.0f64;
        for _ in 0..800 {
            sum2 += trace(&scene, &dark, eye, dir, 3, &mut rng)[0] as f64;
        }
        assert!(lit > 0.05, "the sun must produce a specular return on a black-diffuse metal: {lit}");
        assert!(lit > sum2 / 800.0 * 10.0 + 0.01, "and it must be the sun doing it: {lit} vs {}", sum2 / 800.0);
    }

    /// A procedural material must render its PATTERN, not its average colour. This is the whole
    /// reason `ExportTri::material` and [`crate::proc_tex`] exist.
    #[test]
    fn a_procedural_material_shows_its_grain_in_a_render() {
        let oak = crate::factory::ProcDef::oak();
        let mut tris: Vec<ExportTri> = quad(0.0, 50.0, oak.avg_color(), 1.0).into();
        for t in &mut tris {
            t.material = Some(0);
        }
        let scene = Scene::build_with(&tris, &[Some(oak)], 0.0);
        let flat = Scene::build(&tris);
        let sky = Sky { sun_dir: Vec3::Z, sun_col: [1.5; 3], sky_col: [0.3; 3], ground_col: [0.3; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        // Sample a line of points across the plank direction and measure the spread of each.
        let spread = |sc: &Scene| {
            let mut rng = Rng::new(3);
            let vals: Vec<f32> = (0..40)
                .map(|i| {
                    let x = i as f32 * 0.01;
                    let mut s = 0.0;
                    for _ in 0..24 {
                        s += trace(sc, &sky, Vec3::new(x, 0.0, 3.0), Vec3::NEG_Z, 2, &mut rng)[0];
                    }
                    s / 24.0
                })
                .collect();
            let mx = vals.iter().cloned().fold(f32::MIN, f32::max);
            let mn = vals.iter().cloned().fold(f32::MAX, f32::min);
            (mx - mn) / mx.max(1e-6)
        };
        let with = spread(&scene);
        let without = spread(&flat);
        assert!(with > without * 2.0 + 0.05, "the grain must vary across the surface: {with:.3} vs flat {without:.3}");
    }

    /// A 2×2 sRGB image: red / green on the bottom row, blue / white on the top.
    fn checker_tex(triplanar: bool, tiles_per_m: f32) -> TexImage {
        #[rustfmt::skip]
        let rgba: Vec<u8> = vec![
            255, 0, 0, 255,   0, 255, 0, 255,   // v = 0 row: red, green
            0, 0, 255, 255,   255, 255, 255, 255, // v = 1 row: blue, white
        ];
        TexImage { w: 2, h: 2, rgba: std::sync::Arc::new(rgba), triplanar, tiles_per_m }
    }

    /// A quad whose UVs span the full 0..1 square, so each corner lands in a different texel.
    fn uv_quad(z: f32, half: f32) -> [ExportTri; 2] {
        let v = |x: f32, y: f32| [x, y, z];
        let mut t = quad(z, half, [0.5; 3], 1.0);
        // The quad is (-h,-h) (h,-h) (h,h) and (-h,-h) (h,h) (-h,h); map x,y linearly to u,v.
        t[0].uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        t[1].uv = [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        t[0].has_uv = true;
        t[1].has_uv = true;
        let _ = v;
        t
    }

    /// THE point of the texture work: a material's IMAGE must reach the render. Before this the
    /// tracer only had `albedo` — the image's average — so an offline villa had flat terracotta
    /// roofs where the viewport showed tiles. Traced straight down at four points that fall in
    /// four different texels, the returned colours must differ the way the image does.
    #[test]
    fn an_image_map_is_sampled_at_the_hit_point() {
        let mut tris: Vec<ExportTri> = uv_quad(0.0, 1.0).into();
        for t in &mut tris {
            t.material = Some(0);
        }
        let scene = Scene::build_full(&tris, &[], 0.0, vec![checker_tex(false, 1.0)], &[Some(0)]);
        let sky = Sky { sun_dir: Vec3::Z, sun_col: [1.5; 3], sky_col: [0.3; 3], ground_col: [0.3; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        // Trace at the centre of each quadrant → the centre of each texel.
        let at = |u: f32, v: f32| {
            let mut rng = Rng::new(11);
            let x = (u - 0.5) * 2.0;
            let y = (v - 0.5) * 2.0;
            let mut s = [0.0f32; 3];
            for _ in 0..32 {
                let c = trace(&scene, &sky, Vec3::new(x, y, 3.0), Vec3::NEG_Z, 2, &mut rng);
                for k in 0..3 {
                    s[k] += c[k];
                }
            }
            [s[0] / 32.0, s[1] / 32.0, s[2] / 32.0]
        };
        let red = at(0.25, 0.25);
        let green = at(0.75, 0.25);
        let blue = at(0.25, 0.75);
        let white = at(0.75, 0.75);
        assert!(red[0] > red[1] * 3.0 && red[0] > red[2] * 3.0, "bottom-left texel is red: {red:?}");
        assert!(green[1] > green[0] * 3.0 && green[1] > green[2] * 3.0, "bottom-right is green: {green:?}");
        assert!(blue[2] > blue[0] * 3.0 && blue[2] > blue[1] * 3.0, "top-left is blue: {blue:?}");
        let w_min = white[0].min(white[1]).min(white[2]);
        assert!(w_min > red[0] * 0.5, "top-right is white — brighter than any single primary: {white:?}");
    }

    /// A surface with NO UV layer must still get its image, projected from world space the way the
    /// viewport shader projects it. Otherwise every CSG wall in an offline render falls back to a
    /// flat average while the preview shows the material tiling across it.
    #[test]
    fn a_surface_without_uvs_projects_the_image_from_world_space() {
        // Two tiles per metre over a 2 m quad ⇒ the pattern repeats across the surface.
        let mut tris: Vec<ExportTri> = quad(0.0, 1.0, [0.5; 3], 1.0).into();
        for t in &mut tris {
            t.material = Some(0);
            t.has_uv = false;
        }
        let scene = Scene::build_full(&tris, &[], 0.0, vec![checker_tex(true, 1.0)], &[Some(0)]);
        let sky = Sky { sun_dir: Vec3::Z, sun_col: [1.5; 3], sky_col: [0.3; 3], ground_col: [0.3; 3], dome: None, env: None, env_strength: 1.0, env_rot: 0.0 };
        let at = |x: f32, y: f32| {
            let mut rng = Rng::new(5);
            let mut s = [0.0f32; 3];
            for _ in 0..32 {
                let c = trace(&scene, &sky, Vec3::new(x, y, 3.0), Vec3::NEG_Z, 2, &mut rng);
                for k in 0..3 {
                    s[k] += c[k];
                }
            }
            [s[0] / 32.0, s[1] / 32.0, s[2] / 32.0]
        };
        // The quad's normal is +Z, so the projection uses (x, y) — quarter-metre steps land in
        // different texels of the 2x2 image at one tile per metre.
        let a = at(0.25, 0.25);
        let b = at(0.75, 0.25);
        let diff: f32 = (0..3).map(|k| (a[k] - b[k]).abs()).sum();
        assert!(diff > 0.1, "a projected image must vary across the surface: {a:?} vs {b:?}");
    }

    /// A material carrying only a 1x1 colour swatch must NOT become a texture fetch — the swatch
    /// is exactly the flat albedo the tracer already had, and most imported materials are swatches.
    #[test]
    fn a_one_by_one_swatch_is_not_promoted_to_a_texture() {
        let mut st = crate::factory::FactoryState::default();
        st.add_texture("swatch".into(), 1, 1, vec![200, 40, 40, 255]);
        st.add_texture("image".into(), 2, 2, checker_tex(false, 1.0).rgba.as_ref().clone());
        let (pool, index) = st.export_texture_table();
        assert_eq!(pool.len(), 1, "only the real image joins the pool");
        assert_eq!(index[0], None, "the 1x1 swatch stays a flat albedo");
        assert_eq!(index[1], Some(0), "the image points at the pooled texture");
    }

    /// The denoiser must remove noise WITHOUT bleeding across a depth discontinuity — the failure
    /// that turns a filter into a smear. Built as two constant-colour regions at very different
    /// depths, with noise on top: the noise must go and the step must stay.
    #[test]
    fn the_denoiser_smooths_noise_but_not_silhouettes() {
        let (w, h) = (48usize, 32usize);
        let mut g = Guides { albedo: vec![1.0; w * h * 3], normal: vec![0.0; w * h * 3], depth: vec![0.0; w * h] };
        let mut noisy = vec![0.0f32; w * h * 3];
        let mut rng = Rng::new(9);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let near = x < w / 2;
                g.depth[i] = if near { 2.0 } else { 40.0 };
                g.normal[i * 3 + 2] = 1.0;
                let base = if near { 0.8 } else { 0.2 };
                for c in 0..3 {
                    noisy[i * 3 + c] = base + (rng.next_f32() - 0.5) * 0.6;
                }
            }
        }
        let out = atrous(&noisy, &g, w, h, 1.0);
        let stats = |img: &[f32], x0: usize, x1: usize| {
            let vals: Vec<f32> = (0..h).flat_map(|y| (x0..x1).map(move |x| (y * w + x) * 3)).map(|i| img[i]).collect();
            let mean = vals.iter().sum::<f32>() / vals.len() as f32;
            let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / vals.len() as f32;
            (mean, var.sqrt())
        };
        let (m_in, s_in) = stats(&noisy, 4, w / 2 - 4);
        let (m_out, s_out) = stats(&out, 4, w / 2 - 4);
        assert!(s_out < s_in * 0.4, "noise must actually fall: {s_in:.3} -> {s_out:.3}");
        assert!((m_out - m_in).abs() < 0.05, "and the mean must survive: {m_in:.3} -> {m_out:.3}");
        // The step across the silhouette must not have been rounded off.
        let left = stats(&out, w / 2 - 3, w / 2 - 1).0;
        let right = stats(&out, w / 2 + 1, w / 2 + 3).0;
        assert!(left - right > 0.45, "the depth discontinuity must survive: {left:.3} vs {right:.3} (input step 0.6)");
    }

    #[test]
    fn render_job_runs_to_completion_and_snapshots() {
        let tris: Vec<ExportTri> = quad(0.0, 50.0, [0.6, 0.4, 0.3], 1.0).into();
        let scene = Scene::build(&tris);
        let cam = Camera { eye: Vec3::new(0.0, -8.0, 5.0), target: Vec3::ZERO, fov_deg: 45.0 };
        let set = Settings { w: 32, h: 24, passes: 2, max_depth: 3, color: crate::color::ColorPipeline::default() };
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
