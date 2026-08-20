//! A PDF writer, sized to what a lighting report needs and nothing more.
//!
//! WHY HAND-WRITTEN. A report that can only be produced as HTML cannot be issued: what leaves an
//! office goes out as PDF, because that is the format that paginates, prints the same everywhere,
//! and cannot be edited by whoever receives it. Nothing in the tree wrote one.
//!
//! The alternative was a PDF crate. This document needs four things — text in the base-14
//! Helvetica family, filled rectangles, thin lines, and embedded JPEG images — and every one of
//! them is a handful of operators. A dependency would bring font subsetting, transparency groups,
//! tagged structure and a great deal else this will never emit, and would still have to be driven
//! by exactly the layout model below.
//!
//! WHAT IS DELIBERATELY NOT HERE: font embedding (the base-14 fonts are guaranteed present in
//! every reader, and the report is Latin text), compression of content streams (a lighting report
//! is a few hundred kilobytes uncompressed, and an uncompressed stream is one a person can read
//! with `strings` when something looks wrong), encryption, and outlines.
//!
//! COORDINATES ARE PDF'S: points, origin BOTTOM-LEFT, y upward. The layout model above works in
//! the same units with y DOWNWARD, because that is how a page reads and how the preview paints;
//! the flip happens once, here, in `emit`.

