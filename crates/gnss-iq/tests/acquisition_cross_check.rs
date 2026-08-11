//! Validation of the generated recording by acquiring it.
//!
//! `CLAUDE.md` sets the standard: nothing is switched on until an independent
//! cross-check appropriate to its geometry exists. For a synthesised signal
//! the appropriate check is a *receiver*, because acquisition inverts
//! synthesis by a completely different route -- a frequency-domain circular
//! correlation over a locally regenerated code, rather than a per-sample
//! forward construction. Nothing in this file calls `synth`, and the only
//! thing it shares with the generator is the C/A code itself.
//!
//! What that pins down, all at once:
//!
//! - **Code phase** must come back at the delay implied by the pseudorange the
//!   observables stage computed. This is the check that the whole chain --
//!   propagation, clock correction, atmosphere, light-time solution, the
//!   chip-indexing arithmetic -- is self-consistent. A sign error anywhere in
//!   it moves the peak by kilometres.
//! - **Doppler** must come back at the frequency the observables stage
//!   reported, which it derived analytically from the satellite's velocity.
//!   The synthesiser never used that number: it gets Doppler implicitly from
//!   the rate of change of the range. Two independent derivations agreeing is
//!   the point.
//! - **C/N0** must come back at what the noise model commanded, which pins the
//!   amplitude scaling and the noise variance together.
//!
//! Runs in release; a debug build spends minutes in the FFT.

use std::f64::consts::TAU;
use std::path::PathBuf;

use gnss_iq::code::SpreadingCode;
use gnss_iq::ephemeris::LocalFileSource;
use gnss_iq::job::{
    Cn0Spec, ConstellationSpec, JobSpec, NoiseSpec, OutputSpec, PseudorangeNoiseSpec, Quantization,
    ReceiverSpec, WindowSpec,
};
use gnss_iq::observables::Observables;
use gnss_iq::pipeline::run_job;
use gnss_iq::signal::{Band, Signal};

/// 4.096 MHz gives exactly 4096 samples per 1 ms code period -- a power of two,
/// so the correlation is one radix-2 FFT with no zero padding and no
/// resampling, and one sample is exactly a quarter of a chip.
const SAMPLE_RATE_HZ: f64 = 4.096e6;
const SAMPLES_PER_CODE: usize = 4096;

/// Non-coherent accumulations.
///
/// Ten rather than a handful because of the negative control: the search runs
/// over 4096 code phases times 121 Doppler bins, and the largest of half a
/// million noise trials sits well above the mean. Accumulating longer lifts
/// real peaks without lifting that maximum much, which is what separates an
/// absent satellite from a present one.
const BLOCKS: usize = 10;

/// Doppler search grid \[Hz\]. A 1 ms coherent integration has a null-to-null
/// width near 1 kHz, so 100 Hz steps resolve the peak well.
const DOPPLER_RANGE_HZ: f64 = 6000.0;
const DOPPLER_STEP_HZ: f64 = 100.0;

/// How far the acquired code phase may sit from the pseudorange the
/// observables stage computed \[samples\].
///
/// A quarter sample is 18 m here. Measured residuals across the sky run 0.5 to
/// 11 m, dominated by the code phase drifting during the 10 ms non-coherent
/// accumulation while the truth is evaluated at the first sample -- an
/// artefact of the measurement, not of the recording.
///
/// This number was 50 m until the generator stopped computing chip phase as
/// `gps_seconds * chip_rate`, a product near 1.5e15 where one f64 ulp is a
/// quarter of a chip. That bug is exactly what this test exists to find, and
/// the tolerance is deliberately tight enough to catch its return.
const CODE_PHASE_TOLERANCE_SAMPLES: f64 = 0.25;

