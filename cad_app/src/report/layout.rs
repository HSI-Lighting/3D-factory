//! Turning the calculation into pages.
//!
//! THE PAGE IS THE UNIT, which is the whole difference from the HTML report. On a web page a
//! 33-column false-colour plot is simply 33 columns wide and the reader scrolls; on paper there is
//! no scrolling, so everything has to be MADE to fit — which is what was asked for: "the layout
//! now generates like this and the user have to go an scroll to see the layout with is not
//! intuitive scale the layout so it fits the page".
//!
//! So the plot is scaled to the content box, values are printed in the cells only when a cell is
//! big enough to hold one legibly, and the numeric grid is split into column blocks that fit.
//! Nothing is silently dropped: a block that will not fit says so on the page.
//!
//! Y RUNS DOWN from the top of the page here, in points, because that is the order things are laid
//! out in and the order the preview paints. [`super::pdf`] flips it once on the way out.

use super::options::{Options, ReportImage, Section};
use super::pdf::{Align, Doc, Font, Item, Jpeg, Page};
use cad_light::{CalcPlane, Installation, LuxGrid, Maintenance, SurfaceResult};

/// One row of the luminaire schedule: a fitting TYPE and how many of it there are.
///
/// A schedule is by type, not by fixture. "48 × OCULUS GRANDE 2.0" is what gets ordered, wired and
/// checked on site; forty-eight identical rows are not a schedule, they are a dump.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ScheduleRow {
    pub profile: String,
    pub count: usize,
    pub manufacturer: String,
    pub catalogue: String,
    pub lamp: String,
    /// Per fitting.
    pub watts: f64,
    pub lumens: f64,
    /// Overall size in metres, `(length, width, height)`. Zero where the file declares none.
    pub size_m: (f64, f64, f64),
}

impl ScheduleRow {
    pub fn total_watts(&self) -> f64 {
        self.watts * self.count as f64
    }
    pub fn total_lumens(&self) -> f64 {
        self.lumens * self.count as f64
    }
    /// Efficacy of this type, lm/W. `None` when the file declares no wattage.
    pub fn efficacy(&self) -> Option<f64> {
        (self.watts > 0.0).then(|| self.lumens / self.watts)
    }
}

/// One room's answer, as the report needs it.
pub struct RoomInput<'a> {
    pub name: String,
    pub grid: &'a LuxGrid,
    pub plane: &'a CalcPlane,
    /// THE SAME ROOM ON EN 12464-1's OWN GRID — and, when it exists, the grid EVERY REPORTED FIGURE
    /// comes from. See [`RoomInput::reported`].
    ///
    /// Reported as: *"the max lux differs by a lot from the dialux and relux values. why is
    /// that?"* — and the answer is that a maximum is not a property of a room. It is the brightest
    /// CELL CENTRE on whatever grid was sampled, and the working grid is about twice as fine as the
    /// default in DIALux or Relux. Measured on the project that prompted the question: the same
    /// field reports 1802 lx on the working grid and 1662 lx on the standard's, while the average
    /// moves from 323.6 to 326.9 — the maximum shifted 140 lx and the average 3.
    pub grid_en: Option<&'a LuxGrid>,
    pub plane_en: Option<&'a CalcPlane>,
    /// Which cells of `grid_en` are measurable. Empty when every cell is, OR when the result was
    /// stored before this was kept — see `RoomInput::reported_mask`.
    pub mask_en: &'a [bool],
    pub mask: &'a [bool],
    /// The room's outline, in metres. Empty for the whole-model fallback.
    pub poly: &'a [glam::Vec2],
    /// The fixtures standing in it — where they are, for the layout drawing.
    pub fixtures: &'a [cad_light::Luminaire],
    pub installation: Option<&'a Installation>,
    pub cylindrical_avg: Option<f64>,
    /// The fittings standing in THIS room, by type.
    pub schedule: Vec<ScheduleRow>,
}

impl<'a> RoomInput<'a> {
    /// THE GRID EVERY REPORTED FIGURE COMES FROM — EN 12464-1's, whenever there is one.
    ///
    /// Asked for as: *"lets only show the en grid since its the standard showing 2 results will
    /// confuse the user."* The report printed both for a while and that was worse: two averages and
    /// two maxima under one room heading read as a contradiction, however carefully the difference
    /// is explained underneath.
    ///
    /// ONE ACCESSOR, so a figure cannot be quoted off the wrong grid by accident. Every average,
    /// minimum, maximum, uniformity, percentile and W/m²-per-100-lx in this file goes through here;
    /// the only thing that still reads `grid` directly is the PICTURE, which is a different
    /// question — see [`RoomInput::drawn`].
    ///
    /// NOTE WHAT THIS COSTS. The standard's grid is the coarser of the two, so it reports a HIGHER
    /// minimum and a HIGHER uniformity than the working grid did. That is not a rounding
    /// difference: on the project this came from, U₀ goes 0.32 → 0.33 and Emin 102 → 108 lx. The
    /// standard's grid is the basis a compliance claim rests on, which is the reason to prefer it,
    /// but it is the more generous of the two and the choice should be made knowing that.
    pub fn reported(&self) -> &'a LuxGrid {
        self.grid_en.unwrap_or(self.grid)
    }

    /// The plane that grid sits on.
    pub fn reported_plane(&self) -> &'a CalcPlane {
        self.plane_en.unwrap_or(self.plane)
    }

    /// THE GRID THE FALSE-COLOUR FIELD IS DRAWN FROM — always the fine one.
    ///
    /// A picture is not a measurement. The standard says how finely a room must be SAMPLED for its
    /// figures to mean anything; it does not say how coarsely the result may be drawn, and a plan
    /// rendered at 20 × 16 would throw away detail for nothing. The bands are thresholds in lux, so
    /// a cell reads the same colour on either grid and there is no second number on show.
    pub fn drawn(&self) -> &'a LuxGrid {
        self.grid
    }

    /// The mask belonging to [`reported`](Self::reported) — which cells of it are measurable.
    ///
    /// EVERYTHING THAT WALKS CELLS NEEDS THIS. A masked grid's `avg`, `min` and `max` are computed
    /// over the kept cells, but its `values` still holds every reading — including the ones taken
    /// inside a cupboard. The extremes and the percentiles scan `values` themselves, so without
    /// the mask they would quote a 0 lx reading from inside the furniture as the room's minimum
    /// while the summary two rows above says 108 lx. That is the same defect the buried-cell fix
    /// removed from the headline figures, reappearing one section lower down.
    pub fn reported_mask(&self) -> &'a [bool] {
        if self.grid_en.is_some() { self.mask_en } else { self.mask }
    }
}

/// Everything the layout needs, gathered so it touches no UI types.
///
/// THE ROOMS ARE A LIST. A report used to be one room's numbers, because a calculation used to
/// produce one room's numbers — and a plan with three rooms came out as one plot spanning all
/// three with a bounding box around them.
pub struct Input<'a> {
    pub rooms: Vec<RoomInput<'a>>,
    /// Building-wide: `surface_report_on` groups by MATERIAL, so there is no per-room answer to be
    /// had from it.
    pub surfaces: &'a [SurfaceResult],
    pub maintenance: Maintenance,
    pub eye_height: f32,
    pub room_height: f32,
    pub materials: Vec<(String, f32)>,
    pub unassigned: usize,
    pub ramp: fn(f32) -> (f32, f32, f32),
    pub mask: Vec<bool>,
}

impl<'a> Input<'a> {
    /// A KEY THAT CHANGES WHEN THE DOCUMENT WOULD — so the preview can be laid out once and kept.
    ///
    /// Reported as: *"the app starts lagging once the report window is open."* Laying the document
    /// out is 123 ms on the owner's three-room plan, in a release build, and it was being done on
    /// EVERY FRAME the dialog was open — eight frames a second before the dialog has drawn a single
    /// widget. The preview is the document, which is the right design and the reason it is
    /// trustworthy; it is not a reason to rebuild it sixty times a second when nothing has moved.
    ///
    /// `calc` is [`crate::light::CalcJob::fingerprint`] of the calculation on screen, and it stands
    /// in for the per-cell arrays. Hashing those directly would be tens of thousands of values per
    /// frame to detect a change that can only happen when a calculation lands — and a calculation
    /// landing always brings a new fingerprint with it.
    ///
    /// THE DESTRUCTURES BELOW HAVE NO `..`, for the same reason the calculation's fingerprint has
    /// none: a field added to the report's input must not compile until somebody has decided
    /// whether the preview should redraw for it. The failure otherwise is quiet and maddening —
    /// a control that does nothing until you close the dialog and open it again.
    pub fn preview_key(&self, opt: &Options, calc: u64) -> u64 {
        let Input { rooms, surfaces, maintenance, eye_height, room_height, materials, unassigned, ramp, mask } =
            self;
        let mut h = crate::light::Fnv::new();
        h.u64(calc);
        // Every control in the dialog lives in `Options`, so its serialisation covers all of them
        // at once — including the ones added next month.
        crate::light::hash_json(&mut h, "opt", opt);
        // …except the image BYTES, which `Options` deliberately does not serialise. Whether they
        // have arrived changes the document: an image the writer has not loaded yet is drawn as an
        // empty box, and it must stop being one the moment it loads.
        h.u64(opt.images.iter().filter(|i| i.jpeg.is_some()).count() as u64);
        h.u64(opt.logos.iter().filter(|i| i.jpeg.is_some()).count() as u64);

        crate::light::hash_json(&mut h, "maint", maintenance);
        h.f32(*eye_height);
        h.f32(*room_height);
        crate::light::hash_json(&mut h, "materials", materials);
        h.u64(*unassigned as u64);
        h.u64(mask.len() as u64);
        // A FUNCTION POINTER — which palette the false colours are read through. It is not in
        // `Options` (it belongs to the light panel) and it repaints every field in the report.
        h.u64(*ramp as *const () as usize as u64);

        crate::light::hash_json(&mut h, "surfaces", surfaces);
        h.u64(rooms.len() as u64);
        for r in rooms {
            let RoomInput {
                name,
                grid,
                plane,
                grid_en,
                plane_en,
                mask_en,
                mask,
                poly,
                fixtures,
                installation,
                cylindrical_avg,
                schedule,
            } = r;
            h.str(name);
            // The cells are `calc`'s business; the SHAPE of the grid is cheap and worth having.
            h.u64(grid.cols as u64);
            h.u64(grid.rows as u64);
            h.f64(grid.avg);
            h.f64(grid.min);
            h.f64(grid.max);
            crate::light::hash_json(&mut h, "plane", plane);
            // THE EN GRID IS PRINTED, so it belongs in the key that decides whether the page has to
            // be laid out again. It moves with the working grid in practice — both come out of one
            // calculation — but "in practice" is how a stale preview gets shipped.
            h.u64(grid_en.map(|g| g.cols as u64).unwrap_or(0));
            h.u64(grid_en.map(|g| g.rows as u64).unwrap_or(0));
            h.f64(grid_en.map(|g| g.avg).unwrap_or(0.0));
            h.f64(grid_en.map(|g| g.min).unwrap_or(0.0));
            h.f64(grid_en.map(|g| g.max).unwrap_or(0.0));
            crate::light::hash_json(&mut h, "plane_en", plane_en);
            h.u64(mask.len() as u64);
            // The EN mask decides which cells the extremes and percentiles read, so it is an input
            // to the page like any other.
            h.u64(mask_en.len() as u64);
            h.u64(mask_en.iter().filter(|k| !**k).count() as u64);
            h.u64(poly.len() as u64);
            crate::light::hash_json(&mut h, "fixtures", fixtures);
            crate::light::hash_json(&mut h, "installation", installation);
            crate::light::hash_json(&mut h, "ez", cylindrical_avg);
            crate::light::hash_json(&mut h, "schedule", schedule);
        }
        h.finish()
    }

    /// The whole scheme's schedule, rooms merged.
    pub fn total_schedule(&self) -> Vec<ScheduleRow> {
        let mut out: Vec<ScheduleRow> = Vec::new();
        for r in &self.rooms {
            for row in &r.schedule {
                match out.iter_mut().find(|x| x.profile == row.profile) {
                    Some(x) => x.count += row.count,
                    None => out.push(row.clone()),
                }
            }
        }
        out.sort_by(|a, b| b.count.cmp(&a.count).then(a.profile.cmp(&b.profile)));
        out
    }
}

const INK: [u8; 3] = [17, 17, 17];
const FAINT: [u8; 3] = [125, 125, 125];
const RULE: [u8; 3] = [200, 200, 200];

/// The box a header or footer logo is fitted into, in points — 120 x 24 pt, about 42 x 8.5 mm.
///
/// It is a BOX, not a size: the image keeps its own proportions inside it, so a tall logo comes out
/// 24 pt high and narrow rather than squashed. The height is what the band can hold without the
/// running text moving; the width is what is left beside a project name on A4.
pub const LOGO_W: f64 = 120.0;
pub const LOGO_H: f64 = 24.0;

/// Two thirds of the page, as asked for — a drawing is the subject of the page it is on.
const TWO_THIRDS: f64 = 0.666;
/// Height of the banded legend and its labels.
const LEGEND_H: f64 = 46.0;
/// The size the grid's figures are printed at when they fit — and the target the coarsening
/// works back from. Below about five points a table of four-digit numbers stops being readable.
const GRID_PT: f64 = 5.0;

const MARGIN: f64 = 48.0;
const HEAD_H: f64 = 34.0;
const FOOT_H: f64 = 30.0;

/// A page being filled: where the pen is, and what has been put down.
struct Cursor {
    page: Page,
    pages: Vec<Page>,
    y: f64,
    /// The whole page, for things sized against it rather than against what is left.
    page_w: f64,
    page_h: f64,
    /// Content box.
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    /// ONE SCALE FOR THE WHOLE REPORT — points per metre.
    ///
    /// Asked for as: *"make the grids scale proportional to that of the layout. since we are
    /// showing the rooms illuminance in the grid format they should be comparable."*
    ///
    /// Every drawing used to be fitted to two thirds of the page on its own, which meant a 4 m
    /// store cupboard and a 40 m hall came out THE SAME SIZE ON THE PAGE. Bound together in one
    /// document that is actively misleading: the eye reads two drawings of the same size as two
    /// spaces of the same size, and the only thing saying otherwise is a dimension in small type
    /// underneath. The results field was worse — it was fitted by CELL COUNT rather than by metres,
    /// so the same room calculated at a finer grid drew LARGER THAN ITSELF.
    ///
    /// So the scale is chosen ONCE, from the room that needs the most, and every drawing in the
    /// report is set at it: the layout, the results field and the numeric grid, which is what makes
    /// a room's three pages line up with each other as well as with the other rooms. A room half
    /// the size draws half the size. It is stated on each drawing, so nobody has to infer it.
    scale: f64,
    /// EVERY TEXT SIZE IN THE REPORT, MULTIPLIED BY THIS.
    ///
    /// Asked for as: *"for the report i want font size controls."* One number for the whole
    /// document, so the hierarchy a reader navigates by — cover over chapter over heading over row
    /// over note — survives being made bigger or smaller. Applied in `push`, which is the single
    /// point every piece of text goes through, and in `lh` for the spacing that has to move with
    /// it.
    ///
    /// NOT the drawings THEMSELVES. Their annotations scale, because those are text; the plans do
    /// not, because they are at a stated 1:100 and a scale bar that lies is worse than small type.
    text_scale: f64,
}

/// The scale every drawing in the report is set at, points per metre.
///
/// Chosen from the LARGEST room — the one that has to fit — so nothing overflows and everything
/// else comes out proportionally smaller. Room extents are the working plane's, which is the
/// rectangle the results field and the grid are actually drawn over.
fn common_scale(inp: &Input, page_w: f64, page_h: f64) -> f64 {
    let (target_w, target_h) = (page_w * TWO_THIRDS, page_h * TWO_THIRDS);
    let mut k = f64::INFINITY;
    for r in &inp.rooms {
        let (w, d) = (r.plane.width as f64, r.plane.depth as f64);
        if w > 0.0 && d > 0.0 {
            k = k.min((target_w / w).min(target_h / d));
        }
    }
    // No room with a size — nothing will be drawn, and any finite number will do.
    if k.is_finite() && k > 0.0 {
        k
    } else {
        1.0
    }
}

/// The scale as a ratio a drafter reads — `1:200`, rounded to something sayable.
///
/// A drawing on a page is not to a round scale by construction, and quoting `1:187` implies a
/// precision that is not there. Rounded UP through the usual series so the stated ratio is never
/// finer than the drawing actually is.
fn scale_note(k: f64) -> String {
    // points per metre → metres per point → the denominator, since 1 pt = 1/72 inch = 0.352778 mm.
    let denom = 1000.0 / (k * (25.4 / 72.0));
    const SERIES: [f64; 14] = [
        1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0,
    ];
    let pick = SERIES.iter().copied().find(|s| *s >= denom).unwrap_or(10_000.0);
    format!("1:{pick:.0}")
}

impl Cursor {
    fn new(w: f64, h: f64, has_head: bool, has_foot: bool) -> Self {
        let top = MARGIN + if has_head { HEAD_H } else { 0.0 };
        let bottom = h - MARGIN - if has_foot { FOOT_H } else { 0.0 };
        Self {
            page: Page::default(),
            pages: Vec::new(),
            y: top,
            page_w: w,
            page_h: h,
            left: MARGIN,
            right: w - MARGIN,
            top,
            bottom,
            // Replaced by `layout` once it has seen the rooms; a cursor built for anything else
            // draws nothing scaled.
            scale: 1.0,
            // …and likewise: `layout` sets this from the options.
            text_scale: 1.0,
        }
    }

    fn width(&self) -> f64 {
        self.right - self.left
    }

    /// Start a new page, keeping the one just filled.
    fn brk(&mut self) {
        self.pages.push(std::mem::take(&mut self.page));
        self.y = self.top;
    }

    /// Break to a new page unless `h` points are available AND something is already on this one.
    ///
    /// A figure that will not fit gets a page of its own rather than being shrunk into the gap at
    /// the bottom of this one — but an EMPTY page is never broken, or a figure taller than the
    /// content box would break for ever.
    fn need_or_break(&mut self, h: f64) {
        if self.y + h > self.bottom && !self.page.items.is_empty() {
            self.brk();
        }
    }

    /// Make sure `h` points are available, breaking the page if not.
    fn need(&mut self, h: f64) {
        if self.y + h > self.bottom {
            self.brk();
        }
    }

    /// THE ONE PLACE A TEXT SIZE IS DECIDED.
    ///
    /// Asked for as: *"for the report i want font size controls."* Every size in this file stays
    /// written as the point size it is at 100%, and the multiply happens HERE — because a scale
    /// applied at each of the twenty-odd places text is emitted is a scale somebody will forget at
    /// the twenty-first, and the symptom is one line of a report that does not grow with the rest.
    ///
    /// It follows that `text()` must NOT scale as well, or every size comes out squared.
    ///
    /// ONE KIND OF TEXT IS EXEMPT, through [`push_unscaled`](Self::push_unscaled).
    fn push(&mut self, it: Item) {
        let it = match it {
            Item::Text { x, y, size, font, rgb, align, text } => {
                Item::Text { x, y, size: size * self.text_scale, font, rgb, align, text }
            }
            other => other,
        };
        self.page.items.push(it);
    }

    /// TEXT WHOSE SIZE IS DICTATED BY THE GEOMETRY IT SITS IN, and so must not be scaled.
    ///
    /// There is exactly one: the point values printed inside the cells of the results field. Their
    /// size is computed from the cell — `(cell · 0.26).clamp(4, 8)` — because it has to FIT the
    /// cell, and a value that overflows its cell collides with its neighbours and stops being
    /// readable at all. Enlarging it by the report's text control would defeat the only rule it
    /// has.
    ///
    /// That is the same argument the drawings themselves are exempt under, and it is the reason
    /// this is a named door rather than an `if` inside `push`: an exemption you have to ask for by
    /// name cannot be granted by accident.
    fn push_unscaled(&mut self, it: Item) {
        self.page.items.push(it);
    }

    /// A vertical step that exists to make room for TEXT, scaled with it.
    ///
    /// The sizes and the spacings in this file were written as one set of numbers and only look
    /// independent: `row` advances 10 pt because its text is 9 pt. Grow the type without growing
    /// the rhythm and the lines collide. Geometry — a plot's width, a logo box — goes nowhere near
    /// this: the drawings are at a stated 1:100 and must not move.
    fn lh(&self, v: f64) -> f64 {
        v * self.text_scale
    }

    fn text(&mut self, x: f64, size: f64, font: Font, align: Align, rgb: [u8; 3], s: &str) {
        self.push(Item::Text { x, y: self.y, size, font, rgb, align, text: s.to_string() });
    }

    /// A section heading, with the rule under it.
    fn heading(&mut self, s: &str) {
        self.need(self.lh(40.0));
        self.y += self.lh(14.0);
        self.text(self.left, 12.0, Font::Bold, Align::Left, INK, s);
        self.y += self.lh(5.0);
        let (l, r, y) = (self.left, self.right, self.y);
        self.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: RULE, width: 0.6 });
        self.y += self.lh(14.0);
    }

    /// A label on the left and a value on the right — the shape every figure in this report takes.
    fn row(&mut self, k: &str, v: &str) {
        self.need(self.lh(16.0));
        self.y += self.lh(10.0);
        self.text(self.left, 9.0, Font::Regular, Align::Left, INK, k);
        self.text(self.right, 9.0, Font::Regular, Align::Right, INK, v);
        self.y += self.lh(4.0);
        let (l, r, y) = (self.left, self.right, self.y);
        self.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: [235, 235, 235], width: 0.4 });
    }

    fn note(&mut self, s: &str) {
        self.need(self.lh(14.0));
        self.y += self.lh(11.0);
        self.text(self.left, 8.0, Font::Regular, Align::Left, FAINT, s);
        self.y += self.lh(2.0);
    }

    fn finish(mut self) -> Vec<Page> {
        self.pages.push(std::mem::take(&mut self.page));
        self.pages
    }
}

