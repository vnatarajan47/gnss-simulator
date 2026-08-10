//! Constellation-neutral broadcast ephemeris, and selection of the right
//! ephemeris block for a given satellite and instant.

use std::collections::BTreeMap;
use std::fmt;

use crate::sbas::SbasEphemeris;
use crate::time::GpsTime;
use crate::Constellation;

/// A single satellite vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sv {
    pub constellation: Constellation,
    /// PRN / slot number within the constellation, 1-based.
    pub prn: u8,
}

impl Sv {
    pub const fn new(constellation: Constellation, prn: u8) -> Self {
        Self { constellation, prn }
    }
}

impl fmt::Display for Sv {
    /// RINEX 3 satellite identifier, e.g. `G01`, `E11`, `S31`.
    ///
    /// SBAS is the odd one out: RINEX writes the two-digit field as
    /// `PRN - 100`, so PRN 131 appears as `S31`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.constellation.rinex_code();
        match self.constellation {
            Constellation::Sbas => write!(f, "{code}{:02}", self.prn.saturating_sub(100)),
            _ => write!(f, "{code}{:02}", self.prn),
        }
    }
}

/// One broadcast ephemeris block: the Keplerian element set plus harmonic
/// perturbation terms transmitted in the navigation message.
///
/// GPS LNAV, Galileo I/NAV-F/NAV and BeiDou D1/D2 all broadcast this same
/// parameter set, which is why this struct carries no constellation-specific
/// fields. The differences live in [`Constellation`] (gravitational constant,
/// Earth rotation rate, time-system offset) and are applied at propagation
/// time, so adding a constellation does not require touching this type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeplerianEphemeris {
    /// Satellite this block describes.
    pub sv: Sv,

    /// Time of ephemeris, as an absolute instant on the GPS timescale.
    pub toe: GpsTime,
    /// Time of ephemeris as broadcast, in seconds of week.
    ///
    /// Kept separately from [`Self::toe`] because the longitude-of-ascending-node
    /// term in IS-GPS-200 Table 20-IV is defined against the raw seconds-of-week
    /// value, and reconstructing it from an absolute instant is ambiguous at a
    /// week boundary.
    pub toe_seconds_of_week: f64,
    /// Time of clock, as an absolute instant on the GPS timescale.
    pub toc: GpsTime,

    /// Square root of the semi-major axis \[sqrt(m)\].
    pub sqrt_a: f64,
    /// Eccentricity (dimensionless).
    pub eccentricity: f64,
    /// Inclination angle at reference time \[rad\].
    pub i0: f64,
    /// Longitude of ascending node at weekly epoch \[rad\].
    pub omega0: f64,
    /// Argument of perigee \[rad\].
    pub argument_of_perigee: f64,
    /// Mean anomaly at reference time \[rad\].
    pub mean_anomaly_0: f64,

    /// Mean motion difference from computed value \[rad/s\].
    pub delta_n: f64,
    /// Rate of inclination angle \[rad/s\].
    pub i_dot: f64,
    /// Rate of right ascension \[rad/s\].
    pub omega_dot: f64,

    /// Cosine harmonic correction to the argument of latitude \[rad\].
    pub cuc: f64,
    /// Sine harmonic correction to the argument of latitude \[rad\].
    pub cus: f64,
    /// Cosine harmonic correction to the orbit radius \[m\].
    pub crc: f64,
    /// Sine harmonic correction to the orbit radius \[m\].
    pub crs: f64,
    /// Cosine harmonic correction to the inclination \[rad\].
    pub cic: f64,
    /// Sine harmonic correction to the inclination \[rad\].
    pub cis: f64,

    /// Clock bias \[s\], drift \[s/s\] and drift rate \[s/s^2\].
    ///
    /// Not used for look angles; carried through because pseudorange and IQ
    /// generation (phase 4) need them.
    pub af0: f64,
    pub af1: f64,
    pub af2: f64,

    /// Issue of data, ephemeris. Distinguishes successive uploads.
    pub iode: f64,
    /// Raw SV health word; 0 means healthy for every constellation we support.
    pub health: u16,
}

impl KeplerianEphemeris {
    /// Whether the transmitting satellite reported itself healthy.
    pub fn is_healthy(&self) -> bool {
        self.health == 0
    }

    /// Age of this ephemeris relative to `t`, signed \[s\].
    pub fn age_at(&self, t: GpsTime) -> f64 {
        t.seconds_since(self.toe)
    }

