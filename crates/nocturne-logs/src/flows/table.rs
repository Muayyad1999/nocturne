//! What the flow collector remembers.
//!
//! Per exporter, one bucket a minute; in each bucket, for each of six ways
//! of looking at traffic, the biggest talkers of that minute and how much
//! there was besides them. Nothing else: no flow is kept, so nothing here
//! can be searched for one conversation at one moment, and everything is
//! gone when the application stops.
//!
//! **Every size here has a ceiling**, because a flow export is a UDP
//! datagram anybody who can reach the port can write, with any source
//! address in it and from any address:
//!
//! * the minute that is running holds at most [`LIVE_KEYS`] keys per way of
//!   looking; when a key more arrives, the smaller half is added up into
//!   `other` and let go;
//! * a minute that is over keeps its [`TOP_KEYS`] biggest and adds the rest
//!   to `other`;
//! * a bucket is dropped when it is [`KEEP_MINUTES`] old;
//! * all the finished minutes of all exporters together hold at most
//!   [`MAX_KEYS`] keys, and when they would hold more the oldest minute of
//!   any exporter goes first — the memory then reaches back less than a
//!   day, and every answer says how far;
//! * at most [`MAX_EXPORTERS`] exporters are followed, and one that no
//!   device claims never takes the place of one a device does.
//!
//! What is let go is never guessed at afterwards. A key that was folded
//! into `other` and turns up again starts from nothing, so what a row says
//! about a key is a floor — never more than was seen — and `total` is exact.
//!
//! Time comes in as an argument everywhere. The receiver passes the clock;
//! a test passes whatever minute it wants to be in.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Arc;

use nocturne_netprobe::flow::{FlowBatch, FlowRecord};
use serde::Serialize;

use crate::attribute::Attribution;

/// The width of a bucket.
pub const BUCKET_MS: i64 = 60_000;
/// How long a bucket is kept: a day.
pub const KEEP_MINUTES: i64 = 1_440;
/// The keys a finished minute keeps, per way of looking at it.
pub const TOP_KEYS: usize = 20;
/// The keys the running minute holds, per way of looking at it, before the
/// smaller half is folded into `other`.
pub const LIVE_KEYS: usize = 512;
/// The exporters followed at once.
pub const MAX_EXPORTERS: usize = 256;
/// The keys all finished minutes of all exporters hold together. One costs
/// 64 bytes, so this is about 130 MB at the very worst — twenty routers
/// whose every minute fills every list.
pub const MAX_KEYS: usize = 2_000_000;
/// The sampling rates remembered per bucket and per exporter. An exporter
/// has one, or one per interface; more than this is counted as "and more".
pub const MAX_RATES: usize = 8;

/// Bytes, packets and flow records, as the exporter sent them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub bytes: u64,
    pub packets: u64,
    pub flows: u64,
}

impl Counts {
    fn of(record: &FlowRecord) -> Counts {
        Counts {
            bytes: record.bytes,
            packets: record.packets,
            flows: 1,
        }
    }

    pub fn add(&mut self, other: Counts) {
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.packets = self.packets.saturating_add(other.packets);
        self.flows = self.flows.saturating_add(other.flows);
    }

    pub fn is_zero(&self) -> bool {
        *self == Counts::default()
    }
}

/// A way of looking at traffic: what `flows.top` is asked `by`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dimension {
    Source,
    Destination,
    Conversation,
    Application,
    Interface,
    Dscp,
}

impl Dimension {
    pub const ALL: [Dimension; 6] = [
        Dimension::Source,
        Dimension::Destination,
        Dimension::Conversation,
        Dimension::Application,
        Dimension::Interface,
        Dimension::Dscp,
    ];

    pub fn parse(text: &str) -> Option<Dimension> {
        Dimension::ALL
            .into_iter()
            .find(|dimension| dimension.name() == text)
    }

    pub fn name(self) -> &'static str {
        match self {
            Dimension::Source => "source",
            Dimension::Destination => "destination",
            Dimension::Conversation => "conversation",
            Dimension::Application => "application",
            Dimension::Interface => "interface",
            Dimension::Dscp => "dscp",
        }
    }

    fn slot(self) -> usize {
        self as usize
    }
}

/// What a row of `flows.top` is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// A source or a destination. `None` when the record carried none.
    Address(Option<IpAddr>),
    /// Both directions between two addresses, the lower one first.
    Conversation(Option<IpAddr>, Option<IpAddr>),
    /// The IP protocol and, for the ones that have ports, the port the
    /// service is on (`None` when neither port is one).
    Application(u8, Option<u16>),
    /// An ifIndex of the exporter and whether the traffic came in through
    /// it or left by it. Index 0 is "the exporter did not say".
    Interface(u32, Direction),
    /// The six bits of the ToS / Traffic Class byte that are the marking.
    Dscp(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}

/// The keys one record is counted under, for each way of looking: one, or
/// for the interfaces two (the one it came in on and the one it left by).
fn keys_of(record: &FlowRecord) -> [(Dimension, Key); 7] {
    let src = record.src.parse::<IpAddr>().ok();
    let dst = record.dst.parse::<IpAddr>().ok();
    let (low, high) = if dst < src { (dst, src) } else { (src, dst) };
    [
        (Dimension::Source, Key::Address(src)),
        (Dimension::Destination, Key::Address(dst)),
        (Dimension::Conversation, Key::Conversation(low, high)),
        (
            Dimension::Application,
            Key::Application(
                record.protocol,
                super::label::service_port(record.protocol, record.src_port, record.dst_port),
            ),
        ),
        (
            Dimension::Interface,
            Key::Interface(record.input_if, Direction::In),
        ),
        (
            Dimension::Interface,
            Key::Interface(record.output_if, Direction::Out),
        ),
        (Dimension::Dscp, Key::Dscp(record.tos >> 2)),
    ]
}

