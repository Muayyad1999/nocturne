//! The SNMP trap receiver: one UDP socket, the bounded queue behind it and
//! the task that writes what it hands over.
//!
//! A datagram goes through these, in this order:
//!
//! 1. **Decode** (`nocturne_snmp::decode_trap`). What is not a notification
//!    is counted as malformed. An SNMPv3 notification that is authenticated
//!    or encrypted cannot be verified here — that needs the sender's user
//!    and keys — so it is counted as unauthenticated and **not kept**: a
//!    row would say "this switch reported its link down" on the word of
//!    whoever sent the datagram.
//! 2. **Attribute** by source address, with the index every receiver uses.
//! 3. **Check the community** ([`admit`]) against the one the vault holds
//!    for that device, and forget it. From there on nothing has it.
//! 4. **Queue**, bounded: a storm is dropped and counted, never buffered
//!    without limit. The check comes before the queue so that a flood of
//!    forged traps is turned away at the door and cannot push real ones
//!    out of it.
//! 5. **Write** ([`render`] → `ingest::commit`): the same transaction, tail
//!    and rules a syslog line gets.
//! 6. **Acknowledge** an inform — after it is stored, not before, so that
//!    a sender which keeps an inform until it is acknowledged loses
//!    nothing to a full queue. A retransmission of one already stored is
//!    acknowledged again and stored once.
//!
//! SNMPv1 and v2c are unauthenticated UDP. A community is a shared secret
//! sent in the clear, and a source address can be forged by anyone on the
//! path, so a stored trap is a claim, not a fact — which is why nothing
//! derived from one ever changes a device.

pub mod admit;
pub mod render;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use nocturne_snmp::{Oid, SnmpError, Trap, VarBind};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{Receiver, Sender, channel, error::TrySendError};
use tokio_util::sync::CancellationToken;

use crate::budget::Held;
use crate::ingest::INDEX_MAX_AGE_MS;
use crate::settings::LogsSettings;
use crate::state::{BATCH_ROWS, LogsState, TrapBinding, TrapState};
use crate::{db, ingest, status};

/// The largest datagram UDP can carry.
const MAX_DATAGRAM: usize = 65_535;
/// How long an inform is remembered, so that its retransmissions are
/// recognised. Longer than a sender goes on retrying one: net-snmp gives up
/// after six seconds by default, IOS after about two minutes.
const INFORM_MEMORY_MS: i64 = 300_000;
/// How many informs are remembered at once. Past that a retransmission is
/// not recognised and is stored again — a duplicate row, not a lost one.
const MAX_REMEMBERED_INFORMS: usize = 4_096;
/// How long the monitor's interface map of a device is used before it is
/// asked for again.
const INTERFACES_MAX_AGE_MS: i64 = 60_000;

/// A notification that was decoded and let in. It has no community: the
/// structure the decoder returns does, and this one is built from it field
/// by field so that nothing downstream can store, publish or log one.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    /// 1, 2 (SNMPv2c) or 3.
    pub version: u8,
    /// The address the datagram came from, without the port.
    pub source: String,
    pub device_id: Option<String>,
    /// Why no device was attributed (empty when one was).
    pub reason: String,
    pub received_at: i64,
    pub uptime_ticks: Option<u32>,
    pub trap_oid: Option<Oid>,
    pub enterprise: Option<Oid>,
    pub agent_addr: Option<[u8; 4]>,
    pub generic: Option<u32>,
    pub specific: Option<u32>,
    /// The first [`render::MAX_READ_VARBINDS`] bindings. A row never shows
    /// more, and a datagram can be six thousand of them — each several
    /// times its size on the wire once decoded, for every place in the
    /// queue.
    pub varbinds: Vec<VarBind>,
    /// How many bindings the datagram held, so that the ones not kept are
    /// counted.
    pub varbinds_total: usize,
    pub inform: bool,
}

/// What identifies one inform: who sent it, and its bytes. A sender repeats
/// an inform byte for byte, request id included, until it is answered.
type InformKey = (SocketAddr, i64);

/// The answer an inform is owed once it is stored.
pub struct Ack {
    to: SocketAddr,
    /// The Response PDU. It echoes the inform's community, as the protocol
    /// requires, so it goes back to where the inform came from and nowhere
    /// else.
    datagram: Vec<u8>,
    key: InformKey,
}

