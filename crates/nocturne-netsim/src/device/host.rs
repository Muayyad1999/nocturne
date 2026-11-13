//! The end-host command prompt (`C:\>`): a small Windows-flavoured shell for
//! PCs, laptops, servers and the management station.

use std::fmt::Write as _;
use std::net::Ipv4Addr;

use crate::api::HostConfig;
use crate::device::cli::{Next, Reply};
use crate::device::config::L3Addr;
use crate::device::hostcfg;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::sim::procs::{PingSpec, Style};
use crate::util::net::{len_from_mask, parse_ip};

#[derive(Clone, Debug, Default)]
pub(crate) struct HostShell;

const HELP: &str = "Available commands:
  ipconfig [/all | /release | /renew]   Show or refresh the IP configuration
  ip <address> <mask> [gateway]         Set a static IP configuration
  dhcp                                  Obtain the IP configuration from DHCP
  ping <address|name> [-n count] [-t]   Send ICMP echo requests
  tracert <address>                     Trace the route to a destination
  arp -a                                Show the ARP cache
  nslookup <name>                       Resolve a name through the DNS server
  telnet <address>                      Open a Telnet session
  ssh -l <user> <address>               Open an SSH session
  netstat                               Show listening services
  hostname                              Show the name of this host
  cls                                   Clear the screen
  help                                  Show this list
  exit                                  Close the session
";

impl HostShell {
    pub(crate) fn new() -> HostShell {
        HostShell
    }

    pub(crate) fn prompt(&self) -> String {
        "C:\\>".to_string()
    }

    pub(crate) fn execute(&mut self, node: &mut Node, fx: &mut Fx, line: &str) -> Reply {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(command) = words.first().map(|w| w.to_ascii_lowercase()) else { return Reply::default() };
        let args = &words[1..];
        match command.as_str() {
            "ipconfig" => ipconfig(node, fx, args),
            "ip" => set_static(node, args),
            "dhcp" => {
                let mut host = node.host.clone().unwrap_or_default();
                host.dhcp = true;
                host.ip = None;
                host.mask = None;
                host.gateway = None;
                apply(node, host, "DHCP is now enabled; the address will be requested from the network.\n")
            }
            "ping" => ping(node, fx, args),
            "tracert" | "traceroute" => match args.first().and_then(|a| parse_ip(a)) {
                Some(dst) => Reply { next: Next::Job(node.start_traceroute(fx, dst, Style::Windows)), ..Reply::default() },
                None => Reply::text("Usage: tracert <address>\n"),
            },
            "arp" => arp(node, fx),
            "nslookup" => match (args.first(), node.cfg.name_servers.first()) {
                (Some(name), Some(server)) => {
                    Reply { next: Next::Job(node.start_lookup(fx, name, *server, None, false)), ..Reply::default() }
                }
                (Some(_), None) => Reply::text("*** No DNS server is configured on this host\n"),
                (None, _) => Reply::text("Usage: nslookup <name>\n"),
            },
            "telnet" => match args.first().and_then(|a| parse_ip(a)) {
                Some(dst) => {
                    let proc = node.start_connect(fx, dst, 23, Style::Windows, None);
                    Reply { next: Next::Connect { proc, ssh_user: None }, ..Reply::default() }
                }
                None => Reply::text("Usage: telnet <address>\n"),
            },
            "ssh" => ssh(node, fx, args),
            "netstat" => netstat(node),
            "hostname" => Reply::text(format!("{}\n", node.name)),
            "cls" | "clear" => Reply::text("\x1b[2J\x1b[H"),
            "help" | "?" => Reply::text(HELP),
            "exit" | "logout" | "quit" => Reply { next: Next::Close, ..Reply::default() },
            _ => Reply {
                out: format!(
                    "'{}' is not recognized as an internal or external command,\noperable program or batch file.\n",
                    words[0]
                ),
                error: true,
                ..Reply::default()
            },
        }
    }
}

/// Store new host settings and rebuild the configuration model from them.
fn apply(node: &mut Node, host: HostConfig, message: &str) -> Reply {
    if let Err(error) = hostcfg::validate(node.model, &host) {
        return Reply { out: format!("{error}\n"), error: true, ..Reply::default() };
    }
    node.host = Some(host);
    hostcfg::apply(node);
    Reply { out: message.to_string(), touched: true, ..Reply::default() }
}

