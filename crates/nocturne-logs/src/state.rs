//! What this feature keeps in memory: one block per receiver (its sockets,
//! its queue, its counters — and, for the flow collector, everything it has
//! counted, which is nowhere else), the forwarders' queues and counters, the
//! retention counters, the attribution index, the live-tail buffers and what
//! the rules remember.
//!
//! The counters are deliberately not persisted. "Received 4.2 million lines"
//! is only meaningful next to "since when", and the only honest answer after
//! a restart is zero: what is stored is in the database, what was dropped
//! before the restart is gone and saying otherwise would be inventing a
//! number.
//!
//! **A new receiver** adds its own block beside [`SyslogState`] — a slot for
//! whatever holds its socket, a [`ReceiverCounters`], and a bounded queue if
//! it writes through one — and a field for it in [`LogsState`]. Nothing is
//! shared between receivers except what comes after a row exists: the batch
//! writer, the tail and the rules (`ingest::commit`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use nocturne_ai::redact::Redactor;
use nocturne_netprobe::syslog::{SyslogMessage, SyslogServer};
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc::Sender;

use crate::attribute::Index;
use crate::budget::{Budget, Held};
use crate::model::{ReceiverStatus, TrapStatus};
use crate::rules::track::Memory;

/// How many lines may wait to be written before the receiver starts dropping
/// (the default of the `queue_limit` setting).
///
/// A line is a parsed message of a few hundred bytes, and this is roughly ten
/// seconds of a very chattery campus. Beyond it the honest thing is to drop
/// and say so: growing the queue only moves the failure to the moment the
/// machine runs out of memory.
pub const DEFAULT_QUEUE_LIMIT: usize = 50_000;
/// And how many bytes they may take up (the default of `queue_limit_bytes`).
/// A line can also be a whole 64 KiB datagram, and then fifty thousand of
/// them would be gigabytes; this is what lets the number of lines decide
/// for ordinary traffic and the bytes for datagrams made large.
pub const DEFAULT_QUEUE_BYTES: usize = 64 * 1024 * 1024;
/// Rows written in one transaction. A batch is formed from whatever is
/// already waiting, so a quiet network's single line is written at once and
/// no timer is needed.
pub const BATCH_ROWS: usize = 500;
/// At most four `logs.appended` events a second.
pub const TAIL_INTERVAL_MS: u64 = 250;
/// At most this many entries in one of them.
pub const TAIL_MAX_ENTRIES: usize = 200;

/// What one receiver has taken in, kept and thrown away since the process
/// started. Each receiver has its own, so `logs.status` can say which one a
/// number belongs to.
#[derive(Debug, Default)]
pub struct ReceiverCounters {
    pub received: AtomicU64,
    pub stored: AtomicU64,
    pub dropped: AtomicU64,
    pub last_at: AtomicI64,
}

fn at(value: &AtomicI64) -> Option<i64> {
    match value.load(Ordering::Relaxed) {
        0 => None,
        at => Some(at),
    }
}

/// What retention has removed since the process started.
#[derive(Debug, Default)]
pub struct RetentionCounters {
    pub pruned_by_age: AtomicU64,
    pub pruned_by_rows: AtomicU64,
    pub last_prune_at: AtomicI64,
}

impl RetentionCounters {
    pub fn last_prune_at(&self) -> Option<i64> {
        at(&self.last_prune_at)
    }
}

impl ReceiverCounters {
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

    pub fn last_at(&self) -> Option<i64> {
        at(&self.last_at)
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

/// The running syslog receiver, or why there is none.
#[derive(Default)]
pub struct SyslogReceiver {
    /// Kept so that the sockets close when it is replaced or dropped.
    pub server: Option<SyslogServer>,
    /// The settings the running receiver was started from, so that applying
    /// the same settings again does not rebind the sockets.
    pub applied: Option<crate::settings::SyslogSettings>,
    pub listening: Vec<String>,
    pub error: Option<String>,
    pub enabled: bool,
    /// Bumped on every (re)start, so that a call to `apply` another call
    /// overtook while it was binding knows to give up. Not a signal to the
    /// writer: a stopped receiver's writer still writes what was received.
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

/// Everything the syslog receiver owns.
#[derive(Default)]
pub struct SyslogState {
    pub receiver: Mutex<SyslogReceiver>,
    /// Where the socket tasks hand their messages to the writer, each with
    /// the bytes it holds of the budget. `None` while no receiver runs.
    pub queue: Mutex<Option<Sender<(SyslogMessage, Held)>>>,
    /// What the lines received and not yet written may take up. One for the
    /// life of the application, so that the lines a stopped receiver's
    /// writer has still to write count with the new receiver's.
    pub budget: Arc<Budget>,
    pub counters: ReceiverCounters,
    pub rate: Mutex<RateWindow>,
    /// Lines handed over by a lab through `logs.ingest`: stored, never
    /// received on a socket, so counted apart from `received`.
    pub from_lab: AtomicU64,
    /// Lab devices whose first hand-over has been written to the audit
    /// trail. One entry per device, not one per batch: the simulator hands
    /// lines over many times a second.
    pub lab_devices: Mutex<HashSet<String>>,
}

impl SyslogState {
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
            queued_bytes: self.budget.held() as u64,
            max_queued_bytes: self.budget.limit() as u64,
            rate_per_minute: self.rate.lock().per_minute(now),
            last_at: self.counters.last_at(),
            from_lab: self.from_lab.load(Ordering::Relaxed),
        }
    }
}

/// What the trap receiver was last asked to listen on. Only this decides
/// whether the socket is rebound; `require_known_community` is applied to
/// the running receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrapBinding {
    pub enabled: bool,
    pub port: u16,
    pub bind: String,
}