thread_local! {
    /// HOW MANY TIMES THIS THREAD HAS LAID A DOCUMENT OUT.
    ///
    /// The only way to observe that something did NOT happen. The preview cache exists to stop
    /// `layout` running sixty times a second, and a test that watches the cache KEY cannot see the
    /// difference: a key that never matches is just as stable as one that always does, so a cache
    /// missing on every single frame looks identical from outside. That is not hypothetical — the
    /// first version of the test here watched the key and passed against a deliberately broken
    /// cache. This counts the actual work.
    ///
    /// Thread-local rather than global because tests run in parallel in one process, and a shared
    /// counter would have every test measuring every other test's work.
    ///
    /// One increment on a function that costs a hundred milliseconds is not worth hiding behind
    /// `#[cfg(test)]` — and a counter that exists only in test builds is one nobody can read while
    /// actually debugging a slow dialog.
    pub static LAYOUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Lay the whole report out.
pub fn layout(inp: &Input, opt: &Options) -> Doc {
    LAYOUTS.with(|n| n.set(n.get() + 1));
    let (w, h) = opt.page.points();
    let mut doc = Doc::new((w, h), opt.title.clone());
    // ONE IMAGE TABLE, THREE LISTS. The PDF holds a single table of images, so the renders go in
    // first, then the logos, then the covers — a logo's index into the table is `logo_base + i`
    // and a cover's is `cover_base + i`. Kept apart in the options because they are different
    // things chosen from different buttons, and mixing them is what the reports were about: "i
    // have to add image at the render image addition then add them in the logo", and later the
    // same of the cover. Kept together HERE because the file format has one place to put them.
    for im in opt.images.iter().chain(opt.logos.iter()).chain(opt.covers.iter()) {
        if let Some((bytes, iw, ih)) = &im.jpeg {
            doc.images.push(Jpeg { bytes: bytes.clone(), w: *iw, h: *ih });
        }
    }
    let logo_base = opt.images.iter().filter(|i| i.jpeg.is_some()).count();
    let cover_base = logo_base + opt.logos.iter().filter(|i| i.jpeg.is_some()).count();

    let has_head = !opt.header.trim().is_empty() || opt.header_image.is_some();
    let has_foot =
        !opt.footer.trim().is_empty() || opt.page_numbers || opt.footer_image.is_some();

    let mut cover_pages = Vec::new();
    if opt.cover {
        cover_pages.push(cover(inp, opt, w, h, &doc, cover_base));
    }

    let mut c = Cursor::new(w, h, has_head, has_foot);
    // Chosen before anything is drawn, from every room at once — see `Cursor::scale`.
    c.scale = common_scale(inp, w, h);
    c.text_scale = opt.text_scale.clamp(
        crate::report::options::TEXT_SCALE_MIN,
        crate::report::options::TEXT_SCALE_MAX,
    );
    // The running header and footer are stamped onto finished pages further down, outside the
    // cursor, so they scale by hand like the cover does. Their band is 34 pt deep for 8 pt type, so
    // there is room for this without moving the content box.
    let ts = c.text_scale;
    // A CHAPTER PER ROOM, then the building-wide sections once.
    //
    // Three rooms used to come out as one plot with a bounding box round them, because a
    // calculation produced one room's numbers. They are separate rooms with separate fittings and
    // separate answers, and a report that merges them states an average over ground that is not
    // one space.
    let many = inp.rooms.len() > 1;
    // A BUILDING-WIDE SECTION KEEPS ITS PLACE relative to the room chapters. The order IS the
    // document, so a report with Surfaces listed first prints Surfaces first — the chapters go
    // where the first per-room section sits in the list, not always at the top.
    let first_room_at = opt.sections.iter().position(|s| is_per_room(*s)).unwrap_or(usize::MAX);
    for s in opt.sections.iter().enumerate().filter(|(i, s)| *i < first_room_at && !is_per_room(**s)).map(|(_, s)| s) {
        building_wide(&mut c, inp, opt, &doc, *s, many);
    }
    for room in &inp.rooms {
        if many || !room.name.trim().is_empty() {
            chapter(&mut c, if room.name.trim().is_empty() { "Results" } else { room.name.trim() });
        }
        for s in opt.sections.iter().filter(|s| is_per_room(**s)) {
            match s {
                Section::Summary => summary(&mut c, room, inp.unassigned),
                Section::Installation => installation(&mut c, room, inp.maintenance, inp),
                Section::WorkingPlane => working_plane(&mut c, room, inp.eye_height),
                Section::Layout => layout_page(&mut c, room),
                Section::Results => results(&mut c, inp, room, opt),
                Section::NumericGrid => numeric_grid(&mut c, room),
                Section::Schedule => schedule(&mut c, &room.schedule, "Luminaire schedule"),
                _ => {}
            }
        }
    }
    for s in opt
        .sections
        .iter()
        .enumerate()
        .filter(|(i, s)| *i > first_room_at && !is_per_room(**s))
        .map(|(_, s)| s)
    {
        building_wide(&mut c, inp, opt, &doc, *s, many);
    }
    let mut body = c.finish();

    doc.pages = cover_pages;
    doc.pages.append(&mut body);

    // The furniture goes on LAST, once the page count is known — a footer reading "1 of 7" cannot
    // be written while it is still being decided how many there are.
    let n = doc.pages.len();
    for (i, p) in doc.pages.iter_mut().enumerate() {
        // The cover carries neither: it is a title page, and a running header on it looks like a
        // mistake rather than a choice.
        if opt.cover && i == 0 {
            continue;
        }
        if has_head {
            // The logo sits at the OUTER edge, opposite the text, which is where a letterhead puts
            // it and what leaves the middle of the line free for a long project name.
            if let Some(i) = opt.header_image.map(|i| logo_base + i) {
                if let Some(im) = doc.images.get(i) {
                    let (iw, ih) = fit(im.w as f64, im.h as f64, LOGO_W, LOGO_H);
                    p.items.push(Item::Image {
                        x: w - MARGIN - iw,
                        y: MARGIN + 10.0 - ih,
                        w: iw,
                        h: ih,
                        idx: i,
                    });
                }
            }
            p.items.push(Item::Text {
                x: MARGIN,
                y: MARGIN + 8.0,
                size: 8.0 * ts,
                font: Font::Regular,
                rgb: FAINT,
                align: Align::Left,
                text: opt.header.trim().to_string(),
            });
            p.items.push(Item::Line {
                x1: MARGIN,
                y1: MARGIN + 14.0,
                x2: w - MARGIN,
                y2: MARGIN + 14.0,
                rgb: RULE,
                width: 0.5,
            });
        }
        if has_foot {
            let fy = h - MARGIN - 6.0;
            p.items.push(Item::Line {
                x1: MARGIN,
                y1: fy - 12.0,
                x2: w - MARGIN,
                y2: fy - 12.0,
                rgb: RULE,
                width: 0.5,
            });
            if !opt.footer.trim().is_empty() {
                p.items.push(Item::Text {
                    x: MARGIN,
                    y: fy,
                    size: 8.0 * ts,
                    font: Font::Regular,
                    rgb: FAINT,
                    align: Align::Left,
                    text: opt.footer.trim().to_string(),
                });
            }
            if let Some(i) = opt.footer_image.map(|i| logo_base + i) {
                if let Some(im) = doc.images.get(i) {
                    let (iw, ih) = fit(im.w as f64, im.h as f64, LOGO_W, LOGO_H);
                    p.items.push(Item::Image {
                        x: (w - iw) * 0.5,
                        y: fy - ih + 1.0,
                        w: iw,
                        h: ih,
                        idx: i,
                    });
                }
            }
            if opt.page_numbers {
                p.items.push(Item::Text {
                    x: w - MARGIN,
                    y: fy,
                    size: 8.0 * ts,
                    font: Font::Regular,
                    rgb: FAINT,
                    align: Align::Right,
                    text: format!("{} / {}", i + 1, n),
                });
            }
        }
    }
    doc
}

/// The title page.
/// THE COVER BUILDS ITS OWN PAGE, so it does not pass through `Cursor::push` and does not get the
/// text scale for free. It applies it by hand, and that is not a wart to tidy away later — it is
/// the exact failure the comment on `push` predicted, caught on the first run: a 26 pt cover title
/// sitting unchanged over a report set at 140%.
fn cover(inp: &Input, opt: &Options, w: f64, h: f64, doc: &Doc, cover_base: usize) -> Page {
    let ts = opt.text_scale.clamp(
        crate::report::options::TEXT_SCALE_MIN,
        crate::report::options::TEXT_SCALE_MAX,
    );
    let mut p = Page::default();
    let mut y = h * 0.34;

    // The image sits ABOVE the title when there is one, so the eye lands on the picture and then
    // reads what it is — which is the order a cover is looked at.
    // The cover picture comes from the COVERS list, which has its own Add button — see the note on
    // the image table above.
    if let Some(i) = opt.cover_image.map(|i| cover_base + i) {
        if let Some(im) = doc.images.get(i) {
            let box_w = w - MARGIN * 2.0;
            let box_h = h * 0.34;
            let (iw, ih) = fit(im.w as f64, im.h as f64, box_w, box_h);
            p.items.push(Item::Image {
                x: (w - iw) * 0.5,
                y: h * 0.12 + (box_h - ih) * 0.5,
                w: iw,
                h: ih,
                idx: i,
            });
            y = h * 0.12 + box_h + 56.0;
        }
    }

    let title = if opt.title.trim().is_empty() { "Lighting report" } else { opt.title.trim() };
    p.items.push(Item::Text {
        x: w * 0.5,
        y,
        size: 26.0 * ts,
        font: Font::Bold,
        rgb: INK,
        align: Align::Centre,
        text: title.to_string(),
    });
    y += 22.0;
    if !opt.subtitle.trim().is_empty() {
        p.items.push(Item::Text {
            x: w * 0.5,
            y,
            size: 12.0 * ts,
            font: Font::Regular,
            rgb: [80, 80, 80],
            align: Align::Centre,
            text: opt.subtitle.trim().to_string(),
        });
        y += 20.0;
    }
    p.items.push(Item::Line {
        x1: w * 0.5 - 60.0,
        y1: y + 6.0,
        x2: w * 0.5 + 60.0,
        y2: y + 6.0,
        rgb: RULE,
        width: 0.8,
    });
    p.items.push(Item::Text {
        x: w * 0.5,
        y: h - MARGIN - 18.0,
        size: 8.0 * ts,
        font: Font::Regular,
        rgb: FAINT,
        align: Align::Centre,
        text: format!(
            "SIMLUX {} · maintained values, maintenance factor {:.2}",
            env!("SIMLUX_BUILD"),
            inp.rooms.first().map(|r| r.grid.maintenance).unwrap_or(1.0)
        ),
    });
    p
}

/// Scale `(iw, ih)` into `(bw, bh)` keeping its aspect — the rule every image on the page follows.
fn fit(iw: f64, ih: f64, bw: f64, bh: f64) -> (f64, f64) {
    if iw <= 0.0 || ih <= 0.0 {
        return (0.0, 0.0);
    }
    let k = (bw / iw).min(bh / ih);
    (iw * k, ih * k)
}


/// Sections that describe ONE room, and so repeat inside each room's chapter.
/// Emit one BUILDING-WIDE section, wherever the list has put it.
///
/// One function called from both sides of the room chapters, so a section cannot be movable in one
/// direction and not the other — which is the shape the Whole scheme bug had.
fn building_wide(
    c: &mut Cursor,
    inp: &Input,
    opt: &Options,
    doc: &Doc,
    s: Section,
    many_rooms: bool,
) {
    match s {
        Section::Materials => materials(c, inp),
        Section::Surfaces => surfaces(c, inp),
        Section::Renders => renders(c, opt, doc),
        // Only when there is more than one room to total. On a single-room report it would be the
        // same schedule printed a second time under a grander heading.
        Section::WholeScheme if many_rooms => {
            chapter(c, "Whole scheme");
            schedule(c, &inp.total_schedule(), "Luminaire schedule — all rooms");
        }
        _ => {}
    }
}

fn is_per_room(s: Section) -> bool {
    matches!(
        s,
        Section::Summary
            | Section::Installation
            | Section::WorkingPlane
            | Section::Layout
            | Section::Results
            | Section::NumericGrid
            | Section::Schedule
    )
}

/// A room's chapter heading — only when there is more than one, or it is noise on a single-room
/// report that already says which room it is about on the cover.
fn chapter(c: &mut Cursor, name: &str) {
    c.need_or_break(c.lh(60.0));
    if !c.page.items.is_empty() {
        c.brk();
    }
    c.y += c.lh(8.0);
    c.text(c.left, 17.0, Font::Bold, Align::Left, INK, name);
    c.y += c.lh(7.0);
    let (l, r, y) = (c.left, c.right, c.y);
    c.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: [120, 120, 120], width: 1.2 });
    c.y += c.lh(10.0);
}

fn summary(c: &mut Cursor, room: &RoomInput, unassigned: usize) {
    // ONE GRID, and it is the standard's — see `RoomInput::reported`.
    let g = room.reported();
    let p = room.reported_plane();
    c.heading("Summary");
    c.row("Average E", &format!("{:.0} lx", g.avg));
    c.row("Minimum E", &format!("{:.0} lx", g.min));
    c.row("Maximum E", &format!("{:.0} lx", g.max));
    c.row(
        "Uniformity U0 = Emin/E",
        &if g.avg > 0.0 { format!("{:.2}", g.min / g.avg) } else { "—".into() },
    );
    // WHICH GRID, ON THE ROW ITSELF. A lux figure without its grid is ambiguous — the maximum
    // especially, which is the brightest cell CENTRE and moves with spacing — and naming it here
    // costs a reader nothing while answering the question this whole change came from.
    c.row(
        if room.grid_en.is_some() { "Grid (EN 12464-1)" } else { "Grid" },
        &format!("{} x {} points at {:.2} m", g.cols, g.rows, spacing_of(p, g)),
    );
    c.row(
        "Plane",
        &format!("{:.2} x {:.2} m at {:.2} m", p.width, p.depth, p.origin.z),
    );
    if unassigned > 0 {
        c.note(&format!(
            "{unassigned} fixture(s) in this project have no photometric file assigned and \
             contribute nothing.",
        ));
    }
}

/// The grid spacing a plane and its grid imply, in metres.
///
/// The LONGER of the two axes, so a rectangular cell is described by its coarser side rather than
/// flattered by its finer one — the coarse axis is the one that decides what a maximum misses.
fn spacing_of(p: &CalcPlane, g: &LuxGrid) -> f32 {
    let sx = p.width / g.cols.max(1) as f32;
    let sy = p.depth / g.rows.max(1) as f32;
    sx.max(sy)
}

fn installation(c: &mut Cursor, room: &RoomInput, m: Maintenance, inp: &Input) {
    let Some(i) = room.installation else { return };

    // ---- General ----------------------------------------------------------------------------
    //
    // What the answer was computed UNDER, before the answer itself — the block a lighting report
    // is expected to open a room with, and the one the reference hands over under that heading.
    // Asked for as "make sure we also displaying these info in each room".
    //
    // PER ROOM, not once for the building. Mounting height, connected load and power density are
    // properties of a room's own installation; one figure quoted over three rooms describes none
    // of them.
    c.heading("General");
    c.row("Calculation algorithm", "Ray-traced, 5 diffuse bounces");
    c.row(
        "Height of luminaire plane",
        &room
            .fixtures
            .first()
            .map(|l| format!("{:.2} m", l.position.z))
            .unwrap_or_else(|| "—".into()),
    );
    c.row("Working plane height", &format!("{:.2} m", room.plane.origin.z));
    c.row("Room height", &format!("{:.2} m", inp.room_height));
    c.row("Maintenance factor", &format!("{:.2}", m.factor()));
    c.row("Luminaire luminous flux", &format!("{:.0} lm", i.total_lumens));
    c.row("Total power", &format!("{:.1} W", i.total_watts));
    // W/m² AND W/m² PER 100 LX. The second is what a scheme is actually judged on: 10 W/m² holding
    // a room at 200 lx and 10 W/m² holding one at 600 lx are not comparable installations, and
    // regulations are written against the normalised figure for exactly that reason.
    let per_100 = if room.reported().avg > 0.0 {
        format!("   ({:.2} W/m2 per 100 lx)", i.power_density * 100.0 / room.reported().avg)
    } else {
        String::new()
    };
    c.row(
        &format!("Total power per area ({:.2} m2)", i.area_m2),
        &format!("{:.2} W/m2{per_100}", i.power_density),
    );

    // ---- the room these numbers are about ----------------------------------------------------
    //
    // "the room details need to there for each rooms result." These were printed ONCE for the
    // whole scheme, which is where they come from — but a reader working through one room's
    // chapter should not have to leaf back to a page at the front to learn what reflectance the
    // walls were given. They are the stated conditions of THIS room's figures.
    c.heading("Room & materials");
    c.row("Room size", &format!("{:.2} × {:.2} m", room.plane.width, room.plane.depth));
    c.row("Floor area", &format!("{:.2} m2", i.area_m2));
    for (name, r) in &inp.materials {
        c.row(&format!("Reflectance — {name}"), &format!("{:.0} %", r * 100.0));
    }

    // ---- and the detail ----------------------------------------------------------------------
    c.heading("Installation");
    c.row("Luminaires", &format!("{}", i.count));
    c.row("Connected load", &format!("{:.1} W", i.total_watts));
    c.row("Installed flux", &format!("{:.0} lm", i.total_lumens));
    c.row("Area", &format!("{:.2} m2", i.area_m2));
    c.row("Power density", &format!("{:.2} W/m2", i.power_density));
    c.row("Efficacy", &format!("{:.0} lm/W", i.efficacy));
    c.row(
        "Maintenance",
        &format!(
            "{:.2}  (LLMF {:.2} · LSF {:.2} · LMF {:.2} · RSMF {:.2})",
            m.factor(),
            m.llmf,
            m.lsf,
            m.lmf,
            m.rsmf
        ),
    );
}

/// THE LUMINAIRE SCHEDULE — what is in this room, by type.
///
/// Asked for as "the report should also show information of all the lights, like there
/// manufacturer, their specifications etc." A lighting report that states an illuminance without
/// saying what produced it cannot be checked, ordered from, or handed to an installer.
///
/// BY TYPE, not by fixture: "48 × OCULUS GRANDE 2.0" is what gets ordered and wired. Manufacturer
/// and catalogue number come out of the photometric file's own header — so a file that declares
/// none shows a dash, which is the file's omission rather than the report's.
fn schedule(c: &mut Cursor, rows: &[ScheduleRow], title: &str) {
    if rows.is_empty() {
        return;
    }
    c.heading(title);

    let cols = [0.06, 0.34, 0.50, 0.62, 0.72, 0.84, 1.0];
    let w = c.width();
    let left = c.left;
    // Captured by VALUE, not borrowed from the cursor — the cursor is written to on every row.
    let at = move |f: f64| left + w * f;

    c.need(20.0);
    c.y += 10.0;
    for (i, (h, a)) in [
        ("Qty", Align::Right),
        ("Fitting", Align::Left),
        ("Manufacturer", Align::Left),
        ("Watts", Align::Right),
        ("Flux", Align::Right),
        ("lm/W", Align::Right),
        ("Size (mm)", Align::Right),
    ]
    .into_iter()
    .enumerate()
    {
        let x = if a == Align::Right { at(cols[i]) } else { at(if i == 0 { 0.0 } else { cols[i - 1] }) };
        c.push(Item::Text {
            x,
            y: c.y,
            size: 8.0,
            font: Font::Bold,
            rgb: FAINT,
            align: a,
            text: h.to_string(),
        });
    }
    c.y += 4.0;
    let (l, r, y) = (c.left, c.right, c.y);
    c.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: RULE, width: 0.6 });

    for row in rows {
        c.need(24.0);
        c.y += 11.0;
        let dash = |v: f64, unit: &str, dp: usize| -> String {
            if v > 0.0 {
                format!("{v:.dp$}{unit}")
            } else {
                "—".into()
            }
        };
        let cells: [(f64, Align, String); 7] = [
            (at(cols[0]), Align::Right, format!("{}", row.count)),
            (at(cols[0]), Align::Left, row.profile.clone()),
            (at(cols[1]), Align::Left, if row.manufacturer.trim().is_empty() {
                "—".into()
            } else {
                row.manufacturer.trim().to_string()
            }),
            (at(cols[3]), Align::Right, dash(row.watts, " W", 1)),
            (at(cols[4]), Align::Right, dash(row.lumens, " lm", 0)),
            (at(cols[5]), Align::Right, row.efficacy().map(|e| format!("{e:.0}")).unwrap_or_else(|| "—".into())),
            (at(cols[6]), Align::Right, {
                let (l, w2, h2) = row.size_m;
                if l > 0.0 || w2 > 0.0 {
                    format!("{:.0} × {:.0} × {:.0}", l * 1000.0, w2 * 1000.0, h2 * 1000.0)
                } else {
                    "—".into()
                }
            }),
        ];
        for (i, (x, a, t)) in cells.into_iter().enumerate() {
            let xx = if i == 1 { at(cols[0]) + 6.0 } else { x };
            c.push(Item::Text {
                x: xx,
                y: c.y,
                size: 8.0,
                font: if i == 1 { Font::Bold } else { Font::Regular },
                rgb: INK,
                align: a,
                text: t,
            });
        }
        // The catalogue number and lamp under the name, where there is one — they belong to the
        // fitting rather than to a column of their own, and a schedule of eight columns on A4 is
        // a schedule nobody can read.
        let mut sub = Vec::new();
        if !row.catalogue.trim().is_empty() && row.catalogue.trim() != row.profile.trim() {
            sub.push(format!("cat. {}", row.catalogue.trim()));
        }
        if !row.lamp.trim().is_empty() {
            sub.push(row.lamp.trim().to_string());
        }
        if !sub.is_empty() {
            c.y += 9.0;
            c.push(Item::Text {
                x: at(cols[0]) + 6.0,
                y: c.y,
                size: 7.0,
                font: Font::Regular,
                rgb: FAINT,
                align: Align::Left,
                text: sub.join(" · "),
            });
        }
        c.y += 3.0;
        let (l, r, y) = (c.left, c.right, c.y);
        c.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: [235, 235, 235], width: 0.4 });
    }

    let total_w: f64 = rows.iter().map(|r| r.total_watts()).sum();
    let total_n: usize = rows.iter().map(|r| r.count).sum();
    c.y += 10.0;
    c.text(c.left, 8.0, Font::Bold, Align::Left, INK, &format!("{total_n} fitting(s)"));
    c.push(Item::Text {
        x: c.right,
        y: c.y,
        size: 8.0,
        font: Font::Bold,
        rgb: INK,
        align: Align::Right,
        text: format!("{total_w:.1} W connected"),
    });
    c.y += 4.0;
}

fn materials(c: &mut Cursor, inp: &Input) {
    if inp.materials.is_empty() {
        return;
    }
    c.heading("Room & materials");
    c.row("Room height", &format!("{:.2} m", inp.room_height));
    for (name, r) in &inp.materials {
        c.row(&format!("Reflectance — {name}"), &format!("{:.0} %", r * 100.0));
    }
}

fn working_plane(c: &mut Cursor, room: &RoomInput, eye_height: f32) {
    // The REPORTED grid — percentiles and diversity are figures like any other. See
    // `RoomInput::reported`.
    let g = room.reported();
    c.heading("Working plane");
    // THE MEASURABLE CELLS ONLY. These percentiles used to be taken over every cell in the grid,
    // masked or not — so on a room with furniture in it the 10th percentile was pulled down by
    // readings taken inside a cupboard, while the minimum three rows above already excluded them.
    // A pre-existing defect, and the same one that made a room's minimum 0 lx.
    let mask = room.reported_mask();
    let keep = |i: usize| mask.get(i).copied().unwrap_or(true);
    let mut v: Vec<f64> =
        g.values.iter().enumerate().filter(|(i, _)| keep(*i)).map(|(_, v)| *v).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| -> f64 {
        if v.is_empty() {
            return 0.0;
        }
        v[(((v.len() - 1) as f64) * p).round() as usize]
    };
    c.row("Median", &format!("{:.0} lx", pct(0.5)));
    c.row("10th / 90th percentile", &format!("{:.0} / {:.0} lx", pct(0.1), pct(0.9)));
    c.row(
        "Diversity Ud = Emin/Emax",
        &if g.max > 0.0 { format!("{:.2}", g.min / g.max) } else { "—".into() },
    );
    if let Some(cy) = room.cylindrical_avg {
        c.row(&format!("Cylindrical E at {eye_height:.1} m"), &format!("{cy:.0} lx"));
    }
}

/// THE RESULT, SCALED TO THE PAGE AND DRAWN SMOOTH.
///
/// "see how the false color shows up as grid make it smooth like in the reference." The field was
/// drawn one rectangle per grid point, so a 125-column room came out as visible cross-hatching —
/// the sampling grid rather than the light. A lighting drawing shows bands with curved edges,
/// because illuminance is continuous and the grid is only where it was measured.
///
/// So the field is resampled BILINEARLY onto a much finer raster and each sample is coloured
/// through the scale. With a banded scale that gives smooth curved band boundaries — the reference
/// look — and with a continuous one, a smooth gradient. Runs of equal colour along a row are
/// merged into one rectangle, which is what keeps the file small: a banded field has a handful of
/// runs per row rather than one rectangle per sample.
fn results(c: &mut Cursor, inp: &Input, room: &RoomInput, opt: &Options) {
    let g = room.grid;
    if g.cols == 0 || g.rows == 0 {
        return;
    }
    // IN METRES, AT THE REPORT'S SCALE — not fitted to the page by cell count, which drew the same
    // room larger when it was calculated on a finer grid, and drew every room the same size
    // whatever its actual dimensions. See `Cursor::scale`.
    let plot_w = room.plane.width as f64 * c.scale;
    let plot_h = room.plane.depth as f64 * c.scale;
    if plot_w <= 0.0 || plot_h <= 0.0 {
        return;
    }

    c.need_or_break(40.0 + plot_h + LEGEND_H);
    c.heading("Results — illuminance");
    results_body(c, inp, room, opt, plot_w, plot_h);
}

