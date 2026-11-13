//! Addresses, prefixes and routing tables.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::checks::{NO_LINK, link_ends, owner, skip_link, skip_rest};
use crate::model::{Finding, Severity, Suggestion};
use crate::net::{self, Cidr};
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{DeviceData, World};

/// Interface addresses that are a real statement about where the device is.
fn addresses(data: &DeviceData) -> Vec<(&str, Cidr)> {
    data.interface_rows()
        .iter()
        .flat_map(|port| {
            port.ipv4
                .iter()
                .filter_map(|text| net::parse_cidr(text))
                .filter(|cidr| net::is_real_host(cidr.address))
                .map(|cidr| (port.name.as_str(), cidr))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// An address on a loopback with a host prefix: anycast, by design, on every
/// device that has it.
fn is_anycast_loopback(interface: &str, cidr: Cidr) -> bool {
    cidr.prefix == 32 && interface.to_ascii_lowercase().starts_with("loopback")
}

// --------------------------------------------------------------- duplicate IP

/// One address on two devices.
///
/// Look-alikes this stays quiet about, and they are the reason a naive
/// version of this check is unusable:
///
/// * **HSRP, VRRP and GLBP virtual addresses.** A redundant pair *shares* an
///   address on purpose. The virtual address is taken from each device's
///   configuration (`standby`/`vrrp`/`glbp … ip`) and excluded, and in ARP
///   tables the virtual MACs (`0000.0c07.acXX`, `0000.5e00.01XX`, …) are
///   excluded too.
/// * **Anycast loopbacks.** A /32 on a loopback is meant to exist in several
///   places.
/// * **The same address in two different subnets.** Two isolated networks
///   reusing 10.0.0.1 behind different masks are not in conflict; only
///   addresses in the same subnet are compared.
/// * **ARP tables collected hours apart.** A host that was replaced legitimately
///   has a new MAC; two tables only disagree if they were read close together.
/// * **A router answering for a host** (proxy ARP, or the gateway itself): a
///   MAC that belongs to a managed device's own interface is not a second host.
pub fn duplicate_ip(world: &World, settings: &Settings, report: &mut CheckReport) {
    let stale_hours = settings.thresholds.stale_data_hours;

    // --- configured addresses
    let mut holders: BTreeMap<(u32, u8, Ipv4Addr), Vec<(&DeviceData, String)>> = BTreeMap::new();
    for data in &world.devices {
        if data.interfaces.is_none() {
            continue;
        }
        if data.stale(world.now, stale_hours) {
            continue;
        }
        let virtuals = data
            .config
            .as_ref()
            .map(net::virtual_addresses)
            .unwrap_or_default();
        for (interface, cidr) in addresses(data) {
            if virtuals.contains(&cidr.address) || is_anycast_loopback(interface, cidr) {
                continue;
            }
            holders
                .entry((cidr.network(), cidr.prefix, cidr.address))
                .or_default()
                .push((data, interface.to_string()));
        }
    }

    // Devices holding an address that cannot be judged either way.
    let mut undecided: std::collections::HashSet<String> = Default::default();
    for ((_, prefix, address), mut found) in holders {
        if found.len() < 2 {
            continue;
        }
        found.sort_by(|(a, _), (b, _)| a.id().cmp(b.id()));
        let (first, _) = &found[0];
        if !found.iter().any(|(data, _)| world.is_target(data.id())) {
            continue;
        }
        // A redundancy group is only recognisable from a configuration this
        // crate can read. Without one for *every* holder, a shared address
        // could be a first-hop redundancy group that nothing here can see —
        // so this is not evaluated rather than reported.
        if !found.iter().all(|(data, _)| {
            data.config
                .as_ref()
                .is_some_and(|config| config.syntax == nocturne_drivers::hier::Syntax::Ios)
        }) {
            for (data, _) in &found {
                undecided.insert(data.id().to_string());
            }
            continue;
        }
        let mut finding = Finding::new(
            "duplicate_ip",
            first.id(),
            address.to_string(),
            format!(
                "{address}/{prefix} is configured on {} devices",
                found.len()
            ),
        )
        .severity(Severity::Critical)
        .detail(
            "Two devices answering for one address give intermittent, direction-dependent \
             failures: ARP caches point at whichever answered last, and anything that reaches the \
             wrong one is silently wrong rather than down. Redundancy protocols and anycast \
             loopbacks are excluded, so this is a genuine clash.",
        );
        for (data, interface) in &found {
            finding = finding.evidence(
                format!("{} {}", data.name(), interface),
                format!(
                    "{address}/{prefix}, collected {}",
                    data.interfaces
                        .as_ref()
                        .map(|collected| collected.at)
                        .unwrap_or_default()
                ),
            );
        }
        for (data, interface) in found.iter().skip(1) {
            finding = finding.related(data.id(), data.name(), interface.clone());
        }
        report.found(
            finding.suggest(
                Suggestion::text(
                    "Decide which device should keep the address and renumber the other. Do the \
                 renumbering from a console or a path that does not cross the address being \
                 changed.",
                )
                .commands(vec!["show ip interface brief".to_string()]),
            ),
        );
    }

    for data in world.targets() {
        if data.interfaces.is_none() && data.arp.is_none() {
            report.skip(
                data.id(),
                "no interface addresses and no ARP table collected",
            );
        } else if data.stale(world.now, stale_hours) {
            report.skip(
                data.id(),
                "the device's collected data is older than the staleness threshold",
            );
        } else if undecided.contains(data.id()) {
            report.skip(
                data.id(),
                "the device shares an address with another, and a configuration that would show \
                 a first-hop redundancy group (HSRP, VRRP, GLBP) is not stored for every holder \
                 in a grammar this reads",
            );
        } else {
            report.ok(data.id());
        }
    }

    duplicate_in_arp(world, settings, report);
}

/// Two ARP tables, read close together, disagreeing about who holds an address.
fn duplicate_in_arp(world: &World, settings: &Settings, report: &mut CheckReport) {
    let window = i64::from(settings.thresholds.arp_window_minutes) * 60_000;
    // Every MAC that belongs to a managed device's own interface: a router
    // answering for an address is not a second host.
    let device_macs: std::collections::HashSet<String> = world
        .devices
        .iter()
        .flat_map(|data| data.interface_rows())
        .filter_map(|port| net::normalise_mac(&port.mac))
        .collect();

    let mut seen: BTreeMap<String, Vec<(&DeviceData, String, i64, String)>> = BTreeMap::new();
    for data in &world.devices {
        let Some(arp) = data.arp.as_ref() else {
            continue;
        };
        for entry in &arp.rows {
            let Ok(address) = entry.ip.parse::<Ipv4Addr>() else {
                continue;
            };
            if !net::is_real_host(address) {
                continue;
            }
            let Some(mac) = net::normalise_mac(&entry.mac) else {
                // `Incomplete`, or a notation we do not read.
                continue;
            };
            if net::virtual_mac(&mac).is_some() || device_macs.contains(&mac) {
                continue;
            }
            seen.entry(entry.ip.clone())
                .or_default()
                .push((data, mac, arp.at, entry.mac.clone()));
        }
    }

    for (address, holders) in seen {
        let distinct: Vec<&(&DeviceData, String, i64, String)> = {
            let mut macs: Vec<&(&DeviceData, String, i64, String)> = Vec::new();
            for holder in &holders {
                if !macs.iter().any(|(_, mac, _, _)| *mac == holder.1) {
                    macs.push(holder);
                }
            }
            macs
        };
        if distinct.len() < 2 {
            continue;
        }
        // Only tables read close together disagree about *now*.
        let newest = distinct.iter().map(|(_, _, at, _)| *at).max().unwrap_or(0);
        let oldest = distinct.iter().map(|(_, _, at, _)| *at).min().unwrap_or(0);
        if newest - oldest > window {
            for (data, ..) in &distinct {
                report.skip(
                    data.id(),
                    "the ARP tables that disagree were collected too far apart to be compared",
                );
            }
            continue;
        }
        if !distinct.iter().any(|(data, ..)| world.is_target(data.id())) {
            continue;
        }
        let (first, ..) = distinct[0];
        let mut finding = Finding::new(
            "duplicate_ip",
            first.id(),
            format!("{address} (ARP)"),
            format!(
                "{address} answers with {} different MAC addresses",
                distinct.len()
            ),
        )
        .severity(Severity::High)
        .detail(
            "Two hosts are using one address: each router learned whichever answered its request \
             last. Virtual MACs belonging to a redundancy group, and MACs belonging to the \
             network devices themselves, are excluded — so these are two real hosts.",
        );
        for (data, _, at, printed) in &distinct {
            finding = finding.evidence(
                format!("Seen by {}", data.name()),
                format!("{address} → {printed} (collected {at})"),
            );
        }
        for (data, ..) in distinct.iter().skip(1) {
            finding = finding.related(data.id(), data.name(), format!("{address} (ARP)"));
        }
        report.found(finding.suggest(
            Suggestion::text(
                "Find both hosts before changing anything: the switch port of each can be found \
                 from the MAC addresses in the evidence. One of them has a static address inside \
                 a DHCP range often enough to be worth checking first.",
            )
            .commands(vec![format!("show ip arp {address}")]),
        ));
    }
}

// ------------------------------------------------------- subnet mask mismatch

/// The two ends of a link do not agree how large their subnet is.
///
/// Look-alike: **secondary addressing**. An interface with several addresses
/// is doing something deliberate, so only ends with exactly one address each
/// are compared. Ends whose addresses do not overlap at all are also left
/// alone — that is a different fault, and p2p links, unnumbered interfaces and
/// /31s make guessing at it unreliable.
pub fn subnet_mask_mismatch(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for link in world.target_links() {
        let (a, a_port, b, b_port) = match link_ends(world, link) {
            Ok(ends) => ends,
            Err(reason) => {
                skip_link(report, link, reason);
                continue;
            }
        };
        let one = |port: &nocturne_drivers::model::Interface| -> Option<Cidr> {
            match port.ipv4.as_slice() {
                [only] => net::parse_cidr(only).filter(|cidr| net::is_real_host(cidr.address)),
                _ => None,
            }
        };
        let (Some(a_cidr), Some(b_cidr)) = (one(a_port), one(b_port)) else {
            skip_link(
                report,
                link,
                "one end has no address, or more than one (secondary addressing)",
            );
            continue;
        };
        report.ok(a.id());
        report.ok(b.id());
        if a_cidr.prefix == b_cidr.prefix || !a_cidr.overlaps(b_cidr) {
            continue;
        }
        let near_is_a = owner(a.id(), b.id()).0 == a.id();
        let (near, near_port, near_cidr, far, far_port, far_cidr) = if near_is_a {
            (a, a_port, a_cidr, b, b_port, b_cidr)
        } else {
            (b, b_port, b_cidr, a, a_port, a_cidr)
        };
        report.found(
            Finding::new(
                "subnet_mask_mismatch",
                near.id(),
                &near_port.name,
                format!(
                    "{} {} is /{} but {} {} is /{} on the same link",
                    near.name(),
                    near_port.name,
                    near_cidr.prefix,
                    far.name(),
                    far_port.name,
                    far_cidr.prefix
                ),
            )
            .detail(
                "Each end believes a different range of addresses is on the local wire. Traffic \
                 to addresses inside the wider mask but outside the narrower one is sent directly \
                 by one end and via a router by the other, so it works in one direction only.",
            )
            .evidence(
                format!("{} {}", near.name(), near_port.name),
                format!("{near_cidr} (network {})", near_cidr.network_cidr()),
            )
            .evidence(
                format!("{} {}", far.name(), far_port.name),
                format!("{far_cidr} (network {})", far_cidr.network_cidr()),
            )
            .related(far.id(), far.name(), &far_port.name)
            .suggest(
                Suggestion::text(
                    "Decide which prefix length the segment has and correct the other end. \
                     Changing the mask of the interface you are connected through will drop the \
                     session, so do it from the other side or a console.",
                )
                .commands([
                    format!("show ip interface {}", near_port.name),
                    format!("show ip interface {}", far_port.name),
                ]),
            ),
        );
    }
    skip_rest(world, report, NO_LINK);
}

// ------------------------------------------------------- overlapping subnets

/// One device with two interfaces in overlapping subnets.
///
/// Look-alikes: **host prefixes** (/32, /31) are loopbacks and point-to-point
/// links and overlap by design, and **two addresses on the same interface**
/// are secondary addressing.
pub fn overlapping_subnets(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for data in world.targets() {
        if data.interfaces.is_none() {
            report.skip(data.id(), "no interface data collected");
            continue;
        }
        let addresses: Vec<(&str, Cidr)> = addresses(data)
            .into_iter()
            .filter(|(_, cidr)| cidr.prefix <= 30)
            .collect();
        if addresses.len() < 2 {
            report.skip(data.id(), "fewer than two addressed interfaces");
            continue;
        }
        report.ok(data.id());
        let mut reported: Vec<(String, String)> = Vec::new();
        for (index, (interface, cidr)) in addresses.iter().enumerate() {
            for (other_interface, other) in addresses.iter().skip(index + 1) {
                if interface == other_interface
                    || cidr.network_cidr() == other.network_cidr()
                    || !cidr.overlaps(*other)
                {
                    continue;
                }
                let pair = (interface.to_string(), other_interface.to_string());
                if reported.contains(&pair) {
                    continue;
                }
                reported.push(pair);
                report.found(
                    Finding::new(
                        "overlapping_subnets",
                        data.id(),
                        format!("{interface} / {other_interface}"),
                        format!(
                            "{} and {} are in overlapping subnets ({} and {})",
                            interface,
                            other_interface,
                            cidr.network_cidr(),
                            other.network_cidr()
                        ),
                    )
                    .detail(
                        "The device has two local networks that contain some of the same \
                         addresses, so which interface it uses for them depends on the order its \
                         routing table happens to be in. This is usually a typed mask or a \
                         subnet planned twice.",
                    )
                    .evidence(interface.to_string(), format!("{cidr}"))
                    .evidence(other_interface.to_string(), format!("{other}"))
                    .suggest(
                        Suggestion::text(
                            "Work out which of the two subnets was intended and renumber the \
                             other. Check the addressing plan first: the overlap may be the \
                             symptom of two plans.",
                        )
                        .commands(vec!["show ip interface brief".to_string()]),
                    ),
                );
            }
        }
    }
    skip_rest(world, report, "no interface data collected");
}

// --------------------------------------------------------- no default route

/// A routing device with nowhere to send what it has no route for.
///
/// Look-alikes: a **layer-2 switch** has no routing table to miss a default
/// from — it uses `ip default-gateway`, and is excluded; and a **core** device
/// in a default-free core is reported as information only, because that design
/// is deliberate.
pub fn no_default_route(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for data in world.targets() {
        let Some(routes) = data.routes.as_ref() else {
            report.skip(data.id(), "no routing table collected");
            continue;
        };
        if routes.rows.is_empty() {
            report.skip(data.id(), "the collected routing table is empty");
            continue;
        }
        // A switch that uses `ip default-gateway` is not routing at all.
        if let Some(config) = data.config.as_ref()
            && config.has_line_starting("ip default-gateway ")
            && !config.has_line_starting("ip routing")
        {
            report.skip(
                data.id(),
                "the device is a layer-2 switch (`ip default-gateway`, no `ip routing`)",
            );
            continue;
        }
        report.ok(data.id());
        let has_default = routes.rows.iter().any(|route| {
            let prefix = route.prefix.trim();
            prefix == "0.0.0.0/0" || prefix == "default" || prefix == "0.0.0.0 0.0.0.0"
        });
        if has_default {
            continue;
        }
        let core = data.device.role.eq_ignore_ascii_case("core");
        report.found(
            Finding::new(
                "no_default_route",
                data.id(),
                "0.0.0.0/0",
                format!("{} has no default route", data.name()),
            )
            .severity(if core { Severity::Info } else { Severity::Low })
            .detail(if core {
                "A core device with no default route is often deliberate — a default-free core \
                 relies on specific routes and drops the rest on purpose. Worth confirming, not \
                 worth fixing blindly."
            } else {
                "Anything this device has no specific route for is dropped. On a device that is \
                 not in a default-free core that usually means the static default was lost, or \
                 the protocol that should advertise it is not."
            })
            .evidence("Routes collected", routes.rows.len().to_string())
            .evidence(
                "Protocols seen",
                protocols(routes.rows.iter().map(|route| route.protocol.as_str())),
            )
            .suggest(
                Suggestion::text(
                    "Check whether a default is expected here at all; if it is, find out whether \
                     it was a static route that was removed or an advertisement that stopped \
                     arriving before adding one.",
                )
                .commands(vec!["show ip route 0.0.0.0".to_string()]),
            ),
        );
    }
    skip_rest(world, report, "no routing table collected");
}

fn protocols<'a>(protocols: impl Iterator<Item = &'a str>) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for protocol in protocols {
        let protocol = protocol.trim();
        if !protocol.is_empty() && !seen.contains(&protocol) {
            seen.push(protocol);
        }
    }
    seen.sort_unstable();
    seen.join(", ")
}

#[cfg(test)]
#[path = "l3_tests.rs"]
mod tests;
