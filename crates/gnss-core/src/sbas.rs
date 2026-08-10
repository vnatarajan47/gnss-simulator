//! SBAS: satellite-based augmentation systems (WAAS, EGNOS, MSAS, ...).
//!
//! SBAS satellites are geostationary and broadcast an **ECEF state vector**
//! — position, velocity and acceleration at a reference epoch — rather than
//! the Keplerian element set used by GPS/Galileo/BeiDou/QZSS. Propagation is
//! therefore a second-order Taylor expansion, not an orbit solve.
//!
//! Reference: RINEX 3.05 §6.10 (GEO navigation message), RTCA DO-229 MT 9.

use crate::ephemeris::Sv;
use crate::geodesy::Ecef;
use crate::time::GpsTime;

/// Plausible geostationary orbit radius \[m\].
///
/// Nominal GEO radius is 42 164 km. The band is generous enough to admit
/// inclined and drifting GEOs while still separating metres from kilometres
/// by three orders of magnitude.
const GEO_RADIUS_MIN_M: f64 = 3.5e7;
const GEO_RADIUS_MAX_M: f64 = 4.6e7;

/// The organisation operating an SBAS satellite.
///
/// Providers are identified by PRN. Assignments are allocated centrally and do
/// change as satellites are retired and replaced, so [`SbasProvider::from_prn`]
/// is the single place to update — nothing else in the crate hard-codes a PRN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SbasProvider {
    /// Wide Area Augmentation System — United States.
    Waas,
    /// European Geostationary Navigation Overlay Service.
    Egnos,
    /// Multi-functional Satellite Augmentation System — Japan.
    Msas,
    /// GPS-Aided GEO Augmented Navigation — India.
    Gagan,
    /// System for Differential Corrections and Monitoring — Russia.
    Sdcm,
    /// BeiDou SBAS — China.
    BdSbas,
    /// Korea Augmentation Satellite System.
    Kass,
    /// Southern Positioning Augmentation Network — Australia / New Zealand.
    SouthPan,
    /// A PRN in the SBAS range with no assignment known to this table.
    Unknown,
}

impl SbasProvider {
    /// Map an SBAS PRN (120-158) to its operator.
    ///
    /// Assignments as published for 2025-2026. Adding a satellite is one line
    /// here; nothing downstream needs to change.
    pub const fn from_prn(prn: u8) -> Self {
        match prn {
            131 | 133 | 135 | 138 => SbasProvider::Waas,
            120 | 121 | 123 | 126 | 136 => SbasProvider::Egnos,
            129 | 137 => SbasProvider::Msas,
            127 | 128 | 132 => SbasProvider::Gagan,
            125 | 140 | 141 => SbasProvider::Sdcm,
            130 | 143 | 144 => SbasProvider::BdSbas,
            134 => SbasProvider::Kass,
            122 => SbasProvider::SouthPan,
            _ => SbasProvider::Unknown,
        }
    }

    /// Short identifier, used as the on/off key across the WASM boundary.
    pub const fn key(self) -> &'static str {
        match self {
            SbasProvider::Waas => "WAAS",
            SbasProvider::Egnos => "EGNOS",
            SbasProvider::Msas => "MSAS",
            SbasProvider::Gagan => "GAGAN",
            SbasProvider::Sdcm => "SDCM",
            SbasProvider::BdSbas => "BDSBAS",
            SbasProvider::Kass => "KASS",
            SbasProvider::SouthPan => "SOUTHPAN",
            SbasProvider::Unknown => "SBAS-OTHER",
        }
    }

    /// Parse a [`SbasProvider::key`] back into a provider.
    pub fn from_key(key: &str) -> Option<Self> {
        const ALL: &[SbasProvider] = &[
            SbasProvider::Waas,
            SbasProvider::Egnos,
            SbasProvider::Msas,
            SbasProvider::Gagan,
            SbasProvider::Sdcm,
            SbasProvider::BdSbas,
            SbasProvider::Kass,
            SbasProvider::SouthPan,
            SbasProvider::Unknown,
        ];
        ALL.iter()
            .copied()
            .find(|provider| provider.key().eq_ignore_ascii_case(key))
    }
}

