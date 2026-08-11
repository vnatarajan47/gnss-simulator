//! Coordinate frames and look angles.
//!
//! Chain: geodetic (lat/lon/alt) -> ECEF -> topocentric ENU -> azimuth /
//! elevation. All angles are degrees at the API boundary and radians
//! internally; all lengths are metres.

use crate::constants::wgs84;

/// Geodetic position on the WGS-84 ellipsoid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geodetic {
    /// Latitude \[deg\], positive north.
    pub latitude_deg: f64,
    /// Longitude \[deg\], positive east.
    pub longitude_deg: f64,
    /// Height above the ellipsoid \[m\].
    ///
    /// Note this is *ellipsoidal* height, not height above mean sea level;
    /// the two differ by the geoid undulation (roughly -35 m to +5 m across
    /// CONUS). At sky-plot resolution the distinction is irrelevant.
    pub altitude_m: f64,
}

impl Geodetic {
    /// Construct from degrees and metres.
    pub const fn new(latitude_deg: f64, longitude_deg: f64, altitude_m: f64) -> Self {
        Self {
            latitude_deg,
            longitude_deg,
            altitude_m,
        }
    }
}

/// Earth-centred, Earth-fixed cartesian position \[m\].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ecef {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Ecef {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Euclidean norm \[m\].
    pub fn norm(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// Dot product with another vector.
    pub fn dot(self, other: Ecef) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Unit vector in the same direction, or `None` at the origin.
    ///
    /// Returns `None` rather than NaN so a degenerate geometry has to be
    /// handled by the caller instead of poisoning the arithmetic downstream.
    pub fn normalized(self) -> Option<Ecef> {
        let norm = self.norm();
        (norm > 0.0).then(|| Ecef::new(self.x / norm, self.y / norm, self.z / norm))
    }

    /// Rotate about the Z axis by `angle_rad` (right-handed / counterclockwise).
    ///
    /// Used to account for Earth rotation during signal propagation.
    pub fn rotate_z(self, angle_rad: f64) -> Ecef {
        let (sin_a, cos_a) = angle_rad.sin_cos();
        Ecef::new(
            self.x * cos_a - self.y * sin_a,
            self.x * sin_a + self.y * cos_a,
            self.z,
        )
    }
}

impl std::ops::Sub for Ecef {
    type Output = Ecef;

    /// Component-wise difference, i.e. the vector from `other` to `self`.
    fn sub(self, other: Ecef) -> Ecef {
        Ecef::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

/// Local topocentric East-North-Up offset \[m\].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Enu {
    pub east: f64,
    pub north: f64,
    pub up: f64,
}

/// Look angles from an observer to a target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AzEl {
    /// Azimuth \[deg\], 0 = true north, increasing clockwise through east.
    pub azimuth_deg: f64,
    /// Elevation \[deg\] above the local horizontal plane; negative = below.
    pub elevation_deg: f64,
    /// Straight-line (geometric) range \[m\].
    pub range_m: f64,
}

/// Geodetic -> ECEF (exact, closed form).
pub fn geodetic_to_ecef(position: Geodetic) -> Ecef {
    let lat = position.latitude_deg.to_radians();
    let lon = position.longitude_deg.to_radians();
    let (sin_lat, cos_lat) = lat.sin_cos();
    let (sin_lon, cos_lon) = lon.sin_cos();

    // Radius of curvature in the prime vertical.
    let n = wgs84::SEMI_MAJOR_AXIS / (1.0 - wgs84::ECC_SQ * sin_lat * sin_lat).sqrt();

    Ecef::new(
        (n + position.altitude_m) * cos_lat * cos_lon,
        (n + position.altitude_m) * cos_lat * sin_lon,
        (n * (1.0 - wgs84::ECC_SQ) + position.altitude_m) * sin_lat,
    )
}

