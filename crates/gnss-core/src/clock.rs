//! Satellite clock correction from broadcast parameters.
//!
//! Implements IS-GPS-200 §20.3.3.3.3.1 (the polynomial), §20.3.3.3.3.2 (group
//! delay) and §20.3.3.3.3.3 (the relativistic term). Galileo (OS SIS ICD
//! §5.1.4) and BeiDou specify the same polynomial with their own constants,
//! which live on [`Constellation`](crate::Constellation).
//!
//! ## Why this is separate from `propagate`
//!
//! Geometry answers *where* the satellite is; this answers *what its clock
//! reads there*. They meet only at the eccentric anomaly, which the
//! relativistic term needs and which is imported from
//! [`propagate::eccentric_anomaly_at`] rather than solved a second time --
//! two Kepler solves with independently chosen tolerances is exactly how a
//! clock model drifts away from the orbit it belongs to.

use crate::ephemeris::KeplerianEphemeris;
use crate::propagate::{eccentric_anomaly_at, PropagationConfig};
use crate::time::GpsTime;
use crate::Error;

/// The satellite clock offset at one instant, broken out by contribution.
///
/// Kept as separate terms rather than a single number because the sidecar
/// metadata and the pseudorange both want the breakdown, and because a
/// dual-frequency consumer would need the total *without* `group_delay_s`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockCorrection {
    /// Total offset of satellite time from system time \[s\], as a
    /// single-frequency L1 user must apply it: polynomial + relativistic -
    /// group delay.
    ///
    /// Sign convention follows the ICD: `t_system = t_satellite - bias_s`, so
    /// a positive bias means the satellite clock is ahead, and the correction
    /// *shortens* the pseudorange.
    pub bias_s: f64,
    /// Time derivative of the polynomial and relativistic terms \[s/s\].
    ///
    /// Group delay is a constant hardware bias and contributes nothing here.
    /// This drives the clock's share of the observed Doppler.
    pub drift_s_per_s: f64,
    /// Polynomial part alone \[s\]: `af0 + af1*dt + af2*dt^2`.
    pub polynomial_s: f64,
    /// Eccentricity-driven relativistic correction \[s\].
    pub relativistic_s: f64,
    /// Group delay `T_GD` as applied \[s\] (zero when not applicable).
    pub group_delay_s: f64,
}

/// Whether to apply the L1 group-delay term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupDelay {
    /// Single-frequency user: subtract `T_GD`.
    ApplyL1,
    /// Ionosphere-free combination: `T_GD` is already removed, do not apply.
    Omit,
}

/// Satellite clock correction for `ephemeris.sv` at system time `t`.
///
/// `t` is GPS system time at transmission. The ICD defines the polynomial
/// against satellite time, which differs from system time by the correction
/// being solved for; the resulting fixed point converges instantly because the
/// offset is under a millisecond and `af1` is of order 1e-12 s/s, so the
/// second-order residual is below 1e-15 s. One pass is therefore exact to well
/// past the precision anything downstream can carry.
pub fn clock_correction(
    ephemeris: &KeplerianEphemeris,
    t: GpsTime,
    group_delay: GroupDelay,
    config: &PropagationConfig,
) -> Result<ClockCorrection, Error> {
    let dt = t.seconds_since(ephemeris.toc);

    let polynomial_s = ephemeris.af0 + ephemeris.af1 * dt + ephemeris.af2 * dt * dt;
    let polynomial_rate = ephemeris.af1 + 2.0 * ephemeris.af2 * dt;

    let (relativistic_s, relativistic_rate) = relativistic_term(ephemeris, t, config)?;

    let group_delay_s = match group_delay {
        GroupDelay::ApplyL1 => ephemeris.tgd,
        GroupDelay::Omit => 0.0,
    };

    Ok(ClockCorrection {
        bias_s: polynomial_s + relativistic_s - group_delay_s,
        drift_s_per_s: polynomial_rate + relativistic_rate,
        polynomial_s,
        relativistic_s,
        group_delay_s,
    })
}

