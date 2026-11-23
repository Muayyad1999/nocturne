//! Flow export collection: NetFlow v5, NetFlow v9 and IPFIX.
//!
//! The collector decodes what arrives and hands it on through a queue with a limit. An
//! exporter sends as fast as its flow cache empties, and anybody who can reach the port can
//! send more than that, so what waits to be taken is bounded by the number of flow records
//! and whatever does not fit is dropped and counted ([`FlowCollector::dropped`]) rather than
//! kept until the machine has no memory left.

mod decode;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{Receiver, Sender, channel, error::TrySendError};
use tokio::task::JoinHandle;

pub use self::decode::{Decoder, DecoderStats};
use crate::ProbeError;

/// One flow as an exporter reported it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FlowRecord {
    /// The address of the device that exported the flow.
    pub exporter: String,
    /// 5, 9, or 10 (IPFIX).
    pub version: u8,
    /// Source and destination address; empty when the record does not carry one.
    pub src: String,
    pub dst: String,
    pub src_port: u16,
    /// For ICMP: type * 256 + code, as NetFlow v5 encodes it.
    pub dst_port: u16,
    /// IP protocol number (6 TCP, 17 UDP, 1 ICMP, …).
    pub protocol: u8,
    pub bytes: u64,
    pub packets: u64,
    pub tos: u8,
    /// The TCP flags seen in the flow, or-ed together.
    pub tcp_flags: u8,
    /// SNMP ifIndex of the interface the flow came in on / went out of.
    pub input_if: u32,
    pub output_if: u32,
    pub src_as: u32,
    pub dst_as: u32,
    pub next_hop: Option<String>,
    /// Start and end of the flow in milliseconds: Unix time when the exporter gives absolute
    /// times (the IPFIX way), otherwise relative to the exporter's boot (sysUptime), as in
    /// NetFlow v5 and most v9 templates.
    pub start_ms: u64,
    pub end_ms: u64,
    /// The exporter samples one packet in this many; 0 or 1 when it does not sample (or does
    /// not say). `bytes` and `packets` are as exported, not multiplied.
    pub sampling: u32,
}

/// An entry of a [`top`] list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Talker {
    pub key: String,
    pub bytes: u64,
    pub packets: u64,
    /// How many flow records were added up.
    pub flows: u64,
}

/// Aggregates records and returns the top `n` by bytes.
///
/// `by` chooses the key: `src`, `dst`, `conversation` (both directions of an address pair,
/// `a <-> b`), `application` (protocol and service port, such as `tcp/443`), or `interface`
/// (the input ifIndex). Any other value yields an empty list.
pub fn top(records: &[FlowRecord], by: &str, n: usize) -> Vec<Talker> {
    let key: fn(&FlowRecord) -> String = match by {
        "src" => |record| record.src.clone(),
        "dst" => |record| record.dst.clone(),
        "conversation" => conversation,
        "application" => application,
        "interface" => |record| record.input_if.to_string(),
        _ => return Vec::new(),
    };
    let mut totals: HashMap<String, Talker> = HashMap::new();
    for record in records {
        let key = key(record);
        let talker = totals.entry(key.clone()).or_insert(Talker {
            key,
            bytes: 0,
            packets: 0,
            flows: 0,
        });
        talker.bytes = talker.bytes.saturating_add(record.bytes);
        talker.packets = talker.packets.saturating_add(record.packets);
        talker.flows += 1;
    }
    let mut talkers: Vec<Talker> = totals.into_values().collect();
    // Ties are broken by packets, then by key, so the order does not depend on hashing.
    talkers.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then(b.packets.cmp(&a.packets))
            .then_with(|| a.key.cmp(&b.key))
    });
    talkers.truncate(n);
    talkers
}

/// Both directions of a conversation share one key: the lower address first.
fn conversation(record: &FlowRecord) -> String {
    let order = |address: &str| address.parse::<IpAddr>().ok();
    let (first, second) = match (order(&record.src), order(&record.dst)) {
        (Some(src), Some(dst)) if dst < src => (&record.dst, &record.src),
        (None, None) if record.dst < record.src => (&record.dst, &record.src),
        _ => (&record.src, &record.dst),
    };
    format!("{first} <-> {second}")
}

