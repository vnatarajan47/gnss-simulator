//! Swappable error and signal-strength models.
//!
//! Two traits, injected into the pipeline rather than called by name from it:
//! [`PseudorangeNoiseModel`] decides how far a measured range strays from the
//! true one, and [`Cn0Model`] decides how strong the signal arrives. Nothing
//! in [`crate::observables`] or [`crate::synth`] names a concrete
//! implementation; they receive `&dyn` references built once by
//! [`build_models`] from the job spec.
//!
//! ## Why `describe`
//!
//! Both traits require a [`ModelDescription`]. The output sidecar has to
//! record the exact model and parameters a job ran with, so that a result
//! stays interpretable after the defaults change. If the sidecar writer had to
//! match on concrete types to learn a model's parameters, then adding a model
//! would mean editing the sidecar writer, and the injection would be
//! decorative -- the pipeline would still have to know the full set of
//! implementations. Making each model describe itself is what keeps "swap in a
//! new model" to a single new type plus one arm in [`build_models`].
//!
//! ## What is user-facing and what is not
//!
//! The pseudorange sigma is a job parameter: a user asking for a noisier
//! receiver is asking a question the simulator exists to answer. The C/N0
//! coefficients are *not* -- they are baked into [`ElevationCn0Model`]. They
//! describe an assumed antenna and receiver front end, and exposing them would
//! invite tuning a curve that a later, physically-derived model will not have
//! the same knobs for.

use std::collections::BTreeMap;

use gnss_core::{GpsTime, Sv};

use crate::job::{Cn0Spec, NoiseSpec, PseudorangeNoiseSpec};

/// A model's identity and exact parameters, for the output sidecar.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModelDescription {
    /// Stable model identifier, matching the job spec's `model` tag.
    pub model: &'static str,
    /// Every parameter the model actually used, named as the sidecar reports
    /// them. `BTreeMap` so the ordering is stable across runs -- a sidecar
    /// that reorders itself between identical jobs is needlessly hard to diff.
    #[serde(flatten)]
    pub parameters: BTreeMap<&'static str, f64>,
}

/// 1-sigma pseudorange measurement error.
///
/// Implementations must be deterministic: this returns the *width* of the
/// error distribution, not a draw from it. Drawing happens once, centrally, in
/// [`crate::observables`], so that a job's randomness comes from exactly one
/// seeded generator and stays reproducible.
pub trait PseudorangeNoiseModel: Send + Sync {
    /// 1-sigma pseudorange error \[m\] for one satellite at one epoch.
    fn sigma_m(&self, sv: Sv, epoch: GpsTime, elevation_deg: f64) -> f64;

    fn describe(&self) -> ModelDescription;
}

/// Carrier-to-noise-density ratio of the arriving signal.
pub trait Cn0Model: Send + Sync {
    /// C/N0 \[dB-Hz\] for one satellite at one epoch.
    fn cn0_db_hz(&self, sv: Sv, epoch: GpsTime, elevation_deg: f64) -> f64;

    fn describe(&self) -> ModelDescription;
}

/// A single Gaussian sigma for every satellite at every elevation.
///
/// The MVP model. It is wrong in a specific, well-understood way -- real
/// pseudorange error grows sharply at low elevation, because both multipath
/// and residual tropospheric error do -- which is exactly why the interface
/// hands implementations `elevation_deg` that this one ignores.
#[derive(Debug, Clone, Copy)]
pub struct FixedPseudorangeNoise {
    sigma_m: f64,
}

impl FixedPseudorangeNoise {
    pub const fn new(sigma_m: f64) -> Self {
        Self { sigma_m }
    }
}

impl PseudorangeNoiseModel for FixedPseudorangeNoise {
    fn sigma_m(&self, _sv: Sv, _epoch: GpsTime, _elevation_deg: f64) -> f64 {
        self.sigma_m
    }

    fn describe(&self) -> ModelDescription {
        ModelDescription {
            model: "fixed",
            parameters: BTreeMap::from([("sigma_m", self.sigma_m)]),
        }
    }
}

/// C/N0 falling off exponentially towards the horizon.
///
/// `cn0 = cn0_max - attenuation * exp(-elevation / decay)`, so a satellite at
/// the zenith arrives at `cn0_max` and one on the horizon at
/// `cn0_max - attenuation`. With the constants below that is 45 dB-Hz overhead
/// and 33 dB-Hz on the horizon, which brackets what a decent survey antenna
/// actually sees.
///
/// The curve stands in for three separate physical effects at once -- antenna
/// gain pattern, free-space path loss over a varying slant range, and
/// atmospheric absorption. A model that separates them is future work; the
/// interface already passes everything such a model would need except the
/// range, and adding that is a signature change on one trait rather than a
/// change to any caller.
#[derive(Debug, Clone, Copy)]
pub struct ElevationCn0Model {
    cn0_max_db_hz: f64,
    attenuation_db: f64,
    decay_deg: f64,
}

impl ElevationCn0Model {
    /// C/N0 at the zenith \[dB-Hz\].
    pub const DEFAULT_CN0_MAX_DB_HZ: f64 = 45.0;
    /// Total fall-off from zenith to horizon \[dB\].
    pub const DEFAULT_ATTENUATION_DB: f64 = 12.0;
    /// Elevation scale of the fall-off \[deg\].
    pub const DEFAULT_DECAY_DEG: f64 = 12.0;

