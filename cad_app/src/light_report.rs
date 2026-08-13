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
}

/// Minimal HTML escaping — a room called `Smith & Sons <Ltd>` must not break the document.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Render the report. Self-contained: the returned string IS the file.
pub fn render(inp: &ReportInput) -> String {
    let g = inp.grid;
    let p = inp.plane;
    let mut h = String::with_capacity(16 * 1024);

    h.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    h.push_str(&format!("<title>{} — SIMLUX</title>", esc(&inp.title)));
    h.push_str(STYLE);
    h.push_str("</head><body><div class=\"wrap\">");
    h.push_str(&format!("<h1>{}</h1>", esc(&inp.title)));
    h.push_str(&format!(
        "<p class=\"sub\">SIMLUX {} · maintained values, maintenance factor {:.2}</p>",
        esc(env!("SIMLUX_BUILD")),
        g.maintenance,
    ));

    // ---- headline ---------------------------------------------------------------------------
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

    // ---- conditions -------------------------------------------------------------------------
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

    // ---- the field --------------------------------------------------------------------------
    // The whole grid, not a picture of it: a report has to be checkable, and a colour ramp cannot
    // be read back into numbers.
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

    h.push_str("</div></body></html>");
    h
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

    fn input<'a>(g: &'a LuxGrid, p: &'a CalcPlane) -> ReportInput<'a> {
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