/// Protocol and service port. The service is taken to be on the lower port: servers listen on
/// low, registered ports and clients connect from high, ephemeral ones.
fn application(record: &FlowRecord) -> String {
    let protocol = match record.protocol {
        1 => return "icmp".to_owned(),
        2 => return "igmp".to_owned(),
        6 => "tcp",
        17 => "udp",
        41 => return "ipv6".to_owned(),
        47 => return "gre".to_owned(),
        50 => return "esp".to_owned(),
        51 => return "ah".to_owned(),
        58 => return "icmpv6".to_owned(),
        88 => return "eigrp".to_owned(),
        89 => return "ospf".to_owned(),
        103 => return "pim".to_owned(),
        112 => return "vrrp".to_owned(),
        132 => "sctp",
        other => return format!("ip/{other}"),
    };
    let port = match (record.src_port, record.dst_port) {
        (0, port) | (port, 0) => port,
        (a, b) => a.min(b),
    };
    format!("{protocol}/{port}")
}

/// What one export datagram held, as the collector hands it on.
///
/// A datagram that decoded is always reported, also when it carried no flow: one with only
/// templates, or only data whose template has not arrived, still says that an exporter is
/// there and what it speaks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowBatch {
    /// The address the datagram came from. An IPv4 sender is an IPv4 address also when the
    /// socket is bound to `::` (where the system reports it as `::ffff:a.b.c.d`).
    pub exporter: IpAddr,
    /// 5, 9, or 10 (IPFIX).
    pub version: u8,
    pub records: Vec<FlowRecord>,
    /// Data sets in this datagram that were dropped because their template is not known.
    pub sets_without_template: u32,
    /// How many templates the decoder holds for this exporter's address after this datagram.
    pub templates: u32,
}

/// What a collector threw away because nobody took it in time.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Datagrams that decoded and were not handed on.
    pub datagrams: u64,
    /// The flow records in them.
    pub records: u64,
}

/// The receiving end of a collector: one [`FlowBatch`] per datagram, oldest first.
#[derive(Debug)]
pub struct FlowQueue {
    batches: Receiver<FlowBatch>,
    waiting: Arc<AtomicUsize>,
}

impl FlowQueue {
    /// The next datagram's worth. `None` once the collector has stopped and everything it had
    /// decoded has been taken.
    pub async fn recv(&mut self) -> Option<FlowBatch> {
        let batch = self.batches.recv().await?;
        self.waiting
            .fetch_sub(batch.records.len(), Ordering::Relaxed);
        Some(batch)
    }
}

/// What the socket task and the collector's handle both see.
#[derive(Debug, Default)]
struct Shared {
    stats: Mutex<DecoderStats>,
    /// Flow records decoded and not yet taken from the queue.
    waiting: Arc<AtomicUsize>,
    dropped_datagrams: AtomicU64,
    dropped_records: AtomicU64,
}

/// What a collector has counted. It can be read after the collector has stopped, which is
/// when its numbers are final.
#[derive(Clone, Debug)]
pub struct FlowCounters(Arc<Shared>);

