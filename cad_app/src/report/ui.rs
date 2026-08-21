//! The report dialog, and the preview that paints the real pages.
//!
//! THE PREVIEW IS THE DOCUMENT. It paints the same [`super::pdf::Page`] list the writer emits, so
//! what is on screen is not an impression of the report — it is the report, drawn with a different
//! painter. A preview built separately would drift from the output, and the first time it did
//! nobody would know which one was wrong.

use super::options::{Format, Options, PageSize, Section};
use super::pdf::{Align, Doc, Font, Item};

/// What the dialog asks the app to do once the frame is over.
#[derive(Default)]
pub struct Action {
    /// Write the report.
    pub save: bool,
    /// Choose the output folder.
    pub browse_dir: bool,
    /// Add render images.
    pub add_images: bool,
    /// Drop this image.
    pub remove_image: Option<usize>,
    /// Add logo images — a list of their own, not the renders.
    pub add_logos: bool,
    /// Add a cover picture — a list of its own, not the renders and not the logos.
    pub add_cover: bool,
    /// Drop this cover picture. A list that can only grow is a list with a mistake in it for ever.
    pub remove_cover: Option<usize>,
    /// Drop this logo.
    pub remove_logo: Option<usize>,
    /// Take the current path-tracer render as an image.
    pub capture_render: bool,
}

