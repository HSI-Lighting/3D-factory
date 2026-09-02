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
    /// File path + face index (font collections carry one face per index;
    /// regular fonts are always index 0).
    Path(PathBuf, u32),
    Embedded(&'static [u8]),
}

/// Hard cap on faces registered from one font collection — `fonts_in_collection`
/// returns the raw u32 face count from the file header, and a crafted file
/// must not be able to make the scan loop billions of times.
const MAX_COLLECTION_FACES: u32 = 256;

/// A lazily-built index of installed fonts. Lookup is case-insensitive; display
/// keeps the font's original-case family name. The embedded fonts are seeded
/// into every index so resolution ALWAYS succeeds and the picker always offers
/// an RTL-capable font.
pub(crate) struct FontIndex {
    /// lowercased family name → (file path, face index) (for resolution).
    pub map: HashMap<String, (PathBuf, u32)>,
    /// lowercased family name → original-case display name (for the picker).
    pub display: HashMap<String, String>,
    /// Preferred default family key (lowercased) when a request names an unknown
    /// font — e.g. our current styles say "standard"/"monospace", not a real
    /// family.
    pub default_key: Option<String>,
    /// Display names sorted once at scan time — the font pickers read this
    /// EVERY FRAME, so it must never be re-sorted per call.
    sorted_names: Vec<String>,
    /// `.ttc`/`.otc` collections deferred from the startup scan. Reading a
    /// CJK collection (10-20 MB) on the UI thread stalls launch; they are
    /// scanned lazily when the font picker first opens instead.
    pending_collections: Vec<PathBuf>,
}

impl FontIndex {
    /// Scan the system font directories (Windows, Linux, macOS). Reads + parses
    /// each `.ttf`/`.otf` once to extract its family name. Robust to missing
    /// dirs / unreadable files. `.ttc`/`.otc` collections are deferred to the
    /// first `names()` call (they are heavy reads; see `pending_collections`).
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

