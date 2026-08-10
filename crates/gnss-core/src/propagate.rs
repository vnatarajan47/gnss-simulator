//! Satellite ECEF position from broadcast ephemeris.
//!
//! Implements the user-algorithm of IS-GPS-200 §20.3.3.4.3.1, Table 20-IV.
//! Galileo (OS SIS ICD §5.1.1) and BeiDou (BDS-SIS-ICD-B1I §5.2.4.12)
//! specify the same procedure with different constants, so this module is
//! written against [`Constellation`](crate::Constellation) rather than
//! against GPS specifically.

use crate::constants::SPEED_OF_LIGHT;
use crate::ephemeris::{BroadcastEphemeris, KeplerianEphemeris, Sv};
use crate::geodesy::Ecef;
use crate::time::GpsTime;
use crate::Error;

/// Numerical settings for propagation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PropagationConfig {
    /// Convergence threshold on eccentric anomaly \[rad\].
    ///
    /// 1e-12 rad at the GPS orbit radius is well under a micrometre, i.e.
    /// far below the ~1 m accuracy of the broadcast ephemeris itself.
    pub kepler_tolerance: f64,
    /// Hard iteration cap, so a malformed ephemeris cannot hang the caller.
    ///
    /// This matters more than it looks: this crate runs inside a browser's
    /// main thread via WASM, where a non-terminating loop freezes the tab.
    pub kepler_max_iterations: usize,
    /// Account for signal transit time and Earth rotation during it.
    ///
    /// When set, the satellite is placed at where it was when it *transmitted*
    /// the signal arriving now, and its ECEF position is de-rotated by the
    /// Earth rotation over the flight time (the Sagnac term). The effect on
    /// look angles is small -- of order 0.0005 deg -- but it is the physically
    /// correct quantity and costs one extra iteration.
    pub apply_transit_time_correction: bool,
}

impl Default for PropagationConfig {
    fn default() -> Self {
        Self {
            kepler_tolerance: 1e-12,
            kepler_max_iterations: 30,
            apply_transit_time_correction: true,
        }
    }
}

/// Solve Kepler's equation `M = E - e*sin(E)` for the eccentric anomaly.
///
/// Newton-Raphson from `E0 = M`. For GNSS eccentricities (e < 0.03) the
/// iteration is quadratically convergent and settles in three or four passes.
fn solve_eccentric_anomaly(
    mean_anomaly: f64,
    eccentricity: f64,
    config: &PropagationConfig,
) -> Result<f64, Error> {
    if !(0.0..1.0).contains(&eccentricity) {
        return Err(Error::InvalidEphemeris {
            reason: "eccentricity outside [0, 1)",
        });
    }

    let mut e_anomaly = mean_anomaly;
    for _ in 0..config.kepler_max_iterations {
        let numerator = e_anomaly - eccentricity * e_anomaly.sin() - mean_anomaly;
        let denominator = 1.0 - eccentricity * e_anomaly.cos();
        let step = numerator / denominator;
        e_anomaly -= step;
        if step.abs() < config.kepler_tolerance {
            return Ok(e_anomaly);
        }
    }
    Err(Error::KeplerDidNotConverge {
        iterations: config.kepler_max_iterations,
    })
}

