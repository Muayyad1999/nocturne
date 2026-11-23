# Spec: `nocturne-netprobe` — active probes and small network servers

The I/O half of the NOC toolbox, built on tokio. It is used three ways: by the
**Tools** screen (a person runs a ping or a port scan), by the **monitoring
engine** (thousands of reachability probes per minute, a syslog listener that
never stops), and by the **AI agent** (probing from the operator's
workstation while it investigates).

All result types derive `Serialize, Deserialize, Clone, Debug` (`snake_case`
enums) — they go to the UI as JSON. One error enum, `ProbeError`
(`thiserror`), with messages written for end users. Every operation takes
explicit timeouts and is cancel-safe (dropping the future stops the work and
releases sockets). Nothing may panic on hostile network input: every decoder
has a test that feeds it truncated and random datagrams.

Must build on Linux, macOS and Windows. Platform-specific code lives behind
`cfg` with a working fallback on every platform; check
`cargo check -p nocturne-netprobe --target x86_64-pc-windows-gnu` (install the
target with `rustup target add`, and `mingw-w64` with apt if a linker is
needed) and report the result.

## Public API (contract)

```rust
// -------------------------------------------------------------------- ping
pub mod ping {
    /// How echo requests are sent on this machine.
    pub enum Method { IcmpDatagram, IcmpRaw, SystemCommand }
    #[derive(Clone)]
    pub struct Pinger { /* cheap to clone; safe to use from many tasks at once */ }
    impl Pinger {
        /// Detect the best available method once: unprivileged ICMP datagram sockets
        /// (Linux with ping_group_range, macOS), raw sockets (root/CAP_NET_RAW/Administrator),
        /// otherwise the operating system's `ping` program.
        pub fn new() -> Pinger;
        pub fn method(&self) -> Method;
        /// One echo. `Ok(rtt)` on a reply; errors distinguish timeout, unreachable (with the ICMP reason), and local failures.
        pub async fn ping_once(&self, target: std::net::IpAddr, timeout: std::time::Duration, payload: usize) -> Result<PingReply, ProbeError>;
    }
    pub struct PingReply { pub rtt_ms: f64, pub ttl: Option<u8>, pub bytes: usize }
    pub struct PingOptions { pub count: u32 /*4*/, pub interval_ms: u64 /*1000*/, pub timeout_ms: u64 /*1000*/, pub payload: usize /*56*/ }
    pub struct PingEcho { pub seq: u32, pub rtt_ms: Option<f64>, pub ttl: Option<u8>, pub error: Option<String> }
    pub struct PingReport {
        pub target: String, pub address: String, pub method: Method,
        pub sent: u32, pub received: u32, pub loss_pct: f64,
        pub min_ms: Option<f64>, pub avg_ms: Option<f64>, pub max_ms: Option<f64>, pub stddev_ms: Option<f64>,
        pub echoes: Vec<PingEcho>,
    }
    /// Resolve `host`, send `count` echoes, report. Each echo is also sent to `progress` as it completes.
    pub async fn ping(pinger: &Pinger, host: &str, options: &PingOptions, progress: Option<tokio::sync::mpsc::UnboundedSender<PingEcho>>) -> Result<PingReport, ProbeError>;
}

// ------------------------------------------------------------------- sweep
pub mod sweep {
    pub struct SweepOptions { pub concurrency: usize /*128*/, pub timeout_ms: u64 /*800*/, pub retries: u32 /*1*/,
                              pub tcp_fallback_ports: Vec<u16> /* [22, 23, 80, 443, 8291] */, pub resolve_names: bool, pub limit: usize /*65536*/ }
    pub struct HostResult { pub address: String, pub alive: bool, pub rtt_ms: Option<f64>, pub via: String /* "icmp" | "tcp/22" | "arp" */,
                            pub hostname: Option<String>, pub mac: Option<String>, pub open_ports: Vec<u16> }
    /// Probe every host address in the targets ("10.0.0.0/24", "10.0.0.5-10.0.0.40", single addresses, comma separated).
    /// Alive hosts are streamed to `found` as they answer; the return value is the full sorted list of alive hosts.
    /// After the sweep the local neighbour table is consulted to attach MAC addresses (and to catch hosts that
    /// dropped ICMP but answered ARP).
    pub async fn sweep(pinger: &crate::ping::Pinger, targets: &str, options: &SweepOptions,
                       found: Option<tokio::sync::mpsc::UnboundedSender<HostResult>>) -> Result<Vec<HostResult>, ProbeError>;
    /// Expand a target expression to addresses (bounded by `limit`).
    pub fn expand_targets(targets: &str, limit: usize) -> Result<Vec<std::net::IpAddr>, ProbeError>;
}

// ---------------------------------------------------------------- traceroute
pub mod traceroute {
    pub struct TraceOptions { pub max_hops: u8 /*30*/, pub probes: u8 /*3*/, pub timeout_ms: u64 /*2000*/, pub resolve_names: bool }
    pub struct Hop { pub ttl: u8, pub address: Option<String>, pub hostname: Option<String>, pub rtts_ms: Vec<Option<f64>>, pub note: String }
    pub struct TraceReport { pub target: String, pub address: String, pub hops: Vec<Hop>, pub reached: bool, pub method: String }
    /// Hops are streamed to `progress` as they are discovered. Uses a native implementation where the platform
    /// allows it without privileges (Linux: UDP probes + IP_RECVERR error queue), otherwise drives the system
    /// `traceroute` / `tracert` and parses its output incrementally (Linux, macOS/BSD and Windows formats).
    pub async fn traceroute(host: &str, options: &TraceOptions, progress: Option<tokio::sync::mpsc::UnboundedSender<Hop>>) -> Result<TraceReport, ProbeError>;
}

// ----------------------------------------------------------------------- tcp
pub mod tcp {
    pub enum PortState { Open, Closed, Filtered }
    pub struct PortResult { pub port: u16, pub state: PortState, pub rtt_ms: Option<f64>, pub service: String, pub banner: Option<String> }
    pub struct ScanOptions { pub timeout_ms: u64 /*1000*/, pub concurrency: usize /*200*/, pub grab_banners: bool /*true*/ }
    /// "22,80,443", "1-1024", "top100", "common" (a curated network-admin list), or a mix.
    pub fn parse_ports(spec: &str) -> Result<Vec<u16>, ProbeError>;
    /// Connect scan. Results for open ports are streamed to `progress`; the return value lists every port scanned.
    /// Banner grabbing reads what the server volunteers (SSH, FTP, SMTP, Telnet negotiation → "telnet") and, for
    /// HTTP-looking ports, sends one `HEAD / HTTP/1.0` to read the `Server:` header. Never sends anything else.
    pub async fn scan(host: &str, ports: &[u16], options: &ScanOptions, progress: Option<tokio::sync::mpsc::UnboundedSender<PortResult>>) -> Result<Vec<PortResult>, ProbeError>;
    pub async fn check(host: &str, port: u16, timeout: std::time::Duration) -> Result<PortResult, ProbeError>;
}

// ----------------------------------------------------------------------- dns
pub mod dns {
    pub struct DnsRecord { pub name: String, pub kind: String, pub ttl: u32, pub data: String }
    pub struct DnsAnswer { pub query: String, pub kind: String, pub server: String, pub elapsed_ms: f64, pub records: Vec<DnsRecord>, pub rcode: String }
    /// kind: A, AAAA, CNAME, MX, NS, TXT, SOA, SRV, PTR, CAA, ANY. `server` None = system resolvers.
    /// A PTR query accepts a plain IP address.
    pub async fn lookup(name: &str, kind: &str, server: Option<&str>, timeout: std::time::Duration) -> Result<DnsAnswer, ProbeError>;
    /// Forward-resolve with the system resolver (hosts file included); a literal address is returned as is.
    pub async fn resolve(host: &str) -> Result<Vec<std::net::IpAddr>, ProbeError>;
    pub async fn reverse(address: std::net::IpAddr, timeout: std::time::Duration) -> Option<String>;
}

// --------------------------------------------------------------------- whois
pub mod whois {
    pub struct WhoisAnswer { pub query: String, pub servers: Vec<String>, pub text: String, pub summary: Vec<(String, String)> }
    /// IP addresses, AS numbers ("AS13335") and domain names. Starts at whois.iana.org and follows the
    /// `refer:` / `ReferralServer:` chain (at most 4 hops). `summary` extracts netname / org / country / CIDR / abuse contact.
    pub async fn whois(query: &str, timeout: std::time::Duration) -> Result<WhoisAnswer, ProbeError>;
}

// ----------------------------------------------------------------------- tls
pub mod tls {
    pub struct CertInfo { pub subject: String, pub issuer: String, pub common_name: String, pub sans: Vec<String>,
                          pub not_before: String, pub not_after: String, pub days_left: i64,
                          pub serial: String, pub signature_algorithm: String, pub public_key: String /* "RSA 2048", "ECDSA P-256" */,
                          pub is_ca: bool, pub self_signed: bool, pub sha256_fingerprint: String }
    pub struct TlsReport { pub host: String, pub address: String, pub port: u16, pub protocol: String /* "TLSv1.3" */, pub cipher: String,
                           pub alpn: Option<String>, pub chain: Vec<CertInfo>,
                           pub trusted: bool, pub trust_error: Option<String>, pub hostname_matches: bool,
                           pub handshake_ms: f64, pub expires_in_days: i64 }
    /// Handshake and describe the certificate chain **even when it does not verify** (expired, self-signed, wrong
    /// name — that is exactly what the tool is for): complete the handshake with a verifier that records and
    /// accepts, then evaluate trust separately with the platform verifier and report the outcome.
    /// `now_unix` is passed in so `days_left` is testable.
    pub async fn inspect(host: &str, port: u16, sni: Option<&str>, timeout: std::time::Duration, now_unix: i64) -> Result<TlsReport, ProbeError>;
}

// ---------------------------------------------------------------------- http
pub mod http {
    pub struct HttpReport { pub url: String, pub final_url: String, pub status: u16, pub reason: String, pub version: String,
                            pub headers: Vec<(String, String)>, pub redirects: Vec<(u16, String)>,
                            pub dns_ms: f64, pub connect_ms: f64, pub tls_ms: Option<f64>, pub first_byte_ms: f64, pub total_ms: f64,
                            pub body_bytes: u64, pub body_preview: String, pub tls: Option<crate::tls::TlsReport> }
    pub struct HttpOptions { pub method: String /*GET*/, pub headers: Vec<(String, String)>, pub follow_redirects: u8 /*5*/,
                             pub timeout_ms: u64 /*10000*/, pub verify_tls: bool /*true*/, pub max_body_bytes: u64 /*1 MiB*/ }
    /// A small HTTP/1.1 client written on tokio + tokio-rustls so each phase can be timed
    /// (chunked and content-length bodies, redirects, `Connection: close`). Plain HTTP and HTTPS.
    pub async fn check(url: &str, options: &HttpOptions, now_unix: i64) -> Result<HttpReport, ProbeError>;
}

// ----------------------------------------------------------------------- ntp
pub mod ntp {
    pub struct NtpReport { pub server: String, pub address: String, pub stratum: u8, pub reference: String, pub leap: String, pub version: u8,
                           pub offset_ms: f64, pub delay_ms: f64, pub root_delay_ms: f64, pub root_dispersion_ms: f64, pub server_time_unix: f64 }
    /// One SNTP exchange (RFC 4330). `local_unix` is the caller's clock reading at send time; receive time is
    /// derived from a monotonic clock so the function itself never reads the wall clock.
    pub async fn query(server: &str, timeout: std::time::Duration, local_unix: f64) -> Result<NtpReport, ProbeError>;
}

// ------------------------------------------------------------------- local
pub mod local {
    pub struct LocalInterface { pub name: String, pub addresses: Vec<String> /* CIDR */, pub mac: Option<String>, pub is_loopback: bool, pub is_up: bool }
    pub fn interfaces() -> Vec<LocalInterface>;
    pub struct Neighbour { pub address: String, pub mac: String, pub interface: String, pub state: String }
    /// The workstation's ARP / NDP table (`ip neigh`, `arp -a` — parse each OS's format).
    pub async fn neighbours() -> Vec<Neighbour>;
    pub struct LocalRoute { pub destination: String, pub gateway: String, pub interface: String }
    pub async fn default_gateways() -> Vec<LocalRoute>;
    /// Send a Wake-on-LAN magic packet (UDP 9) to the broadcast address(es).
    pub async fn wake_on_lan(mac: &str, broadcast: Option<&str>) -> Result<Vec<String>, ProbeError>;
}

// ---------------------------------------------------------------------- tftp
pub mod tftp {
    pub struct TftpConfig { pub bind: std::net::SocketAddr, pub root: std::path::PathBuf, pub allow_read: bool, pub allow_write: bool,
                            pub allow_overwrite: bool, pub timeout_ms: u64 /*3000*/, pub retries: u32 /*5*/, pub max_file_bytes: u64 }
    pub enum TftpEvent {
        Started { id: u64, peer: String, file: String, write: bool, size: Option<u64> },
        Progress { id: u64, bytes: u64 },
        Finished { id: u64, bytes: u64, elapsed_ms: u64 },
        Failed { id: u64, error: String },
        Denied { peer: String, file: String, reason: String },
    }
    pub struct TftpServer { /* … */ }
    impl TftpServer {
        /// RFC 1350 read and write, octet mode (netascii accepted and treated as octet), option negotiation
        /// (RFC 2347/2348/2349: blksize up to 65464, tsize, timeout), block-number wraparound for large files,
        /// each transfer on its own ephemeral port, many concurrent transfers.
        /// Files are confined to `root`: absolute paths, `..`, symlinks that leave the root and device names are refused.
        pub async fn start(config: TftpConfig) -> Result<(TftpServer, tokio::sync::mpsc::UnboundedReceiver<TftpEvent>), ProbeError>;
        pub fn local_addr(&self) -> std::net::SocketAddr;
        pub async fn stop(self);
    }
}

// -------------------------------------------------------------------- syslog
pub mod syslog {
    pub struct SyslogMessage {
        pub source: String,                 // sender address
        pub facility: Option<u8>, pub severity: Option<u8>,
        pub timestamp: Option<String>,      // as written by the device (not reinterpreted)
        pub hostname: Option<String>, pub app: Option<String>, pub procid: Option<String>, pub msgid: Option<String>,
        pub message: String,                // the human part
        /// Vendor mnemonic when present: Cisco "%LINK-3-UPDOWN" → ("LINK", 3, "UPDOWN"); Junos tags; RouterOS topics.
        pub mnemonic: Option<Mnemonic>,
        pub sequence: Option<u64>,
        pub raw: String,
    }
    pub struct Mnemonic { pub facility: String, pub severity: Option<u8>, pub name: String }
    /// Parse one datagram / line. Never fails: unparseable input becomes a message with only `raw` and `message` set.
    /// Formats: RFC 5424; RFC 3164 with and without hostname; Cisco IOS/IOS-XE/NX-OS/ASA variants
    /// (`<189>45: *Mar  1 00:02:11.003: %LINK-3-UPDOWN: …`, with `service sequence-numbers`, with year/timezone,
    /// with a hostname before the timestamp); Junos (`<30>Jan  1 12:00:00 host mgd[1234]: UI_COMMIT: …`);
    /// MikroTik RouterOS (`system,info,account user admin logged in from 10.0.0.2 via ssh`, BSD-wrapped or bare);
    /// Arista, Huawei (`%%01IFNET/4/LINK_STATE(l)[5]:…`), FortiGate key=value (`date=… logid=… msg="…"`).
    pub fn parse(datagram: &[u8], source: std::net::SocketAddr) -> SyslogMessage;
    pub fn severity_name(severity: u8) -> &'static str;      // "emergency" … "debug"
    pub fn facility_name(facility: u8) -> &'static str;
    pub struct SyslogServer { /* … */ }
    impl SyslogServer {
        /// Listen on UDP and (optionally) TCP — RFC 6587 octet-counted and newline-framed. Messages arrive on the channel.
        pub async fn start(udp: std::net::SocketAddr, tcp: Option<std::net::SocketAddr>)
            -> Result<(SyslogServer, tokio::sync::mpsc::UnboundedReceiver<SyslogMessage>), ProbeError>;
        pub fn udp_addr(&self) -> std::net::SocketAddr;
        pub fn tcp_addr(&self) -> Option<std::net::SocketAddr>;
        pub async fn stop(self);
    }
}

// ---------------------------------------------------------------------- flow
pub mod flow {
    pub struct FlowRecord {
        pub exporter: String, pub version: u8,                    // 5, 9, 10 (IPFIX)
        pub src: String, pub dst: String, pub src_port: u16, pub dst_port: u16, pub protocol: u8,
        pub bytes: u64, pub packets: u64, pub tos: u8, pub tcp_flags: u8,
        pub input_if: u32, pub output_if: u32, pub src_as: u32, pub dst_as: u32,
        pub next_hop: Option<String>, pub start_ms: u64, pub end_ms: u64,     // exporter sysUptime-relative for v5/v9, epoch for IPFIX when given
        pub sampling: u32,
    }
    /// Stateful decoder: keeps v9 / IPFIX templates per (exporter, source id, template id) — at most 4,096,
    /// the oldest making room — and the sampling rate each (exporter, source id) announced, at most 8,192.
    pub struct Decoder { /* … */ }
    impl Decoder {
        pub fn new() -> Decoder;
        /// Decode one export datagram: NetFlow v5, v9 (templates; options templates read for the sampling
        /// rate only) and IPFIX (enterprise fields skipped, variable-length fields handled). Data sets for
        /// unknown templates are counted and dropped until the template arrives. `sampling` is read from the
        /// v5 header, from v9 options (fields 34, 50) and from IPFIX options the PSAMP way (305 + 306:
        /// take `interval`, skip `space`; 309 + 310: `size` out of `population`).
        pub fn decode(&mut self, datagram: &[u8], exporter: std::net::SocketAddr) -> Result<Vec<FlowRecord>, ProbeError>;
        pub fn stats(&self) -> DecoderStats;        // packets, records, templates, missing-template sets, errors
        /// Templates known for one exporter address, over all its source ports and source ids.
        pub fn templates_of(&self, exporter: std::net::IpAddr) -> u32;
    }
    impl DecoderStats {
        /// What was counted after an earlier reading of the same decoder; `templates` is the latest.
        pub fn since(&self, earlier: &DecoderStats) -> DecoderStats;
    }
    /// A decoder that outlives the collectors that use it (`Clone` shares it). What an exporter announced —
    /// its templates, the sampling rate in its options — is the exporter's, not the socket's: a collector
    /// started with the decoder the one before it used reads what the exporter sends next with what it
    /// said before. One given a fresh decoder knows nothing until the exporter says it again, and reads
    /// every record before its next options as unsampled.
    pub struct SharedDecoder { /* … */ }
    impl SharedDecoder {
        pub fn new() -> SharedDecoder;              // also `Default`
        pub fn templates_of(&self, exporter: std::net::IpAddr) -> u32;
        pub fn stats(&self) -> DecoderStats;        // over every collector that used it
    }
    pub struct Talker { pub key: String, pub bytes: u64, pub packets: u64, pub flows: u64 }
    /// Aggregate records by "src" | "dst" | "conversation" | "application" (protocol/port) | "interface" and return the top N by bytes.
    pub fn top(records: &[FlowRecord], by: &str, n: usize) -> Vec<Talker>;
    /// What one export datagram held. Every datagram that decodes is reported, also one without a flow
    /// (templates only, or data whose template has not arrived): it still says an exporter is there.
    pub struct FlowBatch {
        pub exporter: std::net::IpAddr,             // canonical: an IPv4 sender is IPv4 also on a socket bound to ::
        pub version: u8, pub records: Vec<FlowRecord>,
        pub sets_without_template: u32,             // in this datagram
        pub templates: u32,                         // known for this exporter address after it
    }
    impl FlowBatch {
        /// What it takes up: itself, its records and the addresses they hold as text — an estimate
        /// from what it owns. A datagram of 64 KiB can be sixty thousand one-byte records, each a
        /// whole `FlowRecord` once decoded: about 11 MiB.
        pub fn weight(&self) -> usize;
    }
    pub struct Dropped { pub datagrams: u64, pub records: u64 }
    /// What may wait to be taken: flow records (and as many datagrams, for those that carry none), and
    /// the bytes they take up (`FlowBatch::weight`).
    pub struct QueueLimits { pub records: usize, pub bytes: usize }
    /// The receiving end: `recv().await` → the next datagram's batch, `None` once the collector has
    /// stopped and everything it had decoded has been taken.
    pub struct FlowQueue { /* … */ }
    /// A collector's counters, readable after it has stopped: `stats()`, `dropped()`, `waiting()`,
    /// `waiting_bytes()`. `stats()` is what this collector decoded (its decoder may have been used
    /// before), and the templates known now.
    pub struct FlowCounters { /* … */ }
    pub struct FlowCollector { /* … */ }
    impl FlowCollector {
        /// At most `limits.records` flow records (and that many datagrams) taking up at most
        /// `limits.bytes` wait to be taken. A datagram that would take the queue past either is decoded —
        /// its templates are still learned — then dropped and counted. One datagram is always let onto an
        /// empty queue, whatever it holds; while one that alone is more than a limit waits, what arrives
        /// is measured without it, so that it does not turn every other exporter away (the queue holds at
        /// most the limits and that one datagram). Datagrams are decoded with `decoder`, which the caller keeps.
        pub async fn start(bind: std::net::SocketAddr, limits: QueueLimits, decoder: SharedDecoder) -> Result<(FlowCollector, FlowQueue), ProbeError>;
        pub fn local_addr(&self) -> std::net::SocketAddr;
        pub fn stats(&self) -> DecoderStats;
        pub fn dropped(&self) -> Dropped;           // decoded, then thrown away because the queue was full
        pub fn waiting(&self) -> usize;             // flow records decoded and not yet taken
        pub fn waiting_bytes(&self) -> usize;       // what they take up
        pub fn counters(&self) -> FlowCounters;
        pub async fn stop(self);
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ProbeError { /* Resolve, Timeout, Unreachable(String), Refused, Permission(String), Unsupported(String), Invalid(String), Io(std::io::Error), Protocol(String) */ }
```

