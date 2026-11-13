//! Text utilities: a regex tester, digests and Base64.

use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, NO_PAD, STANDARD};
use md5::Md5;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::ToolError;

/// [`regex_test`] reports at most this many matches.
pub const MAX_REGEX_MATCHES: usize = 10_000;

/// Upper bound on the compiled size of a pattern (bytes), the `regex`
/// crate's own default. Generous for anything written by hand (`\w{200}`
/// fits); patterns like `(a{1000}){1000}` are refused.
const REGEX_SIZE_LIMIT: usize = 10 << 20;

/// Longer patterns are refused without being parsed.
const MAX_PATTERN_LEN: usize = 64 * 1024;

/// One match of a pattern.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RegexMatch {
    /// Byte offset of the start of the match in the haystack.
    pub start: usize,
    /// Byte offset just past its end.
    pub end: usize,
    /// The matched text.
    pub text: String,
    /// The capture groups 1, 2, ... in order; `None` for a group that did not take part.
    pub groups: Vec<Option<String>>,
    /// The named groups, in the order they appear in the pattern.
    pub named: Vec<(String, Option<String>)>,
}

/// What a pattern finds in a text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct RegexResult {
    /// The matches, in order of appearance.
    pub matches: Vec<RegexMatch>,
    /// Why the pattern could not be used, or a note that the list of
    /// matches was cut off at [`MAX_REGEX_MATCHES`].
    pub error: Option<String>,
}

/// Try a regular expression against a text.
///
/// The syntax is that of the Rust `regex` crate (Perl-like, without
/// look-around and back-references), which guarantees linear-time matching:
/// no pattern can hang the tester. With `multiline`, `^` and `$` match at
/// every line, and lines may end in `\r\n` as device output often does.
pub fn regex_test(
    pattern: &str,
    haystack: &str,
    multiline: bool,
    case_insensitive: bool,
) -> RegexResult {
    if pattern.len() > MAX_PATTERN_LEN {
        let error = format!(
            "the pattern is {} bytes long; the limit is {MAX_PATTERN_LEN}",
            pattern.len()
        );
        return RegexResult {
            matches: Vec::new(),
            error: Some(error),
        };
    }
    let regex = RegexBuilder::new(pattern)
        .multi_line(multiline)
        .crlf(multiline)
        .case_insensitive(case_insensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .build();
    let regex = match regex {
        Ok(regex) => regex,
        Err(error) => {
            return RegexResult {
                matches: Vec::new(),
                error: Some(error.to_string()),
            };
        }
    };
    let names: Vec<&str> = regex.capture_names().flatten().collect();
    let mut found = regex.captures_iter(haystack);
    let matches: Vec<RegexMatch> = found
        .by_ref()
        .take(MAX_REGEX_MATCHES)
        .map(|captures| {
            let text =
                |group: Option<regex::Match<'_>>| group.map(|group| group.as_str().to_owned());
            let whole = captures.get(0);
            RegexMatch {
                start: whole.map_or(0, |whole| whole.start()),
                end: whole.map_or(0, |whole| whole.end()),
                text: text(whole).unwrap_or_default(),
                groups: (1..captures.len())
                    .map(|index| text(captures.get(index)))
                    .collect(),
                named: names
                    .iter()
                    .map(|name| ((*name).to_owned(), text(captures.name(name))))
                    .collect(),
            }
        })
        .collect();
    let error = found
        .next()
        .map(|_| format!("there are more than {MAX_REGEX_MATCHES} matches; only the first {MAX_REGEX_MATCHES} are shown"));
    RegexResult { matches, error }
}

/// The common digests of one input, in lower-case hex.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Hashes {
    /// MD5 (32 hex digits).
    pub md5: String,
    /// SHA-1 (40 hex digits).
    pub sha1: String,
    /// SHA-256 (64 hex digits).
    pub sha256: String,
    /// SHA-512 (128 hex digits).
    pub sha512: String,
}

/// Compute the MD5, SHA-1, SHA-256 and SHA-512 digests of `data`, e.g. to
/// check a software image against the vendor's published checksum.
pub fn hashes(data: &[u8]) -> Hashes {
    Hashes {
        md5: hex::encode(Md5::digest(data)),
        sha1: hex::encode(Sha1::digest(data)),
        sha256: hex::encode(Sha256::digest(data)),
        sha512: hex::encode(Sha512::digest(data)),
    }
}

