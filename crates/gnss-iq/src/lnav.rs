//! GPS LNAV navigation message assembly.
//!
//! Builds the 50 bps data stream a GPS satellite transmits on L1: 30-bit
//! words, ten to a subframe, five subframes to a frame, with the broadcast
//! clock and ephemeris parameters packed into subframes 1-3 exactly as
//! IS-GPS-200 §20.3.3 lays them out, and the Hamming parity of Table 20-XIV
//! on every word.
//!
//! ## Why this level of fidelity
//!
//! A file whose data bits are noise is enough to acquire and track, but not to
//! decode. Packing the real parameters means the generated recording can be
//! run through actual receiver software and produce a position fix, which is
//! both the point of the exercise and by far the strongest end-to-end check
//! available: a fix that lands on the requested receiver position exercises
//! the ephemeris parsing, the propagation, the pseudorange assembly, the code
//! phase, and the bit packing simultaneously.
//!
//! ## What is not modelled
//!
//! Subframes 4 and 5 carry almanac and ionospheric pages. They are emitted
//! with a correct TLM word, HOW word and parity but a zero data payload,
//! because this crate has no almanac to put in them. A receiver reads
//! subframes 1-3 for the ephemeris it needs; it will find subframes 4 and 5
//! structurally valid and semantically empty.
//!
//! Bits are *not* scrambled by the P(Y)-code or affected by anti-spoof: the
//! anti-spoof flag in the HOW word is reported as clear.

use gnss_core::KeplerianEphemeris;

/// Bits in one word.
const WORD_BITS: usize = 30;
/// Words in one subframe.
const WORDS_PER_SUBFRAME: usize = 10;
/// Bits in one subframe.
pub const SUBFRAME_BITS: usize = WORD_BITS * WORDS_PER_SUBFRAME;
/// Subframes in one frame.
const SUBFRAMES_PER_FRAME: u64 = 5;
/// Duration of one subframe \[s\]: 300 bits at 50 bps.
pub const SUBFRAME_DURATION_S: f64 = 6.0;

/// TLM preamble, IS-GPS-200 §20.3.3.1: 1000 1011.
const PREAMBLE: u32 = 0b1000_1011;

/// Data-bit positions contributing to each parity bit, IS-GPS-200 Table
/// 20-XIV. One-based, indexing `d1..d24`.
const PARITY_TAPS: [&[u8]; 6] = [
    &[1, 2, 3, 5, 6, 10, 11, 12, 13, 14, 17, 18, 20, 23],
    &[2, 3, 4, 6, 7, 11, 12, 13, 14, 15, 18, 19, 21, 24],
    &[1, 3, 4, 5, 7, 8, 12, 13, 14, 15, 16, 19, 20, 22],
    &[2, 4, 5, 6, 8, 9, 13, 14, 15, 16, 17, 20, 21, 23],
    &[1, 3, 5, 6, 7, 9, 10, 14, 15, 16, 17, 18, 21, 22, 24],
    &[3, 5, 6, 8, 9, 10, 11, 13, 15, 19, 22, 23, 24],
];

/// Which of the previous word's last two bits seeds each parity bit:
/// `true` selects D30*, `false` selects D29*.
const PARITY_SEED_IS_D30: [bool; 6] = [false, true, false, true, true, false];

/// Read bit `position` (one-based, MSB first) out of a 24-bit data field.
#[inline]
fn data_bit(data: u32, position: u8) -> bool {
    (data >> (24 - u32::from(position))) & 1 == 1
}

/// Append the six parity bits to a 24-bit source field, producing a 30-bit
/// transmitted word.
///
/// `previous` is the preceding word, whose bits 29 and 30 seed the
/// computation; pass 0 for the first word of a subframe, whose predecessor is
/// the last word of the previous subframe.
///
/// Two things happen here and the order matters. The transmitted bits are
/// `D_n = d_n XOR D30*`, so the data goes out complemented whenever the
/// previous word ended in a 1 -- receivers undo that before reading any field.
/// But the *parity* is computed over the source bits `d_n`, not over the
/// complemented ones. Computing it over the transmitted bits instead produces
/// words that pass their own inverse check and fail every real receiver's.
fn append_parity(source: u32, previous: u32) -> u32 {
    let d29_previous = (previous >> 1) & 1 == 1;
    let d30_previous = previous & 1 == 1;

    let source = source & 0x00FF_FFFF;
    let transmitted = if d30_previous {
        (!source) & 0x00FF_FFFF
    } else {
        source
    };

    let mut word = transmitted << 6;
    for (index, taps) in PARITY_TAPS.iter().enumerate() {
        let seed = if PARITY_SEED_IS_D30[index] {
            d30_previous
        } else {
            d29_previous
        };
        let parity = taps.iter().fold(seed, |accumulator, &position| {
            accumulator ^ data_bit(source, position)
        });
        if parity {
            word |= 1 << (5 - index);
        }
    }
    word
}

