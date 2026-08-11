//! Baseband IQ synthesis.
//!
//! Turns the per-epoch observables into a stream of complex samples: for each
//! satellite, a spreading code and navigation data stream carried on a
//! Doppler-shifted carrier, summed, buried in thermal noise at the commanded
//! C/N0, and quantised.
//!
//! ## Delay drives everything
//!
//! There is no separate Doppler oscillator. Both the code phase and the
//! carrier phase are derived from the range, which is interpolated between
//! observable epochs:
//!
//! ```text
//! t_sv  = t - rho_code(t) / c              satellite time being received
//! code  = code[(t_sv * chip_rate) mod N]
//! data  = lnav_bit(t_sv)
//! phase = 2*pi*f_if*t - 2*pi*f_carrier*rho_carrier(t)/c
//! ```
//!
//! Differentiating that phase gives `-f_carrier * d(rho)/dt / c`, which *is*
//! the Doppler shift -- so it arrives correct without being computed, and it
//! cannot disagree with the code rate, because both come from the same delay.
//! A synthesiser with independent code and carrier oscillators has to keep
//! them consistent by hand, and code-carrier divergence is the classic bug
//! when it fails.
//!
//! `rho_code` carries the pseudorange noise draw and `rho_carrier` does not;
//! see [`crate::observables`] for why.
//!
//! ## Amplitude and the noise floor
//!
//! Complex noise is generated with unit variance per component, so
//! `E[|n|^2] = 2` and the one-sided noise density over a complex sample rate
//! `fs` is `N0 = 2/fs`. A satellite at `C/N0` therefore needs amplitude
//! `A = sqrt(2 * 10^(cn0/10) / fs)`, which is what
//! [`amplitude_for_cn0`] returns. Setting the noise rather than the signal to
//! unity is deliberate: it keeps the noise floor identical across jobs, so two
//! recordings at different sample rates remain comparable.

use std::io::Write;

use gnss_core::constants::SPEED_OF_LIGHT;
use gnss_core::Sv;

use crate::code::SpreadingCode;
use crate::job::{Quantization, ValidatedJob};
use crate::lnav::{LnavGenerator, NavBitSource};
use crate::observables::Observables;
use crate::rng::Pcg32;
use crate::Error;

/// Where the total RMS is placed relative to full scale.
///
/// Eight means clipping starts at eight sigma, which for a Gaussian is
/// effectively never, while still using four of the eight bits of an int8
/// file. GNSS front ends routinely quantise far harder than this -- one or two
/// bits is normal -- but a file that a user may want to inspect, filter or
/// re-quantise themselves should not arrive pre-damaged.
const FULL_SCALE_SIGMA_HEADROOM: f64 = 8.0;

/// Stream separation for the thermal-noise generator.
///
/// Distinct from the per-satellite pseudorange streams, which are keyed by
/// PRN (1..32), so the two cannot collide and produce correlated noise.
const THERMAL_NOISE_STREAM: u64 = 1_000;

/// Signal amplitude for a given carrier-to-noise-density ratio.
///
/// See the module comment for the derivation.
pub fn amplitude_for_cn0(cn0_db_hz: f64, sample_rate_hz: f64) -> f64 {
    let cn0_linear = 10f64.powf(cn0_db_hz / 10.0);
    (2.0 * cn0_linear / sample_rate_hz).sqrt()
}

/// What a synthesis run produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthesisSummary {
    pub samples_written: u64,
    /// Multiplier applied before quantisation, so absolute units can be
    /// recovered from the file.
    pub scale_factor: f64,
    /// Samples whose I or Q saturated the output format.
    ///
    /// Should be zero in normal operation; a non-zero count means the headroom
    /// estimate was wrong and the recording is distorted, which the sidecar
    /// reports rather than hides.
    pub clipped_samples: u64,
    /// Peak instantaneous magnitude seen, in pre-scaling units.
    pub peak_magnitude: f64,
}

/// Progress notification: fraction of samples written, in `[0, 1]`.
pub type ProgressFn<'a> = &'a mut dyn FnMut(f64);

/// One satellite's synthesis state.
struct SatelliteChannel {
    sv: Sv,
    code: SpreadingCode,
    nav: NavBitSource,
}

