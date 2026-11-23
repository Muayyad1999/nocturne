//! One forwarder's worker: take what has queued up, send it, say how it went.
//!
//! This is the only place a forwarder waits — for a connection, for a
//! collector to take what is written, between attempts. The ingest path
//! never comes here; it puts entries on the queue this worker reads and
//! counts the ones that did not fit ([`super::offer`]).
//!
//! **What an error may say.** For the syslog kinds the destination is in the
//! forwarder's document, and the engine's own sentence about the failure is
//! passed on. For `https_json` the destination is a secret, and the HTTP
//! client's error text contains it; so does, potentially, anything the far
//! end answers. Those errors are written here from the *kind* of failure
//! and nothing else: a status code, an integer the collector returned, a
//! sentence of ours.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use nocturne_app::App;
use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_netprobe::syslog::send::{
    Framing, SyslogSender, Target, Transport, Trust, authorities_from_pem,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use tokio::sync::mpsc::Receiver;

use super::{Counters, Running, wire};
use crate::model::{ForwardFormat, ForwardKind, Forwarder};

/// Entries sent in one go: one write on a stream, one request over HTTPS.
pub(crate) const BATCH: usize = 200;
/// Attempts per batch, the first included. After the last the batch is
/// dropped and counted: holding on to it would only make the entries behind
/// it wait for a collector that is not there.
const ATTEMPTS: u32 = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A collector that takes nothing of a batch for this long has stopped
/// reading. Short enough that the queue behind it does not fill for long
/// before the forwarder says so; long enough for a busy indexer to breathe.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of a refusal's body is read, to find the collector's own error
/// number in it. Nothing else of it is ever used.
const ANSWER_LIMIT: usize = 4 * 1024;

/// A batch that did not all leave.
pub(crate) struct Failure {
    /// How many of its entries are known to have left all the same.
    pub sent: usize,
    pub reason: String,
    /// Whether sending the rest again can help. Not for UDP: each datagram
    /// was tried, and trying the batch again would send the others twice.
    pub retry: bool,
}

impl Failure {
    fn retry(reason: impl Into<String>) -> Failure {
        Failure {
            sent: 0,
            reason: reason.into(),
            retry: true,
        }
    }
}

/// The way out for one forwarder.
pub(crate) enum Outlet {
    Syslog(Box<SyslogSender>),
    Https(Box<HttpOutlet>),
}

pub(crate) struct HttpOutlet {
    client: reqwest::Client,
    url: reqwest::Url,
    authorization: Option<HeaderValue>,
}

/// The `Authorization` header for a token. An HTTP event collector expects
/// `Splunk <token>`; a token written with its own scheme (`Bearer …`) is
/// sent as written.
pub(crate) fn authorization(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let has_scheme = token
        .split_once(char::is_whitespace)
        .is_some_and(|(scheme, rest)| !scheme.is_empty() && !rest.trim().is_empty());
    Some(if has_scheme {
        token.to_string()
    } else {
        format!("Splunk {token}")
    })
}

/// Where a syslog forwarder sends, as the engine's sender wants it.
pub(crate) fn syslog_target(forwarder: &Forwarder) -> Result<Target, String> {
    let transport = match forwarder.kind {
        ForwardKind::SyslogUdp => Transport::Udp,
        ForwardKind::SyslogTcp => Transport::Tcp,
        ForwardKind::SyslogTls => Transport::Tls,
        ForwardKind::HttpsJson => return Err("this forwarder does not send syslog".into()),
    };
    let mut target = Target::new(forwarder.host.clone(), forwarder.port, transport);
    // RFC 5425 has syslog over TLS octet-counted. Everything else on a
    // stream is a message to a line, which is what a plain TCP input and
    // anything that reads JSON lines expects.
    target.framing = match (transport, forwarder.format) {
        (Transport::Tls, ForwardFormat::Rfc5424) => Framing::OctetCounted,
        _ => Framing::Newline,
    };
    target.connect_timeout = CONNECT_TIMEOUT;
    target.write_timeout = WRITE_TIMEOUT;
    if let Some(pem) = authority(forwarder) {
        target.trust =
            Trust::Authorities(authorities_from_pem(pem).map_err(|err| err.to_string())?);
    }
    Ok(target)
}

fn authority(forwarder: &Forwarder) -> Option<&str> {
    forwarder
        .ca_pem
        .as_deref()
        .filter(|pem| !pem.trim().is_empty())
}

impl Outlet {
    /// Set up the way out. Connects nothing; for an HTTPS forwarder this is
    /// where its secret is read.
    pub(crate) fn open(app: &App, forwarder: &Forwarder) -> Result<Outlet, String> {
        if forwarder.kind.is_syslog() {
            let sender =
                SyslogSender::new(syslog_target(forwarder)?).map_err(|err| err.to_string())?;
            return Ok(Outlet::Syslog(Box::new(sender)));
        }
        // The vault's own message names the secret's id; the code is enough.
        let secret = super::read_secret(app, &forwarder.id)
            .map_err(|err| {
                format!(
                    "the collector's URL could not be read from the vault ({})",
                    err.code
                )
            })?
            .ok_or("this forwarder has no URL set")?;
        let url = reqwest::Url::parse(&secret.url)
            .map_err(|_| "the URL stored for this forwarder is not a URL")?;
        let authorization = match authorization(&secret.token) {
            Some(value) => {
                let mut value = HeaderValue::from_str(&value).map_err(
                    |_| "the token stored for this forwarder cannot be sent in a header",
                )?;
                value.set_sensitive(true);
                Some(value)
            }
            None => None,
        };
        // reqwest is built without a TLS provider of its own; ring is the one
        // this application uses everywhere. Someone else may have installed it.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            // No connection is kept between requests, and HTTP/2 is not
            // spoken. Both are about log lines, not about speed: the client
            // library names the host it talks to in its own debug and trace
            // lines, and it writes the ones about kept connections from
            // tasks of its own, where they cannot be silenced (see `post`).
            // A batch is up to two hundred entries, so this is a handshake
            // per batch, not per entry.
            .pool_max_idle_per_host(0)
            .http1_only()
            // Log text goes to the URL that was given and nowhere a reply
            // points to instead.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("nocturne/", env!("CARGO_PKG_VERSION")));
        if let Some(pem) = authority(forwarder) {
            let authorities = reqwest::Certificate::from_pem_bundle(pem.as_bytes()).map_err(
                |_| "the certificate authority given for this forwarder is not valid PEM",
            )?;
            builder = builder.tls_certs_only(authorities);
        }
        let client = builder
            .build()
            .map_err(|_| "the HTTPS client could not be set up")?;
        Ok(Outlet::Https(Box::new(HttpOutlet {
            client,
            url,
            authorization,
        })))
    }

    /// Send a batch of entries, in order.
    pub(crate) async fn send(
        &mut self,
        forwarder: &Forwarder,
        entries: &[Arc<LogEntry>],
        name_of: &(dyn Fn(&str) -> Option<String> + Sync),
    ) -> Result<(), Failure> {
        let named: Vec<Option<String>> = entries
            .iter()
            .map(|entry| entry.device_id.as_deref().and_then(name_of))
            .collect();
        match self {
            Outlet::Syslog(sender) => {
                let limit = match forwarder.kind {
                    ForwardKind::SyslogUdp => wire::DATAGRAM_LIMIT,
                    _ => wire::STREAM_LIMIT,
                };
                let messages: Vec<Vec<u8>> = entries
                    .iter()
                    .zip(&named)
                    .map(|(entry, device)| match forwarder.format {
                        ForwardFormat::Rfc5424 => wire::rfc5424(entry, device.as_deref(), limit),
                        ForwardFormat::Json => wire::json(entry, device.as_deref(), limit),
                    })
                    .collect();
                sender.send(&messages).await.map_err(|failed| Failure {
                    sent: failed.sent,
                    reason: failed.error.to_string(),
                    retry: forwarder.kind != ForwardKind::SyslogUdp,
                })
            }
            Outlet::Https(outlet) => {
                let mut body = Vec::new();
                for (entry, device) in entries.iter().zip(&named) {
                    body.extend(wire::hec(entry, device.as_deref(), wire::STREAM_LIMIT));
                    body.push(b'\n');
                }
                outlet.post(body).await.map_err(Failure::retry)
            }
        }
    }
}

impl HttpOutlet {
    async fn post(&self, body: Vec<u8>) -> Result<(), String> {
        use tracing::instrument::WithSubscriber;
        let request = async {
            let mut request = self
                .client
                .post(self.url.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(body);
            if let Some(authorization) = &self.authorization {
                request = request.header(AUTHORIZATION, authorization.clone());
            }
            let response = request.send().await.map_err(|err| describe(&err))?;
            let status = response.status();
            if status.is_success() {
                return Ok(());
            }
            let mut reason = format!("the collector answered HTTP {}", status.as_u16());
            if status.is_redirection() {
                reason.push_str(
                    " (a redirect, which is not followed: log text only goes to the URL that was given)",
                );
            } else if let Some(code) = collector_code(response).await {
                reason.push_str(&format!(" (its own error code: {code})"));
            }
            Err(reason)
        };
        // The HTTP client and what it is built on log the host and the
        // address they connect to, at debug and trace level. For this
        // request those lines go nowhere: the destination is a secret, and
        // somebody raising the log level to chase another problem must not
        // find it in the log. What went wrong is in the returned reason.
        request
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
    }
}

/// The number an HTTP event collector puts in a refusal (`{"text": …,
/// "code": 4}`), when there is one. A number is all that is taken from the
/// answer: its text is the far end's to write, and could be anything.
async fn collector_code(mut response: reqwest::Response) -> Option<i64> {
    let mut answer = Vec::new();
    let read = async {
        while answer.len() < ANSWER_LIMIT {
            match response.chunk().await {
                Ok(Some(chunk)) => answer.extend_from_slice(&chunk),
                _ => break,
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), read).await;
    answer.truncate(ANSWER_LIMIT);
    serde_json::from_slice::<serde_json::Value>(&answer)
        .ok()?
        .get("code")?
        .as_i64()
}

/// Why a request failed, from what kind of failure it was. `reqwest`'s own
/// message is never used: it carries the URL.
fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "the collector did not answer in time".into();
    }
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = cause {
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return tls_failure(tls);
        }
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            // A failed handshake arrives as an I/O error with the TLS error
            // inside — inside another I/O error, as the HTTP client hands it
            // on. What an I/O error wraps is not its `source()` (that is the
            // source of what it wraps), so it is unwrapped here by hand, or a
            // certificate that was refused reads as a collector that could
            // not be reached.
            let mut wrapped = io.get_ref();
            while let Some(inner) = wrapped {
                if let Some(tls) = inner.downcast_ref::<rustls::Error>() {
                    return tls_failure(tls);
                }
                wrapped = inner
                    .downcast_ref::<std::io::Error>()
                    .and_then(std::io::Error::get_ref);
            }
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    return "the collector refused the connection".into();
                }
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted => {
                    return "the collector closed the connection".into();
                }
                std::io::ErrorKind::HostUnreachable | std::io::ErrorKind::NetworkUnreachable => {
                    return "there is no route to the collector".into();
                }
                _ => {}
            }
        }
        cause = current.source();
    }
    if error.is_connect() {
        "the collector could not be reached".into()
    } else if error.is_body() || error.is_decode() {
        "the collector's answer could not be read".into()
    } else {
        "the request to the collector failed".into()
    }
}

