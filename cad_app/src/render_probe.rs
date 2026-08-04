//! Render the villa to a PNG with no window — so a change to the LOOK can be judged by looking.
//!
//! Every visual claim about this renderer so far has been inference from numbers: a sun colour, an
//! ambient magnitude, a texture's average. That is how a scene ends up shipping with the daylight
//! switched off and nobody noticing. This module closes the loop: it loads the exported scene,
//! stands it up in a `FactoryState` exactly as the app's import does, path-traces it, and writes a
//! PNG. No GL context, no event loop, no user.
//!
//! Run:
//!   SIMLUX_RENDER_OUT=<dir> cargo test -p cad_app --release villa_render_probe -- --ignored --nocapture
//!
//! Optional: `SIMLUX_RENDER_HOUR` (default 16.5), `SIMLUX_RENDER_PASSES` (default 48),
//! `SIMLUX_RENDER_SIZE` (default 800x600), `SIMLUX_RENDER_SCENE` (default the villa GLB).

#![cfg(test)]

use crate::factory::{FactoryState, SunEnv};

/// The scene the renders are judged against.
const VILLA: &str = r"G:\blender dev\staircase\villa scene\villa_scene.glb";

fn env_str(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}
fn env_f32(k: &str, d: f32) -> f32 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn env_u32(k: &str, d: u32) -> u32 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

/// Load a `.glb` into a `FactoryState` the way `App::import_scene_file` does: one asset at 1:1
/// world scale at the origin, every material registered as a texture, and each face-group bound to
/// its part's texture. Kept deliberately parallel to the app path — if this diverges, the probe
/// stops being evidence about the app.
pub fn load_scene(path: &str) -> Option<FactoryState> {
    let bytes = std::fs::read(path).ok()?;
    let base = std::path::Path::new(path).parent();
    let (mesh, pbr) = crate::mesh_io::parse_gltf_ex(&bytes, base);
    if mesh.tri_count() == 0 {
        return None;
    }
    let mut st = FactoryState::default();
    let idx = st.add_furniture_asset("scene".into(), mesh);
    let ntri = st.furniture_lib.get(idx).map(|a| a.positions.len() / 3).unwrap_or(0);
    let k = st.furniture_lib.get(idx).map(|a| a.import_scale).unwrap_or(1.0);
    if let Some(a) = st.furniture_lib.get_mut(idx) {
        if !pbr.uvs.is_empty() {
            a.uvs = pbr.uvs.clone();
        }
        if pbr.part_ids.len() == ntri {
            a.part_ids = pbr.part_ids.clone();
        }
    }
    let globals: Vec<usize> = pbr
        .textures
        .iter()
        .enumerate()
        .map(|(i, (w, h, rgba))| st.add_texture(format!("mat{i}"), *w, *h, rgba.clone()))
        .collect();
    let per_part: Vec<Option<usize>> =
        pbr.part_texture.iter().map(|s| s.and_then(|s| globals.get(s).copied())).collect();
    // The material's surface properties, exactly as the app import applies them.
    for (part, g) in per_part.iter().enumerate() {
        let Some(g) = g else { continue };
        if let Some(t) = st.textures.get_mut(*g) {
            if let Some(r) = pbr.part_rough.get(part) {
                t.roughness = r.clamp(0.0, 1.0);
            }
            if let Some(m) = pbr.part_metal.get(part) {
                t.metallic = m.clamp(0.0, 1.0);
            }
        }
    }

    st.place_furniture(idx, glam::Vec3::ZERO);
    let fi = st.sel_furn_primary()?;
    if let Some(inst) = st.furniture.get_mut(fi) {
        inst.scale = if k > 1e-6 { 1.0 / k } else { 1.0 };
        inst.pos = [0.0; 3];
        inst.rot = [0.0; 3];
    }
    // Bind each face group to its part's texture, exactly as the import does.
    if let Some(a) = st.furniture_lib.get(idx) {
        let g = a.group_geom();
        let mut fg = std::collections::HashMap::new();
        for t in 0..ntri {
            let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
            if let Some(Some(tex)) = per_part.get(part) {
                fg.insert(g.face[t], *tex);
            }
        }
        if let Some(inst) = st.furniture.get_mut(fi) {
            inst.surface_texture = fg;
        }
    }
    st.fit_all();
    Some(st)
}

/// Daylight over Goa, matching the reference render's ~34° sun.
pub fn goa_sun(hour: f32) -> SunEnv {
    SunEnv {
        enabled: true,
        lat_deg: 15.3,
        lon_deg: 74.0,
        utc_offset: 5.5,
        month: 1,
        day: 15,
        hour,
        shadows: true,
        sky_backdrop: true,
        ..Default::default()
    }
}

