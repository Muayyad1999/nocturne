//! Traceroute: the routers between this machine and a destination.
//!
//! On Linux the trace runs natively and needs no privileges: UDP probes with increasing TTL,
//! whose ICMP answers the kernel hands back through the sockets' error queue. Elsewhere (and
//! if the native method is unavailable) the system `traceroute` / `tracert` is driven and its
//! output parsed as it appears.

#[cfg(target_os = "linux")]
mod native;
mod parse;
mod system;

use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::{ProbeError, dns};

/// Options of [`traceroute`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct TraceOptions {
    /// The largest TTL tried.
    pub max_hops: u8,
    /// Probes per hop. Windows `tracert` always sends 3.
    pub probes: u8,
    /// How long each probe waits for an answer.
    pub timeout_ms: u64,
    /// Look up the name of every router.
    pub resolve_names: bool,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            max_hops: 30,
            probes: 3,
            timeout_ms: 2000,
            resolve_names: false,
        }
    }
}

/// One hop of the path.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hop {
    pub ttl: u8,
    /// The router that answered (the first one, if several did); `None` when none answered.
    pub address: Option<String>,
    pub hostname: Option<String>,
    /// One entry per probe; `None` for a probe without answer.
    pub rtts_ms: Vec<Option<f64>>,
    /// Empty for an ordinary hop. Otherwise what is unusual about it: `no reply`, an ICMP error
    /// such as `host unreachable`, or the other routers that answered for the same TTL.
    pub note: String,
}

/// The result of a [`traceroute`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TraceReport {
    /// The host as it was given.
    pub target: String,
    /// The address that was traced.
    pub address: String,
    pub hops: Vec<Hop>,
    /// Whether the destination answered. As with every traceroute, an answer for the
    /// destination that comes from another address counts: routers answer from the interface
    /// nearest to the tracer (the hop's `note` says when that happened).
    pub reached: bool,
    /// `native-udp`, `system-traceroute` or `system-tracert`.
    pub method: String,
}

/// How long a reverse lookup of a router may take.
const NAME_TIMEOUT: Duration = Duration::from_millis(1500);
/// Note on a final hop whose answer did not come from the traced address itself.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const OTHER_ADDRESS_NOTE: &str = "answered for the destination from another address";

/// Traces the route to `host`. Hops are streamed to `progress` as they are discovered.
pub async fn traceroute(
    host: &str,
    options: &TraceOptions,
    progress: Option<UnboundedSender<Hop>>,
) -> Result<TraceReport, ProbeError> {
    if options.max_hops == 0 {
        return Err(ProbeError::invalid("max_hops must be at least 1"));
    }
    if options.probes == 0 {
        return Err(ProbeError::invalid(
            "the number of probes per hop must be at least 1",
        ));
    }
    if options.timeout_ms == 0 {
        return Err(ProbeError::invalid(
            "the traceroute timeout must be at least 1 ms",
        ));
    }
    let address = dns::resolve_one(host).await?;
    let mut trace = Trace {
        target: address,
        hops: Vec::new(),
        reached: false,
        progress,
    };

    #[cfg(target_os = "linux")]
    let method = match native::run(&mut trace, options).await {
        Ok(()) => "native-udp",
        Err(native::Failure::Unavailable(reason)) => {
            tracing::debug!(%reason, "native traceroute unavailable; using the system program");
            trace.hops.clear();
            trace.reached = false;
            system::run(&mut trace, options).await?
        }
        Err(native::Failure::Probe(error)) => return Err(error),
    };
    #[cfg(not(target_os = "linux"))]
    let method = system::run(&mut trace, options).await?;

    Ok(TraceReport {
        target: host.trim().to_owned(),
        address: address.to_string(),
        hops: trace.hops,
        reached: trace.reached,
        method: method.to_owned(),
    })
}

/// A trace in progress.
struct Trace {
    target: IpAddr,
    hops: Vec<Hop>,
    reached: bool,
    progress: Option<UnboundedSender<Hop>>,
}

impl Trace {
    /// Records a finished hop and streams it.
    fn push(&mut self, hop: Hop) {
        if let Some(progress) = &self.progress {
            let _ = progress.send(hop.clone());
        }
        self.hops.push(hop);
    }
}

