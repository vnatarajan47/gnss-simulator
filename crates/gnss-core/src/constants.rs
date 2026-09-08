//! Physical and reference-frame constants.
//!
//! Values are taken from the relevant interface control documents rather than
//! from a generic physics table: GNSS broadcast ephemerides are only self
//! consistent when propagated with the *same* constants the control segment
//! used to fit them.

use crate::{Constellation, TimeSystem};

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
    ///
    /// `None` for SBAS, which broadcasts an ECEF state vector rather than
    /// orbital elements and so has no Keplerian model to evaluate.
    pub const fn mu(self) -> Option<f64> {
        match self {
            Constellation::Gps | Constellation::Qzss => Some(3.986_005e14),
            Constellation::Galileo => Some(3.986_004_418e14),
            Constellation::BeiDou => Some(3.986_004_418e14),
            Constellation::Sbas => None,
        }
    }

    /// Earth rotation rate `OMEGA_e_dot` \[rad/s\].
    ///
    /// Defined for every constellation: even SBAS needs it, for the Earth
    /// rotation over signal transit time.
    pub const fn earth_rotation_rate(self) -> f64 {
        match self {
            Constellation::Gps
            | Constellation::Qzss
            | Constellation::Galileo
            | Constellation::Sbas => 7.292_115_146_7e-5,
            Constellation::BeiDou => 7.292_115e-5,
        }
    }

    /// Relativistic clock-correction constant `F = -2*sqrt(mu)/c^2` \[s/sqrt(m)\].
    ///
    /// Unused for look angles, but part of the ephemeris contract and needed
    /// once pseudoranges are generated (phase 4). `None` for SBAS, whose clock
    /// correction is a plain polynomial with no eccentricity term.
    pub const fn relativistic_f(self) -> Option<f64> {
        match self {
            Constellation::Gps | Constellation::Qzss => Some(-4.442_807_633e-10),
            Constellation::Galileo | Constellation::BeiDou => Some(-4.442_807_309e-10),
            Constellation::Sbas => None,
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
            Constellation::Gps
            | Constellation::Qzss
            | Constellation::Galileo
            | Constellation::Sbas => 0.0,
            Constellation::BeiDou => 14.0,
        }
    }

    /// The time scale this constellation's ranging signals are expressed in.
    ///
    /// Coarser than the constellation itself, because several systems are
    /// deliberately steered to GPS time. Consumed by [`crate::dop`], which
    /// needs one clock unknown per distinct scale — see [`TimeSystem`] for why
    /// each constellation lands where it does.
    pub const fn time_system(self) -> TimeSystem {
        match self {
            Constellation::Gps | Constellation::Qzss | Constellation::Sbas => TimeSystem::Gps,
            Constellation::Galileo => TimeSystem::Galileo,
            Constellation::BeiDou => TimeSystem::BeiDou,
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
            // SBAS state vectors are published every ~4 minutes and are only
            // meant for short extrapolation. 15 minutes tolerates a few missed
            // messages; a geostationary satellite barely moves in ECEF over
            // that span, so the cost of a slightly stale record is small.
            Constellation::Sbas => 900.0,
        }
    }
}

/// What counts as a geostationary orbit.
///
/// Every constellation here has geostationary members -- SBAS entirely,
/// BeiDou's C01-C05, QZSS's J07 and J08 -- and they matter twice over: BeiDou's
/// need a different transformation into Earth-fixed coordinates, and all of
/// them are worth drawing differently, since they sit still where everything
/// else sweeps past.
///
/// The test is on the broadcast elements, not on a PRN table, for the same
/// reason [`crate::sbas::decode_scale`] decides units physically: PRN-to-orbit
/// assignments change as satellites are launched and retired, and a stale table
/// is a silent error where a physical test is not.
pub mod geostationary {
    /// Nominal geostationary orbit radius \[m\].
    pub const RADIUS_M: f64 = 42_164_000.0;

    /// How far from that radius a semi-major axis may sit \[m\].
    ///
    /// Wide enough for the drift operators allow, and still nowhere near the
    /// 27 900 km of a BeiDou MEO or the 42 164 km-with-55-deg-inclination of an
    /// IGSO, which the inclination test excludes anyway.
    pub const RADIUS_TOLERANCE_M: f64 = 1_000_000.0;

    /// Inclination below which an orbit at that radius counts as
    /// geostationary \[rad\].
    ///
    /// BeiDou's GEOs broadcast a few degrees rather than zero, because their
    /// elements are referred to a plane tilted 5 deg out of the equator. Their
    /// IGSO and MEO satellites sit near 55 deg, so the two populations are an
    /// order of magnitude apart and any threshold between them gives the same
    /// answer.
    pub const MAX_INCLINATION_RAD: f64 = 10.0 * std::f64::consts::PI / 180.0;
}

/// BeiDou's geostationary transformation (BDS-SIS-ICD-B1I §5.2.4.12).
///
/// BeiDou's GEO satellites are propagated by the same Keplerian algorithm as
/// everything else and then rotated into the Earth-fixed frame differently:
/// through `Rz(omega_e * t_k) * Rx(-5 deg)` instead of the direct rotation.
/// Skipping it puts a BeiDou GEO thousands of kilometres from where it is.
pub mod beidou_geo {
    /// Fixed tilt applied to a BeiDou GEO position \[rad\].
    ///
    /// The ICD writes it as `Rx(-5 deg)`, where `Rx` rotates the *frame*.
    /// Rotating the vector instead flips the sign, and rotating the vector is
    /// what [`crate::geodesy::Ecef::rotate_x`] does — so the value is stored
    /// in the vector-rotation sense and the call site carries no sign to get
    /// wrong.
    pub const TILT_RAD: f64 = 5.0 * std::f64::consts::PI / 180.0;
}
