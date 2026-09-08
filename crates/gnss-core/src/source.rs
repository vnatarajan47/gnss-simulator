//! RINEX navigation input.
//!
//! The `rinex` crate is used here strictly as a parser. Its `nav` feature
//! offers `Ephemeris::kepler2position`, but that feature pulls in `anise`,
//! whose non-optional dependencies (`memmap2`, `ureq`, `pyo3`) do not build
//! for `wasm32-unknown-unknown`. We therefore lift the raw orbital elements
//! out of the parsed record and propagate them ourselves.
//! See `docs/adr/0005-hand-rolled-ephemeris-propagation.md`.

use std::io::{BufReader, Read};
use std::panic::AssertUnwindSafe;

use rinex::prelude::{Constellation as RinexConstellation, Rinex};

use crate::ephemeris::{EphemerisSet, KeplerianEphemeris, Sv};
use crate::geodesy::Ecef;
use crate::sbas::{self, SbasEphemeris};
use crate::time::GpsTime;
use crate::{Constellation, Error};

/// GPS week number at the BeiDou time epoch (2006-01-01T00:00:00 UTC).
const BDT_EPOCH_GPS_WEEK: u32 = 1356;

/// Parse a RINEX navigation file into an [`EphemerisSet`].
///
/// Accepts plain text or gzip-compressed input; the two are distinguished by
/// magic bytes, so callers do not have to care which one they fetched.
/// Satellites from constellations this crate cannot propagate (GLONASS) are
/// skipped rather than treated as an error, since mixed-constellation files
/// are the norm.
pub fn parse_nav(bytes: &[u8]) -> Result<EphemerisSet, Error> {
    let decompressed;
    let payload: &[u8] = if is_gzip(bytes) {
        decompressed = gunzip(bytes)?;
        &decompressed
    } else {
        bytes
    };

    // The `rinex` parser panics rather than erroring on some malformed input
    // (e.g. a zero-length line: parsing.rs slices `[4..]` unchecked). We feed
    // it files fetched from a 9-year archive spanning several RINEX revisions,
    // so that is reachable in normal use — and in WASM an escaping panic traps
    // and poisons the module, killing the page rather than showing an error.
    // Contain it here so callers only ever see an `Error`.
    let rinex = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut reader = BufReader::new(payload);
        Rinex::parse(&mut reader)
    }))
    .map_err(|_| Error::ParserPanicked)?
    .map_err(|e| Error::Rinex(e.to_string()))?;

    if !rinex.is_navigation_rinex() {
        return Err(Error::NotNavigationRinex);
    }

    let mut set = EphemerisSet::new();
    for (key, frame) in rinex.nav_ephemeris_frames_iter() {
        let Some(constellation) = map_constellation(key.sv.constellation) else {
            continue;
        };
        let sv = Sv::new(constellation, normalise_prn(constellation, key.sv.prn));

        if constellation == Constellation::Sbas {
            if let Some(eph) = lift_sbas(sv, key.epoch, frame) {
                set.insert(eph);
            }
        } else if let Some(eph) = lift_ephemeris(sv, frame) {
            set.insert(eph);
        }
    }

    Ok(set)
}

fn is_gzip(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decompression(e.to_string()))?;
    Ok(out)
}

/// Map the `rinex` crate's constellation enum onto ours.
///
/// Returning `None` means "we cannot propagate this". That is now true only of
/// GLONASS, which broadcasts a state vector *and* requires numerical
/// integration of the equations of motion, unlike SBAS.
fn map_constellation(constellation: RinexConstellation) -> Option<Constellation> {
    match constellation {
        RinexConstellation::GPS => Some(Constellation::Gps),
        RinexConstellation::Galileo => Some(Constellation::Galileo),
        RinexConstellation::BeiDou => Some(Constellation::BeiDou),
        RinexConstellation::QZSS => Some(Constellation::Qzss),
        other if other.is_sbas() => Some(Constellation::Sbas),
        _ => None,
    }
}

/// Convert a parsed satellite number to a true PRN.
///
/// RINEX writes SBAS satellites as `S<PRN-100>`, so PRN 131 appears as `S31`,
/// and the `rinex` crate surfaces that field verbatim as `prn = 31`. We store
/// the real PRN internally, because that is what provider assignments
/// ([`sbas::SbasProvider::from_prn`]) are published against. Formatting puts
/// the two-digit form back for display.
///
/// The `< 100` guard makes this idempotent, so a source that already reports
/// true PRNs is handled correctly too.
fn normalise_prn(constellation: Constellation, prn: u8) -> u8 {
    match constellation {
        Constellation::Sbas if prn < 100 => prn.saturating_add(100),
        _ => prn,
    }
}

