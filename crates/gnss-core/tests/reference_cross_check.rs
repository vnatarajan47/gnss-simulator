//! Validation of the propagation and look-angle math against an independent
//! reference implementation.
//!
//! The golden file `tests/vectors/reference_vectors.json` is produced by
//! `tools/reference_skyplot.py`, which implements the same IS-GPS-200
//! algorithm with different code throughout: fixed-column RINEX parsing
//! instead of the `rinex` crate, fixed-point Kepler iteration instead of
//! Newton-Raphson, `asin` for elevation instead of `atan2`, and an explicit
//! ENU rotation matrix. DOP goes through an explicit design matrix and an SVD
//! pseudo-inverse there, against an in-place normal matrix and Gauss-Jordan
//! elimination here. Two implementations that share no code agreeing to
//! nanodegrees is real evidence; a port agreeing with its original is not.
//!
//! Regenerating needs numpy (for the SVD); running the tests does not, since
//! the golden file is checked in.
//!
//! Regenerate with:
//!     python3 tools/reference_skyplot.py --emit-vectors \
//!         > crates/gnss-core/tests/vectors/reference_vectors.json
//!
//! Input data is a real broadcast file: BKG's BRDC00WRD product for
//! 2025-01-01, trimmed to GPS.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gnss_core::{dop_for, parse_nav, skyplot_from_set, Geodetic, GpsTime, SkyplotOptions};
use serde::Deserialize;

/// Angular agreement required between the two implementations.
///
/// Both are f64 evaluations of the same closed-form expressions, so the only
/// spread should be floating-point ordering and solver tolerance. Anything
/// above this means the algorithms genuinely differ.
const ANGLE_TOLERANCE_DEG: f64 = 1e-7;

/// Range agreement, in metres. Ranges are ~2.5e7 m, so this is ~4e-14 relative.
const RANGE_TOLERANCE_M: f64 = 1e-4;

#[derive(Deserialize)]
struct Vectors {
    source: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    observer: String,
    lat: f64,
    lon: f64,
    alt: f64,
    epoch_iso: String,
    unix_seconds: f64,
    gps_seconds: f64,
    elevation_mask_deg: f64,
    satellites: Vec<ReferenceSatellite>,
    /// `null` where the reference found no solution.
    dop: Option<ReferenceDop>,
}

#[derive(Deserialize)]
struct ReferenceDop {
    gdop: f64,
    pdop: f64,
    hdop: f64,
    vdop: f64,
    tdop: f64,
    satellites: usize,
}

