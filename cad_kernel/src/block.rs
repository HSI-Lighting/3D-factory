//! Blocks — named, reusable groups of dobjects (AutoCAD BLOCK / INSERT).
//!
//! A `Block` is a DEFINITION: a name, a base point, and the contained
//! dobjects stored in DEFINITION SPACE (their coordinates as captured at
//! create time; the base point is the handle they're carried by). It is
//! never drawn directly.
//!
//! A `BlockRef` (`Geom::BlockRef`) is a placed INSTANCE: it references a
//! definition by id and carries a similarity transform —
//! `p_world = insert + R(rotation) · scale · (p_def − base)`.
//!
//! v1 constraints (documented, deliberate):
//! - **Uniform scale only.** A similarity transform maps circles→circles
//!   and arcs→arcs, so every contained Geom transforms exactly with the
//!   existing `scaled`/`rotated`/`translated` methods. Non-uniform scale
//!   (AutoCAD's arc→ellipse pathology) is deferred.
//! - **No true mirrored instances.** `Geom::mirrored` on a BlockRef
//!   reflects the insertion point and rotation but the content keeps its
//!   handedness (a `mirrored: bool` flag is the B2 fix). Mirror a block
//!   you care about by exploding first.
//! - **Cycles cannot form**: a definition gets its id only at creation
//!   and there is no block-redefinition yet, so a block can never
//!   (transitively) contain itself.

use crate::dobject::DObject;
use crate::geom::Geom;
use crate::math::Vec2;

/// Maximum number of parametric parameters a (parametric) block may carry.
/// Kept small + fixed so `BlockRef` stays `Copy` (no heap per instance).
pub const MAX_BLOCK_PARAMS: usize = 8;

/// A placed instance of a block definition.
#[derive(Clone, Copy, Debug)]
pub struct BlockRef {
    /// Id into `Document.blocks`.
    pub block:    u32,
    /// World-space insertion point (where the definition's base lands).
    pub insert:   Vec2,
    /// X-axis scale MAGNITUDE (> 0). A reflected instance keeps a positive
    /// scale and sets `mirror_x` instead (so circles/arcs stay correct).
    pub scale:    f64,
    /// Y-axis scale MAGNITUDE (> 0). Equals `scale` for a uniform instance;
    /// when it differs (a stretched block) circles/arcs become ellipses on
    /// insert/explode. From DXF group 42.
    pub scale_y:  f64,
    /// Rotation in radians, CCW.
    pub rotation: f64,
    /// Mirrored: reflect the definition across its local Y axis (through the
    /// base) BEFORE scale+rotation. Set from a negative INSERT axis scale
    /// (e.g. DXF group 41 < 0 → a mirrored furniture block). Assumes
    /// |sx| == |sy| (a similarity); non-uniform scale isn't modelled yet.
    pub mirror_x: bool,
    /// Per-instance values for the block's parametric `params` (parallel by
    /// index; unused slots ignored). Empty/non-parametric blocks ignore
    /// this. Fixed array so `BlockRef` stays `Copy`. Defaults to 0.0;
    /// the inserter sets each to the param's `source_value`.
    pub param_values: [f64; MAX_BLOCK_PARAMS],
}

/// One MODIFIER VECTOR of a parametric block — a stretch window + a unit
/// direction (taken from the source→target diff). A named `BlockParam`
/// (variable) drives one or more of these. The displacement applied to a
/// vector at a variable value `V` is `dir * gain * (V - original)` — i.e.
/// the value is in the SAME units as the geometry, scaled by `gain` (1
/// unit of value = `gain` units of displacement along `dir`).
#[derive(Clone, Copy, Debug)]
pub struct ParamVector {
    /// Stretch window (definition/base-relative space) this vector moves.
    pub win_min: Vec2,
    pub win_max: Vec2,
    /// Unit direction of the displacement.
    pub dir:     Vec2,
    /// How much this vector moves per unit of `(value - original)`.
    /// 1.0 = moves the full amount (one-sided opening); 0.5 = half (a
    /// symmetric/centered opening where each side takes half the change).
    /// This is what makes several linked vectors CORRELATE to one value.
    pub gain:    f64,
}

