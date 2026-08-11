//! The job specification: what a user submits, and what it validates to.
//!
//! ## Shape of the schema
//!
//! Three fields are enums with a single variant today, which is not
//! over-engineering but the specific thing the brief asked for -- adding the
//! excluded features later must not be a rework:
//!
//! - [`ReceiverSpec`] is tagged `kind`, so a moving receiver arrives as a new
//!   variant rather than as optional fields hung off a flat struct that
//!   already means "static".
//! - [`PseudorangeNoiseSpec`] and [`Cn0Spec`] are tagged `model`, so a model
//!   with different parameters is a new variant carrying its own, rather than
//!   a widening union of every model's parameters with most of them null.
//! - `constellation`/`band` are separate fields resolved through
//!   [`Signal::resolve`], so a second signal is a resolution arm.
//!
//! In each case the serialised form of today's job is unchanged by the
//! addition, so stored job records stay readable.

use gnss_core::{Constellation, Geodetic, GpsTime};
use serde::{Deserialize, Serialize};

use crate::signal::{Band, Signal};
use crate::Error;

/// Version of the job/sidecar schema.
///
/// Bump on any change to the serialised shape. Sidecars carry it so a file
/// found on disk years later can be read without guessing.
pub const SCHEMA_VERSION: u32 = 1;

/// Hard ceiling on generated file size \[bytes\].
///
/// IQ output is enormous and grows as `duration * sample_rate * 2 * width`:
/// five minutes at 4 MHz in int16 is 4.8 GB. The cap exists so an
/// arithmetically reasonable job cannot fill the disk, and is enforced on the
/// computed byte count rather than on duration, because duration alone says
/// nothing without the rate and the quantisation.
pub const MAX_OUTPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Longest window a job may request \[s\].
///
/// Independent of [`MAX_OUTPUT_BYTES`]: a very low sample rate would otherwise
/// let a job run for days of wall time while staying under the size cap.
pub const MAX_DURATION_S: f64 = 3600.0;

/// Sample-rate bounds \[Hz\]. The floor is a signal-dependent check in
/// [`JobSpec::validate`]; this is the absolute ceiling.
pub const MAX_SAMPLE_RATE_HZ: f64 = 50e6;

/// Seed used when a spec omits one and nothing has assigned a replacement.
const DEFAULT_SEED: u64 = 0x5EED_0000_0000_0001;

/// Sample format written to the binary file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantization {
    Int8,
    Int16,
    Float32,
}

impl Quantization {
    /// Bytes per scalar component (one of I or Q).
    pub const fn bytes_per_component(self) -> usize {
        match self {
            Quantization::Int8 => 1,
            Quantization::Int16 => 2,
            Quantization::Float32 => 4,
        }
    }

    /// Largest representable magnitude, for scaling before rounding.
    ///
    /// Integer formats clip; `Float32` is written unscaled, so its "full
    /// scale" is 1.0 and nothing is ever clipped.
    pub const fn full_scale(self) -> f64 {
        match self {
            Quantization::Int8 => 127.0,
            Quantization::Int16 => 32767.0,
            Quantization::Float32 => 1.0,
        }
    }

    /// Name as written to the sidecar.
    pub const fn id(self) -> &'static str {
        match self {
            Quantization::Int8 => "int8",
            Quantization::Int16 => "int16",
            Quantization::Float32 => "float32",
        }
    }
}

/// Where the receiver is.
///
/// Tagged so that a future `trajectory` variant does not disturb the static
/// case's serialised form.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ReceiverSpec {
    /// Fixed position for the whole window.
    Static {
        latitude_deg: f64,
        longitude_deg: f64,
        altitude_m: f64,
    },
}

impl ReceiverSpec {
    /// Receiver position at an instant.
    ///
    /// Takes a time it currently ignores, on purpose: every call site is
    /// already written to ask per-epoch, so a trajectory variant changes this
    /// function and nothing above it.
    pub fn position_at(&self, _t: GpsTime) -> Geodetic {
        match *self {
            ReceiverSpec::Static {
                latitude_deg,
                longitude_deg,
                altitude_m,
            } => Geodetic::new(latitude_deg, longitude_deg, altitude_m),
        }
    }
}