/// Synthesise the whole window into `writer`.
pub fn synthesize<W: Write>(
    job: &ValidatedJob,
    observables: &Observables,
    ephemeris_for: &dyn Fn(Sv) -> Option<gnss_core::KeplerianEphemeris>,
    writer: &mut W,
    progress: ProgressFn<'_>,
) -> Result<SynthesisSummary, Error> {
    let signal = job.signal;
    let sample_rate = job.spec.output.sample_rate_hz;
    let quantization = job.spec.output.quantization;
    let epoch_interval = job.spec.window.epoch_interval_s;
    let code_length = f64::from(signal.code_length_chips());
    let chip_rate = signal.chip_rate_hz();
    let carrier_hz = signal.carrier_hz();
    let intermediate_hz = job.intermediate_frequency_hz;
    let start_gps_s = job.start.seconds();

    // Code phase must be computed from a *small* time value.
    //
    // GPS seconds are around 1.4e9, and multiplying that by a 1.023e6 chip
    // rate gives ~1.5e15 -- where one f64 ulp is a quarter of a chip, or 73 m
    // of range. Reducing modulo the code length afterwards does not recover
    // what has already been lost, so the reduction has to happen first.
    //
    // Splitting off whole seconds is exact and costs nothing, because one
    // second is exactly 1000 code periods: an integer number of seconds
    // contributes an integer number of full code cycles and therefore no
    // phase at all. The same argument covers the navigation bit rate, 50 Hz
    // being a whole number of bits per second -- but the LNAV generator needs
    // absolute time anyway, to put the right TOW in the handover word, and its
    // precision requirement is fifteen orders of magnitude looser.
    debug_assert_eq!(
        signal.code_period_s() * 1000.0,
        1.0,
        "the whole-second reduction below assumes a code period that divides one second"
    );
    let start_whole_seconds = start_gps_s.floor();
    let start_fraction_s = start_gps_s - start_whole_seconds;

    // One channel per satellite that is ever visible. Built once: the C/A code
    // is 1023 chips and regenerating it per epoch would dominate the run.
    let mut channels: Vec<SatelliteChannel> = Vec::new();
    for &sv in &observables.satellites {
        let Some(code) = SpreadingCode::generate(signal, sv.prn) else {
            // A PRN with no assigned code cannot be transmitted. Skipping is
            // right -- it is not an error in the job, it is a satellite this
            // signal definition does not cover.
            continue;
        };
        let Some(ephemeris) = ephemeris_for(sv) else {
            continue;
        };
        channels.push(SatelliteChannel {
            sv,
            code,
            nav: NavBitSource::new(LnavGenerator::new(ephemeris)),
        });
    }

    if channels.is_empty() {
        return Err(Error::NoSatellitesVisible {
            mask_deg: job.spec.elevation_mask_deg,
        });
    }

    // Scale factor from the strongest epoch, so the whole file shares one
    // scaling and a consumer can convert back to absolute units with a single
    // constant from the sidecar.
    let scale_factor = compute_scale_factor(job, observables, quantization);

    let mut noise = Pcg32::seed(job.seed(), THERMAL_NOISE_STREAM);
    let mut writer = std::io::BufWriter::with_capacity(1 << 20, writer);

    let mut samples_written = 0u64;
    let mut clipped_samples = 0u64;
    let mut peak_magnitude = 0.0f64;
    let mut byte_buffer: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut last_reported = 0.0;

    // Walk interval by interval so the per-satellite endpoint lookups happen
    // once per epoch rather than once per sample.
    let interval_count = job.epoch_count.saturating_sub(1).max(1);

    for interval in 0..interval_count {
        let left = &observables.epochs[interval.min(job.epoch_count - 1)];
        let right = &observables.epochs[(interval + 1).min(job.epoch_count - 1)];

        // Satellites present at both ends of the interval. One missing at
        // either end has risen or set inside it; excluding it costs at most
        // one epoch of visibility and avoids interpolating a range that does
        // not exist at one end.
        let active: Vec<(usize, EndpointPair)> = channels
            .iter()
            .enumerate()
            .filter_map(|(channel_index, channel)| {
                let start = left.satellite(channel.sv)?;
                let end = right.satellite(channel.sv)?;
                Some((
                    channel_index,
                    EndpointPair {
                        code_range: (start.pseudorange_m, end.pseudorange_m),
                        carrier_range: (start.carrier_range_m, end.carrier_range_m),
                        amplitude: (
                            amplitude_for_cn0(start.cn0_db_hz, sample_rate),
                            amplitude_for_cn0(end.cn0_db_hz, sample_rate),
                        ),
                    },
                ))
            })
            .collect();

        let first_sample = (interval as f64 * epoch_interval * sample_rate).round() as u64;
        let last_sample = if interval + 1 == interval_count {
            // Final block absorbs any samples past the last epoch. A window
            // whose duration is not a whole number of epoch intervals leaves
            // under one interval over; carrying the linear fit past its right
            // endpoint is exactly as accurate there as interpolating inside it.
            job.total_samples
        } else {
            (((interval + 1) as f64) * epoch_interval * sample_rate).round() as u64
        };

        for sample_index in first_sample..last_sample {
            let elapsed_s = sample_index as f64 / sample_rate;
            let absolute_gps_s = start_gps_s + elapsed_s;
            let fraction = (elapsed_s - interval as f64 * epoch_interval) / epoch_interval;

            let mut in_phase = 0.0f64;
            let mut quadrature = 0.0f64;

            for (channel_index, endpoints) in &active {
                let channel = &mut channels[*channel_index];

                let code_range = lerp(endpoints.code_range, fraction);
                let carrier_range = lerp(endpoints.carrier_range, fraction);
                let amplitude = lerp(endpoints.amplitude, fraction);

                // Satellite time whose transmission is arriving now. The
                // reduced form drops whole seconds, which the code phase is
                // invariant to; the absolute form is what the navigation
                // message is indexed by.
                let propagation_delay_s = code_range / SPEED_OF_LIGHT;
                let satellite_time = absolute_gps_s - propagation_delay_s;
                let reduced_satellite_time =
                    start_fraction_s + elapsed_s - propagation_delay_s;

                let chip_position = (reduced_satellite_time * chip_rate).rem_euclid(code_length);
                let chip = channel.code.chip(chip_position as usize);
                let data_bit = channel.nav.bit_at(satellite_time);

                // Carrier phase: the IF ramp, less the phase accumulated over
                // the propagation delay. Reduced modulo a turn before the
                // trigonometry, because f_carrier * range / c is of order 1e8
                // turns and sin() of that has lost most of its precision.
                let delay_turns = carrier_hz * carrier_range / SPEED_OF_LIGHT;
                let turns = (intermediate_hz * elapsed_s - delay_turns).rem_euclid(1.0);
                let (sin_phase, cos_phase) = (std::f64::consts::TAU * turns).sin_cos();

                let magnitude = amplitude * f64::from(chip) * f64::from(data_bit);
                in_phase += magnitude * cos_phase;
                quadrature += magnitude * sin_phase;
            }

            in_phase += noise.next_normal();
            quadrature += noise.next_normal();

            peak_magnitude =
                peak_magnitude.max((in_phase * in_phase + quadrature * quadrature).sqrt());

            let clipped = write_sample(
                &mut byte_buffer,
                in_phase * scale_factor,
                quadrature * scale_factor,
                quantization,
            );
            clipped_samples += u64::from(clipped);
            samples_written += 1;

            if byte_buffer.len() >= (1 << 16) {
                writer
                    .write_all(&byte_buffer)
                    .map_err(|e| Error::io("IQ output", e))?;
                byte_buffer.clear();
            }
        }

        let done = samples_written as f64 / job.total_samples.max(1) as f64;
        if done - last_reported >= 0.01 {
            progress(done);
            last_reported = done;
        }
    }

    if !byte_buffer.is_empty() {
        writer
            .write_all(&byte_buffer)
            .map_err(|e| Error::io("IQ output", e))?;
    }
    writer.flush().map_err(|e| Error::io("IQ output", e))?;
    progress(1.0);

    Ok(SynthesisSummary {
        samples_written,
        scale_factor,
        clipped_samples,
        peak_magnitude,
    })
}