/// How finely the field is resampled before the bands are traced, in samples across the plot.
///
/// The contour is drawn as one straight segment per raster cell, so this decides how faceted a
/// curve looks — not how coarse the colours are, which the bands decide. At 480 across an A4
/// content width a segment is under a point long, which is past what any reader resolves; the old
/// value of 900 was chosen when every sample became its own rectangle and it bought nothing but
/// file size.
const CONTOUR_RES: usize = 480;

/// WHAT AN OBJECT STANDING IN THE ROOM IS DRAWN IN.
///
/// A plain warm grey, deliberately outside the band palette: it has to be obviously not a lux
/// reading, because the whole point is that no reading was taken there. Light enough to sit under
/// the fitting symbols without swallowing them.
const OBJECT_FILL: [u8; 3] = [214, 211, 206];

fn results_body(
    c: &mut Cursor,
    inp: &Input,
    room: &RoomInput,
    opt: &Options,
    plot_w: f64,
    plot_h: f64,
) {
    // THE FINE GRID DRAWS, THE REPORTED GRID SCALES.
    //
    // The picture uses every cell that was calculated — a plan redrawn at the standard's spacing
    // would throw away detail for nothing, and how coarsely a result may be DRAWN is not something
    // the standard has an opinion about. But the top of an auto scale is a number the reader can
    // see, in the legend's caption, so it comes from the same grid every other figure does. One
    // number on the page, which is the whole point of quoting one grid.
    let g = room.drawn();
    let (gc, gr) = (g.cols as usize, g.rows as usize);
    let x0 = c.left + (c.width() - plot_w) * 0.5;
    let y0 = c.y;
    let room_max = room.reported().max;

    // ---- the field, resampled ------------------------------------------------------------
    //
    // Bilinear onto a raster fine enough that a contour segment is sub-point. Sampled at raster
    // CORNERS rather than centres, because the contour runs between corners.
    let nx = (gc * 4).clamp(2, CONTOUR_RES);
    let ny = ((nx as f64 * plot_h / plot_w).round() as usize).clamp(2, CONTOUR_RES);
    let mut f = vec![0.0_f64; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            // THE PAGE'S y RUNS DOWN AND A PLAN'S RUNS UP.
            //
            // Reported as: "it looks like the result with false color are flipped compared to the
            // lighting layout." It was. Grid row 0 is the plane's MINIMUM y — the bottom of the
            // room — and this drew it at the top of the page, while the layout page has always
            // flipped correctly. The two pages showed the same room mirrored, which is worse than
            // either being wrong alone: a reader checks one against the other, and both looked
            // plausible.
            let gy = (1.0 - j as f64 / ny as f64) * (gr - 1) as f64;
            let gx = (i as f64 / nx as f64) * (gc - 1) as f64;
            f[j * (nx + 1) + i] = bilinear(g, gx, gy);
        }
    }

    // ---- the bands, lowest first ------------------------------------------------------------
    //
    // Each band is painted as the whole region at or above its floor, over the top of the bands
    // below it. Painting SUPERSETS in order is what removes the need for holes: a bright pool
    // inside a dimmer band is simply painted later, so no band has to know what is inside it.
    // A CONTINUOUS SCALE IS DRAWN AS MANY BANDS. `Scale::edges` returns just `[0, top]` when no
    // bands are set — one band, which through a contour renderer is one flat colour over the whole
    // room. That is not "continuous", it is the field erased, and it is exactly what the first
    // version of this did. Quantising into enough levels that the steps fall below what anyone can
    // see gives a gradient and keeps ONE code path for both kinds of scale.
    const SMOOTH_LEVELS: usize = 48;
    let edges: Vec<f64> = if opt.scale.bands.is_empty() {
        let top = opt.scale.top_lx(room_max);
        (0..=SMOOTH_LEVELS).map(|k| top * k as f64 / SMOOTH_LEVELS as f64).collect()
    } else {
        opt.scale.edges(room_max)
    };
    let sw = plot_w / nx as f64;
    let sh = plot_h / ny as f64;
    let at = |i: f64, j: f64| (x0 + i * sw, y0 + j * sh);
    // WHICH RASTER CELLS ARE IN THE ROOM — computed once, shared by every band.
    //
    // A cell outside the room is simply never painted, so nothing has to be covered up afterwards.
    // The first version DID cover it up, with one white shape carrying the plot rectangle and the
    // room outline as two rings under the even-odd rule, and that was a mistake: the PDF filled it
    // correctly and the on-screen preview — which has no even-odd and filled each ring in turn —
    // painted the whole room white. Two renderers, one item, different pictures. Nothing here
    // needs a fill rule any more, so they cannot disagree.
    let p = room.plane;
    // WHICH RASTER CELLS THE PLANE ACTUALLY EXISTS ON — the calculation's own mask, resampled.
    //
    // Separate from the room outline, and needed on top of it. Reported as: *"in our report the
    // light at the furniture is 0 while in dialux it shows theres light on the surface."* The four
    // blocks on that plan are 1.00 m tall and the working plane is at 0.80 m, so the plane runs
    // THROUGH them: those readings are taken inside a solid and are 0 lx because an enclosed point
    // receives nothing. The summary already excludes them — that was the fix for a room minimum of
    // 0 lx — but this drawing did not, because it tested the outline and stopped there. So the
    // picture said "this part of the room is dark" where the numbers beside it said "there is no
    // reading here", and dark is the more believable of the two.
    let measurable: Vec<bool> = if room.mask.is_empty() {
        vec![true; nx * ny]
    } else {
        let mut v = vec![true; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let gx = ((i as f64 + 0.5) / nx as f64 * gc as f64) as usize;
                let gy = ((1.0 - (j as f64 + 0.5) / ny as f64) * gr as f64) as usize;
                v[j * nx + i] =
                    room.mask.get(gy.min(gr - 1) * gc + gx.min(gc - 1)).copied().unwrap_or(true);
            }
        }
        v
    };
    let inside: Vec<bool> = if room.poly.len() >= 3 {
        let mut v = vec![false; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let wx = p.origin.x as f64 + (i as f64 + 0.5) / nx as f64 * p.width as f64;
                // Flipped like the field itself: raster row 0 is the top of the page, which is the
                // room's MAXIMUM y.
                let wy =
                    p.origin.y as f64 + (1.0 - (j as f64 + 0.5) / ny as f64) * p.depth as f64;
                // BOTH tests: in the room, AND somewhere a reading could be taken.
                v[j * nx + i] = crate::factory::point_in_poly(room.poly, wx as f32, wy as f32)
                    && measurable[j * nx + i];
            }
        }
        v
    } else if !room.mask.is_empty() {
        // NO OUTLINE, BUT A MASK. In a real calculation a mask only exists where an outline does,
        // so this is the belt to that braces — but the cost of getting it wrong is colour claiming
        // there is light on ground the room does not cover, and that is not something to leave to
        // an invariant holding somewhere else. Cell by cell, which is a staircase at GRID
        // resolution; a staircase in the right place beats a smooth edge in the wrong one.
        let mut v = vec![true; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let gx = ((i as f64 + 0.5) / nx as f64 * gc as f64) as usize;
                // Flipped like everything else here: raster row 0 is the top of the page, which is
                // the room's maximum y and so the LAST grid row.
                let gy = ((1.0 - (j as f64 + 0.5) / ny as f64) * gr as f64) as usize;
                v[j * nx + i] =
                    room.mask.get(gy.min(gr - 1) * gc + gx.min(gc - 1)).copied().unwrap_or(true);
            }
        }
        v
    } else {
        vec![true; nx * ny]
    };
    let to_page = |v: &glam::Vec2| -> (f64, f64) {
        (
            x0 + (v.x as f64 - p.origin.x as f64) / p.width as f64 * plot_w,
            y0 + plot_h - (v.y as f64 - p.origin.y as f64) / p.depth as f64 * plot_h,
        )
    };

    for (k, pair) in edges.windows(2).enumerate() {
        let mid = (pair[0] + pair[1]) * 0.5;
        let fill = band_fill(inp, opt, k, opt.scale.t_for(mid, room_max));
        if k == 0 {
            // THE FLOOR BAND IS THE ROOM ITSELF — its own outline, filled, so the edge of the
            // colour is the edge of the room exactly. Every brighter band is painted over it.
            if room.poly.len() >= 3 {
                c.push(Item::Poly {
                    rings: vec![room.poly.iter().map(to_page).collect()],
                    fill,
                });
            } else if !room.mask.is_empty() {
                // Mask only, so the floor band gets the same treatment as every other one: every
                // cell is above a threshold of minus infinity, so this fills exactly where the
                // mask says the room is and nowhere else.
                paint_above(c, &f, &inside, nx, ny, f64::NEG_INFINITY, fill, &at);
            } else {
                c.push(Item::Rect { x: x0, y: y0, w: plot_w, h: plot_h, fill });
            }
            continue;
        }
        let t = pair[0];
        paint_above(c, &f, &inside, nx, ny, t, fill, &at);
    }

    // ---- what is standing in the room -------------------------------------------------------
    //
    // The floor band fills the room's OUTLINE, so a cell struck out of `inside` still shows band
    // zero underneath it — the darkest colour on the drawing, and the one that means "this fails".
    // Painting the excluded cells over as a plain neutral is what makes them read as an OBJECT
    // rather than as a hole in the lighting. A cupboard is not a dark patch; it is a cupboard.
    //
    // At grid resolution, so the blocks are the footprints the calculation actually excluded and
    // not a smoothed guess at them.
    if room.poly.len() >= 3 && room.mask.iter().any(|k| !k) {
        let blocked: Vec<bool> = (0..nx * ny)
            .map(|i| {
                !measurable[i] && {
                    // Inside the ROOM but not measurable — the outline test again, since `inside`
                    // has already had the mask folded into it.
                    let (i2, j2) = (i % nx, i / nx);
                    let wx = p.origin.x as f64 + (i2 as f64 + 0.5) / nx as f64 * p.width as f64;
                    let wy =
                        p.origin.y as f64 + (1.0 - (j2 as f64 + 0.5) / ny as f64) * p.depth as f64;
                    crate::factory::point_in_poly(room.poly, wx as f32, wy as f32)
                }
            })
            .collect();
        if blocked.iter().any(|b| *b) {
            // Every blocked cell is "above" minus infinity, so this fills exactly them.
            let flat = vec![0.0_f64; (nx + 1) * (ny + 1)];
            paint_above(c, &flat, &blocked, nx, ny, f64::NEG_INFINITY, OBJECT_FILL, &at);
        }
    }

    // ---- the room's own edge -----------------------------------------------------------------
    //
    // Drawn so the room reads as a room and not merely as where the colour stops.
    if room.poly.len() >= 3 {
        let ring: Vec<(f64, f64)> = room.poly.iter().map(to_page).collect();
        for w in ring.windows(2) {
            c.push(Item::Line {
                x1: w[0].0,
                y1: w[0].1,
                x2: w[1].0,
                y2: w[1].1,
                rgb: [150, 150, 150],
                width: 0.6,
            });
        }
        if let (Some(a), Some(b)) = (ring.first(), ring.last()) {
            c.push(Item::Line {
                x1: b.0,
                y1: b.1,
                x2: a.0,
                y2: a.1,
                rgb: [150, 150, 150],
                width: 0.6,
            });
        }
    }

    // THE VALUES, where the grid is coarse enough to carry them. A smooth field shows the shape of
    // the light; the numbers make it checkable, and on a small room there is room for both.
    let cell_w = plot_w / gc as f64;
    let cell_h = plot_h / gr as f64;
    let values_shown = cell_w.min(cell_h) >= 22.0;
    if values_shown {
        for j in 0..gr {
            for i in 0..gc {
                let Some(v) = g.values.get(j * gc + i) else { continue };
                let Some(fill) = sample(inp, room, opt, i as f64, j as f64, room_max) else {
                    continue;
                };
                // Black on light, white on dark — a fixed ink colour is unreadable over half the
                // ramp, and these exist to be read.
                let lum = 0.299 * fill[0] as f64 + 0.587 * fill[1] as f64 + 0.114 * fill[2] as f64;
                // UNSCALED — see `Cursor::push_unscaled`. This size exists to fit the cell.
                c.push_unscaled(Item::Text {
                    x: x0 + (i as f64 + 0.5) * cell_w,
                    // Flipped with the field above it, or the numbers would describe the mirror
                    // image of the picture they are printed on.
                    y: y0 + plot_h - (j as f64 + 0.5) * cell_h + 2.0,
                    size: (cell_w.min(cell_h) * 0.26).clamp(4.0, 8.0),
                    font: Font::Regular,
                    rgb: if lum > 140.0 { [20, 20, 20] } else { [245, 245, 245] },
                    align: Align::Centre,
                    text: format!("{v:.0}"),
                });
            }
        }
    }

    c.push(Item::Frame { x: x0, y: y0, w: plot_w, h: plot_h, rgb: [140, 140, 140], width: 0.7 });
    c.push(Item::Text {
        x: x0 + plot_w * 0.5,
        y: y0 + plot_h + 11.0,
        size: 7.5,
        font: Font::Regular,
        rgb: FAINT,
        align: Align::Centre,
        // THE SCALE IS PART OF THE CAPTION. Every drawing in the report is set at this one scale,
        // which is what lets a reader hold two rooms against each other — but only if it is said,
        // and said on the drawing rather than once in a preamble nobody reads twice.
        text: format!(
            "{:.2} × {:.2} m   ·   {} — the same scale on every drawing here",
            room.plane.width,
            room.plane.depth,
            scale_note(c.scale),
        ),
    });
    c.y = y0 + plot_h + 24.0;
    legend(c, inp, opt, room_max);
    if !values_shown {
        // AFTER the legend, not before: `note` moves the pen, and emitting it mid-plot would print
        // the sentence across the field it is describing.
        c.note("Point values are omitted at this grid size — the grid table carries every figure.");
    }
    // WHAT THE GREY BLOCKS ARE. Unexplained, a colour outside the legend is just a mystery, and the
    // obvious reading of a pale patch on a lighting drawing is "unlit".
    let blocked = room.mask.iter().filter(|k| !**k).count();
    if blocked > 0 && room.poly.len() >= 3 {
        c.note(&format!(
            "Grey: {blocked} grid point(s) fall inside objects standing in the room — the working \
             plane runs through them, so no reading exists there and none is counted in the \
             figures above.",
        ));
    }
}

/// Paint every part of the field at or above `t`.
///
/// MARCHING SQUARES, cell by cell. For each raster cell the part at or above the threshold is a
/// polygon with three to six corners, found by walking the cell's four corners and interpolating
/// wherever an edge crosses the threshold — so the boundary is a straight segment across one raster
/// cell rather than a step the size of one. That is the whole difference between the pixelated
/// edges reported and a contour.
///
/// Cells wholly above the threshold are merged into horizontal RUNS and emitted as rectangles. A
/// polygon each would be thousands of identical squares: the same picture, several times the file,
/// and slower to open. The interesting shapes are only ever at the boundary.
fn paint_above(
    c: &mut Cursor,
    f: &[f64],
    inside: &[bool],
    nx: usize,
    ny: usize,
    t: f64,
    fill: [u8; 3],
    at: &dyn Fn(f64, f64) -> (f64, f64),
) {
    let v = |i: usize, j: usize| f[j * (nx + 1) + i];
    for j in 0..ny {
        // A run of whole cells, closed off as soon as a cell is not whole.
        let mut run: Option<usize> = None;
        let mut close = |run: &mut Option<usize>, end: usize, c: &mut Cursor| {
            if let Some(s) = run.take() {
                let (ax, ay) = at(s as f64, j as f64);
                let (bx, by) = at(end as f64, (j + 1) as f64);
                // A whisker of overlap, so neighbouring runs meet instead of leaving a hairline of
                // paper between them at the reader's rendering resolution.
                c.push(Item::Rect {
                    x: ax,
                    y: ay,
                    w: bx - ax + 0.12,
                    h: by - ay + 0.12,
                    fill,
                });
            }
        };
        for i in 0..nx {
            // Corners, anticlockwise from the cell's top-left in PAGE terms. The order only has to
            // be consistent: the polygon comes out with the same winding either way.
            let corners = [
                (i as f64, j as f64, v(i, j)),
                ((i + 1) as f64, j as f64, v(i + 1, j)),
                ((i + 1) as f64, (j + 1) as f64, v(i + 1, j + 1)),
                (i as f64, (j + 1) as f64, v(i, j + 1)),
            ];
            // Outside the room there is no answer to draw. Skipping is what removes the need for
            // anything to be painted over afterwards — see the mask above.
            if !inside.get(j * nx + i).copied().unwrap_or(true) {
                close(&mut run, i, c);
                continue;
            }
            let n_in = corners.iter().filter(|(_, _, a)| *a >= t).count();
            if n_in == 4 {
                run.get_or_insert(i);
                continue;
            }
            close(&mut run, i, c);
            if n_in == 0 {
                continue;
            }
            // Sutherland–Hodgman against the half-space `value >= t`, with the value taken as
            // linear along each edge. Four corners in, three to five points out.
            let mut poly: Vec<(f64, f64)> = Vec::with_capacity(6);
            for k in 0..4 {
                let (ax, ay, av) = corners[k];
                let (bx, by, bv) = corners[(k + 1) % 4];
                let a_in = av >= t;
                let b_in = bv >= t;
                if a_in {
                    poly.push(at(ax, ay));
                }
                if a_in != b_in {
                    // Where the edge crosses. Guarded against a zero denominator, which happens
                    // when two corners hold exactly the same value straddling the threshold.
                    let d = bv - av;
                    let s = if d.abs() > 1e-12 { ((t - av) / d).clamp(0.0, 1.0) } else { 0.5 };
                    poly.push(at(ax + (bx - ax) * s, ay + (by - ay) * s));
                }
            }
            if poly.len() >= 3 {
                c.push(Item::Poly { rings: vec![poly], fill });
            }
        }
        close(&mut run, nx, c);
    }
}

/// Bilinear illuminance at a point in GRID coordinates, with no room test.
///
/// Separate from [`sample`], which also decides colour and whether the point is inside the room.
/// The contour tracer wants the raw field over the WHOLE plane: it traces bands across the entire
/// rectangle and covers the outside afterwards with the room's own outline, which is what keeps
/// the room edge smooth instead of stepped.
fn bilinear(g: &LuxGrid, gx: f64, gy: f64) -> f64 {
    let (gc, gr) = (g.cols as usize, g.rows as usize);
    if gc == 0 || gr == 0 {
        return 0.0;
    }
    let x0 = gx.floor().clamp(0.0, (gc - 1) as f64) as usize;
    let y0 = gy.floor().clamp(0.0, (gr - 1) as f64) as usize;
    let x1 = (x0 + 1).min(gc - 1);
    let y1 = (y0 + 1).min(gr - 1);
    let tx = (gx - x0 as f64).clamp(0.0, 1.0);
    let ty = (gy - y0 as f64).clamp(0.0, 1.0);
    let at = |x: usize, y: usize| g.values.get(y * gc + x).copied().unwrap_or(0.0);
    at(x0, y0) * (1.0 - tx) * (1.0 - ty)
        + at(x1, y0) * tx * (1.0 - ty)
        + at(x0, y1) * (1.0 - tx) * ty
        + at(x1, y1) * tx * ty
}

/// The colour at a point in GRID coordinates, or `None` outside the room.
///
/// Bilinear between the four surrounding grid points. The room edge is taken from the room's own
/// OUTLINE where there is one rather than from the cell mask — a mask is a staircase at grid
/// resolution, and drawing a smooth field inside a staircase would put the jaggedness back at the
/// one place the eye looks for it.
fn sample(
    inp: &Input,
    room: &RoomInput,
    opt: &Options,
    gx: f64,
    gy: f64,
    room_max: f64,
) -> Option<[u8; 3]> {
    let g = room.grid;
    let (gc, gr) = (g.cols as usize, g.rows as usize);

    if !room.poly.is_empty() {
        let p = room.plane;
        let x = p.origin.x as f64 + (gx + 0.5) * (p.width as f64 / gc as f64);
        let y = p.origin.y as f64 + (gy + 0.5) * (p.depth as f64 / gr as f64);
        if !crate::factory::point_in_poly(room.poly, x as f32, y as f32) {
            return None;
        }
    } else if !room.mask.is_empty() {
        let i = (gy.round().clamp(0.0, (gr - 1) as f64) as usize) * gc
            + gx.round().clamp(0.0, (gc - 1) as f64) as usize;
        if room.mask.get(i).is_some_and(|inside| !inside) {
            return None;
        }
    }

    let x0 = gx.floor().clamp(0.0, (gc - 1) as f64) as usize;
    let y0 = gy.floor().clamp(0.0, (gr - 1) as f64) as usize;
    let x1 = (x0 + 1).min(gc - 1);
    let y1 = (y0 + 1).min(gr - 1);
    let tx = (gx - x0 as f64).clamp(0.0, 1.0);
    let ty = (gy - y0 as f64).clamp(0.0, 1.0);
    let at = |cx: usize, cy: usize| g.values.get(cy * gc + cx).copied().unwrap_or(0.0);
    let v = at(x0, y0) * (1.0 - tx) * (1.0 - ty)
        + at(x1, y0) * tx * (1.0 - ty)
        + at(x0, y1) * (1.0 - tx) * ty
        + at(x1, y1) * tx * ty;

    // Through the SAME band the field is painted with, so a printed point value picks its ink
    // against the colour it is actually sitting on rather than against the palette's idea of it —
    // a dark number on a dark band chosen by hand would be unreadable. Entering the rule by VALUE
    // is exactly what the viewport's overlay needs, so both go through one door.
    Some(opt.lux_rgb(v, room_max, inp.ramp))
}

