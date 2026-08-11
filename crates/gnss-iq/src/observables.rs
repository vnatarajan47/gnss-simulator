//! Geometry and measurement stage: what each satellite looks like from the
//! receiver at each epoch.
//!
//! This is the boundary between orbital mechanics and signal generation.
//! Everything above it deals in satellites and metres; everything below it
//! deals in samples and radians, and reads only the numbers this stage
//! produces.
//!
//! ## Code noise is not carrier noise
//!
//! Each satellite carries two ranges: [`SatelliteObservable::pseudorange_m`],
//! which includes the draw from the pseudorange noise model, and
//! [`SatelliteObservable::carrier_range_m`], which does not. The synthesiser
//! drives the spreading code from the first and the carrier phase from the
//! second.
//!
//! That split is physical rather than convenient. Pseudorange error is *code*
//! measurement error; carrier-phase measurements on the same signal are two
//! orders of magnitude quieter, which is the entire basis of carrier smoothing
//! and of RTK. Driving the carrier from the noisy range instead would
//! translate a 1.5 m sigma at a 0.1 s epoch spacing into roughly +/-150 Hz of
//! frequency jitter -- a signal no real receiver could track, produced by a
//! parameter the user set to a perfectly ordinary value.
//!
//! ## Elevation masking
//!
//! Minimal by design: a satellite is either above the mask at an epoch or
//! absent from it, with no taper. A satellite crossing the mask therefore
//! switches on at full amplitude. Real rise and set are gradual, and terrain
//! masking (phase 3) will need a softer treatment, but neither is this pass's
//! concern.

use std::collections::BTreeMap;

use gnss_core::constants::SPEED_OF_LIGHT;
use gnss_core::{
    apparent_state, clock_correction, geodetic_to_ecef, look_angles, saastamoinen_delay_m,
    BroadcastEphemeris, EphemerisSet, Geodetic, GpsTime, GroupDelay, KlobucharModel,
    PropagationConfig, SelectionConfig, Sv,
};

use crate::job::ValidatedJob;
use crate::noise::NoiseModels;
use crate::rng::Pcg32;
use crate::Error;

/// Where the Klobuchar coefficients came from.
///
/// Recorded in the sidecar because the two are not interchangeable: broadcast
/// coefficients describe the ionosphere the constellation was actually
/// modelling that day, and the fallback describes a plausible average one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IonosphereSource {
    /// Lifted from the RINEX Nav header.
    Broadcast,
    /// [`KlobucharModel::FALLBACK`], because the header carried none.
    Fallback,
}

/// The ionospheric model a job ran with, and its provenance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ionosphere {
    pub model: KlobucharModel,
    pub source: IonosphereSource,
}

impl Ionosphere {
    /// Take the header's coefficients if there are any, else the fallback.
    pub fn resolve(set: &EphemerisSet) -> Self {
        match set.klobuchar() {
            Some(model) => Self {
                model,
                source: IonosphereSource::Broadcast,
            },
            None => Self {
                model: KlobucharModel::FALLBACK,
                source: IonosphereSource::Fallback,
            },
        }
    }
}

/// One satellite at one epoch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatelliteObservable {
    pub sv: Sv,
    pub azimuth_deg: f64,
    pub elevation_deg: f64,

    /// Straight-line receiver-to-satellite distance \[m\], light-time
    /// corrected.
    pub geometric_range_m: f64,

    /// What a receiver measures \[m\]: geometry, minus the satellite clock
    /// offset, plus atmospheric delay, plus the noise draw.
    pub pseudorange_m: f64,

    /// The same range without the noise draw \[m\], which is what the carrier
    /// phase follows. See the module comment.
    pub carrier_range_m: f64,

    /// Range rate along the line of sight \[m/s\], positive receding.
    pub range_rate_m_s: f64,

    /// Carrier Doppler shift \[Hz\], positive approaching.
    ///
    /// Derived analytically from the satellite velocity and clock drift. The
    /// synthesiser does not use it -- it gets Doppler for free from the rate
    /// of change of `carrier_range_m` -- so this is a reported diagnostic
    /// that two independent paths must agree on.
    pub doppler_hz: f64,

    pub cn0_db_hz: f64,

    /// Satellite clock offset applied \[s\], including the relativistic term
    /// and L1 group delay.
    pub clock_bias_s: f64,
    pub ionospheric_delay_m: f64,
    pub tropospheric_delay_m: f64,

    /// 1-sigma the noise model reported \[m\].
    pub noise_sigma_m: f64,
    /// The realised draw \[m\]. `pseudorange_m - carrier_range_m`.
    pub noise_m: f64,
}

