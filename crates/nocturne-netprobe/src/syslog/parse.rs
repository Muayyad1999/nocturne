//! The syslog message parser: RFC 5424, RFC 3164 and the dialects network devices speak.
//!
//! Parsing never fails. Each step peels a recognisable piece off the front of the line
//! (priority, counters, timestamp, host, process tag, vendor mnemonic) and whatever is left is
//! the message; a step that does not recognise its piece leaves the line alone.

use std::net::SocketAddr;
use std::sync::LazyLock;

use regex::Regex;

use super::{Mnemonic, SyslogMessage};

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the pattern is a valid regular expression")
}

/// Parses one datagram / line. Never fails: input that makes no sense as syslog becomes a
/// message with only `raw` and `message` set.
pub fn parse(datagram: &[u8], source: SocketAddr) -> SyslogMessage {
    let text = String::from_utf8_lossy(datagram);
    let raw = text
        .trim_start_matches('\u{feff}')
        .trim_end_matches(|c: char| c.is_whitespace() || c == '\0');
    let mut message = SyslogMessage {
        source: source.ip().to_string(),
        facility: None,
        severity: None,
        timestamp: None,
        hostname: None,
        app: None,
        procid: None,
        msgid: None,
        message: String::new(),
        mnemonic: None,
        sequence: None,
        raw: raw.to_owned(),
    };
    let rest = match split_priority(raw) {
        Some((priority, rest)) => {
            message.facility = Some(priority >> 3);
            message.severity = Some(priority & 0x07);
            rest
        }
        None => raw,
    };
    if !parse_rfc5424(rest, &mut message) {
        parse_legacy(rest, &mut message);
    }
    message
}

/// `<189>…` -> (189, "…"). Priorities above 191 do not exist.
fn split_priority(text: &str) -> Option<(u8, &str)> {
    let inner = text.strip_prefix('<')?;
    let (digits, rest) = inner.split_once('>')?;
    if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let priority: u8 = digits.parse().ok().filter(|priority| *priority <= 191)?;
    Some((priority, rest))
}

// ------------------------------------------------------------------ RFC 5424

/// `1 TIMESTAMP HOSTNAME APP-NAME PROCID MSGID STRUCTURED-DATA [MSG]`.
/// Returns `false` (touching nothing) when the line is not in this format.
fn parse_rfc5424(rest: &str, message: &mut SyslogMessage) -> bool {
    static TIMESTAMP: LazyLock<Regex> =
        LazyLock::new(|| regex(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}"));
    static SEQUENCE_ID: LazyLock<Regex> = LazyLock::new(|| regex(r#"\bsequenceId="(\d+)""#));

    let Some(body) = rest.strip_prefix("1 ") else {
        return false;
    };
    let mut fields = body.splitn(6, ' ');
    let (Some(timestamp), Some(hostname), Some(app), Some(procid), Some(msgid), Some(tail)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    if timestamp != "-" && !TIMESTAMP.is_match(timestamp) {
        return false;
    }
    let Some((structured_data, text)) = split_structured_data(tail) else {
        return false;
    };
    let value = |field: &str| (field != "-" && !field.is_empty()).then(|| field.to_owned());
    message.timestamp = value(timestamp);
    message.hostname = value(hostname);
    message.app = value(app);
    message.procid = value(procid);
    message.msgid = value(msgid);
    message.sequence = structured_data
        .and_then(|data| SEQUENCE_ID.captures(data))
        .and_then(|captures| captures[1].parse().ok());

    let mut text = text.trim_start_matches('\u{feff}');
    // IOS in RFC 5424 mode keeps its message counter at the front of the text.
    if let Some((counter, after)) = peel_counters(text) {
        message.sequence.get_or_insert(counter);
        text = after;
    }
    if let Some(found) = vendor_mnemonic(text) {
        message.sequence = message.sequence.or(found.sequence);
        message.mnemonic = Some(found.mnemonic);
        message.message = found.rest.to_owned();
    } else {
        // Junos in structured-data mode carries its event tag as the MSGID.
        message.mnemonic = message.msgid.as_deref().and_then(junos_tag);
        message.message = match (text.is_empty(), structured_data) {
            (true, Some(data)) => data.to_owned(),
            _ => text.to_owned(),
        };
    }
    true
}

/// Splits `STRUCTURED-DATA [SP MSG]` into the structured data (`None` for `-`) and the text.
fn split_structured_data(tail: &str) -> Option<(Option<&str>, &str)> {
    if let Some(after) = tail.strip_prefix('-') {
        return match after.strip_prefix(' ') {
            Some(text) => Some((None, text)),
            None if after.is_empty() => Some((None, "")),
            None => None,
        };
    }
    // One or more `[id name="value" …]` elements; `"`, `\` and `]` are escaped inside values.
    let bytes = tail.as_bytes();
    let mut position = 0;
    while bytes.get(position) == Some(&b'[') {
        let mut in_value = false;
        let mut closed = false;
        position += 1;
        while let Some(byte) = bytes.get(position) {
            position += 1;
            match byte {
                b'\\' if in_value => position += 1,
                b'"' => in_value = !in_value,
                b']' if !in_value => {
                    closed = true;
                    break;
                }
                _ => {}
            }
        }
        if !closed {
            return None;
        }
    }
    if position == 0 {
        return None;
    }
    let (data, after) = tail.split_at_checked(position)?;
    match after.strip_prefix(' ') {
        Some(text) => Some((Some(data), text)),
        None if after.is_empty() => Some((Some(data), "")),
        None => None,
    }
}

// ----------------------------------------------------- RFC 3164 and dialects

const MONTH: &str = "(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)";

/// The legacy formats: BSD syslog as devices really send it.
fn parse_legacy(rest: &str, message: &mut SyslogMessage) {
    let mut text = rest;
    if looks_like_fortigate(text) {
        parse_fortigate(text, message);
        return;
    }

    // Cisco: "45: " message counter, optionally followed by a "000123: " sequence number;
    // NX-OS and ASA (EMBLEM format) start with a bare colon instead.
    let mut cisco = false;
    if let Some((counter, after)) = peel_counters(text) {
        message.sequence = Some(counter);
        cisco = true;
        text = after;
    } else if let Some(after) = text.strip_prefix(':').map(str::trim_start)
        && timestamp_len(after, true).is_some()
    {
        cisco = true;
        text = after;
    }
    // Cisco `logging origin-id`: the device's name, then a colon, in front of the timestamp.
    if let Some((origin, after)) = split_origin(text) {
        message.hostname = Some(origin.to_owned());
        cisco = true;
        text = after;
    }
    // IOS-XR: "[name ]RP/0/RSP0/CPU0:" glued to the timestamp.
    if let Some((name, after)) = split_node_id(text) {
        if let Some(name) = name {
            message.hostname = Some(name.to_owned());
        }
        cisco = true;
        text = after;
    }

    let Some(length) = timestamp_len(text, cisco) else {
        content(text, message);
        return;
    };
    let (timestamp, after) = text.split_at(length);
    message.timestamp = Some(timestamp.to_owned());
    match after.strip_prefix(':') {
        // Cisco layout: "<timestamp>: [process[pid]: ]%MNEMONIC: text".
        Some(body) => cisco_body(body.trim_start(), message),
        // BSD layout: "<timestamp> host tag[pid]: text".
        None => bsd_body(after.trim_start(), message),
    }
}

/// Strips Cisco's leading counters and returns the first one (the syslog message counter; the
/// second, if present, is `service sequence-numbers`). Only when what follows still looks
/// like a Cisco line, so that ordinary text beginning with a number is left alone.
fn peel_counters(text: &str) -> Option<(u64, &str)> {
    static COUNTER: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\d{1,10}):\s+"));
    let first = COUNTER.captures(text)?;
    let counter: u64 = first[1].parse().ok()?;
    let mut after = &text[first.get(0)?.end()..];
    if let Some(second) = COUNTER.find(after) {
        after = &after[second.end()..];
    }
    let plausible = after.starts_with('%')
        || timestamp_len(after, true).is_some()
        || split_origin(after).is_some()
        || split_node_id(after).is_some();
    plausible.then_some((counter, after))
}

/// `R1: *Mar  1 …` / `10.0.0.1: %SYS-…` -> ("R1", rest).
fn split_origin(text: &str) -> Option<(&str, &str)> {
    static ORIGIN: LazyLock<Regex> =
        LazyLock::new(|| regex(r"^([A-Za-z0-9][A-Za-z0-9._-]{0,62}):\s+"));
    let captures = ORIGIN.captures(text)?;
    let after = &text[captures.get(0)?.end()..];
    let origin = captures.get(1)?.as_str();
    (after.starts_with('%') || timestamp_len(after, true).is_some()).then_some((origin, after))
}

/// IOS-XR node location in front of the timestamp, with an optional `hostnameprefix`.
fn split_node_id(text: &str) -> Option<(Option<&str>, &str)> {
    static NODE: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"^(?:([A-Za-z0-9][A-Za-z0-9._-]*) )?[A-Z]{2,3}/\d+/[A-Z0-9]+/[A-Z0-9]+:")
    });
    let captures = NODE.captures(text)?;
    let after = &text[captures.get(0)?.end()..];
    timestamp_len(after, true)?;
    Some((captures.get(1).map(|name| name.as_str()), after))
}