/// One notification on its way to the writer.
pub struct Queued {
    notification: Notification,
    ack: Option<Ack>,
    /// What it takes up of the receiver's budget, given back once it is
    /// written (or is not).
    held: Option<Held>,
}

impl Queued {
    /// What it takes up: itself, what the notification holds, and the
    /// answer an inform is owed, which is as large as the inform.
    fn weight(&self) -> usize {
        use std::mem::size_of;
        let oid = |oid: &Oid| oid.len() * size_of::<u32>();
        let notification = &self.notification;
        let bindings: usize = notification
            .varbinds
            .iter()
            .map(|binding| {
                oid(&binding.oid)
                    + match &binding.value {
                        nocturne_snmp::Value::OctetString(bytes)
                        | nocturne_snmp::Value::Opaque(bytes) => bytes.capacity(),
                        nocturne_snmp::Value::Oid(value) => oid(value),
                        _ => 0,
                    }
            })
            .sum();
        size_of::<Queued>()
            + notification.source.capacity()
            + notification.device_id.as_ref().map_or(0, String::capacity)
            + notification.reason.capacity()
            + notification.trap_oid.as_ref().map_or(0, oid)
            + notification.enterprise.as_ref().map_or(0, oid)
            + notification.varbinds.capacity() * size_of::<VarBind>()
            + bindings
            + self.ack.as_ref().map_or(0, |ack| ack.datagram.capacity())
    }
}

/// The informs seen lately: waiting to be written, or stored.
#[derive(Default)]
pub struct Informs {
    /// Whether it is stored yet, and when it was last heard.
    seen: HashMap<InformKey, (bool, i64)>,
    pruned_at: i64,
}

impl Informs {
    /// `Some(stored)` when this inform was seen before.
    fn state(&self, key: &InformKey, now: i64) -> Option<bool> {
        self.seen
            .get(key)
            .filter(|(_, at)| now - at < INFORM_MEMORY_MS)
            .map(|(stored, _)| *stored)
    }

    fn remember(&mut self, key: InformKey, stored: bool, now: i64) {
        if self.seen.len() >= MAX_REMEMBERED_INFORMS && !self.seen.contains_key(&key) {
            // At most once a second: a flood of distinct informs must not
            // turn every datagram into a walk over the whole map.
            if now - self.pruned_at >= 1_000 {
                self.pruned_at = now;
                self.seen.retain(|_, (_, at)| now - *at < INFORM_MEMORY_MS);
            }
            if self.seen.len() >= MAX_REMEMBERED_INFORMS {
                return;
            }
        }
        self.seen.insert(key, (stored, now));
    }

