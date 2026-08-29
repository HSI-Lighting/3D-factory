//! Hatch boundary resolution — shared by rendering (cad_app) and DXF export
//! (cad_io) so a hatch's fill loops are computed IDENTICALLY everywhere.
//!
//! A [`Hatch`](crate::geom::Hatch) stores only `boundary_handles` (+ a pattern),
//! not vertices. Resolving means: for each handle, find its boundary dobject in
//! the [`Document`] and tessellate it — if it is a CLOSED loop — into a polygon
//! of world-space [`Vec2`]s. Outer loop first; successive loops are holes under
//! the even-odd rule at fill time.
//!
//! This is a straight lift of what `cad_app` used to do privately, promoted to
//! the kernel per the DXF-interop ruling. cad_app now delegates here.

use crate::document::Document;
use crate::geom::{Ellipse, Geom, Polyline, Spline};
use crate::math::{Vec2, EPS};

/// Tolerance for treating a polyline/spline's first≈last vertices as coincident
/// — i.e. a loop even when its `closed` flag is still false (the "drew a closed
/// shape but forgot to type `c`" case).
const POLYLINE_EFFECTIVELY_CLOSED_EPS: f64 = 1e-3;

/// Resolve every boundary handle of `hatch` against `doc` into a list of
/// closed vertex loops (world coords). Handles that don't resolve, or resolve
/// to a non-closed / unsupported boundary, are silently skipped — so the hatch
/// degrades gracefully if a boundary is deleted. Loop order follows
/// `boundary_handles` (outer first, then holes).
///
/// Supported boundaries: closed (or effectively-closed) `Polyline`, `Circle`,
/// `Ellipse`, and closed `Spline`. Open polylines, arcs and lines would need
/// boundary-chaining and are not resolved here (matches the app's behavior).
pub fn resolve_hatch_loops(hatch: &crate::geom::Hatch, doc: &Document) -> Vec<Vec<Vec2>> {
    let mut loops: Vec<Vec<Vec2>> = Vec::with_capacity(hatch.boundary_handles.len());
    for handle in &hatch.boundary_handles {
        let Some(d) = doc.find_by_handle(*handle) else { continue; };
        // Issue #17 — boundaries on INVISIBLE or FROZEN layers don't
        // contribute to the fill (matches the selection/render gating:
        // `renders` = layer visible && !frozen, plus the dobject's own
        // visibility flag). Otherwise a hatch on a visible layer would
        // trace geometry the user has hidden.
        //
        // EXCEPTION: `hatch_aux` boundaries — the non-rendering synthetic
        // polylines the pick-point trace creates so the Hatch has a
        // boundary to reference. They are invisible BY DESIGN (they must
        // never draw), but the hatch fill must still resolve them.
        let user_hidden = !d.style.visible && !d.style.hatch_aux;
        if user_hidden || !doc.layers.renders(d.style.layer) { continue; }
        let loop_verts: Option<Vec<Vec2>> = match &d.geom {
            Geom::Polyline(p) if polyline_is_effectively_closed(p) => {
                Some(closed_dobject_polygon(&d.geom))
            }
            Geom::Circle(c) => Some(tessellate_circle_loop(c.center, c.radius, 64)),
            Geom::Ellipse(e) => Some(tessellate_ellipse_loop(e, 64)),
            Geom::Spline(s) if spline_is_effectively_closed(s) => {
                Some(closed_dobject_polygon(&d.geom))
            }
            _ => None,
        };
        if let Some(v) = loop_verts {
            loops.push(v);
        }
    }
    loops
}

/// Tessellate a circle into an N-vertex closed loop (world coords).
fn tessellate_circle_loop(centre: Vec2, radius: f64, n: usize) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f64) / (n as f64) * std::f64::consts::TAU;
        out.push(Vec2::new(centre.x + radius * t.cos(), centre.y + radius * t.sin()));
    }
    out
}

/// Tessellate an ellipse into an N-vertex closed loop via the kernel `point_at`.
fn tessellate_ellipse_loop(e: &Ellipse, n: usize) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f64) / (n as f64) * std::f64::consts::TAU;
        out.push(e.point_at(t));
    }
    out
}

/// True if `p` should be treated as a closed loop: `closed` set, OR first≈last
/// within `POLYLINE_EFFECTIVELY_CLOSED_EPS`. Needs ≥3 vertices to form a loop.
fn polyline_is_effectively_closed(p: &Polyline) -> bool {
    if p.vertices.len() < 3 { return false; }
    if p.closed { return true; }
    let first = p.vertices.first().map(|v| v.pos);
    let last = p.vertices.last().map(|v| v.pos);
    match (first, last) {
        (Some(a), Some(b)) => (a - b).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS,
        _ => false,
    }
}

