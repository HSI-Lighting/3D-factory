//! Modifier subsystem — the 3D-solid analog of simLUX's 2D modify layer.
//!
//! Conforms to `mentor MD/BASIC_MODIFIERS_RULES.md` (the extracted RUST_CAD
//! contract). Mapping so a later merge is 1:1:
//!
//! | 2D (app.rs `RotateState`/…)          | here                                     |
//! |--------------------------------------|------------------------------------------|
//! | `RotateState::WaitingForPivot/Angle` | `first: None`/`Some` + `Ref::None`       |
//! | `RotateState::WaitingForRefSrc1/2/Tgt`| `Ref::RotSrc1/RotSrc2/RotTgt`           |
//! | `rotate_copy`/`scale_copy` flags     | `copy: bool`                             |
//! | `card_anchor` (no anchor at pivot)   | `card` applies only at the 2nd+ pick     |
//! | `apply_rotate_or_copy` / `apply_scale`| `Modify::feed()`/`type_value()`          |
//!
//! ROTATE (spec §3): pivot → angle. The angle is the **absolute pivot→cursor angle
//! from +X, CCW positive** (click) or **typed degrees** (CCW+). `C` toggles copy,
//! `R` enters the 2-point reference-direction sub-flow. CARD snaps the angle to 90°.
//! SCALE (§4): pivot → factor (= distance from pivot) or typed factor; `R` = old/new
//! reference length; `C` = copy. All transform math reuses `Feature::rotated/…`,
//! which delegate to the shared `cad_kernel` geometry — never reimplemented here.

use std::f32::consts::{FRAC_PI_2, PI};

use glam::{Quat, Vec2, Vec3};

use crate::{Model, Plane};

/// The basic set-transform modifiers (the 2D app's move/copy/rotate/scale/mirror).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModifyOp {
    Move,
    Copy,
    Rotate,
    Scale,
    Mirror,
}

impl ModifyOp {
    pub const ALL: [ModifyOp; 5] =
        [ModifyOp::Move, ModifyOp::Copy, ModifyOp::Rotate, ModifyOp::Scale, ModifyOp::Mirror];

    pub fn label(self) -> &'static str {
        match self {
            ModifyOp::Move => "Move",
            ModifyOp::Copy => "Copy",
            ModifyOp::Rotate => "Rotate",
            ModifyOp::Scale => "Scale",
            ModifyOp::Mirror => "Mirror",
        }
    }
}

/// Outcome of feeding a pick to an in-flight op.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feed {
    /// Stored the base/pivot (or a reference point); waiting for the next point.
    NeedMore,
    /// Applied and finished — the caller should drop the `Modify`.
    Applied,
    /// Applied a COPY and is ready for another drop — keep the `Modify` alive.
    AppliedContinue,
}

/// The reference-direction / reference-length sub-flow for ROTATE and SCALE
/// (`Ref::None` = the default single-value path). Mirrors `RotateState`/`ScaleState`
/// `WaitingForRef…` in the app.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ref {
    None,
    RotSrc1,        // rotate-R: waiting SOURCE 1
    RotSrc2(Vec2),  // rotate-R: waiting SOURCE 2 (holds src1)
    RotTgt(f32),    // rotate-R: waiting NEW direction (holds source angle, rad)
    ScaStart,       // scale-R: waiting REFERENCE start
    ScaEnd(Vec2),   // scale-R: waiting REFERENCE end (holds start)
    ScaNew(f32),    // scale-R: waiting NEW length (holds reference distance)
}

