//! What this workstation knows about its own network: interfaces, the neighbour (ARP / NDP)
//! table, default gateways — and Wake-on-LAN, which is sent from here.

mod neigh;
mod route;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use if_addrs::{IfAddr, IfOperStatus};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

use crate::ProbeError;
use crate::process;
use crate::util::{self, format_mac, parse_mac};

/// How long the operating system's own tools (`ip`, `arp`, `netstat`, …) get to answer.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
/// The discard port, the customary destination of magic packets.
const WAKE_PORT: u16 = 9;

/// A network interface of this machine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LocalInterface {
    pub name: String,
    /// Addresses in CIDR notation (`192.168.1.10/24`, `fe80::1/64`).
    pub addresses: Vec<String>,
    pub mac: Option<String>,
    pub is_loopback: bool,
    pub is_up: bool,
}

/// An entry of the neighbour table.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Neighbour {
    pub address: String,
    /// `aa:bb:cc:dd:ee:ff`
    pub mac: String,
    /// The interface name; on Windows the interface's IPv4 address for ARP entries.
    pub interface: String,
    /// The neighbour state where the system tracks one (`reachable`, `stale`, `delay`, `probe`,
    /// `permanent` — Linux and NDP tables), otherwise how the entry was made (`dynamic`,
    /// `static`, `permanent` — ARP tables of macOS, the BSDs and Windows).
    pub state: String,
}

/// A default route.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LocalRoute {
    /// `0.0.0.0/0` or `::/0`.
    pub destination: String,
    pub gateway: String,
    pub interface: String,
}

/// The interfaces of this machine with their addresses.
///
/// On Linux every interface is listed, with or without addresses. Elsewhere only interfaces
/// that have an address are listed, and their MAC addresses come from `ifconfig` (macOS, BSD)
/// or `getmac` (Windows), which this function runs and waits for.
pub fn interfaces() -> Vec<LocalInterface> {
    let mut interfaces: Vec<LocalInterface> = Vec::new();
    for found in if_addrs::get_if_addrs().unwrap_or_default() {
        let address = match &found.addr {
            IfAddr::V4(v4) => format!("{}/{}", v4.ip, v4.prefixlen),
            IfAddr::V6(v6) => format!("{}/{}", v6.ip, v6.prefixlen),
        };
        match interfaces.iter_mut().find(|known| known.name == found.name) {
            Some(known) => known.addresses.push(address),
            None => interfaces.push(LocalInterface {
                addresses: vec![address],
                mac: None,
                is_loopback: found.is_loopback(),
                // Loopback and tunnel interfaces report "unknown" while working.
                is_up: matches!(found.oper_status, IfOperStatus::Up | IfOperStatus::Unknown),
                name: found.name,
            }),
        }
    }
    complete(&mut interfaces);
    interfaces
}

/// The workstation's ARP / NDP table: the hosts on its own links it has recently talked to.
///
/// Unresolved entries and multicast / broadcast mappings are left out. Empty when the table
/// cannot be read.
pub async fn neighbours() -> Vec<Neighbour> {
    let mut found = read_neighbours().await;
    let mut seen = std::collections::HashSet::new();
    found.retain(|neighbour| seen.insert((neighbour.address.clone(), neighbour.interface.clone())));
    found
}

/// The default routes of this machine (IPv4 and IPv6) that have a next hop.
pub async fn default_gateways() -> Vec<LocalRoute> {
    if cfg!(target_os = "linux") {
        let v4 = process::run("ip", &["-4", "route", "show", "default"], TOOL_TIMEOUT).await;
        let mut routes = match v4 {
            Ok(output) if output.success => route::parse_ip_route(&output.stdout),
            // No iproute2: the kernel's own table has the IPv4 routes.
            _ => match tokio::fs::read_to_string("/proc/net/route").await {
                Ok(text) => route::parse_proc_route(&text),
                Err(_) => Vec::new(),
            },
        };
        if let Ok(output) =
            process::run("ip", &["-6", "route", "show", "default"], TOOL_TIMEOUT).await
        {
            routes.extend(route::parse_ip_route(&output.stdout));
        }
        routes
    } else if cfg!(windows) {
        match process::run("route", &["print"], TOOL_TIMEOUT).await {
            Ok(output) => route::parse_route_print(&output.stdout),
            Err(_) => Vec::new(),
        }
    } else {
        match process::run("netstat", &["-rn"], TOOL_TIMEOUT).await {
            Ok(output) => route::parse_netstat_rn(&output.stdout),
            Err(_) => Vec::new(),
        }
    }
}

