//! # gnss-core
//!
//! Broadcast-ephemeris GNSS geometry: parse RINEX navigation files, propagate
//! satellites to ECEF, and compute look angles from a ground station.
//!
//! The crate is deliberately `no_std`-friendly in spirit and free of I/O: it
//! takes bytes in and returns numbers out, so the same code runs in a native
//! test harness and in a browser via `wasm32-unknown-unknown`.
//!
//! ## Typical use
//!
//! ```no_run
//! use gnss_core::{skyplot, Geodetic, SkyplotOptions, GpsTime};
//!
//! let bytes = std::fs::read("data/BRDC00WRD_R_20250010000_01D_GN.rnx").unwrap();
//! let observer = Geodetic::new(39.7392, -104.9903, 1609.0);
//! let t = GpsTime::from_unix_seconds(1_735_732_800.0);
//!
//! let view = skyplot(&bytes, observer, t, &SkyplotOptions::default()).unwrap();
//! for sat in &view.satellites {
//!     println!("{} az={:.1} el={:.1}", sat.sv, sat.azimuth_deg, sat.elevation_deg);
//! }
//! ```
//!
//! ## Constellation support
//!
//! GPS, Galileo, BeiDou and QZSS broadcast the same Keplerian parameter set
//! and share [`propagate`], differing only in constants and — for BeiDou's
//! geostationary satellites — one extra frame rotation. SBAS broadcasts an
//! ECEF state vector instead and takes the [`sbas`] path. Which of them a
//! given call includes is [`SkyplotOptions::constellations`].
//!
//! Mixing them is not free: satellites from different constellations do not
//! share a clock, so [`dop`] carries one clock unknown per [`TimeSystem`] and
//! needs one more satellite for each. GLONASS remains out of scope — it
//! broadcasts a state vector requiring numerical integration of the equations
//! of motion, which is a third propagator rather than a variation on either
//! of these.

pub mod constants;
pub mod dop;
pub mod ephemeris;
pub mod geodesy;
pub mod propagate;
pub mod sbas;
pub mod series;
pub mod skyplot;
pub mod source;
pub mod time;

pub use dop::{dop_for, dop_from_angles, dop_from_observations, min_satellites, Dop};
pub use ephemeris::{
    BroadcastEphemeris, EphemerisSet, KeplerianEphemeris, SelectionConfig, SelectionStrategy, Sv,
};
pub use geodesy::{
    ecef_to_enu, ecef_to_geodetic, geodetic_to_ecef, look_angles, AzEl, Ecef, Enu, Geodetic,
};
pub use propagate::PropagationConfig;
pub use sbas::{SbasEphemeris, SbasProvider};
pub use series::{skyplot_series, SatelliteTrack, SkySeries, TrackSample, MAX_EPOCHS};
pub use skyplot::{skyplot, skyplot_from_set, SatelliteView, SkyView, SkyplotOptions};
pub use source::parse_nav;
pub use time::GpsTime;

/// GNSS constellations this crate can propagate.
///
/// The first four broadcast Keplerian elements. SBAS is the exception: its
/// geostationary satellites broadcast an ECEF state vector, propagated by
/// Taylor expansion rather than an orbit solve (see [`sbas`]).
///
/// GLONASS is still absent. It also broadcasts a state vector, but one that
/// requires numerical integration of the equations of motion including J2 --
/// a different propagator again, not a variation on either of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Constellation {
    Gps,
    Galileo,
    BeiDou,
    Qzss,
    /// Satellite-based augmentation: WAAS, EGNOS, MSAS, and friends.
    Sbas,
}

/// The time reference a constellation's ranging signals are expressed in.
///
/// A receiver solving with satellites from two of these cannot assume their
/// clocks agree: it has to estimate the offset between them as an extra
/// unknown. That is why this is a *coarser* grouping than [`Constellation`] —
/// what matters is which satellites share a clock, not who operates them.
///
/// - QZSS is steered to GPS time and specified for GPS-interoperable use.
/// - SBAS ranging signals are GPS-time coherent by design; that is the whole
///   point of an augmentation system.
/// - Galileo System Time is held close to GPS time, but the offset is a
///   broadcast quantity (the GGTO) rather than zero, so a receiver estimates
///   it. Grouping Galileo with GPS would understate the unknowns.
/// - BeiDou time is a separate scale entirely, 14 s from GPS time.
///
/// The ordering is the column order in the DOP design matrix, and the lowest
/// system present is the reference clock — see [`dop::Dop::tdop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimeSystem {
    /// GPS, and everything steered to it: QZSS and SBAS.
    Gps,
    Galileo,
    BeiDou,
}

impl TimeSystem {
    /// How many distinct time systems exist, i.e. the widest set of clock
    /// unknowns a solution can carry.
    pub const COUNT: usize = 3;
}

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse RINEX data: {0}")]
    Rinex(String),

    #[error("input is not a RINEX navigation file")]
    NotNavigationRinex,

    #[error("no usable ephemeris found for any satellite at the requested time")]
    NoEphemerisInRange,

    #[error("ephemeris is unusable: {reason}")]
    InvalidEphemeris { reason: &'static str },

    #[error("Kepler's equation did not converge in {iterations} iterations")]
    KeplerDidNotConverge { iterations: usize },

    #[error("failed to decompress gzip input: {0}")]
    Decompression(String),

    #[error("the RINEX parser panicked; the file is malformed or uses an unsupported layout")]
    ParserPanicked,

    #[error("invalid time interval: {reason}")]
    InvalidInterval { reason: &'static str },

    #[error("interval would sample {epochs} epochs, above the limit of {max}; use a coarser step")]
    SeriesTooLong { epochs: usize, max: usize },
}
