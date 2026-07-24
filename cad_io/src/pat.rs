//! AutoCAD `.pat` hatch-pattern parser — the "pattern extractor".
//!
//! A `.pat` file defines hatch patterns as families of straight, dashed lines.
//! Format:
//!
//! ```text
//! *NAME, optional description
//! angle, x-origin, y-origin, delta-x, delta-y [, dash1, dash2, ...]
//! angle, x-origin, y-origin, delta-x, delta-y [, dash...]
//! ```
//!
//! A pattern is one or more LINE FAMILIES. Each family repeats a (possibly
//! dashed) line at `angle`, starting at the origin, offset by `delta-y`
//! perpendicular between rows and `delta-x` shifted along the line. Dash values:
//! `+` = dash, `-` = gap, `0` = dot; no dashes = a solid line. `;` starts a
//! comment.
//!
//! IMPORTANT: a `.pat` can ONLY express straight dashed lines — no arcs, curves,
//! dots-as-shapes, or symbols. Ornamental/curved swatches (florals, hexagons,
//! circles, ankhs, …) are NOT representable here; those are blocks. This parser
//! reports each pattern and whether it is USABLE (has ≥1 valid line family).

/// One line FAMILY of a pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct PatLine {
    /// Line angle in degrees.
    pub angle: f64,
    /// Origin (base point) of the family.
    pub base: (f64, f64),
    /// `(delta-x, delta-y)` — shift along the line and perpendicular row spacing.
    pub offset: (f64, f64),
    /// Dash pattern: `+` dash, `-` gap, `0` dot; empty = solid.
    pub dashes: Vec<f64>,
}

/// One hatch pattern (a named set of line families).
#[derive(Clone, Debug)]
pub struct PatPattern {
    pub name: String,
    pub description: String,
    pub lines: Vec<PatLine>,
}

impl PatPattern {
    /// Usable = at least one line family (a header with no families renders
    /// nothing). Solid line families count.
    pub fn is_usable(&self) -> bool {
        !self.lines.is_empty()
    }
    /// True if every family is solid (no dashes) — the simplest kind.
    pub fn is_solid_lines(&self) -> bool {
        self.lines.iter().all(|l| l.dashes.is_empty())
    }
}

/// Result of parsing a `.pat` file: the patterns plus any per-line warnings.
#[derive(Clone, Debug, Default)]
pub struct PatParse {
    pub patterns: Vec<PatPattern>,
    pub warnings: Vec<String>,
}

impl PatParse {
    pub fn usable_count(&self) -> usize {
        self.patterns.iter().filter(|p| p.is_usable()).count()
    }
}

/// Parse `.pat` text into structured patterns. Never errors — malformed lines
/// are skipped and reported in `warnings`, so a partly-broken pack still yields
/// every good pattern.
pub fn parse_pat(text: &str) -> PatParse {
    let mut out = PatParse::default();
    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        // Strip `;` comments and surrounding whitespace.
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() { continue; }

        if let Some(rest) = line.strip_prefix('*') {
            let (name, description) = match rest.split_once(',') {
                Some((n, d)) => (n.trim().to_string(), d.trim().to_string()),
                None => (rest.trim().to_string(), String::new()),
            };
            if name.is_empty() {
                out.warnings.push(format!("line {lineno}: pattern header with no name"));
                continue;
            }
            out.patterns.push(PatPattern { name, description, lines: Vec::new() });
            continue;
        }

        // Otherwise: a line family — strictly comma-separated numbers.
        let mut nums: Vec<f64> = Vec::new();
        let mut bad = false;
        for tok in line.split(',') {
            let t = tok.trim();
            if t.is_empty() { continue; }
            match t.parse::<f64>() {
                Ok(n) if n.is_finite() => nums.push(n),
                _ => { bad = true; break; }
            }
        }
        if bad || nums.len() < 5 {
            out.warnings.push(format!(
                "line {lineno}: expected `angle,x,y,dx,dy[,dashes…]`, got `{line}`"));
            continue;
        }
        let fam = PatLine {
            angle: nums[0],
            base: (nums[1], nums[2]),
            offset: (nums[3], nums[4]),
            dashes: nums[5..].to_vec(),
        };
        match out.patterns.last_mut() {
            Some(p) => p.lines.push(fam),
            None => out.warnings.push(format!(
                "line {lineno}: line family before any `*pattern` header")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_families_and_dashes() {
        let src = "\
; a comment line
*ANSI31, ANSI Iron / 45 deg
45, 0,0, 0,0.125
*DASH, dashed horizontal
0, 0,0, 0,0.25, 0.25,-0.125   ; trailing comment
*BRICK, running bond
0, 0,0, 0,0.5
90, 0,0, 0.25,0.5, 0.25,-0.25
";
        let r = parse_pat(src);
        assert_eq!(r.patterns.len(), 3);
        assert!(r.warnings.is_empty(), "warnings: {:?}", r.warnings);
        assert_eq!(r.usable_count(), 3);

        let ansi = &r.patterns[0];
        assert_eq!(ansi.name, "ANSI31");
        assert_eq!(ansi.lines.len(), 1);
        assert_eq!(ansi.lines[0].angle, 45.0);
        assert!(ansi.is_solid_lines());

        let dash = &r.patterns[1];
        assert_eq!(dash.lines[0].dashes, vec![0.25, -0.125]);
        assert!(!dash.is_solid_lines());

        let brick = &r.patterns[2];
        assert_eq!(brick.lines.len(), 2);                 // two families
        assert_eq!(brick.lines[1].base, (0.0, 0.0));
        assert_eq!(brick.lines[1].offset, (0.25, 0.5));
    }

    #[test]
    fn malformed_lines_warn_but_dont_abort() {
        let src = "\
*GOOD, fine
0, 0,0, 0,0.1
*BADFAM, short family
0, 0, 0            ; only 3 numbers -> warning, skipped
*EMPTY, header only
";
        let r = parse_pat(src);
        assert_eq!(r.patterns.len(), 3);
        assert_eq!(r.usable_count(), 1);                  // only GOOD has a family
        assert!(!r.patterns[1].is_usable());              // BADFAM family was skipped
        assert!(!r.patterns[2].is_usable());              // EMPTY has none
        assert_eq!(r.warnings.len(), 1);
    }
}
