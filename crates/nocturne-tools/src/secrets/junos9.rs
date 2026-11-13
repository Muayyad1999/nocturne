//! Junos `$9$` secrets: a reversible obfuscation, not encryption.
//!
//! After the `$9$` prefix comes one salt character, zero to three filler
//! characters (how many depends on the salt) and then, for every character
//! of the secret, two to four characters of a 65-letter alphabet. Each one is
//! read as its distance from the character before it; the distances are the
//! digits of the secret's character code in a small mixed radix that cycles
//! through seven patterns. The algorithm was published with the Perl module
//! `Crypt::Juniper`, against which the tests check this implementation.

use sha2::{Digest, Sha256};

use super::printable_text;
use crate::ToolError;
use crate::error::snip;

const MAGIC: &str = "$9$";

/// The alphabet, in four families. The salt's family decides how many filler
/// characters follow it: three for the first family down to none for the last.
const FAMILIES: [&str; 4] = ["QzF3n6/9CAtpu0O", "B1IREhcSyrleKvMW8LXx", "7N-dVbwsY2g4oaJZGUDj", "iHkq.mPf5T"];

const ALPHABET_LEN: usize = 65;

/// The weights of the distances for the characters at position 0, 1, ... 6
/// of the secret (then the cycle repeats).
const WEIGHTS: [&[u32]; 7] =
    [&[1, 4, 32], &[1, 16, 32], &[1, 8, 32], &[1, 64], &[1, 32], &[1, 4, 16, 128], &[1, 32, 64]];

const WHAT: &str = "$9$ secret";

fn alphabet() -> [u8; ALPHABET_LEN] {
    let mut alphabet = [0u8; ALPHABET_LEN];
    for (slot, letter) in alphabet.iter_mut().zip(FAMILIES.iter().flat_map(|family| family.bytes())) {
        *slot = letter;
    }
    alphabet
}

/// The index of `letter` in the alphabet and the number of filler characters
/// that follow it when it is the salt.
fn lookup(letter: u8) -> Option<(usize, usize)> {
    let mut index = 0;
    for (family, letters) in FAMILIES.iter().enumerate() {
        if let Some(position) = letters.bytes().position(|candidate| candidate == letter) {
            return Some((index + position, 3 - family));
        }
        index += letters.len();
    }
    None
}

/// Decode a Junos `$9$` secret such as `$9$LbHX-wg4Z`.
///
/// The value may be pasted with its quotes or as a whole configuration line
/// (`secret "$9$LbHX-wg4Z"; ## SECRET-DATA`).
pub fn junos9_decode(encoded: &str) -> Result<String, ToolError> {
    let text = encoded.trim();
    if text.is_empty() {
        return Err(ToolError::invalid(WHAT, "", "nothing was entered"));
    }
    let Some(start) = text.find(MAGIC) else {
        let other = super::identify::identify(text);
        let reason = if other.kind == super::identify::UNKNOWN {
            "a Junos obfuscated secret starts with `$9$`".to_owned()
        } else {
            format!("this looks like {}, not a Junos `$9$` secret", other.name)
        };
        return Err(ToolError::invalid(WHAT, text, reason));
    };
    // The value runs to the closing quote, the `;` or the end of the word.
    let value = &text[start..];
    let value = &value[..value.find(|c: char| c.is_whitespace() || c == '"' || c == ';').unwrap_or(value.len())];
    let fail = |reason: String| Err(ToolError::invalid(WHAT, value, reason));

    let body = &value[MAGIC.len()..];
    if body.contains('$') {
        return fail(
            "this is a Cisco type 9 (scrypt) hash, which shares the `$9$` prefix but is one-way and cannot be decoded"
                .to_owned(),
        );
    }
    let mut letters = Vec::with_capacity(body.len());
    for character in body.chars() {
        match u8::try_from(character).ok().and_then(lookup) {
            Some(found) => letters.push(found),
            None => return fail(format!("`{}` is not a character Junos uses in `$9$` secrets", snip(character.encode_utf8(&mut [0; 4]), 4))),
        }
    }

    let Some(&(salt, filler)) = letters.first() else {
        return fail("there is nothing after `$9$`".to_owned());
    };
    let truncated = || fail("it ends in the middle of a character, so it is truncated or mistyped".to_owned());
    let Some(mut rest) = letters.get(1 + filler..) else { return truncated() };
    if rest.is_empty() {
        return fail("it holds only the salt and no secret, so it is truncated".to_owned());
    }

    let mut previous = salt;
    let mut bytes = Vec::with_capacity(rest.len() / 2);
    while !rest.is_empty() {
        let weights = WEIGHTS[bytes.len() % WEIGHTS.len()];
        let Some((group, tail)) = rest.split_at_checked(weights.len()) else { return truncated() };
        let mut code = 0u32;
        for (&(index, _), weight) in group.iter().zip(weights) {
            // The distance from the previous character, minus one. Zero steps
            // (a repeated character) cannot be produced by the encoder.
            let steps = (index + ALPHABET_LEN - previous) % ALPHABET_LEN;
            if steps == 0 {
                return fail(corrupt(bytes.len()));
            }
            code += (steps as u32 - 1) * weight;
            previous = index;
        }
        let Ok(byte) = u8::try_from(code) else { return fail(corrupt(bytes.len())) };
        bytes.push(byte);
        rest = tail;
    }
    match printable_text(&bytes) {
        Some(plain) => Ok(plain),
        None => fail("it decodes to unprintable bytes, so it is mistyped or not a `$9$` secret".to_owned()),
    }
}