/// What one probe of a hop found.
#[derive(Debug, Clone, PartialEq)]
struct ProbeResult {
    /// Who answered.
    address: Option<IpAddr>,
    rtt_ms: Option<f64>,
    /// The ICMP error, if the answer was not the expected "TTL exceeded" / "port unreachable".
    problem: Option<String>,
}

impl ProbeResult {
    /// A hop that said nothing. Used by the tests below, which a plain
    /// build does not compile.
    #[allow(dead_code)]
    const SILENT: ProbeResult = ProbeResult {
        address: None,
        rtt_ms: None,
        problem: None,
    };
}

/// Summarises the probes of one TTL as a hop.
async fn build_hop(ttl: u8, probes: &[ProbeResult], resolve_names: bool) -> Hop {
    let mut responders: Vec<IpAddr> = Vec::new();
    let mut problems: Vec<&str> = Vec::new();
    for probe in probes {
        if let Some(address) = probe
            .address
            .filter(|address| !responders.contains(address))
        {
            responders.push(address);
        }
        if let Some(problem) = probe.problem.as_deref().filter(|p| !problems.contains(p)) {
            problems.push(problem);
        }
    }
    let mut notes: Vec<String> = problems
        .iter()
        .map(|problem| (*problem).to_owned())
        .collect();
    if let [_, others @ ..] = responders.as_slice()
        && !others.is_empty()
    {
        let others: Vec<String> = others.iter().map(ToString::to_string).collect();
        notes.push(format!("also answered by {}", others.join(", ")));
    }
    if responders.is_empty() && notes.is_empty() {
        notes.push("no reply".to_owned());
    }
    let address = responders.first().copied();
    Hop {
        ttl,
        address: address.map(|address| address.to_string()),
        hostname: match address {
            Some(address) if resolve_names => dns::reverse(address, NAME_TIMEOUT).await,
            _ => None,
        },
        rtts_ms: probes.iter().map(|probe| probe.rtt_ms).collect(),
        note: notes.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn answered(address: &str, rtt_ms: f64) -> ProbeResult {
        ProbeResult {
            address: Some(address.parse().unwrap()),
            rtt_ms: Some(rtt_ms),
            problem: None,
        }
    }

    #[tokio::test]
    async fn hops_are_summarised() {
        let ordinary = [
            answered("10.0.0.1", 1.5),
            ProbeResult::SILENT,
            answered("10.0.0.1", 1.7),
        ];
        assert_eq!(
            build_hop(3, &ordinary, false).await,
            Hop {
                ttl: 3,
                address: Some("10.0.0.1".into()),
                hostname: None,
                rtts_ms: vec![Some(1.5), None, Some(1.7)],
                note: String::new(),
            }
        );

        let silent = [ProbeResult::SILENT, ProbeResult::SILENT];
        assert_eq!(
            build_hop(4, &silent, true).await,
            Hop {
                ttl: 4,
                address: None,
                hostname: None,
                rtts_ms: vec![None, None],
                note: "no reply".into()
            }
        );

        let balanced = [
            answered("10.0.0.1", 1.0),
            answered("10.0.0.2", 1.1),
            answered("10.0.0.3", 1.2),
        ];
        let hop = build_hop(5, &balanced, false).await;
        assert_eq!(hop.address.as_deref(), Some("10.0.0.1"));
        assert_eq!(hop.note, "also answered by 10.0.0.2, 10.0.0.3");

        let unreachable = [
            ProbeResult {
                problem: Some("host unreachable".into()),
                ..answered("10.0.0.9", 3000.0)
            },
            ProbeResult {
                problem: Some("host unreachable".into()),
                ..answered("10.0.0.9", 3001.0)
            },
        ];
        assert_eq!(
            build_hop(6, &unreachable, false).await.note,
            "host unreachable"
        );
    }

    #[test]
    fn options_default_and_deserialize_partially() {
        let options = TraceOptions::default();
        assert_eq!(
            (
                options.max_hops,
                options.probes,
                options.timeout_ms,
                options.resolve_names
            ),
            (30, 3, 2000, false)
        );
        let partial: TraceOptions = serde_json::from_str(r#"{"max_hops": 8}"#).unwrap();
        assert_eq!(
            partial,
            TraceOptions {
                max_hops: 8,
                ..TraceOptions::default()
            }
        );
    }

    #[tokio::test]
    async fn invalid_options_are_rejected() {
        for options in [
            TraceOptions {
                max_hops: 0,
                ..TraceOptions::default()
            },
            TraceOptions {
                probes: 0,
                ..TraceOptions::default()
            },
            TraceOptions {
                timeout_ms: 0,
                ..TraceOptions::default()
            },
        ] {
            let error = traceroute("127.0.0.1", &options, None).await.unwrap_err();
            assert!(matches!(error, ProbeError::Invalid(_)), "{error:?}");
        }
        assert!(matches!(
            traceroute("no-such-host.invalid", &TraceOptions::default(), None).await,
            Err(ProbeError::Resolve)
        ));
    }

    /// The system program, driven directly, against loopback.
    #[tokio::test]
    async fn system_traceroute_reaches_loopback() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut trace = Trace {
            target: "127.0.0.1".parse().unwrap(),
            hops: Vec::new(),
            reached: false,
            progress: Some(sender),
        };
        let options = TraceOptions {
            max_hops: 5,
            probes: 2,
            timeout_ms: 1000,
            resolve_names: false,
        };
        let method = match system::run(&mut trace, &options).await {
            Ok(method) => method,
            Err(ProbeError::Unsupported(reason)) => {
                eprintln!("skipping: {reason}");
                return;
            }
            // Raw-socket based traceroute builds need privileges this test may not have.
            Err(ProbeError::Permission(reason)) => {
                eprintln!("skipping: {reason}");
                return;
            }
            Err(error) => panic!("system traceroute failed: {error}"),
        };
        assert!(method.starts_with("system-"), "{method}");
        assert!(trace.reached);
        assert_eq!(trace.hops.len(), 1, "{:?}", trace.hops);
        let hop = &trace.hops[0];
        assert_eq!((hop.ttl, hop.address.as_deref()), (1, Some("127.0.0.1")));
        assert!(hop.rtts_ms.iter().flatten().count() >= 1, "{hop:?}");
        assert_eq!(receiver.try_recv().unwrap(), *hop);
    }

    /// The system program across the two routers of the namespace lab (`tests/lab/run.sh`).
    #[tokio::test]
    async fn lab_system_traceroute_follows_the_routed_path() {
        if std::env::var_os("NETPROBE_LAB").is_none() {
            eprintln!("skipping: not inside the lab (run tests/lab/run.sh)");
            return;
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut trace = Trace {
            target: "10.201.3.10".parse().unwrap(),
            hops: Vec::new(),
            reached: false,
            progress: Some(sender),
        };
        let options = TraceOptions {
            max_hops: 8,
            probes: 3,
            timeout_ms: 1500,
            resolve_names: false,
        };
        assert_eq!(
            system::run(&mut trace, &options).await.unwrap(),
            "system-traceroute"
        );
        assert!(trace.reached);
        let path: Vec<Option<&str>> = trace
            .hops
            .iter()
            .map(|hop| hop.address.as_deref())
            .collect();
        assert_eq!(
            path,
            [
                Some("10.201.1.254"),
                Some("10.201.2.254"),
                Some("10.201.3.10")
            ]
        );
        assert!(
            trace
                .hops
                .iter()
                .all(|hop| hop.rtts_ms.iter().flatten().count() == 3 && hop.note.is_empty())
        );
        let mut streamed = Vec::new();
        while let Ok(hop) = receiver.try_recv() {
            streamed.push(hop);
        }
        assert_eq!(streamed, trace.hops);

        // A dead end: the first router has no route and says so (`!N`). Linux routers send at
        // most five such errors in a burst; give this one time to recover from other tests.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let mut trace = Trace {
            target: "10.201.9.9".parse().unwrap(),
            hops: Vec::new(),
            reached: false,
            progress: None,
        };
        system::run(&mut trace, &options).await.unwrap();
        assert!(!trace.reached);
        assert_eq!(trace.hops[0].address.as_deref(), Some("10.201.1.254"));
        assert_eq!(trace.hops[0].note, "network unreachable");
    }
}
