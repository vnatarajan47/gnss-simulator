//! SBAS validation, across every augmentation provider in the archive.
//!
//! Keplerian propagation is checked against an independent implementation
//! (`reference_cross_check.rs`, `multignss_cross_check.rs`). SBAS gets two
//! checks that are stronger than another program could be:
//!
//! 1. A geostationary satellite's look angles have a **closed form**, so the
//!    state-vector propagation is verified against analytic spherical geometry
//!    rather than against code that could share a misconception.
//! 2. Several of these satellites broadcast *twice*: BeiDou's geostationary
//!    satellites also carry BDSBAS, and QZSS's carry MSAS. The same physical
//!    spacecraft therefore appears once as a Keplerian element set and once as
//!    an ECEF state vector, propagated by two unrelated algorithms from two
//!    unrelated messages. Requiring those to land in the same place is the
//!    sharpest available check on both — and in particular on BeiDou's
//!    geostationary frame transformation, which is otherwise the least
//!    corroborated piece of the propagator.
//!
//! Fixture: `data/SBAS_20262201100_02H_SN.rnx.gz`, a two-hour all-provider
//! subset of BKG's 2026-08-08 merged broadcast product — WAAS, EGNOS, MSAS,
//! GAGAN, BDSBAS and SouthPAN.

use std::path::PathBuf;

use gnss_core::geodesy::geodetic_to_ecef;
use gnss_core::propagate::apparent_position_any;
use gnss_core::{
    parse_nav, skyplot_from_set, BroadcastEphemeris, Constellation, EphemerisSet, Geodetic,
    GpsTime, PropagationConfig, SbasProvider, SelectionConfig, SkyplotOptions, Sv,
};

/// 2026-08-08T12:00:00Z, inside the fixture's 11:00-13:00 window.
const EPOCH_UNIX: f64 = 1_786_190_400.0;

const DENVER: Geodetic = Geodetic::new(39.7392, -104.9903, 1609.0);

