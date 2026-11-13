//! Unprivileged traceroute for Linux: UDP probes with increasing TTL. With `IP_RECVERR` set,
//! the ICMP message a probe provokes (time exceeded from a router on the way, port unreachable
//! from the destination) is queued on the probe's own socket together with the address of the
//! node that sent it.

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::io::Interest;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;

use super::{Hop, ProbeResult, Trace, TraceOptions, build_hop};
use crate::ProbeError;
use crate::ping::icmp::{self, Family};
use crate::sys::linux::{self, Origin, QueuedError};
use crate::util::millis;

/// Why the native trace did not produce a result.
pub(super) enum Failure {
    /// The mechanism is not usable here; the system program may still work.
    Unavailable(String),
    Probe(ProbeError),
}

/// The first destination port, as every traceroute uses; each probe takes the next one.
const BASE_PORT: u16 = 33_434;
/// The classic probe size: with the IP and UDP headers this makes a 60-byte packet.
const PAYLOAD: [u8; 32] = [0x40; 32];
/// How many consecutive TTLs are probed at the same time. Looking a little ahead keeps
/// silent routers from costing one full timeout each, without flooding the destination
/// (which rate-limits its ICMP answers).
const WINDOW: usize = 3;

/// The probes of one TTL, classified.
struct HopOutcome {
    hop: Hop,
    /// The destination itself answered.
    reached: bool,
    /// A router reported the destination unreachable: there is nothing beyond this hop.
    dead_end: bool,
}

pub(super) async fn run(trace: &mut Trace, options: &TraceOptions) -> Result<(), Failure> {
    let target = trace.target;
    let timeout = Duration::from_millis(options.timeout_ms);
    let (probes, resolve_names) = (options.probes, options.resolve_names);

    // Hops are probed a few at a time and reported strictly in TTL order. Dropping the set
    // (on return, or when the caller drops the trace) aborts the probes still in flight.
    let mut in_flight: JoinSet<Result<(u8, HopOutcome), Failure>> = JoinSet::new();
    let mut finished: BTreeMap<u8, HopOutcome> = BTreeMap::new();
    let mut next_ttl: u8 = 1;
    let mut report_ttl: u8 = 1;
    let mut end_seen = false;
    loop {
        while in_flight.len() < WINDOW && !end_seen && next_ttl != 0 && next_ttl <= options.max_hops
        {
            let ttl = next_ttl;
            next_ttl = next_ttl.checked_add(1).unwrap_or(0);
            in_flight.spawn(async move {
                probe_hop(target, ttl, probes, timeout, resolve_names)
                    .await
                    .map(|outcome| (ttl, outcome))
            });
        }
        let Some(joined) = in_flight.join_next().await else {
            return Ok(());
        };
        let (ttl, outcome) = joined.map_err(|error| {
            Failure::Probe(ProbeError::Io(io::Error::other(format!(
                "a traceroute task failed: {error}"
            ))))
        })??;
        end_seen |= outcome.reached || outcome.dead_end;
        finished.insert(ttl, outcome);
        while let Some(outcome) = finished.remove(&report_ttl) {
            report_ttl = report_ttl.saturating_add(1);
            trace.push(outcome.hop);
            if outcome.reached || outcome.dead_end {
                trace.reached = outcome.reached;
                return Ok(());
            }
        }
    }
}

