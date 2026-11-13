//! A tiny deterministic generator for the bootstrap: `SplitMix64`, seeded
//! explicitly so the same inputs always give the same interval. Not for
//! anything security-related.

/// `SplitMix64` state.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform index in `0..n` (`n` must be non-zero). The modulo bias
    /// is below 2^-40 for any sample set that fits in memory.
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0, "below(0) is undefined");
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.next_u64() % n64).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut rng = SplitMix64::new(7);
        let mut seen = [false; 5];
        for _ in 0..500 {
            let i = rng.below(5);
            assert!(i < 5);
            seen[i] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }
}
