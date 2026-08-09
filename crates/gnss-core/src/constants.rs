//! Physical and reference-frame constants.
//!
//! Values are taken from the relevant interface control documents rather than
//! from a generic physics table: GNSS broadcast ephemerides are only self
//! consistent when propagated with the *same* constants the control segment
//! used to fit them.

use crate::Constellation;

/// WGS-84 reference ellipsoid (NIMA TR8350.2).
pub mod wgs84 {
    /// Semi-major axis \[m\].
    pub const SEMI_MAJOR_AXIS: f64 = 6_378_137.0;
    /// Flattening (dimensionless).
    pub const FLATTENING: f64 = 1.0 / 298.257_223_563;
    /// Semi-minor axis \[m\].
    pub const SEMI_MINOR_AXIS: f64 = SEMI_MAJOR_AXIS * (1.0 - FLATTENING);
    /// First eccentricity squared.
    pub const ECC_SQ: f64 = FLATTENING * (2.0 - FLATTENING);
    /// Second eccentricity squared.
    pub const ECC_SQ_PRIME: f64 = (SEMI_MAJOR_AXIS * SEMI_MAJOR_AXIS
        - SEMI_MINOR_AXIS * SEMI_MINOR_AXIS)
        / (SEMI_MINOR_AXIS * SEMI_MINOR_AXIS);
}

/// Speed of light in vacuum \[m/s\] (IS-GPS-200, §20.3.4.3).
pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;

/// Seconds in one GNSS week.
pub const SECONDS_PER_WEEK: f64 = 604_800.0;

impl Constellation {
    /// WGS-84 / GTRF / CGCS2000 gravitational constant `mu` \[m^3/s^2\].
    ///
    /// GPS uses the older WGS-84 value (IS-GPS-200 Table 20-IV); Galileo and
    /// BeiDou use the newer one. Using the wrong one shifts the computed
    /// mean motion and is worth several metres of along-track error.
    pub const fn mu(self) -> f64 {
        match self {
            Constellation::Gps | Constellation::Qzss => 3.986_005e14,
            Constellation::Galileo => 3.986_004_418e14,
            Constellation::BeiDou => 3.986_004_418e14,
        }
    }

    /// Earth rotation rate `OMEGA_e_dot` \[rad/s\].
    pub const fn earth_rotation_rate(self) -> f64 {
        match self {
            Constellation::Gps | Constellation::Qzss | Constellation::Galileo => 7.292_115_146_7e-5,
            Constellation::BeiDou => 7.292_115e-5,
        }
    }

    /// Relativistic clock-correction constant `F = -2*sqrt(mu)/c^2` \[s/sqrt(m)\].
    ///
    /// Unused for look angles, but part of the ephemeris contract and needed
    /// once pseudoranges are generated (phase 4).
    pub const fn relativistic_f(self) -> f64 {
        match self {
            Constellation::Gps | Constellation::Qzss => -4.442_807_633e-10,
            Constellation::Galileo | Constellation::BeiDou => -4.442_807_309e-10,
        }
    }

    /// Offset added to this constellation's system time to obtain GPS time \[s\].
    ///
    /// BeiDou time is behind GPS time by a fixed 14 s (no leap seconds in
    /// either). Galileo System Time is steered to within nanoseconds of GPS
    /// time and RINEX reports Galileo week numbers on the continuous GPS
    /// week count, so the offset is zero for our purposes.
    pub const fn seconds_to_gpst(self) -> f64 {
        match self {
            Constellation::Gps | Constellation::Qzss | Constellation::Galileo => 0.0,
            Constellation::BeiDou => 14.0,
        }
    }

    /// Default curve-fit half-interval \[s\]: how far either side of ToE a
    /// broadcast ephemeris is considered usable.
    ///
    /// GPS LNAV fits a 4-hour arc centred on ToE (IS-GPS-200 §20.3.4.4), so
    /// the half-interval is 2 h. Galileo I/NAV and BeiDou D1 both nominally
    /// publish on a 1-hour cadence with shorter validity, but 2 h remains a
    /// safe upper bound for selection purposes.
    pub const fn fit_half_interval(self) -> f64 {
        match self {
            Constellation::Gps | Constellation::Qzss => 7200.0,
            Constellation::Galileo | Constellation::BeiDou => 7200.0,
        }
    }
}
