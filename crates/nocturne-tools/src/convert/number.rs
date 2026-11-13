//! One integer in every base an engineer writes it in.

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::error::snip;
use crate::ip::{Addr, format_ipv4};

/// The same number in decimal, hexadecimal, binary and octal, and as an
/// IPv4 address when it fits in 32 bits.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NumberForms {
    /// `3232235777`
    pub decimal: String,
    /// `0xC0A80101`
    pub hex: String,
    /// `0b11000000101010000000000100000001`
    pub binary: String,
    /// `0o30052000401`
    pub octal: String,
    /// `192.168.1.1`, for values up to 2^32 - 1.
    pub ipv4: Option<String>,
}

/// Convert a number between bases.
///
/// Accepts decimal (`3232235777`), hexadecimal (`0xC0A80101`), binary
/// (`0b1010`), octal (`0o17`), an IPv4 address (`192.168.1.1`) or an IPv6
/// address, up to 128 bits. Digits may be grouped with `_` or spaces
/// (`0b1100_0000`, `0xdead beef`).
pub fn number(input: &str) -> Result<NumberForms, ToolError> {
    const WHAT: &str = "number";
    let text = input.trim();
    if text.is_empty() {
        return Err(ToolError::invalid(WHAT, "", "nothing was entered"));
    }
    // 128 binary digits with separators fit several times over.
    if text.len() > 1024 {
        return Err(ToolError::TooLarge(format!(
            "a number of {} characters does not fit in 128 bits",
            text.len()
        )));
    }
    let value = if text.contains([':', '.']) {
        // One dot is a decimal fraction, not an address with three octets missing.
        if text.matches('.').count() == 1 && text.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return Err(ToolError::invalid(
                WHAT,
                text,
                "only whole numbers can be converted",
            ));
        }
        Addr::parse(text)?.value
    } else {
        let (radix, digits, base) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
            Some("0x") => (16, &text[2..], "a hexadecimal"),
            Some("0b") => (2, &text[2..], "a binary"),
            Some("0o") => (8, &text[2..], "an octal"),
            _ => (10, text, "a decimal"),
        };
        parse_digits(digits, radix, base).map_err(|error| match error {
            DigitError::TooLarge => {
                ToolError::TooLarge(format!("`{}` does not fit in 128 bits", snip(text, 60)))
            }
            DigitError::Invalid(reason) => ToolError::invalid(WHAT, text, reason),
        })?
    };
    Ok(NumberForms {
        decimal: value.to_string(),
        hex: format!("0x{value:X}"),
        binary: format!("0b{value:b}"),
        octal: format!("0o{value:o}"),
        ipv4: u32::try_from(value).ok().map(format_ipv4),
    })
}

enum DigitError {
    TooLarge,
    Invalid(String),
}

