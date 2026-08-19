//! **Illuminaire** — a library of light fittings, each one a 2D drawing symbol paired with a
//! photometric file.
//!
//! WHAT A FITTING IS. A product on a lighting plan has two halves that live in different worlds:
//! the SYMBOL a drafter puts on the drawing, and the PHOTOMETRY the calculation needs. They are
//! the same product and nothing in the file format says so, so the pairing gets remade by hand on
//! every project — or, more often, half-remade, and a plan goes out with symbols that no
//! calculation ever saw.
//!
//! A `Fitting` is that pairing, made once and kept. Placing one puts an ordinary `BlockRef` on the
//! 2D drawing — as far as the CAD side is concerned it is a block like any other, which is the
//! point — and gives SIMLUX a luminaire carrying the linked photometry at the same spot.
//!
//! THE LIBRARY IS THE APP'S, NOT THE PROJECT'S. Asked for as "they should be able to save it in
//! the app so when they open another file and want to use they same combo they can". It lives in
//! `$HOME/.config/rust_cad/`, beside the other cross-project preferences.
//!
//! SO A FITTING CARRIES ITS GEOMETRY, not a block name. A name is a reference into one drawing's
//! block table and means nothing in the next file; the whole promise here is that a combo made on
//! Monday works on Thursday in a drawing that has never heard of it.
//!
//! A 3D MODEL COMES LATER. `model_path` is reserved for the STEP file that will let SIMLUX show
//! the real fitting instead of a marker — "the stp file wiring is plan for next development, just
//! keep it in mind". The field exists now so a library written today loads unchanged when it does.


use cad_kernel::{Block, DObject, Document, Geom, Vec2};

/// One library entry: a symbol, a photometric file, and room for a 3D model.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Fitting {
    /// What the user calls it. Free text, and the key nothing else depends on — see `id`.
    pub name: String,
    /// STABLE IDENTITY, minted once and never reused.
    ///
    /// A name is what a person edits; renaming a fitting must not orphan the drawings that already
    /// use it, and two products may legitimately share a name across manufacturers.
    pub id: u32,
    /// The symbol's own geometry, in DRAWING UNITS as authored.
    ///
    /// Carried rather than referenced. A block NAME resolves against one document's table, so a
    /// library that stored names would hand back a fitting that draws nothing the moment it met a
    /// drawing which had never seen it — which is exactly the case this library exists for.
    #[serde(skip)]
    pub symbol: Vec<SymbolGeom>,
    /// What one unit of `symbol` is worth in metres, from the drawing it was taken out of.
    ///
    /// Without it a symbol authored in millimetres arrives in a metre drawing a thousand times too
    /// big. The library spans projects, so it cannot assume they share a unit — this is the same
    /// trap the clipboard and the grid spacing both had.
    pub symbol_unit_m: f64,
    /// The photometric file this symbol means, as an absolute path.
    pub ldt_path: String,
    /// The profile name that file parses to — the key a `Luminaire` stores.
    pub profile: String,
    /// RESERVED: a STEP model, so SIMLUX can show the real fitting rather than a marker. Next
    /// development; the field is here now so today's library file loads unchanged then.
    #[serde(default)]
    pub model_path: String,
}

/// One drawable piece of a symbol, flattened out of the source drawing.
///
/// NOT serialised here — `Geom` is the kernel's own type and has no serde. The symbols travel in
/// the `.rsm` half of the library instead; see [`Library::sym_path`].
#[derive(Clone, Debug)]
pub struct SymbolGeom {
    pub geom: Geom,
    /// ACI colour index, or `None` for ByLayer. Layers do not travel with a fitting — a library
    /// entry has no layer table to resolve against — so a symbol keeps the literal colour it was
    /// drawn in and nothing else.
    pub aci: Option<u8>,
}

impl SymbolGeom {
    /// Back to a drawing object, colour and all. One definition, used by both the library writer
    /// and `ensure_block`, so what is stored and what is placed cannot drift.
    pub fn to_dobject(&self) -> DObject {
        let mut d = DObject::new(self.geom.clone());
        if let Some(a) = self.aci {
            d.style.color = cad_kernel::Color::Aci(a);
        }
        d
    }
}

/// The saved library.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Library {
    pub fittings: Vec<Fitting>,
    /// High-water mark for [`Fitting::id`] — never rewound, so a deleted fitting's id is never
    /// handed to its replacement and a drawing that still names it resolves to nothing rather
    /// than to a stranger.
    #[serde(default)]
    pub next_id: u32,
}


/// The metadata half of the library, in the user's config directory.
const META_FILE: &str = "illuminaire.json";
/// The symbol half, beside it.
const SYM_FILE: &str = "illuminaire.rsm";

/// Write through a temp file and rename, so an interrupted write cannot leave a half-written file
/// behind — the same rule the drawing save follows, and it matters more here because this file is
/// not per-project: losing it loses every combo ever made.
fn write_atomic(p: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", p.display())
    })
}

impl Library {
    /// Where the metadata lives — beside the other cross-project preferences.
    pub fn path() -> Option<std::path::PathBuf> {
        Self::dir().map(|d| d.join(META_FILE))
    }

    /// Where the SYMBOLS live, next to it.
    ///
    /// TWO FILES, because `Geom` is the kernel's own type and does not serialise to JSON — the
    /// project has exactly one serialisation for geometry, `.rsm`, and inventing a second one
    /// here would be a private re-encoding of every variant, drifting from the real one the first
    /// time an arc or a spline changed shape.
    ///
    /// So the symbols are stored the way the app already stores geometry: an ordinary document
    /// whose block table holds one block per fitting. The blocks are named by fitting ID rather
    /// than by the user's name, because the name is editable and the id is what identity means
    /// here — renaming a fitting must not sever it from its own linework.
    pub fn sym_path() -> Option<std::path::PathBuf> {
        Self::dir().map(|d| d.join(SYM_FILE))
    }

