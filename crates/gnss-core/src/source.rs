//! RINEX navigation input.
//!
//! The `rinex` crate is used here strictly as a parser. Its `nav` feature
//! offers `Ephemeris::kepler2position`, but that feature pulls in `anise`,
//! whose non-optional dependencies (`memmap2`, `ureq`, `pyo3`) do not build
//! for `wasm32-unknown-unknown`. We therefore lift the raw orbital elements
//! out of the parsed record and propagate them ourselves.
//! See `docs/adr/0005-hand-rolled-ephemeris-propagation.md`.

use std::io::{BufReader, Read};

use rinex::prelude::{Constellation as RinexConstellation, Rinex};

use crate::ephemeris::{BroadcastEphemeris, EphemerisSet, Sv};
use crate::time::GpsTime;
use crate::{Constellation, Error};

/// GPS week number at the BeiDou time epoch (2006-01-01T00:00:00 UTC).
const BDT_EPOCH_GPS_WEEK: u32 = 1356;

/// Parse a RINEX navigation file into an [`EphemerisSet`].
///
/// Accepts plain text or gzip-compressed input; the two are distinguished by
/// magic bytes, so callers do not have to care which one they fetched.
/// Satellites from constellations this crate cannot propagate (GLONASS, SBAS)
/// are skipped rather than treated as an error, since mixed-constellation
/// files are the norm.
pub fn parse_nav(bytes: &[u8]) -> Result<EphemerisSet, Error> {
    let decompressed;
    let payload: &[u8] = if is_gzip(bytes) {
        decompressed = gunzip(bytes)?;
        &decompressed
    } else {
        bytes
    };

    let mut reader = BufReader::new(payload);
    let rinex = Rinex::parse(&mut reader).map_err(|e| Error::Rinex(e.to_string()))?;

    if !rinex.is_navigation_rinex() {
        return Err(Error::NotNavigationRinex);
    }

    let mut set = EphemerisSet::new();
    for (key, frame) in rinex.nav_ephemeris_frames_iter() {
        let Some(constellation) = map_constellation(key.sv.constellation) else {
            continue;
        };
        let sv = Sv::new(constellation, key.sv.prn);
        if let Some(eph) = lift_ephemeris(sv, frame) {
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
/// Returning `None` means "we cannot propagate this", which is the correct
/// answer for GLONASS (state-vector ephemeris) and SBAS.
fn map_constellation(constellation: RinexConstellation) -> Option<Constellation> {
    match constellation {
        RinexConstellation::GPS => Some(Constellation::Gps),
        RinexConstellation::Galileo => Some(Constellation::Galileo),
        RinexConstellation::BeiDou => Some(Constellation::BeiDou),
        RinexConstellation::QZSS => Some(Constellation::Qzss),
        _ => None,
    }
}

/// Convert a constellation-native (week, seconds-of-week) pair to GPS time.
///
/// RINEX 3 reports Galileo and QZSS week numbers on the continuous GPS week
/// count, so those pass through. BeiDou reports BDT weeks counted from
/// 2006-01-01, and BDT runs 14 s behind GPS time.
fn to_gps_time(constellation: Constellation, week: u32, seconds_of_week: f64) -> GpsTime {
    match constellation {
        Constellation::Gps | Constellation::Galileo | Constellation::Qzss => {
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
/// Returns `None` if any required field is missing, which happens for frame
/// types that share the `Ephemeris` container but carry a different payload.
fn lift_ephemeris(sv: Sv, frame: &rinex::navigation::Ephemeris) -> Option<BroadcastEphemeris> {
    let field = |name: &str| frame.get_orbit_f64(name);

    let week = field("week")? as u32;
    let toe_seconds_of_week = field("toe")?;
    let toe = to_gps_time(sv.constellation, week, toe_seconds_of_week);

    Some(BroadcastEphemeris {
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

        delta_n: field("deltaN")?,
        i_dot: field("idot")?,
        omega_dot: field("omegaDot")?,

        cuc: field("cuc")?,
        cus: field("cus")?,
        crc: field("crc")?,
        crs: field("crs")?,
        cic: field("cic")?,
        cis: field("cis")?,

        af0: frame.clock_bias,
        af1: frame.clock_drift,
        af2: frame.clock_drift_rate,

        iode: field("iode").unwrap_or(f64::NAN),
        // Absent health word is treated as healthy: a missing field means the
        // record did not carry one, not that the satellite is unusable.
        health: field("health").map_or(0, |h| h as u16),
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
}