/// An in-flight modify command over a set of selected features, echoing the 2D
/// select-first → base → second-point flow.
#[derive(Clone, Debug)]
pub struct Modify {
    pub op: ModifyOp,
    pub targets: Vec<u32>,
    /// Copy toggle (rotate/scale). Mirror's "keep original" is separate.
    pub copy: bool,
    /// Human-readable summary of the last apply, for the session recorder (§8).
    pub last_summary: Option<String>,
    /// base / pivot / mirror-A as a FULL 3D world point (`None` until first pick).
    /// Stored in world space — NOT pre-flattened to `(u,v)` — so a pick snapped to a
    /// solid vertex keeps its true Z; Move/Copy translate by the full 3D delta.
    first: Option<Vec3>,
    /// Reference sub-flow state (rotate/scale only).
    refm: Ref,
}

impl Modify {
    pub fn new(op: ModifyOp, targets: Vec<u32>) -> Self {
        Self { op, targets, copy: false, last_summary: None, first: None, refm: Ref::None }
    }

    /// Whether the base/pivot/axis-A has been picked yet.
    pub fn has_base(&self) -> bool {
        self.first.is_some()
    }

    /// The NAME of the pick we are currently waiting for (used by the recorder so a
    /// dump reads "rotate PIVOT = …" / "rotate ANGLE = …", not a bare vector).
    pub fn pick_name(&self) -> &'static str {
        if self.first.is_none() {
            return match self.op {
                ModifyOp::Move | ModifyOp::Copy => "BASE",
                ModifyOp::Rotate | ModifyOp::Scale => "PIVOT",
                ModifyOp::Mirror => "AXIS-1",
            };
        }
        match (self.op, self.refm) {
            (ModifyOp::Rotate, Ref::None) => "ANGLE",
            (ModifyOp::Rotate, Ref::RotSrc1) => "REF-SRC-1",
            (ModifyOp::Rotate, Ref::RotSrc2(_)) => "REF-SRC-2",
            (ModifyOp::Rotate, Ref::RotTgt(_)) => "REF-TARGET",
            (ModifyOp::Scale, Ref::None) => "FACTOR",
            (ModifyOp::Scale, Ref::ScaStart) => "REF-START",
            (ModifyOp::Scale, Ref::ScaEnd(_)) => "REF-END",
            (ModifyOp::Scale, Ref::ScaNew(_)) => "NEW-LENGTH",
            (ModifyOp::Move, _) => "DESTINATION",
            (ModifyOp::Copy, _) => "DESTINATION",
            (ModifyOp::Mirror, _) => "AXIS-2",
            _ => "POINT",
        }
    }

    /// The current prompt (which point / option we're waiting for).
    pub fn prompt(&self) -> String {
        if self.first.is_none() {
            return format!("{}: pick {}", self.op.label().to_lowercase(), self.pick_name());
        }
        let cp = if self.copy { "ON" } else { "off" };
        match (self.op, self.refm) {
            (ModifyOp::Move, _) => "move: pick DESTINATION".into(),
            (ModifyOp::Copy, _) => "copy: pick DESTINATION".into(),
            (ModifyOp::Mirror, _) => "mirror: pick AXIS-2 point".into(),
            (ModifyOp::Rotate, Ref::None) => {
                format!("rotate: pick ANGLE, or type degrees (CCW+) · R=reference · C=copy {cp}")
            }
            (ModifyOp::Rotate, Ref::RotSrc1) => "rotate-R: pick SOURCE point 1 (current direction)".into(),
            (ModifyOp::Rotate, Ref::RotSrc2(_)) => "rotate-R: pick SOURCE point 2 (current direction)".into(),
            (ModifyOp::Rotate, Ref::RotTgt(_)) => "rotate-R: pick NEW direction (anchored at pivot) or type angle".into(),
            (ModifyOp::Scale, Ref::None) => {
                format!("scale: pick FACTOR (dist from pivot), or type factor · R=reference · C=copy {cp}")
            }
            (ModifyOp::Scale, Ref::ScaStart) => "scale-R: pick REFERENCE start (old length)".into(),
            (ModifyOp::Scale, Ref::ScaEnd(_)) => "scale-R: pick REFERENCE end (old length)".into(),
            (ModifyOp::Scale, Ref::ScaNew(_)) => "scale-R: pick NEW length (dist from pivot) or type number".into(),
            _ => self.op.label().to_lowercase(),
        }
    }

    /// The gathered base/pivot in world space, if any (for a rubber-band preview).
    pub fn anchor_world(&self, _plane: &Plane) -> Option<Vec3> {
        self.first
    }

    /// Live ROTATE angle (rad) for the cursor at `cursor_world`, for the ghost +
    /// degree label. `None` unless we're actually waiting on a rotate angle.
    pub fn preview_angle(&self, plane: &Plane, cursor_world: Vec3, card: bool) -> Option<f32> {
        if self.op != ModifyOp::Rotate {
            return None;
        }
        let first_uv = plane.to_uv(self.first?);
        let cursor_uv = plane.to_uv(cursor_world);
        match self.refm {
            Ref::None => Some(angle_from(first_uv, cursor_uv, card)),
            Ref::RotTgt(src) => {
                let d = cursor_uv - first_uv;
                Some(norm_pi(d.y.atan2(d.x) - src))
            }
            _ => None,
        }
    }

    /// Live SCALE factor for the cursor at `cursor_world`, for the ghost + `×f` label.
    pub fn preview_factor(&self, plane: &Plane, cursor_world: Vec3) -> Option<f32> {
        if self.op != ModifyOp::Scale {
            return None;
        }
        let first_uv = plane.to_uv(self.first?);
        let cursor_uv = plane.to_uv(cursor_world);
        match self.refm {
            Ref::None => Some((cursor_uv - first_uv).length().max(1e-4)),
            Ref::ScaNew(ref_d) => Some(((cursor_uv - first_uv).length() / ref_d).max(1e-4)),
            _ => None,
        }
    }

    /// Feed a pick — a world point on `plane`. First call stores the base/pivot;
    /// later calls advance the reference sub-flow or apply the op.
    pub fn feed(&mut self, world: Vec3, plane: &Plane, model: &mut Model, card: bool) -> Feed {
        let Some(first_w) = self.first else {
            self.first = Some(world); // keep the FULL 3D pick (snapped vertices carry Z)
            return Feed::NeedMore;
        };
        // In-plane `(u,v)` coordinates for the planar ops (rotate/scale/mirror).
        let uv = plane.to_uv(world);
        let first_uv = plane.to_uv(first_w);

        match self.op {
            ModifyOp::Move => {
                // FULL 3D delta: snapping base=top-corner, dest=bottom-corner must move
                // by the height too. CARD locks only the in-plane part; Z is preserved.
                let wd = world_delta_carded(plane, first_w, world, card);
                self.apply_translate(model, wd, false);
                self.last_summary = Some(format!("move Δ({:.2},{:.2},{:.2})", wd.x, wd.y, wd.z));
                Feed::Applied
            }
            ModifyOp::Copy => {
                // SINGLE-DROP, like RUST_CAD `apply_copy` (spec §2): base → destination
                // → done. Full 3D delta (see Move) so snapped picks lift by their Z.
                let wd = world_delta_carded(plane, first_w, world, card);
                self.apply_translate(model, wd, true);
                self.last_summary = Some(format!("copy Δ({:.2},{:.2},{:.2})", wd.x, wd.y, wd.z));
                Feed::Applied
            }
            ModifyOp::Rotate => match self.refm {
                Ref::None => {
                    let ang = angle_from(first_uv, uv, card);
                    self.apply_rotate(plane, model, first_w, ang);
                    Feed::Applied
                }
                Ref::RotSrc1 => {
                    self.refm = Ref::RotSrc2(uv);
                    Feed::NeedMore
                }
                Ref::RotSrc2(s1) => {
                    let d = uv - s1;
                    self.refm = Ref::RotTgt(d.y.atan2(d.x));
                    Feed::NeedMore
                }
                Ref::RotTgt(src) => {
                    let d = uv - first_uv;
                    let dtheta = norm_pi(d.y.atan2(d.x) - src);
                    self.apply_rotate(plane, model, first_w, dtheta);
                    Feed::Applied
                }
                _ => Feed::NeedMore,
            },
            ModifyOp::Scale => match self.refm {
                Ref::None => {
                    let k = (uv - first_uv).length().max(1e-4);
                    self.apply_scale(plane, model, first_w, k);
                    Feed::Applied
                }
                Ref::ScaStart => {
                    self.refm = Ref::ScaEnd(uv);
                    Feed::NeedMore
                }
                Ref::ScaEnd(start) => {
                    let ref_d = (uv - start).length().max(1e-4);
                    self.refm = Ref::ScaNew(ref_d);
                    Feed::NeedMore
                }
                Ref::ScaNew(ref_d) => {
                    let k = ((uv - first_uv).length() / ref_d).max(1e-4);
                    self.apply_scale(plane, model, first_w, k);
                    Feed::Applied
                }
                _ => Feed::NeedMore,
            },
            ModifyOp::Mirror => {
                let b_uv = if card { first_uv + card_lock(uv - first_uv, true) } else { uv };
                let a_w = plane.from_uv(first_uv);
                let b_w = plane.from_uv(b_uv);
                let line = (b_w - a_w).normalize_or_zero();
                let mirror_n = plane.normal().cross(line).normalize_or_zero();
                for id in &self.targets {
                    if let Some(f) = model.get_mut(*id) {
                        *f = f.mirrored(a_w, mirror_n);
                    }
                }
                self.last_summary = Some(format!("mirror across ({:.2},{:.2})–({:.2},{:.2})", a_w.x, a_w.y, b_w.x, b_w.y));
                Feed::Applied
            }
        }
    }

    /// Typed command-line input while a pick is pending: degrees / factor / `R` / `C`.
    /// Returns `Some(Feed)` if consumed, `None` if the text isn't a value/keyword for
    /// this op (so the caller can treat it as a new command → override the old one).
    pub fn type_value(&mut self, text: &str, plane: &Plane, model: &mut Model) -> Option<Feed> {
        let t = text.trim().to_lowercase();
        let first = self.first?; // no typed value before the base/pivot
        match self.op {
            ModifyOp::Rotate => {
                if matches!(t.as_str(), "r" | "ref" | "reference") {
                    self.refm = Ref::RotSrc1;
                    return Some(Feed::NeedMore);
                }
                if matches!(t.as_str(), "c" | "cp" | "copy") {
                    self.copy = !self.copy;
                    return Some(Feed::NeedMore);
                }
                let deg = t.parse::<f32>().ok()?;
                let ang = match self.refm {
                    Ref::RotTgt(src) => norm_pi(deg.to_radians() - src),
                    _ => deg.to_radians(),
                };
                self.apply_rotate(plane, model, first, ang);
                Some(Feed::Applied)
            }
            ModifyOp::Scale => {
                if matches!(t.as_str(), "r" | "ref" | "reference") {
                    self.refm = Ref::ScaStart;
                    return Some(Feed::NeedMore);
                }
                if matches!(t.as_str(), "c" | "cp" | "copy") {
                    self.copy = !self.copy;
                    return Some(Feed::NeedMore);
                }
                let n = t.parse::<f32>().ok()?;
                let k = match self.refm {
                    Ref::ScaNew(ref_d) => (n / ref_d).max(1e-4),
                    _ => n.max(1e-4),
                };
                self.apply_scale(plane, model, first, k);
                Some(Feed::Applied)
            }
            // Move/Copy: direct-distance entry is a later parity item; Mirror has no
            // typed value. Not consumed → caller falls through to command parsing.
            _ => None,
        }
    }

    // ---- apply helpers (honour the copy flag; set last_summary for the recorder) ----

    fn apply_translate(&mut self, model: &mut Model, wd: Vec3, copy: bool) {
        if copy {
            let dupes: Vec<_> = self
                .targets
                .iter()
                .filter_map(|id| model.features.iter().find(|f| f.id == *id).copied())
                .map(|f| f.translated(wd))
                .collect();
            for d in dupes {
                model.push_feature(d);
            }
        } else {
            for id in &self.targets {
                if let Some(f) = model.get_mut(*id) {
                    *f = f.translated(wd);
                }
            }
        }
    }

    fn apply_rotate(&mut self, plane: &Plane, model: &mut Model, pivot: Vec3, ang: f32) {
        let axis = plane.normal();
        if self.copy {
            let dupes: Vec<_> = self
                .targets
                .iter()
                .filter_map(|id| model.features.iter().find(|f| f.id == *id).copied())
                .map(|f| f.rotated(pivot, axis, ang))
                .collect();
            for d in dupes {
                model.push_feature(d);
            }
            self.last_summary =
                Some(format!("rotate-copy {:.1}° about ({:.2},{:.2})", ang.to_degrees(), pivot.x, pivot.y));
        } else {
            for id in &self.targets {
                if let Some(f) = model.get_mut(*id) {
                    *f = f.rotated(pivot, axis, ang);
                }
            }
            self.last_summary =
                Some(format!("rotate {:.1}° about ({:.2},{:.2})", ang.to_degrees(), pivot.x, pivot.y));
        }
    }

    fn apply_scale(&mut self, _plane: &Plane, model: &mut Model, pivot: Vec3, k: f32) {
        if self.copy {
            let dupes: Vec<_> = self
                .targets
                .iter()
                .filter_map(|id| model.features.iter().find(|f| f.id == *id).copied())
                .map(|f| f.scaled(pivot, k))
                .collect();
            for d in dupes {
                model.push_feature(d);
            }
            self.last_summary = Some(format!("scale-copy ×{:.3} about ({:.2},{:.2})", k, pivot.x, pivot.y));
        } else {
            for id in &self.targets {
                if let Some(f) = model.get_mut(*id) {
                    *f = f.scaled(pivot, k);
                }
            }
            self.last_summary = Some(format!("scale ×{:.3} about ({:.2},{:.2})", k, pivot.x, pivot.y));
        }
    }
}

