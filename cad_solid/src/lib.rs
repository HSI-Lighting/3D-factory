//! cad_solid — a free-3D **parametric solid modeler** for SIMLUX.
//!
//! UI-agnostic pure Rust: a construction-plane system, parametric [`Primitive`]s,
//! and a boolean **feature history** ([`Model`]) evaluated through **csgrs** (BSP
//! CSG) into a neutral triangle soup ([`SolidMesh`]). The standalone sandbox
//! (`cargo run -p cad_solid --example sandbox`) drives it; nothing here depends on
//! egui/eframe.
//!
//! Design: `Model{ features }` is an ordered CSG tree flattened into a history —
//! each [`Feature`] applies its [`Primitive`] (posed on a [`Plane`]) to the running
//! result via a [`BoolOp`]. Params are the identity; the mesh is DERIVED, so
//! editing any param and re-`eval()`ing reproduces the solid (the "generator
//! re-derives" contract). The eventual simLUX wire-in (S5) converts [`SolidMesh`]
//! → `cad_light::Mesh` — the single coupling point.

use cad_kernel::Vec2 as KVec2;
use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// The coplanarity tolerance every boolean in this app runs at.
///
/// WHY IT IS NOT THE DEFAULT. csgrs classifies a point against a plane with Shewchuk's exact
/// `orient3d` predicate and compares the result against `tolerance()` — but that predicate returns
/// six times a TETRAHEDRON VOLUME, i.e. `2·area(abc)·distance`. So the effective band *in distance*
/// is `tolerance / (2·area)`: the smaller the triangle, the wider the band. The default is `1e-6`,
/// and the crate's own unit table declares it dimensioned for MILLIMETRES. This app works in
/// metres, which puts ordinary work far deeper into that band than the default assumes.
///
/// MEASURED — a plate cut by a 64-sided hole, volume error against closed form:
///
/// ```text
///   plate size   default 1e-6         this constant
///     10.0 m     −0.00 %              0.00 %
///      1.0 m      0.00 %              0.00 %
///      0.3 m     +0.21 … +0.65 %      0.00 %
///      0.1 m     −33 … +732 %         0.00 %   ← BSP collapsed to depth 1
///     0.03 m     +682 %               0.00 %
///     0.01 m    +1048 %               0.00 %
/// ```
///
/// No error, no panic, no open-edge signal — it returns the WRONG SOLID, silently. And it is
/// reachable from ordinary building input: a 5 m-radius circle extruded 3 m and cut by one
/// 1 × 1 m window removes 0.99 m³ at 1,024 segments, 0.66 at 2,048 and 1.67 at 4,096.
pub const BOOLEAN_TOLERANCE: f64 = 1e-6;

/// Set [`BOOLEAN_TOLERANCE`] and PROVE it took. Call once at startup, before anything can run a
/// boolean — including the generators and any autoloaded fixture.
///
/// THE READ-BACK IS THE POINT, not belt and braces. csgrs keeps the tolerance in a `OnceLock` and
/// its setter is `let _ = TOLERANCE_CELL.set(value)` — it SWALLOWS its own failure. `tolerance()`
/// is `get_or_init`, so the first *reader* initialises the cell to the default, and any
/// `set_tolerance` after that is a silent no-op leaving the app on the broken value with nothing
/// said. That is precisely the failure this exists to remove, so it is observed, not assumed.
pub fn init_boolean_tolerance() -> Result<f64, String> {
    csgrs::float_types::set_tolerance(BOOLEAN_TOLERANCE);
    let got = csgrs::float_types::tolerance();
    if got == BOOLEAN_TOLERANCE {
        Ok(got)
    } else {
        Err(format!(
            "boolean tolerance is {got:e}, not {BOOLEAN_TOLERANCE:e} — something ran a boolean \
             before startup finished, so the setter was a no-op. Small cuts will be silently wrong."
        ))
    }
}

pub mod architecture; // staircase / spiral / ramp generators → SolidMesh
pub mod dogleg; // parametric half-turn (dog-leg) staircase → editable CSG parts
pub mod spiral; // parametric helical (spiral) staircase → editable CSG parts
pub mod door; // parametric panelled door (leaf + lining + casing + hardware) → SolidMesh
pub mod cupboard; // parametric cabinet configurator (grid of bays × tiers) → SolidMesh + per-part material
pub mod kitchen; // parametric kitchen cabinet run (base + optional wall lanes, worktop, plinth) → SolidMesh
pub mod cabin; // parametric handleless cabinet UNIT (close-range joinery: overlay fronts, grips, pin rows) → SolidMesh
pub mod sweeplight; // curved office luminaires: a lighting profile swept along a path (RMF sweep) → SolidMesh
pub mod desk; // parametric office workstation desk as a deletable feature tree → SolidMesh
pub mod couch; // parametric run-chain sofa (straight / L / U sectionals) → SolidMesh
mod csg;
pub mod meshcut; // subtract a drawn prism from a finished MESH (furniture / architecture / apertures)
pub mod dbg_recorder; // copied VERBATIM from cad_app (identical to RUST_CAD's recorder)
pub mod draw;
pub mod modify;

/// A flat triangle soup: `positions`/`normals` in lock-step, 3 consecutive
/// entries = one triangle (metres, Z-up, f32). cad_solid's neutral output; the
/// app converts it to `cad_light::Mesh` at wire-in.
#[derive(Clone, Debug, Default)]
pub struct SolidMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// The leading feature id of the body each TRIANGLE came from (`positions.len()/3`
    /// entries). Lets the app colour a body by which feature it belongs to. May be empty
    /// on meshes built without tagging (older paths); callers must tolerate that.
    pub face_ids: Vec<u32>,
}

impl SolidMesh {
    /// Number of triangles.
    pub fn tri_count(&self) -> usize {
        self.positions.len() / 3
    }

    /// Axis-aligned bounds `(min, max)`, or `None` when empty.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.positions.iter();
        let first = *it.next()?;
        let (mut mn, mut mx) = (first, first);
        for p in self.positions.iter() {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        Some((mn, mx))
    }
}

/// Which world plane the active construction plane is parallel to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaneKind {
    XY,
    XZ,
    YZ,
}

impl PlaneKind {
    pub const ALL: [PlaneKind; 3] = [PlaneKind::XY, PlaneKind::XZ, PlaneKind::YZ];
    pub fn label(self) -> &'static str {
        match self {
            PlaneKind::XY => "XY",
            PlaneKind::XZ => "XZ",
            PlaneKind::YZ => "YZ",
        }
    }
}

/// An arbitrarily-oriented plane: a world origin and its in-plane `(u, v)` axes. Stored as
/// `[f32; 3]` arrays (not `glam::Vec3`) so a `Model` serialises — glam is built here without
/// its `serde` feature. This is what lets a sketch on ANY picked face (a tilted or curved
/// wall) become an extrusion that rises along that face's real normal.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CustomBasis {
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
}

/// A construction / drawing plane: a world plane pushed `offset` along its normal, OR — when
/// `custom` is set — an arbitrarily-oriented plane taken straight from a picked face's frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Plane {
    pub kind: PlaneKind,
    pub offset: f32,
    /// When set, OVERRIDES `kind`/`offset` with an arbitrary basis. `#[serde(default)]` so
    /// every sidecar written before arbitrary planes existed still loads (as `None`).
    #[serde(default)]
    pub custom: Option<CustomBasis>,
}

impl Default for Plane {
    fn default() -> Self {
        Self { kind: PlaneKind::XY, offset: 0.0, custom: None }
    }
}

impl Plane {
    /// An arbitrary plane from a face frame: origin + orthonormal in-plane axes. The caller
    /// must pass a right-handed `(u, v)` (so `u × v` is the outward normal).
    pub fn from_basis(origin: Vec3, u: Vec3, v: Vec3) -> Self {
        Self {
            kind: PlaneKind::XY,
            offset: 0.0,
            custom: Some(CustomBasis {
                origin: origin.to_array(),
                u: u.normalize_or_zero().to_array(),
                v: v.normalize_or_zero().to_array(),
            }),
        }
    }

    /// The in-plane `(u, v)` axes. The normal is DERIVED as `u × v`, so the frame
    /// `[u | v | n]` is always right-handed (det +1) — csgrs booleans depend on
    /// consistent winding, and a mirrored (det −1) basis would flip every normal.
    pub fn axes(&self) -> (Vec3, Vec3) {
        if let Some(c) = self.custom {
            return (Vec3::from_array(c.u), Vec3::from_array(c.v));
        }
        match self.kind {
            PlaneKind::XY => (Vec3::X, Vec3::Y),
            PlaneKind::XZ => (Vec3::X, Vec3::Z),
            PlaneKind::YZ => (Vec3::Y, Vec3::Z),
        }
    }

    pub fn normal(&self) -> Vec3 {
        let (u, v) = self.axes();
        u.cross(v).normalize()
    }

    /// World position of the plane's local origin.
    pub fn origin(&self) -> Vec3 {
        if let Some(c) = self.custom {
            return Vec3::from_array(c.origin);
        }
        self.normal() * self.offset
    }

    /// Local → world transform for a primitive posed at `place`: rotate about the
    /// plane normal (spin), then map local `X→u, Y→v, Z→n` and translate to the
    /// placement point on the plane.
    pub fn world_matrix(&self, place: &Placement) -> Mat4 {
        let (u, v) = self.axes();
        let n = u.cross(v).normalize();
        let o = self.origin() + u * place.u + v * place.v + n * place.lift;
        let basis = Mat4::from_cols(u.extend(0.0), v.extend(0.0), n.extend(0.0), o.extend(1.0));
        // Local rotation: spin about Z(=n), pitch about X(=u), roll about Y(=v). Applied
        // before the basis maps local→world, so each angle rotates about that plane axis.
        let rot = Mat4::from_rotation_z(place.spin_deg.to_radians())
            * Mat4::from_rotation_x(place.pitch_deg.to_radians())
            * Mat4::from_rotation_y(place.roll_deg.to_radians());
        basis * rot
    }

