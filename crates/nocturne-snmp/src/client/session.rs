//! The manager side of the User-based Security Model: engine discovery, clock
//! synchronisation, key management and the handling of Report PDUs (RFC 3414 §3 and §4).

use std::collections::HashMap;
use std::time::Instant;

use tracing::{debug, warn};

use crate::pdu::{Pdu, PduKind};
use crate::v3::usm::{AuthProtocol, LocalizedKeys, PrivProtocol};
use crate::v3::{FLAG_AUTH, FLAG_PRIV, FLAG_REPORTABLE, OutgoingMessage, ScopedPdu, V3Message};
use crate::{Oid, SnmpError, VarBind};

use super::transport::Transport;
use super::{IdSequence, random_u64};

/// The three USM security levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "these are the names RFC 3411 gives the levels"
)]
enum Level {
    NoAuthNoPriv,
    AuthNoPriv,
    AuthPriv,
}

impl Level {
    fn flags(self) -> u8 {
        match self {
            Level::NoAuthNoPriv => 0,
            Level::AuthNoPriv => FLAG_AUTH,
            Level::AuthPriv => FLAG_AUTH | FLAG_PRIV,
        }
    }
}

/// What we know about the agent's SNMP engine.
struct Engine {
    id: Vec<u8>,
    boots: u32,
    time: u32,
    /// When `boots`/`time` were learned.
    synced_at: Instant,
}

impl Engine {
    /// Our estimate of the agent's snmpEngineTime right now.
    fn current_time(&self) -> u32 {
        let elapsed = u32::try_from(self.synced_at.elapsed().as_secs()).unwrap_or(u32::MAX);
        // snmpEngineTime never exceeds 2^31-1; past that the agent bumps its boot counter and
        // will tell us so.
        self.time.saturating_add(elapsed).min(i32::MAX as u32)
    }
}

/// The master keys (Ku) of the two passphrases. They cost a megabyte of hashing each and do
/// not depend on the engine, so they are derived once per session.
struct MasterKeys {
    auth: Vec<u8>,
    privacy: Option<Vec<u8>>,
}

/// The usmStats / snmpMPD counters an agent reports instead of answering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReportKind {
    UnsupportedSecLevel,
    NotInTimeWindow,
    UnknownUserName,
    UnknownEngineId,
    WrongDigest,
    DecryptionError,
    UnknownSecurityModel,
    InvalidMessage,
    UnknownPduHandler,
    UnavailableContext,
    UnknownContext,
    /// A Report carrying something else (or nothing at all).
    Other(Option<Oid>),
}

impl ReportKind {
    pub(crate) fn of(varbinds: &[VarBind]) -> ReportKind {
        let Some(first) = varbinds.first() else {
            return ReportKind::Other(None);
        };
        match *first.oid.parts() {
            // usmStats, 1.3.6.1.6.3.15.1.1
            [1, 3, 6, 1, 6, 3, 15, 1, 1, counter, ..] => match counter {
                1 => ReportKind::UnsupportedSecLevel,
                2 => ReportKind::NotInTimeWindow,
                3 => ReportKind::UnknownUserName,
                4 => ReportKind::UnknownEngineId,
                5 => ReportKind::WrongDigest,
                6 => ReportKind::DecryptionError,
                _ => ReportKind::Other(Some(first.oid.clone())),
            },
            // snmpMPDStats, 1.3.6.1.6.3.11.2.1
            [1, 3, 6, 1, 6, 3, 11, 2, 1, counter, ..] => match counter {
                1 => ReportKind::UnknownSecurityModel,
                2 => ReportKind::InvalidMessage,
                3 => ReportKind::UnknownPduHandler,
                _ => ReportKind::Other(Some(first.oid.clone())),
            },
            // snmpTargetObjects, 1.3.6.1.6.3.12.1
            [1, 3, 6, 1, 6, 3, 12, 1, 4, ..] => ReportKind::UnavailableContext,
            [1, 3, 6, 1, 6, 3, 12, 1, 5, ..] => ReportKind::UnknownContext,
            _ => ReportKind::Other(Some(first.oid.clone())),
        }
    }

