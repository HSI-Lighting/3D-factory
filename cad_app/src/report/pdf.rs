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
    /// A FILLED POLYGON — which is what a false-colour band actually is.
    ///
    /// Reported as: "the false color is still way too coarse make it smooth. it looks all
    /// pixelated around the edges." A band drawn as a mosaic of axis-aligned rectangles has
    /// STAIRCASED edges by construction: every boundary must land on a raster row, so the diagonal
    /// edge of a pool of light comes out as a flight of steps the size of the raster — and raising
    /// the raster does not remove them, it only makes the steps smaller and the file bigger. The
    /// reference plots are contours, and a contour is a polygon.
    ///
    /// Several rings, because one band is usually several disjoint pools and may enclose a
    /// brighter one. Filled with the EVEN-ODD rule so an inner ring reads as a hole rather than
    /// painting over the band inside it.
    Poly { rings: Vec<Vec<(f64, f64)>>, fill: [u8; 3] },
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
        // ELLIPSIS. Tabulated because the fallback below is 556 and Helvetica sets this at 1000 --
        // an under-measured ellipsis is exactly the overflow the truncation exists to prevent.
        8230 => 1000.0,
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

/// PDF string escaping, in BYTES.
///
/// A room called `Smith (Ltd)\` must not close the literal early — and the result has to be bytes
/// rather than a `String`, because a PDF string IS a byte sequence in WinAnsiEncoding while Rust
/// writes a `String` out as UTF-8. That is not a subtle difference: the em dash in "Illuminance —
/// false colour" is the single byte 0x96, and encoded as UTF-8 it becomes 0xC2 0x96 — which a
/// reader shows as "Â–". It shipped that way once, and the report said "Illuminance Â– false
/// colour" on every page it appeared.
fn pdf_str(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.extend_from_slice(b"\\\\"),
            '(' => out.extend_from_slice(b"\\("),
            ')' => out.extend_from_slice(b"\\)"),
            // A control byte would be a literal newline inside a string, which some readers take
            // as the end of the line and others as content.
            c if (c as u32) < 32 => out.push(b' '),
            c if (c as u32) < 127 => out.push(c as u8),
            c => out.push(winansi(c)),
        }
    }
    out
}

