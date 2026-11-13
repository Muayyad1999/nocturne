//! Host discovery: probe every address of a range and report the ones that are alive.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinSet;

use crate::ping::{PingReply, Pinger};
use crate::util::{self, millis};
use crate::{ProbeError, dns, local, tcp};

/// Options of [`sweep`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct SweepOptions {
    /// How many hosts are probed at the same time.
    pub concurrency: usize,
    /// How long each probe (echo, TCP connection) waits for an answer.
    pub timeout_ms: u64,
    /// Extra echo requests to a host that did not answer the first one.
    pub retries: u32,
    /// TCP ports tried on every host: a host that ignores ICMP but accepts or resets one of
    /// them is alive, and the ones that accept are reported in `open_ports`.
    pub tcp_fallback_ports: Vec<u16>,
    /// Look up the name of every host found.
    pub resolve_names: bool,
    /// The most addresses one sweep may cover.
    pub limit: usize,
}

impl Default for SweepOptions {
    fn default() -> Self {
        SweepOptions {
            concurrency: 128,
            timeout_ms: 800,
            retries: 1,
            tcp_fallback_ports: vec![22, 23, 80, 443, 8291],
            resolve_names: false,
            limit: 65_536,
        }
    }
}

/// A host found by [`sweep`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HostResult {
    pub address: String,
    pub alive: bool,
    /// Round-trip time of the echo or of the TCP handshake that proved the host alive.
    pub rtt_ms: Option<f64>,
    /// What proved the host alive: `icmp`, `tcp/<port>` or `arp`.
    pub via: String,
    pub hostname: Option<String>,
    /// From the local neighbour table (hosts on this machine's own networks), attached when
    /// the sweep ends.
    pub mac: Option<String>,
    /// The `tcp_fallback_ports` that accept connections.
    pub open_ports: Vec<u16>,
}

/// How long a reverse lookup of a found host may take.
const NAME_TIMEOUT: Duration = Duration::from_millis(1500);
/// How often a probe that failed for a local, passing reason (out of sockets) is retried.
const LOCAL_RETRIES: u32 = 5;

/// Expands a target expression to the addresses to probe, at most `limit`.
///
/// Accepted, separated by commas or white space: single addresses, CIDR blocks
/// (`10.0.0.0/24` — the network and broadcast addresses of an IPv4 block are left out),
/// ranges (`10.0.0.5-10.0.0.40`, or `10.0.0.5-40` for the last octet), and their IPv6
/// equivalents. Duplicates are removed; the order follows the expression.
pub fn expand_targets(targets: &str, limit: usize) -> Result<Vec<IpAddr>, ProbeError> {
    let blocks = parse_targets(targets)?
        .into_iter()
        .map(|target| match target {
            Target::Block(block) => Ok(block),
            Target::Name(name) => Err(ProbeError::invalid(format!(
                "'{name}' is not an IP address, a range (10.0.0.5-10.0.0.40) or a CIDR block (10.0.0.0/24)"
            ))),
        })
        .collect::<Result<Vec<Block>, ProbeError>>()?;
    expand_blocks(&blocks, limit)
}