fn tls_failure(error: &rustls::Error) -> String {
    use rustls::CertificateError as Certificate;
    let why = match error {
        rustls::Error::InvalidCertificate(reason) => match reason {
            Certificate::Expired | Certificate::ExpiredContext { .. } => {
                "its certificate has expired"
            }
            Certificate::NotValidYet | Certificate::NotValidYetContext { .. } => {
                "its certificate is not valid yet"
            }
            Certificate::UnknownIssuer => {
                "its certificate is not signed by an authority this forwarder trusts"
            }
            Certificate::NotValidForName | Certificate::NotValidForNameContext { .. } => {
                "its certificate is not for the host name in the URL"
            }
            _ => "its certificate was not accepted",
        },
        _ => "the two sides could not agree on a secure connection",
    };
    format!("the TLS handshake with the collector failed: {why}")
}

/// `logs.forward.test`: one event that says it is a test, sent now, on a
/// connection of its own.
pub(crate) async fn test(app: &App, forwarder: &Forwarder) -> Result<(), String> {
    let mut outlet = Outlet::open(app, forwarder)?;
    let event = Arc::new(wire::test_entry(now_ms(), &forwarder.name));
    outlet
        .send(forwarder, &[event], &|_| None)
        .await
        .map_err(|failure| failure.reason)
}