    pub(crate) fn into_error(self) -> SnmpError {
        let message = match self {
            ReportKind::UnsupportedSecLevel => "unsupported security level".to_owned(),
            ReportKind::NotInTimeWindow => {
                "not in time window (the agent keeps rejecting its own engine boots/time)"
                    .to_owned()
            }
            ReportKind::UnknownUserName => "unknown user name".to_owned(),
            ReportKind::UnknownEngineId => "unknown engine ID".to_owned(),
            ReportKind::WrongDigest => {
                "wrong digest (authentication passphrase or protocol mismatch)".to_owned()
            }
            ReportKind::DecryptionError => {
                "decryption error (privacy passphrase or protocol mismatch)".to_owned()
            }
            ReportKind::UnknownSecurityModel => "unknown security model".to_owned(),
            ReportKind::InvalidMessage => "the agent rejected the message as invalid".to_owned(),
            ReportKind::UnknownPduHandler => "unknown PDU handler".to_owned(),
            ReportKind::UnavailableContext => "unavailable context".to_owned(),
            ReportKind::UnknownContext => "unknown context name".to_owned(),
            ReportKind::Other(Some(oid)) => format!("the agent sent a report: {oid}"),
            ReportKind::Other(None) => "the agent sent an empty report".to_owned(),
        };
        SnmpError::Auth(message)
    }
}

/// What the agent said to one of our messages.
enum Answer {
    Response(Pdu),
    Report(ReportKind),
}

/// A datagram accepted as the reply to a message we sent.
struct Reply {
    authenticated: bool,
    engine_boots: u32,
    engine_time: u32,
    answer: Answer,
}

pub(crate) struct UsmSession {
    user: Vec<u8>,
    context_name: Vec<u8>,
    auth: Option<(AuthProtocol, String)>,
    privacy: Option<(PrivProtocol, String)>,
    master_keys: Option<MasterKeys>,
    /// Localised keys, cached per engine ID.
    localized: HashMap<Vec<u8>, LocalizedKeys>,
    engine: Option<Engine>,
    msg_ids: IdSequence,
    /// Feeds the privacy IV; incremented for every message so that no IV repeats.
    salt: u64,
    /// Set once an encrypted exchange has worked, which proves the privacy key right.
    privacy_proven: bool,
}

impl UsmSession {
    pub fn new(
        user: &str,
        auth: Option<(AuthProtocol, String)>,
        privacy: Option<(PrivProtocol, String)>,
        context_name: &str,
    ) -> Result<UsmSession, SnmpError> {
        if privacy.is_some() && auth.is_none() {
            return Err(SnmpError::Unsupported(
                "SNMPv3 privacy requires authentication (there is no noAuthPriv level)".to_owned(),
            ));
        }
        if auth
            .as_ref()
            .is_some_and(|(_, passphrase)| passphrase.is_empty())
            || privacy
                .as_ref()
                .is_some_and(|(_, passphrase)| passphrase.is_empty())
        {
            return Err(SnmpError::Auth(
                "an SNMPv3 passphrase must not be empty".to_owned(),
            ));
        }
        Ok(UsmSession {
            user: user.as_bytes().to_vec(),
            context_name: context_name.as_bytes().to_vec(),
            auth,
            privacy,
            master_keys: None,
            localized: HashMap::new(),
            engine: None,
            msg_ids: IdSequence::random(),
            salt: random_u64(),
            privacy_proven: false,
        })
    }

    fn level(&self) -> Level {
        match (&self.auth, &self.privacy) {
            (Some(_), Some(_)) => Level::AuthPriv,
            (Some(_), None) => Level::AuthNoPriv,
            (None, _) => Level::NoAuthNoPriv,
        }
    }