/// ECEF position of `ephemeris.sv` at the instant `t`, treating `t` as the
/// time of *signal transmission*.
///
/// This is the raw Table 20-IV output, with no transit-time or Earth-rotation
/// correction applied. Use [`apparent_position`] for the position an observer
/// actually sees.
pub fn position_at(
    ephemeris: &KeplerianEphemeris,
    t: GpsTime,
    config: &PropagationConfig,
) -> Result<Ecef, Error> {
    let constellation = ephemeris.sv.constellation;
    let mu = constellation.mu().ok_or(Error::InvalidEphemeris {
        reason: "constellation has no Keplerian model",
    })?;
    let earth_rate = constellation.earth_rotation_rate();

    let semi_major_axis = ephemeris.semi_major_axis();
    if semi_major_axis <= 0.0 {
        return Err(Error::InvalidEphemeris {
            reason: "non-positive semi-major axis",
        });
    }

    // Time from ephemeris reference epoch. Because GpsTime is continuous, this
    // needs no +/- half-week correction -- the classic source of bugs when
    // t and ToE straddle a week boundary.
    let t_k = t.seconds_since(ephemeris.toe);

    // Corrected mean motion, then mean anomaly at t.
    let n0 = (mu / semi_major_axis.powi(3)).sqrt();
    let n = n0 + ephemeris.delta_n;
    let mean_anomaly = ephemeris.mean_anomaly_0 + n * t_k;

    let eccentric_anomaly = solve_eccentric_anomaly(mean_anomaly, ephemeris.eccentricity, config)?;
    let (sin_e, cos_e) = eccentric_anomaly.sin_cos();

    // True anomaly.
    let true_anomaly = ((1.0 - ephemeris.eccentricity.powi(2)).sqrt() * sin_e)
        .atan2(cos_e - ephemeris.eccentricity);

    // Argument of latitude, and its second-harmonic corrections.
    let phi_k = true_anomaly + ephemeris.argument_of_perigee;
    let (sin_2phi, cos_2phi) = (2.0 * phi_k).sin_cos();

    let delta_u = ephemeris.cus * sin_2phi + ephemeris.cuc * cos_2phi;
    let delta_r = ephemeris.crs * sin_2phi + ephemeris.crc * cos_2phi;
    let delta_i = ephemeris.cis * sin_2phi + ephemeris.cic * cos_2phi;

    let corrected_argument_of_latitude = phi_k + delta_u;
    let corrected_radius = semi_major_axis * (1.0 - ephemeris.eccentricity * cos_e) + delta_r;
    let corrected_inclination = ephemeris.i0 + delta_i + ephemeris.i_dot * t_k;

    // Position in the orbital plane.
    let (sin_u, cos_u) = corrected_argument_of_latitude.sin_cos();
    let x_orbital = corrected_radius * cos_u;
    let y_orbital = corrected_radius * sin_u;

    // Corrected longitude of ascending node. The final term uses ToE as raw
    // seconds-of-week, per the ICD.
    let omega_k = ephemeris.omega0 + (ephemeris.omega_dot - earth_rate) * t_k
        - earth_rate * ephemeris.toe_seconds_of_week;

    let (sin_omega, cos_omega) = omega_k.sin_cos();
    let (sin_i, cos_i) = corrected_inclination.sin_cos();

    Ok(Ecef::new(
        x_orbital * cos_omega - y_orbital * cos_i * sin_omega,
        x_orbital * sin_omega + y_orbital * cos_i * cos_omega,
        y_orbital * sin_i,
    ))
}

/// ECEF position of the satellite as seen by an observer at `observer_ecef`
/// receiving at `reception_time`.
///
/// Solves the light-time equation: the satellite is propagated to the instant
/// it transmitted, and the resulting ECEF vector is rotated back by the
/// Earth's rotation over the flight time so that transmitter and receiver are
/// expressed in the same (reception-epoch) frame.
pub fn apparent_position(
    ephemeris: &KeplerianEphemeris,
    reception_time: GpsTime,
    observer_ecef: Ecef,
    config: &PropagationConfig,
) -> Result<Ecef, Error> {
    apparent_position_of(
        ephemeris.sv,
        |t| position_at(ephemeris, t, config),
        reception_time,
        observer_ecef,
        config,
    )
}

/// Apparent position of any broadcast record, Keplerian or SBAS.
///
/// Dispatches on the record type; the light-time solution itself is identical
/// either way, so it lives in [`apparent_position_of`].
pub fn apparent_position_any(
    ephemeris: &BroadcastEphemeris,
    reception_time: GpsTime,
    observer_ecef: Ecef,
    config: &PropagationConfig,
) -> Result<Ecef, Error> {
    match ephemeris {
        BroadcastEphemeris::Keplerian(e) => {
            apparent_position(e, reception_time, observer_ecef, config)
        }
        BroadcastEphemeris::Sbas(e) => apparent_position_of(
            e.sv,
            |t| Ok(e.position_at(t)),
            reception_time,
            observer_ecef,
            config,
        ),
    }
}