#[derive(Deserialize)]
struct ReferenceSatellite {
    prn: u8,
    azimuth: f64,
    elevation: f64,
    range_km: f64,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_vectors() -> Vectors {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/reference_vectors.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("malformed reference vectors")
}

fn load_nav_bytes(source: &str) -> Vec<u8> {
    let path = repo_root().join("data").join(source);
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The headline test: every satellite in every case must match the reference.
#[test]
fn matches_independent_python_reference() {
    let vectors = load_vectors();
    let bytes = load_nav_bytes(&vectors.source);
    let set = parse_nav(&bytes).expect("RINEX should parse");

    let mut worst_azimuth: f64 = 0.0;
    let mut worst_elevation: f64 = 0.0;
    let mut worst_range: f64 = 0.0;
    let mut compared = 0usize;

    for case in &vectors.cases {
        let options = SkyplotOptions {
            elevation_mask_deg: case.elevation_mask_deg,
            ..SkyplotOptions::default()
        };
        let observer = Geodetic::new(case.lat, case.lon, case.alt);
        let t = GpsTime::from_unix_seconds(case.unix_seconds);

        // The two implementations must agree on the epoch before they can
        // meaningfully agree on geometry.
        assert!(
            (t.seconds() - case.gps_seconds).abs() < 1e-6,
            "{} @ {}: GPS time mismatch, rust={} python={}",
            case.observer,
            case.epoch_iso,
            t.seconds(),
            case.gps_seconds
        );

        let view = skyplot_from_set(&set, observer, t, &options).expect("sky view should compute");

        let ours: BTreeMap<u8, _> = view.satellites.iter().map(|s| (s.sv.prn, s)).collect();

        assert_eq!(
            ours.len(),
            case.satellites.len(),
            "{} @ {}: visible-satellite count differs (rust={:?}, python={:?})",
            case.observer,
            case.epoch_iso,
            ours.keys().collect::<Vec<_>>(),
            case.satellites.iter().map(|s| s.prn).collect::<Vec<_>>()
        );

        for expected in &case.satellites {
            let actual = ours.get(&expected.prn).unwrap_or_else(|| {
                panic!(
                    "{} @ {}: reference has G{:02} but we do not",
                    case.observer, case.epoch_iso, expected.prn
                )
            });

            // Azimuth is modular: 359.9999 and 0.0001 are adjacent.
            let azimuth_error = {
                let raw = (actual.azimuth_deg - expected.azimuth).abs();
                raw.min(360.0 - raw)
            };
            let elevation_error = (actual.elevation_deg - expected.elevation).abs();
            let range_error = (actual.range_m - expected.range_km * 1000.0).abs();

            assert!(
                azimuth_error < ANGLE_TOLERANCE_DEG,
                "{} @ {} G{:02}: azimuth {} vs {} (delta {azimuth_error:e} deg)",
                case.observer,
                case.epoch_iso,
                expected.prn,
                actual.azimuth_deg,
                expected.azimuth
            );
            assert!(
                elevation_error < ANGLE_TOLERANCE_DEG,
                "{} @ {} G{:02}: elevation {} vs {} (delta {elevation_error:e} deg)",
                case.observer,
                case.epoch_iso,
                expected.prn,
                actual.elevation_deg,
                expected.elevation
            );
            assert!(
                range_error < RANGE_TOLERANCE_M,
                "{} @ {} G{:02}: range {} vs {} m (delta {range_error:e} m)",
                case.observer,
                case.epoch_iso,
                expected.prn,
                actual.range_m,
                expected.range_km * 1000.0
            );

            worst_azimuth = worst_azimuth.max(azimuth_error);
            worst_elevation = worst_elevation.max(elevation_error);
            worst_range = worst_range.max(range_error);
            compared += 1;
        }
    }

    assert!(
        compared > 100,
        "expected a meaningful sample, got {compared}"
    );
    eprintln!(
        "cross-checked {compared} satellites over {} cases; \
         worst az {worst_azimuth:.3e} deg, el {worst_elevation:.3e} deg, \
         range {worst_range:.3e} m",
        vectors.cases.len()
    );
}

/// DOP tolerance, dimensionless.
///
/// This compares the *end* of two independent chains: the reference derives
/// its own az/el and inverts via SVD, we derive ours and eliminate on the
/// normal matrix. Forming `AᵀA` squares the condition number, so some spread
/// was expected — but the observed worst case over these vectors is 4e-15,
/// essentially machine precision, because a real GPS sky is well conditioned.
///
/// Set just tight enough to stay a real gate. A looser bound would pass even
/// if one side's inversion were meaningfully wrong, which would defeat the
/// point of keeping a second implementation around.
const DOP_TOLERANCE: f64 = 1e-12;

/// Cross-check the DOP scalars against the SVD-based reference.
///
/// Worth its own test rather than folding into the geometry comparison: DOP is
/// a different computation on top of the same angles, and a failure here with
/// the geometry test passing localises the bug immediately to the inversion.
#[test]
fn dop_matches_independent_python_reference() {
    let vectors = load_vectors();
    let bytes = load_nav_bytes(&vectors.source);
    let set = parse_nav(&bytes).expect("RINEX should parse");

    let mut worst: f64 = 0.0;
    let mut compared = 0usize;

    for case in &vectors.cases {
        let options = SkyplotOptions {
            elevation_mask_deg: case.elevation_mask_deg,
            ..SkyplotOptions::default()
        };
        let view = skyplot_from_set(
            &set,
            Geodetic::new(case.lat, case.lon, case.alt),
            GpsTime::from_unix_seconds(case.unix_seconds),
            &options,
        )
        .expect("sky view should compute");

        let ours = dop_for(&view.satellites);

        let (Some(ours), Some(expected)) = (ours, case.dop.as_ref()) else {
            // Both must agree that there is no solution; one finding a fix
            // where the other does not is itself a defect.
            assert_eq!(
                ours.is_none(),
                case.dop.is_none(),
                "{} @ {}: disagreement on whether a solution exists",
                case.observer,
                case.epoch_iso
            );
            continue;
        };

        assert_eq!(
            ours.satellites, expected.satellites,
            "{} @ {}: different satellite counts entered the solution",
            case.observer, case.epoch_iso
        );

        for (name, a, b) in [
            ("gdop", ours.gdop, expected.gdop),
            ("pdop", ours.pdop, expected.pdop),
            ("hdop", ours.hdop, expected.hdop),
            ("vdop", ours.vdop, expected.vdop),
            ("tdop", ours.tdop, expected.tdop),
        ] {
            let error = (a - b).abs();
            assert!(
                error < DOP_TOLERANCE,
                "{} @ {}: {name} {a} vs {b} (delta {error:e})",
                case.observer,
                case.epoch_iso
            );
            worst = worst.max(error);
            compared += 1;
        }
    }

    assert!(compared > 0, "no DOP cases were compared");
    eprintln!(
        "cross-checked {compared} DOP values over {} cases; worst delta {worst:.3e}",
        vectors.cases.len()
    );
}

/// Physical plausibility, independent of the reference: a GPS satellite is
/// 20 200 km away at the zenith and about 25 800 km at the horizon.
#[test]
fn ranges_are_physically_plausible() {
    let vectors = load_vectors();
    let bytes = load_nav_bytes(&vectors.source);
    let set = parse_nav(&bytes).unwrap();

    for case in &vectors.cases {
        let view = skyplot_from_set(
            &set,
            Geodetic::new(case.lat, case.lon, case.alt),
            GpsTime::from_unix_seconds(case.unix_seconds),
            &SkyplotOptions::default(),
        )
        .unwrap();

        for sat in &view.satellites {
            let range_km = sat.range_m / 1000.0;
            assert!(
                (19_000.0..27_000.0).contains(&range_km),
                "{} G{:02}: range {range_km} km is not a GPS range",
                case.observer,
                sat.sv.prn
            );
            assert!(
                (5.0..=90.0).contains(&sat.elevation_deg),
                "{} G{:02}: elevation {} deg violates the mask",
                case.observer,
                sat.sv.prn,
                sat.elevation_deg
            );

            // Geometry ties the two together: higher satellites are closer.
            // At 5 deg elevation the range must exceed 25 000 km; at 80 deg
            // it must be under 21 000 km.
            if sat.elevation_deg > 80.0 {
                assert!(range_km < 21_000.0, "high satellite too far: {range_km} km");
            }
            if sat.elevation_deg < 6.0 {
                assert!(
                    range_km > 24_500.0,
                    "low satellite too close: {range_km} km"
                );
            }
        }
    }
}

/// The unhealthy satellites in this file (G01 and G22, health word 63 for the
/// whole day) must never appear in a sky view under default settings.
#[test]
fn unhealthy_satellites_are_excluded() {
    let vectors = load_vectors();
    let bytes = load_nav_bytes(&vectors.source);
    let set = parse_nav(&bytes).unwrap();

    for case in &vectors.cases {
        let view = skyplot_from_set(
            &set,
            Geodetic::new(case.lat, case.lon, case.alt),
            GpsTime::from_unix_seconds(case.unix_seconds),
            &SkyplotOptions::default(),
        )
        .unwrap();

        for sat in &view.satellites {
            assert!(
                sat.sv.prn != 1 && sat.sv.prn != 22,
                "G{:02} is flagged unhealthy all day but appeared in the sky view",
                sat.sv.prn
            );
        }
    }
}

/// Parsing the real file must find every healthy GPS satellite and a full
/// day's worth of blocks.
#[test]
fn parses_the_expected_shape_of_file() {
    let vectors = load_vectors();
    let bytes = load_nav_bytes(&vectors.source);
    let set = parse_nav(&bytes).unwrap();

    let satellite_count = set.satellites().count();
    assert_eq!(
        satellite_count, 32,
        "expected all 32 GPS slots present in a daily broadcast file"
    );

    // ~12 blocks per satellite per day (one every two hours).
    let blocks = set.len();
    assert!(
        (300..600).contains(&blocks),
        "expected 300-600 ephemeris blocks in a daily file, got {blocks}"
    );
}

/// Gzip input must produce byte-identical results to plain input.
#[test]
fn gzip_and_plain_input_agree() {
    use std::io::Write;

    let vectors = load_vectors();
    let plain = load_nav_bytes(&vectors.source);

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&plain).unwrap();
    let gzipped = encoder.finish().unwrap();

    let from_plain = parse_nav(&plain).unwrap();
    let from_gzip = parse_nav(&gzipped).unwrap();
    assert_eq!(from_plain.len(), from_gzip.len());

    let case = &vectors.cases[0];
    let observer = Geodetic::new(case.lat, case.lon, case.alt);
    let t = GpsTime::from_unix_seconds(case.unix_seconds);
    let options = SkyplotOptions::default();

    let a = skyplot_from_set(&from_plain, observer, t, &options).unwrap();
    let b = skyplot_from_set(&from_gzip, observer, t, &options).unwrap();
    assert_eq!(a.satellites, b.satellites);
}
