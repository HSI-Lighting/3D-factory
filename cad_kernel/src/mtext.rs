//! MTEXT inline formatting codes (AutoCAD MTEXT subset).
//!
//! `Geom::Text.text` may carry AutoCAD's inline codes; the RENDERER splits
//! them into runs (per-run color / font / height) and the plain-text helpers
//! strip them (for `list`, DXF TEXT fallbacks, and advance measurement).
//!
//! Supported codes (case-insensitive):
//!   `\\P`          — paragraph break (treated as `\n`)
//!   `\\C<n>;`      — ACI color (1..255); `\\C0;` = reset to base
//!   `\\c<r>;<g>;<b>;` — truecolor (0..255 each)
//!   `\\H<mult>;`   — height multiplier (`\\H2x;` or `\\H1.5;`)
//!   `\\f<name>;`   — font family switch
//! Unknown `\\x` sequences are kept verbatim (never dropped).

/// A run's color override: ACI index or truecolor RGB (the kernel can't
/// intern truecolors without the Document's table — the app converts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MtextColor {
    Aci(u8),
    True(u32),
}

/// One formatted run of an MTEXT paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct MtextRun {
    pub text: String,
    /// Color override for this run (`None` = inherit the entity color).
    pub color: Option<MtextColor>,
    /// Font family override (`None` = inherit the entity font).
    pub font: Option<String>,
    /// Height multiplier (1.0 = inherit).
    pub height_mult: f64,
}

/// Parse `text` into runs. `\P` becomes a `\n` inside a run (the caller
/// splits lines). Consecutive same-format segments merge.
pub fn parse_runs(text: &str) -> Vec<MtextRun> {
    let mut runs: Vec<MtextRun> = Vec::new();
    let mut pending_color: Option<MtextColor> = None;
    let mut pending_font: Option<String> = None;
    let mut pending_h: f64 = 1.0;

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut buf = String::new();
    let flush = |buf: &mut String, runs: &mut Vec<MtextRun>,
                 color: Option<MtextColor>, font: Option<String>, h: f64| {
        if buf.is_empty() { return; }
        if !runs.is_empty() {
            let last = runs.last_mut().unwrap();
            if last.color == color && last.font == font
                && (last.height_mult - h).abs() < 1e-12 {
                last.text.push_str(buf);
                buf.clear();
                return;
            }
        }
        let nr = MtextRun {
            text: std::mem::take(buf),
            color,
            font,
            height_mult: h,
        };
        if nr.text.is_empty() { return; }
        // Merge with the previous run when formats match.
        if let Some(last) = runs.last_mut() {
            if last.color == nr.color && last.font == nr.font
                && (last.height_mult - nr.height_mult).abs() < 1e-12 {
                last.text.push_str(&nr.text);
                return;
            }
        }
        runs.push(nr);
    };

    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            buf.push(c);
            i += 1;
            continue;
        }
        let code = chars[i + 1].to_ascii_lowercase();
        // `\\` → literal backslash.
        if code == '\\' {
            buf.push('\\');
            i += 2;
            continue;
        }
        // \P → paragraph break.
        if code == 'p' {
            buf.push('\n');
            i += 2;
            continue;
        }
        // \C<n>; — ACI color.
        if code == 'c' {
            // Truecolor form \c<r>;<g>;<b>; (lowercase c) vs \C<n>; ACI.
            let rest: String = chars[i + 2..].iter().take(48).collect();
            if rest.contains(';') && !rest.starts_with(char::is_numeric) {
                // Try truecolor: r;g;b;
                let parts: Vec<&str> = rest.split(';').collect();
                if parts.len() >= 3 {
                    if let (Ok(r), Ok(g), Ok(b)) = (
                        parts[0].trim().parse::<u8>(),
                        parts[1].trim().parse::<u8>(),
                        parts[2].trim().parse::<u8>(),
                    ) {
                        let consumed = parts[0].len() + parts[1].len() + parts[2].len() + 3;
                        flush(&mut buf, &mut runs, pending_color, pending_font.clone(), pending_h);
                        let rgb = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
                        pending_color = Some(MtextColor::True(rgb));
                        // Consume r;g;b; plus the leading \c
                        i += 2 + consumed;
                        continue;
                    }
                }
            }
            // ACI form: \C<n>;
            let rest: String = chars[i + 2..].iter().take(16).collect();
            if let Some(idx) = rest.find(';') {
                if let Ok(n) = rest[..idx].trim().parse::<u8>() {
                    flush(&mut buf, &mut runs, pending_color, pending_font.clone(), pending_h);
                    pending_color = if n == 0 { None } else { Some(MtextColor::Aci(n)) };
                    i += 2 + idx + 1;
                    continue;
                }
            }
            // Not a valid color code — keep verbatim.
            buf.push(c);
            i += 1;
            continue;
        }
        // \H<mult>; — height multiplier.
        if code == 'h' {
            let rest: String = chars[i + 2..].iter().take(16).collect();
            if let Some(idx) = rest.find(';') {
                let num = rest[..idx].trim_end_matches('x').trim();
                if let Ok(m) = num.parse::<f64>() {
                    if m > 0.0 && m < 1e6 {
                        flush(&mut buf, &mut runs, pending_color, pending_font.clone(), pending_h);
                        pending_h = m;
                        i += 2 + idx + 1;
                        continue;
                    }
                }
            }
            buf.push(c);
            i += 1;
            continue;
        }
        // \f<name>; — font switch.
        if code == 'f' {
            let rest: String = chars[i + 2..].iter().take(64).collect();
            if let Some(idx) = rest.find(';') {
                let name = rest[..idx].trim().to_string();
                if !name.is_empty() {
                    flush(&mut buf, &mut runs, pending_color, pending_font.clone(), pending_h);
                    pending_font = Some(name);
                    i += 2 + idx + 1;
                    continue;
                }
            }
            buf.push(c);
            i += 1;
            continue;
        }
        // Unknown code — keep the backslash + char verbatim.
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut runs, pending_color, pending_font, pending_h);
    if runs.is_empty() && !text.is_empty() {
        runs.push(MtextRun {
            text: text.to_string(),
            color: None,
            font: None,
            height_mult: 1.0,
        });
    }
    runs
}

