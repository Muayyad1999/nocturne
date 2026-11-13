//! The lab's only source of randomness: a seeded xorshift generator plus a
//! stateless mixer for "noise" that must not depend on call order.

#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        // xorshift must never hold zero.
        Rng(mix(seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n` (`n == 0` yields 0).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

/// splitmix64 finaliser: a good stateless 64-bit mixer.
pub(crate) fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Order-independent pseudo-random value for a tuple of inputs.
pub(crate) fn hash(parts: &[u64]) -> u64 {
    parts
        .iter()
        .fold(0x6E6F_6374_7572_6E65, |acc, p| mix(acc ^ *p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_spread() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        assert!(xs.windows(2).all(|w| w[0] != w[1]));
        assert_ne!(Rng::new(0).next_u64(), Rng::new(1).next_u64());
        assert!((0..100).all(|_| a.below(10) < 10));
        assert_ne!(hash(&[1, 2]), hash(&[2, 1]));
    }
}
