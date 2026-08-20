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
    /// The lighting layout — the room and the fittings in it.
    Layout,
    /// The illuminance result, as a field.
    Results,
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
            Section::Layout => "Lighting layout",
            Section::Results => "Results — illuminance",
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
            Section::Layout,
            Section::Results,
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

    /// Which band a reading falls in — the index into [`edges`](Self::edges)' pairs.
    ///
    /// The ONE place this is worked out. The field, the legend and the printed point values all
    /// have to agree about which band a value belongs to, and three loops with the same comparison
    /// written out separately is three chances to disagree by an epsilon at a boundary — which
    /// shows up as one cell in the wrong colour and is close to impossible to explain afterwards.
    pub fn band_index(&self, v: f64, room_max: f64) -> usize {
        let edges = self.edges(room_max);
        let n = edges.len();
        for (i, pair) in edges.windows(2).enumerate() {
            // The LAST band is closed at the top, so a reading sitting exactly on the ceiling
            // lands in a band rather than falling off the end of the list.
            if v < pair[1] || i + 2 == n {
                return i;
            }
        }
        0
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
    /// A COLOUR PER BAND, chosen by hand, overriding the palette.
    ///
    /// Asked for as: *"the false colors are now picked by the app. the user has choice to select
    /// the colors. in the band add a band color picker."* A practice's drawings are read by people
    /// who have learned what its colours mean, and a house style is not something an app gets to
    /// decide — nor is it something to re-enter on every job, which is why this travels in the
    /// saved settings.
    ///
    /// EMPTY MEANS THE PALETTE, and a short list means the palette for every band past its end. So
    /// this stays out of the way until somebody uses it: an existing project, a fresh install and
    /// a settings file written before the field existed all draw exactly as they did.
    ///
    /// Indexed by BAND, matching [`Scale::band_index`] — band 0 is everything below the first
    /// threshold. Adding or removing a threshold therefore shifts the colours above it, which is
    /// visible in the dialog the moment it happens rather than a surprise in the output.
    #[serde(default)]
    pub band_colours: Vec<[u8; 3]>,
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
            band_colours: Vec::new(),
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


/// The report settings that outlive one project.
///
/// A SEPARATE STRUCT, NOT `#[serde(skip)]` SCATTERED THROUGH `Options`, because the split is a
/// decision and deserves to be written down in one place. A practice's header, its logo, the bands
/// it reads drawings at and the order its reports are laid out in are the same on every job. The
/// project's NAME is not, and neither is where this particular file goes — carrying those forward
/// would put last week's client on this week's cover, which is the kind of mistake that reaches a
/// client.
///
/// Logos travel as PATHS. The bytes are re-read when the dialog opens; a preferences file carrying
/// a megabyte of image per logo is one nobody can open or diff.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(default)]
    pub page: Option<PageSize>,
    #[serde(default)]
    pub cover: Option<bool>,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub footer: String,
    #[serde(default)]
    pub page_numbers: Option<bool>,
    #[serde(default)]
    pub scale: Option<Scale>,
    /// A colour per band — the practice house style, which is exactly the kind of thing that must
    /// not be re-entered on every job. Empty means the palette, so a settings file written before
    /// this existed restores as it always did.
    #[serde(default)]
    pub band_colours: Vec<[u8; 3]>,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub hidden: Vec<(Section, usize)>,
    /// `(path, caption)` per logo, in order.
    #[serde(default)]
    pub logos: Vec<(String, String)>,
    #[serde(default)]
    pub header_image: Option<usize>,
    #[serde(default)]
    pub footer_image: Option<usize>,
}

impl Prefs {
    /// What of these options is worth keeping.
    pub fn of(o: &Options) -> Self {
        Self {
            format: Some(o.format),
            page: Some(o.page),
            cover: Some(o.cover),
            header: o.header.clone(),
            footer: o.footer.clone(),
            page_numbers: Some(o.page_numbers),
            scale: Some(o.scale.clone()),
            band_colours: o.band_colours.clone(),
            sections: o.sections.clone(),
            hidden: o.hidden.clone(),
            logos: o.logos.iter().map(|l| (l.path.clone(), l.caption.clone())).collect(),
            header_image: o.header_image,
            footer_image: o.footer_image,
        }
    }