/// Time zone abbreviations that may follow a timestamp.
const TIME_ZONES: &[&str] = &[
    "UTC", "GMT", "EST", "EDT", "CST", "CDT", "MST", "MDT", "PST", "PDT", "AKST", "AKDT", "HST",
    "AST", "ADT", "NST", "NDT", "BST", "IST", "WET", "WEST", "CET", "CEST", "EET", "EEST", "MSK",
    "TRT", "GST", "PKT", "ICT", "WIB", "SGT", "HKT", "JST", "KST", "AEST", "AEDT", "ACST", "ACDT",
    "AWST", "NZST", "NZDT", "BRT", "ART", "CLT", "SAST", "CAT", "EAT", "WAT", "IRST", "IDT",
];

/// The length of the timestamp at the start of `text`, if there is one.
///
/// Recognised: `Mmm dd hh:mm:ss` with the decorations devices add (a leading `*` or `.`,
/// milliseconds, a year before or after the time, a numeric offset, a zone name), the
/// year-first form of NX-OS, and ISO 8601 / RFC 3339.
fn timestamp_len(text: &str, cisco: bool) -> Option<usize> {
    static BSD: LazyLock<Regex> = LazyLock::new(|| {
        regex(&format!(
            r"^[*.]?{MONTH}\s+\d{{1,2}}\s+(?:\d{{4}}\s+)?\d{{2}}:\d{{2}}:\d{{2}}(?:\.\d+)?"
        ))
    });
    static YEAR_FIRST: LazyLock<Regex> = LazyLock::new(|| {
        regex(&format!(
            r"^\d{{4}}\s+{MONTH}\s+\d{{1,2}}\s+\d{{2}}:\d{{2}}:\d{{2}}(?:\.\d+)?"
        ))
    });
    static ISO: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?")
    });
    static OFFSET: LazyLock<Regex> = LazyLock::new(|| regex(r"^[+-]\d{2}:?\d{2}"));
    static YEAR_AFTER: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s(?:19|20)\d{2}(?:\s|$)"));
    static ZONE: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s([A-Za-z]{2,5})(:|\s)"));
    static ZONE_AND_YEAR: LazyLock<Regex> =
        LazyLock::new(|| regex(r"^\s[A-Z]{2,5}\s(?:19|20)\d{2}(?:\s|$)"));

    if let Some(found) = ISO.find(text) {
        // "2026 Nov 13 …" is not ISO; "2026-11-13 10:15:01" is.
        return Some(found.end());
    }
    let (mut end, year_first) = match (BSD.find(text), YEAR_FIRST.find(text)) {
        (Some(found), _) => (found.end(), false),
        (None, Some(found)) => (found.end(), true),
        (None, None) => return None,
    };
    if let Some(offset) = OFFSET.find(&text[end..]) {
        end += offset.end();
    }
    if YEAR_AFTER.is_match(&text[end..]) {
        // Comware: "Nov 13 10:15:01 2026".
        end += 5;
    }
    if let Some(captures) = ZONE.captures(&text[end..]) {
        let zone = &captures[1];
        let known = TIME_ZONES.contains(&zone);
        let after_zone = &text[end + 1 + zone.len()..];
        let accepted = match &captures[2] {
            // "… EET: %SYS-5-…": a zone name of the administrator's choosing, in Cisco's layout.
            ":" => {
                cisco
                    || known
                    || after_zone
                        .trim_start_matches(':')
                        .trim_start()
                        .starts_with('%')
            }
            // "… UTC host …": only the year-first (NX-OS) form has a zone before the host.
            _ => known && year_first,
        };
        if accepted {
            end += 1 + zone.len();
        } else if known && ZONE_AND_YEAR.is_match(&text[end..]) {
            // "Aug 24 05:34:00 CST 1987" (the nonconforming example of RFC 3164).
            end += 1 + zone.len() + 5;
        }
    }
    Some(end)
}

/// What follows a Cisco-style timestamp: an optional process, then the mnemonic or free text.
fn cisco_body(text: &str, message: &mut SyslogMessage) {
    static PROCESS: LazyLock<Regex> = LazyLock::new(|| regex(r"^([A-Za-z0-9_./-]+)\[(\d+)\]:\s+"));
    let mut text = text;
    if let Some(captures) = PROCESS.captures(text) {
        message.app = Some(captures[1].to_owned());
        message.procid = Some(captures[2].to_owned());
        text = &text[captures.get(0).map_or(0, |all| all.end())..];
    }
    match vendor_mnemonic(text) {
        Some(found) => {
            message.sequence = message.sequence.or(found.sequence);
            message.mnemonic = Some(found.mnemonic);
            message.message = found.rest.to_owned();
        }
        None => message.message = text.to_owned(),
    }
}

/// What follows a BSD-style timestamp: `host content`, or just content.
fn bsd_body(text: &str, message: &mut SyslogMessage) {
    let (first, after) = text.split_once(' ').unwrap_or((text, ""));
    let is_tag = first.ends_with(':') || first.contains('[');
    // A lone word after the timestamp is the message, not a host with nothing to say.
    let alone = after.trim().is_empty();
    if text.starts_with('%')
        || is_tag
        || alone
        || !looks_like_host(first)
        || message.hostname.is_some()
    {
        content(text, message);
        return;
    }
    message.hostname = Some(first.to_owned());
    // ASA with `logging device-id`: "asa1 : %ASA-6-…".
    let after = after.trim_start();
    let after = after.strip_prefix(':').map_or(after, str::trim_start);
    content(after, message);
}

fn looks_like_host(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 255
        && token.starts_with(|c: char| c.is_ascii_alphanumeric())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        // RouterOS topics ("system,info") and key=value pairs are not hosts; commas and '='
        // are excluded above, a trailing colon marks a tag.
        && !token.ends_with(':')
}