/// Rotate a world point about `pivot` around `axis` by `ang` (rad) — for the ghost.
pub fn rot_about(p: Vec3, pivot: Vec3, axis: Vec3, ang: f32) -> Vec3 {
    Quat::from_axis_angle(axis.normalize_or_zero(), ang) * (p - pivot) + pivot
}

/// Scale a world point about `pivot` by `k` — for the ghost.
pub fn scale_about(p: Vec3, pivot: Vec3, k: f32) -> Vec3 {
    pivot + (p - pivot) * k
}

/// The absolute pivot→cursor angle from +X, CCW positive; CARD snaps it to 90°.
fn angle_from(pivot: Vec2, cursor: Vec2, card: bool) -> f32 {
    let d = cursor - pivot;
    let a = d.y.atan2(d.x);
    if card {
        (a / FRAC_PI_2).round() * FRAC_PI_2
    } else {
        a
    }
}

/// Normalise an angle to (−π, π].
fn norm_pi(mut a: f32) -> f32 {
    a %= 2.0 * PI;
    if a > PI {
        a -= 2.0 * PI;
    } else if a <= -PI {
        a += 2.0 * PI;
    }
    a
}

/// Lift an in-plane `(u, v)` delta to a world vector.
fn plane_delta(plane: &Plane, d: Vec2) -> Vec3 {
    plane.from_uv(d) - plane.origin()
}

