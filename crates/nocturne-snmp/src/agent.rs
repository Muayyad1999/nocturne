//! A small SNMP agent (command responder) for SNMPv1 and SNMPv2c.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::ops::Bound;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use tokio::net::UdpSocket;
use tracing::warn;

use crate::ber::{self, Writer};
use crate::error::status;
use crate::pdu::{
    AnyPdu, CommunityMessage, CommunityVersion, Pdu, PduKind, encode_community_message,
    write_varbind,
};
use crate::{Oid, SnmpError, Value, VarBind};

/// The largest UDP payload over IPv4, and the default response size limit.
const MAX_UDP_PAYLOAD: usize = 65_507;

/// Slack for the length octets of the enclosing SEQUENCEs when estimating a response's size.
const FRAMING_SLACK: usize = 12;

type View = BTreeMap<Oid, Value>;

/// Serves v1/v2c GET/GETNEXT/GETBULK from an ordered map. Used by Nocturne to expose simulated
/// lab devices to SNMP, and by this crate's own tests so they run without an external daemon.
///
/// `Agent` is a cheap handle (`Clone`): keep one to update the view with
/// [`set_view`](Self::set_view) while another is consumed by [`run`](Self::run).
///
/// ```no_run
/// # async fn demo() -> Result<(), nocturne_snmp::SnmpError> {
/// use nocturne_snmp::{Agent, Value};
/// let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), "public").await?;
/// agent.set_view([("1.3.6.1.2.1.1.5.0".parse()?, Value::from("lab-sw1"))].into());
/// let server = tokio::spawn(agent.clone().run());
/// // ... later, atomically swap in new counters:
/// agent.set_view(Default::default());
/// server.abort();
/// # Ok(()) }
/// ```
///
/// SET requests are refused (`notWritable`; `noSuchName` for SNMPv1), requests with the wrong
/// community and anything that is not a v1/v2c request are dropped without an answer.
#[derive(Clone)]
pub struct Agent {
    inner: Arc<Inner>,
}

