//! **Light Editor** — wire the blocks in a drawing to photometric files, and place a fitting at
//! every instance.
//!
//! THE PROBLEM IT SOLVES. A lighting layout arrives as a plan full of block references: two
//! hundred copies of `DOWNLIGHT-600`, ninety of `TRACK-3C`. Placing a luminaire on each by hand is
//! the work, and it is work the drawing has already done — the positions are all there, in the
//! block instances. This pairs a block DEFINITION with an `.ldt`/`.ies` file once, and places the
//! fittings for every instance of it.
//!
//! DEFINITIONS, NOT INSTANCES. The left-hand list is one row per block type with a count, so
//! wiring is done once per fitting type rather than once per fitting.
//!
//! CURATION IS NOT DELETION. Most blocks in a real plan are furniture, doors and title-block
//! furniture, not luminaires, so the list needs filtering to be usable at all. Hiding a block
//! removes it from THIS WINDOW and touches neither the drawing nor its block table.
//!
//! TWO SPACES, AND THIS IS THE TRAP. A block's insertion point is in DRAWING UNITS; a luminaire's
//! position is a world (x, y) in METRES. A plan drawn in millimetres would put every fitting a
//! thousand times too far out with no error anywhere — see [`Wiring::plan_placements`], which is
//! the only place the conversion happens and has the test that pins it.

use std::collections::{HashMap, HashSet};

use cad_kernel::{Document, Geom};

/// One row of the left-hand list: a block definition and how many times the drawing places it.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockRow {
    pub id: u32,
    pub name: String,
    /// How many `BlockRef`s in the drawing point at this definition. A definition with none is
    /// still listed — it is in the block table, and wiring it now means the fittings appear the
    /// moment one is inserted.
    pub instances: usize,
}

/// One row of the right-hand list: a photometric file offered to be wired.
#[derive(Clone, Debug, PartialEq)]
pub struct LdtRow {
    /// The profile key, which is what a [`cad_light::Luminaire`] stores.
    pub name: String,
    /// Where it came from, for a file found by scanning. `None` = already in the session library.
    pub path: Option<String>,
    /// True when the profile is loaded and ready to place with. A scanned file that has not been
    /// imported yet is offered, but wiring it has to import it first.
    pub loaded: bool,
}

/// One fitting the wiring says to place: a world position in METRES, plus its rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub block: u32,
    pub x: f32,
    pub y: f32,
    pub rotation_deg: f32,
}

/// The block → photometry pairing, and the curation that goes with it.
///
/// Serialisable so a project reopens with its wiring intact: re-deriving it from the placed
/// fittings would be guesswork, and losing it means doing the pairing again every session.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Wiring {
    /// Block id → profile name.
    #[serde(default)]
    pub links: HashMap<u32, String>,
    /// Blocks curated OUT of the list. Not deleted from anything — see the module note.
    ///
    /// From the ＋ Add flow onward only the CHOOSER consults it: the main list is built from what
    /// has been added, rather than from everything minus what has been hidden. Kept and honoured
    /// because projects carry it, and a plan where somebody hid two hundred door blocks should not
    /// have them come back.
    #[serde(default)]
    pub hidden: HashSet<u32>,
    /// Blocks ADDED to the editor's list, whether or not they have a fitting yet.
    ///
    /// The main window used to list every block in the drawing — hundreds on a real plan, nearly
    /// all of them doors and furniture. It now lists what somebody deliberately put there.
    ///
    /// A WIRED BLOCK IS IN THE LIST WHETHER OR NOT IT IS HERE, which is what makes adding this
    /// field safe: every project already on disk carries its pairs in `links`, and they go on
    /// showing without this set knowing anything about them.
    #[serde(default)]
    pub added: HashSet<u32>,
    /// Folder scanned for `.ldt` / `.ies` files, if the user has pointed at one.
    #[serde(default)]
    pub folder: String,
}

impl Wiring {
    /// Every block definition in `doc`, with its instance count — the left-hand list before
    /// curation. Sorted by name so the list does not reshuffle as the drawing is edited.
    pub fn block_rows(doc: &Document) -> Vec<BlockRow> {
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for d in &doc.dobjects {
            if let Geom::BlockRef(br) = &d.geom {
                *counts.entry(br.block).or_insert(0) += 1;
            }
        }
        let mut rows: Vec<BlockRow> = doc
            .blocks
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| BlockRow {
                id: i as u32,
                name: b.name.clone(),
                instances: counts.get(&(i as u32)).copied().unwrap_or(0),
            })
            .collect();
        rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        rows
    }

    /// The rows actually shown, i.e. [`Self::block_rows`] minus the curated-out ones.
    pub fn visible_rows(&self, doc: &Document) -> Vec<BlockRow> {
        Self::block_rows(doc).into_iter().filter(|r| !self.hidden.contains(&r.id)).collect()
    }

    /// WHERE THE FITTINGS GO, for one wired block — in METRES, which is the whole point.
    ///
    /// A `BlockRef`'s `insert` is in DRAWING units and a luminaire's position is a world (x, y) in
    /// metres. On a plan authored in millimetres those differ by a thousand, and nothing downstream
    /// would report the mistake: the fittings would simply be somewhere else, the lux grid would
    /// find no light, and the answer would look like a design problem rather than a unit one.
    ///
    /// Rotation carries across unscaled — a block turned 30° puts its fitting at 30°, which is what
    /// makes an asymmetric distribution point the way the drawing says.
    pub fn plan_placements(&self, doc: &Document, block: u32) -> Vec<Placement> {
        let k = doc.units.metres_per_unit;
        doc.dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                Geom::BlockRef(br) if br.block == block => Some(Placement {
                    block,
                    x: (br.insert.x * k) as f32,
                    y: (br.insert.y * k) as f32,
                    rotation_deg: br.rotation.to_degrees() as f32,
                }),
                _ => None,
            })
            .collect()
    }

    /// Every placement the current wiring calls for, across every wired block.
    ///
    /// A block that is HIDDEN is still placed if it is wired. Curation is about what the list
    /// shows, not about what the drawing gets — hiding a row to tidy the view must not silently
    /// pull two hundred fittings out of the calculation.
    pub fn all_placements(&self, doc: &Document) -> Vec<(Placement, String)> {
        let mut out = Vec::new();
        for (&block, profile) in &self.links {
            for p in self.plan_placements(doc, block) {
                out.push((p, profile.clone()));
            }
        }
        // Deterministic order — a HashMap iterates arbitrarily, and fittings that change id
        // between two runs of the same wiring would make every diff and every dump noise.
        out.sort_by(|a, b| {
            (a.0.block, a.0.x.to_bits(), a.0.y.to_bits())
                .cmp(&(b.0.block, b.0.x.to_bits(), b.0.y.to_bits()))
        });
        out
    }

    /// The wiring as BLOCK NAMES, for the sidecar.
    ///
    /// IDS ARE POSITIONS IN THE BLOCK TABLE, and positions do not survive a save and reopen: a
    /// re-import, or any edit that reorders the table, renumbers them. Persisting ids would
    /// silently re-pair the wiring to whatever now sits at those numbers — two hundred downlights
    /// landing on the chairs, with nothing to say why.
    ///
    /// This is the convention the sidecar already states for `wall_centerline` ("keyed by name …
    /// so it survives save/reopen even though ids are positional"), and the reason `lux_block_ies`
    /// was declared name-keyed years before anything filled it in.
    ///
    /// A link whose block no longer exists is dropped: the drawing is the authority on what blocks
    /// there are, and a pairing to a block nobody can see is not recoverable state.
    pub fn to_named(&self, doc: &Document) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        for (&id, profile) in &self.links {
            if let Some(b) = doc.blocks.blocks.get(id as usize) {
                out.insert(b.name.clone(), profile.clone());
            }
        }
        out
    }

    /// Curated-out blocks as NAMES, for the sidecar — same reasoning as [`Self::to_named`].
    pub fn hidden_named(&self, doc: &Document) -> Vec<String> {
        let mut v: Vec<String> = self
            .hidden
            .iter()
            .filter_map(|id| doc.blocks.blocks.get(*id as usize).map(|b| b.name.clone()))
            .collect();
        v.sort();
        v
    }

    /// Rebuild a wiring from the sidecar's names, resolving each against THIS document's block
    /// table. A name the drawing no longer has is dropped rather than guessed at.
    pub fn from_named(
        doc: &Document,
        links: &std::collections::BTreeMap<String, String>,
        hidden: &[String],
        folder: String,
    ) -> Self {
        let id_of = |name: &str| -> Option<u32> {
            doc.blocks
                .blocks
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(name))
                .map(|i| i as u32)
        };
        Self {
            links: links.iter().filter_map(|(n, p)| Some((id_of(n)?, p.clone()))).collect(),
            hidden: hidden.iter().filter_map(|n| id_of(n)).collect(),
            added: Default::default(),
            folder,
        }
    }
}