    /// World point → this plane's local `(u, v)` (drops the normal component) —
    /// the 3D analog of a 2D pick on the drawing plane, used by the modifiers.
    pub fn to_uv(&self, w: Vec3) -> Vec2 {
        let (u, v) = self.axes();
        let d = w - self.origin();
        Vec2::new(d.dot(u), d.dot(v))
    }

    /// Local `(u, v)` → world point on the plane.
    pub fn from_uv(&self, uv: Vec2) -> Vec3 {
        let (u, v) = self.axes();
        self.origin() + u * uv.x + v * uv.y
    }
}

/// Pose of a primitive on its plane: in-plane `(u, v)`, `lift` along the normal,
/// and rotation about the three LOCAL axes — `spin` about the normal (local Z),
/// `pitch` about local X (= plane u), `roll` about local Y (= plane v). The two
/// tilt angles are `#[serde(default)]` so sidecars written before 3-axis rotation
/// still load (they read back as 0 = upright, the old behaviour).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default)]
pub struct Placement {
    pub u: f32,
    pub v: f32,
    pub lift: f32,
    pub spin_deg: f32,
    #[serde(default)]
    pub pitch_deg: f32,
    #[serde(default)]
    pub roll_deg: f32,
}

/// Multiply every LENGTH in a primitive by `k`, leaving segment/stack counts (which are
/// topology, not size) and the shared-table ids alone. Split out of [`Model::rescale`] so the
/// exhaustive match is in one place and a newly added primitive fails to compile until its
/// dimensions are handled here.
fn scale_primitive(p: &mut Primitive, k: f32) {
    match p {
        Primitive::Box { w, d, h } => { *w *= k; *d *= k; *h *= k; }
        Primitive::Cylinder { r, h, .. } => { *r *= k; *h *= k; }
        Primitive::Sphere { r, .. } => *r *= k,
        Primitive::Frustum { r_bottom, r_top, h, .. } => { *r_bottom *= k; *r_top *= k; *h *= k; }
        Primitive::Torus { major_r, minor_r, .. } => { *major_r *= k; *minor_r *= k; }
        Primitive::Capsule { r, h, .. } => { *r *= k; *h *= k; }
        Primitive::Tube { r_outer, r_inner, h, .. } => { *r_outer *= k; *r_inner *= k; *h *= k; }
        Primitive::Ellipsoid { rx, ry, rz, .. } => { *rx *= k; *ry *= k; *rz *= k; }
        // `w`/`d` are a CACHE of the profile's extents (see the variant's docs). The profile
        // itself is scaled once in `Model::rescale`; these must track it or `local_aabb` —
        // which ray-pick and the selection highlight both use — reports the old size.
        Primitive::Extrusion { h, w, d, .. } => { *h *= k; *w *= k; *d *= k; }
        // Same: cached local AABB of a swept solid, scaled to match its profile and path.
        Primitive::Sweep { bmin, bmax, .. } => {
            for v in bmin.iter_mut().chain(bmax.iter_mut()) {
                *v *= k;
            }
        }
    }
}

/// Twice the signed area of a closed ring (shoelace). Sign gives the winding.
fn signed_area2(p: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let q = p[(i + 1) % p.len()];
        a += p[i].x * q.y - q.x * p[i].y;
    }
    a
}

/// Do two proper (non-adjacent) edges of the closed ring cross?
///
/// O(n²), which is right for building outlines (tens of points) and keeps the test exact
/// rather than approximate. Adjacent edges are skipped — they legitimately share a vertex.
fn self_intersects(p: &[Vec2]) -> bool {
    let n = p.len();
    let orient = |a: Vec2, b: Vec2, c: Vec2| {
        let v = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if v > 1e-9 { 1 } else if v < -1e-9 { -1 } else { 0 }
    };
    for i in 0..n {
        let (a1, a2) = (p[i], p[(i + 1) % n]);
        for j in (i + 1)..n {
            // skip edges sharing a vertex (including the wrap pair n-1 ↔ 0)
            if j == i || (j + 1) % n == i || (i + 1) % n == j {
                continue;
            }
            let (b1, b2) = (p[j], p[(j + 1) % n]);
            let (d1, d2) = (orient(a1, a2, b1), orient(a1, a2, b2));
            let (d3, d4) = (orient(b1, b2, a1), orient(b1, b2, a2));
            if d1 != d2 && d3 != d4 && d1 != 0 && d2 != 0 && d3 != 0 && d4 != 0 {
                return true;
            }
        }
    }
    false
}

/// Stable key into [`Model::profiles`]. Like a feature id, it is a KEY and not an index:
/// removing a profile never renumbers the others.
pub type ProfileId = u32;

/// A closed 2D outline, stored once and referenced by [`Primitive::Extrusion`].
///
/// Points are `[f32; 2]`, not `glam::Vec2`, because glam is built here without its
/// `serde` feature and a `Model` must serialize. They are CENTRED on their own bounding
/// box, matching this module's canonical-local-coords convention (every primitive is
/// centred on the local origin); the world position comes from the `Placement`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub pts: Vec<[f32; 2]>,
}

/// Stable key into [`Model::paths`] — same "key not index" rule as [`ProfileId`].
pub type PathId = u32;

/// An OPEN 3D polyline referenced by [`Primitive::Sweep`] — the route a cross-section is
/// swept along. Unlike a [`Profile`] it is NOT closed and NOT recentred: it carries the
/// tube's position in the primitive's local frame (the sweep places the centred profile at
/// each of these points). Points are `[f32; 3]` for the same serde reason as `Profile`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Path {
    pub id: PathId,
    pub pts: Vec<[f32; 3]>,
}

/// Why a profile was refused. Reported, never swallowed: earcut's behaviour on a
/// self-intersecting loop is undefined, and a non-watertight mesh would silently poison
/// the light calc downstream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProfileError {
    /// Fewer than 3 distinct points — no area to extrude.
    TooFewPoints,
    /// Zero (or near-zero) area: all points collinear.
    Degenerate,
    /// Two non-adjacent edges cross.
    SelfIntersecting,
}

/// A parametric leaf shape, built in canonical local coords (footprint centred on
/// the local origin, extruding +Z / sitting on the plane). The S2 primitives
/// (Prism / Step / Ramp / Wall / Column) slot in here.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Primitive {
    /// Rectangular box `w×d` footprint, `h` tall.
    Box { w: f32, d: f32, h: f32 },
    /// Cylinder radius `r`, `h` tall, `sides` facets.
    Cylinder { r: f32, h: f32, sides: u32 },
    /// Sphere. `segments` = longitude slices, `stacks` = latitude rings.
    /// Rests ON the plane (lifted by `r`), per this module's convention.
    Sphere { r: f32, segments: u32, stacks: u32 },
    /// Cone / frustum / prism / pyramid — one primitive, four shapes:
    /// `r_top = 0` → **cone** · `r_top = r_bottom` → **prism** ·
    /// `sides = 4, r_top = 0` → **pyramid** · else → **frustum**.
    /// (This is why there is no separate Cone/Prism/Pyramid variant.)
    Frustum { r_bottom: f32, r_top: f32, h: f32, sides: u32 },
    /// Torus — `major_r` = ring radius, `minor_r` = tube thickness.
    Torus { major_r: f32, minor_r: f32, seg_major: u32, seg_minor: u32 },
    /// Capsule — a cylinder of length `h` with hemispherical caps of radius `r`.
    /// **COMPOSED** (csgrs has no capsule): cylinder ∪ sphere ∪ sphere.
    /// Total height = `h + 2r`.
    Capsule { r: f32, h: f32, segments: u32, stacks: u32 },
    /// Hollow tube — **COMPOSED** (csgrs has no tube): outer cylinder ∖ inner.
    Tube { r_outer: f32, r_inner: f32, h: f32, sides: u32 },
    /// Ellipsoid with independent radii. Rests on the plane (lifted by `rz`).
    Ellipsoid { rx: f32, ry: f32, rz: f32, segments: u32, stacks: u32 },
    /// **Vertical extrusion of an arbitrary closed profile** — the building outline.
    ///
    /// The profile lives in [`Model::profiles`] and is referenced by [`ProfileId`] rather
    /// than held inline. That indirection IS the design: a `Vec<Vec2>` in this enum would
    /// drop `Copy` from `Primitive` **and** from [`Feature`], which is returned by value
    /// from `translated` / `rotated` / `scaled` / `mirrored` and passed by value across
    /// the app. An id leaves every one of those sites untouched, and matches this crate's
    /// existing "ids are stable keys, not indices" rule.
    ///
    /// `w`/`d` cache the profile's extents so [`Primitive::local_aabb`] — which drives
    /// ray-pick selection and has no access to the `Model` — stays a pure function of the
    /// primitive. Profiles are IMMUTABLE once created (an edit mints a new one), so the
    /// cache cannot go stale.
    Extrusion { profile: ProfileId, h: f32, w: f32, d: f32 },
    /// **Sweep** — a closed cross-section (`profile`) swept along an open path (`path`),
    /// the section kept perpendicular to the path (csgrs `Sketch::sweep`). Both live in the
    /// shared tables and are referenced by id (keeps `Primitive` `Copy`, same as
    /// [`Primitive::Extrusion`]). `bmin`/`bmax` cache the local AABB so [`Primitive::local_aabb`]
    /// stays a pure function without the `Model` (a swept solid's bounds aren't a simple box).
    Sweep { profile: ProfileId, path: PathId, bmin: [f32; 3], bmax: [f32; 3] },
}