    /// Sends a request PDU and returns the Response PDU, whatever its error-status.
    pub async fn request(
        &mut self,
        transport: &mut Transport,
        pdu: &Pdu,
    ) -> Result<Pdu, SnmpError> {
        let level = self.level();
        // Until an encrypted exchange has worked, the privacy settings are on trial.
        let privacy_untested = level == Level::AuthPriv && !self.privacy_proven;
        match self.send(transport, pdu, level, transport.attempts()).await {
            Ok(Answer::Response(response)) => {
                if level == Level::AuthPriv {
                    self.privacy_proven = true;
                }
                Ok(response)
            }
            // The agent cannot decrypt what we send. Before blaming the passphrase, try the
            // other AES-192/256 key extension.
            Ok(Answer::Report(ReportKind::DecryptionError)) if privacy_untested => {
                match self.retry_with_other_key_extension(transport, pdu).await {
                    Some(result) => result,
                    None => Err(ReportKind::DecryptionError.into_error()),
                }
            }
            Ok(Answer::Report(kind)) => Err(kind.into_error()),
            // The engine answered discovery, yet the encrypted request met silence.
            Err(SnmpError::Timeout) if privacy_untested && self.engine.is_some() => {
                self.explain_silence(transport, pdu).await
            }
            Err(e) => Err(e),
        }
    }

    /// Called when the first encrypted request of a session went unanswered although the
    /// engine had answered discovery.
    ///
    /// Agents are supposed to report decryption failures, but many — net-snmp among them —
    /// just drop a message whose decrypted content makes no sense, which leaves the user
    /// staring at a timeout. Two cheap probes (empty GETs) tell the cases apart:
    ///
    /// 1. authenticated but not encrypted — no answer: the agent is gone, a plain timeout;
    /// 2. encrypted — an answer: privacy works and it was the request itself that got lost
    ///    or took too long, again a plain timeout; no answer: the privacy key is wrong.
    async fn explain_silence(
        &mut self,
        transport: &mut Transport,
        pdu: &Pdu,
    ) -> Result<Pdu, SnmpError> {
        // Any answer to the unencrypted probe — a response, authorizationError, even
        // "unsupported security level" — comes from an agent that is alive and knows this
        // user. Unless it says our authentication is wrong; then that is the news.
        if let Answer::Report(kind @ (ReportKind::WrongDigest | ReportKind::UnknownUserName)) =
            self.probe(transport, Level::AuthNoPriv).await?
        {
            return Err(kind.into_error());
        }
        match self.probe(transport, Level::AuthPriv).await {
            Ok(Answer::Response(_)) => {
                self.privacy_proven = true;
                return Err(SnmpError::Timeout);
            }
            Ok(Answer::Report(ReportKind::DecryptionError)) | Err(SnmpError::Timeout) => {}
            Ok(Answer::Report(kind)) => return Err(kind.into_error()),
            Err(e) => return Err(e),
        }
        if let Some(result) = self.retry_with_other_key_extension(transport, pdu).await {
            return result;
        }
        Err(SnmpError::Auth(
            "decryption error (privacy passphrase or protocol mismatch): the agent accepts this \
             user's authentication but does not answer encrypted requests"
                .to_owned(),
        ))
    }

    /// AES-192/256 exist in two key-extension flavours that nobody can tell apart from a
    /// device's configuration. If the configured one does not work and the other one does,
    /// switches to it for the rest of the session and repeats `pdu`; otherwise `None`.
    async fn retry_with_other_key_extension(
        &mut self,
        transport: &mut Transport,
        pdu: &Pdu,
    ) -> Option<Result<Pdu, SnmpError>> {
        let (auth, configured) = match (&self.auth, &self.privacy) {
            (Some((auth, _)), Some((configured, _))) => (*auth, *configured),
            _ => return None,
        };
        let alternative = configured.alternative_key_extension(auth)?;
        self.use_priv_protocol(alternative);
        if !matches!(
            self.probe(transport, Level::AuthPriv).await,
            Ok(Answer::Response(_))
        ) {
            self.use_priv_protocol(configured);
            return None;
        }
        warn!(
            agent = %transport.target(),
            "agent uses the {alternative:?} key extension rather than the configured \
             {configured:?}; configure it to avoid the detection delay"
        );
        self.privacy_proven = true;
        // The request that started all this, now with keys that work.
        let outcome = self
            .send(transport, pdu, Level::AuthPriv, transport.attempts())
            .await;
        Some(outcome.and_then(|answer| match answer {
            Answer::Response(response) => Ok(response),
            Answer::Report(kind) => Err(kind.into_error()),
        }))
    }