/// Probes every host address in `targets` (see [`expand_targets`]; host names are accepted
/// too) and returns the hosts that are alive, sorted by address.
///
/// Each host gets an echo request (repeated `retries` times) and a connection attempt to every
/// one of `tcp_fallback_ports`. Alive hosts are streamed to `found` as they answer. When all
/// probes are done the local neighbour table is consulted to attach MAC addresses and to add
/// hosts that answered neither probe but did answer ARP / neighbour discovery (`via: "arp"`).
pub async fn sweep(
    pinger: &Pinger,
    targets: &str,
    options: &SweepOptions,
    found: Option<UnboundedSender<HostResult>>,
) -> Result<Vec<HostResult>, ProbeError> {
    if options.concurrency == 0 {
        return Err(ProbeError::invalid(
            "the sweep concurrency must be at least 1",
        ));
    }
    if options.timeout_ms == 0 {
        return Err(ProbeError::invalid(
            "the sweep timeout must be at least 1 ms",
        ));
    }
    if options.tcp_fallback_ports.contains(&0) {
        return Err(ProbeError::invalid(
            "0 is not a TCP port; ports are numbers from 1 to 65535",
        ));
    }
    let mut blocks = Vec::new();
    for target in parse_targets(targets)? {
        blocks.push(match target {
            Target::Block(block) => block,
            Target::Name(name) => Block::single(dns::resolve_one(&name).await?),
        });
    }
    let addresses: Arc<[IpAddr]> = expand_blocks(&blocks, options.limit)?.into();

    // Neighbours known before the sweep cannot prove that a host is alive now.
    let known_before: HashSet<String> = local::neighbours()
        .await
        .into_iter()
        .map(|neighbour| neighbour.address)
        .collect();

    let settings = Arc::new(Settings {
        timeout: Duration::from_millis(options.timeout_ms),
        retries: options.retries,
        ports: options.tcp_fallback_ports.clone(),
        resolve_names: options.resolve_names,
    });
    // Workers take the next address until none is left; dropping the set (when this future is
    // dropped) aborts them and closes their sockets.
    let next = Arc::new(AtomicUsize::new(0));
    let (done, mut finished) = mpsc::unbounded_channel::<Result<HostResult, ProbeError>>();
    let mut workers = JoinSet::new();
    for _ in 0..options.concurrency.min(addresses.len()) {
        let (pinger, addresses, next, settings, done) = (
            pinger.clone(),
            addresses.clone(),
            next.clone(),
            settings.clone(),
            done.clone(),
        );
        workers.spawn(async move {
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(address) = addresses.get(index) else {
                    return;
                };
                let outcome = match probe_host(&pinger, *address, &settings).await {
                    Ok(Some(host)) => Ok(host),
                    Ok(None) => continue,
                    Err(error) => Err(error),
                };
                let failed = outcome.is_err();
                if done.send(outcome).is_err() || failed {
                    return;
                }
            }
        });
    }
    drop(done);

    let mut alive: Vec<HostResult> = Vec::new();
    while let Some(outcome) = finished.recv().await {
        let host = outcome?;
        if let Some(found) = &found {
            let _ = found.send(host.clone());
        }
        alive.push(host);
    }
    drop(workers);

    // The neighbour table knows the MAC of every on-link host that answered, and of hosts
    // that filter ICMP and TCP but still had to answer ARP.
    let answered: HashSet<String> = alive.iter().map(|host| host.address.clone()).collect();
    let swept: HashSet<IpAddr> = addresses.iter().copied().collect();
    let neighbours = settled_neighbours(&swept, &answered).await;
    for host in &mut alive {
        host.mac = host
            .address
            .parse()
            .ok()
            .and_then(|address| local::neighbour_of(&neighbours, address))
            .map(|neighbour| neighbour.mac.clone());
    }
    for neighbour in &neighbours {
        let Ok(address) = neighbour.address.parse::<IpAddr>() else {
            continue;
        };
        let fresh = neighbour.state == "reachable" || !known_before.contains(&neighbour.address);
        if !fresh || !swept.contains(&address) || answered.contains(&neighbour.address) {
            continue;
        }
        let host = HostResult {
            address: neighbour.address.clone(),
            alive: true,
            rtt_ms: None,
            via: "arp".to_owned(),
            hostname: match options.resolve_names {
                true => dns::reverse(address, NAME_TIMEOUT).await,
                false => None,
            },
            mac: Some(neighbour.mac.clone()),
            open_ports: Vec::new(),
        };
        if let Some(found) = &found {
            let _ = found.send(host.clone());
        }
        alive.push(host);
    }

    alive.sort_by_key(|host| host.address.parse::<IpAddr>().ok());
    alive.dedup_by(|a, b| a.address == b.address);
    Ok(alive)
}