fn set_static(node: &mut Node, args: &[&str]) -> Reply {
    let usage = || Reply { out: "Usage: ip <address> <mask> [gateway]\n".to_string(), error: true, ..Reply::default() };
    let (Some(ip), Some(mask)) = (args.first().and_then(|a| parse_ip(a)), args.get(1).and_then(|a| parse_ip(a))) else {
        return usage();
    };
    if len_from_mask(mask).is_none() {
        return Reply { out: "Invalid subnet mask.\n".to_string(), error: true, ..Reply::default() };
    }
    let gateway = match args.get(2) {
        Some(text) => match parse_ip(text) {
            Some(gateway) => Some(gateway),
            None => return usage(),
        },
        None => None,
    };
    let mut host = node.host.clone().unwrap_or_default();
    host.dhcp = false;
    host.ip = Some(ip.to_string());
    host.mask = Some(mask.to_string());
    host.gateway = gateway.map(|g| g.to_string());
    apply(node, host, "Static IP configuration applied.\n")
}

fn ipconfig(node: &mut Node, fx: &mut Fx, args: &[&str]) -> Reply {
    let option = args.first().map(|a| a.to_ascii_lowercase());
    match option.as_deref() {
        Some("/release") => {
            node.dhcp_release_all(fx);
            return Reply { out: "\nWindows IP Configuration\n\nThe DHCP lease was released.\n".to_string(), touched: true, ..Reply::default() };
        }
        Some("/renew") => {
            node.dhcp_release_all(fx);
            return Reply {
                out: "\nWindows IP Configuration\n\nRequesting a new lease from DHCP; run ipconfig to see the result.\n".to_string(),
                touched: true,
                ..Reply::default()
            };
        }
        Some("/all") | None => {}
        Some(other) => return Reply { out: format!("Error: unrecognized or incomplete command line: {other}\n"), error: true, ..Reply::default() },
    }
    let all = option.is_some();
    let mut out = String::from("\nWindows IP Configuration\n\n");
    if all {
        let _ = writeln!(out, "   Host Name . . . . . . . . . . . . : {}", node.name);
        let _ = writeln!(out, "   IP Routing Enabled. . . . . . . . : {}\n", if node.routes_packets() { "Yes" } else { "No" });
    }
    let gateway = node.cfg.default_gateway.or(node.rib.dhcp_gateway);
    for id in node.model.ports.clone() {
        let Some(cfg) = node.cfg.iface(id) else { continue };
        let _ = writeln!(out, "\nEthernet adapter {}:\n", id.name());
        if !node.is_up(id) {
            out.push_str("   Media State . . . . . . . . . . . : Media disconnected\n");
        }
        let dhcp = cfg.addr == Some(L3Addr::Dhcp);
        if all {
            let _ = writeln!(out, "   Description . . . . . . . . . . . : Nocturne Virtual Ethernet Adapter");
            let _ = writeln!(out, "   Physical Address. . . . . . . . . : {}", node.mac_for(id).dashed());
            let _ = writeln!(out, "   DHCP Enabled. . . . . . . . . . . : {}", if dhcp { "Yes" } else { "No" });
        }
        out.push_str("   Connection-specific DNS Suffix  . :\n");
        match node.addr(id) {
            Some(addr) => {
                let _ = writeln!(out, "   IPv4 Address. . . . . . . . . . . : {}", addr.ip);
                let _ = writeln!(out, "   Subnet Mask . . . . . . . . . . . : {}", addr.mask());
            }
            None if dhcp => out.push_str("   IPv4 Address. . . . . . . . . . . : (waiting for DHCP)\n"),
            None => out.push_str("   IPv4 Address. . . . . . . . . . . : (not configured)\n"),
        }
        let _ = writeln!(
            out,
            "   Default Gateway . . . . . . . . . : {}",
            gateway.map(|g| g.to_string()).unwrap_or_default()
        );
        if all {
            if let Some(server) = node.dhcp_server_of(id) {
                let _ = writeln!(out, "   DHCP Server . . . . . . . . . . . : {server}");
            }
            let dns: Vec<String> = node.cfg.name_servers.iter().chain(node.rib.dhcp_dns.iter()).map(|d| d.to_string()).collect();
            let _ = writeln!(out, "   DNS Servers . . . . . . . . . . . : {}", dns.join("\n                                       "));
        }
    }
    Reply::text(out)
}

