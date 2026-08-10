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
//!     [ -e_i  -n_i  -u_i  1 ]
//! ```
//!
//! where `(e, n, u)` is the unit vector from receiver to satellite in the local
//! east/north/up frame and the trailing 1 is the receiver clock bias. Stacking
//! those rows into `A`, the covariance-shaped matrix is
//!
//! ```text
//!     Q = (Aᵀ A)⁻¹
//! ```
//!
//! and the DOP scalars are square roots of sums of its diagonal.
//!
//! ## One clock column
//!
//! `A` carries a single clock unknown, which is correct only when every
//! satellite in the solution shares a time reference. That holds for what this
//! crate currently validates: SBAS is GPS-time coherent by design, and a WAAS
//! GEO's L1 ranging signal is solved in GPS time alongside the GPS satellites.
//!
//! It stops holding the moment Galileo or BeiDou are switched on. Each
//! additional system needs its own column (an inter-system bias), which both
//! widens `A` and raises the minimum satellite count by one per system. That
//! change belongs here, in [`design_matrix`], not at the call sites.
//!
//! ## Sign convention
//!
//! The line-of-sight rows are negated, matching the usual linearisation. The
//! sign makes no difference to the result: negating those three columns is
//! `A → A·D` for `D = diag(-1,-1,-1,1)`, giving `Q → D Q D`, which leaves the
//! diagonal — and therefore every DOP scalar — unchanged.

use crate::skyplot::SatelliteView;

/// Minimum satellites for a solution: three coordinates plus a clock bias.
pub const MIN_SATELLITES: usize = 4;

/// Dilution-of-precision scalars, all dimensionless.
///
/// Smaller is better. Below ~2 is excellent, ~5 is usable, above ~10 the
/// geometry is contributing more error than the ranging does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dop {
    /// Geometric: position and time together, `sqrt(trace Q)`.
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
    /// Time: receiver clock bias, `sqrt(Q₃₃)`.
    pub tdop: f64,
    /// How many satellites entered the solution.
    pub satellites: usize,
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

/// Build the normal matrix `Aᵀ A` directly, without forming `A`.
///
/// Accumulating the 4x4 outer-product sum avoids allocating an n-by-4 matrix
/// for what is a fixed-size result, and the series code calls this once per
/// epoch across hundreds of epochs.
fn design_matrix(satellites: &[(f64, f64)]) -> [[f64; 4]; 4] {
    let mut normal = [[0.0f64; 4]; 4];

    for &(azimuth_deg, elevation_deg) in satellites {
        let (e, n, u) = unit_los(azimuth_deg, elevation_deg);
        let row = [-e, -n, -u, 1.0];

        for (i, &ri) in row.iter().enumerate() {
            for (j, &rj) in row.iter().enumerate() {
                normal[i][j] += ri * rj;
            }
        }
    }

    normal
}

/// Invert a 4x4 matrix by Gauss-Jordan elimination with partial pivoting.
///
/// Returns `None` when the matrix is singular to working precision, which for
/// this application means the satellites do not span three dimensions plus
/// time — all of them coplanar with the receiver, for instance.
fn invert4(mut a: [[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    // Pivots are compared against the largest entry rather than against an
    // absolute epsilon, so the test is scale-free: `Aᵀ A` grows with the
    // satellite count, and a fixed threshold would quietly change meaning.
    let scale = a
        .iter()
        .flat_map(|row| row.iter())
        .fold(0.0f64, |acc, v| acc.max(v.abs()));
    if scale == 0.0 || !scale.is_finite() {
        return None;
    }
    let tolerance = 1e-12 * scale;

    let mut inverse = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    for column in 0..4 {
        let mut pivot_row = column;
        for row in (column + 1)..4 {
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
        for k in 0..4 {
            a[column][k] /= pivot;
            inverse[column][k] /= pivot;
        }

        for row in 0..4 {
            if row == column {
                continue;
            }
            let factor = a[row][column];
            if factor == 0.0 {
                continue;
            }
            for k in 0..4 {
                a[row][k] -= factor * a[column][k];
                inverse[row][k] -= factor * inverse[column][k];
            }
        }
    }

    Some(inverse)
}

/// DOP from a list of `(azimuth_deg, elevation_deg)` pairs.
///
/// Returns `None` when there are too few satellites, or when their geometry is
/// degenerate enough that the normal matrix will not invert. Both are ordinary
/// outcomes for a real sky — a tight elevation mask can leave three satellites
/// up — so they are reported as absence, not as an error.
pub fn dop_from_angles(satellites: &[(f64, f64)]) -> Option<Dop> {
    if satellites.len() < MIN_SATELLITES {
        return None;
    }

    let q = invert4(design_matrix(satellites))?;

    // `Q` is an inverse Gram matrix and so positive semi-definite: its diagonal
    // cannot be negative. If it is, the inversion was numerically hollow even
    // though the pivots cleared the tolerance, and the DOP would be nonsense.
    let diagonal = [q[0][0], q[1][1], q[2][2], q[3][3]];
    if diagonal.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return None;
    }

    let [east, north, up, time] = diagonal;
    let dop = Dop {
        gdop: (east + north + up + time).sqrt(),
        pdop: (east + north + up).sqrt(),
        hdop: (east + north).sqrt(),
        vdop: up.sqrt(),
        tdop: time.sqrt(),
        satellites: satellites.len(),
    };

    dop.gdop.is_finite().then_some(dop)
}

/// DOP for a computed sky view's satellites.
///
/// Takes whatever the caller passes, so DOP always describes exactly the
/// satellites being plotted: if a source is toggled off, it leaves the geometry
/// too. That is the least surprising behaviour, but it does mean the numbers
/// are "DOP for the selected sources", not "DOP this receiver would achieve".
pub fn dop_for(satellites: &[SatelliteView]) -> Option<Dop> {
    let angles: Vec<(f64, f64)> = satellites
        .iter()
        .map(|s| (s.azimuth_deg, s.elevation_deg))
        .collect();
    dop_from_angles(&angles)
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

    #[test]
    fn symmetric_geometry_has_equal_east_and_north_precision() {
        let q = invert4(design_matrix(&tetrahedron())).expect("well-conditioned");
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
}
