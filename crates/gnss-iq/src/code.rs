//! Spreading codes.
//!
//! GPS L1 C/A only for now: the 1023-chip Gold codes of IS-GPS-200 §3.2.1.3,
//! formed by modulo-2 addition of two 10-stage maximal-length shift registers.
//! The per-satellite difference is which pair of G2 stages is tapped, which is
//! the phase-selector table in IS-GPS-200 Table 3-Ia.
//!
//! Codes are generated once per job and held as `+1`/`-1` values, because that
//! is the form the synthesiser multiplies by; storing bits would mean a branch
//! or a lookup in the innermost loop.

use crate::signal::Signal;

/// Number of PRNs with an assigned C/A phase selection in Table 3-Ia.
///
/// 1-32 are the satellite codes. 33-37 exist and are assigned to ground
/// transmitters and other uses; they are excluded because nothing in this
/// crate can produce a broadcast ephemeris for them.
pub const MAX_GPS_PRN: u8 = 32;

/// G2 shift-register stages tapped for each PRN, from IS-GPS-200 Table 3-Ia.
///
/// One-based stage numbers, indexed by `prn - 1`. This table *is* the
/// difference between satellites; a transposed pair produces a valid-looking
/// but wrong code that correlates with nothing.
const G2_TAPS: [(usize, usize); MAX_GPS_PRN as usize] = [
    (2, 6),
    (3, 7),
    (4, 8),
    (5, 9),
    (1, 9),
    (2, 10),
    (1, 8),
    (2, 9),
    (3, 10),
    (2, 3),
    (3, 4),
    (5, 6),
    (6, 7),
    (7, 8),
    (8, 9),
    (9, 10),
    (1, 4),
    (2, 5),
    (3, 6),
    (4, 7),
    (5, 8),
    (6, 9),
    (1, 3),
    (4, 6),
    (5, 7),
    (6, 8),
    (7, 9),
    (8, 10),
    (1, 6),
    (2, 7),
    (3, 8),
    (4, 9),
];

/// A spreading code as `+1`/`-1` chips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadingCode {
    chips: Vec<i8>,
}

impl SpreadingCode {
    /// Generate the code for one satellite.
    ///
    /// `None` for a PRN with no assigned phase selection.
    pub fn generate(signal: Signal, prn: u8) -> Option<Self> {
        match signal {
            Signal::GpsL1Ca => Self::gps_ca(prn),
        }
    }

    /// GPS L1 C/A Gold code.
    fn gps_ca(prn: u8) -> Option<Self> {
        let &(tap_a, tap_b) = G2_TAPS.get(usize::from(prn).checked_sub(1)?)?;

        // Both registers start all ones (IS-GPS-200 §3.2.1.3).
        let mut g1 = [true; 10];
        let mut g2 = [true; 10];
        let mut chips = Vec::with_capacity(1023);

        for _ in 0..1023 {
            // Output is taken before clocking: G1's last stage, plus the
            // modulo-2 sum of the two selected G2 stages.
            let chip = g1[9] ^ (g2[tap_a - 1] ^ g2[tap_b - 1]);
            // A logical 1 modulates the carrier by 180 degrees, so map to -1.
            chips.push(if chip { -1 } else { 1 });

            // G1: 1 + x^3 + x^10.
            let g1_feedback = g1[2] ^ g1[9];
            // G2: 1 + x^2 + x^3 + x^6 + x^8 + x^9 + x^10.
            let g2_feedback = g2[1] ^ g2[2] ^ g2[5] ^ g2[7] ^ g2[8] ^ g2[9];

            g1.rotate_right(1);
            g1[0] = g1_feedback;
            g2.rotate_right(1);
            g2[0] = g2_feedback;
        }

        Some(Self { chips })
    }

    pub fn len(&self) -> usize {
        self.chips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chips.is_empty()
    }

    /// Chip at `index`, wrapping.
    #[inline]
    pub fn chip(&self, index: usize) -> i8 {
        self.chips[index % self.chips.len()]
    }