// ---------------------------------------------------------------------------
// Minimal complex FFT
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
    fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    fn power(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// In-place iterative radix-2 Cooley-Tukey. `inverse` scales by 1/N.
fn fft(buffer: &mut [Complex], inverse: bool) {
    let n = buffer.len();
    assert!(n.is_power_of_two());

    // Bit-reversal permutation.
    let mut target = 0usize;
    for source in 1..n {
        let mut bit = n >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
        if source < target {
            buffer.swap(source, target);
        }
    }

    let mut length = 2;
    while length <= n {
        let sign = if inverse { 1.0 } else { -1.0 };
        let angle = sign * TAU / length as f64;
        for start in (0..n).step_by(length) {
            for offset in 0..length / 2 {
                let twiddle = Complex::new(
                    (angle * offset as f64).cos(),
                    (angle * offset as f64).sin(),
                );
                let even = buffer[start + offset];
                let odd = buffer[start + offset + length / 2].mul(twiddle);
                buffer[start + offset] = Complex::new(even.re + odd.re, even.im + odd.im);
                buffer[start + offset + length / 2] =
                    Complex::new(even.re - odd.re, even.im - odd.im);
            }
        }
        length <<= 1;
    }

    if inverse {
        let scale = 1.0 / n as f64;
        for value in buffer.iter_mut() {
            value.re *= scale;
            value.im *= scale;
        }
    }
}

// ---------------------------------------------------------------------------
// Acquisition
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Acquisition {
    /// Peak position in samples, with sub-sample interpolation.
    code_phase_samples: f64,
    doppler_hz: f64,
    cn0_db_hz: f64,
}

/// Parallel code-phase search over one satellite.
fn acquire(samples: &[Complex], prn: u8) -> Acquisition {
    // Local replica: one code period sampled at the recording's rate.
    let code = SpreadingCode::generate(Signal::GpsL1Ca, prn).expect("valid PRN");
    let chips_per_sample = Signal::GpsL1Ca.chip_rate_hz() / SAMPLE_RATE_HZ;
    let mut replica: Vec<Complex> = (0..SAMPLES_PER_CODE)
        .map(|n| {
            let chip = (n as f64 * chips_per_sample).floor() as usize;
            Complex::new(f64::from(code.chip(chip)), 0.0)
        })
        .collect();
    fft(&mut replica, false);
    let replica: Vec<Complex> = replica.iter().map(|c| c.conj()).collect();

    let mut best = Acquisition {
        code_phase_samples: 0.0,
        doppler_hz: 0.0,
        cn0_db_hz: f64::MIN,
    };
    let mut best_power = f64::MIN;
    let mut best_profile = Vec::new();

    let steps = (2.0 * DOPPLER_RANGE_HZ / DOPPLER_STEP_HZ) as i32;
    for step in -steps / 2..=steps / 2 {
        let doppler = f64::from(step) * DOPPLER_STEP_HZ;

        // Non-coherent accumulation across blocks: immune to the data-bit
        // transitions that would cancel a longer coherent integration.
        let mut profile = vec![0.0f64; SAMPLES_PER_CODE];
        for block in 0..BLOCKS {
            let offset = block * SAMPLES_PER_CODE;
            let mut mixed: Vec<Complex> = (0..SAMPLES_PER_CODE)
                .map(|n| {
                    let t = (offset + n) as f64 / SAMPLE_RATE_HZ;
                    let phase = -TAU * doppler * t;
                    samples[offset + n].mul(Complex::new(phase.cos(), phase.sin()))
                })
                .collect();

            fft(&mut mixed, false);
            for (value, reference) in mixed.iter_mut().zip(&replica) {
                *value = value.mul(*reference);
            }
            fft(&mut mixed, true);

            for (slot, value) in profile.iter_mut().zip(&mixed) {
                *slot += value.power();
            }
        }

        let (index, &power) = profile
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();

        if power > best_power {
            best_power = power;
            best.doppler_hz = doppler;
            best.code_phase_samples = interpolate_peak(&profile, index);
            best_profile = profile;
        }
    }

    best.cn0_db_hz = estimate_cn0(&best_profile);
    best
}

/// Refine the peak position from its two neighbours.
///
/// A BPSK autocorrelation peak is a *triangle*, not a parabola, so the usual
/// three-point parabolic fit is systematically biased -- it was worth about
/// 0.7 samples here, which at four samples per chip is 50 m of apparent range
/// error that the recording does not actually contain.
///
/// For an ideal triangle sampled at `L`, `C`, `R` with the true peak offset
/// `d` from the centre sample, the slopes give `d` exactly:
///
/// ```text
/// d >= 0  =>  (R - L) / (C - L) = 2d
/// d <  0  =>  (R - L) / (C - R) = 2d
/// ```
///
/// The correlation is a triangle convolved with the sample aperture rather
/// than a pure triangle, so this is not exact either, but it is unbiased to
/// first order where the parabola is not.
fn interpolate_peak(profile: &[f64], index: usize) -> f64 {
    let n = profile.len();
    // Amplitudes, not powers: the triangle is linear in amplitude.
    let left = profile[(index + n - 1) % n].sqrt();
    let centre = profile[index].sqrt();
    let right = profile[(index + 1) % n].sqrt();

    let denominator = if right > left {
        centre - left
    } else {
        centre - right
    };
    let shift = if denominator.abs() < 1e-12 {
        0.0
    } else {
        0.5 * (right - left) / denominator
    };
    index as f64 + shift.clamp(-0.5, 0.5)
}

/// C/N0 from the ratio of the correlation peak to the noise floor around it.
///
/// The standard single-trial estimator: with `M` non-coherent accumulations of
/// coherent time `T`, the peak carries signal plus noise and every other bin
/// carries noise alone, so `(peak/floor - 1) / T` is the carrier-to-noise
/// density.
fn estimate_cn0(profile: &[f64]) -> f64 {
    let (peak_index, &peak) = profile
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();

    // Exclude the correlation triangle around the peak, which spans about two
    // chips -- eight samples here -- from the noise-floor estimate.
    const EXCLUSION_SAMPLES: usize = 16;
    let n = profile.len();
    let floor_samples: Vec<f64> = profile
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            let distance = (*index as isize - peak_index as isize).unsigned_abs();
            distance.min(n - distance) > EXCLUSION_SAMPLES
        })
        .map(|(_, &value)| value)
        .collect();

    let floor = floor_samples.iter().sum::<f64>() / floor_samples.len() as f64;
    let coherent_time = SAMPLES_PER_CODE as f64 / SAMPLE_RATE_HZ;
    10.0 * ((peak / floor - 1.0) / coherent_time).log10()
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn fixture_source() -> LocalFileSource {
    LocalFileSource::new([PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/BRDC00WRD_R_20250010000_01D_GN.rnx")])
}

fn spec(quantization: Quantization, sigma_m: f64) -> JobSpec {
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
            duration_s: 0.02,
            epoch_interval_s: 0.01,
        },
        output: OutputSpec {
            sample_rate_hz: SAMPLE_RATE_HZ,
            quantization,
            center_frequency_hz: 1_575.42e6,
        },
        elevation_mask_deg: 10.0,
        noise: NoiseSpec {
            pseudorange: PseudorangeNoiseSpec::Fixed { sigma_m },
            cn0: Cn0Spec::Elevation,
        },
        seed: Some(20_250_101),
    }
}