/// The talkers of one way of looking, while their minute is running.
#[derive(Debug, Default)]
struct Tally {
    keys: HashMap<Key, Counts>,
    /// What was counted under keys that are not held (any more).
    other: Counts,
}

/// Biggest first; ties in an order that does not depend on hashing.
fn biggest_first(a: &(Key, Counts), b: &(Key, Counts)) -> std::cmp::Ordering {
    b.1.bytes
        .cmp(&a.1.bytes)
        .then(b.1.packets.cmp(&a.1.packets))
        .then(b.1.flows.cmp(&a.1.flows))
        .then_with(|| a.0.cmp(&b.0))
}

impl Tally {
    fn add(&mut self, key: Key, counts: Counts) {
        if let Some(held) = self.keys.get_mut(&key) {
            held.add(counts);
            return;
        }
        if self.keys.len() >= LIVE_KEYS {
            // Half, not one: a flood of keys seen once each would otherwise
            // sort the whole list for every record.
            let (kept, folded) = split(std::mem::take(&mut self.keys), LIVE_KEYS / 2);
            self.keys = kept.into_iter().collect();
            self.other.add(folded);
        }
        self.keys.insert(key, counts);
    }

    fn sorted(&self) -> Vec<(Key, Counts)> {
        let mut keys: Vec<(Key, Counts)> = self
            .keys
            .iter()
            .map(|(key, counts)| (*key, *counts))
            .collect();
        keys.sort_by(biggest_first);
        keys
    }
}

/// The `keep` biggest keys, and what the others add up to.
fn split(keys: HashMap<Key, Counts>, keep: usize) -> (Vec<(Key, Counts)>, Counts) {
    let mut keys: Vec<(Key, Counts)> = keys.into_iter().collect();
    keys.sort_by(biggest_first);
    let mut folded = Counts::default();
    for (_, counts) in keys.drain(keep.min(keys.len())..) {
        folded.add(counts);
    }
    (keys, folded)
}

/// What is known about a minute whichever way it is looked at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Minutes since the epoch.
    pub minute: i64,
    pub records: u64,
    pub total: Counts,
    /// Data sets that arrived in this minute before their template.
    pub sets_without_template: u64,
    /// Records the exporter said it sampled (a rate above 1).
    pub sampled_records: u64,
    /// The rates it said, ascending.
    pub rates: Vec<u32>,
}

/// Remember a sampling rate, in order, up to [`MAX_RATES`] of them.
pub fn note_rate(rates: &mut Vec<u32>, rate: u32) {
    if let Err(place) = rates.binary_search(&rate)
        && rates.len() < MAX_RATES
    {
        rates.insert(place, rate);
    }
}

impl Summary {
    fn count(&mut self, record: &FlowRecord) {
        self.records += 1;
        self.total.add(Counts::of(record));
        if record.sampling > 1 {
            self.sampled_records += 1;
            note_rate(&mut self.rates, record.sampling);
        }
    }
}

/// The minute that is running.
#[derive(Debug, Default)]
struct Open {
    summary: Summary,
    tallies: [Tally; 6],
}

/// A minute that is over: its biggest talkers, biggest first.
#[derive(Debug, Default)]
pub struct Minute {
    pub summary: Summary,
    keys: [Vec<(Key, Counts)>; 6],
    other: [Counts; 6],
}

impl Minute {
    fn held(&self) -> usize {
        self.keys.iter().map(Vec::len).sum()
    }

    /// This minute and another count of the same minute, as one.
    fn joined(self, earlier: &Minute) -> Minute {
        let mut summary = self.summary;
        summary.records += earlier.summary.records;
        summary.total.add(earlier.summary.total);
        summary.sets_without_template += earlier.summary.sets_without_template;
        summary.sampled_records += earlier.summary.sampled_records;
        for rate in &earlier.summary.rates {
            note_rate(&mut summary.rates, *rate);
        }
        let mut joined = Minute {
            summary,
            ..Minute::default()
        };
        for (slot, keys) in self.keys.into_iter().enumerate() {
            let mut both: HashMap<Key, Counts> = keys.into_iter().collect();
            for (key, counts) in &earlier.keys[slot] {
                both.entry(*key).or_default().add(*counts);
            }
            let (kept, folded) = split(both, TOP_KEYS);
            joined.keys[slot] = kept;
            joined.other[slot] = self.other[slot];
            joined.other[slot].add(earlier.other[slot]);
            joined.other[slot].add(folded);
        }
        joined
    }
}

/// Put a finished minute with the others, and count its keys.
///
/// A minute is there once. The clock can step back into a minute that was
/// already finished, and what arrives then is that minute's too: the two
/// counts are joined rather than kept side by side, so that minutes stay in
/// order and each keeps one list of its biggest talkers.
fn shelve(minutes: &mut VecDeque<Arc<Minute>>, held: &mut usize, open: Open) {
    let mut finished = open.close();
    if minutes
        .back()
        .is_some_and(|last| last.summary.minute == finished.summary.minute)
        && let Some(last) = minutes.pop_back()
    {
        *held = held.saturating_sub(last.held());
        finished = finished.joined(&last);
    }
    *held += finished.held();
    minutes.push_back(Arc::new(finished));
}