/// Convert a constellation-native (week, seconds-of-week) pair to GPS time.
///
/// RINEX 3 reports Galileo and QZSS week numbers on the continuous GPS week
/// count, so those pass through. BeiDou reports BDT weeks counted from
/// 2006-01-01, and BDT runs 14 s behind GPS time.
fn to_gps_time(constellation: Constellation, week: u32, seconds_of_week: f64) -> GpsTime {
    match constellation {
        // SBAS never reaches here -- its records carry no week number and are
        // timestamped from the record epoch instead (see `lift_sbas`).
        Constellation::Gps | Constellation::Galileo | Constellation::Qzss | Constellation::Sbas => {
            GpsTime::from_week_and_sow(week, seconds_of_week)
        }
        Constellation::BeiDou => {
            GpsTime::from_week_and_sow(week + BDT_EPOCH_GPS_WEEK, seconds_of_week)
                .offset_by(constellation.seconds_to_gpst())
        }
    }
}

/// Pull the orbital elements out of a parsed `rinex` ephemeris frame.
///
/// Returns `None` only if one of the *defining* elements is missing, which
/// happens for frame types that share the `Ephemeris` container but carry a
/// different payload. Perturbation terms — the harmonic corrections and the
/// two rate terms — default to zero instead.
///
/// That distinction is not pedantry. The `rinex` crate does not surface an
/// orbit field whose broadcast value is exactly zero, so a legitimately zero
/// perturbation is indistinguishable from an absent one. Requiring them all
/// silently discarded QZSS's two geostationary satellites (J07, J08), which
/// broadcast `deltaN` and `idot` as exact zeros. A missing perturbation means
/// no perturbation, and zero is the right reading; a missing semi-major axis
/// means the record is not an orbit at all.
fn lift_ephemeris(sv: Sv, frame: &rinex::navigation::Ephemeris) -> Option<KeplerianEphemeris> {
    let field = |name: &str| frame.get_orbit_f64(name);
    // Perturbations and rates: absent is zero, see above.
    let rate_or_zero = |name: &str| field(name).unwrap_or(0.0);

    let week = field("week")? as u32;
    let toe_seconds_of_week = field("toe")?;
    let toe = to_gps_time(sv.constellation, week, toe_seconds_of_week);

    Some(KeplerianEphemeris {
        sv,
        toe,
        toe_seconds_of_week,
        // ToC and ToE coincide for the great majority of broadcast records;
        // where they differ it affects clock correction only, not geometry.
        toc: toe,

        sqrt_a: field("sqrta")?,
        eccentricity: field("e")?,
        i0: field("i0")?,
        omega0: field("omega0")?,
        argument_of_perigee: field("omega")?,
        mean_anomaly_0: field("m0")?,

        delta_n: rate_or_zero("deltaN"),
        i_dot: rate_or_zero("idot"),
        omega_dot: rate_or_zero("omegaDot"),

        cuc: rate_or_zero("cuc"),
        cus: rate_or_zero("cus"),
        crc: rate_or_zero("crc"),
        crs: rate_or_zero("crs"),
        cic: rate_or_zero("cic"),
        cis: rate_or_zero("cis"),

        af0: frame.clock_bias,
        af1: frame.clock_drift,
        af2: frame.clock_drift_rate,

        // Issue of data goes by a different name in each ICD, and the parser
        // keeps the ICD's name. Reading only `iode` left Galileo and BeiDou
        // with NaN, which quietly disabled both the duplicate-block check in
        // `EphemerisSet::insert` (NaN != NaN, so re-broadcasts across the
        // midnight overlap accumulated) and the selection tie-break.
        iode: field("iode")
            .or_else(|| field("iodnav"))
            .or_else(|| field("aode"))
            .unwrap_or(f64::NAN),
        // Absent health word is treated as healthy: a missing field means the
        // record did not carry one, not that the satellite is unusable.
        health: field("health").map_or(0, |h| h as u16),
    })
}