Every options struct implements `Default` with the defaults noted and is `#[serde(default)]`.

## Notes on the hard parts

* **Ping without privileges.** Linux allows `socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP)` when
  `net.ipv4.ping_group_range` covers the user (the kernel rewrites the identifier and demultiplexes per
  socket — one socket per in-flight echo is fine); macOS allows the same. Windows has no such socket: use the
  system `ping` there (the fallback must exist anyway), or `IcmpSendEcho2` through `windows-sys` if you can do it
  cleanly on a blocking thread — optional. The system-command fallback must parse output without depending on the
  UI language (match on `time[=<]\s*[\d.,]+\s*ms`, `TTL=`/`ttl=` and the exit status), never interpolate
  untrusted text into a shell (spawn the binary with arguments), and bound concurrency.
  IPv6 echo is supported on the socket paths (`IPPROTO_ICMPV6`) and through `ping -6`/`ping6`.
* **Sweeps** are how inventory discovery starts; a /16 must not open 65k sockets at once or take an hour.
  Bounded concurrency, per-host timeout, early results.
* **TLS inspection** needs a custom `rustls` `ServerCertVerifier` that accepts anything; that verifier is
  private to the `tls` module and must never be reachable from any other code path. Trust evaluation uses
  `rustls-platform-verifier`. The process-wide rustls crypto provider is `ring`
  (`rustls::crypto::ring::default_provider().install_default()` once, ignoring "already installed").