/// ECEF -> geodetic via Bowring's method.
///
/// Non-iterative and accurate to well under a millimetre for any point near
/// the Earth's surface, which is all this crate needs it for.
pub fn ecef_to_geodetic(position: Ecef) -> Geodetic {
    let p = (position.x * position.x + position.y * position.y).sqrt();

    // Degenerate case: on the polar axis, longitude is undefined -> pick 0.
    if p < 1e-9 {
        let sign = if position.z >= 0.0 { 1.0 } else { -1.0 };
        return Geodetic::new(sign * 90.0, 0.0, position.z.abs() - wgs84::SEMI_MINOR_AXIS);
    }

    let theta = (position.z * wgs84::SEMI_MAJOR_AXIS).atan2(p * wgs84::SEMI_MINOR_AXIS);
    let (sin_theta, cos_theta) = theta.sin_cos();

    let lat = (position.z + wgs84::ECC_SQ_PRIME * wgs84::SEMI_MINOR_AXIS * sin_theta.powi(3))
        .atan2(p - wgs84::ECC_SQ * wgs84::SEMI_MAJOR_AXIS * cos_theta.powi(3));
    let lon = position.y.atan2(position.x);

    let sin_lat = lat.sin();
    let n = wgs84::SEMI_MAJOR_AXIS / (1.0 - wgs84::ECC_SQ * sin_lat * sin_lat).sqrt();
    let alt = p / lat.cos() - n;

    Geodetic::new(lat.to_degrees(), lon.to_degrees(), alt)
}

/// ECEF -> local ENU, relative to an observer at `observer`.
pub fn ecef_to_enu(target: Ecef, observer: Geodetic) -> Enu {
    let observer_ecef = geodetic_to_ecef(observer);
    let d = target - observer_ecef;

    let lat = observer.latitude_deg.to_radians();
    let lon = observer.longitude_deg.to_radians();
    let (sin_lat, cos_lat) = lat.sin_cos();
    let (sin_lon, cos_lon) = lon.sin_cos();

    Enu {
        east: -sin_lon * d.x + cos_lon * d.y,
        north: -sin_lat * cos_lon * d.x - sin_lat * sin_lon * d.y + cos_lat * d.z,
        up: cos_lat * cos_lon * d.x + cos_lat * sin_lon * d.y + sin_lat * d.z,
    }
}