/// Solve the light-time equation for a satellite whose position at an instant
/// is given by `position_at_time`.
///
/// Shared by both ephemeris kinds: the correction depends only on geometry and
/// the Earth rotation rate, not on how the position was obtained.
fn apparent_position_of(
    sv: Sv,
    position_at_time: impl Fn(GpsTime) -> Result<Ecef, Error>,
    reception_time: GpsTime,
    observer_ecef: Ecef,
    config: &PropagationConfig,
) -> Result<Ecef, Error> {
    let uncorrected = position_at_time(reception_time)?;
    if !config.apply_transit_time_correction {
        return Ok(uncorrected);
    }

    let earth_rate = sv.constellation.earth_rotation_rate();

    // Two passes: the transit time converges to well under a nanosecond,
    // because the range changes by at most ~1 km over one iteration.
    let mut transit_time = (uncorrected - observer_ecef).norm() / SPEED_OF_LIGHT;
    let mut corrected = uncorrected;

    for _ in 0..2 {
        let transmit_time = reception_time.offset_by(-transit_time);
        let at_transmit = position_at_time(transmit_time)?;
        // De-rotate into the reception-epoch ECEF frame (Sagnac correction).
        corrected = at_transmit.rotate_z(-earth_rate * transit_time);
        transit_time = (corrected - observer_ecef).norm() / SPEED_OF_LIGHT;
    }

    Ok(corrected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ephemeris::Sv;
    use crate::Constellation;
    use approx::assert_relative_eq;

    /// A circular-orbit ephemeris with every perturbation zeroed, for which
    /// the answer is analytically known.
    fn circular_ephemeris() -> KeplerianEphemeris {
        KeplerianEphemeris {
            sv: Sv::new(Constellation::Gps, 1),
            toe: GpsTime::from_week_and_sow(2347, 0.0),
            toe_seconds_of_week: 0.0,
            toc: GpsTime::from_week_and_sow(2347, 0.0),
            sqrt_a: 5153.0,
            eccentricity: 0.0,
            i0: 0.0,
            omega0: 0.0,
            argument_of_perigee: 0.0,
            mean_anomaly_0: 0.0,
            delta_n: 0.0,
            i_dot: 0.0,
            omega_dot: 0.0,
            cuc: 0.0,
            cus: 0.0,
            crc: 0.0,
            crs: 0.0,
            cic: 0.0,
            cis: 0.0,
            af0: 0.0,
            af1: 0.0,
            af2: 0.0,
            iode: 0.0,
            health: 0,
        }
    }

    #[test]
    fn kepler_solver_is_exact_for_zero_eccentricity() {
        let config = PropagationConfig::default();
        for m in [-3.0, -0.5, 0.0, 0.5, 1.0, 3.0] {
            let e = solve_eccentric_anomaly(m, 0.0, &config).unwrap();
            assert_relative_eq!(e, m, epsilon = 1e-12);
        }
    }

    /// Independent check: whatever E the solver returns must satisfy the
    /// equation it was asked to solve.
    #[test]
    fn kepler_solution_satisfies_the_equation() {
        let config = PropagationConfig::default();
        for &ecc in &[0.001, 0.01, 0.02, 0.1, 0.3] {
            for step in 0..24 {
                let m = -std::f64::consts::PI + f64::from(step) * std::f64::consts::TAU / 24.0;
                let e = solve_eccentric_anomaly(m, ecc, &config).unwrap();
                assert_relative_eq!(e - ecc * e.sin(), m, epsilon = 1e-11);
            }
        }
    }

    #[test]
    fn kepler_solver_rejects_invalid_eccentricity() {
        let config = PropagationConfig::default();
        assert!(solve_eccentric_anomaly(0.5, 1.0, &config).is_err());
        assert!(solve_eccentric_anomaly(0.5, -0.1, &config).is_err());
    }

    /// With every perturbation zero, the orbit radius must equal the
    /// semi-major axis at all times.
    #[test]
    fn unperturbed_orbit_has_constant_radius() {
        let eph = circular_ephemeris();
        let config = PropagationConfig::default();
        let a = eph.semi_major_axis();

        for minutes in 0..120 {
            let t = eph.toe.offset_by(f64::from(minutes) * 60.0);
            let position = position_at(&eph, t, &config).unwrap();
            assert_relative_eq!(position.norm(), a, max_relative = 1e-12);
        }
    }

    /// Sanity on the orbit itself: GPS satellites sit near 26 560 km radius
    /// with a sidereal period of half a sidereal day (11 h 58 min).
    #[test]
    fn unperturbed_orbit_closes_after_one_sidereal_period() {
        let eph = circular_ephemeris();
        let config = PropagationConfig::default();

        let a = eph.semi_major_axis();
        assert!(
            (26_000_000.0..27_000_000.0).contains(&a),
            "semi-major axis {a} m is not a GPS orbit"
        );

        let mu = Constellation::Gps.mu().expect("GPS has a Keplerian model");
        let period = std::f64::consts::TAU * (a.powi(3) / mu).sqrt();
        assert!(
            (43_000.0..43_300.0).contains(&period),
            "orbital period {period} s is not ~11h58m"
        );

        // With zero inclination and zero Omega-dot, one full period in
        // inertial space returns the satellite to the same inertial point;
        // in ECEF it lands wherever Earth rotation has carried it. Comparing
        // radius rather than position keeps this frame-independent.
        let start = position_at(&eph, eph.toe, &config).unwrap();
        let after = position_at(&eph, eph.toe.offset_by(period), &config).unwrap();
        assert_relative_eq!(start.norm(), after.norm(), max_relative = 1e-12);
    }

    /// Transit-time correction should move the satellite by a small but
    /// non-zero amount -- roughly Earth-rotation over ~70 ms at orbit radius.
    #[test]
    fn transit_time_correction_is_small_but_present() {
        let eph = circular_ephemeris();
        let observer =
            crate::geodesy::geodetic_to_ecef(crate::geodesy::Geodetic::new(39.74, -104.99, 1609.0));

        let with_correction = apparent_position(
            &eph,
            eph.toe.offset_by(600.0),
            observer,
            &PropagationConfig::default(),
        )
        .unwrap();

        let without = apparent_position(
            &eph,
            eph.toe.offset_by(600.0),
            observer,
            &PropagationConfig {
                apply_transit_time_correction: false,
                ..PropagationConfig::default()
            },
        )
        .unwrap();

        let shift = (with_correction - without).norm();
        assert!(
            (10.0..3000.0).contains(&shift),
            "transit-time correction moved the satellite {shift} m, expected 10-3000 m"
        );
    }
}