    /// Put them back, leaving everything that belongs to THIS report alone.
    ///
    /// Each field is applied only if the file actually carried one, so a preferences file written
    /// by an older build — or half-written — restores what it has and defaults the rest, rather
    /// than blanking a header because the field did not exist yet.
    pub fn apply(self, o: &mut Options) {
        if let Some(v) = self.format {
            o.format = v;
        }
        if let Some(v) = self.page {
            o.page = v;
        }
        if let Some(v) = self.cover {
            o.cover = v;
        }
        if let Some(v) = self.page_numbers {
            o.page_numbers = v;
        }
        if let Some(v) = self.scale {
            o.scale = v;
        }
        o.band_colours = self.band_colours;
        o.header = self.header;
        o.footer = self.footer;
        // An EMPTY section list is not a preference, it is a file that never held one — restoring
        // it would give a report with nothing in it and no way to tell why.
        if !self.sections.is_empty() {
            o.sections = self.sections;
            o.hidden = self.hidden;
        }
        // The images themselves are re-read from these paths; see `Options::logo_paths`.
        o.logos = self
            .logos
            .into_iter()
            .map(|(path, caption)| ReportImage { path, caption, jpeg: None })
            .collect();
        // A logo index that no longer names anything is dropped rather than left pointing past the
        // end of a list a person has since shortened.
        let n = o.logos.len();
        o.header_image = self.header_image.filter(|i| *i < n);
        o.footer_image = self.footer_image.filter(|i| *i < n);
    }

    /// Where they live — beside the Illuminaire library and the other cross-project preferences.
    pub fn path() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(std::path::PathBuf::from(home).join(".config/rust_cad/report.json"))
    }

    pub fn load() -> Self {
        match Self::path().and_then(|p| std::fs::read_to_string(p).ok()) {
            // A MALFORMED PREFERENCES FILE IS NOT AN ERROR TO SHOW ANYBODY. Unlike the fitting
            // library, nothing here is irreplaceable — the worst case is re-typing a header — so
            // it defaults quietly rather than putting a dialog in front of a report.
            Some(t) => serde_json::from_str(&t).unwrap_or_default(),
            None => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path().ok_or("no home directory")?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Through a temp file and renamed, so an interrupted write cannot leave half a file where
        // the settings were.
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", p.display())
        })
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

/// THE SETTINGS A PRACTICE KEEPS SURVIVE A RESTART — and the ones belonging to one report do not.
///
/// Asked for as "get the report settings going". The split is the whole of it: a header, a logo
/// and the bands a practice reads drawings at are the same on every job; the project's NAME is
/// not, and carrying it forward would put last week's client on this week's cover.
#[cfg(test)]
mod the_report_settings_are_kept {
    use super::*;

    fn set_up() -> Options {
        let mut o = Options::default();
        o.format = Format::Html;
        o.page = PageSize::Letter;
        o.cover = false;
        o.header = "HSI Lighting · Project 2214".into();
        o.footer = "confidential".into();
        o.page_numbers = false;
        o.scale = Scale { top: Some(750.0), bands: vec![50.0, 200.0] };
        o.move_section(Section::Renders, -1);
        o.set(Section::Surfaces, false);
        o.logos = vec![
            ReportImage { path: "D:/brand/hsi.png".into(), caption: "HSI".into(), jpeg: None },
            ReportImage { path: "D:/brand/iso.png".into(), caption: String::new(), jpeg: None },
        ];
        o.header_image = Some(0);
        o.footer_image = Some(1);
        // …and the things that belong to THIS report.
        o.title = "Gym · Level 2".into();
        o.subtitle = "Issued for tender".into();
        o.out_dir = "D:/jobs/2214".into();
        o.file_stem = "gym-lighting".into();
        o
    }

    /// A ROUND TRIP KEEPS THE STANDING CHOICES.
    #[test]
    fn the_standing_choices_come_back() {
        let src = set_up();
        let mut back = Options::default();
        Prefs::of(&src).apply(&mut back);

        assert_eq!(back.format, Format::Html);
        assert_eq!(back.page, PageSize::Letter);
        assert!(!back.cover);
        assert_eq!(back.header, "HSI Lighting · Project 2214");
        assert_eq!(back.footer, "confidential");
        assert!(!back.page_numbers);
        assert_eq!(back.scale.top, Some(750.0));
        assert_eq!(back.scale.bands, vec![50.0, 200.0]);
        assert_eq!(back.sections, src.sections, "the section order was not kept");
        assert!(!back.has(Section::Surfaces), "a switched-off section came back on");
        assert_eq!(back.logos.len(), 2, "the logos were not kept");
        assert_eq!(back.logos[0].path, "D:/brand/hsi.png");
        assert_eq!(back.logos[0].caption, "HSI");
        assert_eq!(back.header_image, Some(0));
        assert_eq!(back.footer_image, Some(1));
    }