/// Scan `dir` for photometric files. Non-recursive, and it never fails loudly: an unreadable or
/// missing folder is an empty list, because a typo'd path must not be an error dialog in front of
/// a window whose other half still works.
pub fn scan_folder(dir: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir.trim().trim_matches('"')) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let is_photometry = p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("ldt") || x.eq_ignore_ascii_case("ies"));
        if !is_photometry {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
        out.push((stem.to_string(), p.to_string_lossy().into_owned()));
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}



// ── PREVIEWS ───────────────────────────────────────────────────────────────────────────────
//
// You pick a block because of what it LOOKS like and a fitting because of what it DOES, and the
// editor showed a name for each. `sdfwer x4` tells a person nothing about whether that is the
// downlight or the fire-alarm sounder.
//
// The three functions below are the whole of both previews, and they are pure and unit-tested for
// exactly that reason: everything the eye checks — is it the right shape, does it throw light
// downward or sideways — is decided here, and a painter that draws the wrong thing correctly is
// indistinguishable from one that draws the right thing badly.

/// A block's linework, fitted into the unit square `0..1` with its ASPECT PRESERVED and centred.
///
/// Returned in unit space rather than pixels so the caller can paint it into any rect, and so the
/// fitting can be tested without a painter. Y is returned MATHS-UP (larger y is further up); the
/// caller flips it, because that is a fact about screens rather than about the block.
///
/// Empty when the block has nothing drawable in it — a block of pure text or attributes, say. The
/// caller shows "nothing to draw" rather than an empty box that reads as a failure.
pub fn block_preview_paths(doc: &Document, block: u32) -> Vec<Vec<[f32; 2]>> {
    let Some(blk) = doc.blocks.get(block) else { return Vec::new() };
    let mut raw: Vec<Vec<[f32; 2]>> = Vec::new();
    for d in &blk.dobjects {
        // The DISPLAY flattener, so a nested block reference inside the definition is expanded
        // rather than silently dropped — a light fitting drawn as a housing block plus a lamp
        // block would otherwise preview as half of itself.
        for path in cad_solid::geom_display_outlines_scaled(&d.geom, doc, 1.0) {
            if path.len() >= 2 {
                raw.push(path.iter().map(|p| [p.x, p.y]).collect());
            }
        }
    }
    let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in raw.iter().flatten() {
        mnx = mnx.min(p[0]);
        mny = mny.min(p[1]);
        mxx = mxx.max(p[0]);
        mxy = mxy.max(p[1]);
    }
    let (w, h) = (mxx - mnx, mxy - mny);
    if raw.is_empty() || !(w.is_finite() && h.is_finite()) {
        return Vec::new();
    }
    // ONE SCALE FOR BOTH AXES. Fitting each axis independently would stretch a round downlight
    // into an ellipse and a 3 m batten into a square — the preview exists to say which is which.
    // A degenerate extent (a single horizontal line) still has to divide by something.
    let span = w.max(h).max(1e-9);
    let k = 1.0 / span;
    let (ox, oy) = (0.5 - 0.5 * w * k, 0.5 - 0.5 * h * k);
    raw.iter()
        .map(|p| p.iter().map(|q| [ox + (q[0] - mnx) * k, oy + (q[1] - mny) * k]).collect())
        .collect()
}

/// One plane of a photometric distribution as a polar curve, in the unit disc centred on `(0, 0)`.
///
/// `plane_deg` is the C-plane: 0 is the C0–C180 section, 90 the C90–C270. Radius is intensity
/// divided by the profile's PEAK, so the shape is comparable between fittings of wildly different
/// output — which is what the curve is read for. Angles run from nadir (straight down, `+y` here
/// negative) out to the last measured angle, both sides.
///
/// Y IS MATHS-UP AND NADIR POINTS AT `-y`, so a downlight's lobe hangs below the origin. The
/// caller maps that to the screen; getting it upside down would show every downlight as an uplight
/// and there would be nothing in the picture to say so.
pub fn polar_points(prof: &cad_light::IesProfile, plane_deg: f64) -> Vec<[f32; 2]> {
    let peak = prof.peak_candela();
    if peak <= 0.0 || prof.vertical_angles.is_empty() {
        return Vec::new();
    }
    let last = *prof.vertical_angles.last().unwrap_or(&0.0);
    let steps = 72;
    let mut out = Vec::with_capacity(steps * 2 + 1);
    // The far side first (C+180), swept back to nadir, then out along the near side — so the
    // result is one continuous polyline the caller can stroke in a single pass.
    for side in [(plane_deg + 180.0, -1.0_f64), (plane_deg, 1.0)] {
        let (c, sign) = side;
        let n = steps;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let g = if sign < 0.0 { last * (1.0 - t) } else { last * t };
            let r = prof.intensity(g, c) / peak;
            let a = g.to_radians();
            out.push([(sign * r * a.sin()) as f32, -(r * a.cos()) as f32]);
        }
    }
    out
}