/// One named parametric VARIABLE of a (semi-smart) block — e.g. `width`.
/// It owns one or more `vectors` (LINKED P's that move together). `original`
/// is the value the SOURCE block represents; at insert the user enters a
/// value `V` and every linked vector displaces by `dir * (V - original)`.
#[derive(Clone, Debug)]
pub struct BlockParam {
    pub name:     String,
    /// The value the SOURCE block represents (displacement 0 here).
    pub original: f64,
    /// The modifier vectors this variable drives together (≥1).
    pub vectors:  Vec<ParamVector>,
}

impl BlockRef {
    /// Map a definition-space geom into world space for this instance:
    /// scale about the base, rotate about the base, then carry the base
    /// to the insertion point. Works for every Geom variant — including
    /// nested `BlockRef`s, which is what makes nested blocks render and
    /// explode for free.
    pub fn transform_geom(&self, g: &Geom, base: Vec2) -> Geom {
        // Mirrored instances reflect across the local Y axis (through base)
        // FIRST, then the positive scale + rotation apply. Composing this way
        // reproduces R(rot)·diag(±|s|,±|s|)·(p−base) exactly, and reuses the
        // correct arc/ellipse reflection in `mirrored` (vs a negative scale,
        // which would leave arc winding wrong).
        let mirrored;
        let src = if self.mirror_x {
            mirrored = g.mirrored(base, base + Vec2 { x: 0.0, y: 1.0 });
            &mirrored
        } else {
            g
        };
        src.scaled_xy(base, self.scale, self.scale_y)
            .rotated(base, self.rotation)
            .translated(self.insert - base)
    }
}

/// A block definition. Contained dobjects keep their full per-dobject
/// style (layer / color / linetype); `Color::ByBlock` entries resolve to
/// the instance's color at render time.
#[derive(Clone, Debug)]
pub struct Block {
    pub name:     String,
    /// Base point in definition space (the "grip" the instance carries).
    pub base:     Vec2,
    pub dobjects: Vec<DObject>,
    /// Smart-block marker. When true the definition is intended to be
    /// re-derived by a (forthcoming) smart-block algorithm rather than
    /// treated as a static instance. No behaviour is attached yet — the
    /// flag is carried so the editor/UI can mark it and the algorithm can
    /// hook in later. NOT yet persisted to RSM (reader defaults it false,
    /// like the dim/wall style tables — see rsm.rs).
    pub smart:    bool,
    /// Parametric parameters (named modifier vectors). Empty = a plain
    /// static block. Populated by the block-diff "Set parameters" flow;
    /// each instance carries values in `BlockRef.param_values`. NOT yet
    /// persisted to RSM (reader defaults it empty).
    pub params:   Vec<BlockParam>,
    /// Indices (into `dobjects`) of the edges that bound an OPENING this block
    /// cuts into host geometry on insert — e.g. a door/window's two jambs.
    /// On insert the region enclosed by these (derived + transformed) edges is
    /// trimmed out of overlapping host lines/polylines. Empty = cuts nothing.
    /// NOT yet persisted to RSM (reader defaults it empty).
    pub cut_edges: Vec<usize>,
}

/// Table of block definitions on the Document. Unlike the style tables
/// there is NO reserved entry at id 0 — an empty drawing has no blocks.
#[derive(Clone, Debug, Default)]
pub struct BlockTable {
    pub blocks: Vec<Block>,
}

