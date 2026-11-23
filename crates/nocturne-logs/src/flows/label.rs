//! What a key is called: the service behind a port, the name of a marking.
//!
//! The names come from the reference tables of `nocturne-tools`, the same
//! ones the toolbox shows, so that a port or a code point has one name in
//! the whole application. A name is a convention, not an observation: flow
//! export says a conversation used TCP port 443, never that it was HTTPS.
//! That is why a label always carries the number it was derived from.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::OnceLock;

use super::table::{Direction, Key};

const TCP: u8 = 6;
const UDP: u8 = 17;
const SCTP: u8 = 132;
/// Ports from here up are handed out to clients by the common stacks
/// (Linux from 32768, the IANA dynamic range from 49152).
const FIRST_EPHEMERAL: u16 = 32_768;

fn transport(protocol: u8) -> Option<&'static str> {
    match protocol {
        TCP => Some("tcp"),
        UDP => Some("udp"),
        SCTP => Some("sctp"),
        _ => None,
    }
}

/// (transport, port) → service name, from the reference table.
fn services() -> &'static HashMap<(u8, u16), String> {
    static SERVICES: OnceLock<HashMap<(u8, u16), String>> = OnceLock::new();
    SERVICES.get_or_init(|| {
        let mut services = HashMap::new();
        for entry in nocturne_tools::reference::ports("") {
            for (protocol, name) in [(TCP, "tcp"), (UDP, "udp"), (SCTP, "sctp")] {
                if entry.protocol.split('/').any(|listed| listed == name) {
                    services
                        .entry((protocol, entry.port))
                        .or_insert_with(|| entry.name.clone());
                }
            }
        }
        services
    })
}

/// IP protocol number → its registered keyword.
fn protocols() -> &'static HashMap<u8, String> {
    static PROTOCOLS: OnceLock<HashMap<u8, String>> = OnceLock::new();
    PROTOCOLS.get_or_init(|| {
        nocturne_tools::reference::ip_protocols()
            .into_iter()
            .filter_map(|entry| Some((u8::try_from(entry.number).ok()?, entry.name)))
            .collect()
    })
}

/// The port of a conversation that says what it is: the side the service
/// listens on.
///
/// A port the reference table knows wins (the lower one when both are
/// known). Failing that the lower port, as long as it is below the range
/// clients are given theirs from — a service nobody has named still sits on
/// a fixed port. When both ports are from that range there is no telling
/// which side is the service, and the answer is none: thousands of client
/// ports would otherwise each be an "application". Port 0 is "not
/// reported".
pub fn service_port(protocol: u8, src_port: u16, dst_port: u16) -> Option<u16> {
    transport(protocol)?;
    let (low, high) = (src_port.min(dst_port), src_port.max(dst_port));
    let known = |port: u16| port != 0 && services().contains_key(&(protocol, port));
    if known(low) {
        return Some(low);
    }
    if known(high) {
        return Some(high);
    }
    let lowest = if low == 0 { high } else { low };
    (lowest != 0 && lowest < FIRST_EPHEMERAL).then_some(lowest)
}

/// The key of an application as `flows.top` returns it: `tcp/443`, `udp`,
/// `ip/1`. Numbers only, so that two protocols registered under one word
/// (253 and 254 are both `experimental`) are still two keys.
pub fn application_key(protocol: u8, port: Option<u16>) -> String {
    match (transport(protocol), port) {
        (Some(transport), Some(port)) => format!("{transport}/{port}"),
        (Some(transport), None) => transport.to_string(),
        (None, _) => format!("ip/{protocol}"),
    }
}

/// What a person reads for it: `tcp/443 (https)`, `tcp/5001`,
/// `tcp, no service port`, `ICMP (IP protocol 1)`, `IP protocol 200`.
pub fn application_label(protocol: u8, port: Option<u16>) -> String {
    match (transport(protocol), port) {
        (Some(transport), Some(port)) => match services().get(&(protocol, port)) {
            Some(name) => format!("{transport}/{port} ({name})"),
            None => format!("{transport}/{port}"),
        },
        (Some(transport), None) => format!("{transport}, no service port"),
        (None, _) => match protocols().get(&protocol) {
            Some(name) => format!("{name} (IP protocol {protocol})"),
            None => format!("IP protocol {protocol}"),
        },
    }
}