    /// One empty GET at `level`, sent once.
    async fn probe(
        &mut self,
        transport: &mut Transport,
        level: Level,
    ) -> Result<Answer, SnmpError> {
        let probe = Pdu::request(PduKind::Get, self.msg_ids.next(), &[]);
        self.send(transport, &probe, level, 1).await
    }

    fn use_priv_protocol(&mut self, protocol: PrivProtocol) {
        if let Some((current, _)) = &mut self.privacy {
            *current = protocol;
        }
        // The cached keys were derived for the previous protocol.
        self.localized.clear();
    }

    /// Sends `pdu` at `level` and returns the agent's answer, discovering the engine and
    /// resynchronising with it as needed — each at most once per call.
    async fn send(
        &mut self,
        transport: &mut Transport,
        pdu: &Pdu,
        level: Level,
        attempts: u32,
    ) -> Result<Answer, SnmpError> {
        let mut resynced = false;
        let mut rediscovered = false;
        loop {
            if self.engine.is_none() {
                self.discover(transport).await?;
            }
            self.derive_keys()?;
            let msg_id = self.msg_ids.next();
            let datagram = self.encode(pdu, level, msg_id)?;
            let reply = transport
                .exchange(&datagram, attempts, |received| {
                    self.accept(received, msg_id, pdu.request_id, level)
                })
                .await?;

            // An authenticated reply to this very message is fresh by construction (the
            // digest covers our msgID), so its clock is the agent's clock right now.
            if reply.authenticated {
                self.synchronise(reply.engine_boots, reply.engine_time);
            }
            match reply.answer {
                Answer::Report(ReportKind::NotInTimeWindow) if !resynced => {
                    resynced = true;
                    if !reply.authenticated {
                        // Agents should sign this report; one that does not still tells us
                        // its clock, and the next request will show whether it was honest.
                        self.synchronise(reply.engine_boots, reply.engine_time);
                    }
                    debug!(agent = %transport.target(), "engine clock resynchronised");
                }
                Answer::Report(ReportKind::UnknownEngineId) if !rediscovered => {
                    rediscovered = true;
                    self.engine = None;
                    debug!(agent = %transport.target(), "engine ID changed, rediscovering");
                }
                answer => return Ok(answer),
            }
        }
    }

    /// RFC 3414 §4: an unauthenticated request with an empty engine ID makes the agent report
    /// its engine ID (and, unauthenticated, its boots and time).
    async fn discover(&mut self, transport: &mut Transport) -> Result<(), SnmpError> {
        let msg_id = self.msg_ids.next();
        let scoped = ScopedPdu {
            context_engine_id: Vec::new(),
            context_name: Vec::new(),
            pdu: Pdu::request(PduKind::Get, self.msg_ids.next(), &[]),
        };
        let probe = OutgoingMessage {
            msg_id,
            flags: FLAG_REPORTABLE,
            engine_id: &[],
            engine_boots: 0,
            engine_time: 0,
            user: &[],
            scoped_pdu: &scoped,
        }
        .encode(None, 0)?;

        let engine = transport
            .exchange(&probe, transport.attempts(), |datagram| {
                let Ok(message) = V3Message::decode(datagram) else {
                    return Ok(None);
                };
                if message.msg_id != msg_id {
                    return Ok(None);
                }
                if message.engine_id.is_empty() {
                    return Err(SnmpError::decode(
                        "the agent answered engine discovery without an engine ID",
                    ));
                }
                Ok(Some(Engine {
                    id: message.engine_id.to_vec(),
                    boots: message.engine_boots,
                    time: message.engine_time,
                    synced_at: Instant::now(),
                }))
            })
            .await?;
        debug!(agent = %transport.target(), engine_boots = engine.boots, "engine discovered");
        self.engine = Some(engine);
        Ok(())
    }