* **Servers** (`tftp`, `syslog`, `flow`) run until `stop()` or drop, never block the runtime, and survive
  malformed or hostile datagrams. The flow collector's queue has a limit in flow records and one in the
  bytes they take up once decoded, and counts what it drops; nothing the decoder remembers per exporter
  grows without bound. Binding to a privileged port without rights must produce
  `ProbeError::Permission` with a message that tells the user what to do (choose a port ≥ 1024, or on Linux
  grant `CAP_NET_BIND_SERVICE`).

## Validation

1. Unit tests for every parser and codec with realistic samples: syslog lines in every listed format (use real
   message shapes), NetFlow v5/v9/IPFIX datagrams built by hand per the RFCs, `traceroute`/`tracert`/`ping`
   outputs from Linux, macOS and Windows (including a non-English Windows sample), neighbour tables from each
   OS, target and port expressions, whois referral parsing, SNTP packets.
2. Loopback integration tests that run anywhere: `Pinger` against 127.0.0.1 and ::1 with whatever method
   the machine allows, plus a forced system-command run; port scan against listeners the test opens (open /
   closed / banner grab / HTTP HEAD); TFTP read + write + options + block wraparound (use a small blksize) +
   path-escape attempts against the crate's own minimal test client; syslog over UDP and both TCP framings;
   TLS inspection against a local `tokio-rustls` server with a self-signed and an expired certificate
   (generate certificates in the test with `rcgen` as a dev-dependency); HTTP check against a local server
   (chunked body, redirect chain, HTTPS).
