//! THE LAST CALCULATION, KEPT.
//!
//! Asked for as: *"once a calculation is run the app should save it. the calculation should only be
//! invalidated if any of the lights, the 3d objects or anything related with the calculation is
//! changed. if the user closed the app after a calculation the they should not lose the result."*
//!
//! A lighting calculation on a real building is a seventy-second job, and on a large one several
//! minutes. Throwing that away because somebody closed the window is not a small annoyance: it
//! makes the result something you can only look at during the session that produced it, so nobody
//! can reopen last week's project and read what it came out at without paying for it again.
//!
//! Two decisions are the whole of this module.
//!
//! **It is a separate file, not part of the sidecar.** `drawing.simlux.json` is small, pretty
//! printed and written on every save; a result is a few hundred thousand numbers. Folding one into
//! the other would make every autosave rewrite megabytes and make the sidecar — which a person can
//! usefully open and read — something no editor wants to load. So the result lives beside it as
//! `drawing.simlux-result.json`, and deleting that file is a complete, obvious way to discard a
//! result without touching the project.
//!
//! **The bulk is a blob, the meaning is not.** Room names, plane geometry, statistics, the
//! schedule and the fingerprint stay as readable JSON; only the per-cell arrays — which are the
//! part no human reads and the part that is 99% of the bytes — are deflated and base64'd, exactly
//! as furniture geometry already is in the sidecar. A 125 × 38 room's three arrays are 55 kB of
//! JSON numbers and about 8 kB this way, and parse in a fraction of the time.
//!
//! The per-cell values go to disk as `f32`, which carries about seven significant figures — six
//! more than a lux figure is ever quoted to. **The summary statistics do not**: `min`, `max`, `avg`
//! and the maintenance factor are stored as the `f64` they were computed as and restored as-is
//! rather than recomputed from the rounded cells, so the number on screen after reopening is the
//! same number, digit for digit, that the calculation produced. A result that quietly shifts in its
//! last decimal when reloaded is a result nobody can quote.

use crate::light::RoomResult;
use cad_light::{CalcPlane, Installation, Luminaire, LuxGrid, SurfaceResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The format version. Bumped when a change would make an older file mean something different;
/// a file from a version this build does not know is DISCARDED rather than guessed at, because a
/// misread result is worse than a missing one.
pub const VERSION: u32 = 1;

/// One room's answer, as it goes to disk.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredRoom {
    pub name: String,
    /// The footprint. `glam::Vec2` has no serde in the version this workspace pins, and a pair of
    /// floats needs no help.
    pub poly: Vec<[f32; 2]>,
    pub plane: Option<CalcPlane>,
    pub grid: StoredGrid,
    /// Which cells are inside the room, one BIT each — a `Vec<bool>` as JSON is six bytes per cell
    /// to say true or false.
    pub mask_bits: String,
    pub mask_len: usize,
    pub plane_en: Option<CalcPlane>,
    pub grid_en: StoredGrid,
    /// The same, for the EN 12464-1 grid — see `crate::light::RoomResult::mask_en`. Defaulted, so a
    /// file written before this existed still loads; the report falls back to the summary figures
    /// rather than scanning cells it cannot filter.
    #[serde(default)]
    pub mask_en_bits: String,
    #[serde(default)]
    pub mask_en_len: usize,
    pub cylindrical_avg: Option<f64>,
    pub installation: Option<Installation>,
    /// The fixtures standing in this room, as RECORDS. They must be the records and not ids: a
    /// room holds user-placed fittings and the lights the model generates, and the generated ones
    /// exist only for the length of a calculation — see [`RoomResult::fixtures`].
    pub fixtures: Vec<Luminaire>,
    pub grid_note: Option<String>,
}

/// A lux grid, with the three per-cell arrays packed.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredGrid {
    pub cols: u32,
    pub rows: u32,
    /// Statistics as COMPUTED, not as recomputed from the packed cells — see the module note.
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub maintenance: f64,
    pub values: String,
    pub direct: String,
    pub indirect: String,
}

