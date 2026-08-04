//! What the parametric door is MADE of.
//!
//! A door is not one material. A joiner specifies the leaf, its panel, the lining and the casing
//! separately as a matter of course — and the case that makes this worth having at all is the
//! glazed door, which is timber everywhere except one panel. So the choice is per COMPONENT, and
//! the components are exactly the `cad_solid::door::Part` ids the builder already tags every
//! triangle with.
//!
//! One palette drives two very different consumers, deliberately:
//!
//! - [`DoorMaterial::look`] — what [`crate::mesh_preview`] shades with, on the CPU, before the door
//!   exists.
//! - [`DoorMaterial::install`] — a real [`crate::factory::TextureAsset`] bound to the placed
//!   object's face groups, which is what the viewport and the path tracer read.
//!
//! They are the same numbers reached two ways. If the preview and the built door ever disagree
//! about what oak looks like, one of these two functions is wrong — which is the whole reason they
//! live side by side in one file.

use crate::factory::{FactoryState, ProcDef};
use crate::mesh_preview::PartLook;

/// A door component that can carry its own material. These map onto `cad_solid::door::Part`, but
/// grouped the way someone specifying a door groups them: the stops go with the lining they are
/// planted on, and the two casings are one architrave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    /// Stiles, rails and the panel mouldings — `Part::Leaf`.
    Leaf,
    /// The panel field alone — `Part::Panel`. This is the one that gets glazed.
    Panel,
    /// Lining + stops — `Part::Lining`, `Part::Stop`.
    Frame,
    /// Both casings — `Part::ArchFront`, `Part::ArchBack`.
    Architrave,
}

impl Slot {
    pub const ALL: [Slot; 4] = [Slot::Leaf, Slot::Panel, Slot::Frame, Slot::Architrave];

    pub fn label(self) -> &'static str {
        match self {
            Slot::Leaf => "Leaf",
            Slot::Panel => "Panel",
            Slot::Frame => "Frame / lining",
            Slot::Architrave => "Architrave",
        }
    }

    /// The `Part` ids this slot covers. Hardware (7, 8 and every welded handle part above it) is
    /// deliberately absent: ironmongery is chosen by FINISH in the Handles dialog, and offering it
    /// twice in two different vocabularies would only let the two disagree.
    pub fn parts(self) -> &'static [u32] {
        use cad_solid::door::Part;
        match self {
            Slot::Leaf => &[Part::Leaf as u32],
            Slot::Panel => &[Part::Panel as u32],
            Slot::Frame => &[Part::Lining as u32, Part::Stop as u32],
            Slot::Architrave => &[Part::ArchFront as u32, Part::ArchBack as u32],
        }
    }
}

/// The palette. Small and door-shaped on purpose — a joinery schedule offers a species, a paint
/// finish or a glass, not a texture browser. Anything outside this list is still reachable the
/// normal way: build the door, then paint a face from ▼ Textures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DoorMaterial {
    Oak,
    Walnut,
    Ash,
    PaintedWhite,
    PaintedGrey,
    PaintedBlack,
    ClearGlass,
    FrostedGlass,
}

