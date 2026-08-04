//! A small CPU-rendered **preview of a mesh**, for looking at a parametric build before it is
//! inserted into the scene.
//!
//! The viewport can already draw anything this draws — but only after the thing exists, somewhere
//! in the model, at some position the user then has to find and possibly undo. A preview is for the
//! decision *before* that: is this the door I meant? Which is why it renders the mesh it is handed,
//! not the scene.
//!
//! It is deliberately the **same shading as [`crate::matball`]** — [`crate::matball::shade_point`],
//! [`crate::matball::preview_sky`] and [`crate::color`] — which is in turn the same maths as the
//! viewport's shader. A preview lit by its own private lighting model is a preview that can lie,
//! and the whole reason to show one is that it does not.
//!
//! Two passes, because they have very different costs:
//!
//! 1. **Raster** — one thread, a z-buffer over triangles, writing a G-buffer of depth, normal and
//!    surface. Triangle setup is cheap; a door is a few thousand triangles.
//! 2. **Shade** — every covered pixel runs the full GGX + SH-ambient + environment BRDF, which is
//!    a hundred times the cost of rasterizing it. That pass is split across cores over disjoint row
//!    bands, so orbiting stays interactive.

use crate::color::ColorPipeline;
use glam::Vec3;

/// How one part id should look. Parametric builders tag their triangles by component, which is
/// exactly the granularity a preview wants: wood reads as wood and hardware reads as metal without
/// anyone authoring a material.
#[derive(Clone, Copy, Debug)]
pub struct PartLook {
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    /// 1.0 = opaque. Below it the surface is blended with what is behind — see the note in
    /// [`render`] about what "behind" means here.
    pub opacity: f32,
    /// A procedural pattern, evaluated in WORLD space exactly as the viewport's shader evaluates
    /// it. This is what lets a preview show the actual grain rather than an average brown.
    pub proc: Option<crate::factory::ProcDef>,
}

impl Default for PartLook {
    fn default() -> Self {
        Self { albedo: [0.55, 0.55, 0.55], roughness: 0.5, metallic: 0.0, opacity: 1.0, proc: None }
    }
}

/// Where the camera is. Angles in radians, `zoom` a multiplier on the fitted distance (bigger =
/// closer).
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Orbit {
    /// A three-quarter view from slightly above — the angle that shows a door's face, its leading
    /// edge and the projection of its handle in one picture.
    fn default() -> Self {
        Self { yaw: -0.62, pitch: 0.20, zoom: 1.0 }
    }
}

/// The six orthogonal views.
///
/// A free orbit re-renders on every mouse-move frame, and this renderer is far too expensive for
/// that — one frame is a hundred thousand GGX evaluations, and a timber surface adds four fBm
/// samples on top of each of those. Six fixed views turn a continuous cost into a discrete one:
/// nothing is drawn until you ask for a different view, and then exactly one image is.
///
/// They are also what you actually want from a joinery preview. "Is the handle at the right
/// height" and "how far does the architrave stand proud" are questions an elevation answers and a
/// tumbling three-quarter view does not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum View {
    #[default]
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl View {
    pub const ALL: [View; 6] =
        [View::Front, View::Back, View::Left, View::Right, View::Top, View::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            View::Front => "Front",
            View::Back => "Back",
            View::Left => "Left",
            View::Right => "Right",
            View::Top => "Top",
            View::Bottom => "Bottom",
        }
    }

    /// The camera for this view at a given zoom. Yaw swings about Z from the FRONT (the camera on
    /// −Y looking toward +Y, which is the face of a door); pitch tips over the top.
    pub fn orbit(self, zoom: f32) -> Orbit {
        use std::f32::consts::{FRAC_PI_2, PI};
        let (yaw, pitch) = match self {
            View::Front => (0.0, 0.0),
            View::Back => (PI, 0.0),
            View::Left => (-FRAC_PI_2, 0.0),
            View::Right => (FRAC_PI_2, 0.0),
            View::Top => (0.0, FRAC_PI_2),
            View::Bottom => (0.0, -FRAC_PI_2),
        };
        Orbit { yaw, pitch, zoom }
    }
}

/// Vertical field of view.
const FOV: f32 = 32.0;

/// One rasterized fragment, before shading. Carries the WORLD position because a procedural is
/// evaluated in world space — the same rule the viewport's shader follows, and the reason a grain
/// runs on across two parts that meet instead of restarting at the seam.
#[derive(Clone, Copy)]
struct Frag {
    depth: f32,
    p: [f32; 3],
    n: [f32; 3],
    look: PartLook,
}