/// Relativistic clock correction `F * e * sqrt(A) * sin(E)` and its rate.
///
/// This is the eccentricity-driven part only. The constant part of the
/// gravitational and second-order Doppler shift is absorbed by the satellite's
/// deliberately offset frequency standard, so it never appears in a user
/// algorithm.
fn relativistic_term(
    ephemeris: &KeplerianEphemeris,
    t: GpsTime,
    config: &PropagationConfig,
) -> Result<(f64, f64), Error> {
    let Some(f) = ephemeris.sv.constellation.relativistic_f() else {
        // SBAS has no Keplerian model, hence no eccentricity term.
        return Ok((0.0, 0.0));
    };

    let eccentric_anomaly = eccentric_anomaly_at(ephemeris, t, config)?;
    let amplitude = f * ephemeris.eccentricity * ephemeris.sqrt_a;

    // dE/dt from the differentiated Kepler equation, matching `propagate`.
    let mu = ephemeris
        .sv
        .constellation
        .mu()
        .ok_or(Error::InvalidEphemeris {
            reason: "constellation has no Keplerian model",
        })?;
    let mean_motion = (mu / ephemeris.semi_major_axis().powi(3)).sqrt() + ephemeris.delta_n;
    let eccentric_anomaly_rate =
        mean_motion / (1.0 - ephemeris.eccentricity * eccentric_anomaly.cos());

    Ok((
        amplitude * eccentric_anomaly.sin(),
        amplitude * eccentric_anomaly.cos() * eccentric_anomaly_rate,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ephemeris::Sv;
    use crate::Constellation;

    fn ephemeris() -> KeplerianEphemeris {
        KeplerianEphemeris {
            sv: Sv::new(Constellation::Gps, 1),
            toe: GpsTime::from_week_and_sow(2347, 259_200.0),
            toe_seconds_of_week: 259_200.0,
            toc: GpsTime::from_week_and_sow(2347, 259_200.0),
            sqrt_a: 5153.6,
            eccentricity: 0.012,
            i0: 0.96,
            omega0: -1.2,
            argument_of_perigee: 0.87,
            mean_anomaly_0: -0.35,
            delta_n: 4.9e-9,
            i_dot: 2.6e-10,
            omega_dot: -7.9e-9,
            cuc: 0.0,
            cus: 0.0,
            crc: 0.0,
            crs: 0.0,
            cic: 0.0,
            cis: 0.0,
            af0: -1.8e-4,
            af1: -7.5e-12,
            af2: 0.0,
            tgd: -1.1e-8,
            iode: 12.0,
            health: 0,
        }
    }

    /// At ToC the polynomial collapses to af0 exactly.
    #[test]
    fn polynomial_at_reference_epoch_is_af0() {
        let eph = ephemeris();
        let correction = clock_correction(
            &eph,
            eph.toc,
            GroupDelay::Omit,
            &PropagationConfig::default(),
        )
        .unwrap();
        assert!((correction.polynomial_s - eph.af0).abs() < 1e-18);
    }

    /// The drift the model reports must be the derivative of the bias it
    /// reports -- checked by finite differences, which shares no code with the
    /// analytic rate expressions.
    #[test]
    fn drift_is_the_derivative_of_the_bias() {
        let eph = ephemeris();
        let config = PropagationConfig::default();
        let h = 10.0;

        for minutes in [-60, 0, 60] {
            let t = eph.toc.offset_by(f64::from(minutes) * 60.0);
            let at = |time| {
                clock_correction(&eph, time, GroupDelay::Omit, &config)
                    .unwrap()
                    .bias_s
            };
            let numerical = (at(t.offset_by(h)) - at(t.offset_by(-h))) / (2.0 * h);
            let analytic = clock_correction(&eph, t, GroupDelay::Omit, &config)
                .unwrap()
                .drift_s_per_s;
            assert!(
                (analytic - numerical).abs() < 1e-16,
                "drift {analytic:.6e} vs numerical {numerical:.6e} s/s"
            );
        }
    }

    /// Magnitude check against the physics: the relativistic term peaks at
    /// `|F| e sqrt(A)`, which for e = 0.012 is about 23 ns.
    #[test]
    fn relativistic_term_has_the_expected_amplitude() {
        let eph = ephemeris();
        let config = PropagationConfig::default();
        let expected_peak =
            (Constellation::Gps.relativistic_f().unwrap() * eph.eccentricity * eph.sqrt_a).abs();

        assert!(
            (2e-8..3e-8).contains(&expected_peak),
            "amplitude {expected_peak:.3e} s is not the ~23 ns this orbit implies"
        );

        // Sweep a full orbit and confirm the term stays inside its envelope
        // and actually reaches it.
        let mut largest: f64 = 0.0;
        for step in 0..720 {
            let t = eph.toc.offset_by(f64::from(step) * 60.0);
            let term = clock_correction(&eph, t, GroupDelay::Omit, &config)
                .unwrap()
                .relativistic_s;
            assert!(term.abs() <= expected_peak * (1.0 + 1e-12));
            largest = largest.max(term.abs());
        }
        assert!(largest > expected_peak * 0.99, "envelope never approached");
    }

    /// Group delay must be subtracted for L1 and absent otherwise, and must
    /// not leak into the drift -- it is a constant hardware bias.
    #[test]
    fn group_delay_applies_only_to_the_single_frequency_user() {
        let eph = ephemeris();
        let config = PropagationConfig::default();
        let t = eph.toc.offset_by(1234.0);

        let l1 = clock_correction(&eph, t, GroupDelay::ApplyL1, &config).unwrap();
        let free = clock_correction(&eph, t, GroupDelay::Omit, &config).unwrap();

        assert_eq!(l1.group_delay_s, eph.tgd);
        assert_eq!(free.group_delay_s, 0.0);
        assert!((l1.bias_s - (free.bias_s - eph.tgd)).abs() < 1e-18);
        assert_eq!(l1.drift_s_per_s, free.drift_s_per_s);
    }
}