/// The content of a message once timestamp and host are gone: a vendor mnemonic, or a process
/// tag followed by text, or plain text.
fn content(text: &str, message: &mut SyslogMessage) {
    static TAG: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"^([A-Za-z0-9_./(][^\s:\[\]]{0,63})(?:\[([^\]\s]{0,32})\])?:(?:\s+|$)")
    });

    if looks_like_fortigate(text) {
        parse_fortigate(text, message);
        return;
    }
    if let Some(found) = vendor_mnemonic(text) {
        message.sequence = message.sequence.or(found.sequence);
        message.mnemonic = Some(found.mnemonic);
        message.message = found.rest.to_owned();
        return;
    }
    let Some(captures) = TAG.captures(text) else {
        message.message = text.to_owned();
        return;
    };
    message.app = Some(captures[1].to_owned());
    message.procid = captures
        .get(2)
        .map(|pid| pid.as_str().to_owned())
        .filter(|pid| !pid.is_empty());
    let body = &text[captures.get(0).map_or(0, |all| all.end())..];
    // Arista and IOS-XR put a Cisco mnemonic after the process; Junos puts its event tag there.
    if let Some(found) = vendor_mnemonic(body) {
        message.mnemonic = Some(found.mnemonic);
        message.message = found.rest.to_owned();
    } else if let Some((tag, rest)) = split_junos_tag(body) {
        message.mnemonic = junos_tag(tag);
        message.message = rest.to_owned();
    } else {
        message.message = body.to_owned();
    }
}

struct Found<'a> {
    mnemonic: Mnemonic,
    /// The log sequence number some vendors put inside the mnemonic.
    sequence: Option<u64>,
    rest: &'a str,
}

/// A vendor mnemonic at the start of `text`: Cisco `%FAC-n-NAME:`, Huawei / H3C
/// `%%01MOD/n/NAME(l)[seq]:`, or RouterOS topics (`system,info,account `).
fn vendor_mnemonic(text: &str) -> Option<Found<'_>> {
    static HUAWEI: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"^%%(\d{2})([A-Za-z0-9_-]+)/(\d)/([A-Za-z0-9_-]+)(?:\(\w+\))?(?:\[(\d+)\])?:\s*")
    });
    // The facility may itself contain hyphens ("ASA-session", "ROUTING-BGP"): it ends where a
    // single severity digit stands between two hyphens.
    static CISCO: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"^%([A-Za-z0-9_]+(?:-[A-Za-z0-9_]+)*?)-([0-7])-([A-Za-z0-9_]+)\s*:\s*")
    });
    static TOPICS: LazyLock<Regex> =
        LazyLock::new(|| regex(r"^([a-z0-9!-]+(?:,[a-z0-9!-]+)+)(?:\s+|$)"));

    if let Some(captures) = HUAWEI.captures(text) {
        return Some(Found {
            mnemonic: Mnemonic {
                facility: captures[2].to_owned(),
                severity: captures[3].parse().ok().filter(|severity| *severity <= 7),
                name: captures[4].to_owned(),
            },
            sequence: captures
                .get(5)
                .and_then(|sequence| sequence.as_str().parse().ok()),
            rest: &text[captures.get(0)?.end()..],
        });
    }
    if let Some(captures) = CISCO.captures(text) {
        return Some(Found {
            mnemonic: Mnemonic {
                facility: captures[1].to_owned(),
                severity: captures[2].parse().ok(),
                name: captures[3].to_owned(),
            },
            sequence: None,
            rest: &text[captures.get(0)?.end()..],
        });
    }
    let captures = TOPICS.captures(text)?;
    let topics: Vec<&str> = captures[1].split(',').collect();
    // Every RouterOS message carries one of the severity topics; that is what tells a topic
    // list from any other comma-separated word.
    let severity = topics
        .iter()
        .filter_map(|topic| routeros_severity(topic))
        .min()?;
    let others: Vec<&str> = topics[1..]
        .iter()
        .copied()
        .filter(|topic| routeros_severity(topic).is_none())
        .collect();
    Some(Found {
        mnemonic: Mnemonic {
            facility: topics[0].to_owned(),
            severity: Some(severity),
            name: others.join(","),
        },
        sequence: None,
        rest: &text[captures.get(0)?.end()..],
    })
}

fn routeros_severity(topic: &str) -> Option<u8> {
    match topic {
        "critical" => Some(2),
        "error" => Some(3),
        "warning" => Some(4),
        "info" => Some(6),
        "debug" => Some(7),
        _ => None,
    }
}

/// `UI_COMMIT: User …` -> ("UI_COMMIT", "User …").
fn split_junos_tag(text: &str) -> Option<(&str, &str)> {
    static TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"^([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+):\s+"));
    let captures = TAG.captures(text)?;
    Some((captures.get(1)?.as_str(), &text[captures.get(0)?.end()..]))
}

/// A Junos event tag as a mnemonic: the prefix names the process or library that logged it
/// (`UI_COMMIT` -> facility `UI`, name `COMMIT`).
fn junos_tag(tag: &str) -> Option<Mnemonic> {
    let (facility, name) = tag.split_once('_')?;
    let well_formed = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    (well_formed(facility)
        && well_formed(name)
        && facility.starts_with(|c: char| c.is_ascii_uppercase()))
    .then(|| Mnemonic {
        facility: facility.to_owned(),
        severity: None,
        name: name.to_owned(),
    })
}

// ------------------------------------------------------------------ FortiGate

/// FortiOS logs are a list of `key=value` pairs beginning with the date.
fn looks_like_fortigate(text: &str) -> bool {
    static START: LazyLock<Regex> = LazyLock::new(|| {
        regex(r"^(?:date=\d{4}-\d{2}-\d{2}\s+time=\d{2}:\d{2}:\d{2}|logver=\d+\s)")
    });
    START.is_match(text)
}

