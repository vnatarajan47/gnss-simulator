//! Ionospheric and tropospheric propagation delay.
//!
//! Both models here are the standard single-frequency broadcast-grade ones:
//! Klobuchar for the ionosphere (IS-GPS-200 §20.3.3.5.2.5) and Saastamoinen
//! for the troposphere (Saastamoinen 1972, with Davis's gravity refinement).
//! Neither is accurate — Klobuchar is specified only to remove about half the
//! RMS ionospheric error, and Saastamoinen driven by a standard atmosphere
//! rather than real met data is good to a few centimetres in the dry component
//! and far worse in the wet one.
//!
//! They are the right models anyway, because they are the models a
//! single-frequency receiver *itself* applies from the broadcast message. A
//! simulator whose delays came from a more accurate source than the receiver
//! can access would produce a signal no real receiver could correct, which is
//! the opposite of useful.

use crate::constants::SPEED_OF_LIGHT;
use crate::geodesy::Geodetic;
use crate::time::GpsTime;

/// Klobuchar coefficients, as broadcast in the RINEX Nav header (`GPSA`/`GPSB`
/// in RINEX 3, `ION ALPHA`/`ION BETA` in RINEX 2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KlobucharModel {
    /// Amplitude coefficients \[s, s/semicircle, s/semicircle^2, s/semicircle^3\].
    pub alpha: [f64; 4],
    /// Period coefficients \[s, s/semicircle, s/semicircle^2, s/semicircle^3\].
    pub beta: [f64; 4],
}

impl KlobucharModel {
    /// A representative coefficient set, for files whose header carries none.
    ///
    /// The BKG merged product is inconsistent about this: the 2026 files carry
    /// `GPSA`/`GPSB`, the 2019 ones carry no ionospheric block at all, and the
    /// GPS-only subsets carry none either. So a consumer that wants an
    /// ionosphere has to supply one for part of the archive.
    ///
    /// These are mid-latitude, moderate-solar-activity values. They are *not*
    /// what the constellation was broadcasting on any particular day, and
    /// anything using them must say so in its output metadata rather than let
    /// them pass for broadcast coefficients -- the whole point of a fallback
    /// being named is that it can be distinguished afterwards.
    pub const FALLBACK: Self = Self {
        alpha: [1.0245e-8, 2.2352e-8, -5.9605e-8, -1.1921e-7],
        beta: [1.2698e5, 1.1469e5, -6.5536e4, -5.8982e5],
    };

    /// Vertical delay at the ionospheric pierce point, plus the obliquity
    /// factor, giving slant delay along the line of sight \[m\] at the GPS L1
    /// frequency.
    ///
    /// `azimuth_deg` is clockwise from true north; `elevation_deg` is above
    /// the local horizon. Both refer to the line of sight from `user` to the
    /// satellite.
    ///
    /// The result is defined for L1 only; scale by `(f_L1 / f)^2` for another
    /// band, since ionospheric delay is inversely proportional to the square
    /// of the carrier frequency.
    pub fn slant_delay_l1_m(
        &self,
        user: Geodetic,
        azimuth_deg: f64,
        elevation_deg: f64,
        t: GpsTime,
    ) -> f64 {
        // The algorithm is specified in semicircles (half-turns) rather than
        // radians or degrees. Getting this wrong is the classic Klobuchar bug:
        // it scales every polynomial term by pi and still returns a
        // plausible-looking few metres.
        let elevation_sc = elevation_deg / 180.0;
        let azimuth_rad = azimuth_deg.to_radians();
        let user_lat_sc = user.latitude_deg / 180.0;
        let user_lon_sc = user.longitude_deg / 180.0;

        // Earth-centred angle between user and pierce point.
        let earth_centred_angle = 0.0137 / (elevation_sc + 0.11) - 0.022;

        // Pierce-point geodetic latitude, clamped to the model's validity band.
        let pierce_lat_sc =
            (user_lat_sc + earth_centred_angle * azimuth_rad.cos()).clamp(-0.416, 0.416);
        let pierce_lon_sc = user_lon_sc
            + earth_centred_angle * azimuth_rad.sin() / (pierce_lat_sc * std::f64::consts::PI).cos();

        // Geomagnetic latitude of the pierce point.
        let geomagnetic_lat_sc = pierce_lat_sc
            + 0.064 * ((pierce_lon_sc - 1.617) * std::f64::consts::PI).cos();

        // Local time at the pierce point, seconds into the day.
        let local_time = (43_200.0 * pierce_lon_sc + t.seconds_of_week()).rem_euclid(86_400.0);

        // Obliquity: the ratio of slant path length to vertical through the
        // thin shell.
        let obliquity = 1.0 + 16.0 * (0.53 - elevation_sc).powi(3);

        let horner = |c: &[f64; 4]| c[0] + geomagnetic_lat_sc * (c[1] + geomagnetic_lat_sc * (c[2] + geomagnetic_lat_sc * c[3]));

        // Both clamps are from the ICD: a negative amplitude or a period under
        // 20 hours is unphysical and means the polynomial has been evaluated
        // outside the fit.
        let amplitude = horner(&self.alpha).max(0.0);
        let period = horner(&self.beta).max(72_000.0);

        // Phase of the diurnal cosine, referenced to 14:00 local time.
        let phase = std::f64::consts::TAU * (local_time - 50_400.0) / period;

        // Night-time floor of 5 ns vertical, with the cosine approximated by
        // its first three Taylor terms inside the daytime half-cycle -- the
        // ICD specifies the truncated series, not the cosine, so a "better"
        // cos() here would disagree with every receiver in the field.
        let vertical_delay_s = if phase.abs() < 1.57 {
            5e-9 + amplitude * (1.0 - phase.powi(2) / 2.0 + phase.powi(4) / 24.0)
        } else {
            5e-9
        };

        SPEED_OF_LIGHT * obliquity * vertical_delay_s
    }
}

