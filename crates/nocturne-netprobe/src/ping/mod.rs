//! ICMP echo ("ping") with whatever this machine allows: unprivileged ICMP datagram sockets,
//! raw sockets, or the operating system's `ping` program.

pub(crate) mod icmp;
mod socket;
mod system;

use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;

use self::icmp::Family;
use self::socket::{EchoError, RawEndpoint, SocketKind};
use self::system::SystemPing;
use crate::util::{round3, summarize};
use crate::{ProbeError, dns};

/// How echo requests are sent on this machine.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Unprivileged ICMP datagram sockets (Linux with `ping_group_range`, macOS).
    IcmpDatagram,
    /// Raw ICMP sockets (root, `CAP_NET_RAW`, Administrator).
    IcmpRaw,
    /// The operating system's `ping` program.
    SystemCommand,
}

/// Sends echo requests. Cheap to clone; safe to use from many tasks at once.
#[derive(Clone)]
pub struct Pinger {
    inner: Arc<Inner>,
}

struct Inner {
    method: Method,
    raw_v4: Mutex<Option<Arc<RawEndpoint>>>,
    raw_v6: Mutex<Option<Arc<RawEndpoint>>>,
    system: SystemPing,
}

impl Pinger {
    /// Detects the best available method once: unprivileged ICMP datagram sockets, raw sockets,
    /// otherwise the operating system's `ping` program.
    pub fn new() -> Pinger {
        let can_open = |kind| {
            socket::open(kind, Family::V4).is_ok() || socket::open(kind, Family::V6).is_ok()
        };
        let method = if can_open(SocketKind::Datagram) {
            Method::IcmpDatagram
        } else if can_open(SocketKind::Raw) {
            Method::IcmpRaw
        } else {
            Method::SystemCommand
        };
        Pinger::with_method(method)
    }

    /// A pinger that uses `method` without probing for it — to compare methods, or to force the
    /// system command. Socket methods this process may not use fall back to the system command
    /// per echo, exactly as with [`Pinger::new`].
    pub fn with_method(method: Method) -> Pinger {
        Pinger {
            inner: Arc::new(Inner {
                method,
                raw_v4: Mutex::new(None),
                raw_v6: Mutex::new(None),
                system: SystemPing::new(),
            }),
        }
    }

    pub fn method(&self) -> Method {
        self.inner.method
    }

    /// One echo request to `target` carrying `payload` data bytes.
    ///
    /// `Ok` on an echo reply. Errors: [`ProbeError::Timeout`] when nothing came back in time,
    /// [`ProbeError::Unreachable`] with the ICMP reason (and the router that reported it) when
    /// the network said the target cannot be reached, anything else for local failures.
    pub async fn ping_once(
        &self,
        target: IpAddr,
        timeout: Duration,
        payload: usize,
    ) -> Result<PingReply, ProbeError> {
        if payload > icmp::MAX_PAYLOAD {
            return Err(ProbeError::invalid(format!(
                "a ping payload of {payload} bytes is too large; the maximum is {} bytes",
                icmp::MAX_PAYLOAD
            )));
        }
        let over_socket = match self.inner.method {
            Method::IcmpDatagram => socket::echo_datagram(target, timeout, payload).await,
            Method::IcmpRaw => match self.raw_endpoint(Family::of(target)) {
                Ok(endpoint) => endpoint
                    .echo(target, timeout, payload)
                    .await
                    .map_err(EchoError::Probe),
                Err(error) => Err(error),
            },
            Method::SystemCommand => Err(EchoError::SocketDenied),
        };
        match over_socket {
            Ok(reply) => Ok(reply),
            Err(EchoError::Probe(error)) => Err(error),
            Err(EchoError::SocketDenied) => self.inner.system.echo(target, timeout, payload).await,
        }
    }

    /// The shared raw socket for `family`, opened on first use and reopened if it died.
    fn raw_endpoint(&self, family: Family) -> Result<Arc<RawEndpoint>, EchoError> {
        let slot = match family {
            Family::V4 => &self.inner.raw_v4,
            Family::V6 => &self.inner.raw_v6,
        };
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(endpoint) = slot.as_ref().filter(|endpoint| endpoint.is_alive()) {
            return Ok(endpoint.clone());
        }
        let endpoint = Arc::new(RawEndpoint::open(family)?);
        *slot = Some(endpoint.clone());
        Ok(endpoint)
    }
}

