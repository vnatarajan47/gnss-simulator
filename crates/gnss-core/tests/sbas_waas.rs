//! SBAS / WAAS validation.
//!
//! Keplerian propagation is checked against an independent implementation
//! (`reference_cross_check.rs`). SBAS gets a stronger check than that: a
//! geostationary satellite's look angles have a *closed form*, so the state
//! vector propagation is verified against analytic geometry rather than
//! against another program that could share a misconception.
//!
//! Fixture: `data/WAAS_20262201100_02H_SN.rnx`, a two-hour WAAS subset of
//! BKG's 2026-08-08 merged broadcast product (PRNs 131, 133, 135).

use std::path::PathBuf;

use gnss_core::{
    parse_nav, skyplot_from_set, Constellation, Geodetic, GpsTime, SbasProvider, SkyplotOptions, Sv,
};

/// 2026-08-08T12:00:00Z, inside the fixture's 11:00-13:00 window.
const EPOCH_UNIX: f64 = 1_786_190_400.0;

const DENVER: Geodetic = Geodetic::new(39.7392, -104.9903, 1609.0);

fn fixture() -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/WAAS_20262201100_02H_SN.rnx");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn sbas_options() -> SkyplotOptions {
    SkyplotOptions {
        elevation_mask_deg: 0.0,
        constellations: vec![Constellation::Sbas],
        sbas_providers: vec![SbasProvider::Waas],
        ..SkyplotOptions::default()
    }
}

/// Closed-form look angles to a geostationary satellite from a point on a
/// spherical Earth, given the satellite's sub-longitude.
///
/// Deliberately a different formulation from the crate's ECEF/ENU chain: it
/// works in spherical trigonometry from the geocentric angle, never forming a
/// position vector at all.
fn analytic_geostationary(
    observer_lat_deg: f64,
    observer_lon_deg: f64,
    sub_longitude_deg: f64,
    orbit_radius_m: f64,
) -> (f64, f64) {
    const EARTH_RADIUS_M: f64 = 6_378_137.0;

    let lat = observer_lat_deg.to_radians();
    let delta_lon = (sub_longitude_deg - observer_lon_deg).to_radians();

    // Geocentric angle between observer and sub-satellite point.
    let cos_gamma = lat.cos() * delta_lon.cos();
    let gamma = cos_gamma.acos();

    let elevation = ((cos_gamma - EARTH_RADIUS_M / orbit_radius_m) / gamma.sin())
        .atan()
        .to_degrees();
    let azimuth = delta_lon
        .sin()
        .atan2(-lat.sin() * delta_lon.cos())
        .to_degrees()
        .rem_euclid(360.0);

    (azimuth, elevation)
}

#[test]
fn waas_satellites_are_parsed() {
    let set = parse_nav(&fixture()).expect("WAAS fixture should parse");

    let satellites: Vec<Sv> = set.satellites().collect();
    assert_eq!(
        satellites.len(),
        3,
        "expected PRNs 131/133/135, got {satellites:?}"
    );
    for sv in &satellites {
        assert_eq!(sv.constellation, Constellation::Sbas);
        assert_eq!(SbasProvider::from_prn(sv.prn), SbasProvider::Waas);
    }

    // RINEX writes SBAS as PRN-100, so 131 -> "S31".
    let labels: Vec<String> = satellites.iter().map(Sv::to_string).collect();
    assert_eq!(labels, vec!["S31", "S33", "S35"]);
}