    fn dir() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(std::path::PathBuf::from(home).join(".config/rust_cad"))
    }

    /// The block name a fitting's symbol is stored under.
    fn sym_key(id: u32) -> String {
        format!("#{id}")
    }

    /// Read the library from the user's config directory, or an empty one when there is none.
    pub fn load() -> Result<Self, String> {
        match Self::dir() {
            Some(d) => Self::load_from(&d),
            None => Ok(Self::default()),
        }
    }

    /// Write it there. Returns the metadata path.
    pub fn save(&self) -> Result<std::path::PathBuf, String> {
        let d = Self::dir().ok_or("no home directory to save the library in")?;
        self.save_to(&d)
    }

    /// Read a library out of `dir`.
    ///
    /// A MALFORMED FILE IS NOT AN EMPTY ONE, and the difference matters: silently starting fresh
    /// would let the next save overwrite a library that was merely unreadable this once. The error
    /// is returned so the caller can say so and decline to write over it.
    ///
    /// Split from [`load`](Self::load) so it can be driven against a temporary directory: a test
    /// that had to point `HOME` somewhere would be changing process-global state that every other
    /// test in the binary shares.
    pub fn load_from(dir: &std::path::Path) -> Result<Self, String> {
        let p = dir.join(META_FILE);
        let text = match std::fs::read_to_string(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("read {}: {e}", p.display())),
            Ok(t) => t,
        };
        let mut lib: Library =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        lib.reserve_ids();
        lib.attach_symbols(&dir.join(SYM_FILE))?;
        Ok(lib)
    }

    /// Fill in the geometry the metadata file does not carry.
    ///
    /// A MISSING SYMBOL FILE IS NOT AN ERROR — the fittings are still real, they still hold their
    /// photometry, and they draw as an empty preview rather than vanishing. An UNREADABLE one is,
    /// for the same reason a malformed metadata file is.
    fn attach_symbols(&mut self, sp: &std::path::Path) -> Result<(), String> {
        let bytes = match std::fs::read(sp) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("read {}: {e}", sp.display())),
            Ok(b) => b,
        };
        let doc = cad_io::rsm::read_rsm(&bytes).map_err(|e| format!("{}: {e}", sp.display()))?;
        for f in &mut self.fittings {
            if let Some(id) = doc.blocks.find(&Self::sym_key(f.id)) {
                f.symbol = flatten_block(&doc, id, 0);
            }
        }
        Ok(())
    }

    /// Write both halves into `dir`. Returns the metadata path.
    pub fn save_to(&self, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

        // SYMBOLS FIRST. If that write fails, the metadata file is left describing the OLD
        // symbols — a library one save behind, rather than a library of fittings that draw
        // nothing.
        let mut doc = Document::default();
        for f in &self.fittings {
            doc.blocks.add(Block {
                name: Self::sym_key(f.id),
                base: Vec2::new(0.0, 0.0),
                dobjects: f.symbol.iter().map(SymbolGeom::to_dobject).collect(),
                smart: false,
                params: Vec::new(),
                cut_edges: Vec::new(),
            });
        }
        write_atomic(&dir.join(SYM_FILE), &cad_io::rsm::write_rsm(&doc))?;

        let p = dir.join(META_FILE);
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&p, text.as_bytes())?;
        Ok(p)
    }

    /// Raise the id counter above anything already stored. Must run after a load: a file written
    /// before `next_id` existed carries 0, and a counter behind the data hands out live ids.
    pub fn reserve_ids(&mut self) {
        let top = self.fittings.iter().map(|f| f.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(top + 1);
    }

    /// Add a fitting, giving it a fresh id. Returns that id.
    pub fn add(&mut self, mut f: Fitting) -> u32 {
        if self.next_id == 0 {
            self.reserve_ids();
        }
        let id = self.next_id;
        f.id = id;
        self.next_id += 1;
        self.fittings.push(f);
        id
    }

    pub fn get(&self, id: u32) -> Option<&Fitting> {
        self.fittings.iter().find(|f| f.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Fitting> {
        self.fittings.iter_mut().find(|f| f.id == id)
    }

    /// Remove a fitting; returns true if one went.
    pub fn remove(&mut self, id: u32) -> bool {
        let n = self.fittings.len();
        self.fittings.retain(|f| f.id != id);
        self.fittings.len() != n
    }
}

/// One block offered in the add panel: what it is called, what it is made of, and how big it is.
#[derive(Clone, Debug, Default)]
pub struct BlockRow {
    pub name: String,
    pub symbol: Vec<SymbolGeom>,
    /// Extent in DRAWING units, `[width, height]`. Zero when there is nothing drawable in it.
    ///
    /// Shown beside the name so the unit can be CHECKED rather than trusted. The real block file
    /// this feature was built for declares itself in inches and is drawn in millimetres — the
    /// 2 m batten in it reads as 2000 units, and 2000 inches is 50 metres. Nothing in the
    /// geometry can settle that; a person reading "2000 × 48 mm" against a product they know can,
    /// in one glance, which is the whole reason the figure is on screen.
    pub size: [f64; 2],
}

/// Every block definition in `doc`, as library-ready symbols.
///
/// This is how a fixture library gets in — point Illuminaire at a drawing full of symbols and it
/// reads the block table. Nested block references are EXPANDED rather than referenced, for the
/// same reason a symbol carries its geometry at all: whatever a fitting is made of has to travel
/// with it.
pub fn symbols_from(doc: &Document) -> Vec<BlockRow> {
    doc.blocks
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let symbol = flatten_block(doc, i as u32, 0);
            BlockRow { name: b.name.clone(), size: symbol_extent(&symbol), symbol }
        })
        .collect()
}

/// The bounding extent of a flattened symbol, in the units it was authored in.
///
/// Measured off the DISPLAY outlines, the same tessellation the preview draws, so a circle
/// measures as its diameter rather than as the extent of its centre point.
pub fn symbol_extent(sym: &[SymbolGeom]) -> [f64; 2] {
    let empty = Document::default();
    let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut any = false;
    for s in sym {
        for path in cad_solid::geom_display_outlines_scaled(&s.geom, &empty, 1.0) {
            for p in path {
                any = true;
                mnx = mnx.min(p.x);
                mny = mny.min(p.y);
                mxx = mxx.max(p.x);
                mxy = mxy.max(p.y);
            }
        }
    }
    if !any {
        return [0.0, 0.0];
    }
    [(mxx - mnx) as f64, (mxy - mny) as f64]
}

/// The units a block file can be declared in, for the add panel's chooser.
///
/// `$INSUNITS` IS OFTEN WRONG, and it is wrong in the file this was built against. A symbol that
/// arrives 25.4 times too big is not subtle on the plan, but it is silent — nothing reports it,
/// and the fitting is simply the wrong size in every drawing made from it afterwards. So the
/// declared unit is the DEFAULT, not the answer.
pub const UNIT_CHOICES: [(&str, f64); 4] =
    [("mm", 0.001), ("cm", 0.01), ("m", 1.0), ("inch", 0.0254)];

/// The label for a metres-per-unit value, or a bare figure when it is none of the usual ones.
pub fn unit_label(m: f64) -> String {
    UNIT_CHOICES
        .iter()
        .find(|(_, k)| (k - m).abs() < 1e-9)
        .map(|(n, _)| (*n).to_string())
        .unwrap_or_else(|| format!("{m} m/unit"))
}

/// Depth cap on nested block expansion. A drawing can describe a cycle — `read_blocks` resolves
/// forward references by assigning into an already-created definition — and a walker without a
/// bound recurses until the stack goes, during what the user asked to be a library import.
const MAX_DEPTH: u32 = 8;

fn flatten_block(doc: &Document, block: u32, depth: u32) -> Vec<SymbolGeom> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }
    let Some(b) = doc.blocks.get(block) else { return Vec::new() };
    let mut out = Vec::new();
    for d in &b.dobjects {
        let aci = match d.style.color {
            cad_kernel::Color::Aci(n) => Some(n),
            _ => None,
        };
        match &d.geom {
            // A nested reference is expanded through its own transform, so what lands in the
            // library is the shape a person would see rather than a name that resolves to nothing.
            Geom::BlockRef(br) => {
                let Some(inner) = doc.blocks.get(br.block) else { continue };
                let base = inner.base;
                for g in flatten_block(doc, br.block, depth + 1) {
                    out.push(SymbolGeom {
                        geom: br.transform_geom(&g.geom, base),
                        aci: g.aci.or(aci),
                    });
                }
            }
            g => out.push(SymbolGeom { geom: g.clone(), aci }),
        }
    }
    out
}

/// Put `fitting`'s symbol into `doc` as a block definition, returning its block id.
///
/// AN ORDINARY BLOCK, which is the requirement: "in the drawing in the 2d we will treat as a
/// normal block. just the block as it is in any 2d cad file." Nothing about the result says
/// Illuminaire made it — it selects, moves, explodes and exports like any other block, and a DXF
/// written from this drawing opens anywhere.
///
/// An existing definition of the same name is REUSED rather than duplicated, so placing the same
/// fitting fifty times leaves one definition and fifty references — which is what a block table is
/// for, and what keeps a plan's file size sane.
///
/// `doc_unit_m` is the destination drawing's unit. A symbol authored in millimetres and placed in
/// a metre drawing is scaled on the way in; without that it arrives a thousand times too big.
pub fn ensure_block(doc: &mut Document, fitting: &Fitting, doc_unit_m: f64) -> u32 {
    if let Some(id) = doc.blocks.find(&fitting.name) {
        return id;
    }
    let k = if doc_unit_m > 0.0 && fitting.symbol_unit_m > 0.0 {
        fitting.symbol_unit_m / doc_unit_m
    } else {
        1.0
    };
    let dobjects = fitting
        .symbol
        .iter()
        .map(|s| {
            let g = if (k - 1.0).abs() < 1e-12 {
                s.geom.clone()
            } else {
                s.geom.scaled(Vec2::new(0.0, 0.0), k)
            };
            let mut d = DObject::new(g);
            if let Some(a) = s.aci {
                d.style.color = cad_kernel::Color::Aci(a);
            }
            d
        })
        .collect();
    doc.blocks.add(Block {
        name: fitting.name.clone(),
        base: Vec2::new(0.0, 0.0),
        dobjects,
        smart: false,
        params: Vec::new(),
        cut_edges: Vec::new(),
    })
}

