//! Writing syslog: an RFC 5424 message from its parts.
//!
//! The counterpart of [`parse`](super::parse). What goes in is text a device or a person wrote;
//! what comes out has to be one well-formed message whatever that text was, because the
//! receiver splits a stream into messages by what this module emits. So every field is brought
//! inside what the grammar allows for it — a space in a host name would shift every field
//! after it, and a line break in a message would start a second message of the sender's
//! choosing at the collector.

use std::fmt::Write as _;

/// `HOSTNAME` may be this long (RFC 5424 §6).
const MAX_HOSTNAME: usize = 255;
const MAX_APP_NAME: usize = 48;
const MAX_PROCID: usize = 128;
const MAX_MSGID: usize = 32;
const MAX_SD_NAME: usize = 32;
/// The marker a UTF-8 `MSG` has to begin with (§6.4).
const BOM: &str = "\u{feff}";

/// One `SD-ELEMENT`: an id and its parameters.
///
/// An id without `@` has to be one IANA registered (`origin`, `meta`, `timeQuality`); a
/// private one is `name@<enterprise number>`. This type does not check which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SdElement<'a> {
    pub id: &'a str,
    pub params: &'a [(&'a str, &'a str)],
}

/// The parts of one RFC 5424 message. `None` is written as the NILVALUE `-`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rfc5424<'a> {
    /// 0–23; anything larger is written as 23, the last facility there is.
    pub facility: u8,
    /// 0 (emergency) – 7 (debug); anything larger is written as 7.
    pub severity: u8,
    /// Milliseconds since the Unix epoch; written in UTC with millisecond precision.
    pub timestamp_ms: Option<i64>,
    pub hostname: Option<&'a str>,
    pub app: Option<&'a str>,
    pub procid: Option<&'a str>,
    pub msgid: Option<&'a str>,
    pub structured: &'a [SdElement<'a>],
    pub message: &'a str,
}

impl Rfc5424<'_> {
    /// The message as it goes on the wire, without any framing.
    ///
    /// Never longer than `max_len` bytes as long as the header fits in that: the message
    /// text is what is cut, at its end and between characters, which is where RFC 5424 says
    /// a message may be truncated. The header and the structured data are never cut — half
    /// a field is a different field.
    pub fn encode(&self, max_len: usize) -> Vec<u8> {
        let mut out = String::with_capacity(96 + self.message.len());
        let priority = u16::from(self.facility.min(23)) * 8 + u16::from(self.severity.min(7));
        let _ = write!(out, "<{priority}>1 ");
        match self.timestamp_ms {
            Some(at) => out.push_str(&timestamp(at)),
            None => out.push('-'),
        }
        for (value, limit) in [
            (self.hostname, MAX_HOSTNAME),
            (self.app, MAX_APP_NAME),
            (self.procid, MAX_PROCID),
            (self.msgid, MAX_MSGID),
        ] {
            out.push(' ');
            push_header_field(&mut out, value.unwrap_or_default(), limit);
        }
        out.push(' ');
        let before = out.len();
        for element in self.structured {
            push_element(&mut out, element);
        }
        if out.len() == before {
            out.push('-');
        }

        let text = clean_text(self.message);
        let text = text.trim_end();
        if !text.is_empty() {
            let marker = if text.is_ascii() { "" } else { BOM };
            let room = max_len.saturating_sub(out.len() + 1 + marker.len());
            let mut cut = text.len().min(room);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            if cut > 0 {
                out.push(' ');
                out.push_str(marker);
                out.push_str(&text[..cut]);
            }
        }
        out.into_bytes()
    }
}

/// `2026-11-22T22:56:48.123Z`.
fn timestamp(unix_ms: i64) -> String {
    let seconds = crate::util::format_unix_utc(unix_ms.div_euclid(1_000));
    let seconds = seconds.strip_suffix('Z').unwrap_or(&seconds);
    format!("{seconds}.{:03}Z", unix_ms.rem_euclid(1_000))
}

/// A header field: printable ASCII without spaces, at most `limit` long, `-` when there is
/// nothing to say. What the grammar does not allow becomes one `_` per run, so `Core Switch`
/// stays recognisable as `Core_Switch` rather than turning into two fields.
fn push_header_field(out: &mut String, value: &str, limit: usize) {
    let start = out.len();
    let mut replaced = false;
    for character in value.trim().chars() {
        if out.len() - start >= limit {
            break;
        }
        if ('!'..='~').contains(&character) {
            out.push(character);
            replaced = false;
        } else if !replaced {
            out.push('_');
            replaced = true;
        }
    }
    if out.len() == start {
        out.push('-');
    }
}

