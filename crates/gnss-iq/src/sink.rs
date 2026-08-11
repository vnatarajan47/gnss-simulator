//! Output files: the raw interleaved binary and its JSON sidecar.
//!
//! The binary is written by [`crate::synth`] directly, so this module owns the
//! sidecar -- which is the part that has to be right for the recording to
//! remain interpretable. A raw IQ file carries no header at all: without the
//! sidecar it is an anonymous block of bytes, and a wrong sample rate or byte
//! order in it makes the recording useless in a way that is hard to diagnose.
//!
//! ## What the sidecar promises
//!
//! Everything needed to (a) read the file and (b) reproduce it. That means the
//! format parameters, the full job spec, the exact noise models *and their
//! parameters as the models themselves report them*, the seed actually used,
//! and the provenance of anything the pipeline had to substitute -- notably
//! the ionospheric coefficients, which are the fallback set whenever the
//! broadcast file carried none.
//!
//! Model parameters come from [`crate::noise::ModelDescription`] rather than
//! from the job spec. Those differ: the C/N0 coefficients are not job inputs
//! at all, and recording only what the user submitted would leave the file
//! silently dependent on whatever the defaults happened to be that day.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;

use crate::job::{Quantization, ValidatedJob, SCHEMA_VERSION};
use crate::noise::{ModelDescription, NoiseModels};
use crate::observables::{IonosphereSource, Observables};
use crate::synth::SynthesisSummary;
use crate::Error;

/// The JSON document written next to the binary.
#[derive(Debug, Clone, Serialize)]
pub struct Sidecar {
    /// Schema version of this document.
    pub format_version: u32,
    pub job_id: String,
    /// ISO 8601 UTC instant the file was produced.
    pub generated_at: String,
    pub generator: &'static str,

    pub signal: SignalMetadata,
    pub format: FormatMetadata,
    pub receiver: ReceiverMetadata,
    pub window: WindowMetadata,
    pub elevation_mask_deg: f64,

    /// Exactly the shape the brief specifies: model tags and their parameters
    /// flattened together.
    pub noise_models: NoiseMetadata,
    pub atmosphere: AtmosphereMetadata,

    pub ephemeris: EphemerisMetadata,
    pub satellites: Vec<SatelliteMetadata>,
    pub synthesis: SynthesisMetadata,