/// Put one instance of `fitting` into the drawing at `at`, in DRAWING units.
///
/// Returns the block id it was inserted as, so the caller can tag the SIMLUX luminaire it places
/// at the same spot with `from_block` — that tag is the link that survives a save and reopen.
///
/// The drawing gets nothing special: a block definition and a `BlockRef`, exactly as an INSERT
/// from any CAD package would leave. Everything that makes it a light lives on the SIMLUX side,
/// which is the requirement — "in the drawing in the 2d we will treat as a normal block".
pub fn insert(doc: &mut Document, fitting: &Fitting, at: Vec2, doc_unit_m: f64) -> u32 {
    // The DEFINITION carries the unit conversion (see `ensure_block`), so the reference is a plain
    // 1:1 insert. Scaling here instead would give the same picture and a wrong block table: fifty
    // instances each at 0.001, and an explode that hands back millimetre linework in a metre file.
    let block = ensure_block(doc, fitting, doc_unit_m);
    doc.push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
        block,
        insert: at,
        scale: 1.0,
        scale_y: 1.0,
        rotation: 0.0,
        mirror_x: false,
        param_values: [0.0; cad_kernel::block::MAX_BLOCK_PARAMS],
    })));
    block
}


/// How close a block instance has to be to a fixture to be the one it was placed as, in metres.
///
/// Positions make the round trip world-metres `f32` → drawing-units `f64` → back, so an exact
/// comparison never matches. A millimetre is far below the spacing of any real layout and far
/// above that error.
pub const INSTANCE_TOL_M: f64 = 1e-3;

