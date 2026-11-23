//! One marking in every way it is written.
//!
//! The byte in question is the second byte of the IPv4 header (the Type of
//! Service byte) and the Traffic Class byte of IPv6. Its top six bits are the
//! DSCP (RFC 2474), its bottom two the ECN field (RFC 3168), and its top
//! three are what was IP precedence before there was a DSCP (RFC 791):
//!
//! ```text
//!   bit   7   6   5   4   3   2   1   0
//!       +---+---+---+---+---+---+---+---+
//!       |         DSCP          |  ECN  |
//!       +---+---+---+---+---+---+---+---+
//!       | precedence|
//! ```
//!
//! The same number therefore means three different things depending on what
//! it is a number *of* — 46 is EF as a DSCP and something else entirely as a
//! ToS byte — which is why [`marking`] is told, and never guesses.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::reference::{dscp_named, dscp_pool, dscp_registered};

/// The longest input worth reading: no marking is written in more than this.
const MAX_INPUT_CHARS: usize = 64;

/// What the value handed to [`marking`] is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarkingForm {
    /// A DSCP: a number from 0 to 63, or a name as a device accepts after `dscp`.
    Dscp,
    /// The whole ToS / Traffic Class byte, 0 to 255, ECN bits included.
    Tos,
    /// An IP precedence: a number from 0 to 7, or its name.
    Precedence,
    /// A code point name and nothing else: `ef`, `af41`, `cs3`, `default`.
    Name,
}

impl MarkingForm {
    /// What this form is called in an error message.
    fn what(self) -> &'static str {
        match self {
            Self::Dscp => "DSCP",
            Self::Tos => "ToS byte",
            Self::Precedence => "IP precedence",
            Self::Name => "DSCP name",
        }
    }
}

impl FromStr for MarkingForm {
    type Err = ToolError;

    /// `dscp`, `tos`, `precedence` or `name`, in either case.
    fn from_str(text: &str) -> Result<Self, ToolError> {
        match text.trim().to_ascii_lowercase().as_str() {
            "dscp" => Ok(Self::Dscp),
            "tos" => Ok(Self::Tos),
            "precedence" => Ok(Self::Precedence),
            "name" => Ok(Self::Name),
            _ => Err(ToolError::invalid(
                "kind of marking",
                text.trim(),
                "say what the value is: `dscp`, `tos`, `precedence` or `name`",
            )),
        }
    }
}

/// A marking in every form, as [`marking`] returns it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Marking {
    /// The six-bit DSCP, 0-63.
    pub dscp: u8,
    /// The code point's name (`EF`, `AF41`, `CS3`, `DF`), or `None` for a
    /// value the registry has not named.
    pub name: Option<String>,
    /// The DSCP as six binary digits: `101110`.
    pub dscp_binary: String,
    /// The DSCP in hexadecimal: `0x2E`.
    pub dscp_hex: String,
    /// The whole ToS / Traffic Class byte. Its ECN bits are the ones that
    /// were given with a ToS byte, and clear otherwise.
    pub tos: u8,
    /// The byte in hexadecimal: `0xB8`.
    pub tos_hex: String,
    /// The byte as eight binary digits: `10111000`.
    pub tos_binary: String,
    /// The IP precedence: the top three bits, 0-7.
    pub precedence: u8,
    /// The precedence's name in RFC 791: `Routine` ... `Network Control`.
    pub precedence_name: String,
    /// The ECN field: the bottom two bits of the byte, 0-3.
    pub ecn: u8,
    /// The ECN field as two binary digits: `10`.
    pub ecn_binary: String,
    /// The ECN codepoint: `Not-ECT`, `ECT(1)`, `ECT(0)` or `CE`.
    pub ecn_name: String,
    /// What that codepoint tells a router and a receiver.
    pub ecn_meaning: String,
    /// The per-hop behaviour and where it is defined; for a value without
    /// one, the registry pool the value belongs to.
    pub phb: String,
    /// What the code point is normally used for, when it has a standard use.
    pub typical_use: Option<String>,
    /// What the answer assumes that the input did not say.
    pub notes: Vec<String>,
}

