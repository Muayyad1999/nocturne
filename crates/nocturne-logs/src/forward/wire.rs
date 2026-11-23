//! A stored entry as it goes on the wire: an RFC 5424 message, a JSON
//! document, or that document inside an HTTP event collector's envelope.
//!
//! Nothing here adds to what is stored. The device's name is looked up
//! because a collector files a line under a name, not under an inventory id;
//! everything else is the row as `logs.query` returns it.

use chrono::{DateTime, SecondsFormat};
use nocturne_app::store::LogEntry;
use nocturne_netprobe::syslog::format::{Rfc5424, SdElement};
use serde_json::{Map, Value, json};

/// The most one datagram carries. A stored line is rarely a tenth of this;
/// what is larger travels as a handful of IP fragments at most, and a JSON
/// document cut by a receiver's own buffer is not JSON any more.
pub(crate) const DATAGRAM_LIMIT: usize = 8_192;
/// The most one message carries on a stream or in a request.
pub(crate) const STREAM_LIMIT: usize = 256 * 1024;

/// RFC 5424 has no way to say "no severity was given". RFC 3164 §4.3.3 has
/// a relay write such a message as `user.notice`, so that is what a line
/// that came without a priority is sent as. The JSON format keeps `null`.
const FACILITY_USER: u8 = 1;
const SEVERITY_NOTICE: u8 = 5;
/// What Nocturne says in its own voice: `daemon` for what it recorded
/// (events, and the text it wrote for a trap), `log audit` for the audit
/// trail, which is what facility 13 is for.
const FACILITY_DAEMON: u8 = 3;
const FACILITY_AUDIT: u8 = 13;
const APP: &str = "nocturne";

fn body_text<'a>(entry: &'a LogEntry, key: &str) -> Option<&'a str> {
    entry
        .body
        .as_ref()?
        .get(key)?
        .as_str()
        .filter(|text| !text.trim().is_empty())
}

/// What a collector should file the entry under: the device it is about by
/// its inventory name; for a line no device claims, the name the line gives
/// itself and then the address it came from. `None` when it is about no
/// device at all — a collector then files it under the machine that sent
/// it, which is this one.
pub(crate) fn host<'a>(entry: &'a LogEntry, device: Option<&'a str>) -> Option<&'a str> {
    let named = device.filter(|name| !name.trim().is_empty());
    let claimed = (entry.stream == crate::db::STREAM)
        .then(|| body_text(entry, "host"))
        .flatten();
    let address = is_device_stream(entry)
        .then_some(entry.source.as_str())
        .filter(|source| !source.trim().is_empty());
    named.or(claimed).or(address)
}

fn is_device_stream(entry: &LogEntry) -> bool {
    crate::db::DEVICE_STREAMS.contains(&entry.stream.as_str())
}

/// The entry as one RFC 5424 message, at most `limit` bytes.
///
/// `HOSTNAME` is [`host`]; `APP-NAME` says who wrote the text — the
/// device's own process for a syslog line, `nocturne` for everything
/// Nocturne wrote itself; `MSGID` is the kind of message (a vendor
/// mnemonic, a trap's name, an audit action); `MSG` is the stored message.
/// The only structured data is the IANA-registered `origin`: the address a
/// device's line came from, or Nocturne's name and version on its own.
pub(crate) fn rfc5424(entry: &LogEntry, device: Option<&str>, limit: usize) -> Vec<u8> {
    let own = !is_device_stream(entry);
    let syslog = entry.stream == crate::db::STREAM;
    let facility = if syslog {
        entry
            .body
            .as_ref()
            .and_then(|body| body.get("facility"))
            .and_then(Value::as_u64)
            .and_then(|facility| u8::try_from(facility).ok())
            .unwrap_or(FACILITY_USER)
    } else if entry.stream == crate::AUDIT_STREAM {
        FACILITY_AUDIT
    } else {
        FACILITY_DAEMON
    };
    let msgid = if syslog {
        body_text(entry, "mnemonic").or_else(|| body_text(entry, "msgid"))
    } else if own {
        Some(entry.source.as_str()).filter(|source| !source.is_empty())
    } else {
        body_text(entry, "trap")
    };
    let address = entry
        .source
        .parse::<std::net::IpAddr>()
        .is_ok()
        .then_some(entry.source.as_str());
    let origin: Vec<(&str, &str)> = if own {
        vec![
            ("software", "Nocturne"),
            ("swVersion", env!("CARGO_PKG_VERSION")),
        ]
    } else {
        address.map(|ip| ("ip", ip)).into_iter().collect()
    };
    let structured = [SdElement {
        id: "origin",
        params: &origin,
    }];
    Rfc5424 {
        facility,
        severity: entry.severity.unwrap_or(SEVERITY_NOTICE),
        timestamp_ms: Some(entry.ts),
        hostname: host(entry, device),
        app: if syslog {
            body_text(entry, "app")
        } else {
            Some(APP)
        },
        procid: syslog.then(|| body_text(entry, "procid")).flatten(),
        msgid,
        structured: if origin.is_empty() {
            &[]
        } else {
            &structured[..]
        },
        message: &entry.message,
    }
    .encode(limit)
}