/// Everything a calculation produced, plus what it was a calculation OF.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredResults {
    pub version: u32,
    /// [`crate::light::CalcJob::fingerprint`] of the scene this answer belongs to. The whole point
    /// of the file: it is how reopening a project can tell a result that is still true from one
    /// that describes a building somebody has since changed.
    pub fingerprint: u64,
    /// The build that produced it, for the status line and for support — never for deciding
    /// whether the result is valid, which is the fingerprint's job alone.
    pub build: String,
    pub rooms: Vec<StoredRoom>,
    pub surfaces: Vec<SurfaceResult>,
    pub timings: Vec<(String, f64)>,
}

// ---------------------------------------------------------------------------------------------
// packing
// ---------------------------------------------------------------------------------------------

/// `f64` cells out to a compact blob. Reuses the encoder the sidecar already stores furniture
/// geometry with, so there is one implementation of this and one place for it to be wrong.
fn pack(v: &[f64]) -> String {
    if v.is_empty() {
        return String::new();
    }
    let f: Vec<f32> = v.iter().map(|x| *x as f32).collect();
    crate::factory::encode_f32_blob(&f)
}

fn unpack(s: &str) -> Vec<f64> {
    if s.is_empty() {
        return Vec::new();
    }
    crate::factory::decode_f32_blob(s).into_iter().map(|x| x as f64).collect()
}