/// Choose bits 23 and 24 of a data field so the resulting word ends in two
/// zeros.
///
/// The HOW word and word 10 of every subframe reserve their last two data bits
/// for this. It is what lets a receiver assume D29 = D30 = 0 at a subframe
/// boundary and resolve the data polarity without waiting for the next word.
/// All four combinations are tried rather than solving the equations, because
/// the search is four iterations and cannot be wrong about which parity
/// equations bits 23 and 24 participate in.
fn solve_trailing_bits(data: u32, previous: u32) -> u32 {
    for candidate in 0..4u32 {
        let trial = (data & !0b11) | candidate;
        if append_parity(trial, previous) & 0b11 == 0 {
            return trial;
        }
    }
    unreachable!("one of the four combinations always zeroes both parity bits");
}

/// Place `value`'s low `width` bits into a 24-bit data field at one-based
/// position `start` (MSB first).
#[inline]
fn place(data: &mut u32, start: u8, width: u8, value: u32) {
    let mask = if width >= 32 { u32::MAX } else { (1 << width) - 1 };
    let shift = 24 - u32::from(start) - u32::from(width) + 1;
    *data |= (value & mask) << shift;
}

/// Encode a signed value as `width`-bit two's complement in broadcast units.
#[inline]
fn scaled_signed(value: f64, scale_exponent: i32, width: u32) -> u32 {
    let quantised = (value / 2f64.powi(scale_exponent)).round() as i64;
    let mask = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    (quantised as u64 & mask) as u32
}

/// Encode an unsigned value in broadcast units.
#[inline]
fn scaled_unsigned(value: f64, scale_exponent: i32, width: u32) -> u32 {
    let quantised = (value / 2f64.powi(scale_exponent)).round() as u64;
    let mask = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    (quantised & mask) as u32
}

/// Radians to semicircles, the unit every angular broadcast field uses.
#[inline]
fn semicircles(radians: f64) -> f64 {
    radians / std::f64::consts::PI
}

/// Assembles the LNAV bit stream for one satellite.
#[derive(Debug, Clone)]
pub struct LnavGenerator {
    ephemeris: KeplerianEphemeris,
    /// Ten-bit truncated week number, as broadcast.
    week_truncated: u32,
}

impl LnavGenerator {
    pub fn new(ephemeris: KeplerianEphemeris) -> Self {
        // The broadcast field is 10 bits and rolls over every 19.6 years;
        // receivers resolve the epoch from context. Truncating here is
        // correct, not lossy in any way the format allows avoiding.
        let week_truncated = ephemeris.toe.week() % 1024;
        Self {
            ephemeris,
            week_truncated,
        }
    }