/// Every visible satellite at one epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct EpochObservables {
    pub index: usize,
    pub time: GpsTime,
    pub receiver: Geodetic,
    /// Satellites above the mask, ordered by PRN.
    pub satellites: Vec<SatelliteObservable>,
}

impl EpochObservables {
    pub fn satellite(&self, sv: Sv) -> Option<&SatelliteObservable> {
        self.satellites.iter().find(|s| s.sv == sv)
    }
}

/// The observables for a whole job.
#[derive(Debug, Clone)]
pub struct Observables {
    pub epochs: Vec<EpochObservables>,
    pub ionosphere: Ionosphere,
    /// Satellites that appeared above the mask at least once.
    pub satellites: Vec<Sv>,
}

impl Observables {
    /// Per-satellite view: for each satellite, the epoch indices at which it
    /// was visible.
    ///
    /// Sparse for the same reason `gnss_core::series` is: a gap is a set and a
    /// later rise, and a consumer that interpolates across one would put a
    /// satellite where it never was. The synthesiser walks these runs and
    /// stops at each break.
    pub fn tracks(&self) -> BTreeMap<Sv, Vec<usize>> {
        let mut tracks: BTreeMap<Sv, Vec<usize>> = BTreeMap::new();
        for epoch in &self.epochs {
            for satellite in &epoch.satellites {
                tracks.entry(satellite.sv).or_default().push(epoch.index);
            }
        }
        tracks
    }
}

