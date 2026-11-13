//! The SNMP manager: [`Client`], its configuration and the request primitives.

mod session;
mod transport;
mod walk;

use std::fmt;
use std::net::SocketAddr;
use std::ops::Range;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ber;
use crate::pdu::{
    AnyPdu, CommunityMessage, CommunityVersion, Envelope, Pdu, PduKind, check_varbinds,
    encode_community_message,
};
use crate::v3::usm::{AuthProtocol, PrivProtocol};
use crate::{Oid, SnmpError, Value, VarBind};
use session::UsmSession;
use transport::Transport;

/// How many variable bindings `get`/`get_next` put in one PDU before starting another.
///
/// Fifty instance OIDs stay well inside a single unfragmented datagram on the way out, and
/// agents with small buffers get a chance to say `tooBig` (which is then handled by splitting)
/// instead of silently dropping an oversized request.
const MAX_VARBINDS_PER_REQUEST: usize = 50;

/// Who we are to the agent.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "version", rename_all = "snake_case")]
pub enum Auth {
    V1 {
        community: String,
    },
    V2c {
        community: String,
    },
    V3 {
        user: String,
        /// Authentication protocol and passphrase; `None` for noAuthNoPriv.
        #[serde(default)]
        auth: Option<(AuthProtocol, String)>,
        /// Privacy protocol and passphrase; requires `auth`.
        #[serde(default)]
        privacy: Option<(PrivProtocol, String)>,
        #[serde(default)]
        context_name: String,
    },
}

/// Prints as `"***"`.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"***\"")
    }
}

/// Communities and passphrases are secrets: `Debug` shows everything else and masks them, so
/// that logging a configuration cannot leak credentials.
impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::V1 { .. } => f.debug_struct("V1").field("community", &Redacted).finish(),
            Auth::V2c { .. } => f.debug_struct("V2c").field("community", &Redacted).finish(),
            Auth::V3 {
                user,
                auth,
                privacy,
                context_name,
            } => f
                .debug_struct("V3")
                .field("user", user)
                .field("auth", &auth.as_ref().map(|(protocol, _)| (protocol, Redacted)))
                .field(
                    "privacy",
                    &privacy.as_ref().map(|(protocol, _)| (protocol, Redacted)),
                )
                .field("context_name", context_name)
                .finish(),
        }
    }
}

/// Where and how to reach an agent.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub target: SocketAddr,
    pub auth: Auth,
    /// Per attempt, default 2 s.
    pub timeout: Duration,
    /// Retransmissions after the first attempt, default 1.
    pub retries: u32,
    /// Rows asked for per GETBULK, default 25.
    pub max_repetitions: u32,
}

impl ClientConfig {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
    pub const DEFAULT_RETRIES: u32 = 1;
    pub const DEFAULT_MAX_REPETITIONS: u32 = 25;

    /// A configuration with the default timeout, retries and max-repetitions.
    pub fn new(target: SocketAddr, auth: Auth) -> ClientConfig {
        ClientConfig {
            target,
            auth,
            timeout: Self::DEFAULT_TIMEOUT,
            retries: Self::DEFAULT_RETRIES,
            max_repetitions: Self::DEFAULT_MAX_REPETITIONS,
        }
    }
}

/// How requests are secured.
enum Security {
    Community {
        version: CommunityVersion,
        community: Vec<u8>,
    },
    Usm(Box<UsmSession>),
}