/// Reads the neighbour table once the kernel has made up its mind about the silent hosts.
///
/// An echo sent to a neighbour the kernel already knew (a stale entry) starts a reachability
/// check that takes a few seconds: the entry sits in `delay`, then `probe`, and ends up
/// `reachable` (it answered ARP) or gone. Hosts in between are worth waiting for; this only
/// happens on systems that expose the state (Linux) and when such entries exist.
async fn settled_neighbours(
    swept: &HashSet<IpAddr>,
    answered: &HashSet<String>,
) -> Vec<local::Neighbour> {
    const SETTLE_LIMIT: Duration = Duration::from_secs(7);
    const POLL: Duration = Duration::from_millis(500);
    let started = Instant::now();
    loop {
        let neighbours = local::neighbours().await;
        let undecided = neighbours.iter().any(|neighbour| {
            matches!(neighbour.state.as_str(), "delay" | "probe")
                && !answered.contains(&neighbour.address)
                && neighbour
                    .address
                    .parse()
                    .is_ok_and(|address| swept.contains(&address))
        });
        if !undecided || started.elapsed() >= SETTLE_LIMIT {
            return neighbours;
        }
        tokio::time::sleep(POLL).await;
    }
}

struct Settings {
    timeout: Duration,
    retries: u32,
    ports: Vec<u16>,
    resolve_names: bool,
}

/// Probes one address. `Ok(None)` when nothing answered.
async fn probe_host(
    pinger: &Pinger,
    address: IpAddr,
    settings: &Settings,
) -> Result<Option<HostResult>, ProbeError> {
    // Most live hosts answer the first echo; only then is anything else worth trying.
    let first = echo(pinger, address, settings.timeout).await?;
    let (reply, tcp) = match first {
        Some(reply) => (Some(reply), tcp_probes(address, settings).await),
        None => {
            let retries = async {
                for _ in 0..settings.retries {
                    if let Some(reply) = echo(pinger, address, settings.timeout).await? {
                        return Ok(Some(reply));
                    }
                }
                Ok::<_, ProbeError>(None)
            };
            let (reply, tcp) = tokio::join!(retries, tcp_probes(address, settings));
            (reply?, tcp)
        }
    };

    let open_ports: Vec<u16> = tcp
        .iter()
        .filter(|probe| probe.accepted)
        .map(|probe| probe.port)
        .collect();
    let (via, rtt_ms) = match (&reply, tcp.iter().find(|p| p.accepted).or(tcp.first())) {
        (Some(reply), _) => ("icmp".to_owned(), Some(reply.rtt_ms)),
        (None, Some(probe)) => (format!("tcp/{}", probe.port), Some(probe.rtt_ms)),
        (None, None) => return Ok(None),
    };
    Ok(Some(HostResult {
        address: address.to_string(),
        alive: true,
        rtt_ms,
        via,
        hostname: match settings.resolve_names {
            true => dns::reverse(address, NAME_TIMEOUT).await,
            false => None,
        },
        mac: None,
        open_ports,
    }))
}