/// One drawable, in page coordinates: points from the TOP-LEFT, y downward.
#[derive(Clone, Debug)]
pub enum Item {
    /// Filled rectangle. The false-colour plot is thousands of these.
    Rect { x: f64, y: f64, w: f64, h: f64, fill: [u8; 3] },
    /// Hairline rectangle outline.
    Frame { x: f64, y: f64, w: f64, h: f64, rgb: [u8; 3], width: f64 },
    /// A straight line — rules under headings, table separators.
    Line { x1: f64, y1: f64, x2: f64, y2: f64, rgb: [u8; 3], width: f64 },
    /// Text with its baseline at `y`.
    Text { x: f64, y: f64, size: f64, font: Font, rgb: [u8; 3], align: Align, text: String },
    /// An image already encoded as JPEG, by index into the document's image table.
    Image { x: f64, y: f64, w: f64, h: f64, idx: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Regular,
    Bold,
}

impl Font {
    fn base14(self) -> &'static str {
        match self {
            Font::Regular => "Helvetica",
            Font::Bold => "Helvetica-Bold",
        }
    }
    fn res(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Centre,
}

/// A JPEG image, ready to embed.
///
/// JPEG rather than PNG because PDF takes a JPEG's bytes VERBATIM (`DCTDecode`) — no re-encoding,
/// no colour-space guessing, and no dependency on a deflate implementation agreeing with the
/// reader's. A render is a photograph-like image, which is what JPEG is for.
#[derive(Clone)]
pub struct Jpeg {
    pub bytes: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// One page of a document.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub items: Vec<Item>,
}

/// A paginated document, ready to write.
#[derive(Clone, Default)]
pub struct Doc {
    pub pages: Vec<Page>,
    pub images: Vec<Jpeg>,
    /// Page size in points. A4 is 595.28 x 841.89.
    pub width: f64,
    pub height: f64,
    pub title: String,
}

/// A4 in points — the size every European office prints on.
pub const A4: (f64, f64) = (595.28, 841.89);
/// US Letter, for offices that do not.
pub const LETTER: (f64, f64) = (612.0, 792.0);

/// Width of `text` at `size`, in points.
///
/// From the Adobe Font Metrics widths for Helvetica, which the base-14 fonts are guaranteed to
/// match. Needed because RIGHT and CENTRE alignment have to be resolved before the operators are
/// written — PDF has no concept of alignment, only a starting point.
///
/// Characters outside the table are treated as 0.556 em, the width of a digit, which is the right
/// guess for the accented Latin this report can contain and never wrong by enough to matter for
/// text that is being centred rather than justified.
pub fn text_width(text: &str, size: f64, font: Font) -> f64 {
    let em: f64 = text.chars().map(|c| char_width(c, font)).sum();
    em * size / 1000.0
}

fn char_width(c: char, font: Font) -> f64 {
    // Helvetica and Helvetica-Bold differ enough that centring with one table visibly drifts on a
    // bold heading, so both are here. Only the ranges this report emits are tabulated.
    let i = c as u32;
    let regular = match i {
        32 => 278.0,  // space
        33 => 278.0,  // !
        34 => 355.0,  // "
        35..=36 => 556.0,
        37 => 889.0, // %
        38 => 667.0, // &
        39 => 191.0, // '
        40 | 41 => 333.0,
        42 => 389.0,
        43 => 584.0,
        44 => 278.0, // ,
        45 => 333.0, // -
        46 => 278.0, // .
        47 => 278.0, // /
        48..=57 => 556.0,
        58 | 59 => 278.0,
        60..=62 => 584.0,
        63 => 556.0,
        64 => 1015.0,
        // A B C D E G H N O Q R U V W X Y — the 722-unit capitals.
        65 | 66 | 67 | 68 | 69 | 71 | 72 | 78 | 79 | 81 | 82 | 85 | 86 | 87 | 88 | 89 => 722.0,
        70 => 611.0,
        73 => 278.0,
        74 => 500.0,
        75 => 722.0,
        76 => 611.0,
        77 => 833.0,
        80 | 83 => 667.0,
        84 | 90 => 611.0,
        91 | 93 => 278.0,
        92 => 278.0,
        94 => 469.0,
        95 => 556.0,
        97 | 98 | 99 | 100 | 101 | 103 | 104 | 110 | 111 | 112 | 113 | 117 => 556.0,
        102 | 116 => 278.0,
        105 | 106 => 222.0,
        107 | 118 | 120 | 121 | 122 => 500.0,
        108 => 222.0,
        109 => 833.0,
        114 => 333.0,
        115 => 500.0,
        119 => 722.0,
        123 | 125 => 334.0,
        124 => 260.0,
        _ => 556.0,
    };
    match font {
        Font::Regular => regular,
        // Bold is uniformly a little wider; the exceptions that matter are the narrow glyphs,
        // which stay narrow.
        Font::Bold => match i {
            32 => 278.0,
            46 | 44 => 278.0,
            105 | 106 | 108 => 278.0,
            102 | 116 => 333.0,
            114 => 389.0,
            _ => regular * 1.06,
        },
    }
}

/// PDF string escaping. A room called `Smith (Ltd)\` must not close the literal early.
fn pdf_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            // WinAnsi is a single-byte encoding: anything outside it has no code point to emit, and
            // a raw multi-byte UTF-8 character would come out as mojibake. A question mark is
            // obviously a substitution; mojibake looks like a corrupt file.
            c if (c as u32) < 32 => out.push(' '),
            c if (c as u32) < 127 => out.push(c),
            c => out.push(winansi(c)),
        }
    }
    out
}

/// Map the few non-ASCII characters this report actually emits into WinAnsiEncoding.
///
/// The list is short because it is exactly what the numbers and headings use: degrees, the middle
/// dot between fields, the subscript-free micro sign, and the dashes. Everything else becomes `?`.
fn winansi(c: char) -> char {
    match c {
        '°' => '\u{b0}',
        '·' => '\u{b7}',
        '–' | '—' => '\u{96}',
        '’' | '\'' => '\u{92}',
        '“' | '”' => '"',
        '×' => '\u{d7}',
        '²' => '\u{b2}',
        '³' => '\u{b3}',
        'µ' => '\u{b5}',
        '€' => '\u{80}',
        _ => '?',
    }
}

fn col(rgb: [u8; 3]) -> String {
    format!(
        "{:.3} {:.3} {:.3}",
        rgb[0] as f64 / 255.0,
        rgb[1] as f64 / 255.0,
        rgb[2] as f64 / 255.0
    )
}

impl Doc {
    pub fn new(size: (f64, f64), title: impl Into<String>) -> Self {
        Self {
            pages: Vec::new(),
            images: Vec::new(),
            width: size.0,
            height: size.1,
            title: title.into(),
        }
    }