/// Paint one page into `rect`, scaled to fit and centred.
///
/// `tex` supplies a texture per image index, when one has been loaded — a preview that showed grey
/// boxes where the renders go would not answer the question the preview is for.
pub fn paint_page(
    painter: &egui::Painter,
    rect: egui::Rect,
    doc: &Doc,
    page: usize,
    tex: &[Option<egui::TextureHandle>],
) {
    let Some(pg) = doc.pages.get(page) else { return };
    let k = (rect.width() / doc.width as f32).min(rect.height() / doc.height as f32);
    let pw = doc.width as f32 * k;
    let ph = doc.height as f32 * k;
    let org = egui::pos2(rect.center().x - pw * 0.5, rect.min.y);
    let paper = egui::Rect::from_min_size(org, egui::vec2(pw, ph));

    painter.rect_filled(paper, 2.0, egui::Color32::WHITE);
    painter.rect_stroke(paper, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(120)));
    let clip = painter.with_clip_rect(paper);

    let at = |x: f64, y: f64| egui::pos2(org.x + x as f32 * k, org.y + y as f32 * k);
    let c32 = |c: [u8; 3]| egui::Color32::from_rgb(c[0], c[1], c[2]);

    for it in &pg.items {
        match it {
            Item::Rect { x, y, w, h, fill } => {
                clip.rect_filled(
                    egui::Rect::from_min_size(at(*x, *y), egui::vec2(*w as f32 * k, *h as f32 * k)),
                    0.0,
                    c32(*fill),
                );
            }
            // THE PREVIEW PAINTS THE SAME POLYGONS THE PDF DOES. egui can fill a CONVEX polygon and
            // nothing else, and a contour band is reliably concave, so each ring is triangulated
            // by ear clipping — which handles any simple polygon.
            //
            // EACH RING ON ITS OWN, which means this cannot render a hole. That is not a gap to
            // be filled in later, it is the contract: nothing the layout emits carries more than
            // one ring, and `no_drawing_needs_a_fill_rule_the_preview_lacks` holds it to that.
            // The first version DID emit a two-ring shape — the plot rectangle with the room as an
            // even-odd hole — and the result was a correct PDF beside a preview with the entire
            // room painted white. A comment here claimed painting order made the rule unnecessary.
            // It did not.
            Item::Poly { rings, fill } => {
                for r in rings {
                    let pts: Vec<egui::Pos2> =
                        r.iter().map(|(x, y)| at(*x, *y)).collect();
                    for tri in ear_clip(&pts) {
                        clip.add(egui::Shape::convex_polygon(
                            tri.to_vec(),
                            c32(*fill),
                            egui::Stroke::NONE,
                        ));
                    }
                }
            }
            Item::Frame { x, y, w, h, rgb, width } => {
                clip.rect_stroke(
                    egui::Rect::from_min_size(at(*x, *y), egui::vec2(*w as f32 * k, *h as f32 * k)),
                    0.0,
                    egui::Stroke::new((*width as f32 * k).max(0.5), c32(*rgb)),
                );
            }
            Item::Line { x1, y1, x2, y2, rgb, width } => {
                clip.line_segment(
                    [at(*x1, *y1), at(*x2, *y2)],
                    egui::Stroke::new((*width as f32 * k).max(0.4), c32(*rgb)),
                );
            }
            Item::Text { x, y, size, font, rgb, align, text } => {
                let px = *size as f32 * k;
                // Below about four pixels a glyph is a smudge that costs a lot to lay out and
                // says nothing. The page still shows WHERE the text is, via everything around it.
                if px < 3.5 {
                    continue;
                }
                let fid = match font {
                    Font::Bold => egui::FontId::new(px, egui::FontFamily::Proportional),
                    Font::Regular => egui::FontId::new(px, egui::FontFamily::Proportional),
                };
                let anchor = match align {
                    Align::Left => egui::Align2::LEFT_BOTTOM,
                    Align::Right => egui::Align2::RIGHT_BOTTOM,
                    Align::Centre => egui::Align2::CENTER_BOTTOM,
                };
                clip.text(at(*x, *y), anchor, text, fid, c32(*rgb));
            }
            Item::Image { x, y, w, h, idx } => {
                let r = egui::Rect::from_min_size(
                    at(*x, *y),
                    egui::vec2(*w as f32 * k, *h as f32 * k),
                );
                match tex.get(*idx).and_then(|t| t.as_ref()) {
                    Some(t) => {
                        clip.image(
                            t.id(),
                            r,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    None => {
                        clip.rect_filled(r, 0.0, egui::Color32::from_gray(225));
                        clip.rect_stroke(r, 0.0, egui::Stroke::new(1.0, egui::Color32::from_gray(160)));
                    }
                }
            }
        }
    }
}

/// The dialog. Edits `opt` in place and returns what it wants done.
#[allow(clippy::too_many_arguments)]
pub fn window_ui(
    ctx: &egui::Context,
    open: &mut bool,
    opt: &mut Options,
    doc: &Doc,
    page: &mut usize,
    tex: &[Option<egui::TextureHandle>],
    can_capture: bool,
    // The brightest value the room reached — what "auto" means, shown so the number is not a
    // mystery when it is the one in force.
    room_max: f64,
    // The palette the app is showing, so an untouched swatch shows the colour the report will
    // ACTUALLY use rather than a placeholder. A picker whose swatches do not match the plot beside
    // them is worse than no picker.
    ramp: fn(f32) -> (f32, f32, f32),
    // THE CALCULATION THESE PAGES ARE BUILT FROM NO LONGER DESCRIBES THE SCENE.
    //
    // Said HERE, at the moment somebody is about to turn it into a document that leaves the
    // building. A report is the one output of this app that reaches a client, and a lux figure for
    // a layout that has since changed is indistinguishable, on paper, from one that is right.
    stale: bool,
) -> Action {
    let mut act = Action::default();
    *page = (*page).min(doc.pages.len().saturating_sub(1));

    egui::Window::new("Report")
        .id(egui::Id::new("report_dialog"))
        .open(open)
        .default_size(egui::vec2(980.0, 680.0))
        // A BACKSTOP, not the mechanism. The preview reserves room for the buttons below it, so
        // this should never bind; it is here so that a font size, a longer path or a warning line
        // nobody anticipated cannot push the dialog past the bottom of the display.
        .max_height((ctx.screen_rect().height() - 24.0).max(320.0))
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                // ---- left: what goes in it ----
                ui.vertical(|ui| {
                    ui.set_width(330.0);
                    egui::ScrollArea::vertical().id_salt("report_opts").show(ui, |ui| {
                        ui.label(egui::RichText::new("Format").strong());
                        ui.horizontal(|ui| {
                            for f in [Format::Pdf, Format::Html] {
                                if ui.selectable_label(opt.format == f, f.label()).clicked() {
                                    opt.format = f;
                                }
                            }
                            ui.add_space(10.0);
                            ui.add_enabled_ui(opt.format == Format::Pdf, |ui| {
                                for p in [PageSize::A4, PageSize::Letter] {
                                    if ui.selectable_label(opt.page == p, p.label()).clicked() {
                                        opt.page = p;
                                    }
                                }
                            });
                        });
                        if opt.format == Format::Html {
                            ui.label(
                                egui::RichText::new(
                                    "HTML has no pages — the cover, header, footer and page \
                                     numbers below apply to the PDF.",
                                )
                                .small()
                                .weak(),
                            );
                        }

                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Cover").strong());
                        ui.checkbox(&mut opt.cover, "Cover page");
                        ui.add_enabled_ui(opt.cover, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Project").small().weak());
                                ui.add(
                                    egui::TextEdit::singleline(&mut opt.title)
                                        .desired_width(220.0)
                                        .hint_text("project name"),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Line 2").small().weak());
                                ui.add(
                                    egui::TextEdit::singleline(&mut opt.subtitle)
                                        .desired_width(220.0)
                                        .hint_text("optional second line"),
                                );
                            });
                            // ITS OWN LIST, AND ITS OWN BUTTON. Reported as "the cover page image
                            // doesnt have a dedicated add image option. it taken from render, it
                            // also needs a dedicated add option" — the same complaint the logos
                            // got. Choosing a cover meant adding the picture as a RENDER first,
                            // where it then appeared full width on the renders page whether it
                            // belonged there or not.
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Image").small().weak());
                                let cur = opt
                                    .cover_image
                                    .and_then(|i| opt.covers.get(i))
                                    .map(|i| i.caption_or_file())
                                    .unwrap_or_else(|| "none".to_string());
                                egui::ComboBox::from_id_salt("cover_img")
                                    .selected_text(cur)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut opt.cover_image, None, "none");
                                        for i in 0..opt.covers.len() {
                                            let label = opt.covers[i].caption_or_file();
                                            ui.selectable_value(&mut opt.cover_image, Some(i), label);
                                        }
                                    });
                                if ui.small_button("＋ Add…").clicked() {
                                    act.add_cover = true;
                                }
                                if let Some(i) = opt.cover_image {
                                    if ui.small_button("✕").on_hover_text("Drop it").clicked() {
                                        act.remove_cover = Some(i);
                                    }
                                }
                            });
                            if opt.covers.is_empty() {
                                ui.label(
                                    egui::RichText::new(
                                        "No cover image yet — Add… puts one here, not on the \
                                         renders page.",
                                    )
                                    .small()
                                    .weak(),
                                );
                            }
                        });

                        // THE SAME EDITOR THE SIMLUX WINDOW SHOWS — see `scale_editor_ui`.
                        scale_editor_ui(ui, opt, room_max, ramp);

                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Header & footer").strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut opt.header)
                                .desired_width(300.0)
                                .hint_text("header — practice, project, revision…"),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut opt.footer)
                                .desired_width(300.0)
                                .hint_text("footer"),
                        );
                        ui.checkbox(&mut opt.page_numbers, "Page numbers");
                        // THE SIZE IS STATED, because a logo is prepared before it is chosen and
                        // "it came out tiny" is the alternative to saying so. It is a BOX: the
                        // image keeps its proportions inside it, so a tall logo is 24 pt high and
                        // narrow rather than squashed.
                        ui.label(
                            egui::RichText::new(format!(
                                "Logos fit a {:.0} × {:.0} pt box ({:.0} × {:.0} mm, about {} × {} \
                                 px at 150 dpi). Wider or taller is scaled down, never stretched.",
                                crate::report::layout::LOGO_W,
                                crate::report::layout::LOGO_H,
                                crate::report::layout::LOGO_W * 25.4 / 72.0,
                                crate::report::layout::LOGO_H * 25.4 / 72.0,
                                (crate::report::layout::LOGO_W * 150.0 / 72.0) as i32,
                                (crate::report::layout::LOGO_H * 150.0 / 72.0) as i32,
                            ))
                            .small()
                            .weak(),
                        );
                        for (label, slot) in [("Header logo", 0usize), ("Footer logo", 1usize)] {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(label).small().weak());
                                let cur = if slot == 0 { opt.header_image } else { opt.footer_image };
                                let text = cur
                                    .and_then(|i| opt.logos.get(i))
                                    .map(|i| i.caption_or_file())
                                    .unwrap_or_else(|| "none".to_string());
                                let mut pick = cur;
                                egui::ComboBox::from_id_salt(("logo", slot))
                                    .selected_text(text)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut pick, None, "none");
                                        for i in 0..opt.logos.len() {
                                            let l = opt.logos[i].caption_or_file();
                                            ui.selectable_value(&mut pick, Some(i), l);
                                        }
                                    });
                                if slot == 0 {
                                    opt.header_image = pick;
                                } else {
                                    opt.footer_image = pick;
                                }
                            });
                        }
                        // THE LOGOS ARE THEIR OWN LIST. They used to share the renders list, so a
                        // header logo had to be added as a render first — where it then appeared,
                        // full width, on the renders page.
                        ui.horizontal(|ui| {
                            if ui.button("＋ Add logo…").clicked() {
                                act.add_logos = true;
                            }
                            if opt.logos.is_empty() {
                                ui.label(
                                    egui::RichText::new("no logos loaded").small().weak(),
                                );
                            }
                        });
                        for i in 0..opt.logos.len() {
                            ui.horizontal(|ui| {
                                if ui.small_button("✕").clicked() {
                                    act.remove_logo = Some(i);
                                }
                                let hint = short(&opt.logos[i].path);
                                ui.add(
                                    egui::TextEdit::singleline(&mut opt.logos[i].caption)
                                        .desired_width(180.0)
                                        .hint_text(hint),
                                );
                            });
                        }


                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Sections").strong());
                            ui.label(
                                egui::RichText::new("— tick to include, ▲▼ to reorder")
                                    .small()
                                    .weak(),
                            );
                        });
                        // Listed in the DOCUMENT's order, with the off ones after, so the list
                        // reads as the report reads.
                        let mut order: Vec<Section> = opt.sections.clone();
                        for s in Section::all() {
                            if !order.contains(&s) {
                                order.push(s);
                            }
                        }
                        let mut mv: Option<(Section, i32)> = None;
                        for s in order {
                            ui.horizontal(|ui| {
                                let mut on = opt.has(s);
                                if ui.checkbox(&mut on, s.label()).changed() {
                                    opt.set(s, on);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_enabled_ui(opt.has(s), |ui| {
                                            if ui.small_button("▼").clicked() {
                                                mv = Some((s, 1));
                                            }
                                            if ui.small_button("▲").clicked() {
                                                mv = Some((s, -1));
                                            }
                                        });
                                    },
                                );
                            });
                        }
                        if let Some((s, d)) = mv {
                            opt.move_section(s, d);
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Renders").strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("＋ Add…").clicked() {
                                    act.add_images = true;
                                }
                                if can_capture && ui.button("Capture").on_hover_text(
                                    "Take the current path-traced render as a report image",
                                ).clicked() {
                                    act.capture_render = true;
                                }
                            });
                        });
                        if opt.images.is_empty() {
                            ui.label(
                                egui::RichText::new("No images. Add PNG/JPG renders to fill the \
                                                     Renders page.")
                                    .small()
                                    .weak(),
                            );
                        }
                        for i in 0..opt.images.len() {
                            ui.horizontal(|ui| {
                                if ui.small_button("✕").clicked() {
                                    act.remove_image = Some(i);
                                }
                                let hint = short(&opt.images[i].path);
                                ui.add(
                                    egui::TextEdit::singleline(&mut opt.images[i].caption)
                                        .desired_width(200.0)
                                        .hint_text(hint),
                                );
                            });
                        }

                    });
                });

                ui.separator();

                // ---- right: the pages themselves ----
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Preview").strong());
                        if doc.pages.len() > 1 {
                            if ui.small_button("◀").clicked() {
                                *page = page.saturating_sub(1);
                            }
                            ui.label(format!("{} / {}", *page + 1, doc.pages.len()));
                            if ui.small_button("▶").clicked() && *page + 1 < doc.pages.len() {
                                *page += 1;
                            }
                        } else {
                            ui.label(format!("{} page", doc.pages.len()));
                        }
                        if opt.format == Format::Html {
                            ui.label(
                                egui::RichText::new("(the PDF layout — HTML flows)").small().weak(),
                            );
                        }
                    });
                    if stale {
                        ui.label(
                            egui::RichText::new(
                                "⚠ These pages are built from a calculation that is OUT OF DATE — \
                                 the lights, the model or the settings have changed since it ran. \
                                 Recalculate before issuing this.",
                            )
                            .small()
                            .strong()
                            .color(egui::Color32::from_rgb(235, 140, 90)),
                        );
                    }
                    // A SIZE THAT DOES NOT DEPEND ON THE WINDOW THE PREVIEW IS IN.
                    //
                    // Reported as: "when the calculation preview sort of loads and expands as its
                    // opened. its looks very buggy." It was a feedback loop, and not a settling
                    // one: this took `ui.available_size()`, the window sized itself to hold its
                    // contents, so every frame the preview grew to fill the window and the window
                    // grew to fit the preview — measured at twelve pixels a frame, for as long as
                    // the dialog stayed open. From outside, a panel inflating without end after it
                    // appears reads as a rendering fault rather than as layout.
                    //
                    // Breaking the loop means sizing it from something already known before any
                    // layout happens. Two such things: the PAGE, whose proportions the preview
                    // should have anyway — a page letterboxed inside a box shaped by leftover space
                    // shows a band of nothing that reads as part of the document — and the SCREEN,
                    // which decides how much of it a preview may reasonably take.
                    let aspect = if doc.width > 0.0 { doc.height / doc.width } else { 1.414 };
                    let screen = ui.ctx().screen_rect().size();
                    // Height first, because a page is taller than it is wide and height is the
                    // scarcer dimension; then the width that height implies, pulled back if it
                    // would crowd the options column beside it.
                    //
                    // THE ROWS UNDERNEATH ARE RESERVED FIRST. Reported as "theres no option to
                    // export the report" — and the option was there, it had simply been pushed off
                    // the bottom of the screen. The folder, the file name and the Save button sit
                    // below the preview, so a preview that takes whatever height it likes takes
                    // theirs, and a dialog whose Save button is past the edge of the display is a
                    // report dialog that cannot produce a report. The growth bug above made that
                    // certain to happen eventually; this makes it impossible however tall the
                    // preview would otherwise want to be.
                    const BELOW: f64 = 190.0;
                    let ph =
                        ((screen.y as f64 * 0.72).min(screen.y as f64 - BELOW)).clamp(220.0, 1100.0);
                    let pw = (ph / aspect).min(screen.x as f64 * 0.42).max(180.0);
                    let (resp, painter) = ui.allocate_painter(
                        egui::vec2(pw as f32, (pw * aspect) as f32),
                        egui::Sense::hover(),
                    );
                    painter.rect_filled(resp.rect, 0.0, egui::Color32::from_gray(40));
                    paint_page(&painter, resp.rect.shrink(8.0), doc, *page, tex);
                    ui.add_space(4.0);
                    // WHERE IT GOES, BESIDE THE BUTTON THAT NEEDS IT. This sat at the bottom of
                    // the options column, below the fold — so the Save button said "Choose a
                    // folder first" about a control the user could not see without scrolling.
                    ui.horizontal(|ui| {
                        if ui.button("📂  Folder…").clicked() {
                            act.browse_dir = true;
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut opt.file_stem)
                                .desired_width(160.0)
                                .hint_text("file name"),
                        );
                        ui.label(
                            egui::RichText::new(if opt.out_dir.trim().is_empty() {
                                "choose a folder".to_string()
                            } else {
                                opt.out_path().to_string_lossy().into_owned()
                            })
                            .small()
                            .weak(),
                        );
                    });
                    ui.horizontal(|ui| {

                        let ready = !opt.out_dir.trim().is_empty();
                        ui.add_enabled_ui(ready, |ui| {
                            if ui
                                .add(
                                    egui::Button::new(format!("  Save {}  ", opt.format.label()))
                                        .fill(egui::Color32::from_rgb(30, 80, 45)),
                                )
                                .clicked()
                            {
                                act.save = true;
                            }
                        });
                        if !ready {
                            ui.label(
                                egui::RichText::new("Choose a folder first")
                                    .small()
                                    .color(egui::Color32::from_rgb(220, 150, 90)),
                            );
                        }
                    });
                });
            });
        });
    act
}