/// An SNMP manager talking to one agent.
///
/// A `Client` owns one UDP socket and performs one request at a time (every method takes
/// `&mut self`); to poll several agents concurrently create one client per agent and drive
/// them from separate tasks. Creating a client is cheap and does no network I/O: an SNMPv3
/// client discovers the agent's engine and derives its keys on the first request, and keeps
/// both for as long as it lives.
pub struct Client {
    transport: Transport,
    security: Security,
    request_ids: IdSequence,
    max_repetitions: u32,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version = match &self.security {
            Security::Community {
                version: CommunityVersion::V1,
                ..
            } => "v1",
            Security::Community {
                version: CommunityVersion::V2c,
                ..
            } => "v2c",
            Security::Usm(_) => "v3",
        };
        f.debug_struct("Client")
            .field("target", &self.transport.target())
            .field("version", &version)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Creates a client for `config.target`. Only the local socket is set up here; an
    /// unreachable agent or wrong credentials show up on the first request.
    pub async fn connect(config: ClientConfig) -> Result<Client, SnmpError> {
        let security = match &config.auth {
            Auth::V1 { community } => Security::Community {
                version: CommunityVersion::V1,
                community: community.as_bytes().to_vec(),
            },
            Auth::V2c { community } => Security::Community {
                version: CommunityVersion::V2c,
                community: community.as_bytes().to_vec(),
            },
            Auth::V3 {
                user,
                auth,
                privacy,
                context_name,
            } => Security::Usm(Box::new(UsmSession::new(
                user,
                auth.clone(),
                privacy.clone(),
                context_name,
            )?)),
        };
        let transport = Transport::bind(config.target, config.timeout, config.retries).await?;
        Ok(Client {
            transport,
            security,
            request_ids: IdSequence::random(),
            max_repetitions: config.max_repetitions.clamp(1, i32::MAX as u32),
        })
    }

    /// The agent this client talks to.
    pub fn target(&self) -> SocketAddr {
        self.transport.target()
    }

    /// GET. Returns one variable binding per OID, in order.
    ///
    /// On SNMPv2c/v3 a missing object comes back as a `NoSuchObject`/`NoSuchInstance` value;
    /// on SNMPv1 it fails the request with `noSuchName`. Long lists are sent as several PDUs
    /// of at most 50 OIDs, and a PDU the agent finds `tooBig` is split in two and retried.
    pub async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, SnmpError> {
        self.retrieve(PduKind::Get, oids).await
    }

