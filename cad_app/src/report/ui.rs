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
) -> Action {
    let mut act = Action::default();
    *page = (*page).min(doc.pages.len().saturating_sub(1));

    egui::Window::new("Report")
        .id(egui::Id::new("report_dialog"))
        .open(open)
        .default_size(egui::vec2(980.0, 680.0))
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
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Image").small().weak());
                                let cur = opt
                                    .cover_image
                                    .and_then(|i| opt.images.get(i))
                                    .map(|i| i.caption_or_file())
                                    .unwrap_or_else(|| "none".to_string());
                                egui::ComboBox::from_id_salt("cover_img")
                                    .selected_text(cur)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut opt.cover_image, None, "none");
                                        for i in 0..opt.images.len() {
                                            let label = opt.images[i].caption_or_file();
                                            ui.selectable_value(&mut opt.cover_image, Some(i), label);
                                        }
                                    });
                            });
                        });

                        ui.add_space(8.0);
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
                        let mut drop_band: Option<usize> = None;
                        for i in 0..opt.scale.bands.len() {
                            ui.horizontal(|ui| {
                                if ui.small_button("✕").clicked() {
                                    drop_band = Some(i);
                                }
                                ui.add(
                                    egui::DragValue::new(&mut opt.scale.bands[i])
                                        .speed(5.0)
                                        .range(1.0..=100_000.0)
                                        .suffix(" lx"),
                                );
                            });
                        }
                        if let Some(i) = drop_band {
                            opt.scale.bands.remove(i);
                        }
                        // Out of order the bands would draw as overlapping blocks with their
                        // labels crossing, so they are kept sorted rather than validated later.
                        opt.scale
                            .bands
                            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

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

                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Save to").strong());
                        ui.horizontal(|ui| {
                            if ui.button("📂 Folder…").clicked() {
                                act.browse_dir = true;
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut opt.file_stem)
                                    .desired_width(150.0)
                                    .hint_text("file name"),
                            );
                        });
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
                    let avail = ui.available_size();
                    let (resp, painter) = ui.allocate_painter(
                        egui::vec2(avail.x.max(200.0), (avail.y - 34.0).max(200.0)),
                        egui::Sense::hover(),
                    );
                    painter.rect_filled(resp.rect, 0.0, egui::Color32::from_gray(40));
                    paint_page(&painter, resp.rect.shrink(8.0), doc, *page, tex);

                    ui.add_space(4.0);
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
