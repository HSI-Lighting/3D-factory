//! The SIMLUX calculation, as a standalone HTML report.
//!
//! A result that lives only in a panel cannot be sent to a client, checked by a colleague, or filed
//! against a project. This is the same set of numbers written out as one self-contained file: no
//! external CSS, no fonts, no images, so it opens anywhere and survives being emailed.
//!
//! Built as a PURE FUNCTION of the state rather than by writing to a file as the panel draws, so
//! the content can be tested without a GPU, a window, or a filesystem. What the tests then check is
//! the thing that matters about a report: that every number in it came from the calculation, and
//! that a quantity which was not calculated is ABSENT rather than shown as zero.

use cad_light::{CalcPlane, Installation, LuxGrid, Maintenance, SurfaceResult};

/// Everything the report needs, gathered from `LightState` so the writer touches no UI types.
pub struct ReportInput<'a> {
    pub title: String,
    pub grid: &'a LuxGrid,
    pub plane: &'a CalcPlane,
    pub maintenance: Maintenance,
    pub installation: Option<&'a Installation>,
    pub surfaces: &'a [SurfaceResult],
    pub cylindrical_avg: Option<f64>,
    pub eye_height: f32,
    pub room_height: f32,
    /// `(name, reflectance)` per material, in the order the room defines them.
    pub materials: Vec<(String, f32)>,
    /// Fixtures that have no photometric file assigned yet.
    pub unassigned: usize,
    /// The false-colour palette, as the app has it set. A plain `fn` pointer because the report is
    /// rendered from a plain struct and must not borrow the app.
    pub ramp: fn(f32) -> (f32, f32, f32),
    /// Top of the colour scale, lx — the number without which a false-colour plot means nothing.
    pub scale_top: f64,
    /// Whether that top was auto (this room's maximum) or pinned. Stated, because two reports at
    /// different auto-scales are not comparable and nothing else on the page would say so.
    pub scale_auto: bool,
    /// Which cells are inside the room; empty when the plane was not placed on one.
    pub mask: Vec<bool>,
    /// The sections to keep. The title and the headline figures are never dropped — see
    /// `filter_sections`.
    pub sections: Vec<crate::report::Section>,
    /// Render images as `(JPEG bytes, caption)`, embedded as data URIs.
    pub images: Vec<(Vec<u8>, String)>,
    /// What this room is lit WITH, by fitting type.
    pub schedule: Vec<crate::report::layout::ScheduleRow>,
    /// The room outline, in metres — for the layout drawing. Empty when there is none.
    pub poly: Vec<glam::Vec2>,
    /// The fittings standing in it, for the same drawing.
    pub fixtures: Vec<cad_light::Luminaire>,
}

/// Minimal HTML escaping — a room called `Smith & Sons <Ltd>` must not break the document.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Render the report. Self-contained: the returned string IS the file.
pub fn render(inp: &ReportInput) -> String {
    render_all(&inp.title.clone(), std::slice::from_ref(inp))
}

/// SEVERAL ROOMS IN ONE DOCUMENT.
///
/// A calculation produces one result per room, and a report that merged three of them stated an
/// average over ground that is not one space. The head and the closing tags belong to the
/// DOCUMENT; everything between them is written once per room.
pub fn render_all(doc_title: &str, rooms: &[ReportInput]) -> String {
    let Some(first) = rooms.first() else { return String::new() };
    // THE DOCUMENT IS THE PROJECT; each room is a section of it. Titling the file after the first
    // room means a three-room report opens in a browser tab called after one of them.
    let title = if doc_title.trim().is_empty() { first.title.as_str() } else { doc_title.trim() };
    let mut h = String::with_capacity(16 * 1024);
    h.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    h.push_str(&format!("<title>{} — SIMLUX</title>", esc(title)));
    h.push_str(STYLE);
    h.push_str("</head><body><div class=\"wrap\">");
    for (i, inp) in rooms.iter().enumerate() {
        if i > 0 {
            h.push_str("<hr class=\"roomrule\">");
        }
        h.push_str(&body(inp, rooms.len() > 1));
    }
    h.push_str("</div></body></html>");
    h
}