struct Inner {
    socket: UdpSocket,
    local_addr: SocketAddr,
    community: Vec<u8>,
    view: RwLock<Arc<View>>,
    max_message_size: AtomicUsize,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("local_addr", &self.inner.local_addr)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Binds the agent's UDP socket (port 0 picks a free port; see
    /// [`local_addr`](Self::local_addr)). The view starts out empty.
    pub async fn bind(addr: SocketAddr, community: &str) -> Result<Agent, SnmpError> {
        let socket = UdpSocket::bind(addr).await?;
        let local_addr = socket.local_addr()?;
        Ok(Agent {
            inner: Arc::new(Inner {
                socket,
                local_addr,
                community: community.as_bytes().to_vec(),
                view: RwLock::new(Arc::new(View::new())),
                max_message_size: AtomicUsize::new(MAX_UDP_PAYLOAD),
            }),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// Replace the whole MIB view atomically.
    ///
    /// Every request is answered from one consistent snapshot. Entries that cannot be served
    /// — OIDs BER cannot encode or with fewer than two arcs, exception markers as values —
    /// are left out.
    pub fn set_view(&self, mut view: BTreeMap<Oid, Value>) {
        view.retain(|oid, value| {
            let servable = oid.len() >= 2
                && ber::check_oid(oid).is_ok()
                && !value.is_exception()
                && value
                    .as_oid()
                    .is_none_or(|inner| ber::check_oid(inner).is_ok());
            if !servable {
                warn!(%oid, "dropping an entry that cannot be served over SNMP");
            }
            servable
        });
        let view = Arc::new(view);
        *self
            .inner
            .view
            .write()
            .unwrap_or_else(PoisonError::into_inner) = view;
    }

    /// Limits the size of response messages, like the packet-size setting of real agents
    /// (default and maximum: 65507 octets, the largest UDP payload).
    ///
    /// A GET/GETNEXT response that does not fit is answered with `tooBig`; a GETBULK
    /// response is cut short to fit, as RFC 3416 prescribes.
    pub fn set_max_message_size(&self, octets: usize) {
        self.inner
            .max_message_size
            .store(octets.min(MAX_UDP_PAYLOAD), Ordering::Relaxed);
    }

    /// Serve until dropped/aborted. Only a failure of the socket itself ends the loop with
    /// an error; malformed and unauthorised datagrams are ignored.
    pub async fn run(self) -> Result<(), SnmpError> {
        let mut buf = vec![0u8; 65_535];
        loop {
            let (len, peer) = match self.inner.socket.recv_from(&mut buf).await {
                Ok(received) => received,
                // ICMP errors for replies to managers that have gone away.
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if let Some(response) = self.inner.respond(&buf[..len]) {
                // A reply that cannot be delivered is no reason to stop serving.
                let _ = self.inner.socket.send_to(&response, peer).await;
            }
        }
    }
}

impl Inner {
    /// The response datagram for a request datagram, if it deserves one.
    fn respond(&self, datagram: &[u8]) -> Option<Vec<u8>> {
        let message = CommunityMessage::decode(datagram).ok()?;
        if message.community != self.community {
            return None;
        }
        let AnyPdu::Standard(request) = message.pdu else {
            return None;
        };
        let view = Arc::clone(&self.view.read().unwrap_or_else(PoisonError::into_inner));
        let responder = Responder {
            view: &view,
            version: message.version,
            community: &self.community,
            max_size: self.max_message_size.load(Ordering::Relaxed),
        };
        match (request.kind, message.version) {
            (PduKind::Get, _) => Some(responder.get(&request)),
            (PduKind::GetNext, _) => Some(responder.get_next(&request)),
            (PduKind::GetBulk, CommunityVersion::V2c) => Some(responder.get_bulk(&request)),
            (PduKind::Set, _) => Some(responder.refuse_set(&request)),
            // GETBULK inside an SNMPv1 message, responses, traps, informs: nothing to answer.
            _ => None,
        }
    }
}

/// Answers one request from one snapshot of the view.
struct Responder<'a> {
    view: &'a View,
    version: CommunityVersion,
    community: &'a [u8],
    max_size: usize,
}

impl Responder<'_> {
    /// SNMPv1 has no Counter64, so for a v1 manager such objects do not exist
    /// (RFC 3584 §4.2.2.1).
    fn visible(&self, value: &Value) -> bool {
        self.version == CommunityVersion::V2c || !matches!(value, Value::Counter64(_))
    }

    fn exact(&self, oid: &Oid) -> Option<&Value> {
        self.view.get(oid).filter(|value| self.visible(value))
    }

    fn next(&self, oid: &Oid) -> Option<(&Oid, &Value)> {
        self.view
            .range::<Oid, _>((Bound::Excluded(oid), Bound::Unbounded))
            .find(|(_, value)| self.visible(value))
    }

    /// The exception for a missing instance. Without MIB knowledge the best available rule
    /// is: if other instances exist next to it, the object is there and only this instance is
    /// missing.
    fn missing(&self, oid: &Oid) -> Value {
        let has_siblings = oid.parent().is_some_and(|parent| {
            self.view
                .range::<Oid, _>((Bound::Included(&parent), Bound::Unbounded))
                .next()
                .is_some_and(|(sibling, _)| sibling.starts_with(&parent))
        });
        if has_siblings {
            Value::NoSuchInstance
        } else {
            Value::NoSuchObject
        }
    }

    fn next_or_end(&self, oid: &Oid) -> VarBind {
        match self.next(oid) {
            Some((next, value)) => VarBind::new(next.clone(), value.clone()),
            None => VarBind::new(oid.clone(), Value::EndOfMibView),
        }
    }

    fn get(&self, request: &Pdu) -> Vec<u8> {
        let mut binds = Vec::with_capacity(request.varbinds.len());
        for (position, bind) in request.varbinds.iter().enumerate() {
            let value = match (self.exact(&bind.oid), self.version) {
                (Some(value), _) => value.clone(),
                (None, CommunityVersion::V2c) => self.missing(&bind.oid),
                (None, CommunityVersion::V1) => {
                    return self.error(request, status::NO_SUCH_NAME, position + 1);
                }
            };
            binds.push(VarBind::new(bind.oid.clone(), value));
        }
        self.reply(request, binds)
    }

    fn get_next(&self, request: &Pdu) -> Vec<u8> {
        let mut binds = Vec::with_capacity(request.varbinds.len());
        for (position, bind) in request.varbinds.iter().enumerate() {
            let next = self.next_or_end(&bind.oid);
            if next.value == Value::EndOfMibView && self.version == CommunityVersion::V1 {
                return self.error(request, status::NO_SUCH_NAME, position + 1);
            }
            binds.push(next);
        }
        self.reply(request, binds)
    }

    /// RFC 3416 §4.2.3: one GETNEXT for each of the first N bindings, then up to M rounds of
    /// GETNEXT over the remaining ones, stopping early when the message is full or a whole
    /// round came back as endOfMibView.
    fn get_bulk(&self, request: &Pdu) -> Vec<u8> {
        let non_repeaters = (request.error_status as usize).min(request.varbinds.len());
        let max_repetitions = request.error_index;
        let (singles, repeaters) = request.varbinds.split_at(non_repeaters);

        let budget = self
            .max_size
            .saturating_sub(self.empty_reply_len(request) + FRAMING_SLACK);
        let mut used = 0usize;
        let mut binds = Vec::new();
        // Adds a binding unless the message is full.
        let mut push = |bind: VarBind| {
            used += encoded_len(&bind);
            let fits = used <= budget;
            if fits {
                binds.push(bind);
            }
            fits
        };

        let mut full = false;
        for bind in singles {
            if !push(self.next_or_end(&bind.oid)) {
                full = true;
                break;
            }
        }
        let mut cursors: Vec<Oid> = repeaters.iter().map(|bind| bind.oid.clone()).collect();
        let mut round = 0;
        while !full && !cursors.is_empty() && round < max_repetitions {
            round += 1;
            let mut all_ended = true;
            for cursor in &mut cursors {
                let next = self.next_or_end(cursor);
                all_ended &= next.value == Value::EndOfMibView;
                cursor.clone_from(&next.oid);
                if !push(next) {
                    full = true;
                    break;
                }
            }
            if all_ended {
                break;
            }
        }
        self.reply(request, binds)
    }

    /// This agent is read-only.
    fn refuse_set(&self, request: &Pdu) -> Vec<u8> {
        let status = match self.version {
            CommunityVersion::V1 => status::NO_SUCH_NAME,
            CommunityVersion::V2c => status::NOT_WRITABLE,
        };
        self.error(request, status, usize::from(!request.varbinds.is_empty()))
    }

    /// A successful response — or `tooBig` if it would exceed the message size limit.
    fn reply(&self, request: &Pdu, binds: Vec<VarBind>) -> Vec<u8> {
        let mut response = Pdu {
            kind: PduKind::Response,
            request_id: request.request_id,
            error_status: status::NO_ERROR,
            error_index: 0,
            varbinds: binds,
        };
        loop {
            let encoded = self.encode(&response);
            if encoded.len() <= self.max_size {
                return encoded;
            }
            if request.kind == PduKind::GetBulk && response.varbinds.pop().is_some() {
                // The size estimate was a little short; give up the last binding.
                continue;
            }
            return self.too_big(request);
        }
    }

    /// RFC 3416 §4.2.1: tooBig carries no bindings; RFC 1157 has v1 echo the request's.
    fn too_big(&self, request: &Pdu) -> Vec<u8> {
        let varbinds = match self.version {
            CommunityVersion::V1 => request.varbinds.clone(),
            CommunityVersion::V2c => Vec::new(),
        };
        self.encode(&Pdu {
            kind: PduKind::Response,
            request_id: request.request_id,
            error_status: status::TOO_BIG,
            error_index: 0,
            varbinds,
        })
    }

    /// An error response: the request's bindings returned unchanged with the error fields set.
    fn error(&self, request: &Pdu, error_status: u32, index: usize) -> Vec<u8> {
        self.encode(&Pdu {
            kind: PduKind::Response,
            request_id: request.request_id,
            error_status,
            error_index: u32::try_from(index).unwrap_or(0),
            varbinds: request.varbinds.clone(),
        })
    }

    fn encode(&self, response: &Pdu) -> Vec<u8> {
        encode_community_message(self.version, self.community, response)
    }

    fn empty_reply_len(&self, request: &Pdu) -> usize {
        self.encode(&Pdu {
            kind: PduKind::Response,
            request_id: request.request_id,
            error_status: status::NO_ERROR,
            error_index: 0,
            varbinds: Vec::new(),
        })
        .len()
    }
}

fn encoded_len(bind: &VarBind) -> usize {
    let mut w = Writer::new();
    write_varbind(&mut w, bind);
    w.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    fn view() -> View {
        [
            ("1.3.6.1.2.1.1.1.0", Value::from("simulated switch")),
            ("1.3.6.1.2.1.1.3.0", Value::TimeTicks(4242)),
            ("1.3.6.1.2.1.1.5.0", Value::from("lab-sw1")),
            ("1.3.6.1.2.1.2.2.1.2.1", Value::from("Gi0/1")),
            ("1.3.6.1.2.1.2.2.1.2.2", Value::from("Gi0/2")),
            ("1.3.6.1.2.1.2.2.1.10.1", Value::Counter32(1000)),
            ("1.3.6.1.2.1.2.2.1.10.2", Value::Counter32(2000)),
            ("1.3.6.1.2.1.31.1.1.1.6.1", Value::Counter64(1 << 40)),
            ("1.3.6.1.2.1.31.1.1.1.6.2", Value::Counter64(1 << 41)),
            ("1.3.6.1.2.1.31.1.1.1.18.1", Value::from("uplink")),
        ]
        .into_iter()
        .map(|(o, v)| (oid(o), v))
        .collect()
    }

    /// Runs one request through the agent logic, without sockets.
    fn exchange(
        view: &View,
        version: CommunityVersion,
        max_size: usize,
        kind: PduKind,
        non_repeaters: u32,
        max_repetitions: u32,
        oids: &[&str],
    ) -> Pdu {
        let oids: Vec<Oid> = oids.iter().map(|o| oid(o)).collect();
        let mut request = Pdu::request(kind, 99, &oids);
        request.error_status = non_repeaters;
        request.error_index = max_repetitions;
        let responder = Responder {
            view,
            version,
            community: b"public",
            max_size,
        };
        let encoded = match kind {
            PduKind::Get => responder.get(&request),
            PduKind::GetNext => responder.get_next(&request),
            PduKind::GetBulk => responder.get_bulk(&request),
            PduKind::Set => responder.refuse_set(&request),
            other => panic!("not a request: {other:?}"),
        };
        assert!(
            encoded.len() <= max_size.max(64),
            "response exceeds the size limit"
        );
        let message = CommunityMessage::decode(&encoded).unwrap();
        assert_eq!(message.version, version);
        assert_eq!(message.community, b"public");
        let AnyPdu::Standard(response) = message.pdu else {
            panic!("expected a response PDU");
        };
        assert_eq!(response.kind, PduKind::Response);
        assert_eq!(response.request_id, 99);
        response
    }

    fn names(pdu: &Pdu) -> Vec<String> {
        pdu.varbinds
            .iter()
            .map(|bind| bind.oid.to_string())
            .collect()
    }

    const V1: CommunityVersion = CommunityVersion::V1;
    const V2C: CommunityVersion = CommunityVersion::V2c;
    const BIG: usize = MAX_UDP_PAYLOAD;

    #[test]
    fn get_returns_values_and_v2_exceptions() {
        let view = view();
        let response = exchange(
            &view,
            V2C,
            BIG,
            PduKind::Get,
            0,
            0,
            &[
                "1.3.6.1.2.1.1.5.0",
                "1.3.6.1.2.1.1.5.1",
                "1.3.6.1.2.1.1.99.0",
                "1.3.6.1.2.1.2.2.1.10.7",
                "1.3.6.1.2.1.1.5",
            ],
        );
        assert_eq!(response.error_status, 0);
        let values: Vec<&Value> = response.varbinds.iter().map(|bind| &bind.value).collect();
        assert_eq!(
            values,
            [
                &Value::from("lab-sw1"),
                // sysName exists, instance 1 does not.
                &Value::NoSuchInstance,
                // Nothing at all under 1.3.6.1.2.1.1.99.
                &Value::NoSuchObject,
                // ifInOctets has other rows.
                &Value::NoSuchInstance,
                // The object itself is not an instance; its siblings are other objects.
                &Value::NoSuchInstance,
            ]
        );
        assert_eq!(names(&response)[1], "1.3.6.1.2.1.1.5.1");
    }

    #[test]
    fn v1_get_fails_with_no_such_name_and_echoes_the_request() {
        let view = view();
        let response = exchange(
            &view,
            V1,
            BIG,
            PduKind::Get,
            0,
            0,
            &[
                "1.3.6.1.2.1.1.5.0",
                "1.3.6.1.2.1.1.99.0",
                "1.3.6.1.2.1.1.1.0",
            ],
        );
        assert_eq!((response.error_status, response.error_index), (2, 2));
        assert_eq!(
            names(&response),
            [
                "1.3.6.1.2.1.1.5.0",
                "1.3.6.1.2.1.1.99.0",
                "1.3.6.1.2.1.1.1.0"
            ]
        );
        assert!(
            response
                .varbinds
                .iter()
                .all(|bind| bind.value == Value::Null)
        );

        let ok = exchange(&view, V1, BIG, PduKind::Get, 0, 0, &["1.3.6.1.2.1.1.5.0"]);
        assert_eq!(ok.error_status, 0);
        assert_eq!(ok.varbinds[0].value, Value::from("lab-sw1"));
    }

    #[test]
    fn get_next_walks_in_order_and_ends_properly() {
        let view = view();
        let response = exchange(
            &view,
            V2C,
            BIG,
            PduKind::GetNext,
            0,
            0,
            &[
                "1.3.6.1.2.1.1",
                "1.3.6.1.2.1.1.1.0",
                "1.3.6.1.2.1.2.2.1.2.2",
                "1.3.6.1.2.1.31.1.1.1.18.1",
                "2.5",
            ],
        );
        assert_eq!(
            names(&response),
            [
                "1.3.6.1.2.1.1.1.0",
                "1.3.6.1.2.1.1.3.0",
                "1.3.6.1.2.1.2.2.1.10.1",
                "1.3.6.1.2.1.31.1.1.1.18.1",
                "2.5",
            ]
        );
        assert_eq!(response.varbinds[3].value, Value::EndOfMibView);
        assert_eq!(response.varbinds[4].value, Value::EndOfMibView);

        // SNMPv1: the end of the MIB is noSuchName with the index of the offender.
        let v1 = exchange(
            &view,
            V1,
            BIG,
            PduKind::GetNext,
            0,
            0,
            &["1.3.6.1.2.1.1.5.0", "1.3.6.1.2.1.31.1.1.1.18.1"],
        );
        assert_eq!((v1.error_status, v1.error_index), (2, 2));
    }

    #[test]
    fn v1_never_sees_counter64() {
        let view = view();
        // GET of a Counter64 object: no such name.
        let get = exchange(
            &view,
            V1,
            BIG,
            PduKind::Get,
            0,
            0,
            &["1.3.6.1.2.1.31.1.1.1.6.1"],
        );
        assert_eq!((get.error_status, get.error_index), (2, 1));
        // GETNEXT skips over the Counter64 column to the next visible object.
        let next = exchange(
            &view,
            V1,
            BIG,
            PduKind::GetNext,
            0,
            0,
            &["1.3.6.1.2.1.2.2.1.10.2"],
        );
        assert_eq!(names(&next), ["1.3.6.1.2.1.31.1.1.1.18.1"]);
        // SNMPv2c sees it.
        let next = exchange(
            &view,
            V2C,
            BIG,
            PduKind::GetNext,
            0,
            0,
            &["1.3.6.1.2.1.2.2.1.10.2"],
        );
        assert_eq!(names(&next), ["1.3.6.1.2.1.31.1.1.1.6.1"]);
    }

    #[test]
    fn get_bulk_interleaves_repeaters() {
        let view = view();
        let response = exchange(
            &view,
            V2C,
            BIG,
            PduKind::GetBulk,
            1,
            2,
            &[
                "1.3.6.1.2.1.1.3",
                "1.3.6.1.2.1.2.2.1.2",
                "1.3.6.1.2.1.2.2.1.10",
            ],
        );
        assert_eq!(
            names(&response),
            [
                // the non-repeater
                "1.3.6.1.2.1.1.3.0",
                // repetition 1
                "1.3.6.1.2.1.2.2.1.2.1",
                "1.3.6.1.2.1.2.2.1.10.1",
                // repetition 2
                "1.3.6.1.2.1.2.2.1.2.2",
                "1.3.6.1.2.1.2.2.1.10.2",
            ]
        );

        // max-repetitions 0 serves the non-repeaters only; more non-repeaters than bindings
        // is fine too.
        let only_singles = exchange(
            &view,
            V2C,
            BIG,
            PduKind::GetBulk,
            1,
            0,
            &["1.3.6.1.2.1.1.3", "1.3.6.1.2.1.2"],
        );
        assert_eq!(names(&only_singles), ["1.3.6.1.2.1.1.3.0"]);
        let all_singles = exchange(
            &view,
            V2C,
            BIG,
            PduKind::GetBulk,
            9,
            5,
            &["1.3.6.1.2.1.1.3", "1.3.6.1.2.1.2"],
        );
        assert_eq!(
            names(&all_singles),
            ["1.3.6.1.2.1.1.3.0", "1.3.6.1.2.1.2.2.1.2.1"]
        );
    }

    #[test]
    fn get_bulk_stops_after_a_round_of_end_of_mib_view() {
        let view = view();
        let response = exchange(
            &view,
            V2C,
            BIG,
            PduKind::GetBulk,
            0,
            1000,
            &["1.3.6.1.2.1.31.1.1.1.6.1", "1.3.6.1.2.1.31.1.1.1.18"],
        );
        assert_eq!(
            names(&response),
            [
                "1.3.6.1.2.1.31.1.1.1.6.2",
                "1.3.6.1.2.1.31.1.1.1.18.1",
                "1.3.6.1.2.1.31.1.1.1.18.1",
                "1.3.6.1.2.1.31.1.1.1.18.1",
                // one full round of endOfMibView, then no more
                "1.3.6.1.2.1.31.1.1.1.18.1",
                "1.3.6.1.2.1.31.1.1.1.18.1",
            ]
        );
        let values: Vec<bool> = response
            .varbinds
            .iter()
            .map(|bind| bind.value == Value::EndOfMibView)
            .collect();
        assert_eq!(values, [false, false, false, true, true, true]);
    }

    #[test]
    fn responses_respect_the_message_size_limit() {
        let view: View = (1..=200u32)
            .map(|i| {
                (
                    oid("1.3.6.1.4.1.99999.1").child(i),
                    Value::from("x".repeat(40)),
                )
            })
            .collect();
        let root = "1.3.6.1.4.1.99999.1";

        // GETBULK is truncated to what fits, down to nothing at all.
        for (limit, at_least) in [(1472usize, 20usize), (484, 5), (120, 1), (60, 0)] {
            let response = exchange(&view, V2C, limit, PduKind::GetBulk, 0, 100, &[root]);
            assert_eq!(response.error_status, 0, "limit {limit}");
            assert!(
                response.varbinds.len() >= at_least,
                "limit {limit}: {}",
                response.varbinds.len()
            );
            assert!(response.varbinds.len() < 100, "limit {limit}");
            // What is there is a correct prefix of the walk.
            for (i, bind) in response.varbinds.iter().enumerate() {
                assert_eq!(bind.oid, oid(root).child(i as u32 + 1));
            }
        }
        let unlimited = exchange(&view, V2C, BIG, PduKind::GetBulk, 0, 100, &[root]);
        assert_eq!(unlimited.varbinds.len(), 100);

        // GET cannot be truncated: tooBig, with no bindings on v2c and the request's on v1.
        let many: Vec<String> = (1..=30).map(|i| format!("{root}.{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let too_big = exchange(&view, V2C, 484, PduKind::Get, 0, 0, &many);
        assert_eq!((too_big.error_status, too_big.error_index), (1, 0));
        assert!(too_big.varbinds.is_empty());
        let fits = exchange(&view, V2C, 484, PduKind::Get, 0, 0, &many[..5]);
        assert_eq!((fits.error_status, fits.varbinds.len()), (0, 5));
        let v1_too_big = exchange(&view, V1, 484, PduKind::Get, 0, 0, &many[..9]);
        assert_eq!(v1_too_big.error_status, 1);
        assert_eq!(v1_too_big.varbinds.len(), 9);
    }

    #[test]
    fn set_is_refused() {
        let view = view();
        let v2 = exchange(&view, V2C, BIG, PduKind::Set, 0, 0, &["1.3.6.1.2.1.1.5.0"]);
        assert_eq!((v2.error_status, v2.error_index), (17, 1));
        let v1 = exchange(&view, V1, BIG, PduKind::Set, 0, 0, &["1.3.6.1.2.1.1.5.0"]);
        assert_eq!((v1.error_status, v1.error_index), (2, 1));
    }

    async fn agent_with(view: View) -> Agent {
        let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), "public")
            .await
            .unwrap();
        agent.set_view(view);
        agent
    }

    #[tokio::test]
    async fn only_matching_v1_v2c_requests_are_answered() {
        let agent = agent_with(view()).await.inner;
        let get = |version, community: &[u8], kind| {
            let request = Pdu::request(kind, 7, &[oid("1.3.6.1.2.1.1.5.0")]);
            encode_community_message(version, community, &request)
        };
        assert!(agent.respond(&get(V2C, b"public", PduKind::Get)).is_some());
        assert!(agent.respond(&get(V1, b"public", PduKind::Get)).is_some());
        assert!(
            agent
                .respond(&get(V2C, b"public", PduKind::GetBulk))
                .is_some()
        );
        assert!(agent.respond(&get(V2C, b"public", PduKind::Set)).is_some());
        // Wrong community: silence, as RFC 3584 demands.
        assert!(agent.respond(&get(V2C, b"private", PduKind::Get)).is_none());
        assert!(agent.respond(&get(V2C, b"", PduKind::Get)).is_none());
        // SNMPv1 has no GETBULK; responses, reports and notifications are not requests.
        assert!(
            agent
                .respond(&get(V1, b"public", PduKind::GetBulk))
                .is_none()
        );
        for kind in [
            PduKind::Response,
            PduKind::Report,
            PduKind::Inform,
            PduKind::TrapV2,
        ] {
            assert!(agent.respond(&get(V2C, b"public", kind)).is_none());
        }
        // Garbage and SNMPv3.
        assert!(agent.respond(&[]).is_none());
        assert!(agent.respond(b"\x30\x03\x02\x01\x03").is_none());
        assert!(agent.respond(&[0xff; 64]).is_none());
    }

    #[tokio::test]
    async fn hostile_datagrams_get_a_proper_answer_or_none() {
        let agent = agent_with(view()).await;
        let inputs = crate::robustness::hostile_inputs();
        for limit in [MAX_UDP_PAYLOAD, 484, 40, 0] {
            agent.set_max_message_size(limit);
            let mut answered = 0;
            for input in &inputs {
                let Some(response) = agent.inner.respond(input) else {
                    continue;
                };
                answered += 1;
                // Whatever the request looked like, the answer is a well-formed Response.
                let message = CommunityMessage::decode(&response).expect("a well-formed response");
                assert_eq!(message.community, b"public");
                let AnyPdu::Standard(pdu) = message.pdu else {
                    panic!("the agent answered with a trap");
                };
                assert_eq!(pdu.kind, PduKind::Response);
            }
            // Corrupted copies of real requests still reach the request handlers.
            assert!(answered > 500, "limit {limit}: {answered} answers");
        }
    }

    #[tokio::test]
    async fn set_view_drops_what_cannot_be_served() {
        let mut view = view();
        let servable = view.len();
        view.insert(oid("7.1.2"), Value::Integer(1));
        view.insert(oid("1.50.1"), Value::Integer(1));
        view.insert(oid("1"), Value::Integer(1));
        view.insert(oid("1.3.6.1.9.1"), Value::EndOfMibView);
        view.insert(oid("1.3.6.1.9.2"), Value::Oid(oid("9.9.9")));
        let agent = agent_with(view).await;
        assert_eq!(
            agent.inner.view.read().unwrap().len(),
            servable,
            "only the encodable entries remain"
        );
    }
}
