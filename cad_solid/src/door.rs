//! Parametric panelled DOOR — leaf, lining, architraves and hardware, ported from `DOOR_REBUILD.md`
//! ("Door classic"). Like [`crate::dogleg`]/[`crate::spiral`] this module is PURE geometry (no CSG,
//! no app types) so it can be unit-tested against the spec's numeric acceptance table (§B8); the app
//! turns the returned [`SolidMesh`] into a furniture asset (each component tagged by `face_ids`).
//!
//! Reference frame (spec §B5): **Z up, metres**, `x = 0` centre of the opening, `y = 0` the FRONT
//! face of the wall (the leaf's front face is flush with it, the lining runs back into `-Y`),
//! `z = 0` the finished floor. The whole model is BOXES (the spec's picture-frame mouldings are
//! emitted as four overlapping bars — no boolean, corners mitre by overlap).

use crate::architecture::ArchError;
use crate::SolidMesh;

/// Client inputs, in METRES (the spec's mm defaults ÷ 1000). Only a handful of headline sizes; the
/// rest keep the measured "Door classic" proportions.
#[derive(Clone, Copy, Debug)]
pub struct DoorInput {
    pub door_width: f32,
    pub door_height: f32,
    pub door_thickness: f32,
    pub door_gap_bottom: f32,
    /// How wide the lining reads in the reveal (the board seen edge-on).
    pub frame_face_width: f32,
    /// Front-to-back — THIS IS THE WALL THICKNESS. The lining spans it exactly.
    pub frame_depth: f32,
    /// How far the stop projects into the opening (the rebate).
    pub frame_stop_depth: f32,
    /// Architrave (casing) face width — the SIDE legs, measured across the board.
    pub arch_width: f32,
    /// Architrave HEAD face width — the depth of the band over the opening, read as its "height"
    /// on elevation. Usually equal to [`Self::arch_width`], but a deeper head is a common
    /// (and deliberate) joinery choice, so it is its own number.
    pub arch_head_width: f32,
    /// Architrave projection proud of the wall face.
    pub arch_thickness: f32,
    pub stile_width: f32,
    pub rail_width: f32,
    pub panel_mould_width: f32,
    pub handle_height: f32,
    pub handle_backset: f32,
    pub hinge_count: usize,
    pub hinge_inset: f32,
    /// +1 = hinges on +X edge, −1 = on −X. The lever always goes on the OTHER edge, pointing inward.
    pub hinge_side: f32,
    pub lever_length: f32,
    pub lever_standoff: f32,
    /// Draw the door's OWN rose + lever. False when a handle from the library is being welded on
    /// instead — otherwise the leaf carries two sets of hardware at the same spindle, which is what
    /// happened before this flag existed.
    pub builtin_hardware: bool,
}

impl Default for DoorInput {
    /// The spec's measured "Door classic" (§B1 defaults, mm → m).
    fn default() -> Self {
        Self {
            door_width: 0.79325,
            door_height: 2.09828,
            door_thickness: 0.03287,
            door_gap_bottom: 0.00172,
            frame_face_width: 0.02124,
            frame_depth: 0.11223,
            frame_stop_depth: 0.0129,
            arch_width: 0.08098,
            arch_head_width: 0.08098, // the measured door's head matches its legs
            arch_thickness: 0.01715,
            stile_width: 0.10707,
            rail_width: 0.10723,
            panel_mould_width: 0.05567,
            handle_height: 1.06874,
            handle_backset: 0.05495,
            hinge_count: 2,
            hinge_inset: 0.2582,
            hinge_side: 1.0,
            lever_length: 0.14331,
            lever_standoff: 0.02179,
            builtin_hardware: true,
        }
    }
}

/// Everywhere two parts meet they OVERLAP by this much rather than abut, so no coplanar faces
/// z-fight (spec §A5 third trap).
const OVERLAP: f32 = 0.0001;

/// Which component a triangle belongs to — the mesh `face_ids`, so the app can select/recolour each
/// piece independently (leaf vs lining vs casing vs hardware).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Part {
    Leaf = 1,
    Panel = 2,
    Lining = 3,
    Stop = 4,
    ArchFront = 5,
    ArchBack = 6,
    Hinge = 7,
    Handle = 8,
}