        let mut map: HashMap<String, (PathBuf, u32)> = HashMap::new();
        let mut display: HashMap<String, String> = HashMap::new();
        let mut pending_collections: Vec<PathBuf> = Vec::new();
        fn visit(
            path: &std::path::Path,
            depth: u32,
            map: &mut HashMap<String, (PathBuf, u32)>,
            display: &mut HashMap<String, String>,
            pending: &mut Vec<PathBuf>,
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
                    visit(&p, depth + 1, map, display, pending);
                    continue;
                }
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase());
                if !matches!(
                    ext.as_deref(),
                    Some("ttf") | Some("otf") | Some("ttc") | Some("otc")
                ) {
                    continue;
                }
                // Collections are deferred: reading a large .ttc fully at
                // launch is a measurable UI-thread stall; the picker (the
                // only consumer of the list) scans them on first open.
                if matches!(ext.as_deref(), Some("ttc") | Some("otc")) {
                    pending.push(p);
                    continue;
                }
                let bytes = match std::fs::read(&p) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                register_face_families(&bytes, &p, map, display);
            }
        }
        for dir in dirs {
            visit(&dir, 0, &mut map, &mut display, &mut pending_collections);
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
            map.entry(key.to_string()).or_insert_with(|| (PathBuf::new(), 0));
            display.entry(key.to_string()).or_insert(display_name.to_string());
        }

        // A fontless system still gets a default (the embedded Liberation Sans).
        let default_key = default_key.or_else(|| Some("liberation sans".to_string()));

        // Sorted display names built ONCE (the pickers read the list every
        // frame — no per-frame sort/alloc).
        let mut sorted_names: Vec<String> = display.values().cloned().collect();
        sorted_names.sort_by_key(|s| s.to_lowercase());

        FontIndex {
            map,
            display,
            default_key,
            sorted_names,
            pending_collections,
        }
    }

    /// Resolve a requested font name: exact family match, else the default.
    /// ALWAYS succeeds — the embedded fonts guarantee a usable font even on a
    /// system with no installed fonts.
    pub fn resolve(&self, requested: &str) -> FontSource {
        let key = requested.to_lowercase();
        if let Some((p, idx)) = self.map.get(&key) {
            if !p.as_os_str().is_empty() {
                return FontSource::Path(p.clone(), *idx);
            }
            if let Some(bytes) = embedded_bytes(&key) {
                return FontSource::Embedded(bytes);
            }
        }
        // Fall back to the default: system font first, then embedded.
        if let Some(k) = self.default_key.as_ref() {
            if let Some((p, idx)) = self.map.get(k) {
                if !p.as_os_str().is_empty() {
                    return FontSource::Path(p.clone(), *idx);
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
    /// offered. Scans the deferred `.ttc`/`.otc` collections on first call.
    pub fn names(&mut self) -> &[String] {
        self.scan_pending();
        &self.sorted_names
    }

    /// Scan the collections deferred at startup (first font-picker open — a
    /// user gesture where a short stall is acceptable, unlike launch).
    fn scan_pending(&mut self) {
        if self.pending_collections.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_collections);
        for p in pending {
            let bytes = match std::fs::read(&p) {
                Ok(b) => b,
                Err(_) => continue,
            };
            register_face_families(&bytes, &p, &mut self.map, &mut self.display);
        }
        let mut v: Vec<String> = self.display.values().cloned().collect();
        v.sort_by_key(|s| s.to_lowercase());
        self.sorted_names = v;
    }
}

/// Register every family name of a font file (`bytes`) under `path`. A font
/// collection registers EVERY face's family with its face index (so resolution
/// renders the correct face); regular fonts are face 0. The face iteration is
/// capped — the count comes straight from the untrusted file header.
fn register_face_families(
    bytes: &[u8],
    path: &std::path::Path,
    map: &mut HashMap<String, (PathBuf, u32)>,
    display: &mut HashMap<String, String>,
) {
    let faces = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    let n = faces.min(MAX_COLLECTION_FACES);
    for i in 0..n {
        if let Ok(face) = ttf_parser::Face::parse(bytes, i) {
            if let Some(name) = family_name(&face) {
                insert_family(name.clone(), path, i, map, display);
                // Legacy alias: documents saved under the OLD decoder's
                // output (which could be mojibake) must keep resolving to
                // this file after the decoder fix.
                if let Some(legacy) = family_name_legacy(&face) {
                    if !legacy.eq_ignore_ascii_case(&name) {
                        map.entry(legacy.to_lowercase())
                            .or_insert((path.to_path_buf(), i));
                    }
                }
            }
        }
    }
}

fn insert_family(
    name: String,
    path: &std::path::Path,
    face_index: u32,
    map: &mut HashMap<String, (PathBuf, u32)>,
    display: &mut HashMap<String, String>,
) {
    let key = name.to_lowercase();
    map.entry(key.clone())
        .or_insert((path.to_path_buf(), face_index));
    display.entry(key).or_insert(name);
}

/// Best family name from a face: typographic family (name id 16) preferred,
/// else the classic family (name id 1). Name records are decoded MANUALLY —
/// `ttf_parser`'s `Name::to_string()` only covers a subset of encodings and
/// happily returns mojibake for malformed Unicode-platform records (verified:
/// `Far_Nazanin.ttf` carries an ASCII byte string tagged as Unicode, which
/// `to_string()` decodes as UTF-16BE into `䙡爮乡穡湩`).
///
/// Ranking: name id 16 over 1; within an id, Windows (3) over Unicode (0)
/// over Macintosh (1); prefer English (lang 0x0409 / 0). Records that fail
/// to decode are skipped — never returned.
fn family_name(face: &ttf_parser::Face) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for name in face.names() {
        if name.name_id != 1 && name.name_id != 16 {
            continue;
        }
        let plat: u8 = match name.platform_id {
            ttf_parser::PlatformId::Windows => 3,
            ttf_parser::PlatformId::Unicode => 0,
            ttf_parser::PlatformId::Macintosh => 1,
            _ => continue,
        };
        let s = match plat {
            // Windows: encoding 1/10 = Unicode (UTF-16BE); 0 = symbol — skip
            // rather than produce mojibake.
            3 => match name.encoding_id {
                1 | 10 => decode_utf16_be(name.name),
                _ => continue,
            },
            // Unicode platform: always UTF-16BE.
            0 => decode_utf16_be(name.name),
            // Macintosh: MacRoman.
            1 => decode_mac_roman(name.name),
            _ => unreachable!(),
        };
        let Some(s) = s else { continue };
        if s.is_empty() {
            continue;
        }
        let english = match plat {
            3 => name.language_id == 0x0409,
            _ => name.language_id == 0,
        };
        let score = (if name.name_id == 16 { 1000 } else { 0 })
            + match plat {
                3 => 300,
                0 => 200,
                _ => 100,
            }
            + if english { 10 } else { 0 };
        if best.as_ref().map_or(true, |(b, _)| score > *b) {
            best = Some((score, s));
        }
    }
    best.map(|(_, s)| s)
}

/// The OLD first-wins `Name::to_string()` decoder, kept ONLY as a
/// compatibility alias: documents saved before the manual decoder shipped
/// persisted these strings as `font_name`, and they must keep resolving to
/// the same file. Not used for display or new lookups.
fn family_name_legacy(face: &ttf_parser::Face) -> Option<String> {
    let mut fam1: Option<String> = None;
    let mut fam16: Option<String> = None;
    for name in face.names() {
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

/// Decode a UTF-16BE name record. `None` on an odd byte length (can't be
/// valid UTF-16); lone surrogates become U+FFFD rather than failing the
/// whole name.
fn decode_utf16_be(bytes: &[u8]) -> Option<String> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Decode a Macintosh-Roman name record (platform 1, encoding 0). Bytes
/// < 0x80 are ASCII; the rest map through the standard MacRoman table.
fn decode_mac_roman(bytes: &[u8]) -> Option<String> {
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x80 {
            s.push(b as char);
        } else {
            s.push(char::from_u32(MAC_ROMAN[(b - 0x80) as usize] as u32)?);
        }
    }
    Some(s)
}

/// MacRoman 0x80..=0xFF → Unicode (standard Apple mapping).
const MAC_ROMAN: [u16; 128] = [
    0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, // 0x80
    0x00E0, 0x00E2, 0x00E4, 0x00E3, 0x00E5, 0x00E7, 0x00E9, 0x00E8, // 0x88
    0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF, 0x00F1, 0x00F3, // 0x90
    0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC, // 0x98
    0x2020, 0x00B0, 0x00A2, 0x00A3, 0x00A7, 0x2022, 0x00B6, 0x00DF, // 0xA0
    0x00AE, 0x00A9, 0x2122, 0x00B4, 0x00A8, 0x2260, 0x00C6, 0x00D8, // 0xA8
    0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202, 0x2211, // 0xB0
    0x220F, 0x03C0, 0x222B, 0x00AA, 0x00BA, 0x03A9, 0x00E6, 0x00F8, // 0xB8
    0x00BF, 0x00A1, 0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB, // 0xC0
    0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3, 0x00D5, 0x0152, 0x0153, // 0xC8
    0x2013, 0x2014, 0x201C, 0x201D, 0x2018, 0x2019, 0x00F7, 0x25CA, // 0xD0
    0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02, // 0xD8
    0x2021, 0x00B7, 0x201A, 0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, // 0xE0
    0x00CB, 0x00C8, 0x00CD, 0x00CE, 0x00CF, 0x00CC, 0x00D3, 0x00D4, // 0xE8
    0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC, // 0xF0
    0x00AF, 0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7, // 0xF8
];

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

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    /// A minimal-but-parseable TTF: sfnt header + head/hhea/maxp/name tables
    /// (ttf_parser requires head, hhea and maxp; only the name table matters
    /// here). `records` = `(platform, encoding, language, name_id, bytes)`.
    fn minimal_ttf(records: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut name_tbl = Vec::new();
        name_tbl.extend_from_slice(&0u16.to_be_bytes()); // format 0
        name_tbl.extend_from_slice(&(records.len() as u16).to_be_bytes());
        let str_off = 6 + 12 * records.len();
        name_tbl.extend_from_slice(&(str_off as u16).to_be_bytes());
        let mut strings = Vec::new();
        let mut cursor = 0usize;
        for (p, e, l, n, bytes) in records {
            name_tbl.extend_from_slice(&p.to_be_bytes());
            name_tbl.extend_from_slice(&e.to_be_bytes());
            name_tbl.extend_from_slice(&l.to_be_bytes());
            name_tbl.extend_from_slice(&n.to_be_bytes());
            name_tbl.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            name_tbl.extend_from_slice(&(cursor as u16).to_be_bytes());
            strings.extend_from_slice(bytes);
            cursor += bytes.len();
        }
        name_tbl.extend_from_slice(&strings);

        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes()); // magic
        head[18..20].copy_from_slice(&2048u16.to_be_bytes()); // unitsPerEm
        head[36..38].copy_from_slice(&(-1000i16).to_be_bytes()); // xMin
        head[38..40].copy_from_slice(&(-200i16).to_be_bytes()); // yMin
        head[40..42].copy_from_slice(&1000u16.to_be_bytes()); // xMax
        head[42..44].copy_from_slice(&1300u16.to_be_bytes()); // yMax
        head[46..48].copy_from_slice(&8u16.to_be_bytes()); // lowestRecPPEM
        head[48..50].copy_from_slice(&2u16.to_be_bytes()); // fontDirectionHint

        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[4..6].copy_from_slice(&1000i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&1u16.to_be_bytes());

        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&1u16.to_be_bytes());

        let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"head", head),
            (b"hhea", hhea),
            (b"maxp", maxp),
            (b"name", name_tbl),
        ];
        let num = tables.len() as u16;
        let mut font = Vec::new();
        font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        font.extend_from_slice(&num.to_be_bytes());
        font.extend_from_slice(&64u16.to_be_bytes()); // searchRange
        font.extend_from_slice(&2u16.to_be_bytes()); // entrySelector
        font.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        let mut off = (12 + 16 * tables.len()) as u32;
        for (tag, data) in &tables {
            font.extend_from_slice(*tag);
            font.extend_from_slice(&0u32.to_be_bytes()); // checksum
            font.extend_from_slice(&off.to_be_bytes());
            font.extend_from_slice(&(data.len() as u32).to_be_bytes());
            off += (data.len() + 3) as u32 / 4 * 4; // 4-byte aligned
        }
        for (_, mut data) in tables {
            while data.len() % 4 != 0 {
                data.push(0);
            }
            font.extend_from_slice(&data);
        }
        font
    }

    #[test]
    fn utf16be_name_decodes() {
        assert_eq!(decode_utf16_be(&utf16("Far Nazanin")).as_deref(), Some("Far Nazanin"));
        // Odd length can't be UTF-16BE → None (record skipped, never mojibake).
        assert_eq!(decode_utf16_be(b"Far"), None);
    }

    #[test]
    fn mac_roman_name_decodes() {
        assert_eq!(decode_mac_roman(b"Caf\x8E").as_deref(), Some("Café"));
        assert_eq!(decode_mac_roman(b"Plain ASCII").as_deref(), Some("Plain ASCII"));
    }

    #[test]
    fn family_name_prefers_windows_over_malformed_unicode_record() {
        // Far_Nazanin.ttf case: the pid=0 record holds raw ASCII bytes
        // ("Far.Nazani") tagged as Unicode — decoding it as UTF-16BE yields
        // mojibake. The pid=3 (Windows) UTF-16BE record must win.
        let font = minimal_ttf(&[
            (0, 0, 0, 1, b"Far.Nazani".to_vec()),
            (1, 0, 0, 1, b"Far.Nazanin".to_vec()),
            (3, 1, 0x0409, 1, utf16("Far.Nazanin")),
        ]);
        let face = ttf_parser::Face::parse(&font, 0).unwrap();
        assert_eq!(family_name(&face).as_deref(), Some("Far.Nazanin"));
    }

    #[test]
    fn family_name_prefers_typographic_over_family() {
        // name id 16 wins even on a lower-priority platform (MacRoman).
        let font = minimal_ttf(&[
            (3, 1, 0x0409, 1, utf16("Family Name")),
            (1, 0, 0, 16, b"Typographic Name".to_vec()),
        ]);
        let face = ttf_parser::Face::parse(&font, 0).unwrap();
        assert_eq!(family_name(&face).as_deref(), Some("Typographic Name"));
    }

    #[test]
    fn symbol_encoded_windows_name_is_skipped() {
        // pid=3 eid=0 (symbol) must never produce mojibake; the Mac record wins.
        let font = minimal_ttf(&[
            (3, 0, 0x0409, 1, b"Symbol bytes".to_vec()),
            (1, 0, 0, 1, b"Mac Family".to_vec()),
        ]);
        let face = ttf_parser::Face::parse(&font, 0).unwrap();
        assert_eq!(family_name(&face).as_deref(), Some("Mac Family"));
    }

    #[test]
    fn utf16be_named_font_resolves_by_name() {
        // End-to-end through FontIndex: a font whose family is stored UTF-16BE
        // must resolve by name and appear in the picker under its real name.
        let font = minimal_ttf(&[(3, 1, 0x0409, 1, utf16("Far Nazanin"))]);
        let dir = std::env::temp_dir()
            .join(format!("cad_text_font_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("far_nazanin.ttf");
        std::fs::write(&path, &font).unwrap();
        let mut map = HashMap::new();
        let mut display = HashMap::new();
        insert_family("Far Nazanin".to_string(), &path, 0, &mut map, &mut display);
        let mut index = FontIndex {
            map,
            display,
            default_key: Some("far nazanin".into()),
            sorted_names: vec!["Far Nazanin".into()],
            pending_collections: Vec::new(),
        };
        assert_eq!(index.resolve("far nazanin"), FontSource::Path(path.clone(), 0));
        assert!(index.names().iter().any(|n| n == "Far Nazanin"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_misdecoded_name_resolves_as_alias() {
        // The old decoder returned mojibake for a malformed pid=0 record;
        // documents saved under that string must still resolve to the file.
        let font = minimal_ttf(&[
            (0, 0, 0, 1, b"Far.Nazani".to_vec()), // mojibake under old decoder
            (3, 1, 0x0409, 1, utf16("Far.Nazanin")),
        ]);
        let face = ttf_parser::Face::parse(&font, 0).unwrap();
        let legacy = family_name_legacy(&face);
        assert!(legacy.is_some(), "old decoder produced SOMETHING for this font");
        assert_ne!(legacy.as_deref(), Some("Far.Nazanin"), "old output was wrong");
        let mut map = HashMap::new();
        let mut display = HashMap::new();
        register_face_families(&font, std::path::Path::new("/tmp/x.ttf"),
            &mut map, &mut display);
        let key = legacy.unwrap().to_lowercase();
        assert_eq!(map.get(&key), Some(&(std::path::PathBuf::from("/tmp/x.ttf"), 0)),
            "legacy name must be registered as a resolution alias");
    }
}


