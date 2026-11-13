//! DNS: a dig-like query against a chosen server or the system resolvers, and the name
//! resolution helpers the other probes share.
//!
//! [`lookup`] talks to the servers itself (UDP with EDNS0, TCP when the answer is truncated)
//! instead of going through a caching stub resolver, so the answer shows exactly what one
//! server said: its records, their TTLs, the response code and how long it took.

use std::future::poll_fn;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use hickory_resolver::Hosts;
use hickory_resolver::proto::op::{Edns, Message, MessageType, Query, ResponseCode};
use hickory_resolver::proto::rr::{Name, RData, RecordType};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};

use crate::ProbeError;
use crate::util::{self, millis};

/// One resource record of an answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DnsRecord {
    /// Owner name, without the trailing dot.
    pub name: String,
    /// Record type mnemonic (`A`, `MX`, …).
    pub kind: String,
    pub ttl: u32,
    /// The record data in presentation form (`10 mail.example.com`, an address, the text of a TXT…).
    pub data: String,
}

/// The answer of one server to one question.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DnsAnswer {
    /// The name that was asked for (for a PTR query of an address: the `.arpa` name).
    pub query: String,
    pub kind: String,
    /// The server that answered, as `address:port`.
    pub server: String,
    pub elapsed_ms: f64,
    /// The answer section. Empty with `rcode` `NOERROR` means "the name exists but has no such record".
    pub records: Vec<DnsRecord>,
    /// `NOERROR`, `NXDOMAIN`, `SERVFAIL`, `REFUSED`, …
    pub rcode: String,
}

/// Record types [`lookup`] accepts.
const KINDS: &[(&str, RecordType)] = &[
    ("A", RecordType::A),
    ("AAAA", RecordType::AAAA),
    ("CNAME", RecordType::CNAME),
    ("MX", RecordType::MX),
    ("NS", RecordType::NS),
    ("TXT", RecordType::TXT),
    ("SOA", RecordType::SOA),
    ("SRV", RecordType::SRV),
    ("PTR", RecordType::PTR),
    ("CAA", RecordType::CAA),
    ("ANY", RecordType::ANY),
];

/// The EDNS0 payload size advertised over UDP: fits in one unfragmented packet on any path.
const UDP_PAYLOAD: u16 = 1232;
/// How many times each server is asked before giving up.
const ROUNDS: usize = 2;
/// The system resolver gets this long before [`resolve`] reports failure.
const RESOLVE_LIMIT: Duration = Duration::from_secs(15);
/// How long a parsed hosts file is reused by [`reverse`].
const HOSTS_CACHE: Duration = Duration::from_secs(30);

/// Asks a DNS server one question.
///
/// `kind` is one of A, AAAA, CNAME, MX, NS, TXT, SOA, SRV, PTR, CAA, ANY (any letter case).
/// `server` is an address or host name with an optional port (`10.0.0.53`, `dns.example:5353`,
/// `[2001:db8::53]:53`); `None` asks the system resolvers in order. A PTR query accepts a plain
/// IP address.
///
/// An answer with a negative response code (`NXDOMAIN`, `SERVFAIL`, …) is still an answer and is
/// returned as `Ok`; errors mean that no server could be asked or none replied.
pub async fn lookup(
    name: &str,
    kind: &str,
    server: Option<&str>,
    timeout: Duration,
) -> Result<DnsAnswer, ProbeError> {
    let record_type = parse_kind(kind)?;
    let question = Query::query(query_name(name, record_type)?, record_type);
    let servers = match server {
        Some(server) => explicit_servers(server).await?,
        None => system_servers()?,
    };
    lookup_at(question, &servers, timeout).await
}