/// The eight precedences of RFC 791 (section 3.1), from 0 up, each with the
/// keywords devices use for it.
const PRECEDENCES: [(&str, &[&str]); 8] = [
    ("Routine", &["routine"]),
    ("Priority", &["priority"]),
    ("Immediate", &["immediate"]),
    ("Flash", &["flash"]),
    ("Flash Override", &["flash-override"]),
    ("CRITIC/ECP", &["critical", "critic", "critical-ecp"]),
    (
        "Internetwork Control",
        &["internet", "internet-control", "internetwork-control"],
    ),
    (
        "Network Control",
        &["network", "net-control", "network-control"],
    ),
];

/// The four ECN codepoints of RFC 3168 (section 5), indexed by the field's
/// value. `ECT(1)` is `01` and `ECT(0)` is `10`: the names are not in
/// numerical order.
const ECN: [(&str, &str); 4] = [
    (
        "Not-ECT",
        "Not ECN-capable: a congested router can only drop this packet (RFC 3168)",
    ),
    (
        "ECT(1)",
        "ECN-capable transport; in RFC 9331 this codepoint identifies L4S traffic",
    ),
    (
        "ECT(0)",
        "ECN-capable transport: the codepoint a sender normally sets (RFC 3168)",
    ),
    (
        "CE",
        "Congestion experienced: a router on the path marked the packet instead of dropping it (RFC 3168)",
    ),
];

