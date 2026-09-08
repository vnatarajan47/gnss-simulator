//! Dilution of precision.
//!
//! DOP answers "how much does the *geometry* amplify ranging error into
//! position error". It depends only on the directions to the visible
//! satellites, not on any measurement: a receiver with four satellites bunched
//! in one quarter of the sky has a far worse DOP than one with four spread
//! evenly, given identical signals.
//!
//! The standard formulation linearises the pseudorange equations about the
//! receiver position. Each visible satellite contributes a row
//!
//! ```text
//!     [ -e_i  -n_i  -u_i  | clock columns ]
//! ```
//!
//! where `(e, n, u)` is the unit vector from receiver to satellite in the local
//! east/north/up frame. Stacking those rows into `A`, the covariance-shaped
//! matrix is
//!
//! ```text
//!     Q = (Aᵀ A)⁻¹
//! ```
//!
//! and the DOP scalars are square roots of sums of its diagonal.
//!
//! ## One clock column per time system
//!
//! A single clock unknown is only correct while every satellite in the
//! solution shares a time reference. That holds within a [`TimeSystem`] group
//! and not across them: a receiver solving GPS *and* Galileo cannot assume the
//! two ranging signals are on the same clock, so it estimates the offset
//! between them as an extra unknown (the inter-system bias).
//!
//! So `A` carries a `1` in the column of the satellite's own time system and a
//! `0` in the others. Two consequences fall out of that and are the reason
//! this is not a cosmetic change:
//!
//! - The minimum satellite count rises by one per additional system:
//!   [`min_satellites`]. Four satellites are enough for GPS alone; four split
//!   two-and-two between GPS and Galileo are not, and must report *no
//!   solution* rather than a number.
//! - Adding a system's satellites can make DOP *worse* than not adding them at
//!   all, because they bring an unknown with them. That is real, not an
//!   artefact: it is why a receiver with two satellites from a second
//!   constellation may ignore them.
//!
//! ## Sign convention
//!
//! The line-of-sight rows are negated, matching the usual linearisation. The
//! sign makes no difference to the result: negating those three columns is
//! `A → A·D` for `D = diag(-1,-1,-1,1,...)`, giving `Q → D Q D`, which leaves
//! the diagonal — and therefore every DOP scalar — unchanged.

use crate::skyplot::SatelliteView;
use crate::TimeSystem;

/// Position unknowns: east, north, up. Every solution carries these three.
const POSITION_UNKNOWNS: usize = 3;

/// Widest normal matrix this module builds: three coordinates plus one clock
/// per time system.
const MAX_UNKNOWNS: usize = POSITION_UNKNOWNS + TimeSystem::COUNT;

/// Minimum satellites for a single-system solution: three coordinates plus one
/// clock bias.
///
/// Multi-system solutions need more — see [`min_satellites`].
pub const MIN_SATELLITES: usize = POSITION_UNKNOWNS + 1;

/// Minimum satellites for a solution spanning `systems` distinct time systems.
///
/// Three coordinates and one clock per system. Note this counts *systems*, not
/// satellites per system: a solution still needs enough geometry overall, and
/// a system contributing a single satellite spends that satellite entirely on
/// its own clock unknown.
pub const fn min_satellites(systems: usize) -> usize {
    POSITION_UNKNOWNS + systems
}

/// Dilution-of-precision scalars, all dimensionless.
///
/// Smaller is better. Below ~2 is excellent, ~5 is usable, above ~10 the
/// geometry is contributing more error than the ranging does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dop {
    /// Geometric: position and every clock unknown together, `sqrt(trace Q)`.
    ///
    /// With more than one time system this includes each inter-system bias, so
    /// it is not directly comparable against a single-system GDOP — the
    /// multi-system value is solving for more.
    pub gdop: f64,
    /// Position: three-dimensional, `sqrt(Q₀₀ + Q₁₁ + Q₂₂)`.
    pub pdop: f64,
    /// Horizontal: east and north, `sqrt(Q₀₀ + Q₁₁)`.
    pub hdop: f64,
    /// Vertical: up only, `sqrt(Q₂₂)`.
    ///
    /// Always the worst of the three in practice — a ground receiver only ever
    /// sees satellites in the hemisphere above it, so the vertical direction is
    /// never bracketed the way the horizontal ones are.
    pub vdop: f64,
    /// Time: the *reference* system's receiver clock bias, `sqrt(Q₃₃)`.
    ///
    /// The reference is the lowest [`TimeSystem`] present, so a solution
    /// containing GPS reports the GPS clock. The other systems' unknowns are
    /// inter-system biases relative to it, and are counted in [`Self::gdop`]
    /// rather than reported separately: a receiver's usable time transfer is
    /// to its reference system.
    pub tdop: f64,
    /// How many satellites entered the solution.
    pub satellites: usize,
    /// How many distinct time systems they spanned, i.e. how many clock
    /// unknowns the solution carried.
    pub systems: usize,
}