/// Forward-resolves `host` with the system resolver (hosts file included).
///
/// A literal address is returned as is, without touching the resolver.
pub async fn resolve(host: &str) -> Result<Vec<IpAddr>, ProbeError> {
    let host = util::validate_host(host)?;
    if let Some(address) = util::parse_ip_literal(host) {
        return Ok(vec![address]);
    }
    // The system resolver does not apply IDNA, so internationalised names go out as punycode.
    let ascii = if host.is_ascii() {
        host.to_owned()
    } else {
        Name::from_utf8(host)
            .map_err(|error| {
                ProbeError::invalid(format!("'{host}' is not a valid host name ({error})"))
            })?
            .to_ascii()
    };
    let lookup = tokio::net::lookup_host((ascii.as_str(), 0));
    let Ok(Ok(found)) = tokio::time::timeout(RESOLVE_LIMIT, lookup).await else {
        return Err(ProbeError::Resolve);
    };
    let mut addresses: Vec<IpAddr> = Vec::new();
    for address in found.map(|socket| socket.ip()) {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    if addresses.is_empty() {
        return Err(ProbeError::Resolve);
    }
    Ok(addresses)
}

/// The name of `address`: from the hosts file, else from a PTR lookup with the system resolvers.
pub async fn reverse(address: IpAddr, timeout: Duration) -> Option<String> {
    let name = arpa_name(address);
    let question = Query::query(name, RecordType::PTR);
    if let Some(found) = hosts_file().lookup_static_host(&question) {
        if let Some(target) = found.answers().iter().find_map(ptr_target) {
            return Some(target);
        }
    }
    let servers = system_servers().ok()?;
    let answer = lookup_at(question, &servers, timeout).await.ok()?;
    answer
        .records
        .into_iter()
        .find(|record| record.kind == "PTR")
        .map(|record| record.data)
}

/// Resolves `host` and picks the address a probe should use: the first one this machine has a
/// route to, so that a name with an unreachable IPv6 address still works over IPv4.
pub(crate) async fn resolve_one(host: &str) -> Result<IpAddr, ProbeError> {
    let addresses = resolve(host).await?;
    for address in &addresses {
        if is_routable(*address).await {
            return Ok(*address);
        }
    }
    Ok(addresses[0])
}

/// Whether the routing table has a route towards `address`. Connecting a UDP socket performs the
/// route lookup without sending anything.
async fn is_routable(address: IpAddr) -> bool {
    let local: SocketAddr = match address {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    match UdpSocket::bind(local).await {
        Ok(socket) => socket.connect((address, 9)).await.is_ok(),
        Err(_) => false,
    }
}

fn parse_kind(kind: &str) -> Result<RecordType, ProbeError> {
    let wanted = kind.trim().to_ascii_uppercase();
    let wanted = if wanted == "*" { "ANY" } else { wanted.as_str() };
    KINDS
        .iter()
        .find(|(name, _)| *name == wanted)
        .map(|(_, record_type)| *record_type)
        .ok_or_else(|| {
            let known: Vec<&str> = KINDS.iter().map(|(name, _)| *name).collect();
            ProbeError::invalid(format!(
                "'{kind}' is not a supported DNS record type; use one of {}",
                known.join(", ")
            ))
        })
}

/// The fully qualified name to ask for. A PTR query for an address asks for its `.arpa` name.
fn query_name(name: &str, record_type: RecordType) -> Result<Name, ProbeError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ProbeError::invalid(
            "the DNS name is empty; enter a name such as example.com",
        ));
    }
    if record_type == RecordType::PTR {
        if let Some(address) = util::parse_ip_literal(name) {
            return Ok(arpa_name(address));
        }
    }
    if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ProbeError::invalid(format!(
            "'{name}' is not a valid DNS name (it contains white space)"
        )));
    }
    let mut parsed = Name::from_str_relaxed(name).map_err(|error| {
        ProbeError::invalid(format!("'{name}' is not a valid DNS name ({error})"))
    })?;
    parsed.set_fqdn(true);
    Ok(parsed)
}

/// `4.3.2.1.in-addr.arpa.` / the 32-nibble `ip6.arpa.` name of an address.
fn arpa_name(address: IpAddr) -> Name {
    let text = match address {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa.")
        }
        IpAddr::V6(v6) => {
            let mut text = String::with_capacity(73);
            for byte in v6.octets().iter().rev() {
                text.push_str(&format!("{:x}.{:x}.", byte & 0x0f, byte >> 4));
            }
            text.push_str("ip6.arpa.");
            text
        }
    };
    Name::from_ascii(&text).expect("an arpa name built from an address is always valid")
}

async fn explicit_servers(server: &str) -> Result<Vec<SocketAddr>, ProbeError> {
    let (host, port) = util::split_host_port(server, 53)?;
    let addresses = match util::parse_ip_literal(&host) {
        Some(address) => vec![address],
        None => resolve(&host).await?,
    };
    Ok(addresses
        .into_iter()
        .map(|address| SocketAddr::new(address, port))
        .collect())
}

/// The DNS servers this machine is configured to use, in order.
fn system_servers() -> Result<Vec<SocketAddr>, ProbeError> {
    let (config, _options) = hickory_resolver::system_conf::read_system_conf().map_err(|error| {
        ProbeError::unsupported(format!(
            "the system DNS configuration could not be read ({error}); name a DNS server explicitly"
        ))
    })?;
    let mut servers: Vec<SocketAddr> = Vec::new();
    for name_server in config.name_servers() {
        let port = name_server
            .connections
            .first()
            .map_or(53, |connection| connection.port);
        let address = SocketAddr::new(name_server.ip, port);
        if !servers.contains(&address) {
            servers.push(address);
        }
    }
    if servers.is_empty() {
        return Err(ProbeError::unsupported(
            "this machine has no DNS servers configured; name a DNS server explicitly",
        ));
    }
    Ok(servers)
}