/// Azimuth / elevation / range from `observer` to a target in ECEF.
pub fn look_angles(target: Ecef, observer: Geodetic) -> AzEl {
    let enu = ecef_to_enu(target, observer);
    let horizontal = (enu.east * enu.east + enu.north * enu.north).sqrt();

    // atan2 rather than asin(up/range): stays well-conditioned near the zenith.
    let elevation = enu.up.atan2(horizontal);
    let azimuth = enu.east.atan2(enu.north);

    AzEl {
        azimuth_deg: azimuth.to_degrees().rem_euclid(360.0),
        elevation_deg: elevation.to_degrees(),
        range_m: (horizontal * horizontal + enu.up * enu.up).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    /// Analytic anchor: on the equator at the prime meridian, the ellipsoid
    /// surface sits at exactly (a, 0, 0).
    #[test]
    fn equator_prime_meridian_is_semi_major_axis_on_x() {
        let e = geodetic_to_ecef(Geodetic::new(0.0, 0.0, 0.0));
        assert_relative_eq!(e.x, wgs84::SEMI_MAJOR_AXIS, epsilon = 1e-6);
        assert_relative_eq!(e.y, 0.0, epsilon = 1e-6);
        assert_relative_eq!(e.z, 0.0, epsilon = 1e-6);
    }

    /// Analytic anchor: the north pole sits at exactly (0, 0, b).
    #[test]
    fn north_pole_is_semi_minor_axis_on_z() {
        let e = geodetic_to_ecef(Geodetic::new(90.0, 0.0, 0.0));
        assert_relative_eq!(e.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(e.y, 0.0, epsilon = 1e-6);
        assert_relative_eq!(e.z, wgs84::SEMI_MINOR_AXIS, epsilon = 1e-6);
    }

    #[test]
    fn geodetic_ecef_round_trip() {
        for &(lat, lon, alt) in &[
            (39.7392, -104.9903, 1609.0), // Denver
            (25.7617, -80.1918, 2.0),     // Miami
            (47.6062, -122.3321, 53.0),   // Seattle
            (-33.8688, 151.2093, 58.0),   // Sydney (southern + eastern)
            (0.0, 0.0, 0.0),
        ] {
            let g = Geodetic::new(lat, lon, alt);
            let back = ecef_to_geodetic(geodetic_to_ecef(g));
            assert_relative_eq!(back.latitude_deg, lat, epsilon = 1e-9);
            assert_relative_eq!(back.longitude_deg, lon, epsilon = 1e-9);
            assert_relative_eq!(back.altitude_m, alt, epsilon = 1e-6);
        }
    }

    /// A target straight above the observer must read 90 deg elevation, and
    /// its range must equal the height difference exactly.
    #[test]
    fn zenith_target_reads_ninety_degrees() {
        let observer = Geodetic::new(39.7392, -104.9903, 1609.0);
        let above = geodetic_to_ecef(Geodetic::new(
            observer.latitude_deg,
            observer.longitude_deg,
            observer.altitude_m + 20_000_000.0,
        ));
        let look = look_angles(above, observer);
        assert_relative_eq!(look.elevation_deg, 90.0, epsilon = 1e-9);
        assert_relative_eq!(look.range_m, 20_000_000.0, epsilon = 1e-3);
    }

    /// Due-north / east / south / west targets on the local horizon.
    #[test]
    fn cardinal_directions_map_to_expected_azimuths() {
        let observer = Geodetic::new(0.0, 0.0, 0.0);
        let origin = geodetic_to_ecef(observer);

        // At (0,0) the ENU axes align with ECEF: E=+Y, N=+Z, U=+X.
        let cases = [
            (Ecef::new(origin.x, 0.0, 1000.0), 0.0),    // north
            (Ecef::new(origin.x, 1000.0, 0.0), 90.0),   // east
            (Ecef::new(origin.x, 0.0, -1000.0), 180.0), // south
            (Ecef::new(origin.x, -1000.0, 0.0), 270.0), // west
        ];
        for (target, expected_az) in cases {
            let look = look_angles(target, observer);
            assert_relative_eq!(look.azimuth_deg, expected_az, epsilon = 1e-9);
            assert_relative_eq!(look.elevation_deg, 0.0, epsilon = 1e-9);
        }
    }

    /// A target on the opposite side of the Earth must be far below the horizon.
    #[test]
    fn antipodal_target_is_below_horizon() {
        let observer = Geodetic::new(39.7392, -104.9903, 0.0);
        let antipode = geodetic_to_ecef(Geodetic::new(-39.7392, 75.0097, 0.0));
        assert!(look_angles(antipode, observer).elevation_deg < -80.0);
    }

    #[test]
    fn azimuth_is_always_in_zero_to_threesixty() {
        let observer = Geodetic::new(41.0, -96.0, 300.0);
        for bearing in 0..360 {
            let angle = f64::from(bearing).to_radians();
            // Sweep a ring of targets around the observer in the ENU plane.
            let observer_ecef = geodetic_to_ecef(observer);
            let east = Ecef::new(
                -observer.longitude_deg.to_radians().sin(),
                observer.longitude_deg.to_radians().cos(),
                0.0,
            );
            let target = Ecef::new(
                observer_ecef.x + 1e5 * angle.sin() * east.x,
                observer_ecef.y + 1e5 * angle.sin() * east.y,
                observer_ecef.z + 1e5 * angle.cos(),
            );
            let az = look_angles(target, observer).azimuth_deg;
            assert!((0.0..360.0).contains(&az), "azimuth out of range: {az}");
        }
    }
}