impl DoorMaterial {
    pub const ALL: [DoorMaterial; 8] = [
        DoorMaterial::Oak,
        DoorMaterial::Walnut,
        DoorMaterial::Ash,
        DoorMaterial::PaintedWhite,
        DoorMaterial::PaintedGrey,
        DoorMaterial::PaintedBlack,
        DoorMaterial::ClearGlass,
        DoorMaterial::FrostedGlass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DoorMaterial::Oak => "Oak",
            DoorMaterial::Walnut => "Walnut",
            DoorMaterial::Ash => "Ash",
            DoorMaterial::PaintedWhite => "Painted — white",
            DoorMaterial::PaintedGrey => "Painted — grey",
            DoorMaterial::PaintedBlack => "Painted — black",
            DoorMaterial::ClearGlass => "Glass — clear",
            DoorMaterial::FrostedGlass => "Glass — frosted",
        }
    }

    /// Stable index in [`Self::ALL`] — a cheap key for "did the material change since the last
    /// frame", which is what the preview's cache turns on.
    pub fn key(self) -> u64 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as u64
    }

    /// See-through, and therefore something the renderer has to draw in its blended pass.
    pub fn is_glass(self) -> bool {
        matches!(self, DoorMaterial::ClearGlass | DoorMaterial::FrostedGlass)
    }

    /// The timber grain, when this is a timber. `None` for paint and glass, which have no pattern.
    pub fn grain(self) -> Option<ProcDef> {
        let oak = ProcDef::oak();
        match self {
            DoorMaterial::Oak => Some(oak),
            // Walnut: darker, browner, and a quieter figure than oak's open pores.
            DoorMaterial::Walnut => Some(ProcDef {
                col_a: [0.13, 0.077, 0.050],
                col_b: [0.36, 0.23, 0.15],
                surf_rough: [0.55, 0.38],
                bump: 0.22,
                contrast: 1.25,
                ..oak
            }),
            // Ash: pale, high-contrast straight grain — a wider ring spacing than oak.
            DoorMaterial::Ash => Some(ProcDef {
                col_a: [0.52, 0.44, 0.33],
                col_b: [0.83, 0.75, 0.62],
                scale: [30.0, 8.0, 2.0],
                surf_rough: [0.62, 0.44],
                bump: 0.28,
                ..oak
            }),
            _ => None,
        }
    }

    /// Base colour (sRGB), surface roughness, and opacity. The single source these numbers come
    /// from — both consumers below read them from here.
    fn base(self) -> ([f32; 3], f32, f32) {
        match self {
            // For a timber the colour is the grain's midpoint; the pattern supplies the rest.
            DoorMaterial::Oak => ([0.48, 0.36, 0.23], 0.55, 1.0),
            DoorMaterial::Walnut => ([0.245, 0.155, 0.10], 0.46, 1.0),
            DoorMaterial::Ash => ([0.675, 0.595, 0.475], 0.52, 1.0),
            // Paint on joinery is eggshell, not gloss and not chalk.
            DoorMaterial::PaintedWhite => ([0.90, 0.90, 0.885], 0.38, 1.0),
            DoorMaterial::PaintedGrey => ([0.44, 0.45, 0.46], 0.40, 1.0),
            DoorMaterial::PaintedBlack => ([0.055, 0.055, 0.060], 0.34, 1.0),
            // Float glass: barely tinted green, very smooth, mostly transparent.
            DoorMaterial::ClearGlass => ([0.82, 0.88, 0.85], 0.04, 0.16),
            // Frosted: the SURFACE is rough, which is what makes it translucent rather than clear.
            DoorMaterial::FrostedGlass => ([0.88, 0.90, 0.89], 0.55, 0.42),
        }
    }

    /// What the CPU preview shades this with.
    pub fn look(self) -> PartLook {
        let (srgb, roughness, opacity) = self.base();
        PartLook {
            albedo: crate::color::srgb_to_linear3(srgb),
            roughness,
            metallic: 0.0,
            opacity,
            proc: self.grain(),
        }
    }

    /// Register (or find) this material in the project's texture library and return its index.
    ///
    /// Reused BY NAME: building ten doors must not leave ten identical "Door oak" materials in the
    /// library for the user to wade through — and re-picking the same material has to land back on
    /// the one they may already have tuned.
    pub fn install(self, st: &mut FactoryState) -> usize {
        let name = format!("Door — {}", self.label());
        if let Some(i) = st.textures.iter().position(|t| t.name == name) {
            return i;
        }
        let (srgb, roughness, opacity) = self.base();
        let i = match self.grain() {
            Some(def) => st.add_procedural_texture(name, def),
            None => {
                // A 1×1 swatch of the authored sRGB — paint and glass have no pattern to evaluate.
                let px = |c: f32| (c * 255.0).round().clamp(0.0, 255.0) as u8;
                st.add_texture(name, 1, 1, vec![px(srgb[0]), px(srgb[1]), px(srgb[2]), 255])
            }
        };
        if let Some(t) = st.textures.get_mut(i) {
            t.roughness = roughness;
            t.opacity = opacity;
            // Glass is smooth enough to carry a real reflection; timber and paint are not mirrors.
            t.reflect = if self.is_glass() { 0.55 } else { 0.0 };
            t.avg = srgb;
        }
        i
    }
}

/// One material per component, as chosen in the Door panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DoorMaterials {
    pub leaf: DoorMaterial,
    pub panel: DoorMaterial,
    pub frame: DoorMaterial,
    pub architrave: DoorMaterial,
}

impl Default for DoorMaterials {
    /// An oak door in an oak frame — the "Door classic" the rest of the defaults describe.
    fn default() -> Self {
        Self {
            leaf: DoorMaterial::Oak,
            panel: DoorMaterial::Oak,
            frame: DoorMaterial::Oak,
            architrave: DoorMaterial::Oak,
        }
    }
}

impl DoorMaterials {
    pub fn get(&self, slot: Slot) -> DoorMaterial {
        match slot {
            Slot::Leaf => self.leaf,
            Slot::Panel => self.panel,
            Slot::Frame => self.frame,
            Slot::Architrave => self.architrave,
        }
    }

    pub fn set(&mut self, slot: Slot, m: DoorMaterial) {
        match slot {
            Slot::Leaf => self.leaf = m,
            Slot::Panel => self.panel = m,
            Slot::Frame => self.frame = m,
            Slot::Architrave => self.architrave = m,
        }
    }