async fn probe_hop(
    target: IpAddr,
    ttl: u8,
    probes: u8,
    timeout: Duration,
    resolve_names: bool,
) -> Result<HopOutcome, Failure> {
    let mut in_flight = JoinSet::new();
    for index in 0..probes {
        // A different destination port per probe, in the range firewalls expect.
        let sequence = (u16::from(ttl) - 1) * u16::from(probes) + u16::from(index);
        let port = BASE_PORT + sequence % 512;
        in_flight.spawn(async move { (index, probe(target, ttl, port, timeout).await) });
    }
    let mut answers: Vec<Option<Answer>> = vec![None; usize::from(probes)];
    while let Some(joined) = in_flight.join_next().await {
        let (index, answer) = joined.map_err(|error| {
            Failure::Probe(ProbeError::Io(io::Error::other(format!(
                "a traceroute probe failed: {error}"
            ))))
        })?;
        answers[usize::from(index)] = answer?;
    }

    let results: Vec<ProbeResult> = answers
        .iter()
        .map(|answer| match answer {
            Some(answer) => ProbeResult {
                address: answer.from,
                rtt_ms: Some(answer.rtt_ms),
                problem: answer.problem.map(str::to_owned),
            },
            None => ProbeResult::SILENT,
        })
        .collect();
    let answered: Vec<&Answer> = answers.iter().flatten().collect();
    let reached = answered.iter().any(|answer| answer.from_destination);
    let mut hop = build_hop(ttl, &results, resolve_names).await;
    if reached && !answered.iter().any(|answer| answer.from == Some(target)) {
        // A router answers from the interface nearest to us, whatever address of it was
        // traced; a firewall rejecting the probe looks the same.
        if !hop.note.is_empty() {
            hop.note.push_str("; ");
        }
        hop.note.push_str(super::OTHER_ADDRESS_NOTE);
    }
    Ok(HopOutcome {
        hop,
        reached,
        dead_end: !answered.is_empty() && answered.iter().all(|answer| answer.problem.is_some()),
    })
}

/// What came back for one probe.
#[derive(Debug, Clone, Copy)]
struct Answer {
    from: Option<IpAddr>,
    rtt_ms: f64,
    /// The destination answered (port unreachable, a UDP reply, or any ICMP error of its own).
    from_destination: bool,
    /// An ICMP error other than the two a traceroute expects.
    problem: Option<&'static str>,
}

