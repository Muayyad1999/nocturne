//! What this feature keeps in memory: the receiver, the counters behind
//! `logs.status`, the attribution index, the live-tail buffers and the rule
//! throttles.
//!
//! The counters are deliberately not persisted. "Received 4.2 million lines"
//! is only meaningful next to "since when", and the only honest answer after
//! a restart is zero: what is stored is in the database, what was dropped
//! before the restart is gone and saying otherwise would be inventing a
//! number.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use nocturne_ai::redact::Redactor;
use nocturne_netprobe::syslog::{SyslogMessage, SyslogServer};
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc::Sender;

use crate::attribute::Index;
use crate::model::ReceiverStatus;

/// How many lines may wait to be written before the receiver starts dropping
/// (the default of the `queue_limit` setting).
///
/// Each one is a parsed message of a few hundred bytes, so this is a few tens
/// of megabytes at worst, and it is roughly ten seconds of a very chattery
/// campus. Beyond it the honest thing is to drop and say so: growing the
/// queue only moves the failure to the moment the machine runs out of memory.
pub const DEFAULT_QUEUE_LIMIT: usize = 50_000;
/// Rows written in one transaction. A batch is formed from whatever is
/// already waiting, so a quiet network's single line is written at once and
/// no timer is needed.
pub const BATCH_ROWS: usize = 500;
/// At most four `logs.appended` events a second.
pub const TAIL_INTERVAL_MS: u64 = 250;
/// At most this many entries in one of them.
pub const TAIL_MAX_ENTRIES: usize = 200;

#[derive(Debug, Default)]
pub struct Counters {
    pub received: AtomicU64,
    pub stored: AtomicU64,
    pub dropped: AtomicU64,
    pub last_at: AtomicI64,
    pub pruned_by_age: AtomicU64,
    pub pruned_by_rows: AtomicU64,
    pub last_prune_at: AtomicI64,
}

impl Counters {
    pub fn received(&self, at: i64) {
        self.received.fetch_add(1, Ordering::Relaxed);
        self.last_at.fetch_max(at, Ordering::Relaxed);
    }

    pub fn dropped(&self, count: u64) {
        self.dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub fn stored(&self, count: u64) {
        self.stored.fetch_add(count, Ordering::Relaxed);
    }

    fn at(value: &AtomicI64) -> Option<i64> {
        match value.load(Ordering::Relaxed) {
            0 => None,
            at => Some(at),
        }
    }

    pub fn last_at(&self) -> Option<i64> {
        Counters::at(&self.last_at)
    }

    pub fn last_prune_at(&self) -> Option<i64> {
        Counters::at(&self.last_prune_at)
    }
}

/// Lines received per second over the last minute, for `logs.status`.
#[derive(Debug, Default)]
pub struct RateWindow {
    /// (second since the epoch, count), one per second, oldest first.
    seconds: Vec<(i64, u64)>,
}

impl RateWindow {
    pub fn record(&mut self, at: i64, count: u64) {
        let second = at / 1_000;
        match self.seconds.last_mut() {
            Some((last, total)) if *last == second => *total += count,
            _ => self.seconds.push((second, count)),
        }
        self.trim(at);
    }

    pub fn per_minute(&mut self, now: i64) -> u64 {
        self.trim(now);
        self.seconds.iter().map(|(_, count)| count).sum()
    }