    /// The 300 bits of the subframe that begins at
    /// `subframe_index * 6` seconds on the GPS timescale.
    ///
    /// Indexing by absolute subframe number rather than by frame and position
    /// is what keeps the TOW count in the HOW word consistent with the
    /// satellite time the synthesiser is sampling at -- the two are the same
    /// number, divided differently.
    pub fn subframe(&self, subframe_index: u64) -> [bool; SUBFRAME_BITS] {
        let subframe_id = (subframe_index % SUBFRAMES_PER_FRAME) as u8 + 1;

        // The HOW carries the TOW count of the *next* subframe, so a receiver
        // that has just finished reading this one knows the time of the edge
        // it is about to see. Counts run 0..100799 and wrap with the week.
        const COUNTS_PER_WEEK: u64 = 100_800;
        let next_tow_count = (subframe_index + 1) % COUNTS_PER_WEEK;

        let mut data = [0u32; WORDS_PER_SUBFRAME];

        // Word 1: telemetry. Preamble, then a message field this crate has
        // nothing to say in, then two reserved bits.
        place(&mut data[0], 1, 8, PREAMBLE);
        place(&mut data[0], 9, 14, 0);
        place(&mut data[0], 23, 2, 0);

        // Word 2: handover. Alert and anti-spoof both clear; last two bits
        // solved below.
        place(&mut data[1], 1, 17, next_tow_count as u32);
        place(&mut data[1], 18, 1, 0);
        place(&mut data[1], 19, 1, 0);
        place(&mut data[1], 20, 3, u32::from(subframe_id));

        match subframe_id {
            1 => self.fill_subframe_one(&mut data),
            2 => self.fill_subframe_two(&mut data),
            3 => self.fill_subframe_three(&mut data),
            // Almanac and ionospheric pages: structurally present, empty.
            _ => {}
        }

        self.assemble(data)
    }

    /// Clock correction and satellite status.
    fn fill_subframe_one(&self, data: &mut [u32; WORDS_PER_SUBFRAME]) {
        let ephemeris = &self.ephemeris;
        // IODC is broadcast split across two words. The eight low bits must
        // agree with the IODE in subframes 2 and 3 for a receiver to accept
        // the set as coherent, so both come from the same source value.
        let iode = ephemeris.iode as u32 & 0xFF;

        // Word 3.
        place(&mut data[2], 1, 10, self.week_truncated);
        place(&mut data[2], 11, 2, 0b01); // C/A available on L2
        place(&mut data[2], 13, 4, 0); // URA index 0: best accuracy
        place(&mut data[2], 17, 6, u32::from(ephemeris.health) & 0x3F);
        place(&mut data[2], 23, 2, 0); // IODC bits 9-10

        // Words 4-6: reserved.
        place(&mut data[3], 1, 1, 0); // L2 P data flag
        place(&mut data[3], 2, 23, 0);

        // Word 7: reserved, then group delay.
        place(&mut data[6], 1, 16, 0);
        place(&mut data[6], 17, 8, scaled_signed(ephemeris.tgd, -31, 8));

        // Word 8: IODC low bits, then time of clock.
        place(&mut data[7], 1, 8, iode);
        place(
            &mut data[7],
            9,
            16,
            scaled_unsigned(ephemeris.toc.seconds_of_week(), 4, 16),
        );

        // Word 9: clock drift rate and drift.
        place(&mut data[8], 1, 8, scaled_signed(ephemeris.af2, -55, 8));
        place(&mut data[8], 9, 16, scaled_signed(ephemeris.af1, -43, 16));

        // Word 10: clock bias, then the two solved bits.
        place(&mut data[9], 1, 22, scaled_signed(ephemeris.af0, -31, 22));
    }

    /// First half of the orbital elements.
    fn fill_subframe_two(&self, data: &mut [u32; WORDS_PER_SUBFRAME]) {
        let ephemeris = &self.ephemeris;
        let iode = ephemeris.iode as u32 & 0xFF;

        place(&mut data[2], 1, 8, iode);
        place(&mut data[2], 9, 16, scaled_signed(ephemeris.crs, -5, 16));

        place(
            &mut data[3],
            1,
            16,
            scaled_signed(semicircles(ephemeris.delta_n), -43, 16),
        );
        let mean_anomaly = scaled_signed(semicircles(ephemeris.mean_anomaly_0), -31, 32);
        place(&mut data[3], 17, 8, mean_anomaly >> 24);
        place(&mut data[4], 1, 24, mean_anomaly & 0x00FF_FFFF);

        place(&mut data[5], 1, 16, scaled_signed(ephemeris.cuc, -29, 16));
        let eccentricity = scaled_unsigned(ephemeris.eccentricity, -33, 32);
        place(&mut data[5], 17, 8, eccentricity >> 24);
        place(&mut data[6], 1, 24, eccentricity & 0x00FF_FFFF);

        place(&mut data[7], 1, 16, scaled_signed(ephemeris.cus, -29, 16));
        let sqrt_a = scaled_unsigned(ephemeris.sqrt_a, -19, 32);
        place(&mut data[7], 17, 8, sqrt_a >> 24);
        place(&mut data[8], 1, 24, sqrt_a & 0x00FF_FFFF);

        place(
            &mut data[9],
            1,
            16,
            scaled_unsigned(ephemeris.toe_seconds_of_week, 4, 16),
        );
        place(&mut data[9], 17, 1, 0); // 4-hour fit interval
        place(&mut data[9], 18, 5, 0); // AODO
    }

