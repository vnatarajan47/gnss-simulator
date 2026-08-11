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

/// ECEF position and velocity of a satellite at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StateVector {
    /// Position \[m\].
    pub position: Ecef,
    /// Velocity \[m/s\], in the rotating ECEF frame.
    ///
    /// Being ECEF-relative rather than inertial is what makes this directly
    /// usable for Doppler against a ground receiver, whose ECEF velocity is
    /// zero: the frame's rotation is already folded into the
    /// `omega_dot - earth_rate` term.
    pub velocity: Ecef,
}

/// Everything the Table 20-IV solve produces, before it is collapsed to a
/// position.
///
/// Velocity is the analytic time derivative of the same expressions, so it
/// needs the identical set of intermediates -- the anomalies, the corrected
/// orbital elements, and the in-plane coordinates. Computing them once and
/// deriving both from this struct is what stops position and velocity drifting
/// apart under later edits; the alternative is two transcriptions of Table
/// 20-IV that have to be kept in step by hand.
struct OrbitSolution {
    /// Corrected mean motion \[rad/s\].
    mean_motion: f64,
    eccentricity: f64,
    sin_eccentric_anomaly: f64,
    cos_eccentric_anomaly: f64,
    /// Argument of latitude before harmonic correction \[rad\].
    phi_k: f64,
    /// Harmonic-corrected argument of latitude \[rad\].
    argument_of_latitude: f64,
    /// Harmonic-corrected orbit radius \[m\].
    radius: f64,
    /// Harmonic-corrected inclination \[rad\].
    inclination: f64,
    /// Corrected longitude of ascending node \[rad\].
    omega_k: f64,
    semi_major_axis: f64,
    /// In-plane coordinates \[m\].
    x_orbital: f64,
    y_orbital: f64,
    /// Rate of the longitude of ascending node in the ECEF frame \[rad/s\].
    omega_k_dot: f64,
}

/// Run the IS-GPS-200 Table 20-IV user algorithm at instant `t`.
fn solve_orbit(
    ephemeris: &KeplerianEphemeris,
    t: GpsTime,
    config: &PropagationConfig,
) -> Result<OrbitSolution, Error> {
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

    let argument_of_latitude = phi_k + delta_u;
    let radius = semi_major_axis * (1.0 - ephemeris.eccentricity * cos_e) + delta_r;
    let inclination = ephemeris.i0 + delta_i + ephemeris.i_dot * t_k;

    // Position in the orbital plane.
    let (sin_u, cos_u) = argument_of_latitude.sin_cos();

    // Corrected longitude of ascending node. The final term uses ToE as raw
    // seconds-of-week, per the ICD.
    let omega_k = ephemeris.omega0 + (ephemeris.omega_dot - earth_rate) * t_k
        - earth_rate * ephemeris.toe_seconds_of_week;

    Ok(OrbitSolution {
        mean_motion: n,
        eccentricity: ephemeris.eccentricity,
        sin_eccentric_anomaly: sin_e,
        cos_eccentric_anomaly: cos_e,
        phi_k,
        argument_of_latitude,
        radius,
        inclination,
        omega_k,
        semi_major_axis,
        x_orbital: radius * cos_u,
        y_orbital: radius * sin_u,
        omega_k_dot: ephemeris.omega_dot - earth_rate,
    })
}

impl OrbitSolution {
    /// Collapse the in-plane coordinates into ECEF.
    fn position(&self) -> Ecef {
        let (sin_omega, cos_omega) = self.omega_k.sin_cos();
        let (sin_i, cos_i) = self.inclination.sin_cos();

        Ecef::new(
            self.x_orbital * cos_omega - self.y_orbital * cos_i * sin_omega,
            self.x_orbital * sin_omega + self.y_orbital * cos_i * cos_omega,
            self.y_orbital * sin_i,
        )
    }

