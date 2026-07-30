//! Solar position — the sun's direction for a place, date and time, computed with **Radiance's**
//! algorithm (LBNL-ETA/Radiance `src/gen/sun.c`, the same physics `gensky`/`gendaylit` use, and the
//! model Blender's Sun-Position add-on follows). Getting the sun *located* correctly here is what
//! lets a scene later be re-rendered in Radiance/Blender and line up.
//!
//! Radiance's formulae (all in **radians**; longitude & meridian are **positive WEST** of Greenwich):
//! ```text
//! sdec(jd)  = 0.4093 * sin( (2π/365) * (jd - 81) )                       // solar declination
//! stadj(jd) = 0.170*sin((4π/373)*(jd-80)) - 0.129*sin((2π/355)*(jd-8))
//!             + (12/π)*(s_meridian - s_longitude)                        // solar-time adjustment (h)
//! st        = hour + stadj(jd)                                           // solar time (h)
//! salt      = asin( sin(lat)*sin(sd) - cos(lat)*cos(sd)*cos(st·π/12) )   // altitude
//! sazi      = -atan2( cos(sd)*sin(st·π/12),
//!                     -cos(lat)*sin(sd) - sin(lat)*cos(sd)*cos(st·π/12) )// azimuth
//! sundir    = [ -sin(azi)·cos(alt), -cos(azi)·cos(alt), sin(alt) ]       // X=east, Y=north, Z=up
//! ```
//! We take the friendlier inputs (latitude +N, longitude +E, UTC offset in hours +E) and convert to
//! Radiance's west-positive convention internally.

use glam::Vec3;
use std::f32::consts::PI;

/// A named place → the three numbers the sun model needs: `(name, latitude +N, longitude +E,
/// standard UTC offset in hours +E)`. Radiance's `gensky` has NO built-in city database — it takes
/// raw `-a lat -o lon -m meridian`; location tables live in weather (EPW) files, not the core. So we
/// ship a curated list here and let the picker fill lat/lon/UTC, then the same Radiance math runs.
/// The UTC offset is STANDARD time (no daylight saving) — add an hour in summer if you want DST.
pub const CITIES: &[(&str, f32, f32, f32)] = &[
    ("London, UK", 51.51, -0.13, 0.0),
    ("Paris, France", 48.86, 2.35, 1.0),
    ("Berlin, Germany", 52.52, 13.40, 1.0),
    ("Madrid, Spain", 40.42, -3.70, 1.0),
    ("Rome, Italy", 41.90, 12.50, 1.0),
    ("Reykjavik, Iceland", 64.15, -21.94, 0.0),
    ("Moscow, Russia", 55.75, 37.62, 3.0),
    ("Istanbul, Turkey", 41.01, 28.98, 3.0),
    ("Dubai, UAE", 25.20, 55.27, 4.0),
    ("Abu Dhabi, UAE", 24.45, 54.38, 4.0),
    ("Doha, Qatar", 25.29, 51.53, 3.0),
    ("Riyadh, Saudi Arabia", 24.71, 46.68, 3.0),
    ("Cairo, Egypt", 30.04, 31.24, 2.0),
    ("Karachi, Pakistan", 24.86, 67.01, 5.0),
    ("Mumbai, India", 19.08, 72.88, 5.5),
    ("New Delhi, India", 28.61, 77.21, 5.5),
    ("Singapore", 1.35, 103.82, 8.0),
    ("Hong Kong", 22.32, 114.17, 8.0),
    ("Beijing, China", 39.90, 116.41, 8.0),
    ("Shanghai, China", 31.23, 121.47, 8.0),
    ("Tokyo, Japan", 35.68, 139.69, 9.0),
    ("Sydney, Australia", -33.87, 151.21, 10.0),
    ("Melbourne, Australia", -37.81, 144.96, 10.0),
    ("Auckland, New Zealand", -36.85, 174.76, 12.0),
    ("Nairobi, Kenya", -1.29, 36.82, 3.0),
    ("Lagos, Nigeria", 6.52, 3.38, 1.0),
    ("Johannesburg, South Africa", -26.20, 28.05, 2.0),
    ("New York, USA", 40.71, -74.01, -5.0),
    ("Chicago, USA", 41.88, -87.63, -6.0),
    ("Denver, USA", 39.74, -104.99, -7.0),
    ("Los Angeles, USA", 34.05, -118.24, -8.0),
    ("Toronto, Canada", 43.65, -79.38, -5.0),
    ("Mexico City, Mexico", 19.43, -99.13, -6.0),
    ("São Paulo, Brazil", -23.55, -46.63, -3.0),
    ("Buenos Aires, Argentina", -34.60, -58.38, -3.0),
];