struct Recording {
    samples: Vec<Complex>,
    observables: Observables,
    directory: PathBuf,
}

impl Drop for Recording {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).ok();
    }
}

fn generate(spec: &JobSpec, label: &str) -> Recording {
    let directory = std::env::temp_dir().join(format!(
        "gnss-iq-acq-{label}-{}",
        std::process::id()
    ));
    let artifacts = run_job(label, spec, &fixture_source(), &directory, &mut |_| {}).unwrap();

    let raw = std::fs::read(&artifacts.binary_path).unwrap();
    let scale = artifacts.summary.scale_factor;

    let samples = match spec.output.quantization {
        Quantization::Int8 => raw
            .chunks_exact(2)
            .map(|pair| {
                Complex::new(
                    f64::from(pair[0] as i8) / scale,
                    f64::from(pair[1] as i8) / scale,
                )
            })
            .collect(),
        Quantization::Int16 => raw
            .chunks_exact(4)
            .map(|chunk| {
                Complex::new(
                    f64::from(i16::from_le_bytes([chunk[0], chunk[1]])) / scale,
                    f64::from(i16::from_le_bytes([chunk[2], chunk[3]])) / scale,
                )
            })
            .collect(),
        Quantization::Float32 => raw
            .chunks_exact(8)
            .map(|chunk| {
                Complex::new(
                    f64::from(f32::from_le_bytes(chunk[0..4].try_into().unwrap())),
                    f64::from(f32::from_le_bytes(chunk[4..8].try_into().unwrap())),
                )
            })
            .collect(),
    };

    Recording {
        samples,
        observables: artifacts.observables,
        directory,
    }
}

