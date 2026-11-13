//! Cisco "type 7": the Vigenère-style obfuscation `service password-encryption` applies.
//!
//! A type 7 string is two decimal digits — an offset into a fixed, public
//! key — followed by the password XORed with the key from that offset on,
//! in hex: `0822455D0A16` is offset 08 and five bytes that decode to `cisco`.

use super::identify::{self, UNKNOWN};
use super::{MAX_INPUT_LEN, MAX_SECRET_LEN, printable_text, too_long};
use crate::ToolError;

/// The key every IOS image has shipped since the early 1990s.
const KEY: &[u8; 53] = b"dsfd;kfoA,.iyewrkldJKDHSUBsgvca69834ncxv9873254k;fg87";

/// The highest offset IOS itself generates.
const MAX_ENCODE_SALT: u8 = 15;

const WHAT: &str = "type 7 string";

/// Decode a Cisco type 7 string such as `0822455D0A16`.
///
/// The bare value, `7 <value>` or a whole configuration line containing
/// `7 <value>` (`enable password 7 0822455D0A16`) are all accepted.
pub fn cisco7_decode(encoded: &str) -> Result<String, ToolError> {
    let text = encoded.trim();
    if text.is_empty() {
        return Err(ToolError::invalid(WHAT, "", "nothing was entered"));
    }
    if text.len() > MAX_INPUT_LEN {
        return Err(ToolError::invalid(
            WHAT,
            text,
            "it is far too long to be a type 7 string",
        ));
    }
    let Some(value) = value_of(text) else {
        return Err(not_type7(
            text,
            text,
            "expected the encoded value on its own (e.g. `0822455D0A16`) or a line containing `7 <value>`".to_owned(),
        ));
    };
    decode(value).map_err(|reason| {
        if value.bytes().all(|b| b.is_ascii_hexdigit()) {
            ToolError::invalid(WHAT, value, reason)
        } else {
            not_type7(text, value, reason)
        }
    })
}

/// An error for input that is not hex at all; if it is a secret of another
/// kind, saying so is more useful than complaining about the first character.
fn not_type7(input: &str, value: &str, reason: String) -> ToolError {
    let other = identify::identify(input);
    if other.kind == UNKNOWN || other.kind == "cisco_type7" {
        ToolError::invalid(WHAT, value, reason)
    } else {
        ToolError::invalid(
            WHAT,
            value,
            format!("this looks like {}, not type 7", other.name),
        )
    }
}

/// Whether `value` is shaped like a type 7 string: a key offset followed by
/// at least one encoded character.
pub(super) fn has_type7_shape(value: &str) -> bool {
    value.len() >= 4 && parse(value).is_ok()
}

/// The plain text of a type 7 value, or `None` if it is not one.
pub(super) fn decode_value(value: &str) -> Option<String> {
    decode(value).ok()
}

/// Split a type 7 value into its key offset and the encoded bytes; the
/// error is the reason it is not well-formed.
fn parse(value: &str) -> Result<(usize, Vec<u8>), String> {
    if let Some(bad) = value.chars().find(|c| !c.is_ascii_hexdigit()) {
        return Err(format!(
            "`{bad}` is not a hex digit; a type 7 string is two decimal digits followed by hex digits"
        ));
    }
    if !value.len().is_multiple_of(2) {
        return Err(format!(
            "its length is {}, but a type 7 string always has an even length \
             (2 offset digits, then 2 hex digits per character); it is probably truncated",
            value.len()
        ));
    }
    let (salt_digits, body) = value.split_at(2.min(value.len()));
    let salt = match salt_digits.parse::<usize>() {
        Ok(salt) if salt < KEY.len() && salt_digits.bytes().all(|b| b.is_ascii_digit()) => salt,
        _ => {
            return Err(format!(
                "it must start with a decimal key offset from 00 to {} (IOS uses 00-{MAX_ENCODE_SALT:02}), not `{salt_digits}`",
                KEY.len() - 1
            ));
        }
    };
    // Every character is a hex digit (checked above).
    let hex = |digit: u8| (digit as char).to_digit(16).unwrap_or(0) as u8;
    Ok((
        salt,
        body.as_bytes()
            .chunks_exact(2)
            .map(|pair| (hex(pair[0]) << 4) | hex(pair[1]))
            .collect(),
    ))
}

