//! What a queue may hold in bytes, beside how many entries it may hold.
//!
//! Each receiver hands what it takes off its socket to a writer through a
//! queue of so many entries, and each forwarder takes the rows it sends
//! through another. An entry is at most a datagram, and a UDP datagram can be
//! 64 KiB: a queue bounded in entries alone is one that anybody who can send
//! it packets can make allocate gigabytes. So every such queue also has a
//! [`Budget`] of bytes. What does not fit is dropped and counted where what
//! does not fit in the number of entries is — taking from a budget never
//! waits, so it never holds up a socket or the ingest path.
//!
//! An entry's bytes are given back when it is done with — written, sent, or
//! let go — not when it leaves the queue: the batch a writer has in its hands
//! is memory too, and it is up to five hundred entries.
//!
//! What an entry costs is estimated from what it holds, its own size and the
//! text and bytes it owns, not measured: an allocator rounds up and keeps
//! some for itself. Where the estimate has to guess (the nodes of a JSON
//! object) it guesses high.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nocturne_app::store::LogEntry;
use nocturne_netprobe::syslog::SyslogMessage;
use serde_json::Value;

/// How many bytes the entries of one queue may take up, and how many they
/// take up now.
#[derive(Debug)]
pub struct Budget {
    held: AtomicUsize,
    limit: AtomicUsize,
}

impl Default for Budget {
    /// The receivers' default (`queue_limit_bytes`); a queue with a budget of
    /// its own says so with [`Budget::new`].
    fn default() -> Self {
        Budget {
            held: AtomicUsize::new(0),
            limit: AtomicUsize::new(crate::state::DEFAULT_QUEUE_BYTES),
        }
    }
}

impl Budget {
    pub fn new(limit: usize) -> Arc<Budget> {
        Arc::new(Budget {
            held: AtomicUsize::new(0),
            limit: AtomicUsize::new(limit),
        })
    }

    /// A new limit applies to what is taken from now on; nothing already
    /// held is let go for it.
    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Relaxed);
    }

    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Relaxed)
    }

    /// What the entries that have not been let go of take up now.
    pub fn held(&self) -> usize {
        self.held.load(Ordering::Relaxed)
    }

    /// Room for an entry of `bytes`, or `None`: it does not fit, and is to
    /// be dropped and counted. An entry larger than the whole budget gets in
    /// when nothing else is held, or a budget smaller than one entry would
    /// let nothing through at all.
    pub fn take(self: &Arc<Self>, bytes: usize) -> Option<Held> {
        let mut held = self.held.load(Ordering::Relaxed);
        loop {
            if held != 0 && held.saturating_add(bytes) > self.limit() {
                return None;
            }
            match self.held.compare_exchange_weak(
                held,
                held + bytes,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(Held {
                        budget: self.clone(),
                        bytes,
                    });
                }
                Err(now) => held = now,
            }
        }
    }
}

/// Bytes taken out of a [`Budget`] for one entry. They are given back when
/// this is dropped, whichever way the entry goes: written, sent, refused by
/// the database, or thrown away with a queue that was closed.
#[derive(Debug)]
pub struct Held {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.budget.held.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// What a parsed syslog message takes up: itself and every text it owns.
pub fn of_message(message: &SyslogMessage) -> usize {
    let optional = |text: &Option<String>| text.as_ref().map_or(0, String::capacity);
    size_of::<SyslogMessage>()
        + message.source.capacity()
        + optional(&message.timestamp)
        + optional(&message.hostname)
        + optional(&message.app)
        + optional(&message.procid)
        + optional(&message.msgid)
        + message.message.capacity()
        + message.raw.capacity()
        + message.mnemonic.as_ref().map_or(0, |mnemonic| {
            mnemonic.facility.capacity() + mnemonic.name.capacity()
        })
}

/// What a stored row takes up: itself, its texts and its body.
pub fn of_entry(entry: &LogEntry) -> usize {
    size_of::<LogEntry>()
        + entry.stream.capacity()
        + entry.device_id.as_ref().map_or(0, String::capacity)
        + entry.source.capacity()
        + entry.message.capacity()
        + entry.body.as_ref().map_or(0, of_json)
}

/// What a JSON value takes up on the heap, beyond its own size.
fn of_json(value: &Value) -> usize {
    match value {
        Value::String(text) => text.capacity(),
        Value::Array(items) => {
            items.capacity() * size_of::<Value>() + items.iter().map(of_json).sum::<usize>()
        }
        // An object keeps each entry with its key, its value and a hash, and
        // an index to find it by: counted as half as much again as those.
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                (size_of::<String>() + size_of::<Value>()) * 3 / 2 + key.capacity() + of_json(value)
            })
            .sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_does_not_fit_is_refused_and_what_is_let_go_makes_room() {
        let budget = Budget::new(1_000);
        let first = budget.take(600).expect("room");
        assert!(budget.take(500).is_none(), "600 + 500 is over 1,000");
        let second = budget.take(400).expect("exactly the limit fits");
        assert_eq!(budget.held(), 1_000);
        drop(first);
        assert_eq!(budget.held(), 400);
        assert!(budget.take(600).is_some(), "room again once one was let go");
        drop(second);
        assert_eq!(
            budget.held(),
            0,
            "and the one taken above was let go at once"
        );
    }

    #[test]
    fn an_entry_larger_than_the_budget_gets_in_alone() {
        let budget = Budget::new(100);
        let large = budget.take(5_000).expect("nothing else is held");
        assert!(budget.take(1).is_none(), "and nothing gets in beside it");
        drop(large);
        assert!(budget.take(1).is_some());
    }

    #[test]
    fn a_lower_limit_turns_away_what_comes_next_and_keeps_what_is_there() {
        let budget = Budget::new(1_000);
        let kept = budget.take(800).expect("room");
        budget.set_limit(500);
        assert!(budget.take(1).is_none());
        assert_eq!(budget.held(), 800, "nothing is let go for it");
        drop(kept);
        assert!(budget.take(400).is_some());
    }

    #[test]
    fn a_line_weighs_at_least_what_its_text_does() {
        let line = format!("<189>1: %SYS-5-CONFIG_I: {}", "x".repeat(60_000));
        let message =
            nocturne_netprobe::syslog::parse(line.as_bytes(), "127.0.0.1:514".parse().unwrap());
        // The text is kept twice: as received, and the message part of it.
        assert!(
            of_message(&message) >= 2 * 60_000,
            "{}",
            of_message(&message)
        );

        let entry = LogEntry {
            id: 1,
            ts: 0,
            stream: "syslog".into(),
            device_id: None,
            severity: Some(5),
            source: "127.0.0.1".into(),
            message: "y".repeat(40_000),
            body: Some(json!({"raw": "z".repeat(30_000), "via": "syslog", "facility": 23})),
        };
        assert!(of_entry(&entry) >= 70_000, "{}", of_entry(&entry));
        let small = LogEntry {
            message: "y".into(),
            body: Some(json!({})),
            ..entry
        };
        assert!(of_entry(&small) < 1_000, "{}", of_entry(&small));
    }
}