impl Open {
    fn close(self) -> Minute {
        let mut minute = Minute {
            summary: self.summary,
            ..Minute::default()
        };
        for (slot, tally) in self.tallies.into_iter().enumerate() {
            let (mut kept, folded) = split(tally.keys, TOP_KEYS);
            kept.shrink_to_fit();
            minute.keys[slot] = kept;
            minute.other[slot] = tally.other;
            minute.other[slot].add(folded);
        }
        minute
    }
}

/// One exporter: who it is, what it has sent, and its minutes.
#[derive(Debug)]
struct Exporter {
    device_id: Option<String>,
    /// Why no device was attributed (empty when one was).
    reason: String,
    /// Which attribution index `device_id` was read from.
    attributed_at: i64,
    version: u8,
    first_at: i64,
    last_at: i64,
    datagrams: u64,
    /// Flow records taken into the buckets.
    records: u64,
    /// Flow records not taken, because no device claims the address and the
    /// settings say to drop what such addresses send.
    dropped: u64,
    sets_without_template: u64,
    templates: u32,
    rates: Vec<u32>,
    open: Option<Open>,
    /// Oldest first.
    minutes: VecDeque<Arc<Minute>>,
}

/// One exporter as `flows.exporters` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExporterInfo {
    pub address: IpAddr,
    pub device_id: Option<String>,
    pub reason: String,
    pub version: u8,
    pub first_at: i64,
    pub last_at: i64,
    pub datagrams: u64,
    pub records: u64,
    pub dropped: u64,
    pub sets_without_template: u64,
    pub templates: u32,
    pub rates: Vec<u32>,
}

/// What one batch came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Its records are in the buckets.
    Counted,
    /// The exporter is listed and its records were not kept (no device
    /// claims it, and such sources are dropped).
    Unclaimed,
    /// There was no room to follow one more exporter.
    NoRoom,
}

/// What the table has let go of, counted since the process started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Released {
    /// Records from exporters there was no room to follow.
    pub refused_records: u64,
    /// Records from exporters no device claims, dropped as the settings say.
    pub unclaimed_records: u64,
    /// Minutes let go before they were a day old, to stay under [`MAX_KEYS`].
    pub evicted_minutes: u64,
}

/// What the table holds right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub exporters: usize,
    pub minutes: usize,
    pub keys: usize,
    /// The start of the oldest minute held, milliseconds since the epoch.
    pub oldest_at: Option<i64>,
}

/// One exporter's part of an answer: its minutes inside the window.
pub struct Slice {
    pub address: IpAddr,
    pub device_id: Option<String>,
    /// The finished minutes, oldest first. Shared, not copied: an answer
    /// over a day of twenty exporters would otherwise copy all of it while
    /// the receiver waits for the lock.
    pub minutes: Vec<Arc<Minute>>,
    /// The running minute, as it stands.
    pub running: Option<(Summary, Vec<(Key, Counts)>, Counts)>,
}

/// Who an answer is about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Only the exporters attributed to this device.
    pub device_id: Option<String>,
    /// Only this exporter.
    pub exporter: Option<IpAddr>,
}

impl Selection {
    fn takes(&self, address: IpAddr, device_id: Option<&str>) -> bool {
        self.exporter.is_none_or(|wanted| wanted == address)
            && self
                .device_id
                .as_deref()
                .is_none_or(|wanted| device_id == Some(wanted))
    }
}

/// Everything the flow collector has aggregated.
#[derive(Debug, Default)]
pub struct Table {
    exporters: HashMap<IpAddr, Exporter>,
    /// Keys held by finished minutes, over all exporters.
    keys: usize,
    released: Released,
    /// The last minute [`Table::tidy`] ran in.
    tidied: i64,
    /// The ceiling on `keys`; [`MAX_KEYS`] unless a test set another.
    max_keys: Option<usize>,
}

impl Minute {
    /// The talkers of one way of looking, and what there was besides them.
    pub fn talkers(&self, dimension: Dimension) -> (&[(Key, Counts)], Counts) {
        (&self.keys[dimension.slot()], self.other[dimension.slot()])
    }
}

impl Table {
    /// A table that lets minutes go at `max_keys` instead of [`MAX_KEYS`]:
    /// the ceiling is the same code either way, and a test cannot fill two
    /// million keys.
    pub fn with_max_keys(max_keys: usize) -> Table {
        Table {
            max_keys: Some(max_keys),
            ..Table::default()
        }
    }

    fn max_keys(&self) -> usize {
        self.max_keys.unwrap_or(MAX_KEYS)
    }