/// Code phase the recording *should* have at sample zero, in samples.
///
/// The generator places code chip `(t_sv * chip_rate) mod 1023` at receiver
/// time `t`, where `t_sv = t - rho/c`. The correlation peaks at the lag `k`
/// where the local replica -- which starts at chip 0 -- aligns, so
/// `k = -phase_chips * samples_per_chip`, reduced into `[0, N)`.
/// Whole seconds are dropped before the multiplication for the same reason the
/// generator drops them: `start_gps_s * chip_rate` is about 1.5e15, where one
/// f64 ulp is a quarter of a chip -- 73 m. Computing the truth naively would
/// quantise it exactly as badly as the bug this test exists to catch, and the
/// two errors would partially cancel.
fn expected_code_phase_samples(pseudorange_m: f64, start_gps_s: f64) -> f64 {
    let chip_rate = Signal::GpsL1Ca.chip_rate_hz();
    // One second is exactly 1000 C/A code periods, so whole seconds carry no
    // code phase.
    let start_fraction = start_gps_s - start_gps_s.floor();
    let satellite_time =
        start_fraction - pseudorange_m / gnss_core::constants::SPEED_OF_LIGHT;
    let phase_chips = (satellite_time * chip_rate).rem_euclid(1023.0);
    let samples_per_chip = SAMPLE_RATE_HZ / chip_rate;
    (-phase_chips * samples_per_chip).rem_euclid(SAMPLES_PER_CODE as f64)
}

/// The pseudorange the accumulated correlation is effectively centred on.
///
/// Each block is exactly one code period, so the elapsed time between block
/// starts advances the code phase by a whole number of cycles and contributes
/// nothing. The *only* thing that moves the peak from block to block is the
/// range, which means the accumulated profile sits at the mean range over the
/// block start times -- 0 to 9 ms, so a centre of 4.5 ms.
///
/// Correcting for this matters most when the range is moving fast, which
/// injected pseudorange noise makes it do: a 30 m sigma redrawn every 10 ms
/// slews the code range far quicker than orbital motion does.
fn pseudorange_at_accumulation_centre(observables: &Observables, sv: gnss_core::Sv) -> f64 {
    let code_period = Signal::GpsL1Ca.code_period_s();
    let centre_s = (BLOCKS as f64 - 1.0) / 2.0 * code_period;

    let first = observables.epochs[0]
        .satellite(sv)
        .expect("satellite visible at the first epoch");
    let Some(second) = observables.epochs.get(1).and_then(|e| e.satellite(sv)) else {
        return first.pseudorange_m;
    };

    let interval = observables.epochs[1]
        .time
        .seconds_since(observables.epochs[0].time);
    let fraction = centre_s / interval;
    first.pseudorange_m + (second.pseudorange_m - first.pseudorange_m) * fraction
}

