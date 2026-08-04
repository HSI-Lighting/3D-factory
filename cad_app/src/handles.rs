//! Swappable door-handle library.
//!
//! Reads `assets/handles/handles.json` (schema 1) and mounts a handle on the parametric door.
//! The library is authored in the HANDLE FRAME, in metres:
//!
//! ```text
//!   origin   the SPINDLE centre, lying ON the door face
//!   +X       the direction the lever points
//!   +Y       up the door
//!   +Z       out of the door face (the projection)
//! ```
//!
//! Two things about that frame are load-bearing and easy to get wrong:
//!
//! - The origin is the **spindle**, not the plate centre. Three of the five plates are
//!   deliberately off-centre about it — the euro backplate's spindle sits 45 mm ABOVE its plate
//!   centre and the smart lock's 18.5 mm below. Mounting either by its plate centre puts the lever
//!   at the wrong height, and no render makes that obvious.
//! - `+X` is normalised across the whole library; one source asset was left-handed and was
//!   mirrored at build time.
//!
//! Everything here works in MILLIMETRES, because that is what the manifest and the measurements
//! are in. The conversion to the app's metres happens at the boundary, once.

use serde::Deserialize;

/// Bounds in the handle frame, millimetres.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct BBoxMm {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One handle from the manifest. Unknown fields are ignored so the library can grow without this
/// struct having to be updated in step.
#[derive(Debug, Clone, Deserialize)]
pub struct Handle {
    pub id: String,
    pub name: String,
    pub style: String,
    pub mount: String,
    #[serde(default)]
    pub source_mirrored: bool,
    pub bbox_mm: BBoxMm,
    pub projection_mm: f32,
    pub lever_reach_mm: f32,
    /// The smallest backset that keeps the plate off the leaf's leading edge. This is the rule the
    /// door's own spec does not have, and it is the one that actually bites.
    pub min_backset_mm: f32,
    #[serde(default = "default_handle_height")]
    pub default_handle_height_mm: f32,
    #[serde(default)]
    pub finishes: Vec<String>,
    #[serde(default)]
    pub default_finish: String,
    #[serde(default)]
    pub parts: Vec<String>,
    pub mesh: String,
    pub preview: String,
    #[serde(default)]
    pub preview_face: String,
}

fn default_handle_height() -> f32 {
    1050.0
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    schema: u32,
    handles: Vec<Handle>,
}

/// The library, plus where it was loaded from (mesh/preview paths are relative to that).
#[derive(Debug, Clone, Default)]
pub struct HandleLibrary {
    pub dir: std::path::PathBuf,
    pub handles: Vec<Handle>,
}

impl HandleLibrary {
    /// Read `<dir>/handles.json`. Reads the FOLDER, not a hardcoded list — a new handle is a new
    /// FBX, a new preview and a new manifest entry, with no code change.
    pub fn load(dir: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let dir = dir.as_ref().to_path_buf();
        let path = dir.join("handles.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if m.schema != 1 {
            return Err(format!("{}: schema {} is not 1", path.display(), m.schema));
        }
        Ok(Self { dir, handles: m.handles })
    }

    pub fn get(&self, id: &str) -> Option<&Handle> {
        self.handles.iter().find(|h| h.id == id)
    }

    /// Absolute path to a handle's mesh / preview / elevation.
    pub fn mesh_path(&self, h: &Handle) -> std::path::PathBuf {
        self.dir.join(&h.mesh)
    }
    pub fn preview_path(&self, h: &Handle) -> std::path::PathBuf {
        self.dir.join(&h.preview)
    }
    pub fn preview_face_path(&self, h: &Handle) -> std::path::PathBuf {
        self.dir.join(&h.preview_face)
    }
}

/// The door a handle is being fitted to. Millimetres, to match the manifest.
#[derive(Debug, Clone, Copy)]
pub struct DoorFit {
    pub door_width_mm: f32,
    pub door_height_mm: f32,
    pub leaf_thickness_mm: f32,
    pub handle_backset_mm: f32,
    pub handle_height_mm: f32,
    /// +1 = hinges on the +X edge, −1 = on −X.
    pub hinge_side: f32,
}

