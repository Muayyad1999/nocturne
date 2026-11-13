//! Password generation from caller-supplied entropy.
//!
//! The generator is a pure function of the policy and a 32-byte seed, which
//! the caller draws from the operating system's random number generator. The
//! seed is expanded with SHA-256 in counter mode:
//!
//! ```text
//! block(i) = SHA-256("nocturne-tools password v1" || seed || length as u64 BE || flags || i as u64 BE)
//! flags    = upper | lower << 1 | digits << 2 | symbols << 3 | avoid_ambiguous << 4
//! ```
//!
//! Characters are drawn from the stream by rejection sampling (a byte is
//! discarded unless it is below the largest multiple of the alphabet size),
//! so every character of the alphabet is equally likely; whole candidates
//! are discarded until one contains every enabled class, so every password
//! that satisfies the policy is equally likely. Binding the policy into the
//! hash keeps passwords made from one seed under different policies unrelated.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ToolError;
use crate::ip::group_thousands;

/// The longest password [`generate_password`] will produce.
pub const MAX_PASSWORD_LENGTH: usize = 1024;

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";

/// The symbols used when `symbols` is enabled.
///
/// Deliberately few: none of them means anything to the CLIs a password ends
/// up being typed into — no `?` (IOS help), `$` (RouterOS and shell
/// variables), `;` `#` `|` `!` quotes or brackets (Junos, RouterOS, shells),
/// and no `@` `:` `/`, which break `scp://user:password@host/` style URLs.
pub const PASSWORD_SYMBOLS: &str = "%+,-.=^_";

/// Characters that are easily mistaken for one another when read aloud or
/// copied by hand.
const AMBIGUOUS: &str = "0Oo1lI.,";

/// Whole candidates tried before falling back to placing the missing
/// classes by hand. A candidate of the shortest allowed length passes with a
/// probability above 4%, so the fallback has a chance below 10^-19.
const MAX_CANDIDATES: usize = 1000;

/// What a generated password must look like.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PasswordPolicy {
    /// Number of characters, 1 to [`MAX_PASSWORD_LENGTH`].
    pub length: usize,
    /// Include upper-case letters.
    pub upper: bool,
    /// Include lower-case letters.
    pub lower: bool,
    /// Include digits.
    pub digits: bool,
    /// Include the symbols of [`PASSWORD_SYMBOLS`].
    pub symbols: bool,
    /// Leave out look-alike characters: `0 O o 1 l I . ,`.
    pub avoid_ambiguous: bool,
}

/// Generate a password that satisfies `policy` from the entropy in `seed`.
///
/// Deterministic for a given policy and seed. The password contains at
/// least one character of every enabled class, and every password that
/// does is equally likely.
pub fn generate_password(policy: &PasswordPolicy, seed: &[u8; 32]) -> Result<String, ToolError> {
    generate(policy, seed, MAX_CANDIDATES)
}

fn generate(
    policy: &PasswordPolicy,
    seed: &[u8; 32],
    max_candidates: usize,
) -> Result<String, ToolError> {
    let classes: Vec<Vec<u8>> = [
        (policy.upper, UPPER),
        (policy.lower, LOWER),
        (policy.digits, DIGITS),
        (policy.symbols, PASSWORD_SYMBOLS),
    ]
    .iter()
    .filter(|(enabled, _)| *enabled)
    .map(|(_, class)| {
        class
            .bytes()
            .filter(|byte| !(policy.avoid_ambiguous && AMBIGUOUS.as_bytes().contains(byte)))
            .collect()
    })
    .collect();
    if classes.is_empty() {
        return Err(ToolError::invalid(
            "password policy",
            "",
            "enable at least one of upper-case letters, lower-case letters, digits or symbols",
        ));
    }
    if policy.length > MAX_PASSWORD_LENGTH {
        return Err(ToolError::TooLarge(format!(
            "a password of {} characters is beyond the limit of {}",
            group_thousands(&policy.length.to_string()),
            group_thousands(&MAX_PASSWORD_LENGTH.to_string())
        )));
    }
    if policy.length < classes.len() {
        return Err(ToolError::invalid(
            "password length",
            &policy.length.to_string(),
            match (policy.length, classes.len()) {
                (0, _) => "a password needs at least one character".to_owned(),
                (_, count) => format!(
                    "it is too short to contain a character from each of the {count} enabled classes"
                ),
            },
        ));
    }

    let alphabet: Vec<u8> = classes.concat();
    let mut stream = Stream::new(policy, seed);
    let mut candidate = vec![0u8; policy.length];
    let complete = |candidate: &[u8]| {
        classes
            .iter()
            .all(|class| candidate.iter().any(|byte| class.contains(byte)))
    };
    for _ in 0..max_candidates {
        for slot in &mut candidate {
            *slot = alphabet[stream.below(alphabet.len())];
        }
        if complete(&candidate) {
            return Ok(candidate.iter().map(|byte| char::from(*byte)).collect());
        }
    }

    // Practically unreachable (see MAX_CANDIDATES): build a password that
    // satisfies the policy by construction instead.
    for slot in &mut candidate {
        *slot = alphabet[stream.below(alphabet.len())];
    }
    for (slot, class) in candidate.iter_mut().zip(&classes) {
        *slot = class[stream.below(class.len())];
    }
    for index in (1..candidate.len()).rev() {
        candidate.swap(index, stream.below(index + 1));
    }
    Ok(candidate.iter().map(|byte| char::from(*byte)).collect())
}

