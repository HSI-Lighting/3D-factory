//! TABLE — a grid of text cells (AutoCAD TABLE, v1).
//!
//! Uniform rows/columns; each cell holds a single-line string. The table
//! anchors at `insert` (its TOP-LEFT corner before rotation; grid grows
//! down + right). Styled text uses `style` (TextStyleTable id) at
//! `font_height`. RSM tag 18 is the canonical storage; DXF exports the grid
//! as fallback LINEs + TEXT records (a real DXF TABLE entity is out of
//! scope). Snaps/intersects/trim: annotation semantics (none).

use crate::math::Vec2;
use crate::text::Text;

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Top-left anchor (before rotation).
    pub insert: Vec2,
    pub n_rows: usize,
    pub n_cols: usize,
    /// Uniform row height (drawing units).
    pub row_h: f64,
    /// Uniform column width.
    pub col_w: f64,
    /// Rotation in radians, CCW.
    pub rotation: f64,
    /// TextStyleTable id for cell text.
    pub style: u32,
    /// Cell text height (drawing units).
    pub font_height: f64,
    /// Cell strings, row-major. May be shorter than rows*cols (empty cells).
    pub cells: Vec<String>,
}

impl Table {
    /// Total grid size (width, height) before rotation.
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.n_cols as f64 * self.col_w,
                  self.n_rows as f64 * self.row_h)
    }

    /// World point of the cell (row, col) top-left corner (unrotated grid
    /// coordinates, rotated into world).
    fn cell_corner(&self, row: usize, col: usize) -> Vec2 {
        let (s, c) = self.rotation.sin_cos();
        let gx = col as f64 * self.col_w;
        let gy = -(row as f64) * self.row_h;
        self.insert + Vec2::new(gx * c - gy * s, gx * s + gy * c)
    }

    /// The cell (row, col) content as a kernel Text dobject (for rendering
    /// and export): left-aligned at the cell's top-left + a small inset.
    pub fn cell_text(&self, row: usize, col: usize) -> Option<Text> {
        let idx = row * self.n_cols + col;
        let s = self.cells.get(idx)?;
        if s.is_empty() { return None; }
        let h = self.font_height;
        let pad = h * 0.25;
        let pos = self.cell_corner(row, col)
            + Vec2::new(pad, -h - pad);
        Some(Text {
            position: pos,
            height: h,
            angle: self.rotation,
            text: s.clone(),
            h_align: crate::text::HAlign::Left,
            v_align: crate::text::VAlign::Baseline,
            style: self.style,
            ..Text::empty()
        })
    }

    /// Grid line segments (world): all horizontal + vertical rules.
    pub fn grid_lines(&self) -> Vec<(Vec2, Vec2)> {
        let mut out = Vec::new();
        let w = self.n_cols as f64 * self.col_w;
        let h = self.n_rows as f64 * self.row_h;
        for r in 0..=self.n_rows {
            let a = self.cell_corner(r, 0);
            let b = self.cell_corner(r, self.n_cols);
            out.push((a, b));
        }
        for c in 0..=self.n_cols {
            let a = self.cell_corner(0, c);
            let b = self.cell_corner(self.n_rows, c);
            out.push((a, b));
        }
        let _ = (w, h);
        out
    }

    pub fn bbox(&self) -> (Vec2, Vec2) {
        let s = self.size();
        let (sin, cos) = self.rotation.sin_cos();
        // The four unrotated corners, rotated + translated.
        let corners = [
            Vec2::new(0.0, 0.0),
            Vec2::new(s.x, 0.0),
            Vec2::new(s.x, -s.y),
            Vec2::new(0.0, -s.y),
        ];
        let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for c in corners {
            let w = self.insert + Vec2::new(
                c.x * cos - c.y * sin,
                c.x * sin + c.y * cos,
            );
            mn.x = mn.x.min(w.x); mn.y = mn.y.min(w.y);
            mx.x = mx.x.max(w.x); mx.y = mx.y.max(w.y);
        }
        (mn, mx)
    }

    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        // Distance to the nearest grid line, then to the nearest cell text
        // bbox — a click on any rule or letter picks the table.
        let mut best = f64::INFINITY;
        for (a, b) in self.grid_lines() {
            best = best.min(crate::geom::Line { a, b }.distance_to_point(p));
        }
        for r in 0..self.n_rows {
            for c in 0..self.n_cols {
                if let Some(t) = self.cell_text(r, c) {
                    best = best.min(t.distance_to_point(p));
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tbl() -> Table {
        Table {
            insert: Vec2::ZERO,
            n_rows: 2,
            n_cols: 3,
            row_h: 5.0,
            col_w: 10.0,
            rotation: 0.0,
            style: 0,
            font_height: 1.0,
            cells: vec!["A1".into(), "B1".into(), "C1".into(),
                        "A2".into(), "B2".into(), "C2".into()],
        }
    }

    #[test]
    fn grid_lines_count_and_extent() {
        let t = tbl();
        let lines = t.grid_lines();
        // 2 rows → 3 horizontals; 3 cols → 4 verticals.
        assert_eq!(lines.len(), 7);
        let (mn, mx) = t.bbox();
        assert_eq!((mn.x, mn.y, mx.x, mx.y), (0.0, -10.0, 30.0, 0.0));
    }

    #[test]
    fn cell_texts_have_content_and_position() {
        let t = tbl();
        let t1 = t.cell_text(0, 0).unwrap();
        assert_eq!(t1.text, "A1");
        assert!((t1.position.x - 0.25).abs() < 1e-9);
        assert!((t1.position.y + 1.25).abs() < 1e-9);
        let t2 = t.cell_text(1, 2).unwrap();
        assert_eq!(t2.text, "C2");
        // Empty cell → None.
        let mut e = tbl();
        e.cells.clear();
        assert!(e.cell_text(0, 0).is_none());
    }

    #[test]
    fn rotated_bbox_is_tight() {
        let mut t = tbl();
        t.rotation = std::f64::consts::FRAC_PI_2;
        let (mn, mx) = t.bbox();
        // 90° CCW: the 30×10 grid now spans x 0..10, y 0..30.
        assert!((mn.x - 0.0).abs() < 1e-9);
        assert!((mx.x - 10.0).abs() < 1e-9);
        assert!((mx.y - 30.0).abs() < 1e-9);
        assert!(mn.y.abs() < 1e-9);
    }

    #[test]
    fn distance_hits_lines_and_text() {
        let t = tbl();
        assert!(t.distance_to_point(Vec2::new(5.0, 0.0)) < 1e-9);   // top rule
        assert!(t.distance_to_point(Vec2::new(0.0, -2.5)) < 1e-9);  // left rule
        assert!(t.distance_to_point(Vec2::new(15.0, 50.0)) > 30.0); // far away
    }
}