    /// Semi-major axis \[m\].
    pub fn semi_major_axis(&self) -> f64 {
        self.sqrt_a * self.sqrt_a
    }
}

/// One broadcast navigation record, in whichever form its constellation uses.
///
/// GPS, Galileo, BeiDou and QZSS transmit Keplerian elements; SBAS transmits
/// an ECEF state vector. They share nothing numerically, so they are kept as
/// distinct types and dispatched at propagation time rather than forced into
/// one struct with half its fields unused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BroadcastEphemeris {
    Keplerian(KeplerianEphemeris),
    Sbas(SbasEphemeris),
}

impl BroadcastEphemeris {
    pub fn sv(&self) -> Sv {
        match self {
            BroadcastEphemeris::Keplerian(e) => e.sv,
            BroadcastEphemeris::Sbas(e) => e.sv,
        }
    }

    /// Reference epoch of the record.
    pub fn toe(&self) -> GpsTime {
        match self {
            BroadcastEphemeris::Keplerian(e) => e.toe,
            BroadcastEphemeris::Sbas(e) => e.toe,
        }
    }

    /// Signed age relative to `t` \[s\].
    pub fn age_at(&self, t: GpsTime) -> f64 {
        match self {
            BroadcastEphemeris::Keplerian(e) => e.age_at(t),
            BroadcastEphemeris::Sbas(e) => e.age_at(t),
        }
    }

    /// Issue-of-data, used to tell successive uploads apart.
    pub fn issue_of_data(&self) -> f64 {
        match self {
            BroadcastEphemeris::Keplerian(e) => e.iode,
            BroadcastEphemeris::Sbas(e) => e.iodn,
        }
    }

    /// Raw health word, as broadcast.
    pub fn health(&self) -> u16 {
        match self {
            BroadcastEphemeris::Keplerian(e) => e.health,
            BroadcastEphemeris::Sbas(e) => e.health,
        }
    }

    /// Whether health filtering should be applied to this record at all.
    ///
    /// Only the Keplerian constellations carry a health word we trust. The
    /// SBAS field in the merged IGS product is dominated by all-ones fillers
    /// even for operational satellites, so gating on it would discard the
    /// whole constellation -- see [`SbasEphemeris::health`].
    pub fn health_is_meaningful(&self) -> bool {
        matches!(self, BroadcastEphemeris::Keplerian(_))
    }

    /// Healthy, or health not meaningful for this record type.
    pub fn is_usable(&self) -> bool {
        match self {
            BroadcastEphemeris::Keplerian(e) => e.is_healthy(),
            BroadcastEphemeris::Sbas(_) => true,
        }
    }
}

impl From<KeplerianEphemeris> for BroadcastEphemeris {
    fn from(value: KeplerianEphemeris) -> Self {
        BroadcastEphemeris::Keplerian(value)
    }
}

impl From<SbasEphemeris> for BroadcastEphemeris {
    fn from(value: SbasEphemeris) -> Self {
        BroadcastEphemeris::Sbas(value)
    }
}

/// How to pick among several ephemeris blocks that all cover an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionStrategy {
    /// Smallest `|t - ToE|`. Most accurate for post-processing, because the
    /// curve fit is centred on ToE and degrades symmetrically either side.
    #[default]
    NearestToe,
    /// Most recent block with `ToE <= t`. Mirrors what a real receiver can do
    /// in real time, since it cannot use an ephemeris it has not yet received.
    LatestNotAfter,
}

/// Selection tuning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionConfig {
    pub strategy: SelectionStrategy,
    /// Reject any block whose `|t - ToE|` exceeds this \[s\].
    ///
    /// Defaults to the constellation's curve-fit half-interval.
    pub max_age_s: Option<f64>,
    /// Skip blocks whose health word is non-zero.
    pub require_healthy: bool,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            strategy: SelectionStrategy::default(),
            max_age_s: None,
            require_healthy: true,
        }
    }
}

/// All broadcast ephemerides parsed from one or more RINEX Nav files,
/// indexed by satellite.
///
/// A daily RINEX Nav file holds roughly a dozen blocks per satellite, so
/// selection is over a short list and a linear scan is the right call.
#[derive(Debug, Clone, Default)]
pub struct EphemerisSet {
    by_sv: BTreeMap<Sv, Vec<BroadcastEphemeris>>,
}