fn decode(value: &str) -> Result<String, String> {
    let (salt, encoded) = parse(value)?;
    let bytes: Vec<u8> = encoded
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ KEY[(salt + index) % KEY.len()])
        .collect();
    printable_text(&bytes).ok_or_else(|| {
        "it decodes to unprintable bytes, so it is not type 7 obfuscation (or it is corrupted)"
            .to_owned()
    })
}

/// Encode `plain` as a Cisco type 7 string with the given key offset
/// (`salt`, 0-15 as IOS uses). The result is upper-case, as IOS prints it.
pub fn cisco7_encode(plain: &str, salt: u8) -> Result<String, ToolError> {
    if salt > MAX_ENCODE_SALT {
        return Err(ToolError::invalid(
            "type 7 salt",
            &salt.to_string(),
            format!("IOS only uses key offsets 0 to {MAX_ENCODE_SALT}"),
        ));
    }
    if plain.is_empty() {
        return Err(ToolError::invalid(
            "password",
            "",
            "there is nothing to encode",
        ));
    }
    if plain.len() > MAX_SECRET_LEN {
        return Err(too_long("password", plain.chars().count()));
    }
    if let Some(bad) = plain.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(ToolError::invalid(
            "password",
            plain,
            format!(
                "`{}` cannot be encoded: IOS passwords are printable ASCII",
                bad.escape_default()
            ),
        ));
    }
    let mut encoded = format!("{salt:02}");
    for (index, byte) in plain.bytes().enumerate() {
        encoded.push_str(&format!(
            "{:02X}",
            byte ^ KEY[(usize::from(salt) + index) % KEY.len()]
        ));
    }
    Ok(encoded)
}