/// Encode bytes as standard Base64 (RFC 4648, with `=` padding).
pub fn base64_encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// Decode Base64 as it is found in the wild.
///
/// Whitespace and line breaks anywhere are ignored, padding may be missing,
/// and the URL-safe alphabet (`-` and `_`) is accepted as well as the
/// standard one.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, ToolError> {
    const WHAT: &str = "Base64 text";
    /// Standard alphabet, no padding expected (it is stripped first), and
    /// tolerant of the unused bits of the last character being set.
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        NO_PAD
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::RequireNone),
    );

    let mut symbols = Vec::with_capacity(text.len());
    for character in text.chars().filter(|c| !c.is_whitespace()) {
        let symbol = match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' | '=' => character as u8,
            '-' => b'+',
            '_' => b'/',
            _ => {
                return Err(ToolError::invalid(
                    WHAT,
                    text.trim(),
                    format!("`{}` is not a Base64 character", character.escape_default()),
                ));
            }
        };
        symbols.push(symbol);
    }
    let data_len = symbols
        .iter()
        .rposition(|symbol| *symbol != b'=')
        .map_or(0, |last| last + 1);
    if symbols[..data_len].contains(&b'=') {
        return Err(ToolError::invalid(
            WHAT,
            text.trim(),
            "`=` padding is only allowed at the end",
        ));
    }
    if data_len % 4 == 1 {
        return Err(ToolError::invalid(
            WHAT,
            text.trim(),
            format!(
                "its length ({data_len}) cannot be right: every 4 characters carry 3 bytes, so the text is truncated"
            ),
        ));
    }
    LENIENT
        .decode(&symbols[..data_len])
        .map_err(|error| ToolError::invalid(WHAT, text.trim(), error.to_string()))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn regex_matches_with_groups() {
        let output = "GigabitEthernet0/1 is up, line protocol is up\nGigabitEthernet0/2 is administratively down, line protocol is down\n";
        let result = regex_test(
            r"^(?P<interface>\S+) is (?P<status>.+), line protocol is (up|down)$",
            output,
            true,
            false,
        );
        assert_eq!(result.error, None);
        assert_eq!(result.matches.len(), 2);
        let first = &result.matches[0];
        assert_eq!((first.start, first.end), (0, 45));
        assert_eq!(first.text, "GigabitEthernet0/1 is up, line protocol is up");
        assert_eq!(
            first.groups,
            [
                Some("GigabitEthernet0/1".to_owned()),
                Some("up".to_owned()),
                Some("up".to_owned())
            ]
        );
        assert_eq!(
            first.named,
            [
                (
                    "interface".to_owned(),
                    Some("GigabitEthernet0/1".to_owned())
                ),
                ("status".to_owned(), Some("up".to_owned()))
            ]
        );
        let second = &result.matches[1];
        assert_eq!(second.start, 46);
        assert_eq!(&output[second.start..second.end], second.text);
        assert_eq!(second.groups[1].as_deref(), Some("administratively down"));
    }

    #[test]
    fn regex_flags() {
        let text = "Vlan10\nvlan20\nVLAN30";
        // Without multiline, ^ only matches at the very start.
        assert_eq!(regex_test(r"^vlan\d+", text, false, false).matches.len(), 0);
        assert_eq!(regex_test(r"^vlan\d+", text, false, true).matches.len(), 1);
        assert_eq!(regex_test(r"^vlan\d+", text, true, false).matches.len(), 1);
        let all = regex_test(r"^vlan\d+$", text, true, true);
        assert_eq!(
            all.matches
                .iter()
                .map(|found| found.text.as_str())
                .collect::<Vec<_>>(),
            ["Vlan10", "vlan20", "VLAN30"]
        );
        // Lines that end in CR LF, as captured from a terminal.
        let crlf = regex_test(r"^vlan\d+$", "Vlan10\r\nvlan20\r\n", true, true);
        assert_eq!(
            crlf.matches
                .iter()
                .map(|found| found.text.as_str())
                .collect::<Vec<_>>(),
            ["Vlan10", "vlan20"]
        );
    }

    #[test]
    fn regex_optional_groups_and_offsets() {
        let result = regex_test(r"(\d+)(?:/(\d+))?", "ports 7 and 1/24", false, false);
        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].groups, [Some("7".to_owned()), None]);
        assert_eq!(
            result.matches[1].groups,
            [Some("1".to_owned()), Some("24".to_owned())]
        );
        assert!(result.matches.iter().all(|found| found.named.is_empty()));
        // Offsets are bytes: "é" takes two.
        let accented = regex_test("b", "éb", false, false);
        assert_eq!((accented.matches[0].start, accented.matches[0].end), (2, 3));
    }

    #[test]
    fn regex_errors_are_reported_not_thrown() {
        let unclosed = regex_test("(unclosed", "text", false, false);
        assert!(unclosed.matches.is_empty());
        assert!(
            unclosed
                .error
                .as_deref()
                .unwrap()
                .contains("unclosed group"),
            "{:?}",
            unclosed.error
        );
        let lookahead = regex_test("(?=x)", "x", false, false);
        assert!(
            lookahead.error.as_deref().unwrap().contains("look-around"),
            "{:?}",
            lookahead.error
        );
        let backreference = regex_test(r"(a)\1", "aa", false, false);
        assert!(
            backreference
                .error
                .as_deref()
                .unwrap()
                .contains("backreferences are not supported"),
            "{:?}",
            backreference.error
        );
        let huge = regex_test("(a{1000}){1000}{1000}", "a", false, false);
        assert!(
            huge.error.as_deref().unwrap().contains("size limit"),
            "{:?}",
            huge.error
        );
        let deep = regex_test(&"(".repeat(5000), "a", false, false);
        assert!(deep.error.is_some());
        let long = regex_test(&"a".repeat(MAX_PATTERN_LEN + 1), "a", false, false);
        assert_eq!(
            long.error.as_deref(),
            Some("the pattern is 65537 bytes long; the limit is 65536")
        );
        // Unicode classes are large, but everyday repetitions of them still fit.
        assert_eq!(regex_test(r"\w{100}", "x", false, false).error, None);
    }

    #[test]
    fn regex_match_lists_are_capped() {
        // The empty pattern matches at every position.
        let haystack = "x".repeat(50_000);
        let result = regex_test("", &haystack, false, false);
        assert_eq!(result.matches.len(), MAX_REGEX_MATCHES);
        assert_eq!(
            result.error.as_deref(),
            Some("there are more than 10000 matches; only the first 10000 are shown")
        );
        let exact = regex_test("x", &"x".repeat(MAX_REGEX_MATCHES), false, false);
        assert_eq!(
            (exact.matches.len(), exact.error),
            (MAX_REGEX_MATCHES, None)
        );
        assert_eq!(regex_test("", "", false, false).matches.len(), 1);
    }

    #[test]
    fn digests_of_the_standard_test_vectors() {
        // RFC 1321, RFC 3174 and FIPS 180-4 examples.
        let empty = hashes(b"");
        assert_eq!(empty.md5, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(empty.sha1, "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            empty.sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            empty.sha512,
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        let abc = hashes(b"abc");
        assert_eq!(abc.md5, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(abc.sha1, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            abc.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            abc.sha512,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hashes(b"The quick brown fox jumps over the lazy dog").md5,
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }

    #[test]
    fn base64_round_trip() {
        // RFC 4648 section 10.
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), encoded);
            assert_eq!(base64_decode(encoded).unwrap(), plain.as_bytes());
        }
        let binary: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&binary)).unwrap(), binary);
        assert_eq!(base64_encode(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn base64_decoding_is_tolerant() {
        // Missing padding.
        assert_eq!(base64_decode("Zg").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8").unwrap(), b"fo");
        assert_eq!(base64_decode("Zg=").unwrap(), b"f");
        // Whitespace and line breaks, as in PEM or a wrapped e-mail.
        assert_eq!(base64_decode("  Zm9v\r\nYmFy\n").unwrap(), b"foobar");
        assert_eq!(base64_decode("Z m 9 v\tY g = =").unwrap(), b"foob");
        // URL-safe alphabet.
        assert_eq!(base64_decode("-_8").unwrap(), [0xfb, 0xff]);
        assert_eq!(base64_decode("+/8=").unwrap(), [0xfb, 0xff]);
        // Unused bits set in the last character (sloppy encoders).
        assert_eq!(base64_decode("Zh==").unwrap(), b"f");
        assert_eq!(base64_decode("").unwrap(), b"");
        assert_eq!(base64_decode(" \n ").unwrap(), b"");
        assert_eq!(base64_decode("====").unwrap(), b"");
    }

    #[test]
    fn base64_rejections_say_what_is_wrong() {
        let message = |text: &str| base64_decode(text).unwrap_err().to_string();
        assert_eq!(
            message("Zm9v!"),
            "invalid Base64 text `Zm9v!`: `!` is not a Base64 character"
        );
        assert_eq!(
            message("Zm=9v"),
            "invalid Base64 text `Zm=9v`: `=` padding is only allowed at the end"
        );
        assert_eq!(
            message("Zm9vY"),
            "invalid Base64 text `Zm9vY`: its length (5) cannot be right: every 4 characters carry 3 bytes, so the text is truncated"
        );
        assert!(message("Zm9v\u{1F600}").contains("is not a Base64 character"));
        assert!(message("Z").contains("its length (1) cannot be right"));
    }
}
