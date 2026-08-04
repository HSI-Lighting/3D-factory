//! Loading a **PBR texture set** — a folder of images that together describe one material.
//!
//! Every free texture library (ambientCG, Poly Haven, Texture Haven, textures.com, Quixel exports…)
//! ships the same thing: five or six greyscale/colour maps in one folder, distinguished only by a
//! suffix on the filename. `Bricks075A_2K_Color.png`, `Bricks075A_2K_NormalGL.png`, and so on.
//!
//! Until now the app could only take ONE image, pasted from the clipboard, and used it as albedo.
//! That is the reason imported textures looked like wallpaper: a brick wall with no normal map is
//! flat, and with no roughness map it is uniformly shiny or uniformly dull, so it reads as a
//! photograph of bricks rather than as bricks.
//!
//! The classification is deliberately a **pure function over the filename** ([`classify`]) so the
//! naming knowledge — which is all convention and no logic — can be tested against the real names
//! those libraries actually ship, with no filesystem and no image decoding involved.

/// Which slot a file in a texture-set folder belongs in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MapKind {
    /// Base colour / albedo / diffuse. The only sRGB-encoded map in the set.
    BaseColor,
    /// Tangent-space normals. **OpenGL convention** (+Y up); a DirectX map needs its green channel
    /// inverted, which [`classify`] flags rather than silently getting wrong.
    Normal,
    /// Inverted green — DirectX convention.
    NormalDx,
    Roughness,
    /// Glossiness — the inverse of roughness. Stored separately so the loader can invert it.
    Gloss,
    Metallic,
    AmbientOcclusion,
    /// Height / displacement. Not used for geometry here; kept so it is recognised rather than
    /// mistaken for a roughness map (both are greyscale, and "Disp" vs "Rough" is the only tell).
    Height,
    /// Opacity / alpha mask.
    Opacity,
    /// **ARM** — occlusion, roughness and metalness packed into one image's R, G and B channels.
    /// Poly Haven ships every material this way. It is not a map in its own right: the loader
    /// splits it into the three it stands for, so `Arm` never reaches the renderer.
    Arm,
}

impl MapKind {
    pub fn label(self) -> &'static str {
        match self {
            MapKind::BaseColor => "Base colour",
            MapKind::Normal => "Normal (GL)",
            MapKind::NormalDx => "Normal (DirectX)",
            MapKind::Roughness => "Roughness",
            MapKind::Gloss => "Glossiness",
            MapKind::Metallic => "Metallic",
            MapKind::AmbientOcclusion => "Ambient occlusion",
            MapKind::Height => "Height",
            MapKind::Opacity => "Opacity",
            MapKind::Arm => "Packed AO/rough/metal",
        }
    }

    /// True for the one map that carries **colour** rather than numbers, and so is the only one
    /// that must be uploaded as sRGB. Getting this backwards on a normal map bends every normal
    /// toward the surface; getting it backwards on albedo washes the whole material out.
    pub fn is_srgb(self) -> bool {
        self == MapKind::BaseColor
    }
}

/// Which slot a filename belongs in, or `None` if it is not a recognised map (a readme, a preview
/// render, a `.usdc`…).
///
/// Order matters and is the whole subtlety here:
///
/// - `NormalDX` must be tested before `Normal`, or every DirectX map is silently loaded as GL and
///   every surface's relief is lit from the wrong side.
/// - `AmbientOcclusion` before anything containing "occlusion", and `_ao` only as a whole token —
///   otherwise "Road**ao**..." or a file called `chao.png` matches.
/// - `Roughness` before `Gloss`, since some sets ship `Roughness` and `Glossiness` in one folder.
/// - `Height`/`Displacement` before the generic greyscale names, so relief is not read as gloss.
pub fn classify(filename: &str) -> Option<MapKind> {
    let stem = filename.rsplit('/').next()?.rsplit('\\').next()?;
    let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
    let s = stem.to_ascii_lowercase();
    // Split on the separators these libraries use, so a suffix is matched as a WHOLE token.
    let toks: Vec<&str> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let has = |t: &str| toks.iter().any(|x| *x == t);
    let ends = |t: &str| s.ends_with(t);

    // DirectX normals first — "normaldx" also contains "normal".
    if has("normaldx") || has("nrmdx") || ends("normal_dx") || has("dx") && (has("normal") || has("nrm")) {
        return Some(MapKind::NormalDx);
    }
    if has("normalgl") || has("normal") || has("nrm") || has("norm") || has("nor") || has("normalmap") {
        return Some(MapKind::Normal);
    }
    // Packed AO/rough/metal — before the individual names, since "arm" is its own token and the
    // file contains all three.
    if has("arm") || has("orm") || has("occlusionroughnessmetallic") {
        return Some(MapKind::Arm);
    }
    if has("ao") || has("occlusion") || has("ambientocclusion") || has("occ") {
        return Some(MapKind::AmbientOcclusion);
    }
    if has("roughness") || has("rough") || has("rgh") {
        return Some(MapKind::Roughness);
    }
    if has("glossiness") || has("gloss") || has("gls") {
        return Some(MapKind::Gloss);
    }
    if has("metalness") || has("metallic") || has("metal") || has("mtl") {
        return Some(MapKind::Metallic);
    }
    if has("displacement") || has("height") || has("disp") || has("bump") {
        return Some(MapKind::Height);
    }
    if has("opacity") || has("alpha") || has("mask") {
        return Some(MapKind::Opacity);
    }
    // Base colour last: "color" is the least specific token and appears in names like
    // "…_Color.png" but never as a qualifier on another map.
    if has("basecolor") || has("basecolour") || has("albedo") || has("diffuse") || has("color") || has("colour") || has("col") || has("diff") || has("base") {
        return Some(MapKind::BaseColor);
    }
    None
}