/// True if a spline is an effectively-closed loop: ≥3 control points and its
/// first ≈ last control point (CAD "Close" appends the first cp).
fn spline_is_effectively_closed(s: &Spline) -> bool {
    let n = s.control_points.len();
    if n < 3 { return false; }
    (s.control_points[0] - s.control_points[n - 1]).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS
}

/// Tessellate any single closed geometry to a world-coord vertex loop. Drops a
/// duplicated closing vertex so the polygon doesn't double-count its start edge.
/// Returns empty for anything not a supported closed loop.
fn closed_dobject_polygon(g: &Geom) -> Vec<Vec2> {
    match g {
        Geom::Polyline(p) if polyline_is_effectively_closed(p) => {
            let n = p.vertices.len();
            let effective_n = if !p.closed {
                let f = p.vertices[0].pos;
                let l = p.vertices[n - 1].pos;
                if (f - l).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS { n - 1 } else { n }
            } else {
                n
            };
            let mut v: Vec<Vec2> = Vec::with_capacity(effective_n * 4);
            v.push(p.vertices[0].pos);
            for k in 0..effective_n {
                let a = p.vertices[k].pos;
                let b = p.vertices[(k + 1) % effective_n].pos;
                append_arc_world_samples(a, b, p.vertices[k].bulge, &mut v);
            }
            v
        }
        Geom::Circle(c) => tessellate_circle_loop(c.center, c.radius, 64),
        Geom::Ellipse(e) => tessellate_ellipse_loop(e, 64),
        Geom::Spline(s) if spline_is_effectively_closed(s) => {
            let mut v = s.tessellate(64);
            if v.len() >= 2 {
                let (f, l) = (v[0], v[v.len() - 1]);
                if (f - l).len() < POLYLINE_EFFECTIVELY_CLOSED_EPS { v.pop(); }
            }
            v
        }
        _ => Vec::new(),
    }
}