    /// Analytic time derivative of [`Self::position`].
    ///
    /// Differentiating the closed form rather than finite-differencing it: a
    /// central difference over the step sizes that would be cheap here (~1 s)
    /// carries truncation error of order millimetres per second, which is the
    /// same size as the Doppler resolution this feeds. `velocity_matches_numerical_differentiation`
    /// checks the two against each other.
    fn velocity(&self, ephemeris: &KeplerianEphemeris) -> Ecef {
        let (sin_omega, cos_omega) = self.omega_k.sin_cos();
        let (sin_i, cos_i) = self.inclination.sin_cos();
        let (sin_u, cos_u) = self.argument_of_latitude.sin_cos();
        let (sin_2phi, cos_2phi) = (2.0 * self.phi_k).sin_cos();

        // dE/dt from differentiating Kepler's equation M = E - e sin E.
        let eccentric_anomaly_rate =
            self.mean_motion / (1.0 - self.eccentricity * self.cos_eccentric_anomaly);

        // dv/dE = sqrt(1 - e^2) / (1 - e cos E), so the true-anomaly rate --
        // which is also the rate of phi_k, since the argument of perigee is
        // constant within one ephemeris block.
        let phi_rate = eccentric_anomaly_rate * (1.0 - self.eccentricity.powi(2)).sqrt()
            / (1.0 - self.eccentricity * self.cos_eccentric_anomaly);

        // Rates of the harmonic-corrected elements. Each correction is of the
        // form c_s sin(2 phi) + c_c cos(2 phi), whose derivative is
        // 2 (c_s cos 2phi - c_c sin 2phi) * dphi/dt.
        let harmonic_rate =
            |sine_coeff: f64, cosine_coeff: f64| 2.0 * (sine_coeff * cos_2phi - cosine_coeff * sin_2phi) * phi_rate;

        let argument_of_latitude_rate = phi_rate + harmonic_rate(ephemeris.cus, ephemeris.cuc);
        let radius_rate = self.semi_major_axis
            * self.eccentricity
            * self.sin_eccentric_anomaly
            * eccentric_anomaly_rate
            + harmonic_rate(ephemeris.crs, ephemeris.crc);
        let inclination_rate = ephemeris.i_dot + harmonic_rate(ephemeris.cis, ephemeris.cic);

        // In-plane velocity.
        let x_orbital_rate = radius_rate * cos_u - self.radius * argument_of_latitude_rate * sin_u;
        let y_orbital_rate = radius_rate * sin_u + self.radius * argument_of_latitude_rate * cos_u;

        // Product rule through the two rotations that carry the orbital plane
        // into ECEF; both the inclination and the node angle are time-varying.
        Ecef::new(
            x_orbital_rate * cos_omega - y_orbital_rate * cos_i * sin_omega
                + self.y_orbital * sin_i * sin_omega * inclination_rate
                - (self.x_orbital * sin_omega + self.y_orbital * cos_i * cos_omega)
                    * self.omega_k_dot,
            x_orbital_rate * sin_omega + y_orbital_rate * cos_i * cos_omega
                - self.y_orbital * sin_i * cos_omega * inclination_rate
                + (self.x_orbital * cos_omega - self.y_orbital * cos_i * sin_omega)
                    * self.omega_k_dot,
            y_orbital_rate * sin_i + self.y_orbital * cos_i * inclination_rate,
        )
    }
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
    Ok(solve_orbit(ephemeris, t, config)?.position())
}

/// ECEF position *and* velocity at the instant `t`, treating `t` as the time
/// of signal transmission.
///
/// The velocity is what Doppler is computed from; see [`StateVector::velocity`]
/// for why it is expressed in the rotating frame.
pub fn state_at(
    ephemeris: &KeplerianEphemeris,
    t: GpsTime,
    config: &PropagationConfig,
) -> Result<StateVector, Error> {
    let solution = solve_orbit(ephemeris, t, config)?;
    Ok(StateVector {
        position: solution.position(),
        velocity: solution.velocity(ephemeris),
    })
}

