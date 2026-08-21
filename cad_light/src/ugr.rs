//! Unified Glare Rating (CIE 117 / EN 12464-1).
//!
//! Illuminance says whether there is enough light. UGR says whether the light hurts to look at, and
//! it is the metric a design is rejected on after it has passed on lux. All three DIALux reports in
//! `tests/Identical testing` quote R_UG,max against a ≤ 19 target; we could not produce the figure
//! at all until now.
//!
//! ```text
//!   UGR = 8 · log₁₀ [ (0.25 / L_b) · Σᵢ (Lᵢ² ωᵢ / pᵢ²) ]
//! ```
//!
//! with, per luminaire seen from the observer:
//!
//!   * `L` — luminance in the observer's direction, `I / A_projected` (cd/m²)
//!   * `ω` — solid angle it subtends, `A_projected / d²` (sr)
//!   * `p` — Guth position index: how much a source annoys from where it actually is, rather than
//!     from straight ahead
//!   * `L_b` — background luminance, the field the source is seen against
//!
//! Note `L²ω = I² / (A_p · d²)`, so the AREA IS THE WHOLE GAME: halve the aperture and the glare
//! term doubles. That is why the EULUMDAT reader now takes the luminous area rather than the
//! housing — see `IesProfile::luminous_length`.
//!
//! **This is the direct calculation, not the table method.** DIALux's quoted figure comes from the
//! CIE UGR *table* — a standard room at a stated spacing-to-height ratio, which is why its reports
//! say "based on a rectangular space of 4.000 m × 4.000 m and SHR of 0.25". The two answer
//! different questions: the table characterises the FITTING, this characterises the INSTALLATION as
//! built, from a given seat looking a given way. They agree only when the real room happens to be
//! the standard one.

use std::collections::HashMap;

use glam::Vec3;

use crate::ies::IesProfile;
use crate::types::{Luminaire, Vertex};

/// One observer: where the eye is, and which way it faces.
#[derive(Debug, Clone, Copy)]
pub struct Observer {
    pub position: Vertex,
    /// View direction; need not be normalised. The vertical component is kept — a person looking
    /// slightly down at a desk sees a different glare field from one looking level.
    pub direction: Vec3,
}

impl Observer {
    /// Seated eye height, the EN 12464-1 default for an office.
    pub const SEATED_EYE_M: f32 = 1.2;
    /// Standing eye height.
    pub const STANDING_EYE_M: f32 = 1.5;

    pub fn looking(position: Vertex, direction: Vec3) -> Self {
        Self { position, direction }
    }
}

/// What one luminaire contributed, kept so a result can be explained rather than just quoted.
#[derive(Debug, Clone, Copy)]
pub struct GlareSource {
    pub id: u32,
    /// Angle from the view direction (degrees).
    pub sigma_deg: f64,
    /// Luminance in the observer's direction (cd/m²).
    pub luminance: f64,
    /// Solid angle subtended (sr).
    pub omega: f64,
    /// Guth position index.
    pub position_index: f64,
    /// This source's term, `L²ω/p²`.
    pub term: f64,
}

/// The result, with the working shown.
#[derive(Debug, Clone)]
pub struct UgrResult {
    pub ugr: f64,
    /// Background luminance used (cd/m²).
    pub background: f64,
    /// Sources in the field of view, brightest contribution first.
    pub sources: Vec<GlareSource>,
    /// Luminaires skipped because their file declares no luminous area — they contribute NOTHING
    /// to the figure, and a UGR computed from half the fittings is worse than none.
    pub skipped_no_area: usize,
}

/// GUTH POSITION INDEX, by Levin's closed form.
///
/// A source directly in the line of sight is at its most annoying (p = 1); the same source off to
/// one side or high above is progressively less so, and p rises to damp its contribution.
///
///   * `sigma_deg` — angle between the view direction and the direction to the source
///   * `alpha_deg` — roll angle about the view axis, measured from straight UP: 0° is directly
///     above the line of sight, 90° directly beside it
///
/// Clamped at both ends. Behind the observer there is no glare, and the fit is only defined
/// forward; a runaway p there would silently discard a source that should have been excluded
/// outright.
pub fn position_index(sigma_deg: f64, alpha_deg: f64) -> f64 {
    let sigma = sigma_deg.clamp(0.0, 90.0);
    let alpha = alpha_deg.clamp(0.0, 90.0);
    let a = (35.2 - 0.31889 * alpha - 1.22 * (-2.0 * alpha / 9.0).exp()) * 1.0e-3;
    let b = (21.0 + 0.26667 * alpha - 0.002963 * alpha * alpha) * 1.0e-5;
    (a * sigma + b * sigma * sigma).exp().clamp(1.0, 100.0)
}