/// The numbers printed beside the curve.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProfileFigures {
    /// Declared flux, or `None` when the file does not state one.
    pub lumens: Option<f64>,
    pub watts: Option<f64>,
    pub efficacy: Option<f64>,
    pub peak_candela: f64,
    /// FULL beam angle: the total spread within which intensity is at least half the peak, in the
    /// C0 plane. `None` when the file has too little of a table to say.
    pub beam_deg: Option<f64>,
}

/// The figures a lighting designer reads off a photometric file.
///
/// Every one of them is `Option` where the file may not state it, and none is invented. A fitting
/// whose `.ldt` declares no wattage has no efficacy — printing a plausible number there would be
/// worse than a blank, because a blank is obviously missing and 0 lm/W is obviously wrong while
/// "94 lm/W" is neither.
pub fn profile_figures(prof: &cad_light::IesProfile) -> ProfileFigures {
    let pos = |v: f64| (v > 0.0).then_some(v);
    let (lumens, watts) = (pos(prof.lumens), pos(prof.watts));
    ProfileFigures {
        lumens,
        watts,
        efficacy: match (lumens, watts) {
            (Some(l), Some(w)) => Some(l / w),
            _ => None,
        },
        peak_candela: prof.peak_candela(),
        beam_deg: beam_angle(prof),
    }
}

/// The full width at half maximum of the C0 plane, in degrees.
///
/// Measured OUTWARD FROM NADIR to the first crossing of half-peak, then doubled — the convention a
/// photometric datasheet quotes. A distribution that never falls to half within its measured range
/// (a bare lamp, an uplighter) has no beam angle in this sense, and says `None` rather than
/// reporting the edge of the table as though it were a beam edge.
fn beam_angle(prof: &cad_light::IesProfile) -> Option<f64> {
    let peak = prof.peak_candela();
    if peak <= 0.0 {
        return None;
    }
    let last = *prof.vertical_angles.last()?;
    if last <= 0.0 {
        return None;
    }
    let half = peak * 0.5;
    // Sampled rather than read off the table, so an unevenly spaced file is treated the same as an
    // evenly spaced one.
    let n = 360;
    let mut prev = prof.intensity(0.0, 0.0);
    if prev < half {
        return None; // the peak is not at nadir — a wall-washer; quoting a beam angle would lie
    }
    for i in 1..=n {
        let g = last * i as f64 / n as f64;
        let cur = prof.intensity(g, 0.0);
        if cur < half {
            // Linear crossing between the two samples.
            let g0 = last * (i - 1) as f64 / n as f64;
            let t = (prev - half) / (prev - cur).max(1e-12);
            return Some(2.0 * (g0 + (g - g0) * t));
        }
        prev = cur;
    }
    None
}

/// What the Light Editor window wants done, collected so the UI never borrows the document and
/// the luminaire store at the same time — the same shape `LightAction` uses next door.
#[derive(Default)]
pub struct EditorAction {
    /// Place the fittings for the current wiring.
    pub apply: bool,
    /// Rescan the photometry folder.
    pub rescan: bool,
    /// Import this file into the profile library, then wire it to the pending block.
    pub import_and_wire: Option<(String, u32)>,
    /// Remove every fitting this block placed.
    pub clear_block: Option<u32>,
    /// Open the file picker for a photometry folder.
    pub browse_folder: bool,
}

/// Colour for a wired row — the amber the sketch banner already uses for "this is live".
const WIRED: egui::Color32 = egui::Color32::from_rgb(255, 178, 60);


/// Which chooser is open, and what is highlighted inside it.
///
/// Highlighting is NOT adding. A person clicks a row to SEE it — that is the whole reason the
/// previews exist — and adds it deliberately with the button. Adding on click would mean browsing
/// a folder of two hundred fittings wired the last one you looked at.
#[derive(Default)]
pub struct Picker {
    pub blocks_open: bool,
    pub lights_open: bool,
    /// Highlighted in the BLOCK chooser: previewed, not yet added.
    pub block: Option<u32>,
    /// Highlighted in the LIGHT chooser.
    pub light: Option<String>,
}

/// Paint a block's linework into `rect`, fitted and centred.
///
/// The unit-space paths come from [`block_preview_paths`], which is where the fitting is decided
/// and tested; this only maps them onto the screen. Y is flipped here and nowhere else — the paths
/// are maths-up, screens are not.
fn paint_block(painter: &egui::Painter, rect: egui::Rect, paths: &[Vec<[f32; 2]>], col: egui::Color32) {
    let pad = 6.0;
    let inner = egui::Rect::from_min_max(
        rect.min + egui::vec2(pad, pad),
        rect.max - egui::vec2(pad, pad),
    );
    let side = inner.width().min(inner.height()).max(1.0);
    let org = inner.center() - egui::vec2(side * 0.5, side * 0.5);
    let stroke = egui::Stroke::new(1.2, col);
    for path in paths {
        let pts: Vec<egui::Pos2> = path
            .iter()
            .map(|p| egui::pos2(org.x + p[0] * side, org.y + (1.0 - p[1]) * side))
            .collect();
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts, stroke));
        }
    }
}

/// Paint a photometric distribution into `rect`: the C0 and C90 planes, with the fitting at the
/// top centre and the lobe hanging below it.
fn paint_polar(painter: &egui::Painter, rect: egui::Rect, prof: &cad_light::IesProfile) {
    let pad = 8.0;
    let inner = egui::Rect::from_min_max(
        rect.min + egui::vec2(pad, pad),
        rect.max - egui::vec2(pad, pad),
    );
    // The origin is the FITTING, near the top — the curve hangs down from it, which is how a
    // photometric diagram is drawn and how a person reads "this one throws light downward".
    let r = (inner.width() * 0.5).min(inner.height() * 0.86).max(1.0);
    let org = egui::pos2(inner.center().x, inner.top() + inner.height() * 0.07 + 2.0);
    let faint = egui::Color32::from_gray(90);

    // Reference rings at 50% and 100% of peak, and the nadir axis — a curve with no scale behind
    // it is a shape, not a measurement.
    for f in [0.5_f32, 1.0] {
        painter.circle_stroke(org, r * f, egui::Stroke::new(0.7, faint));
    }
    painter.line_segment(
        [org, egui::pos2(org.x, org.y + r)],
        egui::Stroke::new(0.7, faint),
    );
    painter.line_segment(
        [egui::pos2(org.x - r, org.y), egui::pos2(org.x + r, org.y)],
        egui::Stroke::new(0.7, faint),
    );

    // C0 solid, C90 dashed — the two sections a datasheet prints. A rotationally symmetric fitting
    // draws them on top of each other, which is itself the useful reading.
    for (plane, col, width) in [
        (0.0_f64, WIRED, 1.6_f32),
        (90.0, egui::Color32::from_rgb(120, 190, 255), 1.2),
    ] {
        let pts: Vec<egui::Pos2> = polar_points(prof, plane)
            .into_iter()
            .map(|p| egui::pos2(org.x + p[0] * r, org.y - p[1] * r))
            .collect();
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts, egui::Stroke::new(width, col)));
        }
    }
}