    /// Take in what one datagram held, received at `now`.
    ///
    /// `attribute` is asked whose address the exporter's is — only when the
    /// answer held for it is from another index than `index_stamp`, so not
    /// for every datagram. `drop_unclaimed` is the `unknown_sources: "drop"`
    /// setting.
    pub fn add(
        &mut self,
        batch: &FlowBatch,
        now: i64,
        index_stamp: i64,
        attribute: impl Fn() -> Attribution,
        drop_unclaimed: bool,
    ) -> Taken {
        let minute = now.div_euclid(BUCKET_MS);
        if minute != self.tidied {
            self.tidy(now);
        }
        let records = batch.records.len() as u64;
        if !self.exporters.contains_key(&batch.exporter) {
            let attribution = attribute();
            if !self.make_room(attribution.device_id.is_some()) {
                self.released.refused_records += records;
                return Taken::NoRoom;
            }
            self.exporters.insert(
                batch.exporter,
                Exporter {
                    device_id: attribution.device_id,
                    reason: attribution.reason,
                    attributed_at: index_stamp,
                    version: batch.version,
                    first_at: now,
                    last_at: now,
                    datagrams: 0,
                    records: 0,
                    dropped: 0,
                    sets_without_template: 0,
                    templates: 0,
                    rates: Vec::new(),
                    open: None,
                    minutes: VecDeque::new(),
                },
            );
        }
        let exporter = self
            .exporters
            .get_mut(&batch.exporter)
            .expect("the exporter was just looked up or added");
        if exporter.attributed_at != index_stamp {
            let attribution = attribute();
            exporter.device_id = attribution.device_id;
            exporter.reason = attribution.reason;
            exporter.attributed_at = index_stamp;
        }
        exporter.version = batch.version;
        exporter.last_at = exporter.last_at.max(now);
        exporter.datagrams += 1;
        exporter.templates = batch.templates;
        exporter.sets_without_template += u64::from(batch.sets_without_template);
        if drop_unclaimed && exporter.device_id.is_none() {
            exporter.dropped += records;
            self.released.unclaimed_records += records;
            return Taken::Unclaimed;
        }
        if batch.records.is_empty() && batch.sets_without_template == 0 {
            // Templates only: nothing a bucket would say.
            return Taken::Counted;
        }

        // A clock that stepped back does not get a bucket before the newest
        // one there is: what arrives now is counted in that one.
        let newest = exporter
            .open
            .as_ref()
            .map(|open| open.summary.minute)
            .or(exporter.minutes.back().map(|last| last.summary.minute));
        let minute = newest.map_or(minute, |newest| newest.max(minute));
        if exporter
            .open
            .as_ref()
            .is_some_and(|open| open.summary.minute < minute)
            && let Some(open) = exporter.open.take()
        {
            shelve(&mut exporter.minutes, &mut self.keys, open);
        }
        let open = exporter.open.get_or_insert_with(|| Open {
            summary: Summary {
                minute,
                ..Summary::default()
            },
            ..Open::default()
        });
        open.summary.sets_without_template += u64::from(batch.sets_without_template);
        for record in &batch.records {
            open.summary.count(record);
            if record.sampling > 1 {
                note_rate(&mut exporter.rates, record.sampling);
            }
            let counts = Counts::of(record);
            for (dimension, key) in keys_of(record) {
                open.tallies[dimension.slot()].add(key, counts);
            }
        }
        exporter.records += records;
        self.trim();
        Taken::Counted
    }

    /// The collector was stopped. The templates were its decoder's, and went
    /// with it: until an exporter sends them again there are none, and
    /// `templates` must not go on saying what a decoder that no longer
    /// exists knew.
    pub fn templates_forgotten(&mut self) {
        for exporter in self.exporters.values_mut() {
            exporter.templates = 0;
        }
    }

    /// Room for one more exporter. A full table gives up the one that has
    /// been silent longest among those no device claims — to a newcomer a
    /// device does claim, never to another stranger: whoever can send a
    /// datagram can invent an exporter, and inventing 256 of them must not
    /// push the real ones out.
    fn make_room(&mut self, claimed: bool) -> bool {
        if self.exporters.len() < MAX_EXPORTERS {
            return true;
        }
        if !claimed {
            return false;
        }
        let stranger = self
            .exporters
            .iter()
            .filter(|(_, exporter)| exporter.device_id.is_none())
            .min_by_key(|(_, exporter)| exporter.last_at)
            .map(|(address, _)| *address);
        match stranger {
            Some(address) => {
                self.forget(address);
                true
            }
            None => false,
        }
    }

    fn forget(&mut self, address: IpAddr) {
        if let Some(exporter) = self.exporters.remove(&address) {
            let held: usize = exporter.minutes.iter().map(|minute| minute.held()).sum();
            self.keys = self.keys.saturating_sub(held);
        }
    }

    /// Finish the minutes that are over, drop the ones that are a day old,
    /// and forget exporters of which nothing is left.
    pub fn tidy(&mut self, now: i64) {
        let minute = now.div_euclid(BUCKET_MS);
        self.tidied = minute;
        let oldest_kept = minute - KEEP_MINUTES;
        let mut gone = Vec::new();
        for (address, exporter) in &mut self.exporters {
            if exporter
                .open
                .as_ref()
                .is_some_and(|open| open.summary.minute < minute)
                && let Some(open) = exporter.open.take()
            {
                shelve(&mut exporter.minutes, &mut self.keys, open);
            }
            while exporter
                .minutes
                .front()
                .is_some_and(|first| first.summary.minute < oldest_kept)
            {
                if let Some(first) = exporter.minutes.pop_front() {
                    self.keys = self.keys.saturating_sub(first.held());
                }
            }
            // An exporter that has been silent for a day has nothing left to
            // show; one that is only listed (its records were never kept)
            // goes the same way.
            if exporter.open.is_none()
                && exporter.minutes.is_empty()
                && exporter.last_at.div_euclid(BUCKET_MS) < oldest_kept
            {
                gone.push(*address);
            }
        }
        for address in gone {
            self.exporters.remove(&address);
        }
        self.trim();
    }

    /// Stay under the ceiling on keys: the oldest finished minute of any
    /// exporter goes first.
    fn trim(&mut self) {
        while self.keys > self.max_keys() {
            let oldest = self
                .exporters
                .iter()
                .filter_map(|(address, exporter)| {
                    Some((exporter.minutes.front()?.summary.minute, *address))
                })
                .min();
            let Some((_, address)) = oldest else {
                return;
            };
            if let Some(first) = self
                .exporters
                .get_mut(&address)
                .and_then(|exporter| exporter.minutes.pop_front())
            {
                self.keys = self.keys.saturating_sub(first.held());
                self.released.evicted_minutes += 1;
            }
        }
    }