/// Reads the neighbour table with this platform's tool.
async fn read_neighbours() -> Vec<Neighbour> {
    if cfg!(target_os = "linux") {
        return match process::run("ip", &["neigh", "show"], TOOL_TIMEOUT).await {
            Ok(output) if output.success => neigh::parse_ip_neigh(&output.stdout),
            // No iproute2: the kernel's own table has the IPv4 entries.
            _ => match tokio::fs::read_to_string("/proc/net/arp").await {
                Ok(text) => neigh::parse_proc_arp(&text),
                Err(_) => Vec::new(),
            },
        };
    }
    let mut found = Vec::new();
    if cfg!(windows) {
        if let Ok(output) = process::run("arp", &["-a"], TOOL_TIMEOUT).await {
            found.extend(neigh::parse_arp_windows(&output.stdout));
        }
        let netsh = ["interface", "ipv6", "show", "neighbors"];
        if let Ok(output) = process::run("netsh", &netsh, TOOL_TIMEOUT).await {
            found.extend(neigh::parse_netsh_neighbors(&output.stdout));
        }
    } else {
        if let Ok(output) = process::run("arp", &["-an"], TOOL_TIMEOUT).await {
            found.extend(neigh::parse_arp_an(&output.stdout));
        }
        if let Ok(output) = process::run("ndp", &["-an"], TOOL_TIMEOUT).await {
            found.extend(neigh::parse_ndp_an(&output.stdout));
        }
    }
    found
}

/// Adds what `if-addrs` does not report: MAC addresses everywhere, and on Linux the kernel's
/// view of "up" and the interfaces that have no address.
fn complete(interfaces: &mut Vec<LocalInterface>) {
    if cfg!(target_os = "linux") {
        complete_from_sysfs(interfaces);
        return;
    }
    let macs = if cfg!(windows) {
        run_and_wait("getmac", &["/fo", "csv", "/nh", "/v"]).map(|text| parse_getmac_csv(&text))
    } else {
        run_and_wait("ifconfig", &["-a"]).map(|text| parse_ifconfig_macs(&text))
    };
    for interface in interfaces {
        interface.mac = macs
            .iter()
            .flatten()
            .find(|(name, _)| *name == interface.name)
            .map(|(_, mac)| mac.clone());
    }
}

/// Runs a system tool to completion and returns what it printed.
fn run_and_wait(program: &str, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(process::find(program)?);
    command.args(args).stdin(std::process::Stdio::null());
    // CREATE_NO_WINDOW: a GUI application must not flash a console.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
    let output = command.output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Linux: `/sys/class/net` lists every interface with its flags, state and MAC address.
fn complete_from_sysfs(interfaces: &mut Vec<LocalInterface>) {
    const IFF_UP: u32 = 0x1;
    const IFF_LOOPBACK: u32 = 0x8;
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let read = |file: &str| {
            std::fs::read_to_string(format!("/sys/class/net/{name}/{file}"))
                .ok()
                .map(|text| text.trim().to_owned())
        };
        let Some(flags) = read("flags")
            .and_then(|flags| u32::from_str_radix(flags.trim_start_matches("0x"), 16).ok())
        else {
            // Not an interface (for example the bonding_masters file).
            continue;
        };
        let operstate = read("operstate").unwrap_or_default();
        let is_up = flags & IFF_UP != 0
            && !matches!(
                operstate.as_str(),
                "down" | "lowerlayerdown" | "notpresent" | "dormant"
            );
        let mac = read("address")
            .and_then(|address| parse_mac(&address))
            .filter(|mac| *mac != [0; 6])
            .map(format_mac);
        match interfaces.iter_mut().find(|known| known.name == name) {
            Some(known) => {
                known.mac = mac;
                known.is_up = is_up;
            }
            None => interfaces.push(LocalInterface {
                name,
                addresses: Vec::new(),
                mac,
                is_loopback: flags & IFF_LOOPBACK != 0,
                is_up,
            }),
        }
    }
}