/// Unit line-of-sight vector in the local ENU frame, from azimuth/elevation.
///
/// Exact rather than approximate: a look angle pair *is* a direction in ENU, so
/// this is a change of coordinates and not a fit. Taking DOP from angles rather
/// than from ECEF vectors also keeps this module a pure function of a sky view,
/// which is what lets the tests state geometry directly.
fn unit_los(azimuth_deg: f64, elevation_deg: f64) -> (f64, f64, f64) {
    let az = azimuth_deg.to_radians();
    let el = elevation_deg.to_radians();
    let (sin_az, cos_az) = az.sin_cos();
    let (sin_el, cos_el) = el.sin_cos();
    (cos_el * sin_az, cos_el * cos_az, sin_el)
}

/// The distinct time systems present, ascending.
///
/// Ascending rather than in order of first appearance so the column layout is a
/// function of the *set* of systems and not of the satellite ordering — which
/// is what makes [`Dop::tdop`] mean "the GPS clock" whenever GPS is present,
/// and makes the result invariant to how the caller sorted its satellites.
fn systems_present(observations: &[(f64, f64, TimeSystem)]) -> Vec<TimeSystem> {
    let mut systems: Vec<TimeSystem> = observations.iter().map(|&(_, _, s)| s).collect();
    systems.sort();
    systems.dedup();
    systems
}

/// Build the normal matrix `Aᵀ A` directly, without forming `A`.
///
/// Accumulating the outer-product sum avoids allocating an n-by-k matrix for
/// what is a fixed-size result, and the series code calls this once per epoch
/// across hundreds of epochs.
///
/// Returns the matrix and its dimension. Columns are east, north, up, then one
/// clock column per entry of `systems`, in that order.
fn design_matrix(
    observations: &[(f64, f64, TimeSystem)],
    systems: &[TimeSystem],
) -> ([[f64; MAX_UNKNOWNS]; MAX_UNKNOWNS], usize) {
    let unknowns = POSITION_UNKNOWNS + systems.len();
    let mut normal = [[0.0f64; MAX_UNKNOWNS]; MAX_UNKNOWNS];

    for &(azimuth_deg, elevation_deg, system) in observations {
        let (e, n, u) = unit_los(azimuth_deg, elevation_deg);
        let mut row = [0.0f64; MAX_UNKNOWNS];
        row[0] = -e;
        row[1] = -n;
        row[2] = -u;
        // A satellite sees only its own system's clock. `systems` was built
        // from these same observations, so the lookup always succeeds.
        let column = systems
            .iter()
            .position(|&s| s == system)
            .expect("system list is built from the observations");
        row[POSITION_UNKNOWNS + column] = 1.0;

        for i in 0..unknowns {
            for j in 0..unknowns {
                normal[i][j] += row[i] * row[j];
            }
        }
    }

    (normal, unknowns)
}