/// The time window to generate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSpec {
    /// Start of the window, Unix seconds (UTC).
    ///
    /// Unix rather than GPS seconds at the API boundary, matching the
    /// convention the series API already uses; conversion happens once on
    /// ingest.
    pub start_unix_s: f64,
    /// Length of the window \[s\].
    pub duration_s: f64,
    /// Spacing of observable evaluations \[s\].
    ///
    /// Not the sample rate. Geometry is evaluated on this grid and the sample
    /// oscillators are driven between grid points; see [`crate::synth`].
    pub epoch_interval_s: f64,
}

/// Output file parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    /// Complex sample rate \[Hz\].
    pub sample_rate_hz: f64,
    pub quantization: Quantization,
    /// RF frequency mapped to 0 Hz in the output \[Hz\].
    ///
    /// Set it to the signal's carrier for a true baseband file; set it below
    /// the carrier for an IF file, where the signal lands at
    /// `carrier - centre`. Expressing it as an absolute RF frequency rather
    /// than as an offset means the field means the same thing when a second
    /// band exists.
    pub center_frequency_hz: f64,
}

/// Pseudorange noise model selection.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "model", rename_all = "lowercase")]
pub enum PseudorangeNoiseSpec {
    /// Fixed Gaussian sigma, user-supplied.
    Fixed { sigma_m: f64 },
}

/// C/N0 model selection.
///
/// Carries no parameters by design: the elevation model's coefficients are
/// fixed constants of the model, not job inputs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "model", rename_all = "lowercase")]
pub enum Cn0Spec {
    Elevation,
}

/// Both noise model selections.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NoiseSpec {
    pub pseudorange: PseudorangeNoiseSpec,
    pub cn0: Cn0Spec,
}

/// A complete job request.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    /// Constellation to generate. One per job for now; a multi-constellation
    /// job would make this a list, which is why the field is not merged into
    /// a single `signal` string.
    pub constellation: ConstellationSpec,
    pub band: Band,
    pub receiver: ReceiverSpec,
    pub window: WindowSpec,
    pub output: OutputSpec,
    /// Satellites below this are excluded \[deg\].
    pub elevation_mask_deg: f64,
    pub noise: NoiseSpec,
    /// Seed for every random draw in the job.
    ///
    /// `None` means the worker picks one and records it, so a sidecar always
    /// names a seed and a completed job is always reproducible -- including
    /// the ones whose submitter did not think about reproducibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Constellation, in the job schema's own vocabulary.
///
/// A separate enum from [`gnss_core::Constellation`] so the wire format is
/// fixed by this crate rather than by whatever the core enum is called this
/// week, and so a constellation the core can propagate but this crate cannot
/// synthesise is still expressible in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConstellationSpec {
    Gps,
    Galileo,
    Beidou,
    Qzss,
    Sbas,
}

impl From<ConstellationSpec> for Constellation {
    fn from(value: ConstellationSpec) -> Self {
        match value {
            ConstellationSpec::Gps => Constellation::Gps,
            ConstellationSpec::Galileo => Constellation::Galileo,
            ConstellationSpec::Beidou => Constellation::BeiDou,
            ConstellationSpec::Qzss => Constellation::Qzss,
            ConstellationSpec::Sbas => Constellation::Sbas,
        }
    }
}

/// A spec that has passed [`JobSpec::validate`].
///
/// Holds the derived quantities every stage would otherwise recompute, and
/// exists so that those stages can index and multiply without re-checking
/// bounds: if a `ValidatedJob` exists, the numbers in it are consistent.
#[derive(Debug, Clone, Copy)]
pub struct ValidatedJob {
    pub spec: JobSpec,
    pub signal: Signal,
    /// Window start on the GPS timescale.
    pub start: GpsTime,
    /// Number of complex samples in the output.
    pub total_samples: u64,
    /// Number of observable epochs, inclusive of both ends.
    pub epoch_count: usize,
    /// Size of the binary file \[bytes\].
    pub output_bytes: u64,
    /// Frequency the signal's nominal carrier lands at in the output \[Hz\].
    pub intermediate_frequency_hz: f64,
}

