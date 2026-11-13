//! Flow export collection: NetFlow v5, NetFlow v9 and IPFIX.

mod decode;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
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

/// A running flow collector. It stops when [`stop`](FlowCollector::stop) is called or it is
/// dropped.
#[derive(Debug)]
pub struct FlowCollector {
    local_addr: SocketAddr,
    stats: Arc<Mutex<DecoderStats>>,
    task: Option<JoinHandle<()>>,
}

impl FlowCollector {
    /// Listens for NetFlow v5 / v9 and IPFIX export datagrams. The records of each datagram
    /// arrive on the channel as one batch.
    pub async fn start(
        bind: SocketAddr,
    ) -> Result<(FlowCollector, UnboundedReceiver<Vec<FlowRecord>>), ProbeError> {
        let socket = UdpSocket::bind(bind)
            .await
            .map_err(|error| ProbeError::from_bind(error, "UDP", bind))?;
        // Exporters send in bursts when their flow cache expires; ask for room to queue them.
        let _ = socket2::SockRef::from(&socket).set_recv_buffer_size(4 * 1024 * 1024);
        let local_addr = socket.local_addr()?;
        let (batches, receiver) = unbounded_channel();
        let stats = Arc::new(Mutex::new(DecoderStats::default()));
        let task = tokio::spawn(collect(socket, batches, stats.clone()));
        Ok((
            FlowCollector {
                local_addr,
                stats,
                task: Some(task),
            },
            receiver,
        ))
    }

    /// The address the collector listens on (useful after binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// What the collector's decoder has seen so far.
    pub fn stats(&self) -> DecoderStats {
        *self.stats.lock().unwrap_or_else(PoisonError::into_inner)
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

/// Receives and decodes datagrams until aborted, or until nobody listens to the records.
async fn collect(
    socket: UdpSocket,
    batches: UnboundedSender<Vec<FlowRecord>>,
    stats: Arc<Mutex<DecoderStats>>,
) {
    let mut decoder = Decoder::new();
    let mut datagram = vec![0u8; 65_536];
    loop {
        let (length, exporter) = match socket.recv_from(&mut datagram).await {
            Ok(received) => received,
            Err(error) => {
                tracing::debug!(%error, "flow receive failed");
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        let decoded = decoder.decode(&datagram[..length], exporter);
        *stats.lock().unwrap_or_else(PoisonError::into_inner) = decoder.stats();
        match decoded {
            Ok(records) if records.is_empty() => {}
            Ok(records) => {
                if batches.send(records).is_err() {
                    return;
                }
            }
            Err(error) => tracing::debug!(%exporter, %error, "flow datagram rejected"),
        }
    }
}

#[cfg(test)]
mod tests;