impl Primitive {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "Box",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Sphere { .. } => "Sphere",
            Primitive::Frustum { r_top, sides, r_bottom, .. } => {
                // one variant, four shapes — name it by what it actually is
                if *r_top <= 1e-6 {
                    if *sides == 4 { "Pyramid" } else { "Cone" }
                } else if (*r_top - *r_bottom).abs() <= 1e-6 {
                    "Prism"
                } else {
                    "Frustum"
                }
            }
            Primitive::Extrusion { .. } => "Extrusion",
            Primitive::Sweep { .. } => "Sweep",
            Primitive::Torus { .. } => "Torus",
            Primitive::Capsule { .. } => "Capsule",
            Primitive::Tube { .. } => "Tube",
            Primitive::Ellipsoid { .. } => "Ellipsoid",
        }
    }

    /// Local-space axis-aligned bounds `(min, max)` before placement (footprint
    /// centred on the local origin, resting on z = 0).
    pub fn local_aabb(&self) -> (Vec3, Vec3) {
        match *self {
            Primitive::Box { w, d, h } => (Vec3::new(-w / 2.0, -d / 2.0, 0.0), Vec3::new(w / 2.0, d / 2.0, h)),
            Primitive::Cylinder { r, h, .. } => (Vec3::new(-r, -r, 0.0), Vec3::new(r, r, h)),
            // The cached extents exist precisely so this stays a pure function of the
            // primitive — resolving the ProfileId here would need the Model.
            Primitive::Extrusion { h, w, d, .. } => {
                (Vec3::new(-w / 2.0, -d / 2.0, 0.0), Vec3::new(w / 2.0, d / 2.0, h))
            }
            // Cached at creation — the swept solid's true bounds (path bbox ± section reach).
            Primitive::Sweep { bmin, bmax, .. } => (Vec3::from(bmin), Vec3::from(bmax)),
            // sphere/ellipsoid are lifted so they REST on the plane
            Primitive::Sphere { r, .. } => (Vec3::new(-r, -r, 0.0), Vec3::new(r, r, 2.0 * r)),
            Primitive::Ellipsoid { rx, ry, rz, .. } => {
                (Vec3::new(-rx, -ry, 0.0), Vec3::new(rx, ry, 2.0 * rz))
            }
            Primitive::Frustum { r_bottom, r_top, h, .. } => {
                let r = r_bottom.max(r_top);
                (Vec3::new(-r, -r, 0.0), Vec3::new(r, r, h))
            }
            // torus lies flat, lifted by minor_r → rests on the plane
            Primitive::Torus { major_r, minor_r, .. } => {
                let o = major_r + minor_r;
                (Vec3::new(-o, -o, 0.0), Vec3::new(o, o, 2.0 * minor_r))
            }
            // capsule: caps of r at each end of an h-long barrel → total h + 2r
            Primitive::Capsule { r, h, .. } => (Vec3::new(-r, -r, 0.0), Vec3::new(r, r, h + 2.0 * r)),
            Primitive::Tube { r_outer, h, .. } => {
                (Vec3::new(-r_outer, -r_outer, 0.0), Vec3::new(r_outer, r_outer, h))
            }
        }
    }
}

/// A boolean combination operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

impl BoolOp {
    pub const ALL: [BoolOp; 3] = [BoolOp::Union, BoolOp::Difference, BoolOp::Intersection];
    pub fn label(self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Difference => "Difference",
            BoolOp::Intersection => "Intersection",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            BoolOp::Union => "∪",
            BoolOp::Difference => "−",
            BoolOp::Intersection => "∩",
        }
    }
}

/// One step of the CSG history: apply `primitive` (posed on `plane` at `placement`)
/// to the running result with `op`. The FIRST feature is the base (its `op` is
/// ignored — nothing to combine with yet).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Feature {
    pub id: u32,
    pub op: BoolOp,
    pub plane: Plane,
    pub placement: Placement,
    pub primitive: Primitive,
}

impl Feature {
    /// World-space AABB of this feature's primitive, posed on its plane — used for
    /// ray-pick selection and the selection highlight.
    pub fn world_aabb(&self) -> (Vec3, Vec3) {
        let (lmn, lmx) = self.primitive.local_aabb();
        let m = self.plane.world_matrix(&self.placement);
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { lmn.x } else { lmx.x },
                if i & 2 == 0 { lmn.y } else { lmx.y },
                if i & 4 == 0 { lmn.z } else { lmx.z },
            );
            let w = m.transform_point3(c);
            mn = mn.min(w);
            mx = mx.max(w);
        }
        (mn, mx)
    }

    // ── Pure transforms — the 3D analogs of cad_kernel `Geom::translated`/
    //    `rotated`/`scaled`/`mirrored`. Each returns a NEW feature (params are the
    //    identity; the mesh re-derives). The modifier layer routes through these,
    //    exactly as the 2D `apply_*` route through the `Geom` methods.

    /// World position of the feature's local origin (footprint centre on its plane).
    pub fn world_origin(&self) -> Vec3 {
        let (u, v) = self.plane.axes();
        self.plane.origin() + u * self.placement.u + v * self.placement.v + self.plane.normal() * self.placement.lift
    }

    /// Copy of this feature relocated so its local origin sits at world point `w`
    /// (keeps its plane, spin, and primitive — only the placement is recomputed).
    pub fn with_world_origin(&self, w: Vec3) -> Feature {
        let (u, v) = self.plane.axes();
        let d = w - self.plane.origin();
        let mut f = *self;
        f.placement.u = d.dot(u);
        f.placement.v = d.dot(v);
        f.placement.lift = d.dot(self.plane.normal());
        f
    }

    /// Translate by a world vector (MOVE / COPY).
    pub fn translated(&self, world_delta: Vec3) -> Feature {
        self.with_world_origin(self.world_origin() + world_delta)
    }

    /// Rotate `angle_rad` about `pivot` around `axis` (ROTATE). When the feature's
    /// plane normal is parallel to the axis (the coplanar, in-plane case), the spin
    /// is updated too so the primitive turns; otherwise only the origin orbits.
    pub fn rotated(&self, pivot: Vec3, axis: Vec3, angle_rad: f32) -> Feature {
        let axis = axis.normalize_or_zero();
        let o = self.world_origin();
        let q = Quat::from_axis_angle(axis, angle_rad);
        let mut f = self.with_world_origin(pivot + q * (o - pivot));
        let align = self.plane.normal().dot(axis);
        if align.abs() > 0.999 {
            f.placement.spin_deg += angle_rad.to_degrees() * align.signum();
        }
        f
    }

    /// Uniform scale by `k` about `pivot` (SCALE) — moves the origin away from the
    /// pivot and scales the primitive's dimensions.
    pub fn scaled(&self, pivot: Vec3, k: f32) -> Feature {
        let o = self.world_origin();
        let mut f = self.with_world_origin(pivot + (o - pivot) * k);
        match &mut f.primitive {
            Primitive::Box { w, d, h } => {
                *w = (*w * k).max(0.001);
                *d = (*d * k).max(0.001);
                *h = (*h * k).max(0.001);
            }
            Primitive::Cylinder { r, h, .. } => {
                *r = (*r * k).max(0.001);
                *h = (*h * k).max(0.001);
            }
            // Uniform scale: every LENGTH scales, every SEGMENT COUNT does not.
            Primitive::Sphere { r, .. } => *r = (*r * k).max(0.001),
            Primitive::Frustum { r_bottom, r_top, h, .. } => {
                *r_bottom = (*r_bottom * k).max(0.0); // 0 is legal — that IS a cone
                *r_top = (*r_top * k).max(0.0);
                *h = (*h * k).max(0.001);
            }
            Primitive::Torus { major_r, minor_r, .. } => {
                *major_r = (*major_r * k).max(0.001);
                *minor_r = (*minor_r * k).max(0.001);
            }
            // An extrusion's SHAPE lives in the shared profile table, which this method
            // cannot reach (it has no `&mut Model`), and profiles are immutable because
            // they are shared between storeys — scaling one in place would silently
            // resize every other extrusion using it. So the feature MOVES relative to the
            // pivot but keeps its size. Scaling the height alone would distort it, which
            // is worse than not scaling.
            //
            // Doing this properly means minting a scaled profile, which needs a
            // Model-level operation rather than a Feature-level one.
            Primitive::Extrusion { .. } => {}
            // Same as Extrusion: shape lives in the shared profile/path tables, so it moves
            // relative to the pivot but keeps its size.
            Primitive::Sweep { .. } => {}
            Primitive::Capsule { r, h, .. } => {
                *r = (*r * k).max(0.001);
                *h = (*h * k).max(0.0); // 0 is legal — that IS a sphere
            }
            Primitive::Tube { r_outer, r_inner, h, .. } => {
                *r_outer = (*r_outer * k).max(0.001);
                *r_inner = (*r_inner * k).max(0.0);
                *h = (*h * k).max(0.001);
            }
            Primitive::Ellipsoid { rx, ry, rz, .. } => {
                *rx = (*rx * k).max(0.001);
                *ry = (*ry * k).max(0.001);
                *rz = (*rz * k).max(0.001);
            }
        }
        f
    }

    /// Reflect across the plane through `plane_pt` with unit-ish normal `plane_n`
    /// (MIRROR). The origin reflects; the primitive (axis-symmetric) is unchanged.
    pub fn mirrored(&self, plane_pt: Vec3, plane_n: Vec3) -> Feature {
        let n = plane_n.normalize_or_zero();
        let o = self.world_origin();
        let refl = o - n * (2.0 * (o - plane_pt).dot(n));
        self.with_world_origin(refl)
    }
}

/// Ray vs axis-aligned box (slab method). Returns the entry distance (clamped to
/// ≥ 0) when the ray `orig + t·dir` hits the box, else `None`. `dir` need not be
/// unit. Zero components are handled via `f32` infinities.
pub fn ray_aabb(orig: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = dir.recip();
    let t0 = (min - orig) * inv;
    let t1 = (max - orig) * inv;
    let enter = t0.min(t1).max_element();
    let exit = t0.max(t1).min_element();
    (exit >= enter.max(0.0)).then(|| enter.max(0.0))
}

/// Ray vs triangle (Möller–Trumbore). Returns the forward hit distance, or `None`.
/// Used to pick a solid's **surface** (and its face normal) for sketch-on-face.
pub fn ray_triangle(orig: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-7 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = orig - a;
    let u = tv.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = tv.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = e2.dot(q) * inv;
    (dist > 1e-6).then_some(dist)
}