/// A broadcast SBAS state vector.
///
/// Position, velocity and acceleration are stored in SI (m, m/s, m/s²)
/// regardless of how the source file encoded them — see [`decode_scale`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SbasEphemeris {
    pub sv: Sv,
    /// Reference epoch of the state vector (the record's time of clock).
    pub toe: GpsTime,
    pub position_m: Ecef,
    /// Velocity \[m/s\], as an ECEF 3-vector.
    pub velocity_m_s: Ecef,
    /// Acceleration \[m/s²\], as an ECEF 3-vector.
    pub acceleration_m_s2: Ecef,
    /// Raw health word.
    ///
    /// Deliberately *not* used to filter. Unlike the GPS health word, the SBAS
    /// field in the merged IGS broadcast product is not reliably populated: it
    /// is dominated by all-ones fillers (31, 63) even for satellites that are
    /// operational — every WAAS record carries 31. Treating non-zero as
    /// unhealthy would silently discard the entire constellation.
    pub health: u16,
    /// Broadcast accuracy (URA) index.
    pub accuracy_index: f64,
    /// Issue of data, navigation.
    pub iodn: f64,
}

impl SbasEphemeris {
    /// Signed age of this record relative to `t` \[s\].
    pub fn age_at(&self, t: GpsTime) -> f64 {
        t.seconds_since(self.toe)
    }

    /// ECEF position at `t`, by second-order Taylor expansion about `toe`.
    ///
    /// This is the propagation the SBAS message is designed for: the state
    /// vector is refreshed every few minutes and is only intended for short
    /// extrapolation, so there is no orbit model to integrate.
    pub fn position_at(&self, t: GpsTime) -> Ecef {
        let dt = t.seconds_since(self.toe);
        let half_dt_sq = 0.5 * dt * dt;
        Ecef::new(
            self.position_m.x + self.velocity_m_s.x * dt + self.acceleration_m_s2.x * half_dt_sq,
            self.position_m.y + self.velocity_m_s.y * dt + self.acceleration_m_s2.y * half_dt_sq,
            self.position_m.z + self.velocity_m_s.z * dt + self.acceleration_m_s2.z * half_dt_sq,
        )
    }
}