/// How long to wait before trying again, by how many attempts in a row
/// have failed: 1 s, 2 s, 4 s … 30 s. A collector that is down for the
/// night costs a connection attempt every half minute.
fn backoff(failures: u32) -> Duration {
    Duration::from_millis((500_u64 << failures.clamp(1, 7)).min(30_000))
}

fn mark(app: &App, counters: &Counters, outcome: Result<(), String>) {
    let (now, error) = match outcome {
        Ok(()) => (super::OK, None),
        Err(reason) => (super::FAILING, Some(reason)),
    };
    *counters.last_error.lock() = error;
    // Only a change is worth an event: a forwarder that works says so once.
    if counters.outcome.swap(now, Ordering::Relaxed) != now {
        crate::status::publish(app);
    }
}

/// The worker. Runs until the forwarder is stopped (saved again, switched
/// off, deleted) or the application shuts down.
pub(crate) async fn run(app: App, running: Arc<Running>, mut queue: Receiver<Arc<LogEntry>>) {
    let counters = running.counters.clone();
    let forwarder = &running.forwarder;
    let mut outlet: Option<Outlet> = None;
    let mut failures = 0_u32;
    'batches: loop {
        let first = tokio::select! {
            _ = running.stop.cancelled() => break,
            entry = queue.recv() => match entry {
                Some(entry) => entry,
                None => break,
            },
        };
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match queue.try_recv() {
                Ok(entry) => batch.push(entry),
                Err(_) => break,
            }
        }
        // Names are looked up here, off the ingest path. An inventory that
        // cannot be read costs the names, not the entries.
        let index = crate::attribute::refresh(&app, now_ms(), crate::ingest::INDEX_MAX_AGE_MS).ok();
        let name_of = |device_id: &str| {
            index
                .as_ref()
                .and_then(|index| index.device(device_id))
                .map(|device| device.name.clone())
        };

        let mut attempt = 0;
        loop {
            attempt += 1;
            let sending = async {
                match &mut outlet {
                    Some(outlet) => outlet.send(forwarder, &batch, &name_of).await,
                    None => match Outlet::open(&app, forwarder) {
                        Ok(opened) => {
                            outlet
                                .insert(opened)
                                .send(forwarder, &batch, &name_of)
                                .await
                        }
                        Err(reason) => Err(Failure::retry(reason)),
                    },
                }
            };
            let sent = tokio::select! {
                // Stopped in the middle of sending: the forwarder was saved
                // again or deleted. What this task says from here on would be
                // said about a forwarder that no longer exists in this form —
                // its successor shares these counters — so it says nothing
                // more than that the batch did not leave.
                _ = running.stop.cancelled() => {
                    counters.dropped.fetch_add(batch.len() as u64, Ordering::Relaxed);
                    break 'batches;
                }
                sent = sending => sent,
            };
            match sent {
                Ok(()) => {
                    counters
                        .sent
                        .fetch_add(batch.len() as u64, Ordering::Relaxed);
                    counters.last_at.store(now_ms(), Ordering::Relaxed);
                    failures = 0;
                    mark(&app, &counters, Ok(()));
                    break;
                }
                Err(failure) => {
                    failures += 1;
                    let left = batch.len().saturating_sub(failure.sent);
                    if failure.sent > 0 {
                        counters
                            .sent
                            .fetch_add(failure.sent as u64, Ordering::Relaxed);
                        counters.last_at.store(now_ms(), Ordering::Relaxed);
                    }
                    mark(&app, &counters, Err(failure.reason));
                    if !failure.retry || attempt >= ATTEMPTS {
                        counters.dropped.fetch_add(left as u64, Ordering::Relaxed);
                        break;
                    }
                    tokio::select! {
                        _ = running.stop.cancelled() => {
                            counters.dropped.fetch_add(left as u64, Ordering::Relaxed);
                            break 'batches;
                        }
                        _ = tokio::time::sleep(backoff(failures)) => {}
                    }
                }
            }
        }
    }
    // Stopped with entries still waiting: they were for a forwarder that no
    // longer exists in this form, and they are counted rather than lost
    // quietly.
    queue.close();
    let mut waiting = 0_u64;
    while queue.try_recv().is_ok() {
        waiting += 1;
    }
    if waiting > 0 {
        counters.dropped.fetch_add(waiting, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_token_is_sent_the_way_an_event_collector_expects_it() {
        assert_eq!(authorization("abc-123").as_deref(), Some("Splunk abc-123"));
        assert_eq!(
            authorization("  Bearer abc-123 ").as_deref(),
            Some("Bearer abc-123")
        );
        assert_eq!(authorization(""), None);
        assert_eq!(authorization("   "), None);
    }

    #[test]
    fn the_wait_between_attempts_grows_and_stops_growing() {
        let waits: Vec<u64> = (1..=9).map(|n| backoff(n).as_millis() as u64).collect();
        assert_eq!(
            waits,
            vec![
                1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000, 30_000
            ]
        );
    }

    #[test]
    fn syslog_over_tls_is_octet_counted_and_json_is_a_document_to_a_line() {
        let mut forwarder = Forwarder {
            kind: ForwardKind::SyslogTls,
            host: "collector.example.net".into(),
            port: 6514,
            ..Forwarder::default()
        };
        assert_eq!(
            syslog_target(&forwarder).unwrap().framing,
            Framing::OctetCounted
        );
        forwarder.format = ForwardFormat::Json;
        assert_eq!(syslog_target(&forwarder).unwrap().framing, Framing::Newline);
        forwarder.kind = ForwardKind::SyslogTcp;
        forwarder.format = ForwardFormat::Rfc5424;
        assert_eq!(syslog_target(&forwarder).unwrap().framing, Framing::Newline);
    }
}