/// Sends one probe and waits for what it provokes. `Ok(None)` when nothing came back.
async fn probe(
    target: IpAddr,
    ttl: u8,
    port: u16,
    timeout: Duration,
) -> Result<Option<Answer>, Failure> {
    let family = Family::of(target);
    let local: SocketAddr = match target {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = loop {
        match UdpSocket::bind(local).await {
            Ok(socket) => break socket,
            Err(error) if crate::util::is_out_of_resources(&error) => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => {
                return Err(Failure::Unavailable(format!(
                    "cannot open a UDP socket: {error}"
                )));
            }
        }
    };
    linux::enable_error_queue(&socket, family.is_v6())
        .and_then(|()| linux::set_ttl(&socket, family.is_v6(), ttl))
        .map_err(|error| {
            Failure::Unavailable(format!("cannot configure the probe socket: {error}"))
        })?;
    let network = |error| Failure::Probe(ProbeError::from_network_io(error));
    socket
        .connect(SocketAddr::new(target, port))
        .await
        .map_err(network)?;
    let sent_at = Instant::now();
    socket.send(&PAYLOAD).await.map_err(network)?;

    let wait = async {
        let mut scratch = [0u8; 256];
        loop {
            let ready = socket.ready(Interest::READABLE | Interest::ERROR).await?;
            if ready.is_error() {
                match socket.try_io(Interest::ERROR, || linux::recv_error(&socket, &mut scratch)) {
                    Ok(error) => return Ok(Event::Icmp(error)),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            if ready.is_readable() {
                match socket.try_recv(&mut scratch) {
                    // Something listens on the probe's port and answered it.
                    Ok(_) => return Ok(Event::Reply),
                    // WouldBlock, or the pending error that mirrors a queue entry (read above
                    // or on the next turn of the loop).
                    Err(_) => {}
                }
            }
        }
    };
    let event: io::Result<Event> = match tokio::time::timeout(timeout, wait).await {
        Ok(event) => event,
        Err(_) => return Ok(None),
    };
    let rtt_ms = millis(sent_at.elapsed());
    match event.map_err(network)? {
        Event::Reply => Ok(Some(Answer {
            from: Some(target),
            rtt_ms,
            from_destination: true,
            problem: None,
        })),
        Event::Icmp(error) => classify(error, family, target, rtt_ms),
    }
}

enum Event {
    Icmp(QueuedError),
    Reply,
}

fn classify(
    error: QueuedError,
    family: Family,
    target: IpAddr,
    rtt_ms: f64,
) -> Result<Option<Answer>, Failure> {
    if !matches!(error.origin, Origin::Icmp | Origin::Icmp6) {
        // The local stack refused the probe (for example "message too long").
        let error = io::Error::from_raw_os_error(error.errno);
        return Err(Failure::Probe(ProbeError::from_network_io(error)));
    }
    let (kind, code) = (error.icmp_type, error.icmp_code);
    let expected =
        icmp::is_time_exceeded(family, kind) || icmp::is_port_unreachable(family, kind, code);
    Ok(Some(Answer {
        from: error.offender,
        rtt_ms,
        from_destination: icmp::is_port_unreachable(family, kind, code)
            || error.offender == Some(target),
        problem: (!expected).then(|| icmp::error_text(family, kind, code)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(origin: Origin, kind: u8, code: u8, offender: &str) -> QueuedError {
        QueuedError {
            origin,
            errno: 113,
            icmp_type: kind,
            icmp_code: code,
            offender: Some(offender.parse().unwrap()),
            payload_len: 32,
        }
    }

    fn classified(error: QueuedError, target: &str) -> Answer {
        let target: IpAddr = target.parse().unwrap();
        match classify(error, Family::of(target), target, 1.0) {
            Ok(Some(answer)) => answer,
            _ => panic!("expected an answer"),
        }
    }

    #[test]
    fn icmp_answers_are_classified() {
        // Time exceeded from a router on the way: an ordinary hop.
        let hop = classified(queued(Origin::Icmp, 11, 0, "10.0.0.1"), "192.0.2.9");
        assert_eq!((hop.from_destination, hop.problem), (false, None));
        assert_eq!(hop.from, Some("10.0.0.1".parse().unwrap()));

        // Port unreachable from the destination: arrived.
        let arrived = classified(queued(Origin::Icmp, 3, 3, "192.0.2.9"), "192.0.2.9");
        assert_eq!((arrived.from_destination, arrived.problem), (true, None));

        // Host unreachable from the last router: a dead end, not an arrival.
        let dead_end = classified(queued(Origin::Icmp, 3, 1, "10.0.0.1"), "192.0.2.9");
        assert_eq!(
            (dead_end.from_destination, dead_end.problem),
            (false, Some("host unreachable"))
        );

        // The destination's own firewall rejecting the probe still proves we got there.
        let rejected = classified(queued(Origin::Icmp, 3, 13, "192.0.2.9"), "192.0.2.9");
        assert!(rejected.from_destination);
        assert_eq!(
            rejected.problem,
            Some("communication administratively prohibited (filtered)")
        );

        // The same over IPv6.
        let v6_hop = classified(queued(Origin::Icmp6, 3, 0, "2001:db8::1"), "2001:db8::9");
        assert_eq!((v6_hop.from_destination, v6_hop.problem), (false, None));
        let v6_arrived = classified(queued(Origin::Icmp6, 1, 4, "2001:db8::9"), "2001:db8::9");
        assert_eq!(
            (v6_arrived.from_destination, v6_arrived.problem),
            (true, None)
        );
        let v6_dead_end = classified(queued(Origin::Icmp6, 1, 0, "2001:db8::1"), "2001:db8::9");
        assert_eq!(v6_dead_end.problem, Some("no route to destination"));
    }

    #[test]
    fn local_errors_fail_the_probe() {
        let local = QueuedError {
            errno: 90,
            ..queued(Origin::Local, 0, 0, "127.0.0.1")
        };
        let target: IpAddr = "192.0.2.9".parse().unwrap();
        assert!(matches!(
            classify(local, Family::V4, target, 1.0),
            Err(Failure::Probe(_))
        ));
    }
}