/// Strip MTEXT codes → plain text (`\P` → newline, unknown codes kept).
pub fn strip_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let code = chars[i + 1].to_ascii_lowercase();
        match code {
            '\\' => { out.push('\\'); i += 2; }
            'p' => { out.push('\n'); i += 2; }
            'c' | 'h' | 'f' => {
                // Skip to the terminating ';'.
                let mut j = i + 2;
                let mut found = false;
                while j < chars.len() {
                    if chars[j] == ';' { found = true; break; }
                    j += 1;
                }
                if found { i = j + 1; } else { out.push(c); i += 1; }
            }
            _ => { out.push(c); i += 1; }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_run() {
        let runs = parse_runs("hello world");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "hello world");
        assert_eq!(runs[0].color, None);
        assert_eq!(runs[0].height_mult, 1.0);
    }

    #[test]
    fn aci_color_runs() {
        let runs = parse_runs("red \\C1;here\\C0; back");
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "red ");
        assert_eq!(runs[0].color, None);
        assert_eq!(runs[1].text, "here");
        assert_eq!(runs[1].color, Some(MtextColor::Aci(1)));
        assert_eq!(runs[2].text, " back");
        assert_eq!(runs[2].color, None, "\\C0; resets");
    }

    #[test]
    fn paragraph_break_and_heights() {
        let runs = parse_runs("line one\\Pline two \\H2x;big");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "line one\nline two ");
        assert_eq!(runs[1].text, "big");
        assert!((runs[1].height_mult - 2.0).abs() < 1e-9);
    }

    #[test]
    fn font_switch() {
        let runs = parse_runs("a\\fArial;b");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].text, "b");
        assert_eq!(runs[1].font.as_deref(), Some("Arial"));
    }

    #[test]
    fn strip_removes_codes_only() {
        let s = strip_codes("a\\C1;b\\H2x;c\\P d\\\\e");
        assert_eq!(s, "abc\n d\\e");
    }

    #[test]
    fn unknown_codes_kept_verbatim() {
        let runs = parse_runs("a\\Qx;b");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "a\\Qx;b");
    }

    #[test]
    fn consecutive_same_format_merges() {
        let runs = parse_runs("\\C1;a\\C1;b\\C1;c");
        assert_eq!(runs.len(), 1, "leading empty run dropped, same-format runs merge");
        assert_eq!(runs[0].text, "abc");
        assert_eq!(runs[0].color, Some(MtextColor::Aci(1)));
    }
}