/// Render `size × size` RGBA8. `ss` supersamples (1 = off, 2 = 4× the samples) and is the knob for
/// trading edge quality against latency while the user is dragging.
///
/// `positions`/`normals` are triangle soup (3 vertices per triangle, flat arrays) and `part_ids` is
/// one id per triangle — the layout every parametric builder in `cad_solid` already emits.
pub fn render(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    part_ids: &[u32],
    look_of: &dyn Fn(u32) -> PartLook,
    orbit: Orbit,
    size: usize,
    ss: usize,
    color: ColorPipeline,
) -> Vec<u8> {
    let size = size.max(16);
    let ss = ss.clamp(1, 3);
    let w = size * ss;
    let tris = positions.len() / 3;

    // ── Fit the camera to the mesh ────────────────────────────────────────────────────────────
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    if tris == 0 || lo[0] > hi[0] {
        return backdrop_only(size, color);
    }
    let centre = Vec3::new((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5);

    let (cy, sy) = (orbit.yaw.cos(), orbit.yaw.sin());
    let (cp, sp) = (orbit.pitch.cos(), orbit.pitch.sin());
    // The door is built with +Y running INTO the wall, so the camera comes from −Y to look at its
    // front face; yaw swings around Z from there.
    let dir = Vec3::new(-sy * cp, -cy * cp, sp);
    let fwd = -dir;
    // Straight down or straight up, `fwd × Z` is zero — the fallback is what makes the top and
    // bottom views work at all rather than collapsing to a degenerate basis.
    let right = fwd.cross(Vec3::Z).normalize_or(Vec3::X);
    let up = right.cross(fwd);

    // Fit to the box's extent ACROSS the view, not to its bounding sphere. A door is 2.1 m tall and
    // 0.15 m deep, so its sphere is seven times what a plan view of it actually needs — fitting to
    // the sphere leaves the top and bottom views as a sliver in the middle of an empty frame.
    let (mut hw, mut hh, mut hd) = (1e-4f32, 1e-4f32, 1e-4f32);
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ) - centre;
        hw = hw.max(c.dot(right).abs());
        hh = hh.max(c.dot(up).abs());
        hd = hd.max(c.dot(fwd).abs());
    }
    let half_fov = (FOV.to_radians() * 0.5).tan();
    // Zoom scales the STANDOFF only, so `dist > hd` always holds and the near plane below stays in
    // front of the object however far in the user pushes.
    let dist = hd + (hw.max(hh) * 1.10 / half_fov) / orbit.zoom.clamp(0.2, 8.0);
    let eye = centre + dir * dist;
    let near = (dist - hd) * 0.5;

    // ── Pass 1: raster into a G-buffer ────────────────────────────────────────────────────────
    let mut gbuf: Vec<Option<Frag>> = vec![None; w * w];
    let half = w as f32 * 0.5;
    // World → view, then a 1/z projection. Kept explicit rather than a matrix because the
    // perspective-correct interpolation below needs the view-space z anyway.
    let to_view = |p: Vec3| {
        let d = p - eye;
        Vec3::new(d.dot(right), d.dot(up), d.dot(fwd))
    };
    for t in 0..tris {
        let v: [Vec3; 3] = [
            to_view(Vec3::from(positions[t * 3])),
            to_view(Vec3::from(positions[t * 3 + 1])),
            to_view(Vec3::from(positions[t * 3 + 2])),
        ];
        // No near-plane clipping: the camera sits outside the bounding sphere, so a triangle that
        // crosses the near plane means the user has zoomed inside the object — drop it rather than
        // let a divide by ~0 smear one triangle across the whole image.
        if v.iter().any(|p| p.z <= near) {
            continue;
        }
        let s: [(f32, f32); 3] = std::array::from_fn(|i| {
            let inv = 1.0 / (v[i].z * half_fov);
            (half + v[i].x * inv * half, half - v[i].y * inv * half)
        });
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
        if area.abs() < 1e-9 {
            continue;
        }
        let x0 = s.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let x1 = (s.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as isize).clamp(0, w as isize) as usize;
        let y0 = s.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let y1 = (s.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as isize).clamp(0, w as isize) as usize;
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let look = look_of(part_ids.get(t).copied().unwrap_or(0));
        let n: [Vec3; 3] = [
            Vec3::from(normals[t * 3]),
            Vec3::from(normals[t * 3 + 1]),
            Vec3::from(normals[t * 3 + 2]),
        ];
        let wp: [Vec3; 3] = [
            Vec3::from(positions[t * 3]),
            Vec3::from(positions[t * 3 + 1]),
            Vec3::from(positions[t * 3 + 2]),
        ];
        let inv_area = 1.0 / area;
        for py in y0..y1 {
            for px in x0..x1 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = ((s[1].0 - fx) * (s[2].1 - fy) - (s[2].0 - fx) * (s[1].1 - fy)) * inv_area;
                let w1 = ((s[2].0 - fx) * (s[0].1 - fy) - (s[0].0 - fx) * (s[2].1 - fy)) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                // Perspective-correct: interpolate 1/z, invert.
                let inv_z = w0 / v[0].z + w1 / v[1].z + w2 / v[2].z;
                if inv_z <= 0.0 {
                    continue;
                }
                let z = 1.0 / inv_z;
                let o = py * w + px;
                if gbuf[o].is_some_and(|f| f.depth <= z) {
                    continue;
                }
                let (b0, b1, b2) = (w0 / v[0].z * z, w1 / v[1].z * z, w2 / v[2].z * z);
                let nn = (n[0] * b0 + n[1] * b1 + n[2] * b2).normalize_or(Vec3::Z);
                let pp = wp[0] * b0 + wp[1] * b1 + wp[2] * b2;
                gbuf[o] = Some(Frag { depth: z, p: pp.into(), n: nn.into(), look });
            }
        }
    }

    // ── Pass 2: shade, in parallel over row bands ─────────────────────────────────────────────
    let (sky, sh, sun_col) = crate::matball::preview_sky();
    let sun_dir = sky.sun_dir;
    // The contact shadow's footprint on the backdrop, in screen pixels: without it a preview object
    // floats, and "floating" is the first thing the eye reads as unreal.
    // …but only when there is a floor to cast it on. Looking straight down or straight up, the
    // ground plane is perpendicular to the view and a shadow under the object is meaningless.
    let base = Vec3::new(centre.x, centre.y, lo[2]);
    let shadow = (dir.z.abs() < 0.9).then_some(()).and_then(|_| project(base, &to_view, half, half_fov)).map(|(sx, sy2)| {
        let rx = (hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.62;
        let edge = project(base + right * rx, &to_view, half, half_fov);
        let r = edge.map(|(ex, _)| (ex - sx).abs()).unwrap_or(w as f32 * 0.2).max(4.0);
        (sx, sy2, r, r * 0.30)
    });

    // A supersampled render is the SETTLED one, so it also pays for the procedural relief; `ss == 1`
    // is the draft the user sees while they are still changing something.
    let relief = ss > 1;
    let mut hi_res = vec![0u8; w * w * 4];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
    let band = w.div_ceil(threads);
    std::thread::scope(|scope| {
        for (bi, rows) in hi_res.chunks_mut(band * w * 4).enumerate() {
            let gbuf = &gbuf;
            let sky = &sky;
            let sh = &sh;
            scope.spawn(move || {
                let y_base = bi * band;
                for (ry, row) in rows.chunks_mut(w * 4).enumerate() {
                    let y = y_base + ry;
                    for x in 0..w {
                        let lin = match gbuf[y * w + x] {
                            Some(f) => {
                                let mut n = Vec3::from(f.n);
                                // A preview must not show black backfaces: the user is looking at
                                // an object from an arbitrary angle, and a normal that happens to
                                // point away is a modelling detail, not something to render.
                                let vdir = view_ray(x, y, half, half_fov, right, up, fwd);
                                if n.dot(vdir) > 0.0 {
                                    n = -n;
                                }
                                // The procedural, in world space at the size it will really be —
                                // same call the material ball and the path tracer make.
                                let (albedo, rough) = match &f.look.proc {
                                    Some(def) => {
                                        // The relief costs three extra fBm evaluations — four
                                        // times the whole rest of the sample. The draft pass goes
                                        // without it; the settled one has it. Same pattern either
                                        // way, so the draft is never a different material.
                                        let s = if relief {
                                            crate::proc_tex::sample(def, Vec3::from(f.p), n, f.look.roughness)
                                        } else {
                                            crate::proc_tex::sample_flat(def, Vec3::from(f.p), n, f.look.roughness)
                                        };
                                        n = s.normal;
                                        (s.albedo, s.roughness)
                                    }
                                    None => (f.look.albedo, f.look.roughness),
                                };
                                let mut c = crate::matball::shade_point(
                                    albedo, rough, f.look.metallic, 1.5,
                                    n, -vdir, sun_dir, sun_col, sky, sh, 0.55,
                                );
                                // TRANSPARENCY, as a straight blend with the BACKDROP — not with
                                // whatever the mesh has behind it. For a glazed door panel those
                                // are the same thing; for glass in front of another part they are
                                // not, and this shows the backdrop through it. The raster viewport
                                // makes the same approximation for a glass pane.
                                if f.look.opacity < 0.999 {
                                    let b = backdrop(x, y, w, shadow);
                                    let k = f.look.opacity;
                                    for i in 0..3 {
                                        c[i] = c[i] * k + b[i] * (1.0 - k);
                                    }
                                }
                                c
                            }
                            None => backdrop(x, y, w, shadow),
                        };
                        let [r, g, b] = crate::color::tonemap8(color, lin);
                        let o = x * 4;
                        row[o] = r;
                        row[o + 1] = g;
                        row[o + 2] = b;
                        row[o + 3] = 255;
                    }
                }
            });
        }
    });

    if ss == 1 {
        return hi_res;
    }
    // Box-downsample. The supersample is the only anti-aliasing here, and a door is nothing but
    // long straight edges, which is precisely what aliases worst.
    let mut out = vec![0u8; size * size * 4];
    let k = (ss * ss) as u32;
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0u32; 4];
            for sy2 in 0..ss {
                for sx in 0..ss {
                    let o = ((y * ss + sy2) * w + x * ss + sx) * 4;
                    for i in 0..4 {
                        acc[i] += hi_res[o + i] as u32;
                    }
                }
            }
            let o = (y * size + x) * 4;
            for i in 0..4 {
                out[o + i] = (acc[i] / k) as u8;
            }
        }
    }
    out
}