/// One room's worth of the document.
fn body(inp: &ReportInput, named: bool) -> String {
    let g = inp.grid;
    let p = inp.plane;
    let mut h = String::with_capacity(16 * 1024);
    if named {
        h.push_str(&format!("<h1 class=\"room\">{}</h1>", esc(&inp.title)));
    }
    h.push_str(&format!("<h1>{}</h1>", esc(&inp.title)));
    h.push_str(&format!(
        "<p class=\"sub\">SIMLUX {} · maintained values, maintenance factor {:.2}</p>",
        esc(env!("SIMLUX_BUILD")),
        g.maintenance,
    ));

    // ---- headline ---------------------------------------------------------------------------
    mark(&mut h, Section::Summary);
    h.push_str("<div class=\"kpi\">");
    h.push_str(&kpi(&format!("{:.0} lx", g.avg), "average maintained"));
    h.push_str(&kpi(&format!("{:.2}", g.u0()), "uniformity U₀"));
    if let Some(i) = inp.installation {
        h.push_str(&kpi(&format!("{:.2} W/m²", i.power_density), "power density"));
        h.push_str(&kpi(&format!("{} fitting(s)", i.count), "installed"));
    }
    h.push_str("</div>");

    // A result computed from half a layout looks exactly like one computed from all of it.
    if inp.unassigned > 0 {
        h.push_str(&format!(
            "<p class=\"warn\">{} light point(s) have no fitting assigned and emit nothing. \
             These results are for the {} that do.</p>",
            inp.unassigned,
            inp.installation.map(|i| i.count).unwrap_or(0),
        ));
    }

    // ---- the layout, as a drawing ---------------------------------------------------------
    //
    // HTML HAS NO PAGES, so this is not "a page showing the lighting layout" — but the DRAWING is
    // just as useful here, and inline SVG keeps the file the one self-contained thing it has
    // always been. A field says how much light there is; it does not say where the fittings are.
    mark(&mut h, Section::Layout);
    if !inp.fixtures.is_empty() || inp.poly.len() >= 3 {
        h.push_str("<h2>Lighting layout</h2>");
        h.push_str(&layout_svg(inp));
        h.push_str(&format!(
            "<p class=\"note\">{:.2} × {:.2} m · {} fitting(s) · mounting {:.2} m</p>",
            p.width,
            p.depth,
            inp.fixtures.len(),
            inp.fixtures.first().map(|l| l.position.z).unwrap_or(0.0),
        ));
    }

    // ---- conditions -------------------------------------------------------------------------
    mark(&mut h, Section::Materials);
    h.push_str("<h2>Conditions</h2><table>");
    h.push_str(&tr2("Room height", &format!("{:.3} m", inp.room_height)));
    h.push_str(&tr2("Working plane", &format!("{:.3} m", p.origin.z)));
    h.push_str(&tr2(
        "Calculation area",
        &format!("{:.2} × {:.2} m — {:.2} m²", p.width, p.depth, p.width * p.depth),
    ));
    // A uniformity figure without its grid cannot be reproduced by anyone, including us. This is
    // the lesson of the DIALux comparison, where their U₀ could not be reproduced for exactly that
    // reason — so ours always says.
    let (wc, wr) = cad_light::en12464_cells(p.width, p.depth);
    let coarse = p.cols < wc || p.rows < wr;
    h.push_str(&tr2(
        "Calculation grid",
        &format!(
            "{}{}",
            esc(&p.grid_note()),
            if coarse {
                format!(" — coarser than EN 12464-1 asks ({wc} × {wr}); U₀ is optimistic")
            } else {
                String::new()
            }
        ),
    ));
    let m = inp.maintenance;
    h.push_str(&tr2(
        "Maintenance factor",
        &format!(
            "{:.2}  (LLMF {:.2} · LSF {:.2} · LMF {:.2} · RSMF {:.2})",
            m.factor(),
            m.llmf,
            m.lsf,
            m.lmf,
            m.rsmf
        ),
    ));
    for (name, rho) in &inp.materials {
        h.push_str(&tr2(&format!("Reflectance — {}", esc(name)), &format!("{:.0} %", rho * 100.0)));
    }
    h.push_str("</table>");

    // ---- work plane -------------------------------------------------------------------------
    mark(&mut h, Section::WorkingPlane);
    h.push_str("<h2>Working plane</h2><table>");
    h.push_str(&tr2("Average  Ē", &format!("{:.0} lx", g.avg)));
    h.push_str(&tr2("Minimum  E<sub>min</sub>", &format!("{:.0} lx", g.min)));
    h.push_str(&tr2("Maximum  E<sub>max</sub>", &format!("{:.0} lx", g.max)));
    h.push_str(&tr2("Median", &format!("{:.0} lx", g.median())));
    h.push_str(&tr2(
        "10th / 90th percentile",
        &format!("{:.0} / {:.0} lx", g.percentile(10.0), g.percentile(90.0)),
    ));
    h.push_str(&tr2("Uniformity  U₀ = E<sub>min</sub>/Ē", &format!("{:.2}", g.u0())));
    h.push_str(&tr2("Diversity  U₁ = E<sub>min</sub>/E<sub>max</sub>", &format!("{:.2}", g.u1())));
    if let Some(f) = g.direct_fraction() {
        h.push_str(&tr2(
            "Direct / indirect",
            &format!("{:.0} % / {:.0} %", f * 100.0, (1.0 - f) * 100.0),
        ));
    }
    if let Some(ez) = inp.cylindrical_avg {
        h.push_str(&tr2(
            &format!("Cylindrical  E<sub>z</sub> at {:.1} m", inp.eye_height),
            &format!("{ez:.0} lx"),
        ));
    }
    h.push_str("</table>");

    // ---- the FALSE-COLOUR FIELD -----------------------------------------------------------------
    //
    // Asked for as: "add the psudo colors layout in the report too."
    //
    // The colour AND the number in the same cell, which is how DIALux prints it and the only form
    // that is both readable and checkable: a ramp shows the shape of the field at a glance and
    // cannot be read back into values; a table of numbers is exact and shows no shape. Printing
    // them apart makes the reader hold one in their head while looking at the other.
    //
    // The scale is stated WITH the picture. A false-colour plot whose top is unstated says nothing:
    // the same room reads "mostly red" or "mostly blue" depending on a number in a menu.
    let top = inp.scale_top.max(1.0);
    mark(&mut h, Section::Results);
    h.push_str("<h2>Illuminance — false colour</h2>");
    // NOT `.scroll` any more — the plot fits the page rather than running off it. See the CSS.
    h.push_str("<div class=\"fcwrap\"><table class=\"fc\">");
    for r in (0..p.rows).rev() {
        h.push_str("<tr>");
        for c in 0..p.cols {
            let i = (r * p.cols + c) as usize;
            // A cell outside the room is not the room's result: left blank rather than coloured,
            // because colouring it reports illuminance on ground the room does not occupy.
            if inp.mask.get(i).is_some_and(|inside| !inside) {
                h.push_str("<td class=\"fc out\"></td>");
                continue;
            }
            let v = g.values.get(i).copied().unwrap_or(0.0);
            let (rr, gg, bb) = (inp.ramp)((v / top) as f32);
            // Dark text on the bright end of a ramp, light on the dark end — the number has to stay
            // readable whichever palette was chosen.
            let lum = 0.2126 * rr + 0.7152 * gg + 0.0722 * bb;
            let fg = if lum > 0.55 { "#111" } else { "#fff" };
            h.push_str(&format!(
                "<td class=\"fc\" style=\"background:rgb({},{},{});color:{fg}\">{v:.0}</td>",
                (rr * 255.0).round() as u8,
                (gg * 255.0).round() as u8,
                (bb * 255.0).round() as u8,
            ));
        }
        h.push_str("</tr>");
    }
    h.push_str("</table></div>");
    h.push_str("<div class=\"legend\"><span class=\"lgz\">0</span><span class=\"lgbar\">");
    const STEPS: usize = 40;
    for i in 0..STEPS {
        let (rr, gg, bb) = (inp.ramp)(i as f32 / (STEPS - 1) as f32);
        h.push_str(&format!(
            "<i style=\"background:rgb({},{},{})\"></i>",
            (rr * 255.0).round() as u8,
            (gg * 255.0).round() as u8,
            (bb * 255.0).round() as u8,
        ));
    }
    h.push_str(&format!("</span><span class=\"lgz\">{top:.0} lx</span></div>"));
    h.push_str(&format!(
        "<p class=\"note\">Scale 0 – {top:.0} lx ({}).</p>",
        if inp.scale_auto { "auto — this room's maximum" } else { "pinned" },
    ));

    // ---- the field, as numbers ------------------------------------------------------------------
    // The whole grid, not a picture of it: a report has to be checkable, and a colour ramp cannot
    // be read back into numbers.
    mark(&mut h, Section::NumericGrid);
    h.push_str("<h2>Illuminance grid (lx)</h2><div class=\"scroll\"><table class=\"grid\">");
    for r in (0..p.rows).rev() {
        h.push_str("<tr>");
        for c in 0..p.cols {
            let v = g.values.get((r * p.cols + c) as usize).copied().unwrap_or(0.0);
            h.push_str(&format!("<td class=\"g\">{v:.0}</td>"));
        }
        h.push_str("</tr>");
    }
    h.push_str("</table></div>");
    h.push_str("<p class=\"note\">Rows run from the far edge of the plane to the near one, matching the plan.</p>");

    // ---- surfaces ---------------------------------------------------------------------------
    mark(&mut h, Section::Surfaces);
    if !inp.surfaces.is_empty() {
        h.push_str("<h2>Room surfaces</h2><table><tr><th>Surface</th><th class=\"n\">Area</th>");
        h.push_str("<th class=\"n\">Ē</th><th class=\"n\">E<sub>min</sub></th>");
        h.push_str("<th class=\"n\">Luminance</th><th class=\"n\">U₀</th></tr>");
        for s in inp.surfaces {
            h.push_str(&format!(
                "<tr><td>{}</td><td class=\"n\">{:.0} m²</td><td class=\"n\">{:.0} lx</td>\
                 <td class=\"n\">{:.0} lx</td><td class=\"n\">{:.0} cd/m²</td>\
                 <td class=\"n\">{:.2}</td></tr>",
                esc(&s.name),
                s.area_m2,
                s.e_avg,
                s.e_min,
                s.l_avg,
                s.u0,
            ));
        }
        h.push_str("</table>");
        h.push_str(
            "<p class=\"note\">Luminance is ρE/π. EN 12464-1 asks roughly 50 lx on walls and \
             30 lx on the ceiling for an office, each at U₀ ≥ 0.10.</p>",
        );
    }

    // ---- load -------------------------------------------------------------------------------
    mark(&mut h, Section::Installation);
    if let Some(i) = inp.installation {
        h.push_str("<h2>Connected load</h2><table>");
        h.push_str(&tr2("Fittings", &format!("{}", i.count)));
        h.push_str(&tr2("Connected load", &format!("{:.0} W", i.total_watts)));
        h.push_str(&tr2("Installed flux", &format!("{:.0} lm", i.total_lumens)));
        h.push_str(&tr2("Power density", &format!("{:.2} W/m²", i.power_density)));
        if g.avg > 0.0 {
            h.push_str(&tr2(
                "Power density per 100 lx",
                &format!("{:.2} W/m²/100 lx", i.power_density / g.avg * 100.0),
            ));
        }
        h.push_str(&tr2("Installation efficacy", &format!("{:.0} lm/W", i.efficacy)));
        h.push_str("</table>");
        // A density computed from half the fittings is worse than none at all, so say which are
        // missing their data rather than quietly averaging over the rest.
        if i.missing_watts > 0 || i.missing_lumens > 0 {
            h.push_str(&format!(
                "<p class=\"warn\">{} fitting(s) declare no wattage and {} declare no flux in their \
                 photometric file; they are excluded from the figures above.</p>",
                i.missing_watts, i.missing_lumens,
            ));
        }
    }

    // The renders, if any were added and the section is on. Embedded as data URIs so the file
    // stays what it has always been: one self-contained document that survives being emailed.
    mark(&mut h, Section::Schedule);
    h.push_str(&schedule_table(&inp.schedule));

    mark(&mut h, Section::Renders);
    if !inp.images.is_empty() {
        h.push_str("<h2>Renders</h2><div class=\"renders\">");
        for (jpeg, caption) in &inp.images {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
            h.push_str(&format!(
                "<figure><img src=\"data:image/jpeg;base64,{b64}\" alt=\"{}\">{}</figure>",
                esc(caption),
                if caption.trim().is_empty() {
                    String::new()
                } else {
                    format!("<figcaption>{}</figcaption>", esc(caption))
                },
            ));
        }
        h.push_str("</div>");
    }

    // THE CLOSING TAGS ARE NOT A SECTION. Left inside the last marked run they would be dropped
    // along with it, and unticking one box would produce a document that never closes.
    h.push_str(END_MARK);
    filter_sections(h, &inp.sections)
}