/// Interpolation endpoints for one satellite over one epoch interval.
struct EndpointPair {
    code_range: (f64, f64),
    carrier_range: (f64, f64),
    amplitude: (f64, f64),
}

#[inline]
fn lerp((start, end): (f64, f64), fraction: f64) -> f64 {
    start + (end - start) * fraction
}

/// Choose the multiplier applied before quantisation.
///
/// Derived from the loudest epoch rather than measured from the samples, so
/// the whole file uses one scale without a first pass over it. Noise
/// contributes variance 1 per component and each satellite contributes
/// `A^2 / 2`; the total per-component RMS is the square root of their sum.
fn compute_scale_factor(
    job: &ValidatedJob,
    observables: &Observables,
    quantization: Quantization,
) -> f64 {
    if quantization == Quantization::Float32 {
        // Floats have the dynamic range to carry absolute units directly, and
        // leaving them unscaled makes the file a reference the integer formats
        // can be checked against.
        return 1.0;
    }

    let sample_rate = job.spec.output.sample_rate_hz;
    let loudest_signal_power = observables
        .epochs
        .iter()
        .map(|epoch| {
            epoch
                .satellites
                .iter()
                .map(|s| amplitude_for_cn0(s.cn0_db_hz, sample_rate).powi(2))
                .sum::<f64>()
        })
        .fold(0.0f64, f64::max);

    let rms_per_component = (1.0 + loudest_signal_power / 2.0).sqrt();
    quantization.full_scale() / (FULL_SCALE_SIGMA_HEADROOM * rms_per_component)
}

