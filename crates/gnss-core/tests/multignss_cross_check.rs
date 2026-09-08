//! Validation of Galileo, BeiDou and QZSS geometry, and of multi-system DOP,
//! against the independent Python reference.
//!
//! The GPS equivalent lives in `reference_cross_check.rs`; this file covers
//! everything phase 5 switched on. The reference (`tools/reference_skyplot.py`)
//! shares no code with the Rust: fixed-column RINEX parsing instead of the
//! `rinex` crate, fixed-point Kepler iteration instead of Newton-Raphson,
//! `asin` for elevation instead of `atan2`, and — for DOP — an explicit design
//! matrix put through an SVD pseudo-inverse where the Rust accumulates the
//! normal matrix and eliminates.
//!
//! The DOP cases are the point of the exercise. Each case names the systems it
//! spans, and the two implementations must agree not only on the numbers but
//! on *how many clock unknowns* the solution carried: a mismatch there means
//! one of them is solving a different problem.
//!
//! Regenerate with:
//!     python3 tools/reference_skyplot.py --emit-multi-vectors \
//!         > crates/gnss-core/tests/vectors/multignss_vectors.json
//!
//! Input data is real broadcast data: four hours of BKG's BRDC00WRD product
//! for 2026-08-08, trimmed to GPS, Galileo, BeiDou and QZSS.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gnss_core::{
    dop_for, parse_nav, skyplot_from_set, Constellation, EphemerisSet, Geodetic, GpsTime,
    SatelliteView, SkyplotOptions,
};
use serde::Deserialize;

/// Angular agreement required between the two implementations.
///
/// The same 1e-7 deg the GPS cross-check uses. Both are f64 evaluations of the
/// same closed-form expressions, so anything above this means the algorithms
/// genuinely differ.
const ANGLE_TOLERANCE_DEG: f64 = 1e-7;

/// Range agreement, in metres. Ranges are ~2e7-4e7 m, so this is ~1e-14
/// relative.
const RANGE_TOLERANCE_M: f64 = 1e-4;

/// DOP agreement. The two reach `Q` by different algorithms — SVD
/// pseudo-inverse against Gauss-Jordan on the normal matrix — so this is a
/// looser bound than the geometry, but still far tighter than any real
/// disagreement would be.
const DOP_TOLERANCE: f64 = 1e-9;

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
    /// RINEX constellation codes this case includes, e.g. `GECJ`.
    systems: String,
    satellites: Vec<ReferenceSatellite>,
    /// `null` where the reference found no solution.
    dop: Option<ReferenceDop>,
}

#[derive(Deserialize)]
struct ReferenceSatellite {
    sv: String,
    azimuth: f64,
    elevation: f64,
    range_km: f64,
}