impl JobSpec {
    /// Check the request and derive everything downstream needs.
    ///
    /// Every limit here produces a message naming the offending value and the
    /// bound, because these are the errors a user sees most often and "invalid
    /// job" tells them nothing about which field to change.
    pub fn validate(&self) -> Result<ValidatedJob, Error> {
        let constellation = Constellation::from(self.constellation);
        let signal = Signal::resolve(constellation, self.band).ok_or_else(|| {
            Error::UnsupportedSignal {
                requested: format!("{constellation:?} {}", self.band),
            }
        })?;

        let invalid = |reason: String| Error::InvalidJob { reason };

        // Receiver.
        let ReceiverSpec::Static {
            latitude_deg,
            longitude_deg,
            altitude_m,
        } = self.receiver;
        if !(-90.0..=90.0).contains(&latitude_deg) {
            return Err(invalid(format!(
                "latitude {latitude_deg} deg is outside [-90, 90]"
            )));
        }
        if !(-180.0..=180.0).contains(&longitude_deg) {
            return Err(invalid(format!(
                "longitude {longitude_deg} deg is outside [-180, 180]"
            )));
        }
        // Generous, but excludes the values that mean "unset" or "metres
        // confused with kilometres".
        if !(-500.0..=100_000.0).contains(&altitude_m) {
            return Err(invalid(format!(
                "altitude {altitude_m} m is outside [-500, 100000]"
            )));
        }

        // Window.
        if !self.window.start_unix_s.is_finite() {
            return Err(invalid("window start is not a finite timestamp".into()));
        }
        if !(0.0..=MAX_DURATION_S).contains(&self.window.duration_s) || self.window.duration_s <= 0.0
        {
            return Err(invalid(format!(
                "duration {} s is outside (0, {MAX_DURATION_S}]",
                self.window.duration_s
            )));
        }
        if self.window.epoch_interval_s <= 0.0 {
            return Err(invalid(format!(
                "epoch interval {} s must be positive",
                self.window.epoch_interval_s
            )));
        }
        if self.window.epoch_interval_s > self.window.duration_s {
            return Err(invalid(format!(
                "epoch interval {} s exceeds the {} s window",
                self.window.epoch_interval_s, self.window.duration_s
            )));
        }

        // Output.
        let minimum_rate = signal.minimum_sample_rate_hz();
        if !(minimum_rate..=MAX_SAMPLE_RATE_HZ).contains(&self.output.sample_rate_hz) {
            return Err(invalid(format!(
                "sample rate {} Hz is outside [{minimum_rate}, {MAX_SAMPLE_RATE_HZ}] for {signal}",
                self.output.sample_rate_hz
            )));
        }

        // The signal must land inside the sampled band, with room for Doppler.
        // GPS Doppler from a static receiver stays within about +/-5 kHz; 10 kHz
        // of guard is comfortable and still catches a centre frequency chosen
        // for the wrong band, which misses by hundreds of megahertz.
        let intermediate_frequency_hz = signal.carrier_hz() - self.output.center_frequency_hz;
        let nyquist = self.output.sample_rate_hz / 2.0;
        if intermediate_frequency_hz.abs() + 10e3 > nyquist {
            return Err(invalid(format!(
                "centre frequency {} Hz puts {signal} at {:.0} Hz, outside the +/-{:.0} Hz \
                 sampled band",
                self.output.center_frequency_hz, intermediate_frequency_hz, nyquist
            )));
        }

        if !(0.0..90.0).contains(&self.elevation_mask_deg) {
            return Err(invalid(format!(
                "elevation mask {} deg is outside [0, 90)",
                self.elevation_mask_deg
            )));
        }

        let PseudorangeNoiseSpec::Fixed { sigma_m } = self.noise.pseudorange;
        if !sigma_m.is_finite() || sigma_m < 0.0 {
            return Err(invalid(format!(
                "pseudorange sigma {sigma_m} m must be finite and non-negative"
            )));
        }

        let total_samples = (self.window.duration_s * self.output.sample_rate_hz).round() as u64;
        let output_bytes =
            total_samples * 2 * self.output.quantization.bytes_per_component() as u64;
        if output_bytes > MAX_OUTPUT_BYTES {
            return Err(invalid(format!(
                "output would be {:.2} GiB, above the {:.0} GiB limit; shorten the window, \
                 lower the sample rate, or use a narrower sample format",
                output_bytes as f64 / 1024.0f64.powi(3),
                MAX_OUTPUT_BYTES as f64 / 1024.0f64.powi(3)
            )));
        }

        // Inclusive of both ends, so a 10 s window at 1 s gives 11 epochs.
        let epoch_count =
            (self.window.duration_s / self.window.epoch_interval_s).floor() as usize + 1;

        Ok(ValidatedJob {
            spec: *self,
            signal,
            start: GpsTime::from_unix_seconds(self.window.start_unix_s),
            total_samples,
            epoch_count,
            output_bytes,
            intermediate_frequency_hz,
        })
    }
}