/// Pick the type 7 value out of what was pasted: the value itself, the word
/// after the last lone `7` of a configuration line, or the word before a
/// final `7` (`ntp authentication-key 1 md5 <value> 7`).
fn value_of(text: &str) -> Option<&str> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let value = match words[..] {
        [only] => only,
        [.., value, "7"] => value,
        _ => words
            .iter()
            .rposition(|word| *word == "7")
            .and_then(|index| words.get(index + 1))?,
    };
    Some(value.trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cisco` under each of the sixteen offsets IOS uses. These strings are
    /// all over real configurations; `0822455D0A16` is the one in the spec.
    const CISCO: [&str; 16] = [
        "00071A150754",
        "01100F175804",
        "02050D480809",
        "030752180500",
        "045802150C2E",
        "05080F1C2243",
        "060506324F41",
        "070C285F4D06",
        "0822455D0A16",
        "094F471A1A0A",
        "104D000A0618",
        "110A1016141D",
        "121A0C041104",
        "13061E010803",
        "14141B180F0B",
        "1511021F0725",
    ];

    #[test]
    fn decodes_strings_from_real_configurations() {
        for encoded in CISCO {
            assert_eq!(cisco7_decode(encoded).unwrap(), "cisco", "{encoded}");
        }
        // Lower-case hex decodes just the same.
        assert_eq!(cisco7_decode("0822455d0a16").unwrap(), "cisco");
        // 0x63 ^ 'A' = 0x22, 0x69 ^ ',' = 0x45, 0x73 ^ '.' = 0x5D, 0x63 ^ 'i' = 0x0A, 0x6F ^ 'y' = 0x16.
        assert_eq!(KEY[8..13], *b"A,.iy");
    }

    #[test]
    fn accepts_configuration_lines() {
        for line in [
            "enable password 7 0822455D0A16",
            " password 7 0822455D0A16",
            "username admin privilege 15 password 7 0822455D0A16",
            "7 0822455D0A16",
            "neighbor 10.0.0.1 password 7 0822455D0A16",
            "tacacs-server key 7 \"0822455D0A16\"",
            "snmp-server community 7 0822455D0A16 RO",
            "ntp authentication-key 10 md5 0822455D0A16 7",
            "  0822455D0A16\n",
        ] {
            assert_eq!(cisco7_decode(line).unwrap(), "cisco", "{line}");
        }
    }

    #[test]
    fn encode_matches_ios_and_round_trips() {
        for (salt, expected) in CISCO.iter().enumerate() {
            assert_eq!(cisco7_encode("cisco", salt as u8).unwrap(), *expected);
        }
        // Longer than the key, so the key index wraps around.
        let long: String = (0..200)
            .map(|index| char::from(b' ' + (index % 95) as u8))
            .collect();
        for salt in 0..=15 {
            let encoded = cisco7_encode(&long, salt).unwrap();
            assert_eq!(encoded.len(), 2 + 2 * long.len());
            assert_eq!(cisco7_decode(&encoded).unwrap(), long);
        }
    }

    #[test]
    fn offsets_above_fifteen_decode_but_are_not_generated() {
        // Offset 52 is the last key byte, '7' (0x37); the next index wraps to 'd'.
        assert_eq!(cisco7_decode("525405").unwrap(), "ca");
        assert!(
            cisco7_decode("530000")
                .unwrap_err()
                .to_string()
                .contains("key offset from 00 to 52")
        );
        assert_eq!(
            cisco7_encode("x", 16).unwrap_err().to_string(),
            "invalid type 7 salt `16`: IOS only uses key offsets 0 to 15"
        );
    }

    #[test]
    fn rejections_say_what_is_wrong() {
        let message = |input: &str| cisco7_decode(input).unwrap_err().to_string();
        assert_eq!(message(""), "invalid type 7 string: nothing was entered");
        assert_eq!(message("   "), "invalid type 7 string: nothing was entered");
        assert!(
            message("0822455D0A1").contains("its length is 11"),
            "{}",
            message("0822455D0A1")
        );
        assert!(message("0822455D0A1G").contains("`G` is not a hex digit"));
        assert!(message("AB22455D0A16").contains("must start with a decimal key offset"));
        assert!(message("0").contains("its length is 1,"));
        // Valid shape, but the bytes are not text: 0x00 ^ key byte is fine, 0xFF ^ ... is not ASCII.
        assert!(message("08FFFFFF").contains("unprintable"));
        assert!(message("0841").contains("unprintable"), "0x41 ^ 'A' is NUL");
        assert!(message("enable secret cisco").contains("a line containing `7 <value>`"));
        // Other secret types are named.
        assert_eq!(
            message("$1$mERr$hx5rVt7rPNoS4wqbXKX7m0"),
            "invalid type 7 string `$1$mERr$hx5rVt7rPNoS4wqbXKX7m0`: this looks like MD5-crypt ($1$), not type 7"
        );
        assert_eq!(
            message("enable secret 5 $1$mERr$hx5rVt7rPNoS4wqbXKX7m0"),
            "invalid type 7 string `enable secret 5 $1$mERr$hx5rVt7rPNoS4wqbXKX7m0`: this looks like Cisco type 5 (MD5-crypt), not type 7"
        );
        assert!(message("$9$LbHX-wg4Z").contains("this looks like Junos $9$"));
    }

    #[test]
    fn absurd_lengths_are_refused_quickly() {
        let long = "A".repeat(MAX_SECRET_LEN);
        assert_eq!(
            cisco7_decode(&cisco7_encode(&long, 0).unwrap()).unwrap(),
            long
        );
        assert_eq!(
            cisco7_encode(&format!("{long}A"), 0).unwrap_err(),
            ToolError::TooLarge(
                "a password of 1,025 characters is beyond the limit of 1,024".into()
            )
        );
        let error = cisco7_decode(&format!("08{}", "22".repeat(MAX_INPUT_LEN)))
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("it is far too long to be a type 7 string"),
            "{error}"
        );
        assert!(error.len() < 200);
    }

    #[test]
    fn an_offset_alone_is_an_empty_password() {
        assert_eq!(cisco7_decode("08").unwrap(), "");
    }

    #[test]
    fn encode_rejects_what_ios_would_not_take() {
        assert!(
            cisco7_encode("", 3)
                .unwrap_err()
                .to_string()
                .contains("nothing to encode")
        );
        assert!(
            cisco7_encode("pässword", 3)
                .unwrap_err()
                .to_string()
                .contains("printable ASCII")
        );
        assert!(
            cisco7_encode("tab\there", 3)
                .unwrap_err()
                .to_string()
                .contains("`\\t` cannot be encoded")
        );
    }
}
