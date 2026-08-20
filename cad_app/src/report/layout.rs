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
            h.u64(mask.len() as u64);
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

    fn push(&mut self, it: Item) {
        self.page.items.push(it);
    }

    fn text(&mut self, x: f64, size: f64, font: Font, align: Align, rgb: [u8; 3], s: &str) {
        self.push(Item::Text { x, y: self.y, size, font, rgb, align, text: s.to_string() });
    }

    /// A section heading, with the rule under it.
    fn heading(&mut self, s: &str) {
        self.need(40.0);
        self.y += 14.0;
        self.text(self.left, 12.0, Font::Bold, Align::Left, INK, s);
        self.y += 5.0;
        let (l, r, y) = (self.left, self.right, self.y);
        self.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: RULE, width: 0.6 });
        self.y += 14.0;
    }

    /// A label on the left and a value on the right — the shape every figure in this report takes.
    fn row(&mut self, k: &str, v: &str) {
        self.need(16.0);
        self.y += 10.0;
        self.text(self.left, 9.0, Font::Regular, Align::Left, INK, k);
        self.text(self.right, 9.0, Font::Regular, Align::Right, INK, v);
        self.y += 4.0;
        let (l, r, y) = (self.left, self.right, self.y);
        self.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: [235, 235, 235], width: 0.4 });
    }

    fn note(&mut self, s: &str) {
        self.need(14.0);
        self.y += 11.0;
        self.text(self.left, 8.0, Font::Regular, Align::Left, FAINT, s);
        self.y += 2.0;
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
    // ONE IMAGE TABLE, TWO LISTS. The PDF holds a single table of images, so the renders go in
    // first and the logos after them — a logo's index into the table is `logo_base + i`. Kept
    // apart in the options because they are different things chosen from different buttons; kept
    // together here because the file format has one place to put them.
    for im in opt.images.iter().chain(opt.logos.iter()) {
        if let Some((bytes, iw, ih)) = &im.jpeg {
            doc.images.push(Jpeg { bytes: bytes.clone(), w: *iw, h: *ih });
        }
    }
    let logo_base = opt.images.iter().filter(|i| i.jpeg.is_some()).count();

    let has_head = !opt.header.trim().is_empty() || opt.header_image.is_some();
    let has_foot =
        !opt.footer.trim().is_empty() || opt.page_numbers || opt.footer_image.is_some();

    let mut cover_pages = Vec::new();
    if opt.cover {
        cover_pages.push(cover(inp, opt, w, h, &doc));
    }

    let mut c = Cursor::new(w, h, has_head, has_foot);
    // Chosen before anything is drawn, from every room at once — see `Cursor::scale`.
    c.scale = common_scale(inp, w, h);
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
        match s {
            Section::Materials => materials(&mut c, inp),
            Section::Surfaces => surfaces(&mut c, inp),
            Section::Renders => renders(&mut c, opt, &doc),
            _ => {}
        }
    }
    for room in &inp.rooms {
        if many || !room.name.trim().is_empty() {
            chapter(&mut c, if room.name.trim().is_empty() { "Results" } else { room.name.trim() });
        }
        for s in opt.sections.iter().filter(|s| is_per_room(**s)) {
            match s {
                Section::Summary => summary(&mut c, room, inp.unassigned),
                Section::Installation => installation(&mut c, room, inp.maintenance),
                Section::WorkingPlane => working_plane(&mut c, room, inp.eye_height),
                Section::Layout => layout_page(&mut c, room),
                Section::Results => results(&mut c, inp, room, opt),
                Section::NumericGrid => numeric_grid(&mut c, room),
                Section::Schedule => schedule(&mut c, &room.schedule, "Luminaire schedule"),
                _ => {}
            }
        }
    }
    // The whole scheme's totals, once, when there is more than one room to total.
    if many && opt.has(Section::Schedule) {
        chapter(&mut c, "Whole scheme");
        schedule(&mut c, &inp.total_schedule(), "Luminaire schedule — all rooms");
    }
    for s in opt
        .sections
        .iter()
        .enumerate()
        .filter(|(i, s)| *i > first_room_at && !is_per_room(**s))
        .map(|(_, s)| s)
    {
        match s {
            Section::Materials => materials(&mut c, inp),
            Section::Surfaces => surfaces(&mut c, inp),
            Section::Renders => renders(&mut c, opt, &doc),
            _ => {}
        }
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
                size: 8.0,
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
                    size: 8.0,
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
                    size: 8.0,
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
fn cover(inp: &Input, opt: &Options, w: f64, h: f64, doc: &Doc) -> Page {
    let mut p = Page::default();
    let mut y = h * 0.34;

    // The image sits ABOVE the title when there is one, so the eye lands on the picture and then
    // reads what it is — which is the order a cover is looked at.
    if let Some(i) = opt.cover_image {
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
        size: 26.0,
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
            size: 12.0,
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
        size: 8.0,
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
    c.need_or_break(60.0);
    if !c.page.items.is_empty() {
        c.brk();
    }
    c.y += 8.0;
    c.text(c.left, 17.0, Font::Bold, Align::Left, INK, name);
    c.y += 7.0;
    let (l, r, y) = (c.left, c.right, c.y);
    c.push(Item::Line { x1: l, y1: y, x2: r, y2: y, rgb: [120, 120, 120], width: 1.2 });
    c.y += 10.0;
}

fn summary(c: &mut Cursor, room: &RoomInput, unassigned: usize) {
    let g = room.grid;
    c.heading("Summary");
    c.row("Average E", &format!("{:.0} lx", g.avg));
    c.row("Minimum E", &format!("{:.0} lx", g.min));
    c.row("Maximum E", &format!("{:.0} lx", g.max));
    c.row(
        "Uniformity U0 = Emin/E",
        &if g.avg > 0.0 { format!("{:.2}", g.min / g.avg) } else { "—".into() },
    );
    c.row("Grid", &format!("{} x {} points", g.cols, g.rows));
    c.row(
        "Plane",
        &format!(
            "{:.2} x {:.2} m at {:.2} m",
            room.plane.width, room.plane.depth, room.plane.origin.z
        ),
    );
    if unassigned > 0 {
        c.note(&format!(
            "{unassigned} fixture(s) in this project have no photometric file assigned and \
             contribute nothing.",
        ));
    }
}

fn installation(c: &mut Cursor, room: &RoomInput, m: Maintenance) {
    let Some(i) = room.installation else { return };
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
    let g = room.grid;
    c.heading("Working plane");
    let mut v: Vec<f64> = g.values.clone();
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

/// How many raster samples across the plot, at most. 900 is well past what 300 dpi resolves on an
/// A4 content width, and past it the only thing that grows is the file.
const MAX_SAMPLES: usize = 900;

fn results_body(
    c: &mut Cursor,
    inp: &Input,
    room: &RoomInput,
    opt: &Options,
    plot_w: f64,
    plot_h: f64,
) {
    let g = room.grid;
    let (gc, gr) = (g.cols as usize, g.rows as usize);
    let x0 = c.left + (c.width() - plot_w) * 0.5;
    let y0 = c.y;
    let room_max = g.max;

    // Enough samples to hide the grid, never more than the page can show.
    let nx = (gc * 8).clamp(gc, MAX_SAMPLES);
    let ny = ((nx as f64) * (plot_h / plot_w)).round().max(1.0) as usize;
    let sw = plot_w / nx as f64;
    let sh = plot_h / ny as f64;

    for j in 0..ny {
        // Sample at the CENTRE of each raster cell, in grid coordinates.
        let gy = ((j as f64 + 0.5) / ny as f64) * gr as f64 - 0.5;
        let mut run: Option<([u8; 3], usize)> = None;
        for i in 0..=nx {
            let colour = if i == nx {
                None
            } else {
                let gx = ((i as f64 + 0.5) / nx as f64) * gc as f64 - 0.5;
                sample(inp, room, opt, gx, gy, room_max)
            };
            match (&mut run, colour) {
                (Some((c0, start)), Some(cc)) if *c0 == cc => {}
                (Some((c0, start)), _) => {
                    // Half a sample of overlap, so neighbouring runs meet rather than leaving a
                    // hairline of white paper between them at the reader's rendering resolution.
                    let x = x0 + *start as f64 * sw;
                    let w = (i - *start) as f64 * sw + 0.15;
                    c.push(Item::Rect { x, y: y0 + j as f64 * sh, w, h: sh + 0.15, fill: *c0 });
                    run = colour.map(|cc| (cc, i));
                }
                (None, Some(cc)) => run = Some((cc, i)),
                (None, None) => {}
            }
        }
    }

    // THE VALUES, where the grid is coarse enough to carry them. A smooth field shows the shape of
    // the light; the numbers make it checkable, and on a small room there is room for both.
    let cell_w = plot_w / gc as f64;
    let cell_h = plot_h / gr as f64;
    if cell_w.min(cell_h) >= 22.0 {
        for j in 0..gr {
            for i in 0..gc {
                let Some(v) = g.values.get(j * gc + i) else { continue };
                let Some(fill) = sample(inp, room, opt, i as f64, j as f64, room_max) else {
                    continue;
                };
                // Black on light, white on dark — a fixed ink colour is unreadable over half the
                // ramp, and these exist to be read.
                let lum = 0.299 * fill[0] as f64 + 0.587 * fill[1] as f64 + 0.114 * fill[2] as f64;
                c.push(Item::Text {
                    x: x0 + (i as f64 + 0.5) * cell_w,
                    y: y0 + (j as f64 + 0.5) * cell_h + 2.0,
                    size: (cell_w.min(cell_h) * 0.26).clamp(4.0, 8.0),
                    font: Font::Regular,
                    rgb: if lum > 140.0 { [20, 20, 20] } else { [245, 245, 245] },
                    align: Align::Centre,
                    text: format!("{v:.0}"),
                });
            }
        }
    }
    let values_shown = cell_w.min(cell_h) >= 22.0;

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

    Some(ramp_rgb(inp.ramp, opt.scale.t_for(v, room_max)))
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

fn ramp_rgb(ramp: fn(f32) -> (f32, f32, f32), t: f32) -> [u8; 3] {
    let (r, g, b) = ramp(t.clamp(0.0, 1.0));
    [
        (r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (b.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

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
            c.push(Item::Rect { x: x + bw * i as f64, y, w: bw, h, fill: ramp_rgb(inp.ramp, t) });
        }
        c.push(Item::Frame { x, y, w, h, rgb: [150, 150, 150], width: 0.5 });
        c.y = y + h + 10.0;
        for (i, e) in edges.iter().enumerate() {
            c.push(Item::Text {
                x: x + bw * i as f64,
                y: c.y,
                size: 7.0,
                font: Font::Regular,
                rgb: FAINT,
                align: if i == 0 { Align::Left } else { Align::Centre },
                text: format!("{e:.0}"),
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
    let g = room.grid;
    let (gc, gr) = (g.cols as usize, g.rows as usize);
    let p = room.plane;
    let inside = |i: usize| room.mask.get(i).copied().unwrap_or(true);
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

    fn grid(cols: u32, rows: u32) -> LuxGrid {
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

    fn plane() -> CalcPlane {
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
            mask: &[],
            poly: &[],
            fixtures: &[],
            installation: None,
            cylindrical_avg: None,
            schedule: Vec::new(),
        }
    }

    fn input<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> Input<'a> {
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

    /// THE FIELD IS DRAWN SMOOTH, not as one rectangle per measured point.
    ///
    /// "see how the false color shows up as grid make it smooth like in the reference." Drawn at
    /// grid resolution, a 33-column room came out as visible cross-hatching — the sampling grid
    /// rather than the light. It is resampled onto a much finer raster, and runs of one colour are
    /// merged, so a banded scale gives smooth curved band edges.
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

        // Every run is at most one raster row tall, which is far thinner than a grid cell.
        let cell_h = ph / 8.0;
        let tallest = r.iter().map(|x| x.3).fold(0.0_f64, f64::max);
        assert!(
            tallest < cell_h * 0.5,
            "the tallest run is {tallest:.2} pt against a {cell_h:.2} pt grid cell — this is still \
             being drawn one rectangle per measured point",
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

        // Where each run begins, to the tenth of a point.
        let starts: std::collections::BTreeSet<i64> =
            field_rects(&d).iter().map(|r| (r.0 * 10.0).round() as i64).collect();
        assert!(
            starts.len() > cols as usize * 3,
            "runs begin at only {} distinct positions across the whole field — with one per grid \
             column that is a nearest-neighbour resample, not an interpolated one",
            starts.len(),
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

    /// GROUND OUTSIDE THE ROOM IS NOT COLOURED — colouring it reports illuminance where the room
    /// is not.
    #[test]
    fn cells_outside_the_room_are_not_coloured() {
        let g = grid(2, 1);
        let p = plane();
        let mut i = input(&g, &p);
        i.rooms[0].mask = &[true, false];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::Results];
        let d = layout(&i, &o);
        let (px, _, pw, _) = plot_frames(&d)[0];
        let r = field_rects(&d);
        assert!(!r.is_empty());
        // Everything drawn sits in the LEFT half — the right cell is outside the room.
        let rightmost = r.iter().map(|x| x.0 + x.2).fold(f64::MIN, f64::max);
        assert!(
            rightmost < px + pw * 0.6,
            "the field reaches {:.1}, past the half-way mark at {:.1}",
            rightmost,
            px + pw * 0.5,
        );
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
        let auto: Vec<[u8; 3]> =
            field_rects(&layout(&input(&g, &p), &o)).iter().map(|x| x.4).collect();

        o.scale = crate::report::options::Scale { top: Some(5000.0), bands: Vec::new() };
        let pinned: Vec<[u8; 3]> =
            field_rects(&layout(&input(&g, &p), &o)).iter().map(|x| x.4).collect();

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

        let t = texts(&layout(&i, &o));
        assert!(t.iter().any(|s| s == "10"), "the combined quantity is not on the page: {t:?}");
        assert!(
            t.iter().any(|s| s.contains("all rooms")),
            "the combined schedule is not labelled as such",
        );
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
