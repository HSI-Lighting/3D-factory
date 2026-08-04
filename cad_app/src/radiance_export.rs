//! Export the 3D Factory scene for an **offline Radiance render** (https://github.com/LBNL-ETA/Radiance).
//!
//! Radiance is the physically-based renderer whose sun model we already ported ([`crate::solar`]),
//! so the `gensky` sun this emits lands in exactly the place the viewport shows. The bundle is:
//!
//! - `scene.rad` — every triangle as a Radiance `polygon`, grouped under de-duplicated materials
//!   (opaque → `plastic`, see-through → `glass`) derived from each surface's colour/roughness.
//! - `sky.rad` — an inline `!gensky` sun+sky for the building's latitude/longitude, date and time,
//!   plus the standard sky/ground glow.
//! - `render.bat` / `render.sh` — the `oconv` → `rpict` → `pfilt` pipeline, aimed with the current
//!   camera.
//! - `README.txt` — how to run it.
//!
//! Pure string builders (no I/O, no app types) so they unit-test cleanly; the app gathers the
//! geometry and writes the files.

/// One world-space triangle to export, with its resolved surface appearance. Shared by the Radiance
/// export (which reads rgb/roughness/opacity) and the in-app path tracer ([`crate::pathtrace`],
/// which additionally reads the Principled fields below).
#[derive(Clone, Copy, Debug)]
pub struct ExportTri {
    pub verts: [[f32; 3]; 3],
    /// Linear RGB 0..1.
    pub rgb: [f32; 3],
    /// 0 = glossy, 1 = matte (mapped to Radiance `plastic` roughness).
    pub roughness: f32,
    /// < 1 → exported as `glass` (transmitting) instead of `plastic`.
    pub opacity: f32,
    /// Principled extras for the path tracer (Radiance export ignores them).
    pub metallic: f32,
    pub ior: f32,
    /// Emission radiance (colour × strength), linear RGB.
    pub emission: [f32; 3],
    /// CLEARCOAT strength and roughness — a thin varnish with its own smooth specular lobe over
    /// whatever the base is doing. 0 = bare. Radiance export ignores them.
    pub clearcoat: f32,
    pub clearcoat_rough: f32,
    /// SHEEN strength and the colour of the fuzz — the grazing rim fabric gets. 0 = none.
    pub sheen: f32,
    pub sheen_tint: [f32; 3],
    /// Which app-side material this surface uses, so the path tracer can look up its PROCEDURAL
    /// definition and evaluate the pattern per hit. Without it the tracer only ever saw `rgb` — the
    /// ramp's midpoint — so an oak cabinet rendered as one flat brown while the viewport showed the
    /// grain. An INDEX rather than the definition itself: there is one `ExportTri` per triangle and
    /// scenes here reach millions, so a `ProcDef` per triangle is not free.
    pub material: Option<u16>,
    /// Per-vertex UVs, when the surface HAS a UV layer. The path tracer needs these to sample a
    /// material's IMAGE at the hit point; without them it could only ever use the image's average
    /// colour, which is why an offline villa render had flat roofs and flat lawn where the
    /// viewport showed tiles and grass. Radiance export ignores them.
    pub uv: [[f32; 2]; 3],
    /// False when the surface carries no UV layer — the tracer then projects from world space,
    /// exactly as the viewport shader does. Distinct from an all-zero `uv`, which is a legitimate
    /// if degenerate mapping.
    pub has_uv: bool,
}

impl ExportTri {
    /// A plain diffuse surface (the common case; Principled extras at their defaults).
    pub fn plain(verts: [[f32; 3]; 3], rgb: [f32; 3], roughness: f32, opacity: f32) -> Self {
        Self {
            verts, rgb, roughness, opacity,
            metallic: 0.0, ior: 1.5, emission: [0.0; 3], material: None,
            clearcoat: 0.0, clearcoat_rough: 0.1, sheen: 0.0, sheen_tint: [1.0; 3],
            uv: [[0.0; 2]; 3], has_uv: false,
        }
    }
}

fn q(x: f32) -> i32 {
    (x.clamp(0.0, 1.0) * 255.0).round() as i32
}

/// A de-dup key so identical-looking surfaces share one material definition.
fn mat_key(t: &ExportTri) -> (i32, i32, i32, i32, bool) {
    let glass = t.opacity < 0.99;
    (q(t.rgb[0]), q(t.rgb[1]), q(t.rgb[2]), (t.roughness.clamp(0.0, 1.0) * 20.0).round() as i32, glass)
}

