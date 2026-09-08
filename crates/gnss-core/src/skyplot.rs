//! End-to-end sky view: RINEX bytes plus an observer and an instant, out to a
//! list of visible satellites with look angles.

use crate::ephemeris::{EphemerisSet, SelectionConfig, Sv};
use crate::geodesy::{geodetic_to_ecef, look_angles, Geodetic};
use crate::propagate::{apparent_position_any, PropagationConfig};
use crate::sbas::SbasProvider;
use crate::source::parse_nav;
use crate::time::GpsTime;
use crate::{Constellation, Error};

/// Knobs for a sky-view computation.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyplotOptions {
    /// Satellites below this elevation are omitted \[deg\].
    ///
    /// 5 deg is the usual default for a survey-grade receiver: below that,
    /// multipath and tropospheric delay make observations unreliable.
    pub elevation_mask_deg: f64,
    /// Constellations to include.
    ///
    /// Include [`Constellation::Sbas`] to enable SBAS at all; which *operators*
    /// are shown is then narrowed by [`Self::sbas_providers`].
    pub constellations: Vec<Constellation>,
    /// SBAS operators to include, when SBAS is enabled.
    ///
    /// Separate from `constellations` because SBAS is one RINEX constellation
    /// but many independent regional systems, and a receiver in CONUS has no
    /// use for satellites parked over the Indian Ocean.
    pub sbas_providers: Vec<SbasProvider>,
    pub selection: SelectionConfig,
    pub propagation: PropagationConfig,
}

impl Default for SkyplotOptions {
    fn default() -> Self {
        Self {
            elevation_mask_deg: 5.0,
            constellations: vec![Constellation::Gps],
            sbas_providers: vec![SbasProvider::Waas],
            selection: SelectionConfig::default(),
            propagation: PropagationConfig::default(),
        }
    }
}

/// One satellite as seen from the observer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatelliteView {
    pub sv: Sv,
    /// Azimuth \[deg\], 0 = true north, clockwise.
    pub azimuth_deg: f64,
    /// Elevation \[deg\] above the local horizon.
    pub elevation_deg: f64,
    /// Geometric range \[m\].
    pub range_m: f64,
    /// Signed age of the ephemeris used, `t - ToE` \[s\].
    ///
    /// Surfaced so the UI can flag results that lean on a stale or
    /// extrapolated block.
    pub ephemeris_age_s: f64,
    /// Issue of data of the ephemeris used.
    pub iode: f64,
    /// Whether this satellite is geostationary.
    ///
    /// Carried on the view rather than derived downstream because it is a
    /// property of the broadcast elements, and the only place those are in
    /// hand is here. A UI that tried to work it out from the satellite
    /// identifier would be back to a PRN table that goes stale -- and it is no
    /// longer only SBAS: BeiDou's C01-C05 and QZSS's J07/J08 are geostationary
    /// too.
    pub geostationary: bool,
}

/// The full result of a sky-view computation.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyView {
    pub observer: Geodetic,
    pub time: GpsTime,
    /// Satellites above the elevation mask, ordered by PRN.
    pub satellites: Vec<SatelliteView>,
    /// Satellites that had a usable ephemeris but fell below the mask.
    pub below_mask: usize,
    /// Satellites present in the file with no ephemeris valid at `time`.
    pub without_ephemeris: usize,
}

/// Parse `nav_bytes` and compute the sky view in one call.
///
/// Convenience wrapper over [`parse_nav`] and [`skyplot_from_set`]. Callers
/// evaluating many timestamps against the same file should parse once and use
/// [`skyplot_from_set`] instead -- parsing a daily file dominates the cost.
pub fn skyplot(
    nav_bytes: &[u8],
    observer: Geodetic,
    t: GpsTime,
    options: &SkyplotOptions,
) -> Result<SkyView, Error> {
    let set = parse_nav(nav_bytes)?;
    skyplot_from_set(&set, observer, t, options)
}

/// Compute the sky view from an already-parsed [`EphemerisSet`].
pub fn skyplot_from_set(
    set: &EphemerisSet,
    observer: Geodetic,
    t: GpsTime,
    options: &SkyplotOptions,
) -> Result<SkyView, Error> {
    let observer_ecef = geodetic_to_ecef(observer);

    let mut satellites = Vec::new();
    let mut below_mask = 0;
    let mut without_ephemeris = 0;

    for sv in set.satellites() {
        if !options.constellations.contains(&sv.constellation) {
            continue;
        }
        if sv.constellation == Constellation::Sbas
            && !options
                .sbas_providers
                .contains(&SbasProvider::from_prn(sv.prn))
        {
            continue;
        }

        let Some(eph) = set.select(sv, t, options.selection) else {
            without_ephemeris += 1;
            continue;
        };

        let position = apparent_position_any(eph, t, observer_ecef, &options.propagation)?;
        let angles = look_angles(position, observer);

        if angles.elevation_deg < options.elevation_mask_deg {
            below_mask += 1;
            continue;
        }

        satellites.push(SatelliteView {
            sv,
            azimuth_deg: angles.azimuth_deg,
            elevation_deg: angles.elevation_deg,
            range_m: angles.range_m,
            ephemeris_age_s: eph.age_at(t),
            iode: eph.issue_of_data(),
            geostationary: eph.is_geostationary(),
        });
    }

    // An empty sky is a legitimate answer (an observer can be masked out), but
    // "no satellite anywhere had a valid ephemeris" means the file does not
    // cover the requested instant, which is a caller error worth reporting.
    if satellites.is_empty() && below_mask == 0 && without_ephemeris > 0 {
        return Err(Error::NoEphemerisInRange);
    }

    satellites.sort_by_key(|s| (s.sv.constellation, s.sv.prn));

    Ok(SkyView {
        observer,
        time: t,
        satellites,
        below_mask,
        without_ephemeris,
    })
}