/// Invert the leading `n`-by-`n` block by Gauss-Jordan elimination with partial
/// pivoting.
///
/// Returns `None` when the matrix is singular to working precision, which for
/// this application means the satellites do not span three dimensions plus one
/// clock per system — all of them coplanar with the receiver, for instance, or
/// a system whose satellites cannot be separated from its own clock offset.
fn invert(
    mut a: [[f64; MAX_UNKNOWNS]; MAX_UNKNOWNS],
    n: usize,
) -> Option<[[f64; MAX_UNKNOWNS]; MAX_UNKNOWNS]> {
    // Pivots are compared against the largest entry rather than against an
    // absolute epsilon, so the test is scale-free: `Aᵀ A` grows with the
    // satellite count, and a fixed threshold would quietly change meaning.
    let scale = a
        .iter()
        .take(n)
        .flat_map(|row| row.iter().take(n))
        .fold(0.0f64, |acc, v| acc.max(v.abs()));
    if scale == 0.0 || !scale.is_finite() {
        return None;
    }
    let tolerance = 1e-12 * scale;

    let mut inverse = [[0.0f64; MAX_UNKNOWNS]; MAX_UNKNOWNS];
    for (i, row) in inverse.iter_mut().enumerate().take(n) {
        row[i] = 1.0;
    }

    for column in 0..n {
        let mut pivot_row = column;
        for row in (column + 1)..n {
            if a[row][column].abs() > a[pivot_row][column].abs() {
                pivot_row = row;
            }
        }
        if a[pivot_row][column].abs() < tolerance {
            return None;
        }
        a.swap(column, pivot_row);
        inverse.swap(column, pivot_row);

        let pivot = a[column][column];
        for k in 0..n {
            a[column][k] /= pivot;
            inverse[column][k] /= pivot;
        }

        for row in 0..n {
            if row == column {
                continue;
            }
            let factor = a[row][column];
            if factor == 0.0 {
                continue;
            }
            for k in 0..n {
                a[row][k] -= factor * a[column][k];
                inverse[row][k] -= factor * inverse[column][k];
            }
        }
    }

    Some(inverse)
}

/// DOP from `(azimuth_deg, elevation_deg, time_system)` triples.
///
/// Returns `None` when there are too few satellites for the number of clock
/// unknowns ([`min_satellites`]), or when the geometry is degenerate enough
/// that the normal matrix will not invert. Both are ordinary outcomes for a
/// real sky — a tight elevation mask can leave three satellites up, and two
/// constellations contributing two satellites each is a perfectly normal thing
/// to plot and still not a solvable system — so they are reported as absence,
/// not as an error.
pub fn dop_from_observations(observations: &[(f64, f64, TimeSystem)]) -> Option<Dop> {
    let systems = systems_present(observations);
    if observations.len() < min_satellites(systems.len()) {
        return None;
    }

    let (normal, unknowns) = design_matrix(observations, &systems);
    let q = invert(normal, unknowns)?;

    // `Q` is an inverse Gram matrix and so positive semi-definite: its diagonal
    // cannot be negative. If it is, the inversion was numerically hollow even
    // though the pivots cleared the tolerance, and the DOP would be nonsense.
    let diagonal: Vec<f64> = (0..unknowns).map(|i| q[i][i]).collect();
    if diagonal.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return None;
    }

    let (east, north, up) = (diagonal[0], diagonal[1], diagonal[2]);
    let clocks: f64 = diagonal[POSITION_UNKNOWNS..].iter().sum();

    let dop = Dop {
        gdop: (east + north + up + clocks).sqrt(),
        pdop: (east + north + up).sqrt(),
        hdop: (east + north).sqrt(),
        vdop: up.sqrt(),
        tdop: diagonal[POSITION_UNKNOWNS].sqrt(),
        satellites: observations.len(),
        systems: systems.len(),
    };

    dop.gdop.is_finite().then_some(dop)
}

/// DOP for satellites that all share one time system.
///
/// The single-system case, kept as its own entry point because most geometry
/// reasoning — and every closed-form test in this module — is about direction
/// alone. Multi-system callers want [`dop_from_observations`].
pub fn dop_from_angles(satellites: &[(f64, f64)]) -> Option<Dop> {
    let observations: Vec<(f64, f64, TimeSystem)> = satellites
        .iter()
        .map(|&(az, el)| (az, el, TimeSystem::Gps))
        .collect();
    dop_from_observations(&observations)
}

