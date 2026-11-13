//! The lab's source of randomness: a stateless mixer, so that "noise" does
//! not depend on the order it is asked for.
//!
//! There is deliberately no running generator. Every value here is a pure
//! function of the world's seed and of what it describes, so the same lab
//! replays identically however the event queue happens to interleave.

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
        // The same inputs always give the same value, different inputs
        // almost never do, and the order of the inputs matters.
        assert_eq!(hash(&[1, 2, 3]), hash(&[1, 2, 3]));
        assert_ne!(hash(&[1, 2]), hash(&[2, 1]));
        assert_ne!(mix(0), mix(1));
        let spread: Vec<u64> = (0..64).map(|i| hash(&[7, i]) % 10).collect();
        assert!(spread.contains(&0) && spread.contains(&9));
        assert!(spread.windows(2).any(|w| w[0] != w[1]));
    }
}