/// The ray through a pixel, in world space.
fn view_ray(x: usize, y: usize, half: f32, half_fov: f32, right: Vec3, up: Vec3, fwd: Vec3) -> Vec3 {
    let px = ((x as f32 + 0.5) - half) / half * half_fov;
    let py = (half - (y as f32 + 0.5)) / half * half_fov;
    (fwd + right * px + up * py).normalize()
}

/// World point → pixel, or `None` when it is behind the camera.
fn project(p: Vec3, to_view: &dyn Fn(Vec3) -> Vec3, half: f32, half_fov: f32) -> Option<(f32, f32)> {
    let v = to_view(p);
    if v.z <= 1e-4 {
        return None;
    }
    let inv = 1.0 / (v.z * half_fov);
    Some((half + v.x * inv * half, half - v.y * inv * half))
}

/// The studio backdrop: a soft vertical gradient with a contact shadow under the object. Linear
/// radiance, so it goes through the same display transform as the mesh.
fn backdrop(x: usize, y: usize, w: usize, shadow: Option<(f32, f32, f32, f32)>) -> [f32; 3] {
    let t = y as f32 / w as f32;
    // Bright above, falling off below — a seamless-cyc lighting setup, which is what makes a
    // product shot read as a product shot.
    let v = 0.44 - 0.30 * t * t;
    let mut c = [v * 0.99, v * 1.0, v * 1.04];
    if let Some((sx, sy, rx, ry)) = shadow {
        let (dx, dy) = ((x as f32 + 0.5 - sx) / rx, (y as f32 + 0.5 - sy) / ry);
        let d = dx * dx + dy * dy;
        if d < 1.0 {
            let k = 1.0 - 0.62 * (1.0 - d) * (1.0 - d);
            for ch in &mut c {
                *ch *= k;
            }
        }
    }
    c
}

