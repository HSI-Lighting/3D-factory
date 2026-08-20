//! What the report dialog decides, and nothing else.
//!
//! Split from the layout so the choices can be saved, defaulted and tested without a page in
//! sight — and so the dialog has one obvious thing to edit.

/// Which file the report becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    /// Paginated, fixed, printable — what leaves an office.
    Pdf,
    /// One flowing page. Opens anywhere, reflows to the window, and is the better format for
    /// reading a long numeric grid on screen.
    Html,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Html => "html",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::Html => "HTML",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PageSize {
    A4,
    Letter,
}

impl PageSize {
    /// Width and height in points.
    pub fn points(self) -> (f64, f64) {
        match self {
            PageSize::A4 => super::pdf::A4,
            PageSize::Letter => super::pdf::LETTER,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PageSize::A4 => "A4",
            PageSize::Letter => "Letter",
        }
    }
}

/// One block of the report, in the order they appear.
///
/// AN ENUM RATHER THAN A PILE OF BOOLEANS, because the render-images page has to be movable among
/// them — "the user should be able to decide the position of the page" — and a position only means
/// something against an ordered list. The order IS the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Section {
    Summary,
    Installation,
    Materials,
    WorkingPlane,
    FalseColour,
    NumericGrid,
    Surfaces,
    /// What the room is lit WITH — manufacturer, catalogue number, load, per fitting type.
    Schedule,
    /// The render images, on a page of their own.
    Renders,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Summary => "Summary",
            Section::Installation => "Installation",
            Section::Materials => "Room & materials",
            Section::WorkingPlane => "Working plane",
            Section::FalseColour => "Illuminance — false colour",
            Section::NumericGrid => "Illuminance grid (lx)",
            Section::Surfaces => "Surfaces",
            Section::Schedule => "Luminaire schedule",
            Section::Renders => "Renders",
        }
    }

    /// Every section, in the order a report reads by default.
    pub fn all() -> Vec<Section> {
        vec![
            Section::Summary,
            Section::Installation,
            Section::Materials,
            Section::WorkingPlane,
            Section::FalseColour,
            Section::Schedule,
            Section::Renders,
            Section::NumericGrid,
            Section::Surfaces,
        ]
    }
}


/// How the false-colour plot's scale is decided.
///
/// THE APP USED TO DECIDE IT and the report simply followed. Two reports of the same room at
/// different auto-scales are not comparable and nothing on either page says so — which is why the
/// scale is a report decision, not a viewport one.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Scale {
    /// Top of the ramp in lux. `None` follows the app — this room's maximum.
    pub top: Option<f64>,
    /// Discrete bands, in lux, low to high. Empty = a continuous ramp.
    ///
    /// BANDS ARE WHAT A LIGHTING DRAWING USES. "0 · 25 · 100 · 300 · 500" says which parts of the
    /// room meet which requirement; a continuous gradient says only that some parts are brighter,
    /// and the eye cannot read a value off it. The reference the user gave is banded.
    pub bands: Vec<f64>,
}

impl Default for Scale {
    fn default() -> Self {
        // The EN 12464-1 steps a lighting drawing is normally banded at.
        Self { top: None, bands: vec![25.0, 100.0, 300.0, 500.0] }
    }
}

impl Scale {
    /// The top of the ramp, given what the room actually reached.
    pub fn top_lx(&self, room_max: f64) -> f64 {
        match self.top {
            Some(t) if t > 0.0 => t,
            _ => {
                // With bands, the ramp runs to the highest band OR the room's maximum, whichever
                // is greater — a room that overshoots the top band must not be clipped to it.
                let band_top = self.bands.last().copied().unwrap_or(0.0);
                room_max.max(band_top).max(1.0)
            }
        }
    }

    /// The colour a value takes, as a ramp position in `0..=1`.
    ///
    /// Banded, the value is snapped to the MIDDLE of the band it falls in, so every reading in a
    /// band gets the same colour — which is what makes a band readable as one region rather than a
    /// gradient with lines drawn on it.
    pub fn t_for(&self, v: f64, room_max: f64) -> f32 {
        let top = self.top_lx(room_max);
        if self.bands.is_empty() {
            return (v / top).clamp(0.0, 1.0) as f32;
        }
        let edges = self.edges(room_max);
        let n = edges.len();
        for (i, pair) in edges.windows(2).enumerate() {
            let (lo, hi) = (pair[0], pair[1]);
            // The LAST band is closed at the top, so a value sitting exactly on the ceiling still
            // lands in a band rather than falling off the end of the list.
            if v < hi || i + 2 == n {
                return (((lo + hi) * 0.5) / top).clamp(0.0, 1.0) as f32;
            }
        }
        1.0
    }