/// The entry as `logs.query` returns it, with the device's name and the
/// time spelled out beside the two fields a collector cannot resolve itself.
fn document(entry: &LogEntry, device: Option<&str>) -> Map<String, Value> {
    let mut document = Map::new();
    document.insert("id".into(), json!(entry.id));
    document.insert("ts".into(), json!(entry.ts));
    document.insert(
        "time".into(),
        DateTime::from_timestamp_millis(entry.ts)
            .map(|at| json!(at.to_rfc3339_opts(SecondsFormat::Millis, true)))
            .unwrap_or(Value::Null),
    );
    document.insert("stream".into(), json!(entry.stream));
    document.insert("device_id".into(), json!(entry.device_id));
    document.insert("device".into(), json!(device));
    document.insert("severity".into(), json!(entry.severity));
    document.insert("source".into(), json!(entry.source));
    document.insert("message".into(), json!(entry.message));
    document.insert("body".into(), entry.body.clone().unwrap_or(Value::Null));
    document
}

/// Serialise `wrap(document)`, making the document smaller until it fits.
///
/// A document that does not fit is sent without its body and says so
/// (`truncated: true`); if that is still too much the message is cut at its
/// end. It is always valid JSON: half a document is worse than a short one.
fn fitted(
    entry: &LogEntry,
    device: Option<&str>,
    limit: usize,
    wrap: impl Fn(Map<String, Value>) -> Value,
) -> Vec<u8> {
    let write = |document: &Map<String, Value>| {
        serde_json::to_vec(&wrap(document.clone())).unwrap_or_default()
    };
    let mut document = document(entry, device);
    let mut bytes = write(&document);
    if bytes.len() <= limit {
        return bytes;
    }
    document.insert("body".into(), Value::Null);
    document.insert("truncated".into(), json!(true));
    bytes = write(&document);
    if bytes.len() <= limit {
        return bytes;
    }
    // The longest beginning of the message that still fits. Searched for
    // rather than computed: what a character costs depends on how JSON
    // writes it. If not even an empty message fits, the document with an
    // empty message is what is sent; no limit in use is that small.
    let message = entry.message.as_str();
    document.insert("message".into(), json!(""));
    let mut best = write(&document);
    let (mut fits, mut too_long) = (0, message.len());
    while too_long - fits > 1 {
        let mut middle = (fits + too_long) / 2;
        while !message.is_char_boundary(middle) {
            middle += 1;
        }
        if middle >= too_long {
            break;
        }
        document.insert("message".into(), json!(&message[..middle]));
        let candidate = write(&document);
        if candidate.len() <= limit {
            best = candidate;
            fits = middle;
        } else {
            too_long = middle;
        }
    }
    best
}

/// The entry as one JSON document on one line.
pub(crate) fn json(entry: &LogEntry, device: Option<&str>, limit: usize) -> Vec<u8> {
    fitted(entry, device, limit, Value::Object)
}