/// The **face** containing triangle `start`: all triangles reachable from it
/// through shared edges whose normal matches (a maximal coplanar-connected
/// region of the surface mesh). `positions` is the flat triangle soup (3 per
/// tri). Returns the triangle indices of the face. Used for face selection.
pub fn coplanar_face(positions: &[[f32; 3]], start: usize) -> Vec<usize> {
    use std::collections::HashMap;
    let ntri = positions.len() / 3;
    if start >= ntri {
        return Vec::new();
    }
    // Weld vertices onto a grid so shared edges match despite float noise.
    let key = |p: [f32; 3]| -> (i64, i64, i64) {
        let q = 1.0e4;
        ((p[0] as f64 * q).round() as i64, (p[1] as f64 * q).round() as i64, (p[2] as f64 * q).round() as i64)
    };
    let edge = |t: usize, e: usize| {
        let (mut a, mut b) = (key(positions[3 * t + e]), key(positions[3 * t + (e + 1) % 3]));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        (a, b)
    };
    let normal = |t: usize| -> Vec3 {
        let a = Vec3::from(positions[3 * t]);
        let b = Vec3::from(positions[3 * t + 1]);
        let c = Vec3::from(positions[3 * t + 2]);
        (b - a).cross(c - a).normalize_or_zero()
    };
    // Edge → adjacent triangles.
    let mut edges: HashMap<((i64, i64, i64), (i64, i64, i64)), Vec<usize>> = HashMap::new();
    for t in 0..ntri {
        for e in 0..3 {
            edges.entry(edge(t, e)).or_default().push(t);
        }
    }
    let n0 = normal(start);
    let mut visited = vec![false; ntri];
    let mut face = Vec::new();
    let mut stack = vec![start];
    visited[start] = true;
    while let Some(t) = stack.pop() {
        face.push(t);
        for e in 0..3 {
            if let Some(adj) = edges.get(&edge(t, e)) {
                for &nt in adj {
                    if !visited[nt] && normal(nt).dot(n0) > 0.999 {
                        visited[nt] = true;
                        stack.push(nt);
                    }
                }
            }
        }
    }
    face
}

/// Group a triangle soup into `(face_groups, body_groups)`, one id per triangle:
/// * `face_groups[t]` — a maximal **coplanar-connected** region (triangles reachable through shared
///   edges whose normal matches the seed's), i.e. one flat surface. Used for "paint this face".
/// * `body_groups[t]` — a **connected component** (triangles reachable through shared edges, any
///   normal), i.e. one welded sub-part. Used for "paint this whole piece".
///
/// Ids are dense (0..n). `positions` is the flat soup (3 verts per tri). Vertices are welded onto a
/// grid so coincident-but-noisy edges still match — the same welding [`coplanar_face`] uses.
pub fn surface_groups(positions: &[[f32; 3]]) -> (Vec<u32>, Vec<u32>) {
    use std::collections::HashMap;
    let ntri = positions.len() / 3;
    let key = |p: [f32; 3]| -> (i64, i64, i64) {
        let q = 1.0e4;
        ((p[0] as f64 * q).round() as i64, (p[1] as f64 * q).round() as i64, (p[2] as f64 * q).round() as i64)
    };
    let edge = |t: usize, e: usize| {
        let (mut a, mut b) = (key(positions[3 * t + e]), key(positions[3 * t + (e + 1) % 3]));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        (a, b)
    };
    let normal = |t: usize| -> Vec3 {
        let a = Vec3::from(positions[3 * t]);
        let b = Vec3::from(positions[3 * t + 1]);
        let c = Vec3::from(positions[3 * t + 2]);
        (b - a).cross(c - a).normalize_or_zero()
    };
    let mut edges: HashMap<((i64, i64, i64), (i64, i64, i64)), Vec<usize>> = HashMap::new();
    for t in 0..ntri {
        for e in 0..3 {
            edges.entry(edge(t, e)).or_default().push(t);
        }
    }
    // Connected components (bodies): any shared edge bridges.
    let mut body = vec![u32::MAX; ntri];
    let mut bid = 0u32;
    for s in 0..ntri {
        if body[s] != u32::MAX {
            continue;
        }
        let mut stack = vec![s];
        body[s] = bid;
        while let Some(t) = stack.pop() {
            for e in 0..3 {
                if let Some(adj) = edges.get(&edge(t, e)) {
                    for &nt in adj {
                        if body[nt] == u32::MAX {
                            body[nt] = bid;
                            stack.push(nt);
                        }
                    }
                }
            }
        }
        bid += 1;
    }
    // Coplanar-connected regions (faces): a shared edge bridges only if the normal matches the seed.
    let mut face = vec![u32::MAX; ntri];
    let mut fid = 0u32;
    for s in 0..ntri {
        if face[s] != u32::MAX {
            continue;
        }
        let n0 = normal(s);
        let mut stack = vec![s];
        face[s] = fid;
        while let Some(t) = stack.pop() {
            for e in 0..3 {
                if let Some(adj) = edges.get(&edge(t, e)) {
                    for &nt in adj {
                        if face[nt] == u32::MAX && normal(nt).dot(n0) > 0.999 {
                            face[nt] = fid;
                            stack.push(nt);
                        }
                    }
                }
            }
        }
        fid += 1;
    }
    (face, body)
}

/// A free (arbitrary) construction plane: an orthonormal `(u, v)` frame at an
/// `origin` in world space. Unlike [`Plane`] (locked to XY/XZ/YZ), a `Frame` sits
/// on any picked surface — the basis for **sketch-on-face**.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: Vec3,
    pub u: Vec3,
    pub v: Vec3,
}

impl Frame {
    /// Build a right-handed frame (`u × v = normal`) at `origin` facing `normal`.
    /// The in-plane axes are chosen deterministically from a world reference.
    pub fn from_point_normal(origin: Vec3, normal: Vec3) -> Self {
        let n = normal.normalize_or_zero();
        let reference = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let u = reference.cross(n).normalize_or_zero();
        let v = n.cross(u).normalize_or_zero();
        Self { origin, u, v }
    }
    pub fn normal(&self) -> Vec3 {
        self.u.cross(self.v).normalize_or_zero()
    }
    pub fn to_uv(&self, w: Vec3) -> Vec2 {
        let d = w - self.origin;
        Vec2::new(d.dot(self.u), d.dot(self.v))
    }
    pub fn from_uv(&self, uv: Vec2) -> Vec3 {
        self.origin + self.u * uv.x + self.v * uv.y
    }

    /// The canonical frame for this frame's PLANE — same plane, but with an origin and axes that
    /// do not depend on where the user happened to click.
    ///
    /// Reported as: "i drew the building on the origin yet then i took a face to draw on, and even
    /// though the face's corner is on the origin the face is shown as if it's in some other place."
    /// The cause: [`Self::from_point_normal`] takes the HIT POINT as the origin, so a sketch's
    /// (0, 0) landed wherever the cursor happened to be when the face was picked, and the face's
    /// own corner appeared at minus that offset. Two clicks a metre apart on the same face gave
    /// two different coordinate systems for the same plane.
    ///
    /// ORIGIN — the point of the plane closest to the world origin, so a sketch's coordinates
    /// agree with the model's. A floor at z = 0 puts (0, 0) on the world origin; a wall on x = 0
    /// puts it on that wall's own corner. A dimension read in a sketch then means the same thing
    /// as a dimension read in the plan, which is the whole reason to prefer a rule to a click.
    ///
    /// AXES — for a HORIZONTAL plane, u = +X and v = ±Y, so a sketch on a floor reads exactly like
    /// the plan. `from_point_normal`'s deterministic-but-arbitrary choice gives u = −Y and v = +X
    /// there: a quarter turn and a mirror away from the drawing it is standing on. For a VERTICAL
    /// plane u runs horizontally and v is up, which is what an elevation means.
    pub fn canonical(&self) -> Self {
        let n = self.normal();
        if n.length_squared() < 0.5 {
            return *self; // degenerate — nothing to canonicalise against
        }
        // The projection of the world origin onto this plane.
        let origin = n * self.origin.dot(n);
        let (u, v) = if n.z.abs() > 0.999 {
            // Horizontal: read it like a plan. v flips with the facing so u × v = n stays true.
            (Vec3::X, Vec3::Y * n.z.signum())
        } else {
            // Vertical or tilted: across, then up.
            let u = Vec3::Z.cross(n).normalize_or_zero();
            (u, n.cross(u).normalize_or_zero())
        };
        Self { origin, u, v }
    }
}

/// A 2D sketch on a [`Frame`] (typically a picked solid face). It holds a real
/// `cad_kernel::Document` — the SAME 2D document the main app edits — so the app's
/// draw / modifier / osnap code operates on it unchanged (flat sketch mode).
#[derive(Clone)]
pub struct Sketch {
    pub frame: Frame,
    pub doc: cad_kernel::Document,
    /// Reference geometry (the selected face's outline, in u,v) — shown faintly so
    /// the user sees WHERE they're drawing, and offered as osnap targets. Not part
    /// of the drawing itself.
    pub reference: Vec<cad_kernel::Geom>,
    /// What this plane is CALLED in the 2D view list.
    ///
    /// The 2D canvas used to offer fixed Top / Front / Back / Left / Right views of the whole
    /// model. It now offers the faces that have actually been drawn on, which is what a person is
    /// looking for when they go back to a drawing — so each one needs a name. Auto-given from the
    /// object the face belongs to, and renameable to whatever the drawing is of.
    ///
    /// EMPTY means "not named yet"; the caller supplies one. That is also what a model loaded from
    /// a file written before this field existed carries.
    pub name: String,
}

impl Sketch {
    pub fn new(frame: Frame) -> Self {
        Self {
            frame,
            doc: cad_kernel::Document::default(),
            reference: Vec::new(),
            name: String::new(),
        }
    }
}

/// Flatten a kernel geom into uv-space polyline paths (each inner `Vec` is one
/// path; closed shapes include the wrap point). Reuses the kernel's own geometry
/// (arc/ellipse samplers + DXF bulge). Point returns a small cross (two paths).
///
/// Output is in DRAWING UNITS — the caller decides what those mean. Use
/// [`geom_outlines_scaled`] to flatten straight into metres for the 3D side.
pub fn geom_outlines(g: &cad_kernel::Geom) -> Vec<Vec<Vec2>> {
    geom_outlines_scaled(g, 1.0)
}