impl ValidatedJob {
    /// The seed every random draw in this job derives from.
    ///
    /// Falls back to a fixed constant rather than to entropy when the spec
    /// omits one, so that `validate` stays a pure function. A worker that
    /// wants a fresh seed per job assigns one into the spec before validating,
    /// which also puts it in the sidecar.
    pub fn seed(&self) -> u64 {
        self.spec.seed.unwrap_or(DEFAULT_SEED)
    }

    /// Time of observable epoch `index`.
    pub fn epoch_time(&self, index: usize) -> GpsTime {
        self.start
            .offset_by(index as f64 * self.spec.window.epoch_interval_s)
    }

    /// Receiver position at epoch `index`.
    pub fn receiver_at(&self, index: usize) -> Geodetic {
        self.spec.receiver.position_at(self.epoch_time(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> JobSpec {
        JobSpec {
            constellation: ConstellationSpec::Gps,
            band: Band::L1,
            receiver: ReceiverSpec::Static {
                latitude_deg: 39.7392,
                longitude_deg: -104.9903,
                altitude_m: 1609.0,
            },
            window: WindowSpec {
                start_unix_s: 1_735_732_800.0,
                duration_s: 10.0,
                epoch_interval_s: 0.1,
            },
            output: OutputSpec {
                sample_rate_hz: 2.6e6,
                quantization: Quantization::Int8,
                center_frequency_hz: 1_575.42e6,
            },
            elevation_mask_deg: 5.0,
            noise: NoiseSpec {
                pseudorange: PseudorangeNoiseSpec::Fixed { sigma_m: 1.5 },
                cn0: Cn0Spec::Elevation,
            },
            seed: Some(42),
        }
    }

    #[test]
    fn a_reasonable_job_validates_and_derives_its_sizes() {
        let job = spec().validate().unwrap();
        assert_eq!(job.signal, Signal::GpsL1Ca);
        assert_eq!(job.total_samples, 26_000_000);
        assert_eq!(job.output_bytes, 52_000_000);
        // 10 s at 0.1 s, both ends inclusive.
        assert_eq!(job.epoch_count, 101);
        // Centre frequency equals the carrier, so the signal sits at DC.
        assert_eq!(job.intermediate_frequency_hz, 0.0);
    }

    #[test]
    fn epochs_are_evenly_spaced_from_the_window_start() {
        let job = spec().validate().unwrap();
        assert_eq!(job.epoch_time(0).seconds(), job.start.seconds());
        let last = job.epoch_time(job.epoch_count - 1);
        assert!((last.seconds_since(job.start) - 10.0).abs() < 1e-9);
    }

    /// Sampling below twice the chip rate removes real signal power rather
    /// than merely aliasing noise, so it has to be refused rather than warned
    /// about.
    #[test]
    fn sample_rate_below_the_code_main_lobe_is_refused() {
        let mut spec = spec();
        spec.output.sample_rate_hz = 2.0e6;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("2046000"), "unhelpful message: {error}");
    }

    /// A centre frequency for the wrong band misses by hundreds of MHz and
    /// must be caught, not silently produce an empty spectrum.
    #[test]
    fn a_centre_frequency_outside_the_sampled_band_is_refused() {
        let mut spec = spec();
        spec.output.center_frequency_hz = 1_176.45e6; // L5, not L1
        assert!(spec.validate().is_err());

        // A legitimate few-hundred-kHz IF is fine at this rate.
        let mut offset = self::spec();
        offset.output.center_frequency_hz = 1_575.42e6 - 500e3;
        assert!(offset.validate().is_ok());
    }

    #[test]
    fn oversized_output_is_refused_with_a_size_in_the_message() {
        let mut spec = spec();
        spec.window.duration_s = 3000.0;
        spec.output.sample_rate_hz = 20e6;
        spec.output.quantization = Quantization::Int16;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("GiB"), "unhelpful message: {error}");
    }

    #[test]
    fn unimplemented_signals_name_what_was_asked_for() {
        let mut spec = spec();
        spec.band = Band::L5;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("L5"), "unhelpful message: {error}");

        let mut galileo = self::spec();
        galileo.constellation = ConstellationSpec::Galileo;
        galileo.band = Band::E1;
        assert!(galileo.validate().is_err());
    }

