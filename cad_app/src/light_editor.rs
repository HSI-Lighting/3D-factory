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
    #[serde(default)]
    pub hidden: HashSet<u32>,
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

/// The Light Editor window.
///
/// Takes what it needs rather than `&mut CadApp`, so the borrow checker is satisfied without the
/// whole app in scope and the layout can be reasoned about on its own.
#[allow(clippy::too_many_arguments)]
pub fn window_ui(
    ctx: &egui::Context,
    open: &mut bool,
    wiring: &mut Wiring,
    pick: &mut Option<u32>,
    rows: &[BlockRow],
    hidden_rows: &[BlockRow],
    loaded: &[String],
    scanned: &[(String, String)],
    placed_counts: &HashMap<u32, usize>,
) -> EditorAction {
    let mut act = EditorAction::default();
    egui::Window::new("Light Editor")
        .open(open)
        .default_width(720.0)
        .default_height(460.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(
                    "Pair a block in the drawing with a photometric file - one fitting is placed \
                     at every instance of it.",
                )
                .small()
                .weak(),
            );
            ui.separator();

            let wired_total: usize =
                wiring.links.keys().map(|b| placed_counts.get(b).copied().unwrap_or(0)).sum();

            ui.columns(2, |col| {
                // ---- LEFT: the drawing's blocks ------------------------------------------
                col[0].label(egui::RichText::new("BLOCKS IN DRAWING").small().strong());
                col[0].label(
                    egui::RichText::new(format!(
                        "{} shown, {} hidden", rows.len(), hidden_rows.len()))
                        .small().weak(),
                );
                egui::ScrollArea::vertical().id_source("le_blocks").max_height(300.0).show(
                    &mut col[0],
                    |ui| {
                        for r in rows {
                            let linked = wiring.links.get(&r.id).cloned();
                            ui.horizontal(|ui| {
                                let mut t = egui::RichText::new(
                                    format!("{}  x{}", r.name, r.instances));
                                if linked.is_some() {
                                    t = t.color(WIRED);
                                }
                                if ui.selectable_label(*pick == Some(r.id), t).clicked() {
                                    *pick = if *pick == Some(r.id) { None } else { Some(r.id) };
                                }
                                // HIDE, NOT DELETE. Most blocks in a plan are furniture; the list
                                // is unusable without filtering, and the drawing is never touched.
                                if ui.small_button("N")
                                    .on_hover_text("Hide from this list. The drawing and its block table are not changed")
                                    .clicked()
                                {
                                    wiring.hidden.insert(r.id);
                                    if *pick == Some(r.id) {
                                        *pick = None;
                                    }
                                }
                                if linked.is_some() {
                                    if ui.small_button("x")
                                        .on_hover_text("Unwire. Fittings already placed stay until you clear them")
                                        .clicked()
                                    {
                                        wiring.links.remove(&r.id);
                                    }
                                    if ui.small_button("del")
                                        .on_hover_text("Remove the fittings this block placed")
                                        .clicked()
                                    {
                                        act.clear_block = Some(r.id);
                                    }
                                }
                            });
                            if let Some(p) = linked {
                                ui.label(
                                    egui::RichText::new(format!("      -> {p}")).small().color(WIRED));
                            }
                        }
                        if rows.is_empty() {
                            ui.label(
                                egui::RichText::new("No blocks in this drawing.").small().weak());
                        }
                    },
                );
                if !hidden_rows.is_empty() {
                    egui::CollapsingHeader::new(format!("Hidden ({})", hidden_rows.len()))
                        .id_source("le_hidden")
                        .show(&mut col[0], |ui| {
                            for r in hidden_rows {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(
                                        format!("{}  x{}", r.name, r.instances)).small().weak());
                                    if ui.small_button("show").clicked() {
                                        wiring.hidden.remove(&r.id);
                                    }
                                });
                            }
                        });
                }

                // ---- RIGHT: the photometry ----------------------------------------------
                col[1].label(egui::RichText::new("PHOTOMETRY (.ldt / .ies)").small().strong());
                col[1].horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut wiring.folder)
                            .desired_width(140.0)
                            .hint_text("folder of .ldt files"),
                    );
                    if ui.small_button("...").on_hover_text("Choose a folder").clicked() {
                        act.browse_folder = true;
                    }
                    if ui.small_button("scan").on_hover_text("Rescan the folder").clicked() {
                        act.rescan = true;
                    }
                });
                if pick.is_some() {
                    col[1].label(
                        egui::RichText::new("Now click a fitting to wire it.").small().color(WIRED));
                } else {
                    col[1].label(
                        egui::RichText::new("Pick a block on the left first.").small().weak());
                }
                egui::ScrollArea::vertical().id_source("le_ldt").max_height(300.0).show(
                    &mut col[1],
                    |ui| {
                        // Already imported - wiring these is instant.
                        for name in loaded {
                            let used = wiring.links.values().any(|v| v == name);
                            let mut t = egui::RichText::new(name);
                            if used {
                                t = t.color(WIRED);
                            }
                            if ui
                                .selectable_label(false, t)
                                .on_hover_text("In the library - click to wire it to the picked block")
                                .clicked()
                            {
                                if let Some(b) = *pick {
                                    wiring.links.insert(b, name.clone());
                                    *pick = None;
                                }
                            }
                        }
                        // Found on disk but not imported. Offered, because a folder of two hundred
                        // files is the point - but wiring one has to import it first, and saying so
                        // beats a silent failure later when the profile does not resolve.
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
                                    .on_hover_text(path.as_str())
                                    .clicked()
                                {
                                    if let Some(b) = *pick {
                                        act.import_and_wire = Some((path.clone(), b));
                                        *pick = None;
                                    }
                                }
                            }
                        }
                        if loaded.is_empty() && scanned.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Nothing yet - point at a folder above, or import a file in the SIMLUX panel.",
                                )
                                .small()
                                .weak(),
                            );
                        }
                    },
                );
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
                         what these blocks placed before - it never doubles them, and never touches \
                         a fitting you placed by hand.",
                    )
                    .clicked()
                {
                    act.apply = true;
                }
                ui.label(egui::RichText::new(format!("{n} block(s) wired")).small().weak());
            });
        });
    act
}
#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::{Block, BlockRef, DObject, Line, Vec2, MAX_BLOCK_PARAMS};

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

    /// CURATION IS ABOUT THE LIST, NOT THE DRAWING. Hiding a row tidies the window; it must not
    /// quietly pull that block's two hundred fittings out of the calculation.
    #[test]
    fn hiding_a_block_removes_it_from_the_list_but_not_from_the_placement() {
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
}