    /// The content stream for one page, with y flipped into PDF's upward axis.
    fn stream(&self, page: &Page) -> String {
        let mut s = String::with_capacity(4096);
        let flip = |y: f64| self.height - y;
        for it in &page.items {
            match it {
                Item::Rect { x, y, w, h, fill } => {
                    // A zero-area rectangle emits nothing: `re` with a zero side draws a hairline
                    // in some readers and nothing in others, and a false-colour plot with a
                    // degenerate cell would differ between them.
                    if *w <= 0.0 || *h <= 0.0 {
                        continue;
                    }
                    s.push_str(&format!(
                        "{} rg {:.3} {:.3} {:.3} {:.3} re f\n",
                        col(*fill),
                        x,
                        flip(y + h),
                        w,
                        h
                    ));
                }
                Item::Frame { x, y, w, h, rgb, width } => {
                    if *w <= 0.0 || *h <= 0.0 {
                        continue;
                    }
                    s.push_str(&format!(
                        "{} RG {:.3} w {:.3} {:.3} {:.3} {:.3} re S\n",
                        col(*rgb),
                        width,
                        x,
                        flip(y + h),
                        w,
                        h
                    ));
                }
                Item::Line { x1, y1, x2, y2, rgb, width } => {
                    s.push_str(&format!(
                        "{} RG {:.3} w {:.3} {:.3} m {:.3} {:.3} l S\n",
                        col(*rgb),
                        width,
                        x1,
                        flip(*y1),
                        x2,
                        flip(*y2)
                    ));
                }
                Item::Text { x, y, size, font, rgb, align, text } => {
                    if text.is_empty() {
                        continue;
                    }
                    let w = text_width(text, *size, *font);
                    let x0 = match align {
                        Align::Left => *x,
                        Align::Right => x - w,
                        Align::Centre => x - w * 0.5,
                    };
                    s.push_str(&format!(
                        "BT /{} {:.2} Tf {} rg {:.3} {:.3} Td ({}) Tj ET\n",
                        font.res(),
                        size,
                        col(*rgb),
                        x0,
                        flip(*y),
                        pdf_str(text)
                    ));
                }
                Item::Image { x, y, w, h, idx } => {
                    if *idx >= self.images.len() || *w <= 0.0 || *h <= 0.0 {
                        continue;
                    }
                    // An image is drawn by scaling the unit square, so the matrix IS the placement.
                    s.push_str(&format!(
                        "q {:.3} 0 0 {:.3} {:.3} {:.3} cm /Im{} Do Q\n",
                        w,
                        h,
                        x,
                        flip(y + h),
                        idx
                    ));
                }
            }
        }
        s
    }