    /// Band edges including 0 and the top, low to high.
    pub fn edges(&self, room_max: f64) -> Vec<f64> {
        let top = self.top_lx(room_max);
        let mut e = vec![0.0];
        for b in &self.bands {
            if *b > 0.0 && *b < top {
                e.push(*b);
            }
        }
        e.push(top);
        e.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        e
    }

    /// What the legend says under the plot.
    pub fn caption(&self, room_max: f64) -> String {
        let mode = if self.top.is_some() { "pinned" } else { "auto" };
        if self.bands.is_empty() {
            format!("0 to {:.0} lx — {mode}", self.top_lx(room_max))
        } else {
            format!("banded to {:.0} lx — {mode}", self.top_lx(room_max))
        }
    }
}

/// An image the user added: where it came from, and what to call it.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ReportImage {
    pub path: String,
    pub caption: String,
    /// JPEG bytes and pixel size, loaded once when the image is added.
    ///
    /// NOT serialised — the path is the record, and a library file carrying a megabyte of image
    /// per report would be a preferences file nobody could open.
    #[serde(skip)]
    pub jpeg: Option<(Vec<u8>, u32, u32)>,
}

/// Everything the dialog decides.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Options {
    pub format: Format,
    pub page: PageSize,
    /// The project name, on the cover and in the header.
    pub title: String,
    /// The extra line the cover can carry — "if they want to add another line of text".
    pub subtitle: String,
    pub cover: bool,
    /// Which image goes on the cover, by index into `images`. `None` = a plain cover.
    pub cover_image: Option<usize>,
    pub header: String,
    pub footer: String,
    pub page_numbers: bool,
    /// The false-colour scale, chosen here rather than followed from the viewport.
    #[serde(default)]
    pub scale: Scale,
    /// A logo for the header, by index into `logos`.
    #[serde(default)]
    pub header_image: Option<usize>,
    /// A logo for the footer, by index into `logos`.
    #[serde(default)]
    pub footer_image: Option<usize>,
    /// The sections to include, IN ORDER. Moving `Renders` within this list is what "decide the
    /// position of the page" means.
    pub sections: Vec<Section>,
    /// Sections switched OFF, with the position each held when it went — so switching one back on
    /// returns it to where it was rather than to where the default order would put it. Without
    /// this, unticking a box and reticking it silently undoes any reordering the user had done.
    #[serde(default)]
    pub hidden: Vec<(Section, usize)>,
    #[serde(skip)]
    pub images: Vec<ReportImage>,
    /// LOGOS, kept apart from the renders.
    ///
    /// They shared one list, so putting a logo in the header meant first adding it as a RENDER —
    /// where it then appeared, full width, on the renders page. They are different things used in
    /// different places and are chosen from different buttons.
    #[serde(skip)]
    pub logos: Vec<ReportImage>,
    /// Where the file goes. Empty until chosen.
    pub out_dir: String,
    pub file_stem: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            format: Format::Pdf,
            page: PageSize::A4,
            title: String::new(),
            subtitle: String::new(),
            cover: true,
            cover_image: None,
            header: String::new(),
            footer: String::new(),
            page_numbers: true,
            scale: Scale::default(),
            header_image: None,
            footer_image: None,
            sections: Section::all(),
            hidden: Vec::new(),
            images: Vec::new(),
            logos: Vec::new(),
            out_dir: String::new(),
            file_stem: String::new(),
        }
    }
}

impl Options {
    /// Whether a section is switched on.
    pub fn has(&self, s: Section) -> bool {
        self.sections.contains(&s)
    }

    /// Turn a section on or off, RETURNING IT TO WHERE IT WAS when it comes back.
    ///
    /// Two things would be wrong here and both are silent. Appending a re-enabled section puts it
    /// at the end of the report; re-inserting it by the DEFAULT order undoes any reordering the
    /// user had done, so unticking a box to look at the page without it and reticking it would
    /// quietly move the renders page. The order a report reads in is a decision, and an accidental
    /// one is still one — so where it was is remembered.
    pub fn set(&mut self, s: Section, on: bool) {
        if on == self.has(s) {
            return;
        }
        if !on {
            if let Some(i) = self.sections.iter().position(|x| *x == s) {
                self.sections.remove(i);
                self.hidden.retain(|(x, _)| *x != s);
                self.hidden.push((s, i));
            }
            return;
        }
        let at = match self.hidden.iter().position(|(x, _)| *x == s) {
            Some(k) => {
                let (_, i) = self.hidden.remove(k);
                i.min(self.sections.len())
            }
            // Never seen before — fall back to the order a report reads by default.
            None => {
                let order = Section::all();
                let rank = |x: Section| order.iter().position(|y| *y == x).unwrap_or(usize::MAX);
                self.sections.iter().position(|x| rank(*x) > rank(s)).unwrap_or(self.sections.len())
            }
        };
        self.sections.insert(at, s);
    }