    /// Second half of the orbital elements.
    fn fill_subframe_three(&self, data: &mut [u32; WORDS_PER_SUBFRAME]) {
        let ephemeris = &self.ephemeris;

        place(&mut data[2], 1, 16, scaled_signed(ephemeris.cic, -29, 16));
        let omega0 = scaled_signed(semicircles(ephemeris.omega0), -31, 32);
        place(&mut data[2], 17, 8, omega0 >> 24);
        place(&mut data[3], 1, 24, omega0 & 0x00FF_FFFF);

        place(&mut data[4], 1, 16, scaled_signed(ephemeris.cis, -29, 16));
        let inclination = scaled_signed(semicircles(ephemeris.i0), -31, 32);
        place(&mut data[4], 17, 8, inclination >> 24);
        place(&mut data[5], 1, 24, inclination & 0x00FF_FFFF);

        place(&mut data[6], 1, 16, scaled_signed(ephemeris.crc, -5, 16));
        let perigee = scaled_signed(semicircles(ephemeris.argument_of_perigee), -31, 32);
        place(&mut data[6], 17, 8, perigee >> 24);
        place(&mut data[7], 1, 24, perigee & 0x00FF_FFFF);

        place(
            &mut data[8],
            1,
            24,
            scaled_signed(semicircles(ephemeris.omega_dot), -43, 24),
        );

        place(&mut data[9], 1, 8, ephemeris.iode as u32 & 0xFF);
        place(
            &mut data[9],
            9,
            14,
            scaled_signed(semicircles(ephemeris.i_dot), -43, 14),
        );
    }

    /// Attach parity to every word and flatten to bits.
    fn assemble(&self, data: [u32; WORDS_PER_SUBFRAME]) -> [bool; SUBFRAME_BITS] {
        let mut bits = [false; SUBFRAME_BITS];
        // The first word of a subframe is parity-chained from the last word of
        // the previous one. Starting each subframe from zero costs nothing
        // real -- words 2 and 10 are solved to end in two zeros, so the last
        // word of every subframe genuinely does end 00.
        let mut previous = 0u32;

        for (index, &field) in data.iter().enumerate() {
            // Words 2 and 10 reserve their last two data bits for parity.
            let field = if index == 1 || index == WORDS_PER_SUBFRAME - 1 {
                solve_trailing_bits(field, previous)
            } else {
                field
            };

            let word = append_parity(field, previous);
            for bit in 0..WORD_BITS {
                bits[index * WORD_BITS + bit] = (word >> (WORD_BITS - 1 - bit)) & 1 == 1;
            }
            previous = word;
        }

        bits
    }
}

/// Caching bit source for the synthesiser.
///
/// Regenerates a subframe only when the satellite clock crosses a six-second
/// boundary, which at any usable sample rate is once every few million
/// samples.
#[derive(Debug, Clone)]
pub struct NavBitSource {
    generator: LnavGenerator,
    cached_index: u64,
    cached: [bool; SUBFRAME_BITS],
}

impl NavBitSource {
    pub fn new(generator: LnavGenerator) -> Self {
        let cached = generator.subframe(0);
        Self {
            generator,
            cached_index: 0,
            cached,
        }
    }