/// Write one marking in every form.
///
/// `form` says what `value` is:
///
/// * [`MarkingForm::Dscp`] — `46`, `0x2e`, `0b101110`, or a name the way a
///   device takes it after `dscp` (`ef`, `af41`);
/// * [`MarkingForm::Name`] — a code point name only: `ef`, `af41`, `cs3`,
///   `le`, `default`, `be`;
/// * [`MarkingForm::Tos`] — the whole byte, `184`, `0xb8` or `0b10111000`,
///   whose bottom two bits are reported as the ECN field;
/// * [`MarkingForm::Precedence`] — `0` to `7`, or `routine` ... `network`.
///   Precedence fixes only the top three bits, so the answer is the class
///   selector for it and a note says so.
///
/// Numbers are decimal unless prefixed. A number outside the form's range is
/// refused rather than read as some other form.
pub fn marking(value: &str, form: MarkingForm) -> Result<Marking, ToolError> {
    let text = value.trim();
    let refuse = |reason: &str| ToolError::invalid(form.what(), text, reason);
    if text.is_empty() {
        return Err(refuse("nothing was entered"));
    }
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err(refuse("too long to be a marking"));
    }

    let mut notes = Vec::new();
    let tos = match form {
        MarkingForm::Dscp => {
            let dscp = match number(text) {
                Some(value @ 0..=63) => value as u8,
                Some(_) => {
                    // Six bare binary digits are the likeliest way to get
                    // here after a byte, so the refusal says how to write them.
                    return Err(refuse(
                        "a DSCP is six bits, 0 to 63; write binary with a prefix, as in 0b101110, and for the whole byte choose `tos`",
                    ));
                }
                None => dscp_named(text).ok_or_else(|| {
                    refuse(
                        "neither a number from 0 to 63 nor a code point name such as ef, af41 or cs3",
                    )
                })?,
            };
            dscp << 2
        }
        MarkingForm::Name => {
            if text.starts_with(|c: char| c.is_ascii_digit()) {
                return Err(refuse(
                    "that is a number, not a name; say whether it is a `dscp`, a `tos` byte or a `precedence`",
                ));
            }
            let dscp = dscp_named(text).ok_or_else(|| {
                refuse(
                    "not a code point name; the names are CS0 to CS7, AF11 to AF43, EF, VOICE-ADMIT, NQB, LE and DF (also `default` and `be`)",
                )
            })?;
            dscp << 2
        }
        MarkingForm::Tos => match number(text) {
            Some(value @ 0..=255) => value as u8,
            Some(_) => {
                return Err(refuse(
                    "the byte is eight bits, 0 to 255; write binary with a prefix, as in 0b10111000",
                ));
            }
            None => {
                return Err(refuse(
                    "not a number; write the byte in decimal (184), hexadecimal (0xb8) or binary (0b10111000)",
                ));
            }
        },
        MarkingForm::Precedence => {
            let precedence = match number(text) {
                Some(value @ 0..=7) => value as u8,
                Some(_) => return Err(refuse("IP precedence is three bits, 0 to 7")),
                None => precedence_named(text).ok_or_else(|| {
                    refuse(
                        "neither a number from 0 to 7 nor one of routine, priority, immediate, flash, flash-override, critical, internet, network",
                    )
                })?,
            };
            notes.push(format!(
                "IP precedence fixes only the top three bits, so precedence {precedence} covers DSCP {} to {}. \
                 Shown is the class selector CS{precedence}: what a precedence-{precedence} packet carries when the other bits are clear (RFC 2474).",
                precedence << 3,
                (precedence << 3) | 0b111,
            ));
            precedence << 5
        }
    };

    let dscp = tos >> 2;
    let ecn = tos & 0b11;
    let precedence = tos >> 5;
    match form {
        MarkingForm::Tos if ecn != 0 => notes.push(format!(
            "The DSCP is the top six bits of the byte; with the ECN bits clear the byte is {} (0x{:02X}).",
            dscp << 2,
            dscp << 2,
        )),
        MarkingForm::Tos => {}
        _ => notes.push(
            "The two ECN bits are not part of a DSCP or a precedence; the ToS byte is shown with them clear."
                .to_owned(),
        ),
    }

    let (name, phb, typical_use) = match dscp_registered(dscp) {
        Some((name, phb, typical_use)) => (
            Some(name.to_owned()),
            phb.to_owned(),
            Some(typical_use.to_owned()),
        ),
        None => (None, dscp_pool(dscp).to_owned(), None),
    };
    let (ecn_name, ecn_meaning) = ECN[usize::from(ecn)];
    Ok(Marking {
        dscp,
        name,
        dscp_binary: format!("{dscp:06b}"),
        dscp_hex: format!("0x{dscp:02X}"),
        tos,
        tos_hex: format!("0x{tos:02X}"),
        tos_binary: format!("{tos:08b}"),
        precedence,
        precedence_name: PRECEDENCES[usize::from(precedence)].0.to_owned(),
        ecn,
        ecn_binary: format!("{ecn:02b}"),
        ecn_name: ecn_name.to_owned(),
        ecn_meaning: ecn_meaning.to_owned(),
        phb,
        typical_use,
        notes,
    })
}

/// A whole number written in decimal, `0x` hexadecimal or `0b` binary.
///
/// `None` when the text is not a number at all; a number too big for the
/// type comes back as the largest one, so that the caller's range check is
/// what refuses it — "out of range" is a better answer than "not a number".
fn number(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let (digits, radix) = if let Some(hex) = lower.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(binary) = lower.strip_prefix("0b") {
        (binary, 2)
    } else {
        (lower.as_str(), 10)
    };
    // `from_str_radix` would take a sign; a marking has none.
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    Some(u32::from_str_radix(digits, radix).unwrap_or(u32::MAX))
}