3. **Interoperability with real tools on this machine** (skip, do not fail, when missing; you are root and may
   `apt-get install` test-only packages): TFTP against `curl tftp://…` (get and `-T` put) and, if installable,
   `tftp-hpa`; syslog with `logger -n 127.0.0.1 -P <port>` in `--rfc3164` and `--rfc5424` modes, UDP and TCP;
   NetFlow with `softflowd` exporting v5 / v9 / IPFIX — from a capture file the test writes
   (`softflowd -r`), which needs no capture rights and makes the right answer known, rather than from
   traffic on `lo` — and the datagrams softflowd and nfdump's `nfreplay` sent, written down and decoded
   where neither is installed (`flow/tests/captures.rs` says how they were made); DNS against the machine's resolver and a public one if reachable; ping/traceroute against 127.0.0.1.
   Outbound internet may be filtered by a proxy: tests that need the internet must detect that and skip.
4. Robustness: random and truncated datagrams into `syslog::parse`, `flow::Decoder::decode`, the TFTP server
   socket and the SNTP reply parser — no panics, no unbounded memory.
5. `cargo clippy -p nocturne-netprobe --all-targets -- -D warnings` clean. `unsafe` only if a platform API
   leaves no alternative, isolated in one small module with a safety comment.

## Dependencies

Declared already: `tokio`, `serde`, `serde_json`, `thiserror`, `tracing`, `socket2`, `hickory-resolver`,
`rustls`, `tokio-rustls`, `rustls-platform-verifier`, `x509-parser`, `if-addrs`, `regex`, `bytes`; dev:
`pretty_assertions`, `tempfile`. You may add small pure-Rust crates with explicit versions when they earn their
place (`rcgen` for tests, `windows-sys` for a Windows-only path) — say which and why in the report. No C
dependencies, no OpenSSL.