    /// Bring every exporter's attribution in line with a newer index.
    pub fn attribute_all(&mut self, index_stamp: i64, attribute: impl Fn(IpAddr) -> Attribution) {
        for (address, exporter) in &mut self.exporters {
            if exporter.attributed_at != index_stamp {
                let attribution = attribute(*address);
                exporter.device_id = attribution.device_id;
                exporter.reason = attribution.reason;
                exporter.attributed_at = index_stamp;
            }
        }
    }

    /// The minutes of the selected exporters from `first_minute` on, for one
    /// way of looking. Exporters with nothing in the window are left out.
    pub fn slices(
        &self,
        selection: &Selection,
        dimension: Dimension,
        first_minute: i64,
    ) -> Vec<Slice> {
        let mut slices: Vec<Slice> = self
            .exporters
            .iter()
            .filter(|(address, exporter)| selection.takes(**address, exporter.device_id.as_deref()))
            .filter_map(|(address, exporter)| {
                let minutes: Vec<Arc<Minute>> = exporter
                    .minutes
                    .iter()
                    .filter(|minute| minute.summary.minute >= first_minute)
                    .cloned()
                    .collect();
                let running = exporter
                    .open
                    .as_ref()
                    .filter(|open| open.summary.minute >= first_minute)
                    .map(|open| {
                        let tally = &open.tallies[dimension.slot()];
                        (open.summary.clone(), tally.sorted(), tally.other)
                    });
                (!minutes.is_empty() || running.is_some()).then(|| Slice {
                    address: *address,
                    device_id: exporter.device_id.clone(),
                    minutes,
                    running,
                })
            })
            .collect();
        slices.sort_by_key(|slice| slice.address);
        slices
    }

    /// Every exporter followed, most recently heard first.
    pub fn exporters(&self) -> Vec<ExporterInfo> {
        let mut exporters: Vec<ExporterInfo> = self
            .exporters
            .iter()
            .map(|(address, exporter)| ExporterInfo {
                address: *address,
                device_id: exporter.device_id.clone(),
                reason: exporter.reason.clone(),
                version: exporter.version,
                first_at: exporter.first_at,
                last_at: exporter.last_at,
                datagrams: exporter.datagrams,
                records: exporter.records,
                dropped: exporter.dropped,
                sets_without_template: exporter.sets_without_template,
                templates: exporter.templates,
                rates: exporter.rates.clone(),
            })
            .collect();
        exporters.sort_by(|a, b| b.last_at.cmp(&a.last_at).then(a.address.cmp(&b.address)));
        exporters
    }

    pub fn released(&self) -> Released {
        self.released
    }

    pub fn held(&self) -> Held {
        let mut held = Held {
            exporters: self.exporters.len(),
            keys: self.keys,
            ..Held::default()
        };
        let mut oldest: Option<i64> = None;
        for exporter in self.exporters.values() {
            held.minutes += exporter.minutes.len() + usize::from(exporter.open.is_some());
            held.keys += exporter
                .open
                .iter()
                .flat_map(|open| open.tallies.iter())
                .map(|tally| tally.keys.len())
                .sum::<usize>();
            let first = exporter
                .minutes
                .front()
                .map(|minute| minute.summary.minute)
                .or(exporter.open.as_ref().map(|open| open.summary.minute));
            oldest = match (oldest, first) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        held.oldest_at = oldest.map(|minute| minute * BUCKET_MS);
        held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_763_000_040_000; // the start of a minute

    fn record(src: &str, dst: &str, bytes: u64) -> FlowRecord {
        FlowRecord {
            exporter: "192.0.2.1".into(),
            version: 9,
            src: src.into(),
            dst: dst.into(),
            src_port: 50_000,
            dst_port: 443,
            protocol: 6,
            bytes,
            packets: 1,
            tos: 0,
            tcp_flags: 0,
            input_if: 1,
            output_if: 2,
            src_as: 0,
            dst_as: 0,
            next_hop: None,
            start_ms: 0,
            end_ms: 0,
            sampling: 0,
        }
    }

    fn batch(exporter: &str, records: Vec<FlowRecord>) -> FlowBatch {
        FlowBatch {
            exporter: exporter.parse().unwrap(),
            version: 9,
            records,
            sets_without_template: 0,
            templates: 1,
        }
    }

    fn nobody() -> Attribution {
        Attribution {
            device_id: None,
            reason: "no device has this address".into(),
        }
    }

    fn device(id: &str) -> Attribution {
        Attribution {
            device_id: Some(id.into()),
            reason: String::new(),
        }
    }

    fn add(table: &mut Table, exporter: &str, records: Vec<FlowRecord>, now: i64) -> Taken {
        table.add(&batch(exporter, records), now, 1, nobody, false)
    }

    /// Everything one exporter has for one way of looking, added up over
    /// its minutes: (keys, other, total).
    fn all(table: &Table, dimension: Dimension) -> (HashMap<Key, Counts>, Counts, Counts) {
        let mut keys: HashMap<Key, Counts> = HashMap::new();
        let (mut other, mut total) = (Counts::default(), Counts::default());
        for slice in table.slices(&Selection::default(), dimension, i64::MIN) {
            for minute in &slice.minutes {
                let (talkers, rest) = minute.talkers(dimension);
                for (key, counts) in talkers {
                    keys.entry(*key).or_default().add(*counts);
                }
                other.add(rest);
                total.add(minute.summary.total);
            }
            if let Some((summary, talkers, rest)) = &slice.running {
                for (key, counts) in talkers {
                    keys.entry(*key).or_default().add(*counts);
                }
                other.add(*rest);
                total.add(summary.total);
            }
        }
        (keys, other, total)
    }

    fn address(text: &str) -> Key {
        Key::Address(Some(text.parse().unwrap()))
    }

    #[test]
    fn a_record_is_counted_once_under_each_way_of_looking_and_twice_under_its_interfaces() {
        let mut table = Table::default();
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.1", "10.0.0.2", 700)],
            T0,
        );
        let one = Counts {
            bytes: 700,
            packets: 1,
            flows: 1,
        };
        for (dimension, expected) in [
            (Dimension::Source, vec![address("10.0.0.1")]),
            (Dimension::Destination, vec![address("10.0.0.2")]),
            (
                Dimension::Conversation,
                vec![Key::Conversation(
                    Some("10.0.0.1".parse().unwrap()),
                    Some("10.0.0.2".parse().unwrap()),
                )],
            ),
            (Dimension::Application, vec![Key::Application(6, Some(443))]),
            (
                Dimension::Interface,
                vec![
                    Key::Interface(1, Direction::In),
                    Key::Interface(2, Direction::Out),
                ],
            ),
            (Dimension::Dscp, vec![Key::Dscp(0)]),
        ] {
            let (keys, other, total) = all(&table, dimension);
            let mut found: Vec<Key> = keys.keys().copied().collect();
            found.sort();
            assert_eq!(found, expected, "{dimension:?}");
            assert!(keys.values().all(|counts| *counts == one), "{dimension:?}");
            assert_eq!(other, Counts::default());
            assert_eq!(total, one, "the total counts a record once: {dimension:?}");
        }
    }

    #[test]
    fn a_key_costs_what_the_ceiling_was_worked_out_with() {
        // `MAX_KEYS` is a memory bound only as long as a key is this size.
        assert!(std::mem::size_of::<(Key, Counts)>() <= 64);
    }

    #[test]
    fn both_directions_of_a_conversation_are_one_key() {
        let mut table = Table::default();
        add(
            &mut table,
            "192.0.2.1",
            vec![
                record("10.0.0.9", "10.0.0.2", 100),
                record("10.0.0.2", "10.0.0.9", 900),
            ],
            T0,
        );
        let (keys, _, _) = all(&table, Dimension::Conversation);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.values().next().unwrap().bytes, 1_000);
    }

