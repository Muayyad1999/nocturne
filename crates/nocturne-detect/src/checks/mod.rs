//! The detections themselves, one module per source they are decided from.
//!
//! Each takes what an event said, reads what it has to compare it with, and
//! returns a [`Finding`](crate::model::Finding) or nothing. None of them
//! records, raises or publishes: that is [`crate::hits`], once, for all.

pub mod audit;
pub mod config_change;
pub mod host_key;
pub mod protected;
pub mod reload;

use std::hash::{DefaultHasher, Hash, Hasher};

use nocturne_app::store::{LogEntry, LogQuery};
use nocturne_app::{App, inventory};
use serde_json::{Map, Value};

/// Drop the fields that have nothing to say, so evidence reads as what was
/// known rather than as a form with blanks.
pub fn compact(fields: Value) -> Value {
    match fields {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, value)| !value.is_null())
                .collect::<Map<String, Value>>(),
        ),
        other => other,
    }
}

/// What a hit calls a device: its name, or its id once it has left the
/// inventory (the hit is still about that device, and says which).
pub fn device_name(app: &App, device_id: &str) -> String {
    inventory::get(app, device_id)
        .map(|device| device.name)
        .unwrap_or_else(|_| format!("a device no longer in the inventory ({device_id})"))
}

/// One stored log entry by id, if it is still there.
pub fn log_entry(app: &App, id: i64) -> Option<LogEntry> {
    app.store()
        .log_query(&LogQuery {
            before_id: Some(id.checked_add(1)?),
            limit: Some(1),
            ..LogQuery::default()
        })
        .ok()?
        .into_iter()
        // The query returns the newest entry below the bound, which is some
        // other entry when this one has been pruned.
        .find(|entry| entry.id == id)
}

/// The sentence a hit carries when it was decided from something anybody on
/// the network could have sent.
pub const UNAUTHENTICATED: &str = "Syslog and SNMP v1/v2c traps are unauthenticated: anything that can reach the receiver could have sent this.";

/// The stored log line behind a signal: how it reached Nocturne, whether
/// that way can be forged, and a fingerprint of its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub stream: Option<String>,
    pub via: Option<String>,
    pub forgeable: bool,
    /// Equal for two lines with the same text, and nothing else: it is how
    /// the same line arriving twice is told from a second line, without the
    /// text itself being kept anywhere. Absent when the line is not stored.
    pub fingerprint: Option<u64>,
}

/// Read the line an event names.
///
/// A line the lab handed over in process (`via: lab`) never crossed a
/// network, and one read back from the device's own buffer over the CLI
/// (`via: pull`) came through a session Nocturne opened itself — the log
/// rules do not run over pulled lines today, so that case is here for the
/// day they do. Everything else arrived on a receiver. Unknown is treated as
/// forgeable: the caution is cheap and its absence is a claim.
pub fn line(app: &App, entry_id: Option<i64>) -> Line {
    let Some(entry) = entry_id.and_then(|id| log_entry(app, id)) else {
        return Line {
            forgeable: true,
            ..Line::default()
        };
    };
    let body = entry.body.as_ref();
    let via = body
        .and_then(|body| body.get("via"))
        .and_then(Value::as_str)
        .map(str::to_string);
    // The line as it was received when the log feature kept that (a
    // device's own sequence number and timestamp are in it), else what was
    // stored as its message.
    let text = body
        .and_then(|body| body.get("raw"))
        .and_then(Value::as_str)
        .unwrap_or(&entry.message);
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    Line {
        forgeable: !matches!(via.as_deref(), Some("pull" | "lab")),
        stream: Some(entry.stream),
        via,
        fingerprint: Some(hasher.finish()),
    }
}

/// Whether a character changes how the text around it is laid out without
/// being seen itself: a control character, or one of the marks, embeddings,
/// overrides and isolates that turn the reading direction around.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// A word a device wrote about itself (a user name, a line), made safe to
/// put in a title: printable, and short. It is whatever the device — or
/// whoever forged the line — chose to say.
pub fn word(value: Option<&str>) -> Option<String> {
    let cleaned: String = value?.chars().filter(|c| !hidden(*c)).take(64).collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_from_a_device_is_kept_short_and_printable() {
        assert_eq!(word(Some("admin")).as_deref(), Some("admin"));
        assert_eq!(word(Some("  ")), None);
        assert_eq!(word(None), None);
        assert_eq!(
            word(Some("ad\u{1b}[31mmin\n")).as_deref(),
            Some("ad[31mmin")
        );
        assert_eq!(word(Some(&"x".repeat(500))).unwrap().len(), 64);
    }

    #[test]
    fn a_word_cannot_turn_the_title_it_is_put_in_around() {
        // A right-to-left override in a user name would make the rest of
        // the title read backwards on the screen and in a notification.
        assert_eq!(
            word(Some("adm\u{202E}in\u{2066}\u{200F}")).as_deref(),
            Some("admin")
        );
    }
}