impl EphemerisSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a block, keeping each satellite's list sorted by ToE and
    /// discarding exact re-broadcasts.
    ///
    /// Duplicates are common: consecutive RINEX files overlap at midnight, and
    /// a satellite repeats the same block for its whole two-hour window. SBAS
    /// is a stronger case again -- a daily file holds hundreds of state
    /// vectors per satellite.
    pub fn insert(&mut self, ephemeris: impl Into<BroadcastEphemeris>) {
        let ephemeris = ephemeris.into();
        let list = self.by_sv.entry(ephemeris.sv()).or_default();

        let is_duplicate = list.iter().any(|existing| {
            existing.toe() == ephemeris.toe()
                && existing.issue_of_data() == ephemeris.issue_of_data()
        });
        if is_duplicate {
            return;
        }

        list.push(ephemeris);
        list.sort_by(|a, b| {
            a.toe()
                .seconds()
                .partial_cmp(&b.toe().seconds())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Satellites present in this set.
    pub fn satellites(&self) -> impl Iterator<Item = Sv> + '_ {
        self.by_sv.keys().copied()
    }

    /// Blocks held for one satellite, ordered by ToE.
    pub fn blocks_for(&self, sv: Sv) -> &[BroadcastEphemeris] {
        self.by_sv.get(&sv).map_or(&[], Vec::as_slice)
    }

    /// Total number of blocks across all satellites.
    pub fn len(&self) -> usize {
        self.by_sv.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_sv.values().all(Vec::is_empty)
    }

    /// Pick the ephemeris to use for `sv` at instant `t`.
    ///
    /// Returns `None` if the satellite is absent, every candidate is stale, or
    /// every candidate is unhealthy and `require_healthy` is set.
    pub fn select(
        &self,
        sv: Sv,
        t: GpsTime,
        config: SelectionConfig,
    ) -> Option<&BroadcastEphemeris> {
        let max_age = config
            .max_age_s
            .unwrap_or_else(|| sv.constellation.fit_half_interval());

        let candidates = self.blocks_for(sv).iter().filter(|eph| {
            // Health filtering only applies where the health word means
            // something; for SBAS `is_usable` is always true.
            if config.require_healthy && !eph.is_usable() {
                return false;
            }
            let age = eph.age_at(t);
            match config.strategy {
                SelectionStrategy::NearestToe => age.abs() <= max_age,
                SelectionStrategy::LatestNotAfter => age >= 0.0 && age <= max_age,
            }
        });

        match config.strategy {
            // Ties (a block re-issued with the same ToE) resolve to the later
            // issue of data, which is the more recent upload.
            SelectionStrategy::NearestToe => candidates.min_by(|a, b| {
                a.age_at(t)
                    .abs()
                    .partial_cmp(&b.age_at(t).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        a.issue_of_data()
                            .partial_cmp(&b.issue_of_data())
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            }),
            SelectionStrategy::LatestNotAfter => candidates.max_by(|a, b| {
                a.toe()
                    .seconds()
                    .partial_cmp(&b.toe().seconds())
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
        }
    }
}

impl Constellation {
    /// RINEX 3 single-character constellation code.
    pub const fn rinex_code(self) -> char {
        match self {
            Constellation::Gps => 'G',
            Constellation::Galileo => 'E',
            Constellation::BeiDou => 'C',
            Constellation::Qzss => 'J',
            Constellation::Sbas => 'S',
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unwrap the Keplerian variant, for tests that only build Keplerian stubs.
    fn keplerian_of(record: &BroadcastEphemeris) -> &KeplerianEphemeris {
        match record {
            BroadcastEphemeris::Keplerian(e) => e,
            BroadcastEphemeris::Sbas(_) => panic!("expected a Keplerian record"),
        }
    }

    fn stub(prn: u8, toe_sow: f64, iode: f64, health: u16) -> KeplerianEphemeris {
        KeplerianEphemeris {
            sv: Sv::new(Constellation::Gps, prn),
            toe: GpsTime::from_week_and_sow(2347, toe_sow),
            toe_seconds_of_week: toe_sow,
            toc: GpsTime::from_week_and_sow(2347, toe_sow),
            sqrt_a: 5153.6,
            eccentricity: 0.01,
            i0: 0.96,
            omega0: 1.0,
            argument_of_perigee: 0.5,
            mean_anomaly_0: 0.2,
            delta_n: 4.5e-9,
            i_dot: 1e-10,
            omega_dot: -8e-9,
            cuc: 0.0,
            cus: 0.0,
            crc: 0.0,
            crs: 0.0,
            cic: 0.0,
            cis: 0.0,
            af0: 0.0,
            af1: 0.0,
            af2: 0.0,
            iode,
            health,
        }
    }

    #[test]
    fn sv_formats_as_rinex_identifier() {
        assert_eq!(Sv::new(Constellation::Gps, 1).to_string(), "G01");
        assert_eq!(Sv::new(Constellation::Galileo, 11).to_string(), "E11");
    }

    #[test]
    fn duplicate_blocks_are_dropped() {
        let mut set = EphemerisSet::new();
        set.insert(stub(1, 7200.0, 10.0, 0));
        set.insert(stub(1, 7200.0, 10.0, 0));
        assert_eq!(set.len(), 1);

        // Same ToE but a new upload is a genuinely different block.
        set.insert(stub(1, 7200.0, 11.0, 0));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn blocks_are_kept_sorted_regardless_of_insert_order() {
        let mut set = EphemerisSet::new();
        set.insert(stub(1, 21600.0, 3.0, 0));
        set.insert(stub(1, 7200.0, 1.0, 0));
        set.insert(stub(1, 14400.0, 2.0, 0));

        let toes: Vec<f64> = set
            .blocks_for(Sv::new(Constellation::Gps, 1))
            .iter()
            .map(keplerian_of)
            .map(|e| e.toe_seconds_of_week)
            .collect();
        assert_eq!(toes, vec![7200.0, 14400.0, 21600.0]);
    }

    #[test]
    fn nearest_toe_can_select_a_block_from_the_future() {
        let mut set = EphemerisSet::new();
        set.insert(stub(1, 7200.0, 1.0, 0));
        set.insert(stub(1, 14400.0, 2.0, 0));

        // t = 13000 s: 1400 s after the 14400 block, 5800 s after the 7200 one.
        let t = GpsTime::from_week_and_sow(2347, 13_000.0);
        let chosen = set
            .select(
                Sv::new(Constellation::Gps, 1),
                t,
                SelectionConfig::default(),
            )
            .expect("a block should be in range");
        assert_eq!(keplerian_of(chosen).toe_seconds_of_week, 14400.0);
    }

    #[test]
    fn latest_not_after_never_selects_a_future_block() {
        let mut set = EphemerisSet::new();
        set.insert(stub(1, 7200.0, 1.0, 0));
        set.insert(stub(1, 14400.0, 2.0, 0));

        let t = GpsTime::from_week_and_sow(2347, 13_000.0);
        let config = SelectionConfig {
            strategy: SelectionStrategy::LatestNotAfter,
            ..SelectionConfig::default()
        };
        let chosen = set
            .select(Sv::new(Constellation::Gps, 1), t, config)
            .expect("a block should be in range");
        assert_eq!(keplerian_of(chosen).toe_seconds_of_week, 7200.0);
    }

    #[test]
    fn stale_blocks_are_rejected() {
        let mut set = EphemerisSet::new();
        set.insert(stub(1, 7200.0, 1.0, 0));

        // 3 h past ToE, beyond the 2 h GPS half-interval.
        let t = GpsTime::from_week_and_sow(2347, 18_000.0);
        assert!(set
            .select(
                Sv::new(Constellation::Gps, 1),
                t,
                SelectionConfig::default()
            )
            .is_none());
    }

    #[test]
    fn unhealthy_blocks_are_skipped_unless_allowed() {
        let mut set = EphemerisSet::new();
        set.insert(stub(1, 7200.0, 1.0, 63));

        let t = GpsTime::from_week_and_sow(2347, 7500.0);
        let sv = Sv::new(Constellation::Gps, 1);
        assert!(set.select(sv, t, SelectionConfig::default()).is_none());

        let permissive = SelectionConfig {
            require_healthy: false,
            ..SelectionConfig::default()
        };
        assert!(set.select(sv, t, permissive).is_some());
    }

    #[test]
    fn selection_works_across_a_week_boundary() {
        let mut set = EphemerisSet::new();
        // Last block of week 2347 and first of week 2348.
        set.insert(stub(1, 604_000.0, 1.0, 0));
        let mut next = stub(1, 800.0, 2.0, 0);
        next.toe = GpsTime::from_week_and_sow(2348, 800.0);
        next.toc = next.toe;
        set.insert(next);

        // 600 s into the new week: 1400 s from the new block, 1400 s... no:
        // 604800+600-604000 = 1400 from the old, 800-600 = 200 from the new.
        let t = GpsTime::from_week_and_sow(2348, 600.0);
        let chosen = set
            .select(
                Sv::new(Constellation::Gps, 1),
                t,
                SelectionConfig::default(),
            )
            .expect("a block should be in range");
        assert_eq!(keplerian_of(chosen).iode, 2.0);
    }
}