/// Sends a Wake-on-LAN magic packet (UDP port 9) for `mac`.
///
/// With `broadcast` (an address, optionally `address:port`) the packet goes there — use the
/// directed broadcast address of the sleeping machine's subnet. Without it the packet is sent
/// to the broadcast address of every local IPv4 network and to 255.255.255.255.
///
/// Returns the destinations the packet was sent to.
pub async fn wake_on_lan(mac: &str, broadcast: Option<&str>) -> Result<Vec<String>, ProbeError> {
    let Some(mac_bytes) = parse_mac(mac) else {
        return Err(ProbeError::invalid(format!(
            "'{}' is not a MAC address; expected six hexadecimal bytes such as 00:1a:2b:3c:4d:5e",
            mac.trim()
        )));
    };
    let packet = magic_packet(mac_bytes);

    let mut destinations: Vec<SocketAddr> = Vec::new();
    match broadcast {
        Some(text) => {
            let (host, port) = util::split_host_port(text, WAKE_PORT)?;
            let Some(address) = util::parse_ip_literal(&host) else {
                return Err(ProbeError::invalid(format!(
                    "'{host}' is not an IP address; give the broadcast address of the target's \
                     subnet, for example 192.168.1.255"
                )));
            };
            destinations.push(SocketAddr::new(address, port));
        }
        None => {
            for found in if_addrs::get_if_addrs().unwrap_or_default() {
                if let IfAddr::V4(v4) = &found.addr {
                    if let Some(broadcast) = v4.broadcast.filter(|_| !v4.is_loopback()) {
                        destinations.push((broadcast, WAKE_PORT).into());
                    }
                }
            }
            destinations.push((Ipv4Addr::BROADCAST, WAKE_PORT).into());
            destinations.dedup();
        }
    }

    let mut sent = Vec::new();
    let mut last_error = None;
    for destination in destinations {
        match send_broadcast(&packet, destination).await {
            Ok(()) => {
                let text = destination.to_string();
                if !sent.contains(&text) {
                    sent.push(text);
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    match (sent.is_empty(), last_error) {
        (true, Some(error)) => Err(ProbeError::from_network_io(error)),
        _ => Ok(sent),
    }
}

/// Six `ff` bytes followed by the MAC address sixteen times.
fn magic_packet(mac: [u8; 6]) -> Vec<u8> {
    let mut packet = vec![0xff; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&mac);
    }
    packet
}

async fn send_broadcast(packet: &[u8], destination: SocketAddr) -> std::io::Result<()> {
    let local: SocketAddr = match destination {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local).await?;
    if destination.is_ipv4() {
        socket.set_broadcast(true)?;
    }
    socket.send_to(packet, destination).await?;
    Ok(())
}

/// The neighbour-table entry of `address`, if this machine has one.
pub(crate) fn neighbour_of(neighbours: &[Neighbour], address: IpAddr) -> Option<&Neighbour> {
    let text = address.to_string();
    neighbours
        .iter()
        .find(|neighbour| neighbour.address == text)
}

/// `ether aa:bb:cc:dd:ee:ff` lines of `ifconfig`, per interface.
fn parse_ifconfig_macs(text: &str) -> Vec<(String, String)> {
    let mut current: Option<&str> = None;
    let mut macs = Vec::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            current = line.split(':').next().filter(|name| !name.is_empty());
            continue;
        }
        let mut words = line.split_whitespace();
        if matches!(words.next(), Some("ether" | "lladdr" | "address:")) {
            if let (Some(name), Some(mac)) = (current, words.next().and_then(parse_mac)) {
                macs.push((name.to_owned(), format_mac(mac)));
            }
        }
    }
    macs
}

/// `getmac /fo csv /nh /v` rows: `"Ethernet","Intel(R) …","A4-83-E7-12-34-56","\Device\Tcpip_{…}"`.
fn parse_getmac_csv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix('"')?.strip_suffix('"')?;
            let fields: Vec<&str> = line.split("\",\"").collect();
            let mac = parse_mac(fields.get(2)?)?;
            Some((fields[0].to_owned(), format_mac(mac)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn magic_packet_layout() {
        let packet = magic_packet([0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        assert_eq!(packet.len(), 102);
        assert_eq!(packet[..6], [0xff; 6]);
        for repetition in 0..16 {
            let start = 6 + repetition * 6;
            assert_eq!(
                packet[start..start + 6],
                [0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]
            );
        }
    }

    #[tokio::test]
    async fn wake_on_lan_sends_the_magic_packet_to_the_given_address() {
        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sent = wake_on_lan("00-1A-2B-3C-4D-5E", Some(&format!("127.0.0.1:{port}")))
            .await
            .unwrap();
        assert_eq!(sent, vec![format!("127.0.0.1:{port}")]);
        let mut buffer = [0u8; 200];
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(2), listener.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            buffer[..length],
            magic_packet([0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e])[..]
        );
    }

    #[tokio::test]
    async fn wake_on_lan_rejects_bad_input_with_a_helpful_message() {
        let error = wake_on_lan("00:1a:2b:3c:4d", None).await.unwrap_err();
        assert!(error.to_string().contains("00:1a:2b:3c:4d:5e"), "{error}");
        assert!(matches!(error, ProbeError::Invalid(_)));
        let error = wake_on_lan("00:1a:2b:3c:4d:5e", Some("office-lan"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("192.168.1.255"), "{error}");
        let error = wake_on_lan("00:1a:2b:3c:4d:5e", Some("10.0.0.255:99999"))
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Invalid(_)), "{error:?}");
    }

    #[tokio::test]
    async fn wake_on_lan_without_a_destination_uses_local_broadcast_addresses() {
        let result = wake_on_lan("00:1a:2b:3c:4d:5e", None).await;
        match result {
            Ok(sent) => {
                assert!(!sent.is_empty());
                assert!(
                    sent.iter().all(|destination| destination.ends_with(":9")),
                    "{sent:?}"
                );
            }
            // A machine with nothing but loopback has nowhere to broadcast.
            Err(ProbeError::Unreachable(_) | ProbeError::Permission(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn interfaces_include_loopback_with_cidr_addresses() {
        let interfaces = interfaces();
        let loopback = interfaces
            .iter()
            .find(|interface| interface.is_loopback)
            .expect("every machine has a loopback interface");
        assert!(loopback.is_up, "{loopback:?}");
        assert!(
            loopback
                .addresses
                .iter()
                .any(|a| a == "127.0.0.1/8" || a == "::1/128"),
            "{loopback:?}"
        );
        assert_eq!(loopback.mac, None);
        let names: std::collections::HashSet<&str> = interfaces
            .iter()
            .map(|interface| interface.name.as_str())
            .collect();
        assert_eq!(names.len(), interfaces.len(), "one entry per interface");
        for interface in &interfaces {
            for address in &interface.addresses {
                let (ip, prefix) = address.split_once('/').expect("CIDR notation");
                assert!(ip.parse::<IpAddr>().is_ok(), "{address}");
                assert!(prefix.parse::<u8>().is_ok(), "{address}");
            }
            if let Some(mac) = &interface.mac {
                assert!(parse_mac(mac).is_some(), "{mac}");
            }
        }
    }

    #[tokio::test]
    async fn neighbours_and_gateways_are_well_formed() {
        for neighbour in neighbours().await {
            assert!(neighbour.address.parse::<IpAddr>().is_ok(), "{neighbour:?}");
            assert!(parse_mac(&neighbour.mac).is_some(), "{neighbour:?}");
            assert!(!neighbour.interface.is_empty(), "{neighbour:?}");
            assert!(!neighbour.state.is_empty(), "{neighbour:?}");
        }
        for gateway in default_gateways().await {
            assert!(gateway.gateway.parse::<IpAddr>().is_ok(), "{gateway:?}");
            assert!(gateway.destination == "0.0.0.0/0" || gateway.destination == "::/0");
            assert!(!gateway.interface.is_empty());
        }
    }

    #[test]
    fn ifconfig_macs() {
        let text = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\
                    \toptions=1203<RXCSUM,TXCSUM,TXSTATUS,SW_TIMESTAMP>\n\
                    \tinet 127.0.0.1 netmask 0xff000000\n\
                    en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
                    \tether a4:83:e7:12:34:56\n\
                    \tinet 192.168.1.10 netmask 0xffffff00 broadcast 192.168.1.255\n\
                    \tstatus: active\n\
                    utun0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380\n\
                    bridge100: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
                    \tether 3e:22:fb:1:2:64\n";
        assert_eq!(
            parse_ifconfig_macs(text),
            vec![
                ("en0".to_owned(), "a4:83:e7:12:34:56".to_owned()),
                ("bridge100".to_owned(), "3e:22:fb:01:02:64".to_owned()),
            ]
        );
    }

    #[test]
    fn getmac_csv() {
        let text = "\r\n\"Ethernet\",\"Intel(R) Ethernet Connection (7) I219-V\",\"A4-83-E7-12-34-56\",\"\\Device\\Tcpip_{6F5E2F0A-1B2C-4D3E-8F90-123456789ABC}\"\r\n\
                    \"Wi-Fi\",\"Intel(R) Wi-Fi 6 AX200 160MHz\",\"B4-83-E7-65-43-21\",\"Media disconnected\"\r\n\
                    \"Bluetooth Network Connection\",\"Bluetooth Device (Personal Area Network)\",\"N/A\",\"Hardware not present\"\r\n";
        assert_eq!(
            parse_getmac_csv(text),
            vec![
                ("Ethernet".to_owned(), "a4:83:e7:12:34:56".to_owned()),
                ("Wi-Fi".to_owned(), "b4:83:e7:65:43:21".to_owned()),
            ]
        );
    }

    #[test]
    fn neighbour_lookup() {
        let table = vec![Neighbour {
            address: "10.0.0.7".into(),
            mac: "00:1a:2b:3c:4d:5e".into(),
            interface: "eth0".into(),
            state: "reachable".into(),
        }];
        assert!(neighbour_of(&table, "10.0.0.7".parse().unwrap()).is_some());
        assert!(neighbour_of(&table, "10.0.0.8".parse().unwrap()).is_none());
    }
}