/// The numbers, as rows under the curve.
fn figures_ui(ui: &mut egui::Ui, prof: &cad_light::IesProfile) {
    let f = profile_figures(prof);
    let row = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(k).small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(v).small());
            });
        });
    };
    // A MISSING FIGURE READS AS MISSING. `—` where the file states nothing, never a stand-in
    // number: a blank is obviously absent, and a plausible figure is not obviously anything.
    let or_dash = |v: Option<f64>, unit: &str, dp: usize| match v {
        Some(x) => format!("{x:.dp$} {unit}"),
        None => "—".to_string(),
    };
    row(ui, "Flux", or_dash(f.lumens, "lm", 0));
    row(ui, "Power", or_dash(f.watts, "W", 1));
    row(ui, "Efficacy", or_dash(f.efficacy, "lm/W", 0));
    row(ui, "Peak", format!("{:.0} cd", f.peak_candela));
    row(ui, "Beam", or_dash(f.beam_deg, "°", 0));
}

/// The Light Editor window.
///
/// Takes what it needs rather than `&mut CadApp`, so the borrow checker is satisfied without the
/// whole app in scope and the layout can be reasoned about on its own.
///
/// THE MAIN WINDOW IS THE PAIRING, and nothing else. It used to be two lists side by side, one of
/// every block in the drawing — hundreds, on a real plan, nearly all of them doors and furniture —
/// and one of every photometric file, both showing bare names. `sdfwer x4` says nothing about
/// whether that is the downlight or the fire-alarm sounder.
///
/// So the two lists moved into CHOOSERS behind `＋ Add`, where each has room for a preview, and
/// what remains here is the short list that matters: which block gets which fitting.
#[allow(clippy::too_many_arguments)]
pub fn window_ui(
    ctx: &egui::Context,
    open: &mut bool,
    wiring: &mut Wiring,
    pick: &mut Option<u32>,
    picker: &mut Picker,
    doc: &Document,
    rows: &[BlockRow],
    profiles: &HashMap<String, cad_light::IesProfile>,
    loaded: &[String],
    scanned: &[(String, String)],
    placed_counts: &HashMap<u32, usize>,
) -> EditorAction {
    let mut act = EditorAction::default();
    let wired_total: usize =
        wiring.links.keys().map(|b| placed_counts.get(b).copied().unwrap_or(0)).sum();

    egui::Window::new("Light Editor")
        .open(open)
        .default_width(560.0)
        .default_height(360.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Pair a block in the drawing with a photometric file — one fitting is placed \
                     at every instance of it.",
                )
                .small()
                .weak(),
            );
            ui.separator();

            // ---- THE PAIRS ---------------------------------------------------------------
            let listed: Vec<&BlockRow> = rows
                .iter()
                .filter(|r| wiring.added.contains(&r.id) || wiring.links.contains_key(&r.id))
                .collect();
            ui.label(egui::RichText::new("WIRED PAIRS").small().strong());
            egui::ScrollArea::vertical().id_source("le_pairs").max_height(220.0).show(ui, |ui| {
                if listed.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Nothing paired yet — ＋ Add block to choose one from the drawing.",
                        )
                        .small()
                        .weak(),
                    );
                }
                for r in &listed {
                    let linked = wiring.links.get(&r.id).cloned();
                    ui.horizontal(|ui| {
                        let mut t = egui::RichText::new(format!("{}  ×{}", r.name, r.instances));
                        if linked.is_some() {
                            t = t.color(WIRED);
                        }
                        if ui
                            .selectable_label(*pick == Some(r.id), t)
                            .on_hover_text("Select, then ＋ Add light to give it a fitting")
                            .clicked()
                        {
                            *pick = if *pick == Some(r.id) { None } else { Some(r.id) };
                        }
                        match &linked {
                            Some(p) => {
                                ui.label(egui::RichText::new("⟶").small().weak());
                                ui.label(egui::RichText::new(p).small().color(WIRED));
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("⟶  no fitting yet").small().weak(),
                                );
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // REMOVE FROM THE LIST, NOT FROM THE DRAWING. The block table is never
                            // touched here; this only stops the editor listing it.
                            if ui
                                .small_button("✖")
                                .on_hover_text(
                                    "Remove this pair from the list. The drawing and its block \
                                     table are not changed",
                                )
                                .clicked()
                            {
                                wiring.added.remove(&r.id);
                                wiring.links.remove(&r.id);
                                if *pick == Some(r.id) {
                                    *pick = None;
                                }
                            }
                            if linked.is_some()
                                && ui
                                    .small_button("🗑")
                                    .on_hover_text("Remove the fittings this block placed")
                                    .clicked()
                            {
                                act.clear_block = Some(r.id);
                            }
                        });
                    });
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("＋ Add block")
                    .on_hover_text("Choose a block from the drawing, with a preview of it")
                    .clicked()
                {
                    picker.blocks_open = true;
                    picker.block = None;
                }
                let can_light = pick.is_some();
                if ui
                    .add_enabled(can_light, egui::Button::new("＋ Add light"))
                    .on_hover_text(if can_light {
                        "Choose a fitting for the selected pair, with its photometric curve"
                    } else {
                        "Select a pair above first"
                    })
                    .clicked()
                {
                    picker.lights_open = true;
                    picker.light = None;
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                let n = wiring.links.len();
                if ui
                    .add_enabled(
                        n > 0,
                        egui::Button::new(
                            egui::RichText::new(format!("Place {wired_total} fitting(s)")).strong()),
                    )
                    .on_hover_text(
                        "Place one fitting per instance of every wired block. Re-applying replaces \
                         what these blocks placed before — it never doubles them, and never touches \
                         a fitting you placed by hand.",
                    )
                    .clicked()
                {
                    act.apply = true;
                }
                ui.label(egui::RichText::new(format!("{n} block(s) wired")).small().weak());
            });
        });

    block_chooser(ctx, wiring, pick, picker, doc, rows);
    light_chooser(ctx, wiring, pick, picker, profiles, loaded, scanned, &mut act);
    act
}