/// The room mask as bits, LSB first.
fn pack_mask(m: &[bool]) -> String {
    if m.is_empty() {
        return String::new();
    }
    use base64::Engine;
    let mut bytes = vec![0u8; m.len().div_ceil(8)];
    for (i, b) in m.iter().enumerate() {
        if *b {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    let comp = miniz_oxide::deflate::compress_to_vec(&bytes, 1);
    base64::engine::general_purpose::STANDARD.encode(comp)
}

/// Unpack `len` bits. A blob that does not carry them is treated as NO MASK rather than as a
/// partial one — a half-decoded mask would silently drop cells out of a room's average.
fn unpack_mask(s: &str, len: usize) -> Vec<bool> {
    if s.is_empty() || len == 0 {
        return Vec::new();
    }
    use base64::Engine;
    let Ok(comp) = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) else {
        return Vec::new();
    };
    let Ok(bytes) = miniz_oxide::inflate::decompress_to_vec(&comp) else {
        return Vec::new();
    };
    if bytes.len() < len.div_ceil(8) {
        return Vec::new();
    }
    (0..len).map(|i| bytes[i / 8] & (1 << (i % 8)) != 0).collect()
}

impl StoredGrid {
    fn of(g: &LuxGrid) -> Self {
        StoredGrid {
            cols: g.cols,
            rows: g.rows,
            min: g.min,
            max: g.max,
            avg: g.avg,
            maintenance: g.maintenance,
            values: pack(&g.values),
            direct: pack(&g.direct),
            indirect: pack(&g.indirect),
        }
    }

    /// Back to a grid — built FIELD BY FIELD rather than through `LuxGrid::from_parts`, which
    /// would recompute the statistics from the `f32`-rounded cells and shift them in their last
    /// digits. The stored statistics are the calculated ones.
    fn to_grid(&self) -> LuxGrid {
        LuxGrid {
            cols: self.cols,
            rows: self.rows,
            values: unpack(&self.values),
            min: self.min,
            max: self.max,
            avg: self.avg,
            maintenance: self.maintenance,
            direct: unpack(&self.direct),
            indirect: unpack(&self.indirect),
        }
    }

    /// Whether this grid actually carries the cells it claims to.
    fn intact(&self) -> bool {
        let n = self.cols as usize * self.rows as usize;
        n > 0 && unpack(&self.values).len() == n
    }
}

impl StoredResults {
    /// Capture a finished calculation.
    pub fn of(
        rooms: &[RoomResult],
        surfaces: &[SurfaceResult],
        timings: &[(&'static str, f64)],
        fingerprint: u64,
        build: &str,
    ) -> Self {
        StoredResults {
            version: VERSION,
            fingerprint,
            build: build.to_string(),
            rooms: rooms
                .iter()
                .map(|r| StoredRoom {
                    name: r.name.clone(),
                    poly: r.poly.iter().map(|v| [v.x, v.y]).collect(),
                    plane: Some(r.plane),
                    grid: StoredGrid::of(&r.grid),
                    mask_bits: pack_mask(&r.mask),
                    mask_len: r.mask.len(),
                    plane_en: Some(r.plane_en),
                    grid_en: StoredGrid::of(&r.grid_en),
                    mask_en_bits: pack_mask(&r.mask_en),
                    mask_en_len: r.mask_en.len(),
                    cylindrical_avg: r.cylindrical_avg,
                    installation: r.installation,
                    fixtures: r.fixtures.clone(),
                    grid_note: r.grid_note.clone(),
                })
                .collect(),
            surfaces: surfaces.to_vec(),
            timings: timings.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    /// Rebuild the results, or `None` if this file cannot be trusted to be one.
    ///
    /// A room whose plane or cells did not survive the round trip is not repaired and not
    /// half-restored: the WHOLE file is refused. Half a calculation looks exactly like a whole one
    /// on screen, and the cost of refusing is that somebody presses Calculate again.
    pub fn rooms(&self) -> Option<Vec<RoomResult>> {
        if self.version != VERSION || self.rooms.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(self.rooms.len());
        for r in &self.rooms {
            let (Some(plane), Some(plane_en)) = (r.plane, r.plane_en) else {
                return None;
            };
            if !r.grid.intact() {
                return None;
            }
            let mask = unpack_mask(&r.mask_bits, r.mask_len);
            // A mask that was written and did not come back is refused rather than dropped: with
            // no mask every cell counts, so an L-shaped room would silently start averaging the
            // ground outside it and report a different number than it did before it was saved.
            if r.mask_len > 0 && mask.len() != r.mask_len {
                return None;
            }
            let mask_en = unpack_mask(&r.mask_en_bits, r.mask_en_len);
            if r.mask_en_len > 0 && mask_en.len() != r.mask_en_len {
                return None;
            }
            out.push(RoomResult {
                name: r.name.clone(),
                poly: r.poly.iter().map(|p| glam::Vec2::new(p[0], p[1])).collect(),
                plane,
                grid: r.grid.to_grid(),
                mask,
                plane_en,
                grid_en: r.grid_en.to_grid(),
                mask_en,
                cylindrical_avg: r.cylindrical_avg,
                installation: r.installation,
                fixtures: r.fixtures.clone(),
                grid_note: r.grid_note.clone(),
            });
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------------------------
// the file
// ---------------------------------------------------------------------------------------------

/// `foo.rsm` → `foo.simlux-result.json`, beside the sidecar.
pub fn result_path(drawing: &Path) -> PathBuf {
    drawing.with_extension("simlux-result.json")
}

/// Read the stored result for `drawing`. `Ok(None)` = there isn't one.
///
/// A file that will not parse is reported as absent rather than as an error. It is a CACHE: the
/// calculation that produced it can always be run again, so the worst case is the wait, and a
/// dialog about a corrupt cache in front of somebody opening a drawing helps nobody.
pub fn load(drawing: &Path) -> Option<StoredResults> {
    let p = result_path(drawing);
    let text = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write it, atomically — temp then rename, so a close or a crash mid-write cannot leave half a
/// result where the last good one was.
pub fn save(drawing: &Path, r: &StoredResults) -> Result<PathBuf, String> {
    let p = result_path(drawing);
    // NOT pretty printed. The metadata is a few dozen lines either way and the blobs are single
    // enormous strings that no indentation helps; compact halves the file for nothing lost.
    let text = serde_json::to_string(r).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.savetmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(p)
}

/// Drop the stored result for `drawing`, if there is one.
pub fn discard(drawing: &Path) {
    let _ = std::fs::remove_file(result_path(drawing));
}