    /// Data-bit value at satellite time `gps_seconds`, as `+1` or `-1`.
    ///
    /// A logical 1 inverts the carrier, matching the spreading code's
    /// convention, so the two multiply together directly.
    pub fn bit_at(&mut self, gps_seconds: f64) -> i8 {
        let bit_index = (gps_seconds * 50.0).floor() as i64;
        // Negative times cannot arise from a validated job, but flooring a
        // negative into an unsigned index would wrap catastrophically rather
        // than fail, so clamp rather than trust.
        let bit_index = bit_index.max(0) as u64;

        let subframe_index = bit_index / SUBFRAME_BITS as u64;
        if subframe_index != self.cached_index {
            self.cached = self.generator.subframe(subframe_index);
            self.cached_index = subframe_index;
        }

        if self.cached[(bit_index % SUBFRAME_BITS as u64) as usize] {
            -1
        } else {
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gnss_core::{Constellation, GpsTime, Sv};

    fn ephemeris() -> KeplerianEphemeris {
        KeplerianEphemeris {
            sv: Sv::new(Constellation::Gps, 1),
            toe: GpsTime::from_week_and_sow(2347, 259_200.0),
            toe_seconds_of_week: 259_200.0,
            toc: GpsTime::from_week_and_sow(2347, 259_200.0),
            sqrt_a: 5_153.755_249_02,
            eccentricity: 2.085_076_412_19e-4,
            i0: 0.959_645_428_81,
            omega0: -1.782_896_012_34,
            argument_of_perigee: -1.317_975_728_42,
            mean_anomaly_0: 3.125_812_576_13,
            delta_n: 4.723_053_877_01e-9,
            i_dot: 1.582_208_762_49e-10,
            omega_dot: -8.442_851_678_66e-9,
            cuc: 5.071_982_741_36e-6,
            cus: 1.234_933_733_94e-6,
            crc: 352.843_75,
            crs: 95.656_25,
            cic: 7.636_845_111_85e-8,
            cis: 8.195_638_656_62e-8,
            af0: 8.645_467_460_16e-6,
            af1: 3.649_347_490_86e-11,
            af2: 0.0,
            tgd: -1.396_983_861_92e-9,
            iode: 39.0,
            health: 0,
        }
    }

    /// Independent parity checker, written from the receiving side of Table
    /// 20-XIV: recompute the six parity bits from the received word's own data
    /// bits and confirm they match what was transmitted.
    fn parity_is_valid(word: u32, previous: u32) -> bool {
        let data = (word >> 6) & 0x00FF_FFFF;
        let d29_previous = (previous >> 1) & 1 == 1;
        let d30_previous = previous & 1 == 1;

        // The receiver un-inverts first, then checks.
        let d = if d30_previous {
            (!data) & 0x00FF_FFFF
        } else {
            data
        };

        for (index, taps) in PARITY_TAPS.iter().enumerate() {
            let seed = if PARITY_SEED_IS_D30[index] {
                d30_previous
            } else {
                d29_previous
            };
            let expected = taps
                .iter()
                .fold(seed, |accumulator, &position| {
                    accumulator ^ data_bit(d, position)
                });
            let transmitted = (word >> (5 - index)) & 1 == 1;
            if expected != transmitted {
                return false;
            }
        }
        true
    }

    fn words_of(bits: &[bool; SUBFRAME_BITS]) -> Vec<u32> {
        bits.chunks(WORD_BITS)
            .map(|chunk| chunk.iter().fold(0u32, |acc, &b| (acc << 1) | u32::from(b)))
            .collect()
    }

    /// Every word of every subframe must carry valid parity, or a receiver
    /// discards the frame and never reaches the ephemeris.
    #[test]
    fn every_word_carries_valid_parity() {
        let generator = LnavGenerator::new(ephemeris());

        for subframe_index in 0..25u64 {
            let words = words_of(&generator.subframe(subframe_index));
            let mut previous = 0u32;
            for (position, &word) in words.iter().enumerate() {
                assert!(
                    parity_is_valid(word, previous),
                    "subframe {subframe_index} word {} failed parity",
                    position + 1
                );
                previous = word;
            }
        }
    }

    /// Words 2 and 10 must end in two zero bits so a receiver can resolve
    /// data polarity at a subframe boundary without waiting.
    #[test]
    fn the_solved_words_end_in_two_zeros() {
        let generator = LnavGenerator::new(ephemeris());
        for subframe_index in 0..10u64 {
            let words = words_of(&generator.subframe(subframe_index));
            assert_eq!(words[1] & 0b11, 0, "HOW word did not end 00");
            assert_eq!(words[9] & 0b11, 0, "word 10 did not end 00");
        }
    }

    /// The preamble is what a receiver searches for to find a subframe edge.
    #[test]
    fn every_subframe_opens_with_the_preamble() {
        let generator = LnavGenerator::new(ephemeris());
        for subframe_index in 0..10u64 {
            let bits = generator.subframe(subframe_index);
            let preamble = bits[..8]
                .iter()
                .fold(0u32, |acc, &b| (acc << 1) | u32::from(b));
            // The first word of a subframe is never inverted, because the
            // previous word ends in 00 by construction.
            assert_eq!(preamble, PREAMBLE, "subframe {subframe_index}");
        }
    }

    /// Subframe IDs cycle 1..5 and the TOW count advances by one per
    /// subframe, pointing at the *next* one.
    #[test]
    fn subframe_ids_cycle_and_tow_counts_advance() {
        let generator = LnavGenerator::new(ephemeris());

        for subframe_index in 0..12u64 {
            let words = words_of(&generator.subframe(subframe_index));
            let how = (words[1] >> 6) & 0x00FF_FFFF;

            let tow_count = how >> 7;
            let subframe_id = (how >> 2) & 0b111;

            assert_eq!(u64::from(subframe_id), subframe_index % 5 + 1);
            assert_eq!(u64::from(tow_count), subframe_index + 1);
        }
    }

    /// Round-trip the packed parameters back out, decoding from the ICD
    /// tables independently of the packing code, and confirm each lands
    /// within its own quantisation step. This is what catches a field placed
    /// at the wrong bit offset or scaled by the wrong power of two -- both of
    /// which leave the parity valid and the frame structurally perfect.
    #[test]
    fn ephemeris_parameters_survive_the_round_trip() {
        let source = ephemeris();
        let generator = LnavGenerator::new(source);

        // Un-invert and strip parity, giving the 24 data bits of each word.
        let payload = |subframe_index: u64| -> Vec<u32> {
            let words = words_of(&generator.subframe(subframe_index));
            let mut previous = 0u32;
            let mut fields = Vec::new();
            for &word in &words {
                let data = (word >> 6) & 0x00FF_FFFF;
                let data = if previous & 1 == 1 {
                    (!data) & 0x00FF_FFFF
                } else {
                    data
                };
                fields.push(data);
                previous = word;
            }
            fields
        };

        // Pull `width` bits starting at one-based `start` from a 24-bit field.
        let take = |field: u32, start: u32, width: u32| -> u32 {
            (field >> (24 - start - width + 1)) & ((1 << width) - 1)
        };
        let sign_extend = |value: u32, width: u32| -> i64 {
            let shift = 64 - width;
            (i64::from(value) << shift) >> shift
        };

        // --- subframe 1: clock ---
        let one = payload(0);
        assert_eq!(take(one[2], 1, 10), 2347 % 1024, "week number");

        let toc = f64::from(take(one[7], 9, 16)) * 16.0;
        assert_eq!(toc, source.toc.seconds_of_week());

        let af1 = sign_extend(take(one[8], 9, 16), 16) as f64 * 2f64.powi(-43);
        assert!((af1 - source.af1).abs() < 2f64.powi(-43));

        let af0 = sign_extend(take(one[9], 1, 22), 22) as f64 * 2f64.powi(-31);
        assert!((af0 - source.af0).abs() < 2f64.powi(-31));

        let tgd = sign_extend(take(one[6], 17, 8), 8) as f64 * 2f64.powi(-31);
        assert!((tgd - source.tgd).abs() < 2f64.powi(-31));

        // --- subframe 2: first half of the orbit ---
        let two = payload(1);
        assert_eq!(take(two[2], 1, 8), 39, "IODE");

        let crs = sign_extend(take(two[2], 9, 16), 16) as f64 * 2f64.powi(-5);
        assert!((crs - source.crs).abs() < 2f64.powi(-5));

        let delta_n = sign_extend(take(two[3], 1, 16), 16) as f64
            * 2f64.powi(-43)
            * std::f64::consts::PI;
        assert!((delta_n - source.delta_n).abs() < 1e-16);

        let mean_anomaly_raw = (take(two[3], 17, 8) << 24) | take(two[4], 1, 24);
        let mean_anomaly =
            sign_extend(mean_anomaly_raw, 32) as f64 * 2f64.powi(-31) * std::f64::consts::PI;
        assert!(
            (mean_anomaly - source.mean_anomaly_0).abs() < 1e-8,
            "M0 {mean_anomaly} vs {}",
            source.mean_anomaly_0
        );

        let eccentricity =
            f64::from((take(two[5], 17, 8) << 24) | take(two[6], 1, 24)) * 2f64.powi(-33);
        assert!((eccentricity - source.eccentricity).abs() < 1e-11);

        let sqrt_a = f64::from((take(two[7], 17, 8) << 24) | take(two[8], 1, 24)) * 2f64.powi(-19);
        assert!((sqrt_a - source.sqrt_a).abs() < 1e-5, "sqrtA {sqrt_a}");

        let toe = f64::from(take(two[9], 1, 16)) * 16.0;
        assert_eq!(toe, source.toe_seconds_of_week);

        // --- subframe 3: second half of the orbit ---
        let three = payload(2);
        let omega0_raw = (take(three[2], 17, 8) << 24) | take(three[3], 1, 24);
        let omega0 = sign_extend(omega0_raw, 32) as f64 * 2f64.powi(-31) * std::f64::consts::PI;
        assert!((omega0 - source.omega0).abs() < 1e-8, "Omega0 {omega0}");

        let inclination_raw = (take(three[4], 17, 8) << 24) | take(three[5], 1, 24);
        let inclination =
            sign_extend(inclination_raw, 32) as f64 * 2f64.powi(-31) * std::f64::consts::PI;
        assert!((inclination - source.i0).abs() < 1e-8, "i0 {inclination}");

        let perigee_raw = (take(three[6], 17, 8) << 24) | take(three[7], 1, 24);
        let perigee = sign_extend(perigee_raw, 32) as f64 * 2f64.powi(-31) * std::f64::consts::PI;
        assert!(
            (perigee - source.argument_of_perigee).abs() < 1e-8,
            "omega {perigee}"
        );

        let omega_dot = sign_extend(take(three[8], 1, 24), 24) as f64
            * 2f64.powi(-43)
            * std::f64::consts::PI;
        assert!((omega_dot - source.omega_dot).abs() < 1e-16);

        let i_dot = sign_extend(take(three[9], 9, 14), 14) as f64
            * 2f64.powi(-43)
            * std::f64::consts::PI;
        assert!((i_dot - source.i_dot).abs() < 1e-16);

        assert_eq!(take(three[9], 1, 8), 39, "IODE in subframe 3");
    }

    /// The bit source has to land on the same bits the generator produced, and
    /// hold each for exactly 20 ms.
    #[test]
    fn the_bit_source_tracks_subframe_and_bit_boundaries() {
        let generator = LnavGenerator::new(ephemeris());
        let expected = generator.subframe(0);
        let mut source = NavBitSource::new(generator.clone());

        for (bit_index, &expected_bit) in expected.iter().enumerate() {
            let expected_value = if expected_bit { -1 } else { 1 };
            // Sampled strictly inside each 20 ms bit. Probing the edges
            // themselves would test floating-point rounding rather than this
            // module: `29 * 0.02 * 50` evaluates to 28.999999999999996, so
            // which side of an edge a sample lands on is decided by
            // representation. Real sample times land on an edge with
            // probability zero, and either answer is correct when they do.
            for offset in [0.002, 0.01, 0.018] {
                let t = bit_index as f64 * 0.02 + offset;
                assert_eq!(
                    source.bit_at(t),
                    expected_value,
                    "bit {bit_index} at offset {offset}"
                );
            }
        }

        // Crossing into the next subframe must pick up the new one.
        let next = generator.subframe(1);
        let expected_value = if next[0] { -1 } else { 1 };
        assert_eq!(source.bit_at(6.002), expected_value);
    }

    /// Subframes 4 and 5 are emitted with real structure and an empty payload.
    #[test]
    fn almanac_subframes_are_structurally_valid_but_empty() {
        let generator = LnavGenerator::new(ephemeris());
        for subframe_index in [3u64, 4] {
            let words = words_of(&generator.subframe(subframe_index));
            let mut previous = 0u32;
            for &word in &words {
                assert!(parity_is_valid(word, previous));
                previous = word;
            }
            // Words 3-9 carry no data.
            for &word in &words[2..9] {
                assert_eq!((word >> 6) & 0x00FF_FFFF, 0);
            }
        }
    }
}