/// The running trap receiver, or why there is none.
#[derive(Default)]
pub struct TrapReceiver {
    pub applied: Option<TrapBinding>,
    pub listening: Vec<String>,
    pub error: Option<String>,
    pub enabled: bool,
    pub queue_limit: usize,
    /// Ends the socket task, which is what holds the socket open.
    pub stop: Option<tokio_util::sync::CancellationToken>,
    pub task: Option<tokio::task::JoinHandle<()>>,
}

/// Everything the SNMP trap receiver owns.
///
/// Nothing here holds a community a datagram carried. What the vault said
/// about a device is cached for a few seconds in `known`, inside the type
/// the vault returns it in.
pub struct TrapState {
    pub receiver: Mutex<TrapReceiver>,
    /// One `apply` at a time.
    pub applying: tokio::sync::Mutex<()>,
    /// Where the socket task hands notifications to the writer. `None`
    /// while no receiver runs.
    pub queue: Mutex<Option<Sender<crate::traps::Queued>>>,
    /// What the notifications received and not yet written may take up.
    pub budget: Arc<Budget>,
    pub counters: ReceiverCounters,
    pub rate: Mutex<RateWindow>,
    pub rejected: AtomicU64,
    pub unauthenticated: AtomicU64,
    pub malformed: AtomicU64,
    pub duplicates: AtomicU64,
    pub acknowledged: AtomicU64,
    /// The setting, as the socket task reads it for every datagram.
    pub require_known_community: AtomicBool,
    pub known: Mutex<crate::traps::admit::KnownCache>,
    pub rejections: Mutex<crate::traps::admit::Rejections>,
    pub informs: Mutex<crate::traps::Informs>,
    pub interfaces: Mutex<crate::traps::InterfaceNames>,
}

impl Default for TrapState {
    fn default() -> Self {
        TrapState {
            receiver: Mutex::default(),
            applying: tokio::sync::Mutex::default(),
            queue: Mutex::default(),
            budget: Arc::default(),
            counters: ReceiverCounters::default(),
            rate: Mutex::default(),
            rejected: AtomicU64::default(),
            unauthenticated: AtomicU64::default(),
            malformed: AtomicU64::default(),
            duplicates: AtomicU64::default(),
            acknowledged: AtomicU64::default(),
            // Until the settings are read, the careful answer.
            require_known_community: AtomicBool::new(true),
            known: Mutex::default(),
            rejections: Mutex::default(),
            informs: Mutex::default(),
            interfaces: Mutex::default(),
        }
    }
}

impl TrapState {
    pub fn status(&self, now: i64) -> TrapStatus {
        let receiver = self.receiver.lock();
        TrapStatus {
            enabled: receiver.enabled,
            implemented: true,
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
            queued_bytes: self.budget.held() as u64,
            max_queued_bytes: self.budget.limit() as u64,
            rate_per_minute: self.rate.lock().per_minute(now),
            last_at: self.counters.last_at(),
            rejected: self.rejected.load(Ordering::Relaxed),
            unauthenticated: self.unauthenticated.load(Ordering::Relaxed),
            malformed: self.malformed.load(Ordering::Relaxed),
            duplicates: self.duplicates.load(Ordering::Relaxed),
            acknowledged: self.acknowledged.load(Ordering::Relaxed),
            require_known_community: self.require_known_community.load(Ordering::Relaxed),
            rejections: self.rejections.lock().recent(),
            lab: "A simulated device sends no traps, so nothing counted here came from a lab."
                .to_string(),
        }
    }
}

pub struct LogsState {
    pub index: RwLock<Option<Arc<Index>>>,
    /// The rules, compiled. Dropped whenever one is saved or deleted.
    pub rules: RwLock<Option<Arc<Vec<crate::rules::Compiled>>>>,
    /// What the rules remember between lines: throttle windows, threshold
    /// counts and the alerts they have open.
    pub memory: Mutex<Memory>,
    /// The calls the rules have for the monitoring feature (`alerts.raise`,
    /// `alerts.clear`), waiting to be made in order.
    pub outbox: crate::rules::Outbox,
    pub syslog: SyslogState,
    pub traps: TrapState,
    /// The flow collector: its socket, and the buckets it counts into.
    pub flows: crate::flows::FlowState,
    /// The forwarders that are running, their queues and their counters.
    pub forward: crate::forward::ForwardState,
    pub retention: RetentionCounters,
    /// Entries waiting for the next `logs.appended` event, and how many were
    /// left out because more arrived than an event may carry.
    pub tail: Mutex<(Vec<nocturne_app::store::LogEntry>, u64)>,
    pub follows: Mutex<HashMap<String, Follow>>,
    /// One redactor for the whole feature: the placeholder for a given secret
    /// is then the same in every line it appears in, which is what makes
    /// "this key is on three switches" visible without showing the key.
    pub redactor: Redactor,
}

impl Default for LogsState {
    fn default() -> Self {
        LogsState {
            index: RwLock::new(None),
            rules: RwLock::new(None),
            memory: Mutex::new(Memory::default()),
            outbox: crate::rules::Outbox::default(),
            syslog: SyslogState::default(),
            traps: TrapState::default(),
            flows: crate::flows::FlowState::default(),
            forward: crate::forward::ForwardState::default(),
            retention: RetentionCounters::default(),
            tail: Mutex::new((Vec::new(), 0)),
            follows: Mutex::new(HashMap::new()),
            redactor: crate::guard::redactor(),
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
        let counters = ReceiverCounters::default();
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