fn ping(node: &mut Node, fx: &mut Fx, args: &[&str]) -> Reply {
    let mut count: u32 = 4;
    let mut target = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_ascii_lowercase().as_str() {
            "-t" => count = u32::MAX,
            "-n" => match iter.next().and_then(|n| n.parse::<u32>().ok()).filter(|n| *n > 0) {
                Some(n) => count = n,
                None => return Reply { out: "Bad value for option -n, valid range is from 1 to 4294967295.\n".to_string(), error: true, ..Reply::default() },
            },
            _ if target.is_none() => target = Some(*arg),
            _ => return Reply { out: format!("Bad parameter {arg}.\n"), error: true, ..Reply::default() },
        }
    }
    let Some(target) = target else {
        return Reply::text("\nUsage: ping [-t] [-n count] target_name\n");
    };
    let spec = |dst: Ipv4Addr| PingSpec { dst, src: None, count, timeout_micros: 2_000_000, size: 32, style: Style::Windows, label: None };
    match (parse_ip(target), node.cfg.name_servers.first().or(node.rib.dhcp_dns.first()).copied()) {
        (Some(dst), _) => Reply { next: Next::Job(node.start_ping(fx, spec(dst), None)), ..Reply::default() },
        (None, Some(server)) => {
            let then = spec(Ipv4Addr::UNSPECIFIED);
            Reply { next: Next::Job(node.start_lookup(fx, target, server, Some(then), true)), ..Reply::default() }
        }
        (None, None) => Reply {
            out: format!("Ping request could not find host {target}. Please check the name and try again.\n"),
            error: true,
            ..Reply::default()
        },
    }
}

fn arp(node: &Node, fx: &Fx) -> Reply {
    let rows = node.arp.rows(fx.now);
    if rows.is_empty() {
        return Reply::text("No ARP Entries Found.\n");
    }
    let mut out = String::new();
    for (iface, addr) in &node.rib.addrs {
        let entries: Vec<_> = rows.iter().filter(|(_, e)| e.iface == *iface).collect();
        if entries.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\nInterface: {} --- 0x{:x}", addr.ip, iface.number() + 2);
        out.push_str("  Internet Address      Physical Address      Type\n");
        for (ip, entry) in entries {
            let _ = writeln!(out, "  {:<22}{:<22}dynamic", ip.to_string(), entry.mac.dashed().to_ascii_lowercase());
        }
    }
    Reply::text(out)
}

fn ssh(node: &mut Node, fx: &mut Fx, args: &[&str]) -> Reply {
    let (user, target) = match args {
        ["-l", user, target] => (Some(*user), parse_ip(target)),
        [target] => match target.split_once('@') {
            Some((user, host)) => (Some(user), parse_ip(host)),
            None => (None, parse_ip(target)),
        },
        _ => (None, None),
    };
    match (user, target) {
        (Some(user), Some(dst)) => {
            let proc = node.start_connect(fx, dst, 22, Style::Windows, None);
            Reply { next: Next::Connect { proc, ssh_user: Some(user.to_string()) }, ..Reply::default() }
        }
        _ => Reply::text("Usage: ssh -l <user> <address>\n"),
    }
}

fn netstat(node: &Node) -> Reply {
    let mut out = String::from("\nActive Connections\n\n  Proto  Local Address          Foreign Address        State\n");
    let services = node.host.as_ref().map(|h| h.services.clone()).unwrap_or_default();
    for service in services {
        let (proto, port) = match service.to_ascii_lowercase().as_str() {
            "http" => ("TCP", 80),
            "dns" => ("UDP", 53),
            "dhcp" => ("UDP", 67),
            "tftp" => ("UDP", 69),
            "syslog" => ("UDP", 514),
            "ntp" => ("UDP", 123),
            "ssh" => ("TCP", 22),
            "telnet" => ("TCP", 23),
            _ => continue,
        };
        let _ = writeln!(out, "  {proto:<7}{:<23}{:<23}LISTENING", format!("0.0.0.0:{port}"), "0.0.0.0:0");
    }
    Reply::text(out)
}