    pub fn chips(&self) -> &[i8] {
        &self.chips
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first ten chips of every C/A code, from IS-GPS-200 Table 3-Ia.
    ///
    /// The ICD's octal notation is idiosyncratic: the leading digit is always
    /// `1` and stands for the *first chip alone*, and the remaining three
    /// octal digits are the next nine chips. So `1440` means chip 1 is 1,
    /// followed by octal 440 = `100100000`.
    const FIRST_TEN_CHIPS_OCTAL: [u16; MAX_GPS_PRN as usize] = [
        0o1440, 0o1620, 0o1710, 0o1744, 0o1133, 0o1455, 0o1131, 0o1454, 0o1626, 0o1504, 0o1642,
        0o1750, 0o1764, 0o1772, 0o1775, 0o1776, 0o1156, 0o1467, 0o1633, 0o1715, 0o1746, 0o1763,
        0o1063, 0o1706, 0o1743, 0o1761, 0o1770, 0o1774, 0o1127, 0o1453, 0o1625, 0o1712,
    ];

    fn code(prn: u8) -> SpreadingCode {
        SpreadingCode::generate(Signal::GpsL1Ca, prn).expect("valid PRN")
    }

    /// The strongest check available on this module: an external, published
    /// table that the generator had no part in producing. Every one of the 32
    /// codes is pinned by its first ten chips, which is enough to catch a
    /// wrong tap pair, a wrong feedback polynomial, a wrong register
    /// initialisation, or an off-by-one in when the output is taken.
    #[test]
    fn first_ten_chips_match_the_icd_table() {
        for prn in 1..=MAX_GPS_PRN {
            let octal = FIRST_TEN_CHIPS_OCTAL[usize::from(prn) - 1];

            // Decode the ICD notation into ten expected chips.
            let mut expected = [false; 10];
            expected[0] = true; // the leading '1' digit
            for (index, slot) in expected[1..].iter_mut().enumerate() {
                // Nine bits, MSB first, held in the low nine bits of the octal.
                *slot = (octal >> (8 - index)) & 1 == 1;
            }

            let code = code(prn);
            for (index, &is_one) in expected.iter().enumerate() {
                let actual = code.chip(index);
                let expected_chip = if is_one { -1 } else { 1 };
                assert_eq!(
                    actual, expected_chip,
                    "PRN {prn} chip {} is {actual}, ICD table says {expected_chip}",
                    index + 1
                );
            }
        }
    }

    /// A maximal-length Gold code of length 1023 is balanced: 512 ones and
    /// 511 zeros. An imbalance means the register sequence is not maximal --
    /// typically a wrong feedback polynomial that shortens the period.
    #[test]
    fn every_code_is_balanced() {
        for prn in 1..=MAX_GPS_PRN {
            let ones = code(prn).chips().iter().filter(|&&c| c == -1).count();
            assert_eq!(ones, 512, "PRN {prn} has {ones} ones, expected 512");
        }
    }

    /// Autocorrelation: 1023 at zero lag, and one of the three Gold values at
    /// every other lag. This is the property the whole system rests on -- it
    /// is what lets a receiver find the code phase at all.
    #[test]
    fn autocorrelation_peaks_sharply_and_is_bounded_off_peak() {
        for prn in [1u8, 7, 19, 32] {
            let code = code(prn);
            let chips = code.chips();

            for lag in 0..1023usize {
                let correlation: i32 = (0..1023)
                    .map(|i| i32::from(chips[i]) * i32::from(chips[(i + lag) % 1023]))
                    .sum();

                if lag == 0 {
                    assert_eq!(correlation, 1023, "PRN {prn} zero-lag");
                } else {
                    // The three-valued Gold correlation set for n = 10.
                    assert!(
                        [-65, -1, 63].contains(&correlation),
                        "PRN {prn} lag {lag} correlated {correlation}, not a Gold value"
                    );
                }
            }
        }
    }

    /// Cross-correlation between different satellites must stay in the same
    /// bounded set, which is what keeps satellites from masking each other in
    /// a summed composite signal.
    #[test]
    fn cross_correlation_is_bounded_between_satellites() {
        let pairs = [(1u8, 2u8), (5, 19), (12, 30), (25, 32)];
        for (a, b) in pairs {
            let (first, second) = (code(a), code(b));
            for lag in 0..1023usize {
                let correlation: i32 = (0..1023)
                    .map(|i| {
                        i32::from(first.chips()[i]) * i32::from(second.chips()[(i + lag) % 1023])
                    })
                    .sum();
                assert!(
                    [-65, -1, 63].contains(&correlation),
                    "PRN {a} vs {b} at lag {lag}: {correlation}"
                );
            }
        }
    }

    #[test]
    fn codes_are_the_right_length_and_prns_are_bounded() {
        assert_eq!(code(1).len(), 1023);
        assert!(SpreadingCode::generate(Signal::GpsL1Ca, 0).is_none());
        assert!(SpreadingCode::generate(Signal::GpsL1Ca, 33).is_none());
        assert!(SpreadingCode::generate(Signal::GpsL1Ca, 255).is_none());
    }

    #[test]
    fn chip_indexing_wraps_at_the_code_period() {
        let code = code(1);
        assert_eq!(code.chip(0), code.chip(1023));
        assert_eq!(code.chip(5), code.chip(1023 + 5));
    }
}
