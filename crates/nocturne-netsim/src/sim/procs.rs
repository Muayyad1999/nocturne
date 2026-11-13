//! Long-running commands as node-level processes: ping, traceroute, TCP
//! connection attempts and DNS lookups.
//!
//! A process is driven purely by simulation events (replies and timers), so its
//! timing does not depend on how the host slices `advance` calls. Consoles and
//! the automation helpers only read the text it accumulates.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{SimTime, TraceId};
use crate::sim::frame::{DnsMsg, Flow, Icmp, L4, Meta, PORT_DNS, PORT_TRACEROUTE, Packet, TcpFlags, UdpApp, Unreach};
use crate::sim::fx::{Fx, Timer};
use crate::sim::ip::Sent;
use crate::sim::node::Node;
use crate::util::mac::Mac;

/// Output conventions of the shell that started the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Ios,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Pending,
    /// Succeeded; round-trip time in microseconds.
    Ok { rtt: u64 },
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct PingSpec {
    pub(crate) dst: Ipv4Addr,
    pub(crate) src: Option<Ipv4Addr>,
    pub(crate) count: u32,
    pub(crate) timeout_micros: u64,
    /// ICMP payload size (Windows) or datagram size (IOS), bytes.
    pub(crate) size: u16,
    pub(crate) style: Style,
    /// Name typed by the user, if the target was resolved through DNS.
    pub(crate) label: Option<String>,
}

#[derive(Clone, Debug)]
struct Ping {
    spec: PingSpec,
    ident: u16,
    seq: u16,
    sent: u32,
    received: u32,
    awaiting: bool,
    sent_at: SimTime,
    last_send: Sent,
    rtt_min: u64,
    rtt_max: u64,
    rtt_sum: u64,
}

#[derive(Clone, Debug)]
struct Connect {
    dst: Ipv4Addr,
    port: u16,
    sport: u16,
    style: Style,
    attempts: u32,
    started: SimTime,
}

#[derive(Clone, Debug)]
struct Traceroute {
    dst: Ipv4Addr,
    style: Style,
    ident: u16,
    ttl: u8,
    /// Probe within the current hop (0..3).
    probe: u8,
    sport: u16,
    sent_at: SimTime,
    awaiting: bool,
    /// Address that answered the current hop, once known.
    hop_addr: Option<Ipv4Addr>,
    reached: bool,
    unreachable: bool,
    max_ttl: u8,
}

#[derive(Clone, Debug)]
struct Lookup {
    name: String,
    server: Ipv4Addr,
    ident: u16,
    attempts: u32,
    /// Continue with a ping of the answer (host `ping <name>`).
    then_ping: Option<PingSpec>,
    silent: bool,
}

impl Lookup {
    /// Stand-in used for the instant between creating a process and giving it its job.
    fn placeholder() -> Lookup {
        Lookup { name: String::new(), server: Ipv4Addr::UNSPECIFIED, ident: 0, attempts: 0, then_ping: None, silent: true }
    }
}

#[derive(Clone, Debug)]
enum Kind {
    Ping(Ping),
    Connect(Connect),
    Traceroute(Traceroute),
    Lookup(Lookup),
}

#[derive(Clone, Debug)]
pub(crate) struct Proc {
    /// Text produced so far (`\n` line ends).
    pub(crate) out: String,
    pub(crate) done: bool,
    pub(crate) outcome: Outcome,
    /// Address resolved by a lookup, or reached by a connect.
    pub(crate) address: Option<Ipv4Addr>,
    token: u32,
    trace: Option<TraceId>,
    kind: Kind,
}

