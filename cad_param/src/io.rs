//! `.rsmp` — the parametric sketch file format, owned ENTIRELY by `cad_param`.
//!
//! A simple, human-readable line format (the core RSM/DXF code is never touched).
//! Non-parametric drawings keep using the core `.rsm`/`.dxf`; selecting
//! "parametric" in File ▸ New uses this instead.
//!
//! ```text
//! RSMP2
//! P <x> <y>                 ; a point (order = point id)
//! S <v>                     ; a scalar unknown, e.g. a radius (order = scalar id)
//! L <a> <b>                 ; a line between point ids
//! O <center_pt> <radius_s>  ; a circle (center point id + radius scalar id)
//! C fixed <p> <x> <y>
//! C coincident <p> <q>
//! C distance <p> <q> <d>
//! C ponline <p> <line>
//! C symmetric <p> <q> <line>
//! C horizontal <line>
//! C vertical <line>
//! C parallel <l1> <l2>
//! C perpendicular <l1> <l2>
//! C collinear <l1> <l2>
//! C equal <l1> <l2>
//! C angle <l1> <l2> <radians>
//! C radius <circle> <r>
//! C concentric <c1> <c2>
//! C eqradius <c1> <c2>
//! C poncircle <p> <circle>
//! C tangentlc <line> <circle>
//! C tangentcc <c1> <c2> <internal 0|1>
//! ```

use crate::model::{Constraint, Sketch};

const MAGIC: &str = "RSMP2";
/// The original points+lines-only format (still readable).
const MAGIC_V1: &str = "RSMP1";

pub fn write_rsmp(s: &Sketch) -> String {
    let mut out = String::from(MAGIC);
    out.push('\n');
    for p in &s.points {
        out.push_str(&format!("P {} {}\n", p.x, p.y));
    }
    for v in &s.scalars {
        out.push_str(&format!("S {}\n", v));
    }
    for l in &s.lines {
        out.push_str(&format!("L {} {}\n", l.a, l.b));
    }
    for c in &s.circles {
        out.push_str(&format!("O {} {}\n", c.center, c.radius));
    }
    for c in &s.constraints {
        match *c {
            Constraint::Fixed { p, x, y } => out.push_str(&format!("C fixed {p} {x} {y}\n")),
            Constraint::Coincident { p, q } => out.push_str(&format!("C coincident {p} {q}\n")),
            Constraint::Distance { p, q, d } => out.push_str(&format!("C distance {p} {q} {d}\n")),
            Constraint::PointOnLine { p, line } => out.push_str(&format!("C ponline {p} {line}\n")),
            Constraint::Symmetric { p, q, line } => out.push_str(&format!("C symmetric {p} {q} {line}\n")),
            Constraint::Horizontal { line } => out.push_str(&format!("C horizontal {line}\n")),
            Constraint::Vertical { line } => out.push_str(&format!("C vertical {line}\n")),
            Constraint::Parallel { a, b } => out.push_str(&format!("C parallel {a} {b}\n")),
            Constraint::Perpendicular { a, b } => out.push_str(&format!("C perpendicular {a} {b}\n")),
            Constraint::Collinear { a, b } => out.push_str(&format!("C collinear {a} {b}\n")),
            Constraint::EqualLength { a, b } => out.push_str(&format!("C equal {a} {b}\n")),
            Constraint::Angle { a, b, radians } => out.push_str(&format!("C angle {a} {b} {radians}\n")),
            Constraint::Radius { circle, r } => out.push_str(&format!("C radius {circle} {r}\n")),
            Constraint::Concentric { a, b } => out.push_str(&format!("C concentric {a} {b}\n")),
            Constraint::EqualRadius { a, b } => out.push_str(&format!("C eqradius {a} {b}\n")),
            Constraint::PointOnCircle { p, circle } => out.push_str(&format!("C poncircle {p} {circle}\n")),
            Constraint::TangentLineCircle { line, circle } => out.push_str(&format!("C tangentlc {line} {circle}\n")),
            Constraint::TangentCircleCircle { a, b, internal } => {
                out.push_str(&format!("C tangentcc {a} {b} {}\n", internal as u8))
            }
        }
    }
    out
}