/// An empty mesh still gets a picture — a blank frame says "nothing to show" far more clearly than
/// a missing widget does.
fn backdrop_only(size: usize, color: ColorPipeline) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let [r, g, b] = crate::color::tonemap8(color, backdrop(x, y, size, None));
            let o = (y * size + x) * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
            out[o + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube at the origin, as triangle soup with outward normals.
    fn cube(part: u32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
        let c = [
            [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5],
        ];
        let quads: [([usize; 4], [f32; 3]); 6] = [
            ([0, 3, 2, 1], [0.0, 0.0, -1.0]), ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]), ([3, 7, 6, 2], [0.0, 1.0, 0.0]),
            ([0, 4, 7, 3], [-1.0, 0.0, 0.0]), ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
        ];
        let (mut p, mut n, mut ids) = (Vec::new(), Vec::new(), Vec::new());
        for (q, nn) in quads {
            for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                for &vi in &tri {
                    p.push(c[vi]);
                    n.push(nn);
                }
                ids.push(part);
            }
        }
        (p, n, ids)
    }

    fn white(_: u32) -> PartLook {
        PartLook { albedo: [0.8, 0.8, 0.8], roughness: 0.4, ..Default::default() }
    }

    /// The mesh must actually appear, and appear in the MIDDLE — the camera fit is the part most
    /// likely to be silently wrong, and "renders a plausible gradient with nothing in it" is
    /// exactly what a broken fit looks like.
    #[test]
    fn the_mesh_lands_in_the_middle_of_the_frame() {
        let (p, n, ids) = cube(1);
        let size = 96;
        let img = render(&p, &n, &ids, &white, Orbit::default(), size, 1, ColorPipeline::default());
        assert_eq!(img.len(), size * size * 4);
        let centre = |x: usize, y: usize| img[(y * size + x) * 4] as i32;
        let mid = centre(size / 2, size / 2);
        let corner = centre(2, 2);
        assert!(mid > corner + 20, "object ({mid}) is brighter than the backdrop ({corner})");
        // …and it does not fill the frame, or the fit is too tight to be a preview.
        assert!(centre(1, size / 2) < mid, "the frame has margin on the left");
        assert!(centre(size - 2, size / 2) < mid, "…and on the right");
    }

    /// Orbiting must change the picture — a preview whose camera controls do nothing is worse than
    /// no controls, because the user concludes the geometry is wrong.
    #[test]
    fn orbiting_changes_what_is_drawn() {
        let (p, n, ids) = cube(1);
        let size = 64;
        let a = render(&p, &n, &ids, &white, Orbit::default(), size, 1, ColorPipeline::default());
        let b = render(
            &p, &n, &ids, &white,
            Orbit { yaw: 0.9, ..Orbit::default() },
            size, 1, ColorPipeline::default(),
        );
        let diff = a.iter().zip(&b).filter(|(x, y)| x.abs_diff(**y) > 4).count();
        assert!(diff > size * size / 20, "a 0.9 rad yaw moved {diff} subpixels");
    }

    /// Parts are shaded independently: a hardware id must not pick up the wood's albedo.
    #[test]
    fn each_part_gets_its_own_surface() {
        let (mut p, mut n, mut ids) = cube(1);
        // A second cube beside the first, tagged as a different part.
        let (p2, n2, _) = cube(2);
        for v in &p2 {
            p.push([v[0] + 1.2, v[1], v[2]]);
        }
        n.extend_from_slice(&n2);
        ids.extend(std::iter::repeat_n(2u32, 12));
        let size = 96;
        let look = |id: u32| PartLook {
            albedo: if id == 1 { [0.85, 0.15, 0.15] } else { [0.15, 0.15, 0.85] },
            roughness: 0.4,
            ..Default::default()
        };
        let img = render(&p, &n, &ids, &look, Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 }, size, 1, ColorPipeline::default());
        // Somewhere on the left half a pixel must be red-dominant, and on the right, blue-dominant.
        let mut redish = 0;
        let mut blueish = 0;
        for y in 0..size {
            for x in 0..size {
                let o = (y * size + x) * 4;
                let (r, b) = (img[o] as i32, img[o + 2] as i32);
                if x < size / 2 && r > b + 30 {
                    redish += 1;
                }
                if x > size / 2 && b > r + 30 {
                    blueish += 1;
                }
            }
        }
        assert!(redish > 40, "the part-1 cube is red ({redish} px)");
        assert!(blueish > 40, "the part-2 cube is blue ({blueish} px)");
    }

    /// A see-through material must actually let the backdrop through — the whole reason the door
    /// panel can be glazed. An "opacity" that renders identically to opaque is worse than none.
    #[test]
    fn glass_lets_the_backdrop_through() {
        let (p, n, ids) = cube(1);
        let size = 64;
        let orbit = Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 };
        let solid = |_: u32| PartLook { albedo: [0.05, 0.35, 0.08], roughness: 0.3, ..Default::default() };
        let glass = |_: u32| PartLook {
            albedo: [0.05, 0.35, 0.08],
            roughness: 0.3,
            opacity: 0.15,
            ..Default::default()
        };
        let a = render(&p, &n, &ids, &solid, orbit, size, 1, ColorPipeline::default());
        let b = render(&p, &n, &ids, &glass, orbit, size, 1, ColorPipeline::default());
        let o = ((size / 2) * size + size / 2) * 4;
        let bg = ColorPipeline::default();
        let want = crate::color::tonemap8(bg, backdrop(size / 2, size / 2, size, None));
        let d_solid = (a[o] as i32 - want[0] as i32).abs() + (a[o + 1] as i32 - want[1] as i32).abs();
        let d_glass = (b[o] as i32 - want[0] as i32).abs() + (b[o + 1] as i32 - want[1] as i32).abs();
        assert!(d_glass * 2 < d_solid, "glass ({d_glass}) sits far nearer the backdrop than the solid ({d_solid})");
        assert_ne!(a[o..o + 3], b[o..o + 3], "and the two are not the same pixel");
    }

    /// A procedural must paint a PATTERN, not its average colour. Flat brown where there should be
    /// grain is exactly the failure that makes a preview useless for choosing a timber.
    #[test]
    fn a_procedural_paints_its_grain() {
        let (p, n, ids) = cube(1);
        let size = 80;
        let orbit = Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 };
        let oak = crate::factory::ProcDef::oak();
        let flat = |_: u32| PartLook {
            albedo: crate::color::srgb_to_linear3(oak.avg_color()),
            roughness: 0.5,
            ..Default::default()
        };
        let grain = move |_: u32| PartLook {
            albedo: crate::color::srgb_to_linear3(oak.avg_color()),
            roughness: 0.5,
            proc: Some(oak),
            ..Default::default()
        };
        let spread = |img: &[u8]| {
            // Sample a band across the middle of the face and measure how much it varies.
            let row: Vec<i32> = (size / 4..size * 3 / 4)
                .map(|x| img[((size / 2) * size + x) * 4] as i32)
                .collect();
            let mean = row.iter().sum::<i32>() / row.len() as i32;
            row.iter().map(|v| (v - mean).abs()).sum::<i32>() / row.len() as i32
        };
        let a = render(&p, &n, &ids, &flat, orbit, size, 1, ColorPipeline::default());
        let b = render(&p, &n, &ids, &grain, orbit, size, 1, ColorPipeline::default());
        let (fa, fb) = (spread(&a), spread(&b));
        assert!(fa <= 1, "a flat colour on a flat face is flat (spread {fa})");
        assert!(fb > 4, "the grain varies across the face (spread {fb})");
    }

    /// A door-shaped box: 1.0 wide, 0.2 deep, 2.0 tall, centred on the origin.
    fn slab() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
        let (mut p, n, ids) = cube(1);
        for v in &mut p {
            v[0] *= 1.0;
            v[1] *= 0.2;
            v[2] *= 2.0;
        }
        (p, n, ids)
    }

    /// How much of the frame the object spans, as a fraction of width and height. Object pixels are
    /// found by colour, so the caller must shade it something the backdrop is not.
    fn spans(img: &[u8], size: usize) -> (f32, f32) {
        let (mut x0, mut x1, mut y0, mut y1) = (size, 0usize, size, 0usize);
        for y in 0..size {
            for x in 0..size {
                let o = (y * size + x) * 4;
                if img[o] as i32 > img[o + 2] as i32 + 30 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if x1 < x0 {
            return (0.0, 0.0);
        }
        ((x1 - x0) as f32 / size as f32, (y1 - y0) as f32 / size as f32)
    }

    /// THE regression behind the six fixed views: the camera must fit to the object's extent ACROSS
    /// the view, not to its bounding sphere. A door is 2.1 m tall and 0.15 m deep, so a
    /// sphere-fitted plan view puts a sliver in the middle of an empty frame — which reads as a
    /// broken preview, not as a correct plan.
    #[test]
    fn every_fixed_view_fills_its_frame() {
        let (p, n, ids) = slab();
        let size = 96;
        let red = |_: u32| PartLook { albedo: [0.75, 0.05, 0.05], roughness: 0.5, ..Default::default() };
        for v in View::ALL {
            let img = render(&p, &n, &ids, &red, v.orbit(1.0), size, 1, ColorPipeline::default());
            let (sx, sy) = spans(&img, size);
            assert!(sx > 0.02 && sy > 0.02, "{}: the object is visible at all ({sx}, {sy})", v.label());
            // Whichever way round it is, its LONG axis must nearly fill the frame.
            assert!(
                sx.max(sy) > 0.60,
                "{}: fills the frame — spans {sx:.2} × {sy:.2}", v.label()
            );
            // …and never overflow it, or the fit is too tight to see the whole thing.
            assert!(sx < 0.99 && sy < 0.99, "{}: still has margin ({sx:.2}, {sy:.2})", v.label());
        }
    }

    /// The six views must be six DIFFERENT pictures — a `View` that silently maps two entries to
    /// the same camera would look like a working picker that ignores half its buttons.
    #[test]
    fn the_six_views_are_all_distinct() {
        let (p, n, ids) = slab();
        let size = 48;
        // A cube is symmetric, so tag one end differently to tell front from back.
        let look = |id: u32| PartLook {
            albedo: if id == 1 { [0.75, 0.05, 0.05] } else { [0.05, 0.05, 0.75] },
            roughness: 0.5,
            ..Default::default()
        };
        let imgs: Vec<Vec<u8>> = View::ALL
            .iter()
            .map(|v| render(&p, &n, &ids, &look, v.orbit(1.0), size, 1, ColorPipeline::default()))
            .collect();
        for i in 0..imgs.len() {
            for j in i + 1..imgs.len() {
                let d = imgs[i].iter().zip(&imgs[j]).filter(|(a, b)| a.abs_diff(**b) > 4).count();
                assert!(
                    d > size * size / 10,
                    "{} and {} differ ({d} subpixels)",
                    View::ALL[i].label(),
                    View::ALL[j].label()
                );
            }
        }
    }

    /// Zoom must actually zoom, and must not push the camera through the object at the far end of
    /// its range — the near-plane cull would drop every triangle and hand back an empty frame.
    #[test]
    fn zooming_in_enlarges_the_object_without_losing_it() {
        let (p, n, ids) = slab();
        let size = 96;
        let red = |_: u32| PartLook { albedo: [0.75, 0.05, 0.05], roughness: 0.5, ..Default::default() };
        let at = |z: f32| {
            let img = render(&p, &n, &ids, &red, View::Front.orbit(z), size, 1, ColorPipeline::default());
            spans(&img, size).0
        };
        let (out, mid, in_) = (at(0.5), at(1.0), at(4.0));
        assert!(out < mid, "zooming out shrinks it ({out:.2} < {mid:.2})");
        assert!(in_ > mid, "zooming in grows it ({in_:.2} > {mid:.2})");
        // The extreme is still a picture of the door, not an empty frame.
        let img = render(&p, &n, &ids, &red, View::Front.orbit(8.0), size, 1, ColorPipeline::default());
        assert!(spans(&img, size).0 > 0.5, "fully zoomed in, the object is still there");
    }

    /// The draft pass drops the procedural RELIEF, never the pattern — so it is the same material
    /// seen more cheaply, not a different one. If the draft were flat brown, every material choice
    /// would look identical until the refine landed.
    #[test]
    fn the_draft_keeps_the_pattern_it_drops_the_relief() {
        let (p, n, ids) = slab();
        let size = 80;
        let oak = crate::factory::ProcDef::oak();
        let grain = move |_: u32| PartLook {
            albedo: crate::color::srgb_to_linear3(oak.avg_color()),
            roughness: 0.5,
            proc: Some(oak),
            ..Default::default()
        };
        let orbit = View::Front.orbit(1.0);
        let draft = render(&p, &n, &ids, &grain, orbit, size, 1, ColorPipeline::default());
        let spread = |img: &[u8]| {
            let row: Vec<i32> = (size / 4..size * 3 / 4)
                .map(|x| img[((size / 2) * size + x) * 4] as i32)
                .collect();
            let mean = row.iter().sum::<i32>() / row.len() as i32;
            row.iter().map(|v| (v - mean).abs()).sum::<i32>() / row.len() as i32
        };
        assert!(spread(&draft) > 4, "the draft still shows the grain (spread {})", spread(&draft));
    }

    /// Nothing to draw must still produce a well-formed opaque image, not a panic or a black hole.
    #[test]
    fn an_empty_mesh_renders_an_empty_frame() {
        let img = render(&[], &[], &[], &white, Orbit::default(), 32, 2, ColorPipeline::default());
        assert_eq!(img.len(), 32 * 32 * 4);
        assert!(img.chunks(4).all(|p| p[3] == 255), "fully opaque");
        assert!(img.chunks(4).any(|p| p[0] > 0), "and not black");
    }

    /// Write the real thing to a PNG so it can be LOOKED at. Numbers say the camera fit is
    /// centred; only a picture says the door reads as a door.
    #[test]
    #[ignore = "writes a PNG for eyeballing"]
    fn door_preview_probe() {
        // The library handle, welded on exactly as the app welds it — with the door's own lever
        // switched off, which is the pair of decisions this probe exists to look at.
        let lib = crate::handles::HandleLibrary::load(
            std::env::var("SIMLUX_HANDLES")
                .unwrap_or_else(|_| r"G:\blender dev\staircase\door handles_\assets\handles".into()),
        )
        .ok();
        let chosen = std::env::var("SIMLUX_HANDLE").unwrap_or_else(|_| "lever_rose_chrome".into());
        let handle = lib.as_ref().and_then(|l| l.get(&chosen).cloned());

        let inp = cad_solid::door::DoorInput {
            builtin_hardware: handle.is_none(),
            ..cad_solid::door::DoorInput::default()
        };
        let (_m, mut mesh) = cad_solid::door::build(&inp).unwrap();
        if let (Some(l), Some(h)) = (&lib, &handle) {
            let path = l.mesh_path(h);
            let bytes = std::fs::read(&path).unwrap();
            let (hm, hp) = crate::mesh_io::parse_fbx_pbr_at(&bytes, path.parent());
            let fit = crate::handles::DoorFit {
                door_width_mm: inp.door_width * 1000.0,
                door_height_mm: inp.door_height * 1000.0,
                leaf_thickness_mm: inp.door_thickness * 1000.0,
                handle_backset_mm: inp.handle_backset * 1000.0,
                handle_height_mm: inp.handle_height * 1000.0,
                hinge_side: inp.hinge_side,
            };
            crate::handles::weld_onto(
                &fit, &hm.positions, &hm.normals, &hp.part_ids,
                &mut mesh.positions, &mut mesh.normals, &mut mesh.face_ids,
            );
            println!("welded '{}' ({} tris)", h.id, hm.tri_count());
        } else {
            println!("no handle library — rendering the door's own lever");
        }
        // Materials per component, exactly as the Door panel sets them.
        use crate::door_mat::{DoorMaterial, DoorMaterials};
        let mats = DoorMaterials {
            leaf: DoorMaterial::Walnut,
            panel: DoorMaterial::ClearGlass,
            frame: DoorMaterial::PaintedWhite,
            architrave: DoorMaterial::PaintedWhite,
        };
        let metal = PartLook {
            albedo: [0.52, 0.51, 0.49],
            roughness: 0.28,
            metallic: 1.0,
            ..Default::default()
        };
        let look = |id: u32| mats.for_part(id).map(|m| m.look()).unwrap_or(metal);
        let dir = std::path::PathBuf::from(
            std::env::var("SIMLUX_RENDER_OUT").unwrap_or_else(|_| ".".into()),
        );
        // Both passes of every fixed view: the draft the user sees the instant they click, and the
        // supersampled one that replaces it a frame later. The two timings ARE the interactivity
        // argument — if the draft is not several times cheaper, the progressive split buys nothing.
        for v in View::ALL {
            for (label, size, ss) in [("draft", 230usize, 1usize), ("final", 460, 2)] {
                let t0 = std::time::Instant::now();
                let px = render(
                    &mesh.positions, &mesh.normals, &mesh.face_ids, &look, v.orbit(1.0), size, ss,
                    ColorPipeline::default(),
                );
                println!("{:>6} {label}: {} tris in {:?}", v.label(), mesh.tri_count(), t0.elapsed());
                if ss == 2 {
                    let name = format!("door_view_{}", v.label().to_ascii_lowercase());
                    crate::render_probe::write_png(&dir.join(format!("{name}.png")), size, size, &px).unwrap();
                }
            }
        }
    }

    /// Supersampling must change only the EDGES, never the overall image — if it shifted the
    /// camera or the fit, the two would disagree in the middle too.
    #[test]
    fn supersampling_only_touches_the_edges() {
        let (p, n, ids) = cube(1);
        let size = 64;
        let a = render(&p, &n, &ids, &white, Orbit::default(), size, 1, ColorPipeline::default());
        let b = render(&p, &n, &ids, &white, Orbit::default(), size, 2, ColorPipeline::default());
        let far: Vec<usize> = (0..size * size)
            .filter(|i| {
                let (x, y) = (i % size, i / size);
                // Interior pixels only — 3 px in from wherever the two images already differ.
                (12..size - 12).contains(&x) && (12..size - 12).contains(&y)
            })
            .collect();
        let bad = far.iter().filter(|&&i| a[i * 4].abs_diff(b[i * 4]) > 24).count();
        assert!(bad * 20 < far.len(), "{bad}/{} interior pixels moved", far.len());
    }
}