/// The material primitive block for a bucket.
fn material_block(name: &str, t: &ExportTri) -> String {
    if t.opacity < 0.99 {
        // Radiance glass: transmissivity ≈ colour (a light tint through it).
        format!(
            "void glass {name}\n0\n0\n3 {:.4} {:.4} {:.4}\n\n",
            t.rgb[0].max(0.02), t.rgb[1].max(0.02), t.rgb[2].max(0.02)
        )
    } else {
        // Radiance plastic: 5 = R G B specular roughness. Dielectric spec 0.05; our 0..1 roughness
        // maps into Radiance's 0..~0.2 useful range.
        let rough = (t.roughness.clamp(0.0, 1.0) * 0.2).max(0.0);
        format!(
            "void plastic {name}\n0\n0\n5 {:.4} {:.4} {:.4} 0.05 {:.4}\n\n",
            t.rgb[0], t.rgb[1], t.rgb[2], rough
        )
    }
}

/// Build `scene.rad`: material definitions followed by one `polygon` per triangle.
pub fn scene_rad(tris: &[ExportTri]) -> String {
    use std::collections::HashMap;
    let mut order: Vec<(i32, i32, i32, i32, bool)> = Vec::new();
    let mut names: HashMap<(i32, i32, i32, i32, bool), (String, ExportTri)> = HashMap::new();
    for t in tris {
        let k = mat_key(t);
        if !names.contains_key(&k) {
            let n = format!("mat_{}", order.len());
            names.insert(k, (n, *t));
            order.push(k);
        }
    }
    let mut out = String::new();
    out.push_str("# 3D Factory → Radiance scene. Z up, X east, Y north (metres).\n\n");
    for k in &order {
        let (name, sample) = &names[k];
        out.push_str(&material_block(name, sample));
    }
    for (i, t) in tris.iter().enumerate() {
        let (name, _) = &names[&mat_key(t)];
        out.push_str(&format!(
            "{name} polygon face_{i}\n0\n0\n9\n\t{:.5} {:.5} {:.5}\n\t{:.5} {:.5} {:.5}\n\t{:.5} {:.5} {:.5}\n\n",
            t.verts[0][0], t.verts[0][1], t.verts[0][2],
            t.verts[1][0], t.verts[1][1], t.verts[1][2],
            t.verts[2][0], t.verts[2][1], t.verts[2][2],
        ));
    }
    out
}

/// Build `sky.rad`: an inline `!gensky` sun+sky matched to the location/date/time, plus glow.
/// `lon_deg` is +east and `utc_offset` +east — converted to gensky's west-positive `-o`/`-m`.
pub fn sky_rad(lat_deg: f32, lon_deg: f32, utc_offset: f32, month: u32, day: u32, hour: f32) -> String {
    let lon_west = -lon_deg;
    let meridian_west = -15.0 * utc_offset;
    format!(
        "# gensky sun + sky, matched to the 3D Factory ☀ Sun settings.\n\
         !gensky {month} {day} {hour:.3} +s -a {lat_deg:.4} -o {lon_west:.4} -m {meridian_west:.4}\n\n\
         skyfunc glow sky_glow\n0\n0\n4 0.90 0.90 1.05 0\n\n\
         sky_glow source sky_dome\n0\n0\n4 0 0 1 180\n\n\
         skyfunc glow ground_glow\n0\n0\n4 0.60 0.57 0.52 0\n\n\
         ground_glow source ground\n0\n0\n4 0 0 -1 180\n"
    )
}

/// The `rpict` view flags for the current camera — the SAME framing as the in-app path tracer:
/// 45° VERTICAL field of view, horizontal widened to the image aspect (`2·atan(tan(22.5°)·w/h)`).
/// The old hardcoded `-vh 45 -vv 32` was a much narrower cone than the viewport, which is why a
/// Radiance render showed only a portion of the scene.
fn view_flags(eye: [f32; 3], target: [f32; 3], w: u32, h: u32) -> String {
    let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-4);
    let vd = [d[0] / len, d[1] / len, d[2] / len];
    let vv = 45.0f32;
    let aspect = w as f32 / h.max(1) as f32;
    let vh = 2.0 * ((vv.to_radians() * 0.5).tan() * aspect).atan().to_degrees();
    format!(
        "-vtv -vp {:.4} {:.4} {:.4} -vd {:.4} {:.4} {:.4} -vu 0 0 1 -vh {vh:.2} -vv {vv:.2}",
        eye[0], eye[1], eye[2], vd[0], vd[1], vd[2]
    )
}

/// Build a Windows `render.bat` rendering at `w`×`h`.
pub fn render_bat(eye: [f32; 3], target: [f32; 3], w: u32, h: u32) -> String {
    format!(
        "@echo off\r\n\
         REM Requires Radiance on PATH (https://github.com/LBNL-ETA/Radiance).\r\n\
         oconv sky.rad scene.rad > scene.oct\r\n\
         rpict {view} -ab 3 -ad 1024 -as 512 -aa 0.15 -x {w} -y {h} scene.oct > render.hdr\r\n\
         pfilt render.hdr > render_small.hdr\r\n\
         ra_bmp render_small.hdr render.bmp\r\n\
         echo Wrote render.bmp\r\n",
        view = view_flags(eye, target, w, h)
    )
}