/// THE LIGHTING LAYOUT — the room and what is in it, before the result.
///
/// Asked for as "have a page showing the lighting layout before it shows the false colors". A
/// false-colour field says how much light there is; it does not say where the fittings are, and a
/// reader checking a design needs to see the layout that produced the numbers.
fn layout_page(c: &mut Cursor, room: &RoomInput) {
    let p = room.plane;
    if p.width <= 0.0 || p.depth <= 0.0 {
        return;
    }
    // The report's one scale, so this page and the results field that follows it are the same room
    // at the same size — and so are the other rooms' pages. See `Cursor::scale`.
    let k = c.scale;
    let (dw, dh) = (p.width as f64 * k, p.depth as f64 * k);

    c.need_or_break(40.0 + dh + 40.0);
    c.heading("Lighting layout");

    let x0 = c.left + (c.width() - dw) * 0.5;
    let y0 = c.y;
    // Plan coordinates → page. Y IS FLIPPED: a plan reads with +y up, a page with +y down, and a
    // layout printed upside down against its own result is worse than no layout at all.
    let to_page = |wx: f64, wy: f64| -> (f64, f64) {
        (
            x0 + (wx - p.origin.x as f64) * k,
            y0 + dh - (wy - p.origin.y as f64) * k,
        )
    };

    // The room outline, or the plane's own rectangle when there is none.
    if room.poly.len() >= 3 {
        let pts: Vec<(f64, f64)> =
            room.poly.iter().map(|v| to_page(v.x as f64, v.y as f64)).collect();
        for w in pts.windows(2) {
            c.push(Item::Line {
                x1: w[0].0,
                y1: w[0].1,
                x2: w[1].0,
                y2: w[1].1,
                rgb: [190, 40, 40],
                width: 1.4,
            });
        }
        // Closed, in case the footprint does not repeat its first point.
        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
            c.push(Item::Line { x1: b.0, y1: b.1, x2: a.0, y2: a.1, rgb: [190, 40, 40], width: 1.4 });
        }
    } else {
        c.push(Item::Frame { x: x0, y: y0, w: dw, h: dh, rgb: [190, 40, 40], width: 1.4 });
    }

    // EVERY FITTING, AT ITS OWN SIZE AND FACING THE WAY IT FACES.
    //
    // Reported as: "why are the lights shown as circular downlights even though these are linear
    // linear light." Every fitting was drawn as the same little cross-and-square, 2.6 pt across,
    // whatever it actually was. On a plan of 2 m linear luminaires that is not a simplification,
    // it is wrong: a 2 m batten and a 100 mm downlight light a room in completely different
    // shapes, and this page exists to be read against the field beside it. Nobody can ask "is that
    // dark strip between the two runs?" of a drawing that has drawn every run as a dot.
    //
    // The sizes come from THIS ROOM'S SCHEDULE, keyed by profile name — the same figures the
    // schedule prints as "2000 × 48 × 80 mm" — so the drawing and the table cannot disagree.
    let size_of = |name: &str| -> Option<(f64, f64)> {
        room.schedule
            .iter()
            .find(|s| s.profile == name)
            .map(|s| (s.size_m.0, s.size_m.1))
            .filter(|(l, w)| *l > 0.0 && *w > 0.0)
    };
    for l in room.fixtures {
        let (x, y) = to_page(l.position.x as f64, l.position.y as f64);
        // Below a couple of points a true-to-scale outline is a smudge that says less than a
        // marker does, so a genuinely small fitting keeps the marker. The rule is about
        // LEGIBILITY rather than about type: the same 100 mm downlight gets an outline on a plan
        // of one room and a marker on a site plan.
        let outline = size_of(&l.profile).and_then(|(len, wid)| {
            let (hl, hw) = (len * 0.5, wid * 0.5);
            ((hl.max(hw) * k) >= 2.0).then_some((hl, hw))
        });
        match outline {
            // Half-extents in METRES: rotated in world coordinates and only then mapped to the
            // page. Rotating on the page instead would silently mirror every fitting that is not
            // on an axis, because the page's y runs the other way.
            Some((u, v)) => {
                let a = (l.rotation_deg as f64).to_radians();
                let (sa, ca) = (a.sin(), a.cos());
                let corner = |du: f64, dv: f64| -> (f64, f64) {
                    let (dx, dy) = (ca * du - sa * dv, sa * du + ca * dv);
                    (x + dx * k, y - dy * k)
                };
                let p = [corner(-u, -v), corner(u, -v), corner(u, v), corner(-u, v)];
                for i in 0..4 {
                    let (s, e) = (p[i], p[(i + 1) % 4]);
                    c.push(Item::Line {
                        x1: s.0,
                        y1: s.1,
                        x2: e.0,
                        y2: e.1,
                        rgb: [200, 150, 40],
                        width: 0.9,
                    });
                }
                // A tick along the fitting, so it reads as a luminaire rather than as a hole in
                // the ceiling — and so a nearly square one still shows which way it is turned.
                let (m0, m1) = (corner(-u, 0.0), corner(u, 0.0));
                c.push(Item::Line {
                    x1: m0.0,
                    y1: m0.1,
                    x2: m1.0,
                    y2: m1.1,
                    rgb: [30, 30, 30],
                    width: 0.6,
                });
            }
            None => {
                let r = 2.6;
                c.push(Item::Line {
                    x1: x - r,
                    y1: y,
                    x2: x + r,
                    y2: y,
                    rgb: [30, 30, 30],
                    width: 0.8,
                });
                c.push(Item::Line {
                    x1: x,
                    y1: y - r,
                    x2: x,
                    y2: y + r,
                    rgb: [30, 30, 30],
                    width: 0.8,
                });
                c.push(Item::Frame {
                    x: x - r * 0.66,
                    y: y - r * 0.66,
                    w: r * 1.32,
                    h: r * 1.32,
                    rgb: [200, 150, 40],
                    width: 0.9,
                });
            }
        }
    }

    c.push(Item::Frame { x: x0, y: y0, w: dw, h: dh, rgb: [215, 215, 215], width: 0.5 });
    c.y = y0 + dh + 12.0;
    c.text(
        c.left,
        7.5,
        Font::Regular,
        Align::Left,
        FAINT,
        &format!(
            "{:.2} × {:.2} m  ·  {} fitting(s)  ·  mounting {:.2} m",
            p.width,
            p.depth,
            room.fixtures.len(),
            room.fixtures.first().map(|l| l.position.z).unwrap_or(0.0),
        ),
    );
    c.y += 6.0;
}

/// THE COLOUR BAND `k` IS DRAWN IN — the practice's own choice where it has made one, the palette
/// where it has not.
///
/// Asked for as: *"in the band add a band color picker … so this color band will come for all
/// future report generation."* A practice's drawings are read by people who have learned what its
/// colours mean, and which colours those are is not the app's decision to make.
///
/// One function, called by the FIELD, the LEGEND and the printed point values alike. A legend in
/// different colours from the picture it explains is worse than no legend, and that is exactly what
/// three call sites reading the palette their own way would eventually produce.
///
/// A THIN WRAPPER, and deliberately so: the rule itself lives on [`Options::band_rgb`], because the
/// SIMLUX viewport paints the same field and used to have its own idea of what a lux value looks
/// like. Two implementations agree right up until one of them is edited.
fn band_fill(inp: &Input, opt: &Options, k: usize, t: f32) -> [u8; 3] {
    opt.band_rgb(k, t, inp.ramp)
}

use crate::report::options::ramp_rgb;

/// The legend: banded blocks with their edges written under them, or a smooth bar when the scale
/// has no bands.
///
/// A BANDED LEGEND CAN BE READ; a gradient can only be compared. "0 · 25 · 100 · 300 · 500" says
/// which parts of the room meet which requirement, which is the question a lighting drawing is
/// looked at to answer.
fn legend(c: &mut Cursor, inp: &Input, opt: &Options, room_max: f64) {
    let w = c.width().min(360.0);
    let x = c.left + (c.width() - w) * 0.5;
    let y = c.y;
    let h = 12.0;

    let edges = opt.scale.edges(room_max);
    if opt.scale.bands.is_empty() {
        let n = 60;
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            c.push(Item::Rect {
                x: x + w * i as f64 / n as f64,
                y,
                w: w / n as f64 + 0.4,
                h,
                fill: ramp_rgb(inp.ramp, t),
            });
        }
        c.push(Item::Frame { x, y, w, h, rgb: [150, 150, 150], width: 0.5 });
        c.y = y + h + 11.0;
        c.text(x, 7.5, Font::Regular, Align::Left, FAINT, "0 lx");
        c.push(Item::Text {
            x: x + w,
            y: c.y,
            size: 7.5,
            font: Font::Regular,
            rgb: FAINT,
            align: Align::Right,
            text: format!("{:.0} lx", opt.scale.top_lx(room_max)),
        });
    } else {
        // Equal-width blocks, not proportional ones: the bands a drawing is read at are not evenly
        // spaced in lux (25, 100, 300, 500), and a proportional bar would squeeze the low ones —
        // which are the bands that decide whether a space passes.
        let n = edges.len() - 1;
        let bw = w / n as f64;
        for i in 0..n {
            let mid = (edges[i] + edges[i + 1]) * 0.5;
            let t = (mid / opt.scale.top_lx(room_max)).clamp(0.0, 1.0) as f32;
            c.push(Item::Rect { x: x + bw * i as f64, y, w: bw, h, fill: band_fill(inp, opt, i, t) });
        }
        c.push(Item::Frame { x, y, w, h, rgb: [150, 150, 150], width: 0.5 });
        c.y = y + h + 10.0;
        for (i, e) in edges.iter().enumerate() {
            // THE TOP BAND IS OPEN-ENDED, and its label says so rather than naming the room's peak.
            //
            // The last edge is whatever the brightest point in the room happened to reach, so a
            // room with one bright spot under a downlight ended its legend at "1802" — a number
            // describing a single cell, which makes every band beneath it look like a narrow
            // sliver of the range. What that band means is "300 lx and above", which is how both
            // reference tools label it and how a reader actually uses it.
            let last = i + 1 == edges.len();
            let text = if last && opt.scale.top.is_none() && i > 0 {
                format!("{:.0}+", edges[i - 1])
            } else {
                format!("{e:.0}")
            };
            c.push(Item::Text {
                x: x + bw * i as f64,
                y: c.y,
                size: 7.0,
                font: Font::Regular,
                rgb: FAINT,
                align: if i == 0 { Align::Left } else { Align::Centre },
                text,
            });
        }
    }
    c.y += 12.0;
    c.text(
        c.left,
        7.0,
        Font::Regular,
        Align::Left,
        FAINT,
        &format!("Illuminance [lx] — {}", opt.scale.caption(room_max)),
    );
    c.y += 4.0;
}

/// The numbers, COARSENED to fit one page.
///
/// "for the illumination grid coarsen it. no need to have a millions points scale it so it fits
/// the a4 size but make sure the report doesnt miss the main points."
///
/// A 125 × 38 room is 4,750 values, which on A4 is 0.9 pt type — a grey texture, not numbers. So
/// only every Nth point is printed, at a spacing chosen to land the type at a readable size.
///
/// DECIMATED, NOT AVERAGED. Every figure on the page is a real measured value at a real place, so
/// a reader can go back to the model and find it. A block average would be a number that was never
/// measured anywhere, and it would hide exactly what a lighting report is read for — the dark
/// corner. The spacing is stated, and the true extremes are printed underneath with their
/// coordinates, so nothing that decides whether the room passes can fall between the printed
/// points.
fn numeric_grid(c: &mut Cursor, room: &RoomInput) {
    let g = room.grid;
    let (gc, gr) = (g.cols as usize, g.rows as usize);
    if gc == 0 || gr == 0 {
        return;
    }
    // THE TABLE IS THE ROOM, at the report's scale — the same box the results field above it is
    // drawn in, and the same box every other room's table is drawn in.
    //
    // It used to be stretched across the full content width whatever shape the room was, so a
    // 31 × 9 m hall printed as a table nothing like the hall: the numbers did not sit where the
    // points they describe are, and the reader could not lay the table against the field and read
    // one from the other. Since the whole reason for printing figures next to a picture is to
    // check the picture, that is the table failing at its only job.
    //
    // Clamped to what is actually available, so a room drawn wider than the content box — possible
    // when the scale is set by a room with a very different aspect — is not printed off the page.
    let table_w = (room.plane.width as f64 * c.scale).min(c.width()).max(1.0);
    // MEASURED AGAINST A WHOLE PAGE, not against what is left of this one. The grid gets a fresh
    // page if it needs one, so sizing it to the tail end of the previous page would reject a table
    // that fits perfectly well.
    let full_h = c.bottom - c.top - 60.0;
    let table_h = (room.plane.depth as f64 * c.scale).min(full_h).max(1.0);

    // The smallest stride that lands the type at a readable size, inside THAT box. Stride 1 is the
    // whole grid. Coarsening further is the right trade: a table shaped like the room with fewer
    // figures on it still reads as the room, and `extremes` below prints the darkest and brightest
    // points whether or not the stride happened to land on them.
    let mut stride = 1usize;
    let (mut cols, mut rows, mut size) = (gc, gr, 0.0_f64);
    loop {
        cols = gc.div_ceil(stride);
        rows = gr.div_ceil(stride);
        size = (table_w / (cols as f64 * 4.6)).min(table_h / (rows as f64 * 1.45));
        if size >= GRID_PT || (cols <= 2 && rows <= 2) {
            break;
        }
        stride += 1;
    }
    // Never smaller than the type this report will set, even at the coarsest useful stride.
    let size = size.min(9.0);
    let avail_w = table_w;

    c.need_or_break(46.0 + rows as f64 * size * 1.45);
    c.heading("Illuminance grid (lx)");
    if stride > 1 {
        let (dx, dy) = (
            room.plane.width as f64 / gc as f64 * stride as f64,
            room.plane.depth as f64 / gr as f64 * stride as f64,
        );
        c.note(&format!(
            "Every {}{} point of the {} × {} grid — {:.2} × {:.2} m spacing. Measured values, not \
             averages; the extremes below are over the WHOLE grid.",
            stride,
            ordinal(stride),
            gc,
            gr,
            dx,
            dy,
        ));
        c.y += 4.0;
    }

    let colw = avail_w / cols as f64;
    let rowh = size * 1.45;
    let y0 = c.y;
    // Centred like the drawings it belongs with, rather than pinned to the left margin — a table
    // the shape of the room and a field the shape of the room, sitting under different parts of the
    // page, are harder to read together than either alone.
    let tx = c.left + (c.width() - avail_w) * 0.5;
    for (rr, r) in (0..gr).step_by(stride).enumerate() {
        for (cc, col) in (0..gc).step_by(stride).enumerate() {
            let i = r * gc + col;
            let Some(v) = g.values.get(i) else { continue };
            let inside = room.mask.get(i).copied().unwrap_or(true);
            c.push(Item::Text {
                x: tx + (cc as f64 + 1.0) * colw - colw * 0.12,
                y: y0 + (rr as f64 + 1.0) * rowh,
                size,
                font: Font::Regular,
                rgb: if inside { INK } else { [190, 190, 190] },
                align: Align::Right,
                text: if inside { format!("{v:.0}") } else { "-".into() },
            });
        }
    }
    c.y = y0 + rows as f64 * rowh + 6.0;

    // THE MAIN POINTS, whether or not they were printed above. A decimated grid can step straight
    // over the darkest point in the room, and that point is the one the whole report is read for.
    extremes(c, room);
}