/// Derived sizes (spec §B2). `structural_*` is the hole the wall must leave — the number that gets a
/// door rejected on site, so it is reported prominently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoorMetrics {
    pub rebate_opening_w: f32,
    pub rebate_opening_h: f32,
    pub stop_opening_w: f32,
    pub stop_opening_h: f32,
    pub structural_opening_w: f32,
    pub structural_opening_h: f32,
    pub overall_w: f32,
    pub overall_h: f32,
    pub overall_depth: f32,
    pub panel_w: f32,
    pub panel_h: f32,
    pub frame_depth: f32,
}

/// Validate + derive (spec §B2 / §B7). Hard errors → [`ArchError`].
pub fn plan(inp: &DoorInput) -> Result<(DoorMetrics, Vec<String>), ArchError> {
    if inp.door_width <= 0.0 || inp.door_height <= 0.0 || inp.door_thickness <= 0.0 {
        return Err(ArchError::NonPositive("door size"));
    }
    if inp.door_thickness >= inp.frame_depth {
        return Err(ArchError::NonPositive("leaf thicker than the wall (door_thickness ≥ frame_depth)"));
    }
    if inp.frame_stop_depth >= inp.door_width / 2.0 {
        return Err(ArchError::NonPositive("stop closes the opening (frame_stop_depth ≥ door_width/2)"));
    }
    if inp.stile_width * 2.0 >= inp.door_width {
        return Err(ArchError::NonPositive("stiles fill the leaf (stile_width·2 ≥ door_width)"));
    }
    if inp.rail_width * 2.0 >= inp.door_height {
        return Err(ArchError::NonPositive("rails fill the leaf (rail_width·2 ≥ door_height)"));
    }
    let panel_w = inp.door_width - 2.0 * inp.stile_width;
    let panel_h = inp.door_height - 2.0 * inp.rail_width;
    if inp.panel_mould_width * 2.0 >= panel_w || inp.panel_mould_width * 2.0 >= panel_h {
        return Err(ArchError::NonPositive("panel moulding fills the panel (panel_mould_width·2 ≥ panel)"));
    }
    if inp.handle_backset + inp.lever_length >= inp.door_width {
        return Err(ArchError::NonPositive("lever overruns the leaf (handle_backset + lever_length ≥ door_width)"));
    }
    if inp.arch_width <= 0.0 || inp.arch_head_width <= 0.0 || inp.arch_thickness <= 0.0 {
        return Err(ArchError::NonPositive("architrave size"));
    }
    // The handle must sit ON the leaf, not above its head or below its bottom rail.
    if inp.handle_height <= inp.door_gap_bottom || inp.handle_height >= inp.door_gap_bottom + inp.door_height {
        return Err(ArchError::NonPositive("handle height is off the leaf"));
    }

    let rebate_opening_w = inp.door_width;
    let rebate_opening_h = inp.door_height + inp.door_gap_bottom;
    let m = DoorMetrics {
        rebate_opening_w,
        rebate_opening_h,
        stop_opening_w: rebate_opening_w - 2.0 * inp.frame_stop_depth,
        stop_opening_h: rebate_opening_h - inp.frame_stop_depth,
        structural_opening_w: rebate_opening_w + 2.0 * inp.frame_face_width,
        structural_opening_h: rebate_opening_h + inp.frame_face_width,
        overall_w: rebate_opening_w + 2.0 * inp.arch_width,
        overall_h: rebate_opening_h + inp.arch_head_width,
        overall_depth: inp.frame_depth + 2.0 * inp.arch_thickness,
        panel_w,
        panel_h,
        frame_depth: inp.frame_depth,
    };

    let mut warn = Vec::new();
    warn.push(format!(
        "structural opening {:.0} × {:.0} mm — leave this hole in the wall",
        m.structural_opening_w * 1000.0, m.structural_opening_h * 1000.0,
    ));
    if inp.door_width < 0.750 {
        warn.push(format!("leaf {:.0} mm is below the ~750 mm accessible clear-width minimum", inp.door_width * 1000.0));
    }
    if !(0.900..=1.100).contains(&inp.handle_height) {
        warn.push(format!("handle at {:.0} mm is outside the usual 900–1100 mm", inp.handle_height * 1000.0));
    }
    Ok((m, warn))
}