/// UGR at one observer, from `luminaires`, against a background luminance `background` (cd/m²).
///
/// `background` is normally `E_indirect / π` — the indirect illuminance on a vertical plane at the
/// eye, facing the view direction. It is passed in rather than computed here so this stays a pure
/// function of the glare geometry, and so a caller measuring the background properly and a caller
/// estimating it both go through the same formula.
///
/// Returns `None` when there is nothing to rate: no source in the field of view, or no background
/// to see it against. Zero would be a lie — zero is an excellent UGR.
pub fn ugr_at(
    observer: &Observer,
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    background: f64,
) -> Option<UgrResult> {
    if !(background.is_finite() && background > 0.0) {
        return None;
    }
    let eye = observer.position.to_vec3();
    let view = observer.direction.normalize_or_zero();
    if view.length_squared() < 0.5 {
        return None;
    }
    // "Up" for the roll angle. A view direction that IS vertical has no meaningful roll, and the
    // cross product would vanish, so fall back to +X — any consistent reference will do there.
    let world_up = Vec3::Z;
    let right = view.cross(world_up).normalize_or_zero();
    let (right, up) = if right.length_squared() < 1e-6 {
        (Vec3::X, Vec3::X.cross(view).normalize_or_zero())
    } else {
        (right, right.cross(view).normalize_or_zero())
    };

    let mut sources = Vec::new();
    let mut sum = 0.0;
    let mut skipped_no_area = 0usize;

    for l in luminaires {
        let Some(prof) = profiles.get(&l.profile) else { continue };
        let to = l.position.to_vec3() - eye;
        let d2 = to.length_squared();
        if d2 < 1e-6 {
            continue; // the observer is inside the fitting; not a glare question
        }
        let dir = to / d2.sqrt();
        let cos_sigma = dir.dot(view) as f64;
        if cos_sigma <= 0.0 {
            continue; // behind the observer
        }
        let sigma_deg = cos_sigma.clamp(-1.0, 1.0).acos().to_degrees();

        // The luminaire's own emission angle toward the eye: gamma from ITS nadir (−Z).
        //
        // `dir` runs eye → luminaire, so the ray reaching the eye runs −`dir`, and
        // cos γ = (−dir)·(0,0,−1) = dir.z. Negating it here instead put a fitting directly
        // OVERHEAD at γ = 180° — pointing at the ceiling — which read back zero intensity and zero
        // projected area, so every realistic observer got no rating at all.
        let gamma_deg = (dir.z as f64).clamp(-1.0, 1.0).acos().to_degrees();
        let Some(area) = prof.projected_luminous_area(gamma_deg) else {
            skipped_no_area += 1;
            continue;
        };
        if area <= 0.0 {
            skipped_no_area += 1;
            continue;
        }

        // Azimuth about the luminaire's own axis, so a non-symmetric distribution is sampled the
        // way it is aimed.
        let azim = {
            let a = (dir.y as f64).atan2(dir.x as f64).to_degrees() - l.rotation_deg as f64;
            a.rem_euclid(360.0)
        };
        let intensity = prof.intensity(gamma_deg, azim) * prof.multiplier * l.dimming as f64;
        if intensity <= 0.0 {
            continue;
        }

        // Roll about the view axis, measured from straight up.
        let (u, r) = (dir.dot(up) as f64, dir.dot(right) as f64);
        let alpha_deg = r.abs().atan2(u.max(0.0)).to_degrees();
        let p = position_index(sigma_deg, alpha_deg);

        let omega = area / d2 as f64;
        let luminance = intensity / area;
        let term = luminance * luminance * omega / (p * p);
        sum += term;
        sources.push(GlareSource {
            id: l.id,
            sigma_deg,
            luminance,
            omega,
            position_index: p,
            term,
        });
    }

    if sum <= 0.0 {
        return None;
    }
    sources.sort_by(|a, b| b.term.partial_cmp(&a.term).unwrap_or(std::cmp::Ordering::Equal));
    let ugr = 8.0 * (0.25 / background * sum).log10();
    Some(UgrResult { ugr, background, sources, skipped_no_area })
}