/// Compute observables for the whole window.
///
/// `models` is borrowed as trait objects: this function cannot name a concrete
/// noise model, and adding one does not change it.
pub fn compute(
    job: &ValidatedJob,
    set: &EphemerisSet,
    models: &NoiseModels,
) -> Result<Observables, Error> {
    let signal = job.signal;
    let constellation = signal.constellation();
    let propagation = PropagationConfig::default();
    let selection = SelectionConfig::default();
    let ionosphere = Ionosphere::resolve(set);

    // Ionospheric delay is inversely proportional to the square of the carrier
    // frequency, and the Klobuchar output is defined at L1. For L1 this is 1;
    // it is written out so a second band gets the scaling automatically.
    let ionosphere_band_scale = (KLOBUCHAR_REFERENCE_HZ / signal.carrier_hz()).powi(2);

    let mut epochs = Vec::with_capacity(job.epoch_count);
    let mut seen: BTreeMap<Sv, ()> = BTreeMap::new();

    // Candidates fixed up front, each with its own noise stream. One stream
    // per satellite -- rather than one shared stream consumed in whatever
    // order satellites happen to be processed -- so that a satellite's noise
    // does not depend on how many others were visible. Streams are keyed by
    // PRN, so the same job with a different mask gives every satellite that
    // survives both masks identical draws.
    let candidates: Vec<Sv> = set
        .satellites()
        .filter(|sv| sv.constellation == constellation)
        .collect();
    let mut streams: BTreeMap<Sv, Pcg32> = candidates
        .iter()
        .map(|&sv| (sv, Pcg32::seed(job.seed(), u64::from(sv.prn))))
        .collect();

    for index in 0..job.epoch_count {
        let time = job.epoch_time(index);
        let receiver = job.receiver_at(index);
        let receiver_ecef = geodetic_to_ecef(receiver);

        let mut satellites = Vec::new();

        for &sv in &candidates {
            // Drawn for every candidate at every epoch, before any filtering,
            // so that the draw at (satellite, epoch) is the same whether or
            // not the satellite turned out to be visible. Doing it after the
            // mask check would make each satellite's noise depend on its own
            // rise and set times, and a mask change would reshuffle the lot.
            let noise_draw = streams
                .get_mut(&sv)
                .expect("every candidate has a stream")
                .next_normal();

            let Some(BroadcastEphemeris::Keplerian(ephemeris)) = set.select(sv, time, selection)
            else {
                // Either no block covers this instant, or the satellite is
                // SBAS -- which has no Keplerian model and no L1 C/A signal
                // this crate generates.
                continue;
            };

            let state = apparent_state(ephemeris, time, receiver_ecef, &propagation)?;
            let angles = look_angles(state.position, receiver);
            if angles.elevation_deg < job.spec.elevation_mask_deg {
                continue;
            }

            // Signal transmission instant, for the clock polynomial.
            let transit_time_s = angles.range_m / SPEED_OF_LIGHT;
            let transmit_time = time.offset_by(-transit_time_s);
            let clock = clock_correction(
                ephemeris,
                transmit_time,
                GroupDelay::ApplyL1,
                &propagation,
            )?;

            let ionospheric_delay_m = ionosphere_band_scale
                * ionosphere.model.slant_delay_l1_m(
                    receiver,
                    angles.azimuth_deg,
                    angles.elevation_deg,
                    time,
                );
            let tropospheric_delay_m = saastamoinen_delay_m(receiver, angles.elevation_deg);

            // A satellite clock reading ahead of system time makes the signal
            // appear to have left earlier, shortening the measured range --
            // hence the minus sign, per the ICD's sign convention.
            let carrier_range_m = angles.range_m - SPEED_OF_LIGHT * clock.bias_s
                + ionospheric_delay_m
                + tropospheric_delay_m;

            let noise_sigma_m = models.pseudorange.sigma_m(sv, time, angles.elevation_deg);
            let noise_m = noise_sigma_m * noise_draw;

            let unit_line_of_sight = (state.position - receiver_ecef)
                .normalized()
                .ok_or_else(|| Error::InvalidJob {
                    reason: "receiver and satellite are at the same point".into(),
                })?;
            // Static receiver, so its ECEF velocity is zero and the relative
            // velocity is the satellite's alone.
            let range_rate_m_s = state.velocity.dot(unit_line_of_sight);

            // Approaching (negative range rate) raises the observed frequency.
            // The clock-drift term is the satellite's own frequency error.
            let doppler_hz = -signal.carrier_hz() * range_rate_m_s / SPEED_OF_LIGHT
                + signal.carrier_hz() * clock.drift_s_per_s;

            seen.insert(sv, ());
            satellites.push(SatelliteObservable {
                sv,
                azimuth_deg: angles.azimuth_deg,
                elevation_deg: angles.elevation_deg,
                geometric_range_m: angles.range_m,
                pseudorange_m: carrier_range_m + noise_m,
                carrier_range_m,
                range_rate_m_s,
                doppler_hz,
                cn0_db_hz: models.cn0.cn0_db_hz(sv, time, angles.elevation_deg),
                clock_bias_s: clock.bias_s,
                ionospheric_delay_m,
                tropospheric_delay_m,
                noise_sigma_m,
                noise_m,
            });
        }

        satellites.sort_by_key(|s| s.sv.prn);
        epochs.push(EpochObservables {
            index,
            time,
            receiver,
            satellites,
        });
    }

    if seen.is_empty() {
        return Err(Error::NoSatellitesVisible {
            mask_deg: job.spec.elevation_mask_deg,
        });
    }

    Ok(Observables {
        epochs,
        ionosphere,
        satellites: seen.into_keys().collect(),
    })
}