/// The index of the [`CITIES`] entry matching this latitude/longitude (within ~5 km), else `None`
/// (the user has dialled in custom coordinates). Lets the picker show the chosen place or "Custom".
pub fn city_at(lat_deg: f32, lon_deg: f32) -> Option<usize> {
    CITIES
        .iter()
        .position(|c| (c.1 - lat_deg).abs() < 0.05 && (c.2 - lon_deg).abs() < 0.05)
}

/// Cumulative days before the 1st of each month (non-leap; ample for a sun angle).
const CUM_DAYS: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Day-of-year (1..=366) for a calendar month (1..=12) and day (1..=31).
pub fn day_of_year(month: u32, day: u32) -> u32 {
    let m = month.clamp(1, 12) as usize;
    CUM_DAYS[m - 1] + day.clamp(1, 31)
}

/// Radiance `sdec` — solar declination (radians) for day-of-year `jd`.
fn declination(jd: f32) -> f32 {
    0.4093 * ((2.0 * PI / 365.0) * (jd - 81.0)).sin()
}

/// Radiance `stadj` — the solar-time adjustment (hours): equation-of-time + the longitude/meridian
/// correction. `s_long`/`s_merid` are radians, **positive west**.
fn solar_time_adjust(jd: f32, s_long: f32, s_merid: f32) -> f32 {
    0.170 * ((4.0 * PI / 373.0) * (jd - 80.0)).sin()
        - 0.129 * ((2.0 * PI / 355.0) * (jd - 8.0)).sin()
        + (12.0 / PI) * (s_merid - s_long)
}

/// Radiance `salt` — solar altitude (radians) from latitude, declination and solar time (hours).
fn altitude(lat: f32, sd: f32, st: f32) -> f32 {
    (lat.sin() * sd.sin() - lat.cos() * sd.cos() * (st * (PI / 12.0)).cos()).asin()
}

/// Radiance `sazi` — solar azimuth (radians) from latitude, declination and solar time (hours).
fn azimuth(lat: f32, sd: f32, st: f32) -> f32 {
    let ha = st * (PI / 12.0);
    -(sd.cos() * ha.sin()).atan2(-lat.cos() * sd.sin() - lat.sin() * sd.cos() * ha.cos())
}

/// The sun's position and direction for a place/date/time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunPos {
    /// Altitude above the horizon, degrees (negative → the sun is down).
    pub altitude_deg: f32,
    /// Compass bearing of the sun, degrees clockwise from North (0 = N, 90 = E, 180 = S, 270 = W).
    pub bearing_deg: f32,
    /// Unit direction TO the sun in app world axes (X, Y horizontal; Z up), already rotated by the
    /// model's `north_offset` so it lines up with how the building is drawn.
    pub dir: Vec3,
    /// True when the sun is above the horizon.
    pub up: bool,
}

