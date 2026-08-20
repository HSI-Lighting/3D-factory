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

/// Everything the layout needs, gathered so it touches no UI types.
pub struct Input<'a> {
    pub grid: &'a LuxGrid,
    pub plane: &'a CalcPlane,
    pub maintenance: Maintenance,
    pub installation: Option<&'a Installation>,
    pub surfaces: &'a [SurfaceResult],
    pub cylindrical_avg: Option<f64>,
    pub eye_height: f32,
    pub room_height: f32,
    pub materials: Vec<(String, f32)>,
    pub unassigned: usize,
    pub ramp: fn(f32) -> (f32, f32, f32),
    pub scale_top: f64,
    pub scale_auto: bool,
    pub mask: Vec<bool>,
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
    for im in &opt.images {
        if let Some((bytes, iw, ih)) = &im.jpeg {
            doc.images.push(Jpeg { bytes: bytes.clone(), w: *iw, h: *ih });
        }
    }

    let has_head = !opt.header.trim().is_empty();
    let has_foot = !opt.footer.trim().is_empty() || opt.page_numbers;

    let mut cover_pages = Vec::new();
    if opt.cover {
        cover_pages.push(cover(inp, opt, w, h, &doc));
    }

    let mut c = Cursor::new(w, h, has_head, has_foot);
    for s in &opt.sections {
        match s {
            Section::Summary => summary(&mut c, inp),
            Section::Installation => installation(&mut c, inp),
            Section::Materials => materials(&mut c, inp),
            Section::WorkingPlane => working_plane(&mut c, inp),
            Section::FalseColour => false_colour(&mut c, inp),
            Section::NumericGrid => numeric_grid(&mut c, inp),
            Section::Surfaces => surfaces(&mut c, inp),
            Section::Renders => renders(&mut c, opt, &doc),
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
            inp.grid.maintenance
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

fn summary(c: &mut Cursor, inp: &Input) {
    let g = inp.grid;
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
        &format!("{:.2} x {:.2} m at {:.2} m", inp.plane.width, inp.plane.depth, inp.plane.origin.z),
    );
    if inp.unassigned > 0 {
        c.note(&format!(
            "{} fixture(s) have no photometric file assigned and contribute nothing.",
            inp.unassigned
        ));
    }
}

fn installation(c: &mut Cursor, inp: &Input) {
    let Some(i) = inp.installation else { return };
    c.heading("Installation");
    c.row("Luminaires", &format!("{}", i.count));
    c.row("Total load", &format!("{:.1} W", i.total_watts));
    c.row("Installed flux", &format!("{:.0} lm", i.total_lumens));
    c.row("Area", &format!("{:.2} m2", i.area_m2));
    c.row("Power density", &format!("{:.2} W/m2", i.power_density));
    c.row("Efficacy", &format!("{:.0} lm/W", i.efficacy));
    let m = inp.maintenance;
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

fn working_plane(c: &mut Cursor, inp: &Input) {
    let g = inp.grid;
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
    if let Some(cy) = inp.cylindrical_avg {
        c.row(&format!("Cylindrical E at {:.1} m", inp.eye_height), &format!("{cy:.0} lx"));
    }
}

/// THE FALSE-COLOUR FIELD, SCALED TO THE PAGE.
///
/// This is the section the complaint was about. The cell size is whatever makes the whole plot fit
/// the content box in BOTH directions, so a 33 x 40 grid and a 4 x 4 one both arrive whole. Values
/// are printed in the cells only when a cell is wide enough to hold one at a legible size — a plot
/// of unreadable smudges is worse than a plot with no numbers on it, and the numeric grid carries
/// the exact figures anyway.
fn false_colour(c: &mut Cursor, inp: &Input) {
    let g = inp.grid;
    if g.cols == 0 || g.rows == 0 {
        return;
    }
    c.heading("Illuminance — false colour");

    let avail_w = c.width();
    // Leave room for the legend under the plot, and never take more than one page.
    let avail_h = (c.bottom - c.y - 46.0).max(60.0);
    let cell = (avail_w / g.cols as f64).min(avail_h / g.rows as f64);
    let plot_w = cell * g.cols as f64;
    let plot_h = cell * g.rows as f64;

    // The plot is one indivisible object: if what is left on this page is too little to read it at
    // even a third of the width, start a fresh page rather than shrink it into a stamp.
    if cell < (avail_w / g.cols as f64) / 3.0 {
        c.brk();
        return false_colour_at(c, inp);
    }
    false_colour_body(c, inp, cell, plot_w, plot_h);
}

fn false_colour_at(c: &mut Cursor, inp: &Input) {
    let g = inp.grid;
    let avail_w = c.width();
    let avail_h = (c.bottom - c.y - 46.0).max(60.0);
    let cell = (avail_w / g.cols as f64).min(avail_h / g.rows as f64);
    false_colour_body(c, inp, cell, cell * g.cols as f64, cell * g.rows as f64);
}

fn false_colour_body(c: &mut Cursor, inp: &Input, cell: f64, plot_w: f64, plot_h: f64) {
    let g = inp.grid;
    let x0 = c.left + (c.width() - plot_w) * 0.5;
    let y0 = c.y;
    let top = inp.scale_top.max(1e-6);
    // A value needs about 4.2 points per digit at 5 pt, plus a point of air each side.
    let label = cell >= 22.0;

    for r in 0..g.rows as usize {
        for col in 0..g.cols as usize {
            let i = r * g.cols as usize + col;
            let Some(v) = g.values.get(i) else { continue };
            let x = x0 + col as f64 * cell;
            let y = y0 + r as f64 * cell;
            if inp.mask.get(i).is_some_and(|inside| !inside) {
                // Outside the room: blank, not coloured. Colouring it would report illuminance on
                // ground the room does not occupy.
                c.push(Item::Frame { x, y, w: cell, h: cell, rgb: [238, 238, 238], width: 0.3 });
                continue;
            }
            let t = (v / top).clamp(0.0, 1.0) as f32;
            let (rr, gg, bb) = (inp.ramp)(t);
            let fill = [
                (rr.clamp(0.0, 1.0) * 255.0).round() as u8,
                (gg.clamp(0.0, 1.0) * 255.0).round() as u8,
                (bb.clamp(0.0, 1.0) * 255.0).round() as u8,
            ];
            c.push(Item::Rect { x, y, w: cell, h: cell, fill });
            if label {
                // Black on light, white on dark — a fixed ink colour is unreadable over half the
                // ramp, and this plot exists to be read.
                let lum = 0.299 * rr + 0.587 * gg + 0.114 * bb;
                let ink = if lum > 0.55 { [20, 20, 20] } else { [245, 245, 245] };
                c.push(Item::Text {
                    x: x + cell * 0.5,
                    y: y + cell * 0.5 + 2.0,
                    size: 5.5,
                    font: Font::Regular,
                    rgb: ink,
                    align: Align::Centre,
                    text: format!("{v:.0}"),
                });
            }
        }
    }
    c.push(Item::Frame { x: x0, y: y0, w: plot_w, h: plot_h, rgb: [170, 170, 170], width: 0.6 });
    c.y = y0 + plot_h + 14.0;

    // The legend, in the same ramp — without it the colours mean nothing.
    let lw = (c.width() * 0.6).min(240.0);
    let lx = c.left;
    let n = 48;
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let (rr, gg, bb) = (inp.ramp)(t);
        c.push(Item::Rect {
            x: lx + lw * i as f64 / n as f64,
            y: c.y,
            w: lw / n as f64 + 0.4,
            h: 7.0,
            fill: [
                (rr.clamp(0.0, 1.0) * 255.0).round() as u8,
                (gg.clamp(0.0, 1.0) * 255.0).round() as u8,
                (bb.clamp(0.0, 1.0) * 255.0).round() as u8,
            ],
        });
    }
    c.y += 16.0;
    c.text(lx, 7.5, Font::Regular, Align::Left, FAINT, "0 lx");
    c.push(Item::Text {
        x: lx + lw,
        y: c.y,
        size: 7.5,
        font: Font::Regular,
        rgb: FAINT,
        align: Align::Right,
        text: format!(
            "{:.0} lx — scale {}",
            inp.scale_top,
            if inp.scale_auto { "auto" } else { "pinned" }
        ),
    });
    c.y += 6.0;
    if !label {
        c.note("Cell values are omitted at this grid size — the exact figures are in the grid table.");
    }
}

/// The numbers, split into column blocks that fit the page.
///
/// A 33-column grid is 33 columns wide whatever the paper is, so it is cut into blocks and each
/// block is labelled with the columns it covers. Dropping the table instead would lose the only
/// part of the report that is checkable.
fn numeric_grid(c: &mut Cursor, inp: &Input) {
    let g = inp.grid;
    if g.cols == 0 || g.rows == 0 {
        return;
    }
    c.heading("Illuminance grid (lx)");

    let colw = 34.0_f64;
    let rowh = 11.0_f64;
    let per_block = ((c.width() / colw).floor() as usize).max(1);
    let blocks = (g.cols as usize).div_ceil(per_block);

    for b in 0..blocks {
        let c0 = b * per_block;
        let c1 = ((b + 1) * per_block).min(g.cols as usize);
        if blocks > 1 {
            c.need(rowh * 3.0);
            c.y += 10.0;
            c.text(
                c.left,
                8.0,
                Font::Bold,
                Align::Left,
                FAINT,
                &format!("Columns {}–{}", c0 + 1, c1),
            );
            c.y += 6.0;
        }
        for r in 0..g.rows as usize {
            c.need(rowh + 2.0);
            c.y += rowh;
            for (k, col) in (c0..c1).enumerate() {
                let i = r * g.cols as usize + col;
                let Some(v) = g.values.get(i) else { continue };
                let inside = inp.mask.get(i).copied().unwrap_or(true);
                c.push(Item::Text {
                    x: c.left + (k as f64 + 1.0) * colw - 4.0,
                    y: c.y,
                    size: 7.0,
                    font: Font::Regular,
                    rgb: if inside { INK } else { [190, 190, 190] },
                    align: Align::Right,
                    text: if inside { format!("{v:.0}") } else { "—".into() },
                });
            }
        }
        c.y += 6.0;
    }
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

    fn input<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> Input<'a> {
        Input {
            grid: g,
            plane: p,
            maintenance: Maintenance { llmf: 0.8, lsf: 1.0, lmf: 1.0, rsmf: 1.0 },
            installation: None,
            surfaces: &[],
            cylindrical_avg: None,
            eye_height: 1.2,
            room_height: 3.0,
            materials: vec![("Floor".into(), 0.2)],
            unassigned: 0,
            ramp: crate::light::lux_rgb,
            scale_top: 500.0,
            scale_auto: true,
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

    /// THE PLOT IS SCALED TO FIT, WHATEVER THE GRID. A 33-column field and a 4-column one both
    /// arrive whole and within the content box — which is what "scale the layout so it fits the
    /// page" asks for.
    #[test]
    fn the_false_colour_plot_fits_the_content_box() {
        for (cols, rows) in [(4u32, 4u32), (33, 40), (80, 3)] {
            let g = grid(cols, rows);
            let p = plane();
            let mut o = opts();
            o.sections = vec![Section::FalseColour];
            o.cover = false;
            let d = layout(&input(&g, &p), &o);
            let r = rects(&d);
            // The plot's cells, ignoring the legend swatches (which are 7 points tall).
            let cells: Vec<_> = r.iter().filter(|(_, _, _, h)| (*h - 7.0).abs() > 0.01).collect();
            assert_eq!(
                cells.len(),
                (cols * rows) as usize,
                "{cols}x{rows}: expected a cell per grid point",
            );
            let minx = cells.iter().map(|c| c.0).fold(f64::MAX, f64::min);
            let maxx = cells.iter().map(|c| c.0 + c.2).fold(f64::MIN, f64::max);
            let (pw, _) = PageSize::A4.points();
            assert!(minx >= MARGIN - 0.5, "{cols}x{rows}: plot starts at {minx:.1}");
            assert!(maxx <= pw - MARGIN + 0.5, "{cols}x{rows}: plot ends at {maxx:.1}");
            // Square cells, or the field is stretched and the picture lies about the room.
            for (_, _, cw, ch) in &cells {
                assert!((cw - ch).abs() < 1e-6, "{cols}x{rows}: cell {cw:.2}x{ch:.2} is not square");
            }
        }
    }

    /// VALUES APPEAR WHEN THERE IS ROOM AND NOT WHEN THERE IS NOT — and when they are dropped the
    /// page says so, rather than leaving a reader to wonder whether the plot is the whole story.
    #[test]
    fn cell_values_are_printed_only_when_legible() {
        let mut o = opts();
        o.sections = vec![Section::FalseColour];
        o.cover = false;

        let small = grid(4, 4);
        let p = plane();
        let d = layout(&input(&small, &p), &o);
        let t = texts(&d);
        assert!(t.iter().any(|s| s == "100"), "a 4x4 plot has room for its values: {t:?}");

        let big = grid(40, 40);
        let d = layout(&input(&big, &p), &o);
        let t = texts(&d);
        assert!(
            !t.iter().any(|s| s == "100"),
            "a 40x40 plot printed values that cannot be read at that size",
        );
        assert!(
            t.iter().any(|s| s.contains("omitted")),
            "the page must say the values were left off: {t:?}",
        );
    }

    /// A MASKED CELL IS BLANK, not coloured — colouring it reports illuminance on ground the room
    /// does not occupy.
    #[test]
    fn cells_outside_the_room_are_not_coloured() {
        let g = grid(2, 1);
        let p = plane();
        let mut i = input(&g, &p);
        i.mask = vec![true, false];
        let mut o = opts();
        o.sections = vec![Section::FalseColour];
        o.cover = false;
        let d = layout(&i, &o);
        let cells: Vec<_> =
            rects(&d).into_iter().filter(|(_, _, _, h)| (*h - 7.0).abs() > 0.01).collect();
        assert_eq!(cells.len(), 1, "only the inside cell is filled");
        let frames = d
            .pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter(|i| matches!(i, Item::Frame { .. }))
            .count();
        assert!(frames >= 2, "the outside cell is outlined, and the plot has its border");
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

    /// THE NUMERIC GRID IS SPLIT, NOT DROPPED. A 33-column table does not fit any paper, and
    /// losing it would lose the only part of the report that can be checked against another tool.
    #[test]
    fn a_wide_numeric_grid_is_split_into_labelled_blocks() {
        let g = grid(33, 4);
        let p = plane();
        let mut o = opts();
        o.cover = false;
        o.sections = vec![Section::NumericGrid];
        let d = layout(&input(&g, &p), &o);
        let t = texts(&d);
        assert!(t.iter().any(|s| s.starts_with("Columns 1")), "the first block is unlabelled: {t:?}");
        assert!(
            t.iter().filter(|s| s.starts_with("Columns ")).count() >= 2,
            "33 columns must be split across blocks",
        );
        // Every value is still present somewhere.
        let numbers = t.iter().filter(|s| s.parse::<f64>().is_ok()).count();
        assert!(numbers >= (33 * 4), "values went missing: only {numbers} printed");
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