fn parse_digits(digits: &str, radix: u32, base: &str) -> Result<u128, DigitError> {
    let mut value: u128 = 0;
    let mut seen = 0usize;
    let mut previous_was_separator = true;
    for character in digits.chars() {
        if matches!(character, '_' | ' ') {
            if previous_was_separator {
                return Err(DigitError::Invalid(
                    "digit separators must sit between digits".to_owned(),
                ));
            }
            previous_was_separator = true;
            continue;
        }
        previous_was_separator = false;
        let Some(digit) = character.to_digit(radix) else {
            return Err(DigitError::Invalid(match character {
                '-' => "only non-negative whole numbers can be converted".to_owned(),
                'a'..='f' | 'A'..='F' if radix == 10 => {
                    format!(
                        "`{character}` is not a decimal digit; write hexadecimal numbers with a `0x` prefix"
                    )
                }
                _ => format!("`{character}` is not {base} digit"),
            }));
        };
        value = value
            .checked_mul(u128::from(radix))
            .and_then(|value| value.checked_add(u128::from(digit)))
            .ok_or(DigitError::TooLarge)?;
        seen += 1;
    }
    if seen == 0 {
        return Err(DigitError::Invalid(format!(
            "the digits are missing after the prefix for {base} number"
        )));
    }
    if previous_was_separator {
        return Err(DigitError::Invalid(
            "digit separators must sit between digits".to_owned(),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn every_input_form_gives_every_output_form() {
        let expected = NumberForms {
            decimal: "3232235777".into(),
            hex: "0xC0A80101".into(),
            binary: "0b11000000101010000000000100000001".into(),
            octal: "0o30052000401".into(),
            ipv4: Some("192.168.1.1".into()),
        };
        for input in [
            "3232235777",
            "0xC0A80101",
            "0xc0a80101",
            "0XC0A80101",
            "0b11000000101010000000000100000001",
            "0b1100_0000_1010_1000_0000_0001_0000_0001",
            "0b11000000 10101000 00000001 00000001",
            "0o30052000401",
            "192.168.1.1",
            " 3_232_235_777 ",
            "0xc0a8 0101",
            "::c0a8:101",
        ] {
            assert_eq!(number(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn small_and_large_values() {
        let ten = number("0b1010").unwrap();
        assert_eq!(
            (
                ten.decimal.as_str(),
                ten.hex.as_str(),
                ten.binary.as_str(),
                ten.octal.as_str(),
                ten.ipv4.as_deref()
            ),
            ("10", "0xA", "0b1010", "0o12", Some("0.0.0.10"))
        );
        let zero = number("0").unwrap();
        assert_eq!(
            (
                zero.decimal.as_str(),
                zero.hex.as_str(),
                zero.binary.as_str(),
                zero.octal.as_str()
            ),
            ("0", "0x0", "0b0", "0o0")
        );
        assert_eq!(number("0x0").unwrap(), zero);
        assert_eq!(number("0.0.0.0").unwrap(), zero);

        assert_eq!(
            number("4294967295").unwrap().ipv4.as_deref(),
            Some("255.255.255.255")
        );
        let beyond = number("4294967296").unwrap();
        assert_eq!((beyond.hex.as_str(), beyond.ipv4), ("0x100000000", None));

        let v6 = number("2001:db8::1").unwrap();
        assert_eq!(v6.decimal, "42540766411282592856903984951653826561");
        assert_eq!(v6.hex, "0x20010DB8000000000000000000000001");
        assert_eq!(v6.ipv4, None);

        let max = number("340282366920938463463374607431768211455").unwrap();
        assert_eq!(max.hex, format!("0x{}", "F".repeat(32)));
        assert_eq!(max.binary.len(), 2 + 128);
        assert_eq!(number("0xffffffffffffffffffffffffffffffff").unwrap(), max);
        assert_eq!(
            number("0o3777777777777777777777777777777777777777777").unwrap(),
            max
        );
    }

    #[test]
    fn rejections_say_what_is_wrong() {
        let message = |input: &str| number(input).unwrap_err().to_string();
        assert_eq!(message(""), "invalid number: nothing was entered");
        assert_eq!(
            message("340282366920938463463374607431768211456"),
            "`340282366920938463463374607431768211456` does not fit in 128 bits"
        );
        assert!(matches!(
            number(&"9".repeat(500)),
            Err(ToolError::TooLarge(_))
        ));
        assert_eq!(
            message("-5"),
            "invalid number `-5`: only non-negative whole numbers can be converted"
        );
        assert_eq!(
            message("ff"),
            "invalid number `ff`: `f` is not a decimal digit; write hexadecimal numbers with a `0x` prefix"
        );
        assert_eq!(
            message("0b102"),
            "invalid number `0b102`: `2` is not a binary digit"
        );
        assert_eq!(
            message("0o8"),
            "invalid number `0o8`: `8` is not an octal digit"
        );
        assert_eq!(
            message("0xg"),
            "invalid number `0xg`: `g` is not a hexadecimal digit"
        );
        assert_eq!(
            message("0x"),
            "invalid number `0x`: the digits are missing after the prefix for a hexadecimal number"
        );
        assert_eq!(
            message("12a"),
            "invalid number `12a`: `a` is not a decimal digit; write hexadecimal numbers with a `0x` prefix"
        );
        assert_eq!(
            message("1__0"),
            "invalid number `1__0`: digit separators must sit between digits"
        );
        assert_eq!(
            message("_10"),
            "invalid number `_10`: digit separators must sit between digits"
        );
        assert_eq!(
            message("10_"),
            "invalid number `10_`: digit separators must sit between digits"
        );
        assert_eq!(
            message("1,000"),
            "invalid number `1,000`: `,` is not a decimal digit"
        );
        assert_eq!(
            message("١٢٣"),
            "invalid number `١٢٣`: `١` is not a decimal digit"
        );
        assert_eq!(
            message("300.1.1.1"),
            "invalid IPv4 address `300.1.1.1`: `300` is not a valid octet (0-255)"
        );
        assert_eq!(
            message("1.5"),
            "invalid number `1.5`: only whole numbers can be converted"
        );
        assert_eq!(
            message("10.1.5"),
            "invalid IPv4 address `10.1.5`: expected 4 octets separated by dots, found 3"
        );
        assert!(message("é0").contains("is not a decimal digit"));
    }
}