    #[test]
    fn a_minute_that_is_over_keeps_its_biggest_talkers_and_adds_up_the_rest() {
        let mut table = Table::default();
        // Thirty sources in one minute, the n-th sending n kilobytes.
        let records: Vec<FlowRecord> = (1..=30u64)
            .map(|n| record(&format!("10.0.1.{n}"), "10.0.0.2", n * 1_000))
            .collect();
        add(&mut table, "192.0.2.1", records, T0);
        // While the minute runs, all thirty are there.
        let (keys, other, _) = all(&table, Dimension::Source);
        assert_eq!((keys.len(), other), (30, Counts::default()));

        table.tidy(T0 + BUCKET_MS);
        let (keys, other, total) = all(&table, Dimension::Source);
        assert_eq!(keys.len(), TOP_KEYS);
        assert!(
            keys.contains_key(&address("10.0.1.30")) && keys.contains_key(&address("10.0.1.11")),
            "the twenty biggest are the ones kept"
        );
        assert!(!keys.contains_key(&address("10.0.1.10")));
        // 1 + 2 + … + 10 kilobytes went into `other`, and nothing was lost.
        assert_eq!(
            other,
            Counts {
                bytes: 55_000,
                packets: 10,
                flows: 10
            }
        );
        let kept: u64 = keys.values().map(|counts| counts.bytes).sum();
        assert_eq!(kept + other.bytes, total.bytes);
        assert_eq!(total.bytes, 465_000);
        // Twenty sources, twenty conversations, and the one destination, the
        // one application, the two interfaces and the one marking they share.
        assert_eq!(table.held().keys, 2 * TOP_KEYS + 1 + 1 + 2 + 1);
    }