/// Build a POSIX `render.sh` rendering at `w`×`h`.
pub fn render_sh(eye: [f32; 3], target: [f32; 3], w: u32, h: u32) -> String {
    format!(
        "#!/bin/sh\n\
         # Requires Radiance on PATH (https://github.com/LBNL-ETA/Radiance).\n\
         set -e\n\
         oconv sky.rad scene.rad > scene.oct\n\
         rpict {view} -ab 3 -ad 1024 -as 512 -aa 0.15 -x {w} -y {h} scene.oct > render.hdr\n\
         pfilt render.hdr > render_small.hdr\n\
         ra_bmp render_small.hdr render.bmp\n\
         echo 'Wrote render.bmp'\n",
        view = view_flags(eye, target, w, h)
    )
}

/// The README.
pub fn readme() -> String {
    "3D Factory — Radiance export\n\
     ============================\n\n\
     Files:\n\
       scene.rad   geometry + materials (Z up, X east, Y north, metres)\n\
       sky.rad     gensky sun + sky, matched to the ☀ Sun date/time/location\n\
       render.bat  Windows render pipeline (render.sh for macOS/Linux)\n\n\
     Install Radiance (https://github.com/LBNL-ETA/Radiance), put its bin/ on PATH, then:\n\
       render.bat        (or:  sh render.sh)\n\n\
     It builds an octree with oconv, ray-traces with rpict, and writes render.bmp.\n\
     Increase -ab / -ad / -x / -y in the script for a cleaner, larger image.\n\n\
     The sun is placed by Radiance's own gensky, so it matches the viewport's ☀ Sun exactly.\n\
     Note: materials are solid colours + roughness (image textures are not baked into the\n\
     export yet); glass uses the surface's opacity.\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(rgb: [f32; 3], rough: f32, op: f32) -> ExportTri {
        ExportTri::plain([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], rgb, rough, op)
    }

    #[test]
    fn scene_dedups_materials_and_emits_polygons() {
        let tris = vec![
            tri([0.8, 0.2, 0.2], 0.5, 1.0),
            tri([0.8, 0.2, 0.2], 0.5, 1.0), // identical → shares the material
            tri([0.2, 0.2, 0.8], 0.5, 1.0),
        ];
        let s = scene_rad(&tris);
        assert_eq!(s.matches("void plastic").count(), 2, "two distinct materials");
        assert_eq!(s.matches("polygon face_").count(), 3, "three polygons");
    }

    #[test]
    fn transparent_surface_becomes_glass() {
        let s = scene_rad(&[tri([0.6, 0.7, 0.8], 0.5, 0.4)]);
        assert!(s.contains("void glass"), "opacity<1 → glass");
        assert!(!s.contains("void plastic"));
    }

    /// The gensky line converts +east lon/UTC to west-positive and carries the date/time.
    #[test]
    fn sky_matches_location_and_time() {
        let s = sky_rad(51.5, -0.13, 1.0, 6, 21, 13.0);
        assert!(s.contains("!gensky 6 21 13.000 +s"), "date/time: {s}");
        assert!(s.contains("-a 51.5000"));
        assert!(s.contains("-o 0.1300"), "lon +east → west-positive");
        assert!(s.contains("-m -15.0000"), "UTC+1 → meridian −15° west");
        assert!(s.contains("source sky_dome") && s.contains("source ground"));
    }

    #[test]
    fn render_script_aims_the_camera() {
        let b = render_bat([5.0, -5.0, 2.0], [0.0, 0.0, 1.0], 1280, 960);
        assert!(b.contains("oconv sky.rad scene.rad"));
        assert!(b.contains("-vp 5.0000 -5.0000 2.0000"));
        assert!(b.contains("rpict"));
        assert!(b.contains("-x 1280 -y 960"), "chosen resolution reaches rpict");
        // Same framing as the path tracer: 45° vertical; horizontal widened by the 4:3 aspect
        // (2·atan(tan 22.5°·4/3) ≈ 57.8°).
        assert!(b.contains("-vv 45.00"), "45° vertical fov: {b}");
        assert!(b.contains("-vh 57.7") || b.contains("-vh 57.8"), "aspect-matched horizontal fov: {b}");
        // pfilt must NOT downscale — the picked size is the delivered size.
        assert!(!b.contains("-x /2"), "no half-size pfilt");
    }
}