/// The entry in the envelope an HTTP event collector takes: `time` in
/// seconds, `host` when there is one to name (left out otherwise, so the
/// collector's own default applies rather than a made-up host), and the
/// document as `event`.
pub(crate) fn hec(entry: &LogEntry, device: Option<&str>, limit: usize) -> Vec<u8> {
    let host = host(entry, device).map(str::to_string);
    let sourcetype = format!("nocturne:{}", entry.stream);
    let time = entry.ts as f64 / 1_000.0;
    fitted(entry, device, limit, move |document| {
        let mut envelope = Map::new();
        envelope.insert("time".into(), json!(time));
        if let Some(host) = &host {
            envelope.insert("host".into(), json!(host));
        }
        envelope.insert("source".into(), json!(APP));
        envelope.insert("sourcetype".into(), json!(sourcetype));
        envelope.insert("event".into(), Value::Object(document));
        Value::Object(envelope)
    })
}

/// What `logs.forward.test` sends. It is not a stored row and says so: a
/// collector must not take it for something a device reported.
pub(crate) fn test_entry(now: i64, forwarder: &str) -> LogEntry {
    LogEntry {
        id: 0,
        ts: now,
        stream: "event".into(),
        device_id: None,
        severity: Some(6),
        source: "logs.forward.test".into(),
        message: format!(
            "Nocturne test event for the log forwarder `{forwarder}`. This is a test: no device sent it and nothing is wrong."
        ),
        body: Some(json!({"test": true})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("UTF-8")
    }

    fn syslog_line() -> LogEntry {
        LogEntry {
            id: 42,
            ts: 1_795_388_208_123,
            stream: "syslog".into(),
            device_id: Some("dev1".into()),
            severity: Some(3),
            source: "192.0.2.7".into(),
            message: "Interface GigabitEthernet0/1, changed state to down".into(),
            body: Some(json!({
                "raw": "<187>81: *Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet0/1, changed state to down",
                "via": "syslog",
                "facility": 23,
                "mnemonic": "LINK-3-UPDOWN",
                "host": "sw1-as-it-calls-itself",
            })),
        }
    }

    #[test]
    fn a_device_line_is_filed_under_the_device_and_says_where_it_came_from() {
        assert_eq!(
            text(rfc5424(&syslog_line(), Some("SW1"), STREAM_LIMIT)),
            "<187>1 2026-11-22T22:56:48.123Z SW1 - - LINK-3-UPDOWN [origin ip=\"192.0.2.7\"] \
             Interface GigabitEthernet0/1, changed state to down"
        );
    }

    #[test]
    fn a_line_no_device_claims_keeps_the_name_it_gave_itself_and_then_its_address() {
        let mut line = syslog_line();
        line.device_id = None;
        assert!(
            text(rfc5424(&line, None, STREAM_LIMIT))
                .starts_with("<187>1 2026-11-22T22:56:48.123Z sw1-as-it-calls-itself - "),
        );
        // Nothing the line says about itself: the address, and facility
        // `user` in front of the severity it does have.
        line.body = Some(json!({"raw": "x"}));
        assert!(
            text(rfc5424(&line, None, STREAM_LIMIT))
                .starts_with("<11>1 2026-11-22T22:56:48.123Z 192.0.2.7 - "),
            "{}",
            text(rfc5424(&line, None, STREAM_LIMIT))
        );
    }

    #[test]
    fn a_line_without_a_severity_is_sent_as_a_relay_sends_one_without_a_priority() {
        let mut line = syslog_line();
        line.severity = None;
        line.body = Some(json!({"raw": "x"}));
        assert!(text(rfc5424(&line, Some("SW1"), STREAM_LIMIT)).starts_with("<13>1 "));
        // …and the JSON keeps what is true: there was none.
        let document: Value =
            serde_json::from_slice(&json(&line, Some("SW1"), STREAM_LIMIT)).unwrap();
        assert_eq!(document["severity"], Value::Null);
    }

    #[test]
    fn what_nocturne_wrote_itself_says_so_and_the_audit_trail_has_its_own_facility() {
        let audit = LogEntry {
            id: 7,
            ts: 1_795_388_208_123,
            stream: "audit".into(),
            device_id: None,
            severity: Some(6),
            source: "logs.forward.save".into(),
            message: "Saved the log forwarder `SIEM`".into(),
            body: Some(json!({"id": "f1"})),
        };
        assert_eq!(
            text(rfc5424(&audit, None, STREAM_LIMIT)),
            format!(
                "<110>1 2026-11-22T22:56:48.123Z - nocturne - logs.forward.save \
                 [origin software=\"Nocturne\" swVersion=\"{}\"] Saved the log forwarder `SIEM`",
                env!("CARGO_PKG_VERSION")
            )
        );
        let trap = LogEntry {
            id: 8,
            ts: 1_795_388_208_123,
            stream: "trap".into(),
            device_id: Some("dev1".into()),
            severity: None,
            source: "192.0.2.7".into(),
            message: "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)".into(),
            body: Some(json!({"version": "v2c", "trap": "linkDown"})),
        };
        assert_eq!(
            text(rfc5424(&trap, Some("SW1"), STREAM_LIMIT)),
            "<29>1 2026-11-22T22:56:48.123Z SW1 nocturne - linkDown [origin ip=\"192.0.2.7\"] \
             linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)"
        );
    }

    #[test]
    fn the_json_document_is_the_stored_row_with_the_device_named() {
        let document: Value =
            serde_json::from_slice(&json(&syslog_line(), Some("SW1"), STREAM_LIMIT)).unwrap();
        assert_eq!(
            document,
            json!({
                "id": 42,
                "ts": 1_795_388_208_123_i64,
                "time": "2026-11-22T22:56:48.123Z",
                "stream": "syslog",
                "device_id": "dev1",
                "device": "SW1",
                "severity": 3,
                "source": "192.0.2.7",
                "message": "Interface GigabitEthernet0/1, changed state to down",
                "body": syslog_line().body,
            })
        );
    }

    #[test]
    fn a_document_is_one_line_whatever_the_message_holds() {
        let mut line = syslog_line();
        line.message = "first\nsecond\r\n{\"forged\": true}".into();
        let written = json(&line, None, STREAM_LIMIT);
        assert!(!written.contains(&b'\n') && !written.contains(&b'\r'));
    }

    #[test]
    fn the_collector_envelope_carries_seconds_a_host_and_the_document() {
        let envelope: Value =
            serde_json::from_slice(&hec(&syslog_line(), Some("SW1"), STREAM_LIMIT)).unwrap();
        assert_eq!(envelope["time"], json!(1_795_388_208.123));
        assert_eq!(envelope["host"], json!("SW1"));
        assert_eq!(envelope["source"], json!("nocturne"));
        assert_eq!(envelope["sourcetype"], json!("nocturne:syslog"));
        assert_eq!(envelope["event"]["id"], json!(42));

        // About no device: no host is made up for it.
        let own = test_entry(1_795_388_208_123, "SIEM");
        let envelope: Value = serde_json::from_slice(&hec(&own, None, STREAM_LIMIT)).unwrap();
        assert!(envelope.get("host").is_none(), "{envelope}");
        assert_eq!(envelope["event"]["body"], json!({"test": true}));
    }

    #[test]
    fn a_document_too_large_for_a_datagram_loses_its_body_then_its_tail_and_stays_json() {
        let mut line = syslog_line();
        line.body = Some(json!({"raw": "r".repeat(9_000)}));
        let written = json(&line, Some("SW1"), DATAGRAM_LIMIT);
        assert!(written.len() <= DATAGRAM_LIMIT);
        let document: Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(document["body"], Value::Null);
        assert_eq!(document["truncated"], json!(true));
        assert_eq!(document["message"], json!(line.message));

        // A message that is itself too long, and full of what JSON escapes.
        line.message = "é\"\n".repeat(5_000);
        for limit in [DATAGRAM_LIMIT, 600] {
            let written = hec(&line, Some("SW1"), limit);
            assert!(written.len() <= limit, "{} > {limit}", written.len());
            let envelope: Value = serde_json::from_slice(&written).unwrap();
            assert_eq!(envelope["event"]["truncated"], json!(true));
            let kept = envelope["event"]["message"].as_str().unwrap();
            assert!(line.message.starts_with(kept) && !kept.is_empty());
        }
    }

    #[test]
    fn the_test_event_says_that_it_is_one() {
        let event = test_entry(1_795_388_208_123, "SIEM");
        assert!(event.message.contains("This is a test"));
        let written = text(rfc5424(&event, None, STREAM_LIMIT));
        assert!(
            written.contains(" nocturne - logs.forward.test ")
                && written.contains("This is a test"),
            "{written}"
        );
    }
}