fn short(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

impl super::options::ReportImage {
    /// What to call this image in a menu: its caption, or failing that its file name.
    pub fn caption_or_file(&self) -> String {
        if self.caption.trim().is_empty() {
            short(&self.path)
        } else {
            self.caption.trim().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::options::ReportImage;

    /// AN IMAGE IS NAMED BY ITS CAPTION, and by its file when it has none — a menu of blank rows
    /// is one nobody can choose a cover from.
    #[test]
    fn an_image_always_has_something_to_call_it() {
        let mut im = ReportImage {
            path: "D:/renders/from the door.png".into(),
            caption: String::new(),
            jpeg: None,
        };
        assert_eq!(im.caption_or_file(), "from the door.png");
        im.caption = "  Entrance  ".into();
        assert_eq!(im.caption_or_file(), "Entrance", "the caption wins, trimmed");
    }
}

/// Triangulate a simple polygon by EAR CLIPPING.
///
/// egui can fill a CONVEX polygon and nothing else, and a contour band is reliably concave — a
/// pool of light with a bite out of it, an L-shaped room, a band wrapping round a brighter one.
/// Handing such a ring to `convex_polygon` does not fail, it draws the convex hull, which paints
/// over the very shape the contour was computed to show.
///
/// O(n²) and that is fine: the rings here are the output of a contour trace, tens to low hundreds
/// of points, and this runs only when the preview is rebuilt — which, since the cache, is when
/// something actually changed rather than sixty times a second.
///
/// A ring that cannot be reduced — self-intersecting, or degenerate to a sliver — stops early
/// rather than looping: an unfillable polygon is a missing patch of colour, and a hung preview is
/// a hung app.
fn ear_clip(pts: &[egui::Pos2]) -> Vec<[egui::Pos2; 3]> {
    let mut out = Vec::new();
    if pts.len() < 3 {
        return out;
    }
    let area2 = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
        (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
    };
    // Work in a known winding, so "convex vertex" has one meaning below.
    let mut v: Vec<egui::Pos2> = pts.to_vec();
    let signed: f32 = v
        .windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum::<f32>()
        + (v[v.len() - 1].x * v[0].y - v[0].x * v[v.len() - 1].y);
    if signed < 0.0 {
        v.reverse();
    }

    let mut guard = v.len() * v.len() + 8;
    while v.len() > 3 && guard > 0 {
        guard -= 1;
        let n = v.len();
        let mut clipped = false;
        for i in 0..n {
            let (a, b, c) = (v[(i + n - 1) % n], v[i], v[(i + 1) % n]);
            if area2(a, b, c) <= 0.0 {
                continue; // reflex or collinear — not an ear
            }
            // No other vertex may lie inside the candidate ear.
            let inside = (0..n).filter(|k| ![(i + n - 1) % n, i, (i + 1) % n].contains(k)).any(|k| {
                let p = v[k];
                area2(a, b, p) >= 0.0 && area2(b, c, p) >= 0.0 && area2(c, a, p) >= 0.0
            });
            if inside {
                continue;
            }
            out.push([a, b, c]);
            v.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break; // not a simple polygon — draw what was clipped and stop
        }
    }
    if v.len() == 3 {
        out.push([v[0], v[1], v[2]]);
    }
    out
}

/// THE FALSE-COLOUR SCALE EDITOR — one editor, wherever it is shown.
///
/// Reported as: *"linking the false color of the report with the simlux window. looks like it not
/// wired in."* It was not. The overlay read `report_opts` correctly, but the SIMLUX Display menu
/// still carried its OWN controls — a separate "auto / pin top" and a palette picker — over state
/// nothing drew from any more. Turning them changed nothing, which is indistinguishable from
/// broken.
///
/// So there is one editor and one set of settings, called from the report dialog and from the
/// Display menu alike. Anything changed in either place is changed in both, because there is only
/// one place for it to be changed.
///
/// `ramp` is the fallback palette — what a band with no colour of its own, and a CONTINUOUS scale
/// with no bands at all, are drawn in.
pub fn scale_editor_ui(
    ui: &mut egui::Ui,
    opt: &mut Options,
    room_max: f64,
    ramp: fn(f32) -> (f32, f32, f32),
) {
ui.label(egui::RichText::new("False-colour scale").strong());
ui.horizontal(|ui| {
    let mut pinned = opt.scale.top.is_some();
    if ui.checkbox(&mut pinned, "Pin top").changed() {
        // Pinning starts from whatever the room reached, so the first
        // click changes nothing and the number can be edited from there.
        opt.scale.top = if pinned { Some(room_max.max(1.0)) } else { None };
    }
    if let Some(t) = opt.scale.top.as_mut() {
        ui.add(
            egui::DragValue::new(t)
                .speed(10.0)
                .range(1.0..=100_000.0)
                .suffix(" lx"),
        );
    } else {
        ui.label(
            egui::RichText::new(format!("auto — {room_max:.0} lx"))
                .small()
                .weak(),
        );
    }
});
ui.horizontal(|ui| {
    let mut banded = !opt.scale.bands.is_empty();
    if ui
        .checkbox(&mut banded, "Bands")
        .on_hover_text(
            "Discrete steps rather than a gradient — which parts of the \
             room meet which requirement",
        )
        .changed()
    {
        opt.scale.bands =
            if banded { vec![25.0, 100.0, 300.0, 500.0] } else { Vec::new() };
    }
    if !opt.scale.bands.is_empty() && ui.small_button("＋").clicked() {
        let last = opt.scale.bands.last().copied().unwrap_or(0.0);
        opt.scale.bands.push(last * 2.0 + 1.0);
    }
});
// A COLOUR PER BAND, from a wheel, kept with the settings.
//
// Asked for as "in the band add a band color picker … so this color band
// will come for all future report generation."
//
// The list is filled from the PALETTE the first time it is touched, not
// left blank: a swatch that does not show the colour the report will
// actually use is a picker that lies, and the first thing anybody does is
// compare it against the plot beside it.
//
// NOTHING IS WRITTEN UNTIL SOMEBODY PICKS A COLOUR. The obvious
// implementation fills the list from the palette as soon as the dialog
// opens, so the swatches have something to point at — and then merely
// LOOKING at the dialog has silently made every band an explicit choice,
// saved it to the settings, and cut the report off from the palette for
// ever. Changing the app's colour scheme afterwards would do nothing and
// nothing would say why.
//
// So each swatch shows the colour the report will actually use, edits a
// COPY, and only writes back when it changed.
let top = opt.scale.top_lx(room_max);
let edges = opt.scale.edges(room_max);
let n_bands = edges.len().saturating_sub(1);
let shown = |opt: &Options, k: usize| -> [u8; 3] {
    if let Some(c) = opt.band_colours.get(k) {
        return *c;
    }
    let mid = match edges.get(k..k + 2) {
        Some(p) => (p[0] + p[1]) * 0.5,
        None => top,
    };
    let (r, g, b) = ramp((mid / top).clamp(0.0, 1.0) as f32);
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    ]
};
// Writing one band's colour has to make the others explicit too, or the
// list would be short and every band past the end would silently fall back
// to the palette — which is not what "I chose this one" means.
let mut set_band: Option<(usize, [u8; 3])> = None;
if !opt.scale.bands.is_empty() {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Band colours").small().weak());
        if ui
            .small_button("reset")
            .on_hover_text("Back to the palette")
            .clicked()
        {
            opt.band_colours.clear();
        }
    });
    // Band 0 has no threshold row of its own — it is everything BELOW the
    // first step — so it gets a row here, or it could never be recoloured.
    ui.horizontal(|ui| {
        let mut c = shown(opt, 0);
        if ui.color_edit_button_srgb(&mut c).changed() {
            set_band = Some((0, c));
        }
        ui.label(
            egui::RichText::new(format!(
                "0 – {:.0} lx",
                opt.scale.bands.first().copied().unwrap_or(top),
            ))
            .small()
            .weak(),
        );
    });
}
let mut drop_band: Option<usize> = None;
for i in 0..opt.scale.bands.len() {
    ui.horizontal(|ui| {
        if ui.small_button("✕").clicked() {
            drop_band = Some(i);
        }
        // The band STARTING at this threshold, so the swatch sits beside
        // the number that is its floor.
        let mut c = shown(opt, i + 1);
        if ui.color_edit_button_srgb(&mut c).changed() {
            set_band = Some((i + 1, c));
        }
        ui.add(
            egui::DragValue::new(&mut opt.scale.bands[i])
                .speed(5.0)
                .range(1.0..=100_000.0)
                .suffix(" lx"),
        );
    });
}
if let Some((k, c)) = set_band {
    let mut all: Vec<[u8; 3]> =
        (0..n_bands.max(k + 1)).map(|j| shown(opt, j)).collect();
    all[k] = c;
    opt.band_colours = all;
}
if let Some(i) = drop_band {
    opt.scale.bands.remove(i);
    // The colour of the band that has just lost its floor goes with it, or
    // every colour above the gap would shift down by one.
    if i + 1 < opt.band_colours.len() {
        opt.band_colours.remove(i + 1);
    }
}
// Out of order the bands would draw as overlapping blocks with their
// labels crossing, so they are kept sorted rather than validated later.
opt.scale
    .bands
    .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

}
