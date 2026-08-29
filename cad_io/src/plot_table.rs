//! `.pst` — standalone Plot Style Table files (our native JSON CTB analog).
//!
//! The kernel `PlotStyleTable` is serde-able; the I/O lives here (cad_io owns
//! the `serde_json` dependency, the kernel keeps serde derive-only). Round-trips
//! all 12 CTB properties + the lineweight ladder + the General-tab fields.
//!
//! `.ctb` (AutoCAD's binary CTB) import/export is a later phase; our model is a
//! superset so the mapping is 1:1 when it lands.

use cad_kernel::plotstyle::PlotStyleTable;
use std::path::Path;

/// Serialize a plot-style table to pretty JSON (the `.pst` payload).
pub fn plot_table_to_json(t: &PlotStyleTable) -> String {
    serde_json::to_string_pretty(t).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a `.pst` JSON payload back into a table. Missing fields fall back to
/// defaults (see the kernel's `Deserialize` impl), so older/partial files load.
pub fn plot_table_from_json(s: &str) -> Result<PlotStyleTable, String> {
    serde_json::from_str(s).map_err(|e| format!("plot table (.pst) parse error: {e}"))
}

/// Save a table to a `.pst` file.
pub fn save_plot_table(path: impl AsRef<Path>, t: &PlotStyleTable) -> Result<(), String> {
    let p = path.as_ref();
    std::fs::write(p, plot_table_to_json(t).as_bytes())
        .map_err(|e| format!("write {}: {e}", p.display()))
}

/// Load a table from a `.pst` file. The table name follows the file stem
/// (AutoCAD convention: the table's name is its filename).
pub fn load_plot_table(path: impl AsRef<Path>) -> Result<PlotStyleTable, String> {
    let p = path.as_ref();
    let s = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let mut t = plot_table_from_json(&s)?;
    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
        t.name = stem.to_string();
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::plotstyle::{
        EndStyle, FillStyle, JoinStyle, PenNum, PlotColor, PlotLinetype, PlotWidth,
    };

    #[test]
    fn pst_round_trips_all_12_props_and_ladder() {
        let mut t = PlotStyleTable::named("shop");
        t.description = "house pens".into();
        t.apply_global_ltscale = true;
        t.ltscale_percent = 80.0;
        t.lineweight_ladder.push(0.65);
        // Exercise every property on color 1.
        {
            let s = t.style_mut(1);
            s.plot_color = PlotColor::Black;
            s.dither = true;
            s.grayscale = true;
            s.pen_number = PenNum::N(7);
            s.virtual_pen = PenNum::N(42);
            s.screening = 50;
            s.linetype = PlotLinetype::Id(2);
            s.adaptive = false;
            s.lineweight = PlotWidth::Fixed(0.70);
            s.end_style = EndStyle::Round;
            s.join_style = JoinStyle::Bevel;
            s.fill_style = FillStyle::Crosshatch;
        }
        t.set_fixed_width(3, 0.13);

        let json = plot_table_to_json(&t);
        let back = plot_table_from_json(&json).unwrap();
        assert_eq!(t, back);
        assert_eq!(back.style(1).lineweight, PlotWidth::Fixed(0.70));
        assert_eq!(back.style(1).plot_color, PlotColor::Black);
        assert_eq!(back.style(3).lineweight, PlotWidth::Fixed(0.13));
        assert!(back.lineweight_ladder.contains(&0.65));
        assert_eq!(back.description, "house pens");
        assert!(back.apply_global_ltscale);
        assert_eq!(back.ltscale_percent, 80.0);
    }

    #[test]
    fn pst_file_round_trip_names_from_stem() {
        let mut t = PlotStyleTable::default();
        t.set_fixed_width(1, 0.50);
        let dir = std::env::temp_dir();
        let path = dir.join("cad_io_pst_test.pst");
        save_plot_table(&path, &t).unwrap();
        let back = load_plot_table(&path).unwrap();
        assert_eq!(back.style(1).lineweight, PlotWidth::Fixed(0.50));
        assert_eq!(back.name, "cad_io_pst_test"); // named from the file stem
        let _ = std::fs::remove_file(&path);
    }
}