/// Flatten as [`geom_outlines`], but multiply every coordinate by `k` (metres per drawing
/// unit) on the way out.
///
/// The multiply happens in **f64, before the f32 cast**, and that ordering is the point of
/// this function rather than scaling the returned `Vec2`s. An architectural plan sits at
/// coordinates like X = 3_619_000 mm; in f32 the spacing there is ~0.25, so a plan flattened
/// first and scaled afterwards would already have lost a quarter-millimetre of precision per
/// point. Scaling first yields 3619.0 m held to ~0.0002 m.
pub fn geom_outlines_scaled(g: &cad_kernel::Geom, k: f64) -> Vec<Vec<Vec2>> {
    use cad_kernel::Geom as G;
    match g {
        G::Line(l) => vec![vec![gvec(l.a, k), gvec(l.b, k)]],
        G::Circle(c) => vec![circle_path(c.center, c.radius, 64, k)],
        G::Arc(a) => vec![arc_path(a.center, a.radius, a.start_angle, a.sweep_angle, 48, k)],
        G::Ellipse(e) => {
            let n = 72;
            vec![(0..=n).map(|i| gvec(e.point_at(std::f64::consts::TAU * i as f64 / n as f64), k)).collect()]
        }
        G::EllipseArc(ea) => {
            let n = 48;
            vec![(0..=n)
                .map(|i| gvec(ea.ellipse.point_at(ea.start_param + ea.sweep_param * i as f64 / n as f64), k))
                .collect()]
        }
        G::Polyline(p) => vec![polyline_path(p, k)],
        G::Point(pt) => {
            let c = gvec(pt.location, k);
            // The cross is a MARKER, so its size belongs to the output space, not the drawing's
            // — it is not multiplied by `k`. Unchanged at k = 1; a sane 15 cm at k = 0.001.
            let s = 0.15;
            vec![
                vec![c - Vec2::new(s, 0.0), c + Vec2::new(s, 0.0)],
                vec![c - Vec2::new(0.0, s), c + Vec2::new(0.0, s)],
            ]
        }
        _ => Vec::new(),
    }
}

#[inline]
fn gvec(p: KVec2, k: f64) -> Vec2 {
    Vec2::new((p.x * k) as f32, (p.y * k) as f32)
}

fn circle_path(c: KVec2, r: f64, n: usize, k: f64) -> Vec<Vec2> {
    (0..=n)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            gvec(KVec2::new(c.x + r * t.cos(), c.y + r * t.sin()), k)
        })
        .collect()
}

fn arc_path(c: KVec2, r: f64, start: f64, sweep: f64, n: usize, k: f64) -> Vec<Vec2> {
    (0..=n)
        .map(|i| {
            let t = start + sweep * i as f64 / n as f64;
            gvec(KVec2::new(c.x + r * t.cos(), c.y + r * t.sin()), k)
        })
        .collect()
}

/// Flatten a (possibly bulged / closed) polyline into a single uv path.
fn polyline_path(p: &cad_kernel::Polyline, k: f64) -> Vec<Vec2> {
    let n = p.vertices.len();
    if n == 0 {
        return Vec::new();
    }
    let mut path = Vec::new();
    let segs = if p.closed { n } else { n - 1 };
    for i in 0..segs {
        let a = p.vertices[i];
        let b = p.vertices[(i + 1) % n];
        if a.bulge.abs() < 1e-9 {
            path.push(gvec(a.pos, k));
        } else if let Some((c, r, sa, sw)) = cad_kernel::bulge_arc(a.pos, b.pos, a.bulge) {
            let steps = 16;
            for j in 0..steps {
                let t = sa + sw * j as f64 / steps as f64;
                path.push(gvec(KVec2::new(c.x + r * t.cos(), c.y + r * t.sin()), k));
            }
        } else {
            path.push(gvec(a.pos, k));
        }
    }
    path.push(gvec(p.vertices[if p.closed { 0 } else { n - 1 }].pos, k));
    path
}

/// The parametric solid: an ordered boolean feature history, plus any 2D sketches
/// drawn on faces (not yet persisted — sketch serialization arrives with S5).
/// (No `Debug` derive — `cad_kernel::Document` on `Sketch` isn't `Debug`.)
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Model {
    pub features: Vec<Feature>,
    /// Closed outlines referenced by [`Primitive::Extrusion`]. Held once and shared: every
    /// storey of the same building outline points at ONE profile, so re-extruding an
    /// outline never duplicates its geometry.
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// Open polylines referenced by [`Primitive::Sweep`] — the sweep paths. Same shared-table
    /// pattern as `profiles`.
    #[serde(default)]
    pub paths: Vec<Path>,
    #[serde(skip)]
    pub sketches: Vec<Sketch>,
}

impl Model {
    /// Append a feature, assigning it a fresh id; returns that id.
    pub fn push(&mut self, op: BoolOp, plane: Plane, placement: Placement, primitive: Primitive) -> u32 {
        let id = self.features.iter().map(|f| f.id).max().map_or(1, |m| m + 1);
        self.features.push(Feature { id, op, plane, placement, primitive });
        id
    }

    /// Store a closed outline and return `(id, centre, w, d)`.
    ///
    /// The caller puts `centre` into the `Placement` (u,v) and `w`/`d` into the
    /// `Extrusion` variant — the stored points are recentred on their own bounding box so
    /// they obey this module's canonical-local-coords convention.
    ///
    /// The outline is CLOSED implicitly: a repeated final point is dropped, so the stored
    /// ring never duplicates its first vertex. Winding is normalised to counter-clockwise
    /// so extruded faces point consistently outward.
    ///
    /// Validation is refusal, not repair — see [`ProfileError`].
    pub fn add_profile(&mut self, pts: &[Vec2]) -> Result<(ProfileId, Vec2, f32, f32), ProfileError> {
        // Drop a repeated wrap point, then collapse consecutive duplicates.
        let mut p: Vec<Vec2> = Vec::with_capacity(pts.len());
        for &q in pts {
            if p.last().is_none_or(|l: &Vec2| (*l - q).length() > 1e-6) {
                p.push(q);
            }
        }
        if p.len() > 1 && (p[0] - p[p.len() - 1]).length() <= 1e-6 {
            p.pop();
        }
        if p.len() < 3 {
            return Err(ProfileError::TooFewPoints);
        }
        // Self-intersection is checked BEFORE area: a symmetric bow-tie has exactly zero
        // shoelace area, so an area-first order would report the vaguer "degenerate" for
        // the very shape the user can actually see is crossed.
        if self_intersects(&p) {
            return Err(ProfileError::SelfIntersecting);
        }
        let area2 = signed_area2(&p);
        if area2.abs() < 1e-8 {
            return Err(ProfileError::Degenerate);
        }
        if area2 < 0.0 {
            p.reverse(); // normalise to CCW
        }
        let (mut mn, mut mx) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for q in &p {
            mn = mn.min(*q);
            mx = mx.max(*q);
        }
        let centre = (mn + mx) * 0.5;
        let (w, d) = (mx.x - mn.x, mx.y - mn.y);
        let id = self.profiles.iter().map(|x| x.id).max().map_or(1, |m| m + 1);
        self.profiles.push(Profile {
            id,
            pts: p.iter().map(|q| [q.x - centre.x, q.y - centre.y]).collect(),
        });
        Ok((id, centre, w, d))
    }

    /// Look up a profile by id. `None` for a stale id — never panics.
    pub fn profile(&self, id: ProfileId) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Store an OPEN sweep path and return `(id, bbox_min, bbox_max)`. Unlike a profile the
    /// path is NOT recentred (it carries the tube's position) and NOT closed. Consecutive
    /// duplicate points are dropped; `None` if fewer than 2 distinct points remain.
    pub fn add_path(&mut self, pts: &[Vec3]) -> Option<(PathId, Vec3, Vec3)> {
        let mut p: Vec<Vec3> = Vec::with_capacity(pts.len());
        for &q in pts {
            if p.last().is_none_or(|l: &Vec3| (*l - q).length() > 1e-6) {
                p.push(q);
            }
        }
        if p.len() < 2 {
            return None;
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for q in &p {
            mn = mn.min(*q);
            mx = mx.max(*q);
        }
        let id = self.paths.iter().map(|x| x.id).max().map_or(1, |m| m + 1);
        self.paths.push(Path { id, pts: p.iter().map(|q| [q.x, q.y, q.z]).collect() });
        Some((id, mn, mx))
    }

    /// Look up a sweep path by id. `None` for a stale id — never panics.
    pub fn path(&self, id: PathId) -> Option<&Path> {
        self.paths.iter().find(|p| p.id == id)
    }

    /// Scale the WHOLE model uniformly about the world origin.
    ///
    /// This is the operation [`Feature::scaled`] cannot be: that one deliberately refuses to
    /// touch `Extrusion` / `Sweep`, because their shapes live in the shared profile and path
    /// tables and resizing one entry would silently resize every other feature referencing it.
    /// Here that objection disappears — every consumer is being scaled by the same factor, so
    /// scaling each shared entry exactly once is not just safe, it is the only correct way to
    /// do it. (Scaling per-feature would multiply a shared profile once per reference.)
    ///
    /// Scales every LENGTH and leaves every direction, angle and segment count alone:
    /// placements, plane offsets and custom-basis origins, all primitive dimensions, and the
    /// shared tables. Basis vectors `u`/`v` are unit directions and must NOT be scaled, or the
    /// plane stops being orthonormal and every solid on it shears.
    ///
    /// Callers are responsible for anything living OUTSIDE the model that is keyed on world
    /// coordinates — notably per-face paint keys, which quantise a plane offset.
    pub fn rescale(&mut self, k: f32) {
        if !(k.is_finite() && k > 0.0) || (k - 1.0).abs() < f32::EPSILON {
            return;
        }
        // Shared tables FIRST and ONCE each — see the note above.
        for pr in &mut self.profiles {
            for p in &mut pr.pts {
                p[0] *= k;
                p[1] *= k;
            }
        }
        for pa in &mut self.paths {
            for p in &mut pa.pts {
                p[0] *= k;
                p[1] *= k;
                p[2] *= k;
            }
        }
        for f in &mut self.features {
            f.placement.u *= k;
            f.placement.v *= k;
            f.placement.lift *= k;
            // The plane's POSITION scales; its orientation does not.
            f.plane.offset *= k;
            if let Some(c) = f.plane.custom.as_mut() {
                c.origin = [c.origin[0] * k, c.origin[1] * k, c.origin[2] * k];
            }
            scale_primitive(&mut f.primitive, k);
        }
        // Sketches are live UI state (not persisted), but a stale metre-space sketch sitting
        // beside a rescaled solid would draw in the wrong place until it was reopened.
        for sk in &mut self.sketches {
            sk.frame.origin *= k;
        }
    }

    /// World-space triangle positions (3 per triangle) of one feature's raw mesh — for
    /// ray-pick against the real surface rather than the bounding box.
    pub fn feature_world_positions(&self, f: &Feature) -> Vec<[f32; 3]> {
        crate::csg::feature_world_positions(self, f)
    }

    /// Next free feature id.
    pub fn next_id(&self) -> u32 {
        self.features.iter().map(|f| f.id).max().map_or(1, |m| m + 1)
    }

    /// Append a fully-formed feature, re-stamping it with a fresh id (COPY).
    pub fn push_feature(&mut self, mut f: Feature) -> u32 {
        let id = self.next_id();
        f.id = id;
        self.features.push(f);
        id
    }

    /// Mutable access to a feature by id.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Feature> {
        self.features.iter_mut().find(|f| f.id == id)
    }

