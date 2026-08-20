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
#[derive(Clone, Debug, Default)]
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

/// Lay the whole report out.
pub fn layout(inp: &Input, opt: &Options) -> Doc {
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
                Section::FalseColour => false_colour(&mut c, inp, room, opt),
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
            | Section::FalseColour
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

/// THE FALSE-COLOUR FIELD, GIVEN THE PAGE.
///
/// Two complaints shaped this. The plot came out as a stamp in the middle of a page — "the layout
/// is too small it should occupy 2/3 of the pages real estate while maintaining proportions" — and
/// the scale was whatever the viewport happened to be set to.
///
/// So the plot is sized to fill TWO THIRDS OF THE PAGE, keeping the plane's proportions, and it
/// starts a fresh page when what is left of this one cannot hold that. The room is the subject of
/// the page it is on, which is what the reference drawing does.
fn false_colour(c: &mut Cursor, inp: &Input, room: &RoomInput, opt: &Options) {
    let g = room.grid;
    if g.cols == 0 || g.rows == 0 {
        return;
    }
    // The target: two thirds of the page in each direction, which is two thirds of its area at the
    // plane's own aspect and reads as a figure rather than a thumbnail.
    let target_w = c.page_w * TWO_THIRDS;
    let target_h = c.page_h * TWO_THIRDS;
    let cell = (target_w / g.cols as f64).min(target_h / g.rows as f64);
    let plot_w = cell * g.cols as f64;
    let plot_h = cell * g.rows as f64;

    // Heading, plot, legend. If they will not fit below what is already on the page, the plot gets
    // a page of its own rather than being shrunk to whatever is left.
    let needed = 40.0 + plot_h + LEGEND_H;
    c.need_or_break(needed);
    c.heading("Illuminance — false colour");
    false_colour_body(c, inp, room, opt, cell, plot_w, plot_h);
}

/// The box a header or footer logo is fitted into, in points — 120 x 24 pt, about 42 x 8.5 mm.
///
/// It is a BOX, not a size: the image keeps its own proportions inside it, so a tall logo comes out
/// 24 pt high and narrow rather than squashed. The height is what the band can hold without the
/// running text moving; the width is what is left beside a project name on A4.
pub const LOGO_W: f64 = 120.0;
pub const LOGO_H: f64 = 24.0;

/// Two thirds, as asked for.
const TWO_THIRDS: f64 = 0.666;
/// Height of the banded legend and its labels.
const LEGEND_H: f64 = 46.0;

fn false_colour_body(
    c: &mut Cursor,
    inp: &Input,
    room: &RoomInput,
    opt: &Options,
    cell: f64,
    plot_w: f64,
    plot_h: f64,
) {
    let g = room.grid;
    let x0 = c.left + (c.width() - plot_w) * 0.5;
    let y0 = c.y;
    let room_max = g.max;
    let label = cell >= 22.0;

    for r in 0..g.rows as usize {
        for col in 0..g.cols as usize {
            let i = r * g.cols as usize + col;
            let Some(v) = g.values.get(i) else { continue };
            let x = x0 + col as f64 * cell;
            let y = y0 + r as f64 * cell;
            if room.mask.get(i).is_some_and(|inside| !inside) {
                // Outside the room: blank, not coloured. Colouring it would report illuminance on
                // ground the room does not occupy.
                continue;
            }
            let t = opt.scale.t_for(*v, room_max);
            let fill = ramp_rgb(inp.ramp, t);
            c.push(Item::Rect { x, y, w: cell, h: cell, fill });
            if label {
                // Black on light, white on dark — a fixed ink colour is unreadable over half the
                // ramp, and this plot exists to be read.
                let lum = 0.299 * fill[0] as f64 + 0.587 * fill[1] as f64 + 0.114 * fill[2] as f64;
                let ink = if lum > 140.0 { [20, 20, 20] } else { [245, 245, 245] };
                c.push(Item::Text {
                    x: x + cell * 0.5,
                    y: y + cell * 0.5 + 2.0,
                    size: (cell * 0.26).clamp(4.0, 8.0),
                    font: Font::Regular,
                    rgb: ink,
                    align: Align::Centre,
                    text: format!("{v:.0}"),
                });
            }
        }
    }
    c.push(Item::Frame { x: x0, y: y0, w: plot_w, h: plot_h, rgb: [140, 140, 140], width: 0.7 });

    // The dimensions of the plane, along the bottom — the reference drawing carries an axis, and
    // without one a picture of a room states no size.
    c.push(Item::Text {
        x: x0 + plot_w * 0.5,
        y: y0 + plot_h + 11.0,
        size: 7.5,
        font: Font::Regular,
        rgb: FAINT,
        align: Align::Centre,
        text: format!("{:.2} × {:.2} m", room.plane.width, room.plane.depth),
    });
    c.y = y0 + plot_h + 24.0;

    legend(c, inp, opt, room_max);
    if !label {
        c.note("Cell values are omitted at this grid size — the exact figures are in the grid table.");
    }
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

/// The numbers, on ONE page.
///
/// "the illuminance grid comes in multiple pages. it should all be shown in a single page." A
/// 70-column field was coming out as 23 pages of column blocks, which is not a table anybody reads.
///
/// So the type is sized to the grid: whatever makes all of it fit the page, down to a floor. Below
/// that floor it would be a grey texture rather than numbers, so the page says so and gives the
/// figures that can still be read — the false-colour plot and the HTML report both carry the field.
fn numeric_grid(c: &mut Cursor, room: &RoomInput) {
    let g = room.grid;
    if g.cols == 0 || g.rows == 0 {
        return;
    }
    let cols = g.cols as f64;
    let rows = g.rows as f64;
    let avail_w = c.width();
    // MEASURED AGAINST A WHOLE PAGE, not against what is left of this one. The grid gets a fresh
    // page if it needs one, so sizing it to the tail end of the previous page would reject a table
    // that fits perfectly well.
    let full_h = c.bottom - c.top - 30.0;

    // The widest figure decides the column, and four digits is the practical worst case.
    let size = (avail_w / (cols * 4.6)).min(full_h / (rows * 1.45));

    // DECIDED BEFORE ANYTHING IS PUT DOWN.
    //
    // This used to print the heading first and then discover the grid was unprintable, so an
    // apology got a page of its own with 700 points of white under it. Reported as "the
    // illumination grid is show empty page". A section that has nothing to say says it in one
    // line, where the reader already is.
    if size < MIN_GRID_PT {
        c.note(&format!(
            "Illuminance grid: {} × {} = {} values. At one page that is {:.1} pt type, below the \
             {MIN_GRID_PT:.0} pt this report will set — the false-colour plot above carries the \
             same field, and the HTML report prints every value at a size a screen can zoom.",
            g.cols,
            g.rows,
            g.values.len(),
            size,
        ));
        return;
    }

    // It fits — but it needs the room. Take a fresh page rather than start it in a corner.
    c.need_or_break(30.0 + rows * size * 1.45);
    c.heading("Illuminance grid (lx)");

    let colw = avail_w / cols;
    let rowh = size * 1.45;
    let y0 = c.y;
    for r in 0..g.rows as usize {
        for col in 0..g.cols as usize {
            let i = r * g.cols as usize + col;
            let Some(v) = g.values.get(i) else { continue };
            let inside = room.mask.get(i).copied().unwrap_or(true);
            c.push(Item::Text {
                x: c.left + (col as f64 + 1.0) * colw - colw * 0.12,
                y: y0 + (r as f64 + 1.0) * rowh,
                size,
                font: Font::Regular,
                rgb: if inside { INK } else { [190, 190, 190] },
                align: Align::Right,
                text: if inside { format!("{v:.0}") } else { "-".into() },
            });
        }
    }
    c.y = y0 + rows * rowh + 6.0;
}

/// Below this the figures stop being numbers and become a texture.
const MIN_GRID_PT: f64 = 3.2;

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

    /// The plot's cells, told apart from the legend by their size.
    fn plot_cells(d: &Doc, cols: u32, rows: u32) -> Vec<(f64, f64, f64, f64)> {
        let mut r = rects(d);
        // The plot is square cells; the legend is 12 pt tall blocks. Keeping only the squares is
        // enough, and the count is asserted against the grid so a mistake here cannot pass.
        r.retain(|(_, _, w, h)| (w - h).abs() < 1e-6);
        let _ = (cols, rows);
        r
    }

    /// THE PLOT TAKES TWO THIRDS OF THE PAGE.
    ///
    /// "the layout is too small it should occupy 2/3 of the pages real estate while maintaining
    /// proportions". It used to be sized to whatever was left below the heading, which on a busy
    /// page was a stamp in the middle of white space.
    #[test]
    fn the_plot_fills_two_thirds_of_the_page() {
        let (pw, ph) = PageSize::A4.points();
        for (cols, rows) in [(4u32, 4u32), (33, 40), (80, 3), (3, 80)] {
            let g = grid(cols, rows);
            let p = plane();
            let mut o = opts();
            o.cover = false;
            o.sections = vec![Section::FalseColour];
            let d = layout(&input(&g, &p), &o);
            let cells = plot_cells(&d, cols, rows);
            assert_eq!(cells.len(), (cols * rows) as usize, "{cols}x{rows}: a cell per point");

            let minx = cells.iter().map(|c| c.0).fold(f64::MAX, f64::min);
            let maxx = cells.iter().map(|c| c.0 + c.2).fold(f64::MIN, f64::max);
            let miny = cells.iter().map(|c| c.1).fold(f64::MAX, f64::min);
            let maxy = cells.iter().map(|c| c.1 + c.3).fold(f64::MIN, f64::max);
            let (w, h) = (maxx - minx, maxy - miny);

            // ONE of the two directions must reach two thirds — whichever the plane's aspect makes
            // the binding one. Both would only happen for a square plot on square paper.
            let fills_w = w >= pw * 0.66 - 1.0;
            let fills_h = h >= ph * 0.66 - 1.0;
            assert!(
                fills_w || fills_h,
                "{cols}x{rows}: plot is {w:.0}x{h:.0} on {pw:.0}x{ph:.0} — neither side reaches \
                 two thirds",
            );
            assert!(w <= pw * TWO_THIRDS + 1.0 && h <= ph * TWO_THIRDS + 1.0, "{cols}x{rows}: too big");
            // Square cells, or the field is stretched and the picture lies about the room.
            for (_, _, cw, ch) in &cells {
                assert!((cw - ch).abs() < 1e-6, "{cols}x{rows}: cell {cw:.2}x{ch:.2} is not square");
            }
        }
    }

    /// …AND IT STILL LANDS ON THE PAPER. Two thirds of the page is only useful if it is the right
    /// two thirds.
    #[test]
    fn the_plot_stays_within_the_margins() {
        let (pw, _) = PageSize::A4.points();
        let g = grid(33, 40);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::FalseColour];
        let d = layout(&input(&g, &p), &o);
        let cells = plot_cells(&d, 33, 40);
        let minx = cells.iter().map(|c| c.0).fold(f64::MAX, f64::min);
        let maxx = cells.iter().map(|c| c.0 + c.2).fold(f64::MIN, f64::max);
        assert!(minx >= 0.0 && maxx <= pw, "plot spans {minx:.1}..{maxx:.1} on a {pw:.0} page");
    }

    /// A MASKED CELL IS BLANK, not coloured — colouring it reports illuminance on ground the room
    /// does not occupy.
    #[test]
    fn cells_outside_the_room_are_not_coloured() {
        let g = grid(2, 1);
        let p = plane();
        let mut i = input(&g, &p);
        i.rooms[0].mask = &[true, false];
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::FalseColour];
        let d = layout(&i, &o);
        assert_eq!(plot_cells(&d, 2, 1).len(), 1, "only the inside cell is filled");
    }

    /// VALUES APPEAR WHEN THERE IS ROOM AND NOT WHEN THERE IS NOT — and when they are dropped the
    /// page says so, rather than leaving a reader to wonder whether the plot is the whole story.
    #[test]
    fn cell_values_are_printed_only_when_legible() {
        let mut o = opts();
        o.sections = vec![Section::FalseColour];
        o.cover = false;
        // A CONTINUOUS SCALE, so the only "100" on the page can be a cell value. With the default
        // bands the legend writes "100" under itself, and this test would pass on that instead —
        // which is a test that cannot fail for the reason it names.
        o.scale = crate::report::options::Scale { top: None, bands: Vec::new() };

        let small = grid(4, 4);
        let p = plane();
        let d = layout(&input(&small, &p), &o);
        let t = texts(&d);
        assert!(t.iter().any(|s| s == "100"), "a 4x4 plot has room for its values: {t:?}");

        let big = grid(60, 60);
        let d = layout(&input(&big, &p), &o);
        let t = texts(&d);
        assert!(
            !t.iter().any(|s| s == "100"),
            "a 60x60 plot printed values that cannot be read at that size",
        );
        assert!(
            t.iter().any(|s| s.contains("omitted")),
            "the page must say the values were left off: {t:?}",
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

    /// A GRID TOO BIG TO READ SAYS SO rather than printing a grey texture or spilling over.
    #[test]
    fn an_unprintable_grid_explains_itself() {
        let g = grid(200, 200);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.page_numbers = false;
        o.sections = vec![Section::NumericGrid];
        let d = layout(&input(&g, &p), &o);
        assert_eq!(d.pages.len(), 1, "it must not paginate its way out of this");
        let t = texts(&d);
        assert!(
            t.iter().any(|s| s.contains("pt") && s.contains("200")),
            "the page must state the size and why it was left out: {t:?}",
        );
        // …and nothing was printed as a texture.
        assert!(!t.iter().any(|s| s.parse::<f64>().is_ok()), "values were printed anyway");
    }

    /// THE SCALE IS THE REPORT'S DECISION, not the viewport's.
    ///
    /// Pinning the top changes the colours, and a report that ignored the setting would file a
    /// picture drawn at a scale nobody chose.
    #[test]
    fn pinning_the_scale_changes_the_colours() {
        let g = grid(4, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::FalseColour];
        o.scale = crate::report::options::Scale { top: None, bands: Vec::new() };
        let auto = plot_cells(&layout(&input(&g, &p), &o), 4, 4);

        o.scale = crate::report::options::Scale { top: Some(5000.0), bands: Vec::new() };
        let d = layout(&input(&g, &p), &o);
        let pinned = plot_cells(&d, 4, 4);
        assert_eq!(auto.len(), pinned.len());

        let fills = |d: &Doc| -> Vec<[u8; 3]> {
            d.pages
                .iter()
                .flat_map(|p| p.items.iter())
                .filter_map(|i| match i {
                    Item::Rect { w, h, fill, .. } if (w - h).abs() < 1e-6 => Some(*fill),
                    _ => None,
                })
                .collect()
        };
        let a = fills(&layout(
            &input(&g, &p),
            &Options {
                scale: crate::report::options::Scale { top: None, bands: Vec::new() },
                ..o.clone()
            },
        ));
        let b = fills(&d);
        assert_ne!(a, b, "pinning the top to 5000 lx left every colour unchanged");
        assert!(
            t_sum(&b) < t_sum(&a),
            "a higher ceiling must push the same room DOWN the ramp, not up",
        );
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
        o.sections = vec![Section::FalseColour];
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



    /// AN UNPRINTABLE GRID DOES NOT COST A PAGE.
    ///
    /// Reported as "the illumination grid is show empty page": the heading went down first, the
    /// grid then turned out to be unprintable, and the apology got a page of its own with 700
    /// points of white under it. The decision has to come before anything is put on the page.
    #[test]
    fn an_unprintable_grid_costs_no_page_and_no_heading() {
        let g = grid(125, 38); // the owner's Room 1 — 4,750 values
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.page_numbers = false;
        o.sections = vec![Section::Summary, Section::NumericGrid];
        let d = layout(&input(&g, &p), &o);

        assert_eq!(d.pages.len(), 1, "the note took {} pages", d.pages.len());
        let t = texts(&d);
        assert!(
            !t.iter().any(|s| s == "Illuminance grid (lx)"),
            "a heading was printed over a section with nothing under it: {t:?}",
        );
        assert!(
            t.iter().any(|s| s.contains("125") && s.contains("pt")),
            "the note must still say what was left out and why: {t:?}",
        );
    }

    /// A GRID THAT FITS IS STILL PRINTED, and still gets its heading. The fix must not have turned
    /// the section off.
    #[test]
    fn a_grid_that_fits_is_printed_with_its_heading() {
        let g = grid(10, 10);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::NumericGrid];
        let t = texts(&layout(&input(&g, &p), &o));
        assert!(t.iter().any(|s| s == "Illuminance grid (lx)"), "the heading went missing");
        let numbers = t.iter().filter(|s| s.parse::<f64>().is_ok()).count();
        assert!(numbers >= 100, "only {numbers} of 100 values were printed");
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
        o.sections = vec![Section::FalseColour, Section::NumericGrid];
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
        o.sections = vec![Section::Summary, Section::FalseColour];
        let mut i = input(&g, &p);
        i.rooms = vec![one_room(&g, &p, "Office"), one_room(&g, &p, "Store")];
        let d = layout(&i, &o);
        let t = texts(&d);

        for name in ["Office", "Store"] {
            assert!(t.iter().any(|s| s == name), "no chapter for {name}: {t:?}");
        }
        // Two plots, not one — a cell per point, twice.
        let cells = plot_cells(&d, 6, 6);
        assert_eq!(cells.len(), 6 * 6 * 2, "expected a plot per room, got {} cells", cells.len());
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