use crate::report::Section;

/// The marker a section filter cuts on.
///
/// AN HTML REPORT IS A FLOW, not a tree of pages, and it is written as one long push — which is
/// what makes it a good web page and a poor thing to slice. Rather than restructure a renderer
/// whose every number is already under test, each section is announced by a comment, and the
/// filter keeps the runs the user asked for. The markers are inert in a browser and visible to
/// anyone reading the file, which is more than a CSS class would be.

/// The lighting layout as inline SVG — the room outline and a marker per fitting.
///
/// SVG rather than a raster, so it stays crisp at any zoom and costs a few hundred bytes; inline
/// rather than a file, so the report is still the one thing you can email. Y IS FLIPPED once: a
/// plan reads with +y up, SVG measures down, and a layout printed upside down against its own
/// result is worse than no layout at all.
fn layout_svg(inp: &ReportInput) -> String {
    let p = inp.plane;
    if p.width <= 0.0 || p.depth <= 0.0 {
        return String::new();
    }
    // A viewBox in METRES, so everything below is written in the room's own coordinates and the
    // browser does the scaling.
    let (ox, oy) = (p.origin.x as f64, p.origin.y as f64);
    let (w, d) = (p.width as f64, p.depth as f64);
    let fy = |y: f64| d - (y - oy); // flip
    let mut s = format!(
        "<svg class=\"layout\" viewBox=\"0 0 {w:.3} {d:.3}\" preserveAspectRatio=\"xMidYMid meet\" \
         xmlns=\"http://www.w3.org/2000/svg\">"
    );
    // Stroke widths are in metres too, so they have to be small — 1/400 of the room reads as a
    // hairline at any size the browser picks.
    let hair = (w.max(d) / 400.0).max(0.005);

    if inp.poly.len() >= 3 {
        let pts: Vec<String> = inp
            .poly
            .iter()
            .map(|v| format!("{:.3},{:.3}", v.x as f64 - ox, fy(v.y as f64)))
            .collect();
        s.push_str(&format!(
            "<polygon points=\"{}\" fill=\"none\" stroke=\"#be2828\" stroke-width=\"{:.4}\"/>",
            pts.join(" "),
            hair * 2.0,
        ));
    } else {
        s.push_str(&format!(
            "<rect x=\"0\" y=\"0\" width=\"{w:.3}\" height=\"{d:.3}\" fill=\"none\" \
             stroke=\"#be2828\" stroke-width=\"{:.4}\"/>",
            hair * 2.0,
        ));
    }

    let r = (w.max(d) / 90.0).max(0.05);
    for l in &inp.fixtures {
        let (x, y) = (l.position.x as f64 - ox, fy(l.position.y as f64));
        s.push_str(&format!(
            "<g stroke=\"#1e1e1e\" stroke-width=\"{hw:.4}\">\
             <line x1=\"{:.3}\" y1=\"{y:.3}\" x2=\"{:.3}\" y2=\"{y:.3}\"/>\
             <line x1=\"{x:.3}\" y1=\"{:.3}\" x2=\"{x:.3}\" y2=\"{:.3}\"/></g>\
             <rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" fill=\"none\" \
             stroke=\"#c8963c\" stroke-width=\"{hw:.4}\"/>",
            x - r,
            x + r,
            y - r,
            y + r,
            x - r * 0.66,
            y - r * 0.66,
            r * 1.32,
            r * 1.32,
            hw = hair,
        ));
    }
    s.push_str("</svg>");
    s
}