    /// Write the whole file.
    ///
    /// Objects are numbered in one pass and their byte offsets recorded as they are written, which
    /// is what the cross-reference table at the end is: a reader seeks to `startxref`, reads the
    /// table, and jumps straight to any object. Getting an offset wrong produces a file that opens
    /// in a forgiving reader and fails in a strict one, so the offsets are taken from the buffer
    /// itself rather than computed.
    pub fn write(&self) -> Vec<u8> {
        let n_pages = self.pages.len().max(1);
        // 1 catalogue, 2 pages tree, 3..3+n page objects, then n content streams, then 2 fonts,
        // then one object per image.
        let first_page = 3usize;
        let first_content = first_page + n_pages;
        let font_regular = first_content + n_pages;
        let font_bold = font_regular + 1;
        let first_image = font_bold + 1;
        // The LAST object number, not one past it. `first_image + len` counts an image object that
        // does not exist when there are none, and the cross-reference table then advertises an
        // entry whose offset was never filled in — a zero, which sends a reader to the file header.
        let n_objects = font_bold + self.images.len();

        let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut offsets = vec![0usize; n_objects + 1];

        out.extend_from_slice(b"%PDF-1.4\n");
        // A comment of high bytes, which is how a file announces itself as binary to anything that
        // might otherwise transfer it as text and corrupt the image streams.
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

        let mut obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, n: usize, body: &[u8]| {
            offsets[n] = out.len();
            out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        };

        obj(&mut out, &mut offsets, 1, b"<< /Type /Catalog /Pages 2 0 R >>");

        let kids: String =
            (0..n_pages).map(|i| format!("{} 0 R ", first_page + i)).collect::<String>();
        obj(
            &mut out,
            &mut offsets,
            2,
            format!("<< /Type /Pages /Count {n_pages} /Kids [{kids}] >>").as_bytes(),
        );

        let xobjects: String = (0..self.images.len())
            .map(|i| format!("/Im{i} {} 0 R ", first_image + i))
            .collect::<String>();
        for i in 0..n_pages {
            let body = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] \
                 /Resources << /Font << /F1 {font_regular} 0 R /F2 {font_bold} 0 R >> \
                 /XObject << {xobjects} >> >> /Contents {} 0 R >>",
                self.width,
                self.height,
                first_content + i,
            );
            obj(&mut out, &mut offsets, first_page + i, body.as_bytes());
        }

        let blank = Page::default();
        for i in 0..n_pages {
            let s = self.stream(self.pages.get(i).unwrap_or(&blank));
            let body = format!("<< /Length {} >>\nstream\n{s}endstream", s.len());
            obj(&mut out, &mut offsets, first_content + i, body.as_bytes());
        }

        for (n, f) in [(font_regular, Font::Regular), (font_bold, Font::Bold)] {
            let body = format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding >>",
                f.base14()
            );
            obj(&mut out, &mut offsets, n, body.as_bytes());
        }

        for (i, im) in self.images.iter().enumerate() {
            let n = first_image + i;
            offsets[n] = out.len();
            out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            out.extend_from_slice(
                format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\n\
                     stream\n",
                    im.w,
                    im.h,
                    im.bytes.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&im.bytes);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        }

        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", n_objects + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for n in 1..=n_objects {
            out.extend_from_slice(format!("{:010} 00000 n \n", offsets[n]).as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info << /Title ({}) /Producer (SIMLUX {}) >> >>\n\
                 startxref\n{xref}\n%%EOF\n",
                n_objects + 1,
                pdf_str(&self.title),
                env!("SIMLUX_BUILD"),
            )
            .as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Doc {
        let mut d = Doc::new(A4, "Test");
        d.pages.push(Page {
            items: vec![
                Item::Text {
                    x: 40.0,
                    y: 60.0,
                    size: 18.0,
                    font: Font::Bold,
                    rgb: [0, 0, 0],
                    align: Align::Left,
                    text: "Working plane".into(),
                },
                Item::Rect { x: 40.0, y: 80.0, w: 100.0, h: 20.0, fill: [255, 0, 0] },
            ],
        });
        d
    }

    /// THE FILE IS STRUCTURALLY A PDF.
    ///
    /// A reader finds everything through the cross-reference table, so a byte offset that is off
    /// by one produces a file that opens in a forgiving viewer and fails in a strict one — which
    /// is the worst way for this to break, because it passes the only check anyone does by hand.
    #[test]
    fn the_cross_reference_table_points_at_the_objects() {
        let bytes = doc().write();
        // EVERY OFFSET IS CHECKED AGAINST THE BYTES, never against a lossy string of them. The
        // header carries a deliberate run of high bytes to mark the file binary, and
        // `from_utf8_lossy` turns each into a three-byte replacement character — so string indices
        // and byte offsets part company in the first line of the file, and a table that is exactly
        // right reads as eight bytes wrong.
        let at = |off: usize| -> &[u8] { &bytes[off.min(bytes.len())..] };
        assert!(bytes.starts_with(b"%PDF-1.4"), "no header");
        assert!(bytes.ends_with(b"%%EOF\n"), "no trailer");

        let tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]).into_owned();
        let xref_at: usize = tail
            .rsplit("startxref\n")
            .next()
            .and_then(|s| s.split('\n').next())
            .and_then(|s| s.trim().parse().ok())
            .expect("startxref must name a byte offset");
        assert!(at(xref_at).starts_with(b"xref"), "startxref does not point at the table");

        // Every entry must land exactly on its own "<n> 0 obj".
        let n: usize = tail
            .split("/Size ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .expect("the trailer must state a size");
        let table = String::from_utf8_lossy(at(xref_at)).into_owned();
        // "xref", "0 N", then the free entry for object 0 — the real entries start at the fourth.
        let mut lines = table.lines().skip(3);
        for i in 1..n {
            let l = lines.next().expect("an entry per object");
            let off: usize = l[..10].parse().expect("a ten-digit offset");
            assert!(off > 0 && off < bytes.len(), "object {i} offset {off} is out of the file");
            assert!(
                at(off).starts_with(format!("{i} 0 obj").as_bytes()),
                "the table sends object {i} to {off}, which holds {:?}",
                String::from_utf8_lossy(&at(off)[..12.min(at(off).len())]),
            );
        }
    }

    /// THE PAGE TREE COUNTS ITS PAGES. A `/Count` that disagrees with `/Kids` gives a document
    /// that opens showing one page and prints another number of them.
    #[test]
    fn the_page_tree_matches_the_pages() {
        let mut d = doc();
        d.pages.push(Page::default());
        d.pages.push(Page::default());
        let text = String::from_utf8_lossy(&d.write()).into_owned();
        assert!(text.contains("/Count 3"), "the tree does not count three pages");
        assert_eq!(text.matches("/Type /Page ").count(), 3, "three page objects");
        assert_eq!(text.matches("/Type /Pages").count(), 1);
    }

    /// Y IS FLIPPED EXACTLY ONCE. The layout model measures DOWN from the top because that is how
    /// a page reads; PDF measures UP from the bottom. Getting this wrong mirrors the whole report
    /// vertically, which looks like a layout bug rather than a coordinate bug.
    #[test]
    fn the_origin_is_moved_to_the_top_left() {
        let mut d = Doc::new(A4, "T");
        d.pages.push(Page {
            items: vec![Item::Rect { x: 10.0, y: 0.0, w: 5.0, h: 20.0, fill: [0, 0, 0] }],
        });
        let text = String::from_utf8_lossy(&d.write()).into_owned();
        // A band across the TOP 20 points of the page sits at y = height - 20 in PDF space.
        let want = format!("{:.3} {:.3} {:.3} {:.3} re", 10.0, A4.1 - 20.0, 5.0, 20.0);
        assert!(text.contains(&want), "expected {want:?} in the stream");
    }

    /// A JPEG GOES IN VERBATIM. `DCTDecode` means the reader does the decoding, so re-encoding
    /// here would only lose a generation of quality — and a stream `/Length` that disagreed with
    /// the bytes would truncate the image.
    #[test]
    fn an_image_is_embedded_unaltered() {
        let mut d = doc();
        let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        d.images.push(Jpeg { bytes: bytes.clone(), w: 640, h: 480 });
        d.pages[0].items.push(Item::Image { x: 0.0, y: 0.0, w: 100.0, h: 75.0, idx: 0 });
        let out = d.write();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains("/Filter /DCTDecode"));
        assert!(text.contains("/Width 640 /Height 480"));
        assert!(text.contains(&format!("/Length {}", bytes.len())));
        assert!(
            out.windows(bytes.len()).any(|w| w == bytes.as_slice()),
            "the image bytes were altered on the way in",
        );
        assert!(text.contains("/Im0 "), "the image is not in the page resources");
    }

    /// AN IMAGE NOBODY ADDED IS NOT DRAWN. A stale index would emit `/Im7 Do` against a resource
    /// dictionary that has no `Im7`, which most readers report as a broken document.
    #[test]
    fn an_out_of_range_image_is_skipped() {
        let mut d = doc();
        d.pages[0].items.push(Item::Image { x: 0.0, y: 0.0, w: 10.0, h: 10.0, idx: 3 });
        let text = String::from_utf8_lossy(&d.write()).into_owned();
        assert!(!text.contains("/Im3 Do"), "a missing image was referenced anyway");
    }

    /// TEXT IS ESCAPED. An unescaped bracket closes the string literal early and everything after
    /// it becomes operators — which is a corrupt page, not a stray character.
    #[test]
    fn brackets_and_backslashes_survive() {
        assert_eq!(pdf_str("Smith (Ltd)"), "Smith \\(Ltd\\)");
        assert_eq!(pdf_str(r"C:\plans"), r"C:\\plans");
        let mut d = Doc::new(A4, "x");
        d.pages.push(Page {
            items: vec![Item::Text {
                x: 0.0,
                y: 0.0,
                size: 10.0,
                font: Font::Regular,
                rgb: [0, 0, 0],
                align: Align::Left,
                text: "a (b) c".into(),
            }],
        });
        let text = String::from_utf8_lossy(&d.write()).into_owned();
        assert!(text.contains("(a \\(b\\) c) Tj"));
    }

    /// A CHARACTER WITH NO CODE POINT BECOMES A QUESTION MARK, not mojibake. Raw UTF-8 in a
    /// WinAnsi string comes out as two wrong glyphs, which reads as a corrupt file; `?` reads as a
    /// substitution.
    #[test]
    fn text_outside_the_encoding_is_substituted() {
        assert_eq!(pdf_str("36°"), "36\u{b0}", "degrees are in WinAnsi and must survive");
        assert_eq!(pdf_str("a·b"), "a\u{b7}b");
        assert_eq!(pdf_str("日"), "?");
        assert!(!pdf_str("日").is_empty(), "a substitution, not a deletion");
    }

    /// ALIGNMENT IS RESOLVED HERE, because PDF has none. A right-aligned number that was emitted
    /// at its left edge would run off the page rather than line up with the column above it.
    #[test]
    fn alignment_moves_the_starting_point() {
        let make = |align| {
            let mut d = Doc::new(A4, "x");
            d.pages.push(Page {
                items: vec![Item::Text {
                    x: 500.0,
                    y: 100.0,
                    size: 10.0,
                    font: Font::Regular,
                    rgb: [0, 0, 0],
                    align,
                    text: "1333 lx".into(),
                }],
            });
            let t = String::from_utf8_lossy(&d.write()).into_owned();
            let seg = t.split(" Td (").next().unwrap().to_string();
            seg.rsplit("rg ").next().unwrap().split_whitespace().next().unwrap().parse::<f64>().unwrap()
        };
        let left = make(Align::Left);
        let right = make(Align::Right);
        let centre = make(Align::Centre);
        assert!((left - 500.0).abs() < 1e-6, "left starts where it was put");
        let w = text_width("1333 lx", 10.0, Font::Regular);
        assert!(w > 20.0, "the width table gave {w} for seven characters");
        assert!((right - (500.0 - w)).abs() < 1e-6, "right ends at x");
        assert!((centre - (500.0 - w * 0.5)).abs() < 1e-6, "centre straddles x");
    }

    /// A ZERO-SIZED RECTANGLE IS NOT DRAWN. `re` with a zero side is a hairline in some readers
    /// and nothing in others, so a false-colour plot with a degenerate cell would not be the same
    /// document everywhere — which is the one thing a PDF is chosen for.
    #[test]
    fn degenerate_shapes_emit_nothing() {
        let mut d = Doc::new(A4, "x");
        d.pages.push(Page {
            items: vec![
                Item::Rect { x: 0.0, y: 0.0, w: 0.0, h: 10.0, fill: [1, 2, 3] },
                Item::Frame { x: 0.0, y: 0.0, w: 10.0, h: 0.0, rgb: [1, 2, 3], width: 1.0 },
            ],
        });
        let text = String::from_utf8_lossy(&d.write()).into_owned();
        assert!(!text.contains(" re f"), "a zero-width rectangle was filled");
        assert!(!text.contains(" re S"), "a zero-height frame was stroked");
    }
}