    #[test]
    fn a_flood_of_made_up_sources_cannot_grow_a_minute_without_bound() {
        let mut table = Table::default();
        // One real talker early in the minute, then fifty thousand sources
        // seen once each, then the real talker again.
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.9.9.9", "10.0.0.2", 5_000_000)],
            T0,
        );
        let mut most = 0;
        for chunk in 0..500u32 {
            let records: Vec<FlowRecord> = (0..100u32)
                .map(|n| {
                    let made_up = chunk * 100 + n;
                    record(
                        &format!(
                            "172.{}.{}.{}",
                            16 + (made_up >> 16),
                            (made_up >> 8) & 255,
                            made_up & 255
                        ),
                        "10.0.0.2",
                        40,
                    )
                })
                .collect();
            add(&mut table, "192.0.2.1", records, T0 + 1_000);
            most = most.max(table.held().keys);
        }
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.9.9.9", "10.0.0.2", 5_000_000)],
            T0 + 2_000,
        );
        // Sources and conversations at their ceiling, and the one
        // destination, application and marking and the two interfaces.
        assert!(
            most <= 2 * LIVE_KEYS + 5,
            "the running minute never held more than its ceiling: {most}"
        );
        let (keys, other, total) = all(&table, Dimension::Source);
        assert!(keys.len() <= LIVE_KEYS);
        assert_eq!(
            keys[&address("10.9.9.9")].bytes,
            10_000_000,
            "the one that mattered was never let go"
        );
        // Every byte is either under a key that is held or in `other`.
        let held: u64 = keys.values().map(|counts| counts.bytes).sum();
        assert_eq!(held + other.bytes, total.bytes);
        assert_eq!(total.flows, 50_002);
        assert!(other.flows >= 50_000 - LIVE_KEYS as u64, "{other:?}");
    }

    #[test]
    fn a_key_that_was_folded_and_comes_back_is_a_floor_not_a_guess() {
        let mut table = Table::default();
        // A small talker, pushed out by a flood of bigger ones, then back.
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.9.9.9", "10.0.0.2", 10)],
            T0,
        );
        let flood: Vec<FlowRecord> = (0..LIVE_KEYS as u32 + 10)
            .map(|n| record(&format!("172.16.{}.{}", n >> 8, n & 255), "10.0.0.2", 1_000))
            .collect();
        add(&mut table, "192.0.2.1", flood, T0 + 1_000);
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.9.9.9", "10.0.0.2", 7)],
            T0 + 2_000,
        );
        let (keys, other, total) = all(&table, Dimension::Source);
        assert_eq!(
            keys[&address("10.9.9.9")].bytes,
            7,
            "only what was counted since it came back: never more than was seen"
        );
        let held: u64 = keys.values().map(|counts| counts.bytes).sum();
        assert_eq!(
            held + other.bytes,
            total.bytes,
            "and its first ten bytes are in `other`"
        );
    }

    #[test]
    fn a_bucket_is_gone_when_it_is_a_day_old_and_not_a_minute_before() {
        let mut table = Table::default();
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.1", "10.0.0.2", 100)],
            T0,
        );
        // Still there in the last minute of its day.
        table.tidy(T0 + KEEP_MINUTES * BUCKET_MS + BUCKET_MS - 1);
        assert_eq!(table.held().minutes, 1);
        assert_eq!(table.held().oldest_at, Some(T0));
        assert_eq!(all(&table, Dimension::Source).2.bytes, 100);
        // And gone a millisecond later, with its exporter.
        table.tidy(T0 + (KEEP_MINUTES + 1) * BUCKET_MS);
        assert_eq!(
            table.held(),
            Held::default(),
            "nothing is left, and nothing is counted as held"
        );
        assert!(table.exporters().is_empty());
    }

    #[test]
    fn an_answer_only_reaches_back_as_far_as_it_is_asked_to() {
        let mut table = Table::default();
        for (minute, bytes) in [(0, 1), (5, 10), (9, 100)] {
            add(
                &mut table,
                "192.0.2.1",
                vec![record("10.0.0.1", "10.0.0.2", bytes)],
                T0 + minute * BUCKET_MS,
            );
        }
        let from = (T0 + 5 * BUCKET_MS) / BUCKET_MS;
        let slices = table.slices(&Selection::default(), Dimension::Source, from);
        let finished: u64 = slices[0]
            .minutes
            .iter()
            .map(|minute| minute.summary.total.bytes)
            .sum();
        let running = slices[0].running.as_ref().unwrap().0.total.bytes;
        assert_eq!((finished, running), (10, 100));
    }

    #[test]
    fn the_oldest_minutes_go_first_when_everything_together_is_too_much() {
        // One record is seven keys (two of them interfaces), so a minute of
        // one record holds seven.
        let mut table = Table::with_max_keys(7 * 3);
        for minute in 0..6 {
            // Two exporters, alternating, so the oldest minute is not always
            // the same exporter's.
            let exporter = if minute % 2 == 0 {
                "192.0.2.1"
            } else {
                "192.0.2.2"
            };
            add(
                &mut table,
                exporter,
                vec![record("10.0.0.1", "10.0.0.2", 100)],
                T0 + minute * BUCKET_MS,
            );
        }
        table.tidy(T0 + 6 * BUCKET_MS);
        let held = table.held();
        assert_eq!(held.keys, 21, "never more than the ceiling");
        assert_eq!(held.minutes, 3);
        assert_eq!(
            held.oldest_at,
            Some(T0 + 3 * BUCKET_MS),
            "what is left is the newest three minutes, whoever they belong to"
        );
        assert_eq!(table.released().evicted_minutes, 3);
    }

    #[test]
    fn a_stranger_never_takes_the_place_of_an_exporter_and_a_device_takes_a_strangers() {
        let mut table = Table::default();
        for n in 0..MAX_EXPORTERS as u32 {
            let taken = table.add(
                &batch(
                    &format!("198.51.{}.{}", n >> 8, n & 255),
                    vec![record("10.0.0.1", "10.0.0.2", 1)],
                ),
                T0 + i64::from(n),
                1,
                nobody,
                false,
            );
            assert_eq!(taken, Taken::Counted);
        }
        // One more stranger: no room, and what it sent is counted as refused.
        let taken = table.add(
            &batch(
                "203.0.113.9",
                vec![
                    record("10.0.0.1", "10.0.0.2", 1),
                    record("10.0.0.1", "10.0.0.3", 1),
                ],
            ),
            T0 + 1_000,
            1,
            nobody,
            false,
        );
        assert_eq!(taken, Taken::NoRoom);
        assert_eq!(table.released().refused_records, 2);
        assert_eq!(table.held().exporters, MAX_EXPORTERS);

        // A device of the inventory starts exporting: the stranger that has
        // been silent longest makes way for it.
        let taken = table.add(
            &batch("10.0.0.254", vec![record("10.0.0.1", "10.0.0.2", 1)]),
            T0 + 2_000,
            1,
            || device("core1"),
            false,
        );
        assert_eq!(taken, Taken::Counted);
        assert_eq!(table.held().exporters, MAX_EXPORTERS);
        let listed = table.exporters();
        assert_eq!(listed[0].device_id.as_deref(), Some("core1"));
        assert!(
            !listed
                .iter()
                .any(|exporter| exporter.address == "198.51.0.0".parse::<IpAddr>().unwrap()),
            "the first stranger, silent longest, is the one that went"
        );
    }

    #[test]
    fn an_unclaimed_exporter_is_listed_with_what_was_not_kept_when_such_sources_are_dropped() {
        let mut table = Table::default();
        let taken = table.add(
            &batch("203.0.113.9", vec![record("10.0.0.1", "10.0.0.2", 500)]),
            T0,
            1,
            nobody,
            true,
        );
        assert_eq!(taken, Taken::Unclaimed);
        let listed = table.exporters();
        assert_eq!(
            (listed[0].records, listed[0].dropped, listed[0].datagrams),
            (0, 1, 1)
        );
        assert_eq!(listed[0].reason, "no device has this address");
        assert_eq!(table.held().minutes, 0, "and nothing of it is in a bucket");
        assert_eq!(table.released().unclaimed_records, 1);
    }

    #[test]
    fn an_exporter_is_attributed_again_when_the_inventory_changed() {
        let mut table = Table::default();
        add(
            &mut table,
            "10.0.0.254",
            vec![record("10.0.0.1", "10.0.0.2", 1)],
            T0,
        );
        assert_eq!(table.exporters()[0].device_id, None);
        // Same index: nobody is asked again.
        table.add(
            &batch("10.0.0.254", vec![]),
            T0 + 1,
            1,
            || panic!("the answer held is from this index"),
            false,
        );
        // A newer index knows the address.
        table.attribute_all(2, |_| device("core1"));
        assert_eq!(table.exporters()[0].device_id.as_deref(), Some("core1"));
        assert_eq!(table.exporters()[0].reason, "");
        let selected = Selection {
            device_id: Some("core1".into()),
            exporter: None,
        };
        assert_eq!(
            table.slices(&selected, Dimension::Source, i64::MIN).len(),
            1
        );
        let other = Selection {
            device_id: Some("core2".into()),
            exporter: None,
        };
        assert!(table.slices(&other, Dimension::Source, i64::MIN).is_empty());
    }

    #[test]
    fn a_sampling_rate_is_remembered_beside_the_counts_and_never_multiplied_in() {
        let mut table = Table::default();
        let mut sampled = record("10.0.0.1", "10.0.0.2", 1_000);
        sampled.sampling = 100;
        let mut unsampled = record("10.0.0.3", "10.0.0.2", 50);
        unsampled.sampling = 1;
        add(&mut table, "192.0.2.1", vec![sampled, unsampled], T0);
        let slices = table.slices(&Selection::default(), Dimension::Source, i64::MIN);
        let (summary, talkers, _) = slices[0].running.as_ref().unwrap();
        assert_eq!(summary.rates, vec![100]);
        assert_eq!((summary.records, summary.sampled_records), (2, 1));
        assert_eq!(
            talkers[0],
            (
                address("10.0.0.1"),
                Counts {
                    bytes: 1_000,
                    packets: 1,
                    flows: 1
                }
            ),
            "as exported: a thousand bytes, not a hundred thousand"
        );
        assert_eq!(table.exporters()[0].rates, vec![100]);
    }

    #[test]
    fn a_set_without_its_template_is_counted_in_its_minute_and_adds_no_traffic() {
        let mut table = Table::default();
        let mut early = batch("192.0.2.1", vec![]);
        early.sets_without_template = 3;
        early.templates = 0;
        table.add(&early, T0, 1, nobody, false);
        let slices = table.slices(&Selection::default(), Dimension::Source, i64::MIN);
        let (summary, talkers, other) = slices[0].running.as_ref().unwrap();
        assert_eq!(summary.sets_without_template, 3);
        assert_eq!((summary.records, summary.total), (0, Counts::default()));
        assert!(talkers.is_empty() && other.is_zero());
        assert_eq!(table.exporters()[0].sets_without_template, 3);
    }

    #[test]
    fn a_minute_is_kept_once_even_when_the_clock_stepped_back_into_it() {
        let mut table = Table::default();
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.1", "10.0.0.2", 100)],
            T0,
        );
        // The minute is finished…
        table.tidy(T0 + BUCKET_MS);
        assert_eq!(table.held().keys, 7);
        // …and then the clock is back in it, twice, with more of the same
        // source and with a new one.
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.1", "10.0.0.2", 10)],
            T0 + 30_000,
        );
        table.tidy(T0 + BUCKET_MS);
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.3", "10.0.0.2", 1)],
            T0 - 5 * BUCKET_MS,
        );
        table.tidy(T0 + BUCKET_MS);

        let held = table.held();
        assert_eq!(
            (held.minutes, held.oldest_at),
            (1, Some(T0)),
            "one bucket for the minute, not three"
        );
        let (keys, other, total) = all(&table, Dimension::Source);
        assert_eq!(keys[&address("10.0.0.1")].bytes, 110);
        assert_eq!(keys[&address("10.0.0.3")].bytes, 1);
        assert_eq!(
            (other, total.bytes, total.flows),
            (Counts::default(), 111, 3)
        );
        // Two sources, two conversations, and what they share.
        assert_eq!(held.keys, 2 + 1 + 2 + 1 + 2 + 1);
    }

    #[test]
    fn a_clock_that_steps_back_does_not_make_a_bucket_in_the_past() {
        let mut table = Table::default();
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.1", "10.0.0.2", 1)],
            T0 + 5 * BUCKET_MS,
        );
        add(
            &mut table,
            "192.0.2.1",
            vec![record("10.0.0.1", "10.0.0.2", 2)],
            T0,
        );
        let held = table.held();
        assert_eq!(
            (held.minutes, held.oldest_at),
            (1, Some(T0 + 5 * BUCKET_MS))
        );
        assert_eq!(all(&table, Dimension::Source).2.bytes, 3);
    }
}