    fn trim(&mut self, now: i64) {
        let oldest = now / 1_000 - 60;
        self.seconds.retain(|(second, _)| *second > oldest);
    }
}

/// The running receiver, or why there is none.
#[derive(Default)]
pub struct Receiver {
    /// Kept so that the sockets close when it is replaced or dropped.
    pub server: Option<SyslogServer>,
    /// The settings the running receiver was started from, so that applying
    /// the same settings again does not rebind the sockets.
    pub applied: Option<crate::settings::SyslogSettings>,
    pub listening: Vec<String>,
    pub error: Option<String>,
    pub enabled: bool,
    /// Bumped on every (re)start so a stale ingest task knows to stop.
    pub generation: u64,
    /// The queue size this receiver was started with.
    pub queue_limit: usize,
}

/// One live tail: a filter a UI asked for, and what has piled up for it.
pub struct Follow {
    pub filter: crate::query::Filter,
    pub pending: Vec<nocturne_app::store::LogEntry>,
    pub dropped: u64,
    pub expires_at: i64,
}

/// What a rule has already said, so a flapping port raises one signal and a
/// count rather than four hundred events.
#[derive(Clone, Copy, Debug, Default)]
pub struct Throttle {
    pub window_started_at: i64,
    pub suppressed: u64,
}

pub struct LogsState {
    pub index: RwLock<Option<Arc<Index>>>,
    /// The rules, compiled. Dropped whenever one is saved or deleted.
    pub rules: RwLock<Option<Arc<Vec<crate::rules::Compiled>>>>,
    pub counters: Counters,
    pub rate: Mutex<RateWindow>,
    pub receiver: Mutex<Receiver>,
    /// Where the socket tasks hand their messages to the writer. `None`
    /// while no receiver runs.
    pub queue: Mutex<Option<Sender<SyslogMessage>>>,
    /// Entries waiting for the next `logs.appended` event, and how many were
    /// left out because more arrived than an event may carry.
    pub tail: Mutex<(Vec<nocturne_app::store::LogEntry>, u64)>,
    pub follows: Mutex<HashMap<String, Follow>>,
    pub throttles: Mutex<HashMap<(String, String), Throttle>>,
    /// One redactor for the whole feature: the placeholder for a given secret
    /// is then the same in every line it appears in, which is what makes
    /// "this key is on three switches" visible without showing the key.
    pub redactor: Redactor,
    /// Whether the receiver follows the settings on its own. Tests that are
    /// not about the supervisor start it themselves.
    pub autostart: bool,
}

impl Default for LogsState {
    fn default() -> Self {
        LogsState {
            index: RwLock::new(None),
            rules: RwLock::new(None),
            counters: Counters::default(),
            rate: Mutex::new(RateWindow::default()),
            receiver: Mutex::new(Receiver::default()),
            queue: Mutex::new(None),
            tail: Mutex::new((Vec::new(), 0)),
            follows: Mutex::new(HashMap::new()),
            throttles: Mutex::new(HashMap::new()),
            redactor: crate::guard::redactor(),
            autostart: true,
        }
    }
}

impl LogsState {
    pub fn status(&self, now: i64) -> ReceiverStatus {
        let receiver = self.receiver.lock();
        ReceiverStatus {
            enabled: receiver.enabled,
            listening: receiver.listening.clone(),
            error: receiver.error.clone(),
            received: self.counters.received.load(Ordering::Relaxed),
            stored: self.counters.stored.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            queued: self
                .queue
                .lock()
                .as_ref()
                .map(|queue| receiver.queue_limit.saturating_sub(queue.capacity()) as u64)
                .unwrap_or_default(),
            rate_per_minute: self.rate.lock().per_minute(now),
            last_at: self.counters.last_at(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_window_only_counts_the_last_minute() {
        let mut rate = RateWindow::default();
        rate.record(10_000, 5);
        rate.record(11_000, 7);
        assert_eq!(rate.per_minute(11_500), 12);
        assert_eq!(rate.per_minute(90_000), 0);
    }

    #[test]
    fn counters_start_at_zero_rather_than_at_a_remembered_number() {
        let counters = Counters::default();
        assert_eq!(counters.last_at(), None);
        counters.received(1_700_000_000_000);
        counters.received(1_699_999_999_000);
        assert_eq!(
            counters.last_at(),
            Some(1_700_000_000_000),
            "a line timestamped earlier does not move the clock backwards"
        );
    }
}