/// The precedence a name stands for, in RFC 791's words or a device's keyword.
fn precedence_named(text: &str) -> Option<u8> {
    let wanted: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if matches!(c, ' ' | '_') { '-' } else { c })
        .collect();
    PRECEDENCES
        .iter()
        .position(|(name, keywords)| {
            keywords.contains(&wanted.as_str())
                || name.to_ascii_lowercase().replace(' ', "-") == wanted
        })
        .map(|index| index as u8)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::reference::dscp_table;

    fn read(value: &str, form: MarkingForm) -> Marking {
        marking(value, form).unwrap_or_else(|error| panic!("{value} as {form:?}: {error}"))
    }

    fn refusal(value: &str, form: MarkingForm) -> String {
        marking(value, form)
            .expect_err(&format!("{value} as {form:?} should be refused"))
            .to_string()
    }

    #[test]
    fn ef_is_the_same_marking_however_it_is_written() {
        // Hand-checked: EF is 46 = 0b101110 (RFC 3246); shifted past the two
        // ECN bits that is 0b10111000 = 184 = 0xB8; its top three bits are
        // 0b101 = 5, which RFC 791 calls CRITIC/ECP.
        let ef = read("46", MarkingForm::Dscp);
        assert_eq!(
            ef,
            Marking {
                dscp: 46,
                name: Some("EF".into()),
                dscp_binary: "101110".into(),
                dscp_hex: "0x2E".into(),
                tos: 184,
                tos_hex: "0xB8".into(),
                tos_binary: "10111000".into(),
                precedence: 5,
                precedence_name: "CRITIC/ECP".into(),
                ecn: 0,
                ecn_binary: "00".into(),
                ecn_name: "Not-ECT".into(),
                ecn_meaning: ECN[0].1.into(),
                phb: "Expedited Forwarding (RFC 3246)".into(),
                typical_use: ef.typical_use.clone(),
                notes: ef.notes.clone(),
            }
        );
        assert!(ef.typical_use.as_deref().unwrap().contains("Voice"));

        for (value, form) in [
            ("ef", MarkingForm::Name),
            (" EF ", MarkingForm::Name),
            ("ef", MarkingForm::Dscp),
            ("0x2e", MarkingForm::Dscp),
            ("0b101110", MarkingForm::Dscp),
        ] {
            assert_eq!(read(value, form), ef, "{value} as {form:?}");
        }
        // As a ToS byte the ECN bits were given, so the note about leaving
        // them clear is not there; everything else is the same marking.
        for value in ["184", "0xb8", "0XB8", "0b10111000"] {
            let from_tos = read(value, MarkingForm::Tos);
            assert_eq!(from_tos.notes, Vec::<String>::new(), "{value}");
            assert_eq!(
                Marking {
                    notes: ef.notes.clone(),
                    ..from_tos
                },
                ef,
                "{value}"
            );
        }
    }

    /// The IANA registry "Differentiated Services Field Codepoints (DSCP)",
    /// pools 1 and 3, as fetched from
    /// <https://www.iana.org/assignments/dscp-registry/dscp-registry.txt> on
    /// 2026-11-22 (last updated 2026-05-13): name, value in binary, value in
    /// decimal, reference. Beside each row, worked out by hand and not by this
    /// code: the DSCP in hexadecimal, the ToS byte (the six bits with two
    /// clear ECN bits to their right) in decimal and hexadecimal, and the
    /// precedence (the first three bits).
    #[rustfmt::skip]
    const IANA_REGISTRY: [(&str, &str, u8, &str, &str, u8, &str, u8); 24] = [
        ("CS0",         "000000",  0, "RFC 2474", "0x00",   0, "0x00", 0),
        ("CS1",         "001000",  8, "RFC 2474", "0x08",  32, "0x20", 1),
        ("CS2",         "010000", 16, "RFC 2474", "0x10",  64, "0x40", 2),
        ("CS3",         "011000", 24, "RFC 2474", "0x18",  96, "0x60", 3),
        ("CS4",         "100000", 32, "RFC 2474", "0x20", 128, "0x80", 4),
        ("CS5",         "101000", 40, "RFC 2474", "0x28", 160, "0xA0", 5),
        ("CS6",         "110000", 48, "RFC 2474", "0x30", 192, "0xC0", 6),
        ("CS7",         "111000", 56, "RFC 2474", "0x38", 224, "0xE0", 7),
        ("AF11",        "001010", 10, "RFC 2597", "0x0A",  40, "0x28", 1),
        ("AF12",        "001100", 12, "RFC 2597", "0x0C",  48, "0x30", 1),
        ("AF13",        "001110", 14, "RFC 2597", "0x0E",  56, "0x38", 1),
        ("AF21",        "010010", 18, "RFC 2597", "0x12",  72, "0x48", 2),
        ("AF22",        "010100", 20, "RFC 2597", "0x14",  80, "0x50", 2),
        ("AF23",        "010110", 22, "RFC 2597", "0x16",  88, "0x58", 2),
        ("AF31",        "011010", 26, "RFC 2597", "0x1A", 104, "0x68", 3),
        ("AF32",        "011100", 28, "RFC 2597", "0x1C", 112, "0x70", 3),
        ("AF33",        "011110", 30, "RFC 2597", "0x1E", 120, "0x78", 3),
        ("AF41",        "100010", 34, "RFC 2597", "0x22", 136, "0x88", 4),
        ("AF42",        "100100", 36, "RFC 2597", "0x24", 144, "0x90", 4),
        ("AF43",        "100110", 38, "RFC 2597", "0x26", 152, "0x98", 4),
        ("EF",          "101110", 46, "RFC 3246", "0x2E", 184, "0xB8", 5),
        ("VOICE-ADMIT", "101100", 44, "RFC 5865", "0x2C", 176, "0xB0", 5),
        ("LE",          "000001",  1, "RFC 8622", "0x01",   4, "0x04", 0),
        ("NQB",         "101101", 45, "RFC 9956", "0x2D", 180, "0xB4", 5),
    ];

    #[test]
    fn every_registered_code_point_reads_the_same_in_every_direction() {
        for (name, binary, dscp, reference, dscp_hex, tos, tos_hex, precedence) in IANA_REGISTRY {
            // The registry's CS0 is the code point RFC 4594 calls DF, and
            // that is the name the reference table shows it under.
            let shown = if name == "CS0" { "DF" } else { name };

            // Name to everything else.
            let by_name = read(name, MarkingForm::Name);
            assert_eq!(by_name.name.as_deref(), Some(shown), "{name}");
            assert_eq!(
                (
                    by_name.dscp,
                    by_name.dscp_binary.as_str(),
                    by_name.dscp_hex.as_str()
                ),
                (dscp, binary, dscp_hex),
                "{name}"
            );
            assert_eq!(
                (
                    by_name.tos,
                    by_name.tos_hex.as_str(),
                    by_name.tos_binary.as_str()
                ),
                (tos, tos_hex, format!("{binary}00").as_str()),
                "{name}"
            );
            assert_eq!(
                (by_name.precedence, by_name.ecn, by_name.ecn_name.as_str()),
                (precedence, 0, "Not-ECT"),
                "{name}"
            );
            assert!(by_name.phb.contains(reference), "{name}: {}", by_name.phb);
            assert!(by_name.typical_use.is_some(), "{name}");
            assert_eq!(read(&name.to_lowercase(), MarkingForm::Name), by_name);

            // DSCP, in each way a number is written and as a name, to the same.
            for written in [
                dscp.to_string(),
                dscp_hex.to_owned(),
                dscp_hex.to_lowercase(),
                format!("0b{binary}"),
                name.to_owned(),
            ] {
                assert_eq!(
                    read(&written, MarkingForm::Dscp),
                    by_name,
                    "{name} as {written}"
                );
            }

            // ToS byte to the same code point, whatever the ECN bits are,
            // and the ECN bits come back as they went in.
            for ecn in 0..4u8 {
                for written in [
                    (tos + ecn).to_string(),
                    format!("0x{:02x}", tos + ecn),
                    format!("0b{binary}{ecn:02b}"),
                ] {
                    let by_byte = read(&written, MarkingForm::Tos);
                    assert_eq!(
                        (by_byte.dscp, by_byte.name.as_deref(), by_byte.precedence),
                        (dscp, Some(shown), precedence),
                        "{name} as byte {written}"
                    );
                    assert_eq!((by_byte.tos, by_byte.ecn), (tos + ecn, ecn), "{written}");
                    assert_eq!(
                        (&by_byte.phb, &by_byte.typical_use),
                        (&by_name.phb, &by_name.typical_use)
                    );
                }
            }
            // With the ECN bits clear the byte is the marking the name gave,
            // but for the note that says the ECN bits were not given.
            let clear = read(&tos.to_string(), MarkingForm::Tos);
            assert_eq!(
                Marking {
                    notes: by_name.notes.clone(),
                    ..clear
                },
                by_name,
                "{name}"
            );

            // Precedence is the one direction that loses something: it gives
            // the class selector of the same three bits, which is this code
            // point only when this code point is a class selector.
            let by_precedence = read(&precedence.to_string(), MarkingForm::Precedence);
            assert_eq!(by_precedence.dscp, precedence * 8, "{name}");
            assert_eq!(by_precedence.precedence, precedence, "{name}");
            assert_eq!(by_precedence.dscp == dscp, name.starts_with("CS"), "{name}");
        }
        // The registry's names are all there is: every other value is unnamed.
        for value in 0..=63u8 {
            let registered = IANA_REGISTRY.iter().any(|row| row.2 == value);
            assert_eq!(
                read(&value.to_string(), MarkingForm::Dscp).name.is_some(),
                registered,
                "DSCP {value}"
            );
        }
    }

    #[test]
    fn the_same_number_is_a_different_marking_as_a_dscp_and_as_a_tos_byte() {
        // 46 as a byte is 0b00101110: DSCP 0b001011 = 11, ECN 0b10 = ECT(0).
        let byte = read("46", MarkingForm::Tos);
        assert_eq!((byte.dscp, byte.ecn, byte.precedence), (11, 2, 1));
        assert_eq!(byte.tos, 46);
        assert_ne!(byte.dscp, read("46", MarkingForm::Dscp).dscp);
        // And 5 is a precedence, a DSCP and a byte, three different markings.
        assert_eq!(read("5", MarkingForm::Precedence).dscp, 40);
        assert_eq!(read("5", MarkingForm::Dscp).dscp, 5);
        assert_eq!(read("5", MarkingForm::Tos).dscp, 1);
    }

    #[test]
    fn the_ecn_bits_of_a_tos_byte_are_reported_and_do_not_move_the_code_point() {
        // RFC 3168 section 5: 00 Not-ECT, 01 ECT(1), 10 ECT(0), 11 CE.
        for (byte, ecn, binary, name) in [
            ("0xb8", 0, "00", "Not-ECT"),
            ("0xb9", 1, "01", "ECT(1)"),
            ("0xba", 2, "10", "ECT(0)"),
            ("0xbb", 3, "11", "CE"),
        ] {
            let read = read(byte, MarkingForm::Tos);
            assert_eq!(
                (read.ecn, read.ecn_binary.as_str(), read.ecn_name.as_str()),
                (ecn, binary, name),
                "{byte}"
            );
            assert_eq!(
                (read.dscp, read.name.as_deref()),
                (46, Some("EF")),
                "{byte}"
            );
            // The byte that was given is the byte that is shown.
            assert_eq!(read.tos_hex.to_lowercase(), byte);
        }
        let marked = read("0xbb", MarkingForm::Tos);
        assert!(marked.ecn_meaning.contains("Congestion experienced"));
        assert_eq!(
            marked.notes,
            [
                "The DSCP is the top six bits of the byte; with the ECN bits clear the byte is 184 (0xB8)."
            ]
        );
    }

    #[test]
    fn a_precedence_is_shown_as_its_class_selector_and_says_what_it_leaves_open() {
        // RFC 791 section 3.1, and RFC 2474 section 4.2.2: CSn = n * 8.
        for (precedence, name, selector) in [
            (0u8, "Routine", "DF"),
            (1, "Priority", "CS1"),
            (2, "Immediate", "CS2"),
            (3, "Flash", "CS3"),
            (4, "Flash Override", "CS4"),
            (5, "CRITIC/ECP", "CS5"),
            (6, "Internetwork Control", "CS6"),
            (7, "Network Control", "CS7"),
        ] {
            let read = read(&precedence.to_string(), MarkingForm::Precedence);
            assert_eq!(read.precedence, precedence);
            assert_eq!(read.precedence_name, name);
            assert_eq!(read.dscp, precedence * 8);
            assert_eq!(read.tos, precedence * 32);
            assert_eq!(read.name.as_deref(), Some(selector));
        }
        let critical = read("critical", MarkingForm::Precedence);
        assert_eq!(critical, read("5", MarkingForm::Precedence));
        assert_eq!(
            critical.notes[0],
            "IP precedence fixes only the top three bits, so precedence 5 covers DSCP 40 to 47. \
             Shown is the class selector CS5: what a precedence-5 packet carries when the other bits are clear (RFC 2474)."
        );
        for (name, precedence) in [
            ("routine", 0),
            ("Flash Override", 4),
            ("flash-override", 4),
            ("flash_override", 4),
            ("CRITIC/ECP", 5),
            ("internet", 6),
            ("Internetwork Control", 6),
            ("network", 7),
            ("net-control", 7),
        ] {
            assert_eq!(
                read(name, MarkingForm::Precedence).precedence,
                precedence,
                "{name}"
            );
        }
        // EF read back: its precedence is 5 although it is not a class selector.
        assert_eq!(read("ef", MarkingForm::Name).precedence, 5);
    }

    #[test]
    fn a_value_nobody_named_has_no_name_rather_than_an_invented_one() {
        let unnamed = read("47", MarkingForm::Dscp);
        assert_eq!(unnamed.name, None);
        assert_eq!(unnamed.typical_use, None);
        assert!(unnamed.phb.contains("pool 2"), "{}", unnamed.phb);
        assert_eq!((unnamed.tos, unnamed.precedence), (188, 5));
        // Sixty-four values, of which the registry names twenty-four.
        let named = (0..=63u8)
            .filter(|value| read(&value.to_string(), MarkingForm::Dscp).name.is_some())
            .count();
        assert_eq!(named, 24);
    }

    #[test]
    fn names_and_behaviours_are_the_reference_tables_own() {
        // One table, two readers: a name that changed there changes here.
        for entry in dscp_table() {
            let read = read(&entry.name, MarkingForm::Name);
            assert_eq!(read.dscp, entry.dscp, "{}", entry.name);
            assert_eq!(read.name.as_deref(), Some(entry.name.as_str()));
            assert_eq!(read.phb, entry.phb);
            assert_eq!(
                read.typical_use.as_deref(),
                Some(entry.typical_use.as_str())
            );
            assert_eq!(read.tos, entry.tos);
            assert_eq!(read.precedence, entry.precedence);
            assert_eq!(read.dscp_binary, entry.binary);
        }
        for (alias, dscp) in [("default", 0), ("be", 0), ("cs0", 0), ("va", 44)] {
            assert_eq!(read(alias, MarkingForm::Name).dscp, dscp, "{alias}");
        }
    }

    #[test]
    fn every_byte_comes_apart_into_its_three_fields_and_goes_back_together() {
        for byte in 0..=255u8 {
            let read = read(&byte.to_string(), MarkingForm::Tos);
            assert_eq!(read.tos, byte);
            assert_eq!(read.dscp, byte / 4);
            assert_eq!(read.ecn, byte % 4);
            assert_eq!(read.precedence, byte / 32);
            assert_eq!(u8::from_str_radix(&read.tos_binary, 2), Ok(byte));
            assert_eq!(
                format!("{}{}", read.dscp_binary, read.ecn_binary),
                read.tos_binary
            );
            assert_eq!(u8::from_str_radix(&read.tos_hex[2..], 16), Ok(byte));
            assert_eq!(u8::from_str_radix(&read.dscp_hex[2..], 16), Ok(byte / 4));
            // Read back as a DSCP the code point is the same and the ECN
            // bits, which a DSCP does not have, are clear.
            let again = super::marking(&read.dscp.to_string(), MarkingForm::Dscp).unwrap();
            assert_eq!(
                (again.dscp, again.ecn, again.tos),
                (byte / 4, 0, byte & 0xfc)
            );
            assert_eq!((again.name, again.phb), (read.name, read.phb));
        }
    }

    #[test]
    fn a_number_out_of_range_is_refused_not_read_as_something_else() {
        // The lookup in the reference table reads 184 as a ToS byte when it
        // is asked for a DSCP; the calculator was told what the number is.
        let too_big = refusal("184", MarkingForm::Dscp);
        assert_eq!(
            too_big,
            "invalid DSCP `184`: a DSCP is six bits, 0 to 63; write binary with a prefix, as in 0b101110, \
             and for the whole byte choose `tos`"
        );
        // Six bare binary digits are the decimal number 101 110, and the
        // refusal says how binary is written rather than reading them as EF.
        assert!(refusal("101110", MarkingForm::Dscp).contains("0b101110"));
        assert!(refusal("64", MarkingForm::Dscp).contains("0 to 63"));
        assert!(refusal("256", MarkingForm::Tos).contains("0 to 255"));
        assert!(refusal("0x100", MarkingForm::Tos).contains("0 to 255"));
        // Eight bare binary digits are a decimal number far out of range,
        // and the refusal says how binary is written.
        assert!(refusal("10111000", MarkingForm::Tos).contains("0b10111000"));
        assert!(refusal("8", MarkingForm::Precedence).contains("0 to 7"));
        assert!(refusal("99999999999999999999", MarkingForm::Dscp).contains("0 to 63"));
    }

    #[test]
    fn what_is_not_a_marking_is_refused_with_what_would_have_been_one() {
        assert_eq!(
            refusal("46", MarkingForm::Name),
            "invalid DSCP name `46`: that is a number, not a name; say whether it is a `dscp`, a `tos` byte or a `precedence`"
        );
        assert!(refusal("af44", MarkingForm::Name).contains("AF11 to AF43"));
        assert!(refusal("critical", MarkingForm::Name).contains("not a code point name"));
        assert!(refusal("voice", MarkingForm::Dscp).contains("ef, af41 or cs3"));
        assert!(refusal("ef", MarkingForm::Tos).contains("not a number"));
        assert!(refusal("urgent", MarkingForm::Precedence).contains("flash-override"));
        assert_eq!(
            refusal("  ", MarkingForm::Tos),
            "invalid ToS byte: nothing was entered"
        );
        for junk in [
            "-1", "+46", "4.6", "4 6", "0x", "0b", "0b102", "0xg1", "४६", "1e1",
        ] {
            for form in [MarkingForm::Dscp, MarkingForm::Tos, MarkingForm::Precedence] {
                assert!(marking(junk, form).is_err(), "{junk:?} as {form:?}");
            }
        }
        assert!(refusal(&"1".repeat(65), MarkingForm::Tos).contains("too long"));
    }

    #[test]
    fn the_form_is_one_of_four_words() {
        assert_eq!("dscp".parse(), Ok(MarkingForm::Dscp));
        assert_eq!(" ToS ".parse(), Ok(MarkingForm::Tos));
        assert_eq!("precedence".parse(), Ok(MarkingForm::Precedence));
        assert_eq!("name".parse(), Ok(MarkingForm::Name));
        let refused = "cos".parse::<MarkingForm>().unwrap_err().to_string();
        assert_eq!(
            refused,
            "invalid kind of marking `cos`: say what the value is: `dscp`, `tos`, `precedence` or `name`"
        );
        assert!("".parse::<MarkingForm>().is_err());
        assert_eq!(
            serde_json::to_string(&MarkingForm::Precedence).unwrap(),
            "\"precedence\""
        );
    }
}