/// `[id name="value" …]`. An element whose id has nothing printable in it is left out: there
/// is no way to write it that a receiver could read back.
fn push_element(out: &mut String, element: &SdElement<'_>) {
    let id = sd_name(element.id);
    if id.is_empty() {
        return;
    }
    out.push('[');
    out.push_str(&id);
    for (name, value) in element.params {
        let name = sd_name(name);
        if name.is_empty() {
            continue;
        }
        out.push(' ');
        out.push_str(&name);
        out.push_str("=\"");
        for character in value.chars() {
            match character {
                // The three characters a PARAM-VALUE has to escape (§6.3.3).
                '"' | '\\' | ']' => {
                    out.push('\\');
                    out.push(character);
                }
                character if character.is_control() => out.push(' '),
                character => out.push(character),
            }
        }
        out.push('"');
    }
    out.push(']');
}

/// An SD-NAME: printable ASCII except `=`, space, `]` and `"`, at most 32 characters.
fn sd_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|character| match character {
            '=' | ']' | '"' => '_',
            character if ('!'..='~').contains(&character) => character,
            _ => '_',
        })
        .take(MAX_SD_NAME)
        .collect()
}

/// The message text with every control character but the tab turned into a space.
///
/// A line break is the one that matters: on a stream framed by line ends it would end this
/// message and begin another, written by whoever wrote the text.
fn clean_text(text: &str) -> std::borrow::Cow<'_, str> {
    let unsafe_here = |character: char| character.is_control() && character != '\t';
    if !text.chars().any(unsafe_here) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.chars()
            .map(|character| {
                if unsafe_here(character) {
                    ' '
                } else {
                    character
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn text(message: &Rfc5424<'_>) -> String {
        String::from_utf8(message.encode(64 * 1024)).expect("the encoder writes UTF-8")
    }

    fn source() -> std::net::SocketAddr {
        "192.0.2.7:514".parse().expect("an address")
    }

    #[test]
    fn a_message_is_written_field_by_field_as_the_rfc_orders_them() {
        let message = Rfc5424 {
            facility: 23,
            severity: 3,
            timestamp_ms: Some(1_795_388_208_123),
            hostname: Some("sw1.example.net"),
            app: Some("nocturne"),
            procid: None,
            msgid: Some("LINK-3-UPDOWN"),
            structured: &[SdElement {
                id: "origin",
                params: &[("ip", "192.0.2.7")],
            }],
            message: "Interface GigabitEthernet0/1, changed state to down",
        };
        assert_eq!(
            text(&message),
            "<187>1 2026-11-22T22:56:48.123Z sw1.example.net nocturne - LINK-3-UPDOWN \
             [origin ip=\"192.0.2.7\"] Interface GigabitEthernet0/1, changed state to down"
        );
    }

    #[test]
    fn what_is_not_known_is_a_nil_value_and_an_empty_message_is_no_message() {
        let message = Rfc5424 {
            facility: 1,
            severity: 5,
            ..Rfc5424::default()
        };
        assert_eq!(text(&message), "<13>1 - - - - - -");
    }

    #[test]
    fn our_own_parser_reads_back_what_was_written() {
        let message = Rfc5424 {
            facility: 13,
            severity: 6,
            timestamp_ms: Some(1_700_000_000_042),
            hostname: Some("CORE1"),
            app: Some("nocturne"),
            procid: Some("77"),
            msgid: Some("logs.forward.save"),
            structured: &[SdElement {
                id: "origin",
                params: &[("software", "Nocturne"), ("swVersion", "0.1.0")],
            }],
            message: "Saved the log forwarder `SIEM`",
        };
        let parsed = super::super::parse(&message.encode(64 * 1024), source());
        assert_eq!(parsed.facility, Some(13));
        assert_eq!(parsed.severity, Some(6));
        assert_eq!(
            parsed.timestamp.as_deref(),
            Some("2023-11-14T22:13:20.042Z")
        );
        assert_eq!(parsed.hostname.as_deref(), Some("CORE1"));
        assert_eq!(parsed.app.as_deref(), Some("nocturne"));
        assert_eq!(parsed.procid.as_deref(), Some("77"));
        assert_eq!(parsed.msgid.as_deref(), Some("logs.forward.save"));
        assert_eq!(parsed.message, "Saved the log forwarder `SIEM`");
    }

    #[test]
    fn a_name_with_spaces_stays_one_field() {
        let message = Rfc5424 {
            hostname: Some("Core Switch  Zürich"),
            app: Some(" "),
            msgid: Some("a\tb"),
            message: "up",
            ..Rfc5424::default()
        };
        assert_eq!(text(&message), "<0>1 - Core_Switch_Z_rich - - a_b - up");
    }

    #[test]
    fn a_field_longer_than_the_rfc_allows_is_cut_to_what_it_allows() {
        let long = "x".repeat(400);
        let message = Rfc5424 {
            hostname: Some(&long),
            app: Some(&long),
            procid: Some(&long),
            msgid: Some(&long),
            ..Rfc5424::default()
        };
        let written = text(&message);
        let fields: Vec<&str> = written.split(' ').collect();
        assert_eq!(
            fields[2..6]
                .iter()
                .map(|field| field.len())
                .collect::<Vec<_>>(),
            vec![255, 48, 128, 32]
        );
    }

    #[test]
    fn a_line_break_in_the_text_cannot_start_a_second_message() {
        let message = Rfc5424 {
            facility: 23,
            severity: 5,
            hostname: Some("sw1\n<0>1 - forged"),
            structured: &[SdElement {
                id: "origin",
                params: &[("ip", "10.0.0.1\n<0>1 - forged")],
            }],
            message: "logged in\r\n<0>1 - core1 - - - the core is on fire\u{0}",
            ..Rfc5424::default()
        };
        let written = message.encode(64 * 1024);
        assert!(
            !written.iter().any(|byte| matches!(byte, b'\n' | b'\r' | 0)),
            "{:?}",
            String::from_utf8_lossy(&written)
        );
        // And it still parses as the one message it is.
        let parsed = super::super::parse(&written, source());
        assert_eq!(parsed.severity, Some(5));
        assert!(parsed.message.starts_with("logged in"), "{parsed:?}");
    }

    #[test]
    fn quotes_and_brackets_in_a_parameter_are_escaped() {
        let message = Rfc5424 {
            structured: &[
                SdElement {
                    id: "x@32473",
                    params: &[("note", r#"a "b" ]c\"#), ("bad name=", "v")],
                },
                SdElement {
                    id: "  ",
                    params: &[("left", "out")],
                },
            ],
            ..Rfc5424::default()
        };
        assert_eq!(
            text(&message),
            r#"<0>1 - - - - - [x@32473 note="a \"b\" \]c\\" bad_name_="v"]"#
        );
    }

    #[test]
    fn text_that_is_not_ascii_is_marked_as_utf8_and_plain_text_is_not() {
        let plain = Rfc5424 {
            message: "port up",
            ..Rfc5424::default()
        };
        assert!(!text(&plain).contains(BOM));
        let accented = Rfc5424 {
            message: "Schnittstelle geändert",
            ..Rfc5424::default()
        };
        assert!(text(&accented).ends_with("- \u{feff}Schnittstelle geändert"));
    }

    #[test]
    fn a_message_too_long_for_its_transport_is_cut_at_the_end_between_characters() {
        let message = Rfc5424 {
            hostname: Some("sw1"),
            message: &"é".repeat(600),
            ..Rfc5424::default()
        };
        let written = message.encode(480);
        assert!(written.len() <= 480, "{}", written.len());
        assert!(written.len() >= 478);
        let written = String::from_utf8(written).expect("cut between characters");
        assert!(written.starts_with("<0>1 - sw1 - - - - \u{feff}éé"));

        // A limit the header alone does not fit in leaves the header whole.
        assert_eq!(
            String::from_utf8(message.encode(4)).unwrap(),
            "<0>1 - sw1 - - - -"
        );
    }

    #[test]
    fn a_facility_or_severity_out_of_range_is_written_as_the_last_one_there_is() {
        let message = Rfc5424 {
            facility: 200,
            severity: 9,
            ..Rfc5424::default()
        };
        assert!(text(&message).starts_with("<191>1 "));
    }

    #[test]
    fn a_time_before_1970_is_still_a_time() {
        let message = Rfc5424 {
            timestamp_ms: Some(-1),
            ..Rfc5424::default()
        };
        assert!(text(&message).starts_with("<0>1 1969-12-31T23:59:59.999Z "));
    }
}