fn data(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn fixture() -> EphemerisSet {
    parse_nav(&data("SBAS_20262201100_02H_SN.rnx.gz")).expect("SBAS fixture should parse")
}

/// Every provider present in the fixture, with a site inside its service area
/// and the satellites it should show there.
const PROVIDERS: &[(SbasProvider, &str, f64, f64, usize)] = &[
    (SbasProvider::Waas, "Denver", 39.7392, -104.9903, 3),
    (SbasProvider::Egnos, "London", 51.5074, -0.1278, 3),
    (SbasProvider::Msas, "Tokyo", 35.6762, 139.6503, 2),
    (SbasProvider::Gagan, "New Delhi", 28.6139, 77.2090, 3),
    (SbasProvider::BdSbas, "Beijing", 39.9042, 116.4074, 3),
    (SbasProvider::SouthPan, "Sydney", -33.8688, 151.2093, 1),
];

fn options_for(providers: Vec<SbasProvider>, mask_deg: f64) -> SkyplotOptions {
    SkyplotOptions {
        elevation_mask_deg: mask_deg,
        constellations: vec![Constellation::Sbas],
        sbas_providers: providers,
        ..SkyplotOptions::default()
    }
}

fn all_providers() -> Vec<SbasProvider> {
    PROVIDERS.iter().map(|entry| entry.0).collect()
}

/// Closed-form look angles to a satellite over a fixed sub-satellite point on
/// a spherical Earth.
///
/// Deliberately a different formulation from the crate's ECEF/ENU chain: it
/// works in spherical trigonometry from the geocentric angle between observer
/// and sub-satellite point, never forming a position vector at all.
///
/// Written for a general sub-latitude rather than assuming the equator.
/// Several of these "geostationary" satellites are not: GAGAN's PRN 127 sits
/// 2.1 deg off the equatorial plane, which is worth 3 deg of azimuth — an
/// equator-only form would have to declare that a failure when the propagator
/// is right and the assumption is wrong.
fn analytic_geostationary(
    observer_lat_deg: f64,
    observer_lon_deg: f64,
    sub_latitude_deg: f64,
    sub_longitude_deg: f64,
    orbit_radius_m: f64,
) -> (f64, f64) {
    const EARTH_RADIUS_M: f64 = 6_378_137.0;

    let lat = observer_lat_deg.to_radians();
    let sub_lat = sub_latitude_deg.to_radians();
    let delta_lon = (sub_longitude_deg - observer_lon_deg).to_radians();

    // Geocentric angle between observer and sub-satellite point.
    let cos_gamma = lat.sin() * sub_lat.sin() + lat.cos() * sub_lat.cos() * delta_lon.cos();
    let gamma = cos_gamma.acos();

    let elevation = ((cos_gamma - EARTH_RADIUS_M / orbit_radius_m) / gamma.sin())
        .atan()
        .to_degrees();
    let azimuth = (sub_lat.cos() * delta_lon.sin())
        .atan2(lat.cos() * sub_lat.sin() - lat.sin() * sub_lat.cos() * delta_lon.cos())
        .to_degrees()
        .rem_euclid(360.0);

    (azimuth, elevation)
}

/// Sub-satellite point (geocentric latitude, longitude) and orbit radius of
/// one SBAS satellite, straight from its broadcast state vector.
fn sub_satellite(set: &EphemerisSet, sv: Sv, t: GpsTime) -> (f64, f64, f64) {
    let record = set
        .select(sv, t, SelectionConfig::default())
        .unwrap_or_else(|| panic!("{sv} should have a usable record"));
    let BroadcastEphemeris::Sbas(sbas) = record else {
        panic!("{sv} must parse as an SBAS state vector");
    };
    let position = sbas.position_m;
    let radius = position.norm();
    (
        (position.z / radius).asin().to_degrees(),
        position.y.atan2(position.x).to_degrees(),
        radius,
    )
}

#[test]
fn every_satellite_in_the_fixture_belongs_to_a_known_provider() {
    let set = fixture();
    let satellites: Vec<Sv> = set.satellites().collect();

    assert_eq!(
        satellites.len(),
        15,
        "expected the full provider spread, got {satellites:?}"
    );

    for sv in &satellites {
        assert_eq!(sv.constellation, Constellation::Sbas);
        assert_ne!(
            SbasProvider::from_prn(sv.prn),
            SbasProvider::Unknown,
            "{sv} has no provider assignment in SbasProvider::from_prn"
        );
    }

    // Display shows the full PRN; the RINEX on-disk form is `PRN - 100`, and
    // both must survive a round trip through the parser.
    assert!(satellites.iter().any(|sv| sv.to_string() == "S131"));
    assert!(satellites.iter().any(|sv| sv.rinex_id() == "S31"));
}

/// The headline check: computed look angles must match closed-form
/// geostationary geometry, for every satellite of every provider.
///
/// The observer is placed relative to each satellite's own sub-longitude so
/// the test covers providers whose satellites are nowhere near CONUS, and so
/// the geometry is well conditioned — 20 deg off in both latitude and
/// longitude is high in the sky without being at the zenith, where azimuth
/// stops being meaningful.
#[test]
fn look_angles_match_analytic_geostationary_geometry() {
    let set = fixture();
    let t = GpsTime::from_unix_seconds(EPOCH_UNIX);
    let mut checked = 0;

    for sv in set.satellites() {
        let (sub_latitude, sub_longitude, radius) = sub_satellite(&set, sv, t);

        // Physical sanity before the geometry: this has to be a GEO at all.
        assert!(
            (4.19e7..4.23e7).contains(&radius),
            "{sv}: radius {radius:.0} m is not geostationary"
        );

        let observer = Geodetic::new(20.0, sub_longitude - 20.0, 0.0);
        let view = skyplot_from_set(
            &set,
            observer,
            t,
            &options_for(vec![SbasProvider::from_prn(sv.prn)], 0.0),
        )
        .expect("sky view should compute");

        let sat = view
            .satellites
            .iter()
            .find(|s| s.sv == sv)
            .unwrap_or_else(|| panic!("{sv} should be visible from beneath its own slot"));

        let (analytic_az, analytic_el) = analytic_geostationary(
            observer.latitude_deg,
            observer.longitude_deg,
            sub_latitude,
            sub_longitude,
            radius,
        );

        // The analytic form assumes a spherical Earth, so it cannot match to
        // machine precision -- the ellipsoid is worth a few hundredths of a
        // degree at these latitudes.
        let az_error = {
            let raw = (sat.azimuth_deg - analytic_az).abs();
            raw.min(360.0 - raw)
        };
        assert!(
            az_error < 0.1,
            "{sv}: azimuth {:.4} vs analytic {analytic_az:.4} (delta {az_error:.4} deg)",
            sat.azimuth_deg
        );
        assert!(
            (sat.elevation_deg - analytic_el).abs() < 0.1,
            "{sv}: elevation {:.4} vs analytic {analytic_el:.4}",
            sat.elevation_deg
        );

        // Slant range to a GEO seen well up in the sky.
        assert!(
            (35_000.0..39_000.0).contains(&(sat.range_m / 1000.0)),
            "{sv}: range {:.0} km is not a GEO slant range",
            sat.range_m / 1000.0
        );

        checked += 1;
    }

    assert_eq!(checked, 15, "every satellite in the fixture must be checked");
}

/// Satellites carrying both a Keplerian element set and an SBAS state vector.
///
/// `same_spacecraft` distinguishes the two situations. Where it holds, the two
/// messages describe one satellite and must agree to metres. Where it does
/// not, the augmentation signal comes from a *different* spacecraft parked in
/// the same orbital slot — operators separate co-located satellites by a few
/// kilometres deliberately — so the offset is real, and the thing to test is
/// that it stays constant instead of growing.
const DUAL_BROADCAST: &[(Constellation, u8, u8, bool)] = &[
    (Constellation::BeiDou, 1, 130, true),
    (Constellation::BeiDou, 2, 144, false),
    (Constellation::BeiDou, 3, 143, true),
    (Constellation::Qzss, 7, 137, true),
    (Constellation::Qzss, 8, 129, true),
];

/// Two propagators, two message formats, one spacecraft.
///
/// This is the strongest evidence available that BeiDou's geostationary
/// transformation is right: the Keplerian path runs the ICD's GEO rotations,
/// the SBAS path is a Taylor expansion of a broadcast state vector, and they
/// share no code. A wrong rotation would put the satellite thousands of
/// kilometres away, not metres.
#[test]
fn the_two_propagators_agree_where_a_satellite_broadcasts_on_both() {
    let mut set = parse_nav(&data("BRDC00WRD_R_20262201000_04H_MN.rnx.gz")).unwrap();
    set.merge(fixture());

    let config = PropagationConfig::default();
    // Any observer will do -- the comparison is of ECEF positions, and the
    // light-time correction both paths apply is a function of geometry only.
    let observer = geodetic_to_ecef(Geodetic::new(0.0, 110.0, 0.0));

    let separation_at = |sv_a: Sv, sv_b: Sv, t: GpsTime| -> f64 {
        let pick = |sv: Sv| {
            set.select(sv, t, SelectionConfig::default())
                .unwrap_or_else(|| panic!("{sv} should have a usable record at {}", t.seconds()))
        };
        let a = apparent_position_any(pick(sv_a), t, observer, &config).unwrap();
        let b = apparent_position_any(pick(sv_b), t, observer, &config).unwrap();
        (a - b).norm()
    };

    for &(constellation, prn, sbas_prn, same_spacecraft) in DUAL_BROADCAST {
        let keplerian = Sv::new(constellation, prn);
        let sbas = Sv::new(Constellation::Sbas, sbas_prn);

        let now = separation_at(keplerian, sbas, GpsTime::from_unix_seconds(EPOCH_UNIX));
        let later = separation_at(
            keplerian,
            sbas,
            GpsTime::from_unix_seconds(EPOCH_UNIX + 1800.0),
        );

        if same_spacecraft {
            assert!(
                now < 50.0,
                "{keplerian} and {sbas} are the same spacecraft but landed {now:.0} m apart"
            );
        } else {
            assert!(
                now < 5_000.0,
                "{keplerian} and {sbas} share an orbital slot but landed {now:.0} m apart"
            );
        }

        // A frame error grows with time from ToE; a co-location offset does
        // not. Half an hour is long enough to separate the two: a missing
        // Earth-rotation term would move the satellite by ~2000 km over it.
        assert!(
            (now - later).abs() < 100.0,
            "{keplerian} vs {sbas}: separation drifted from {now:.0} m to {later:.0} m \
             over 30 min, which is a propagation error rather than an offset"
        );
    }
}

/// A geostationary satellite must stay put: look angles over an hour should
/// barely move, unlike a MEO satellite which sweeps tens of degrees.
#[test]
fn geostationary_satellites_barely_move() {
    let set = fixture();
    let options = options_for(all_providers(), 0.0);

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

    assert!(!first.satellites.is_empty());
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

/// Each provider is visible from the region it serves, and from nowhere near
/// the far side of the world.
///
/// This is the coverage claim the UI now makes: SBAS is regional, so a global
/// receiver has to be told which augmentation applies where.
#[test]
fn provider_visibility_is_regional() {
    let set = fixture();
    let t = GpsTime::from_unix_seconds(EPOCH_UNIX);

    for &(provider, place, lat, lon, expected) in PROVIDERS {
        let options = options_for(vec![provider], 5.0);

        let home = skyplot_from_set(&set, Geodetic::new(lat, lon, 0.0), t, &options).unwrap();
        assert_eq!(
            home.satellites.len(),
            expected,
            "{provider:?} should show {expected} satellites from {place}, got {:?}",
            home.satellites
                .iter()
                .map(|s| s.sv.to_string())
                .collect::<Vec<_>>()
        );
        for sat in &home.satellites {
            assert!(
                sat.elevation_deg > 10.0,
                "{} is only {:.1} deg up from {place}",
                sat.sv,
                sat.elevation_deg
            );
        }

        // The antipode: a geostationary satellite is below the horizon there
        // by construction, whatever the mask.
        let antipode = Geodetic::new(-lat, (lon + 360.0).rem_euclid(360.0) - 180.0, 0.0);
        let away = skyplot_from_set(&set, antipode, t, &options_for(vec![provider], 0.0)).unwrap();
        assert!(
            away.satellites.is_empty(),
            "{provider:?} should not be visible from the antipode of {place}, got {:?}",
            away.satellites
                .iter()
                .map(|s| s.sv.to_string())
                .collect::<Vec<_>>()
        );
    }
}

/// Provider filtering must actually filter: enabling one operator must not
/// quietly bring in the rest of SBAS.
#[test]
fn provider_filtering_selects_only_the_requested_operator() {
    let set = fixture();
    let t = GpsTime::from_unix_seconds(EPOCH_UNIX);

    for &(provider, _, _, _, _) in PROVIDERS {
        let view = skyplot_from_set(
            &set,
            Geodetic::new(20.0, 0.0, 0.0),
            t,
            &options_for(vec![provider], 0.0),
        )
        .unwrap();
        for sat in &view.satellites {
            assert_eq!(
                SbasProvider::from_prn(sat.sv.prn),
                provider,
                "{} is not a {provider:?} satellite",
                sat.sv
            );
        }
    }

    // Turning SBAS off at the constellation level empties the sky regardless
    // of which providers are listed.
    let no_sbas = SkyplotOptions {
        constellations: vec![Constellation::Gps],
        ..options_for(all_providers(), 0.0)
    };
    let view = skyplot_from_set(&set, DENVER, t, &no_sbas).unwrap();
    assert!(view.satellites.is_empty());
}