    /// Move a section one place earlier or later. Returns whether anything moved.
    pub fn move_section(&mut self, s: Section, delta: i32) -> bool {
        let Some(i) = self.sections.iter().position(|x| *x == s) else { return false };
        let j = i as i32 + delta;
        if j < 0 || j as usize >= self.sections.len() {
            return false;
        }
        self.sections.swap(i, j as usize);
        true
    }

    /// The file the report will be written to.
    pub fn out_path(&self) -> std::path::PathBuf {
        let stem = if self.file_stem.trim().is_empty() { "report" } else { self.file_stem.trim() };
        std::path::Path::new(&self.out_dir).join(format!("{stem}.{}", self.format.ext()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A SECTION SWITCHED BACK ON RETURNS TO ITS PLACE, not to the end.
    ///
    /// Appending would silently reorder the document — untick Materials to see the page without
    /// it, tick it again, and the report now reads in a different order than it did. The order is
    /// a decision, and an accidental one is still one.
    #[test]
    fn re_enabling_a_section_restores_its_order() {
        let mut o = Options::default();
        o.set(Section::Materials, false);
        assert!(!o.has(Section::Materials));
        o.set(Section::Materials, true);
        let want = Section::all();
        assert_eq!(o.sections, want, "the order changed after a round trip");
    }

    /// …unless the user MOVED it, which is a decision and must survive.
    #[test]
    fn a_moved_section_stays_where_it_was_put() {
        let mut o = Options::default();
        // Renders to the very front of the body.
        while o.move_section(Section::Renders, -1) {}
        assert_eq!(o.sections[0], Section::Renders);
        o.set(Section::Summary, false);
        o.set(Section::Summary, true);
        assert_eq!(o.sections[0], Section::Renders, "an unrelated toggle undid the move");
    }

    /// MOVING STOPS AT THE ENDS rather than wrapping — a list that jumps from top to bottom on one
    /// more click is one nobody can aim.
    #[test]
    fn moving_past_the_end_does_nothing() {
        let mut o = Options::default();
        let first = o.sections[0];
        assert!(!o.move_section(first, -1), "the first section moved up");
        assert_eq!(o.sections[0], first);
        let last = *o.sections.last().expect("sections");
        assert!(!o.move_section(last, 1), "the last section moved down");
        assert_eq!(*o.sections.last().expect("sections"), last);
    }

    /// TOGGLING IS IDEMPOTENT. Ticking a section that is already on must not put it in twice, or
    /// the report prints it twice.
    #[test]
    fn switching_on_twice_does_not_duplicate() {
        let mut o = Options::default();
        o.set(Section::Summary, true);
        o.set(Section::Summary, true);
        assert_eq!(o.sections.iter().filter(|s| **s == Section::Summary).count(), 1);
    }

    /// THE EXTENSION FOLLOWS THE FORMAT. A PDF written as `.html` opens in a browser as a wall of
    /// binary, which reads as a corrupt report rather than a misnamed one.
    #[test]
    fn the_file_name_follows_the_format() {
        let mut o = Options::default();
        o.out_dir = if cfg!(windows) { "C:\\out".into() } else { "/out".into() };
        o.file_stem = "gym".into();
        assert!(o.out_path().to_string_lossy().ends_with("gym.pdf"));
        o.format = Format::Html;
        assert!(o.out_path().to_string_lossy().ends_with("gym.html"));
    }

    /// AN EMPTY NAME STILL PRODUCES A FILE rather than a directory with a dot in it.
    #[test]
    fn a_blank_name_falls_back() {
        let mut o = Options::default();
        o.out_dir = if cfg!(windows) { "C:\\out".into() } else { "/out".into() };
        o.file_stem = "   ".into();
        assert!(o.out_path().to_string_lossy().ends_with("report.pdf"));
    }
}