    /// The material on a given `Part` id, or `None` for hardware (which follows the handle finish)
    /// and for anything unrecognised.
    pub fn for_part(&self, part: u32) -> Option<DoorMaterial> {
        Slot::ALL
            .iter()
            .find(|s| s.parts().contains(&part))
            .map(|s| self.get(*s))
    }

    /// One number that changes whenever any slot does.
    pub fn key(&self) -> u64 {
        Slot::ALL.iter().fold(0u64, |a, s| a * 16 + self.get(*s).key() + 1)
    }

    /// True when any component is glazed — the door then needs the renderer's blended pass.
    pub fn has_glass(&self) -> bool {
        Slot::ALL.iter().any(|s| self.get(*s).is_glass())
    }
}

/// Every `Part` a door emits that is NOT joinery: hinges, its own lever, and every part id a
/// welded library handle contributes (which start above the door's own). Used by both consumers to
/// decide "this is ironmongery, use the finish".
pub const FIRST_HARDWARE_PART: u32 = cad_solid::door::Part::Hinge as u32;

#[cfg(test)]
mod tests {
    use super::*;

    /// The slots must partition the door's JOINERY exactly — every timber part covered once, and
    /// no slot straying onto hardware. A part covered twice would mean two materials fighting over
    /// the same triangles, and one covered zero times would render as untextured base colour with
    /// no way to reach it from the panel.
    #[test]
    fn the_slots_cover_every_joinery_part_exactly_once() {
        use cad_solid::door::Part;
        let mut seen: Vec<u32> = Slot::ALL.iter().flat_map(|s| s.parts().iter().copied()).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "no part belongs to two slots");
        for p in [Part::Leaf, Part::Panel, Part::Lining, Part::Stop, Part::ArchFront, Part::ArchBack] {
            assert!(seen.contains(&(p as u32)), "{p:?} has a material slot");
        }
        for p in [Part::Hinge, Part::Handle] {
            assert!(!seen.contains(&(p as u32)), "{p:?} is ironmongery, not joinery");
            assert!(p as u32 >= FIRST_HARDWARE_PART);
        }
    }

    /// `for_part` and `parts()` must be each other's inverse, or the preview and the build would
    /// look the material up two different ways and could disagree.
    #[test]
    fn a_part_resolves_back_to_the_slot_that_claims_it() {
        let mats = DoorMaterials {
            leaf: DoorMaterial::Walnut,
            panel: DoorMaterial::ClearGlass,
            frame: DoorMaterial::PaintedWhite,
            architrave: DoorMaterial::Ash,
        };
        for slot in Slot::ALL {
            for &p in slot.parts() {
                assert_eq!(mats.for_part(p), Some(mats.get(slot)), "part {p} → {slot:?}");
            }
        }
        assert_eq!(mats.for_part(FIRST_HARDWARE_PART), None, "hardware has no joinery material");
        assert_eq!(mats.for_part(999), None, "an unknown part has none either");
        assert!(mats.has_glass(), "a glazed panel counts as glass");
        assert!(!DoorMaterials::default().has_glass(), "an all-oak door does not");
    }

    /// The preview and the built material must agree about the colour, the roughness and the
    /// transparency of every entry. This is the check that stops the two drifting: they read the
    /// same `base()`, and this proves it end to end rather than by inspection.
    #[test]
    fn the_preview_and_the_installed_material_agree() {
        let mut st = FactoryState::default();
        for m in DoorMaterial::ALL {
            let look = m.look();
            let i = m.install(&mut st);
            let t = &st.textures[i];
            let (srgb, rough, opacity) = m.base();
            assert_eq!(t.avg, srgb, "{m:?}: same base colour");
            assert!((t.roughness - rough).abs() < 1e-6, "{m:?}: same roughness");
            assert!((t.opacity - opacity).abs() < 1e-6, "{m:?}: same opacity");
            assert!((look.opacity - t.opacity).abs() < 1e-6, "{m:?}: the preview is as see-through");
            assert_eq!(look.proc.is_some(), t.proc.is_some(), "{m:?}: both have the grain, or neither");
            assert_eq!(m.is_glass(), t.opacity < 0.999, "{m:?}: glass is the transparent one");
            // The preview shades in LINEAR light; the library stores the authored sRGB.
            assert_eq!(look.albedo, crate::color::srgb_to_linear3(srgb), "{m:?}: decoded once");
        }
    }

    /// Picking the same material twice must not grow the library — someone who builds ten doors
    /// should not find ten copies of "Door — Oak" to scroll past.
    #[test]
    fn installing_a_material_twice_reuses_it() {
        let mut st = FactoryState::default();
        let a = DoorMaterial::Oak.install(&mut st);
        let b = DoorMaterial::Oak.install(&mut st);
        let c = DoorMaterial::Walnut.install(&mut st);
        assert_eq!(a, b, "the same material is the same texture");
        assert_ne!(a, c, "a different one is not");
        assert_eq!(st.textures.len(), 2, "two materials, two entries");
    }
}