    /// The model as jobs get it: fixed coefficients, not user-tunable.
    pub const fn with_default_coefficients() -> Self {
        Self {
            cn0_max_db_hz: Self::DEFAULT_CN0_MAX_DB_HZ,
            attenuation_db: Self::DEFAULT_ATTENUATION_DB,
            decay_deg: Self::DEFAULT_DECAY_DEG,
        }
    }
}

impl Cn0Model for ElevationCn0Model {
    fn cn0_db_hz(&self, _sv: Sv, _epoch: GpsTime, elevation_deg: f64) -> f64 {
        self.cn0_max_db_hz - self.attenuation_db * (-elevation_deg / self.decay_deg).exp()
    }

    fn describe(&self) -> ModelDescription {
        ModelDescription {
            model: "elevation",
            parameters: BTreeMap::from([
                ("cn0_max", self.cn0_max_db_hz),
                ("attenuation", self.attenuation_db),
                ("decay_deg", self.decay_deg),
            ]),
        }
    }
}

/// The models a job will run with.
pub struct NoiseModels {
    pub pseudorange: Box<dyn PseudorangeNoiseModel>,
    pub cn0: Box<dyn Cn0Model>,
}

/// Build the models named by a job spec.
///
/// This is the *only* place in the crate that maps a spec tag to a concrete
/// type. Adding a model means a new implementation plus one arm here; no
/// pipeline stage changes, because no pipeline stage names an implementation.
pub fn build_models(spec: &NoiseSpec) -> NoiseModels {
    NoiseModels {
        pseudorange: match spec.pseudorange {
            PseudorangeNoiseSpec::Fixed { sigma_m } => {
                Box::new(FixedPseudorangeNoise::new(sigma_m))
            }
        },
        cn0: match spec.cn0 {
            // Deliberately takes nothing from the spec: the coefficients are
            // the model's, not the job's.
            Cn0Spec::Elevation => Box::new(ElevationCn0Model::with_default_coefficients()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gnss_core::Constellation;

    fn sv() -> Sv {
        Sv::new(Constellation::Gps, 5)
    }

    fn epoch() -> GpsTime {
        GpsTime::from_week_and_sow(2347, 259_200.0)
    }

    #[test]
    fn fixed_noise_is_the_same_everywhere() {
        let model = FixedPseudorangeNoise::new(1.5);
        for elevation in [5.0, 30.0, 90.0] {
            assert_eq!(model.sigma_m(sv(), epoch(), elevation), 1.5);
        }
    }

    /// The endpoints are the model's whole contract: `cn0_max` overhead,
    /// `cn0_max - attenuation` at the horizon, monotone between.
    #[test]
    fn elevation_cn0_spans_its_stated_range_monotonically() {
        let model = ElevationCn0Model::with_default_coefficients();

        let horizon = model.cn0_db_hz(sv(), epoch(), 0.0);
        assert!((horizon - 33.0).abs() < 1e-12, "horizon C/N0 {horizon}");

        // e^-90/12 is 5.5e-4, so the zenith is within a millidecibel of the max.
        let zenith = model.cn0_db_hz(sv(), epoch(), 90.0);
        assert!((zenith - 45.0).abs() < 0.01, "zenith C/N0 {zenith}");

        let mut previous = f64::MIN;
        for step in 0..=90 {
            let cn0 = model.cn0_db_hz(sv(), epoch(), f64::from(step));
            assert!(cn0 > previous, "C/N0 not monotone at {step} deg");
            previous = cn0;
        }
    }

    /// `describe` is what the sidecar records, so it has to name the same
    /// parameters the model actually evaluated. The spec's own example is the
    /// expected shape.
    #[test]
    fn descriptions_carry_every_parameter_used() {
        let pseudorange = FixedPseudorangeNoise::new(1.5).describe();
        assert_eq!(pseudorange.model, "fixed");
        assert_eq!(pseudorange.parameters["sigma_m"], 1.5);

        let cn0 = ElevationCn0Model::with_default_coefficients().describe();
        assert_eq!(cn0.model, "elevation");
        assert_eq!(cn0.parameters["cn0_max"], 45.0);
        assert_eq!(cn0.parameters["attenuation"], 12.0);
        assert_eq!(cn0.parameters["decay_deg"], 12.0);

        // Flattened, so the sidecar reads {"model": "...", "sigma_m": 1.5}
        // rather than nesting the parameters under a key.
        let json = serde_json::to_value(&pseudorange).unwrap();
        assert_eq!(json["model"], "fixed");
        assert_eq!(json["sigma_m"], 1.5);
    }

    /// The spec's sigma reaches the model; the spec's C/N0 tag does not carry
    /// coefficients, so the built model must hold the defaults.
    #[test]
    fn build_models_wires_the_spec_through() {
        let models = build_models(&NoiseSpec {
            pseudorange: PseudorangeNoiseSpec::Fixed { sigma_m: 2.25 },
            cn0: Cn0Spec::Elevation,
        });

        assert_eq!(models.pseudorange.sigma_m(sv(), epoch(), 45.0), 2.25);
        assert_eq!(
            models.cn0.describe().parameters["cn0_max"],
            ElevationCn0Model::DEFAULT_CN0_MAX_DB_HZ
        );
    }
}