impl FlowCounters {
    /// What the collector's decoder has seen so far.
    pub fn stats(&self) -> DecoderStats {
        *self.0.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What was decoded and then thrown away because the queue was full.
    pub fn dropped(&self) -> Dropped {
        Dropped {
            datagrams: self.0.dropped_datagrams.load(Ordering::Relaxed),
            records: self.0.dropped_records.load(Ordering::Relaxed),
        }
    }

    /// Flow records decoded and waiting to be taken from the queue right now.
    pub fn waiting(&self) -> usize {
        self.0.waiting.load(Ordering::Relaxed)
    }
}

/// A running flow collector. It stops when [`stop`](FlowCollector::stop) is called or it is
/// dropped.
#[derive(Debug)]
pub struct FlowCollector {
    local_addr: SocketAddr,
    counters: FlowCounters,
    task: Option<JoinHandle<()>>,
}

impl FlowCollector {
    /// Listens for NetFlow v5 / v9 and IPFIX export datagrams. What each datagram held
    /// arrives on the queue as one [`FlowBatch`].
    ///
    /// At most `queue_records` flow records wait to be taken (and at most that many
    /// datagrams, for the ones that carry none); a datagram that would take the queue past
    /// that is decoded — its templates are still learned — and then dropped and counted.
    /// One datagram is always let onto an empty queue, however many records it holds, or a
    /// limit smaller than a datagram would drop everything. A limit of zero is taken as one.
    pub async fn start(
        bind: SocketAddr,
        queue_records: usize,
    ) -> Result<(FlowCollector, FlowQueue), ProbeError> {
        let socket = UdpSocket::bind(bind)
            .await
            .map_err(|error| ProbeError::from_bind(error, "UDP", bind))?;
        // Exporters send in bursts when their flow cache expires; ask for room to queue them.
        let _ = socket2::SockRef::from(&socket).set_recv_buffer_size(4 * 1024 * 1024);
        let local_addr = socket.local_addr()?;
        let limit = queue_records.max(1);
        let (batches, receiver) = channel(limit);
        let shared = Arc::new(Shared::default());
        let task = tokio::spawn(collect(socket, batches, shared.clone(), limit));
        let waiting = shared.waiting.clone();
        Ok((
            FlowCollector {
                local_addr,
                counters: FlowCounters(shared),
                task: Some(task),
            },
            FlowQueue {
                batches: receiver,
                waiting,
            },
        ))
    }

    /// The address the collector listens on (useful after binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The collector's counters, as a handle that outlives it.
    pub fn counters(&self) -> FlowCounters {
        self.counters.clone()
    }

    /// What the collector's decoder has seen so far.
    pub fn stats(&self) -> DecoderStats {
        self.counters.stats()
    }

    /// What was decoded and then thrown away because the queue was full.
    pub fn dropped(&self) -> Dropped {
        self.counters.dropped()
    }

    /// Flow records decoded and waiting to be taken from the queue right now.
    pub fn waiting(&self) -> usize {
        self.counters.waiting()
    }

    /// Stops collecting; returns when the socket is closed.
    pub async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for FlowCollector {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// One exporter is one address, whichever family the socket reports it in: a socket bound to
/// `::` reports an IPv4 sender as `::ffff:a.b.c.d`, and everything that knows the exporter —
/// an inventory, its templates from before the collector was rebound — knows it as `a.b.c.d`.
fn canonical(peer: SocketAddr) -> SocketAddr {
    SocketAddr::new(peer.ip().to_canonical(), peer.port())
}

/// Receives and decodes datagrams until aborted, or until nobody listens to the records.
async fn collect(socket: UdpSocket, batches: Sender<FlowBatch>, shared: Arc<Shared>, limit: usize) {
    let mut decoder = Decoder::new();
    let mut datagram = vec![0u8; 65_536];
    loop {
        let (length, peer) = match socket.recv_from(&mut datagram).await {
            Ok(received) => received,
            Err(error) => {
                tracing::debug!(%error, "flow receive failed");
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        let exporter = canonical(peer);
        let before = decoder.stats().missing_template_sets;
        let decoded = decoder.decode(&datagram[..length], exporter);
        let stats = decoder.stats();
        *shared.stats.lock().unwrap_or_else(PoisonError::into_inner) = stats;
        let records = match decoded {
            Ok(records) => records,
            Err(error) => {
                tracing::debug!(%exporter, %error, "flow datagram rejected");
                continue;
            }
        };
        let count = records.len();
        // Counted before it is offered, so the bound holds even while the other side is
        // in the middle of taking a batch.
        let waiting = shared.waiting.fetch_add(count, Ordering::Relaxed);
        let refused = if waiting > 0 && waiting + count > limit {
            true
        } else {
            let batch = FlowBatch {
                exporter: exporter.ip(),
                // The decoder accepted it, so the first two bytes are 5, 9 or 10.
                version: datagram[1],
                records,
                sets_without_template: (stats.missing_template_sets - before)
                    .min(u64::from(u32::MAX)) as u32,
                templates: decoder.templates_of(exporter.ip()),
            };
            match batches.try_send(batch) {
                Ok(()) => false,
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Closed(_)) => return,
            }
        };
        if refused {
            shared.waiting.fetch_sub(count, Ordering::Relaxed);
            shared.dropped_datagrams.fetch_add(1, Ordering::Relaxed);
            shared
                .dropped_records
                .fetch_add(count as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