/// Eccentric anomaly at `t`, needed by the relativistic clock correction.
///
/// Exposed rather than recomputed in [`crate::clock`] because it is the one
/// piece of the orbit solve that the clock model shares, and solving Kepler's
/// equation twice with two different tolerances is how the clock and the
/// geometry come to disagree.
pub fn eccentric_anomaly_at(
    ephemeris: &KeplerianEphemeris,
    t: GpsTime,
    config: &PropagationConfig,
) -> Result<f64, Error> {
    let solution = solve_orbit(ephemeris, t, config)?;
    Ok(solution
        .sin_eccentric_anomaly
        .atan2(solution.cos_eccentric_anomaly))
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

/// Apparent position *and* velocity of a Keplerian satellite, as seen by an
/// observer at `observer_ecef` receiving at `reception_time`.
///
/// Same light-time solution as [`apparent_position`], with the velocity carried
/// through the same Sagnac rotation. The rotation's own time derivative
/// (`omega x r`, of order 100 m/s) is deliberately not added: the receiver's
/// frame is the reception-epoch ECEF frame, and the de-rotation expresses the
/// transmit-epoch vector in it rather than transforming between two frames in
/// relative motion. This matches the treatment in RTKLIB's `satposs`.
pub fn apparent_state(
    ephemeris: &KeplerianEphemeris,
    reception_time: GpsTime,
    observer_ecef: Ecef,
    config: &PropagationConfig,
) -> Result<StateVector, Error> {
    let earth_rate = ephemeris.sv.constellation.earth_rotation_rate();
    let uncorrected = state_at(ephemeris, reception_time, config)?;

    if !config.apply_transit_time_correction {
        return Ok(uncorrected);
    }

    let mut transit_time = (uncorrected.position - observer_ecef).norm() / SPEED_OF_LIGHT;
    let mut corrected = uncorrected;

    for _ in 0..2 {
        let at_transmit = state_at(ephemeris, reception_time.offset_by(-transit_time), config)?;
        let angle = -earth_rate * transit_time;
        corrected = StateVector {
            position: at_transmit.position.rotate_z(angle),
            velocity: at_transmit.velocity.rotate_z(angle),
        };
        transit_time = (corrected.position - observer_ecef).norm() / SPEED_OF_LIGHT;
    }

    Ok(corrected)
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
            tgd: 0.0,
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

    /// A realistic perturbed ephemeris: every harmonic term non-zero, so the
    /// velocity derivation is exercised on all of its branches rather than
    /// only the two-body part.
    fn perturbed_ephemeris() -> KeplerianEphemeris {
        KeplerianEphemeris {
            eccentricity: 0.012,
            i0: 0.96,
            omega0: -1.2,
            argument_of_perigee: 0.87,
            mean_anomaly_0: -0.35,
            delta_n: 4.9e-9,
            i_dot: 2.6e-10,
            omega_dot: -7.9e-9,
            cuc: 3.1e-6,
            cus: 7.4e-6,
            crc: 235.0,
            crs: -41.0,
            cic: -1.2e-7,
            cis: 8.9e-8,
            ..circular_ephemeris()
        }
    }

    /// The analytic velocity is the derivative of a closed form; a central
    /// difference of that same closed form is a different computation
    /// entirely -- no shared expressions, only the shared position function.
    /// If the hand-differentiated product rule has a sign or a missing term,
    /// the two disagree immediately.
    #[test]
    fn velocity_matches_numerical_differentiation() {
        let eph = perturbed_ephemeris();
        let config = PropagationConfig::default();
        let h = 1.0;

        for minutes in [-90, -30, 0, 30, 90] {
            let t = eph.toe.offset_by(f64::from(minutes) * 60.0);

            let analytic = state_at(&eph, t, &config).unwrap().velocity;
            let ahead = position_at(&eph, t.offset_by(h), &config).unwrap();
            let behind = position_at(&eph, t.offset_by(-h), &config).unwrap();
            let numerical = Ecef::new(
                (ahead.x - behind.x) / (2.0 * h),
                (ahead.y - behind.y) / (2.0 * h),
                (ahead.z - behind.z) / (2.0 * h),
            );

            // A central difference has O(h^2 * d3x/dt3) truncation error. At
            // GPS orbital rates with h = 1 s that is sub-mm/s, so 1e-3 m/s is
            // a real gate on the analytic form and not merely on the stencil.
            let error = (analytic - numerical).norm();
            assert!(
                error < 1e-3,
                "at t0{minutes:+} min analytic and numerical velocity differ by {error:.3e} m/s"
            );
        }
    }

    /// Sanity on magnitude: a GPS satellite moves at roughly 3.9 km/s in the
    /// rotating frame (about 3.07 km/s inertially, plus the frame's own
    /// rotation at that radius).
    #[test]
    fn velocity_magnitude_is_a_gps_orbital_speed() {
        let eph = perturbed_ephemeris();
        let config = PropagationConfig::default();

        for minutes in 0..24 {
            let t = eph.toe.offset_by(f64::from(minutes) * 300.0);
            let speed = state_at(&eph, t, &config).unwrap().velocity.norm();
            assert!(
                (2_000.0..5_000.0).contains(&speed),
                "ECEF speed {speed:.1} m/s is not a GPS orbital speed"
            );
        }
    }

    /// The eccentric anomaly the clock model uses must be the same one the
    /// geometry solved for -- that is the entire reason it is exposed.
    #[test]
    fn exposed_eccentric_anomaly_satisfies_keplers_equation() {
        let eph = perturbed_ephemeris();
        let config = PropagationConfig::default();
        let mu = Constellation::Gps.mu().unwrap();

        for minutes in [-60, 0, 60] {
            let t_k = f64::from(minutes) * 60.0;
            let t = eph.toe.offset_by(t_k);
            let e_anomaly = eccentric_anomaly_at(&eph, t, &config).unwrap();

            let n = (mu / eph.semi_major_axis().powi(3)).sqrt() + eph.delta_n;
            let expected_m = eph.mean_anomaly_0 + n * t_k;
            let actual_m = e_anomaly - eph.eccentricity * e_anomaly.sin();

            // atan2 returns the principal value, so compare modulo 2*pi.
            let residual = (actual_m - expected_m).rem_euclid(std::f64::consts::TAU);
            let residual = residual.min(std::f64::consts::TAU - residual);
            assert!(residual < 1e-9, "Kepler residual {residual:.3e} rad");
        }
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