/// ＋ Add block — every block in the drawing, with a preview of the highlighted one.
fn block_chooser(
    ctx: &egui::Context,
    wiring: &mut Wiring,
    pick: &mut Option<u32>,
    picker: &mut Picker,
    doc: &Document,
    rows: &[BlockRow],
) {
    if !picker.blocks_open {
        return;
    }
    let mut open = true;
    let mut add: Option<u32> = None;
    egui::Window::new("Choose a block")
        .id(egui::Id::new("le_block_chooser"))
        .open(&mut open)
        .default_width(480.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.columns(2, |col| {
                col[0].label(egui::RichText::new("IN THE DRAWING").small().strong());
                egui::ScrollArea::vertical().id_source("le_bc").max_height(260.0).show(
                    &mut col[0],
                    |ui| {
                        // Blocks already paired are still listed, greyed — seeing that the one you
                        // are looking for is already in is more useful than wondering where it is.
                        for r in rows {
                            let already = wiring.added.contains(&r.id)
                                || wiring.links.contains_key(&r.id);
                            let mut t = egui::RichText::new(format!("{}  ×{}", r.name, r.instances));
                            if already {
                                t = t.weak();
                            }
                            if ui.selectable_label(picker.block == Some(r.id), t).clicked() {
                                picker.block = Some(r.id);
                            }
                        }
                        if rows.is_empty() {
                            ui.label(
                                egui::RichText::new("No blocks in this drawing.").small().weak(),
                            );
                        }
                    },
                );

                col[1].label(egui::RichText::new("PREVIEW").small().strong());
                let (_, painter) = col[1].allocate_painter(
                    egui::vec2(col[1].available_width(), 200.0),
                    egui::Sense::hover(),
                );
                let rect = painter.clip_rect();
                painter.rect_filled(rect, 2.0, egui::Color32::from_gray(24));
                match picker.block {
                    Some(id) => {
                        let paths = block_preview_paths(doc, id);
                        if paths.is_empty() {
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "nothing to draw",
                                egui::FontId::proportional(12.0),
                                egui::Color32::from_gray(120),
                            );
                        } else {
                            paint_block(&painter, rect, &paths, egui::Color32::from_gray(210));
                        }
                    }
                    None => {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "pick a block to see it",
                            egui::FontId::proportional(12.0),
                            egui::Color32::from_gray(120),
                        );
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let sel = picker.block;
                if ui
                    .add_enabled(sel.is_some(), egui::Button::new("Add"))
                    .clicked()
                {
                    add = sel;
                }
                if ui.button("Cancel").clicked() {
                    picker.blocks_open = false;
                }
            });
        });
    if let Some(id) = add {
        wiring.added.insert(id);
        // Selected on the way out, so ＋ Add light is immediately available for the block just
        // added — which is what somebody adding a block is about to want.
        *pick = Some(id);
        picker.blocks_open = false;
    }
    if !open {
        picker.blocks_open = false;
    }
}