/// THE LUMINAIRE SCHEDULE — what the room is lit with, by type.
///
/// It was in the PDF and not in the HTML at all, so a report asked for as HTML said what the
/// illuminance was and never what produced it. A lighting report without a schedule cannot be
/// checked, ordered from, or handed to an installer.
fn schedule_table(rows: &[crate::report::layout::ScheduleRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut h = String::from(
        "<h2>Luminaire schedule</h2><table class=\"sched\"><tr><th class=\"n\">Qty</th>\
         <th>Fitting</th><th>Manufacturer</th><th class=\"n\">W</th><th class=\"n\">lm</th>\
         <th class=\"n\">lm/W</th><th class=\"n\">Size</th></tr>",
    );
    let dash = |v: f64, dp: usize| -> String {
        if v > 0.0 {
            format!("{v:.dp$}")
        } else {
            "—".to_string()
        }
    };
    for r in rows {
        // The catalogue number and the lamp belong to the fitting, under its name — a schedule of
        // nine columns on a phone is a schedule nobody reads.
        let mut sub = Vec::new();
        if !r.catalogue.trim().is_empty() && r.catalogue.trim() != r.profile.trim() {
            sub.push(format!("cat. {}", esc(r.catalogue.trim())));
        }
        if !r.lamp.trim().is_empty() {
            sub.push(esc(r.lamp.trim()));
        }
        let (l, wd, ht) = r.size_m;
        h.push_str(&format!(
            "<tr><td class=\"n\">{}</td><td><b>{}</b>{}</td><td>{}</td><td class=\"n\">{}</td>\
             <td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
            r.count,
            esc(&r.profile),
            if sub.is_empty() {
                String::new()
            } else {
                format!("<span class=\"sub\">{}</span>", sub.join(" · "))
            },
            if r.manufacturer.trim().is_empty() { "—".into() } else { esc(r.manufacturer.trim()) },
            dash(r.watts, 1),
            dash(r.lumens, 0),
            r.efficacy().map(|e| format!("{e:.0}")).unwrap_or_else(|| "—".into()),
            if l > 0.0 || wd > 0.0 {
                format!("{:.0} × {:.0} × {:.0} mm", l * 1000.0, wd * 1000.0, ht * 1000.0)
            } else {
                "—".into()
            },
        ));
    }
    let n: usize = rows.iter().map(|r| r.count).sum();
    let watts: f64 = rows.iter().map(|r| r.total_watts()).sum();
    h.push_str(&format!(
        "<tr class=\"tot\"><td class=\"n\">{n}</td><td colspan=\"2\">fitting(s)</td>\
         <td class=\"n\">{watts:.1}</td><td colspan=\"3\">connected</td></tr>",
    ));
    h.push_str("</table>");
    h
}

const END_MARK: &str = "<!--SEC:END-->";

fn mark(h: &mut String, s: Section) {
    h.push_str(&format!("<!--SEC:{:?}-->", s));
}

/// Keep only the marked runs whose section is selected.
///
/// Everything BEFORE the first marker is the title and the headline figures, which are the report
/// identifying itself and are never dropped — a page with no title is not a shorter report, it is
/// an anonymous one.
fn filter_sections(html: String, keep: &[Section]) -> String {
    let Some(first) = html.find("<!--SEC:") else { return html };
    let mut out = String::with_capacity(html.len());
    out.push_str(&html[..first]);
    let mut rest = &html[first..];
    while let Some(start) = rest.find("<!--SEC:") {
        let name_at = start + 8;
        let Some(end_tag) = rest[name_at..].find("-->") else { break };
        let name = &rest[name_at..name_at + end_tag];
        let body_at = name_at + end_tag + 3;
        let body_end = rest[body_at..].find("<!--SEC:").map(|i| body_at + i).unwrap_or(rest.len());
        // "END" is not a section: it marks the closing tags, which are part of the document
        // rather than part of any of its contents.
        let on = name == "END" || keep.iter().any(|s| format!("{s:?}") == name);
        if on {
            out.push_str(&rest[body_at..body_end]);
        }
        rest = &rest[body_end..];
    }
    out
}

fn kpi(value: &str, label: &str) -> String {
    format!("<div><b>{}</b><span>{}</span></div>", esc(value), esc(label))
}

fn tr2(k: &str, v: &str) -> String {
    format!("<tr><td>{k}</td><td class=\"v\">{v}</td></tr>")
}

const STYLE: &str = r#"<style>
:root{--bg:#fff;--fg:#14181f;--muted:#5b6472;--line:#e3e7ee;--panel:#f7f9fc;--warn:#8a5a00;--warnbg:#fdf3e0}
@media (prefers-color-scheme:dark){:root{--bg:#0e1116;--fg:#e6eaf1;--muted:#98a2b3;--line:#232a35;--panel:#151a22;--warn:#e8b866;--warnbg:#2a2113}}
body{background:var(--bg);color:var(--fg);font:15px/1.6 -apple-system,Segoe UI,Roboto,sans-serif;margin:0;padding:40px 20px 80px}
.wrap{max-width:880px;margin:0 auto}
h1{font-size:27px;margin:0 0 4px;letter-spacing:-.01em}
h2{font-size:18px;margin:36px 0 10px;padding-bottom:6px;border-bottom:1px solid var(--line)}
.sub{color:var(--muted);margin:0 0 22px;font-size:14px}
table{border-collapse:collapse;width:100%;font-size:14px;margin:10px 0}
td,th{text-align:left;padding:7px 9px;border-bottom:1px solid var(--line)}
th{color:var(--muted);font-size:12px;text-transform:uppercase;letter-spacing:.04em}
td.v,td.n,th.n{text-align:right;font-variant-numeric:tabular-nums}
table.grid td.g{text-align:right;font-variant-numeric:tabular-nums;padding:4px 7px;font-size:12.5px}
/* FALSE COLOUR: the cell carries the colour AND the number, so the field can be read at a glance
   and checked value by value.

   THE PLOT FITS THE PAGE. It used to be a fixed 44px per cell, so a 33-column field was 1,450px
   wide and had to be scrolled sideways to be seen at all — reported as "the user have to go an
   scroll to see the layout with is not intuitive". `table-layout:fixed` with a 100% width makes
   the browser divide the page between the columns instead, and the cells stay square-ish because
   the row height follows. `max-width` on the wrapper keeps a 2x2 grid from being blown up to the
   width of the window, which would be the same mistake in the other direction. */
table.fc{border-collapse:collapse;margin:2px 0;table-layout:fixed;width:100%}
.fcwrap{max-width:100%;overflow:hidden}
table.fc td.fc{text-align:center;vertical-align:middle;padding:0;
  font:600 clamp(5px,1.1vw,11px)/1 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;
  font-variant-numeric:tabular-nums;border:1px solid rgba(0,0,0,.18);
  overflow:hidden;white-space:nowrap}
table.fc td.out{background:transparent;border:1px dashed rgba(128,128,128,.35)}
/* Renders: one per row when there is one, side by side when there are more. */
.renders{display:flex;flex-wrap:wrap;gap:14px;margin:12px 0}
.renders figure{flex:1 1 320px;margin:0}
.renders img{width:100%;height:auto;border:1px solid var(--line);border-radius:6px;display:block}
.renders figcaption{font-size:12px;color:#667;margin-top:5px;text-align:center}
/* The layout drawing. A viewBox in metres, so the browser scales it and it stays crisp. */
.layout{width:100%;max-width:560px;height:auto;display:block;margin:12px auto;
  background:#fff;border:1px solid var(--line);border-radius:6px;padding:8px;box-sizing:border-box}
/* The schedule: what the room is lit WITH. */
table.sched{width:100%;border-collapse:collapse;margin:10px 0}
table.sched th,table.sched td{padding:6px 8px;border-bottom:1px solid var(--line);font-size:13px}
table.sched .sub{display:block;font-size:11px;color:#667;margin-top:2px}
table.sched tr.tot td{font-weight:600;border-top:2px solid var(--line);border-bottom:none}
/* The legend is the scale the plot was drawn at; without it the picture states nothing. */
.legend{display:flex;align-items:center;gap:8px;margin:6px 0 2px}
.legend .lgbar{display:flex;flex:0 0 320px;height:14px;border:1px solid rgba(128,128,128,.5)}
.legend .lgbar i{flex:1}
.legend .lgz{font-size:12px;color:#667}
.scroll{overflow-x:auto}
.kpi{display:flex;flex-wrap:wrap;gap:10px;margin:18px 0}
.kpi div{flex:1 1 150px;background:var(--panel);border:1px solid var(--line);border-radius:9px;padding:11px 13px}
.kpi b{display:block;font-size:22px;font-variant-numeric:tabular-nums;letter-spacing:-.02em}
.kpi span{color:var(--muted);font-size:12px}
.note{color:var(--muted);font-size:13px}
.warn{color:var(--warn);background:var(--warnbg);border-radius:8px;padding:9px 12px;font-size:13.5px}
</style>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use cad_light::Vertex;

    fn grid(values: Vec<f64>, cols: u32, rows: u32) -> LuxGrid {
        let min = values.iter().cloned().fold(f64::MAX, f64::min);
        let max = values.iter().cloned().fold(0.0, f64::max);
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        LuxGrid {
            cols,
            rows,
            values,
            min,
            max,
            avg,
            maintenance: 0.8,
            direct: Vec::new(),
            indirect: Vec::new(),
        }
    }

    fn plane(cols: u32, rows: u32) -> CalcPlane {
        CalcPlane { origin: Vertex::new(0.0, 0.0, 0.8), width: 4.0, depth: 4.0, cols, rows }
    }

    pub(super) fn input<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> ReportInput<'a> {
        ReportInput {
            title: "Test room".into(),
            grid: g,
            plane: p,
            maintenance: Maintenance { llmf: 0.8, lsf: 1.0, lmf: 1.0, rsmf: 1.0 },
            installation: None,
            surfaces: &[],
            cylindrical_avg: None,
            eye_height: 1.2,
            room_height: 3.0,
            materials: vec![("Floor".into(), 0.2), ("Ceiling".into(), 0.7)],
            unassigned: 0,
            ramp: crate::light::lux_rgb,
            scale_top: 500.0,
            scale_auto: true,
            mask: Vec::new(),
            sections: crate::report::Section::all(),
            images: Vec::new(),
            schedule: Vec::new(),
            poly: Vec::new(),
            fixtures: Vec::new(),
        }
    }

    /// EVERY CELL of the grid is in the file. A report that shows a picture of the field cannot be
    /// checked by the person receiving it; one that shows the numbers can.
    #[test]
    fn the_whole_grid_is_written_out() {
        let g = grid((0..16).map(|i| (i * 10) as f64).collect(), 4, 4);
        let p = plane(4, 4);
        let html = render(&input(&g, &p));
        for v in [0, 50, 100, 150] {
            assert!(html.contains(&format!("\"g\">{v}</td>")), "cell {v} missing");
        }
        // Grid cells carry their own class so this counts THEM and not the conditions table, which
        // is also made of `<td>` — the first version of this counted 30 and was measuring the
        // wrong thing.
        assert_eq!(html.matches("<td class=\"g\">").count(), 16, "16 cells expected");
    }

    /// Rows run far-to-near so the table reads like the plan, not upside down.
    #[test]
    fn the_grid_is_written_in_plan_order() {
        // Row 0 (nearest) is all 1s, row 1 (far) all 99s — so 99 must come FIRST in the document.
        let g = grid(vec![1.0, 1.0, 99.0, 99.0], 2, 2);
        let p = plane(2, 2);
        let html = render(&input(&g, &p));
        let first_99 = html.find(">99</td>").expect("99 present");
        let first_1 = html.find(">1</td>").expect("1 present");
        assert!(first_99 < first_1, "the far row must be written first");
    }

    /// A QUANTITY THAT WAS NOT CALCULATED IS ABSENT, not shown as zero. Zero lux of cylindrical
    /// illuminance is a specific, alarming claim; "not calculated" is the truth.
    #[test]
    fn uncalculated_quantities_are_omitted_not_zeroed() {
        let g = grid(vec![100.0; 4], 2, 2);
        let p = plane(2, 2);
        let html = render(&input(&g, &p));
        assert!(!html.contains("Cylindrical"), "cylindrical was never calculated");
        assert!(!html.contains("Room surfaces"), "no surfaces were reported");
        assert!(!html.contains("Connected load"), "no installation was summarised");

        let mut with = input(&g, &p);
        with.cylindrical_avg = Some(75.0);
        let html = render(&with);
        assert!(html.contains("Cylindrical") && html.contains("75 lx"));
    }

    /// The grid the uniformity was measured on is always stated — and a grid coarser than
    /// EN 12464-1 asks for says so, because that is the case where U₀ flatters the design.
    #[test]
    fn the_report_states_its_grid_and_flags_a_coarse_one() {
        let g = grid(vec![100.0; 64], 8, 8);
        let p = plane(8, 8);
        let html = render(&input(&g, &p));
        assert!(html.contains("0.50 m spacing"), "the grid must be stated");
        assert!(!html.contains("optimistic"), "8 × 8 on a 4 m room IS the standard grid");

        // …and a 2 × 2 grid on the same room is not.
        let g = grid(vec![100.0; 4], 2, 2);
        let p = plane(2, 2);
        let html = render(&input(&g, &p));
        assert!(html.contains("optimistic"), "a coarse grid must be flagged");
        assert!(html.contains("8 × 8"), "…and say what the standard asks for");
    }

    /// Unassigned points are declared. A result computed from half a layout looks exactly like one
    /// computed from all of it.
    #[test]
    fn unassigned_fittings_are_declared() {
        let g = grid(vec![100.0; 4], 2, 2);
        let p = plane(2, 2);
        let mut i = input(&g, &p);
        i.unassigned = 3;
        let html = render(&i);
        assert!(html.contains("3 light point(s) have no fitting"), "must say so");
    }

    /// A room name with HTML in it must not break the document.
    #[test]
    fn the_title_is_escaped() {
        let g = grid(vec![100.0; 4], 2, 2);
        let p = plane(2, 2);
        let mut i = input(&g, &p);
        i.title = "Smith & Sons <script>alert(1)</script>".into();
        let html = render(&i);
        assert!(!html.contains("<script>"), "raw markup leaked into the document");
        assert!(html.contains("Smith &amp; Sons"));
    }

    /// The maintenance factor is stated WITH its four sub-factors, so a reader can see whether the
    /// number was assumed or built up.
    #[test]
    fn the_maintenance_factor_shows_its_working() {
        let g = grid(vec![100.0; 4], 2, 2);
        let p = plane(2, 2);
        let mut i = input(&g, &p);
        i.maintenance = Maintenance { llmf: 0.95, lsf: 1.0, lmf: 0.90, rsmf: 0.94 };
        let html = render(&i);
        assert!(html.contains("LLMF 0.95"), "sub-factors must be visible");
        assert!(html.contains("0.80"), "…and their product");
    }
}

/// THE FALSE-COLOUR FIELD IS ON THE REPORT.
///
/// Asked for as: "add the psudo colors layout in the report too." Colour and number in the same
/// cell — a ramp shows the shape of the field and cannot be read back into values; a table of
/// numbers is exact and shows no shape.
#[cfg(test)]
mod the_report_carries_the_false_colour_field {
    use super::*;

    fn grid(vals: Vec<f64>, cols: u32, rows: u32) -> LuxGrid {
        let min = vals.iter().cloned().fold(f64::MAX, f64::min);
        let max = vals.iter().cloned().fold(f64::MIN, f64::max);
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        LuxGrid { cols, rows, values: vals, min, max, avg, maintenance: 0.8,
                  direct: Vec::new(), indirect: Vec::new() }
    }

    fn plane(cols: u32, rows: u32) -> CalcPlane {
        CalcPlane { origin: cad_light::Vertex::new(0.0, 0.0, 0.8), width: 4.0, depth: 4.0, cols, rows }
    }

    fn base<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> ReportInput<'a> {
        ReportInput {
            title: "R".into(), grid: g, plane: p,
            maintenance: Maintenance { llmf: 0.8, lsf: 1.0, lmf: 1.0, rsmf: 1.0 },
            installation: None, surfaces: &[], cylindrical_avg: None,
            eye_height: 1.2, room_height: 3.0, materials: Vec::new(), unassigned: 0,
            ramp: crate::light::lux_rgb, scale_top: 500.0, scale_auto: true, mask: Vec::new(),
            sections: crate::report::Section::all(),
            images: Vec::new(),
            schedule: Vec::new(),
            poly: Vec::new(),
            fixtures: Vec::new(),
        }
    }

    /// A coloured cell per grid point, each carrying its own value.
    #[test]
    fn every_cell_is_coloured_and_labelled() {
        let g = grid(vec![100.0, 200.0, 300.0, 400.0], 2, 2);
        let p = plane(2, 2);
        let html = render(&base(&g, &p));
        assert_eq!(html.matches("class=\"fc\" style=\"background:rgb(").count(), 4);
        for v in ["100", "200", "300", "400"] {
            assert!(html.contains(&format!(">{v}</td>")), "{v} lx is not on the plot");
        }
    }

    /// THE SCALE IS STATED. Without it the same room reads "mostly red" or "mostly blue" depending
    /// on a number in a menu, and two reports are not comparable.
    #[test]
    fn the_scale_and_its_mode_are_stated() {
        let g = grid(vec![100.0, 900.0], 2, 1);
        let p = plane(2, 1);
        let auto = render(&base(&g, &p));
        assert!(auto.contains("500 lx"), "the top of the scale must be on the page");
        assert!(auto.contains("auto"), "…and whether it was auto");

        let mut pinned = base(&g, &p);
        pinned.scale_auto = false;
        assert!(render(&pinned).contains("pinned"), "a pinned scale must say so");
    }

    /// It follows the CHOSEN palette, so the file matches the screen.
    #[test]
    fn it_uses_the_chosen_palette() {
        let g = grid(vec![500.0], 1, 1);
        let p = plane(1, 1);
        let mut grey = base(&g, &p);
        grey.ramp = crate::light::LuxRamp::Grey.rgb_fn();
        let html = render(&grey);
        // Greyscale at the top of the scale is near-white: r == g == b.
        let c = html
            .split("class=\"fc\" style=\"background:rgb(")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .expect("a coloured cell");
        let n: Vec<i32> = c.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        assert_eq!(n.len(), 3);
        assert!(n[0] == n[1] && n[1] == n[2], "greyscale must be neutral, got {n:?}");
    }

    /// Cells outside the room are blank, not coloured — colouring them would report illuminance on
    /// ground the room does not occupy.
    #[test]
    fn masked_cells_are_left_blank() {
        let g = grid(vec![100.0, 900.0], 2, 1);
        let p = plane(2, 1);
        let mut masked = base(&g, &p);
        masked.mask = vec![true, false];
        let html = render(&masked);
        assert_eq!(html.matches("class=\"fc out\"").count(), 1, "the outside cell is blank");
        assert_eq!(
            html.matches("class=\"fc\" style=\"background:rgb(").count(),
            1,
            "and only the inside one is coloured",
        );
    }

    /// The numeric grid SURVIVES alongside it. The picture is for reading at a glance; the table is
    /// what makes the report checkable, and losing it to a prettier page would be a regression.
    #[test]
    fn the_numeric_grid_is_still_there() {
        let g = grid(vec![100.0, 200.0, 300.0, 400.0], 2, 2);
        let p = plane(2, 2);
        let html = render(&base(&g, &p));
        assert_eq!(html.matches("class=\"g\"").count(), 4, "every value still has a plain cell");
        assert!(html.contains("Illuminance grid (lx)"));
        assert!(html.contains("Illuminance — false colour"));
    }

    /// A legend, in the same ramp, or the colours mean nothing.
    #[test]
    fn there_is_a_legend() {
        let g = grid(vec![250.0], 1, 1);
        let p = plane(1, 1);
        let html = render(&base(&g, &p));
        assert!(html.contains("class=\"legend\""), "the plot needs its scale drawn");
        assert!(html.matches("<i style=\"background:rgb(").count() >= 20, "sampled across the ramp");
    }
}

/// UNTICKING A SECTION LEAVES IT OUT OF THE HTML TOO.
///
/// Asked for as "the user will be able to unselect info that they dont need to be generated" —
/// which has to mean both formats, or the choice is a property of the button you pressed.
#[cfg(test)]
mod the_html_honours_the_chosen_sections {
    use super::*;
    use crate::report::Section;

    fn grid() -> LuxGrid {
        LuxGrid {
            cols: 2,
            rows: 2,
            values: vec![100.0, 200.0, 300.0, 400.0],
            min: 100.0,
            max: 400.0,
            avg: 250.0,
            maintenance: 0.8,
            direct: Vec::new(),
            indirect: Vec::new(),
        }
    }

    fn plane() -> CalcPlane {
        CalcPlane { origin: cad_light::Vertex::new(0.0, 0.0, 0.8), width: 4.0, depth: 4.0, cols: 2, rows: 2 }
    }

    fn with<'a>(g: &'a LuxGrid, p: &'a CalcPlane, keep: Vec<Section>) -> ReportInput<'a> {
        let mut i = tests::input(g, p);
        i.sections = keep;
        i
    }

    /// Everything on is the report as it always was.
    #[test]
    fn all_sections_selected_changes_nothing() {
        let (g, p) = (grid(), plane());
        let html = render(&with(&g, &p, Section::all()));
        for h in ["Conditions", "Working plane", "Illuminance grid (lx)"] {
            assert!(html.contains(h), "{h} went missing with everything selected");
        }
        assert!(!html.contains("<!--SEC:"), "the markers must not survive into the file");
    }

    /// A SECTION SWITCHED OFF IS GONE — heading, table and all.
    #[test]
    fn an_unselected_section_is_absent() {
        let (g, p) = (grid(), plane());
        let html = render(&with(&g, &p, vec![Section::WorkingPlane]));
        assert!(html.contains("Working plane"), "the one that was kept is missing");
        assert!(!html.contains("Illuminance grid (lx)"), "the grid heading survived");
        assert!(!html.contains("<h2>Conditions</h2>"), "the conditions table survived");
        // …and the numbers that only that section prints went with it.
        assert!(!html.contains("class=\"grid\""), "the grid table survived its heading");
    }

    /// THE TITLE IS NEVER DROPPED. A report with no sections is a shorter report; a report with no
    /// title is an anonymous one, and nobody can file it.
    ///
    /// The HEADLINE figures are part of Summary and go with it — they used to sit above the first
    /// marker, which meant unticking Summary did nothing whatever to the HTML while emptying the
    /// PDF. A tick that works in one format and not the other is worse than no tick.
    #[test]
    fn the_title_survives_an_empty_selection() {
        let (g, p) = (grid(), plane());
        let html = render(&with(&g, &p, Vec::new()));
        assert!(html.contains("Test room"), "the title went with the sections");
        assert!(html.ends_with("</html>"), "the document did not close");
        assert!(!html.contains("<h2>"), "no section should have been printed");
        assert!(
            !html.contains("average maintained"),
            "the headline survived an empty selection — Summary is a section like any other",
        );
    }

    /// …and it comes back with Summary.
    #[test]
    fn the_headline_belongs_to_summary() {
        let (g, p) = (grid(), plane());
        let html = render(&with(&g, &p, vec![Section::Summary]));
        assert!(html.contains("average maintained"), "Summary did not bring the headline back");
        assert!(html.contains("Test room"));
    }

    /// THE MARKERS ARE INERT. They are HTML comments, so a file that somehow kept one would still
    /// render — but they are removed, and a stray one in the output would mean the filter had
    /// stopped running.
    #[test]
    fn the_markers_never_reach_the_file() {
        let (g, p) = (grid(), plane());
        for keep in [Section::all(), vec![Section::Surfaces], Vec::new()] {
            let html = render(&with(&g, &p, keep));
            assert!(!html.contains("SEC:"), "a marker was left in the output");
        }
    }
}

/// THE HTML CARRIES THE SCHEDULE AND THE LAYOUT TOO.
///
/// They were in the PDF and not in the HTML at all, so a report asked for as HTML said what the
/// illuminance was and never what produced it — and the tick boxes for them did nothing.
#[cfg(test)]
mod the_html_carries_the_same_sections_as_the_pdf {
    use super::*;
    use crate::report::Section;

    fn grid2() -> LuxGrid {
        LuxGrid {
            cols: 2,
            rows: 2,
            values: vec![100.0, 200.0, 300.0, 400.0],
            min: 100.0,
            max: 400.0,
            avg: 250.0,
            maintenance: 0.8,
            direct: Vec::new(),
            indirect: Vec::new(),
        }
    }

    fn plane2() -> CalcPlane {
        CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 8.0,
            depth: 6.0,
            cols: 2,
            rows: 2,
        }
    }

    fn lum(x: f32, y: f32, id: u32) -> cad_light::Luminaire {
        cad_light::Luminaire {
            id,
            profile: "OCULUS".into(),
            position: cad_light::Vertex::new(x, y, 2.7),
            rotation_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }
    }

    fn furnished<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> ReportInput<'a> {
        let mut i = tests::input(g, p);
        i.schedule = vec![crate::report::layout::ScheduleRow {
            profile: "OCULUS GRANDE 2.0".into(),
            count: 12,
            manufacturer: "HSI Lighting".into(),
            catalogue: "OG20-36".into(),
            lamp: "LED 3000K".into(),
            watts: 22.0,
            lumens: 2400.0,
            size_m: (0.095, 0.095, 0.06),
        }];
        i.poly = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
        ];
        i.fixtures = vec![lum(2.0, 2.0, 1), lum(6.0, 4.0, 2)];
        i
    }

    /// THE SCHEDULE IS THERE, with everything the fitting's own file declared.
    #[test]
    fn the_schedule_is_in_the_html() {
        let (g, p) = (grid2(), plane2());
        let html = render(&furnished(&g, &p));
        for want in ["Luminaire schedule", "OCULUS GRANDE 2.0", "HSI Lighting", "OG20-36", "LED 3000K"] {
            assert!(html.contains(want), "{want:?} is not in the HTML report");
        }
        assert!(html.contains("264.0"), "the connected load (12 × 22 W) is missing");
        assert!(html.contains("109"), "the efficacy (2400/22) is missing");
    }

    /// THE LAYOUT IS DRAWN, as inline SVG so the file stays self-contained.
    #[test]
    fn the_layout_is_drawn_as_svg() {
        let (g, p) = (grid2(), plane2());
        let html = render(&furnished(&g, &p));
        assert!(html.contains("<h2>Lighting layout</h2>"), "no layout section");
        assert!(html.contains("<svg class=\"layout\""), "the drawing is not inline SVG");
        assert!(html.contains("viewBox=\"0 0 8.000 6.000\""), "the viewBox is not the room");
        assert!(html.contains("<polygon"), "the room outline was not drawn");
        assert!(html.contains("2 fitting(s)"), "the fitting count is missing");
        // Two markers, each a pair of crossing lines plus a box.
        assert_eq!(html.matches("stroke=\"#c8963c\"").count(), 2, "a marker per fitting");
    }

    /// THE DRAWING IS NOT UPSIDE DOWN. A plan reads with +y up and SVG measures down, so it is
    /// flipped once — and a layout mirrored against its own result is worse than none.
    #[test]
    fn the_svg_is_the_same_way_up_as_the_plan() {
        let (g, p) = (grid2(), plane2());
        let mut i = furnished(&g, &p);
        // Near the plan's top (y = 5.5) and near its bottom (y = 0.5).
        i.fixtures = vec![lum(4.0, 5.5, 1), lum(4.0, 0.5, 2)];
        let html = render(&i);
        // The marker boxes, in emission order: the first is the y = 5.5 fitting.
        let ys: Vec<f64> = html
            .split("stroke=\"#c8963c\"")
            .skip(1)
            .filter_map(|_| None::<f64>)
            .collect();
        let _ = ys;
        let boxes: Vec<f64> = html
            .match_indices("<rect x=\"")
            .filter_map(|(at, _)| {
                let rest = &html[at..];
                let y = rest.split("y=\"").nth(1)?.split('"').next()?;
                y.parse::<f64>().ok()
            })
            .collect();
        assert_eq!(boxes.len(), 2, "expected two marker boxes, got {boxes:?}");
        assert!(
            boxes[0] < boxes[1],
            "the fitting at y = 5.5 m drew at {:.2} and the one at y = 0.5 m at {:.2} — the \
             drawing is mirrored",
            boxes[0],
            boxes[1],
        );
    }

    /// AND BOTH CAN BE SWITCHED OFF, like every other section.
    #[test]
    fn both_honour_their_tick_boxes() {
        let (g, p) = (grid2(), plane2());
        let mut i = furnished(&g, &p);
        i.sections = Section::all().into_iter().filter(|s| *s != Section::Schedule).collect();
        let html = render(&i);
        assert!(!html.contains("Luminaire schedule"), "the schedule ignored its tick box");
        assert!(html.contains("Lighting layout"), "the layout went with it");

        let mut i = furnished(&g, &p);
        i.sections = Section::all().into_iter().filter(|s| *s != Section::Layout).collect();
        let html = render(&i);
        assert!(!html.contains("Lighting layout"), "the layout ignored its tick box");
        assert!(html.contains("Luminaire schedule"), "the schedule went with it");
    }
}