/// One echo. `Ok(None)` when the host did not answer; `Err` only for failures of this machine
/// that make the whole sweep meaningless.
async fn echo(
    pinger: &Pinger,
    address: IpAddr,
    timeout: Duration,
) -> Result<Option<PingReply>, ProbeError> {
    let mut local_failures = 0;
    loop {
        match pinger.ping_once(address, timeout, 0).await {
            Ok(reply) => return Ok(Some(reply)),
            // No answer, an ICMP error, or an address the system refuses to send to
            // (a broadcast address inside the range).
            Err(
                ProbeError::Timeout
                | ProbeError::Unreachable(_)
                | ProbeError::Refused
                | ProbeError::Permission(_)
                | ProbeError::Invalid(_),
            ) => return Ok(None),
            Err(ProbeError::Io(error)) if local_failures < LOCAL_RETRIES => {
                // Typically "too many open files": wait for other probes to finish.
                local_failures += 1;
                tracing::debug!(%address, %error, "echo failed locally; retrying");
                tokio::time::sleep(Duration::from_millis(50 * u64::from(local_failures))).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// A TCP port that answered: it accepted the connection or reset it. Either proves a host.
struct TcpAnswer {
    port: u16,
    accepted: bool,
    rtt_ms: f64,
}

/// Tries every fallback port at once and returns the ones that answered, in list order.
async fn tcp_probes(address: IpAddr, settings: &Settings) -> Vec<TcpAnswer> {
    let mut probes = JoinSet::new();
    for (index, port) in settings.ports.iter().copied().enumerate() {
        let timeout = settings.timeout;
        probes.spawn(async move {
            let started = Instant::now();
            let attempt =
                tokio::time::timeout(timeout, tcp::connect(SocketAddr::new(address, port))).await;
            let accepted = match attempt {
                Ok(Ok(_stream)) => true,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => false,
                _ => return None,
            };
            Some((
                index,
                TcpAnswer {
                    port,
                    accepted,
                    rtt_ms: millis(started.elapsed()),
                },
            ))
        });
    }
    let mut answers = Vec::new();
    while let Some(result) = probes.join_next().await {
        if let Ok(Some(answer)) = result {
            answers.push(answer);
        }
    }
    answers.sort_by_key(|(index, _)| *index);
    answers.into_iter().map(|(_, answer)| answer).collect()
}

/// An inclusive range of addresses of one family, as 128-bit numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    first: u128,
    last: u128,
    v6: bool,
}

impl Block {
    fn single(address: IpAddr) -> Self {
        let (number, v6) = to_number(address);
        Block {
            first: number,
            last: number,
            v6,
        }
    }

    fn len(&self) -> u128 {
        (self.last - self.first).saturating_add(1)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Block(Block),
    /// Something that can only be a host name.
    Name(String),
}

fn to_number(address: IpAddr) -> (u128, bool) {
    match address {
        IpAddr::V4(v4) => (u128::from(u32::from(v4)), false),
        IpAddr::V6(v6) => (u128::from(v6), true),
    }
}

fn from_number(number: u128, v6: bool) -> IpAddr {
    if v6 {
        IpAddr::V6(Ipv6Addr::from(number))
    } else {
        IpAddr::V4(Ipv4Addr::from(number as u32))
    }
}

fn parse_targets(targets: &str) -> Result<Vec<Target>, ProbeError> {
    let items: Vec<&str> = targets
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|item| !item.is_empty())
        .collect();
    if items.is_empty() {
        return Err(ProbeError::invalid(
            "no targets given; enter addresses, a range (10.0.0.5-10.0.0.40) or a CIDR block (10.0.0.0/24)",
        ));
    }
    items.into_iter().map(parse_target).collect()
}

fn parse_target(item: &str) -> Result<Target, ProbeError> {
    if let Some((network, prefix)) = item.split_once('/') {
        return parse_cidr(item, network, prefix).map(Target::Block);
    }
    if let Some(address) = util::parse_ip_literal(item) {
        return Ok(Target::Block(Block::single(address)));
    }
    if let Some((first, last)) = item.split_once('-')
        && let Ok(first) = first.parse::<IpAddr>()
    {
        return parse_range(item, first, last).map(Target::Block);
    }
    // Digits and dots only is a mistyped IPv4 address, not a host name (the system resolver
    // would even accept shorthand such as "10.1" and probe an address nobody meant).
    if item.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(ProbeError::invalid(format!(
            "'{item}' is not an IP address; an IPv4 address has four numbers from 0 to 255"
        )));
    }
    Ok(Target::Name(util::validate_host(item)?.to_owned()))
}

fn parse_cidr(item: &str, network: &str, prefix: &str) -> Result<Block, ProbeError> {
    let Ok(network) = network.parse::<IpAddr>() else {
        return Err(ProbeError::invalid(format!(
            "'{item}' is not a CIDR block: '{network}' is not an IP address"
        )));
    };
    let bits: u32 = if network.is_ipv6() { 128 } else { 32 };
    let prefix = prefix
        .parse::<u32>()
        .ok()
        .filter(|prefix| *prefix <= bits)
        .ok_or_else(|| {
            ProbeError::invalid(format!(
                "'{item}' is not a CIDR block: the prefix length must be a number from 0 to {bits}"
            ))
        })?;
    let (number, v6) = to_number(network);
    let host_bits = bits - prefix;
    let host_mask = match host_bits {
        128 => u128::MAX,
        _ => (1u128 << host_bits) - 1,
    };
    let (mut first, mut last) = (number & !host_mask, number | host_mask);
    // The all-zeros and all-ones addresses of an IPv4 subnet are not hosts (RFC 3021 makes
    // /31 point-to-point links the exception).
    if !v6 && host_bits >= 2 {
        first += 1;
        last -= 1;
    }
    Ok(Block { first, last, v6 })
}

fn parse_range(item: &str, first: IpAddr, last: &str) -> Result<Block, ProbeError> {
    let last = match (first, last.parse::<IpAddr>(), last.parse::<u8>()) {
        (IpAddr::V4(_), Ok(last @ IpAddr::V4(_)), _)
        | (IpAddr::V6(_), Ok(last @ IpAddr::V6(_)), _) => last,
        // "10.0.0.5-40": the end replaces the last octet.
        (IpAddr::V4(first), Err(_), Ok(octet)) => {
            let [a, b, c, _] = first.octets();
            IpAddr::V4(Ipv4Addr::new(a, b, c, octet))
        }
        _ => {
            return Err(ProbeError::invalid(format!(
                "'{item}' is not a range: write both ends as addresses of the same family, \
                 for example 10.0.0.5-10.0.0.40"
            )));
        }
    };
    let ((first_number, v6), (last_number, _)) = (to_number(first), to_number(last));
    if first_number > last_number {
        return Err(ProbeError::invalid(format!(
            "the range '{item}' is backwards: {first} comes after {last}"
        )));
    }
    Ok(Block {
        first: first_number,
        last: last_number,
        v6,
    })
}

fn expand_blocks(blocks: &[Block], limit: usize) -> Result<Vec<IpAddr>, ProbeError> {
    let total = blocks
        .iter()
        .fold(0u128, |total, block| total.saturating_add(block.len()));
    if total > limit as u128 {
        let shown = match total {
            u128::MAX => "more than 10^38".to_owned(),
            total => total.to_string(),
        };
        return Err(ProbeError::invalid(format!(
            "the targets cover {shown} addresses; the limit is {limit}. Narrow the range or sweep it in parts"
        )));
    }
    let mut seen = HashSet::with_capacity(total as usize);
    let mut addresses = Vec::with_capacity(total as usize);
    for block in blocks {
        for number in block.first..=block.last {
            let address = from_number(number, block.v6);
            if seen.insert(address) {
                addresses.push(address);
            }
        }
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn expand(targets: &str) -> Vec<String> {
        expand_targets(targets, 65_536)
            .unwrap()
            .into_iter()
            .map(|address| address.to_string())
            .collect()
    }

    #[test]
    fn cidr_blocks_list_host_addresses() {
        assert_eq!(expand("192.168.1.0/30"), ["192.168.1.1", "192.168.1.2"]);
        assert_eq!(expand("192.168.1.0/29").len(), 6);
        let c = expand("10.0.0.0/24");
        assert_eq!(c.len(), 254);
        assert_eq!((c[0].as_str(), c[253].as_str()), ("10.0.0.1", "10.0.0.254"));
        // Host bits in the block are ignored, as every tool does.
        assert_eq!(expand("10.0.0.77/24"), c);
        assert_eq!(expand("10.0.0.4/31"), ["10.0.0.4", "10.0.0.5"]);
        assert_eq!(expand("10.0.0.4/32"), ["10.0.0.4"]);
        assert_eq!(expand("172.16.0.0/16").len(), 65_534);
        assert_eq!(expand("10.1.0.0/23").len(), 510);
    }

    #[test]
    fn ranges_and_lists() {
        assert_eq!(
            expand("10.0.0.5-10.0.0.8"),
            ["10.0.0.5", "10.0.0.6", "10.0.0.7", "10.0.0.8"]
        );
        assert_eq!(expand("10.0.0.5-7"), ["10.0.0.5", "10.0.0.6", "10.0.0.7"]);
        assert_eq!(
            expand("10.0.0.254-10.0.1.1"),
            ["10.0.0.254", "10.0.0.255", "10.0.1.0", "10.0.1.1"]
        );
        assert_eq!(expand("10.0.0.9-10.0.0.9"), ["10.0.0.9"]);
        assert_eq!(
            expand(" 10.0.0.1, 10.0.0.9 ;10.0.0.3\t10.0.0.1 "),
            ["10.0.0.1", "10.0.0.9", "10.0.0.3"]
        );
        // Overlapping items do not repeat addresses.
        assert_eq!(
            expand("10.0.0.0/30,10.0.0.2-10.0.0.4"),
            ["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"]
        );
    }

    #[test]
    fn ipv6_targets() {
        assert_eq!(expand("2001:db8::1"), ["2001:db8::1"]);
        assert_eq!(expand("[2001:db8::1]"), ["2001:db8::1"]);
        assert_eq!(
            expand("2001:db8::fffe-2001:db8::1:1"),
            [
                "2001:db8::fffe",
                "2001:db8::ffff",
                "2001:db8::1:0",
                "2001:db8::1:1"
            ]
        );
        let block = expand("2001:db8::/126");
        assert_eq!(
            block,
            ["2001:db8::", "2001:db8::1", "2001:db8::2", "2001:db8::3"]
        );
        assert_eq!(expand("fd00::/120").len(), 256);
    }

    #[test]
    fn the_limit_is_enforced_before_anything_is_allocated() {
        assert_eq!(expand_targets("10.0.0.0/24", 254).unwrap().len(), 254);
        let error = expand_targets("10.0.0.0/24", 253).unwrap_err();
        assert!(error.to_string().contains("254 addresses"), "{error}");
        assert!(error.to_string().contains("limit is 253"), "{error}");
        for huge in [
            "10.0.0.0/8",
            "0.0.0.0/0",
            "2001:db8::/64",
            "::/0",
            "::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        ] {
            let error = expand_targets(huge, 65_536).unwrap_err();
            assert!(matches!(error, ProbeError::Invalid(_)), "{huge}: {error:?}");
        }
        assert!(expand_targets("10.0.0.0/16,10.1.0.0/16", 65_536 * 2).is_ok());
        assert!(expand_targets("10.0.0.0/16,10.1.0.0/16", 100_000).is_err());
    }

    #[test]
    fn bad_targets_explain_themselves() {
        let cases = [
            ("", "no targets given"),
            (" ,; ", "no targets given"),
            ("10.0.0.0/33", "0 to 32"),
            ("10.0.0.0/abc", "0 to 32"),
            ("2001:db8::/129", "0 to 128"),
            ("10.0.0/24", "'10.0.0' is not an IP address"),
            ("10.0.0.9-10.0.0.1", "backwards"),
            ("10.0.0.9-3", "backwards"),
            ("10.0.0.1-2001:db8::1", "same family"),
            ("10.0.0.1-300", "same family"),
            ("10.0.0.1-", "same family"),
            ("core-sw1", "'core-sw1' is not an IP address"),
            ("10.0.0.300", "'10.0.0.300' is not an IP address"),
            ("10.1", "four numbers"),
        ];
        for (targets, expected) in cases {
            let error = expand_targets(targets, 1000).unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_)),
                "{targets:?}: {error:?}"
            );
            assert!(error.to_string().contains(expected), "{targets:?}: {error}");
        }
    }

    #[test]
    fn names_are_kept_for_the_resolver() {
        assert_eq!(
            parse_targets("core-sw1.lab, 10.0.0.1").unwrap(),
            vec![
                Target::Name("core-sw1.lab".into()),
                Target::Block(Block::single("10.0.0.1".parse().unwrap())),
            ]
        );
    }

    #[test]
    fn options_default_and_deserialize_partially() {
        let options = SweepOptions::default();
        assert_eq!(
            (
                options.concurrency,
                options.timeout_ms,
                options.retries,
                options.limit
            ),
            (128, 800, 1, 65_536)
        );
        assert_eq!(options.tcp_fallback_ports, [22, 23, 80, 443, 8291]);
        assert!(!options.resolve_names);
        let partial: SweepOptions =
            serde_json::from_str(r#"{"retries": 0, "tcp_fallback_ports": []}"#).unwrap();
        assert_eq!(partial.retries, 0);
        assert!(partial.tcp_fallback_ports.is_empty());
        assert_eq!(partial.concurrency, 128);
    }
}