/// `1st`, `2nd`, `3rd`, `4th` — the suffix, given the number.
fn ordinal(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// Where the darkest and brightest points actually are.
fn extremes(c: &mut Cursor, room: &RoomInput) {
    // THE REPORTED GRID, WITH ITS OWN MASK. These two rows quote a maximum and a minimum, so taking
    // them off the drawn grid would put a second pair of extremes on the page — which is exactly
    // what quoting one grid is for. The mask has to come with it: `values` still holds the readings
    // the statistics exclude.
    let g = room.reported();
    let (gc, gr) = (g.cols as usize, g.rows as usize);
    let p = room.reported_plane();
    let mask = room.reported_mask();
    let inside = |i: usize| mask.get(i).copied().unwrap_or(true);
    let at = |i: usize| -> (f64, f64) {
        let (cx, cy) = (i % gc, i / gc);
        (
            p.origin.x as f64 + (cx as f64 + 0.5) * (p.width as f64 / gc as f64),
            p.origin.y as f64 + (cy as f64 + 0.5) * (p.depth as f64 / gr as f64),
        )
    };
    let mut lo: Option<(usize, f64)> = None;
    let mut hi: Option<(usize, f64)> = None;
    for (i, v) in g.values.iter().enumerate() {
        if !inside(i) {
            continue;
        }
        if lo.is_none_or(|(_, b)| *v < b) {
            lo = Some((i, *v));
        }
        if hi.is_none_or(|(_, b)| *v > b) {
            hi = Some((i, *v));
        }
    }
    let (Some((li, lv)), Some((hi_i, hv))) = (lo, hi) else { return };
    let (lx, ly) = at(li);
    let (hx, hy) = at(hi_i);
    c.row("Minimum over the whole grid", &format!("{lv:.0} lx at ({lx:.2}, {ly:.2}) m"));
    c.row("Maximum over the whole grid", &format!("{hv:.0} lx at ({hx:.2}, {hy:.2}) m"));
}

fn surfaces(c: &mut Cursor, inp: &Input) {
    if inp.surfaces.is_empty() {
        return;
    }
    c.heading("Surfaces");
    for s in inp.surfaces {
        c.row(
            &s.name,
            &format!("{:.0} lx avg · {:.0} min · U0 {:.2} · {:.0} m2", s.e_avg, s.e_min, s.u0, s.area_m2),
        );
    }
}

/// The render images, on a page of their own.
///
/// A PAGE OF ITS OWN, and where in the document it falls is the user's — "a page dedicated to
/// render images the user should be able to decide the position of the page". That is why the
/// sections are an ordered list rather than a set of switches.
fn renders(c: &mut Cursor, opt: &Options, doc: &Doc) {
    let usable: Vec<(usize, &ReportImage)> = opt
        .images
        .iter()
        .enumerate()
        .filter(|(i, im)| im.jpeg.is_some() && doc.images.get(*i).is_some())
        .collect();
    if usable.is_empty() {
        return;
    }
    // Always starts a page: a render squeezed under the tail of a table is not "a page dedicated
    // to render images".
    if !c.page.items.is_empty() {
        c.brk();
    }
    c.heading("Renders");

    // One per row when there is one, two per row otherwise — a single render deserves the width,
    // and a pair reads as a comparison.
    let per_row = if usable.len() == 1 { 1 } else { 2 };
    let gap = 14.0;
    let cw = (c.width() - gap * (per_row as f64 - 1.0)) / per_row as f64;

    for chunk in usable.chunks(per_row) {
        let box_h = if per_row == 1 { 380.0 } else { 240.0 };
        c.need(box_h + 26.0);
        let row_y = c.y;
        let mut tallest: f64 = 0.0;
        for (k, (idx, im)) in chunk.iter().enumerate() {
            let Some(j) = doc.images.get(*idx) else { continue };
            let (iw, ih) = fit(j.w as f64, j.h as f64, cw, box_h);
            let x = c.left + k as f64 * (cw + gap) + (cw - iw) * 0.5;
            c.push(Item::Image { x, y: row_y, w: iw, h: ih, idx: *idx });
            tallest = tallest.max(ih);
            if !im.caption.trim().is_empty() {
                c.push(Item::Text {
                    x: c.left + k as f64 * (cw + gap) + cw * 0.5,
                    y: row_y + ih + 11.0,
                    size: 8.0,
                    font: Font::Regular,
                    rgb: FAINT,
                    align: Align::Centre,
                    text: im.caption.trim().to_string(),
                });
            }
        }
        c.y = row_y + tallest + 26.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::options::{Format, PageSize};

    pub(super) fn grid(cols: u32, rows: u32) -> LuxGrid {
        let n = (cols * rows) as usize;
        let vals: Vec<f64> = (0..n).map(|i| 100.0 + (i % 7) as f64 * 50.0).collect();
        let min = vals.iter().cloned().fold(f64::MAX, f64::min);
        let max = vals.iter().cloned().fold(f64::MIN, f64::max);
        let avg = vals.iter().sum::<f64>() / n as f64;
        LuxGrid {
            cols,
            rows,
            values: vals,
            min,
            max,
            avg,
            maintenance: 0.8,
            direct: Vec::new(),
            indirect: Vec::new(),
        }
    }

    pub(super) fn plane() -> CalcPlane {
        CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 8.0,
            depth: 6.0,
            cols: 8,
            rows: 6,
        }
    }

    fn one_room<'a>(g: &'a LuxGrid, p: &'a CalcPlane, name: &str) -> RoomInput<'a> {
        RoomInput {
            name: name.to_string(),
            grid: g,
            plane: p,
            grid_en: None,
            plane_en: None,
            mask_en: &[],
            mask: &[],
            poly: &[],
            fixtures: &[],
            installation: None,
            cylindrical_avg: None,
            schedule: Vec::new(),
        }
    }

    pub(super) fn input<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> Input<'a> {
        Input {
            rooms: vec![one_room(g, p, "")],
            maintenance: Maintenance { llmf: 0.8, lsf: 1.0, lmf: 1.0, rsmf: 1.0 },
            surfaces: &[],
            eye_height: 1.2,
            room_height: 3.0,
            materials: vec![("Floor".into(), 0.2)],
            unassigned: 0,
            ramp: crate::light::lux_rgb,
            mask: Vec::new(),
        }
    }

    fn opts() -> Options {
        Options { format: Format::Pdf, page: PageSize::A4, ..Default::default() }
    }

    fn rects(d: &Doc) -> Vec<(f64, f64, f64, f64)> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Rect { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect()
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// NOTHING IS DRAWN OFF THE PAGE. This is the whole point of laying out to a page size: a plot
    /// that runs past the edge is exactly the "have to go and scroll" complaint, made permanent in
    /// a file where there is nowhere to scroll to.
    #[test]
    fn every_item_lands_inside_the_paper() {
        for (cols, rows) in [(4u32, 4u32), (33, 40), (60, 8), (8, 60)] {
            let g = grid(cols, rows);
            let p = plane();
            let d = layout(&input(&g, &p), &opts());
            let (w, h) = PageSize::A4.points();
            for (i, page) in d.pages.iter().enumerate() {
                for it in &page.items {
                    let (x, y, iw, ih) = match it {
                        Item::Rect { x, y, w, h, .. } | Item::Frame { x, y, w, h, .. } => {
                            (*x, *y, *w, *h)
                        }
                        Item::Image { x, y, w, h, .. } => (*x, *y, *w, *h),
                        Item::Line { x1, y1, x2, y2, .. } => {
                            (x1.min(*x2), y1.min(*y2), (x2 - x1).abs(), (y2 - y1).abs())
                        }
                        Item::Text { x, y, .. } => (*x, *y - 8.0, 0.0, 8.0),
                        // A contour band, as its bounding box — the same question is being asked
                        // of it as of everything else: does any of it fall off the page.
                        Item::Poly { rings, .. } => {
                            let (mut ax, mut ay, mut bx, mut by) =
                                (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                            for r in rings {
                                for (px, py) in r {
                                    ax = ax.min(*px);
                                    ay = ay.min(*py);
                                    bx = bx.max(*px);
                                    by = by.max(*py);
                                }
                            }
                            if ax > bx {
                                continue; // no points at all
                            }
                            (ax, ay, bx - ax, by - ay)
                        }
                    };
                    assert!(
                        x >= -0.5 && y >= -0.5 && x + iw <= w + 0.5 && y + ih <= h + 0.5,
                        "{cols}x{rows}: page {i} draws at ({x:.1},{y:.1}) size {iw:.1}x{ih:.1}, \
                         outside {w:.0}x{h:.0}",
                    );
                }
            }
        }
    }

    /// The plot's own bounds, taken from the frame drawn around it — a stable anchor now that the
    /// field is a raster of merged runs rather than one rectangle per grid point.
    fn plot_frames(d: &Doc) -> Vec<(f64, f64, f64, f64)> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Frame { x, y, w, h, rgb, .. } if *rgb == [140, 140, 140] => {
                    Some((*x, *y, *w, *h))
                }
                _ => None,
            })
            .collect()
    }

    /// Every filled rectangle of the field, ignoring the legend's 12 pt blocks.
    fn field_rects(d: &Doc) -> Vec<(f64, f64, f64, f64, [u8; 3])> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Rect { x, y, w, h, fill } if (*h - 12.0).abs() > 0.01 => {
                    Some((*x, *y, *w, *h, *fill))
                }
                _ => None,
            })
            .collect()
    }

    /// Every fill the field puts down, rectangles and contour bands alike.
    ///
    /// The field is no longer only rectangles: flat interiors merge into runs and the BOUNDARIES
    /// are polygons, which is the whole point of the contour rewrite. A helper that looked only at
    /// rectangles would quietly stop seeing the interesting half.
    fn field_fills(d: &Doc) -> Vec<[u8; 3]> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Rect { h, fill, .. } if (*h - 12.0).abs() > 0.01 => Some(*fill),
                Item::Poly { fill, .. } => Some(*fill),
                _ => None,
            })
            .collect()
    }

    /// THE COLOUR A READER ACTUALLY SEES at a point on the page.
    ///
    /// A painter's algorithm over the item list, and the only honest oracle now that the field is
    /// drawn as overlapping bands with the outside covered afterwards. "Is any coloured shape
    /// drawn out here" stopped being the right question the moment bands began to be traced across
    /// the whole rectangle and hidden by a later shape: shapes DO extend past the room now, and
    /// none of them shows. Asking what is on top asks what the reader sees.
    ///
    /// Even-odd across every ring together, which is exactly how the PDF fills a `Poly`.
    fn colour_at(d: &Doc, x: f64, y: f64) -> Option<[u8; 3]> {
        let mut seen = None;
        for pg in &d.pages {
            for it in &pg.items {
                match it {
                    Item::Rect { x: rx, y: ry, w, h, fill } => {
                        if x >= *rx && x <= rx + w && y >= *ry && y <= ry + h {
                            seen = Some(*fill);
                        }
                    }
                    Item::Poly { rings, fill } => {
                        let mut crossings = 0usize;
                        for r in rings {
                            for k in 0..r.len() {
                                let (x1, y1) = r[k];
                                let (x2, y2) = r[(k + 1) % r.len()];
                                if (y1 > y) != (y2 > y) {
                                    let t = (y - y1) / (y2 - y1);
                                    if x1 + t * (x2 - x1) > x {
                                        crossings += 1;
                                    }
                                }
                            }
                        }
                        if crossings % 2 == 1 {
                            seen = Some(*fill);
                        }
                    }
                    _ => {}
                }
            }
        }
        seen
    }

    /// THE PLOT TAKES TWO THIRDS OF THE PAGE.
    ///
    /// "the layout is too small it should occupy 2/3 of the pages real estate while maintaining
    /// proportions". It used to be sized to whatever was left below the heading, which on a busy
    /// page was a stamp in a field of white.
    #[test]
    fn the_plot_fills_two_thirds_of_the_page() {
        let (pw, ph) = PageSize::A4.points();
        for (cols, rows) in [(4u32, 4u32), (33, 40), (80, 3), (3, 80)] {
            let g = grid(cols, rows);
            let p = plane();
            let mut o = opts();
            o.cover = false;
            o.sections = vec![Section::Results];
            let d = layout(&input(&g, &p), &o);
            let f = plot_frames(&d);
            assert_eq!(f.len(), 1, "{cols}x{rows}: expected one plot");
            let (x, y, w, h) = f[0];
            let _ = y;

            let fills_w = w >= pw * 0.66 - 1.0;
            let fills_h = h >= ph * 0.66 - 1.0;
            assert!(
                fills_w || fills_h,
                "{cols}x{rows}: plot is {w:.0}x{h:.0} on {pw:.0}x{ph:.0} — neither side reaches \
                 two thirds",
            );
            assert!(
                w <= pw * TWO_THIRDS + 1.0 && h <= ph * TWO_THIRDS + 1.0,
                "{cols}x{rows}: too big",
            );
            assert!(x >= 0.0 && x + w <= pw, "{cols}x{rows}: spans {x:.1}..{:.1}", x + w);
            // THE FIELD IS THE SHAPE OF THE ROOM — the plane's proportions, whatever grid it
            // happened to be sampled on.
            //
            // This used to assert SQUARE CELLS, which is a different and wrong thing: it made the
            // drawn field `cols : rows` rather than `width : depth`, so the same room came out a
            // slightly different shape depending on how many points it was calculated at, and a
            // different shape again from its own layout page, which has always been drawn in
            // metres. That is why the same four grids are run here against one fixed plane — under
            // the old rule each produced a different rectangle for one room.
            let want = p.width as f64 / p.depth as f64;
            let got = w / h;
            assert!(
                (got - want).abs() < 1e-6,
                "{cols}x{rows}: the field is {got:.4} wide-to-tall for a room that is {want:.4}",
            );
        }
    }

    /// A BAND IS DRAWN IN THE COLOUR THE PRACTICE CHOSE FOR IT.
    ///
    /// Asked for as: *"in the band add a band color picker … so this color band will come for all
    /// future report generation."* A practice's drawings are read by people who have learned what
    /// its colours mean, and which colours those are is not the app's decision.
    ///
    /// The FIELD, the LEGEND and the printed point values are all checked, because three call
    /// sites reading the palette their own way is how a legend ends up in different colours from
    /// the picture it explains.
    #[test]
    fn a_chosen_band_colour_is_used_everywhere() {
        let g = grid(4, 4); // 100..250 lx — spans the 100 and 300 bands
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        o.scale = crate::report::options::Scale { top: Some(500.0), bands: vec![100.0, 300.0] };

        // Three bands: 0–100, 100–300, 300–500. Colours nothing a palette would produce.
        let mine = [[7u8, 11, 13], [201, 17, 19], [23, 197, 29]];
        o.band_colours = mine.to_vec();
        let d = layout(&input(&g, &p), &o);

        let fills: std::collections::BTreeSet<[u8; 3]> = field_fills(&d).into_iter().collect();
        assert!(
            fills.contains(&mine[1]),
            "the 100–300 band was not drawn in the chosen colour: {fills:?}",
        );
        // The legend blocks are 12 pt tall — the one height `field_fills` filters out — so they
        // are gathered separately. A legend that disagrees with its own plot is the failure this
        // guards against.
        let legend: Vec<[u8; 3]> = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter_map(|i| match i {
                Item::Rect { h, fill, .. } if (*h - 12.0).abs() < 0.01 => Some(*fill),
                _ => None,
            })
            .collect();
        for (k, c) in mine.iter().enumerate() {
            assert!(
                legend.contains(c),
                "band {k} is missing from the legend, which shows {legend:?}",
            );
        }
    }

    /// THE DEFAULT SCHEME IS THE ONE THE REFERENCE TOOLS USE.
    ///
    /// Reported as: *"the false color looks comical in our report… look at the false colors in the
    /// screenshot i uploaded that should be the default false colors for simlux."*
    ///
    /// The old bands were 25 / 100 / 300 / 500. On the room this came from — 305 lx average, 1802
    /// lx under one downlight — that put nearly everything into two colours, and the legend ran to
    /// 1802 because the top of the ramp followed the room's brightest cell. A single bright spot
    /// therefore decided how the whole drawing was coloured.
    #[test]
    fn the_default_bands_are_the_ones_a_lighting_plan_is_read_at() {
        let o = Options::default();
        assert_eq!(o.scale.bands, vec![50.0, 100.0, 200.0, 300.0]);
        assert_eq!(
            o.band_colours.len(),
            o.scale.bands.len() + 1,
            "a colour per band, including the one below the first step",
        );
        // Low is warm and high is pale, which is the reference convention and the opposite of a
        // heat map: the eye is pulled to the dark saturated patches, and those are the FAILING
        // parts of a room.
        let dim = o.band_colours[0];
        let bright = *o.band_colours.last().expect("a top band");
        let lum = |c: [u8; 3]| 0.299 * c[0] as f64 + 0.587 * c[1] as f64 + 0.114 * c[2] as f64;
        assert!(lum(bright) > lum(dim) + 60.0, "{bright:?} is not clearly paler than {dim:?}");

        // A ROOM AT 305 LX RESOLVES ACROSS SEVERAL BANDS, which is the whole complaint.
        let room_max = 1802.0;
        let bands: std::collections::BTreeSet<usize> =
            [120.0, 210.0, 260.0, 305.0, 340.0, 420.0].iter().map(|v| o.scale.band_index(*v, room_max)).collect();
        assert!(
            bands.len() >= 3,
            "readings from 120 to 420 lx land in only {} band(s) — the drawing cannot show where \
             the room is brighter",
            bands.len(),
        );
    }

    /// AND THE LEGEND SAYS "300+" RATHER THAN THE ROOM'S BRIGHTEST CELL.
    ///
    /// The top band is open-ended. Ending the legend at 1802 describes one cell under one
    /// downlight and makes every band beneath it look like a sliver of the range.
    #[test]
    fn the_top_band_is_labelled_open_ended() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        o.scale = crate::report::options::Scale { top: None, bands: vec![50.0, 100.0, 200.0, 300.0] };
        let t = texts(&layout(&input(&g, &p), &o));
        assert!(t.iter().any(|s| s == "300+"), "the legend does not say 300+: {t:?}");
        // The CAPTION names the steps rather than the room's brightest cell. Checked here rather
            // than by looking for the number anywhere on the page, which the first version did — and
            // which caught a printed point value instead.
        assert!(
            t.iter().any(|s| s.contains("banded at 50 · 100 · 200 · 300 lx")),
            "the caption does not say where the steps are: {t:?}",
        );

        // PINNED, the number is the number — somebody who set a ceiling means it.
        o.scale.top = Some(600.0);
        let t = texts(&layout(&input(&g, &p), &o));
        assert!(t.iter().any(|s| s == "600"), "a pinned ceiling must be stated: {t:?}");
        assert!(!t.iter().any(|s| s == "300+"), "a pinned scale is not open-ended: {t:?}");
    }


    /// AN EMPTY LIST MEANS THE PALETTE — so a project made before the picker existed, and a
    /// settings file that never held one, both draw exactly as they did.
    #[test]
    fn without_a_choice_the_palette_still_decides() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];

        // THE BASELINE IS THE PALETTE, so it has to be asked for. `Options::default` now carries
        // the reference scheme's colours — that is the whole point of it being the default — so
        // comparing against an unmodified `opts()` compares two lists of chosen colours and proves
        // nothing about the fallback.
        o.band_colours = Vec::new();
        let palette = field_fills(&layout(&input(&g, &p), &o));
        o.band_colours = Vec::new();
        let same = field_fills(&layout(&input(&g, &p), &o));
        assert_eq!(palette, same, "an empty list changed the drawing");
        // And it really is the palette, not the reference scheme leaking through.
        assert!(
            !palette.contains(&crate::report::options::DEFAULT_BAND_COLOURS[0]),
            "an empty list still drew the default band colours",
        );

        // And a SHORT list falls back for the bands past its end rather than running off it.
        o.band_colours = vec![[1, 2, 3]];
        let short = field_fills(&layout(&input(&g, &p), &o));
        assert!(short.contains(&[1, 2, 3]), "the one chosen colour was not used");
        assert!(
            short.iter().any(|c| palette.contains(c)),
            "a short list threw away the palette for every other band",
        );
    }

    /// THE COLOURS TRAVEL WITH THE SETTINGS. "so this color band will come for all future report
    /// generation" — a house style re-entered on every job is not a house style.
    #[test]
    fn the_chosen_colours_are_kept_between_sessions() {
        let mut src = Options::default();
        src.band_colours = vec![[9, 8, 7], [6, 5, 4]];
        let mut back = Options::default();
        crate::report::Prefs::of(&src).apply(&mut back);
        assert_eq!(back.band_colours, src.band_colours, "the band colours were not kept");
    }

    /// THE NUMERIC GRID IS THE SHAPE OF THE ROOM TOO, at the same scale as the field above it.
    ///
    /// This is the half that was actually complained about: *"make the grids scale proportional to
    /// that of the layout."* The table was stretched across the full content width whatever shape
    /// the room was, so a 30 × 4 m corridor printed as a table nothing like a corridor. The numbers
    /// did not sit where the points they describe are, and a reader could not lay the table against
    /// the field and read one from the other — which is the only reason to print figures beside a
    /// picture at all.
    #[test]
    fn the_grid_table_is_the_shape_of_the_room() {
        let g = grid(30, 4);
        let p = CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 30.0,
            depth: 4.0,
            cols: 30,
            rows: 4,
        };
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results, Section::NumericGrid];
        let d = layout(&input(&g, &p), &o);

        let (field_x, field_w) = plot_frames(&d).first().map(|f| (f.0, f.2)).expect("a field");

        // The cells are the right-aligned items that read as a plain lux figure — the heading, the
        // note and the extremes rows all carry words.
        let mut xs: Vec<f64> = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter_map(|i| match i {
                Item::Text { x, text, align: Align::Right, .. }
                    if text.parse::<u32>().is_ok() || text == "-" =>
                {
                    Some(*x)
                }
                _ => None,
            })
            .collect();
        assert!(xs.len() > 8, "only {} grid cells were printed", xs.len());
        xs.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert!(xs.len() >= 2, "the table has one column — nothing to measure");

        // Reconstruct the table's box from the column pitch, and hold it against the field's.
        let colw = (xs[xs.len() - 1] - xs[0]) / (xs.len() - 1) as f64;
        let table_w = colw * xs.len() as f64;
        assert!(
            (table_w - field_w).abs() < 1.0,
            "the table is {table_w:.0} pt wide under a field {field_w:.0} pt wide — a table the \
             shape of the page rather than the shape of the room",
        );
        // AND IT SITS UNDER THE FIELD, not off to one side. The first cell's anchor is its RIGHT
        // edge, one column in from the table's left, so the table starts a column-and-a-bit back.
        let table_x = xs[0] - colw * 0.88;
        assert!(
            (table_x - field_x).abs() < 1.0,
            "the table starts at {table_x:.0} and the field at {field_x:.0} — a table the shape of \
             the room, printed somewhere else on the page",
        );
    }

    /// A LINEAR LUMINAIRE IS DRAWN AS A LINE, NOT AS A DOT.
    ///
    /// Reported as: *"why are the lights shown as circular downlights even though these are linear
    /// linear light."* Every fitting was drawn as the same 2.6 pt cross-and-square whatever it
    /// was. On a plan of 2 m battens that is not a simplification but a wrong drawing: a 2 m
    /// batten and a 100 mm downlight light a room in completely different shapes, and this page
    /// exists to be read against the field beside it.
    #[test]
    fn a_two_metre_batten_is_drawn_two_metres_long() {
        let g = grid(8, 6);
        let p = plane(); // 8.00 x 6.00 m
        let lum = |x: f32, y: f32, rot: f32| cad_light::Luminaire {
            id: 1,
            profile: "BATTEN 2.0".into(),
            position: cad_light::Vertex::new(x, y, 2.9),
            rotation_deg: rot,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        };
        let fixtures = vec![lum(4.0, 3.0, 0.0)];
        let sched = vec![ScheduleRow {
            profile: "BATTEN 2.0".into(),
            count: 1,
            size_m: (2.0, 0.048, 0.08),
            ..Default::default()
        }];

        let mut room = one_room(&g, &p, "");
        room.fixtures = &fixtures;
        room.schedule = sched;
        let mut inp = input(&g, &p);
        inp.rooms = vec![room];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout];
        let d = layout(&inp, &o);

        // The fitting is drawn in its own gold. A MARKER uses a Frame; an OUTLINE uses Lines.
        let gold_frames = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter(|i| matches!(i, Item::Frame { rgb, .. } if *rgb == [200, 150, 40]))
            .count();
        assert_eq!(gold_frames, 0, "the fitting was still drawn as the little square marker");

        let lines: Vec<(f64, f64, f64, f64)> = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter_map(|i| match i {
                Item::Line { x1, y1, x2, y2, rgb, .. } if *rgb == [200, 150, 40] => {
                    Some((*x1, *y1, *x2, *y2))
                }
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 4, "expected a four-sided outline, got {} lines", lines.len());

        let xs: Vec<f64> = lines.iter().flat_map(|l| [l.0, l.2]).collect();
        let ys: Vec<f64> = lines.iter().flat_map(|l| [l.1, l.3]).collect();
        let span = |v: &[f64]| {
            let (lo, hi) = v.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
            hi - lo
        };
        // The report's own scale, from the room that set it — the same number the drawing used.
        let (pw, ph) = PageSize::A4.points();
        let k = ((pw * TWO_THIRDS) / 8.0).min((ph * TWO_THIRDS) / 6.0);
        assert!(
            (span(&xs) - 2.0 * k).abs() < 0.5,
            "the outline is {:.1} pt long where 2.00 m is {:.1} pt",
            span(&xs),
            2.0 * k,
        );
        assert!(
            (span(&ys) - 0.048 * k).abs() < 0.5,
            "the outline is {:.2} pt across where 48 mm is {:.2} pt",
            span(&ys),
            0.048 * k,
        );
    }

    /// AND IT POINTS THE WAY THE FITTING POINTS — INCLUDING OFF THE AXES.
    ///
    /// Rotation is applied in WORLD coordinates and only then mapped to the page. Doing it the
    /// other way round MIRRORS every fitting, and that is a bug which hides: at 0° and at 90° the
    /// two orderings give the same answer, so a plan of battens all laid the same way looks
    /// perfectly correct either way. This test used ninety degrees at first and passed against the
    /// mirrored version; thirty is where the two part company.
    ///
    /// A plan reads with +y up and a page with +y down, so a fitting turned COUNTER-CLOCKWISE in
    /// the drawing must rise to the right on the page — the end with the larger x has the smaller
    /// y. That is the assertion the bounding box alone cannot make.
    #[test]
    fn a_rotated_batten_turns_with_the_room() {
        let g = grid(8, 6);
        let p = plane();
        let fixtures = vec![cad_light::Luminaire {
            id: 1,
            profile: "BATTEN 2.0".into(),
            position: cad_light::Vertex::new(4.0, 3.0, 2.9),
            rotation_deg: 30.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];
        let mut room = one_room(&g, &p, "");
        room.fixtures = &fixtures;
        room.schedule = vec![ScheduleRow {
            profile: "BATTEN 2.0".into(),
            count: 1,
            size_m: (2.0, 0.048, 0.08),
            ..Default::default()
        }];
        let mut inp = input(&g, &p);
        inp.rooms = vec![room];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout];
        let d = layout(&inp, &o);

        // The tick down the middle of the fitting, which is the one line whose two ends say which
        // way round it is — a bounding box cannot, since a shape and its mirror share one.
        let tick = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .find_map(|i| match i {
                Item::Line { x1, y1, x2, y2, rgb, width } if *rgb == [30, 30, 30] && *width < 0.7 => {
                    Some((*x1, *y1, *x2, *y2))
                }
                _ => None,
            })
            .expect("the fitting drew no centre tick");
        let (near, far) = if tick.0 < tick.2 {
            ((tick.0, tick.1), (tick.2, tick.3))
        } else {
            ((tick.2, tick.3), (tick.0, tick.1))
        };
        assert!(
            far.1 < near.1,
            "turned 30° counter-clockwise in plan, the fitting falls to the right on the page \
             ({:.1},{:.1}) → ({:.1},{:.1}) — it has been mirrored",
            near.0,
            near.1,
            far.0,
            far.1,
        );
        // And it is tilted, not axis-aligned: at 30° the rise is about tan(30°) of the run.
        let (run, rise) = (far.0 - near.0, near.1 - far.1);
        let got = rise / run;
        assert!(
            (got - (30.0_f64).to_radians().tan()).abs() < 0.02,
            "the fitting sits at a slope of {got:.3} where 30° is {:.3}",
            (30.0_f64).to_radians().tan(),
        );
    }

    /// A FITTING TOO SMALL TO DRAW KEEPS ITS MARKER.
    ///
    /// True-to-scale is not the goal on its own — being READABLE is. A 60 mm downlight at a site
    /// plan's scale is a third of a point across, which prints as a smudge and says less than a
    /// marker does. The rule is about size on the page, not about the type of fitting.
    #[test]
    fn a_fitting_too_small_to_see_keeps_the_marker() {
        let g = grid(8, 6);
        let p = plane();
        let fixtures = vec![cad_light::Luminaire {
            id: 1,
            profile: "DOWNLIGHT".into(),
            position: cad_light::Vertex::new(4.0, 3.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];
        let mut room = one_room(&g, &p, "");
        room.fixtures = &fixtures;
        room.schedule = vec![ScheduleRow {
            profile: "DOWNLIGHT".into(),
            count: 1,
            size_m: (0.06, 0.06, 0.05),
            ..Default::default()
        }];
        let mut inp = input(&g, &p);
        inp.rooms = vec![room];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout];
        let d = layout(&inp, &o);

        let gold_frames = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter(|i| matches!(i, Item::Frame { rgb, .. } if *rgb == [200, 150, 40]))
            .count();
        assert_eq!(gold_frames, 1, "a 60 mm downlight lost its marker and drew nothing readable");
    }

    /// AND A FITTING WHOSE FILE DECLARES NO SIZE KEEPS IT TOO — rather than being drawn at some
    /// invented dimension. A manufacturer file with no geometry is common, and a plan that guessed
    /// would be stating something nobody supplied.
    #[test]
    fn a_fitting_with_no_declared_size_keeps_the_marker() {
        let g = grid(8, 6);
        let p = plane();
        let fixtures = vec![cad_light::Luminaire {
            id: 1,
            profile: "UNKNOWN".into(),
            position: cad_light::Vertex::new(4.0, 3.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];
        let mut room = one_room(&g, &p, "");
        room.fixtures = &fixtures;
        room.schedule = vec![ScheduleRow {
            profile: "UNKNOWN".into(),
            count: 1,
            size_m: (0.0, 0.0, 0.0),
            ..Default::default()
        }];
        let mut inp = input(&g, &p);
        inp.rooms = vec![room];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout];
        let d = layout(&inp, &o);

        let gold_frames = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter(|i| matches!(i, Item::Frame { rgb, .. } if *rgb == [200, 150, 40]))
            .count();
        assert_eq!(gold_frames, 1, "a fitting with no declared size was drawn at a made-up one");
    }

    /// EVERY ROOM AT ONE SCALE, so two rooms in one report can be compared.
    ///
    /// "make the grids scale proportional to that of the layout. since we are showing the rooms
    /// illuminance in the grid format they should be comparable." A 4 m cupboard and a 40 m hall
    /// used to print the same size, each fitted to two thirds of its own page. Nothing on the page
    /// contradicted the impression that they were comparable spaces except a dimension in small
    /// type, and the drawings themselves said the opposite of the truth.
    #[test]
    fn a_room_twice_the_size_draws_twice_the_size() {
        let (pw, ph) = PageSize::A4.points();
        let small = CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 4.0,
            depth: 3.0,
            cols: 8,
            rows: 6,
        };
        let big = CalcPlane { width: 8.0, depth: 6.0, ..small };
        let g = grid(8, 6);
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];

        let mut inp = input(&g, &small);
        inp.rooms = vec![one_room(&g, &small, "Store"), one_room(&g, &big, "Hall")];
        let d = layout(&inp, &o);
        let f = plot_frames(&d);
        assert_eq!(f.len(), 2, "expected a field per room");

        let (_, _, w_small, h_small) = f[0];
        let (_, _, w_big, h_big) = f[1];
        assert!(
            (w_big / w_small - 2.0).abs() < 1e-6 && (h_big / h_small - 2.0).abs() < 1e-6,
            "a room twice the size drew {:.2}× wide and {:.2}× tall",
            w_big / w_small,
            h_big / h_small,
        );
        // And the LARGER one is the one that fills the page — the scale is set by the room that
        // needs the most, so nothing is drawn off the edge.
        assert!(w_big >= pw * 0.66 - 1.0 || h_big >= ph * 0.66 - 1.0, "the big room is not filling");
        assert!(w_big <= pw * TWO_THIRDS + 1.0 && h_big <= ph * TWO_THIRDS + 1.0, "and not more");
    }

    /// THE LAYOUT PAGE AND THE RESULTS PAGE ARE THE SAME ROOM AT THE SAME SIZE.
    ///
    /// They are read against each other — "which fitting is over that dark patch" is the question
    /// the pair exists to answer — and it cannot be answered by eye if one is drawn larger than
    /// the other. The results field used to be sized by cell count and the layout in metres, so
    /// they agreed only by coincidence.
    ///
    /// TWO ROOMS, AND THE SMALL ONE IS THE ONE CHECKED. With one room the shared scale and a
    /// page-fit are the same number, so a single-room fixture cannot tell the two rules apart —
    /// it passes just as happily against the behaviour this replaces. The small room is the case
    /// where they differ: the report's scale is set by the LARGE room, so a layout page still
    /// fitting itself to the page would draw the small room bigger than its own field.
    #[test]
    fn the_layout_and_the_field_line_up() {
        let g = grid(37, 11); // deliberately NOT proportional to either plane below
        let p = CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 9.0,
            depth: 7.0,
            cols: 37,
            rows: 11,
        };
        let big = CalcPlane { width: 30.0, depth: 22.0, ..p };
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout, Section::Results];
        let mut inp = input(&g, &p);
        inp.rooms = vec![one_room(&g, &p, "Store"), one_room(&g, &big, "Hall")];
        let d = layout(&inp, &o);
        // The two are drawn in different colours — the layout's room outline red, the field's
        // border grey — so they are collected by colour rather than by their order on the page.
        let outline: Vec<(f64, f64)> = d
            .pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Frame { w, h, rgb, .. } if *rgb == [190, 40, 40] => Some((*w, *h)),
                _ => None,
            })
            .collect();
        let field = plot_frames(&d);
        assert_eq!(outline.len(), 2, "expected a layout outline per room");
        assert_eq!(field.len(), 2, "expected a field per room");
        for (i, room) in ["Store", "Hall"].iter().enumerate() {
            let (w1, h1) = outline[i];
            let (_, _, w2, h2) = field[i];
            assert!(
                (w1 - w2).abs() < 1e-6 && (h1 - h2).abs() < 1e-6,
                "{room}: the layout is {w1:.1}×{h1:.1} and the field {w2:.1}×{h2:.1} — one room, \
                 two sizes",
            );
        }
    }

    /// THE BAND EDGES ARE CONTOURS, not a staircase of rectangles.
    ///
    /// "the false color is still way too coarse make it smooth. it looks all pixelated around the
    /// edges." A band drawn as a mosaic of axis-aligned rectangles has stepped edges BY
    /// CONSTRUCTION — every boundary has to land on a raster row — and raising the raster only
    /// makes the steps smaller. The edges are now traced with linear interpolation and emitted as
    /// polygons.
    ///
    /// THIS USED TO ASSERT "no rectangle taller than half a grid cell", which was the old
    /// implementation's shape rather than the requirement: a flat interior legitimately merges into
    /// one big rectangle now, and that is a smaller file drawing the same picture. What has to be
    /// true is that the BOUNDARY is finer than the grid.
    #[test]
    fn the_field_is_finer_than_the_grid_it_came_from() {
        let g = grid(8, 8);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        let d = layout(&input(&g, &p), &o);
        let (_, _, pw, ph) = plot_frames(&d)[0];
        let r = field_rects(&d);
        assert!(!r.is_empty(), "nothing was drawn");

        // The contour polygons, and how long their edges are.
        let polys: Vec<&Vec<Vec<(f64, f64)>>> = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter_map(|i| match i {
                Item::Poly { rings, .. } => Some(rings),
                _ => None,
            })
            .collect();
        assert!(!polys.is_empty(), "the band edges are not being traced at all");
        let cell_h = ph / 8.0;
        let mut longest = 0.0_f64;
        // THE DIVERSITY OF EDGE DIRECTIONS is what tells a traced contour from a stepped one.
        //
        // "Some edge is not axis-aligned" was the first version of this check and it does not
        // discriminate: snapping every crossing to the nearest cell CORNER — a staircase by any
        // reading — still throws 45° diagonals across corner cells, and that check passed against
        // it. A boundary interpolated to where the threshold actually falls takes a continuum of
        // angles; one snapped to the lattice can only ever take a handful.
        let mut angles: std::collections::BTreeSet<i64> = Default::default();
        for rings in &polys {
            for r in rings.iter() {
                for k in 0..r.len() {
                    let (x1, y1) = r[k];
                    let (x2, y2) = r[(k + 1) % r.len()];
                    let (dx, dy) = (x2 - x1, y2 - y1);
                    let len = dx.hypot(dy);
                    longest = longest.max(len);
                    if len > 1e-9 {
                        let mut a = dy.atan2(dx).to_degrees();
                        if a < 0.0 {
                            a += 180.0; // a direction, not an orientation
                        }
                        angles.insert((a / 3.0).round() as i64);
                    }
                }
            }
        }
        assert!(
            longest < cell_h * 0.5,
            "a band edge runs {longest:.2} pt in one straight piece, against a {cell_h:.2} pt grid \
             cell — the boundary is no finer than the measurements",
        );
        assert!(
            angles.len() > 8,
            "the band edges take only {} distinct directions — a contour interpolated to where the \
             threshold falls takes many; this is snapped to the raster",
            angles.len(),
        );
        // …and the rows cover the plot from top to bottom.
        let top = r.iter().map(|x| x.1).fold(f64::MAX, f64::min);
        let bot = r.iter().map(|x| x.1 + x.3).fold(f64::MIN, f64::max);
        assert!(bot - top > ph * 0.9, "the field covers {:.0} of {ph:.0} pt", bot - top);
        assert!(pw > 0.0);
    }


    /// THE FIELD IS INTERPOLATED, not merely drawn at a finer pitch.
    ///
    /// Resampling without interpolating gives the same blocky field, only made of more rectangles
    /// — the band edges still land exactly on grid-cell boundaries, so the sampling grid is still
    /// what the eye sees. Bilinear is what makes a band edge a curve.
    ///
    /// Told apart by WHERE the colour changes: with nearest-neighbour every row breaks at the same
    /// handful of x positions, one per grid column. With bilinear over a field that varies in y,
    /// each row breaks somewhere slightly different.
    #[test]
    fn the_field_is_interpolated_and_not_just_finely_diced() {
        // A diagonal ramp, so the band edges are diagonal lines rather than horizontal ones.
        let cols = 8u32;
        let rows = 8u32;
        let vals: Vec<f64> = (0..(cols * rows))
            .map(|i| {
                let (x, y) = ((i % cols) as f64, (i / cols) as f64);
                (x + y) * 60.0
            })
            .collect();
        let min = vals.iter().cloned().fold(f64::MAX, f64::min);
        let max = vals.iter().cloned().fold(f64::MIN, f64::max);
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        let g = LuxGrid {
            cols,
            rows,
            values: vals,
            min,
            max,
            avg,
            maintenance: 1.0,
            direct: Vec::new(),
            indirect: Vec::new(),
        };
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        let d = layout(&input(&g, &p), &o);

        // OFF THE GRID LATTICE, which is what interpolation means here.
        //
        // This used to count distinct run-start positions and require more than three per grid
        // column — a number tuned to the fixture rather than to the question, and one that duly
        // came out at 23 against a required 24 the moment the default bands changed. How MANY runs
        // there are depends on how many bands the fixture happens to cross; where their edges FALL
        // does not. Nearest-neighbour can only ever break at a grid column; bilinear breaks
        // wherever the threshold actually is.
        let (px, _, pw, _) = plot_frames(&d).first().copied().expect("a field");
        let pitch = pw / cols as f64;
        let mut off_lattice = 0usize;
        let mut total = 0usize;
        for r in field_rects(&d) {
            // The plot's own left edge is a lattice position by construction; ignore it.
            let along = r.0 - px;
            if along < pitch * 0.25 {
                continue;
            }
            total += 1;
            let frac = (along / pitch).fract();
            if frac > 0.08 && frac < 0.92 {
                off_lattice += 1;
            }
        }
        assert!(total > 0, "the field drew no runs at all");
        assert!(
            off_lattice * 4 > total,
            "only {off_lattice} of {total} run edges fall between grid columns — a run can only \
             begin ON a column if the field was resampled nearest-neighbour rather than \
             interpolated",
        );
    }

    /// A BANDED SCALE GIVES FEW COLOURS, which is what makes the bands readable — and the runs
    /// merge, so a smooth field does not cost one rectangle per sample.
    #[test]
    fn a_banded_field_merges_into_runs() {
        let g = grid(8, 8);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        let d = layout(&input(&g, &p), &o);
        let r = field_rects(&d);
        let colours: std::collections::BTreeSet<[u8; 3]> = r.iter().map(|x| x.4).collect();
        assert!(
            colours.len() <= 8,
            "a four-band scale produced {} colours — the bands are not being snapped",
            colours.len(),
        );
        // One rectangle per sample would be hundreds of thousands; merged runs are far fewer.
        assert!(r.len() < 20_000, "{} rectangles is not a merged field", r.len());
    }

    /// EVERY ROOM CARRIES ITS OWN CONDITIONS.
    ///
    /// Asked for as: *"the room details need to there for each rooms result"* and *"make sure we
    /// also displaying these info in each room"*. The room height, the reflectances and the
    /// installation's headline figures were printed ONCE at the front of the report, which is
    /// where they come from — but they are the stated conditions of every room's numbers, and a
    /// reader working through one room's chapter should not have to leaf backwards to find out
    /// what the walls were given.
    ///
    /// Counted PER ROOM rather than merely "present": one copy at the front would satisfy "the
    /// report mentions reflectance" while leaving the complaint exactly as it was.
    #[test]
    fn each_room_states_its_own_conditions() {
        let g = grid(8, 6);
        let p = plane();
        let fixtures = vec![cad_light::Luminaire {
            id: 1,
            profile: "F".into(),
            position: cad_light::Vertex::new(4.0, 3.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];
        let inst = Installation {
            count: 1,
            total_watts: 52.0,
            total_lumens: 4160.0,
            area_m2: 48.0,
            power_density: 1.08,
            efficacy: 80.0,
            missing_watts: 0,
            missing_lumens: 0,
        };
        let mut rooms = Vec::new();
        for name in ["Store", "Hall"] {
            let mut r = one_room(&g, &p, name);
            r.fixtures = &fixtures;
            r.installation = Some(&inst);
            rooms.push(r);
        }
        let mut inp = input(&g, &p);
        inp.rooms = rooms;
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Installation];
        let d = layout(&inp, &o);
        let t = texts(&d);
        // EXACT, not `contains`. "Total power" is a substring of "Total power per area (48.00 m2)",
        // and counting loosely made a row that appears twice look like four.
        let count = |needle: &str| t.iter().filter(|s| s.as_str() == needle).count();

        for want in [
            "General",
            "Calculation algorithm",
            "Height of luminaire plane",
            "Working plane height",
            "Room height",
            "Maintenance factor",
            "Luminaire luminous flux",
            "Total power",
            "Room & materials",
            "Room size",
            "Reflectance — Floor",
        ] {
            assert_eq!(
                count(want),
                2,
                "{want:?} appears {} time(s) across two rooms — it belongs in each chapter",
                count(want),
            );
        }
        // The area is written into the label, so this one is matched by its stem.
        assert_eq!(
            t.iter().filter(|s| s.starts_with("Total power per area")).count(),
            2,
            "the power density is not stated in each room's chapter",
        );
        // The normalised figure, which is the one a scheme is actually judged on.
        assert!(
            t.iter().any(|s| s.contains("per 100 lx")),
            "the power density is not given per 100 lx: {t:?}",
        );
        // The luminaire plane is the FITTINGS' height, not the working plane's.
        assert!(t.iter().any(|s| s == "2.90 m"), "the mounting height is not stated: {t:?}");
    }

    /// THE FIELD AND THE LAYOUT SHOW THE ROOM THE SAME WAY UP.
    ///
    /// Reported as: *"it looks like the result with false color are flipped compared to the
    /// lighting layout. make sure both in the same orientation."* They were. A plan reads with +y
    /// UP and a page with +y DOWN; the layout page has always flipped, and the field did not — so
    /// grid row 0, which is the plane's MINIMUM y, was drawn at the top of the page.
    ///
    /// That is worse than either page being wrong on its own. The two exist to be read against
    /// each other — "which fitting is over that dark patch" — and mirrored, both look entirely
    /// plausible while every answer taken from the pair is wrong.
    ///
    /// The fixture is deliberately asymmetric in y: bright along the room's NORTH edge, dark along
    /// its south. On the page the bright band must be at the TOP.
    #[test]
    fn the_field_is_the_same_way_up_as_the_layout() {
        let (cols, rows) = (8u32, 8u32);
        // Row 0 is the minimum y — the SOUTH edge — so brightness must rise with the row index.
        let vals: Vec<f64> =
            (0..(cols * rows)).map(|i| 10.0 + (i / cols) as f64 * 120.0).collect();
        let g = LuxGrid::from_values(cols, rows, vals);
        let p = plane();
        // A fitting hard against the north edge, so the layout page is asymmetric the same way.
        let fixtures = vec![cad_light::Luminaire {
            id: 1,
            profile: "F".into(),
            position: cad_light::Vertex::new(4.0, 5.5, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }];
        let mut room = one_room(&g, &p, "");
        room.fixtures = &fixtures;
        let mut inp = input(&g, &p);
        inp.rooms = vec![room];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout, Section::Results];
        let d = layout(&inp, &o);

        let field = plot_frames(&d).first().copied().expect("a field");
        let (fx, fy, fw, fh) = field;
        let top = colour_at(&d, fx + fw * 0.5, fy + fh * 0.12).expect("the top of the field");
        let bottom = colour_at(&d, fx + fw * 0.5, fy + fh * 0.88).expect("the bottom of the field");
        assert_ne!(top, bottom, "the fixture is not asymmetric enough to tell either way up");

        // BY BAND INDEX, not by how warm the colour looks.
        //
        // This used to measure nearness to the warm end of the classic ramp, on the reasoning that
        // bright is warm. That is a property of one palette and not of the question being asked —
        // and the default scheme is now the one DIALux and Relux use, where LOW is warm and HIGH is
        // pale. The oracle was inverted overnight while the thing it tests was untouched. Which
        // band a colour is answers the question in any palette.
        let band_of = |c: [u8; 3]| {
            o.band_colours.iter().position(|x| *x == c).unwrap_or_else(|| {
                panic!("{c:?} is not one of the scale's band colours: {:?}", o.band_colours)
            })
        };
        assert!(
            band_of(top) > band_of(bottom),
            "the bright end of the room is at the BOTTOM of the page ({top:?} over {bottom:?}) — \
             the field is upside down against the layout",
        );

        // And the fitting, which is near the north edge, is likewise drawn in the top half.
        let marker_y = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter_map(|i| match i {
                Item::Frame { y, h, rgb, .. } if *rgb == [200, 150, 40] => Some(y + h * 0.5),
                _ => None,
            })
            .next()
            .expect("the fitting's marker");
        let outline = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .find_map(|i| match i {
                Item::Frame { y, h, rgb, .. } if *rgb == [190, 40, 40] => Some((*y, *h)),
                _ => None,
            })
            .expect("the layout outline");
        assert!(
            marker_y < outline.0 + outline.1 * 0.5,
            "a fitting near the room's north edge was drawn in the bottom half of the layout",
        );
    }

    /// THE PREVIEW CAN DRAW EVERYTHING THE PDF CAN.
    ///
    /// Reported as: the saved report looks right and the preview does not. It did not — the field
    /// came out as an empty white box on screen and correct on paper.
    ///
    /// The cause was ONE ITEM NEEDING A FILL RULE THE PREVIEW DOES NOT HAVE. The ground outside
    /// the room was hidden with a single white shape carrying two rings — the plot rectangle and
    /// the room outline — relying on the even-odd rule to turn the second into a hole. A PDF
    /// reader does that. egui has no even-odd, filled each ring in turn, and painted the entire
    /// room white. Two renderers, one item, different pictures, and only one of them is the thing
    /// that gets issued.
    ///
    /// So the invariant is structural rather than visual: no item may carry more than one ring.
    /// With a single ring every fill rule agrees, which is what makes "the preview IS the document"
    /// true rather than merely intended. A visual test would have to rasterise egui and would
    /// still only cover the page it happened to look at.
    #[test]
    fn no_drawing_needs_a_fill_rule_the_preview_lacks() {
        let l = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 3.0),
            glam::Vec2::new(3.0, 3.0),
            glam::Vec2::new(3.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
        ];
        let g = grid(24, 18);
        let p = plane();
        let mut inp = input(&g, &p);
        inp.rooms[0].poly = &l; // an L-shaped room, which is where holes would come from
        let mut o = opts();
        o.cover = false;
        o.sections = crate::report::Section::all();
        let d = layout(&inp, &o);

        let worst = d
            .pages
            .iter()
            .flat_map(|pg| pg.items.iter())
            .filter_map(|i| match i {
                Item::Poly { rings, .. } => Some(rings.len()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        assert!(worst > 0, "nothing was drawn as a polygon at all — the fixture proves nothing");
        assert_eq!(
            worst, 1,
            "an item carries {worst} rings, so the PDF and the on-screen preview will fill it \
             differently",
        );
    }


    /// GROUND OUTSIDE THE ROOM IS NOT COLOURED — colouring it reports illuminance where the room
    /// is not.
    ///
    /// ASKED OF THE FINISHED PAGE, not of the shape list. This used to check that no coloured
    /// rectangle was drawn beyond the room, which was true of the old renderer and is no longer
    /// the question: bands are now traced across the whole rectangle and the outside is hidden by
    /// a shape drawn afterwards, so coloured geometry does extend past the room and none of it
    /// shows. What matters is what is on top — see `colour_at`.
    #[test]
    fn cells_outside_the_room_are_not_coloured() {
        // Both halves of this: a room given as an OUTLINE, which is the real case and gets the
        // smooth cover, and a room given only as a cell MASK, which gets the stepped one.
        for by_outline in [true, false] {
            let g = grid(2, 1);
            let p = plane();
            let mut i = input(&g, &p);
            // The LEFT half of the plane only.
            let poly = [
                glam::Vec2::new(0.0, 0.0),
                glam::Vec2::new(4.0, 0.0),
                glam::Vec2::new(4.0, 6.0),
                glam::Vec2::new(0.0, 6.0),
            ];
            if by_outline {
                i.rooms[0].poly = &poly;
            } else {
                i.rooms[0].mask = &[true, false];
            }
            let mut o = opts();
            o.cover = false;
            o.sections = vec![Section::Results];
            let d = layout(&i, &o);
            let (px, py, pw, ph) = plot_frames(&d)[0];

            let inside = colour_at(&d, px + pw * 0.25, py + ph * 0.5);
            let outside = colour_at(&d, px + pw * 0.75, py + ph * 0.5);
            let how = if by_outline { "outline" } else { "mask" };
            assert!(
                inside.is_some_and(|c| c != [255, 255, 255]),
                "{how}: inside the room came out {inside:?} — the field is not painted",
            );
            // EITHER NOTHING OR WHITE. Nothing is the better of the two — bare paper, because the
            // field is never painted out there at all — and white is what a cover leaves behind.
            // What must not appear is a BAND COLOUR, which would report illuminance on ground the
            // room does not cover.
            assert!(
                matches!(outside, None | Some([255, 255, 255])),
                "{how}: ground outside the room shows {outside:?}, which reports illuminance where \
                 the room is not",
            );
        }
    }

    /// VALUES APPEAR WHEN THERE IS ROOM AND NOT WHEN THERE IS NOT — and when they are dropped the
    /// page says so, rather than leaving a reader to wonder whether the plot is the whole story.
    #[test]
    fn point_values_are_printed_only_when_legible() {
        let mut o = opts();
        o.sections = vec![Section::Results];
        o.cover = false;
        // A CONTINUOUS SCALE, so the only "100" on the page can be a point value. With the default
        // bands the legend writes "100" under itself, and this test would pass on that instead —
        // which is a test that cannot fail for the reason it names.
        o.scale = crate::report::options::Scale { top: None, bands: Vec::new() };

        let small = grid(4, 4);
        let p = plane();
        let t = texts(&layout(&input(&small, &p), &o));
        assert!(t.iter().any(|s| s == "100"), "a 4x4 plot has room for its values: {t:?}");

        let big = grid(60, 60);
        let t = texts(&layout(&input(&big, &p), &o));
        assert!(
            !t.iter().any(|s| s == "100"),
            "a 60x60 plot printed values that cannot be read at that size",
        );
        assert!(
            t.iter().any(|s| s.contains("omitted")),
            "the page must say the values were left off: {t:?}",
        );
    }

    /// THE SCALE IS THE REPORT'S DECISION, not the viewport's.
    #[test]
    fn pinning_the_scale_changes_the_colours() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];

        o.scale = crate::report::options::Scale { top: None, bands: Vec::new() };
        let auto: Vec<[u8; 3]> = field_fills(&layout(&input(&g, &p), &o));

        o.scale = crate::report::options::Scale { top: Some(5000.0), bands: Vec::new() };
        let pinned: Vec<[u8; 3]> = field_fills(&layout(&input(&g, &p), &o));

        assert_ne!(auto, pinned, "pinning the top to 5000 lx left every colour unchanged");
        let mean = |v: &[[u8; 3]]| -> u64 {
            v.iter().map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum::<u64>()
                / v.len().max(1) as u64
        };
        assert!(
            mean(&pinned) < mean(&auto),
            "a higher ceiling must push the same room DOWN the ramp, not up",
        );
    }

    /// THE LAYOUT PAGE SHOWS THE ROOM AND WHAT IS IN IT.
    ///
    /// "have a page showing the lighting layout before it shows the false colors." A field says how
    /// much light there is; it does not say where the fittings are, and a reader checking a design
    /// needs the layout that produced the numbers.
    #[test]
    fn the_layout_page_draws_the_room_and_its_fittings() {
        let g = grid(6, 6);
        let p = plane();
        let poly = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
        ];
        let lums: Vec<cad_light::Luminaire> = [(2.0_f32, 2.0_f32), (6.0, 2.0), (4.0, 4.5)]
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| cad_light::Luminaire {
                id: i as u32 + 1,
                profile: "P".into(),
                position: cad_light::Vertex::new(x, y, 2.7),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            })
            .collect();

        let mut i = input(&g, &p);
        i.rooms[0].poly = &poly;
        i.rooms[0].fixtures = &lums;
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout];
        let d = layout(&i, &o);
        let t = texts(&d);

        assert!(t.iter().any(|s| s == "Lighting layout"), "no heading: {t:?}");
        assert!(t.iter().any(|s| s.contains("3 fitting(s)")), "the count is missing: {t:?}");
        // A marker per fitting.
        let boxes = d.pages[0]
            .items
            .iter()
            .filter(|x| matches!(x, Item::Frame { rgb, .. } if *rgb == [200, 150, 40]))
            .count();
        assert_eq!(boxes, 3, "expected a marker per fitting, got {boxes}");
        // …and the outline, in red.
        let outline = d.pages[0]
            .items
            .iter()
            .filter(|x| matches!(x, Item::Line { rgb, .. } if *rgb == [190, 40, 40]))
            .count();
        assert!(outline >= 4, "the room outline has {outline} segments");
    }

    /// THE LAYOUT IS NOT UPSIDE DOWN. A plan reads with +y up and a page with +y down, so the
    /// drawing has to be flipped once — and a layout printed upside down against its own result is
    /// worse than no layout at all.
    #[test]
    fn the_layout_is_the_same_way_up_as_the_plan() {
        let g = grid(4, 4);
        let p = plane();
        // One fitting near the plan's TOP (high y), one near the bottom.
        let lums: Vec<cad_light::Luminaire> = [(4.0_f32, 5.5_f32), (4.0, 0.5)]
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| cad_light::Luminaire {
                id: i as u32 + 1,
                profile: "P".into(),
                position: cad_light::Vertex::new(x, y, 2.7),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            })
            .collect();
        let mut i = input(&g, &p);
        i.rooms[0].fixtures = &lums;
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Layout];
        let d = layout(&i, &o);

        // IN ITEM ORDER, not sorted — the markers are emitted in fixture order, so the first is
        // the one at y = 5.5 m. Sorting them first made the comparison vacuous: two numbers put
        // in order are always in order, whichever way up the drawing is.
        let ys: Vec<f64> = d.pages[0]
            .items
            .iter()
            .filter_map(|x| match x {
                Item::Frame { y, rgb, .. } if *rgb == [200, 150, 40] => Some(*y),
                _ => None,
            })
            .collect();
        assert_eq!(ys.len(), 2);
        // The fitting at y = 5.5 m is nearer the TOP of the plan, so nearer the top of the page —
        // which is the SMALLER page y.
        assert!(
            ys[0] < ys[1],
            "the fitting at y = 5.5 m printed at {:.1} and the one at y = 0.5 m at {:.1} — the \n             layout is mirrored vertically",
            ys[0],
            ys[1],
        );
    }

    /// THE NUMERIC GRID IS ONE PAGE, ALWAYS.
    ///
    /// "the illuminance grid comes in multiple pages. it should all be shown in a single page." A
    /// 70-column field was coming out as 23 pages of column blocks.
    #[test]
    fn the_numeric_grid_never_spans_pages() {
        for (cols, rows) in [(4u32, 4u32), (20, 20), (33, 40)] {
            let g = grid(cols, rows);
            let p = plane();
            let mut o = opts();
            o.cover = false;
            o.page_numbers = false;
            o.sections = vec![Section::NumericGrid];
            let d = layout(&input(&g, &p), &o);
            assert_eq!(d.pages.len(), 1, "{cols}x{rows}: the grid took {} pages", d.pages.len());
            let t = texts(&d);
            assert!(!t.iter().any(|s| s.starts_with("Columns ")), "{cols}x{rows}: still in blocks");
        }
    }



    /// Rough "how far up the ramp" — brighter ramp positions are lighter overall.
    fn t_sum(c: &[[u8; 3]]) -> u32 {
        c.iter().map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32).sum()
    }

    /// A BANDED SCALE DRAWS A BANDED LEGEND, with its edges written out — which is the thing that
    /// makes a lighting drawing readable rather than merely colourful.
    #[test]
    fn a_banded_scale_labels_its_edges() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        o.scale = crate::report::options::Scale { top: Some(500.0), bands: vec![25.0, 100.0, 300.0] };
        let d = layout(&input(&g, &p), &o);
        let t = texts(&d);
        for e in ["0", "25", "100", "300", "500"] {
            assert!(t.iter().any(|s| s == e), "band edge {e} is not written under the legend: {t:?}");
        }
        assert!(t.iter().any(|s| s.contains("Illuminance [lx]")), "the legend is unlabelled");
    }

    /// A LOGO GOES IN THE HEADER AND THE FOOTER, fitted to a stated box and never stretched.
    #[test]
    fn header_and_footer_logos_are_fitted_not_stretched() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.header = "HSI Lighting".into();
        // LOGOS, not renders — they are separate lists.
        o.logos = vec![ReportImage {
            path: "logo.png".into(),
            caption: "logo".into(),
            // Deliberately not the box's aspect: 4:1 against a 5:1 box.
            jpeg: Some((vec![0xFF, 0xD8], 800, 200)),
        }];
        o.header_image = Some(0);
        o.footer_image = Some(0);
        let d = layout(&input(&g, &p), &o);

        // The cover carries neither, so look at a body page.
        let body = &d.pages[1];
        let imgs: Vec<(f64, f64)> = body
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Image { w, h, .. } => Some((*w, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(imgs.len(), 2, "a header logo and a footer logo");
        for (w, h) in imgs {
            assert!(w <= LOGO_W + 1e-6 && h <= LOGO_H + 1e-6, "logo {w}x{h} escapes its box");
            assert!((w / h - 4.0).abs() < 1e-6, "logo {w}x{h} was stretched from 4:1");
        }
    }



    /// A HUGE GRID IS COARSENED, not dropped and not spread over pages.
    ///
    /// "for the illumination grid coarsen it. no need to have a millions points scale it so it
    /// fits the a4 size but make sure the report doesnt miss the main points." A 125 × 38 room is
    /// 4,750 values — 0.9 pt type on A4, which is a grey texture rather than numbers.
    #[test]
    fn a_huge_grid_is_coarsened_onto_one_page() {
        let g = grid(125, 38);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.page_numbers = false;
        o.sections = vec![Section::NumericGrid];
        let d = layout(&input(&g, &p), &o);

        assert_eq!(d.pages.len(), 1, "the grid took {} pages", d.pages.len());
        let t = texts(&d);
        assert!(t.iter().any(|s| s == "Illuminance grid (lx)"), "the section went missing: {t:?}");
        assert!(
            t.iter().any(|s| s.contains("point of the 125 × 38 grid") && s.contains("spacing")),
            "the page must say what spacing it printed at: {t:?}",
        );
        // Coarsened, so far fewer figures than the grid holds — but not none.
        let numbers = t.iter().filter(|s| s.parse::<f64>().is_ok()).count();
        assert!(numbers > 20, "only {numbers} figures were printed");
        assert!(numbers < 4750 / 4, "{numbers} figures is not a coarsened grid");
    }

    /// A GRID THAT FITS IS NOT COARSENED, and says nothing about spacing — every point is there.
    #[test]
    fn a_small_grid_is_printed_whole() {
        let g = grid(10, 10);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::NumericGrid];
        let t = texts(&layout(&input(&g, &p), &o));
        assert!(t.iter().any(|s| s == "Illuminance grid (lx)"));
        assert!(
            !t.iter().any(|s| s.contains("spacing")),
            "a grid that fits must not claim to have been coarsened: {t:?}",
        );
        let numbers = t.iter().filter(|s| s.parse::<f64>().is_ok()).count();
        assert!(numbers >= 100, "only {numbers} of 100 values were printed");
    }

    /// THE EXTREMES ARE OVER THE WHOLE GRID, not over what was printed.
    ///
    /// This is the "does not miss the main points" guarantee. Decimation steps over points, and
    /// the one it steps over may be the darkest in the room — which is the figure the whole report
    /// is read for. So the true minimum and maximum are stated with their coordinates, whether or
    /// not they were among the printed ones.
    #[test]
    fn the_true_extremes_are_reported_even_when_decimation_skips_them() {
        // A flat field with ONE dark point, placed so a stride of 2 or more steps over it.
        let (cols, rows) = (60u32, 40u32);
        let mut vals = vec![300.0_f64; (cols * rows) as usize];
        let dark_at = (13 * cols + 27) as usize; // odd row, odd column
        vals[dark_at] = 7.0;
        let bright_at = (21 * cols + 35) as usize;
        vals[bright_at] = 990.0;
        let g = LuxGrid {
            cols,
            rows,
            values: vals,
            min: 7.0,
            max: 990.0,
            avg: 300.0,
            maintenance: 1.0,
            direct: Vec::new(),
            indirect: Vec::new(),
        };
        let p = CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 60.0,
            depth: 40.0,
            cols,
            rows,
        };
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::NumericGrid];
        let d = layout(&input(&g, &p), &o);
        let t = texts(&d);

        assert!(
            t.iter().any(|s| s.contains("spacing")),
            "precondition: this grid must have been coarsened",
        );
        assert!(
            !t.iter().any(|s| s == "7"),
            "precondition: the dark point must have been stepped over by the decimation",
        );
        assert!(t.iter().any(|s| s.contains("Minimum over the whole grid")), "no minimum: {t:?}");
        assert!(
            t.iter().any(|s| s.starts_with("7 lx at (")),
            "the true minimum is missing — the report stepped over the darkest point: {t:?}",
        );
        assert!(
            t.iter().any(|s| s.starts_with("990 lx at (")),
            "the true maximum is missing: {t:?}",
        );
        // …and the coordinates are the real ones: column 27 of 60 across 60 m is 27.5 m.
        assert!(
            t.iter().any(|s| s.contains("(27.50, 13.50) m")),
            "the minimum is reported at the wrong place: {t:?}",
        );
    }

    /// A MASKED POINT IS NOT AN EXTREME. The darkest cell of the rectangle may be outside the room
    /// entirely, and reporting it would state a minimum on ground the room does not occupy.
    #[test]
    fn the_extremes_ignore_ground_outside_the_room() {
        let g = grid(2, 1);
        let p = plane();
        let mut i = input(&g, &p);
        i.rooms[0].mask = &[true, false];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::NumericGrid];
        let t = texts(&layout(&i, &o));
        // Values are 100 and 150; the second is outside, so both extremes are 100.
        assert!(t.iter().any(|s| s.starts_with("100 lx at (")), "the minimum is wrong: {t:?}");
        assert!(
            !t.iter().any(|s| s.starts_with("150 lx at (")),
            "a point outside the room was reported as the maximum: {t:?}",
        );
    }

    /// THE GRID IS SIZED AGAINST A WHOLE PAGE, not the tail of the previous one.
    ///
    /// Measuring against what is left would reject a table that fits perfectly well simply because
    /// a plot happened to run to the bottom of the page above it.
    #[test]
    fn a_grid_after_a_full_page_still_prints() {
        // Tall enough that it does NOT fit in the sliver left under a two-thirds plot, and small
        // enough that it fits a page of its own — which is the whole distinction being tested.
        let g = grid(20, 60);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        // The plot fills two thirds of the page, so the grid starts near the bottom.
        o.sections = vec![Section::Results, Section::NumericGrid];
        let t = texts(&layout(&input(&g, &p), &o));
        assert!(
            t.iter().any(|s| s == "Illuminance grid (lx)"),
            "the grid was rejected for want of space on a page it does not have to share: {t:?}",
        );
    }

    /// LOGOS ARE THEIR OWN LIST, and a logo is not a render.
    ///
    /// They shared one list, so putting a logo in the header meant adding it as a RENDER first —
    /// where it then appeared, full width, on the renders page. Reported as "i have to add image
    /// at the render image addition then add them in the logo. i need it to be seperate".
    #[test]
    fn a_logo_is_not_a_render() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.header = "HSI Lighting".into();
        o.sections = vec![Section::Summary, Section::Renders];
        o.logos = vec![ReportImage {
            path: "logo.png".into(),
            caption: "logo".into(),
            jpeg: Some((vec![0xFF, 0xD8], 800, 200)),
        }];
        o.header_image = Some(0);
        let d = layout(&input(&g, &p), &o);

        // The logo is on the page furniture…
        let body_imgs = d.pages[1].items.iter().filter(|i| matches!(i, Item::Image { .. })).count();
        assert_eq!(body_imgs, 1, "the header logo is not on the body page");
        // …and there is NO renders page, because no render was added.
        assert!(
            !texts(&d).iter().any(|s| s == "Renders"),
            "a logo produced a renders page",
        );
    }

    /// AND THE TWO LISTS SHARE ONE IMAGE TABLE without crossing over. A logo's index has to be
    /// offset past the renders, or the header shows the first render instead.
    #[test]
    fn a_logo_index_does_not_point_at_a_render() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.header = "HSI".into();
        o.sections = vec![Section::Summary, Section::Renders];
        o.images = vec![ReportImage {
            path: "render.jpg".into(),
            caption: "the room".into(),
            // 2:1, deliberately unlike the logo below.
            jpeg: Some((vec![0xFF, 0xD8], 2000, 1000)),
        }];
        o.logos = vec![ReportImage {
            path: "logo.png".into(),
            caption: "logo".into(),
            jpeg: Some((vec![0xFF, 0xD9], 800, 200)), // 4:1
        }];
        o.header_image = Some(0);
        let d = layout(&input(&g, &p), &o);

        assert_eq!(d.images.len(), 2, "both lists reach the file's image table");
        // The header image must be the LOGO — told apart by its aspect, since the two differ.
        let header = d.pages[1]
            .items
            .iter()
            .find_map(|i| match i {
                Item::Image { w, h, idx, .. } => Some((*w, *h, *idx)),
                _ => None,
            })
            .expect("a header image");
        assert_eq!(header.2, 1, "the header points at image {} — the render", header.2);
        assert!(
            (header.0 / header.1 - 4.0).abs() < 1e-6,
            "the header image is {:.0}x{:.0}, which is the render's shape",
            header.0,
            header.1,
        );
    }

    /// A CHAPTER PER ROOM.
    ///
    /// "if theres multiple rooms does it generate report for all the rooms seperately and show the
    /// lights used in that room" — it did not: three rooms came out as one plot with a bounding
    /// box around them, because a calculation produced one room's numbers.
    #[test]
    fn each_room_gets_its_own_chapter_and_plot() {
        let g = grid(6, 6);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Summary, Section::Results];
        let mut i = input(&g, &p);
        i.rooms = vec![one_room(&g, &p, "Office"), one_room(&g, &p, "Store")];
        let d = layout(&i, &o);
        let t = texts(&d);

        for name in ["Office", "Store"] {
            assert!(t.iter().any(|s| s == name), "no chapter for {name}: {t:?}");
        }
        // TWO plots, not one — each room gets its own field, with its own frame around it.
        assert_eq!(plot_frames(&d).len(), 2, "expected a plot per room");
        // …and two Summary headings, one under each chapter.
        assert_eq!(t.iter().filter(|s| *s == "Summary").count(), 2);
    }

    /// A SINGLE-ROOM REPORT IS UNCHANGED — no chapter heading for a room that is the whole report.
    #[test]
    fn one_unnamed_room_gets_no_chapter_heading() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Summary];
        let d = layout(&input(&g, &p), &o);
        let t = texts(&d);
        assert!(t.iter().any(|s| s == "Summary"));
        assert_eq!(d.pages.len(), 1, "a one-room summary should not need a chapter page");
    }

    /// THE SCHEDULE SAYS WHAT THE ROOM IS LIT WITH — by type, with the manufacturer the file
    /// declares. A report that states an illuminance without saying what produced it cannot be
    /// checked, ordered from, or handed to an installer.
    #[test]
    fn the_schedule_lists_the_fittings_by_type() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Schedule];
        let mut i = input(&g, &p);
        i.rooms[0].schedule = vec![
            ScheduleRow {
                profile: "OCULUS GRANDE 2.0".into(),
                count: 12,
                manufacturer: "HSI Lighting".into(),
                catalogue: "OG20-36".into(),
                lamp: "LED 3000K · CRI 90".into(),
                watts: 22.0,
                lumens: 2400.0,
                size_m: (0.095, 0.095, 0.06),
            },
            ScheduleRow {
                profile: "LINEA W48".into(),
                count: 3,
                manufacturer: String::new(),
                catalogue: String::new(),
                lamp: String::new(),
                watts: 52.0,
                lumens: 5600.0,
                size_m: (2.0, 0.048, 0.08),
            },
        ];
        let d = layout(&i, &o);
        let t = texts(&d);

        assert!(t.iter().any(|s| s == "OCULUS GRANDE 2.0"), "the fitting is not named: {t:?}");
        assert!(t.iter().any(|s| s == "12"), "the quantity is missing");
        assert!(t.iter().any(|s| s == "HSI Lighting"), "the manufacturer is missing");
        assert!(t.iter().any(|s| s.contains("OG20-36")), "the catalogue number is missing");
        assert!(t.iter().any(|s| s.contains("CRI 90")), "the lamp description is missing");
        assert!(t.iter().any(|s| s == "22.0 W"), "the wattage is missing");
        assert!(t.iter().any(|s| s == "2400 lm"), "the flux is missing");
        assert!(t.iter().any(|s| s == "109"), "the efficacy is missing (2400/22)");
        assert!(t.iter().any(|s| s.contains("95 × 95 × 60")), "the size is missing: {t:?}");

        // A FILE THAT DECLARES NO MANUFACTURER SHOWS A DASH — that is the file's omission, not the
        // report's, and inventing one would be worse than saying nothing.
        assert!(t.iter().any(|s| s == "—"), "a missing manufacturer must read as absent");

        // The totals, which is what a schedule is read for.
        assert!(t.iter().any(|s| s == "15 fitting(s)"), "no total count: {t:?}");
        assert!(
            t.iter().any(|s| s.starts_with("420.0 W")),
            "no connected load — 12×22 + 3×52 = 420 W: {t:?}",
        );
    }

    /// SEVERAL ROOMS ALSO GET A WHOLE-SCHEME TOTAL, because the order goes out as one job.
    #[test]
    fn many_rooms_get_a_combined_schedule() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Schedule];
        let row = |n: usize| ScheduleRow {
            profile: "OCULUS".into(),
            count: n,
            manufacturer: "HSI".into(),
            catalogue: String::new(),
            lamp: String::new(),
            watts: 20.0,
            lumens: 2000.0,
            size_m: (0.1, 0.1, 0.05),
        };
        let mut i = input(&g, &p);
        i.rooms = vec![one_room(&g, &p, "A"), one_room(&g, &p, "B")];
        i.rooms[0].schedule = vec![row(4)];
        i.rooms[1].schedule = vec![row(6)];

        let merged = i.total_schedule();
        assert_eq!(merged.len(), 1, "one type across both rooms");
        assert_eq!(merged[0].count, 10, "4 + 6");

        // ITS OWN SECTION. It used to arrive free with the per-room Schedule, which is why it
        // could be neither switched off nor moved.
        o.sections = vec![Section::Schedule, Section::WholeScheme];
        let t = texts(&layout(&i, &o));
        assert!(t.iter().any(|s| s == "10"), "the combined quantity is not on the page: {t:?}");
        assert!(
            t.iter().any(|s| s.contains("all rooms")),
            "the combined schedule is not labelled as such",
        );

        // AND IT CAN BE TURNED OFF, which was impossible before.
        o.sections = vec![Section::Schedule];
        let without = texts(&layout(&i, &o));
        assert!(
            !without.iter().any(|s| s.contains("all rooms")),
            "the whole-scheme total printed with its section switched off: {without:?}",
        );

        // AND MOVED. Reported as "the whole scheme isnt movable its always stuck to the last
        // page" — it was emitted after the last room regardless of where it sat in the list, so a
        // practice that opens its reports with the totals had no way to.
        //
        // Compared by PAGE, since that is what "stuck to the last page" means: put first, the
        // total must appear before the room chapters begin.
        o.sections = vec![Section::WholeScheme, Section::Schedule];
        let d = layout(&i, &o);
        let page_of = |needle: &str| -> Option<usize> {
            d.pages.iter().position(|pg| {
                pg.items.iter().any(|it| match it {
                    Item::Text { text, .. } => text.contains(needle),
                    _ => false,
                })
            })
        };
        let total = page_of("all rooms").expect("the whole-scheme page");
        let first_room = page_of("15 fitting(s)").or_else(|| page_of("4 fitting(s)"));
        assert_eq!(total, 0, "listed first, the whole-scheme total is on page {total}");
        if let Some(r) = first_room {
            assert!(
                total <= r,
                "the total is on page {total} and the first room's schedule on page {r}",
            );
        }
    }

    /// A BUILDING-WIDE SECTION KEEPS ITS PLACE. The order IS the document, so listing Surfaces
    /// first must print it first rather than sweeping it to the end behind the room chapters.
    #[test]
    fn a_building_wide_section_keeps_its_position() {
        let g = grid(4, 4);
        let p = plane();
        let surf = [SurfaceResult {
            material: 0,
            name: "Ceiling".into(),
            area_m2: 10.0,
            e_avg: 120.0,
            e_min: 80.0,
            e_max: 200.0,
            l_avg: 26.0,
            u0: 0.67,
            samples: 64,
        }];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Surfaces, Section::Summary];
        let mut i = input(&g, &p);
        i.surfaces = &surf;
        let t = texts(&layout(&i, &o));
        let at = |s: &str| t.iter().position(|x| x == s).unwrap_or(usize::MAX);
        assert!(at("Surfaces") < at("Summary"), "Surfaces was swept behind the rooms: {t:?}");
    }

    /// THE SECTION ORDER IS THE DOCUMENT'S ORDER. Moving the renders page is the whole reason the
    /// sections are a list, so the layout has to honour it rather than emit a fixed sequence.
    #[test]
    fn sections_come_out_in_the_order_they_are_listed() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Surfaces, Section::Summary, Section::WorkingPlane];
        let surf = [SurfaceResult {
            material: 0,
            name: "Ceiling".into(),
            area_m2: 10.0,
            e_avg: 120.0,
            e_min: 80.0,
            e_max: 200.0,
            l_avg: 26.0,
            u0: 0.67,
            samples: 64,
        }];
        let mut i = input(&g, &p);
        i.surfaces = &surf;
        let d = layout(&i, &o);
        let t = texts(&d);
        let at = |s: &str| t.iter().position(|x| x == s).unwrap_or(usize::MAX);
        assert!(at("Surfaces") < at("Summary"), "Surfaces was not first: {t:?}");
        assert!(at("Summary") < at("Working plane"), "Summary was not second: {t:?}");
    }

    /// A SECTION THAT WAS SWITCHED OFF IS ABSENT.
    #[test]
    fn an_unselected_section_does_not_appear() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.set(Section::NumericGrid, false);
        let d = layout(&input(&g, &p), &o);
        assert!(
            !texts(&d).iter().any(|s| s == "Illuminance grid (lx)"),
            "the grid was printed after being switched off",
        );
    }

    /// THE COVER IS A PAGE OF ITS OWN, and carries neither header nor page furniture — a running
    /// header on a title page reads as a mistake.
    #[test]
    fn the_cover_is_its_own_page() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.title = "Gym · Level 2".into();
        o.subtitle = "Issued for tender".into();
        o.header = "HSI Lighting".into();
        o.footer = "confidential".into();
        let d = layout(&input(&g, &p), &o);
        assert!(d.pages.len() >= 2, "a cover and at least one body page");
        let cover: Vec<String> = d.pages[0]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(cover.iter().any(|s| s == "Gym · Level 2"), "the title is not on the cover");
        assert!(cover.iter().any(|s| s == "Issued for tender"), "the extra line is not there");
        assert!(!cover.iter().any(|s| s == "HSI Lighting"), "the header ran on the cover");
        assert!(!cover.iter().any(|s| s == "confidential"), "the footer ran on the cover");

        // …and the body pages DO carry them.
        let body: Vec<String> = d.pages[1]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(body.iter().any(|s| s == "HSI Lighting"), "no header on the body");
        assert!(body.iter().any(|s| s == "confidential"), "no footer on the body");
    }

    /// THE PAGE NUMBER KNOWS THE TOTAL. "1 / 7" cannot be written while it is still being decided
    /// how many pages there are, which is why the furniture goes on last.
    #[test]
    fn page_numbers_count_the_whole_document() {
        let g = grid(33, 40);
        let p = plane();
        let mut o = opts();
        o.page_numbers = true;
        let d = layout(&input(&g, &p), &o);
        let n = d.pages.len();
        assert!(n >= 2, "this grid needs more than one page");
        let last: Vec<String> = d.pages[n - 1]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            last.iter().any(|s| *s == format!("{n} / {n}")),
            "the last page is not numbered {n} of {n}: {last:?}",
        );
    }

    /// RENDERS GET A PAGE OF THEIR OWN, wherever the user put the section.
    #[test]
    fn renders_start_a_page_and_follow_the_chosen_position() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.images = vec![ReportImage {
            path: "a.jpg".into(),
            caption: "View from the entrance".into(),
            jpeg: Some((vec![0xFF, 0xD8, 0xFF, 0xD9], 1600, 900)),
        }];
        o.sections = vec![Section::Summary, Section::Renders, Section::Surfaces];
        let d = layout(&input(&g, &p), &o);
        // The renders page is a page whose first item belongs to the section.
        let page_with_image = d
            .pages
            .iter()
            .position(|pg| pg.items.iter().any(|i| matches!(i, Item::Image { .. })))
            .expect("the render was not placed");
        let t: Vec<String> = d.pages[page_with_image]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(t.iter().any(|s| s == "Renders"), "the page is not headed Renders");
        assert!(t.iter().any(|s| s == "View from the entrance"), "the caption is missing");
        assert!(
            !t.iter().any(|s| s == "Summary"),
            "the renders were squeezed onto the summary page rather than given one",
        );
    }

    /// AN IMAGE KEEPS ITS SHAPE. A render stretched to the box would misrepresent the room it is a
    /// picture of.
    #[test]
    fn a_render_is_not_stretched() {
        assert_eq!(fit(1600.0, 900.0, 400.0, 400.0), (400.0, 225.0));
        assert_eq!(fit(900.0, 1600.0, 400.0, 400.0), (225.0, 400.0));
        assert_eq!(fit(0.0, 0.0, 400.0, 400.0), (0.0, 0.0), "a zero image is not a division by it");
    }

    /// A RENDER SECTION WITH NO IMAGES PRINTS NOTHING — not an empty page with a heading on it.
    #[test]
    fn an_empty_render_section_is_absent() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Renders];
        let d = layout(&input(&g, &p), &o);
        assert!(!texts(&d).iter().any(|s| s == "Renders"), "an empty renders page was produced");
    }
}