fn corrupt(position: usize) -> String {
    format!(
        "the characters for position {} of the secret are not a valid encoding, so the string is mistyped or damaged",
        position + 1
    )
}

/// Encode `plain` as a Junos `$9$` secret.
///
/// Junos picks the salt and filler characters at random; here they are
/// derived from `seed`, so the result is deterministic for a given seed
/// (the caller supplies the entropy). Any result decodes to `plain` on a
/// Junos device, whatever the seed.
pub fn junos9_encode(plain: &str, seed: &[u8; 32]) -> Result<String, ToolError> {
    if plain.is_empty() {
        return Err(ToolError::invalid("secret", "", "there is nothing to encode"));
    }
    if let Some(bad) = plain.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(ToolError::invalid(
            "secret",
            plain,
            format!("`{}` cannot be encoded: Junos secrets are printable ASCII", bad.escape_default()),
        ));
    }
    let alphabet = alphabet();
    // Four characters' worth of randomness: the salt and up to three fillers.
    // 256 = 3 * 65 + 61, so bytes below 195 map evenly onto the alphabet;
    // the hash is re-run in the (rare) case that too few of them qualify.
    let mut random = Vec::with_capacity(4);
    let mut counter = 0u32;
    while random.len() < 4 {
        let digest = Sha256::new().chain_update(b"nocturne-tools junos9 v1").chain_update(seed).chain_update(counter.to_be_bytes()).finalize();
        random.extend(digest.iter().filter(|byte| **byte < 195).map(|byte| usize::from(*byte) % ALPHABET_LEN));
        counter += 1;
    }

    let salt = random[0];
    let filler = lookup(alphabet[salt]).map_or(0, |(_, filler)| filler);
    let mut encoded = String::with_capacity(MAGIC.len() + 4 + 4 * plain.len());
    encoded.push_str(MAGIC);
    encoded.extend(random[..=filler].iter().map(|index| char::from(alphabet[*index])));

    let mut previous = salt;
    for (position, byte) in plain.bytes().enumerate() {
        let weights = WEIGHTS[position % WEIGHTS.len()];
        // The digits of the character code, most significant first ...
        let mut remainder = u32::from(byte);
        let mut digits = [0u32; 4];
        for (digit, weight) in digits.iter_mut().zip(weights).rev() {
            *digit = remainder / weight;
            remainder %= weight;
        }
        // ... written least significant first, each as a step through the alphabet.
        for digit in &digits[..weights.len()] {
            previous = (previous + *digit as usize + 1) % ALPHABET_LEN;
            encoded.push(char::from(alphabet[previous]));
        }
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alphabet_has_65_distinct_characters() {
        let alphabet = alphabet();
        let mut sorted = alphabet.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ALPHABET_LEN);
        for (index, letter) in alphabet.iter().enumerate() {
            assert_eq!(lookup(*letter).map(|(found, _)| found), Some(index));
        }
        assert_eq!(lookup(b'Q'), Some((0, 3)));
        assert_eq!(lookup(b'B'), Some((15, 2)));
        assert_eq!(lookup(b'7'), Some((35, 1)));
        assert_eq!(lookup(b'T'), Some((64, 0)));
        assert_eq!(lookup(b'$'), None);
        // Every weight pattern can express 0..=255 with digits below 64.
        for weights in WEIGHTS {
            let largest: u32 = weights.windows(2).map(|pair| (pair[1] / pair[0] - 1) * pair[0]).sum();
            let top = weights[weights.len() - 1];
            assert!(largest + (255 / top) * top >= 255 && 255 / top < 64, "{weights:?}");
        }
    }

    #[test]
    fn decodes_published_examples() {
        // The example from the Crypt::Juniper documentation.
        assert_eq!(junos9_decode("$9$LbHX-wg4Z").unwrap(), "lc");
        // Worked by hand: salt 'P' (last family, no filler), then 'a' = 97 = 1 + 0*4 + 3*32
        // as the steps 2, 1, 4 through the alphabet: P -> 5 -> T -> 3.
        assert_eq!(junos9_decode("$9$P5T3").unwrap(), "a");
    }

    #[test]
    fn accepts_configuration_lines() {
        for line in [
            "$9$LbHX-wg4Z",
            "\"$9$LbHX-wg4Z\"",
            "secret \"$9$LbHX-wg4Z\"; ## SECRET-DATA",
            "set system radius-server 10.0.0.1 secret \"$9$LbHX-wg4Z\"",
            "authentication-key \"$9$LbHX-wg4Z\";",
            "  pre-shared-key ascii-text $9$LbHX-wg4Z;  ",
        ] {
            assert_eq!(junos9_decode(line).unwrap(), "lc", "{line}");
        }
    }

    #[test]
    fn encode_round_trips_for_every_salt_family() {
        let mut families_seen = [false; 4];
        for seed_byte in 0..=255u8 {
            let seed = [seed_byte; 32];
            for plain in ["a", "lc", "juniper123", "Tr1cky~!@#$%^&*()_+{}|:<>? pass", &"x".repeat(300)] {
                let encoded = junos9_encode(plain, &seed).unwrap();
                assert!(encoded.starts_with(MAGIC));
                assert_eq!(junos9_decode(&encoded).unwrap(), plain, "{encoded}");
            }
            let encoded = junos9_encode("a", &seed).unwrap();
            let (_, filler) = lookup(encoded.as_bytes()[3]).unwrap();
            families_seen[filler] = true;
            // salt + filler + three characters for the first position
            assert_eq!(encoded.len(), 3 + 1 + filler + 3);
        }
        assert_eq!(families_seen, [true; 4]);
        // Deterministic for a seed, different across seeds.
        assert_eq!(junos9_encode("secret", &[7; 32]).unwrap(), junos9_encode("secret", &[7; 32]).unwrap());
        assert_ne!(junos9_encode("secret", &[7; 32]).unwrap(), junos9_encode("secret", &[8; 32]).unwrap());
    }

    #[test]
    fn every_byte_value_position_round_trips() {
        // All printable ASCII at every position of the weight cycle.
        for offset in 0..7 {
            let plain: String = std::iter::repeat_n('.', offset).chain((' '..='~').cycle().take(95 * 7)).collect();
            let encoded = junos9_encode(&plain, &[offset as u8; 32]).unwrap();
            assert_eq!(junos9_decode(&encoded).unwrap(), plain);
        }
    }

    #[test]
    fn rejections_say_what_is_wrong() {
        let message = |input: &str| junos9_decode(input).unwrap_err().to_string();
        assert_eq!(message(""), "invalid $9$ secret: nothing was entered");
        assert_eq!(message("hunter2"), "invalid $9$ secret `hunter2`: a Junos obfuscated secret starts with `$9$`");
        assert_eq!(message("$9$"), "invalid $9$ secret `$9$`: there is nothing after `$9$`");
        assert!(message("$9$LbHX-wg4").contains("ends in the middle of a character"));
        assert!(message("$9$Lb").contains("ends in the middle of a character"));
        assert!(message("$9$P").contains("holds only the salt"));
        assert!(message("$9$QzF3").contains("holds only the salt"));
        assert!(message("$9$LbHX-wg4Z*").contains("`*` is not a character Junos uses"));
        assert!(message("$9$LbHX-wgéZ").contains("`é` is not a character Junos uses"));
        // The examples Crypt::Juniper's own test suite expects to fail.
        for invalid in ["$9jadsfdf", "$9$asd", "$9$asdf*", "$9$dLw2ajHmFnCZUnCtuEhVwYY", "$9$dLw2ajHmFnCZUnCtuEhVw"] {
            assert!(junos9_decode(invalid).is_err(), "{invalid}");
        }
        // A repeated character would be a step of zero.
        assert!(message("$9$PPT3").contains("position 1 of the secret are not a valid encoding"));
        // Steps that add up to more than a byte.
        assert!(message("$9$P5TP").contains("position 1 of the secret are not a valid encoding"));
        assert!(message("$9$nhEmQVczB7dqsO$X.HsgL6x1il0RxkOSSvyQYwucySCt7qFm4v7pqCxkKM").contains("Cisco type 9 (scrypt) hash"));
        assert!(message("0822455D0A16").contains("this looks like Cisco type 7"));
        assert!(message("$1$mERr$hx5rVt7rPNoS4wqbXKX7m0").contains("this looks like MD5-crypt"));
    }

    #[test]
    fn encode_rejects_what_cannot_be_represented() {
        assert!(junos9_encode("", &[0; 32]).unwrap_err().to_string().contains("nothing to encode"));
        assert!(junos9_encode("pässword", &[0; 32]).unwrap_err().to_string().contains("printable ASCII"));
        assert!(junos9_encode("line\nbreak", &[0; 32]).unwrap_err().to_string().contains("`\\n` cannot be encoded"));
    }
}