impl Proc {
    pub(crate) fn trace(&self) -> Option<TraceId> {
        self.trace
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Procs {
    next_id: u32,
    pub(crate) list: BTreeMap<u32, Proc>,
}

impl Procs {
    pub(crate) fn clear(&mut self) {
        self.list.clear();
    }

    pub(crate) fn get(&self, id: u32) -> Option<&Proc> {
        self.list.get(&id)
    }

    pub(crate) fn remove(&mut self, id: u32) {
        self.list.remove(&id);
    }
}

/// Milliseconds as IOS prints them: never less than 1.
fn ios_ms(micros: u64) -> u64 {
    micros.div_ceil(1000).max(1)
}

const SYN_RETRY_MICROS: u64 = 1_000_000;
const SYN_ATTEMPTS: u32 = 4;
const WINDOWS_PING_GAP_MICROS: u64 = 1_000_000;
const TRACEROUTE_TIMEOUT_MICROS: u64 = 1_000_000;
const DNS_TIMEOUT_MICROS: u64 = 1_000_000;

impl Node {
    fn spawn(&mut self, kind: Kind, trace: Option<TraceId>) -> u32 {
        self.procs.next_id += 1;
        let id = self.procs.next_id;
        self.procs.list.insert(
            id,
            Proc { out: String::new(), done: false, outcome: Outcome::Pending, address: None, token: 0, trace, kind },
        );
        id
    }

    /// Stop a process as if the user pressed the abort key.
    pub(crate) fn proc_abort(&mut self, id: u32) {
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        if proc.done {
            return;
        }
        proc.token = proc.token.wrapping_add(1);
        match &mut proc.kind {
            Kind::Ping(ping) => {
                let summary = ping_summary(ping);
                proc.out.push_str(&summary);
            }
            Kind::Connect(_) | Kind::Traceroute(_) | Kind::Lookup(_) => proc.out.push('\n'),
        }
        proc.done = true;
        if proc.outcome == Outcome::Pending {
            proc.outcome = Outcome::Failed;
        }
    }

    // ------------------------------------------------------------------ ping

    pub(crate) fn start_ping(&mut self, fx: &mut Fx, spec: PingSpec, trace: Option<TraceId>) -> u32 {
        let id = self.spawn(Kind::Lookup(Lookup::placeholder()), trace);
        self.ping_begin(fx, id, spec);
        id
    }

    /// Turn process `id` into a ping (keeping any text it already printed).
    fn ping_begin(&mut self, fx: &mut Fx, id: u32, spec: PingSpec) {
        let ident = self.fresh_ident();
        let header = match spec.style {
            Style::Ios => {
                let mut text = format!(
                    "Type escape sequence to abort.\nSending {}, {}-byte ICMP Echos to {}, timeout is {} seconds:\n",
                    spec.count,
                    spec.size,
                    spec.dst,
                    spec.timeout_micros / 1_000_000
                );
                if let Some(src) = spec.src {
                    text.push_str(&format!("Packet sent with a source address of {src}\n"));
                }
                text
            }
            Style::Windows => match &spec.label {
                Some(name) => format!("\nPinging {name} [{}] with {} bytes of data:\n", spec.dst, spec.size),
                None => format!("\nPinging {} with {} bytes of data:\n", spec.dst, spec.size),
            },
        };
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        proc.out.push_str(&header);
        proc.kind = Kind::Ping(Ping {
            spec,
            ident,
            seq: 0,
            sent: 0,
            received: 0,
            awaiting: false,
            sent_at: 0,
            last_send: Sent::Ok,
            rtt_min: u64::MAX,
            rtt_max: 0,
            rtt_sum: 0,
        });
        self.ping_next(fx, id);
    }

    /// Send the next echo request, or finish.
    fn ping_next(&mut self, fx: &mut Fx, id: u32) {
        let default_ttl = self.default_ttl();
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Ping(ping) = &mut proc.kind else { return };
        if ping.sent >= ping.spec.count {
            proc.out.push_str(&ping_summary(ping));
            proc.done = true;
            if proc.outcome == Outcome::Pending {
                proc.outcome = Outcome::Failed;
            }
            return;
        }
        ping.seq = ping.seq.wrapping_add(1);
        ping.sent += 1;
        ping.awaiting = true;
        ping.sent_at = fx.now;
        proc.token = proc.token.wrapping_add(1);
        let (token, trace, timeout) = (proc.token, proc.trace, ping.spec.timeout_micros);
        // IOS sizes are whole datagrams; Windows sizes are payload only.
        let size = match ping.spec.style {
            Style::Ios => ping.spec.size.max(36),
            Style::Windows => ping.spec.size.saturating_add(28),
        };
        let packet = Packet {
            src: ping.spec.src.unwrap_or(Ipv4Addr::UNSPECIFIED),
            dst: ping.spec.dst,
            ttl: default_ttl,
            size,
            l4: L4::Icmp(Icmp::EchoRequest { id: ping.ident, seq: ping.seq }),
        };
        let (_, sent) = self.ip_send(fx, packet, trace, None);
        if let Some(Proc { kind: Kind::Ping(ping), .. }) = self.procs.list.get_mut(&id) {
            ping.last_send = sent;
        }
        fx.arm(timeout, Timer::Proc { id, token });
    }

    /// Record the result of the probe in flight and schedule what comes next.
    fn ping_result(&mut self, fx: &mut Fx, id: u32, text: &str, rtt: Option<u64>) {
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Ping(ping) = &mut proc.kind else { return };
        ping.awaiting = false;
        proc.out.push_str(text);
        if let Some(rtt) = rtt {
            ping.received += 1;
            ping.rtt_min = ping.rtt_min.min(rtt);
            ping.rtt_max = ping.rtt_max.max(rtt);
            ping.rtt_sum += rtt;
            if !matches!(proc.outcome, Outcome::Ok { .. }) {
                proc.outcome = Outcome::Ok { rtt };
            }
            if let Some(trace) = proc.trace {
                fx.trace_ok.push(trace);
            }
        }
        // Windows paces its probes one second apart; IOS sends the next at once.
        let gap = ping.spec.style == Style::Windows && rtt.is_some() && ping.sent < ping.spec.count;
        if gap {
            proc.token = proc.token.wrapping_add(1);
            fx.arm(WINDOWS_PING_GAP_MICROS, Timer::Proc { id, token: proc.token });
        } else {
            self.ping_next(fx, id);
        }
    }

    pub(crate) fn proc_echo_reply(&mut self, fx: &mut Fx, packet: &Packet, ident: u16, seq: u16) {
        if self.proc_traceroute_echo(fx, packet.src, ident, seq) {
            return;
        }
        let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
            Kind::Ping(p) if !proc.done && p.awaiting && p.ident == ident && p.seq == seq => {
                Some((*id, p.spec.style, p.spec.size, fx.now.saturating_sub(p.sent_at)))
            }
            _ => None,
        });
        let Some((id, style, size, rtt)) = hit else { return };
        let text = match style {
            Style::Ios => "!".to_string(),
            Style::Windows => {
                let time = if rtt < 1000 { "time<1ms".to_string() } else { format!("time={}ms", rtt / 1000) };
                format!("Reply from {}: bytes={size} {time} TTL={}\n", packet.src, packet.ttl)
            }
        };
        self.ping_result(fx, id, &text, Some(rtt));
    }