/// Frequency the Klobuchar delay is defined at \[Hz\] -- GPS L1.
const KLOBUCHAR_REFERENCE_HZ: f64 = 1_575.42e6;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{
        Cn0Spec, ConstellationSpec, JobSpec, NoiseSpec, OutputSpec, PseudorangeNoiseSpec,
        Quantization, ReceiverSpec, WindowSpec,
    };
    use crate::noise::build_models;
    use crate::signal::Band;

    fn fixture_set() -> EphemerisSet {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/BRDC00WRD_R_20250010000_01D_GN.rnx");
        gnss_core::parse_nav(&std::fs::read(path).expect("checked-in fixture")).unwrap()
    }

    fn job(sigma_m: f64) -> ValidatedJob {
        JobSpec {
            constellation: ConstellationSpec::Gps,
            band: Band::L1,
            receiver: ReceiverSpec::Static {
                latitude_deg: 39.7392,
                longitude_deg: -104.9903,
                altitude_m: 1609.0,
            },
            window: WindowSpec {
                // 2025-01-01T12:00:00Z, inside the fixture file.
                start_unix_s: 1_735_732_800.0,
                duration_s: 10.0,
                epoch_interval_s: 1.0,
            },
            output: OutputSpec {
                sample_rate_hz: 2.6e6,
                quantization: Quantization::Int8,
                center_frequency_hz: 1_575.42e6,
            },
            elevation_mask_deg: 5.0,
            noise: NoiseSpec {
                pseudorange: PseudorangeNoiseSpec::Fixed { sigma_m },
                cn0: Cn0Spec::Elevation,
            },
            seed: Some(1234),
        }
        .validate()
        .unwrap()
    }

    fn observables(sigma_m: f64) -> Observables {
        let job = job(sigma_m);
        compute(&job, &fixture_set(), &build_models(&job.spec.noise)).unwrap()
    }

    #[test]
    fn a_real_file_yields_a_plausible_sky() {
        let result = observables(0.0);
        assert_eq!(result.epochs.len(), 11);

        // Denver at midday: eight to twelve GPS satellites above 5 degrees is
        // the normal range.
        let count = result.epochs[0].satellites.len();
        assert!((6..=14).contains(&count), "{count} satellites above the mask");

        // The fixture's header carries no ionospheric block, so this job must
        // be recorded as having used the fallback.
        assert_eq!(result.ionosphere.source, IonosphereSource::Fallback);

        for satellite in &result.epochs[0].satellites {
            assert!(satellite.elevation_deg >= 5.0);
            assert!((20_000_000.0..27_000_000.0).contains(&satellite.geometric_range_m));
            // Pseudorange sits above geometric range: the atmosphere adds
            // several metres and the clock term is under a millisecond either
            // way.
            let excess = satellite.pseudorange_m - satellite.geometric_range_m;
            assert!(
                (-400_000.0..400_000.0).contains(&excess),
                "{} excess {excess:.1} m",
                satellite.sv
            );
            assert!((25.0..50.0).contains(&satellite.cn0_db_hz));
        }
    }

    /// The reported Doppler comes from the analytic satellite velocity; the
    /// carrier the synthesiser generates comes from the rate of change of
    /// `carrier_range_m`. Those are separate code paths -- one differentiates
    /// the orbit, the other differences a range series -- and the whole signal
    /// is wrong if they disagree.
    #[test]
    fn reported_doppler_matches_the_rate_of_change_of_the_carrier_range() {
        let result = observables(0.0);
        let carrier_hz = 1_575.42e6;

        for window in result.epochs.windows(3) {
            let (before, at, after) = (&window[0], &window[1], &window[2]);
            let step = at.time.seconds_since(before.time);

            for satellite in &at.satellites {
                let (Some(previous), Some(next)) = (
                    before.satellite(satellite.sv),
                    after.satellite(satellite.sv),
                ) else {
                    continue; // rose or set inside the window
                };

                let numerical_rate =
                    (next.carrier_range_m - previous.carrier_range_m) / (2.0 * step);
                let numerical_doppler = -carrier_hz * numerical_rate / SPEED_OF_LIGHT;

                assert!(
                    (satellite.doppler_hz - numerical_doppler).abs() < 0.5,
                    "{}: analytic {:.2} Hz vs numerical {:.2} Hz",
                    satellite.sv,
                    satellite.doppler_hz,
                    numerical_doppler
                );
            }
        }
    }

    /// GPS Doppler seen from a fixed point on the ground is bounded by about
    /// +/-5 kHz. A value outside that means the velocity or the sign is wrong.
    #[test]
    fn doppler_stays_within_the_gps_envelope() {
        for satellite in observables(0.0).epochs.iter().flat_map(|e| &e.satellites) {
            assert!(
                satellite.doppler_hz.abs() < 5_000.0,
                "{} Doppler {:.1} Hz",
                satellite.sv,
                satellite.doppler_hz
            );
        }
    }

    /// Noise must reach the code range and leave the carrier range alone.
    #[test]
    fn noise_lands_on_the_code_range_only() {
        let quiet = observables(0.0);
        let noisy = observables(3.0);

        for (quiet_epoch, noisy_epoch) in quiet.epochs.iter().zip(&noisy.epochs) {
            for noisy_satellite in &noisy_epoch.satellites {
                let quiet_satellite = quiet_epoch.satellite(noisy_satellite.sv).unwrap();

                // Same geometry, so the carrier range is bit-for-bit untouched.
                assert_eq!(
                    noisy_satellite.carrier_range_m,
                    quiet_satellite.carrier_range_m
                );
                // And the code range differs by exactly the recorded draw.
                // Tolerance is set by the ulp of a 2.4e7 m range, about 4 nm,
                // not by anything the model does.
                assert!(
                    (noisy_satellite.pseudorange_m
                        - noisy_satellite.carrier_range_m
                        - noisy_satellite.noise_m)
                        .abs()
                        < 1e-6
                );
            }
        }

        // With sigma = 3 m the draws should have roughly that spread.
        let draws: Vec<f64> = noisy
            .epochs
            .iter()
            .flat_map(|e| &e.satellites)
            .map(|s| s.noise_m)
            .collect();
        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        let variance =
            draws.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / draws.len() as f64;
        assert!(
            (1.0..6.0).contains(&variance.sqrt()),
            "sample sigma {:.2} m for a requested 3 m",
            variance.sqrt()
        );
    }

    /// Zero sigma must mean exactly zero noise, not a small draw. Users set it
    /// to get a noise-free reference, and "almost noise-free" is not that.
    #[test]
    fn zero_sigma_produces_no_noise_at_all() {
        for satellite in observables(0.0).epochs.iter().flat_map(|e| &e.satellites) {
            assert_eq!(satellite.noise_m, 0.0);
            assert_eq!(satellite.pseudorange_m, satellite.carrier_range_m);
        }
    }

    /// The same seed must give the same draws, since the sidecar promises it.
    #[test]
    fn the_recorded_seed_reproduces_the_run() {
        let first = observables(2.0);
        let second = observables(2.0);
        for (a, b) in first.epochs.iter().zip(&second.epochs) {
            assert_eq!(a.satellites, b.satellites);
        }
    }

    /// Tracks index the shared epoch axis and stay ascending, so a consumer
    /// can detect a set by looking for a break.
    #[test]
    fn tracks_are_ascending_indices_into_the_epoch_axis() {
        let result = observables(0.0);
        let tracks = result.tracks();
        assert!(!tracks.is_empty());

        for (sv, indices) in tracks {
            assert!(!indices.is_empty(), "{sv} has an empty track");
            for pair in indices.windows(2) {
                assert!(pair[0] < pair[1], "{sv} track is not ascending");
            }
            assert!(indices.iter().all(|&i| i < result.epochs.len()));
        }
    }

    /// A mask above every satellite is an error, not an empty file: the output
    /// would be indistinguishable from noise and nothing about it would say
    /// why.
    #[test]
    fn a_sky_emptied_by_the_mask_is_an_error() {
        let mut spec = job(0.0).spec;
        spec.elevation_mask_deg = 89.0;
        let job = spec.validate().unwrap();
        let error = compute(&job, &fixture_set(), &build_models(&spec.noise)).unwrap_err();
        assert!(matches!(error, Error::NoSatellitesVisible { .. }));
    }
}