/// The dobject indices of the block instances these fixtures were placed as.
///
/// `from_block` names the DEFINITION, which every instance of a fitting shares — fifty downlights
/// are one definition and fifty references, which is the point of a block table. So the instance
/// is found by POSITION as well: the reference to that definition standing where the fixture
/// stands.
///
/// A FIXTURE THAT HAS BEEN DRAGGED AWAY FROM ITS SYMBOL MATCHES NOTHING, and that is deliberate.
/// Dragging a marker moves the light and not the block, so the two genuinely are apart; taking the
/// nearest instance instead would erase a DIFFERENT fitting's symbol and leave this one's, which
/// is a worse answer than leaving both alone.
///
/// Indices come back sorted and unique, ready to remove from the highest down.
pub fn instances_for<'a>(
    doc: &Document,
    fixtures: impl Iterator<Item = &'a cad_light::Luminaire>,
    doc_unit_m: f64,
) -> Vec<usize> {
    let k = if doc_unit_m.is_finite() && doc_unit_m > 0.0 { doc_unit_m } else { 1.0 };
    let tol = INSTANCE_TOL_M / k; // the tolerance, in drawing units
    let want: Vec<(u32, f64, f64)> = fixtures
        .filter_map(|l| l.from_block.map(|b| (b, l.position.x as f64 / k, l.position.y as f64 / k)))
        .collect();
    if want.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<usize> = Vec::new();
    for (i, d) in doc.dobjects.iter().enumerate() {
        let Geom::BlockRef(br) = &d.geom else { continue };
        let hit = want.iter().any(|(b, x, y)| {
            *b == br.block
                && (br.insert.x - x).abs() <= tol
                && (br.insert.y - y).abs() <= tol
        });
        if hit {
            out.push(i);
        }
    }
    out
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

/// A symbol's linework, fitted into the unit square `0..1` with its ASPECT PRESERVED and centred.
///
/// Returned in unit space rather than pixels so the caller can paint it into any rect, and so the
/// fitting can be tested without a painter. Y is returned MATHS-UP (larger y is further up); the
/// caller flips it, because that is a fact about screens rather than about the block.
///
/// Empty when the symbol has nothing drawable in it — a block of pure text or attributes, say. The
/// caller shows "nothing to draw" rather than an empty box that reads as a failure.
///
/// ONE FITTER for both the add panel and the library tile, so what a block previews as before it
/// is added is the same picture it shows afterwards. Two would eventually disagree, and the
/// disagreement would look like a bad import.
pub fn symbol_preview_paths(sym: &[SymbolGeom]) -> Vec<Vec<[f32; 2]>> {
    // The flattener has already resolved nested references, so the display tessellator needs no
    // block table to look anything up — an empty document is the honest argument here.
    let empty = Document::default();
    let mut raw: Vec<Vec<[f32; 2]>> = Vec::new();
    for d in sym {
        for path in cad_solid::geom_display_outlines_scaled(&d.geom, &empty, 1.0) {
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


// ── THE WINDOW ─────────────────────────────────────────────────────────────────────────────
//
// Shaped like the hatch dialogue, because that is what was asked for: "we will have block and
// thumpnail of ies in one set rectangle shape". A fitting IS the pair, so the tile shows both
// halves side by side — symbol on the left, distribution on the right — and a library of them
// reads at a glance the way a page of hatch patterns does.

/// Colour for a live/linked row — the amber the sketch banner already uses for "this is on".
const WIRED: egui::Color32 = egui::Color32::from_rgb(255, 178, 60);

/// What the window wants done, collected so the UI never borrows the document and the library at
/// the same time — the same shape `LightAction` uses next door.
#[derive(Default)]
pub struct Action {
    /// Open a file picker for a DXF holding block definitions (the converted LIGHT BLOCK.dwg).
    pub browse_blocks: bool,
    /// Open the folder picker for the photometry folder.
    pub browse_folder: bool,
    /// Rescan that folder.
    pub rescan: bool,
    /// Offer the CURRENT drawing's blocks in the add panel.
    pub blocks_from_drawing: bool,
    /// Add a fitting from the block at this index of `blocks`.
    pub add: Option<usize>,
    /// Reinterpret the loaded blocks as being drawn in this many metres per unit.
    pub set_blocks_unit: Option<f64>,
    /// Link (or relink) this fitting to this photometric file.
    pub link: Option<(u32, String)>,
    /// Rename this fitting.
    pub rename: Option<(u32, String)>,
    /// Delete this fitting from the library.
    pub remove: Option<u32>,
    /// Arm this fitting for placement — the next click on the plan puts one down.
    pub place: Option<u32>,
    /// Stop placing.
    pub stop_placing: bool,
}

/// Everything the window needs that is not the library itself.
pub struct WindowInput<'a> {
    /// Blocks offered in the add panel, with their flattened symbols. Cached by the caller: a
    /// real plan has hundreds of block definitions and flattening them all every frame is work
    /// nobody asked for.
    pub blocks: &'a [BlockRow],
    /// Where those blocks came from, for the panel's heading.
    pub blocks_from: &'a str,
    /// Photometric files found in the folder: (stem, full path).
    pub scanned: &'a [(String, String)],
    /// The folder they were found in.
    pub folder: &'a str,
    /// Profiles already parsed, so a tile can draw its distribution.
    pub profiles: &'a std::collections::HashMap<String, cad_light::IesProfile>,
    /// The fitting currently armed for placement, if any.
    pub placing: Option<u32>,
    /// What one unit of those blocks is worth in metres.
    pub blocks_unit_m: f64,
}

/// Paint a symbol's linework into `rect`, fitted and centred.
///
/// The unit-space paths come from [`symbol_preview_paths`], which is where the fitting is decided
/// and tested; this only maps them onto the screen. Y is flipped here and nowhere else — the paths
/// are maths-up, screens are not.
fn paint_symbol(
    painter: &egui::Painter,
    rect: egui::Rect,
    paths: &[Vec<[f32; 2]>],
    col: egui::Color32,
) {
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
    painter.line_segment([org, egui::pos2(org.x, org.y + r)], egui::Stroke::new(0.7, faint));
    painter.line_segment(
        [egui::pos2(org.x - r, org.y), egui::pos2(org.x + r, org.y)],
        egui::Stroke::new(0.7, faint),
    );

    // C0 and C90 — the two sections a datasheet prints. A rotationally symmetric fitting draws
    // them on top of each other, which is itself the useful reading.
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
    // A MISSING FIGURE READS AS MISSING. An em dash where the file states nothing, never a
    // stand-in number: a blank is obviously absent, and a plausible figure is not obviously
    // anything.
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


/// What `place_fitting` becomes when a tile is clicked: `Some(id)` to re-arm, `None` to leave it.
///
/// WHILE PLACING, PICKING A DIFFERENT FITTING SWITCHES TO IT. Reported as "i placed 3 different
/// light yet they are all showing the same legend in the drawing", with a session dump showing
/// four instances all pointing at `block=#0`.
///
/// Nothing was wrong with the placement. Arming was done ONLY by the Place button, so selecting
/// another tile moved the highlight, the name, the photometry and the figures — everything the
/// user reads as "this one is current" — while the next click on the plan still put down the
/// fitting armed several minutes ago. Silent, and every symbol on the drawing wrong.
///
/// Clicking the fitting ALREADY being placed changes nothing: it must not toggle placement off,
/// or clicking the highlighted tile to confirm what is armed would disarm it.
pub fn rearm_on_select(placing: Option<u32>, clicked: u32) -> Option<u32> {
    match placing {
        Some(cur) if cur != clicked => Some(clicked),
        _ => None,
    }
}

/// One tile: the symbol and the distribution in a single rectangle, the name underneath.
///
/// The two halves are drawn even when one is missing — an unlinked fitting shows its symbol beside
/// an empty polar frame, which reads as "this half is not done yet" rather than as a broken tile.
fn tile(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    paths: &[Vec<[f32; 2]>],
    prof: Option<&cad_light::IesProfile>,
    name: &str,
    selected: bool,
    id: egui::Id,
) -> egui::Response {
    let resp = ui.interact(rect, id, egui::Sense::click());
    let bg = if selected {
        egui::Color32::from_rgb(30, 60, 110)
    } else {
        egui::Color32::from_rgb(18, 22, 28)
    };
    let border = if selected {
        egui::Color32::from_rgb(120, 180, 255)
    } else if resp.hovered() {
        egui::Color32::from_rgb(150, 160, 175)
    } else {
        egui::Color32::from_rgb(70, 80, 95)
    };
    let p = ui.painter();
    p.rect_filled(rect, 3.0, bg);
    p.rect_stroke(rect, 3.0, egui::Stroke::new(if selected { 2.0 } else { 1.0 }, border));

    // Split down the middle: block on the left, photometry on the right.
    let mid = rect.center().x;
    let left = egui::Rect::from_min_max(rect.min, egui::pos2(mid, rect.max.y));
    let right = egui::Rect::from_min_max(egui::pos2(mid, rect.min.y), rect.max);
    p.line_segment(
        [egui::pos2(mid, rect.top() + 4.0), egui::pos2(mid, rect.bottom() - 4.0)],
        egui::Stroke::new(0.6, egui::Color32::from_gray(60)),
    );
    paint_symbol(p, left, paths, egui::Color32::from_rgb(200, 215, 235));
    match prof {
        Some(pr) => paint_polar(p, right, pr),
        None => {
            p.text(
                right.center(),
                egui::Align2::CENTER_CENTER,
                "no LDT",
                egui::FontId::proportional(10.0),
                egui::Color32::from_gray(110),
            );
        }
    }
    p.text(
        egui::pos2(rect.center().x, rect.bottom() + 9.0),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(11.0),
        if selected {
            egui::Color32::from_rgb(190, 215, 255)
        } else {
            egui::Color32::from_gray(180)
        },
    );
    resp
}

/// The Illuminaire window.
///
/// Takes what it needs rather than `&mut CadApp`, so the borrow checker is satisfied without the
/// whole app in scope and the layout can be reasoned about on its own.
pub fn window_ui(
    ctx: &egui::Context,
    open: &mut bool,
    lib: &Library,
    sel: &mut Option<u32>,
    add_open: &mut bool,
    name_buf: &mut String,
    input: WindowInput<'_>,
) -> Action {
    let mut act = Action::default();
    let tile_w = 132.0_f32;
    let tile_h = 66.0_f32;
    let cell_w = tile_w + 8.0;
    let cell_h = tile_h + 22.0;
    // The add panel carries a second line under each tile — the measured size.
    let block_cell_h = cell_h + 11.0;

    egui::Window::new("Illuminaire")
        .id(egui::Id::new("illuminaire"))
        .open(open)
        .default_size(egui::vec2(720.0, 560.0))
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            // ---- the library, as tiles ----
            ui.horizontal(|ui| {
                ui.heading("Fittings");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("{} in the library", lib.fittings.len()))
                        .small()
                        .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(if *add_open { "✖  Close add" } else { "＋  Add fitting" })
                        .on_hover_text("Add a fitting from a block")
                        .clicked()
                    {
                        *add_open = !*add_open;
                    }
                });
            });
            ui.add_space(4.0);

            if lib.fittings.is_empty() {
                ui.label(
                    egui::RichText::new("Nothing here yet. Add a fitting, then link a .ldt to it.")
                        .weak(),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("illum_lib_scroll")
                .max_height(220.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let cols = ((ui.available_width() / cell_w).floor() as usize).max(1);
                    let rows = lib.fittings.len().div_ceil(cols).max(1);
                    let (alloc, _) = ui.allocate_exact_size(
                        egui::vec2(cols as f32 * cell_w, rows as f32 * cell_h + 4.0),
                        egui::Sense::hover(),
                    );
                    let org = alloc.left_top();
                    for (i, f) in lib.fittings.iter().enumerate() {
                        let r = egui::Rect::from_min_size(
                            org + egui::vec2(
                                (i % cols) as f32 * cell_w,
                                (i / cols) as f32 * cell_h,
                            ),
                            egui::vec2(tile_w, tile_h),
                        );
                        let paths = symbol_preview_paths(&f.symbol);
                        let prof = input.profiles.get(&f.profile);
                        let resp = tile(
                            ui,
                            r,
                            &paths,
                            prof,
                            &f.name,
                            *sel == Some(f.id),
                            egui::Id::new(("illum_tile", f.id)),
                        );
                        if resp.clicked() {
                            *sel = Some(f.id);
                            name_buf.clear();
                            name_buf.push_str(&f.name);
                            if let Some(id) = rearm_on_select(input.placing, f.id) {
                                act.place = Some(id);
                            }
                        }
                        if input.placing == Some(f.id) {
                            ui.painter().rect_stroke(
                                r.expand(2.0),
                                4.0,
                                egui::Stroke::new(2.0, WIRED),
                            );
                        }
                    }
                });

            ui.separator();

            // ---- what is selected, and what can be done with it ----
            if let Some(f) = sel.and_then(|id| lib.get(id)) {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Name").small().weak());
                    if ui
                        .add(egui::TextEdit::singleline(name_buf).desired_width(180.0))
                        .lost_focus()
                        && !name_buf.trim().is_empty()
                        && name_buf.trim() != f.name
                    {
                        act.rename = Some((f.id, name_buf.trim().to_string()));
                    }
                    ui.add_space(8.0);
                    let placing = input.placing == Some(f.id);
                    let btn = egui::Button::new(if placing { "■  Stop placing" } else { "▣  Place" })
                        .fill(if placing {
                            egui::Color32::from_rgb(120, 60, 20)
                        } else {
                            egui::Color32::from_rgb(30, 70, 40)
                        });
                    if ui
                        .add(btn)
                        .on_hover_text("Click on the plan to drop one. Esc stops.")
                        .clicked()
                    {
                        if placing {
                            act.stop_placing = true;
                        } else {
                            act.place = Some(f.id);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("🗑")
                            .on_hover_text("Remove from the library. Blocks already placed stay.")
                            .clicked()
                        {
                            act.remove = Some(f.id);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Photometry").small().weak());
                    let txt = if f.profile.is_empty() {
                        egui::RichText::new("not linked")
                            .color(egui::Color32::from_rgb(220, 130, 90))
                    } else {
                        egui::RichText::new(&f.profile).color(WIRED)
                    };
                    ui.label(txt);
                });
                if let Some(prof) = input.profiles.get(&f.profile) {
                    ui.add_space(2.0);
                    ui.push_id("illum_figs", |ui| {
                        ui.set_max_width(240.0);
                        figures_ui(ui, prof);
                    });
                }
                ui.add_space(4.0);
            } else {
                ui.label(egui::RichText::new("Select a fitting above.").weak());
            }

            ui.separator();

            // ---- the add panel: blocks to make fittings out of ----
            if *add_open {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Blocks").strong());
                    ui.label(egui::RichText::new(input.blocks_from).small().weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("📂  Load block file…")
                            .on_hover_text(
                                "Fixture symbols from a drawing’s block table — .dwg or .dxf. A .dwg is converted on the way in.",
                            )
                            .clicked()
                        {
                            act.browse_blocks = true;
                        }
                        if ui.button("This drawing").clicked() {
                            act.blocks_from_drawing = true;
                        }
                    });
                });
                // WHAT THE BLOCKS ARE DRAWN IN — offered, not assumed.
                //
                // A DXF states its unit and the statement is often wrong. The file this was built
                // against declares inches and holds a 2 m batten drawn as 2000 units, which under
                // its own declaration is a fifty-metre luminaire. Nothing downstream would report
                // that; the fitting would simply be the wrong size in every drawing made from it.
                // So the declared unit fills this in and the sizes beside each block let it be
                // checked in one glance.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Drawn in").small().weak());
                    for (label, m) in UNIT_CHOICES {
                        let on = (input.blocks_unit_m - m).abs() < 1e-9;
                        if ui.selectable_label(on, label).clicked() && !on {
                            act.set_blocks_unit = Some(m);
                        }
                    }
                    ui.label(
                        egui::RichText::new("— sizes below are in this unit; if they look wrong, \
                                             the file's declaration was wrong")
                            .small()
                            .weak(),
                    );
                });
                ui.add_space(3.0);
                if input.blocks.is_empty() {
                    ui.label(egui::RichText::new("No blocks to show.").weak());
                }
                egui::ScrollArea::vertical()
                    .id_salt("illum_blocks_scroll")
                    .max_height(180.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let cols = ((ui.available_width() / cell_w).floor() as usize).max(1);
                        let rows = input.blocks.len().div_ceil(cols).max(1);
                        let (alloc, _) = ui.allocate_exact_size(
                            egui::vec2(cols as f32 * cell_w, rows as f32 * block_cell_h + 4.0),
                            egui::Sense::hover(),
                        );
                        let org = alloc.left_top();
                        let u = unit_label(input.blocks_unit_m);
                        for (i, b) in input.blocks.iter().enumerate() {
                            let r = egui::Rect::from_min_size(
                                org + egui::vec2(
                                    (i % cols) as f32 * cell_w,
                                    (i / cols) as f32 * block_cell_h,
                                ),
                                egui::vec2(tile_w, tile_h),
                            );
                            let paths = symbol_preview_paths(&b.symbol);
                            let resp = tile(
                                ui,
                                r,
                                &paths,
                                None,
                                &b.name,
                                false,
                                egui::Id::new(("illum_block", i)),
                            );
                            // The measured size, under the tile, in whichever unit is selected.
                            ui.painter().text(
                                egui::pos2(r.center().x, r.bottom() + 20.0),
                                egui::Align2::CENTER_CENTER,
                                format!("{:.0} × {:.0} {u}", b.size[0], b.size[1]),
                                egui::FontId::proportional(9.0),
                                egui::Color32::from_gray(130),
                            );
                            if resp.on_hover_text("Add this block as a fitting").clicked() {
                                act.add = Some(i);
                            }
                        }
                    });
                ui.separator();
            }

            // ---- photometry: the other half of a combo ----
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Photometry").strong());
                ui.label(
                    egui::RichText::new(if input.folder.is_empty() {
                        "no folder chosen".to_string()
                    } else {
                        format!("{}  ·  {} file(s)", input.folder, input.scanned.len())
                    })
                    .small()
                    .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("📂  Folder…").clicked() {
                        act.browse_folder = true;
                    }
                    if ui.button("⟳").on_hover_text("Rescan").clicked() {
                        act.rescan = true;
                    }
                });
            });
            ui.add_space(3.0);
            let target = *sel;
            if target.is_none() {
                ui.label(
                    egui::RichText::new("Select a fitting to link one of these to it.").weak(),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("illum_ldt_scroll")
                .max_height(150.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let cur = target.and_then(|id| lib.get(id)).map(|f| f.profile.as_str());
                    for (stem, path) in input.scanned {
                        let linked = cur == Some(stem.as_str());
                        let label = if linked {
                            egui::RichText::new(format!("● {stem}")).color(WIRED)
                        } else {
                            egui::RichText::new(format!("○ {stem}"))
                        };
                        let r = ui.add_enabled(
                            target.is_some(),
                            egui::Button::new(label)
                                .frame(false)
                                .min_size(egui::vec2(200.0, 0.0)),
                        );
                        if r.on_hover_text(path.as_str()).clicked() {
                            if let Some(id) = target {
                                act.link = Some((id, path.clone()));
                            }
                        }
                    }
                });
        });
    act
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::{Circle, Line};

    fn sym(x: f64) -> Vec<SymbolGeom> {
        vec![SymbolGeom {
            geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(x, 0.0) }),
            aci: Some(2),
        }]
    }

    fn fitting(name: &str, unit_m: f64, len: f64) -> Fitting {
        Fitting {
            name: name.into(),
            id: 0,
            symbol: sym(len),
            symbol_unit_m: unit_m,
            ldt_path: "C:/ldt/OCULUS.ldt".into(),
            profile: "OCULUS GRANDE 2.0 - 36°".into(),
            model_path: String::new(),
        }
    }

    /// AN ID IS MINTED ONCE AND NEVER REUSED — the same rule feature and plane ids follow, and for
    /// the same reason: a drawing that still names a deleted fitting must resolve to nothing
    /// rather than to whatever took its place.
    #[test]
    fn ids_are_never_reused() {
        let mut lib = Library::default();
        let a = lib.add(fitting("A", 0.001, 100.0));
        let b = lib.add(fitting("B", 0.001, 100.0));
        assert_ne!(a, b);
        assert!(lib.remove(a));
        let c = lib.add(fitting("C", 0.001, 100.0));
        assert_ne!(c, a, "a new fitting inherited a deleted one's identity");
    }

    /// RENAMING DOES NOT ORPHAN ANYTHING. The name is what a person edits; the id is what
    /// everything else holds.
    #[test]
    fn renaming_keeps_the_identity() {
        let mut lib = Library::default();
        let id = lib.add(fitting("OCULUS", 0.001, 100.0));
        lib.get_mut(id).expect("present").name = "OCULUS GRANDE".into();
        assert_eq!(lib.get(id).map(|f| f.name.as_str()), Some("OCULUS GRANDE"));
    }

    /// RELINKING THE PHOTOMETRY IS AN EDIT, not a new fitting — "the user should also be able to
    /// edit it or link another ldt file to the block".
    #[test]
    fn the_photometry_can_be_relinked_in_place() {
        let mut lib = Library::default();
        let id = lib.add(fitting("PULSE", 0.001, 100.0));
        {
            let f = lib.get_mut(id).expect("present");
            f.ldt_path = "C:/ldt/PULSE-14.ldt".into();
            f.profile = "PULSE MG - 14°".into();
        }
        assert_eq!(lib.fittings.len(), 1, "relinking must not add a second entry");
        assert_eq!(lib.get(id).map(|f| f.profile.as_str()), Some("PULSE MG - 14°"));
    }

    /// A SYMBOL DRAWN IN MILLIMETRES LANDS THE RIGHT SIZE IN A METRE DRAWING.
    ///
    /// The library spans projects, so it cannot assume they share a unit. A 100 mm symbol placed
    /// in a metre-measured drawing is 0.1 units there — and without the conversion it would be
    /// 100 of them, a symbol the size of a building.
    #[test]
    fn a_symbol_is_scaled_into_the_drawings_own_unit() {
        let f = fitting("OCULUS", 0.001, 100.0); // 100 mm across
        let mut metres = Document::default();
        let id = ensure_block(&mut metres, &f, 1.0);
        let blk = metres.blocks.get(id).expect("the block was added");
        let (mn, mx) = blk.dobjects[0].bbox();
        assert!(
            ((mx.x - mn.x) - 0.1).abs() < 1e-9,
            "a 100 mm symbol came into a metre drawing {} units wide",
            mx.x - mn.x,
        );
    }

    /// …AND IS UNTOUCHED WHEN THE UNITS AGREE, which is the ordinary case.
    #[test]
    fn a_symbol_is_not_rescaled_when_the_units_match() {
        let f = fitting("OCULUS", 0.001, 100.0);
        let mut mm = Document::default();
        let id = ensure_block(&mut mm, &f, 0.001);
        let blk = mm.blocks.get(id).expect("added");
        let (mn, mx) = blk.dobjects[0].bbox();
        assert!((( mx.x - mn.x) - 100.0).abs() < 1e-9, "got {}", mx.x - mn.x);
    }

    /// PLACING THE SAME FITTING TWICE MAKES ONE DEFINITION. Fifty downlights are fifty references
    /// to one block, which is what a block table is for.
    #[test]
    fn placing_a_fitting_twice_reuses_its_definition() {
        let f = fitting("OCULUS", 0.001, 100.0);
        let mut doc = Document::default();
        let a = ensure_block(&mut doc, &f, 0.001);
        let before = doc.blocks.blocks.len();
        let b = ensure_block(&mut doc, &f, 0.001);
        assert_eq!(a, b, "a second placement made a second definition");
        assert_eq!(doc.blocks.blocks.len(), before, "the block table grew");
    }

    /// A SYMBOL CARRIES ITS GEOMETRY, so a combo works in a drawing that has never seen it. This
    /// is the whole promise of an app-wide library, and a version that stored block NAMES would
    /// satisfy every other test here and fail this one.
    #[test]
    fn a_fitting_draws_in_a_document_that_has_never_seen_it() {
        let f = fitting("NEVER-SEEN", 0.001, 100.0);
        let mut stranger = Document::default();
        assert!(stranger.blocks.find("NEVER-SEEN").is_none(), "precondition");
        let id = ensure_block(&mut stranger, &f, 0.001);
        assert!(
            !stranger.blocks.get(id).expect("added").dobjects.is_empty(),
            "the fitting arrived as an empty block — it would place an invisible symbol",
        );
    }

    /// READING A LIBRARY DRAWING gives one symbol per block, with its geometry.
    #[test]
    fn a_drawing_of_blocks_reads_as_symbols() {
        let mut doc = Document::default();
        doc.blocks.add(Block {
            name: "DOWNLIGHT".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![DObject::new(Geom::Circle(Circle {
                center: Vec2::new(0.0, 0.0),
                radius: 50.0,
            }))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let syms = symbols_from(&doc);
        let d = syms.iter().find(|b| b.name == "DOWNLIGHT").expect("the block");
        assert_eq!(d.symbol.len(), 1, "the symbol lost its geometry");
        // AND ITS MEASURED SIZE, which is what lets a wrong unit declaration be seen. A 50-unit
        // radius circle is 100 across; reporting the radius, or the extent of its centre point,
        // would put every round fitting at half size or at nothing.
        assert!((d.size[0] - 100.0).abs() < 0.5, "the block measured {} across", d.size[0]);
        assert!((d.size[1] - 100.0).abs() < 0.5, "the block measured {} tall", d.size[1]);
    }

    /// A BLOCK FILE'S DECLARED UNIT IS OFTEN WRONG, and the panel has to make that visible.
    ///
    /// The real file this was built against (`LIGHT BLOCK.dwg`) declares `$INSUNITS` = inches and
    /// holds a batten named "(2M)" drawn as 2000 units — millimetres. Trusted, that fitting enters
    /// the library at 2000 × 0.0254 = 50.8 metres long, and every plan drawn from it is wrong by a
    /// factor of 25.4 with nothing anywhere reporting it.
    ///
    /// Nothing in the geometry can settle which unit is meant. What CAN settle it is a person
    /// reading the measured size against a product they know — so the size is measured here, and
    /// the label it is shown in is chosen, not assumed.
    #[test]
    fn a_block_reports_the_size_it_was_drawn_at() {
        let mut doc = Document::default();
        doc.blocks.add(Block {
            name: "LINEA W48X80 - (2M)".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![DObject::new(Geom::Line(Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(2000.0, 0.0),
            }))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = symbols_from(&doc);
        let b = rows.iter().find(|b| b.name.starts_with("LINEA")).expect("the batten");
        // The FIGURE is the drawn one — unconverted. Converting it here would fold the wrong
        // declaration into the very number that exists to expose it.
        assert!((b.size[0] - 2000.0).abs() < 1e-6, "the batten measured {}", b.size[0]);
        assert!(b.size[1].abs() < 1e-6, "a single line has no height: {}", b.size[1]);
    }

    /// AN EMPTY BLOCK MEASURES NOTHING rather than a bounding box between two infinities.
    #[test]
    fn a_block_with_nothing_in_it_measures_zero() {
        let mut doc = Document::default();
        doc.blocks.add(Block {
            name: "EMPTY".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = symbols_from(&doc);
        assert_eq!(rows[0].size, [0.0, 0.0], "an empty block measured {:?}", rows[0].size);
    }

    /// THE UNIT CHOICES NAME REAL UNITS, and the label round-trips the value the file states.
    ///
    /// The chooser writes `metres_per_unit` straight onto the fitting, so a choice mislabelled by
    /// a factor of ten is a silent tenfold error in every fitting added under it.
    #[test]
    fn the_unit_choices_are_what_they_say() {
        for (label, m) in UNIT_CHOICES {
            assert_eq!(unit_label(m), label, "{label} round-tripped wrong");
        }
        let by = |n: &str| UNIT_CHOICES.iter().find(|(l, _)| *l == n).expect(n).1;
        assert!((by("mm") - 0.001).abs() < 1e-12);
        assert!((by("cm") - 0.01).abs() < 1e-12);
        assert!((by("m") - 1.0).abs() < 1e-12);
        assert!((by("inch") - 0.0254).abs() < 1e-12, "an inch is 25.4 mm");
        // The unit the real block file declares must be one of the offered ones, or the panel
        // opens with nothing selected and the user cannot tell what it defaulted to.
        assert_eq!(unit_label(0.0254), "inch");
        // Anything else says so rather than silently reading as one of them.
        assert!(unit_label(0.3048).contains("0.3048"), "a foot must not be labelled as an offer");
    }

    /// A NESTED BLOCK IS EXPANDED, not dropped. A fixture drawn as a housing block plus a lamp
    /// block would otherwise arrive as half of itself.
    #[test]
    fn a_nested_block_is_expanded_into_the_symbol() {
        let mut doc = Document::default();
        let lamp = doc.blocks.add(Block {
            name: "LAMP".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![DObject::new(Geom::Circle(Circle {
                center: Vec2::new(0.0, 0.0),
                radius: 10.0,
            }))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        doc.blocks.add(Block {
            name: "FIXTURE".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![
                DObject::new(Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(100.0, 0.0) })),
                DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
                    block: lamp,
                    insert: Vec2::new(50.0, 0.0),
                    scale: 1.0,
                    scale_y: 1.0,
                    rotation: 0.0,
                    mirror_x: false,
                    param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                })),
            ],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let syms = symbols_from(&doc);
        let f = syms.iter().find(|b| b.name == "FIXTURE").expect("the fixture");
        assert_eq!(f.symbol.len(), 2, "the nested lamp was dropped: {:?}", f.symbol.len());
        assert!(
            f.symbol.iter().any(|g| matches!(g.geom, Geom::Circle(_))),
            "the nested block came through as something other than its own geometry",
        );
    }

    /// A LIBRARY WRITTEN BEFORE THE 3D MODEL FIELD EXISTED STILL LOADS. `model_path` is reserved
    /// for the STEP wiring that comes next, and a library saved today has to survive its arrival.
    /// Hand-written rather than round-tripped, because a round trip writes the field and so never
    /// exercises its absence.
    #[test]
    fn a_library_saved_without_a_model_path_still_loads() {
        let json = r#"{ "fittings": [ {
            "name": "OCULUS", "id": 3, "symbol": [], "symbol_unit_m": 0.001,
            "ldt_path": "C:/x.ldt", "profile": "OCULUS"
        } ], "next_id": 4 }"#;
        let lib: Library = serde_json::from_str(json).expect("an older library must load");
        assert_eq!(lib.fittings.len(), 1);
        assert!(lib.fittings[0].model_path.is_empty(), "the reserved field must default to empty");
    }

    /// A LIBRARY LOADED FROM A FILE WITH NO COUNTER DOES NOT HAND OUT A LIVE ID.
    #[test]
    fn loading_reserves_ids_above_what_the_file_holds() {
        let json = r#"{ "fittings": [ { "name": "A", "id": 7, "symbol": [],
            "symbol_unit_m": 0.001, "ldt_path": "", "profile": "" } ] }"#;
        let mut lib: Library = serde_json::from_str(json).expect("loads");
        lib.reserve_ids();
        let fresh = lib.add(fitting("B", 0.001, 1.0));
        assert!(fresh > 7, "a fresh fitting took the id {fresh}, colliding with the stored 7");
    }

    /// A scratch directory of this test's own, so a round trip never touches the real library.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("illuminaire_test_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    }

    /// THE COMBO SURVIVES A RESTART — BOTH HALVES OF IT.
    ///
    /// This is the whole promise: "they should be able to save it in the app so when they open
    /// another file and want to use they same combo they can". The metadata is JSON and the
    /// geometry is a second file, so a save that wrote only one of them would look completely
    /// correct in the window — the names, the photometry, the counts, all there — and place
    /// blocks containing nothing.
    #[test]
    fn a_saved_library_comes_back_with_its_geometry() {
        let dir = scratch("roundtrip");
        let mut lib = Library::default();
        let a = lib.add(fitting("OCULUS GRANDE", 0.001, 250.0));
        let b = lib.add(Fitting {
            name: "LINEAR 1500".into(),
            id: 0,
            symbol: vec![
                SymbolGeom {
                    geom: Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 40.0 }),
                    aci: Some(7),
                },
                SymbolGeom {
                    geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(1500.0, 0.0) }),
                    aci: None,
                },
            ],
            symbol_unit_m: 0.001,
            ldt_path: "D:/ldt/LIN.ldt".into(),
            profile: "LIN 4000K".into(),
            model_path: String::new(),
        });
        lib.save_to(&dir).expect("the library must save");

        let back = Library::load_from(&dir).expect("the library must load");
        assert_eq!(back.fittings.len(), 2, "both fittings must come back");

        let fa = back.get(a).expect("the first fitting kept its id");
        assert_eq!(fa.name, "OCULUS GRANDE");
        assert_eq!(fa.ldt_path, "C:/ldt/OCULUS.ldt", "the photometry link must survive");
        assert_eq!(fa.profile, "OCULUS GRANDE 2.0 - 36°");
        assert_eq!(fa.symbol.len(), 1, "the symbol came back empty — the combo is half a combo");
        assert!((fa.symbol_unit_m - 0.001).abs() < 1e-12, "the unit must survive");

        let fb = back.get(b).expect("the second fitting kept its id");
        assert_eq!(fb.symbol.len(), 2, "a two-piece symbol must come back whole");
        // The SHAPES, not just the count: an rsm that round-tripped every circle as a line would
        // satisfy a length check and preview every downlight as a stick.
        let circles = fb
            .symbol
            .iter()
            .filter(|s| matches!(s.geom, Geom::Circle(_)))
            .count();
        assert_eq!(circles, 1, "the circle came back as something else");
        assert_eq!(fb.symbol.iter().filter(|s| s.aci == Some(7)).count(), 1, "colour must survive");

        // And the counter, so a fitting added after a restart cannot collide with a stored one.
        let mut back = back;
        let c = back.add(fitting("C", 1.0, 1.0));
        assert!(c > a && c > b, "a fresh id ({c}) collided with a stored one");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AN UNREADABLE LIBRARY IS AN ERROR, NOT AN EMPTY ONE. Reporting nothing there would let the
    /// next save write over a file that was merely corrupt for one session.
    #[test]
    fn a_malformed_library_is_not_silently_empty() {
        let dir = scratch("malformed");
        std::fs::write(dir.join(META_FILE), "{ not json at all").expect("write");
        assert!(Library::load_from(&dir).is_err(), "a broken metadata file must be reported");

        // …and so is a broken SYMBOL file, for the same reason: loading past it would show every
        // fitting with an empty preview and then save that emptiness back.
        let dir = scratch("malformed_sym");
        Library::default().save_to(&dir).expect("save");
        std::fs::write(dir.join(SYM_FILE), b"not an rsm file").expect("write");
        assert!(Library::load_from(&dir).is_err(), "a broken symbol file must be reported");

        // A library that has simply never been saved is NOT an error.
        let dir = scratch("absent");
        let lib = Library::load_from(&dir).expect("an absent library loads as empty");
        assert!(lib.fittings.is_empty());

        // Nor is a metadata file with no symbols beside it — the fittings are still real.
        let dir = scratch("no_syms");
        let mut lib = Library::default();
        lib.add(fitting("A", 0.001, 10.0));
        lib.save_to(&dir).expect("save");
        std::fs::remove_file(dir.join(SYM_FILE)).expect("remove");
        let back = Library::load_from(&dir).expect("a missing symbol file is not an error");
        assert_eq!(back.fittings.len(), 1, "the fitting must survive its symbol going missing");
    }

    /// THE NAME IS NOT THE IDENTITY, and the symbol store must not treat it as one.
    ///
    /// A block table is keyed by name, so the obvious implementation files each symbol under the
    /// fitting's name. Two products legitimately share one — the same catalogue name from two
    /// manufacturers, or two housings of one luminaire — and under a name key the second write
    /// collides with the first, so both fittings come back drawing the SAME symbol. Nothing in the
    /// window would say so: two tiles, two names, one picture, and every plan drawn from the wrong
    /// one of them.
    #[test]
    fn two_fittings_sharing_a_name_keep_their_own_symbols() {
        let dir = scratch("samename");
        let mut lib = Library::default();
        let round = lib.add(Fitting {
            symbol: vec![SymbolGeom {
                geom: Geom::Circle(Circle { center: Vec2::new(0.0, 0.0), radius: 75.0 }),
                aci: None,
            }],
            ..fitting("DOWNLIGHT 20W", 0.001, 1.0)
        });
        let square = lib.add(Fitting {
            symbol: vec![SymbolGeom {
                geom: Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(150.0, 0.0) }),
                aci: None,
            }],
            ..fitting("DOWNLIGHT 20W", 0.001, 1.0)
        });
        lib.save_to(&dir).expect("save");

        let back = Library::load_from(&dir).expect("load");
        let a = &back.get(round).expect("the round one").symbol;
        let b = &back.get(square).expect("the square one").symbol;
        assert!(
            matches!(a.first().map(|s| &s.geom), Some(Geom::Circle(_))),
            "the round fitting came back drawing something else",
        );
        assert!(
            matches!(b.first().map(|s| &s.geom), Some(Geom::Line(_))),
            "the square fitting came back drawing the round one's symbol",
        );

        // …and renaming one does not sever it from its own linework either.
        let mut back = back;
        back.get_mut(round).expect("there").name = "DOWNLIGHT 20W ROUND".into();
        back.save_to(&dir).expect("save again");
        let again = Library::load_from(&dir).expect("load again");
        assert_eq!(again.get(round).expect("there").symbol.len(), 1, "the rename lost the symbol");
        let _ = std::fs::remove_dir_all(&dir);
    }


    /// PICKING ANOTHER FITTING WHILE PLACING SWITCHES TO IT.
    ///
    /// The reported one: "i placed 3 different light yet they are all showing the same legend".
    /// Selecting a tile moved every visible sign of which fitting was current, and left the armed
    /// one alone — so the plan filled with the first fitting's symbol and nothing said so.
    #[test]
    fn selecting_another_fitting_while_placing_switches_to_it() {
        assert_eq!(rearm_on_select(Some(3), 7), Some(7), "the new pick was not armed");
    }

    /// CLICKING THE ONE ALREADY BEING PLACED IS NOT A TOGGLE. Clicking the highlighted tile to
    /// confirm what is armed must not disarm it.
    #[test]
    fn reselecting_the_armed_fitting_changes_nothing() {
        assert_eq!(rearm_on_select(Some(7), 7), None);
    }

    /// AND BROWSING IS NOT PLACING. With nothing armed, clicking through the library to look at
    /// each fitting's distribution must not start dropping them on the drawing.
    #[test]
    fn selecting_a_fitting_when_idle_does_not_start_placing() {
        assert_eq!(rearm_on_select(None, 7), None, "browsing the library armed a placement");
    }

    /// TWO DIFFERENT FITTINGS GET TWO DIFFERENT DEFINITIONS.
    ///
    /// Reported as "i placed 3 different light yet they are all showing the same legend in the
    /// drawing", with a session dump showing four instances all pointing at `block=#0`. The
    /// definition is looked up BY NAME and reused when found, so a lookup that matched too
    /// eagerly — or an `add` that did not return the new index — would give every fitting on the
    /// plan the first one's symbol.
    #[test]
    fn different_fittings_do_not_share_a_definition() {
        let mut doc = Document::default();
        let names = ["NUCLEO", "LINEA W48X80 - (2M)", "VEGA", "OCULUS GRANDE 2.0", "PULSE MG"];
        let ids: Vec<u32> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut f = fitting(n, 0.001, 100.0 + i as f64);
                f.name = (*n).into();
                insert(&mut doc, &f, Vec2::new(i as f64, 0.0), 0.001)
            })
            .collect();
        let mut uniq = ids.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), names.len(), "fittings shared a definition: {ids:?}");
        assert_eq!(doc.blocks.blocks.len(), names.len(), "the block table is the wrong size");
        for (i, n) in names.iter().enumerate() {
            assert_eq!(
                doc.blocks.get(ids[i]).expect("the definition").name,
                *n,
                "definition {} is not {n}", ids[i],
            );
        }
        // …and every instance points at its OWN definition, which is what the dump showed it did
        // not: four references, all `block=#0`.
        let refs: Vec<u32> = doc
            .dobjects
            .iter()
            .filter_map(|d| match d.geom {
                Geom::BlockRef(b) => Some(b.block),
                _ => None,
            })
            .collect();
        assert_eq!(refs, ids, "the instances do not match the definitions they were made from");
    }

    /// PLACING PUTS AN ORDINARY BLOCK REFERENCE ON THE DRAWING, at the point asked for.
    ///
    /// "in the drawing in the 2d we will treat as a normal block. just the block as it is in any
    /// 2d cad file." Nothing about the result may be special: a `BlockRef` into the table, at
    /// scale 1, and a definition it shares with every other instance.
    #[test]
    fn placing_leaves_a_plain_block_reference() {
        let mut doc = Document::default();
        let f = fitting("OCULUS", 0.001, 100.0);
        let b1 = insert(&mut doc, &f, Vec2::new(3000.0, 1500.0), 0.001);
        let b2 = insert(&mut doc, &f, Vec2::new(9000.0, 1500.0), 0.001);
        assert_eq!(b1, b2, "the second instance must reuse the first definition");
        assert_eq!(doc.blocks.blocks.len(), 1, "one definition, not one per instance");

        let refs: Vec<cad_kernel::BlockRef> = doc
            .dobjects
            .iter()
            .filter_map(|d| match d.geom {
                Geom::BlockRef(br) => Some(br),
                _ => None,
            })
            .collect();
        assert_eq!(refs.len(), 2, "two instances must be on the drawing");
        assert!((refs[0].insert.x - 3000.0).abs() < 1e-9, "placed at the wrong point");
        assert!((refs[1].insert.x - 9000.0).abs() < 1e-9, "placed at the wrong point");
        for r in &refs {
            assert_eq!(r.block, b1, "an instance points at some other definition");
            assert!(!r.mirror_x);
        }

        // AND INTO A DRAWING OF A DIFFERENT UNIT, which is the case the scale is really about.
        // The DEFINITION carries the conversion (see `ensure_block`); a reference that scaled as
        // well would square it, and a millimetre symbol would arrive in a metre drawing a million
        // times too small — invisible, and indistinguishable from a failed import.
        let mut metres = Document::default();
        insert(&mut metres, &f, Vec2::new(3.0, 1.5), 1.0);
        let br = metres
            .dobjects
            .iter()
            .find_map(|d| match d.geom {
                Geom::BlockRef(b) => Some(b),
                _ => None,
            })
            .expect("an instance must be on the drawing");
        assert!((br.scale - 1.0).abs() < 1e-12, "the reference is scaled: {}", br.scale);
        assert!((br.scale_y - 1.0).abs() < 1e-12, "the reference is scaled in y: {}", br.scale_y);

        // The symbol is 100 units at 0.001 m/unit = 0.1 m, and the drawing measures in metres.
        let def = metres.blocks.get(br.block).expect("the definition");
        let len = match &def.dobjects[0].geom {
            Geom::Line(l) => (l.b - l.a).len(),
            g => panic!("the definition holds {g:?}"),
        };
        assert!((len - 0.1).abs() < 1e-9, "the symbol arrived {len} m long, not 0.1 m");
    }
}

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
            let paths = symbol_preview_paths(&flatten_block(&d, 0, 0));
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
        let (mnx, mny, mxx, mxy) = bounds(&symbol_preview_paths(&flatten_block(&d, 0, 0)));
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
        assert!(symbol_preview_paths(&flatten_block(&d, 0, 0)).is_empty());
        // …and an id that names no block at all must not panic.
        assert!(symbol_preview_paths(&flatten_block(&d, 999, 0)).is_empty());
    }

    /// A CIRCLE PREVIEWS AS A CIRCLE — the flattener is actually being called, rather than only
    /// the straight-line geometry a naive version would handle.
    #[test]
    fn a_round_block_previews_round() {
        let d = doc_with_block(
            "DOWNLIGHT",
            vec![Geom::Circle(Circle { center: Vec2::new(3.0, 3.0), radius: 2.0 })],
        );
        let paths = symbol_preview_paths(&flatten_block(&d, 0, 0));
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