/// Circular difference in samples.
fn circular_error(measured: f64, expected: f64) -> f64 {
    let raw = (measured - expected).rem_euclid(SAMPLES_PER_CODE as f64);
    raw.min(SAMPLES_PER_CODE as f64 - raw)
}

/// The headline check: acquire every satellite in a noise-free-code recording
/// and confirm code phase, Doppler and C/N0 all come back at their truth.
#[test]
fn acquisition_recovers_code_phase_doppler_and_cn0() {
    // Zero pseudorange sigma so the code phase has one unambiguous truth to
    // compare against. The thermal noise floor is still fully present -- it is
    // what the C/N0 assertion is measured against.
    let recording = generate(&spec(Quantization::Float32, 0.0), "truth");
    let first_epoch = &recording.observables.epochs[0];
    let start_gps_s = first_epoch.time.seconds();

    assert!(
        first_epoch.satellites.len() >= 6,
        "only {} satellites to check",
        first_epoch.satellites.len()
    );

    for satellite in &first_epoch.satellites {
        let result = acquire(&recording.samples, satellite.sv.prn);

        let expected_phase = expected_code_phase_samples(
            pseudorange_at_accumulation_centre(&recording.observables, satellite.sv),
            start_gps_s,
        );
        let phase_error = circular_error(result.code_phase_samples, expected_phase);

        assert!(
            phase_error < CODE_PHASE_TOLERANCE_SAMPLES,
            "{}: code phase {:.3} samples, expected {:.3} (error {:.3} samples, {:.1} m)",
            satellite.sv,
            result.code_phase_samples,
            expected_phase,
            phase_error,
            phase_error * gnss_core::constants::SPEED_OF_LIGHT / SAMPLE_RATE_HZ
        );

        // Doppler within one and a half search bins of the analytically
        // derived value: the truth is generally not on the grid, so a full bin
        // of quantisation is expected and half a bin of margin keeps a truth
        // sitting near a bin edge from failing spuriously.
        let doppler_error = (result.doppler_hz - satellite.doppler_hz).abs();
        assert!(
            doppler_error <= 1.5 * DOPPLER_STEP_HZ,
            "{}: acquired Doppler {:.0} Hz, observables say {:.1} Hz",
            satellite.sv,
            result.doppler_hz,
            satellite.doppler_hz
        );

        // Measured C/N0 runs about a decibel below commanded, consistently.
        // That is not a scaling error: the recording is point-sampled rather
        // than band-limited, so the correlation loses a few tenths of a dB to
        // sub-sample code misalignment and to spectrum aliased outside the
        // main lobe, and one block in twenty straddles a navigation bit edge.
        // A genuine amplitude or noise-variance mistake shows up as many
        // decibels, not one.
        let cn0_error = (result.cn0_db_hz - satellite.cn0_db_hz).abs();
        assert!(
            cn0_error < 2.5,
            "{}: measured {:.1} dB-Hz, commanded {:.1} dB-Hz",
            satellite.sv,
            result.cn0_db_hz,
            satellite.cn0_db_hz
        );
    }
}