impl Default for Pinger {
    fn default() -> Self {
        Pinger::new()
    }
}

impl std::fmt::Debug for Pinger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pinger")
            .field("method", &self.inner.method)
            .finish()
    }
}

/// An echo reply.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PingReply {
    /// Round-trip time. With the system command on Windows a reply printed as `time<1ms` is
    /// reported as 1.0.
    pub rtt_ms: f64,
    /// TTL / hop limit of the reply, when the platform reveals it.
    pub ttl: Option<u8>,
    /// Size of the ICMP reply: 8 header bytes plus the echoed data.
    pub bytes: usize,
}

/// Options of [`ping`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct PingOptions {
    /// Number of echo requests.
    pub count: u32,
    /// Time between the start of consecutive echoes.
    pub interval_ms: u64,
    /// How long each echo waits for its reply.
    pub timeout_ms: u64,
    /// Data bytes per echo.
    pub payload: usize,
}

impl Default for PingOptions {
    fn default() -> Self {
        PingOptions {
            count: 4,
            interval_ms: 1000,
            timeout_ms: 1000,
            payload: 56,
        }
    }
}

/// The outcome of one echo of a [`ping`] run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PingEcho {
    /// 1 for the first echo.
    pub seq: u32,
    pub rtt_ms: Option<f64>,
    pub ttl: Option<u8>,
    /// Why there was no reply (`None` on success).
    pub error: Option<String>,
}

/// The result of a [`ping`] run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PingReport {
    /// The host as it was given.
    pub target: String,
    /// The address that was pinged.
    pub address: String,
    pub method: Method,
    pub sent: u32,
    pub received: u32,
    pub loss_pct: f64,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Population standard deviation of the round-trip times (what `ping` prints as mdev / stddev).
    pub stddev_ms: Option<f64>,
    /// One entry per echo, in sequence order.
    pub echoes: Vec<PingEcho>,
}

/// The most echoes one [`ping`] run may send.
const MAX_COUNT: u32 = 100_000;

/// Resolves `host`, sends `count` echoes `interval_ms` apart and reports.
///
/// Each echo is also sent to `progress` as it completes. A run in which nothing could be sent
/// at all (no permission, no `ping` program, …) is an error rather than a report of 100 % loss.
pub async fn ping(
    pinger: &Pinger,
    host: &str,
    options: &PingOptions,
    progress: Option<UnboundedSender<PingEcho>>,
) -> Result<PingReport, ProbeError> {
    if options.count == 0 || options.count > MAX_COUNT {
        return Err(ProbeError::invalid(format!(
            "the ping count must be between 1 and {MAX_COUNT}, not {}",
            options.count
        )));
    }
    if options.timeout_ms == 0 {
        return Err(ProbeError::invalid(
            "the ping timeout must be at least 1 ms",
        ));
    }
    if options.payload > icmp::MAX_PAYLOAD {
        return Err(ProbeError::invalid(format!(
            "a ping payload of {} bytes is too large; the maximum is {} bytes",
            options.payload,
            icmp::MAX_PAYLOAD
        )));
    }
    let address = dns::resolve_one(host).await?;
    let timeout = Duration::from_millis(options.timeout_ms);
    let interval = Duration::from_millis(options.interval_ms);
    let payload = options.payload;

    let mut results: Vec<Option<Result<PingReply, ProbeError>>> =
        (0..options.count).map(|_| None).collect();
    let mut record = |index: usize, result: Result<PingReply, ProbeError>| {
        if let Some(progress) = &progress {
            // The receiver going away only means nobody is watching any more.
            let _ = progress.send(echo_of(index, &result));
        }
        results[index] = Some(result);
    };

    // Echoes run as tasks so that a slow reply does not delay the next request; dropping the
    // set (when this future is dropped) aborts them.
    let mut echoes = JoinSet::new();
    let started = Instant::now();
    for index in 0..options.count as usize {
        let due = tokio::time::Instant::from_std(started + interval * index as u32);
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(due) => break,
                Some(finished) = echoes.join_next() => {
                    let (index, result) = finished.map_err(task_failure)?;
                    record(index, result);
                }
            }
        }
        let pinger = pinger.clone();
        echoes.spawn(async move { (index, pinger.ping_once(address, timeout, payload).await) });
    }
    while let Some(finished) = echoes.join_next().await {
        let (index, result) = finished.map_err(task_failure)?;
        record(index, result);
    }

    let results: Vec<Result<PingReply, ProbeError>> = results
        .into_iter()
        .map(|result| result.expect("every echo task reports once"))
        .collect();
    let rtts: Vec<f64> = results
        .iter()
        .filter_map(|result| result.as_ref().ok().map(|reply| reply.rtt_ms))
        .collect();
    if rtts.is_empty() && results.iter().all(|result| matches!(result, Err(e) if is_local(e))) {
        let first = results.into_iter().next().expect("count is at least 1");
        return Err(first.expect_err("no echo succeeded"));
    }

    let sent = options.count;
    let received = rtts.len() as u32;
    let summary = summarize(&rtts);
    Ok(PingReport {
        target: host.trim().to_owned(),
        address: address.to_string(),
        method: pinger.method(),
        sent,
        received,
        loss_pct: round3(f64::from(sent - received) * 100.0 / f64::from(sent)),
        min_ms: summary.map(|s| s.min),
        avg_ms: summary.map(|s| s.avg),
        max_ms: summary.map(|s| s.max),
        stddev_ms: summary.map(|s| s.stddev),
        echoes: results
            .iter()
            .enumerate()
            .map(|(index, result)| echo_of(index, result))
            .collect(),
    })
}