/// Append one complex sample, returning whether it clipped.
#[inline]
fn write_sample(
    buffer: &mut Vec<u8>,
    in_phase: f64,
    quadrature: f64,
    quantization: Quantization,
) -> bool {
    match quantization {
        Quantization::Int8 => {
            let (i, i_clipped) = clamp_to_i8(in_phase);
            let (q, q_clipped) = clamp_to_i8(quadrature);
            buffer.push(i as u8);
            buffer.push(q as u8);
            i_clipped || q_clipped
        }
        Quantization::Int16 => {
            let (i, i_clipped) = clamp_to_i16(in_phase);
            let (q, q_clipped) = clamp_to_i16(quadrature);
            buffer.extend_from_slice(&i.to_le_bytes());
            buffer.extend_from_slice(&q.to_le_bytes());
            i_clipped || q_clipped
        }
        Quantization::Float32 => {
            buffer.extend_from_slice(&(in_phase as f32).to_le_bytes());
            buffer.extend_from_slice(&(quadrature as f32).to_le_bytes());
            false
        }
    }
}

#[inline]
fn clamp_to_i8(value: f64) -> (i8, bool) {
    let rounded = value.round();
    if rounded > 127.0 {
        (127, true)
    } else if rounded < -128.0 {
        (-128, true)
    } else {
        (rounded as i8, false)
    }
}

#[inline]
fn clamp_to_i16(value: f64) -> (i16, bool) {
    let rounded = value.round();
    if rounded > 32767.0 {
        (32767, true)
    } else if rounded < -32768.0 {
        (-32768, true)
    } else {
        (rounded as i16, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The amplitude formula is the bridge between "C/N0 in dB-Hz" and the
    /// numbers in the file, so anchor it on the definition rather than on
    /// itself: a signal at amplitude `A` has power `A^2`, the noise density is
    /// `2/fs`, and their ratio must be the requested C/N0.
    #[test]
    fn amplitude_reproduces_the_requested_cn0() {
        for sample_rate in [2.046e6, 2.6e6, 10e6] {
            for cn0_db in [30.0, 37.5, 45.0, 52.0] {
                let amplitude = amplitude_for_cn0(cn0_db, sample_rate);
                let noise_density = 2.0 / sample_rate;
                let recovered = 10.0 * (amplitude.powi(2) / noise_density).log10();
                assert!(
                    (recovered - cn0_db).abs() < 1e-9,
                    "{cn0_db} dB-Hz at {sample_rate} Hz came back as {recovered}"
                );
            }
        }
    }

    /// Doubling the sample rate spreads the same signal power over twice the
    /// noise bandwidth, so the amplitude has to fall by 3 dB to hold C/N0.
    #[test]
    fn amplitude_tracks_sample_rate_correctly() {
        let low = amplitude_for_cn0(45.0, 2.6e6);
        let high = amplitude_for_cn0(45.0, 5.2e6);
        assert!((low / high - 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn quantisation_clamps_rather_than_wrapping() {
        assert_eq!(clamp_to_i8(200.0), (127, true));
        assert_eq!(clamp_to_i8(-200.0), (-128, true));
        assert_eq!(clamp_to_i8(12.4), (12, false));
        assert_eq!(clamp_to_i16(40_000.0), (32767, true));
        assert_eq!(clamp_to_i16(-40_000.0), (-32768, true));
    }

    /// Interleaving and byte order are the file format's whole contract.
    #[test]
    fn samples_are_written_interleaved_and_little_endian() {
        let mut buffer = Vec::new();
        write_sample(&mut buffer, 1.0, -2.0, Quantization::Int8);
        assert_eq!(buffer, vec![1, 0xFE]);

        buffer.clear();
        write_sample(&mut buffer, 258.0, -2.0, Quantization::Int16);
        assert_eq!(buffer, vec![0x02, 0x01, 0xFE, 0xFF]);

        buffer.clear();
        write_sample(&mut buffer, 1.0, 0.5, Quantization::Float32);
        assert_eq!(buffer.len(), 8);
        assert_eq!(f32::from_le_bytes(buffer[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(buffer[4..8].try_into().unwrap()), 0.5);
    }
}