/// Background luminance from an indirect vertical illuminance: `L_b = E_ind / π`.
///
/// Lambertian, so this is exact for a diffuse field rather than a rule of thumb.
pub fn background_from_indirect(e_indirect_lx: f64) -> f64 {
    e_indirect_lx / std::f64::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ies::PhotometryType;

    /// A 1 m² flat emitter with a uniform 1000 cd — simple enough that every quantity in the
    /// formula can be checked by hand.
    fn flat_source(cd: f64, side: f64) -> IesProfile {
        IesProfile {
            manufacturer: String::new(),
            catalogue: String::new(),
            lamp: String::new(),
            name: "test".into(),
            photometry: PhotometryType::C,
            lumens: 1000.0,
            multiplier: 1.0,
            vertical_angles: vec![0.0, 90.0],
            horizontal_angles: vec![0.0, 360.0],
            candela: vec![vec![cd, cd], vec![cd, cd]],
            watts: 10.0,
            width: side,
            length: side,
            height: 0.0,
            luminous_length: side,
            luminous_width: side,
        }
    }

    fn one_lamp(x: f32, y: f32, z: f32) -> Vec<Luminaire> {
        vec![Luminaire {
            id: 1,
            profile: "p".into(),
            position: Vertex::new(x, y, z),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }]
    }

    fn profiles(p: IesProfile) -> HashMap<String, IesProfile> {
        let mut m = HashMap::new();
        m.insert("p".to_string(), p);
        m
    }

    /// The position index is 1 straight ahead and rises as the source moves off axis — that is the
    /// whole content of it, and it damps the contribution as it rises.
    #[test]
    fn position_index_is_one_on_axis_and_grows_off_it() {
        assert!((position_index(0.0, 0.0) - 1.0).abs() < 1e-9);
        let mut last = 1.0;
        for s in [5.0, 10.0, 20.0, 40.0, 60.0, 80.0] {
            let p = position_index(s, 0.0);
            assert!(p > last, "p should rise with sigma: {s}° gave {p}, previous {last}");
            last = p;
        }
        // A source the same angle OVERHEAD is less annoying than one out to the side, so its
        // position index is higher — a fitting above your head bothers you less than the same
        // fitting at eye level beside you. (p divides, so higher p = less glare.)
        assert!(
            position_index(30.0, 0.0) > position_index(30.0, 90.0),
            "overhead {:.2} should damp more than beside {:.2}",
            position_index(30.0, 0.0),
            position_index(30.0, 90.0),
        );
    }

    /// Nonsense in, a usable number out — a position index of zero or NaN would divide the whole
    /// sum by zero.
    #[test]
    fn position_index_survives_extremes() {
        for (s, a) in [(-10.0, 0.0), (200.0, 0.0), (30.0, -50.0), (30.0, 400.0), (0.0, 0.0)] {
            let p = position_index(s, a);
            assert!(p.is_finite() && p >= 1.0, "sigma {s}, alpha {a} gave {p}");
        }
    }

    /// THE FORMULA, checked by hand.
    ///
    /// Observer at (0, 0, 1.2) looking +Y; a 0.5 m square source at (0, 2, 3.2) — 2 m ahead and
    /// 2 m above the eye. So d² = 8, and the source is at 45° both from the line of sight and from
    /// its own nadir:
    ///
    ///   A_p = 0.25 · cos45 = 0.176777 m²      ω = A_p / 8 = 0.0220971 sr
    ///   L   = 1000 / 0.176777 = 5656.85 cd/m²
    ///   α   = 0° (directly above the line of sight), σ = 45°  →  p = 7.0596
    ///   term = L²ω / p² = 14 188
    ///   UGR  = 8·log₁₀(0.25/50 · 14 188) = 14.81
    #[test]
    fn the_formula_matches_a_hand_calculation() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let lums = one_lamp(0.0, 2.0, 3.2);
        let r = ugr_at(&obs, &lums, &profiles(flat_source(1000.0, 0.5)), 50.0).expect("a result");
        assert_eq!(r.sources.len(), 1);
        let s = r.sources[0];
        assert!((s.sigma_deg - 45.0).abs() < 1e-3, "sigma {}", s.sigma_deg);
        assert!((s.omega - 0.0220971).abs() < 1e-6, "omega {}", s.omega);
        assert!((s.luminance - 5656.85).abs() < 1.0, "luminance {}", s.luminance);
        assert!((s.position_index - 7.0596).abs() < 1e-3, "p {}", s.position_index);
        assert!((r.ugr - 14.81).abs() < 0.05, "UGR {}", r.ugr);
    }

    /// A source BEHIND the observer contributes nothing. Turning round is the oldest glare remedy
    /// there is, and a formula that ignored the view direction would miss it.
    #[test]
    fn a_source_behind_the_observer_does_not_glare() {
        let p = profiles(flat_source(1000.0, 0.5));
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        assert!(ugr_at(&obs, &one_lamp(0.0, -2.0, 3.2), &p, 50.0).is_none());
        // …and the same fitting in front does.
        assert!(ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &p, 50.0).is_some());
    }

    /// A BRIGHTER BACKGROUND lowers UGR: the same lamp against a bright ceiling is less
    /// objectionable than against a dark one. This is the term people are surprised by, so it is
    /// worth pinning.
    #[test]
    fn a_brighter_background_lowers_the_rating() {
        let p = profiles(flat_source(1000.0, 0.5));
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let dark = ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &p, 20.0).unwrap().ugr;
        let bright = ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &p, 200.0).unwrap().ugr;
        assert!(bright < dark, "bright {bright:.1} should rate below dark {dark:.1}");
        // A tenfold background is exactly 8 dB of log — 8·log₁₀(10) = 8.
        assert!((dark - bright - 8.0).abs() < 0.01, "{dark:.2} - {bright:.2}");
    }

    /// A SMALLER APERTURE at the same output glares MORE — L² ω = I²/(A·d²), so halving the area
    /// doubles the term. This is why reading the housing instead of the aperture was a real bug and
    /// not a cosmetic one.
    #[test]
    fn a_smaller_aperture_at_the_same_output_glares_more() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let big = ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &profiles(flat_source(1000.0, 0.6)), 50.0)
            .unwrap()
            .ugr;
        let small =
            ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &profiles(flat_source(1000.0, 0.3)), 50.0)
                .unwrap()
                .ugr;
        assert!(small > big, "small aperture {small:.1} should out-glare big {big:.1}");
        // L²ω = I²/(A_p·d²), so the term goes as 1/A. Halving the SIDE quarters the area and
        // quadruples the term: 8·log₁₀(4) = 4.82.
        assert!((small - big - 4.82).abs() < 0.05, "{small:.2} - {big:.2}");
    }

    /// MORE FITTINGS, more glare — and each adds in the sum rather than replacing it.
    #[test]
    fn glare_accumulates_over_fittings() {
        let p = profiles(flat_source(1000.0, 0.5));
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let mk = |n: u32| -> Vec<Luminaire> {
            (0..n)
                .map(|i| Luminaire {
                    id: i + 1,
                    profile: "p".into(),
                    position: Vertex::new(i as f32 * 0.01, 2.0, 3.2),
                    rotation_deg: 0.0,
                    tilt_deg: 0.0,
                    dimming: 1.0,
                    watts_override: None,
                    flux_override: None,
            from_block: None,
                })
                .collect()
        };
        let one = ugr_at(&obs, &mk(1), &p, 50.0).unwrap().ugr;
        let four = ugr_at(&obs, &mk(4), &p, 50.0).unwrap().ugr;
        assert!(four > one);
        // Four identical sources quadruple the sum: 8·log₁₀(4) = 4.82.
        assert!((four - one - 4.82).abs() < 0.05, "{four:.2} - {one:.2}");
    }

    /// A fitting whose file declares NO luminous area is counted as skipped, not silently dropped.
    /// A UGR built from half the fittings looks like a good result.
    #[test]
    fn a_fitting_with_no_declared_area_is_reported_not_ignored() {
        let mut p = flat_source(1000.0, 0.5);
        p.luminous_length = 0.0;
        p.luminous_width = 0.0;
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        assert!(ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &profiles(p), 50.0).is_none());
    }

    /// Nothing to rate is not a rating of zero — zero is an excellent UGR.
    #[test]
    fn an_empty_room_has_no_rating_rather_than_a_good_one() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let p = profiles(flat_source(1000.0, 0.5));
        assert!(ugr_at(&obs, &[], &p, 50.0).is_none());
        // …and neither is a black room.
        assert!(ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &p, 0.0).is_none());
    }

    #[test]
    fn background_luminance_is_the_lambertian_conversion() {
        assert!((background_from_indirect(std::f64::consts::PI) - 1.0).abs() < 1e-12);
    }
}