/// ONE GRID IS QUOTED, AND IT IS THE STANDARD'S.
///
/// Asked for as: *"lets only show the en grid since its the standard showing 2 results will confuse
/// the user."* An earlier version printed both, with a note explaining the difference; two averages
/// and two maxima under one room heading read as a contradiction however carefully the note is
/// worded.
#[cfg(test)]
mod every_figure_comes_from_the_standards_grid {
    use super::tests::*;
    use super::*;

    /// A coarse grid over the same plane, DELIBERATELY different in every statistic — so a figure
    /// taken off the wrong grid is a different number rather than a rounding difference.
    fn en_grid() -> LuxGrid {
        let (cols, rows) = (4u32, 3u32);
        let vals: Vec<f64> = vec![
            180.0, 220.0, 240.0, 190.0, //
            210.0, 260.0, 255.0, 205.0, //
            175.0, 215.0, 235.0, 185.0,
        ];
        let min = vals.iter().cloned().fold(f64::MAX, f64::min);
        let max = vals.iter().cloned().fold(f64::MIN, f64::max);
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        LuxGrid { cols, rows, values: vals, min, max, avg, maintenance: 0.8, direct: Vec::new(), indirect: Vec::new() }
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn with_en<'a>(g: &'a LuxGrid, p: &'a CalcPlane, eg: &'a LuxGrid) -> Input<'a> {
        let mut i = input(g, p);
        i.rooms[0].grid_en = Some(eg);
        i.rooms[0].plane_en = Some(p);
        i
    }

    /// THE STANDARD'S FIGURES ARE THE ONES ON THE PAGE — and the working grid's are NOT.
    ///
    /// The second half is the point of the change. It is not enough that the EN numbers appear;
    /// the others have to be gone, or the reader is back to two answers.
    #[test]
    fn the_working_grids_figures_are_not_on_the_page() {
        let (g, p, eg) = (grid(16, 12), plane(), en_grid());
        let t = texts(&layout(&with_en(&g, &p, &eg), &Options::default()));
        let has = |s: &str| t.iter().any(|x| x.contains(s));

        assert!(
            (g.max - eg.max).abs() > 1.0 && (g.avg - eg.avg).abs() > 1.0 && (g.min - eg.min).abs() > 1.0,
            "the fixture does not discriminate — the two grids report the same statistics",
        );
        assert!(has(&format!("{:.0} lx", eg.max)), "the standard's maximum is missing");
        assert!(has(&format!("{:.0} lx", eg.avg)), "the standard's average is missing");
        assert!(has("4 x 3 points"), "the standard's grid shape is missing");
        assert!(has("Grid (EN 12464-1)"), "the page does not say which grid it is quoting");

        assert!(
            !has(&format!("{:.0} lx", g.max)),
            "the working grid's maximum ({:.0} lx) is still on the page beside the standard's",
            g.max,
        );
        assert!(
            !has("16 x 12 points"),
            "the working grid's shape is still on the page",
        );
    }

    /// THE PICTURE KEEPS THE FINE GRID. The standard sets how finely a room must be SAMPLED for its
    /// figures to mean anything; it says nothing about how coarsely the result may be drawn, and a
    /// plan redrawn at 4 x 3 would throw away every bit of detail for no gain.
    #[test]
    fn the_field_is_still_drawn_from_the_calculated_grid() {
        let (g, p, eg) = (grid(16, 12), plane(), en_grid());
        let i = with_en(&g, &p, &eg);
        assert_eq!(i.rooms[0].drawn().cols, 16, "the drawing dropped to the standard's grid");
        assert_eq!(i.rooms[0].reported().cols, 4, "the figures came off the fine grid");
    }

    /// A ROOM WITH NO EN GRID FALLS BACK TO THE ONE IT HAS. Older stored results and the
    /// whole-model fallback carry none, and must still report something.
    #[test]
    fn a_room_with_no_en_grid_reports_its_working_one() {
        let (g, p) = (grid(16, 12), plane());
        let i = input(&g, &p);
        assert_eq!(i.rooms[0].reported().cols, 16, "a room with no EN grid reported nothing");
        let t = texts(&layout(&i, &Options::default()));
        assert!(
            t.iter().any(|x| x.contains("16 x 12 points")),
            "the working grid is not on the page either",
        );
        // …and it does NOT claim to be the standard's.
        assert!(
            !t.iter().any(|x| x.contains("Grid (EN 12464-1)")),
            "a working grid was labelled as EN 12464-1's",
        );
    }

    /// THE SCALE IS TOPPED BY THE REPORTED MAXIMUM. An auto scale prints its ceiling in the
    /// legend's caption, so taking it off the drawn grid would put a second maximum on the page
    /// through the back door — which is exactly what this change is removing.
    #[test]
    fn an_auto_scale_is_capped_by_the_figure_the_page_quotes() {
        let (g, p, eg) = (grid(16, 12), plane(), en_grid());
        let mut o = Options::default();
        o.scale.bands.clear(); // continuous, so the caption names a number
        o.scale.top = None; // auto
        let t = texts(&layout(&with_en(&g, &p, &eg), &o));
        let caption = t.iter().find(|x| x.contains(" to ") && x.contains("auto"));
        let caption = caption.expect("the continuous scale's caption");
        assert!(
            caption.contains(&format!("{:.0}", eg.max)),
            "the caption reads {caption:?} — it should be capped at the reported {:.0} lx, not the \
             drawn grid's {:.0}",
            eg.max,
            g.max,
        );
    }

    /// THE GRID SPACING IS THE COARSER AXIS. A rectangular cell described by its finer side
    /// flatters the grid, and the coarse axis is the one that decides what a maximum misses.
    #[test]
    fn spacing_is_quoted_from_the_coarser_axis() {
        // 8 m over 4 columns is 2.00 m; 6 m over 12 rows is 0.50 m.
        let p = CalcPlane { origin: cad_light::Vertex::new(0.0, 0.0, 0.8), width: 8.0, depth: 6.0, cols: 4, rows: 12 };
        let g = grid(4, 12);
        assert!(
            (spacing_of(&p, &g) - 2.0).abs() < 1e-6,
            "spacing came out {:.3} m, expected the coarser 2.000",
            spacing_of(&p, &g),
        );
    }
}