impl BlockTable {
    pub fn get(&self, id: u32) -> Option<&Block> {
        self.blocks.get(id as usize)
    }
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Block> {
        self.blocks.get_mut(id as usize)
    }
    pub fn add(&mut self, b: Block) -> u32 {
        let id = self.blocks.len() as u32;
        self.blocks.push(b);
        id
    }
    pub fn find(&self, name: &str) -> Option<u32> {
        self.blocks.iter().position(|b| b.name.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }
    pub fn len(&self) -> usize { self.blocks.len() }
    pub fn is_empty(&self) -> bool { self.blocks.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Line};

    fn close(p: Vec2, q: Vec2) -> bool { (p - q).len() < 1e-9 }

    #[test]
    fn transform_geom_composes_scale_rotate_translate() {
        // Definition: line (1,0)→(2,0), base (1,0). Instance at (10,10),
        // scale 2, rotation 90° CCW. Expected: start lands ON the insert
        // point; end = insert + R90·2·(1,0) = (10,12).
        let br = BlockRef {
            block: 0,
            insert: Vec2::new(10.0, 10.0),
            scale: 2.0, scale_y: 2.0,
            rotation: std::f64::consts::FRAC_PI_2,
            mirror_x: false,
            param_values: [0.0; MAX_BLOCK_PARAMS],
        };
        let g = Geom::Line(Line { a: Vec2::new(1.0, 0.0), b: Vec2::new(2.0, 0.0) });
        let out = br.transform_geom(&g, Vec2::new(1.0, 0.0));
        let Geom::Line(l) = out else { panic!("expected line") };
        assert!(close(l.a, Vec2::new(10.0, 10.0)), "a={:?}", l.a);
        assert!(close(l.b, Vec2::new(10.0, 12.0)), "b={:?}", l.b);
    }

    #[test]
    fn transform_geom_keeps_circles_circular() {
        // Uniform scale ⇒ circle stays a circle with scaled radius.
        let br = BlockRef {
            block: 0,
            insert: Vec2::new(5.0, 0.0),
            scale: 3.0, scale_y: 3.0,
            rotation: 1.234,
            mirror_x: false,
            param_values: [0.0; MAX_BLOCK_PARAMS],
        };
        let g = Geom::Circle(Circle { center: Vec2::new(0.0, 0.0), radius: 2.0 });
        let out = br.transform_geom(&g, Vec2::new(0.0, 0.0));
        let Geom::Circle(c) = out else { panic!("expected circle") };
        assert!(close(c.center, Vec2::new(5.0, 0.0)));
        assert!((c.radius - 6.0).abs() < 1e-9);
    }

    #[test]
    fn mirrored_instance_reflects_across_local_y() {
        // mirror_x reflects the definition across the vertical line through
        // base BEFORE scale/rotation. base=(0,0), no scale/rotation/insert:
        // line (1,0)→(2,0) ⇒ (-1,0)→(-2,0).
        let br = BlockRef {
            block: 0, insert: Vec2::new(0.0, 0.0), scale: 1.0, scale_y: 1.0, rotation: 0.0,
            mirror_x: true, param_values: [0.0; MAX_BLOCK_PARAMS],
        };
        let g = Geom::Line(Line { a: Vec2::new(1.0, 0.0), b: Vec2::new(2.0, 0.0) });
        let Geom::Line(l) = br.transform_geom(&g, Vec2::new(0.0, 0.0)) else { panic!() };
        assert!(close(l.a, Vec2::new(-1.0, 0.0)), "a={:?}", l.a);
        assert!(close(l.b, Vec2::new(-2.0, 0.0)), "b={:?}", l.b);
    }

    #[test]
    fn nonuniform_scale_turns_circle_into_ellipse() {
        // A block inserted with scale (2,1) stretches a unit circle into an
        // ellipse: semi-major 2 along x, ratio 0.5 (LibreCAD parity).
        let br = BlockRef {
            block: 0, insert: Vec2::new(0.0, 0.0), scale: 2.0, scale_y: 1.0,
            rotation: 0.0, mirror_x: false, param_values: [0.0; MAX_BLOCK_PARAMS],
        };
        let g = Geom::Circle(Circle { center: Vec2::new(0.0, 0.0), radius: 1.0 });
        match br.transform_geom(&g, Vec2::new(0.0, 0.0)) {
            Geom::Ellipse(e) => {
                assert!((e.major.len() - 2.0).abs() < 1e-9, "major={:?}", e.major);
                assert!((e.ratio - 0.5).abs() < 1e-9, "ratio={}", e.ratio);
                assert!(e.major.y.abs() < 1e-9, "major should lie on x: {:?}", e.major);
            }
            other => panic!("expected ellipse, got {other:?}"),
        }
    }
}