fn parse_fortigate(text: &str, message: &mut SyslogMessage) {
    static PAIR: LazyLock<Regex> =
        LazyLock::new(|| regex(r#"([A-Za-z_][A-Za-z0-9_.-]*)=(?:"((?:[^"\\]|\\.)*)"|(\S*))"#));
    let mut date = None;
    let mut time = None;
    let mut kind = None;
    let mut subtype = None;
    let mut level = None;
    let mut description = None;
    let mut text_field = None;
    for captures in PAIR.captures_iter(text) {
        let value = captures
            .get(2)
            .or(captures.get(3))
            .map_or("", |value| value.as_str());
        match &captures[1] {
            "date" => date = Some(value),
            "time" => time = Some(value),
            "devname" => message.hostname = Some(value.to_owned()),
            "logid" => message.msgid = Some(value.to_owned()),
            "type" => kind = Some(value),
            "subtype" => subtype = Some(value),
            "level" => level = Some(value),
            "logdesc" => description = Some(value),
            "msg" => text_field = Some(value),
            _ => {}
        }
    }
    message.timestamp = match (date, time) {
        (Some(date), Some(time)) => Some(format!("{date} {time}")),
        _ => None,
    };
    let level = level.and_then(|level| match level {
        "emergency" => Some(0),
        "alert" => Some(1),
        "critical" => Some(2),
        "error" => Some(3),
        "warning" => Some(4),
        "notice" => Some(5),
        "information" => Some(6),
        "debug" => Some(7),
        _ => None,
    });
    message.severity = message.severity.or(level);
    message.mnemonic = kind.map(|kind| Mnemonic {
        facility: kind.to_owned(),
        severity: level,
        name: subtype.unwrap_or_default().to_owned(),
    });
    // The administrator-readable part, when the log has one; otherwise the fields themselves.
    message.message = text_field
        .or(description)
        .unwrap_or(text)
        .replace("\\\"", "\"");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// The fields a test expects; everything not mentioned must be absent.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Expected {
        priority: Option<(u8, u8)>,
        timestamp: Option<&'static str>,
        hostname: Option<&'static str>,
        app: Option<&'static str>,
        procid: Option<&'static str>,
        msgid: Option<&'static str>,
        mnemonic: Option<(&'static str, Option<u8>, &'static str)>,
        sequence: Option<u64>,
        message: &'static str,
    }

    fn parse_line(line: &str) -> SyslogMessage {
        parse(line.as_bytes(), "192.0.2.77:51234".parse().unwrap())
    }

    #[track_caller]
    fn check(line: &str, expected: Expected) {
        let message = parse_line(line);
        let got = (
            message.facility.zip(message.severity),
            message.timestamp.as_deref(),
            message.hostname.as_deref(),
            message.app.as_deref(),
            message.procid.as_deref(),
            message.msgid.as_deref(),
            message.mnemonic.as_ref().map(|mnemonic| {
                (
                    mnemonic.facility.as_str(),
                    mnemonic.severity,
                    mnemonic.name.as_str(),
                )
            }),
            message.sequence,
            message.message.as_str(),
        );
        let want = (
            expected.priority,
            expected.timestamp,
            expected.hostname,
            expected.app,
            expected.procid,
            expected.msgid,
            expected.mnemonic,
            expected.sequence,
            expected.message,
        );
        assert_eq!(got, want, "\nline: {line}");
        assert_eq!(
            message.raw,
            line.trim_end_matches(|c: char| c.is_whitespace() || c == '\0')
        );
        assert_eq!(message.source, "192.0.2.77");
    }

    #[test]
    fn rfc5424_examples_from_the_rfc() {
        check(
            "<34>1 2003-10-11T22:14:15.003Z mymachine.example.com su - ID47 - \u{feff}'su root' failed for lonvick on /dev/pts/8",
            Expected {
                priority: Some((4, 2)),
                timestamp: Some("2003-10-11T22:14:15.003Z"),
                hostname: Some("mymachine.example.com"),
                app: Some("su"),
                msgid: Some("ID47"),
                message: "'su root' failed for lonvick on /dev/pts/8",
                ..Expected::default()
            },
        );
        check(
            "<165>1 2003-08-24T05:14:15.000003-07:00 192.0.2.1 myproc 8710 - - %% It's time to make the do-nuts.",
            Expected {
                priority: Some((20, 5)),
                timestamp: Some("2003-08-24T05:14:15.000003-07:00"),
                hostname: Some("192.0.2.1"),
                app: Some("myproc"),
                procid: Some("8710"),
                message: "%% It's time to make the do-nuts.",
                ..Expected::default()
            },
        );
        check(
            "<165>1 2003-10-11T22:14:15.003Z mymachine.example.com evntslog - ID47 [exampleSDID@32473 iut=\"3\" eventSource=\"Application\" eventID=\"1011\"] \u{feff}An application event log entry...",
            Expected {
                priority: Some((20, 5)),
                timestamp: Some("2003-10-11T22:14:15.003Z"),
                hostname: Some("mymachine.example.com"),
                app: Some("evntslog"),
                msgid: Some("ID47"),
                message: "An application event log entry...",
                ..Expected::default()
            },
        );
        // Structured data only: it is all there is to show.
        check(
            "<165>1 2003-10-11T22:14:15.003Z mymachine.example.com evntslog - ID47 [exampleSDID@32473 iut=\"3\"][examplePriority@32473 class=\"high\"]",
            Expected {
                priority: Some((20, 5)),
                timestamp: Some("2003-10-11T22:14:15.003Z"),
                hostname: Some("mymachine.example.com"),
                app: Some("evntslog"),
                msgid: Some("ID47"),
                message: "[exampleSDID@32473 iut=\"3\"][examplePriority@32473 class=\"high\"]",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn rfc5424_details() {
        // Every field nil.
        check(
            "<13>1 - - - - - -",
            Expected {
                priority: Some((1, 5)),
                ..Expected::default()
            },
        );
        check(
            "<13>1 - - - - - - text",
            Expected {
                priority: Some((1, 5)),
                message: "text",
                ..Expected::default()
            },
        );
        // Escapes inside structured data, and the RFC's sequenceId.
        check(
            r#"<14>1 2026-11-13T10:15:01Z sw1 app 77 - [meta sequenceId="4711" sysUpTime="1"][x@1 a="b\"]c \\"] done"#,
            Expected {
                priority: Some((1, 6)),
                timestamp: Some("2026-11-13T10:15:01Z"),
                hostname: Some("sw1"),
                app: Some("app"),
                procid: Some("77"),
                sequence: Some(4711),
                message: "done",
                ..Expected::default()
            },
        );
        // What `logger --rfc5424` sends.
        check(
            "<13>1 2026-11-13T10:15:01.123456+00:00 noc-ws root - - [timeQuality tzKnown=\"1\" isSynced=\"0\"] link flap on core1",
            Expected {
                priority: Some((1, 5)),
                timestamp: Some("2026-11-13T10:15:01.123456+00:00"),
                hostname: Some("noc-ws"),
                app: Some("root"),
                message: "link flap on core1",
                ..Expected::default()
            },
        );
        // A line that only begins like version 1 is not RFC 5424.
        check(
            "<13>1 potato salad",
            Expected {
                priority: Some((1, 5)),
                message: "1 potato salad",
                ..Expected::default()
            },
        );
        check(
            "<13>1 2026-11-13T10:15:01Z host app - - [broken sd",
            Expected {
                priority: Some((1, 5)),
                message: "1 2026-11-13T10:15:01Z host app - - [broken sd",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn rfc3164_examples_from_the_rfc() {
        check(
            "<34>Oct 11 22:14:15 mymachine su: 'su root' failed for lonvick on /dev/pts/8",
            Expected {
                priority: Some((4, 2)),
                timestamp: Some("Oct 11 22:14:15"),
                hostname: Some("mymachine"),
                app: Some("su"),
                message: "'su root' failed for lonvick on /dev/pts/8",
                ..Expected::default()
            },
        );
        check(
            "<13>Feb  5 17:32:18 10.0.0.99 Use the BFG!",
            Expected {
                priority: Some((1, 5)),
                timestamp: Some("Feb  5 17:32:18"),
                hostname: Some("10.0.0.99"),
                message: "Use the BFG!",
                ..Expected::default()
            },
        );
        // The RFC's example of a sender that adds a zone and a year.
        check(
            "<165>Aug 24 05:34:00 CST 1987 mymachine myproc[10]: %% It's time to make the do-nuts.  %%  Ingredients: Mix=OK, Jelly=OK #",
            Expected {
                priority: Some((20, 5)),
                timestamp: Some("Aug 24 05:34:00 CST 1987"),
                hostname: Some("mymachine"),
                app: Some("myproc"),
                procid: Some("10"),
                message: "%% It's time to make the do-nuts.  %%  Ingredients: Mix=OK, Jelly=OK #",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn bsd_syslog_from_unix_hosts() {
        check(
            "<30>Nov 13 10:15:01 gw systemd[1]: Started Daily apt download activities.",
            Expected {
                priority: Some((3, 6)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("gw"),
                app: Some("systemd"),
                procid: Some("1"),
                message: "Started Daily apt download activities.",
                ..Expected::default()
            },
        );
        check(
            "<86>Nov  3 10:15:01 gw sshd[2211]: Accepted publickey for admin from 10.0.0.2 port 51234 ssh2: ED25519 SHA256:abc",
            Expected {
                priority: Some((10, 6)),
                timestamp: Some("Nov  3 10:15:01"),
                hostname: Some("gw"),
                app: Some("sshd"),
                procid: Some("2211"),
                message: "Accepted publickey for admin from 10.0.0.2 port 51234 ssh2: ED25519 SHA256:abc",
                ..Expected::default()
            },
        );
        check(
            "<6>Nov 13 10:15:01 gw kernel: [12345.678901] eth0: link up",
            Expected {
                priority: Some((0, 6)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("gw"),
                app: Some("kernel"),
                message: "[12345.678901] eth0: link up",
                ..Expected::default()
            },
        );
        check(
            "<22>Nov 13 10:15:01 mx postfix/smtpd[991]: connect from unknown[203.0.113.9]",
            Expected {
                priority: Some((2, 6)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("mx"),
                app: Some("postfix/smtpd"),
                procid: Some("991"),
                message: "connect from unknown[203.0.113.9]",
                ..Expected::default()
            },
        );
        // Without a host name: the first word is the tag.
        check(
            "<78>Nov 13 10:15:01 CRON[4242]: (root) CMD (run-parts /etc/cron.hourly)",
            Expected {
                priority: Some((9, 6)),
                timestamp: Some("Nov 13 10:15:01"),
                app: Some("CRON"),
                procid: Some("4242"),
                message: "(root) CMD (run-parts /etc/cron.hourly)",
                ..Expected::default()
            },
        );
        check(
            "<38>Oct 11 22:14:15 su: 'su root' failed",
            Expected {
                priority: Some((4, 6)),
                timestamp: Some("Oct 11 22:14:15"),
                app: Some("su"),
                message: "'su root' failed",
                ..Expected::default()
            },
        );
        // rsyslog's forwarding format: an RFC 3339 timestamp in a BSD-shaped line.
        check(
            "<13>2026-11-13T10:15:01.123456+03:00 noc-ws root: test message",
            Expected {
                priority: Some((1, 5)),
                timestamp: Some("2026-11-13T10:15:01.123456+03:00"),
                hostname: Some("noc-ws"),
                app: Some("root"),
                message: "test message",
                ..Expected::default()
            },
        );
        // No timestamp at all.
        check(
            "<13>myscript: backup finished",
            Expected {
                priority: Some((1, 5)),
                app: Some("myscript"),
                message: "backup finished",
                ..Expected::default()
            },
        );
        check(
            "<13>Nov 13 10:15:01 rebooting",
            Expected {
                priority: Some((1, 5)),
                timestamp: Some("Nov 13 10:15:01"),
                message: "rebooting",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn cisco_ios() {
        check(
            "<189>45: *Mar  1 00:02:11.003: %LINK-3-UPDOWN: Interface FastEthernet0/1, changed state to up",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("*Mar  1 00:02:11.003"),
                mnemonic: Some(("LINK", Some(3), "UPDOWN")),
                sequence: Some(45),
                message: "Interface FastEthernet0/1, changed state to up",
                ..Expected::default()
            },
        );
        // `service sequence-numbers` adds a second counter.
        check(
            "<189>46: 000123: *Mar  1 00:02:12.003: %LINEPROTO-5-UPDOWN: Line protocol on Interface FastEthernet0/1, changed state to up",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("*Mar  1 00:02:12.003"),
                mnemonic: Some(("LINEPROTO", Some(5), "UPDOWN")),
                sequence: Some(46),
                message: "Line protocol on Interface FastEthernet0/1, changed state to up",
                ..Expected::default()
            },
        );
        // `service timestamps log datetime msec localtime show-timezone year`.
        check(
            "<189>123: Nov 13 2026 10:15:01.123 EET: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.2)",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 2026 10:15:01.123 EET"),
                mnemonic: Some(("SYS", Some(5), "CONFIG_I")),
                sequence: Some(123),
                message: "Configured from console by admin on vty0 (10.0.0.2)",
                ..Expected::default()
            },
        );
        // A zone name of the administrator's own making.
        check(
            "<189>7: Nov 13 10:15:01.123 Baghd: %SYS-5-CONFIG_I: Configured from console by console",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 10:15:01.123 Baghd"),
                mnemonic: Some(("SYS", Some(5), "CONFIG_I")),
                sequence: Some(7),
                message: "Configured from console by console",
                ..Expected::default()
            },
        );
        // `logging origin-id hostname`: the name comes before the timestamp.
        check(
            "<189>124: R1: Nov 13 10:15:02.456: %SEC_LOGIN-5-LOGIN_SUCCESS: Login Success [user: admin] [Source: 10.0.0.2] [localport: 22] at 10:15:02 EET Fri Nov 13 2026",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 10:15:02.456"),
                hostname: Some("R1"),
                mnemonic: Some(("SEC_LOGIN", Some(5), "LOGIN_SUCCESS")),
                sequence: Some(124),
                message: "Login Success [user: admin] [Source: 10.0.0.2] [localport: 22] at 10:15:02 EET Fri Nov 13 2026",
                ..Expected::default()
            },
        );
        check(
            "<189>core-sw1.example.net: *Mar  1 00:02:11.003: %LINK-3-UPDOWN: Interface Gi1/0/1, changed state to down",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("*Mar  1 00:02:11.003"),
                hostname: Some("core-sw1.example.net"),
                mnemonic: Some(("LINK", Some(3), "UPDOWN")),
                message: "Interface Gi1/0/1, changed state to down",
                ..Expected::default()
            },
        );
        // A leading dot: the clock is not synchronised.
        check(
            "<189>125: .Nov 13 10:15:03: %OSPF-5-ADJCHG: Process 1, Nbr 10.0.0.2 on GigabitEthernet0/1 from LOADING to FULL, Loading Done",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some(".Nov 13 10:15:03"),
                mnemonic: Some(("OSPF", Some(5), "ADJCHG")),
                sequence: Some(125),
                message: "Process 1, Nbr 10.0.0.2 on GigabitEthernet0/1 from LOADING to FULL, Loading Done",
                ..Expected::default()
            },
        );
        // Debug output has no mnemonic.
        check(
            "<191>126: *Mar  1 00:03:11.003 UTC: OSPF: Rcv hello from 10.0.0.2 area 0 from FastEthernet0/1 10.0.0.2",
            Expected {
                priority: Some((23, 7)),
                timestamp: Some("*Mar  1 00:03:11.003 UTC"),
                sequence: Some(126),
                message: "OSPF: Rcv hello from 10.0.0.2 area 0 from FastEthernet0/1 10.0.0.2",
                ..Expected::default()
            },
        );
        // `no service timestamps log` and `no logging message-counter syslog`.
        check(
            "<187>%LINK-3-UPDOWN: Interface GigabitEthernet1/0/1, changed state to down",
            Expected {
                priority: Some((23, 3)),
                mnemonic: Some(("LINK", Some(3), "UPDOWN")),
                message: "Interface GigabitEthernet1/0/1, changed state to down",
                ..Expected::default()
            },
        );
        check(
            "<189>47: %SYS-5-RESTART: System restarted --",
            Expected {
                priority: Some((23, 5)),
                mnemonic: Some(("SYS", Some(5), "RESTART")),
                sequence: Some(47),
                message: "System restarted --",
                ..Expected::default()
            },
        );
        // Mnemonics with digits and underscores, and text containing colons.
        check(
            "<188>9: Nov 13 10:15:01: %C4K_EBM-4-HOSTFLAPPING: Host 00:1A:2B:3C:4D:5E in vlan 10 is flapping between port Gi1/1 and port Gi1/2",
            Expected {
                priority: Some((23, 4)),
                timestamp: Some("Nov 13 10:15:01"),
                mnemonic: Some(("C4K_EBM", Some(4), "HOSTFLAPPING")),
                sequence: Some(9),
                message: "Host 00:1A:2B:3C:4D:5E in vlan 10 is flapping between port Gi1/1 and port Gi1/2",
                ..Expected::default()
            },
        );
        // IOS-XE in RFC 5424 mode keeps the mnemonic in the text.
        check(
            "<189>1 2026-11-13T10:15:01.123Z sw1 - - - - 48: %LINK-3-UPDOWN: Interface Gi1/0/2, changed state to up",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("2026-11-13T10:15:01.123Z"),
                hostname: Some("sw1"),
                mnemonic: Some(("LINK", Some(3), "UPDOWN")),
                sequence: Some(48),
                message: "Interface Gi1/0/2, changed state to up",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn cisco_ios_xr_nx_os_and_asa() {
        check(
            "<189>123: RP/0/RSP0/CPU0:Nov 13 10:15:01.123 UTC: ifmgr[402]: %PKT_INFRA-LINK-3-UPDOWN : Interface GigabitEthernet0/0/0/1, changed state to Down",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 10:15:01.123 UTC"),
                app: Some("ifmgr"),
                procid: Some("402"),
                mnemonic: Some(("PKT_INFRA-LINK", Some(3), "UPDOWN")),
                sequence: Some(123),
                message: "Interface GigabitEthernet0/0/0/1, changed state to Down",
                ..Expected::default()
            },
        );
        check(
            "<189>124: xr1 RP/0/RSP0/CPU0:Nov 13 10:15:02.456 UTC: bgp[1052]: %ROUTING-BGP-5-ADJCHANGE : neighbor 10.0.0.2 Down - BGP Notification sent (VRF: default)",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 10:15:02.456 UTC"),
                hostname: Some("xr1"),
                app: Some("bgp"),
                procid: Some("1052"),
                mnemonic: Some(("ROUTING-BGP", Some(5), "ADJCHANGE")),
                sequence: Some(124),
                message: "neighbor 10.0.0.2 Down - BGP Notification sent (VRF: default)",
                ..Expected::default()
            },
        );
        check(
            "<189>: 2026 Nov 13 10:15:01 UTC: %ETHPORT-5-IF_UP: Interface Ethernet1/1 is up in mode trunk",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("2026 Nov 13 10:15:01 UTC"),
                mnemonic: Some(("ETHPORT", Some(5), "IF_UP")),
                message: "Interface Ethernet1/1 is up in mode trunk",
                ..Expected::default()
            },
        );
        check(
            "<187>2026 Nov 13 10:15:09 UTC nxos1 %ETHPORT-3-IF_DOWN_LINK_FAILURE: Interface Ethernet1/2 is down (Link failure)",
            Expected {
                priority: Some((23, 3)),
                timestamp: Some("2026 Nov 13 10:15:09 UTC"),
                hostname: Some("nxos1"),
                mnemonic: Some(("ETHPORT", Some(3), "IF_DOWN_LINK_FAILURE")),
                message: "Interface Ethernet1/2 is down (Link failure)",
                ..Expected::default()
            },
        );
        check(
            "<166>Nov 13 2026 10:15:01: %ASA-6-302013: Built outbound TCP connection 12345 for outside:203.0.113.5/443 (203.0.113.5/443) to inside:10.0.0.50/51234 (192.0.2.10/51234)",
            Expected {
                priority: Some((20, 6)),
                timestamp: Some("Nov 13 2026 10:15:01"),
                mnemonic: Some(("ASA", Some(6), "302013")),
                message: "Built outbound TCP connection 12345 for outside:203.0.113.5/443 (203.0.113.5/443) to inside:10.0.0.50/51234 (192.0.2.10/51234)",
                ..Expected::default()
            },
        );
        // `logging device-id hostname`.
        check(
            "<166>Nov 13 2026 10:15:01 asa1 : %ASA-6-302014: Teardown TCP connection 12345 for outside:203.0.113.5/443 to inside:10.0.0.50/51234 duration 0:00:30 bytes 4096 TCP FINs",
            Expected {
                priority: Some((20, 6)),
                timestamp: Some("Nov 13 2026 10:15:01"),
                hostname: Some("asa1"),
                mnemonic: Some(("ASA", Some(6), "302014")),
                message: "Teardown TCP connection 12345 for outside:203.0.113.5/443 to inside:10.0.0.50/51234 duration 0:00:30 bytes 4096 TCP FINs",
                ..Expected::default()
            },
        );
        check(
            "<164>%ASA-4-106023: Deny tcp src outside:198.51.100.7/4444 dst inside:10.0.0.5/22 by access-group \"outside_in\" [0x0, 0x0]",
            Expected {
                priority: Some((20, 4)),
                mnemonic: Some(("ASA", Some(4), "106023")),
                message: "Deny tcp src outside:198.51.100.7/4444 dst inside:10.0.0.5/22 by access-group \"outside_in\" [0x0, 0x0]",
                ..Expected::default()
            },
        );
        // EMBLEM format.
        check(
            "<166>:Nov 13 10:15:01 UTC: %ASA-session-6-302013: Built outbound TCP connection 9 for outside:203.0.113.5/443",
            Expected {
                priority: Some((20, 6)),
                timestamp: Some("Nov 13 10:15:01 UTC"),
                mnemonic: Some(("ASA-session", Some(6), "302013")),
                message: "Built outbound TCP connection 9 for outside:203.0.113.5/443",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn junos() {
        check(
            "<30>Nov 13 10:15:01 mx1 mgd[1234]: UI_COMMIT: User 'admin' requested 'commit' operation (comment: none)",
            Expected {
                priority: Some((3, 6)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("mx1"),
                app: Some("mgd"),
                procid: Some("1234"),
                mnemonic: Some(("UI", None, "COMMIT")),
                message: "User 'admin' requested 'commit' operation (comment: none)",
                ..Expected::default()
            },
        );
        check(
            "<28>Nov 13 10:15:05 mx1 rpd[1456]: RPD_BGP_NEIGHBOR_STATE_CHANGED: BGP peer 10.0.0.2 (External AS 65002) changed state from Established to Idle (event HoldTime) (instance master)",
            Expected {
                priority: Some((3, 4)),
                timestamp: Some("Nov 13 10:15:05"),
                hostname: Some("mx1"),
                app: Some("rpd"),
                procid: Some("1456"),
                mnemonic: Some(("RPD", None, "BGP_NEIGHBOR_STATE_CHANGED")),
                message: "BGP peer 10.0.0.2 (External AS 65002) changed state from Established to Idle (event HoldTime) (instance master)",
                ..Expected::default()
            },
        );
        check(
            "<14>Nov 13 10:15:09 mx1 mib2d[1500]: SNMP_TRAP_LINK_DOWN: ifIndex 530, ifAdminStatus up(1), ifOperStatus down(2), ifName ge-0/0/1",
            Expected {
                priority: Some((1, 6)),
                timestamp: Some("Nov 13 10:15:09"),
                hostname: Some("mx1"),
                app: Some("mib2d"),
                procid: Some("1500"),
                mnemonic: Some(("SNMP", None, "TRAP_LINK_DOWN")),
                message: "ifIndex 530, ifAdminStatus up(1), ifOperStatus down(2), ifName ge-0/0/1",
                ..Expected::default()
            },
        );
        // `explicit-priority` adds a Cisco-style prefix.
        check(
            "<29>Nov 13 10:15:01 mx1 mgd[1234]: %INTERACT-5-UI_COMMIT: User 'admin' requested 'commit' operation",
            Expected {
                priority: Some((3, 5)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("mx1"),
                app: Some("mgd"),
                procid: Some("1234"),
                mnemonic: Some(("INTERACT", Some(5), "UI_COMMIT")),
                message: "User 'admin' requested 'commit' operation",
                ..Expected::default()
            },
        );
        // Structured-data mode: the tag is the MSGID.
        check(
            "<165>1 2026-11-13T10:15:01.123+03:00 mx1 mgd 3046 UI_DBASE_LOGOUT_EVENT [junos@2636.1.1.1.2.29 username=\"admin\"] User 'admin' exiting configuration mode",
            Expected {
                priority: Some((20, 5)),
                timestamp: Some("2026-11-13T10:15:01.123+03:00"),
                hostname: Some("mx1"),
                app: Some("mgd"),
                procid: Some("3046"),
                msgid: Some("UI_DBASE_LOGOUT_EVENT"),
                mnemonic: Some(("UI", None, "DBASE_LOGOUT_EVENT")),
                message: "User 'admin' exiting configuration mode",
                ..Expected::default()
            },
        );
        // A daemon message without an event tag.
        check(
            "<27>Nov 13 10:15:11 mx1 sshd[2001]: error: PAM: authentication error for admin from 10.0.0.9",
            Expected {
                priority: Some((3, 3)),
                timestamp: Some("Nov 13 10:15:11"),
                hostname: Some("mx1"),
                app: Some("sshd"),
                procid: Some("2001"),
                message: "error: PAM: authentication error for admin from 10.0.0.9",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn mikrotik_routeros() {
        // With "bsd-syslog=yes".
        check(
            "<134>Nov 13 10:15:01 MikroTik system,info,account user admin logged in from 10.0.0.2 via ssh",
            Expected {
                priority: Some((16, 6)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("MikroTik"),
                mnemonic: Some(("system", Some(6), "account")),
                message: "user admin logged in from 10.0.0.2 via ssh",
                ..Expected::default()
            },
        );
        // Bare: topics and text, with or without a priority.
        check(
            "system,info,account user admin logged in from 10.0.0.2 via ssh",
            Expected {
                mnemonic: Some(("system", Some(6), "account")),
                message: "user admin logged in from 10.0.0.2 via ssh",
                ..Expected::default()
            },
        );
        check(
            "<30>dhcp,info dhcp1 assigned 192.168.88.254 for AA:BB:CC:DD:EE:FF host1",
            Expected {
                priority: Some((3, 6)),
                mnemonic: Some(("dhcp", Some(6), "")),
                message: "dhcp1 assigned 192.168.88.254 for AA:BB:CC:DD:EE:FF host1",
                ..Expected::default()
            },
        );
        check(
            "<134>Nov 13 10:15:07 edge-rtr interface,info ether2 link down",
            Expected {
                priority: Some((16, 6)),
                timestamp: Some("Nov 13 10:15:07"),
                hostname: Some("edge-rtr"),
                mnemonic: Some(("interface", Some(6), "")),
                message: "ether2 link down",
                ..Expected::default()
            },
        );
        // Several severity topics: the most severe one counts.
        check(
            "system,error,critical login failure for user root from 203.0.113.9 via ssh",
            Expected {
                mnemonic: Some(("system", Some(2), "")),
                message: "login failure for user root from 203.0.113.9 via ssh",
                ..Expected::default()
            },
        );
        check(
            "<132>Nov 13 10:15:09 MikroTik firewall,warning forward: in:bridge out:ether1, connection-state:new src-mac aa:bb:cc:dd:ee:ff, proto TCP (SYN), 192.168.88.10:51234->203.0.113.5:443, len 60",
            Expected {
                priority: Some((16, 4)),
                timestamp: Some("Nov 13 10:15:09"),
                hostname: Some("MikroTik"),
                mnemonic: Some(("firewall", Some(4), "")),
                message: "forward: in:bridge out:ether1, connection-state:new src-mac aa:bb:cc:dd:ee:ff, proto TCP (SYN), 192.168.88.10:51234->203.0.113.5:443, len 60",
                ..Expected::default()
            },
        );
        check(
            "ospf,route,debug installing route 10.9.0.0/16 via 10.0.0.2",
            Expected {
                mnemonic: Some(("ospf", Some(7), "route")),
                message: "installing route 10.9.0.0/16 via 10.0.0.2",
                ..Expected::default()
            },
        );
        // Comma-separated words without a severity topic are just text.
        check(
            "red,green,blue are colours",
            Expected {
                message: "red,green,blue are colours",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn arista_eos() {
        check(
            "<189>Nov 13 10:15:01 arista1 Ebra: %LINEPROTO-5-UPDOWN: Line protocol on Interface Ethernet1, changed state to up",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 10:15:01"),
                hostname: Some("arista1"),
                app: Some("Ebra"),
                mnemonic: Some(("LINEPROTO", Some(5), "UPDOWN")),
                message: "Line protocol on Interface Ethernet1, changed state to up",
                ..Expected::default()
            },
        );
        check(
            "<190>Nov 13 10:15:02 arista1 ConfigAgent: %SYS-5-CONFIG_I: Configured from console by admin on vty3 (10.0.0.2)",
            Expected {
                priority: Some((23, 6)),
                timestamp: Some("Nov 13 10:15:02"),
                hostname: Some("arista1"),
                app: Some("ConfigAgent"),
                mnemonic: Some(("SYS", Some(5), "CONFIG_I")),
                message: "Configured from console by admin on vty3 (10.0.0.2)",
                ..Expected::default()
            },
        );
        check(
            "<189>1 2026-11-13T10:15:01.123456+00:00 arista1 Ebra 1789 - - %LINEPROTO-5-UPDOWN: Line protocol on Interface Ethernet2, changed state to down",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("2026-11-13T10:15:01.123456+00:00"),
                hostname: Some("arista1"),
                app: Some("Ebra"),
                procid: Some("1789"),
                mnemonic: Some(("LINEPROTO", Some(5), "UPDOWN")),
                message: "Line protocol on Interface Ethernet2, changed state to down",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn huawei_vrp_and_h3c_comware() {
        check(
            "<188>Nov 13 2026 10:15:01 HUAWEI %%01IFNET/4/LINK_STATE(l)[5]:The line protocol IP on the interface GigabitEthernet0/0/1 has entered the UP state.",
            Expected {
                priority: Some((23, 4)),
                timestamp: Some("Nov 13 2026 10:15:01"),
                hostname: Some("HUAWEI"),
                mnemonic: Some(("IFNET", Some(4), "LINK_STATE")),
                sequence: Some(5),
                message: "The line protocol IP on the interface GigabitEthernet0/0/1 has entered the UP state.",
                ..Expected::default()
            },
        );
        check(
            "<189>Nov 13 2026 10:15:07+08:00 CE6850 %%01SHELL/5/CMDRECORD(s)[12]:Recorded command information. (Task=VT0, Ip=10.0.0.2, User=admin, Command=\"display current-configuration\")",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 2026 10:15:07+08:00"),
                hostname: Some("CE6850"),
                mnemonic: Some(("SHELL", Some(5), "CMDRECORD")),
                sequence: Some(12),
                message: "Recorded command information. (Task=VT0, Ip=10.0.0.2, User=admin, Command=\"display current-configuration\")",
                ..Expected::default()
            },
        );
        check(
            "<189>Nov 13 10:15:01 2026 H3C %%10IFNET/5/LINK_UPDOWN: Line protocol state on the interface GigabitEthernet1/0/1 changed to up.",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("Nov 13 10:15:01 2026"),
                hostname: Some("H3C"),
                mnemonic: Some(("IFNET", Some(5), "LINK_UPDOWN")),
                message: "Line protocol state on the interface GigabitEthernet1/0/1 changed to up.",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn fortigate() {
        check(
            "<189>date=2026-11-13 time=10:15:01 devname=\"FGT60F\" devid=\"FGT60FTK20001234\" eventtime=1794565501123456789 tz=\"+0300\" logid=\"0100032001\" type=\"event\" subtype=\"system\" level=\"information\" vd=\"root\" logdesc=\"Admin login successful\" sn=\"1794565501\" user=\"admin\" ui=\"ssh(10.0.0.2)\" method=\"ssh\" srcip=10.0.0.2 dstip=10.0.0.1 action=\"login\" status=\"success\" reason=\"none\" profile=\"super_admin\" msg=\"Administrator admin logged in successfully from ssh(10.0.0.2)\"",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("2026-11-13 10:15:01"),
                hostname: Some("FGT60F"),
                msgid: Some("0100032001"),
                mnemonic: Some(("event", Some(6), "system")),
                message: "Administrator admin logged in successfully from ssh(10.0.0.2)",
                ..Expected::default()
            },
        );
        // A traffic log has no text of its own: the fields are the message.
        let traffic = "date=2026-11-13 time=10:15:02 devname=\"FGT60F\" logid=\"0000000013\" type=\"traffic\" subtype=\"forward\" level=\"notice\" srcip=10.0.0.50 srcport=51234 dstip=203.0.113.5 dstport=443 action=\"close\" service=\"HTTPS\" sentbyte=1234 rcvdbyte=5678";
        let message = parse_line(&format!("<189>{traffic}"));
        assert_eq!(message.message, traffic);
        assert_eq!(message.hostname.as_deref(), Some("FGT60F"));
        assert_eq!(
            message.mnemonic,
            Some(Mnemonic {
                facility: "traffic".into(),
                severity: Some(5),
                name: "forward".into()
            })
        );
        // Without a priority the level gives the severity; quotes inside a value are unescaped.
        let message = parse_line(
            "date=2026-11-13 time=10:15:03 devname=FGT60F logid=0101037130 type=event subtype=vpn level=error msg=\"IPsec phase 1 error: \\\"no proposal chosen\\\"\"",
        );
        assert_eq!((message.facility, message.severity), (None, Some(3)));
        assert_eq!(
            message.message,
            "IPsec phase 1 error: \"no proposal chosen\""
        );
        // Behind a relay that added a BSD header.
        check(
            "<189>Nov 13 10:15:04 fw-relay date=2026-11-13 time=10:15:04 devname=\"FGT60F\" logid=\"0100020099\" type=\"event\" subtype=\"system\" level=\"warning\" logdesc=\"Interface status changed\"",
            Expected {
                priority: Some((23, 5)),
                timestamp: Some("2026-11-13 10:15:04"),
                hostname: Some("FGT60F"),
                msgid: Some("0100020099"),
                mnemonic: Some(("event", Some(4), "system")),
                message: "Interface status changed",
                ..Expected::default()
            },
        );
    }

    #[test]
    fn input_that_is_not_syslog_becomes_a_plain_message() {
        for line in [
            "just some text without any structure",
            "<999>priority out of range",
            "<abc>not a priority",
            "<13",
            "<>empty priority",
            "GET / HTTP/1.1",
            "\u{1b}[31mcoloured\u{1b}[0m output",
            "1 2 3 4 5 6 7",
            "%%% not a mnemonic %%%",
            "%NOT-A-MNEMONIC: text",
        ] {
            check(
                line,
                Expected {
                    message: line.trim_end(),
                    ..Expected::default()
                },
            );
        }
        check("", Expected::default());
        check(
            "<13>",
            Expected {
                priority: Some((1, 5)),
                ..Expected::default()
            },
        );
        check("\n\r\n\0", Expected::default());
        // Bytes that are not UTF-8 are replaced, not rejected.
        let message = parse(
            b"<13>Nov 13 10:15:01 host app: caf\xe9 \xff\xfe",
            "192.0.2.1:514".parse().unwrap(),
        );
        assert_eq!(message.hostname.as_deref(), Some("host"));
        assert_eq!(message.message, "caf\u{fffd} \u{fffd}\u{fffd}");
        // A trailing newline or NUL (some devices send one) is not part of the message.
        let message = parse(
            b"<13>Nov 13 10:15:01 host app: done\n\0",
            "192.0.2.1:514".parse().unwrap(),
        );
        assert_eq!(message.message, "done");
        assert_eq!(message.raw, "<13>Nov 13 10:15:01 host app: done");
    }

    #[test]
    fn source_is_the_sender_address_for_both_families() {
        let v6 = parse(b"<13>x", "[2001:db8::7]:514".parse().unwrap());
        assert_eq!(v6.source, "2001:db8::7");
        let json = serde_json::to_value(parse_line(
            "<189>45: *Mar  1 00:02:11.003: %LINK-3-UPDOWN: Interface Fa0/1, changed state to up",
        ))
        .unwrap();
        assert_eq!(json["severity"], 5);
        assert_eq!(json["mnemonic"]["facility"], "LINK");
        assert_eq!(json["mnemonic"]["severity"], 3);
        assert_eq!(json["sequence"], 45);
        assert_eq!(json["hostname"], serde_json::Value::Null);
    }

    #[test]
    fn truncated_and_random_input_never_panics() {
        let samples = [
            "<165>1 2003-10-11T22:14:15.003Z mymachine.example.com evntslog - ID47 [exampleSDID@32473 iut=\"3\" eventSource=\"Application\"] \u{feff}An application event",
            "<189>124: R1: Nov 13 2026 10:15:02.456 EET: %SEC_LOGIN-5-LOGIN_SUCCESS: Login Success [user: admin]",
            "<189>123: xr1 RP/0/RSP0/CPU0:Nov 13 10:15:01.123 UTC: ifmgr[402]: %PKT_INFRA-LINK-3-UPDOWN : Interface Gi0/0/0/1",
            "<188>Nov 13 2026 10:15:01+08:00 HUAWEI %%01IFNET/4/LINK_STATE(l)[5]:The line protocol",
            "<189>date=2026-11-13 time=10:15:01 devname=\"FGT60F\" logid=\"0100032001\" type=\"event\" msg=\"größer \\\" als\"",
            "<134>Nov 13 10:15:01 MikroTik système,info,account user admin logged in",
            "<30>Nov 13 10:15:01 mx1 mgd[1234]: UI_COMMIT: User 'admin' requested 'commit'",
        ];
        let source: SocketAddr = "192.0.2.1:514".parse().unwrap();
        for sample in samples {
            let bytes = sample.as_bytes();
            // Every prefix, including ones that end inside a multi-byte character.
            for cut in 0..=bytes.len() {
                let message = parse(&bytes[..cut], source);
                assert!(message.raw.len() <= cut + 3);
            }
            // Every single-byte corruption.
            for index in 0..bytes.len() {
                for replacement in [
                    0u8, b'<', b'>', b'[', b']', b'"', b'\\', b':', b' ', b'%', 0xff,
                ] {
                    let mut corrupted = bytes.to_vec();
                    corrupted[index] = replacement;
                    let _ = parse(&corrupted, source);
                }
            }
        }
        let mut state = 0xfeed_face_dead_beefu64;
        for round in 0..100_000u32 {
            let length = (state % 200) as usize;
            let mut junk = Vec::with_capacity(length);
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Half random bytes, half characters the parser gives meaning to.
                const MEANINGFUL: &[u8] = b"<>1234567890 :-[]\"\\%/,.=TZNovJan*abcXYZ\n";
                let byte = if round % 2 == 0 {
                    state as u8
                } else {
                    MEANINGFUL[(state % MEANINGFUL.len() as u64) as usize]
                };
                junk.push(byte);
            }
            let message = parse(&junk, source);
            assert!(message.message.len() <= message.raw.len() + 3);
        }
    }
}
