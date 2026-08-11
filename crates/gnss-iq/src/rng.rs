//! Deterministic random number generation.
//!
//! ## Why this is hand-rolled
//!
//! Every job's sidecar records a seed, and the promise that goes with it is
//! that re-running the job reproduces the file. The `rand` crate explicitly
//! does not offer that across versions -- `StdRng`'s algorithm is allowed to
//! change in a minor release, which would silently break every stored seed.
//! A fixed, specified generator written out here cannot drift, and PCG-XSH-RR
//! is fifty lines.
//!
//! Statistical quality is not the binding constraint: the noise this feeds is
//! summed with a spread-spectrum signal and then quantised to eight bits.
//! Reproducibility is.

/// PCG-XSH-RR 64/32 (O'Neill 2014), the `pcg32` variant.
#[derive(Debug, Clone)]
pub struct Pcg32 {
    state: u64,
    /// Stream selector; must be odd, which `seed` guarantees.
    increment: u64,
    /// Cached second variate from the last Box-Muller pair.
    spare_normal: Option<f64>,
}

/// Multiplier from the reference implementation.
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

impl Pcg32 {
    /// Seed the generator.
    ///
    /// `stream` selects one of 2^63 distinct sequences. Different satellites
    /// draw from different streams rather than sharing one, so that adding or
    /// removing a satellite does not shift every other satellite's noise --
    /// which would make two otherwise-comparable runs incomparable.
    pub fn seed(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (stream << 1) | 1,
            spare_normal: None,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let previous = self.state;
        self.state = previous
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(self.increment);

        // Xorshift, then a rotation whose amount comes from the high bits --
        // the output permutation that gives PCG its statistical quality.
        let xorshifted = (((previous >> 18) ^ previous) >> 27) as u32;
        let rotation = (previous >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// Uniform in the half-open interval \[0, 1).
    pub fn next_f64(&mut self) -> f64 {
        // 53 bits, the full mantissa, assembled from two draws.
        let high = u64::from(self.next_u32()) >> 5; // 27 bits
        let low = u64::from(self.next_u32()) >> 6; // 26 bits
        ((high << 26) | low) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal variate, by the polar Box-Muller transform.
    ///
    /// Produces two variates at a time; the spare is cached rather than
    /// discarded, which halves the draws and keeps the sequence deterministic
    /// regardless of how callers interleave their requests.
    pub fn next_normal(&mut self) -> f64 {
        if let Some(spare) = self.spare_normal.take() {
            return spare;
        }

        // Rejection-sample a point inside the unit disc.
        let (x, y, radius_sq) = loop {
            let x = 2.0 * self.next_f64() - 1.0;
            let y = 2.0 * self.next_f64() - 1.0;
            let radius_sq = x * x + y * y;
            if radius_sq > 0.0 && radius_sq < 1.0 {
                break (x, y, radius_sq);
            }
        };

        let scale = (-2.0 * radius_sq.ln() / radius_sq).sqrt();
        self.spare_normal = Some(y * scale);
        x * scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason this generator is written out rather than pulled in.
    #[test]
    fn the_same_seed_reproduces_the_same_sequence() {
        let draw = |seed, stream| {
            let mut rng = Pcg32::seed(seed, stream);
            (0..64).map(|_| rng.next_u32()).collect::<Vec<_>>()
        };

        assert_eq!(draw(42, 0), draw(42, 0));
        assert_ne!(draw(42, 0), draw(43, 0));
        // Different streams from the same seed must not overlap either --
        // that is what keeps per-satellite noise independent.
        assert_ne!(draw(42, 0), draw(42, 1));
    }

    /// Check against the reference implementation's own published output.
    ///
    /// O'Neill's `pcg32-demo` seeds with `initstate = 42`, `initseq = 54` and
    /// prints these six words. Matching them establishes that this is really
    /// PCG-XSH-RR 64/32 and not merely a self-consistent LCG -- and it pins
    /// the sequence against an external source rather than against whatever
    /// this file happened to produce on the day the test was written.
    ///
    /// If this ever fails, every seed recorded in an existing sidecar has
    /// stopped meaning what it meant.
    #[test]
    fn output_matches_the_reference_implementations_published_vector() {
        let mut rng = Pcg32::seed(42, 54);
        let produced: Vec<u32> = (0..6).map(|_| rng.next_u32()).collect();
        assert_eq!(
            produced,
            vec![
                0xa15c_02b7,
                0x7b47_f409,
                0xba1d_3330,
                0x83d2_f293,
                0xbfa4_784b,
                0xcbed_606e,
            ]
        );
    }

    #[test]
    fn uniform_draws_stay_in_the_unit_interval() {
        let mut rng = Pcg32::seed(7, 3);
        for _ in 0..100_000 {
            let value = rng.next_f64();
            assert!((0.0..1.0).contains(&value), "{value} outside [0, 1)");
        }
    }

    /// Mean and variance of the normal draws, over enough samples that the
    /// standard error is well under the tolerance. This is what the thermal
    /// noise floor depends on: a variance of 0.9 instead of 1.0 offsets every
    /// generated C/N0 by half a decibel.
    #[test]
    fn normal_draws_have_zero_mean_and_unit_variance() {
        let mut rng = Pcg32::seed(11, 0);
        let count = 1_000_000;

        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for _ in 0..count {
            let value = rng.next_normal();
            sum += value;
            sum_sq += value * value;
        }

        let mean = sum / f64::from(count);
        let variance = sum_sq / f64::from(count) - mean * mean;

        // Standard error of the mean is 1/sqrt(1e6) = 1e-3.
        assert!(mean.abs() < 5e-3, "mean {mean}");
        assert!((variance - 1.0).abs() < 5e-3, "variance {variance}");
    }

    /// Box-Muller caches a spare; taking an odd number of draws must not
    /// desynchronise the sequence from a fresh generator's.
    #[test]
    fn cached_spare_does_not_break_reproducibility() {
        let mut a = Pcg32::seed(5, 1);
        let mut b = Pcg32::seed(5, 1);

        let from_a: Vec<f64> = (0..7).map(|_| a.next_normal()).collect();
        let from_b: Vec<f64> = (0..7).map(|_| b.next_normal()).collect();
        assert_eq!(from_a, from_b);
    }
}