    /// GETNEXT. Returns, for each OID, the first object instance after it.
    ///
    /// Past the end of the MIB SNMPv2c/v3 return `EndOfMibView` values and SNMPv1 fails with
    /// `noSuchName`. Chunked and split like [`get`](Self::get).
    pub async fn get_next(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, SnmpError> {
        self.retrieve(PduKind::GetNext, oids).await
    }

    /// GETBULK (SNMPv2c/v3 only): one GETNEXT for each of the first `non_repeaters` OIDs and
    /// up to `max_repetitions` successive GETNEXTs for each of the others, interleaved.
    ///
    /// The agent may return fewer bindings than asked for. If it answers `tooBig`,
    /// `max_repetitions` is halved and the request retried.
    pub async fn get_bulk(
        &mut self,
        oids: &[Oid],
        non_repeaters: u32,
        max_repetitions: u32,
    ) -> Result<Vec<VarBind>, SnmpError> {
        for oid in oids {
            ber::check_oid(oid)?;
        }
        let mut repetitions = max_repetitions;
        self.bulk(oids, non_repeaters, &mut repetitions).await
    }

    /// Walk a subtree (GETBULK on v2c/v3, GETNEXT on v1). Stops at the subtree end, on a
    /// non-increasing OID (loop guard), or after `limit` varbinds.
    ///
    /// Like net-snmp's `snmpwalk`, a walk that finds nothing below `root` tries a GET of
    /// `root` itself, so walking a scalar instance (`sysName.0`) yields that instance.
    /// `limit` is taken literally: 0 returns nothing; use `usize::MAX` for "everything".
    pub async fn walk(&mut self, root: &Oid, limit: usize) -> Result<Vec<VarBind>, SnmpError> {
        ber::check_oid(root)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut found = self
            .walk_subtrees(std::slice::from_ref(root), limit)
            .await?
            .pop()
            .unwrap_or_default();
        if found.is_empty() {
            match self.get(std::slice::from_ref(root)).await {
                Ok(binds) => found.extend(
                    binds
                        .into_iter()
                        .filter(|bind| bind.oid == *root && !bind.value.is_exception()),
                ),
                // noSuchName and friends: there is simply nothing there.
                Err(SnmpError::Agent { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(found)
    }

    /// Walk several columns of one table and pivot by index:
    /// index suffix → column oid → value.
    ///
    /// All columns are fetched together (one GETBULK carries every column as a repeater, so a
    /// response holds whole rows). Rows come back in index order, cells in the order of
    /// `columns`; a row only has the columns the agent actually instantiates. At most `limit`
    /// rows are returned — the first ones in index order.
    pub async fn table(
        &mut self,
        columns: &[Oid],
        limit: usize,
    ) -> Result<indexmap::IndexMap<Vec<u32>, indexmap::IndexMap<Oid, Value>>, SnmpError> {
        for column in columns {
            ber::check_oid(column)?;
        }
        let walked = self.walk_subtrees(columns, limit).await?;
        Ok(walk::pivot(columns, walked, limit))
    }

    /// SET. All bindings are sent in one PDU so that the agent applies them atomically; the
    /// agent's echo of the bindings is returned.
    pub async fn set(&mut self, binds: &[VarBind]) -> Result<Vec<VarBind>, SnmpError> {
        check_varbinds(binds)?;
        for bind in binds {
            self.check_settable(&bind.value)?;
        }
        if binds.is_empty() {
            return Ok(Vec::new());
        }
        let echoed = self.request(PduKind::Set, 0, 0, binds.to_vec()).await?;
        expect_count(&echoed, binds.len())?;
        Ok(echoed)
    }

    fn check_settable(&self, value: &Value) -> Result<(), SnmpError> {
        match value {
            Value::Integer(v) if i32::try_from(*v).is_err() => Err(SnmpError::Unsupported(format!(
                "INTEGER {v} is outside the Integer32 range"
            ))),
            Value::Counter64(_) if self.is_v1() => Err(SnmpError::Unsupported(
                "Counter64 does not exist in SNMPv1".to_owned(),
            )),
            Value::NoSuchObject | Value::NoSuchInstance | Value::EndOfMibView => Err(
                SnmpError::Unsupported("an exception marker is not a value that can be set".to_owned()),
            ),
            _ => Ok(()),
        }
    }

    fn is_v1(&self) -> bool {
        matches!(
            self.security,
            Security::Community {
                version: CommunityVersion::V1,
                ..
            }
        )
    }

    /// GET or GETNEXT for any number of OIDs.
    async fn retrieve(&mut self, kind: PduKind, oids: &[Oid]) -> Result<Vec<VarBind>, SnmpError> {
        for oid in oids {
            ber::check_oid(oid)?;
        }
        let mut results = Vec::with_capacity(oids.len());
        // Ranges of `oids` still to fetch; the next one is on top.
        let mut pending: Vec<Range<usize>> = (0..oids.len())
            .step_by(MAX_VARBINDS_PER_REQUEST)
            .map(|start| start..oids.len().min(start + MAX_VARBINDS_PER_REQUEST))
            .rev()
            .collect();

        while let Some(range) = pending.pop() {
            let chunk = &oids[range.clone()];
            match self.request(kind, 0, 0, placeholders(chunk)).await {
                Ok(binds) => {
                    expect_count(&binds, chunk.len())?;
                    results.extend(binds);
                }
                Err(e) if e.is_too_big() && chunk.len() > 1 => {
                    let middle = range.start + chunk.len() / 2;
                    pending.push(middle..range.end);
                    pending.push(range.start..middle);
                }
                // error-index counts within the PDU that was sent; callers think in terms of
                // the list they passed.
                Err(SnmpError::Agent { status, name, index }) if index > 0 => {
                    let offset = u32::try_from(range.start).unwrap_or(u32::MAX);
                    return Err(SnmpError::Agent {
                        status,
                        name,
                        index: index.saturating_add(offset),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        Ok(results)
    }

    /// One GETBULK. On `tooBig` the repetition count is halved — and left halved in
    /// `repetitions` for the caller's next request — until the agent manages to answer.
    async fn bulk(
        &mut self,
        oids: &[Oid],
        non_repeaters: u32,
        repetitions: &mut u32,
    ) -> Result<Vec<VarBind>, SnmpError> {
        if self.is_v1() {
            return Err(SnmpError::Unsupported(
                "GETBULK does not exist in SNMPv1".to_owned(),
            ));
        }
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        let limit = i32::MAX as u32;
        loop {
            let request = self.request(
                PduKind::GetBulk,
                non_repeaters.min(limit),
                (*repetitions).min(limit),
                placeholders(oids),
            );
            match request.await {
                Err(e) if e.is_too_big() && *repetitions > 1 => *repetitions /= 2,
                other => return other,
            }
        }
    }

    /// Sends one request and returns the variable bindings of the response, turning a
    /// non-zero error-status into [`SnmpError::Agent`].
    async fn request(
        &mut self,
        kind: PduKind,
        non_repeaters: u32,
        max_repetitions: u32,
        varbinds: Vec<VarBind>,
    ) -> Result<Vec<VarBind>, SnmpError> {
        let request = Pdu {
            kind,
            request_id: self.request_ids.next(),
            error_status: non_repeaters,
            error_index: max_repetitions,
            varbinds,
        };
        let response = match &mut self.security {
            Security::Community { version, community } => {
                let datagram = encode_community_message(*version, community, &request);
                let attempts = self.transport.attempts();
                self.transport
                    .exchange(&datagram, attempts, |received| {
                        community_reply(received, request.request_id)
                    })
                    .await?
            }
            Security::Usm(session) => session.request(&mut self.transport, &request).await?,
        };
        if response.error_status != 0 {
            return Err(SnmpError::agent(response.error_status, response.error_index));
        }
        Ok(response.varbinds)
    }
}

/// NULL-valued bindings, as requests carry them.
fn placeholders(oids: &[Oid]) -> Vec<VarBind> {
    oids.iter()
        .map(|oid| VarBind::new(oid.clone(), Value::Null))
        .collect()
}

fn expect_count(binds: &[VarBind], expected: usize) -> Result<(), SnmpError> {
    if binds.len() == expected {
        Ok(())
    } else {
        Err(SnmpError::decode(format!(
            "the agent answered {} variable bindings to a request for {expected}",
            binds.len()
        )))
    }
}

/// Picks the Response PDU that answers `request_id` out of a received datagram.
fn community_reply(datagram: &[u8], request_id: i32) -> Result<Option<Pdu>, SnmpError> {
    match CommunityMessage::decode(datagram) {
        Ok(CommunityMessage {
            pdu: AnyPdu::Standard(pdu),
            ..
        }) if pdu.kind == PduKind::Response && pdu.request_id == request_id => Ok(Some(pdu)),
        // Somebody else's datagram, or a late answer to an earlier request.
        Ok(_) => Ok(None),
        // Our answer, but broken: better a clear error than a timeout.
        Err(e) if response_id(datagram) == Some(request_id) => Err(e),
        Err(_) => Ok(None),
    }
}

/// The request-id of a community-based Response, read without decoding the bindings.
fn response_id(datagram: &[u8]) -> Option<i32> {
    let mut body = Envelope::open(datagram).ok()?.body;
    body.octet_string().ok()?;
    let (tag, mut pdu) = body.read_tlv().ok()?;
    if tag != PduKind::Response.tag() {
        return None;
    }
    pdu.integer32().ok()
}

/// Request ids and msgIDs: a random starting point, then consecutive values in `1..=i32::MAX`
/// so that a late answer to an earlier request can never be mistaken for the current one.
pub(crate) struct IdSequence(i32);

impl IdSequence {
    pub fn random() -> IdSequence {
        IdSequence((random_u64() & 0x7fff_ffff) as i32)
    }

    pub fn next(&mut self) -> i32 {
        self.0 = if self.0 == i32::MAX { 1 } else { self.0 + 1 };
        self.0
    }
}

/// Random bits from the operating system. Ids and salts need to be unique rather than
/// secret, so the clock is an acceptable stand-in on a system without an entropy source.
pub(crate) fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_ok() {
        return u64::from_be_bytes(bytes);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    nanos ^ u64::from(std::process::id()).rotate_left(32)
}

#[cfg(test)]
mod tests;