/// AND THE CELLS THE STATISTICS EXCLUDE STAY EXCLUDED FURTHER DOWN THE PAGE.
///
/// `apply_room_mask` fixes a grid's `avg`, `min` and `max` and leaves its `values` alone — so the
/// two sections that walk the cells themselves have to be handed the mask, or they quote readings
/// the summary has already thrown out. This is the buried-cell defect — *"our min lux was 0 while
/// for relux it was 133"* — reappearing one section lower down.
#[cfg(test)]
mod the_mask_reaches_the_sections_that_walk_cells {
    use super::tests::*;
    use super::*;

    /// A grid with a 0 lx cell in it that the SUMMARY already excludes: `min` says 100, `values`
    /// still holds the zero. Exactly the shape `apply_room_mask` leaves behind.
    fn grid_with_a_buried_cell() -> (LuxGrid, Vec<bool>) {
        let (cols, rows) = (4u32, 3u32);
        let mut vals = vec![200.0_f64; 12];
        vals[5] = 0.0; // inside the cupboard
        vals[0] = 100.0; // the real minimum
        vals[11] = 400.0;
        let mut mask = vec![true; 12];
        mask[5] = false;
        // The statistics as the engine would leave them: over the KEPT cells only.
        let kept: Vec<f64> =
            vals.iter().zip(&mask).filter_map(|(v, k)| k.then_some(*v)).collect();
        let g = LuxGrid {
            cols,
            rows,
            values: vals,
            min: kept.iter().cloned().fold(f64::MAX, f64::min),
            max: kept.iter().cloned().fold(f64::MIN, f64::max),
            avg: kept.iter().sum::<f64>() / kept.len() as f64,
            maintenance: 0.8,
            direct: Vec::new(),
            indirect: Vec::new(),
        };
        (g, mask)
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// THE EXTREMES DO NOT NAME A CELL NOBODY COULD MEASURE.
    #[test]
    fn the_minimum_over_the_grid_skips_the_buried_cell() {
        let (eg, emask) = grid_with_a_buried_cell();
        let (g, p) = (grid(16, 12), plane());
        let mut i = input(&g, &p);
        i.rooms[0].grid_en = Some(&eg);
        i.rooms[0].plane_en = Some(&p);
        i.rooms[0].mask_en = &emask;
        let t = texts(&layout(&i, &Options::default()));

        assert!(
            t.iter().any(|s| s.starts_with("100 lx at (")),
            "the minimum should be the 100 lx cell; got {:?}",
            t.iter().filter(|s| s.contains(" lx at (")).collect::<Vec<_>>(),
        );
        assert!(
            !t.iter().any(|s| s.starts_with("0 lx at (")),
            "a 0 lx reading from inside the furniture is quoted as the room's minimum",
        );
    }

    /// AND NEITHER DO THE PERCENTILES. With one of twelve cells at 0 lx, an unmasked 10th
    /// percentile lands on it; masked, the lowest reading in the room is 100 lx.
    #[test]
    fn the_percentiles_are_taken_over_measurable_cells_only() {
        let (eg, emask) = grid_with_a_buried_cell();
        let (g, p) = (grid(16, 12), plane());
        let mut i = input(&g, &p);
        i.rooms[0].grid_en = Some(&eg);
        i.rooms[0].plane_en = Some(&p);
        i.rooms[0].mask_en = &emask;
        let t = texts(&layout(&i, &Options::default()));
        let pcts = t
            .iter()
            .zip(t.iter().skip(1))
            .find(|(a, _)| a.starts_with("10th / 90th"))
            .map(|(_, b)| b.clone())
            .expect("the percentile row");
        // THE EXACT VALUE, because "not zero" does not discriminate here and a mutation proved it.
        //
        // Twelve cells, one of them buried. Unmasked the sorted list is [0, 100, 200×9, 400] and
        // the 10th percentile lands on index 1 — which is 100, not the zero. So an assertion that
        // the row is merely "not 0" passes with the mask ignored entirely. Masked, the list is the
        // eleven measurable cells and the same percentile lands on 200. That difference is the
        // whole test.
        assert!(
            pcts.starts_with("200 /"),
            "the 10th / 90th percentile row reads {pcts:?}; over the measurable cells it is 200. \
             Reading 100 means the buried cell is still in the sample.",
        );
    }

    /// A ROOM WITH NO MASK LOSES NOTHING. An empty mask means every cell counts, and reading it as
    /// "nothing counts" would empty the room.
    #[test]
    fn an_empty_mask_keeps_every_cell() {
        let (g, p) = (grid(16, 12), plane());
        let i = input(&g, &p);
        assert!(i.rooms[0].reported_mask().is_empty());
        let t = texts(&layout(&i, &Options::default()));
        assert!(
            t.iter().any(|s| s.contains(" lx at (")),
            "a room with no mask reported no extremes at all",
        );
    }
}

/// AN OBJECT IS DRAWN AS AN OBJECT, NOT AS DARKNESS.
///
/// Reported as: *"in our report the light at the furniture is 0 while in dialux it shows theres
/// light on the surface."* Measured on that plan: the four blocks are 1.00 m tall and the working
/// plane is at 0.80 m, so the plane runs through them and those readings are taken inside a solid.
/// The summary already excluded them — that was the fix for a room minimum of 0 lx — but the
/// drawing tested only the room outline, so it painted them with their raw 0 lx in the lowest band.
/// The picture said "dark" where the numbers said "no reading", and dark is the more believable of
/// the two.
#[cfg(test)]
mod objects_are_not_painted_as_darkness {
    use super::tests::*;
    use super::*;