/// A box `[x0,x1]×[y0,y1]×[z0,z1]` tagged with `part`, as 12 triangles with outward flat normals.
fn push_box(mesh: &mut SolidMesh, part: Part, x: [f32; 2], y: [f32; 2], z: [f32; 2]) {
    let (x0, x1) = (x[0].min(x[1]), x[0].max(x[1]));
    let (y0, y1) = (y[0].min(y[1]), y[0].max(y[1]));
    let (z0, z1) = (z[0].min(z[1]), z[0].max(z[1]));
    if (x1 - x0) < 1e-6 || (y1 - y0) < 1e-6 || (z1 - z0) < 1e-6 {
        return; // a degenerate slab contributes nothing
    }
    let c = [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]];
    // (a, b, c, d) quads wound CCW seen from outside, with the outward normal.
    let quads: [([usize; 4], [f32; 3]); 6] = [
        ([0, 3, 2, 1], [0.0, 0.0, -1.0]), // bottom  z0
        ([4, 5, 6, 7], [0.0, 0.0, 1.0]),  // top     z1
        ([0, 1, 5, 4], [0.0, -1.0, 0.0]), // front   y0
        ([3, 7, 6, 2], [0.0, 1.0, 0.0]),  // back    y1
        ([0, 4, 7, 3], [-1.0, 0.0, 0.0]), // left    x0
        ([1, 2, 6, 5], [1.0, 0.0, 0.0]),  // right   x1
    ];
    for (q, n) in quads {
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            for &vi in &tri {
                mesh.positions.push(c[vi]);
                mesh.normals.push(n);
            }
            mesh.face_ids.push(part as u32);
        }
    }
}