/// Map the few non-ASCII characters this report actually emits into WinAnsiEncoding.
///
/// The list is short because it is exactly what the numbers and headings use: degrees, the middle
/// dot between fields, the micro sign, and the dashes. Everything else becomes `?` — obviously a
/// substitution, where mojibake looks like a corrupt file.
fn winansi(c: char) -> u8 {
    match c {
        '°' => 0xb0,
        '·' => 0xb7,
        '–' | '—' => 0x96,
        '’' | '\'' => 0x92,
        '“' | '”' => b'"',
        '×' => 0xd7,
        '²' => 0xb2,
        '³' => 0xb3,
        'µ' => 0xb5,
        '€' => 0x80,
        // Used by the schedule to mark a name it had to cut — see `layout::fit_to`.
        '…' => 0x85,
        _ => b'?',
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
    fn stream(&self, page: &Page) -> Vec<u8> {
        let mut s: Vec<u8> = Vec::with_capacity(4096);
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
                    s.extend_from_slice(
                        format!(
                            "{} rg {:.3} {:.3} {:.3} {:.3} re f\n",
                            col(*fill),
                            x,
                            flip(y + h),
                            w,
                            h
                        )
                        .as_bytes(),
                    );
                }
                Item::Poly { rings, fill } => {
                    // A ring of fewer than three points encloses nothing; emitting it would leave
                    // a stray `f` operator acting on whatever path came before.
                    let usable: Vec<&Vec<(f64, f64)>> =
                        rings.iter().filter(|r| r.len() >= 3).collect();
                    if usable.is_empty() {
                        continue;
                    }
                    s.extend_from_slice(format!("{} rg\n", col(*fill)).as_bytes());
                    for r in usable {
                        let (x, y) = r[0];
                        s.extend_from_slice(format!("{x:.3} {:.3} m\n", flip(y)).as_bytes());
                        for (x, y) in &r[1..] {
                            s.extend_from_slice(format!("{x:.3} {:.3} l\n", flip(*y)).as_bytes());
                        }
                        s.extend_from_slice(b"h\n");
                    }
                    // `f*` — even-odd. See `Item::Poly`: a band that encloses a brighter one needs
                    // the inner ring to become a hole, and the non-zero rule would fill it solid
                    // whenever the two rings happened to wind the same way.
                    s.extend_from_slice(b"f*\n");
                }
                Item::Frame { x, y, w, h, rgb, width } => {
                    if *w <= 0.0 || *h <= 0.0 {
                        continue;
                    }
                    s.extend_from_slice(
                        format!(
                            "{} RG {:.3} w {:.3} {:.3} {:.3} {:.3} re S\n",
                            col(*rgb),
                            width,
                            x,
                            flip(y + h),
                            w,
                            h
                        )
                        .as_bytes(),
                    );
                }
                Item::Line { x1, y1, x2, y2, rgb, width } => {
                    s.extend_from_slice(
                        format!(
                            "{} RG {:.3} w {:.3} {:.3} m {:.3} {:.3} l S\n",
                            col(*rgb),
                            width,
                            x1,
                            flip(*y1),
                            x2,
                            flip(*y2)
                        )
                        .as_bytes(),
                    );
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
                    // THE TEXT IS SPLICED IN AS BYTES, not formatted into the string. `pdf_str`
                    // returns WinAnsi bytes, and putting them through `format!` would re-encode
                    // them as UTF-8 — which is the whole bug it exists to avoid.
                    s.extend_from_slice(
                        format!(
                            "BT /{} {:.2} Tf {} rg {:.3} {:.3} Td (",
                            font.res(),
                            size,
                            col(*rgb),
                            x0,
                            flip(*y),
                        )
                        .as_bytes(),
                    );
                    s.extend_from_slice(&pdf_str(text));
                    s.extend_from_slice(b") Tj ET\n");
                }
                Item::Image { x, y, w, h, idx } => {
                    if *idx >= self.images.len() || *w <= 0.0 || *h <= 0.0 {
                        continue;
                    }
                    // An image is drawn by scaling the unit square, so the matrix IS the placement.
                    s.extend_from_slice(
                        format!(
                            "q {:.3} 0 0 {:.3} {:.3} {:.3} cm /Im{} Do Q\n",
                            w,
                            h,
                            x,
                            flip(y + h),
                            idx
                        )
                        .as_bytes(),
                    );
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
            let mut body = format!("<< /Length {} >>\nstream\n", s.len()).into_bytes();
            body.extend_from_slice(&s);
            body.extend_from_slice(b"endstream");
            obj(&mut out, &mut offsets, first_content + i, &body);
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
            format!("trailer\n<< /Size {} /Root 1 0 R /Info << /Title (", n_objects + 1).as_bytes(),
        );
        out.extend_from_slice(&pdf_str(&self.title));
        out.extend_from_slice(
            format!(
                ") /Producer (SIMLUX {}) >> >>\nstartxref\n{xref}\n%%EOF\n",
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
        assert_eq!(pdf_str("Smith (Ltd)"), b"Smith \\(Ltd\\)".to_vec());
        assert_eq!(pdf_str(r"C:\plans"), br"C:\\plans".to_vec());
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


    /// A NON-ASCII HEADING REACHES THE FILE AS THE BYTES A READER EXPECTS.
    ///
    /// The reported symptom was a page reading "Illuminance Â– false colour". The heading is right,
    /// the escaping is right, and the encoding table is right — what was wrong is that the content
    /// stream was assembled as a Rust `String` and written out as UTF-8, so every WinAnsi byte
    /// above 127 became two. This drives the WHOLE writer, because that is where the fault was:
    /// checking `pdf_str` alone would have passed throughout.
    #[test]
    fn a_dash_in_a_heading_is_one_byte_in_the_file() {
        let mut d = Doc::new(A4, "Gym — level 2");
        d.pages.push(Page {
            items: vec![Item::Text {
                x: 40.0,
                y: 40.0,
                size: 12.0,
                font: Font::Bold,
                rgb: [0, 0, 0],
                align: Align::Left,
                text: "Illuminance — false colour".into(),
            }],
        });
        let out = d.write();
        let want: Vec<u8> = {
            let mut v = b"Illuminance ".to_vec();
            v.push(0x96);
            v.extend_from_slice(b" false colour");
            v
        };
        assert!(
            out.windows(want.len()).any(|w| w == want.as_slice()),
            "the heading is not in the file as WinAnsi bytes",
        );
        // And the UTF-8 form must NOT be there — that is exactly what the reader showed as "Â–".
        let utf8 = "Illuminance — false colour".as_bytes();
        assert!(
            !out.windows(utf8.len()).any(|w| w == utf8),
            "the heading went in as UTF-8, which reads as mojibake",
        );
        // The title in the trailer goes the same way.
        let title: Vec<u8> = {
            let mut v = b"Gym ".to_vec();
            v.push(0x96);
            v.extend_from_slice(b" level 2");
            v
        };
        assert!(
            out.windows(title.len()).any(|w| w == title.as_slice()),
            "the document title was not encoded the same way",
        );
    }

    /// THE STREAM LENGTH COUNTS THE BYTES THAT ARE THERE. `/Length` is how a reader knows where the
    /// stream ends; a count taken before the encoding changed the byte count would truncate the
    /// last operators on the page, losing whatever was drawn last.
    #[test]
    fn the_stream_length_matches_the_stream() {
        let mut d = Doc::new(A4, "x");
        d.pages.push(Page {
            items: vec![Item::Text {
                x: 10.0,
                y: 10.0,
                size: 9.0,
                font: Font::Regular,
                rgb: [0, 0, 0],
                align: Align::Left,
                text: "40° · 2 × 3".into(),
            }],
        });
        let out = d.write();
        let text = String::from_utf8_lossy(&out).into_owned();
        let len: usize = text
            .split("/Length ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|s| s.trim().parse().ok())
            .expect("a stream length");
        let start = out.windows(8).position(|w| w == b"stream\n\x71").map(|i| i + 7);
        let start = start.or_else(|| out.windows(7).position(|w| w == b"stream\n").map(|i| i + 7))
            .expect("a stream");
        let end = out
            .windows(9)
            .position(|w| w == b"endstream")
            .expect("the stream must be closed");
        assert_eq!(end - start, len, "/Length says {len}, the stream holds {}", end - start);
    }
    /// A CHARACTER WITH NO CODE POINT BECOMES A QUESTION MARK, not mojibake. Raw UTF-8 in a
    /// WinAnsi string comes out as two wrong glyphs, which reads as a corrupt file; `?` reads as a
    /// substitution.
    #[test]
    fn text_outside_the_encoding_is_substituted() {
        // ONE BYTE PER CHARACTER, which is the whole of it: `°` is 0xB0 and nothing else. A
        // two-byte answer here is the mojibake that shipped.
        assert_eq!(pdf_str("36°"), vec![b'3', b'6', 0xb0]);
        assert_eq!(pdf_str("a·b"), vec![b'a', 0xb7, b'b']);
        assert_eq!(pdf_str("日"), vec![b'?'], "a substitution, not a deletion");
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