/// Encode linear-free RGBA bytes to a PNG on disk.
pub fn write_png(path: &std::path::Path, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    let f = std::fs::File::create(path)?;
    let enc = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(f));
    image::ImageEncoder::write_image(enc, rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8)
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the startup autoload actually COSTS. The villa is a 134 MB glTF at ~1.9 M triangles and
    /// it used to be imported on the FIRST FRAME, before the window had drawn anything. This prints
    /// the number, so "the app is slow to open" is a measurement rather than a hunch.
    #[test]
    #[ignore = "reads the 134 MB villa GLB"]
    fn villa_import_cost() {
        let path = env_str("SIMLUX_RENDER_SCENE", VILLA);
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let t0 = std::time::Instant::now();
        let Some(st) = load_scene(&path) else {
            println!("no scene at {path}");
            return;
        };
        let tris: usize = st.furniture_lib.iter().map(|a| a.positions.len() / 3).sum();
        println!(
            "villa import: {:.1} MB → {tris} triangles in {:?}",
            bytes as f64 / 1e6,
            t0.elapsed()
        );
    }

    /// Path-trace the villa and write a PNG. This is the feedback loop: without it, a change to the
    /// sun, the ambient or the texture binding can only be argued about.
    #[test]
    #[ignore = "renders a real scene; needs the villa GLB"]
    fn villa_render_probe() {
        let path = env_str("SIMLUX_RENDER_SCENE", VILLA);
        let out = std::path::PathBuf::from(env_str("SIMLUX_RENDER_OUT", "."));
        let Some(mut st) = load_scene(&path) else {
            println!("no scene at {path} — set SIMLUX_RENDER_SCENE");
            return;
        };
        st.sun = goa_sun(env_f32("SIMLUX_RENDER_HOUR", 16.5));
        // Blender's reference grades at AgX with a "Punchy" look and -0.2 EV; ours is plain AgX at
        // 0. `SIMLUX_RENDER_EV` / `SIMLUX_RENDER_LOOK` make that difference measurable instead of
        // arguable.
        st.color.exposure = env_f32("SIMLUX_RENDER_EV", 0.0);
        st.color.look = env_f32("SIMLUX_RENDER_LOOK", 0.0);
        st.color.punchy = env_f32("SIMLUX_RENDER_PUNCHY", 0.0);

        let size = env_str("SIMLUX_RENDER_SIZE", "800x600");
        let (w, h) = size.split_once('x').unwrap_or(("800", "600"));
        let (w, h) = (w.parse().unwrap_or(800usize), h.parse().unwrap_or(600usize));
        let passes = env_u32("SIMLUX_RENDER_PASSES", 48);

        let t0 = std::time::Instant::now();
        let tris = st.export_render_tris();
        let (pool, tex_of) = st.export_texture_table();
        let images = pool.len();
        let off = -st.sun.north_offset_deg.to_radians();
        let scene =
            crate::pathtrace::Scene::build_full(&tris, &st.export_proc_table(), off, pool, &tex_of);
        println!(
            "scene: {} triangles, {images} images, built in {:?}",
            scene.tri_count(),
            t0.elapsed()
        );

        let (_en, dir, sun_col, env_render) = st.sun.resolve_env();
        let (c, s) = (off.cos(), off.sin());
        let sun_dir = glam::Vec3::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c, dir.z);
        // Is the analytic dome actually driving the background, or has it fallen back to the flat
        // two-colour hemisphere? A uniform grey sky in the render is the symptom of the latter.
        match &env_render.sky {
            Some(s) => println!(
                "dome: valid={} scale={:.4} zenith={:?} horizon={:?} ground={:?}",
                s.valid,
                s.scale,
                s.radiance(glam::Vec3::Z),
                s.radiance(glam::Vec3::new(1.0, 0.0, 0.02).normalize()),
                s.ground,
            ),
            None => println!("dome: NONE — background is the flat two-colour hemisphere"),
        }
        let sky = crate::pathtrace::Sky::from_env(sun_dir, sun_col, &env_render);
        // `fit_all` frames the whole 86x74 m site from 120 m up, so most of the frame is BELOW the
        // horizon and the "sky" is the dome's grey ground half. Any judgement about the look needs
        // a view like the one the reference was rendered from: standing on the lawn.
        let (mut eye, mut target) = st.export_camera();
        let parse3 = |s: String| -> Option<[f32; 3]> {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        if let Some(e) = std::env::var("SIMLUX_RENDER_EYE").ok().and_then(parse3) {
            eye = e;
        }
        if let Some(t) = std::env::var("SIMLUX_RENDER_TARGET").ok().and_then(parse3) {
            target = t;
        }
        let cam = crate::pathtrace::Camera {
            eye: glam::Vec3::from(eye),
            target: glam::Vec3::from(target),
            fov_deg: 45.0,
        };
        let settings = crate::pathtrace::Settings {
            w,
            h,
            passes,
            max_depth: 6,
            color: st.color,
        };
        println!(
            "sun: dir {:?} colour {:?} | camera {:?} -> {:?}",
            (sun_dir.x, sun_dir.y, sun_dir.z),
            sun_col,
            eye,
            target
        );

        let t1 = std::time::Instant::now();
        let job = crate::pathtrace::RenderJob::start(
            scene,
            cam,
            sky,
            settings,
            crate::pathtrace::Device::Cpu,
        );
        while job.passes_done() < passes {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let Some((rw, rh, rgba)) = job.snapshot_rgba_opt(true) else {
            panic!("no image produced");
        };
        println!("{passes} passes in {:?}", t1.elapsed());

        // The headline numbers a look can be argued about afterwards: how bright is it, and is it
        // grey? A washed-out render is a high mean with a low saturation.
        let n = (rgba.len() / 4).max(1);
        let (mut sum, mut sat) = ([0f64; 3], 0f64);
        for px in rgba.chunks_exact(4) {
            for k in 0..3 {
                sum[k] += px[k] as f64;
            }
            let mx = px[0].max(px[1]).max(px[2]) as f64;
            let mn = px[0].min(px[1]).min(px[2]) as f64;
            sat += if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        }
        println!(
            "image {rw}x{rh}  mean rgb ({:.0},{:.0},{:.0})  mean saturation {:.3}",
            sum[0] / n as f64,
            sum[1] / n as f64,
            sum[2] / n as f64,
            sat / n as f64
        );

        let file = out.join("villa_render.png");
        write_png(&file, rw, rh, &rgba).expect("write png");
        println!("wrote {}", file.display());
    }
}