impl Default for DoorFit {
    fn default() -> Self {
        // The parametric door's own defaults (`cad_solid::door::DoorInput`), in mm.
        Self {
            door_width_mm: 793.25,
            door_height_mm: 2098.28,
            leaf_thickness_mm: 44.0,
            handle_backset_mm: 54.95,
            handle_height_mm: 1068.74,
            hinge_side: 1.0,
        }
    }
}

/// Why a handle cannot be mounted, or should be questioned. Carries its own message so the picker
/// can show it on hover without restating the rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Complaint {
    pub rule: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct Fitness {
    pub errors: Vec<Complaint>,
    pub warnings: Vec<Complaint>,
}

impl Fitness {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
    /// One line for a tooltip: why it is unavailable, or what to watch for.
    pub fn summary(&self) -> String {
        let all: Vec<&Complaint> = self.errors.iter().chain(self.warnings.iter()).collect();
        all.iter().map(|c| c.message.as_str()).collect::<Vec<_>>().join("; ")
    }
}

/// Check a handle against a door.
///
/// Errors refuse the mount; warnings mount anyway. The picker must GREY OUT rather than hide a
/// failing handle — a user who has chosen the smart lock and cannot see why it vanished will
/// assume the library is broken.
pub fn fitness(h: &Handle, d: &DoorFit) -> Fitness {
    let mut f = Fitness::default();
    let reach = h.lever_reach_mm;

    // ── hard errors ──
    if d.handle_backset_mm < h.min_backset_mm {
        f.errors.push(Complaint {
            rule: "B4.1",
            message: format!(
                "backset {:.1} mm is under this handle's minimum {:.1} mm — the plate would overhang the leaf's leading edge",
                d.handle_backset_mm, h.min_backset_mm
            ),
        });
    }
    if d.handle_backset_mm + reach >= d.door_width_mm {
        f.errors.push(Complaint {
            rule: "B4.2",
            message: format!(
                "lever reaches {:.1} mm from the edge on a {:.0} mm leaf — it would pass the architrave and the door could not close",
                d.handle_backset_mm + reach,
                d.door_width_mm
            ),
        });
    }
    let top = d.handle_height_mm + h.bbox_mm.max[1];
    let bottom = d.handle_height_mm + h.bbox_mm.min[1];
    if top > d.door_height_mm {
        f.errors.push(Complaint {
            rule: "B4.3",
            message: format!(
                "the plate top lands at {:.1} mm and overruns the {:.0} mm leaf by {:.1} mm",
                top,
                d.door_height_mm,
                top - d.door_height_mm
            ),
        });
    }
    if bottom < 0.0 {
        f.errors.push(Complaint {
            rule: "B4.3",
            message: format!("the plate bottom lands at {bottom:.1} mm — below the foot of the leaf"),
        });
    }

    // ── warnings ──
    if h.projection_mm > 75.0 {
        f.warnings.push(Complaint {
            rule: "B4.4",
            message: format!(
                "stands {:.1} mm off the leaf — it will strike a wall before a 90° opening",
                h.projection_mm
            ),
        });
    }
    if d.handle_backset_mm + reach > d.door_width_mm / 2.0 {
        f.warnings.push(Complaint {
            rule: "B4.5",
            message: format!(
                "the lever crosses the leaf's centreline ({:.1} mm of {:.0} mm) — legal, but it looks wrong on a narrow door",
                d.handle_backset_mm + reach,
                d.door_width_mm
            ),
        });
    }
    if !(900.0..=1100.0).contains(&d.handle_height_mm) {
        f.warnings.push(Complaint {
            rule: "B4.6",
            message: format!("handle at {:.0} mm is outside the usual 900–1100 mm", d.handle_height_mm),
        });
    }
    if matches!(h.mount.as_str(), "backplate_euro" | "backplate_smart" | "euro" | "smart") {
        f.warnings.push(Complaint {
            rule: "B4.7",
            message: "this pattern expects a lock case; without one the keyhole and thumbturn are decoration".into(),
        });
    }
    f
}

