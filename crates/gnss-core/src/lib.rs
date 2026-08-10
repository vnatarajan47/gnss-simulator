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
//! Phase 1 targets GPS. Galileo, BeiDou and QZSS broadcast the same Keplerian
//! parameter set, so [`propagate`] already handles them; enabling one is a
//! matter of adding it to [`SkyplotOptions::constellations`]. GLONASS is a
//! different problem entirely -- it broadcasts a position/velocity state
//! vector requiring numerical integration, not Keplerian elements -- and is
//! out of scope.

pub mod constants;
pub mod ephemeris;
pub mod geodesy;
pub mod propagate;
pub mod sbas;
pub mod skyplot;
pub mod source;
pub mod time;

pub use ephemeris::{
    BroadcastEphemeris, EphemerisSet, KeplerianEphemeris, SelectionConfig, SelectionStrategy, Sv,
};
pub use geodesy::{
    ecef_to_enu, ecef_to_geodetic, geodetic_to_ecef, look_angles, AzEl, Ecef, Enu, Geodetic,
};
pub use propagate::PropagationConfig;
pub use sbas::{SbasEphemeris, SbasProvider};
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
}