/// Append world-space samples of the arc from `a` to `b` implied by `bulge`
/// (0 = straight → just `b`). Mirrors the polyline-bulge tessellation.
fn append_arc_world_samples(a: Vec2, b: Vec2, bulge: f64, out: &mut Vec<Vec2>) {
    if bulge.abs() < 1e-9 {
        out.push(b);
        return;
    }
    let chord = b - a;
    let chord_len = chord.len();
    if chord_len < EPS {
        out.push(b);
        return;
    }
    let theta = 4.0 * bulge.atan();
    let half = theta * 0.5;
    let sin_half = half.sin();
    if sin_half.abs() < EPS {
        out.push(b);
        return;
    }
    let r = chord_len / (2.0 * sin_half.abs());
    let chord_hat = chord / chord_len;
    let perp = Vec2::new(-chord_hat.y, chord_hat.x);
    let mid = (a + b) * 0.5;
    let centre_off = r * half.cos();
    let centre = mid + perp * (if bulge > 0.0 { centre_off } else { -centre_off });
    let start_ang = (a - centre).angle();
    let end_ang = (b - centre).angle();
    let sweep = if bulge > 0.0 {
        (end_ang - start_ang).rem_euclid(std::f64::consts::TAU)
    } else {
        -((start_ang - end_ang).rem_euclid(std::f64::consts::TAU))
    };
    let n: usize = 24;
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let ang = start_ang + sweep * t;
        out.push(centre + Vec2::new(r * ang.cos(), r * ang.sin()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dobject::DObject;
    use crate::geom::{Circle, PolyVertex};

    fn closed_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(x0, y0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(x1, y0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(x1, y1), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(x0, y1), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        }
    }

    #[test]
    fn resolves_a_closed_rectangle_boundary() {
        let mut doc = Document::default();
        let idx = doc.push(Geom::Polyline(closed_rect(0.0, 0.0, 10.0, 5.0)).into());
        let handle = doc.dobjects[idx].handle;
        let hatch = crate::geom::Hatch {
            boundary_handles: vec![handle],
            pattern: crate::geom::HatchPattern::Solid,
        };
        let loops = resolve_hatch_loops(&hatch, &doc);
        assert_eq!(loops.len(), 1, "one boundary → one loop");
        // closed_dobject_polygon repeats the first vertex as the closing one, so
        // a 4-corner rectangle resolves to 5 points (v0..v3,v0); the DXF writer
        // drops that trailing duplicate.
        assert_eq!(loops[0].len(), 5, "rectangle → 4 corners + duplicated close");
        assert_eq!(loops[0][0], loops[0][4], "loop closes back on its first vertex");
    }

    #[test]
    fn resolves_two_loops_outer_and_hole() {
        let mut doc = Document::default();
        let oi = doc.push(Geom::Polyline(closed_rect(0.0, 0.0, 10.0, 10.0)).into());
        let outer = doc.dobjects[oi].handle;
        let ci = doc.push(Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 2.0 }).into());
        let hole = doc.dobjects[ci].handle;
        let hatch = crate::geom::Hatch {
            boundary_handles: vec![outer, hole],
            pattern: crate::geom::HatchPattern::Solid,
        };
        let loops = resolve_hatch_loops(&hatch, &doc);
        assert_eq!(loops.len(), 2, "outer + hole → two loops");
        assert_eq!(loops[0].len(), 5, "outer rectangle → 4 corners + duplicated close");
        assert_eq!(loops[1].len(), 64, "circle hole → 64 samples");
    }

    #[test]
    fn skips_unresolvable_and_open_boundaries() {
        let mut doc = Document::default();
        // A dangling handle (never in the doc) + an open 2-point polyline.
        let open = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(1.0, 0.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let oi = doc.push(Geom::Polyline(open).into());
        let open_h = doc.dobjects[oi].handle;
        let hatch = crate::geom::Hatch {
            boundary_handles: vec![u64::MAX, open_h],   // Handle = u64; MAX never allocated
            pattern: crate::geom::HatchPattern::Solid,
        };
        let loops = resolve_hatch_loops(&hatch, &doc);
        assert!(loops.is_empty(), "dangling + open boundaries resolve to nothing");
    }

    #[test]
    fn skips_boundaries_on_invisible_or_frozen_layers() {
        // Issue #17 — a hatch on a visible layer must NOT trace boundaries
        // whose layer is turned off or frozen (or whose dobject is hidden).
        let mut doc = Document::default();
        // A layer that is invisible + one that is frozen.
        let mut mk_layer = |name: &str, visible: bool, frozen: bool| {
            doc.layers.add(crate::layer::Layer {
                name: name.into(), color: crate::color::Color::Aci(7),
                linetype: 0, lineweight: crate::lineweight::Lineweight::Default,
                visible, locked: false, frozen, plottable: true, order: 0,
            })
        };
        let off_id = mk_layer("OFF", false, false);
        let frozen_id = mk_layer("FROZEN", true, true);
        // Boundary on the hidden layer.
        let mut off_d = DObject::new(Geom::Polyline(closed_rect(0.0, 0.0, 10.0, 5.0)));
        off_d.style.layer = off_id;
        let off_h = off_d.handle;
        doc.push(off_d);
        // Boundary on the frozen layer.
        let mut fr_d = DObject::new(Geom::Polyline(closed_rect(0.0, 0.0, 20.0, 5.0)));
        fr_d.style.layer = frozen_id;
        let fr_h = fr_d.handle;
        doc.push(fr_d);
        // Boundary whose dobject is individually hidden (still visible layer).
        let mut hid_d = DObject::new(Geom::Polyline(closed_rect(0.0, 0.0, 30.0, 5.0)));
        hid_d.style.visible = false;
        let hid_h = hid_d.handle;
        doc.push(hid_d);
        // A valid visible boundary.
        let ok_d = DObject::new(Geom::Polyline(closed_rect(0.0, 0.0, 5.0, 5.0)));
        let ok_h = ok_d.handle;
        doc.push(ok_d);
        // A SYNTHETIC hatch-aux boundary: invisible by design (never drawn),
        // but the fill must still resolve it — the pick-point trace creates
        // these for the Hatch to reference by handle.
        let mut aux_d = DObject::new(Geom::Polyline(closed_rect(0.0, 0.0, 40.0, 5.0)));
        aux_d.style.visible = false;
        aux_d.style.hatch_aux = true;
        let aux_h = aux_d.handle;
        doc.push(aux_d);

        let hatch = crate::geom::Hatch {
            boundary_handles: vec![off_h, fr_h, hid_h, ok_h, aux_h],
            pattern: crate::geom::HatchPattern::Solid,
        };
        let loops = resolve_hatch_loops(&hatch, &doc);
        assert_eq!(loops.len(), 2, "only the visible + the aux boundary contribute");
        assert_eq!(loops[0][0], Vec2::new(0.0, 0.0));
        assert_eq!(loops[0][2], Vec2::new(5.0, 5.0));
        assert_eq!(loops[1][2], Vec2::new(40.0, 5.0), "aux boundary resolves");
    }
}