/// A file with an image extension this build can decode.
pub fn is_image_file(filename: &str) -> bool {
    let e = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(e.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff")
}

/// One decoded map on its way into the texture list.
pub struct LoadedMap {
    pub kind: MapKind,
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Read every recognised map in a folder, decoded to RGBA8.
///
/// When a folder ships both `Roughness` and `Glossiness` the roughness one wins; when it ships only
/// gloss, it is **inverted** into roughness here rather than at the shader, so downstream there is
/// exactly one convention. Likewise a DirectX normal map has its green channel flipped on the way
/// in, so `MapKind::NormalDx` never escapes this function.
///
/// Highest resolution wins if the same slot appears twice (many sets ship 1K and 2K side by side).
pub fn load_folder(dir: &std::path::Path) -> Result<Vec<LoadedMap>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut best: std::collections::HashMap<MapKind, LoadedMap> = std::collections::HashMap::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !is_image_file(name) {
            continue;
        }
        let Some(kind) = classify(name) else { continue };
        let Ok(img) = image::open(&path) else { continue };
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        let mut buf = rgba.into_raw();

        // Normalise the two conventions that would otherwise leak downstream.
        let kind = match kind {
            MapKind::NormalDx => {
                for px in buf.chunks_exact_mut(4) {
                    px[1] = 255 - px[1]; // DirectX green is down; OpenGL green is up
                }
                MapKind::Normal
            }
            MapKind::Gloss => {
                for px in buf.chunks_exact_mut(4) {
                    px[0] = 255 - px[0];
                    px[1] = 255 - px[1];
                    px[2] = 255 - px[2];
                }
                MapKind::Roughness
            }
            k => k,
        };

        let cand = LoadedMap { kind, name: name.to_string(), w, h, rgba: buf };
        match best.get(&kind) {
            // Between two files for the same slot, take the larger — sets often ship 1K and 2K
            // side by side.
            Some(prev) if prev.w * prev.h >= cand.w * cand.h => {}
            _ => {
                best.insert(kind, cand);
            }
        }
    }

    // Unpack an ARM/ORM image into the three maps it stands for, but only where a DEDICATED file
    // did not already provide one: a standalone Roughness map is higher quality than the green
    // channel of a packed one, so it wins.
    if let Some(arm) = best.remove(&MapKind::Arm) {
        for (kind, chan) in [(MapKind::AmbientOcclusion, 0usize), (MapKind::Roughness, 1), (MapKind::Metallic, 2)] {
            if best.contains_key(&kind) {
                continue;
            }
            let mut rgba = vec![255u8; arm.rgba.len()];
            for (dst, src) in rgba.chunks_exact_mut(4).zip(arm.rgba.chunks_exact(4)) {
                let v = src[chan];
                dst[0] = v;
                dst[1] = v;
                dst[2] = v;
            }
            best.insert(kind, LoadedMap { kind, name: format!("{} [{}]", arm.name, kind.label()), w: arm.w, h: arm.h, rgba });
        }
    }

    if best.is_empty() {
        return Err(format!("no recognised texture maps in {}", dir.display()));
    }
    let mut out: Vec<LoadedMap> = best.into_values().collect();
    // Base colour first — the caller mints it as the material and hangs the rest off it.
    out.sort_by_key(|m| match m.kind {
        MapKind::BaseColor => 0,
        MapKind::Normal => 1,
        MapKind::Roughness => 2,
        MapKind::Metallic => 3,
        MapKind::AmbientOcclusion => 4,
        _ => 5,
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real filenames these libraries ship. Every one of these was copied from an actual
    /// download, because the value of this function is entirely in matching conventions nobody
    /// wrote down — a hand-invented test set would agree with the code and prove nothing.
    #[test]
    fn real_library_filenames_land_in_the_right_slot() {
        let cases: &[(&str, MapKind)] = &[
            // ambientCG
            ("Bricks075A_2K-PNG_Color.png", MapKind::BaseColor),
            ("Bricks075A_2K-PNG_NormalGL.png", MapKind::Normal),
            ("Bricks075A_2K-PNG_NormalDX.png", MapKind::NormalDx),
            ("Bricks075A_2K-PNG_Roughness.png", MapKind::Roughness),
            ("Bricks075A_2K-PNG_AmbientOcclusion.png", MapKind::AmbientOcclusion),
            ("Bricks075A_2K-PNG_Displacement.png", MapKind::Height),
            ("Metal046A_2K-PNG_Metalness.png", MapKind::Metallic),
            ("Metal046A_2K-PNG_Opacity.png", MapKind::Opacity),
            // Poly Haven
            ("wood_floor_deck_diff_2k.jpg", MapKind::BaseColor),
            ("wood_floor_deck_nor_gl_2k.jpg", MapKind::Normal),
            ("wood_floor_deck_rough_2k.jpg", MapKind::Roughness),
            ("wood_floor_deck_ao_2k.jpg", MapKind::AmbientOcclusion),
            ("wood_floor_deck_disp_2k.png", MapKind::Height),
            ("marble_01_arm_2k.jpg", MapKind::Arm), // ARM = occlusion/roughness/metal in R/G/B
            // Quixel / Substance style
            ("T_Concrete_BaseColor.png", MapKind::BaseColor),
            ("T_Concrete_Normal.png", MapKind::Normal),
            ("T_Concrete_Roughness.png", MapKind::Roughness),
            ("T_Concrete_Metallic.png", MapKind::Metallic),
            // textures.com / generic
            ("BrickOldMixedSize_albedo.jpg", MapKind::BaseColor),
            ("BrickOldMixedSize_glossiness.jpg", MapKind::Gloss),
            ("BrickOldMixedSize_height.png", MapKind::Height),
            ("oak-diffuse.png", MapKind::BaseColor),
            ("oak-bump.png", MapKind::Height),
            // Full paths must work too — the loader hands over whatever the OS gives it.
            (r"C:\tex\Bricks075A\Bricks075A_2K_Color.png", MapKind::BaseColor),
            ("/home/x/tex/oak_nor_gl_1k.png", MapKind::Normal),
        ];
        for (name, want) in cases {
            assert_eq!(classify(name), Some(*want), "{name}");
        }
    }

    /// A DirectX normal map must never be mistaken for a GL one. It is the single most damaging
    /// misclassification here: the material still renders, still looks like a material, and every
    /// bump is lit from the wrong side — which reads as "the lighting is a bit odd", not as a bug.
    #[test]
    fn directx_normals_are_never_read_as_opengl() {
        for n in ["x_NormalDX.png", "x_normal_dx.png", "x_nrmDX.jpg", "Wood_2K_NormalDX.png"] {
            assert_eq!(classify(n), Some(MapKind::NormalDx), "{n}");
        }
        for n in ["x_NormalGL.png", "x_normal.png", "x_nor_gl_2k.jpg"] {
            assert_eq!(classify(n), Some(MapKind::Normal), "{n}");
        }
    }

    /// Names that must NOT match, because a false positive silently binds the wrong image.
    /// `_ao` is the dangerous one: matched as a substring it fires on any word containing "ao".
    #[test]
    fn unrelated_files_are_left_alone() {
        for n in [
            "readme.txt",
            "preview.usdc",
            "LICENSE",
            "Bricks075A.mtlx",
            "chao.png",            // contains "ao" but is not an occlusion map
            "rainbow.png",         // contains "bow", "rain" — no token matches
            "render_preview_2k.exe",
        ] {
            assert!(classify(n).is_none() || !is_image_file(n), "{n} should not be taken as a map: {:?}", classify(n));
        }
        // A preview render IS a png and DOES contain no map token — it must simply not classify.
        assert_eq!(classify("preview.png"), None);
        assert_eq!(classify("thumbnail.png"), None);
    }

    /// Only base colour is sRGB. Every other map holds numbers, and decoding numbers through a
    /// display curve is wrong in a way that looks like a subtle material bug rather than an error.
    #[test]
    fn only_base_colour_is_srgb() {
        for k in [MapKind::Normal, MapKind::Roughness, MapKind::Metallic, MapKind::AmbientOcclusion, MapKind::Height, MapKind::Opacity, MapKind::Arm] {
            assert!(!k.is_srgb(), "{:?} must upload linear", k);
        }
        assert!(MapKind::BaseColor.is_srgb());
    }

    #[test]
    fn image_extensions_are_recognised_case_insensitively() {
        for n in ["a.PNG", "a.jpg", "a.JPEG", "a.tif", "a.bmp"] {
            assert!(is_image_file(n), "{n}");
        }
        for n in ["a.exr", "a.txt", "a", "a.usdc"] {
            assert!(!is_image_file(n), "{n}");
        }
    }
}
