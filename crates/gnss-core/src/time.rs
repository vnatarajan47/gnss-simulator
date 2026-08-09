//! GPS time handling.
//!
//! Everything inside this crate is expressed as [`GpsTime`]: continuous
//! seconds since the GPS epoch (1980-01-06T00:00:00 UTC), with no leap
//! seconds. Conversion to and from Unix time is the only place leap seconds
//! appear, which keeps the propagation math free of calendar concerns and
//! makes week rollover a non-issue.

use crate::constants::SECONDS_PER_WEEK;

/// Unix timestamp of the GPS epoch, 1980-01-06T00:00:00Z.
const GPS_EPOCH_UNIX: f64 = 315_964_800.0;

/// (Unix timestamp at which the leap took effect, cumulative GPS-UTC offset).
///
/// GPS-UTC was 0 s at the GPS epoch. The most recent entry is 2017-01-01;
/// no leap second has been announced since, so 18 s is current as of writing.
/// Appending a row here is the only change needed if IERS declares another.
const LEAP_SECONDS: &[(f64, f64)] = &[
    (362_793_600.0, 1.0),    // 1981-07-01
    (394_329_600.0, 2.0),    // 1982-07-01
    (425_865_600.0, 3.0),    // 1983-07-01
    (489_024_000.0, 4.0),    // 1985-07-01
    (567_993_600.0, 5.0),    // 1988-01-01
    (631_152_000.0, 6.0),    // 1990-01-01
    (662_688_000.0, 7.0),    // 1991-01-01
    (709_948_800.0, 8.0),    // 1992-07-01
    (741_484_800.0, 9.0),    // 1993-07-01
    (773_020_800.0, 10.0),   // 1994-07-01
    (820_454_400.0, 11.0),   // 1996-01-01
    (867_715_200.0, 12.0),   // 1997-07-01
    (915_148_800.0, 13.0),   // 1999-01-01
    (1_136_073_600.0, 14.0), // 2006-01-01
    (1_230_768_000.0, 15.0), // 2009-01-01
    (1_341_100_800.0, 16.0), // 2012-07-01
    (1_435_708_800.0, 17.0), // 2015-07-01
    (1_483_228_800.0, 18.0), // 2017-01-01
];

/// Cumulative GPS-UTC offset in effect at a given Unix timestamp \[s\].
fn gps_utc_offset(unix_seconds: f64) -> f64 {
    let mut offset = 0.0;
    for &(effective_from, value) in LEAP_SECONDS {
        if unix_seconds >= effective_from {
            offset = value;
        } else {
            break;
        }
    }
    offset
}

/// An instant on the GPS timescale, as continuous seconds since 1980-01-06.
///
/// Continuous representation means `a - b` is always the true elapsed
/// interval; there is no week-rollover correction anywhere in this crate.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct GpsTime(f64);

impl GpsTime {
    /// Build from raw seconds since the GPS epoch.
    pub const fn from_seconds(seconds: f64) -> Self {
        Self(seconds)
    }

    /// Seconds since the GPS epoch.
    pub const fn seconds(self) -> f64 {
        self.0
    }

    /// Build from a continuous (rollover-free) GPS week number and
    /// seconds-of-week, as carried in RINEX 3 navigation records.
    pub fn from_week_and_sow(week: u32, seconds_of_week: f64) -> Self {
        Self(f64::from(week) * SECONDS_PER_WEEK + seconds_of_week)
    }

    /// Seconds elapsed within the current GPS week.
    ///
    /// Required by the longitude-of-ascending-node term of the broadcast
    /// ephemeris algorithm, which is defined against seconds-of-week rather
    /// than against continuous time.
    pub fn seconds_of_week(self) -> f64 {
        self.0.rem_euclid(SECONDS_PER_WEEK)
    }

    /// Continuous GPS week number.
    pub fn week(self) -> u32 {
        (self.0 / SECONDS_PER_WEEK).floor() as u32
    }

    /// Convert from a Unix timestamp (seconds since 1970-01-01 UTC).
    pub fn from_unix_seconds(unix_seconds: f64) -> Self {
        Self(unix_seconds - GPS_EPOCH_UNIX + gps_utc_offset(unix_seconds))
    }

    /// Convert to a Unix timestamp (seconds since 1970-01-01 UTC).
    pub fn to_unix_seconds(self) -> f64 {
        // The offset depends on the instant we are solving for, so approximate
        // once and refine. One pass is always enough away from a leap instant.
        let approx_unix = self.0 + GPS_EPOCH_UNIX;
        let offset = gps_utc_offset(approx_unix);
        approx_unix - offset
    }

    /// Signed interval `self - other` in seconds.
    pub fn seconds_since(self, other: GpsTime) -> f64 {
        self.0 - other.0
    }

    /// Shift by a number of seconds.
    pub fn offset_by(self, seconds: f64) -> Self {
        Self(self.0 + seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_epoch_maps_to_zero() {
        assert_eq!(GpsTime::from_unix_seconds(GPS_EPOCH_UNIX).seconds(), 0.0);
    }

    #[test]
    fn modern_epoch_carries_18_leap_seconds() {
        // 2025-01-01T00:00:00Z == GPS week 2347, sow 259218.0
        // (18 s of accumulated leap seconds put GPS ahead of UTC).
        let t = GpsTime::from_unix_seconds(1_735_689_600.0);
        assert_eq!(t.week(), 2347);
        assert!((t.seconds_of_week() - 259_218.0).abs() < 1e-6);
    }

    #[test]
    fn week_and_sow_round_trip() {
        let t = GpsTime::from_week_and_sow(2347, 259_200.0);
        assert_eq!(t.week(), 2347);
        assert!((t.seconds_of_week() - 259_200.0).abs() < 1e-9);
    }

    #[test]
    fn unix_round_trip_is_stable() {
        let unix = 1_735_732_800.0; // 2025-01-01T12:00:00Z
        let t = GpsTime::from_unix_seconds(unix);
        assert!((t.to_unix_seconds() - unix).abs() < 1e-6);
    }

    #[test]
    fn differences_cross_week_boundaries_without_wrapping() {
        let before = GpsTime::from_week_and_sow(2347, 604_000.0);
        let after = GpsTime::from_week_and_sow(2348, 800.0);
        assert!((after.seconds_since(before) - 1600.0).abs() < 1e-9);
    }
}