    fn forget(&mut self, key: &InformKey) {
        self.seen.remove(key);
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// What the monitor knows of each device's interfaces by ifIndex, for a
/// short while. `None` is "the monitor has no map for it" (or is not in
/// this build), remembered so that a device without one is not asked about
/// for every trap.
#[derive(Default)]
pub struct InterfaceNames {
    by_device: HashMap<String, (i64, Option<Arc<HashMap<u32, String>>>)>,
}

impl InterfaceNames {
    pub fn clear(&mut self) {
        self.by_device.clear();
    }
}

/// Why a socket could not be bound, with what to do about it.
fn why(address: SocketAddr, err: &std::io::Error) -> String {
    use std::io::ErrorKind;
    let hint = match err.kind() {
        ErrorKind::PermissionDenied => {
            ": a port below 1024 needs privileges this application does not ask for. \
             Use a port such as 1162 and tell the devices to send there"
        }
        ErrorKind::AddrInUse => {
            ": something else is already listening there (an `snmptrapd`, perhaps)"
        }
        ErrorKind::AddrNotAvailable => {
            ": that address is not on this machine (0.0.0.0 listens on every interface)"
        }
        _ => "",
    };
    format!("cannot listen for traps on udp {address}: {err}{hint}")
}

/// Stop whatever is running, then start what the settings ask for.
///
/// Never returns an error for a port that cannot be bound: that is a state
/// of the receiver, not a failure of the call that tried to start it.
pub async fn apply(app: &App) -> AppResult<()> {
    let state = app.state::<LogsState>();
    let traps = &state.traps;
    // One at a time: two calls that each stop the old socket and bind a new
    // one would find the port taken by each other. The settings are read
    // once it is this call's turn, so the last call applies the latest.
    let _turn = traps.applying.lock().await;
    let settings = LogsSettings::load(app);

    // Neither is the socket's: applied to the running receiver.
    traps.budget.set_limit(settings.queue_limit_bytes as usize);
    // Read for every datagram, so switching it needs no new socket — and
    // loses no trap that arrives while one would be rebound.
    let required = settings.traps.require_known_community;
    let changed = traps
        .require_known_community
        .swap(required, Ordering::Relaxed)
        != required;

    let wanted = TrapBinding {
        enabled: settings.traps.enabled,
        port: settings.traps.port,
        bind: settings.traps.bind.trim().to_string(),
    };
    let previous = {
        let mut receiver = traps.receiver.lock();
        // Nothing to do when the receiver is already running exactly this:
        // rebinding would change the port under whoever is sending. One
        // that could not start is tried again — whatever held the port may
        // have gone, and `logs.restart` is how a person says so.
        if receiver.applied.as_ref() == Some(&wanted)
            && (receiver.task.is_some() || !wanted.enabled)
        {
            drop(receiver);
            if changed {
                status::publish(app);
            }
            return Ok(());
        }
        receiver.applied = Some(wanted.clone());
        receiver.enabled = wanted.enabled;
        receiver.listening.clear();
        receiver.error = None;
        if let Some(stop) = receiver.stop.take() {
            stop.cancel();
        }
        receiver.task.take()
    };
    *traps.queue.lock() = None;
    if let Some(previous) = previous {
        // The old socket is closed before the new one is bound: the port
        // may well be the same one.
        let _ = previous.await;
    }

    if !wanted.enabled {
        status::publish(app);
        return Ok(());
    }
    let address: IpAddr = match wanted.bind.parse() {
        Ok(address) => address,
        Err(_) => {
            fail(
                app,
                format!(
                    "`{}` is not an address this machine can listen on (try 0.0.0.0 for every interface)",
                    wanted.bind
                ),
            );
            return Ok(());
        }
    };
    let address = SocketAddr::new(address, wanted.port);
    let socket = match UdpSocket::bind(address).await {
        Ok(socket) => Arc::new(socket),
        Err(err) => {
            fail(app, why(address, &err));
            return Ok(());
        }
    };
    let listening = match socket.local_addr() {
        Ok(bound) => vec![format!("udp {bound}")],
        Err(err) => {
            fail(app, why(address, &err));
            return Ok(());
        }
    };

    let limit = settings.queue_limit as usize;
    let (queue, waiting) = channel(limit);
    let stop = CancellationToken::new();
    let task = tokio::spawn(listen(
        app.clone(),
        socket.clone(),
        queue.clone(),
        stop.clone(),
    ));
    {
        let mut receiver = traps.receiver.lock();
        receiver.listening = listening.clone();
        receiver.queue_limit = limit;
        receiver.stop = Some(stop);
        receiver.task = Some(task);
    }
    *traps.queue.lock() = Some(queue);
    // The writer answers informs through the socket but must not keep it
    // open: only the listener does, so that stopping the listener closes
    // the port.
    app.spawn(
        "logs.traps.writer",
        writer(app.clone(), waiting, Arc::downgrade(&socket)),
    );
    app.audit(
        "logs.receiver.start",
        None,
        format!("SNMP trap receiver listening on {}", listening.join(", ")),
        serde_json::json!({"receiver": "traps", "listening": listening}),
    );
    status::publish(app);
    Ok(())
}

fn fail(app: &App, error: String) {
    tracing::warn!(%error, "the trap receiver could not start");
    app.state::<LogsState>().traps.receiver.lock().error = Some(error);
    status::publish(app);
}

/// The socket task: every datagram is decoded, checked and handed to the
/// writer — or counted under the reason it was not.
async fn listen(app: App, socket: Arc<UdpSocket>, queue: Sender<Queued>, stop: CancellationToken) {
    let state = app.state::<LogsState>();
    let shutdown = app.shutdown_token();
    let mut datagram = vec![0u8; MAX_DATAGRAM];
    loop {
        let received = tokio::select! {
            _ = stop.cancelled() => return,
            _ = shutdown.cancelled() => return,
            received = socket.recv_from(&mut datagram) => received,
        };
        let (length, peer) = match received {
            Ok(received) => received,
            Err(err) => {
                // Not a reason to stop listening: on some platforms an
                // acknowledgement that bounced off a closed port comes back
                // as an error on the next read. The pause keeps a socket
                // that is truly broken from spinning.
                tracing::debug!(kind = ?err.kind(), "the trap socket reported an error");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Some(mut queued) = take(&app, &state.traps, &socket, &datagram[..length], peer) else {
            continue;
        };
        // What does not fit in bytes is dropped like what does not fit in
        // number: a datagram can be 64 KiB, and an inform's answer as much
        // again.
        queued.held = state.traps.budget.take(queued.weight());
        let (refused, closed) = match queued.held {
            None => (Some(queued), false),
            Some(_) => match queue.try_send(queued) {
                Ok(()) => (None, false),
                Err(TrySendError::Full(refused)) => (Some(refused), false),
                // Nobody writes any more (the application is stopping): what
                // was taken off the socket is counted all the same.
                Err(TrySendError::Closed(refused)) => (Some(refused), true),
            },
        };
        if let Some(refused) = refused {
            state.traps.counters.dropped(1);
            if let Some(ack) = refused.ack {
                // Not stored, so not acknowledged: the sender's next
                // attempt is a new one.
                state.traps.informs.lock().forget(&ack.key);
            }
        }
        if closed {
            return;
        }
    }
}

/// Decide what one datagram is. Counts it under whatever it turns out to
/// be, and returns it only when it is to be written.
fn take(
    app: &App,
    traps: &TrapState,
    socket: &UdpSocket,
    datagram: &[u8],
    peer: SocketAddr,
) -> Option<Queued> {
    let now = now_ms();
    traps.counters.received(now);
    traps.rate.lock().record(now, 1);

    let trap = match nocturne_snmp::decode_trap(datagram, peer) {
        Ok(trap) => trap,
        // SNMPv3 with authentication or privacy: what it says may be true
        // and cannot be checked here. Counted, and not kept as if it had
        // been.
        Err(SnmpError::Unsupported(_)) => {
            traps.unauthenticated.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        // Not a notification. The error is not logged: at this point it is
        // the only thing that knows what was in the datagram.
        Err(_) => {
            traps.malformed.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    // A socket bound to `::` reports an IPv4 sender as `::ffff:a.b.c.d`;
    // the inventory knows it as `a.b.c.d`.
    let source = peer.ip().to_canonical();
    let index = match crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS) {
        Ok(index) => index,
        Err(err) => {
            tracing::error!(%err, "the inventory could not be read to attribute a trap");
            traps.counters.dropped(1);
            return None;
        }
    };
    let attribution = index.attribute(&source.to_string());
    let Trap {
        version,
        community,
        source: _,
        uptime_ticks,
        trap_oid,
        enterprise,
        agent_addr,
        generic,
        specific,
        mut varbinds,
        is_inform,
    } = trap;

    if traps.require_known_community.load(Ordering::Relaxed) {
        let device = attribution
            .device_id
            .as_deref()
            .map(|id| (id, index.name_of(id)));
        let allowed = admit::check(
            app,
            &traps.known,
            device.as_ref().map(|(id, name)| (*id, name.as_str())),
            &attribution.reason,
            version,
            &community,
            now,
        );
        if let Err(reason) = allowed {
            traps.rejected.fetch_add(1, Ordering::Relaxed);
            traps
                .rejections
                .lock()
                .record(source, attribution.device_id.as_deref(), reason, now);
            return None;
        }
    }
    // Only what a row can show is kept, and how many there were.
    let varbinds_total = varbinds.len();
    varbinds.truncate(render::MAX_READ_VARBINDS);
    varbinds.shrink_to_fit();
    // The last use of the community: it is cut out of the bindings (a
    // forwarded trap repeats it there), and then it is gone.
    admit::scrub(&mut varbinds, community.as_bytes());
    drop(community);

    // Only an SNMPv2c inform can be answered; an SNMPv3 one would have to
    // be answered as an authoritative engine, which this is not.
    let ack = if is_inform && version == 2 {
        let key: InformKey = (peer, db::fingerprint_bytes(datagram));
        let seen = traps.informs.lock().state(&key, now);
        match seen {
            Some(stored) => {
                // The same inform again: its acknowledgement was lost, or
                // is not sent yet. Stored once either way.
                traps.duplicates.fetch_add(1, Ordering::Relaxed);
                if stored
                    && let Ok(response) = nocturne_snmp::inform_response(datagram)
                    && socket.try_send_to(&response, peer).is_ok()
                {
                    traps.acknowledged.fetch_add(1, Ordering::Relaxed);
                }
                return None;
            }
            None => nocturne_snmp::inform_response(datagram)
                .ok()
                .map(|response| {
                    traps.informs.lock().remember(key, false, now);
                    Ack {
                        to: peer,
                        datagram: response,
                        key,
                    }
                }),
        }
    } else {
        None
    };

    Some(Queued {
        notification: Notification {
            version,
            source: source.to_string(),
            device_id: attribution.device_id,
            reason: attribution.reason,
            received_at: now,
            uptime_ticks,
            trap_oid,
            enterprise,
            agent_addr,
            generic,
            specific,
            varbinds,
            varbinds_total,
            inform: is_inform,
        },
        ack,
        held: None,
    })
}

/// The writer task: everything the socket hands over, in batches.
///
/// It ends when its queue is closed and empty — that is, after the receiver
/// it belongs to has been stopped *and* everything that receiver had
/// already taken off the socket is written. A receiver that is switched
/// off or rebound does not get to throw away what it had received.
async fn writer(app: App, mut queue: Receiver<Queued>, socket: Weak<UdpSocket>) {
    let state = app.state::<LogsState>();
    let mut batch = Vec::with_capacity(BATCH_ROWS);
    // `recv_many` returns as soon as there is anything at all, so a single
    // trap is written straight away and a storm arrives in batches without
    // a timer.
    while queue.recv_many(&mut batch, BATCH_ROWS).await > 0 {
        store(&app, &state.traps, &socket, std::mem::take(&mut batch)).await;
    }
}

/// The monitor's interface names of one device, when it has them.
async fn interfaces_of(
    app: &App,
    traps: &TrapState,
    device_id: &str,
    now: i64,
) -> Option<Arc<HashMap<u32, String>>> {
    if let Some((at, names)) = traps.interfaces.lock().by_device.get(device_id)
        && now - at < INTERFACES_MAX_AGE_MS
    {
        return names.clone();
    }
    // The monitoring feature is reached by name. It may not be in this
    // build, or may never have polled this device: either way the answer
    // is "no names", and an interface is shown by its number.
    let names = app
        .rpc(
            "monitor.ifindex",
            serde_json::json!({"device_id": device_id}),
        )
        .await
        .ok()
        .map(|answer| render::interface_map(&answer))
        .filter(|names| !names.is_empty())
        .map(Arc::new);
    traps
        .interfaces
        .lock()
        .by_device
        .insert(device_id.to_string(), (now, names.clone()));
    names
}

/// The monitor's interface names of one device by ifIndex, for whoever
/// else has an ifIndex to name (the flow collector's interface rows). The
/// same answer, remembered for the same minute, as a trap is named from.
pub(crate) async fn interface_names(
    app: &App,
    device_id: &str,
    now: i64,
) -> Option<Arc<HashMap<u32, String>>> {
    let state = app.state::<LogsState>();
    interfaces_of(app, &state.traps, device_id, now).await
}

/// Write one batch: name what can be named, [`ingest::commit`], then
/// acknowledge the informs that were kept.
async fn store(app: &App, traps: &TrapState, socket: &Weak<UdpSocket>, batch: Vec<Queued>) {
    let state = app.state::<LogsState>();
    let now = now_ms();
    let forget = |acks: Vec<(Ack, bool)>| {
        let mut informs = traps.informs.lock();
        for (ack, _) in acks {
            informs.forget(&ack.key);
        }
    };
    let mut rows = Vec::with_capacity(batch.len());
    let mut acks = Vec::new();
    // Given back when this returns, whichever way: once the batch is
    // written, or once it is known that it will not be.
    let mut holding = Vec::with_capacity(batch.len());
    let index = match crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS) {
        Ok(index) => index,
        Err(err) => {
            tracing::error!(%err, count = batch.len(), "a batch of traps could not be stored");
            // Taken off the socket and not kept: that is what `dropped`
            // means, and a datagram that is in no count at all would make
            // the others add up to less than was received.
            traps.counters.dropped(batch.len() as u64);
            forget(
                batch
                    .into_iter()
                    .filter_map(|queued| Some((queued.ack?, false)))
                    .collect(),
            );
            return;
        }
    };
    let settings = LogsSettings::load(app);
    for mut queued in batch {
        holding.push(queued.held.take());
        let notification = &queued.notification;
        let interfaces = match &notification.device_id {
            Some(device_id) if render::names_an_interface(&notification.varbinds) => {
                interfaces_of(app, traps, device_id, now).await
            }
            _ => None,
        };
        let row = render::prepare(&state, &index, notification, interfaces.as_deref());
        if let Some(ack) = queued.ack {
            acks.push((ack, ingest::keeps(&settings, &row)));
        }
        rows.push(row);
    }
    let count = rows.len();
    if let Err(unwritten) = ingest::commit(app, &index, &traps.counters, rows) {
        tracing::error!(err = %unwritten.error, count, "a batch of traps could not be stored");
        // The rows `commit` turned away for their source it has counted
        // itself; these are the ones the failed write lost.
        traps.counters.dropped(unwritten.rows as u64);
        forget(acks);
        return;
    }
    for (ack, kept) in acks {
        if !kept {
            // From an unknown source, with the setting that drops those: it
            // was not stored, so it is not acknowledged.
            traps.informs.lock().forget(&ack.key);
            continue;
        }
        // Stored, whether or not the answer gets through: if it does not,
        // the sender repeats the inform and is answered then.
        traps.informs.lock().remember(ack.key, true, now);
        let Some(socket) = socket.upgrade() else {
            continue;
        };
        if socket.send_to(&ack.datagram, ack.to).await.is_ok() {
            traps.acknowledged.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Forget what was learnt from the vault and the monitor: a credential, a
/// device or the vault itself changed.
pub fn forget(app: &App) {
    let state = app.state::<LogsState>();
    state.traps.known.lock().clear();
    state.traps.interfaces.lock().clear();
}

/// The address the receiver is listening on, for tests and for the
/// "point your devices here" hint.
pub fn listening(app: &App) -> Vec<String> {
    app.state::<LogsState>()
        .traps
        .receiver
        .lock()
        .listening
        .clone()
}

/// The UDP port actually bound, which is not the configured one when the
/// configured one was 0.
pub fn bound_port(app: &App) -> AppResult<u16> {
    listening(app)
        .iter()
        .find_map(|entry| {
            entry
                .strip_prefix("udp")?
                .trim()
                .parse::<SocketAddr>()
                .ok()
                .map(|address| address.port())
        })
        .ok_or_else(|| AppError::unavailable("the trap receiver is not listening"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(port: u16, fingerprint: i64) -> InformKey {
        (
            SocketAddr::new("192.0.2.7".parse().unwrap(), port),
            fingerprint,
        )
    }

    #[test]
    fn an_inform_is_known_while_it_waits_and_once_it_is_stored() {
        let mut informs = Informs::default();
        assert_eq!(informs.state(&key(4000, 1), 1_000), None);
        informs.remember(key(4000, 1), false, 1_000);
        assert_eq!(informs.state(&key(4000, 1), 1_500), Some(false));
        informs.remember(key(4000, 1), true, 1_600);
        assert_eq!(informs.state(&key(4000, 1), 2_000), Some(true));
        assert_eq!(
            informs.state(&key(4000, 2), 2_000),
            None,
            "another request from the same sender is another inform"
        );
        assert_eq!(
            informs.state(&key(4001, 1), 2_000),
            None,
            "and the same bytes from another port are another sender"
        );
        assert_eq!(
            informs.state(&key(4000, 1), 1_600 + INFORM_MEMORY_MS),
            None,
            "long after a sender has stopped retrying, the same bytes are new"
        );
        informs.forget(&key(4000, 1));
        assert!(informs.is_empty());
    }

    /// BER by hand (X.690), for a datagram no tool sends: an SNMPv2c trap
    /// with `count` bindings of a null each, named 1.3.0, 1.3.1, …
    fn stuffed(count: u32) -> Vec<u8> {
        fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
            let mut out = vec![tag];
            match content.len() {
                length @ 0..=127 => out.push(length as u8),
                length @ 128..=255 => out.extend([0x81, length as u8]),
                length => out.extend([0x82, (length >> 8) as u8, length as u8]),
            }
            out.extend_from_slice(content);
            out
        }
        let mut list = Vec::new();
        for index in 0..count {
            // 1.3 is one byte; the last arc is base 128, most significant first.
            let mut name = vec![0x2b];
            if index >= 128 * 128 {
                name.push((index >> 14) as u8 | 0x80);
            }
            if index >= 128 {
                name.push(((index >> 7) & 0x7f) as u8 | 0x80);
            }
            name.push((index & 0x7f) as u8);
            let mut bind = tlv(0x06, &name);
            bind.extend(tlv(0x05, &[]));
            list.extend(tlv(0x30, &bind));
        }
        let mut pdu = Vec::new();
        for number in [7u8, 0, 0] {
            pdu.extend(tlv(0x02, &[number]));
        }
        pdu.extend(tlv(0x30, &list));
        let mut message = tlv(0x02, &[1]);
        message.extend(tlv(0x04, b"any"));
        message.extend(tlv(0xa7, &pdu));
        tlv(0x30, &message)
    }

    #[tokio::test]
    async fn of_a_datagram_of_thousands_of_bindings_only_what_a_row_can_show_waits_in_the_queue() {
        let mut builder = nocturne_app::testing::builder();
        crate::init(&mut builder);
        let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
        let app = &lab.app;
        app.rpc(
            "devices.update",
            serde_json::json!({"id": lab.id("SW1"), "address": "127.0.0.1"}),
        )
        .await
        .unwrap();
        let state = app.state::<LogsState>();
        // Not what is tested here: the community is checked in `admit`.
        state
            .traps
            .require_known_community
            .store(false, Ordering::Relaxed);
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();

        let datagram = stuffed(6_000);
        assert!(datagram.len() < MAX_DATAGRAM);
        let decoded = nocturne_snmp::decode_trap(&datagram, peer).unwrap();
        assert_eq!(decoded.varbinds.len(), 6_000, "the decoder reads them all");

        let queued = take(app, &state.traps, &socket, &datagram, peer).expect("it is let in");
        assert_eq!(
            queued.notification.varbinds.len(),
            render::MAX_READ_VARBINDS,
            "what waits in the queue is what a row can show, not the datagram decoded"
        );
        assert_eq!(queued.notification.varbinds_total, 6_000);
        assert_eq!(queued.notification.varbinds[127].oid.to_string(), "1.3.127");

        // An ordinary trap keeps everything it has.
        let ordinary = take(app, &state.traps, &socket, &stuffed(3), peer).expect("let in");
        assert_eq!(
            (
                ordinary.notification.varbinds.len(),
                ordinary.notification.varbinds_total
            ),
            (3, 3)
        );
    }

    #[test]
    fn a_flood_of_informs_is_remembered_only_so_far() {
        let mut informs = Informs::default();
        for index in 0..(MAX_REMEMBERED_INFORMS as i64 * 3) {
            informs.remember(key(4000, index), false, 1_000 + index);
        }
        assert_eq!(informs.len(), MAX_REMEMBERED_INFORMS);
        // Once the old ones have aged out there is room again.
        let later = 1_000 + MAX_REMEMBERED_INFORMS as i64 * 3 + INFORM_MEMORY_MS;
        informs.remember(key(5000, 1), false, later);
        assert_eq!(informs.state(&key(5000, 1), later), Some(false));
        assert!(informs.len() <= MAX_REMEMBERED_INFORMS);
    }
}