/// Which face of the leaf a handle instance sits on. Mount BOTH for every door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Front,
    Back,
}

impl Face {
    fn sign(self) -> f32 {
        match self {
            Face::Front => 1.0,
            Face::Back => -1.0,
        }
    }
}

/// The spindle position on the leaf, in the door's own frame (metres, `x` across the opening,
/// `y` out of the wall, `z` up, front face at `y = 0`).
pub fn spindle(d: &DoorFit) -> (f32, f32) {
    let x = -d.hinge_side * (d.door_width_mm / 2.0 - d.handle_backset_mm);
    (x / 1000.0, d.handle_height_mm / 1000.0)
}

/// Handle frame → door frame, for one face of the leaf.
///
/// ```text
///            |  hand   0     0     spindle_x |
///   M(face) =|  0      0     face  y0        |     hand = hinge_side
///            |  0      1     0     spindle_z |     y0 = 0 (front) or -leaf_t (back)
///            |  0      0     0     1         |
/// ```
///
/// **The X coefficient is `hand` on BOTH faces — never `hand * face`.** That is a
/// plausible-sounding mistake: the back handle *is* the mirror of the front one, but the mirror is
/// through the leaf's MID-PLANE, which reverses Y and leaves X alone. Multiplying by `face` swings
/// the back lever round to point out over the leaf's leading edge and through the architrave, and
/// the door will not close. Every front-face render looks perfect either way; only a view from
/// behind shows it.
///
/// The handedness flip falls out on its own: `det M = -hand * face`, so exactly one of the two
/// faces carries the mirrored part — which is precisely what a handed pair is.
pub fn mount_matrix(d: &DoorFit, face: Face) -> glam::Mat4 {
    let hand = if d.hinge_side >= 0.0 { 1.0 } else { -1.0 };
    let f = face.sign();
    let (sx, sz) = spindle(d);
    let y0 = match face {
        Face::Front => 0.0,
        Face::Back => -d.leaf_thickness_mm / 1000.0,
    };
    // glam is COLUMN-major: from_cols takes the columns of the matrix written above.
    glam::Mat4::from_cols(
        glam::Vec4::new(hand, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
        glam::Vec4::new(0.0, f, 0.0, 0.0),
        glam::Vec4::new(sx, y0, sz, 1.0),
    )
}

/// True when this mount mirrors the part — and therefore when any LETTERING has to be reflected
/// about the handle's centreline `x = 0` before mounting.
///
/// The mirror is right for hardware and wrong for text. Reflecting each glyph about ITS OWN centre
/// un-reverses the character but leaves it on the mirrored key, so the keypad reads a tidy,
/// plausible, still-wrong `9 8 7 / # 0 *`. Reflecting about `x = 0` moves the glyph to the opposite
/// column as well, and the mount's own mirror carries it back to the column it belongs on.
pub fn mirrors_lettering(m: &glam::Mat4) -> bool {
    m.determinant() < 0.0
}

/// Reflect a lettering vertex about the handle's centreline. Apply to every part named `Legend_*`
/// when [`mirrors_lettering`] is true, BEFORE the mount matrix.
pub fn unmirror_legend(p: glam::Vec3) -> glam::Vec3 {
    glam::Vec3::new(-p.x, p.y, p.z)
}

/// Transform a handle's mesh onto **both faces** of a leaf and append it to the door's own arrays,
/// in the door's frame. Triangle soup in, triangle soup out.
///
/// This is the whole of "the door and its handle are one object", and it lives here — pure, with no
/// file I/O and no app types — so it can be tested and rendered without an app around it. The
/// caller supplies the bytes; this decides where they go.
///
/// Handle part ids CONTINUE after the door's, so a lever and a rose stay individually selectable
/// and recolourable instead of merging into the leaf.
pub fn weld_onto(
    fit: &DoorFit,
    src_pos: &[[f32; 3]],
    src_nrm: &[[f32; 3]],
    src_parts: &[u32],
    out_pos: &mut Vec<[f32; 3]>,
    out_nrm: &mut Vec<[f32; 3]>,
    out_parts: &mut Vec<u32>,
) {
    let tris = src_pos.len() / 3;
    if tris == 0 || src_nrm.len() < tris * 3 {
        return;
    }
    let base = out_parts.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let tagged = src_parts.len() == tris;
    for face in [Face::Front, Face::Back] {
        let m = mount_matrix(fit, face);
        let n3 = glam::Mat3::from_mat4(m);
        // A mirrored face reverses the winding, so its normals must be negated with it or the back
        // handle lights inside-out — the one artefact that only shows from behind the door.
        let flip = if m.determinant() < 0.0 { -1.0 } else { 1.0 };
        for t in 0..tris {
            for k in 0..3 {
                let i = t * 3 + k;
                let w = m.transform_point3(glam::Vec3::from(src_pos[i]));
                out_pos.push([w.x, w.y, w.z]);
                let n = (n3 * glam::Vec3::from(src_nrm[i])).normalize_or_zero() * flip;
                out_nrm.push([n.x, n.y, n.z]);
            }
            out_parts.push(base + if tagged { src_parts[t] } else { 0 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped library. Skipped rather than failed when it is not on this machine, so the
    /// suite still runs on a checkout without the Blender assets.
    fn lib() -> Option<HandleLibrary> {
        HandleLibrary::load(r"G:\blender dev\staircase\door handles_\assets\handles").ok()
    }

    /// B7, numeric. These are the defaults the manifest must produce — the check the report says
    /// caught every bug that at least one render missed.
    #[test]
    fn the_library_reports_the_measured_numbers() {
        let Some(l) = lib() else { return };
        #[rustfmt::skip]
        const WANT: [(&str, f32, f32, f32); 5] = [
            ("lever_rose_black",     72.49, 120.71, 29.2),
            ("lever_rose_chrome",    54.55,  99.84, 35.6),
            ("gothic_backplate",     39.74, 177.77, 36.7),
            ("lever_backplate_euro", 51.15, 127.48, 33.0),
            ("smartlock_keypad",     86.61, 166.75, 41.2),
        ];
        assert_eq!(l.handles.len(), 5, "five handles in the library");
        for (id, proj, reach, min_backset) in WANT {
            let h = l.get(id).unwrap_or_else(|| panic!("{id} missing from the manifest"));
            assert!((h.projection_mm - proj).abs() < 0.01, "{id} projection {} want {proj}", h.projection_mm);
            assert!((h.lever_reach_mm - reach).abs() < 0.01, "{id} reach {} want {reach}", h.lever_reach_mm);
            assert!((h.min_backset_mm - min_backset).abs() < 0.05, "{id} min backset {} want {min_backset}", h.min_backset_mm);
        }
    }

    /// Every handle must have been normalised to +X: the lever tip IS the bbox's max x, and the
    /// plate straddles the spindle. If `max.x` is not the reach, the handle was not normalised and
    /// one wrong sign puts a lever through the architrave.
    #[test]
    fn every_handle_points_its_lever_along_plus_x() {
        let Some(l) = lib() else { return };
        for h in &l.handles {
            assert!(h.bbox_mm.min[0] < 0.0, "{}: nothing behind the spindle", h.id);
            assert!(h.bbox_mm.max[0] > 0.0, "{}: nothing in front of the spindle", h.id);
            assert!(
                (h.bbox_mm.max[0] - h.lever_reach_mm).abs() < 0.01,
                "{}: max.x {} is not the lever tip {}", h.id, h.bbox_mm.max[0], h.lever_reach_mm
            );
        }
    }

    /// B7, parametric — the proof it is a library and not five models. At the door's own defaults,
    /// all five must mount on all four leaves.
    #[test]
    fn all_five_mount_on_all_four_leaves() {
        let Some(l) = lib() else { return };
        const LEAVES: [(f32, f32, &str); 4] = [
            (793.25, 2098.28, "default"),
            (838.0, 1981.0, "imperial"),
            (626.0, 2040.0, "narrow"),
            (926.0, 2340.0, "oversize"),
        ];
        for (w, ht, label) in LEAVES {
            for h in &l.handles {
                let d = DoorFit { door_width_mm: w, door_height_mm: ht, ..Default::default() };
                let f = fitness(h, &d);
                assert!(f.ok(), "{} on the {label} leaf: {}", h.id, f.summary());
            }
        }
    }

    /// …and the two failures the report names, with the right handles and the right reasons. A
    /// validation that never refuses anything is not a validation.
    #[test]
    fn the_named_failures_fail_for_the_named_reasons() {
        let Some(l) = lib() else { return };
        // A 35 mm backset is under three handles' minimum; the other two mount.
        let d = DoorFit { handle_backset_mm: 35.0, ..Default::default() };
        let mut refused: Vec<&str> = Vec::new();
        for h in &l.handles {
            let f = fitness(h, &d);
            if f.errors.iter().any(|c| c.rule == "B4.1") {
                refused.push(&h.id);
            }
        }
        refused.sort();
        assert_eq!(
            refused,
            vec!["gothic_backplate", "lever_rose_chrome", "smartlock_keypad"],
            "the backset rule must bite exactly these three"
        );

        // A 1850 mm handle on a 1981 mm leaf overruns for the smart lock and clears for the euro.
        let d = DoorFit { handle_height_mm: 1850.0, door_height_mm: 1981.0, door_width_mm: 838.0, ..Default::default() };
        let smart = l.get("smartlock_keypad").unwrap();
        let f = fitness(smart, &d);
        let over = f.errors.iter().find(|c| c.rule == "B4.3").expect("the smart lock must overrun");
        assert!(over.message.contains("37.7"), "by 37.7 mm, got: {}", over.message);
        let euro = l.get("lever_backplate_euro").unwrap();
        assert!(
            !fitness(euro, &d).errors.iter().any(|c| c.rule == "B4.3"),
            "the euro plate clears the same leaf"
        );
    }

    /// A5.1 — the bug that only a view from BEHIND reveals. The X coefficient is `hand` on both
    /// faces; taking it as `hand * face` swings the back lever out over the leading edge.
    #[test]
    fn the_back_lever_points_the_same_way_as_the_front() {
        let d = DoorFit::default();
        let hand = d.hinge_side;
        // The lever tip in the handle frame is +X. Where does it land on each face?
        let tip = glam::Vec3::new(0.12071, 0.0, 0.0); // lever_rose_black's reach, in metres
        let (sx, _) = spindle(&d);
        for face in [Face::Front, Face::Back] {
            let p = mount_matrix(&d, face).transform_point3(tip);
            let toward_hinges = (p.x - sx) * hand;
            assert!(
                toward_hinges > 0.0,
                "{face:?}: the lever must point toward the hinges, went {:.4} m",
                p.x - sx
            );
        }
    }

    /// …and exactly ONE of the two faces is mirrored, which is what a handed pair is.
    #[test]
    fn exactly_one_face_carries_the_mirrored_part() {
        for hinge in [1.0f32, -1.0] {
            let d = DoorFit { hinge_side: hinge, ..Default::default() };
            let front = mount_matrix(&d, Face::Front);
            let back = mount_matrix(&d, Face::Back);
            assert!(
                (front.determinant() * back.determinant()) < 0.0,
                "hinge {hinge}: one face mirrored, one not — dets {} and {}",
                front.determinant(), back.determinant()
            );
            // det M = -hand * face, exactly.
            assert!((front.determinant() + hinge).abs() < 1e-5);
            assert!((back.determinant() - hinge).abs() < 1e-5);
        }
    }

    /// A5.7 — lettering. Reflecting a glyph about its OWN centre gives the plausible, still-wrong
    /// `9 8 7`; the reflection has to be about the handle's centreline so the glyph changes column
    /// too, and the mount's mirror carries it back.
    #[test]
    fn lettering_is_reflected_about_the_handle_centreline() {
        let d = DoorFit::default();
        let mirrored = [Face::Front, Face::Back]
            .into_iter()
            .filter(|f| mirrors_lettering(&mount_matrix(&d, *f)))
            .count();
        assert_eq!(mirrored, 1, "exactly one face needs its lettering un-mirrored");

        // A glyph at the left column must land in the left column after un-mirror + mount.
        let face = if mirrors_lettering(&mount_matrix(&d, Face::Front)) { Face::Front } else { Face::Back };
        let m = mount_matrix(&d, face);
        let left = glam::Vec3::new(-0.02, 0.0, 0.01); // a key left of the centreline
        let naive = m.transform_point3(left);
        let fixed = m.transform_point3(unmirror_legend(left));
        let (sx, _) = spindle(&d);
        assert!(
            (naive.x - sx).signum() != (fixed.x - sx).signum(),
            "un-mirroring must move the glyph to the other column: {naive:?} vs {fixed:?}"
        );
    }

    /// The mount puts the spindle where the door says it is — on the face, at handle height. The
    /// origin is the SPINDLE, not the plate centre; three of five plates are deliberately
    /// off-centre about it, so mounting by the plate would put the lever at the wrong height.
    #[test]
    fn the_spindle_lands_on_the_leaf_face_at_handle_height() {
        let d = DoorFit::default();
        let (sx, sz) = spindle(&d);
        let front = mount_matrix(&d, Face::Front).transform_point3(glam::Vec3::ZERO);
        assert!((front.x - sx).abs() < 1e-6 && (front.z - sz).abs() < 1e-6);
        assert!(front.y.abs() < 1e-6, "the front spindle lies ON the front face, y = 0");
        let back = mount_matrix(&d, Face::Back).transform_point3(glam::Vec3::ZERO);
        assert!(
            (back.y + d.leaf_thickness_mm / 1000.0).abs() < 1e-6,
            "the back spindle lies on the back face"
        );
        // Projection goes OUT of each face, in opposite directions.
        let out = glam::Vec3::new(0.0, 0.0, 0.05); // +Z in the handle frame
        let pf = mount_matrix(&d, Face::Front).transform_point3(out);
        let pb = mount_matrix(&d, Face::Back).transform_point3(out);
        assert!(pf.y > 0.0, "the front handle stands out of the front face");
        assert!(pb.y < -d.leaf_thickness_mm / 1000.0, "the back handle stands out of the back face");
    }


    /// The weld puts the real library mesh where it belongs on the real door: one copy per face,
    /// both standing OUT of their own face, both centred on the spindle, and neither overrunning
    /// the leaf's leading edge. This is the check that a picture of the front would pass and a
    /// door in the model would fail.
    #[test]
    fn a_welded_handle_lands_on_both_faces_of_the_leaf() {
        let Some(l) = lib() else { return };
        let fit = DoorFit::default();
        let leaf_t = fit.leaf_thickness_mm / 1000.0;
        let (sx, sz) = spindle(&fit);
        for h in &l.handles {
            let path = l.mesh_path(h);
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let (mesh, pbr) = crate::mesh_io::parse_fbx_pbr_at(&bytes, path.parent());
            if mesh.tri_count() == 0 {
                continue;
            }
            // Start from a door that already has parts 1..=7, as a real leaf does.
            let (mut p, mut n, mut ids) = (Vec::new(), Vec::new(), vec![1u32, 7]);
            crate::handles::weld_onto(
                &fit, &mesh.positions, &mesh.normals, &pbr.part_ids, &mut p, &mut n, &mut ids,
            );
            let welded = p.len() / 3;
            assert_eq!(welded, mesh.tri_count() * 2, "{}: one copy per face", h.id);
            assert_eq!(n.len(), p.len(), "{}: a normal per vertex", h.id);
            assert!(ids[2..].iter().all(|&i| i > 7), "{}: handle ids continue past the door's", h.id);

            // Half the vertices stand out of the FRONT face (y > 0), half out of the BACK
            // (y < −leaf). Neither set may sit inside the leaf.
            let front = p.iter().filter(|v| v[1] > 1e-4).count();
            let back = p.iter().filter(|v| v[1] < -leaf_t - 1e-4).count();
            assert!(front > 0 && back > 0, "{}: {front} front / {back} back vertices", h.id);

            let (lo, hi) = p.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(mut a, mut b), v| {
                for i in 0..3 {
                    a[i] = a[i].min(v[i]);
                    b[i] = b[i].max(v[i]);
                }
                (a, b)
            });
            // The lever reaches INWARD from the spindle, never out past the leading edge.
            let lead = -fit.hinge_side * fit.door_width_mm / 2000.0;
            assert!(
                (lo[0] - lead.min(sx)).abs() < 0.05 || lo[0] > lead.min(sx) - 0.05,
                "{}: x {} runs past the leading edge {lead}", h.id, lo[0]
            );
            // And it is at the handle HEIGHT, not the plate's own centre — the trap the euro
            // backplate (spindle 45 mm above centre) exists to catch.
            assert!(lo[2] < sz && hi[2] > sz, "{}: z {lo:?}..{hi:?} straddles the spindle {sz}", h.id);
        }
    }

    /// Every handle's MESH must measure what its manifest says it does. This is the check that
    /// catches a unit error, and the one that was missing: the library loaded, the numbers in the
    /// manifest were right, the transform was right — and the geometry still arrived 100x too big
    /// because FBX measures in centimetres. A handle towering over a villa is what that looks like.
    #[test]
    fn every_handle_mesh_matches_its_manifest_bbox() {
        let Some(l) = lib() else { return };
        for h in &l.handles {
            let path = l.mesh_path(h);
            let Ok(bytes) = std::fs::read(&path) else { panic!("{}: unreadable", path.display()) };
            let (mesh, _) = crate::mesh_io::parse_fbx_pbr_at(&bytes, path.parent());
            assert!(mesh.tri_count() > 0, "{}: no triangles", h.id);
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &mesh.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            // The manifest is in millimetres; the mesh must arrive in METRES.
            for k in 0..3 {
                let want_lo = h.bbox_mm.min[k] / 1000.0;
                let want_hi = h.bbox_mm.max[k] / 1000.0;
                assert!(
                    (lo[k] - want_lo).abs() < 5e-4 && (hi[k] - want_hi).abs() < 5e-4,
                    "{}: axis {k} spans {:.4}..{:.4} m, manifest says {:.4}..{:.4}",
                    h.id, lo[k], hi[k], want_lo, want_hi
                );
            }
            // …and the two numbers the picker shows must be the mesh's own.
            assert!(
                (hi[2] * 1000.0 - h.projection_mm).abs() < 0.5,
                "{}: mesh projects {:.2} mm, manifest says {:.2}", h.id, hi[2] * 1000.0, h.projection_mm
            );
            assert!(
                (hi[0] * 1000.0 - h.lever_reach_mm).abs() < 0.5,
                "{}: mesh reaches {:.2} mm, manifest says {:.2}", h.id, hi[0] * 1000.0, h.lever_reach_mm
            );
        }
    }
    /// Every mesh and preview the manifest names must actually be there — the picker reads the
    /// folder, so a missing file is a broken tile rather than a compile error.
    #[test]
    fn every_manifest_path_resolves() {
        let Some(l) = lib() else { return };
        for h in &l.handles {
            for p in [l.mesh_path(h), l.preview_path(h), l.preview_face_path(h)] {
                assert!(p.exists(), "{}: {} is missing", h.id, p.display());
            }
        }
    }
}