/// Work out the scale factor that converts a raw RINEX GEO position to metres.
///
/// RINEX 3 specifies kilometres for the GEO position/velocity/acceleration
/// triplet, but the merged IGS broadcast product is not consistent: within a
/// single file, some providers' records are in kilometres (spec-compliant)
/// and others are in metres. Roughly half the records for PRNs 121, 123, 127,
/// 128, 136 and 144 in a 2026 file are metres, while WAAS is uniformly
/// kilometres.
///
/// Rather than trusting a per-file or per-PRN rule, decide physically: an SBAS
/// satellite is geostationary, so exactly one interpretation puts it at a
/// plausible GEO radius. Returns `None` when neither does, which rejects the
/// zero-filled placeholder records that also appear in these files.
pub fn decode_scale(x: f64, y: f64, z: f64) -> Option<f64> {
    // Kilometres first: that is what the specification says.
    for scale in [1000.0, 1.0] {
        let radius = ((x * scale).powi(2) + (y * scale).powi(2) + (z * scale).powi(2)).sqrt();
        if (GEO_RADIUS_MIN_M..=GEO_RADIUS_MAX_M).contains(&radius) {
            return Some(scale);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Constellation;
    use approx::assert_relative_eq;

    #[test]
    fn waas_prns_map_to_waas() {
        for prn in [131, 133, 135, 138] {
            assert_eq!(SbasProvider::from_prn(prn), SbasProvider::Waas);
        }
    }

    #[test]
    fn other_providers_are_distinguished() {
        assert_eq!(SbasProvider::from_prn(121), SbasProvider::Egnos);
        assert_eq!(SbasProvider::from_prn(137), SbasProvider::Msas);
        assert_eq!(SbasProvider::from_prn(127), SbasProvider::Gagan);
        assert_eq!(SbasProvider::from_prn(134), SbasProvider::Kass);
        assert_eq!(SbasProvider::from_prn(122), SbasProvider::SouthPan);
        assert_eq!(SbasProvider::from_prn(159), SbasProvider::Unknown);
    }

    #[test]
    fn provider_keys_round_trip() {
        for prn in [120, 122, 125, 127, 129, 131, 134, 143, 159] {
            let provider = SbasProvider::from_prn(prn);
            assert_eq!(SbasProvider::from_key(provider.key()), Some(provider));
        }
        assert_eq!(SbasProvider::from_key("waas"), Some(SbasProvider::Waas));
        assert_eq!(SbasProvider::from_key("nonsense"), None);
    }

    /// Kilometres and metres must both resolve to the same physical radius.
    #[test]
    fn scale_is_decided_by_geostationary_radius() {
        // A real WAAS record, in kilometres (spec-compliant).
        let km = (4.2004684e4, -3.67493296e3, 0.0);
        assert_eq!(decode_scale(km.0, km.1, km.2), Some(1000.0));

        // The same satellite written in metres, as some providers do.
        let m = (4.2004684e7, -3.67493296e6, 0.0);
        assert_eq!(decode_scale(m.0, m.1, m.2), Some(1.0));

        // Both interpretations land on the same place.
        let r_km = ((km.0 * 1000.0).powi(2) + (km.1 * 1000.0).powi(2)).sqrt();
        let r_m = (m.0.powi(2) + m.1.powi(2)).sqrt();
        assert_relative_eq!(r_km, r_m, max_relative = 1e-12);
    }

    #[test]
    fn placeholder_records_are_rejected() {
        assert_eq!(decode_scale(0.0, 0.0, 0.0), None);
        // A low-Earth-orbit radius is not a GEO under either reading.
        assert_eq!(decode_scale(7000.0, 0.0, 0.0), None);
    }

    /// With zero velocity — correct for an ideal geostationary satellite in
    /// ECEF — the position must not drift.
    #[test]
    fn a_stationary_geo_does_not_move() {
        let eph = SbasEphemeris {
            sv: Sv::new(Constellation::Sbas, 131),
            toe: GpsTime::from_week_and_sow(2347, 0.0),
            position_m: Ecef::new(4.2004684e7, -3.67493296e6, 0.0),
            velocity_m_s: Ecef::new(0.0, 0.0, 0.0),
            acceleration_m_s2: Ecef::new(0.0, 0.0, 0.0),
            health: 31,
            accuracy_index: 15.0,
            iodn: 168.0,
        };
        let moved = eph.position_at(eph.toe.offset_by(600.0));
        assert_relative_eq!(moved.x, eph.position_m.x, epsilon = 1e-9);
        assert_relative_eq!(moved.y, eph.position_m.y, epsilon = 1e-9);
        assert_relative_eq!(moved.z, eph.position_m.z, epsilon = 1e-9);
    }

    #[test]
    fn taylor_expansion_applies_velocity_and_acceleration() {
        let eph = SbasEphemeris {
            sv: Sv::new(Constellation::Sbas, 133),
            toe: GpsTime::from_week_and_sow(2347, 0.0),
            position_m: Ecef::new(1000.0, 2000.0, 3000.0),
            velocity_m_s: Ecef::new(1.0, -2.0, 0.5),
            acceleration_m_s2: Ecef::new(0.02, 0.0, -0.01),
            health: 0,
            accuracy_index: 0.0,
            iodn: 0.0,
        };
        let dt = 100.0;
        let p = eph.position_at(eph.toe.offset_by(dt));
        assert_relative_eq!(
            p.x,
            1000.0 + 1.0 * dt + 0.5 * 0.02 * dt * dt,
            epsilon = 1e-9
        );
        assert_relative_eq!(p.y, 2000.0 - 2.0 * dt, epsilon = 1e-9);
        assert_relative_eq!(
            p.z,
            3000.0 + 0.5 * dt - 0.5 * 0.01 * dt * dt,
            epsilon = 1e-9
        );
    }

    /// Propagating backwards must be the exact inverse of propagating forwards
    /// when acceleration is zero.
    #[test]
    fn propagation_is_symmetric_without_acceleration() {
        let eph = SbasEphemeris {
            sv: Sv::new(Constellation::Sbas, 135),
            toe: GpsTime::from_week_and_sow(2347, 43_200.0),
            position_m: Ecef::new(2.4e7, -3.4e7, 1.0e5),
            velocity_m_s: Ecef::new(2.5, 1.8, -0.1),
            acceleration_m_s2: Ecef::new(0.0, 0.0, 0.0),
            health: 31,
            accuracy_index: 15.0,
            iodn: 1.0,
        };
        let ahead = eph.position_at(eph.toe.offset_by(300.0));
        let behind = eph.position_at(eph.toe.offset_by(-300.0));
        assert_relative_eq!((ahead.x + behind.x) / 2.0, eph.position_m.x, epsilon = 1e-6);
    }
}