fn echo_of(index: usize, result: &Result<PingReply, ProbeError>) -> PingEcho {
    PingEcho {
        seq: index as u32 + 1,
        rtt_ms: result.as_ref().ok().map(|reply| reply.rtt_ms),
        ttl: result.as_ref().ok().and_then(|reply| reply.ttl),
        error: result.as_ref().err().map(ToString::to_string),
    }
}

/// `true` for failures of this machine rather than answers (or silence) from the network.
fn is_local(error: &ProbeError) -> bool {
    !matches!(
        error,
        ProbeError::Timeout | ProbeError::Unreachable(_) | ProbeError::Refused
    )
}

fn task_failure(error: tokio::task::JoinError) -> ProbeError {
    ProbeError::Io(std::io::Error::other(format!("a ping task failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn options_default_and_deserialize_partially() {
        let options = PingOptions::default();
        assert_eq!(
            (options.count, options.interval_ms, options.timeout_ms, options.payload),
            (4, 1000, 1000, 56)
        );
        let partial: PingOptions = serde_json::from_str(r#"{"count": 2}"#).unwrap();
        assert_eq!(partial, PingOptions { count: 2, ..PingOptions::default() });
    }

    #[test]
    fn method_serializes_as_snake_case() {
        assert_eq!(serde_json::to_string(&Method::IcmpDatagram).unwrap(), r#""icmp_datagram""#);
        assert_eq!(serde_json::to_string(&Method::IcmpRaw).unwrap(), r#""icmp_raw""#);
        assert_eq!(serde_json::to_string(&Method::SystemCommand).unwrap(), r#""system_command""#);
    }

    #[test]
    fn echo_entries_describe_success_and_failure() {
        let ok = Ok(PingReply { rtt_ms: 1.25, ttl: Some(64), bytes: 64 });
        assert_eq!(
            echo_of(0, &ok),
            PingEcho { seq: 1, rtt_ms: Some(1.25), ttl: Some(64), error: None }
        );
        let failed = Err(ProbeError::Unreachable("host unreachable (reported by 10.0.0.1)".into()));
        assert_eq!(
            echo_of(2, &failed),
            PingEcho {
                seq: 3,
                rtt_ms: None,
                ttl: None,
                error: Some("destination unreachable: host unreachable (reported by 10.0.0.1)".into()),
            }
        );
    }

    #[tokio::test]
    async fn invalid_options_are_rejected_with_the_reason() {
        let pinger = Pinger::with_method(Method::SystemCommand);
        let bad = [
            PingOptions { count: 0, ..PingOptions::default() },
            PingOptions { timeout_ms: 0, ..PingOptions::default() },
            PingOptions { payload: 70_000, ..PingOptions::default() },
        ];
        for options in bad {
            let error = ping(&pinger, "127.0.0.1", &options, None).await.unwrap_err();
            assert!(matches!(error, ProbeError::Invalid(_)), "{options:?}: {error:?}");
        }
        let error = pinger
            .ping_once("127.0.0.1".parse().unwrap(), Duration::from_secs(1), 70_000)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("65507"), "{error}");
    }
}