/// SHA-256 in counter mode over the seed and the policy.
struct Stream {
    prefix: Sha256,
    counter: u64,
    block: [u8; 32],
    used: usize,
}

impl Stream {
    fn new(policy: &PasswordPolicy, seed: &[u8; 32]) -> Self {
        let flags = u8::from(policy.upper)
            | u8::from(policy.lower) << 1
            | u8::from(policy.digits) << 2
            | u8::from(policy.symbols) << 3
            | u8::from(policy.avoid_ambiguous) << 4;
        let prefix = Sha256::new()
            .chain_update(b"nocturne-tools password v1")
            .chain_update(seed)
            .chain_update((policy.length as u64).to_be_bytes())
            .chain_update([flags]);
        Self {
            prefix,
            counter: 0,
            block: [0; 32],
            used: 32,
        }
    }

    fn next_byte(&mut self) -> u8 {
        if self.used == self.block.len() {
            self.block = self
                .prefix
                .clone()
                .chain_update(self.counter.to_be_bytes())
                .finalize()
                .into();
            self.counter += 1;
            self.used = 0;
        }
        let byte = self.block[self.used];
        self.used += 1;
        byte
    }

    /// A uniformly distributed index below `bound` (at most 256 for single
    /// bytes; larger bounds combine two bytes).
    fn below(&mut self, bound: usize) -> usize {
        let (range, bytes) = if bound <= 256 {
            (256usize, 1)
        } else {
            (65_536usize, 2)
        };
        let limit = range - range % bound;
        // Each draw is accepted with a probability above one half, so the
        // loop bound is never reached with a working hash function.
        for _ in 0..256 {
            let draw = (0..bytes).fold(0usize, |draw, _| {
                (draw << 8) | usize::from(self.next_byte())
            });
            if draw < limit {
                return draw % bound;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(
        length: usize,
        upper: bool,
        lower: bool,
        digits: bool,
        symbols: bool,
        avoid_ambiguous: bool,
    ) -> PasswordPolicy {
        PasswordPolicy {
            length,
            upper,
            lower,
            digits,
            symbols,
            avoid_ambiguous,
        }
    }

    fn seed(number: u32) -> [u8; 32] {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&number.to_be_bytes());
        seed
    }

    fn has(password: &str, class: &str) -> bool {
        password.chars().any(|c| class.contains(c))
    }

    #[test]
    fn passwords_satisfy_the_policy() {
        for number in 0..500 {
            for length in [4, 5, 8, 16, 32, 100] {
                let password = generate_password(
                    &policy(length, true, true, true, true, false),
                    &seed(number),
                )
                .unwrap();
                assert_eq!(password.len(), length);
                assert!(
                    has(&password, UPPER)
                        && has(&password, LOWER)
                        && has(&password, DIGITS)
                        && has(&password, PASSWORD_SYMBOLS),
                    "{password}"
                );
                assert!(password.chars().all(|c| UPPER.contains(c)
                    || LOWER.contains(c)
                    || DIGITS.contains(c)
                    || PASSWORD_SYMBOLS.contains(c)));
            }
        }
    }

    #[test]
    fn disabled_classes_never_appear() {
        for number in 0..200 {
            let digits_only =
                generate_password(&policy(12, false, false, true, false, false), &seed(number))
                    .unwrap();
            assert!(
                digits_only.chars().all(|c| c.is_ascii_digit()),
                "{digits_only}"
            );
            let letters =
                generate_password(&policy(12, true, true, false, false, false), &seed(number))
                    .unwrap();
            assert!(
                letters.chars().all(|c| c.is_ascii_alphabetic()),
                "{letters}"
            );
            assert!(has(&letters, UPPER) && has(&letters, LOWER));
            let unambiguous =
                generate_password(&policy(40, true, true, true, true, true), &seed(number))
                    .unwrap();
            assert!(
                !unambiguous.chars().any(|c| AMBIGUOUS.contains(c)),
                "{unambiguous}"
            );
            assert!(
                has(&unambiguous, UPPER)
                    && has(&unambiguous, LOWER)
                    && has(&unambiguous, DIGITS)
                    && has(&unambiguous, PASSWORD_SYMBOLS)
            );
        }
    }

    #[test]
    fn deterministic_for_a_seed_and_different_across_seeds_and_policies() {
        let base = policy(20, true, true, true, true, false);
        let first = generate_password(&base, &seed(1)).unwrap();
        assert_eq!(first, generate_password(&base, &seed(1)).unwrap());
        assert_ne!(first, generate_password(&base, &seed(2)).unwrap());
        // A longer password from the same seed does not reveal the shorter one.
        let longer =
            generate_password(&policy(21, true, true, true, true, false), &seed(1)).unwrap();
        assert!(!longer.starts_with(&first[..8]), "{first} / {longer}");
        let other_flags =
            generate_password(&policy(20, true, true, true, true, true), &seed(1)).unwrap();
        assert_ne!(first[..8], other_flags[..8]);
    }

    #[test]
    fn pinned_outputs() {
        // Reference values from a second implementation of the algorithm on
        // top of Python's hashlib (tests/generate_vectors.py).
        assert_eq!(
            generate_password(&policy(16, true, true, true, true, false), &[0; 32]).unwrap(),
            "0oXgMHYTJXU,Ksq."
        );
        assert_eq!(
            generate_password(&policy(24, true, true, true, false, true), &[0xAB; 32]).unwrap(),
            "z3TtkvmrMTW9ZxVjJmFmMspg"
        );
        assert_eq!(
            generate_password(&policy(8, false, false, true, false, false), &seed(42)).unwrap(),
            "76452600"
        );
    }

    #[test]
    fn characters_are_evenly_distributed() {
        // 62 unambiguous characters, 200 x 500 draws: each expected ~1613 times.
        let policy = policy(500, true, true, true, true, true);
        let mut counts = std::collections::BTreeMap::new();
        for number in 0..200 {
            for character in generate_password(&policy, &seed(number)).unwrap().chars() {
                *counts.entry(character).or_insert(0u32) += 1;
            }
        }
        assert_eq!(counts.len(), 24 + 24 + 8 + 6);
        let expected = 200.0 * 500.0 / 62.0;
        let chi_square: f64 = counts
            .values()
            .map(|count| (f64::from(*count) - expected).powi(2) / expected)
            .sum();
        // 61 degrees of freedom: the 99.9th percentile is about 100.
        assert!(chi_square < 100.0, "chi-square {chi_square}");
    }

    #[test]
    fn the_shortest_passwords_still_cover_every_class() {
        for number in 0..300 {
            let password =
                generate_password(&policy(4, true, true, true, true, true), &seed(number)).unwrap();
            assert!(
                has(&password, UPPER)
                    && has(&password, LOWER)
                    && has(&password, DIGITS)
                    && has(&password, PASSWORD_SYMBOLS),
                "{password}"
            );
            let one =
                generate_password(&policy(1, false, false, false, true, false), &seed(number))
                    .unwrap();
            assert!(one.len() == 1 && PASSWORD_SYMBOLS.contains(&one));
        }
    }

    #[test]
    fn the_constructive_fallback_also_satisfies_the_policy() {
        for number in 0..300 {
            for length in [4, 5, 12] {
                let password = generate(
                    &policy(length, true, true, true, true, true),
                    &seed(number),
                    0,
                )
                .unwrap();
                assert_eq!(password.len(), length);
                assert!(
                    has(&password, UPPER)
                        && has(&password, LOWER)
                        && has(&password, DIGITS)
                        && has(&password, PASSWORD_SYMBOLS),
                    "{password}"
                );
                assert!(!password.chars().any(|c| AMBIGUOUS.contains(c)));
            }
        }
    }

    #[test]
    fn sampling_is_uniform_for_awkward_bounds() {
        let mut stream = Stream::new(&policy(1, true, false, false, false, false), &seed(9));
        for bound in [1usize, 2, 3, 7, 62, 70, 255, 256, 257, 1000, 1024] {
            let mut counts = vec![0u32; bound];
            let draws = 200 * bound;
            for _ in 0..draws {
                counts[stream.below(bound)] += 1;
            }
            assert!(
                counts.iter().all(|count| (100..=320).contains(count)),
                "bound {bound}: {counts:?}"
            );
        }
    }

    #[test]
    fn impossible_policies_are_rejected() {
        let none =
            generate_password(&policy(12, false, false, false, false, true), &seed(0)).unwrap_err();
        assert_eq!(
            none.to_string(),
            "invalid password policy: enable at least one of upper-case letters, lower-case letters, digits or symbols"
        );
        let empty =
            generate_password(&policy(0, true, true, true, true, false), &seed(0)).unwrap_err();
        assert_eq!(
            empty.to_string(),
            "invalid password length `0`: a password needs at least one character"
        );
        let short =
            generate_password(&policy(3, true, true, true, true, false), &seed(0)).unwrap_err();
        assert_eq!(
            short.to_string(),
            "invalid password length `3`: it is too short to contain a character from each of the 4 enabled classes"
        );
        let long =
            generate_password(&policy(1025, true, true, true, true, false), &seed(0)).unwrap_err();
        assert_eq!(
            long,
            ToolError::TooLarge(
                "a password of 1,025 characters is beyond the limit of 1,024".into()
            )
        );
        assert!(matches!(
            generate_password(
                &policy(usize::MAX, true, false, false, false, false),
                &seed(0)
            ),
            Err(ToolError::TooLarge(_))
        ));
        assert!(generate_password(&policy(1024, true, true, true, true, false), &seed(0)).is_ok());
    }
}