/// ＋ Add light — the photometry on offer, with the highlighted one's distribution curve.
#[allow(clippy::too_many_arguments)]
fn light_chooser(
    ctx: &egui::Context,
    wiring: &mut Wiring,
    pick: &mut Option<u32>,
    picker: &mut Picker,
    profiles: &HashMap<String, cad_light::IesProfile>,
    loaded: &[String],
    scanned: &[(String, String)],
    act: &mut EditorAction,
) {
    if !picker.lights_open {
        return;
    }
    let mut open = true;
    let mut chosen: Option<String> = None;
    egui::Window::new("Choose a light")
        .id(egui::Id::new("le_light_chooser"))
        .open(&mut open)
        .default_width(560.0)
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut wiring.folder)
                        .desired_width(220.0)
                        .hint_text("folder of .ldt / .ies files"),
                );
                if ui.small_button("…").on_hover_text("Choose a folder").clicked() {
                    act.browse_folder = true;
                }
                if ui.small_button("scan").on_hover_text("Rescan the folder").clicked() {
                    act.rescan = true;
                }
            });
            ui.separator();
            ui.columns(2, |col| {
                col[0].label(egui::RichText::new("AVAILABLE").small().strong());
                egui::ScrollArea::vertical().id_source("le_lc").max_height(240.0).show(
                    &mut col[0],
                    |ui| {
                        for name in loaded {
                            if ui
                                .selectable_label(
                                    picker.light.as_deref() == Some(name.as_str()),
                                    egui::RichText::new(name),
                                )
                                .clicked()
                            {
                                picker.light = Some(name.clone());
                            }
                        }
                        // Found on disk but not imported. Offered, because a folder of two hundred
                        // files is the point — but there is no photometry to preview until it is
                        // read, so these say so rather than showing an empty diagram.
                        let fresh: Vec<&(String, String)> =
                            scanned.iter().filter(|(n, _)| !loaded.contains(n)).collect();
                        if !fresh.is_empty() {
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} in folder, not yet imported", fresh.len()))
                                    .small().weak(),
                            );
                            for (name, path) in fresh {
                                if ui
                                    .selectable_label(false, egui::RichText::new(name).weak())
                                    .on_hover_text(format!("{path}\nAdds it and reads it in"))
                                    .clicked()
                                {
                                    if let Some(b) = *pick {
                                        act.import_and_wire = Some((path.clone(), b));
                                        *pick = None;
                                        picker.lights_open = true; // closed below, once
                                    }
                                }
                            }
                        }
                        if loaded.is_empty() && scanned.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Nothing yet — point at a folder above, or import a file in \
                                     the SIMLUX panel.",
                                )
                                .small()
                                .weak(),
                            );
                        }
                    },
                );

                col[1].label(egui::RichText::new("DISTRIBUTION").small().strong());
                let (_, painter) = col[1].allocate_painter(
                    egui::vec2(col[1].available_width(), 180.0),
                    egui::Sense::hover(),
                );
                let rect = painter.clip_rect();
                painter.rect_filled(rect, 2.0, egui::Color32::from_gray(24));
                match picker.light.as_ref().and_then(|n| profiles.get(n)) {
                    Some(p) => paint_polar(&painter, rect, p),
                    None => { painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "pick a fitting to see its curve",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(120),
                    ); }
                }
                if let Some(p) = picker.light.as_ref().and_then(|n| profiles.get(n)) {
                    figures_ui(&mut col[1], p);
                    col[1].label(
                        egui::RichText::new("C0 amber · C90 blue · rings 50% and 100% of peak")
                            .small()
                            .weak(),
                    );
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let sel = picker.light.clone();
                if ui.add_enabled(sel.is_some(), egui::Button::new("Add")).clicked() {
                    chosen = sel;
                }
                if ui.button("Cancel").clicked() {
                    picker.lights_open = false;
                }
            });
        });
    if let Some(name) = chosen {
        if let Some(b) = *pick {
            wiring.links.insert(b, name);
            wiring.added.insert(b);
        }
        picker.lights_open = false;
    }
    if act.import_and_wire.is_some() {
        picker.lights_open = false;
    }
    if !open {
        picker.lights_open = false;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::{Block, BlockRef, DObject, Line, Vec2, MAX_BLOCK_PARAMS};


    fn named_block(name: &str) -> Block {
        Block {
            name: name.into(), base: Vec2::ZERO, dobjects: Vec::new(),
            smart: false, params: Vec::new(), cut_edges: Vec::new(),
        }
    }
    fn block_ref(id: u32, at: Vec2, rot: f64) -> Geom {
        Geom::BlockRef(BlockRef {
            block: id, insert: at, scale: 1.0, scale_y: 1.0, rotation: rot, mirror_x: false,
            param_values: [0.0; MAX_BLOCK_PARAMS],
        })
    }

    /// A drawing with one `DOWNLIGHT` definition placed `n` times, plus an unrelated `CHAIR`.
    fn plan(n: usize) -> Document {
        let mut doc = Document::default();
        doc.dobjects.clear();
        let mut d = Block {
            name: "DOWNLIGHT".into(), base: Vec2::ZERO, dobjects: Vec::new(),
            smart: false, params: Vec::new(), cut_edges: Vec::new(),
        };
        d.dobjects.push(DObject::new(Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(0.3, 0.0) })));
        let down = doc.blocks.add(d);
        let chair = doc.blocks.add(Block {
            name: "CHAIR".into(), base: Vec2::ZERO, dobjects: Vec::new(),
            smart: false, params: Vec::new(), cut_edges: Vec::new(),
        });
        for i in 0..n {
            doc.push(DObject::new(block_ref(down, Vec2::new(i as f64 * 2.0, 3.0), 0.0)));
        }
        doc.push(DObject::new(block_ref(chair, Vec2::new(50.0, 50.0), 0.0)));
        doc
    }

    #[test]
    fn the_list_is_block_definitions_with_their_instance_counts() {
        let doc = plan(5);
        let rows = Wiring::block_rows(&doc);
        let down = rows.iter().find(|r| r.name == "DOWNLIGHT").expect("the fitting block");
        assert_eq!(down.instances, 5, "five copies placed, so the row says five");
        let chair = rows.iter().find(|r| r.name == "CHAIR").expect("the furniture block");
        assert_eq!(chair.instances, 1);
        // Sorted by name, so the list does not reshuffle under the user as the drawing changes.
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|s| s.to_lowercase());
        assert_eq!(names, sorted, "the list must be in a stable order");
    }

    /// ONE WIRING, EVERY INSTANCE — the labour this feature exists to save.
    #[test]
    fn wiring_one_block_places_a_fitting_at_every_instance_of_it() {
        let doc = plan(200);
        let down = Wiring::block_rows(&doc).iter().find(|r| r.name == "DOWNLIGHT").unwrap().id;
        let mut w = Wiring::default();
        w.links.insert(down, "PULSE-14".into());

        let all = w.all_placements(&doc);
        assert_eq!(all.len(), 200, "one wiring must place two hundred fittings");
        assert!(all.iter().all(|(_, p)| p == "PULSE-14"), "all of them get the wired profile");
        // …and nothing else in the drawing is touched.
        assert!(all.iter().all(|(p, _)| p.block == down), "an unwired block got fittings");
    }

    /// THE UNIT TRAP. A block's insert is in DRAWING units; a luminaire's position is in METRES.
    /// On a millimetre plan those differ by a thousand, and every downstream symptom — no light in
    /// the grid, fittings off the edge of the room — reads as a design problem rather than a unit
    /// one, because nothing errors.
    #[test]
    fn placements_are_in_metres_whatever_the_drawing_is_in() {
        let mut doc = plan(1);
        let down = Wiring::block_rows(&doc).iter().find(|r| r.name == "DOWNLIGHT").unwrap().id;
        let w = Wiring::default();

        // Metres: the insert at x = 0 is at x = 0, and a second at 2.0 units is 2.0 m.
        doc.units = cad_kernel::DocUnits::new(cad_kernel::DocUnits::M, cad_kernel::UnitSource::User);
        let m = w.plan_placements(&doc, down);
        assert!((m[0].x - 0.0).abs() < 1e-4, "metres: {}", m[0].x);

        // The SAME drawing declared in millimetres: 2000 units is 2 m, so a point at 2.0 units is
        // 2 mm — a thousandth of where the metre reading put it.
        doc.units = cad_kernel::DocUnits::new(cad_kernel::DocUnits::MM, cad_kernel::UnitSource::User);
        doc.dobjects.clear();
        doc.push(DObject::new(block_ref(down, Vec2::new(2000.0, 0.0), 0.0)));
        let mm = w.plan_placements(&doc, down);
        assert!(
            (mm[0].x - 2.0).abs() < 1e-3,
            "a 2000-unit insert in a millimetre drawing must be 2 m, got {}",
            mm[0].x,
        );
    }

    /// Rotation carries, so an asymmetric distribution points the way the drawing says.
    #[test]
    fn a_rotated_block_places_a_rotated_fitting() {
        let mut doc = plan(0);
        let down = Wiring::block_rows(&doc).iter().find(|r| r.name == "DOWNLIGHT").unwrap().id;
        doc.push(DObject::new(block_ref(down, Vec2::new(1.0, 1.0), std::f64::consts::FRAC_PI_6)));
        let p = Wiring::default().plan_placements(&doc, down);
        assert!((p[0].rotation_deg - 30.0).abs() < 1e-3, "expected 30°, got {}", p[0].rotation_deg);
    }

    /// CURATION IS ABOUT THE LIST, NOT THE DRAWING. Taking a row out of the window must not
    /// quietly pull that block's two hundred fittings out of the calculation.
    ///
    /// Asserted on `hidden`, the curation older projects carry, AND on `added`, which is how the
    /// ＋ Add flow curates. Both have to leave the placements alone.
    #[test]
    fn curating_the_list_does_not_touch_the_placements() {
        let doc = plan(4);
        let down = Wiring::block_rows(&doc).iter().find(|r| r.name == "DOWNLIGHT").unwrap().id;
        let mut w = Wiring::default();
        w.links.insert(down, "PULSE-14".into());
        w.hidden.insert(down);
        assert!(
            !w.visible_rows(&doc).iter().any(|r| r.id == down),
            "a hidden block must not be listed",
        );
        assert_eq!(
            w.all_placements(&doc).len(), 4,
            "hiding a row deleted its fittings — curation is not deletion",
        );

        // AND A PROJECT FROM BEFORE ＋ Add STILL PLACES. Its pairs are in `links` with `added`
        // empty, so if placement ever went through `added` every one of those projects would
        // silently stop placing anything — the field would look harmless and be a data loss.
        let mut w = Wiring::default();
        w.links.insert(down, "PULSE-14".into());
        assert!(w.added.is_empty(), "the fixture is a project from before ＋ Add existed");
        assert_eq!(
            w.all_placements(&doc).len(), 4,
            "an older project's wiring stopped placing its fittings",
        );
    }

    /// The same wiring must produce the same order twice, or every dump and diff is noise.
    #[test]
    fn placement_order_is_deterministic() {
        let doc = plan(30);
        let down = Wiring::block_rows(&doc).iter().find(|r| r.name == "DOWNLIGHT").unwrap().id;
        let mut w = Wiring::default();
        w.links.insert(down, "A".into());
        let a: Vec<_> = w.all_placements(&doc).iter().map(|(p, _)| (p.x, p.y)).collect();
        let b: Vec<_> = w.all_placements(&doc).iter().map(|(p, _)| (p.x, p.y)).collect();
        assert_eq!(a, b, "two runs of one wiring disagreed on order");
    }

    #[test]
    fn a_missing_photometry_folder_is_an_empty_list_not_an_error() {
        assert!(scan_folder("Z:/no/such/folder/anywhere").is_empty());
        assert!(scan_folder("").is_empty());
    }

    /// THE WIRING SURVIVES A REOPEN, AND SURVIVES THE BLOCK TABLE BEING REORDERED.
    ///
    /// This is why the sidecar is keyed by NAME. A block id is a POSITION in the table, so a
    /// re-import — or any edit that inserts a definition ahead of another — renumbers them. Had
    /// the ids been persisted, reopening would re-pair the wiring to whatever now sits at those
    /// numbers: two hundred downlights placed on the chairs, silently, with the layout looking
    /// plausible until someone checked what was where.
    #[test]
    fn the_wiring_survives_a_reordered_block_table() {
        let mut before = Document::default();
        before.blocks.add(named_block("CHAIR"));
        let down = before.blocks.add(named_block("DOWNLIGHT"));
        let mut w = Wiring::default();
        w.links.insert(down, "PULSE-14".into());
        w.hidden.insert(0); // CHAIR curated out

        let saved = w.to_named(&before);
        let saved_hidden = w.hidden_named(&before);
        assert_eq!(saved.get("DOWNLIGHT").map(String::as_str), Some("PULSE-14"));
        assert_eq!(saved_hidden, vec!["CHAIR".to_string()]);

        // Reopened with the table in a DIFFERENT order — DOWNLIGHT is now id 0, CHAIR id 2.
        let mut after = Document::default();
        let down_after = after.blocks.add(named_block("DOWNLIGHT"));
        after.blocks.add(named_block("DESK"));
        let chair_after = after.blocks.add(named_block("CHAIR"));
        assert_ne!(down, down_after, "the fixture must actually renumber, or it proves nothing");

        let restored = Wiring::from_named(&after, &saved, &saved_hidden, String::new());
        assert_eq!(
            restored.links.get(&down_after).map(String::as_str),
            Some("PULSE-14"),
            "the fitting was re-paired to the wrong block on reopen",
        );
        assert!(!restored.links.contains_key(&chair_after), "CHAIR must not be wired");
        assert!(restored.hidden.contains(&chair_after), "the curation followed the wrong block");
    }

    /// A block the drawing no longer has is DROPPED rather than guessed at. The drawing is the
    /// authority on what blocks exist; a pairing to one nobody can see is not recoverable state.
    #[test]
    fn a_wiring_for_a_deleted_block_is_dropped_on_reopen() {
        let mut doc = Document::default();
        doc.blocks.add(named_block("DOWNLIGHT"));
        let mut saved = std::collections::BTreeMap::new();
        saved.insert("DOWNLIGHT".to_string(), "PULSE".to_string());
        saved.insert("A-BLOCK-THAT-WENT-AWAY".to_string(), "OTHER".to_string());

        let w = Wiring::from_named(&doc, &saved, &[], String::new());
        assert_eq!(w.links.len(), 1, "the vanished block was resurrected as some other id");
        assert!(w.links.values().any(|v| v == "PULSE"));
    }

    /// Round-trip with no reordering at all — the ordinary case, and the one that would hide a
    /// translation that simply dropped everything.
    #[test]
    fn an_unchanged_document_round_trips_the_wiring_exactly() {
        let mut doc = Document::default();
        let a = doc.blocks.add(named_block("DOWNLIGHT"));
        let b = doc.blocks.add(named_block("TRACK"));
        let mut w = Wiring::default();
        w.links.insert(a, "PULSE".into());
        w.links.insert(b, "SPOT".into());
        w.folder = "D:/photometry".into();

        let back = Wiring::from_named(&doc, &w.to_named(&doc), &w.hidden_named(&doc), w.folder.clone());
        assert_eq!(back.links.len(), 2);
        assert_eq!(back.links.get(&a).map(String::as_str), Some("PULSE"));
        assert_eq!(back.links.get(&b).map(String::as_str), Some("SPOT"));
        assert_eq!(back.folder, "D:/photometry");
    }
}