#[derive(Deserialize)]
struct ReferenceDop {
    gdop: f64,
    pdop: f64,
    hdop: f64,
    vdop: f64,
    tdop: f64,
    satellites: usize,
    /// Distinct clock unknowns the reference solved for.
    systems: usize,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_vectors() -> Vectors {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/multignss_vectors.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("malformed multi-GNSS vectors")
}

fn load_set(source: &str) -> EphemerisSet {
    let path = repo_root().join("data").join(source);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_nav(&bytes).expect("fixture should parse")
}

/// Map RINEX constellation codes onto the enum the options take.
fn constellations_for(codes: &str) -> Vec<Constellation> {
    codes
        .chars()
        .map(|code| match code {
            'G' => Constellation::Gps,
            'E' => Constellation::Galileo,
            'C' => Constellation::BeiDou,
            'J' => Constellation::Qzss,
            other => panic!("unexpected constellation code {other}"),
        })
        .collect()
}

fn options_for(case: &Case) -> SkyplotOptions {
    SkyplotOptions {
        elevation_mask_deg: case.elevation_mask_deg,
        constellations: constellations_for(&case.systems),
        ..SkyplotOptions::default()
    }
}

/// The headline test: every satellite in every case must match the reference.
#[test]
fn matches_the_independent_python_reference() {
    let vectors = load_vectors();
    let set = load_set(&vectors.source);

    let mut worst_azimuth: f64 = 0.0;
    let mut worst_elevation: f64 = 0.0;
    let mut worst_range: f64 = 0.0;
    let mut compared = 0usize;

    for case in &vectors.cases {
        let t = GpsTime::from_unix_seconds(case.unix_seconds);
        assert!(
            (t.seconds() - case.gps_seconds).abs() < 1e-6,
            "{} @ {}: GPS time mismatch, rust={} python={}",
            case.observer,
            case.epoch_iso,
            t.seconds(),
            case.gps_seconds
        );

        let view = skyplot_from_set(
            &set,
            Geodetic::new(case.lat, case.lon, case.alt),
            t,
            &options_for(case),
        )
        .unwrap_or_else(|e| panic!("{} @ {}: {e}", case.observer, case.epoch_iso));

        let ours: BTreeMap<String, _> = view
            .satellites
            .iter()
            .map(|s| (s.sv.to_string(), s))
            .collect();

        // Set equality first: a satellite one implementation sees and the
        // other does not is a bigger failure than a numeric disagreement, and
        // comparing only the intersection would hide it.
        let theirs: Vec<&str> = case.satellites.iter().map(|s| s.sv.as_str()).collect();
        let mine: Vec<&str> = ours.keys().map(String::as_str).collect();
        assert_eq!(
            mine, theirs,
            "{} @ {} [{}]: different satellites above the mask",
            case.observer, case.epoch_iso, case.systems
        );

        for reference in &case.satellites {
            let sat = ours[&reference.sv];

            // Azimuth wraps at 360, so compare the shorter way round.
            let d_azimuth = {
                let raw = (sat.azimuth_deg - reference.azimuth).abs();
                raw.min(360.0 - raw)
            };
            let d_elevation = (sat.elevation_deg - reference.elevation).abs();
            let d_range = (sat.range_m - reference.range_km * 1000.0).abs();

            assert!(
                d_azimuth < ANGLE_TOLERANCE_DEG,
                "{} @ {} {}: azimuth {} vs {} (delta {d_azimuth})",
                case.observer,
                case.epoch_iso,
                reference.sv,
                sat.azimuth_deg,
                reference.azimuth
            );
            assert!(
                d_elevation < ANGLE_TOLERANCE_DEG,
                "{} @ {} {}: elevation {} vs {} (delta {d_elevation})",
                case.observer,
                case.epoch_iso,
                reference.sv,
                sat.elevation_deg,
                reference.elevation
            );
            assert!(
                d_range < RANGE_TOLERANCE_M,
                "{} @ {} {}: range {} m vs {} m (delta {d_range})",
                case.observer,
                case.epoch_iso,
                reference.sv,
                sat.range_m,
                reference.range_km * 1000.0
            );

            worst_azimuth = worst_azimuth.max(d_azimuth);
            worst_elevation = worst_elevation.max(d_elevation);
            worst_range = worst_range.max(d_range);
            compared += 1;
        }
    }

    assert!(compared > 500, "expected a broad comparison, got {compared}");
    println!(
        "compared {compared} satellites across {} cases; worst az {worst_azimuth:.3e} deg, \
         el {worst_elevation:.3e} deg, range {worst_range:.3e} m",
        vectors.cases.len()
    );
}

/// DOP must agree, and must agree about how many clocks it solved for.
#[test]
fn multi_system_dop_matches_the_reference() {
    let vectors = load_vectors();
    let set = load_set(&vectors.source);

    let mut worst: f64 = 0.0;
    let mut compared = 0usize;

    for case in &vectors.cases {
        let view = skyplot_from_set(
            &set,
            Geodetic::new(case.lat, case.lon, case.alt),
            GpsTime::from_unix_seconds(case.unix_seconds),
            &options_for(case),
        )
        .unwrap();

        let ours = dop_for(&view.satellites);
        match (&ours, &case.dop) {
            (None, None) => continue,
            (Some(a), Some(b)) => {
                assert_eq!(
                    a.satellites, b.satellites,
                    "{} @ {} [{}]: satellite count",
                    case.observer, case.epoch_iso, case.systems
                );
                // The count of clock unknowns is the whole multi-system
                // question: agreeing on DOP while disagreeing here would mean
                // agreeing by accident.
                assert_eq!(
                    a.systems, b.systems,
                    "{} @ {} [{}]: clock unknowns",
                    case.observer, case.epoch_iso, case.systems
                );

                for (name, mine, theirs) in [
                    ("gdop", a.gdop, b.gdop),
                    ("pdop", a.pdop, b.pdop),
                    ("hdop", a.hdop, b.hdop),
                    ("vdop", a.vdop, b.vdop),
                    ("tdop", a.tdop, b.tdop),
                ] {
                    let delta = (mine - theirs).abs();
                    assert!(
                        delta < DOP_TOLERANCE,
                        "{} @ {} [{}]: {name} {mine} vs {theirs} (delta {delta})",
                        case.observer,
                        case.epoch_iso,
                        case.systems
                    );
                    worst = worst.max(delta);
                }
                compared += 1;
            }
            (a, b) => panic!(
                "{} @ {} [{}]: one implementation found a solution and the other did not \
                 (rust={:?}, python={:?})",
                case.observer,
                case.epoch_iso,
                case.systems,
                a.is_some(),
                b.is_some()
            ),
        }
    }

    assert!(compared > 30, "expected every case to solve, got {compared}");
    println!("compared DOP over {compared} cases; worst delta {worst:.3e}");
}

/// The number of clock unknowns must track the constellations actually
/// switched on, not the number of satellites or the file contents.
#[test]
fn the_clock_count_follows_the_selected_constellations() {
    let vectors = load_vectors();
    let set = load_set(&vectors.source);
    let case = &vectors.cases[0];
    let observer = Geodetic::new(case.lat, case.lon, case.alt);
    let t = GpsTime::from_unix_seconds(case.unix_seconds);

    for (codes, expected) in [("G", 1), ("E", 1), ("GE", 2), ("GJ", 1), ("GECJ", 3)] {
        let options = SkyplotOptions {
            elevation_mask_deg: 5.0,
            constellations: constellations_for(codes),
            ..SkyplotOptions::default()
        };
        let view = skyplot_from_set(&set, observer, t, &options).unwrap();
        let dop = dop_for(&view.satellites).expect("a real sky has a solution");
        assert_eq!(
            dop.systems, expected,
            "{codes} should solve for {expected} clock(s), got {}",
            dop.systems
        );
    }
}

/// Parsing the real file must find the constellations the fixture contains,
/// including the geostationary satellites that are easiest to lose.
#[test]
fn parses_the_expected_shape_of_file() {
    let vectors = load_vectors();
    let set = load_set(&vectors.source);

    let mut per_constellation: BTreeMap<Constellation, usize> = BTreeMap::new();
    for sv in set.satellites() {
        *per_constellation.entry(sv.constellation).or_default() += 1;
    }

    assert_eq!(per_constellation.get(&Constellation::Gps), Some(&32));
    assert!(
        per_constellation[&Constellation::Galileo] >= 28,
        "expected most of the Galileo constellation, got {:?}",
        per_constellation.get(&Constellation::Galileo)
    );
    assert!(
        per_constellation[&Constellation::BeiDou] >= 30,
        "expected most of BeiDou, got {:?}",
        per_constellation.get(&Constellation::BeiDou)
    );

    // QZSS is the one that silently vanished: J07 and J08 broadcast `deltaN`
    // and `idot` as exact zeros, which the `rinex` crate does not surface, and
    // treating an absent perturbation as a missing field dropped both.
    let qzss: Vec<String> = set
        .satellites()
        .filter(|sv| sv.constellation == Constellation::Qzss)
        .map(|sv| sv.to_string())
        .collect();
    assert!(
        qzss.contains(&"J07".to_string()) && qzss.contains(&"J08".to_string()),
        "expected QZSS's geostationary satellites, got {qzss:?}"
    );
}

/// The geostationary flag has to be right for every constellation that flies
/// one, because the sky plot draws those satellites differently and because
/// BeiDou's take a different propagation path.
///
/// Checked against the orbits themselves rather than a PRN list: over three
/// hours a geostationary satellite stays within a few degrees of where it was
/// on the sky, and everything else sweeps tens of degrees. In this fixture the
/// two populations are 2.6 deg and 19.3 deg at their closest — the nearest
/// non-GEO is a BeiDou IGSO, which is geosynchronous but inclined, and tracing
/// a figure-eight is exactly the thing the flag must not call standing still.
///
/// The measure is angular distance across the sky, not change in elevation.
/// Elevation alone does not separate the two populations — a MEO caught at the
/// top of its arc barely changes elevation while racing across in azimuth —
/// and using it would make this a test of which satellites happened to be
/// turning over.
#[test]
fn the_geostationary_flag_matches_what_the_orbits_do() {
    /// Great-circle angle between two look directions [deg].
    fn swept(a: &SatelliteView, b: &SatelliteView) -> f64 {
        let (e1, e2) = (a.elevation_deg.to_radians(), b.elevation_deg.to_radians());
        let delta_az = (a.azimuth_deg - b.azimuth_deg).to_radians();
        (e1.sin() * e2.sin() + e1.cos() * e2.cos() * delta_az.cos())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    let vectors = load_vectors();
    let set = load_set(&vectors.source);

    let options = SkyplotOptions {
        elevation_mask_deg: 0.0,
        constellations: vec![
            Constellation::Gps,
            Constellation::Galileo,
            Constellation::BeiDou,
            Constellation::Qzss,
        ],
        ..SkyplotOptions::default()
    };
    // Over south-east Asia, where BeiDou's and QZSS's geostationary satellites
    // are all well up.
    let observer = Geodetic::new(20.0, 110.0, 0.0);
    let base = GpsTime::from_unix_seconds(1_786_186_800.0); // 2026-08-08T11:00Z

    let at = |t: GpsTime| {
        skyplot_from_set(&set, observer, t, &options)
            .unwrap()
            .satellites
            .into_iter()
            .map(|s| (s.sv.to_string(), s))
            .collect::<BTreeMap<_, _>>()
    };

    let first = at(base);
    let later = at(base.offset_by(3.0 * 3600.0));

    let mut stationary = Vec::new();
    for (sv, early) in &first {
        let Some(late) = later.get(sv) else { continue };
        let moved = swept(early, late);

        if early.geostationary {
            // Not zero: real geostationary satellites are allowed a small
            // inclination and a slow drift, and BeiDou's C02 and C03 use it.
            assert!(
                moved < 5.0,
                "{sv} is flagged geostationary but swept {moved:.1} deg across the sky in three hours"
            );
            stationary.push(sv.clone());
        } else {
            assert!(
                moved > 10.0,
                "{sv} is not flagged geostationary but swept only {moved:.1} deg in three hours"
            );
        }
    }

    // BeiDou and QZSS must both contribute, or this is only re-testing SBAS
    // under another name.
    assert!(
        stationary.iter().any(|sv| sv.starts_with('C')),
        "expected BeiDou geostationary satellites, got {stationary:?}"
    );
    assert!(
        stationary.iter().any(|sv| sv.starts_with('J')),
        "expected QZSS geostationary satellites, got {stationary:?}"
    );
}