/// Build the whole door as a triangle soup with per-component `face_ids`, in the spec frame (§B5).
/// Every hole is a four-bar picture frame (spec §B4) — boxes only, no boolean.
pub fn build(inp: &DoorInput) -> Result<(DoorMetrics, SolidMesh), ArchError> {
    let (m, _w) = plan(inp)?;
    let mut mesh = SolidMesh::default();
    let (dw, dh, dt) = (inp.door_width, inp.door_height, inp.door_thickness);
    let gz = inp.door_gap_bottom; // leaf floats this far off the floor
    let hw = dw / 2.0;
    let fw = inp.frame_face_width;
    let fd = inp.frame_depth;
    let rw = m.rebate_opening_w; // = door width
    let rh = m.rebate_opening_h;
    let sw = inp.stile_width;
    let rl = inp.rail_width;

    // ── LEAF: two stiles + two rails (the frame) around a raised panel. ──
    let leaf_y = [-dt, 0.0]; // front face flush with the wall (y = 0)
    push_box(&mut mesh, Part::Leaf, [-hw, -hw + sw], leaf_y, [gz, gz + dh]); // left stile
    push_box(&mut mesh, Part::Leaf, [hw - sw, hw], leaf_y, [gz, gz + dh]); // right stile
    push_box(&mut mesh, Part::Leaf, [-hw + sw - OVERLAP, hw - sw + OVERLAP], leaf_y, [gz, gz + rl]); // bottom rail
    push_box(&mut mesh, Part::Leaf, [-hw + sw - OVERLAP, hw - sw + OVERLAP], leaf_y, [gz + dh - rl, gz + dh]); // top rail
    // Panel opening between the members, and a raised field standing 1.23 mm proud of the leaf face.
    let (px0, px1) = (-hw + sw, hw - sw);
    let (pz0, pz1) = (gz + rl, gz + dh - rl);
    push_box(&mut mesh, Part::Panel, [px0, px1], [-dt + 0.006, 0.00123], [pz0, pz1]);
    // A moulded border (four bars) framing the panel on the front, projecting slightly.
    let pm = inp.panel_mould_width.min((px1 - px0) / 2.0 - 0.001).min((pz1 - pz0) / 2.0 - 0.001);
    let mould_y = [-0.001, 0.004];
    push_box(&mut mesh, Part::Leaf, [px0 - OVERLAP, px0 + pm], mould_y, [pz0 - OVERLAP, pz1 + OVERLAP]); // left
    push_box(&mut mesh, Part::Leaf, [px1 - pm, px1 + OVERLAP], mould_y, [pz0 - OVERLAP, pz1 + OVERLAP]); // right
    push_box(&mut mesh, Part::Leaf, [px0 - OVERLAP, px1 + OVERLAP], mould_y, [pz0 - OVERLAP, pz0 + pm]); // bottom
    push_box(&mut mesh, Part::Leaf, [px0 - OVERLAP, px1 + OVERLAP], mould_y, [pz1 - pm, pz1 + OVERLAP]); // top

    // ── LINING: two jambs + head, spanning the wall depth (y = 0 → −frame_depth). ──
    let lin_y = [-fd, 0.0];
    push_box(&mut mesh, Part::Lining, [-rw / 2.0 - fw, -rw / 2.0 + OVERLAP], lin_y, [0.0, rh]); // left jamb
    push_box(&mut mesh, Part::Lining, [rw / 2.0 - OVERLAP, rw / 2.0 + fw], lin_y, [0.0, rh]); // right jamb
    push_box(&mut mesh, Part::Lining, [-rw / 2.0 - fw, rw / 2.0 + fw], lin_y, [rh - OVERLAP, rh + fw]); // head

    // ── STOPS: thin strips behind the leaf, projecting `stop_depth` into the opening. ──
    let sd = inp.frame_stop_depth;
    let stop_y = [-dt - 0.012, -dt + OVERLAP];
    push_box(&mut mesh, Part::Stop, [-rw / 2.0 - OVERLAP, -rw / 2.0 + sd], stop_y, [0.0, m.stop_opening_h]); // left
    push_box(&mut mesh, Part::Stop, [rw / 2.0 - sd, rw / 2.0 + OVERLAP], stop_y, [0.0, m.stop_opening_h]); // right
    push_box(&mut mesh, Part::Stop, [-rw / 2.0, rw / 2.0], stop_y, [m.stop_opening_h - sd, m.stop_opening_h]); // head stop

    // ── ARCHITRAVES: front (on the rebate line) + back (on the stop face), different reveals so the
    //    two casings differ in width (spec §B6.3). Each is a three-bar frame (no bottom casing). ──
    let mut casing = |mesh: &mut SolidMesh, part: Part, inner_w: f32, inner_h: f32, y: [f32; 2]| {
        let ox = inner_w / 2.0 + inp.arch_width;
        // The head band is its own depth — a deeper head over equal legs is deliberate joinery,
        // not a mistake, so it does not follow `arch_width`.
        let oz = inner_h + inp.arch_head_width;
        push_box(mesh, part, [-ox, -inner_w / 2.0 + OVERLAP], y, [0.0, oz]); // left
        push_box(mesh, part, [inner_w / 2.0 - OVERLAP, ox], y, [0.0, oz]); // right
        push_box(mesh, part, [-ox, ox], y, [inner_h - OVERLAP, oz]); // head
    };
    casing(&mut mesh, Part::ArchFront, rw, rh, [0.0, inp.arch_thickness]);
    casing(&mut mesh, Part::ArchBack, m.stop_opening_w, m.stop_opening_h, [-fd - inp.arch_thickness, -fd]);

    // ── HINGES: on the `hinge_side` edge, evenly spaced between the top/bottom insets. ──
    let hinge_x = if inp.hinge_side >= 0.0 { hw } else { -hw };
    let n_h = inp.hinge_count.max(1);
    let (z_lo, z_hi) = (gz + inp.hinge_inset, gz + dh - inp.hinge_inset);
    for k in 0..n_h {
        let t = if n_h == 1 { 0.5 } else { k as f32 / (n_h - 1) as f32 };
        let zc = z_lo + (z_hi - z_lo) * t;
        push_box(&mut mesh, Part::Hinge, [hinge_x - 0.006, hinge_x + 0.006], [-dt, 0.0], [zc - 0.057, zc + 0.057]);
    }

    // ── HANDLE: rose + lever on the OTHER edge, the lever pointing INWARD toward the hinges. ──
    // Skipped when a library handle is being welded on at the same spindle — two levers on one
    // leaf is worse than none, because it reads as a modelling error rather than a missing part.
    if inp.builtin_hardware {
        let lever_dir = if inp.hinge_side >= 0.0 { 1.0 } else { -1.0 }; // toward the hinge side
        let handle_edge = -hinge_x; // opposite the hinges
        let hx = handle_edge + lever_dir * inp.handle_backset; // lever centre, backset in from the leading edge
        let hz = inp.handle_height;
        push_box(&mut mesh, Part::Handle, [hx - 0.0266, hx + 0.0266], [0.0, 0.0066], [hz - 0.0266, hz + 0.0266]); // front rose
        push_box(&mut mesh, Part::Handle, [hx - 0.0266, hx + 0.0266], [-dt - 0.0066, -dt], [hz - 0.0266, hz + 0.0266]); // back rose
        let bar_x = [hx, hx + lever_dir * inp.lever_length]; // toward the hinge side
        let so = inp.lever_standoff;
        push_box(&mut mesh, Part::Handle, bar_x, [so - 0.014, so + 0.014], [hz - 0.014, hz + 0.014]); // front lever
        push_box(&mut mesh, Part::Handle, bar_x, [-dt - so - 0.014, -dt - so + 0.014], [hz - 0.014, hz + 0.014]); // back lever
    }

    Ok((m, mesh))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §B8 — the default parameters must reproduce the measured openings.
    #[test]
    fn reference_openings_match_spec() {
        let (m, _w) = plan(&DoorInput::default()).unwrap();
        let mm = |v: f32| v * 1000.0;
        assert!((mm(m.panel_w) - 579.11).abs() < 0.5, "panel opening w {}", mm(m.panel_w));
        assert!((mm(m.panel_h) - 1883.82).abs() < 0.5, "panel opening h {}", mm(m.panel_h));
        assert!((mm(m.stop_opening_w) - 767.45).abs() < 0.5, "stop opening w {}", mm(m.stop_opening_w));
        assert!((mm(m.stop_opening_h) - 2087.10).abs() < 0.5, "stop opening h {}", mm(m.stop_opening_h));
        // THE number that matters: the hole in the wall.
        assert!((mm(m.structural_opening_w) - 835.73).abs() < 0.5, "structural w {}", mm(m.structural_opening_w));
        assert!((mm(m.structural_opening_h) - 2121.24).abs() < 0.5, "structural h {}", mm(m.structural_opening_h));
        // §B2 casing-to-casing depth (the §B8 "179.61 incl. lever" also counts the lever standoff).
        assert!((mm(m.overall_depth) - 146.53).abs() < 1.0, "casing depth {}", mm(m.overall_depth));
    }

    /// Spec §B8 parametric row — structural opening == door_width + 2·frame_face_width, exactly, for
    /// several very different leaves. Proves it is a tool, not one baked door.
    #[test]
    fn structural_opening_is_parametric() {
        for (w, h, t) in [(0.79325, 2.09828, 0.03287), (0.838, 1.981, 0.035), (0.626, 2.040, 0.040), (0.926, 2.340, 0.044)] {
            let inp = DoorInput { door_width: w, door_height: h, door_thickness: t, ..Default::default() };
            let (m, _w) = plan(&inp).unwrap();
            assert!((m.structural_opening_w - (w + 2.0 * inp.frame_face_width)).abs() < 1e-6, "w={w}");
        }
    }

    /// The build yields a non-empty mesh tagged with every component, all coordinates finite, and a
    /// bounding box matching the overall door (architrave to architrave) + the lever standoff.
    #[test]
    fn build_yields_a_tagged_mesh() {
        let inp = DoorInput::default();
        let (m, mesh) = build(&inp).unwrap();
        assert!(mesh.tri_count() > 0);
        assert_eq!(mesh.face_ids.len(), mesh.tri_count(), "one part id per triangle");
        for p in &mesh.positions { for v in p { assert!(v.is_finite()); } }
        let parts: std::collections::HashSet<u32> = mesh.face_ids.iter().copied().collect();
        for want in [Part::Leaf, Part::Panel, Part::Lining, Part::Stop, Part::ArchFront, Part::ArchBack, Part::Hinge, Part::Handle] {
            assert!(parts.contains(&(want as u32)), "mesh carries {want:?}");
        }
        let (mn, mx) = mesh.bounds().unwrap();
        // X spans the front architrave outline; Z spans floor → head casing.
        assert!((mx[0] - mn[0] - m.overall_w).abs() < 0.01, "width {} vs overall {}", mx[0] - mn[0], m.overall_w);
        assert!((mx[2] - mn[2] - m.overall_h).abs() < 0.05, "height {} vs overall {}", mx[2] - mn[2], m.overall_h);
    }

    /// Spec §B6.2 — the lever must point INWARD (toward the hinge side); flipping the hinge side
    /// flips the lever with it, and it never overruns the leaf.
    #[test]
    fn lever_points_inward() {
        for side in [1.0_f32, -1.0] {
            let inp = DoorInput { hinge_side: side, ..Default::default() };
            let (_m, mesh) = build(&inp).unwrap();
            // Handle triangles only; the lever bar's far end must sit between the leaf edges.
            let mut xs: Vec<f32> = Vec::new();
            for (t, &id) in mesh.face_ids.iter().enumerate() {
                if id == Part::Handle as u32 {
                    for k in 0..3 { xs.push(mesh.positions[t * 3 + k][0]); }
                }
            }
            let hw = inp.door_width / 2.0;
            let (lo, hi) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
            assert!(lo >= -hw - 1e-3 && hi <= hw + 1e-3, "handle stays within the leaf (side {side})");
        }
    }

    /// The head architrave is its OWN depth: raising it must raise the overall height and the
    /// mesh's top, and must NOT touch the width (which the legs own).
    #[test]
    fn the_architrave_head_is_independent_of_its_legs() {
        let base = DoorInput::default();
        let (m0, mesh0) = build(&base).unwrap();
        let tall = DoorInput { arch_head_width: base.arch_width + 0.120, ..base };
        let (m1, mesh1) = build(&tall).unwrap();
        assert!((m1.overall_h - m0.overall_h - 0.120).abs() < 1e-5, "head raises overall height");
        assert!((m1.overall_w - m0.overall_w).abs() < 1e-6, "head leaves the width alone");
        let top = |m: &SolidMesh| m.bounds().unwrap().1[2];
        assert!((top(&mesh1) - top(&mesh0) - 0.120).abs() < 1e-4, "the built mesh really is taller");

        // …and the legs still own the width, without moving the head band's own depth.
        let wide = DoorInput { arch_width: base.arch_width + 0.050, ..base };
        let (m2, _) = build(&wide).unwrap();
        assert!((m2.overall_w - m0.overall_w - 0.100).abs() < 1e-5, "legs widen both sides");
        assert!((m2.overall_h - m0.overall_h).abs() < 1e-6, "legs leave the head alone");
    }

    /// The whole point of `builtin_hardware`: with a library handle welded on, the door must not
    /// also draw its own lever at the same spindle.
    #[test]
    fn the_builtin_lever_can_be_dropped_for_a_library_handle() {
        let with = DoorInput::default();
        let (_m, a) = build(&with).unwrap();
        assert!(a.face_ids.contains(&(Part::Handle as u32)), "default door carries its own lever");

        let without = DoorInput { builtin_hardware: false, ..with };
        let (_m, b) = build(&without).unwrap();
        assert!(!b.face_ids.contains(&(Part::Handle as u32)), "no hardware triangles remain");
        // Nothing ELSE may vanish with it — this must remove the lever, not the door.
        for want in [Part::Leaf, Part::Panel, Part::Lining, Part::Stop, Part::ArchFront, Part::ArchBack, Part::Hinge] {
            assert!(b.face_ids.contains(&(want as u32)), "{want:?} survives");
        }
        assert!(b.tri_count() < a.tri_count(), "and it is strictly less geometry");
    }

    /// The UI offers 50–150 mm backset and 750–1300 mm handle height (the user-facing ranges).
    /// Every corner of that box must build — a slider that can reach an error is a broken slider.
    #[test]
    fn every_offered_handle_position_builds() {
        for backset in [0.050_f32, 0.100, 0.150] {
            for height in [0.750_f32, 1.050, 1.300] {
                for side in [1.0_f32, -1.0] {
                    let inp = DoorInput {
                        handle_backset: backset,
                        handle_height: height,
                        hinge_side: side,
                        ..Default::default()
                    };
                    let (_m, mesh) = build(&inp).expect("backset {backset} height {height}");
                    // The lever must still land on the leaf, at the height asked for.
                    let mut zs: Vec<f32> = Vec::new();
                    for (t, &id) in mesh.face_ids.iter().enumerate() {
                        if id == Part::Handle as u32 {
                            for k in 0..3 { zs.push(mesh.positions[t * 3 + k][2]); }
                        }
                    }
                    let zc = (zs.iter().cloned().fold(f32::MAX, f32::min)
                        + zs.iter().cloned().fold(f32::MIN, f32::max)) / 2.0;
                    assert!((zc - height).abs() < 0.03, "handle centred at {height} m, got {zc}");
                }
            }
        }
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(plan(&DoorInput { door_thickness: 0.2, ..Default::default() }).is_err(), "leaf thicker than wall");
        assert!(plan(&DoorInput { frame_stop_depth: 0.5, ..Default::default() }).is_err(), "stop closes opening");
        assert!(plan(&DoorInput { lever_length: 1.0, ..Default::default() }).is_err(), "lever overruns leaf");
        assert!(plan(&DoorInput { door_width: 0.0, ..Default::default() }).is_err());
        assert!(plan(&DoorInput { arch_head_width: 0.0, ..Default::default() }).is_err(), "no head casing");
        assert!(plan(&DoorInput { handle_height: 3.0, ..Default::default() }).is_err(), "handle above the leaf");
    }
}