    /// A room with a block in the middle of it: the centre quarter of the grid is unmeasurable.
    fn room_with_a_block<'a>(
        g: &'a LuxGrid,
        p: &'a CalcPlane,
        poly: &'a [glam::Vec2],
        mask: &'a [bool],
    ) -> Input<'a> {
        let mut i = input(g, p);
        i.rooms[0].poly = poly;
        i.rooms[0].mask = mask;
        i
    }

    fn fixture() -> (LuxGrid, CalcPlane, Vec<glam::Vec2>, Vec<bool>) {
        let (cols, rows) = (8u32, 6u32);
        let g = grid(cols, rows);
        let p = plane();
        let poly = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        // Strike out a 2 x 2 patch of cells near the middle.
        let mut mask = vec![true; (cols * rows) as usize];
        for r in 2..4u32 {
            for c in 3..5u32 {
                mask[(r * cols + c) as usize] = false;
            }
        }
        (g, p, poly, mask)
    }

    fn fills(d: &Doc) -> Vec<[u8; 3]> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Poly { fill, .. } => Some(*fill),
                Item::Rect { fill, .. } => Some(*fill),
                _ => None,
            })
            .collect()
    }

    fn texts(d: &Doc) -> Vec<String> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// THE EXCLUDED CELLS ARE PAINTED IN THE OBJECT COLOUR — a neutral that is not in the legend,
    /// so it cannot be read as a lux band.
    #[test]
    fn a_cell_the_calculation_excluded_is_drawn_as_an_object() {
        let (g, p, poly, mask) = fixture();
        let d = layout(&room_with_a_block(&g, &p, &poly, &mask), &Options::default());
        let f = fills(&d);
        assert!(
            f.contains(&OBJECT_FILL),
            "nothing on the page is drawn in the object colour — the block is still showing as a \
             lux band",
        );
        // …and it is NOT one of the band colours, or it would read as a reading.
        let o = Options::default();
        assert!(
            !o.band_colours.contains(&OBJECT_FILL),
            "the object colour is one of the band colours — it will be read as a lux value",
        );
    }

    /// AND THE PAGE SAYS WHAT THE GREY IS. An unexplained colour outside the legend is a mystery,
    /// and the obvious reading of a pale patch on a lighting drawing is "unlit".
    #[test]
    fn the_page_explains_the_object_colour() {
        let (g, p, poly, mask) = fixture();
        let d = layout(&room_with_a_block(&g, &p, &poly, &mask), &Options::default());
        let t = texts(&d);
        assert!(
            t.iter().any(|s| s.contains("inside objects standing in the room")),
            "the drawing carries a colour the legend does not explain",
        );
        assert!(
            t.iter().any(|s| s.contains("4 grid point(s)")),
            "the note does not say how many points were excluded: {:?}",
            t.iter().filter(|s| s.contains("Grey")).collect::<Vec<_>>(),
        );
    }

    /// A ROOM WITH NOTHING IN IT GETS NO GREY AND NO NOTE. The note would be noise, and a stray
    /// neutral block on a clear plan would be a defect.
    #[test]
    fn a_room_with_nothing_in_it_is_drawn_as_before() {
        let (g, p, poly, _) = fixture();
        let all = vec![true; (g.cols * g.rows) as usize];
        let d = layout(&room_with_a_block(&g, &p, &poly, &all), &Options::default());
        assert!(
            !fills(&d).contains(&OBJECT_FILL),
            "an empty room was given an object block",
        );
        assert!(
            !texts(&d).iter().any(|s| s.contains("inside objects standing")),
            "an empty room was given the object note",
        );
    }
}

/// ONE NUMBER SETS EVERY TYPE SIZE IN THE REPORT.
///
/// Asked for as: *"for the report i want font size controls."* One control rather than six,
/// because the report's sizes are a HIERARCHY — cover over chapter over heading over row over note
/// — and that hierarchy is what a reader navigates by.
#[cfg(test)]
mod the_text_size_control {
    use super::tests::*;
    use super::*;

    /// A grid too fine for point values, so every piece of text on the page is the document's own.
    fn doc_at(scale: f64) -> Doc {
        let (g, p) = (grid(48, 36), plane());
        let mut o = Options::default();
        o.text_scale = scale;
        layout(&input(&g, &p), &o)
    }

    /// …and a coarse one, where the values ARE printed in their cells.
    pub(super) fn doc_with_point_values(scale: f64) -> Doc {
        let (g, p) = (grid(8, 6), plane());
        let mut o = Options::default();
        o.text_scale = scale;
        layout(&input(&g, &p), &o)
    }

    fn text_items(d: &Doc) -> Vec<(f64, f64, String)> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Text { size, y, text, .. } => Some((*size, *y, text.clone())),
                _ => None,
            })
            .collect()
    }

    /// EVERY SIZE MOVES, AND BY THE SAME FACTOR. A control that scaled most of the report and left
    /// one heading behind would be worse than none — the odd size out reads as a mistake.
    ///
    /// ON A GRID TOO FINE FOR POINT VALUES, deliberately. Those are the one exempt text in the
    /// report — their size is computed to FIT a grid cell, so scaling them would defeat the only
    /// rule they have — and this test is about the document's own type. `the_point_values_stay_the
    /// _size_that_fits_their_cell` is where the exemption itself is pinned. The first version of
    /// this test used a coarse grid, caught the exempt size, and failed for a reason that was not a
    /// defect: 6.44 pt is a cell measurement, not a typographic choice.
    #[test]
    fn every_size_in_the_document_scales_together() {
        let k = 1.4;
        let base = text_items(&doc_at(1.0));
        let big = text_items(&doc_at(k));
        assert!(base.len() > 20, "the fixture is too small to say anything: {} items", base.len());

        let distinct = |v: &[(f64, f64, String)]| {
            let mut s: Vec<i64> = v.iter().map(|(sz, _, _)| (sz * 1000.0).round() as i64).collect();
            s.sort_unstable();
            s.dedup();
            s
        };
        let a = distinct(&base);
        let b = distinct(&big);
        assert!(a.len() >= 4, "the report should use at least four sizes; found {a:?}");
        assert_eq!(
            a.len(),
            b.len(),
            "scaling changed how many DISTINCT sizes the report uses — {a:?} became {b:?}, so some \
             sizes moved and others did not",
        );
        for (x, y) in a.iter().zip(&b) {
            let want = (*x as f64 * k).round() as i64;
            assert!(
                (want - *y).abs() <= 2,
                "a size of {:.2} pt should become {:.2} at {k}x, but came out {:.2}",
                *x as f64 / 1000.0,
                want as f64 / 1000.0,
                *y as f64 / 1000.0,
            );
        }
    }

    /// AND THE SPACING MOVES WITH IT. The sizes and the vertical rhythm in this file were written
    /// as one set of numbers; grow the type and leave the rhythm and the lines collide.
    #[test]
    fn the_rows_get_further_apart_as_the_type_grows() {
        let gap = |scale: f64| -> f64 {
            let d = doc_at(scale);
            // EVERY page, not page zero — which is the cover and carries no table rows at all.
            // Consecutive rows are the tightest spacing in the report and so the first thing to
            // collide.
            let ys: Vec<f64> = d
                .pages
                .iter()
                .flat_map(|p| p.items.iter())
                .filter_map(|i| match i {
                    Item::Text { size, y, .. } if (*size - 9.0 * scale).abs() < 0.2 => Some(*y),
                    _ => None,
                })
                .collect();
            let mut steps: Vec<f64> = ys.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 1.0).collect();
            steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert!(!steps.is_empty(), "no row spacing to measure at {scale}x");
            steps[steps.len() / 2]
        };
        let (small, large) = (gap(1.0), gap(1.4));
        assert!(
            large > small * 1.25,
            "rows are {small:.2} pt apart at 100% and {large:.2} pt at 140% — the spacing did not \
             follow the type, so the lines will run into each other",
        );
    }

    /// THE DRAWINGS DO NOT SCALE. Their labels do — those are text — but the plans are at a stated
    /// ratio, and a scale note that no longer matches the drawing under it is worse than small type.
    #[test]
    fn the_drawings_keep_their_stated_scale() {
        let width_of_biggest_poly = |scale: f64| -> f64 {
            let d = doc_at(scale);
            d.pages
                .iter()
                .flat_map(|p| p.items.iter())
                .filter_map(|i| match i {
                    Item::Poly { rings, .. } => rings.first().map(|r| {
                        let xs: Vec<f64> = r.iter().map(|(x, _)| *x).collect();
                        xs.iter().cloned().fold(f64::MIN, f64::max)
                            - xs.iter().cloned().fold(f64::MAX, f64::min)
                    }),
                    _ => None,
                })
                .fold(0.0_f64, f64::max)
        };
        let (a, b) = (width_of_biggest_poly(1.0), width_of_biggest_poly(1.4));
        assert!(a > 1.0, "no drawing in the fixture to measure");
        assert!(
            (a - b).abs() < 0.01,
            "the plan is {a:.2} pt wide at 100% text and {b:.2} pt at 140% — the drawing moved with \
             the type, so its stated scale is now a lie",
        );
        // …and the note that states it says the same thing.
        let note_at = |scale: f64| -> Vec<String> {
            text_items(&doc_at(scale))
                .into_iter()
                .map(|(_, _, t)| t)
                .filter(|t| t.contains("1:"))
                .collect()
        };
        assert_eq!(note_at(1.0), note_at(1.4), "the stated drawing scale changed with the type size");
    }

    /// AN ABSURD SETTING IS CLAMPED rather than producing a document nobody can use. The dialog
    /// bounds it, but a settings file is a text file somebody can edit.
    #[test]
    fn a_setting_from_outside_the_dialog_is_still_bounded() {
        for absurd in [0.0, -3.0, 40.0, f64::INFINITY] {
            let sizes: Vec<f64> = text_items(&doc_at(absurd)).into_iter().map(|(s, _, _)| s).collect();
            assert!(!sizes.is_empty(), "a text_scale of {absurd} produced no text at all");
            for s in sizes {
                assert!(
                    s.is_finite() && s > 3.0 && s < 80.0,
                    "a text_scale of {absurd} produced a {s} pt size",
                );
            }
        }
    }

    /// AND THE DEFAULT CHANGES NOTHING. Every report written before this existed must come out
    /// exactly as it did.
    #[test]
    fn the_default_is_the_report_as_it_was() {
        assert_eq!(Options::default().text_scale, 1.0);
        let d = doc_at(1.0);
        let sizes: Vec<i64> =
            text_items(&d).into_iter().map(|(s, _, _)| (s * 100.0).round() as i64).collect();
        // The sizes the report is designed at, unrounded and unscaled.
        assert!(sizes.contains(&900), "the 9 pt table row is missing at 100%: {sizes:?}");
        assert!(sizes.contains(&1200), "the 12 pt heading is missing at 100%");
        assert!(sizes.contains(&800), "the 8 pt note is missing at 100%");
    }
}

/// THE ONE TEXT THE SIZE CONTROL MUST NOT TOUCH.
#[cfg(test)]
mod the_point_values_keep_fitting_their_cells {
    use super::the_text_size_control::*;
    use super::*;

    /// The sizes of the numbers printed inside the grid cells.
    ///
    /// SELECTED BY INK, NOT BY SIZE. The first version of this filter took integer text under
    /// 10 pt — and the legend's band labels are integers too, so at 150% they crossed the 10 pt
    /// line and dropped out of the sample. A filter whose criterion moves with the thing under
    /// test measures nothing. A point value is the only text drawn in near-black or near-white,
    /// chosen per cell for contrast against the band it sits on.
    fn in_cell_sizes(d: &Doc) -> Vec<f64> {
        d.pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|i| match i {
                Item::Text { size, rgb, .. }
                    if *rgb == [20, 20, 20] || *rgb == [245, 245, 245] =>
                {
                    Some(*size)
                }
                _ => None,
            })
            .collect()
    }

    /// A VALUE PRINTED IN A CELL STAYS THE SIZE THAT FITS THE CELL.
    ///
    /// Its size is `(cell · 0.26).clamp(4, 8)` — computed from the geometry it has to sit inside,
    /// not chosen typographically. Scaling it with the report's text control would make it overflow
    /// the cell and collide with its neighbours, which is the one thing it must not do.
    #[test]
    fn the_point_values_stay_the_size_that_fits_their_cell() {
        let small = in_cell_sizes(&doc_with_point_values(1.0));
        assert!(!small.is_empty(), "the coarse fixture printed no point values to check");
        let big = in_cell_sizes(&doc_with_point_values(1.5));
        assert_eq!(
            small.len(),
            big.len(),
            "the number of point values changed with the text size — {} against {}",
            small.len(),
            big.len(),
        );
        for (a, b) in small.iter().zip(&big) {
            assert!(
                (a - b).abs() < 1e-6,
                "a point value is {a:.2} pt at 100% and {b:.2} pt at 150% — it no longer fits its \
                 cell",
            );
        }
    }
}
