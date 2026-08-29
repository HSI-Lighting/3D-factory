//! Font discovery (Windows + Linux/macOS font dirs) + on-demand loading.
//!
//! Scans the system font directories once, mapping a lowercased family name to
//! its file path. Fonts are parsed lazily by `FontManager` when first requested.

use std::collections::HashMap;
use std::path::PathBuf;

/// Embedded fonts — always available, regardless of installed system fonts.
/// `(lowercased key, display name, font bytes)`.
///
/// `DejaVuSans` carries the Arabic + Hebrew blocks, so it doubles as the RTL
/// script fallback (see `TextRenderer`); `Liberation Sans` is the guaranteed
/// default so the engine is ALWAYS ready (no egui fallback that cannot shape
/// Arabic). Both are permissively licensed (see `assets/README.txt`).
const EMBEDDED: &[(&str, &str, &'static [u8])] = &[
    (
        "liberation sans",
        "Liberation Sans",
        include_bytes!("../assets/LiberationSans-Regular.ttf"),
    ),
    (
        "dejavu sans",
        "DejaVu Sans",
        include_bytes!("../assets/DejaVuSans.ttf"),
    ),
];

/// Where a requested font resolves to: a file on disk, or one of the embedded
/// bytes (no path exists for those).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FontSource {
    Path(PathBuf),
    Embedded(&'static [u8]),
}

/// A lazily-built index of installed fonts. Lookup is case-insensitive; display
/// keeps the font's original-case family name. The embedded fonts are seeded
/// into every index so resolution ALWAYS succeeds and the picker always offers
/// an RTL-capable font.
pub(crate) struct FontIndex {
    /// lowercased family name → file path (for resolution).
    pub map: HashMap<String, PathBuf>,
    /// lowercased family name → original-case display name (for the picker).
    pub display: HashMap<String, String>,
    /// Preferred default family key (lowercased) when a request names an unknown
    /// font — e.g. our current styles say "standard"/"monospace", not a real
    /// family.
    pub default_key: Option<String>,
}

impl FontIndex {
    /// Scan the system font directories (Windows, Linux, macOS). Reads + parses
    /// each `.ttf`/`.otf` once to extract its family name. Robust to missing
    /// dirs / unreadable files.
    pub fn scan() -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(w) = std::env::var_os("WINDIR") {
            let mut p = PathBuf::from(w);
            p.push("Fonts");
            dirs.push(p);
        } else if cfg!(windows) {
            dirs.push(PathBuf::from("C:/Windows/Fonts"));
        }
        if let Some(la) = std::env::var_os("LOCALAPPDATA") {
            let mut p = PathBuf::from(la);
            p.push("Microsoft");
            p.push("Windows");
            p.push("Fonts");
            dirs.push(p);
        }
        // Linux / macOS — scan recursively: fontconfig dirs are nested
        // (/usr/share/fonts/truetype/dejavu/…), macOS uses /System/Library/Fonts.
        if !cfg!(windows) {
            for d in [
                "/usr/share/fonts",
                "/usr/local/share/fonts",
                "/System/Library/Fonts",
                "/Library/Fonts",
            ] {
                dirs.push(PathBuf::from(d));
            }
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(&home).join(".fonts"));
                dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
                dirs.push(PathBuf::from(&home).join("Library/Fonts"));
            }
            if let Some(xdg) = std::env::var_os("XDG_DATA_DIRS") {
                for d in std::env::split_paths(&xdg) {
                    dirs.push(d.join("fonts"));
                }
            }
        }

        let mut map: HashMap<String, PathBuf> = HashMap::new();
        let mut display: HashMap<String, String> = HashMap::new();
        fn visit(
            path: &std::path::Path,
            depth: u32,
            map: &mut HashMap<String, PathBuf>,
            display: &mut HashMap<String, String>,
        ) {
            if depth > 6 {
                return; // runaway guard — font dirs are never deeper
            }
            let rd = match std::fs::read_dir(path) {
                Ok(rd) => rd,
                Err(_) => return,
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    visit(&p, depth + 1, map, display);
                    continue;
                }
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase());
                if !matches!(ext.as_deref(), Some("ttf") | Some("otf")) {
                    continue;
                }
                let bytes = match std::fs::read(&p) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                if let Ok(face) = ttf_parser::Face::parse(&bytes, 0) {
                    if let Some(name) = family_name(&face) {
                        let key = name.to_lowercase();
                        map.entry(key.clone()).or_insert(p.clone());
                        display.entry(key).or_insert(name);
                    }
                }
            }
        }
        for dir in dirs {
            visit(&dir, 0, &mut map, &mut display);
        }

        // Pick a sensible default among what's actually installed.
        let default_key = [
            "arial",
            "segoe ui",
            "tahoma",
            "verdana",
            "calibri",
            "dejavu sans",
            "liberation sans",
            "carlito",
            "noto sans",
            "cantarell",
            "helvetica",
        ]
        .iter()
        .find(|k| map.contains_key(**k))
        .map(|k| k.to_string())
        .or_else(|| {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.first().map(|s| (*s).clone())
        });

        // Seed the embedded fonts: a family already installed on the system
        // keeps its file path; otherwise the embedded copy becomes resolvable.
        for (key, display_name, _) in EMBEDDED {
            map.entry(key.to_string()).or_insert_with(|| PathBuf::new());
            display.entry(key.to_string()).or_insert(display_name.to_string());
        }

        // A fontless system still gets a default (the embedded Liberation Sans).
        let default_key = default_key.or_else(|| Some("liberation sans".to_string()));

        FontIndex { map, display, default_key }
    }

    /// Resolve a requested font name: exact family match, else the default.
    /// ALWAYS succeeds — the embedded fonts guarantee a usable font even on a
    /// system with no installed fonts.
    pub fn resolve(&self, requested: &str) -> FontSource {
        let key = requested.to_lowercase();
        if let Some(p) = self.map.get(&key) {
            if !p.as_os_str().is_empty() {
                return FontSource::Path(p.clone());
            }
            if let Some(bytes) = embedded_bytes(&key) {
                return FontSource::Embedded(bytes);
            }
        }
        // Fall back to the default: system font first, then embedded.
        if let Some(k) = self.default_key.as_ref() {
            if let Some(p) = self.map.get(k) {
                if !p.as_os_str().is_empty() {
                    return FontSource::Path(p.clone());
                }
            }
            if let Some(bytes) = embedded_bytes(k) {
                return FontSource::Embedded(bytes);
            }
        }
        // Absolute last resort: the first embedded font.
        FontSource::Embedded(EMBEDDED[0].2)
    }

    /// Sorted list of available family names in their original case (for the
    /// font picker). Case-insensitive sort so "Arial" and "arial Narrow" order
    /// naturally. Includes the embedded fonts, so an RTL-capable font is always
    /// offered.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.display.values().cloned().collect();
        v.sort_by_key(|s| s.to_lowercase());
        v
    }
}

/// Best family name from a face: typographic family (name id 16) preferred,
/// else the classic family (name id 1).
fn family_name(face: &ttf_parser::Face) -> Option<String> {
    let mut fam1: Option<String> = None;
    let mut fam16: Option<String> = None;
    for name in face.names() {
        // name.to_string() decodes only names in a Unicode-ish encoding.
        let s = match name.to_string() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        match name.name_id {
            16 if fam16.is_none() => fam16 = Some(s),
            1 if fam1.is_none() => fam1 = Some(s),
            _ => {}
        }
    }
    fam16.or(fam1)
}

/// Bytes of the embedded font with the given lowercased family key.
fn embedded_bytes(key: &str) -> Option<&'static [u8]> {
    EMBEDDED.iter().find(|(k, _, _)| *k == key).map(|(_, _, b)| *b)
}

/// The embedded RTL-capable font (Arabic + Hebrew + Latin) used as the
/// per-run script fallback when the requested font lacks those glyphs.
pub(crate) fn rtl_fallback_bytes() -> &'static [u8] {
    EMBEDDED
        .iter()
        .find(|(k, _, _)| *k == "dejavu sans")
        .map(|(_, _, b)| *b)
        .unwrap_or(EMBEDDED[0].2)
}