    /// AND DOES NOT CARRY THE PROJECT ACROSS.
    ///
    /// This is the half that matters more: a title, a subtitle and an output folder that followed
    /// a practice from one job to the next would put the wrong client's name on a cover, and the
    /// file somewhere nobody meant.
    #[test]
    fn nothing_belonging_to_one_report_comes_back() {
        let src = set_up();
        let mut back = Options::default();
        Prefs::of(&src).apply(&mut back);

        assert!(back.title.is_empty(), "the project name followed: {:?}", back.title);
        assert!(back.subtitle.is_empty(), "the cover line followed: {:?}", back.subtitle);
        assert!(back.out_dir.is_empty(), "the output folder followed: {:?}", back.out_dir);
        assert!(back.file_stem.is_empty(), "the file name followed: {:?}", back.file_stem);
        assert!(back.images.is_empty(), "the renders followed");
    }

    /// LOGO BYTES ARE NOT IN THE FILE. A preferences file carrying a megabyte of image per logo is
    /// one nobody can open, diff or copy between machines.
    #[test]
    fn only_the_logo_paths_are_written() {
        let mut o = set_up();
        o.logos[0].jpeg = Some((vec![0xAB; 4096], 800, 200));
        let json = serde_json::to_string(&Prefs::of(&o)).expect("serialises");
        assert!(json.contains("hsi.png"), "the path is the record");
        assert!(!json.contains("171,171,171"), "the image bytes went into the file");
        assert!(json.len() < 2000, "the preferences file is {} bytes", json.len());
    }

    /// A FILE FROM AN OLDER BUILD RESTORES WHAT IT HAS and defaults the rest, rather than blanking
    /// a setting because the field did not exist when it was written.
    #[test]
    fn a_partial_file_does_not_blank_what_it_does_not_mention() {
        let p: Prefs = serde_json::from_str(r#"{"header":"HSI Lighting"}"#).expect("older file");
        let mut o = Options::default();
        let defaults = Options::default();
        p.apply(&mut o);
        assert_eq!(o.header, "HSI Lighting");
        assert_eq!(o.format, defaults.format, "the format was blanked");
        assert_eq!(o.page, defaults.page, "the paper size was blanked");
        assert_eq!(o.cover, defaults.cover, "the cover was switched off");
        assert_eq!(o.sections, defaults.sections, "the sections were emptied");
        assert_eq!(o.scale.bands, defaults.scale.bands, "the bands were blanked");
    }

    /// AN EMPTY SECTION LIST IS NOT A PREFERENCE. A file that never held one must not produce a
    /// report with nothing in it and no way to tell why.
    #[test]
    fn an_empty_section_list_is_ignored() {
        let p: Prefs = serde_json::from_str(r#"{"sections":[]}"#).expect("parses");
        let mut o = Options::default();
        p.apply(&mut o);
        assert_eq!(o.sections, Section::all(), "an empty list emptied the report");
    }

    /// A LOGO INDEX THAT NO LONGER NAMES ANYTHING IS DROPPED, not left pointing past the end of a
    /// list somebody has since shortened.
    #[test]
    fn a_stale_logo_index_is_dropped() {
        let mut p = Prefs::of(&set_up());
        p.logos.truncate(1);
        let mut o = Options::default();
        p.apply(&mut o);
        assert_eq!(o.header_image, Some(0), "the surviving logo kept its slot");
        assert_eq!(o.footer_image, None, "the footer pointed past the end of the list");
    }

    /// A CORRUPT FILE IS NOT AN ERROR TO SHOW ANYBODY. Nothing here is irreplaceable — the worst
    /// case is re-typing a header — so it defaults quietly rather than putting a dialog in front
    /// of a report.
    #[test]
    fn a_malformed_file_defaults_quietly() {
        let p: Prefs = serde_json::from_str("{ not json").unwrap_or_default();
        let mut o = Options::default();
        p.apply(&mut o);
        assert_eq!(o.sections, Section::all());
        assert!(o.header.is_empty());
    }
}