/// Tropospheric slant delay \[m\] by the Saastamoinen model, driven by a
/// standard atmosphere.
///
/// `elevation_deg` is above the local horizon. Below about 5 degrees the
/// `1/cos(z)` mapping degrades badly, so the elevation is floored — a
/// simulated satellite at 1 degree should produce a large delay, not an
/// unbounded one.
///
/// The `1 - 0.00266 cos(2 lat) - 0.00028 h` divisor is Davis's correction for
/// the variation of gravity with latitude and height; without it the dry
/// delay is wrong by a couple of centimetres at high latitude.
pub fn saastamoinen_delay_m(user: Geodetic, elevation_deg: f64) -> f64 {
    /// Relative humidity assumed by the standard atmosphere.
    const RELATIVE_HUMIDITY: f64 = 0.7;
    /// Below this the mapping function is no longer trustworthy \[deg\].
    const MIN_ELEVATION_DEG: f64 = 3.0;

    let height_m = user.altitude_m.max(0.0);
    let elevation_rad = elevation_deg.max(MIN_ELEVATION_DEG).to_radians();
    let zenith_angle = std::f64::consts::FRAC_PI_2 - elevation_rad;

    // 1976 US Standard Atmosphere at the receiver, referenced to 1013.25 hPa
    // and 15 C at sea level.
    let pressure_hpa = 1013.25 * (1.0 - 2.2557e-5 * height_m).powf(5.2568);
    let temperature_k = 15.0 - 6.5e-3 * height_m + 273.16;
    let water_vapour_hpa = 6.108
        * RELATIVE_HUMIDITY
        * ((17.15 * temperature_k - 4684.0) / (temperature_k - 38.45)).exp();

    let gravity_correction =
        1.0 - 0.00266 * (2.0 * user.latitude_deg.to_radians()).cos() - 0.00028 * height_m / 1000.0;

    let dry = 0.0022768 * pressure_hpa / gravity_correction / zenith_angle.cos();
    let wet = 0.002277 * (1255.0 / temperature_k + 0.05) * water_vapour_hpa / zenith_angle.cos();

    dry + wet
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative broadcast set, from the BKG BRDC product for
    /// 2025-01-01.
    fn model() -> KlobucharModel {
        KlobucharModel {
            alpha: [1.0245e-8, 2.2352e-8, -1.1921e-7, -1.1921e-7],
            beta: [1.2698e5, 1.6384e5, -6.5536e4, -5.2429e5],
        }
    }

    /// Physical envelope: zenith ionospheric delay at L1 runs from a few
    /// metres at night to tens of metres at midday solar maximum, and the
    /// obliquity factor at the horizon is about 3.
    #[test]
    fn ionospheric_delay_stays_in_its_physical_envelope() {
        let user = Geodetic::new(39.74, -104.99, 1609.0);
        let base = GpsTime::from_week_and_sow(2347, 259_200.0);

        for hour in 0..24 {
            let t = base.offset_by(f64::from(hour) * 3600.0);
            for elevation in [5.0, 30.0, 60.0, 90.0] {
                let delay = model().slant_delay_l1_m(user, 45.0, elevation, t);
                assert!(
                    (0.5..100.0).contains(&delay),
                    "{elevation} deg at hour {hour}: {delay:.2} m is outside the physical range"
                );
            }
        }
    }

    /// Exact anchor on the obliquity factor and the semicircle handling.
    ///
    /// With the amplitude coefficients zeroed the model collapses to the
    /// night-time floor, `delay = c * F * 5 ns`, where `F` is the closed form
    /// `1 + 16 (0.53 - E)^3` with `E` in semicircles. Everything that would
    /// otherwise confound an elevation sweep -- the pierce point moving, the
    /// geomagnetic latitude changing, the diurnal phase -- drops out, so this
    /// tests the obliquity and the degree-to-semicircle conversion on their
    /// own. A radian/semicircle mix-up leaves the magnitude plausible but
    /// misses these values by orders of magnitude.
    #[test]
    fn obliquity_matches_its_closed_form_at_the_night_time_floor() {
        let floor_only = KlobucharModel {
            alpha: [0.0; 4],
            beta: model().beta,
        };
        let user = Geodetic::new(39.74, -104.99, 1609.0);
        let t = GpsTime::from_week_and_sow(2347, 259_200.0);

        for elevation_deg in [5.0f64, 15.0, 30.0, 45.0, 60.0, 90.0] {
            let elevation_sc = elevation_deg / 180.0;
            let expected = SPEED_OF_LIGHT * (1.0 + 16.0 * (0.53 - elevation_sc).powi(3)) * 5e-9;
            let actual = floor_only.slant_delay_l1_m(user, 0.0, elevation_deg, t);
            assert!(
                (actual - expected).abs() < 1e-9,
                "{elevation_deg} deg: {actual:.9} m vs closed form {expected:.9} m"
            );
        }

        // And the floor is monotone in elevation, since F is.
        let horizon = floor_only.slant_delay_l1_m(user, 0.0, 5.0, t);
        let zenith = floor_only.slant_delay_l1_m(user, 0.0, 90.0, t);
        assert!((horizon / zenith - 3.02).abs() < 0.05);
    }

    /// The diurnal cosine must peak at 14:00 *local* time at the pierce
    /// point, not at 14:00 UTC. That distinction is the whole reason the
    /// longitude term enters the time calculation, and getting it wrong
    /// shifts the ionosphere around the globe by up to twelve hours.
    #[test]
    fn ionospheric_delay_peaks_at_two_pm_local_time() {
        // Zenith, so the pierce point sits over the user and local time is
        // unambiguous.
        let user = Geodetic::new(39.74, -104.99, 1609.0);
        let longitude_sc = user.longitude_deg / 180.0;

        let mut peak_delay = f64::MIN;
        let mut peak_local_time = 0.0;

        for step in 0..(24 * 12) {
            let seconds_of_week = f64::from(step) * 300.0;
            let t = GpsTime::from_week_and_sow(2347, seconds_of_week);
            let delay = model().slant_delay_l1_m(user, 0.0, 90.0, t);
            if delay > peak_delay {
                peak_delay = delay;
                peak_local_time = (43_200.0 * longitude_sc + seconds_of_week).rem_euclid(86_400.0);
            }
        }

        // 14:00 = 50 400 s, within one sampling step either side.
        assert!(
            (peak_local_time - 50_400.0).abs() < 400.0,
            "diurnal peak at {:.0} s local time, expected 50400 s (14:00)",
            peak_local_time
        );
    }

    /// Zenith tropospheric delay at sea level is close to 2.3 m for the dry
    /// component plus a small wet component. This is one of the most
    /// well-established numbers in GNSS.
    #[test]
    fn zenith_tropospheric_delay_is_about_two_and_a_half_metres() {
        let sea_level = Geodetic::new(0.0, 0.0, 0.0);
        let delay = saastamoinen_delay_m(sea_level, 90.0);
        assert!(
            (2.3..2.7).contains(&delay),
            "zenith troposphere {delay:.3} m is not the expected ~2.5 m"
        );
    }

    /// Delay must fall with altitude (less atmosphere overhead) and grow
    /// towards the horizon (longer path).
    #[test]
    fn tropospheric_delay_falls_with_height_and_grows_with_zenith_angle() {
        let sea_level = saastamoinen_delay_m(Geodetic::new(39.74, -104.99, 0.0), 90.0);
        let denver = saastamoinen_delay_m(Geodetic::new(39.74, -104.99, 1609.0), 90.0);
        let everest = saastamoinen_delay_m(Geodetic::new(39.74, -104.99, 8848.0), 90.0);
        assert!(everest < denver && denver < sea_level);

        let mut previous = 0.0;
        for elevation in [90.0, 60.0, 30.0, 15.0, 5.0] {
            let delay = saastamoinen_delay_m(Geodetic::new(39.74, -104.99, 1609.0), elevation);
            assert!(
                delay > previous,
                "delay {delay:.3} m at {elevation} deg did not grow past {previous:.3} m"
            );
            previous = delay;
        }
    }

    /// The low-elevation floor must bound the delay rather than let 1/cos(z)
    /// run away. At 0 degrees the unclamped model would return ~44 m.
    #[test]
    fn tropospheric_delay_is_bounded_at_and_below_the_horizon() {
        let user = Geodetic::new(39.74, -104.99, 1609.0);
        for elevation in [3.0, 1.0, 0.0, -5.0] {
            let delay = saastamoinen_delay_m(user, elevation);
            assert!(
                (10.0..50.0).contains(&delay),
                "{elevation} deg gave {delay:.1} m; the elevation floor is not holding"
            );
        }
    }
}