/// A marking by name and number: `EF (46)`, `DF (0)`, `DSCP 47`.
pub fn dscp_label(dscp: u8) -> String {
    match nocturne_tools::reference::dscp(&dscp.to_string()) {
        // A code point without a name is described by its number already.
        Some(entry) if entry.name.starts_with("DSCP ") => entry.name,
        Some(entry) => format!("{} ({dscp})", entry.name),
        None => format!("DSCP {dscp}"),
    }
}

fn address(address: Option<IpAddr>) -> String {
    match address {
        Some(address) => address.to_string(),
        None => "(no address)".to_string(),
    }
}

/// The key of a row, as text. An interface's is written by the caller,
/// which knows the exporter it belongs to.
pub fn key(key: &Key) -> String {
    match key {
        Key::Address(found) => address(*found),
        Key::Conversation(low, high) => format!("{} <-> {}", address(*low), address(*high)),
        Key::Application(protocol, port) => application_key(*protocol, *port),
        Key::Interface(index, direction) => format!("{index} {}", direction_word(*direction)),
        Key::Dscp(dscp) => dscp.to_string(),
    }
}

pub fn direction_word(direction: Direction) -> &'static str {
    match direction {
        Direction::In => "in",
        Direction::Out => "out",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_is_the_port_that_has_a_name_whichever_side_it_is_on() {
        assert_eq!(service_port(6, 51_000, 443), Some(443));
        assert_eq!(service_port(6, 443, 51_000), Some(443));
        // 3389 is above a client port that happens to be low: the named one
        // is the service.
        assert_eq!(service_port(6, 1_050, 3_389), Some(3_389));
        assert_eq!(service_port(17, 40_000, 53), Some(53));
    }

    #[test]
    fn a_service_nobody_named_is_still_the_lower_fixed_port() {
        assert_eq!(service_port(6, 50_000, 9_999), Some(9_999));
        assert_eq!(service_port(17, 20_001, 20_001), Some(20_001));
    }

    #[test]
    fn two_client_ports_are_not_an_application() {
        assert_eq!(service_port(6, 51_000, 60_000), None);
        assert_eq!(service_port(6, 0, 0), None, "port 0 is `not reported`");
        assert_eq!(service_port(6, 0, 45_000), None);
        assert_eq!(
            service_port(1, 0, 2_048),
            None,
            "ICMP has no ports; that field holds its type and code"
        );
    }

    #[test]
    fn an_application_is_keyed_by_numbers_and_labelled_with_the_name_beside_them() {
        assert_eq!(application_key(6, Some(443)), "tcp/443");
        assert_eq!(application_label(6, Some(443)), "tcp/443 (https)");
        assert_eq!(application_label(17, Some(53)), "udp/53 (domain)");
        assert_eq!(application_key(6, Some(9_999)), "tcp/9999");
        assert_eq!(application_label(6, Some(9_999)), "tcp/9999");
        assert_eq!(application_key(6, None), "tcp");
        assert_eq!(application_label(6, None), "tcp, no service port");
        assert_eq!(application_key(1, None), "ip/1");
        assert_eq!(application_label(1, None), "ICMP (IP protocol 1)");
        assert_ne!(
            application_key(253, None),
            application_key(254, None),
            "two protocols the registry calls by one word are still two keys"
        );
        assert_eq!(application_key(200, None), "ip/200");
        assert_eq!(application_label(200, None), "IP protocol 200");
    }

    #[test]
    fn a_marking_is_named_where_it_has_a_name() {
        assert_eq!(dscp_label(46), "EF (46)");
        assert_eq!(dscp_label(34), "AF41 (34)");
        assert_eq!(dscp_label(0), "DF (0)");
        assert_eq!(dscp_label(47), "DSCP 47");
    }

    #[test]
    fn a_key_is_written_the_same_way_every_time() {
        let a: IpAddr = "10.0.0.5".parse().unwrap();
        let b: IpAddr = "2001:db8::22".parse().unwrap();
        assert_eq!(key(&Key::Address(Some(a))), "10.0.0.5");
        assert_eq!(key(&Key::Address(None)), "(no address)");
        assert_eq!(
            key(&Key::Conversation(Some(a), Some(b))),
            "10.0.0.5 <-> 2001:db8::22"
        );
        assert_eq!(key(&Key::Dscp(46)), "46");
    }
}