/// The FULL 3D world delta `to − from`, with CARD applied only to the in-plane
/// component (the out-of-plane / normal component is always preserved). With `card`
/// off this returns exactly `to − from`, so a Move/Copy between two snapped solid
/// vertices carries their real height difference (the fix for "copy went 2D").
fn world_delta_carded(plane: &Plane, from: Vec3, to: Vec3, card: bool) -> Vec3 {
    let d = to - from;
    if !card {
        return d;
    }
    // Decompose into plane basis: in-plane (u,v) + normal, lock the in-plane part.
    let n = plane.normal();
    let normal_comp = d.dot(n);
    let uv_delta = plane.to_uv(to) - plane.to_uv(from);
    plane_delta(plane, card_lock(uv_delta, true)) + n * normal_comp
}

/// CARD cardinal lock: collapse a delta to its dominant axis (the 2D
/// `apply_constraints` H/V snap).
fn card_lock(d: Vec2, card: bool) -> Vec2 {
    if card {
        if d.x.abs() >= d.y.abs() {
            Vec2::new(d.x, 0.0)
        } else {
            Vec2::new(0.0, d.y)
        }
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoolOp, Placement, Primitive};

    fn model_with_box(u: f32, v: f32) -> (Model, u32) {
        let mut m = Model::default();
        let id = m.push(
            BoolOp::Union,
            Plane::default(),
            Placement { u, v, lift: 0.0, spin_deg: 0.0 },
            Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
        );
        (m, id)
    }

    #[test]
    fn move_two_picks_translates_target() {
        let (mut m, id) = model_with_box(0.0, 0.0);
        let mut op = Modify::new(ModifyOp::Move, vec![id]);
        let plane = Plane::default();
        assert_eq!(op.feed(Vec3::ZERO, &plane, &mut m, false), Feed::NeedMore);
        assert_eq!(op.feed(Vec3::new(2.0, 1.0, 0.0), &plane, &mut m, false), Feed::Applied);
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(2.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn move_with_card_locks_to_dominant_axis() {
        let (mut m, id) = model_with_box(0.0, 0.0);
        let mut op = Modify::new(ModifyOp::Move, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, true);
        op.feed(Vec3::new(3.0, 0.4, 0.0), &plane, &mut m, true);
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4, "y locked out, got {o:?}");
    }

    #[test]
    fn copy_carries_full_3d_delta_when_snapped() {
        // base snapped to a top corner (z=1), dest to a bottom corner (z=0) → the
        // duplicate must drop by the height, not stay flat (the "copy went 2D" bug).
        let (mut m, id) = model_with_box(0.0, 0.0);
        let mut op = Modify::new(ModifyOp::Copy, vec![id]);
        let plane = Plane::default(); // XY plane, normal +Z
        op.feed(Vec3::new(1.0, 1.0, 1.0), &plane, &mut m, false);
        let r = op.feed(Vec3::new(1.0, 1.0, 0.0), &plane, &mut m, false);
        assert_eq!(r, Feed::Applied);
        assert_eq!(m.features.len(), 2);
        let orig = m.features[0].world_origin();
        let dup = m.features[1].world_origin();
        assert!((dup - (orig + Vec3::new(0.0, 0.0, -1.0))).length() < 1e-4, "copy carried Δz, got {dup:?}");
    }

    #[test]
    fn move_card_locks_in_plane_but_keeps_z() {
        let (mut m, id) = model_with_box(0.0, 0.0);
        let mut op = Modify::new(ModifyOp::Move, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, true);
        // in-plane mostly +x (locks out the small +y) but +2 in Z must survive.
        op.feed(Vec3::new(3.0, 0.4, 2.0), &plane, &mut m, true);
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(3.0, 0.0, 2.0)).length() < 1e-4, "x locked, y dropped, z kept, got {o:?}");
    }

    #[test]
    fn copy_adds_one_feature_then_finishes() {
        let (mut m, id) = model_with_box(0.0, 0.0);
        let mut op = Modify::new(ModifyOp::Copy, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false);
        let r = op.feed(Vec3::new(4.0, 0.0, 0.0), &plane, &mut m, false);
        assert_eq!(r, Feed::Applied, "single-drop: base → dest → done");
        assert_eq!(m.features.len(), 2, "copy adds one feature");
        assert!((m.features[0].world_origin() - Vec3::ZERO).length() < 1e-4);
        assert!((m.features[1].world_origin() - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn rotate_90_moves_target_across() {
        let (mut m, id) = model_with_box(1.0, 0.0);
        let mut op = Modify::new(ModifyOp::Rotate, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false); // pivot at origin
        op.feed(Vec3::new(0.0, 1.0, 0.0), &plane, &mut m, false); // angle point on +v → +90°
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4, "rotated (1,0)→(0,1), got {o:?}");
    }

    #[test]
    fn rotate_typed_degrees_ccw_positive() {
        let (mut m, id) = model_with_box(1.0, 0.0);
        let mut op = Modify::new(ModifyOp::Rotate, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false); // pivot
        let r = op.type_value("90", &plane, &mut m).expect("typed degrees consumed");
        assert_eq!(r, Feed::Applied);
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4, "90° CCW (1,0)→(0,1), got {o:?}");
        assert!(op.last_summary.as_deref().unwrap().contains("90"));
    }

    #[test]
    fn rotate_copy_toggle_keeps_original() {
        let (mut m, id) = model_with_box(1.0, 0.0);
        let mut op = Modify::new(ModifyOp::Rotate, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false); // pivot
        assert_eq!(op.type_value("c", &plane, &mut m), Some(Feed::NeedMore)); // copy ON
        assert!(op.copy);
        op.type_value("90", &plane, &mut m);
        assert_eq!(m.features.len(), 2, "copy adds a rotated feature, original kept");
        assert!((m.features[0].world_origin() - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4, "original unmoved");
    }

    #[test]
    fn rotate_reference_three_picks() {
        let (mut m, id) = model_with_box(1.0, 0.0);
        let mut op = Modify::new(ModifyOp::Rotate, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false); // pivot
        assert_eq!(op.type_value("r", &plane, &mut m), Some(Feed::NeedMore)); // reference mode
        // current direction = +X (src1→src2 along +x)
        assert_eq!(op.feed(Vec3::new(0.0, 0.0, 0.0), &plane, &mut m, false), Feed::NeedMore);
        assert_eq!(op.feed(Vec3::new(1.0, 0.0, 0.0), &plane, &mut m, false), Feed::NeedMore);
        // new direction = +Y → rotate +90°
        assert_eq!(op.feed(Vec3::new(0.0, 1.0, 0.0), &plane, &mut m, false), Feed::Applied);
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4, "ref rotate +90°, got {o:?}");
    }

    #[test]
    fn scale_reference_old_new_length() {
        let (mut m, id) = model_with_box(2.0, 0.0);
        let mut op = Modify::new(ModifyOp::Scale, vec![id]);
        let plane = Plane::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false); // pivot at origin
        assert_eq!(op.type_value("r", &plane, &mut m), Some(Feed::NeedMore));
        op.feed(Vec3::new(0.0, 0.0, 0.0), &plane, &mut m, false); // ref start
        op.feed(Vec3::new(2.0, 0.0, 0.0), &plane, &mut m, false); // ref end → old length 2
        // new length 4 → factor 2 → the box centre at (2,0) goes to (4,0)
        assert_eq!(op.feed(Vec3::new(4.0, 0.0, 0.0), &plane, &mut m, false), Feed::Applied);
        let o = m.get_mut(id).unwrap().world_origin();
        assert!((o - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-4, "scale ×2 about origin, got {o:?}");
    }

    #[test]
    fn pick_names_track_the_flow() {
        let mut op = Modify::new(ModifyOp::Rotate, vec![0]);
        assert_eq!(op.pick_name(), "PIVOT");
        let plane = Plane::default();
        let mut m = Model::default();
        op.feed(Vec3::ZERO, &plane, &mut m, false);
        assert_eq!(op.pick_name(), "ANGLE");
    }
}
