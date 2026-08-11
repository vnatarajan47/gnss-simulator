//! # gnss-iq
//!
//! Synthesis of GNSS baseband IQ recordings from broadcast ephemeris.
//!
//! A job describes a receiver, a time window, an output format and a choice of
//! error models; running it produces an interleaved IQ file plus a JSON
//! sidecar recording exactly how it was made.
//!
//! ## Stages
//!
//! ```text
//! JobSpec --validate--> ValidatedJob
//!    |
//!    +-- ephemeris::EphemerisSource      fetch/cache RINEX Nav
//!    +-- observables                     geometry, Doppler, pseudorange, C/N0
//!    +-- code + lnav                     spreading code and navigation bits
//!    +-- synth                           per-sample carrier/code, noise, quantise
//!    +-- sink                            binary file and sidecar
//! ```
//!
//! Each stage consumes the previous stage's output and nothing else, so the
//! observables can be inspected without generating samples -- which is what
//! the acquisition cross-check in `tests/` relies on to compare recovered
//! measurements against the truth they were generated from.
//!
//! ## Relationship to `gnss-core`
//!
//! All orbital mechanics, time handling and geometry come from `gnss-core`,
//! which is cross-checked against an independent Python implementation. This
//! crate adds no propagation math of its own; where it needs a satellite
//! position it calls the same code the sky plot uses. That is deliberate --
//! see the validation standard in `CLAUDE.md`.
//!
//! Unlike `gnss-core`, this crate is native-only. It is not part of the WASM
//! build and does not need to be: see
//! `docs/adr/0007-server-side-iq-worker.md`.

pub mod code;
pub mod ephemeris;
pub mod job;
pub mod lnav;
pub mod noise;
pub mod observables;
pub mod pipeline;
pub mod rng;
pub mod signal;
pub mod sink;
pub mod synth;

pub use job::{JobSpec, Quantization, ValidatedJob};
pub use noise::{
    build_models, Cn0Model, ElevationCn0Model, FixedPseudorangeNoise, ModelDescription, NoiseModels,
    PseudorangeNoiseModel,
};
pub use observables::{EpochObservables, Observables, SatelliteObservable};
pub use pipeline::{run_job, JobArtifacts, Progress};
pub use signal::{Band, Signal};

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("job is invalid: {reason}")]
    InvalidJob { reason: String },

    #[error("no signal implementation for {requested}; this build generates GPS L1 C/A only")]
    UnsupportedSignal { requested: String },

    #[error(transparent)]
    Core(#[from] gnss_core::Error),

    #[error("could not read or write {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("no broadcast ephemeris available for {date}: {reason}")]
    EphemerisUnavailable { date: String, reason: String },

    /// Distinct from an empty sky, which is legitimate: this means the window
    /// produced no usable satellite at *any* epoch, so the output would be
    /// pure noise. Generating that silently is worse than refusing.
    #[error(
        "no satellite was above the {mask_deg} deg elevation mask at any epoch in the window; \
         the output would contain noise only"
    )]
    NoSatellitesVisible { mask_deg: f64 },
}

impl Error {
    /// Attach a path to an I/O failure.
    pub(crate) fn io(path: impl std::fmt::Display, source: std::io::Error) -> Self {
        Error::Io {
            path: path.to_string(),
            source,
        }
    }
}
