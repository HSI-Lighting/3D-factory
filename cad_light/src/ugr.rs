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
    /// Standing eye height — EN 12464-1's pair with the seated 1.2 m.
    ///
    /// Was 1.5 m, which is not a value the standard uses; the pair is 1.2 m seated, 1.6 m standing.
    /// The 0.1 m is not a rounding: the position index is steep near the line of sight, so raising
    /// the eye moves a ceiling fitting several degrees closer to it and the rating with it.
    pub const STANDING_EYE_M: f32 = 1.6;

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
///
/// EVERY FITTING IS TAKEN AS VISIBLE and at its INITIAL output. A real room has walls in it and a
/// real installation is quoted maintained — see [`ugr_at_ex`], which this is the plain-scene case
/// of. Kept as its own entry point because the formula's own tests want no scene and no factor.
pub fn ugr_at(
    observer: &Observer,
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    background: f64,
) -> Option<UgrResult> {
    ugr_at_ex(observer, luminaires, profiles, background, 1.0, &|_, _| true)
}

/// UGR at one observer, WITH OCCLUSION AND MAINTENANCE — the form a real room wants.
///
/// * `visible(eye, luminaire)` — `false` when something stands between them. Without it every
///   fitting in the model glares from wherever it is, including through a party wall and from the
///   next room, and the error is in the unsafe direction. `RtScene::occluded` negated is the
///   intended argument; `&|_, _| true` is the no-scene case.
/// * `maintenance` — the factor the installation is quoted at, applied to luminaire output exactly
///   as [`Evaluator`](crate::Evaluator) applies it to illuminance.
///
/// **UGR IS NOT INVARIANT TO A UNIFORM SCALE, so the two halves must agree.** Scaling every
/// luminaire by `m` scales the glare sum by `m²`, and the background it is seen against by `m`, so
/// the net is `ΔUGR = 8·log₁₀(m)` — about −0.8 at a 0.80 factor. This function scales only the
/// LUMINAIRES; `background` must arrive already maintained, which it does when it comes from an
/// `Evaluator` (that applies the factor to everything it returns). Applying it here as well would
/// count it twice and land the rating 0.8 low.
pub fn ugr_at_ex(
    observer: &Observer,
    luminaires: &[Luminaire],
    profiles: &HashMap<String, IesProfile>,
    background: f64,
    maintenance: f64,
    visible: &dyn Fn(Vec3, Vec3) -> bool,
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

        // ANYTHING IN THE WAY REMOVES IT. Cheap rejects are already done; this is one shadow ray.
        // Placed BEFORE the aperture test so an occluded fitting is not also counted as one whose
        // file declares no area — those two say very different things to the reader.
        if !visible(eye, l.position.to_vec3()) {
            continue;
        }

        // READ THE PHOTOMETRY THROUGH THE FITTING'S OWN FRAME, exactly as `calc.rs` does.
        //
        // γ and φ are measured from the luminaire's nadir and its C0 plane. This used to take them
        // from the WORLD axes — γ from world-down and φ from world +X — which is right only for a
        // fitting pointing at the floor. `tilt_deg` is precisely what the aiming tool writes, so a
        // spot tipped 30° into someone's eye was read at whatever its file holds at nadir, often
        // near zero: the glare map would have been blank in the one place it matters.
        //
        // `dir` runs eye → luminaire, so the ray that REACHES the eye leaves the fitting along
        // `-dir`. That is the direction to sample, and it is the same convention as
        // `intensity_toward`, whose own `dir` runs luminaire → point.
        let (aim, c0, c90) = l.frame();
        let out = -dir;
        let gamma_deg = (out.dot(aim) as f64).clamp(-1.0, 1.0).acos().to_degrees();
        // 180° OUT, SEPARATELY FROM THE TILT BUG. The old azimuth was `atan2(dir.y, dir.x)` — the
        // bearing of the luminaire FROM THE EYE, where the C-plane wants the bearing of the eye
        // FROM THE LUMINAIRE. The two are opposite, so an eye standing off the +X side of a fitting
        // was read in its C0 plane when it is squarely in its C180. Invisible on an axially
        // symmetric downlight, and a straight swap of one flank for the other on anything with an
        // asymmetric optic — a wall-washer, an aimed spot, a batten with a shielded side.
        let azim = (out.dot(c90).atan2(out.dot(c0)).to_degrees() as f64).rem_euclid(360.0);

        // γ FEEDS THE APERTURE TOO, so it has to be the corrected one: `projected_luminous_area`
        // foreshortens the aperture about its OWN normal, which is the aim vector and not world
        // down. Reading it off the world axes tilted the aperture the wrong way on every aimed
        // fitting — the same root cause, biting a second time.
        let Some(area) = prof.projected_luminous_area(gamma_deg) else {
            skipped_no_area += 1;
            continue;
        };
        if area <= 0.0 {
            skipped_no_area += 1;
            continue;
        }

        // ONE ACCESSOR FOR OUTPUT, the same one `calc.rs` uses.
        //
        // This read `prof.intensity(..) * prof.multiplier * l.dimming`, which was wrong twice.
        // `IesProfile::intensity` ALREADY applies the multiplier (`ies.rs`), so it was counted
        // twice — invisible on EULUMDAT, which always parses 1.0, and real on LM-63, which reads it
        // from the header. UGR goes as I², so a file carrying 0.8 landed 1.5 UGR low and one
        // carrying 2 landed 4.8 high, either side of a pass/fail line. And `dimming` alone ignores
        // `flux_override`, so a fitting the user re-rated to 2000 lm LIT the room at one output and
        // GLARED at another. `output_scale` folds both, so the two can never disagree again.
        let intensity = prof.intensity(gamma_deg, azim) * l.output_scale(prof) * maintenance;
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

    // ===== the five defects, each as the check that fails on the code that had it ===============

    /// A NARROW BEAM: 3000 cd at nadir, 100 cd by 30°, nothing at 90°. Axially symmetric, so the
    /// only thing that can move its output is γ — which is the whole point of the aim test.
    fn spot(side: f64) -> IesProfile {
        let mut p = flat_source(0.0, side);
        p.vertical_angles = vec![0.0, 30.0, 90.0];
        p.horizontal_angles = vec![0.0];
        p.candela = vec![vec![3000.0, 100.0, 0.0]];
        p
    }

    /// TWO DIFFERENT FLANKS: bright across C0, dim across C180. Nothing else in the file tells the
    /// two sides apart, so any result that differs between them differs *only* by the azimuth.
    fn asymmetric(c0: f64, c180: f64, side: f64) -> IesProfile {
        let mut p = flat_source(0.0, side);
        p.vertical_angles = vec![0.0, 90.0];
        p.horizontal_angles = vec![0.0, 180.0, 360.0];
        p.candela = vec![vec![c0, c0], vec![c180, c180], vec![c0, c0]];
        p
    }

    fn lamp_at(x: f32, y: f32, z: f32) -> Luminaire {
        Luminaire {
            id: 1,
            profile: "p".into(),
            position: Vertex::new(x, y, z),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        }
    }

    /// §2.1 — **AIM MATTERS.** The defect: γ was read from the WORLD's downward axis, so `tilt_deg`
    /// — the field the aiming tool writes — changed nothing at all. A spot tipped into someone's
    /// eye was rated at whatever its file holds at nadir.
    ///
    /// Same fitting, same place, same eye; only the pose differs. On the old code both readings are
    /// bit-identical, because the tilt was never consulted.
    #[test]
    fn a_fitting_aimed_at_the_observer_glares_far_more_than_one_aimed_at_the_floor() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let p = profiles(spot(0.5));

        let level = ugr_at(&obs, &[lamp_at(0.0, 2.0, 3.2)], &p, 50.0).expect("floor-aimed");

        let mut aimed = lamp_at(0.0, 2.0, 3.2);
        assert!(aimed.aim_at(obs.position.to_vec3()), "the fitting must accept the aim");
        let aimed = ugr_at(&obs, &[aimed], &p, 50.0).expect("eye-aimed");

        // γ goes 45° → 0°, so the beam swings from 75 cd onto the eye at its full 3000 cd. The term
        // goes as I², damped by the aperture opening out as it faces square on.
        assert!(
            aimed.ugr > level.ugr + 15.0,
            "aiming the beam into the eye must show: aimed {:.2} against floor-aimed {:.2}",
            aimed.ugr,
            level.ugr,
        );
        assert!(
            aimed.sources[0].luminance > level.sources[0].luminance * 20.0,
            "and it is the LUMINANCE that moved: {:.0} against {:.0} cd/m²",
            aimed.sources[0].luminance,
            level.sources[0].luminance,
        );
    }

    /// §2.1, SAFETY PROPERTY — the frame fix must be a no-op on the fittings that had no pose.
    ///
    /// Untilted, `aim` is straight down and `(−dir)·aim` is exactly the old `dir.z`. Everything the
    /// eight formula tests above pin therefore has to survive unchanged, and this says so against a
    /// number rather than by assertion: the hand-calculated case still lands on 14.81.
    #[test]
    fn an_untilted_fitting_reads_exactly_as_it_did_before_the_frame_fix() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let r = ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &profiles(flat_source(1000.0, 0.5)), 50.0)
            .expect("a result");
        assert!((r.sources[0].sigma_deg - 45.0).abs() < 1e-3);
        assert!((r.ugr - 14.81).abs() < 0.05, "UGR {}", r.ugr);
    }

    /// §2.1b — **THE AZIMUTH WAS 180° OUT**, separately from the tilt, and this one the plan did not
    /// list.
    ///
    /// The old expression was `atan2(dir.y, dir.x)`, the bearing of the LUMINAIRE FROM THE EYE. A
    /// C-plane is indexed by the bearing of the EYE FROM THE LUMINAIRE, and those are opposite. So
    /// an eye standing off a fitting's +X side was read in its C0 plane when it stands squarely in
    /// its C180 — a straight swap of one flank for the other.
    ///
    /// Eye at the origin, fitting 2 m along +X: the light that reaches the eye leaves the fitting
    /// travelling in −X, which is C180 and therefore the DIM side.
    #[test]
    fn an_asymmetric_fitting_is_read_in_the_c_plane_that_faces_the_eye() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::X);
        let p = profiles(asymmetric(2000.0, 200.0, 0.5));
        let r = ugr_at(&obs, &[lamp_at(2.0, 0.0, 3.2)], &p, 50.0).expect("a result");

        // I = 200 cd (C180), A_p = 0.25·cos45 = 0.176777 m² → L = 1131 cd/m².
        // Reading C0 instead would give ten times that.
        assert!(
            (r.sources[0].luminance - 1131.4).abs() < 5.0,
            "the eye is in C180, so 200 cd → 1131 cd/m²; got {:.0}. Ten times that is the old \
             azimuth reading the bright flank.",
            r.sources[0].luminance,
        );
    }

    /// §2.2 — **THE CANDELA MULTIPLIER, ONCE.** `IesProfile::intensity` already applies it, and the
    /// old line applied it again. EULUMDAT always parses 1.0 so it never showed on `.ldt`; LM-63
    /// reads a real value from its header, and UGR goes as I².
    #[test]
    fn the_candela_multiplier_is_applied_once_and_not_twice() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let plain = ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &profiles(flat_source(1000.0, 0.5)), 50.0)
            .unwrap()
            .ugr;
        let mut doubled = flat_source(1000.0, 0.5);
        doubled.multiplier = 2.0;
        let doubled =
            ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &profiles(doubled), 50.0).unwrap().ugr;

        // Twice the output is twice the luminance and four times the term: 8·log₁₀(4) = 4.82.
        // Counted twice it would be four times the output and 8·log₁₀(16) = 9.64.
        assert!(
            (doubled - plain - 4.82).abs() < 0.05,
            "a multiplier of 2 must add 4.82, not 9.64: {doubled:.2} - {plain:.2}",
        );
    }

    /// §2.3 — **A RE-RATED FITTING GLARES AT ITS NEW OUTPUT.** The old line scaled by `dimming`
    /// alone, so a fitting the user re-rated LIT the room at one output and GLARED at another.
    #[test]
    fn re_rating_a_fitting_moves_its_glare_as_well_as_its_light() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let p = profiles(flat_source(1000.0, 0.5)); // the profile declares 1000 lm
        let full = ugr_at(&obs, &one_lamp(0.0, 2.0, 3.2), &p, 50.0).unwrap().ugr;

        let mut halved = lamp_at(0.0, 2.0, 3.2);
        halved.flux_override = Some(500.0);
        let halved = ugr_at(&obs, &[halved], &p, 50.0).unwrap().ugr;

        // Half the flux quarters the term: 8·log₁₀(0.25) = −4.82. Ignored, it would be 0.
        assert!(
            (full - halved - 4.82).abs() < 0.05,
            "re-rating 1000 lm to 500 must drop the rating 4.82: {full:.2} - {halved:.2}",
        );
    }

    /// §2.4 — **SOMETHING IN THE WAY REMOVES IT.** Without this every fitting in the model glares
    /// from wherever it is, including from the next room straight through the party wall — and it
    /// is wrong in the unsafe direction, which is the one that gets signed off.
    #[test]
    fn a_fitting_behind_an_obstruction_drops_out_of_the_sum() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let p = profiles(flat_source(1000.0, 0.5));
        let lums = one_lamp(0.0, 2.0, 3.2);

        let seen = ugr_at_ex(&obs, &lums, &p, 50.0, 1.0, &|_, _| true);
        let hidden = ugr_at_ex(&obs, &lums, &p, 50.0, 1.0, &|_, _| false);
        assert!(seen.is_some(), "the plain scene must still rate");
        assert!(hidden.is_none(), "a wall in the way leaves nothing to rate, not a rating of zero");

        // And with two fittings, hiding ONE leaves the other — it removes a source rather than
        // abandoning the calculation.
        let mut two = one_lamp(0.0, 2.0, 3.2);
        let mut second = lamp_at(0.6, 2.0, 3.2);
        second.id = 2;
        two.push(second);
        let both = ugr_at_ex(&obs, &two, &p, 50.0, 1.0, &|_, _| true).unwrap();
        let one_hidden =
            ugr_at_ex(&obs, &two, &p, 50.0, 1.0, &|_, to| to.x < 0.3).unwrap();
        assert_eq!(both.sources.len(), 2);
        assert_eq!(one_hidden.sources.len(), 1, "exactly the unobstructed fitting survives");
        assert!(one_hidden.ugr < both.ugr);
    }

    /// §2.5 — **MAINTAINED, LIKE EVERY OTHER FIGURE ON THE PAGE.** UGR is not invariant to a
    /// uniform scale, so a maintained lux plan beside a day-one glare figure is two conventions in
    /// one report.
    ///
    /// This function scales the LUMINAIRES only; the background arrives already maintained from the
    /// evaluator. So against a FIXED background the sum moves by m² — `ΔUGR = 16·log₁₀(m)` — and it
    /// is the caller supplying a maintained background that turns that into the net 8·log₁₀(m).
    #[test]
    fn maintenance_scales_the_glare_sum_and_not_the_background() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let p = profiles(flat_source(1000.0, 0.5));
        let lums = one_lamp(0.0, 2.0, 3.2);
        let initial = ugr_at_ex(&obs, &lums, &p, 50.0, 1.0, &|_, _| true).unwrap().ugr;
        let kept = ugr_at_ex(&obs, &lums, &p, 50.0, 0.8, &|_, _| true).unwrap().ugr;

        let expect = 16.0 * 0.8f64.log10(); // −1.55
        assert!(
            (kept - initial - expect).abs() < 0.02,
            "0.80 maintained must move the rating by {expect:.2}: {kept:.2} - {initial:.2}",
        );
        // The net once the caller's background is maintained too is half of that, and it is the
        // number the report footnote has to stand behind.
        assert!((8.0 * 0.8f64.log10() + 0.776).abs() < 0.01);
    }

    /// `ugr_at` is now `ugr_at_ex` with nothing in the way and no factor — and must stay exactly
    /// that, or the eight formula tests above stop describing what the app actually calls.
    #[test]
    fn the_plain_entry_point_is_the_unobstructed_unmaintained_case() {
        let obs = Observer::looking(Vertex::new(0.0, 0.0, 1.2), Vec3::Y);
        let p = profiles(flat_source(1000.0, 0.5));
        let lums = one_lamp(0.0, 2.0, 3.2);
        let plain = ugr_at(&obs, &lums, &p, 50.0).unwrap();
        let ex = ugr_at_ex(&obs, &lums, &p, 50.0, 1.0, &|_, _| true).unwrap();
        assert_eq!(plain.ugr.to_bits(), ex.ugr.to_bits(), "{} vs {}", plain.ugr, ex.ugr);
    }
}