pub fn read_rsmp(text: &str) -> Result<Sketch, String> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("").trim();
    if header != MAGIC && header != MAGIC_V1 {
        return Err(format!("not a .rsmp file (header `{header}`, expected `{MAGIC}`)"));
    }
    let mut s = Sketch::new();
    for (i, raw) in lines.enumerate() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        let lineno = i + 2;
        let f = |k: usize| -> Result<f64, String> {
            t.get(k).ok_or_else(|| format!("line {lineno}: missing field {k}"))?
                .parse::<f64>().map_err(|e| format!("line {lineno}: bad number: {e}"))
        };
        let u = |k: usize| -> Result<usize, String> {
            t.get(k).ok_or_else(|| format!("line {lineno}: missing field {k}"))?
                .parse::<usize>().map_err(|e| format!("line {lineno}: bad index: {e}"))
        };
        match t[0] {
            "P" => { s.add_point(f(1)?, f(2)?); }
            "S" => { s.add_scalar(f(1)?); }
            "L" => { s.add_line(u(1)?, u(2)?); }
            "O" => { s.add_circle(u(1)?, u(2)?); }
            "C" => {
                let kind = *t.get(1).ok_or_else(|| format!("line {lineno}: C with no kind"))?;
                let c = match kind {
                    "fixed" => Constraint::Fixed { p: u(2)?, x: f(3)?, y: f(4)? },
                    "coincident" => Constraint::Coincident { p: u(2)?, q: u(3)? },
                    "distance" => Constraint::Distance { p: u(2)?, q: u(3)?, d: f(4)? },
                    "ponline" => Constraint::PointOnLine { p: u(2)?, line: u(3)? },
                    "symmetric" => Constraint::Symmetric { p: u(2)?, q: u(3)?, line: u(4)? },
                    "horizontal" => Constraint::Horizontal { line: u(2)? },
                    "vertical" => Constraint::Vertical { line: u(2)? },
                    "parallel" => Constraint::Parallel { a: u(2)?, b: u(3)? },
                    "perpendicular" => Constraint::Perpendicular { a: u(2)?, b: u(3)? },
                    "collinear" => Constraint::Collinear { a: u(2)?, b: u(3)? },
                    "equal" => Constraint::EqualLength { a: u(2)?, b: u(3)? },
                    "angle" => Constraint::Angle { a: u(2)?, b: u(3)?, radians: f(4)? },
                    "radius" => Constraint::Radius { circle: u(2)?, r: f(3)? },
                    "concentric" => Constraint::Concentric { a: u(2)?, b: u(3)? },
                    "eqradius" => Constraint::EqualRadius { a: u(2)?, b: u(3)? },
                    "poncircle" => Constraint::PointOnCircle { p: u(2)?, circle: u(3)? },
                    "tangentlc" => Constraint::TangentLineCircle { line: u(2)?, circle: u(3)? },
                    "tangentcc" => Constraint::TangentCircleCircle { a: u(2)?, b: u(3)?, internal: u(4)? != 0 },
                    other => return Err(format!("line {lineno}: unknown constraint `{other}`")),
                };
                s.add(c);
            }
            other => return Err(format!("line {lineno}: unknown record `{other}`")),
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Constraint;

    #[test]
    fn rsmp_round_trips() {
        let mut s = Sketch::new();
        let p0 = s.add_point(0.0, 0.0);
        let p1 = s.add_point(10.0, 0.0);
        let l0 = s.add_line(p0, p1);
        let c0 = s.add_circle_xy(5.0, 5.0, 3.0);
        s.add(Constraint::Fixed { p: p0, x: 0.0, y: 0.0 });
        s.add(Constraint::Distance { p: p0, q: p1, d: 10.0 });
        s.add(Constraint::Horizontal { line: l0 });
        s.add(Constraint::Radius { circle: c0, r: 3.0 });
        s.add(Constraint::TangentCircleCircle { a: c0, b: c0, internal: true });
        s.add(Constraint::Angle { a: l0, b: l0, radians: 1.5 });

        let text = write_rsmp(&s);
        let back = read_rsmp(&text).expect("parse");
        assert_eq!(back.points.len(), 3); // 2 line points + 1 circle center
        assert_eq!(back.scalars.len(), 1);
        assert_eq!(back.lines.len(), 1);
        assert_eq!(back.circles.len(), 1);
        assert_eq!(back.constraints.len(), 6);
        assert_eq!(back.constraints[1], Constraint::Distance { p: 0, q: 1, d: 10.0 });
        assert_eq!(back.circles[0], crate::model::Circle { center: 2, radius: 0 });
        assert_eq!(back.constraints[4], Constraint::TangentCircleCircle { a: 0, b: 0, internal: true });
    }

    #[test]
    fn reads_legacy_v1_header() {
        let s = read_rsmp("RSMP1\nP 0 0\nP 1 1\nL 0 1\nC horizontal 0\n").expect("v1");
        assert_eq!(s.points.len(), 2);
        assert_eq!(s.lines.len(), 1);
        assert_eq!(s.constraints.len(), 1);
    }

    #[test]
    fn rejects_foreign_header() {
        assert!(read_rsmp("DXF\n0\nSECTION\n").is_err());
    }
}