/// Pull an SBAS state vector out of a parsed GEO navigation frame.
///
/// The RINEX field names below (`satPosX`, `velX`, `accelX`, ...) are the
/// `rinex` crate's identifiers for the GEO message layout. Units are decided
/// physically rather than trusted, because this product mixes kilometres and
/// metres between providers -- see [`sbas::decode_scale`].
///
/// Returns `None` for records whose position is not a plausible geostationary
/// radius under either interpretation, which discards the zero-filled
/// placeholder frames that also appear in these files.
fn lift_sbas(
    sv: Sv,
    epoch: rinex::prelude::Epoch,
    frame: &rinex::navigation::Ephemeris,
) -> Option<SbasEphemeris> {
    let field = |name: &str| frame.get_orbit_f64(name).unwrap_or(0.0);

    // Absent reads as zero here for the same reason as in `lift_ephemeris`:
    // the parser does not surface an orbit field whose value is exactly zero,
    // and a geostationary satellite sitting exactly on the equator broadcasts
    // `satPosZ` as an exact zero. Requiring the field present discarded every
    // such record -- which was all of EGNOS and SouthPAN. Nothing is lost by
    // defaulting: a genuinely absent position collapses to the origin, and
    // `decode_scale` rejects that along with the other implausible radii.
    let (px, py, pz) = (field("satPosX"), field("satPosY"), field("satPosZ"));

    // Rejects placeholders as well as deciding km vs m.
    let scale = sbas::decode_scale(px, py, pz)?;

    let (vx, vy, vz) = (field("velX"), field("velY"), field("velZ"));
    let (ax, ay, az) = (field("accelX"), field("accelY"), field("accelZ"));

    // SBAS has no week/ToE fields: the state vector's reference epoch is the
    // record's own epoch, which the parser has already resolved.
    let toe = GpsTime::from_seconds(epoch.to_gpst_seconds());

    Some(SbasEphemeris {
        sv,
        toe,
        position_m: Ecef::new(px * scale, py * scale, pz * scale),
        velocity_m_s: Ecef::new(vx * scale, vy * scale, vz * scale),
        acceleration_m_s2: Ecef::new(ax * scale, ay * scale, az * scale),
        health: field("health") as u16,
        accuracy_index: field("accuracy"),
        iodn: field("iodn"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beidou_week_maps_onto_the_gps_week_count() {
        // BDT week 0 / sow 0 is 2006-01-01T00:00:00 UTC, which is GPS week
        // 1356 sow 14 (GPS-UTC was 14 s then).
        let t = to_gps_time(Constellation::BeiDou, 0, 0.0);
        assert_eq!(t.week(), BDT_EPOCH_GPS_WEEK);
        assert!((t.seconds_of_week() - 14.0).abs() < 1e-9);
    }

    #[test]
    fn gps_week_passes_through_unchanged() {
        let t = to_gps_time(Constellation::Gps, 2347, 259_200.0);
        assert_eq!(t.week(), 2347);
        assert!((t.seconds_of_week() - 259_200.0).abs() < 1e-9);
    }

    /// RINEX writes SBAS as `S<PRN-100>`; we store the true PRN so provider
    /// lookup works. Must be idempotent.
    #[test]
    fn sbas_prns_are_lifted_to_true_prns() {
        assert_eq!(normalise_prn(Constellation::Sbas, 31), 131);
        assert_eq!(normalise_prn(Constellation::Sbas, 35), 135);
        assert_eq!(normalise_prn(Constellation::Sbas, 131), 131);
        // Other constellations are untouched.
        assert_eq!(normalise_prn(Constellation::Gps, 31), 31);
    }

    #[test]
    fn gzip_is_detected_by_magic_bytes() {
        assert!(is_gzip(&[0x1f, 0x8b, 0x08, 0x00]));
        assert!(!is_gzip(b"     3.05           N: GNSS NAV DATA"));
        assert!(!is_gzip(&[]));
    }

    #[test]
    fn non_navigation_input_is_rejected() {
        let err = parse_nav(b"not a rinex file at all\n");
        assert!(err.is_err());
    }

    /// A zero-length line inside the record body makes the `rinex` parser
    /// panic (parsing.rs slices `[4..]` unchecked). Real archive files end
    /// with a trailing newline, so this is reachable from ordinary input --
    /// it must surface as an `Error`, never as an escaping panic.
    #[test]
    fn a_parser_panic_is_contained_as_an_error() {
        let malformed = concat!(
            "     3.03           N: GNSS NAV DATA    G: GPS              RINEX VERSION / TYPE\n",
            "                                                            END OF HEADER\n",
            "G01 2019 03 15 00 00 00-1.809163950384e-04-7.503331289627e-12 0.000000000000e+00\n",
            "\n",
            "\n",
        );

        // Keep the panic hook quiet so the test output stays readable.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = parse_nav(malformed.as_bytes());
        std::panic::set_hook(previous);

        assert!(
            matches!(result, Err(Error::ParserPanicked)),
            "expected a contained ParserPanicked error, got {result:?}"
        );
    }
}