    /// Remove the feature with `id`; returns true if one was removed.
    pub fn remove(&mut self, id: u32) -> bool {
        let n = self.features.len();
        self.features.retain(|f| f.id != id);
        self.features.len() != n
    }

    /// Fold the whole history through csgrs into a triangle soup. Re-run on ANY
    /// param edit — this IS the parametric re-derivation.
    ///
    /// ON A THREAD WITH A BIG STACK, and that is not a performance decision.
    ///
    /// csgrs's BSP degenerates to a linked list for a convex body: `Node::build`,
    /// `clip_polygons`, `clip_to` and `Drop` are all recursive, and the tree depth of an extruded
    /// N-gon is measured at exactly N + 2. A curved wall traced from a DXF is an N-gon with a
    /// large N, so a real drawing walks a recursion thousands deep. On the default 8 MB main
    /// thread that is not a slow rebuild — it is `STATUS_STACK_OVERFLOW`, and the process is gone
    /// along with the drawing.
    ///
    /// WHY HERE AND NOT AT THE CALLERS. This one-line delegation is the choke point for every
    /// evaluation in the app: the rebuild after any model edit, the selection mesh on every 3D
    /// selection change, the repair command's before/after counts — and, decisively,
    /// `startup_repair`, which evaluates TWICE WHILE OPENING A FILE. Wrap only the rebuild and a
    /// deep model still kills the app on open, which means it cannot be opened, cannot be
    /// measured, and cannot be repaired.
    ///
    /// SCOPED, NOT SPAWNED. `std::thread::spawn` requires `'static`, so the closure would have to
    /// OWN the model — a full clone of every feature, profile and sketch, on every evaluation, at
    /// every call site. `scope` lets it borrow `&self` and hand the mesh back, so this costs one
    /// thread launch and nothing else.
    ///
    /// 64 MB is a judgement: eight times the default, comfortably past the depths measured so far,
    /// and trivial beside the mesh being built. It is reserved address space, not committed
    /// memory.
    pub fn eval(&self) -> SolidMesh {
        const EVAL_STACK: usize = 64 << 20;
        std::thread::scope(|s| {
            match std::thread::Builder::new()
                .name("csg-eval".into())
                .stack_size(EVAL_STACK)
                .spawn_scoped(s, || csg::eval(self))
            {
                Ok(h) => match h.join() {
                    Ok(mesh) => mesh,
                    // The worker panicked. Propagate rather than swallow — an empty mesh here
                    // reads as "the model is empty", and the caller would cache that and move on.
                    Err(e) => std::panic::resume_unwind(e),
                },
                // The OS refused a thread. Evaluate in place: a possible overflow on a very deep
                // model is still better than refusing to draw the scene at all.
                Err(_) => csg::eval(self),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_groups_split_bodies_and_faces() {
        // Two SEPARATE unit cubes (12 tris each) far apart → 2 bodies, 6 flat faces each = 12.
        let cube = |o: [f32; 3], out: &mut Vec<[f32; 3]>| {
            let v = |x: f32, y: f32, z: f32| [o[0] + x, o[1] + y, o[2] + z];
            // 8 corners, 6 faces × 2 tris.
            let c = [
                v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(1.0, 1.0, 0.0), v(0.0, 1.0, 0.0),
                v(0.0, 0.0, 1.0), v(1.0, 0.0, 1.0), v(1.0, 1.0, 1.0), v(0.0, 1.0, 1.0),
            ];
            let quad = |a: usize, b: usize, cc: usize, d: usize, out: &mut Vec<[f32; 3]>| {
                out.extend_from_slice(&[c[a], c[b], c[cc], c[a], c[cc], c[d]]);
            };
            quad(0, 1, 2, 3, out); // bottom
            quad(4, 5, 6, 7, out); // top
            quad(0, 1, 5, 4, out); // front
            quad(3, 2, 6, 7, out); // back
            quad(0, 3, 7, 4, out); // left
            quad(1, 2, 6, 5, out); // right
        };
        let mut pos: Vec<[f32; 3]> = Vec::new();
        cube([0.0, 0.0, 0.0], &mut pos);
        cube([10.0, 0.0, 0.0], &mut pos);
        let (face, body) = surface_groups(&pos);
        assert_eq!(face.len(), pos.len() / 3);
        let nbody = body.iter().copied().max().unwrap() + 1;
        let nface = face.iter().copied().max().unwrap() + 1;
        assert_eq!(nbody, 2, "two disjoint cubes → two bodies");
        assert_eq!(nface, 12, "each cube has 6 flat faces → 12 total");
        // Every triangle of cube 0 shares one body id, distinct from cube 1's.
        assert_ne!(body[0], body[body.len() - 1], "the two cubes are different bodies");
    }

    #[test]
    fn ray_hits_box_ahead_and_misses_beside() {
        let (mn, mx) = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        // Ray from -Z straight at the box.
        let hit = ray_aabb(Vec3::new(0.0, 0.0, -5.0), Vec3::Z, mn, mx);
        assert!(hit.is_some());
        assert!((hit.unwrap() - 4.0).abs() < 1e-4, "enters the box at t=4");
        // Ray parallel but offset well outside → miss.
        assert!(ray_aabb(Vec3::new(5.0, 0.0, -5.0), Vec3::Z, mn, mx).is_none());
    }

    /// An extrusion on a CUSTOM (arbitrary) plane must rise along THAT plane's normal, not
    /// world +Z — this is what makes "draw on a tilted face → extrude" land on the face.
    #[test]
    fn extrusion_on_a_custom_plane_rises_along_its_normal() {
        // A plane whose normal is +X (u = +Y, v = +Z ⇒ u×v = +X), at the origin.
        let plane = Plane::from_basis(Vec3::ZERO, Vec3::Y, Vec3::Z);
        assert!((plane.normal() - Vec3::X).length() < 1e-5, "normal is +X");
        let mut m = Model::default();
        let (profile, centre, w, d) = m.add_profile(&[
            Vec2::new(-0.5, -0.5), Vec2::new(0.5, -0.5),
            Vec2::new(0.5, 0.5), Vec2::new(-0.5, 0.5),
        ]).unwrap();
        m.push(
            BoolOp::Union, plane,
            Placement { u: centre.x, v: centre.y, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Extrusion { profile, h: 3.0, w, d },
        );
        let (mn, mx) = m.eval().bounds().expect("has geometry");
        // Extruded 3 m along +X ⇒ X spans ~0..3; the 1×1 profile stays thin in Y and Z.
        assert!((mx[0] - mn[0] - 3.0).abs() < 0.05, "extrudes 3 m along the plane normal (+X)");
        assert!((mx[1] - mn[1] - 1.0).abs() < 0.05 && (mx[2] - mn[2] - 1.0).abs() < 0.05,
            "the 1×1 profile is preserved in the plane");
    }

    /// A shape drawn OFF-CENTRE on a custom plane must land at ONE world spot, not also its
    /// mirror image — the "shows up on the opposite side too" report.
    #[test]
    fn custom_plane_extrusion_is_not_mirrored() {
        // Plane normal +X (u=Y, v=Z), origin at (5,0,0).
        let plane = Plane::from_basis(Vec3::new(5.0, 0.0, 0.0), Vec3::Y, Vec3::Z);
        let mut m = Model::default();
        // A 1×1 square centred at uv (2, 1).
        let (profile, centre, w, d) = m.add_profile(&[
            Vec2::new(1.5, 0.5), Vec2::new(2.5, 0.5),
            Vec2::new(2.5, 1.5), Vec2::new(1.5, 1.5),
        ]).unwrap();
        m.push(
            BoolOp::Union, plane,
            Placement { u: centre.x, v: centre.y, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Extrusion { profile, h: 2.0, w, d },
        );
        let (mn, mx) = m.eval().bounds().unwrap();
        // world = (5,0,0) + Y*2 + Z*1 = (5,2,1); extrude +X by 2 ⇒ x 5..7, y 1.5..2.5, z 0.5..1.5.
        assert!(mn[0] > 4.9 && mx[0] < 7.1, "x spans 5..7, got {}..{}", mn[0], mx[0]);
        assert!(mn[1] > 1.4 && mx[1] < 2.6, "y around 2, got {}..{}", mn[1], mx[1]);
        assert!(mn[2] > 0.4 && mx[2] < 1.6, "z around 1, got {}..{}", mn[2], mx[2]);
    }

    /// A whole-model rescale must shrink the evaluated geometry by exactly `k`.
    #[test]
    fn rescaling_the_model_scales_the_built_geometry() {
        let mut m = Model::default();
        m.push(
            BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Box { w: 3000.0, d: 1000.0, h: 2700.0 },
        );
        let (mn0, mx0) = m.eval().bounds().unwrap();
        assert!((mx0[0] - mn0[0] - 3000.0).abs() < 1.0, "starts 3000 wide");
        m.rescale(0.001);
        let (mn, mx) = m.eval().bounds().unwrap();
        assert!((mx[0] - mn[0] - 3.0).abs() < 0.01, "3000 units → 3 m, got {}", mx[0] - mn[0]);
        assert!((mx[2] - mn[2] - 2.7).abs() < 0.01, "height came with it");
    }

    /// THE subtle one. Profiles are SHARED — several storeys of a building point at one
    /// outline. Scaling per-feature would multiply that profile once per reference, so a
    /// two-storey building would come out 1000x too small on a x0.001 rescale. `rescale`
    /// must touch each shared entry exactly once.
    #[test]
    fn a_shared_profile_is_scaled_exactly_once() {
        let mut m = Model::default();
        let (profile, centre, w, d) = m.add_profile(&[
            Vec2::new(0.0, 0.0), Vec2::new(1000.0, 0.0),
            Vec2::new(1000.0, 1000.0), Vec2::new(0.0, 1000.0),
        ]).unwrap();
        // TWO features referencing the SAME profile — the storey pattern.
        for lift in [0.0_f32, 1000.0] {
            m.push(
                BoolOp::Union, Plane::default(),
                Placement { u: centre.x, v: centre.y, lift, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
                Primitive::Extrusion { profile, h: 1000.0, w, d },
            );
        }
        m.rescale(0.001);
        let (mn, mx) = m.eval().bounds().unwrap();
        // 1000-unit footprint → 1 m; two 1000-tall storeys stacked → 2 m.
        assert!((mx[0] - mn[0] - 1.0).abs() < 0.01,
            "footprint scaled ONCE (got {}, would be 0.001 if scaled twice)", mx[0] - mn[0]);
        assert!((mx[2] - mn[2] - 2.0).abs() < 0.01, "both storeys, {} tall", mx[2] - mn[2]);
    }

    /// Rescaling must not shear anything: a custom plane's basis vectors are DIRECTIONS, so
    /// only its origin moves. Scaling `u`/`v` would break orthonormality.
    #[test]
    fn rescaling_moves_a_custom_plane_without_distorting_it() {
        let mut m = Model::default();
        m.push(
            BoolOp::Union,
            Plane::from_basis(Vec3::new(1000.0, 0.0, 0.0), Vec3::Y, Vec3::Z),
            Placement::default(),
            Primitive::Box { w: 100.0, d: 100.0, h: 100.0 },
        );
        m.rescale(0.001);
        let p = m.features[0].plane;
        let c = p.custom.expect("still a custom plane");
        assert_eq!(c.origin, [1.0, 0.0, 0.0], "origin moved");
        assert_eq!(c.u, [0.0, 1.0, 0.0], "u is a direction — unchanged");
        assert_eq!(c.v, [0.0, 0.0, 1.0], "v is a direction — unchanged");
        assert!((p.normal().length() - 1.0).abs() < 1e-6, "still orthonormal");
    }

    /// A no-op factor must be exactly that, and a nonsensical one must be refused rather
    /// than silently collapsing the model to a point.
    #[test]
    fn rescale_refuses_nonsense_factors() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Box { w: 2.0, d: 2.0, h: 2.0 });
        for bad in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, 1.0] {
            m.rescale(bad);
            match m.features[0].primitive {
                Primitive::Box { w, .. } => assert_eq!(w, 2.0, "factor {bad} left it alone"),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn world_aabb_follows_placement_and_offset() {
        let f = Feature {
            id: 1,
            op: BoolOp::Union,
            plane: Plane { kind: PlaneKind::XY, offset: 2.0, custom: None },
            placement: Placement { u: 3.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            primitive: Primitive::Box { w: 2.0, d: 2.0, h: 1.0 },
        };
        let (mn, mx) = f.world_aabb();
        // Centred at u=3 on X → x spans 2..4; sits on plane at z=offset=2 → z 2..3.
        assert!((mn.x - 2.0).abs() < 1e-4 && (mx.x - 4.0).abs() < 1e-4);
        assert!((mn.z - 2.0).abs() < 1e-4 && (mx.z - 3.0).abs() < 1e-4);
    }

    fn box_at(u: f32, v: f32) -> Feature {
        Feature {
            id: 1,
            op: BoolOp::Union,
            plane: Plane::default(),
            placement: Placement { u, v, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            primitive: Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
        }
    }

    #[test]
    fn translate_moves_world_origin() {
        let f = box_at(1.0, 1.0).translated(Vec3::new(2.0, 0.0, 0.0));
        let o = f.world_origin();
        assert!((o - Vec3::new(3.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn rotate_90_about_z_at_origin() {
        // A feature at (1,0) rotated +90° about Z lands at (0,1), spin +90.
        let f = box_at(1.0, 0.0).rotated(Vec3::ZERO, Vec3::Z, std::f32::consts::FRAC_PI_2);
        let o = f.world_origin();
        assert!((o - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4, "origin orbits to (0,1), got {o:?}");
        assert!((f.placement.spin_deg - 90.0).abs() < 1e-3, "spin picks up +90°");
    }

    #[test]
    fn scale_2x_doubles_dims_and_pushes_origin() {
        let f = box_at(1.0, 0.0).scaled(Vec3::ZERO, 2.0);
        assert!((f.world_origin() - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-4);
        match f.primitive {
            Primitive::Box { w, d, h } => assert!((w - 2.0).abs() < 1e-4 && (d - 2.0).abs() < 1e-4 && (h - 2.0).abs() < 1e-4),
            _ => panic!("still a box"),
        }
    }

    #[test]
    fn ray_triangle_hits_and_misses() {
        // Triangle in the z=1 plane; ray from below straight up hits at t=1.
        let (a, b, c) = (Vec3::new(-1.0, -1.0, 1.0), Vec3::new(1.0, -1.0, 1.0), Vec3::new(0.0, 1.0, 1.0));
        let t = ray_triangle(Vec3::ZERO, Vec3::Z, a, b, c);
        assert!(t.map_or(false, |t| (t - 1.0).abs() < 1e-4));
        // A ray off to the side misses.
        assert!(ray_triangle(Vec3::new(5.0, 5.0, 0.0), Vec3::Z, a, b, c).is_none());
    }

    #[test]
    fn frame_uv_round_trips_on_a_tilted_face() {
        let f = Frame::from_point_normal(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 1.0, 0.0));
        let uv = Vec2::new(0.7, -1.3);
        let back = f.to_uv(f.from_uv(uv));
        assert!((back - uv).length() < 1e-4, "uv→world→uv is identity");
        // The reconstructed world point lies in the plane (normal component ~0).
        let w = f.from_uv(uv);
        assert!((w - f.origin).dot(f.normal()).abs() < 1e-4);
    }

    #[test]
    fn coplanar_face_groups_only_the_same_plane() {
        // Two coplanar tris (z=0 quad) + one tri on the x=0 plane sharing an edge.
        let positions = vec![
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], // tri 0 (z=0)
            [0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], // tri 1 (z=0, adjacent)
            [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], // tri 2 (x=0)
        ];
        let face = coplanar_face(&positions, 0);
        assert_eq!(face.len(), 2, "the z=0 face is the two coplanar tris");
        assert!(face.contains(&0) && face.contains(&1));
        assert!(!face.contains(&2), "the perpendicular tri is a different face");
    }

    #[test]
    fn circle_geom_outlines_to_a_closed_loop() {
        let g = cad_kernel::Geom::Circle(cad_kernel::Circle { center: KVec2::new(0.0, 0.0), radius: 2.0 });
        let paths = geom_outlines(&g);
        assert_eq!(paths.len(), 1);
        let p = &paths[0];
        assert!(p.len() > 8);
        // first ≈ last (closed) and every point is ~radius from centre.
        assert!((p[0] - p[p.len() - 1]).length() < 1e-3);
        assert!(p.iter().all(|q| (q.length() - 2.0).abs() < 1e-3));
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    fn ell() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 2.0),
            Vec2::new(2.0, 2.0), Vec2::new(2.0, 4.0), Vec2::new(0.0, 4.0),
        ]
    }

    /// The point of the whole design: `Primitive` and `Feature` must STAY `Copy`. A
    /// `Vec<Vec2>` inside the enum would have dropped it from both, and `Feature` is
    /// returned by value from `translated`/`rotated`/`scaled`/`mirrored`.
    ///
    /// This is a compile-time proof — if `Copy` were lost, this would not build.
    #[test]
    fn primitive_and_feature_are_still_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Primitive>();
        assert_copy::<Feature>();
    }

    /// An outline is stored once, recentred on its own bounds, and reports the extents
    /// the `Extrusion` variant caches for `local_aabb`.
    #[test]
    fn a_profile_is_stored_centred_with_its_extents() {
        let mut m = Model::default();
        let (id, centre, w, d) = m.add_profile(&ell()).expect("an L is a valid outline");
        assert_eq!((w, d), (4.0, 4.0));
        assert_eq!(centre, Vec2::new(2.0, 2.0));
        let p = m.profile(id).expect("the profile must be retrievable by id");
        assert_eq!(p.pts.len(), 6, "6 corners, no repeated wrap point");
        let (mut mnx, mut mxx) = (f32::INFINITY, f32::NEG_INFINITY);
        for q in &p.pts {
            mnx = mnx.min(q[0]);
            mxx = mxx.max(q[0]);
        }
        assert!((mnx + mxx).abs() < 1e-6, "stored points must be centred on the origin");
    }

    /// A closed ring must not keep its repeated first point, or the extrusion would carry
    /// a zero-length edge into earcut.
    #[test]
    fn a_repeated_wrap_point_is_dropped() {
        let mut m = Model::default();
        let mut closed = ell();
        closed.push(closed[0]);
        let (id, ..) = m.add_profile(&closed).unwrap();
        assert_eq!(m.profile(id).unwrap().pts.len(), 6);
    }

    /// Winding is normalised so extruded faces point consistently outward.
    #[test]
    fn winding_is_normalised_to_ccw() {
        let mut m = Model::default();
        let mut cw = ell();
        cw.reverse();
        let (id, ..) = m.add_profile(&cw).unwrap();
        let pts: Vec<Vec2> =
            m.profile(id).unwrap().pts.iter().map(|q| Vec2::new(q[0], q[1])).collect();
        assert!(signed_area2(&pts) > 0.0, "stored winding must be CCW whatever came in");
    }

    /// Refusal, not repair. earcut's behaviour on a self-intersecting loop is undefined,
    /// and a non-watertight mesh would silently poison the light calc downstream.
    #[test]
    fn bad_outlines_are_refused_with_a_reason() {
        let mut m = Model::default();
        assert_eq!(
            m.add_profile(&[Vec2::ZERO, Vec2::new(1.0, 0.0)]),
            Err(ProfileError::TooFewPoints)
        );
        assert_eq!(
            m.add_profile(&[Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(4.0, 0.0)]),
            Err(ProfileError::Degenerate),
            "collinear points enclose nothing"
        );
        // A bow-tie: the two diagonals cross.
        assert_eq!(
            m.add_profile(&[
                Vec2::new(0.0, 0.0), Vec2::new(4.0, 4.0),
                Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0),
            ]),
            Err(ProfileError::SelfIntersecting)
        );
        assert!(m.profiles.is_empty(), "a refused outline must not be stored");
    }

    /// A convex square is NOT self-intersecting — the adjacency skip must not produce
    /// false positives, or every legitimate outline would be rejected.
    #[test]
    fn an_ordinary_outline_is_not_flagged() {
        let mut m = Model::default();
        assert!(m
            .add_profile(&[
                Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0), Vec2::new(0.0, 4.0)
            ])
            .is_ok());
        assert!(m.add_profile(&ell()).is_ok(), "an L-shape is a valid building outline");
    }

    /// Ids are KEYS, not indices — the rule the rest of this crate already follows.
    #[test]
    fn profile_ids_are_stable_keys() {
        let mut m = Model::default();
        let (a, ..) = m.add_profile(&ell()).unwrap();
        let (b, ..) = m.add_profile(&ell()).unwrap();
        assert_ne!(a, b);
        assert_eq!(m.profile(a).map(|p| p.id), Some(a));
        assert!(m.profile(999).is_none(), "a stale id resolves to None, never a panic");
    }

    /// The whole point: an L-shaped outline becomes a real solid — the shape no Box
    /// could represent. Proven through csgrs, not just the data structures.
    #[test]
    fn an_l_shaped_outline_extrudes_to_a_real_solid() {
        let mut m = Model::default();
        let (id, centre, w, d) = m.add_profile(&ell()).unwrap();
        m.push(
            BoolOp::Union,
            Plane::default(),
            Placement { u: centre.x, v: centre.y, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Extrusion { profile: id, h: 3.0, w, d },
        );
        let mesh = m.eval();
        assert!(mesh.tri_count() > 0, "the extrusion must produce geometry");
        let (mn, mx) = mesh.bounds().expect("a solid has bounds");
        assert!((mx[2] - mn[2] - 3.0).abs() < 1e-3, "it must rise to its height");
        assert!((mx[0] - mn[0] - 4.0).abs() < 1e-3, "and span the outline's width");
    }

    /// A dangling profile id must degrade to empty geometry, never a panic — a stale id
    /// should not take the app down.
    #[test]
    fn a_stale_profile_id_yields_no_geometry_and_does_not_panic() {
        let mut m = Model::default();
        m.push(
            BoolOp::Union,
            Plane::default(),
            Placement::default(),
            Primitive::Extrusion { profile: 404, h: 2.0, w: 1.0, d: 1.0 },
        );
        assert_eq!(m.eval().tri_count(), 0);
    }

    /// `local_aabb` must stay a pure function of the primitive (it drives ray-pick and
    /// has no Model) — which is why the extents are cached in the variant.
    #[test]
    fn local_aabb_uses_the_cached_extents() {
        let p = Primitive::Extrusion { profile: 1, h: 3.0, w: 4.0, d: 6.0 };
        let (mn, mx) = p.local_aabb();
        assert_eq!(mn, Vec3::new(-2.0, -3.0, 0.0));
        assert_eq!(mx, Vec3::new(2.0, 3.0, 3.0));
    }
}

/// THE PLANE DECIDES THE COORDINATES, NOT THE CLICK.
///
/// Reported as: "i drew the building on the origin yet then i took a face to draw on, and even
/// though the face's corner is on the origin the face is shown as if it's in some other place."
/// `Frame::from_point_normal` takes the HIT POINT as the origin, so a sketch's (0, 0) landed
/// wherever the cursor happened to be and the face's own corner appeared at minus that offset —
/// two clicks a metre apart on the same face giving two coordinate systems for the same plane.
#[cfg(test)]
mod a_sketch_plane_is_canonical {
    use super::*;

    /// The reported case: a building whose corner is on the world origin, a face picked by
    /// clicking somewhere in the middle of it. Its corner must land on the sketch's (0, 0).
    #[test]
    fn the_corner_on_the_world_origin_lands_on_the_sketch_origin() {
        // The wall on y = 0 of a building spanning x 0..9.4, z 0..3.05 — clicked at (4.5, 0, 1.9),
        // metres away from the corner at the world origin.
        let clicked = Frame::from_point_normal(Vec3::new(4.5, 0.0, 1.9), Vec3::new(0.0, -1.0, 0.0));
        let before = clicked.to_uv(Vec3::ZERO);
        assert!(before.length() > 4.0, "precondition: the click puts the corner far from (0,0): {before:?}");

        let f = clicked.canonical();
        let after = f.to_uv(Vec3::ZERO);
        assert!(after.length() < 1e-5, "the corner must be the sketch origin, got {after:?}");
    }

    /// …and it must not depend on WHERE on the face you clicked.
    #[test]
    fn two_clicks_on_one_face_give_one_frame() {
        let n = Vec3::new(0.0, -1.0, 0.0);
        let a = Frame::from_point_normal(Vec3::new(1.0, 0.0, 0.5), n).canonical();
        let b = Frame::from_point_normal(Vec3::new(8.0, 0.0, 2.9), n).canonical();
        assert!((a.origin - b.origin).length() < 1e-5, "{:?} vs {:?}", a.origin, b.origin);
        assert!((a.u - b.u).length() < 1e-5 && (a.v - b.v).length() < 1e-5);
    }

    /// A sketch on a FLOOR must read like the plan: u = +X, v = +Y. The arbitrary-but-deterministic
    /// axes gave u = −Y and v = +X there — a quarter turn and a mirror away from the drawing the
    /// face is standing on.
    #[test]
    fn a_horizontal_face_reads_like_the_plan() {
        let f = Frame::from_point_normal(Vec3::new(3.0, 2.0, 0.0), Vec3::Z).canonical();
        assert!((f.u - Vec3::X).length() < 1e-5, "u = {:?}", f.u);
        assert!((f.v - Vec3::Y).length() < 1e-5, "v = {:?}", f.v);
        // A point 2 m east and 1 m north of the origin reads as (2, 1).
        let uv = f.to_uv(Vec3::new(2.0, 1.0, 0.0));
        assert!((uv.x - 2.0).abs() < 1e-5 && (uv.y - 1.0).abs() < 1e-5, "{uv:?}");
    }

    /// A sketch on a WALL must read like an elevation: across, then UP.
    #[test]
    fn a_vertical_face_reads_like_an_elevation() {
        for n in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y] {
            let f = Frame::from_point_normal(n * 2.0, n).canonical();
            assert!(f.u.z.abs() < 1e-5, "u must be horizontal for n = {n:?}, got {:?}", f.u);
            assert!((f.v - Vec3::Z).length() < 1e-5, "v must be up for n = {n:?}, got {:?}", f.v);
        }
    }

    /// Whatever else it does, it must still describe the SAME PLANE — same normal, same offset —
    /// or a sketch would quietly move the face it is drawn on.
    #[test]
    fn it_is_the_same_plane_it_started_as() {
        for (p, n) in [
            (Vec3::new(4.5, 0.0, 1.9), Vec3::new(0.0, -1.0, 0.0)),
            (Vec3::new(1.0, 2.0, 3.0), Vec3::Z),
            (Vec3::new(-7.0, 5.0, 2.0), Vec3::new(1.0, 1.0, 0.0).normalize()),
            (Vec3::new(0.5, 0.5, 4.0), Vec3::new(0.3, -0.2, 0.9).normalize()),
        ] {
            let a = Frame::from_point_normal(p, n);
            let b = a.canonical();
            assert!(a.normal().dot(b.normal()) > 0.9999, "the normal turned: {:?} → {:?}", a.normal(), b.normal());
            // The original origin still lies IN the canonical plane.
            let off = (p - b.origin).dot(b.normal());
            assert!(off.abs() < 1e-4, "the plane moved by {off} m");
            // …and the frame is still right-handed and orthonormal.
            assert!((b.u.length() - 1.0).abs() < 1e-5 && (b.v.length() - 1.0).abs() < 1e-5);
            assert!(b.u.dot(b.v).abs() < 1e-5, "axes are not perpendicular");
            assert!(b.u.cross(b.v).dot(b.normal()) > 0.9999, "left-handed — a sketch would mirror");
        }
    }

    /// Round-tripping through the canonical frame must be exact, or drawing on a face would
    /// deposit geometry slightly off it.
    #[test]
    fn uv_round_trips() {
        let f = Frame::from_point_normal(Vec3::new(4.5, 0.0, 1.9), Vec3::new(0.0, -1.0, 0.0)).canonical();
        for w in [Vec3::new(0.0, 0.0, 0.0), Vec3::new(9.4, 0.0, 3.05), Vec3::new(2.0, 0.0, 1.0)] {
            let back = f.from_uv(f.to_uv(w));
            assert!((back - w).length() < 1e-5, "{w:?} → {back:?}");
        }
    }
}