    /// The job as submitted, verbatim.
    pub job: crate::job::JobSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignalMetadata {
    /// e.g. `GPS_L1CA`.
    pub id: &'static str,
    pub constellation: String,
    pub band: String,
    pub carrier_frequency_hz: f64,
    pub chip_rate_hz: f64,
    pub code_length_chips: u32,
    pub data_rate_bps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatMetadata {
    pub sample_rate_hz: f64,
    pub center_frequency_hz: f64,
    /// Where the nominal carrier sits in the recording \[Hz\]. Zero for a true
    /// baseband file.
    pub intermediate_frequency_hz: f64,
    pub quantization: &'static str,
    pub byte_order: &'static str,
    pub interleaving: &'static str,
    pub complex: bool,
    pub bytes_per_sample: usize,
    pub sample_count: u64,
    /// Multiplier applied before quantisation; divide by it to recover the
    /// units the amplitudes were computed in.
    pub scale_factor: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiverMetadata {
    pub kind: &'static str,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowMetadata {
    pub start_unix_s: f64,
    pub start_iso: String,
    pub duration_s: f64,
    pub epoch_interval_s: f64,
    pub epoch_count: usize,
    pub start_gps_week: u32,
    pub start_gps_seconds_of_week: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoiseMetadata {
    pub pseudorange: ModelDescription,
    pub cn0: ModelDescription,
    /// The seed every draw came from. Always present, even when the request
    /// omitted one.
    pub seed: u64,
    pub generator: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct AtmosphereMetadata {
    pub ionosphere_model: &'static str,
    /// `broadcast` or `fallback`.
    pub ionosphere_source: IonosphereSource,
    pub ionosphere_alpha: [f64; 4],
    pub ionosphere_beta: [f64; 4],
    pub troposphere_model: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct EphemerisMetadata {
    pub source: &'static str,
    pub files: Vec<String>,
}

/// Per-satellite summary, so the recording can be checked without rerunning
/// the geometry.
#[derive(Debug, Clone, Serialize)]
pub struct SatelliteMetadata {
    pub prn: u8,
    pub id: String,
    /// Epoch indices where the satellite was above the mask.
    pub first_epoch: usize,
    pub last_epoch: usize,
    pub epochs_visible: usize,
    pub elevation_min_deg: f64,
    pub elevation_max_deg: f64,
    pub cn0_min_db_hz: f64,
    pub cn0_max_db_hz: f64,
    pub doppler_min_hz: f64,
    pub doppler_max_hz: f64,
    pub pseudorange_first_m: f64,
    pub pseudorange_last_m: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SynthesisMetadata {
    pub samples_written: u64,
    pub clipped_samples: u64,
    pub peak_magnitude: f64,
    pub satellites_generated: usize,
}

impl Sidecar {
    /// Assemble the sidecar for a completed run.
    pub fn build(
        job_id: &str,
        job: &ValidatedJob,
        observables: &Observables,
        models: &NoiseModels,
        summary: &SynthesisSummary,
        ephemeris_files: Vec<String>,
        generated_at_unix_s: f64,
    ) -> Self {
        let signal = job.signal;
        let spec = &job.spec;

        let receiver = job.receiver_at(0);
        let quantization = spec.output.quantization;

        let tracks = observables.tracks();
        let satellites = tracks
            .iter()
            .map(|(&sv, indices)| {
                let samples: Vec<_> = indices
                    .iter()
                    .filter_map(|&index| observables.epochs[index].satellite(sv))
                    .collect();

                let extremes = |extract: fn(&crate::observables::SatelliteObservable) -> f64| {
                    samples.iter().fold((f64::MAX, f64::MIN), |(lo, hi), s| {
                        let value = extract(s);
                        (lo.min(value), hi.max(value))
                    })
                };

                let (elevation_min_deg, elevation_max_deg) = extremes(|s| s.elevation_deg);
                let (cn0_min_db_hz, cn0_max_db_hz) = extremes(|s| s.cn0_db_hz);
                let (doppler_min_hz, doppler_max_hz) = extremes(|s| s.doppler_hz);

                SatelliteMetadata {
                    prn: sv.prn,
                    id: sv.to_string(),
                    first_epoch: *indices.first().expect("non-empty track"),
                    last_epoch: *indices.last().expect("non-empty track"),
                    epochs_visible: indices.len(),
                    elevation_min_deg,
                    elevation_max_deg,
                    cn0_min_db_hz,
                    cn0_max_db_hz,
                    doppler_min_hz,
                    doppler_max_hz,
                    pseudorange_first_m: samples.first().map_or(f64::NAN, |s| s.pseudorange_m),
                    pseudorange_last_m: samples.last().map_or(f64::NAN, |s| s.pseudorange_m),
                }
            })
            .collect::<Vec<_>>();

        Self {
            format_version: SCHEMA_VERSION,
            job_id: job_id.to_string(),
            generated_at: iso8601_utc(generated_at_unix_s),
            generator: concat!("gnss-iq ", env!("CARGO_PKG_VERSION")),

            signal: SignalMetadata {
                id: signal.id(),
                constellation: format!("{:?}", signal.constellation()).to_uppercase(),
                band: signal.band().to_string(),
                carrier_frequency_hz: signal.carrier_hz(),
                chip_rate_hz: signal.chip_rate_hz(),
                code_length_chips: signal.code_length_chips(),
                data_rate_bps: signal.data_rate_hz(),
            },
            format: FormatMetadata {
                sample_rate_hz: spec.output.sample_rate_hz,
                center_frequency_hz: spec.output.center_frequency_hz,
                intermediate_frequency_hz: job.intermediate_frequency_hz,
                quantization: quantization.id(),
                byte_order: "little",
                interleaving: "IQIQ",
                complex: true,
                bytes_per_sample: quantization.bytes_per_component() * 2,
                sample_count: summary.samples_written,
                scale_factor: summary.scale_factor,
            },
            receiver: ReceiverMetadata {
                kind: "static",
                latitude_deg: receiver.latitude_deg,
                longitude_deg: receiver.longitude_deg,
                altitude_m: receiver.altitude_m,
            },
            window: WindowMetadata {
                start_unix_s: spec.window.start_unix_s,
                start_iso: iso8601_utc(spec.window.start_unix_s),
                duration_s: spec.window.duration_s,
                epoch_interval_s: spec.window.epoch_interval_s,
                epoch_count: job.epoch_count,
                start_gps_week: job.start.week(),
                start_gps_seconds_of_week: job.start.seconds_of_week(),
            },
            elevation_mask_deg: spec.elevation_mask_deg,
            noise_models: NoiseMetadata {
                pseudorange: models.pseudorange.describe(),
                cn0: models.cn0.describe(),
                seed: job.seed(),
                generator: "pcg32-xsh-rr",
            },
            atmosphere: AtmosphereMetadata {
                ionosphere_model: "klobuchar",
                ionosphere_source: observables.ionosphere.source,
                ionosphere_alpha: observables.ionosphere.model.alpha,
                ionosphere_beta: observables.ionosphere.model.beta,
                troposphere_model: "saastamoinen",
            },
            ephemeris: EphemerisMetadata {
                source: "rinex-nav-broadcast",
                files: ephemeris_files,
            },
            synthesis: SynthesisMetadata {
                samples_written: summary.samples_written,
                clipped_samples: summary.clipped_samples,
                peak_magnitude: summary.peak_magnitude,
                satellites_generated: satellites.len(),
            },
            satellites,
            job: *spec,
        }
    }

    /// Write as pretty-printed JSON.
    ///
    /// Pretty rather than compact on purpose: this file is read by people at
    /// least as often as by programs, and it is a few kilobytes next to a
    /// multi-hundred-megabyte recording.
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        let json = serde_json::to_string_pretty(self)?;
        let mut file = std::fs::File::create(path).map_err(|e| Error::io(path.display(), e))?;
        file.write_all(json.as_bytes())
            .map_err(|e| Error::io(path.display(), e))?;
        file.write_all(b"\n")
            .map_err(|e| Error::io(path.display(), e))?;
        Ok(())
    }
}

/// Format a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
fn iso8601_utc(unix_seconds: f64) -> String {
    let whole = unix_seconds.floor() as i64;
    let day = crate::ephemeris::Day::from_unix_seconds(unix_seconds);
    let seconds_into_day = whole.rem_euclid(86_400);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        day.year,
        day.month,
        day.day,
        seconds_into_day / 3600,
        (seconds_into_day % 3600) / 60,
        seconds_into_day % 60
    )
}

/// Total bytes a quantisation writes per complex sample. Exposed for callers
/// sizing a download response.
pub fn bytes_per_sample(quantization: Quantization) -> usize {
    quantization.bytes_per_component() * 2
}

/// Convenience map of the two output filenames for a job.
pub fn output_names(job_id: &str) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("binary", format!("{job_id}.iq")),
        ("sidecar", format!("{job_id}.json")),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_format_as_iso8601_utc() {
        assert_eq!(iso8601_utc(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_735_732_800.0), "2025-01-01T12:00:00Z");
        assert_eq!(iso8601_utc(1_735_775_999.0), "2025-01-01T23:59:59Z");
        // Fractional seconds truncate rather than round up past midnight.
        assert_eq!(iso8601_utc(1_735_775_999.9), "2025-01-01T23:59:59Z");
    }

    #[test]
    fn output_names_pair_the_binary_with_its_sidecar() {
        let names = output_names("abc-123");
        assert_eq!(names["binary"], "abc-123.iq");
        assert_eq!(names["sidecar"], "abc-123.json");
    }

    #[test]
    fn bytes_per_sample_accounts_for_both_components() {
        assert_eq!(bytes_per_sample(Quantization::Int8), 2);
        assert_eq!(bytes_per_sample(Quantization::Int16), 4);
        assert_eq!(bytes_per_sample(Quantization::Float32), 8);
    }
}