    /// Makes sure the localised keys for the current engine are in the cache.
    fn derive_keys(&mut self) -> Result<(), SnmpError> {
        let (Some((auth_protocol, auth_passphrase)), Some(engine)) = (&self.auth, &self.engine)
        else {
            return Ok(());
        };
        if self.localized.contains_key(&engine.id) {
            return Ok(());
        }
        if self.master_keys.is_none() {
            let privacy = match &self.privacy {
                Some((_, passphrase)) => Some(auth_protocol.master_key(passphrase.as_bytes())?),
                None => None,
            };
            self.master_keys = Some(MasterKeys {
                auth: auth_protocol.master_key(auth_passphrase.as_bytes())?,
                privacy,
            });
        }
        let Some(master) = &self.master_keys else {
            return Ok(());
        };
        let privacy = match (&self.privacy, &master.privacy) {
            (Some((protocol, _)), Some(master_key)) => Some((
                *protocol,
                protocol.localized_key(*auth_protocol, master_key, &engine.id),
            )),
            _ => None,
        };
        let keys = LocalizedKeys {
            auth_protocol: *auth_protocol,
            auth_key: auth_protocol.localize(&master.auth, &engine.id),
            privacy,
        };
        self.localized.insert(engine.id.clone(), keys);
        Ok(())
    }

    /// How many engines' keys are in the cache.
    #[cfg(test)]
    pub fn cached_key_sets(&self) -> usize {
        self.localized.len()
    }

    /// The keys for the current engine, if this user authenticates at all.
    fn keys(&self) -> Option<&LocalizedKeys> {
        self.localized.get(&self.engine.as_ref()?.id)
    }

    fn synchronise(&mut self, boots: u32, time: u32) {
        if let Some(engine) = &mut self.engine {
            engine.boots = boots;
            engine.time = time;
            engine.synced_at = Instant::now();
        }
    }

    fn encode(&mut self, pdu: &Pdu, level: Level, msg_id: i32) -> Result<Vec<u8>, SnmpError> {
        self.salt = self.salt.wrapping_add(1);
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| SnmpError::Auth("internal error: engine not discovered".to_owned()))?;
        let scoped = ScopedPdu {
            context_engine_id: engine.id.clone(),
            context_name: self.context_name.clone(),
            pdu: pdu.clone(),
        };
        OutgoingMessage {
            msg_id,
            flags: level.flags() | FLAG_REPORTABLE,
            engine_id: &engine.id,
            engine_boots: engine.boots,
            engine_time: engine.current_time(),
            user: &self.user,
            scoped_pdu: &scoped,
        }
        .encode(self.keys(), self.salt)
    }

    /// Decides whether `datagram` is the reply to the message `msg_id` we sent at `level`.
    fn accept(
        &self,
        datagram: &[u8],
        msg_id: i32,
        request_id: i32,
        level: Level,
    ) -> Result<Option<Reply>, SnmpError> {
        let Ok(message) = V3Message::decode(datagram) else {
            return Ok(None);
        };
        if message.msg_id != msg_id {
            return Ok(None);
        }
        let keys = self.keys();
        if message.is_authenticated() && !keys.is_some_and(|keys| message.verify(datagram, keys)) {
            // Forged, corrupted, or signed with a key we do not hold: not ours to trust.
            return Ok(None);
        }

        // An authentic message that will not decode is a real error; an unauthenticated one
        // (when we do authenticate) is just noise.
        let trusted = message.is_authenticated() || level == Level::NoAuthNoPriv;
        let scoped = match message.scoped_pdu(keys) {
            Ok(scoped) => scoped,
            Err(e) if trusted => return Err(e),
            Err(_) => return Ok(None),
        };

        let answer = match scoped.pdu.kind {
            // Reports may legitimately arrive at a lower security level than the request.
            PduKind::Report => Answer::Report(ReportKind::of(&scoped.pdu.varbinds)),
            PduKind::Response => {
                // A response must come at the security level of its request (RFC 3412 §7.2).
                let same_level = message.flags & (FLAG_AUTH | FLAG_PRIV) == level.flags();
                if !same_level || scoped.pdu.request_id != request_id {
                    return Ok(None);
                }
                Answer::Response(scoped.pdu)
            }
            _ => return Ok(None),
        };
        Ok(Some(Reply {
            authenticated: message.is_authenticated(),
            engine_boots: message.engine_boots,
            engine_time: message.engine_time,
            answer,
        }))
    }
}