/// Compute the sun's direction. Inputs are the friendly convention: `lat_deg` +north, `lon_deg`
/// +east, `utc_offset_hours` +east (e.g. British Summer Time = +1, PST = −8). `north_offset_deg`
/// rotates the result about Z so world +Y need not be true north (0 = +Y is north).
pub fn sun_position(lat_deg: f32, lon_deg: f32, utc_offset_hours: f32, doy: u32, hour: f32, north_offset_deg: f32) -> SunPos {
    let lat = lat_deg.to_radians();
    // Radiance is west-positive: flip the +east inputs.
    let s_long = (-lon_deg).to_radians();
    let s_merid = (-15.0 * utc_offset_hours).to_radians();
    let jd = doy.max(1) as f32;

    let sd = declination(jd);
    let st = hour + solar_time_adjust(jd, s_long, s_merid);
    let alt = altitude(lat, sd, st);
    let azi = azimuth(lat, sd, st);

    // Radiance sun direction: X east, Y north, Z up.
    let (ca, sa) = (alt.cos(), alt.sin());
    let east = -azi.sin() * ca;
    let north = -azi.cos() * ca;
    let up = sa;

    // Compass bearing (clockwise from north) BEFORE the model rotation — for the readout.
    let bearing = east.atan2(north).to_degrees().rem_euclid(360.0);

    // Rotate about Z so the model's north (which may not be world +Y) lines up.
    let no = north_offset_deg.to_radians();
    let (c, s) = (no.cos(), no.sin());
    let dir = Vec3::new(east * c - north * s, east * s + north * c, up);

    SunPos { altitude_deg: alt.to_degrees(), bearing_deg: bearing, dir, up: alt > 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The city table round-trips: each entry's coordinates resolve back to that same entry, and a
    /// point far from any city (mid-Atlantic) is "Custom" (None).
    #[test]
    fn city_lookup_round_trips() {
        for (i, c) in CITIES.iter().enumerate() {
            assert_eq!(city_at(c.1, c.2), Some(i), "{} resolves to itself", c.0);
        }
        assert_eq!(city_at(0.0, -30.0), None, "open ocean is Custom");
    }

    /// Declination pins to the physical extremes: ~0 at the equinox (doy 81), +23.4° at the June
    /// solstice (doy ≈ 172), −23.4° at the December solstice (doy ≈ 355).
    #[test]
    fn declination_matches_the_seasons() {
        assert!(declination(81.0).abs() < 0.02, "≈0 at equinox");
        assert!((declination(172.0).to_degrees() - 23.4).abs() < 1.0, "+23.4° June");
        assert!((declination(355.0).to_degrees() + 23.4).abs() < 1.0, "−23.4° December");
    }

    /// Radiance `salt` at the equinox, solar noon (st = 12), latitude φ gives altitude 90°−φ — the
    /// textbook result. Checks the altitude formula directly.
    #[test]
    fn equinox_noon_altitude_is_90_minus_latitude() {
        let sd = declination(81.0);
        let alt = altitude(40.0f32.to_radians(), sd, 12.0).to_degrees();
        assert!((alt - 50.0).abs() < 0.3, "alt {alt} ≈ 50° at 40°N equinox noon");
        let alt0 = altitude(0.0, sd, 12.0).to_degrees();
        assert!((alt0 - 90.0).abs() < 0.3, "overhead at the equator");
    }

    /// June-solstice noon at 40°N reaches 90 − (40 − 23.4) = 73.4°, roughly due south.
    #[test]
    fn summer_noon_is_high_and_south() {
        // Choose an hour near local solar noon for a place ON its standard meridian.
        let p = sun_position(40.0, -75.0, -5.0, day_of_year(6, 21), 12.0, 0.0);
        assert!(p.up && p.altitude_deg > 68.0 && p.altitude_deg < 75.0, "high summer sun {}", p.altitude_deg);
        // Near south: bearing within ±20° of 180.
        assert!((p.bearing_deg - 180.0).abs() < 25.0, "≈ south, bearing {}", p.bearing_deg);
        // Direction points up and is a unit vector.
        assert!(p.dir.z > 0.9, "sun is high, dir.z {}", p.dir.z);
        assert!((p.dir.length() - 1.0).abs() < 1e-3, "unit vector");
    }

    /// Sunrise/sunset: the sun is DOWN at midnight and UP at midday, and its altitude is negative
    /// before dawn.
    #[test]
    fn day_and_night() {
        let doy = day_of_year(3, 21);
        let midnight = sun_position(51.5, -0.13, 0.0, doy, 0.0, 0.0);
        let noon = sun_position(51.5, -0.13, 0.0, doy, 12.0, 0.0);
        assert!(!midnight.up && midnight.altitude_deg < 0.0, "down at midnight");
        assert!(noon.up && noon.altitude_deg > 0.0, "up at noon");
    }

    /// The north-offset rotates the horizontal bearing of the direction vector by that many degrees
    /// (leaving the altitude — the Z component — untouched).
    #[test]
    fn north_offset_rotates_the_direction() {
        let a = sun_position(45.0, 0.0, 0.0, day_of_year(9, 21), 9.0, 0.0);
        let b = sun_position(45.0, 0.0, 0.0, day_of_year(9, 21), 9.0, 90.0);
        assert!((a.dir.z - b.dir.z).abs() < 1e-4, "altitude unchanged by north offset");
        // Horizontal component rotates 90°: a.(x,y) ≈ b rotated back.
        let ah = glam::Vec2::new(a.dir.x, a.dir.y);
        let bh = glam::Vec2::new(b.dir.x, b.dir.y);
        assert!((ah.length() - bh.length()).abs() < 1e-4, "same horizontal magnitude");
        let dot = ah.normalize().dot(bh.normalize()).clamp(-1.0, 1.0);
        assert!((dot.acos().to_degrees() - 90.0).abs() < 1.0, "rotated 90°");
    }
}