/// DOP for a computed sky view's satellites.
///
/// Takes whatever the caller passes, so DOP always describes exactly the
/// satellites being plotted: if a source is toggled off, it leaves the geometry
/// too. That is the least surprising behaviour, but it does mean the numbers
/// are "DOP for the selected sources", not "DOP this receiver would achieve".
pub fn dop_for(satellites: &[SatelliteView]) -> Option<Dop> {
    let observations: Vec<(f64, f64, TimeSystem)> = satellites
        .iter()
        .map(|s| {
            (
                s.azimuth_deg,
                s.elevation_deg,
                s.sv.constellation.time_system(),
            )
        })
        .collect();
    dop_from_observations(&observations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    /// One satellite at the zenith and three on the horizon 120 deg apart.
    ///
    /// This geometry has a closed-form answer, which makes it a real check on
    /// the algebra rather than a change-detector. With the three horizon
    /// satellites symmetric in azimuth, the east and north columns are
    /// interchangeable, so Q00 == Q11 exactly and HDOP = sqrt(2·Q00).
    fn tetrahedron() -> Vec<(f64, f64)> {
        vec![(0.0, 90.0), (0.0, 0.0), (120.0, 0.0), (240.0, 0.0)]
    }

    /// The same geometry as observations in one time system.
    fn single_system(angles: &[(f64, f64)]) -> Vec<(f64, f64, TimeSystem)> {
        angles
            .iter()
            .map(|&(az, el)| (az, el, TimeSystem::Gps))
            .collect()
    }

    #[test]
    fn symmetric_geometry_has_equal_east_and_north_precision() {
        let observations = single_system(&tetrahedron());
        let systems = systems_present(&observations);
        let (normal, unknowns) = design_matrix(&observations, &systems);
        let q = invert(normal, unknowns).expect("well-conditioned");
        assert_relative_eq!(q[0][0], q[1][1], epsilon = 1e-12);

        let dop = dop_from_angles(&tetrahedron()).expect("well-conditioned");
        assert_relative_eq!(dop.hdop, (2.0 * q[0][0]).sqrt(), epsilon = 1e-12);
    }

    /// The whole tetrahedron solved by hand, as an independent check on the
    /// inversion.
    ///
    /// The three horizon satellites are symmetric in azimuth, which zeroes
    /// every off-diagonal term except the up/clock pair, so `Aᵀ A` reduces to
    ///
    /// ```text
    ///     diag(3/2, 3/2) ⊕ [[1, -1], [-1, 4]]
    /// ```
    ///
    /// Inverting each block by hand gives Q = (2/3, 2/3, 4/3, 1/3).
    #[test]
    fn tetrahedron_matches_the_closed_form() {
        let dop = dop_from_angles(&tetrahedron()).expect("well-conditioned");

        assert_relative_eq!(dop.hdop, (4.0f64 / 3.0).sqrt(), epsilon = 1e-9);
        assert_relative_eq!(dop.vdop, (4.0f64 / 3.0).sqrt(), epsilon = 1e-9);
        assert_relative_eq!(dop.pdop, (8.0f64 / 3.0).sqrt(), epsilon = 1e-9);
        assert_relative_eq!(dop.tdop, (1.0f64 / 3.0).sqrt(), epsilon = 1e-9);
        assert_relative_eq!(dop.gdop, 3.0f64.sqrt(), epsilon = 1e-9);
        assert_eq!(dop.satellites, 4);
        assert_eq!(dop.systems, 1);
    }

    /// Vertical is always worse than horizontal for a ground receiver: every
    /// satellite is above the horizon, so the up direction is only ever
    /// observed from one side.
    #[test]
    fn vertical_is_worse_than_horizontal_for_a_realistic_sky() {
        // A plausible mid-latitude sky, spread but all above the mask.
        let sky = vec![
            (32.0, 61.0),
            (95.0, 24.0),
            (168.0, 47.0),
            (231.0, 15.0),
            (287.0, 38.0),
            (350.0, 72.0),
        ];
        let dop = dop_from_angles(&sky).expect("well-conditioned");
        assert!(dop.vdop > dop.hdop, "vdop {} hdop {}", dop.vdop, dop.hdop);
        assert!(dop.pdop > dop.hdop);
        assert!(dop.gdop > dop.pdop);
    }

    #[test]
    fn adding_a_satellite_cannot_worsen_dop() {
        // Extra measurements can only shrink the covariance -- a strictly
        // monotone property of least squares, and a good trap for sign errors.
        // True only *within* one time system; a satellite from a new system
        // brings an unknown with it, which is the next test.
        let base = tetrahedron();
        let mut extended = base.clone();
        extended.push((60.0, 35.0));

        let before = dop_from_angles(&base).unwrap();
        let after = dop_from_angles(&extended).unwrap();

        assert!(after.gdop <= before.gdop);
        assert!(after.pdop <= before.pdop);
        assert!(after.hdop <= before.hdop);
        assert!(after.vdop <= before.vdop);
    }

    #[test]
    fn dop_is_invariant_to_satellite_ordering() {
        let mut shuffled = tetrahedron();
        shuffled.reverse();
        let a = dop_from_angles(&tetrahedron()).unwrap();
        let b = dop_from_angles(&shuffled).unwrap();
        assert_relative_eq!(a.gdop, b.gdop, epsilon = 1e-12);
        assert_relative_eq!(a.hdop, b.hdop, epsilon = 1e-12);
    }

    #[test]
    fn fewer_than_four_satellites_has_no_solution() {
        assert!(dop_from_angles(&[]).is_none());
        assert!(dop_from_angles(&[(0.0, 45.0), (120.0, 45.0), (240.0, 45.0)]).is_none());
    }

    /// Four satellites at the same point in the sky span one direction, not
    /// three. The normal matrix is singular and there is no solution.
    #[test]
    fn coincident_satellites_are_singular() {
        let stacked = vec![(45.0, 30.0); 4];
        assert!(dop_from_angles(&stacked).is_none());
    }

    /// The case that matters for this project: WAAS's geostationary satellites
    /// sit in a tight clump low in the southern sky from CONUS. On their own
    /// they cannot fix a position, and DOP must say so rather than return a
    /// large-but-plausible number.
    #[test]
    fn the_waas_geos_alone_cannot_fix_a_position() {
        let waas = vec![(198.41, 42.39), (215.10, 37.70), (209.71, 39.56)];
        assert!(dop_from_angles(&waas).is_none(), "three satellites");

        // Even a fourth GEO does not help much: they are nearly coplanar with
        // the receiver, so if it inverts at all the DOP must be dreadful.
        let mut four = waas.clone();
        four.push((225.0, 35.0));
        if let Some(dop) = dop_from_angles(&four) {
            assert!(dop.pdop > 20.0, "expected unusable geometry, got {dop:?}");
        }
    }

    /// All satellites on the horizon leaves the vertical direction unobserved.
    #[test]
    fn a_horizon_only_sky_cannot_resolve_height() {
        let horizon: Vec<(f64, f64)> = (0..8).map(|i| (i as f64 * 45.0, 0.0)).collect();
        match dop_from_angles(&horizon) {
            None => {}
            Some(dop) => assert!(dop.vdop > 1e6, "expected unobservable height, got {dop:?}"),
        }
    }

    // ---------------------------------------------------------- multi-system

    #[test]
    fn each_extra_system_costs_one_satellite() {
        assert_eq!(min_satellites(1), MIN_SATELLITES);
        assert_eq!(min_satellites(2), 5);
        assert_eq!(min_satellites(3), 6);
    }

    /// Four satellites split two-and-two across two systems is five unknowns
    /// from four equations. It has no solution, and must not silently be
    /// treated as the four-satellite single-system case.
    #[test]
    fn four_satellites_across_two_systems_has_no_solution() {
        let mixed = vec![
            (0.0, 90.0, TimeSystem::Gps),
            (0.0, 0.0, TimeSystem::Gps),
            (120.0, 0.0, TimeSystem::Galileo),
            (240.0, 0.0, TimeSystem::Galileo),
        ];
        assert!(dop_from_observations(&mixed).is_none());

        // The identical geometry in one system is the closed-form tetrahedron.
        assert!(dop_from_angles(&tetrahedron()).is_some());
    }

    /// A system contributing exactly one satellite contributes *nothing* to
    /// position precision, exactly.
    ///
    /// This is analytic, not empirical. Eliminating that system's clock
    /// unknown from the normal matrix by its Schur complement subtracts
    /// `b bᵀ` — precisely the outer product its own row contributed — leaving
    /// the other system's normal matrix untouched. So HDOP, VDOP, PDOP and
    /// TDOP must be bit-for-bit the values from the tetrahedron alone, while
    /// GDOP grows by the new clock's variance.
    ///
    /// It is also the sharpest available check that the clock columns are
    /// wired to the right satellites: put that fifth satellite in the *same*
    /// system and PDOP improves instead.
    #[test]
    fn a_lone_satellite_from_another_system_only_pays_for_its_own_clock() {
        let alone = dop_from_angles(&tetrahedron()).unwrap();

        let mut mixed = single_system(&tetrahedron());
        mixed.push((60.0, 35.0, TimeSystem::Galileo));
        let with_galileo = dop_from_observations(&mixed).expect("five satellites, five unknowns");

        assert_eq!(with_galileo.systems, 2);
        assert_eq!(with_galileo.satellites, 5);
        assert_relative_eq!(with_galileo.hdop, alone.hdop, epsilon = 1e-12);
        assert_relative_eq!(with_galileo.vdop, alone.vdop, epsilon = 1e-12);
        assert_relative_eq!(with_galileo.pdop, alone.pdop, epsilon = 1e-12);
        assert_relative_eq!(with_galileo.tdop, alone.tdop, epsilon = 1e-12);
        assert!(
            with_galileo.gdop > alone.gdop,
            "the extra clock unknown must show up in GDOP"
        );

        // The same satellite in the same system does improve the position.
        let mut same = tetrahedron();
        same.push((60.0, 35.0));
        let together = dop_from_angles(&same).unwrap();
        assert!(together.pdop < alone.pdop);
    }

    /// The reference clock is the lowest system present, not the first one the
    /// caller happened to list, so TDOP does not depend on satellite order.
    #[test]
    fn the_reference_clock_does_not_depend_on_ordering() {
        let sky = |system_of: fn(usize) -> TimeSystem| -> Vec<(f64, f64, TimeSystem)> {
            [
                (32.0, 61.0),
                (95.0, 24.0),
                (168.0, 47.0),
                (231.0, 15.0),
                (287.0, 38.0),
                (350.0, 72.0),
            ]
            .iter()
            .enumerate()
            .map(|(i, &(az, el))| (az, el, system_of(i)))
            .collect()
        };

        let interleaved = sky(|i| {
            if i % 2 == 0 {
                TimeSystem::Galileo
            } else {
                TimeSystem::Gps
            }
        });
        let mut reversed = interleaved.clone();
        reversed.reverse();

        let a = dop_from_observations(&interleaved).expect("well-conditioned");
        let b = dop_from_observations(&reversed).expect("well-conditioned");
        assert_relative_eq!(a.tdop, b.tdop, epsilon = 1e-12);
        assert_relative_eq!(a.gdop, b.gdop, epsilon = 1e-12);
        assert_eq!(a.systems, 2);
    }

    /// Three systems is the widest matrix this module builds; it must invert.
    #[test]
    fn three_systems_still_solve() {
        let sky = vec![
            (32.0, 61.0, TimeSystem::Gps),
            (95.0, 24.0, TimeSystem::Gps),
            (168.0, 47.0, TimeSystem::Galileo),
            (231.0, 15.0, TimeSystem::Galileo),
            (287.0, 38.0, TimeSystem::BeiDou),
            (350.0, 72.0, TimeSystem::BeiDou),
        ];
        let dop = dop_from_observations(&sky).expect("six satellites, six unknowns");
        assert_eq!(dop.systems, 3);
        assert!(dop.gdop.is_finite() && dop.gdop > 0.0);

        // Six unknowns from five equations has no solution.
        assert!(dop_from_observations(&sky[..5]).is_none());
    }

    /// SBAS rides on GPS time by design, so a WAAS satellite must *not* open a
    /// second clock column — if it did, four GPS plus one GEO would report no
    /// solution where a real receiver has one.
    #[test]
    fn sbas_shares_the_gps_clock() {
        use crate::Constellation;
        assert_eq!(Constellation::Sbas.time_system(), TimeSystem::Gps);
        assert_eq!(Constellation::Qzss.time_system(), TimeSystem::Gps);
        assert_ne!(Constellation::Galileo.time_system(), TimeSystem::Gps);
        assert_ne!(Constellation::BeiDou.time_system(), TimeSystem::Gps);
    }
}