/// The headline check: computed look angles must match closed-form
/// geostationary geometry.
#[test]
fn look_angles_match_analytic_geostationary_geometry() {
    let set = parse_nav(&fixture()).unwrap();
    let view = skyplot_from_set(
        &set,
        DENVER,
        GpsTime::from_unix_seconds(EPOCH_UNIX),
        &sbas_options(),
    )
    .expect("sky view should compute");

    assert_eq!(
        view.satellites.len(),
        3,
        "all three WAAS GEOs are visible from Denver"
    );

    for sat in &view.satellites {
        // Recover the sub-satellite longitude and orbit radius from the
        // computed range/angles by re-deriving the ECEF position.
        let ephemeris = set
            .blocks_for(sat.sv)
            .iter()
            .min_by(|a, b| {
                let t = GpsTime::from_unix_seconds(EPOCH_UNIX);
                a.age_at(t).abs().partial_cmp(&b.age_at(t).abs()).unwrap()
            })
            .unwrap();

        let gnss_core::BroadcastEphemeris::Sbas(sbas) = ephemeris else {
            panic!("WAAS records must parse as SBAS state vectors");
        };

        let position = sbas.position_m;
        let radius = position.norm();
        let sub_longitude = position.y.atan2(position.x).to_degrees();

        // Physical sanity: a geostationary satellite, over the Americas.
        assert!(
            (4.20e7..4.23e7).contains(&radius),
            "{}: radius {radius:.0} m is not geostationary",
            sat.sv
        );
        assert!(
            (-135.0..-110.0).contains(&sub_longitude),
            "{}: sub-longitude {sub_longitude:.2} is not a WAAS slot",
            sat.sv
        );

        let (analytic_az, analytic_el) = analytic_geostationary(
            DENVER.latitude_deg,
            DENVER.longitude_deg,
            sub_longitude,
            radius,
        );

        // The analytic form assumes a spherical Earth and an observer at sea
        // level, so it cannot match to machine precision -- the ellipsoid and
        // the 1609 m station height are worth a few hundredths of a degree.
        let az_error = {
            let raw = (sat.azimuth_deg - analytic_az).abs();
            raw.min(360.0 - raw)
        };
        assert!(
            az_error < 0.1,
            "{}: azimuth {:.4} vs analytic {analytic_az:.4} (delta {az_error:.4} deg)",
            sat.sv,
            sat.azimuth_deg
        );
        assert!(
            (sat.elevation_deg - analytic_el).abs() < 0.1,
            "{}: elevation {:.4} vs analytic {analytic_el:.4}",
            sat.sv,
            sat.elevation_deg
        );

        // Slant range to GEO from mid-latitudes.
        assert!(
            (36_000.0..39_000.0).contains(&(sat.range_m / 1000.0)),
            "{}: range {:.0} km is not a GEO slant range",
            sat.sv,
            sat.range_m / 1000.0
        );
    }
}

/// A geostationary satellite must stay put: look angles over an hour should
/// barely move, unlike a GPS satellite which sweeps tens of degrees.
#[test]
fn geostationary_satellites_barely_move() {
    let set = parse_nav(&fixture()).unwrap();
    let options = sbas_options();

    let first = skyplot_from_set(
        &set,
        DENVER,
        GpsTime::from_unix_seconds(EPOCH_UNIX - 1800.0),
        &options,
    )
    .unwrap();
    let later = skyplot_from_set(
        &set,
        DENVER,
        GpsTime::from_unix_seconds(EPOCH_UNIX + 1800.0),
        &options,
    )
    .unwrap();

    for (a, b) in first.satellites.iter().zip(later.satellites.iter()) {
        assert_eq!(a.sv, b.sv);
        assert!(
            (a.elevation_deg - b.elevation_deg).abs() < 0.5,
            "{} moved {:.3} deg in elevation over an hour",
            a.sv,
            (a.elevation_deg - b.elevation_deg).abs()
        );
    }
}

/// Provider filtering must actually filter.
#[test]
fn disabling_waas_yields_an_empty_sky() {
    let set = parse_nav(&fixture()).unwrap();

    let without_waas = SkyplotOptions {
        sbas_providers: vec![SbasProvider::Egnos],
        ..sbas_options()
    };
    let view = skyplot_from_set(
        &set,
        DENVER,
        GpsTime::from_unix_seconds(EPOCH_UNIX),
        &without_waas,
    )
    .unwrap();
    assert!(view.satellites.is_empty());

    // And disabling SBAS entirely does the same.
    let no_sbas = SkyplotOptions {
        constellations: vec![Constellation::Gps],
        ..sbas_options()
    };
    let view = skyplot_from_set(
        &set,
        DENVER,
        GpsTime::from_unix_seconds(EPOCH_UNIX),
        &no_sbas,
    )
    .unwrap();
    assert!(view.satellites.is_empty());
}

/// WAAS is positioned for North America; it should be well up from CONUS and
/// below the horizon from the far side of the world.
#[test]
fn waas_visibility_is_regional() {
    let set = parse_nav(&fixture()).unwrap();
    let t = GpsTime::from_unix_seconds(EPOCH_UNIX);

    let options = SkyplotOptions {
        elevation_mask_deg: 5.0,
        ..sbas_options()
    };

    let denver = skyplot_from_set(&set, DENVER, t, &options).unwrap();
    assert_eq!(
        denver.satellites.len(),
        3,
        "all of WAAS is visible from Denver"
    );
    for sat in &denver.satellites {
        assert!(
            sat.elevation_deg > 30.0,
            "{} is only {:.1} deg up from Denver",
            sat.sv,
            sat.elevation_deg
        );
    }

    // Singapore: on the opposite side of the globe from the WAAS slots.
    let singapore = Geodetic::new(1.3521, 103.8198, 15.0);
    let view = skyplot_from_set(&set, singapore, t, &options).unwrap();
    assert!(
        view.satellites.is_empty(),
        "WAAS should not be visible from Singapore, got {:?}",
        view.satellites
            .iter()
            .map(|s| s.sv.to_string())
            .collect::<Vec<_>>()
    );
}