/// THE PREVIEWS ARE READ, SO THEY HAVE TO BE RIGHT.
///
/// You pick a block because of what it looks like and a fitting because of what it does. Both
/// previews are decided by three pure functions, and they are tested rather than eyeballed because
/// a painter that draws the wrong thing correctly looks exactly like one that draws the right
/// thing badly.
#[cfg(test)]
mod previews {
    use super::*;
    use cad_kernel::{Block, Circle, DObject, Line, Vec2};

    fn doc_with_block(name: &str, geoms: Vec<Geom>) -> Document {
        let mut d = Document::default();
        d.blocks.add(Block {
            name: name.into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: geoms.into_iter().map(DObject::new).collect(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        d
    }

    fn bounds(paths: &[Vec<[f32; 2]>]) -> (f32, f32, f32, f32) {
        let (mut a, mut b, mut c, mut e) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in paths.iter().flatten() {
            a = a.min(p[0]);
            b = b.min(p[1]);
            c = c.max(p[0]);
            e = e.max(p[1]);
        }
        (a, b, c, e)
    }

    /// A BLOCK FITS ITS BOX, whatever size it was drawn at. Two blocks a thousand times apart in
    /// scale must preview the same size, or the list is a row of dots and one enormous smear.
    #[test]
    fn a_block_is_fitted_into_the_unit_square_at_any_drawn_scale() {
        for side in [0.5_f64, 500.0, 500_000.0] {
            let d = doc_with_block(
                "B",
                vec![Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(side, side) })],
            );
            let paths = block_preview_paths(&d, 0);
            assert!(!paths.is_empty(), "a line block previewed as nothing at scale {side}");
            let (mnx, mny, mxx, mxy) = bounds(&paths);
            assert!(
                mnx >= -1e-4 && mny >= -1e-4 && mxx <= 1.0 + 1e-4 && mxy <= 1.0 + 1e-4,
                "at scale {side} the preview left the unit square: {mnx}..{mxx}, {mny}..{mxy}",
            );
        }
    }

    /// AND IT KEEPS ITS SHAPE. Fitting each axis on its own would stretch a round downlight into
    /// an ellipse and squash a 3 m batten into a square — the two things the preview exists to
    /// tell apart.
    #[test]
    fn a_wide_block_stays_wide() {
        // A 10 x 1 batten.
        let d = doc_with_block(
            "BATTEN",
            vec![
                Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }),
                Geom::Line(Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 1.0) }),
                Geom::Line(Line { a: Vec2::new(10.0, 1.0), b: Vec2::new(0.0, 1.0) }),
                Geom::Line(Line { a: Vec2::new(0.0, 1.0), b: Vec2::new(0.0, 0.0) }),
            ],
        );
        let (mnx, mny, mxx, mxy) = bounds(&block_preview_paths(&d, 0));
        let (w, h) = (mxx - mnx, mxy - mny);
        assert!(
            (w / h - 10.0).abs() < 0.1,
            "a 10:1 batten previewed at {:.2}:1 — the aspect was not preserved",
            w / h,
        );
        assert!((w - 1.0).abs() < 1e-3, "the long axis should fill the box, got {w}");
    }

    /// A BLOCK WITH NOTHING DRAWABLE PREVIEWS AS NOTHING, and says so by being empty rather than
    /// by drawing an empty box that reads as a broken preview.
    #[test]
    fn an_empty_block_previews_as_nothing() {
        let d = doc_with_block("EMPTY", Vec::new());
        assert!(block_preview_paths(&d, 0).is_empty());
        // …and an id that names no block at all must not panic.
        assert!(block_preview_paths(&d, 999).is_empty());
    }

    /// A CIRCLE PREVIEWS AS A CIRCLE — the flattener is actually being called, rather than only
    /// the straight-line geometry a naive version would handle.
    #[test]
    fn a_round_block_previews_round() {
        let d = doc_with_block(
            "DOWNLIGHT",
            vec![Geom::Circle(Circle { center: Vec2::new(3.0, 3.0), radius: 2.0 })],
        );
        let paths = block_preview_paths(&d, 0);
        let (mnx, mny, mxx, mxy) = bounds(&paths);
        assert!(
            ((mxx - mnx) - (mxy - mny)).abs() < 1e-3,
            "a circle previewed {:.3} wide and {:.3} tall",
            mxx - mnx, mxy - mny,
        );
        assert!(paths.iter().map(|p| p.len()).sum::<usize>() > 8, "a circle needs more than a box");
    }

    // ── THE PHOTOMETRIC CURVE ──────────────────────────────────────────────────────────────

    fn downlight() -> cad_light::IesProfile {
        let vertical_angles: Vec<f64> = (0..=18).map(|i| i as f64 * 5.0).collect();
        let candela: Vec<f64> =
            vertical_angles.iter().map(|g| 1000.0 * g.to_radians().cos().max(0.0)).collect();
        cad_light::IesProfile {
            name: "d".into(),
            photometry: cad_light::PhotometryType::C,
            lumens: 3140.0,
            multiplier: 1.0,
            vertical_angles,
            horizontal_angles: vec![0.0],
            candela: vec![candela],
            watts: 28.0,
            width: 0.0,
            length: 0.0,
            height: 0.0,
            luminous_length: 0.0,
            luminous_width: 0.0,
        }
    }

    /// A DOWNLIGHT'S LOBE HANGS DOWN. If nadir ever ends up at `+y` every downlight in the library
    /// previews as an uplighter, and there is nothing in the picture to say so — which is exactly
    /// the kind of wrong a drawing routine gets away with.
    #[test]
    fn a_downlight_points_down() {
        let pts = polar_points(&downlight(), 0.0);
        assert!(!pts.is_empty(), "a downlight produced no curve");
        let lowest = pts.iter().cloned().fold([0.0_f32, 0.0], |a, b| if b[1] < a[1] { b } else { a });
        assert!(lowest[1] < -0.9, "the peak of the lobe is at y = {}, not below", lowest[1]);
        assert!(
            pts.iter().all(|p| p[1] <= 1e-6),
            "part of a downlight's curve was drawn ABOVE the fitting",
        );
    }

    /// THE CURVE IS NORMALISED TO THE PEAK, so a 500 lm fitting and a 50,000 lm one are compared
    /// on shape rather than on which is bigger. Every point inside the unit disc.
    #[test]
    fn the_curve_is_normalised_to_the_peak() {
        for scale in [1.0_f64, 1000.0] {
            let mut p = downlight();
            for row in &mut p.candela {
                for c in row.iter_mut() {
                    *c *= scale;
                }
            }
            let pts = polar_points(&p, 0.0);
            let far = pts.iter().fold(0.0_f32, |m, q| m.max((q[0] * q[0] + q[1] * q[1]).sqrt()));
            assert!(
                (far - 1.0).abs() < 1e-3,
                "at {scale}x the curve reached {far}, not the unit disc",
            );
        }
    }

    /// A PROFILE WITH NO PHOTOMETRY DRAWS NOTHING rather than a NaN-shaped smear.
    #[test]
    fn a_profile_with_no_output_has_no_curve() {
        let mut p = downlight();
        p.candela = vec![vec![0.0; p.vertical_angles.len()]];
        assert!(polar_points(&p, 0.0).is_empty());
    }

    /// THE FIGURES ARE THE FILE'S, and the derived one is derived.
    #[test]
    fn the_figures_come_off_the_file() {
        let f = profile_figures(&downlight());
        assert_eq!(f.lumens, Some(3140.0));
        assert_eq!(f.watts, Some(28.0));
        let e = f.efficacy.expect("both flux and power are declared");
        assert!((e - 3140.0 / 28.0).abs() < 1e-9, "efficacy came out {e}");
        assert!((f.peak_candela - 1000.0).abs() < 1e-6);
    }

    /// NOTHING IS INVENTED. A file that declares no wattage has no efficacy — a blank is obviously
    /// missing, where a plausible number is not obviously anything.
    #[test]
    fn a_file_that_states_no_power_reports_no_efficacy() {
        let mut p = downlight();
        p.watts = 0.0;
        let f = profile_figures(&p);
        assert_eq!(f.watts, None);
        assert_eq!(f.efficacy, None, "an efficacy was invented from a missing wattage");
        assert_eq!(f.lumens, Some(3140.0), "…but the flux it DOES state is still reported");
    }

    /// THE BEAM ANGLE IS THE HALF-PEAK WIDTH. A perfect cosine downlight falls to half at 60° off
    /// nadir, so its full beam angle is 120° — a number that can be checked against the maths
    /// rather than against whatever the code happens to produce.
    #[test]
    fn the_beam_angle_is_the_full_width_at_half_peak() {
        let b = profile_figures(&downlight()).beam_deg.expect("a downlight has a beam angle");
        assert!((b - 120.0).abs() < 2.0, "a cosine downlight's beam came out {b:.1}°, not 120°");
    }

    /// AND A FITTING WITH NO BEAM SAYS SO. A distribution that never falls to half within its
    /// measured range has no beam angle in this sense, and reporting the edge of the table as
    /// though it were a beam edge would be a made-up number in a datasheet-shaped box.
    #[test]
    fn a_distribution_with_no_half_peak_point_reports_no_beam() {
        let mut p = downlight();
        // Uniform in every measured direction — a bare lamp.
        p.candela = vec![vec![1000.0; p.vertical_angles.len()]];
        assert_eq!(profile_figures(&p).beam_deg, None);
    }
}