/// Pseudorange noise must actually reach the code phase, and at the size
/// requested. This is the counterpart to the unit test that checks the model
/// output: here the draw is measured back out of the finished waveform.
#[test]
fn injected_pseudorange_noise_moves_the_acquired_code_phase() {
    let sigma_m = 30.0;
    let recording = generate(&spec(Quantization::Float32, sigma_m), "noisy");
    let first_epoch = &recording.observables.epochs[0];
    let start_gps_s = first_epoch.time.seconds();

    let mut errors_vs_clean = Vec::new();

    for satellite in &first_epoch.satellites {
        let result = acquire(&recording.samples, satellite.sv.prn);

        // Truth including the draw: the peak must follow the noisy range.
        let expected = expected_code_phase_samples(
            pseudorange_at_accumulation_centre(&recording.observables, satellite.sv),
            start_gps_s,
        );
        assert!(
            circular_error(result.code_phase_samples, expected) < CODE_PHASE_TOLERANCE_SAMPLES,
            "{}: peak did not follow the noisy pseudorange",
            satellite.sv
        );

        // And it must differ from where the clean range would have put it, by
        // about the draw.
        let clean = expected_code_phase_samples(satellite.carrier_range_m, start_gps_s);
        let offset_samples = circular_error(result.code_phase_samples, clean);
        errors_vs_clean
            .push(offset_samples * gnss_core::constants::SPEED_OF_LIGHT / SAMPLE_RATE_HZ);
    }

    // Mean absolute deviation of a zero-mean Gaussian is sigma*sqrt(2/pi),
    // about 0.8 sigma. With a handful of satellites the spread is wide, so
    // this is a loose bracket -- it is checking that the noise is there and
    // roughly the right size, not estimating sigma.
    let mean_offset = errors_vs_clean.iter().sum::<f64>() / errors_vs_clean.len() as f64;
    assert!(
        (0.2 * sigma_m..3.0 * sigma_m).contains(&mean_offset),
        "mean code-phase offset {mean_offset:.1} m for a requested sigma of {sigma_m} m"
    );
}

/// The integer formats must carry the same signal as the float reference.
/// This is what pins the scale factor recorded in the sidecar: dividing by it
/// has to recover the same waveform the float file holds directly.
#[test]
fn quantised_recordings_acquire_the_same_as_the_float_reference() {
    let reference = generate(&spec(Quantization::Float32, 0.0), "ref");
    let start_gps_s = reference.observables.epochs[0].time.seconds();

    for quantization in [Quantization::Int8, Quantization::Int16] {
        let recording = generate(&spec(quantization, 0.0), &format!("{quantization:?}"));
        assert_eq!(recording.samples.len(), reference.samples.len());

        for satellite in &reference.observables.epochs[0].satellites {
            let result = acquire(&recording.samples, satellite.sv.prn);
            let expected = expected_code_phase_samples(
                pseudorange_at_accumulation_centre(&reference.observables, satellite.sv),
                start_gps_s,
            );

            assert!(
                circular_error(result.code_phase_samples, expected) < CODE_PHASE_TOLERANCE_SAMPLES,
                "{:?} {}: code phase off by {:.2} samples",
                quantization,
                satellite.sv,
                circular_error(result.code_phase_samples, expected)
            );
            assert!(
                (result.cn0_db_hz - satellite.cn0_db_hz).abs() < 2.5,
                "{:?} {}: {:.1} dB-Hz vs commanded {:.1}",
                quantization,
                satellite.sv,
                result.cn0_db_hz,
                satellite.cn0_db_hz
            );
        }
    }
}

/// A satellite that is not in the recording must not acquire. Without this,
/// every assertion above could be satisfied by a correlator that peaks on
/// noise -- this is the negative control.
#[test]
fn satellites_absent_from_the_recording_do_not_acquire() {
    let recording = generate(&spec(Quantization::Float32, 0.0), "absent");
    let present: Vec<u8> = recording.observables.epochs[0]
        .satellites
        .iter()
        .map(|s| s.sv.prn)
        .collect();

    let absent: Vec<u8> = (1..=32u8).filter(|prn| !present.contains(prn)).collect();
    assert!(!absent.is_empty(), "every PRN was visible; no control available");

    let weakest_present = recording.observables.epochs[0]
        .satellites
        .iter()
        .map(|s| s.cn0_db_hz)
        .fold(f64::MAX, f64::min);

    for prn in absent {
        let result = acquire(&recording.samples, prn);
        assert!(
            result.cn0_db_hz < weakest_present - 6.0,
            "PRN {prn} is not in the recording but acquired at {:.1} dB-Hz, \
             against a weakest real satellite of {:.1} dB-Hz",
            result.cn0_db_hz,
            weakest_present
        );
    }
}