    pub(crate) fn proc_icmp_error(&mut self, fx: &mut Fx, from: Ipv4Addr, icmp: Icmp) {
        let (orig, expired, code) = match icmp {
            Icmp::Unreachable { orig, code } => (orig, false, Some(code)),
            Icmp::TimeExceeded { orig } => (orig, true, None),
            _ => return,
        };
        match orig.flow {
            Flow::Icmp { id: ident, seq } => {
                let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
                    Kind::Ping(p) if !proc.done && p.awaiting && p.ident == ident && p.seq == seq => {
                        Some((*id, p.spec.style))
                    }
                    Kind::Traceroute(t) if !proc.done && t.awaiting && t.ident == ident && u16::from(t.probe_seq()) == seq => {
                        Some((*id, t.style))
                    }
                    _ => None,
                });
                let Some((id, style)) = hit else { return };
                if matches!(self.procs.list.get(&id).map(|p| &p.kind), Some(Kind::Traceroute(_))) {
                    self.traceroute_answer(fx, id, from, expired, code);
                    return;
                }
                let text = match (style, expired) {
                    (Style::Ios, _) => "U".to_string(),
                    (Style::Windows, true) => format!("Reply from {from}: TTL expired in transit.\n"),
                    (Style::Windows, false) => format!("Reply from {from}: Destination host unreachable.\n"),
                };
                self.ping_result(fx, id, &text, None);
            }
            Flow::Udp { sport, dport } => {
                let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
                    Kind::Traceroute(t) if !proc.done && t.awaiting && t.sport == sport && t.dport() == dport => Some(*id),
                    _ => None,
                });
                if let Some(id) = hit {
                    self.traceroute_answer(fx, id, from, expired, code);
                }
            }
            Flow::Tcp { sport, dport } => {
                let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
                    Kind::Connect(c) if !proc.done && c.sport == sport && c.port == dport => Some(*id),
                    _ => None,
                });
                if let Some(id) = hit {
                    self.connect_finish(id, None, "% Destination unreachable; gateway or host down");
                }
            }
            Flow::Other => {}
        }
    }

    // -------------------------------------------------------------- connect

    /// Try to open a TCP connection (telnet, ssh, or a reachability probe).
    pub(crate) fn start_connect(&mut self, fx: &mut Fx, dst: Ipv4Addr, port: u16, style: Style, trace: Option<TraceId>) -> u32 {
        let sport = 49152 + self.fresh_ident() % 16000;
        let id = self.spawn(Kind::Connect(Connect { dst, port, sport, style, attempts: 0, started: fx.now }), trace);
        if let Some(proc) = self.procs.list.get_mut(&id) {
            proc.out.push_str(&match style {
                Style::Ios => format!("Trying {dst} ... "),
                Style::Windows => format!("Connecting To {dst}..."),
            });
        }
        self.connect_send(fx, id);
        id
    }

    fn connect_send(&mut self, fx: &mut Fx, id: u32) {
        let ttl = self.default_ttl();
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Connect(connect) = &mut proc.kind else { return };
        if connect.attempts >= SYN_ATTEMPTS {
            let style = connect.style;
            self.connect_finish(
                id,
                None,
                match style {
                    Style::Ios => "\n% Connection timed out; remote host not responding",
                    Style::Windows => "Could not open connection to the host: Connect failed",
                },
            );
            return;
        }
        connect.attempts += 1;
        proc.token = proc.token.wrapping_add(1);
        let (token, trace) = (proc.token, proc.trace);
        let packet = Packet {
            src: Ipv4Addr::UNSPECIFIED,
            dst: connect.dst,
            ttl,
            size: 44,
            l4: L4::Tcp { sport: connect.sport, dport: connect.port, flags: TcpFlags::Syn },
        };
        self.ip_send(fx, packet, trace, None);
        fx.arm(SYN_RETRY_MICROS, Timer::Proc { id, token });
    }

    fn connect_finish(&mut self, id: u32, rtt: Option<u64>, text: &str) {
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Connect(connect) = &proc.kind else { return };
        proc.address = Some(connect.dst);
        proc.out.push_str(text);
        proc.out.push('\n');
        proc.done = true;
        proc.token = proc.token.wrapping_add(1);
        proc.outcome = match rtt {
            Some(rtt) => Outcome::Ok { rtt },
            None => Outcome::Failed,
        };
    }

    pub(crate) fn proc_tcp_reply(&mut self, fx: &mut Fx, from: Ipv4Addr, sport: u16, dport: u16, open: bool) {
        let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
            Kind::Connect(c) if !proc.done && c.dst == from && c.port == sport && c.sport == dport => {
                Some((*id, c.style, fx.now.saturating_sub(c.started), proc.trace))
            }
            _ => None,
        });
        let Some((id, style, rtt, trace)) = hit else { return };
        if open {
            if let Some(trace) = trace {
                fx.trace_ok.push(trace);
            }
            self.connect_finish(id, Some(rtt), if style == Style::Ios { "Open" } else { "" });
        } else {
            self.connect_finish(
                id,
                None,
                match style {
                    Style::Ios => "\n% Connection refused by remote host",
                    Style::Windows => "Could not open connection to the host: Connect failed",
                },
            );
        }
    }

    // ----------------------------------------------------------- traceroute

    pub(crate) fn start_traceroute(&mut self, fx: &mut Fx, dst: Ipv4Addr, style: Style) -> u32 {
        let ident = self.fresh_ident();
        let sport = 49152 + self.fresh_ident() % 16000;
        let id = self.spawn(
            Kind::Traceroute(Traceroute {
                dst,
                style,
                ident,
                ttl: 1,
                probe: 0,
                sport,
                sent_at: 0,
                awaiting: false,
                hop_addr: None,
                reached: false,
                unreachable: false,
                max_ttl: 30,
            }),
            None,
        );
        if let Some(proc) = self.procs.list.get_mut(&id) {
            proc.out.push_str(&match style {
                Style::Ios => format!(
                    "Type escape sequence to abort.\nTracing the route to {dst}\nVRF info: (vrf in name/id, vrf out name/id)\n"
                ),
                Style::Windows => format!("\nTracing route to {dst} over a maximum of 30 hops\n\n"),
            });
        }
        self.traceroute_send(fx, id);
        id
    }

    fn traceroute_send(&mut self, fx: &mut Fx, id: u32) {
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Traceroute(t) = &mut proc.kind else { return };
        if t.probe == 0 {
            proc.out.push_str(&match t.style {
                Style::Ios => format!("{:>3} ", t.ttl),
                Style::Windows => format!("{:>3}  ", t.ttl),
            });
        }
        t.awaiting = true;
        t.sent_at = fx.now;
        proc.token = proc.token.wrapping_add(1);
        let token = proc.token;
        // Network devices probe with UDP to unlikely ports, hosts with ICMP echo.
        let l4 = match t.style {
            Style::Ios => L4::Udp { sport: t.sport, dport: t.dport(), app: UdpApp::Data },
            Style::Windows => L4::Icmp(Icmp::EchoRequest { id: t.ident, seq: u16::from(t.probe_seq()) }),
        };
        let packet = Packet { src: Ipv4Addr::UNSPECIFIED, dst: t.dst, ttl: t.ttl, size: 60, l4 };
        self.ip_send(fx, packet, None, None);
        fx.arm(TRACEROUTE_TIMEOUT_MICROS, Timer::Proc { id, token });
    }

    /// A probe was answered (`from` is `None` on timeout).
    fn traceroute_step(&mut self, fx: &mut Fx, id: u32, from: Option<Ipv4Addr>, reached: bool, unreachable: bool) {
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Traceroute(t) = &mut proc.kind else { return };
        t.awaiting = false;
        let rtt = fx.now.saturating_sub(t.sent_at);
        match (t.style, from) {
            (Style::Ios, Some(addr)) => {
                if t.hop_addr != Some(addr) {
                    proc.out.push_str(&format!("{addr} "));
                }
                proc.out.push_str(&format!("{} msec ", rtt / 1000));
            }
            (Style::Ios, None) => proc.out.push_str("*  "),
            (Style::Windows, Some(_)) => {
                proc.out.push_str(&if rtt < 1000 { "  <1 ms  ".to_string() } else { format!("{:>4} ms  ", rtt / 1000) });
            }
            (Style::Windows, None) => proc.out.push_str("   *     "),
        }
        if let Some(addr) = from {
            t.hop_addr = Some(addr);
            t.reached |= reached;
            t.unreachable |= unreachable;
        }
        t.probe += 1;
        if t.probe < 3 {
            self.traceroute_send(fx, id);
            return;
        }
        // End of the hop line.
        match (t.style, t.hop_addr) {
            (Style::Ios, _) => {
                let trimmed = proc.out.trim_end().len();
                proc.out.truncate(trimmed);
            }
            (Style::Windows, Some(addr)) => proc.out.push_str(&addr.to_string()),
            (Style::Windows, None) => proc.out.push_str("Request timed out."),
        }
        if t.unreachable && t.style == Style::Ios {
            proc.out.push_str(" !H");
        }
        proc.out.push('\n');
        let finished = t.reached || t.unreachable || t.ttl >= t.max_ttl;
        if finished {
            if t.style == Style::Windows {
                proc.out.push_str("\nTrace complete.\n");
            }
            proc.outcome = if t.reached { Outcome::Ok { rtt } } else { Outcome::Failed };
            proc.done = true;
            proc.token = proc.token.wrapping_add(1);
            return;
        }
        t.ttl += 1;
        t.probe = 0;
        t.hop_addr = None;
        self.traceroute_send(fx, id);
    }

    fn traceroute_answer(&mut self, fx: &mut Fx, id: u32, from: Ipv4Addr, expired: bool, code: Option<Unreach>) {
        // Port unreachable from the target means the probe arrived; any other
        // unreachable means the path ends here.
        let reached = !expired && code == Some(Unreach::Port);
        let unreachable = !expired && code != Some(Unreach::Port);
        self.traceroute_step(fx, id, Some(from), reached, unreachable);
    }

    /// Echo replies end a host-style traceroute.
    fn proc_traceroute_echo(&mut self, fx: &mut Fx, from: Ipv4Addr, ident: u16, seq: u16) -> bool {
        let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
            Kind::Traceroute(t) if !proc.done && t.awaiting && t.ident == ident && u16::from(t.probe_seq()) == seq => Some(*id),
            _ => None,
        });
        match hit {
            Some(id) => {
                self.traceroute_step(fx, id, Some(from), true, false);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------------ DNS

    /// Resolve `name` through `server`; optionally continue with a ping.
    pub(crate) fn start_lookup(
        &mut self,
        fx: &mut Fx,
        name: &str,
        server: Ipv4Addr,
        then_ping: Option<PingSpec>,
        silent: bool,
    ) -> u32 {
        let ident = self.fresh_ident();
        let id = self.spawn(
            Kind::Lookup(Lookup { name: name.to_string(), server, ident, attempts: 0, then_ping, silent }),
            None,
        );
        self.lookup_send(fx, id);
        id
    }

    fn lookup_send(&mut self, fx: &mut Fx, id: u32) {
        let ttl = self.default_ttl();
        let Some(proc) = self.procs.list.get_mut(&id) else { return };
        let Kind::Lookup(lookup) = &mut proc.kind else { return };
        if lookup.attempts >= 3 {
            let text = if lookup.then_ping.is_some() {
                format!("Ping request could not find host {}. Please check the name and try again.\n", lookup.name)
            } else {
                format!("DNS request timed out.\n    timeout was 2 seconds.\n*** Request to {} timed-out\n", lookup.server)
            };
            proc.out.push_str(&text);
            proc.done = true;
            proc.outcome = Outcome::Failed;
            return;
        }
        lookup.attempts += 1;
        proc.token = proc.token.wrapping_add(1);
        let token = proc.token;
        let packet = Packet {
            src: Ipv4Addr::UNSPECIFIED,
            dst: lookup.server,
            ttl,
            size: 60,
            l4: L4::Udp {
                sport: 49152 + lookup.ident % 16000,
                dport: PORT_DNS,
                app: UdpApp::Dns(DnsMsg { id: lookup.ident, name: lookup.name.clone(), answer: None }),
            },
        };
        self.ip_send(fx, packet, None, None);
        fx.arm(DNS_TIMEOUT_MICROS, Timer::Proc { id, token });
    }

    /// DNS traffic: answer queries (servers) and complete lookups (clients).
    pub(crate) fn dns_rx(
        &mut self,
        fx: &mut Fx,
        packet: &Packet,
        msg: &DnsMsg,
        meta: &Meta,
        l2: (Mac, Mac),
        iface: crate::ifname::IfId,
    ) {
        let L4::Udp { sport, dport, .. } = packet.l4 else { return };
        match msg.answer {
            None => {
                if !self.has_service("dns") {
                    self.icmp_error(fx, Some(iface), packet, meta, crate::sim::ip::IcmpError::Unreachable(Unreach::Port));
                    return;
                }
                let answer = self.dns_zone.iter().find(|(n, _)| n.eq_ignore_ascii_case(&msg.name)).map(|(_, ip)| *ip);
                let noun = self.noun();
                let via = self.ip_note(fx, crate::api::PacketEventKind::Delivered, packet, meta, l2, Some(iface), None, || {
                    match answer {
                        Some(ip) => format!("The DNS service on this {noun} knows {}: it answers {ip}.", msg.name),
                        None => format!("The DNS service on this {noun} has no record for {}.", msg.name),
                    }
                });
                let reply = Packet {
                    src: packet.dst,
                    dst: packet.src,
                    ttl: self.default_ttl(),
                    size: 76,
                    l4: L4::Udp {
                        sport: dport,
                        dport: sport,
                        app: UdpApp::Dns(DnsMsg { id: msg.id, name: msg.name.clone(), answer: Some(answer) }),
                    },
                };
                self.ip_send(fx, reply, None, Some(&Meta { via, ..*meta }));
            }
            Some(answer) => {
                let hit = self.procs.list.iter().find_map(|(id, proc)| match &proc.kind {
                    Kind::Lookup(l) if !proc.done && l.ident == msg.id => Some(*id),
                    _ => None,
                });
                let Some(id) = hit else { return };
                let Some(proc) = self.procs.list.get_mut(&id) else { return };
                let Kind::Lookup(lookup) = &mut proc.kind else { return };
                proc.token = proc.token.wrapping_add(1);
                proc.address = answer;
                let then_ping = lookup.then_ping.take();
                match (answer, then_ping) {
                    (Some(ip), Some(mut spec)) => {
                        spec.dst = ip;
                        spec.label = Some(lookup.name.clone());
                        self.ping_begin(fx, id, spec);
                    }
                    (None, Some(_)) => {
                        proc.out.push_str(&format!(
                            "Ping request could not find host {}. Please check the name and try again.\n",
                            lookup.name
                        ));
                        proc.done = true;
                        proc.outcome = Outcome::Failed;
                    }
                    (answer, None) => {
                        if !lookup.silent {
                            proc.out.push_str(&match answer {
                                Some(ip) => format!(
                                    "Server:  {}\nAddress:  {}\n\nName:    {}\nAddress:  {ip}\n\n",
                                    lookup.server, lookup.server, lookup.name
                                ),
                                None => format!(
                                    "Server:  {}\nAddress:  {}\n\n*** {} can't find {}: Non-existent domain\n",
                                    lookup.server, lookup.server, lookup.server, lookup.name
                                ),
                            });
                        }
                        proc.done = true;
                        proc.outcome = if answer.is_some() { Outcome::Ok { rtt: 0 } } else { Outcome::Failed };
                    }
                }
            }
        }
    }

    // --------------------------------------------------------------- timers

    pub(crate) fn proc_timer(&mut self, fx: &mut Fx, id: u32, token: u32) {
        let Some(proc) = self.procs.list.get(&id) else { return };
        if proc.done || proc.token != token {
            return;
        }
        match &proc.kind {
            Kind::Ping(ping) if ping.awaiting => {
                let text = match ping.spec.style {
                    Style::Ios => ".".to_string(),
                    Style::Windows => self.windows_timeout_text(ping),
                };
                self.ping_result(fx, id, &text, None);
            }
            Kind::Ping(_) => self.ping_next(fx, id),
            Kind::Connect(_) => self.connect_send(fx, id),
            Kind::Traceroute(_) => self.traceroute_step(fx, id, None, false, false),
            Kind::Lookup(_) => self.lookup_send(fx, id),
        }
    }

    /// What a Windows host prints for a probe nobody answered.
    fn windows_timeout_text(&self, ping: &Ping) -> String {
        let own = self.source_for(ping.spec.dst);
        let unresolved = self.route_lookup(ping.spec.dst).is_some_and(|r| self.arp.is_pending(r.next_hop));
        match (ping.last_send, own) {
            (Sent::NoRoute, _) => "Destination host unreachable.\n".to_string(),
            // The neighbour (or gateway) never answered ARP.
            (_, Some(own)) if unresolved => format!("Reply from {own}: Destination host unreachable.\n"),
            _ => "Request timed out.\n".to_string(),
        }
    }
}

impl Traceroute {
    /// Each probe uses its own destination port / sequence number.
    fn probe_seq(&self) -> u8 {
        self.ttl.wrapping_sub(1).wrapping_mul(3).wrapping_add(self.probe)
    }

    fn dport(&self) -> u16 {
        PORT_TRACEROUTE + u16::from(self.probe_seq())
    }
}

fn ping_summary(ping: &Ping) -> String {
    // A probe still in flight when the ping was interrupted was never counted.
    let sent = if ping.awaiting { ping.sent.saturating_sub(1) } else { ping.sent };
    match ping.spec.style {
        Style::Ios => {
            let percent = if sent == 0 { 0 } else { u64::from(ping.received) * 100 / u64::from(sent) };
            let mut text = format!("\nSuccess rate is {percent} percent ({}/{sent})", ping.received);
            if ping.received > 0 {
                text.push_str(&format!(
                    ", round-trip min/avg/max = {}/{}/{} ms",
                    ios_ms(ping.rtt_min),
                    ios_ms(ping.rtt_sum / u64::from(ping.received)),
                    ios_ms(ping.rtt_max)
                ));
            }
            text.push('\n');
            text
        }
        Style::Windows => {
            let lost = sent.saturating_sub(ping.received);
            let percent = if sent == 0 { 0 } else { u64::from(lost) * 100 / u64::from(sent) };
            let mut text = format!(
                "\nPing statistics for {}:\n    Packets: Sent = {sent}, Received = {}, Lost = {lost} ({percent}% loss),\n",
                ping.spec.dst, ping.received
            );
            if ping.received > 0 {
                text.push_str(&format!(
                    "Approximate round trip times in milli-seconds:\n    Minimum = {}ms, Maximum = {}ms, Average = {}ms\n",
                    ping.rtt_min / 1000,
                    ping.rtt_max / 1000,
                    ping.rtt_sum / u64::from(ping.received) / 1000
                ));
            }
            text
        }
    }
}