fn hosts_file() -> Arc<Hosts> {
    static CACHE: Mutex<Option<(Instant, Arc<Hosts>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((loaded, hosts)) = cache.as_ref() {
        if loaded.elapsed() < HOSTS_CACHE {
            return hosts.clone();
        }
    }
    let hosts = Arc::new(Hosts::from_system().unwrap_or_default());
    *cache = Some((Instant::now(), hosts.clone()));
    hosts
}

fn ptr_target(record: &hickory_resolver::proto::rr::Record) -> Option<String> {
    match &record.data {
        RData::PTR(target) => Some(display_name(&target.0)),
        _ => None,
    }
}

/// The state of one server during a lookup.
enum Channel {
    /// Not asked yet.
    Idle,
    Open(UdpSocket),
    /// Unusable for this lookup: it could not be reached or gave a failure answer.
    Done,
}

/// Asks `servers` in rotation until one gives a usable answer or `timeout` passes.
///
/// Each server keeps one socket for the whole lookup, so a slow answer to an earlier
/// transmission is still accepted after a retransmission.
async fn lookup_at(
    question: Query,
    servers: &[SocketAddr],
    timeout: Duration,
) -> Result<DnsAnswer, ProbeError> {
    let mut request = Message::query();
    request.metadata.recursion_desired = true;
    request.add_query(question.clone());
    let mut edns = Edns::new();
    edns.set_max_payload(UDP_PAYLOAD);
    request.set_edns(edns);
    let wire = request
        .to_vec()
        .map_err(|error| ProbeError::invalid(format!("the DNS query cannot be encoded ({error})")))?;
    let id = request.metadata.id;

    let started = Instant::now();
    let deadline = started + timeout;
    let transmissions = servers.len() * ROUNDS;
    let step = timeout / transmissions.max(1) as u32;
    let mut channels: Vec<Channel> = servers.iter().map(|_| Channel::Idle).collect();
    let mut sent = 0usize;
    let mut cursor = 0usize;
    let mut next_send = started;
    let mut failure_answer: Option<DnsAnswer> = None;
    let mut last_error = ProbeError::Timeout;
    let mut buffer = vec![0u8; 65_535];

    loop {
        let now = Instant::now();
        if now >= deadline || channels.iter().all(|c| matches!(c, Channel::Done)) {
            break;
        }
        if sent < transmissions && now >= next_send {
            // Next server in rotation that is still worth asking.
            while matches!(channels[cursor % servers.len()], Channel::Done) {
                cursor += 1;
            }
            let index = cursor % servers.len();
            cursor += 1;
            sent += 1;
            next_send = now + step;
            if let Err(error) = transmit(&mut channels[index], servers[index], &wire).await {
                channels[index] = Channel::Done;
                last_error = error;
                next_send = now;
            }
            continue;
        }
        let wake = if sent < transmissions {
            next_send.min(deadline)
        } else {
            deadline
        };
        let received =
            tokio::time::timeout_at(wake.into(), receive_any(&channels, &mut buffer)).await;
        let (index, result) = match received {
            Ok(received) => received,
            Err(_) => continue,
        };
        let length = match result {
            Ok(length) => length,
            Err(error) => {
                channels[index] = Channel::Done;
                last_error = ProbeError::from_network_io(error);
                next_send = Instant::now();
                continue;
            }
        };
        let Some(mut response) = matching_response(&buffer[..length], id, &question) else {
            continue;
        };
        if response.metadata.truncation {
            match tcp_exchange(servers[index], &wire, id, &question, deadline).await {
                Ok(full) => response = full,
                Err(error) => {
                    tracing::debug!(server = %servers[index], %error, "TCP retry of a truncated DNS answer failed");
                }
            }
        }
        let answer = build_answer(&question, servers[index], started.elapsed(), &response);
        if is_server_failure(response.metadata.response_code) {
            // Another server may do better; keep this answer in case none does.
            channels[index] = Channel::Done;
            failure_answer.get_or_insert(answer);
            next_send = Instant::now();
            continue;
        }
        return Ok(answer);
    }
    failure_answer.ok_or(last_error)
}

fn is_server_failure(code: ResponseCode) -> bool {
    matches!(
        code,
        ResponseCode::ServFail | ResponseCode::Refused | ResponseCode::NotImp | ResponseCode::FormErr
    )
}

async fn transmit(channel: &mut Channel, server: SocketAddr, wire: &[u8]) -> Result<(), ProbeError> {
    if matches!(channel, Channel::Idle) {
        let local: SocketAddr = match server {
            SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(local).await?;
        socket
            .connect(server)
            .await
            .map_err(ProbeError::from_network_io)?;
        *channel = Channel::Open(socket);
    }
    if let Channel::Open(socket) = channel {
        socket
            .send(wire)
            .await
            .map_err(ProbeError::from_network_io)?;
    }
    Ok(())
}

/// Waits for a datagram on any open channel; never resolves when none is open.
async fn receive_any(channels: &[Channel], buffer: &mut [u8]) -> (usize, io::Result<usize>) {
    poll_fn(|cx| {
        for (index, channel) in channels.iter().enumerate() {
            let Channel::Open(socket) = channel else {
                continue;
            };
            let mut read = ReadBuf::new(buffer);
            if let Poll::Ready(result) = socket.poll_recv(cx, &mut read) {
                return Poll::Ready((index, result.map(|()| read.filled().len())));
            }
        }
        Poll::Pending
    })
    .await
}

/// Decodes a datagram and returns it only if it is the response to our question.
fn matching_response(datagram: &[u8], id: u16, question: &Query) -> Option<Message> {
    let message = Message::from_vec(datagram).ok()?;
    let metadata = &message.metadata;
    if metadata.id != id || metadata.message_type != MessageType::Response {
        return None;
    }
    // Servers echo the question; some omit it on errors.
    if let Some(echoed) = message.queries.first() {
        if echoed.query_type() != question.query_type() || echoed.name() != question.name() {
            return None;
        }
    }
    Some(message)
}

async fn tcp_exchange(
    server: SocketAddr,
    wire: &[u8],
    id: u16,
    question: &Query,
    deadline: Instant,
) -> Result<Message, ProbeError> {
    let exchange = async {
        let mut stream = TcpStream::connect(server)
            .await
            .map_err(ProbeError::from_network_io)?;
        let length = u16::try_from(wire.len())
            .map_err(|_| ProbeError::invalid("the DNS query is too long"))?;
        let mut framed = Vec::with_capacity(wire.len() + 2);
        framed.extend_from_slice(&length.to_be_bytes());
        framed.extend_from_slice(wire);
        stream.write_all(&framed).await?;
        let mut length = [0u8; 2];
        stream.read_exact(&mut length).await?;
        let mut body = vec![0u8; usize::from(u16::from_be_bytes(length))];
        stream.read_exact(&mut body).await?;
        matching_response(&body, id, question)
            .ok_or_else(|| ProbeError::protocol("the DNS server sent an unrelated answer over TCP"))
    };
    tokio::time::timeout_at(deadline.into(), exchange)
        .await
        .map_err(|_| ProbeError::Timeout)?
}

fn build_answer(
    question: &Query,
    server: SocketAddr,
    elapsed: Duration,
    response: &Message,
) -> DnsAnswer {
    DnsAnswer {
        query: display_name(question.name()),
        kind: question.query_type().to_string(),
        server: server.to_string(),
        elapsed_ms: millis(elapsed),
        records: response
            .answers
            .iter()
            .map(|record| DnsRecord {
                name: display_name(&record.name),
                kind: record.record_type().to_string(),
                ttl: record.ttl,
                data: display_data(&record.data),
            })
            .collect(),
        rcode: rcode_name(response.metadata.response_code),
    }
}

/// A name as it is on the wire (punycode for IDN), without the trailing dot.
fn display_name(name: &Name) -> String {
    let text = name.to_ascii();
    match text.strip_suffix('.') {
        Some(stripped) if !stripped.is_empty() => stripped.to_owned(),
        _ => text,
    }
}

fn display_data(data: &RData) -> String {
    match data {
        RData::A(address) => address.0.to_string(),
        RData::AAAA(address) => address.0.to_string(),
        RData::CNAME(target) => display_name(&target.0),
        RData::NS(target) => display_name(&target.0),
        RData::PTR(target) => display_name(&target.0),
        RData::MX(mx) => format!("{} {}", mx.preference, display_name(&mx.exchange)),
        RData::SRV(srv) => format!(
            "{} {} {} {}",
            srv.priority,
            srv.weight,
            srv.port,
            display_name(&srv.target)
        ),
        RData::SOA(soa) => format!(
            "{} {} {} {} {} {} {}",
            display_name(&soa.mname),
            display_name(&soa.rname),
            soa.serial,
            soa.refresh,
            soa.retry,
            soa.expire,
            soa.minimum
        ),
        // The character-strings of one TXT record are one logical text (SPF, DKIM keys).
        RData::TXT(txt) => txt
            .txt_data
            .iter()
            .map(|part| String::from_utf8_lossy(part))
            .collect(),
        RData::CAA(caa) => format!(
            "{} {} \"{}\"",
            caa.flags(),
            caa.tag,
            String::from_utf8_lossy(&caa.value)
        ),
        other => other.to_string(),
    }
}

fn rcode_name(code: ResponseCode) -> String {
    let name = match code {
        ResponseCode::NoError => "NOERROR",
        ResponseCode::FormErr => "FORMERR",
        ResponseCode::ServFail => "SERVFAIL",
        ResponseCode::NXDomain => "NXDOMAIN",
        ResponseCode::NotImp => "NOTIMP",
        ResponseCode::Refused => "REFUSED",
        ResponseCode::YXDomain => "YXDOMAIN",
        ResponseCode::YXRRSet => "YXRRSET",
        ResponseCode::NXRRSet => "NXRRSET",
        ResponseCode::NotAuth => "NOTAUTH",
        ResponseCode::NotZone => "NOTZONE",
        ResponseCode::BADVERS => "BADVERS",
        other => return format!("RCODE{}", u16::from(other)),
    };
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tokio::net::TcpListener;

    #[test]
    fn record_types_are_case_insensitive_and_validated() {
        assert_eq!(parse_kind("a").unwrap(), RecordType::A);
        assert_eq!(parse_kind(" Aaaa ").unwrap(), RecordType::AAAA);
        assert_eq!(parse_kind("any").unwrap(), RecordType::ANY);
        assert_eq!(parse_kind("*").unwrap(), RecordType::ANY);
        let error = parse_kind("AXFR").unwrap_err().to_string();
        assert!(error.contains("'AXFR'"), "{error}");
        assert!(error.contains("A, AAAA, CNAME, MX"), "{error}");
    }

    #[test]
    fn arpa_names() {
        assert_eq!(
            arpa_name("192.0.2.10".parse().unwrap()).to_ascii(),
            "10.2.0.192.in-addr.arpa."
        );
        // RFC 3596 section 2.5 example.
        assert_eq!(
            arpa_name("4321:0:1:2:3:4:567:89ab".parse().unwrap()).to_ascii(),
            "b.a.9.8.7.6.5.0.4.0.0.0.3.0.0.0.2.0.0.0.1.0.0.0.0.0.0.0.1.2.3.4.ip6.arpa."
        );
    }

    #[test]
    fn query_names() {
        let name = query_name("Example.COM", RecordType::A).unwrap();
        assert!(name.is_fqdn());
        assert_eq!(display_name(&name), "example.com");
        assert_eq!(
            display_name(&query_name("8.8.4.4", RecordType::PTR).unwrap()),
            "4.4.8.8.in-addr.arpa"
        );
        // Only PTR turns an address into an arpa name.
        assert_eq!(
            display_name(&query_name("8.8.4.4", RecordType::A).unwrap()),
            "8.8.4.4"
        );
        assert_eq!(
            display_name(&query_name("_sip._tcp.example.com.", RecordType::SRV).unwrap()),
            "_sip._tcp.example.com"
        );
        assert_eq!(
            display_name(&query_name("my_host.lab.example", RecordType::A).unwrap()),
            "my_host.lab.example"
        );
        assert_eq!(
            display_name(&query_name("bücher.example", RecordType::A).unwrap()),
            "xn--bcher-kva.example"
        );
        assert_eq!(display_name(&query_name(".", RecordType::NS).unwrap()), ".");
        for bad in ["", "   ", "two words.example", "a..b"] {
            assert!(
                matches!(query_name(bad, RecordType::A), Err(ProbeError::Invalid(_))),
                "{bad:?}"
            );
        }
    }

    /// A response written out by hand from RFC 1035: header, question, then the given records.
    fn response_bytes(id: u16, flags: u16, question: &[u8], answers: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&flags.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(answers.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(question);
        for answer in answers {
            out.extend_from_slice(answer);
        }
        out
    }

    /// `example.com` as labels, then type and class IN.
    fn question_bytes(record_type: u16) -> Vec<u8> {
        let mut out = b"\x07example\x03com\x00".to_vec();
        out.extend_from_slice(&record_type.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out
    }

    /// A record owned by the question name (compression pointer to offset 12).
    fn record_bytes(record_type: u16, ttl: u32, rdata: &[u8]) -> Vec<u8> {
        let mut out = vec![0xc0, 0x0c];
        out.extend_from_slice(&record_type.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&ttl.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(rdata);
        out
    }

    fn example_question(record_type: RecordType) -> Query {
        Query::query(query_name("example.com", record_type).unwrap(), record_type)
    }

    #[test]
    fn decodes_hand_built_answers_of_every_supported_type() {
        let cases: Vec<(RecordType, u16, Vec<u8>, &str)> = vec![
            (RecordType::A, 1, vec![93, 184, 216, 34], "93.184.216.34"),
            (
                RecordType::AAAA,
                28,
                "2606:2800:220:1:248:1893:25c8:1946"
                    .parse::<Ipv6Addr>()
                    .unwrap()
                    .octets()
                    .to_vec(),
                "2606:2800:220:1:248:1893:25c8:1946",
            ),
            // "www" + pointer to example.com
            (RecordType::CNAME, 5, b"\x03www\xc0\x0c".to_vec(), "www.example.com"),
            (RecordType::NS, 2, b"\x01a\x0ciana-servers\x03net\x00".to_vec(), "a.iana-servers.net"),
            (RecordType::MX, 15, b"\x00\x0a\x04mail\xc0\x0c".to_vec(), "10 mail.example.com"),
            // Two character-strings are one text.
            (RecordType::TXT, 16, b"\x0bv=spf1 -all\x04 two".to_vec(), "v=spf1 -all two"),
            (
                RecordType::SOA,
                6,
                [
                    b"\x02ns\x05icann\x03org\x00".as_slice(),
                    b"\x03noc\x03dns\x05icann\x03org\x00",
                    &2_024_081_503u32.to_be_bytes(),
                    &7200u32.to_be_bytes(),
                    &3600u32.to_be_bytes(),
                    &1_209_600u32.to_be_bytes(),
                    &3600u32.to_be_bytes(),
                ]
                .concat(),
                "ns.icann.org noc.dns.icann.org 2024081503 7200 3600 1209600 3600",
            ),
            (
                RecordType::SRV,
                33,
                [
                    &10u16.to_be_bytes()[..],
                    &60u16.to_be_bytes(),
                    &5060u16.to_be_bytes(),
                    b"\x03sip\xc0\x0c",
                ]
                .concat(),
                "10 60 5060 sip.example.com",
            ),
            (RecordType::PTR, 12, b"\x03dns\x06google\x00".to_vec(), "dns.google"),
            (
                RecordType::CAA,
                257,
                b"\x00\x05issueletsencrypt.org".to_vec(),
                "0 issue \"letsencrypt.org\"",
            ),
        ];
        for (record_type, code, rdata, expected) in cases {
            let question = example_question(record_type);
            let wire = response_bytes(
                0x1234,
                0x8180,
                &question_bytes(code),
                &[&record_bytes(code, 300, &rdata)],
            );
            let response = matching_response(&wire, 0x1234, &question)
                .unwrap_or_else(|| panic!("{record_type} response should match"));
            let answer = build_answer(
                &question,
                "192.0.2.53:53".parse().unwrap(),
                Duration::from_micros(1500),
                &response,
            );
            assert_eq!(
                answer,
                DnsAnswer {
                    query: "example.com".into(),
                    kind: record_type.to_string(),
                    server: "192.0.2.53:53".into(),
                    elapsed_ms: 1.5,
                    records: vec![DnsRecord {
                        name: "example.com".into(),
                        kind: record_type.to_string(),
                        ttl: 300,
                        data: expected.into(),
                    }],
                    rcode: "NOERROR".into(),
                }
            );
        }
    }

    #[test]
    fn unrelated_and_malformed_datagrams_are_ignored() {
        let question = example_question(RecordType::A);
        let good = response_bytes(7, 0x8180, &question_bytes(1), &[]);
        assert!(matching_response(&good, 7, &question).is_some());
        // Wrong id, a query instead of a response, a different question.
        assert!(matching_response(&good, 8, &question).is_none());
        let query = response_bytes(7, 0x0100, &question_bytes(1), &[]);
        assert!(matching_response(&query, 7, &question).is_none());
        let other_type = response_bytes(7, 0x8180, &question_bytes(28), &[]);
        assert!(matching_response(&other_type, 7, &question).is_none());
        // Every truncation of a valid response either decodes to something unrelated or fails.
        let full = response_bytes(
            7,
            0x8180,
            &question_bytes(1),
            &[&record_bytes(1, 60, &[192, 0, 2, 1])],
        );
        for cut in 0..full.len() {
            let _ = matching_response(&full[..cut], 7, &question);
        }
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..2000 {
            let mut junk = vec![0u8; (state % 300) as usize];
            for byte in &mut junk {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *byte = (state >> 56) as u8;
            }
            let _ = matching_response(&junk, 7, &question);
        }
    }

    #[test]
    fn response_codes_use_dig_names() {
        assert_eq!(rcode_name(ResponseCode::NoError), "NOERROR");
        assert_eq!(rcode_name(ResponseCode::NXDomain), "NXDOMAIN");
        assert_eq!(rcode_name(ResponseCode::ServFail), "SERVFAIL");
        assert_eq!(rcode_name(ResponseCode::Refused), "REFUSED");
        assert_eq!(
            rcode_name(<ResponseCode as From<u16>>::from(4095)),
            "RCODE4095"
        );
    }

    /// What a fake server does with each query it receives.
    #[derive(Clone)]
    enum Behaviour {
        /// Reply with these flags and answer records.
        Answer(u16, Vec<Vec<u8>>),
        /// Reply after a delay.
        Slow(Duration, Vec<Vec<u8>>),
        /// Say nothing.
        Silent,
    }

    /// Starts a UDP DNS server on loopback that answers `example.com` questions.
    async fn fake_server(behaviour: Behaviour) -> (SocketAddr, tokio::task::JoinHandle<u32>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut buffer = [0u8; 1500];
            let mut queries = 0u32;
            loop {
                let Ok(Ok((length, peer))) =
                    tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut buffer)).await
                else {
                    return queries;
                };
                queries += 1;
                let id = u16::from_be_bytes([buffer[0], buffer[1]]);
                // Echo the question section exactly as asked (it ends 4 bytes after the root label).
                let end = 12 + buffer[12..length].iter().position(|b| *b == 0).unwrap() + 5;
                let question = buffer[12..end].to_vec();
                let (delay, flags, answers) = match &behaviour {
                    Behaviour::Answer(flags, answers) => (Duration::ZERO, *flags, answers.clone()),
                    Behaviour::Slow(delay, answers) => (*delay, 0x8180, answers.clone()),
                    Behaviour::Silent => continue,
                };
                tokio::time::sleep(delay).await;
                let answers: Vec<&[u8]> = answers.iter().map(Vec::as_slice).collect();
                let reply = response_bytes(id, flags, &question, &answers);
                socket.send_to(&reply, peer).await.unwrap();
            }
        });
        (address, task)
    }

    fn a_record(last_octet: u8) -> Vec<u8> {
        record_bytes(1, 120, &[192, 0, 2, last_octet])
    }

    #[tokio::test]
    async fn asks_the_named_server_and_reports_its_answer() {
        let (server, _task) = fake_server(Behaviour::Answer(0x8180, vec![a_record(1), a_record(2)])).await;
        let answer = lookup("example.com", "a", Some(&server.to_string()), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(answer.query, "example.com");
        assert_eq!(answer.kind, "A");
        assert_eq!(answer.server, server.to_string());
        assert_eq!(answer.rcode, "NOERROR");
        let data: Vec<&str> = answer.records.iter().map(|r| r.data.as_str()).collect();
        assert_eq!(data, ["192.0.2.1", "192.0.2.2"]);
        assert!(answer.elapsed_ms > 0.0 && answer.elapsed_ms < 2000.0);
    }

    #[tokio::test]
    async fn nxdomain_is_an_answer_not_an_error() {
        let (server, _task) = fake_server(Behaviour::Answer(0x8183, vec![])).await;
        let answer = lookup("example.com", "MX", Some(&server.to_string()), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(answer.rcode, "NXDOMAIN");
        assert!(answer.records.is_empty());
    }

    #[tokio::test]
    async fn fails_over_from_a_failing_server_to_the_next() {
        let (broken, _t1) = fake_server(Behaviour::Answer(0x8182, vec![])).await;
        let (silent, _t2) = fake_server(Behaviour::Silent).await;
        let (healthy, _t3) = fake_server(Behaviour::Answer(0x8180, vec![a_record(9)])).await;
        let started = Instant::now();
        let answer = lookup_at(
            example_question(RecordType::A),
            &[broken, silent, healthy],
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(answer.server, healthy.to_string());
        assert_eq!(answer.records[0].data, "192.0.2.9");
        // SERVFAIL moves on at once; the silent server costs one step (3 s / 6 transmissions).
        assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
    }

    #[tokio::test]
    async fn a_failure_answer_is_returned_when_no_server_does_better() {
        let (broken, _t1) = fake_server(Behaviour::Answer(0x8182, vec![])).await;
        let (refusing, _t2) = fake_server(Behaviour::Answer(0x8185, vec![])).await;
        let started = Instant::now();
        let answer = lookup_at(
            example_question(RecordType::A),
            &[broken, refusing],
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(answer.rcode, "SERVFAIL");
        assert_eq!(answer.server, broken.to_string());
        assert!(started.elapsed() < Duration::from_secs(2), "must not wait for the deadline");
    }

    #[tokio::test]
    async fn a_slow_answer_to_the_first_transmission_is_still_accepted() {
        // One server, 1.2 s timeout: retransmission at 0.6 s, the answer to the first query at 0.8 s.
        let (server, task) =
            fake_server(Behaviour::Slow(Duration::from_millis(800), vec![a_record(7)])).await;
        let answer = lookup_at(
            example_question(RecordType::A),
            &[server],
            Duration::from_millis(1200),
        )
        .await
        .unwrap();
        assert_eq!(answer.records[0].data, "192.0.2.7");
        assert!(answer.elapsed_ms >= 800.0, "{}", answer.elapsed_ms);
        drop(task);
    }

    #[tokio::test]
    async fn silence_times_out_and_a_closed_port_is_refused() {
        let (silent, task) = fake_server(Behaviour::Silent).await;
        let started = Instant::now();
        let error = lookup_at(
            example_question(RecordType::A),
            &[silent],
            Duration::from_millis(400),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProbeError::Timeout), "{error:?}");
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(390) && waited < Duration::from_millis(900), "{waited:?}");
        assert_eq!(task.await.unwrap(), 2, "one transmission per round");

        // A port nobody listens on answers with ICMP port unreachable.
        let closed = {
            let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            socket.local_addr().unwrap()
        };
        let started = Instant::now();
        let error = lookup_at(example_question(RecordType::A), &[closed], Duration::from_secs(3))
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Refused), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "refusal must not wait for the timeout");
    }

    #[tokio::test]
    async fn truncated_udp_answer_is_retried_over_tcp() {
        // UDP and TCP on the same port: bind TCP first, then UDP on that port number.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let udp = UdpSocket::bind(address).await.unwrap();
        let records: Vec<Vec<u8>> = (1..=40).map(a_record).collect();
        let tcp_records = records.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 1500];
            let (length, peer) = udp.recv_from(&mut buffer).await.unwrap();
            let id = u16::from_be_bytes([buffer[0], buffer[1]]);
            let end = 12 + buffer[12..length].iter().position(|b| *b == 0).unwrap() + 5;
            // TC set, one record only.
            let first = a_record(1);
            let reply = response_bytes(id, 0x8380, &buffer[12..end], &[&first]);
            udp.send_to(&reply, peer).await.unwrap();
        });
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut length = [0u8; 2];
            stream.read_exact(&mut length).await.unwrap();
            let mut query = vec![0u8; usize::from(u16::from_be_bytes(length))];
            stream.read_exact(&mut query).await.unwrap();
            let id = u16::from_be_bytes([query[0], query[1]]);
            let end = 12 + query[12..].iter().position(|b| *b == 0).unwrap() + 5;
            let answers: Vec<&[u8]> = tcp_records.iter().map(Vec::as_slice).collect();
            let reply = response_bytes(id, 0x8180, &query[12..end], &answers);
            stream.write_all(&(reply.len() as u16).to_be_bytes()).await.unwrap();
            stream.write_all(&reply).await.unwrap();
        });
        let answer = lookup_at(example_question(RecordType::A), &[address], Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(answer.records.len(), 40);
        assert_eq!(answer.records[39].data, "192.0.2.40");
    }

    #[tokio::test]
    async fn resolve_handles_literals_and_bad_input() {
        assert_eq!(
            resolve("192.0.2.7").await.unwrap(),
            vec!["192.0.2.7".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            resolve(" [2001:db8::1] ").await.unwrap(),
            vec!["2001:db8::1".parse::<IpAddr>().unwrap()]
        );
        assert!(matches!(resolve("").await, Err(ProbeError::Invalid(_))));
        assert!(matches!(resolve("10.0.0.0/24").await, Err(ProbeError::Invalid(_))));
        assert!(matches!(resolve("two words").await, Err(ProbeError::Invalid(_))));
        // RFC 6761: .invalid never resolves.
        assert!(matches!(
            resolve("no-such-host.invalid").await,
            Err(ProbeError::Resolve)
        ));
    }

    #[tokio::test]
    async fn localhost_resolves_through_the_hosts_file() {
        let addresses = resolve("localhost").await.unwrap();
        assert!(addresses.iter().all(IpAddr::is_loopback), "{addresses:?}");
        assert_eq!(resolve_one("localhost").await.unwrap().is_loopback(), true);
    }

    #[tokio::test]
    async fn reverse_uses_the_hosts_file_before_dns() {
        // 127.0.0.1 is named in every hosts file; no DNS server is involved.
        let name = reverse("127.0.0.1".parse().unwrap(), Duration::from_millis(500)).await;
        let hosts = std::fs::read_to_string("/etc/hosts").unwrap_or_default();
        let listed = hosts.lines().any(|line| {
            let mut fields = line.split('#').next().unwrap_or("").split_whitespace();
            fields.next() == Some("127.0.0.1") && fields.next().is_some()
        });
        if listed {
            assert!(name.is_some(), "127.0.0.1 is in /etc/hosts but was not reverse-resolved");
        }
    }
}