    /// The serialised form is a stored artefact -- job records and sidecars
    /// both hold it -- so its shape is a contract. In particular the tagged
    /// enums must round-trip through their `kind`/`model` discriminators.
    #[test]
    fn the_wire_format_round_trips_and_uses_tagged_enums() {
        let json = serde_json::to_value(spec()).unwrap();

        assert_eq!(json["constellation"], "gps");
        assert_eq!(json["band"], "l1");
        assert_eq!(json["receiver"]["kind"], "static");
        assert_eq!(json["output"]["quantization"], "int8");
        assert_eq!(json["noise"]["pseudorange"]["model"], "fixed");
        assert_eq!(json["noise"]["pseudorange"]["sigma_m"], 1.5);
        assert_eq!(json["noise"]["cn0"]["model"], "elevation");
        // The C/N0 model must not have acquired user-facing coefficients.
        assert!(json["noise"]["cn0"]["cn0_max"].is_null());

        let back: JobSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back, spec());
    }

    /// An omitted seed is valid and means "choose one"; the worker records
    /// what it chose.
    #[test]
    fn the_seed_is_optional_and_omitted_when_absent() {
        let mut spec = spec();
        spec.seed = None;
        assert!(spec.validate().is_ok());
        let json = serde_json::to_value(spec).unwrap();
        assert!(json.get("seed").is_none());
    }

    #[test]
    fn out_of_range_receiver_and_window_values_are_refused() {
        /// A named mutation that should make the spec invalid.
        type Case = (&'static str, Box<dyn Fn(&mut JobSpec)>);

        let cases: Vec<Case> = vec![
            (
                "latitude",
                Box::new(|s: &mut JobSpec| {
                    s.receiver = ReceiverSpec::Static {
                        latitude_deg: 91.0,
                        longitude_deg: 0.0,
                        altitude_m: 0.0,
                    }
                }),
            ),
            ("duration", Box::new(|s: &mut JobSpec| s.window.duration_s = 0.0)),
            (
                "epoch interval",
                Box::new(|s: &mut JobSpec| s.window.epoch_interval_s = -1.0),
            ),
            (
                "interval past window",
                Box::new(|s: &mut JobSpec| s.window.epoch_interval_s = 20.0),
            ),
            (
                "mask",
                Box::new(|s: &mut JobSpec| s.elevation_mask_deg = 90.0),
            ),
            (
                "sigma",
                Box::new(|s: &mut JobSpec| {
                    s.noise.pseudorange = PseudorangeNoiseSpec::Fixed { sigma_m: -1.0 }
                }),
            ),
        ];

        for (name, mutate) in cases {
            let mut spec = spec();
            mutate(&mut spec);
            assert!(spec.validate().is_err(), "{name} should have been refused");
        }
    }
}
