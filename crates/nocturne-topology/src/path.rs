//! The path between two points, walked over collected routing tables.
//!
//! Longest-prefix match on each device in turn, then ARP and the MAC tables
//! for the last hop. Where the data runs out the walk stops and says which
//! device it stopped at and what is missing — it never guesses the next hop.

use std::collections::BTreeSet;

use nocturne_app::App;
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::settings;
use nocturne_drivers::ifname;
use nocturne_drivers::model::Route;
use serde::{Deserialize, Serialize};

use crate::data::{Collected, host_of};
use crate::endpoints::{self, PortKind};
use crate::model::{SETTINGS_SECTION, Settings};
use crate::physical;

/// How far the walk may go before it is obviously a loop.
const MAX_HOPS: usize = 32;

#[derive(Debug, Deserialize)]
pub struct Params {
    /// A device id, name or IP address to start from.
    pub from: String,
    /// A device id, name or IP address to reach.
    pub to: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hop {
    /// The name of the device, or of the host at the end.
    pub node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_interface: Option<String>,
    /// `connected`, `route`, `arp` or `mac`.
    pub via: String,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Trace {
    pub from: String,
    pub to: String,
    /// The address the walk was aiming at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    pub hops: Vec<Hop>,
    /// Whether the walk reached the destination.
    pub complete: bool,
    /// Why it stopped, when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_because: Option<String>,
    /// Equal-cost choices, VRFs and anything else worth saying.
    pub notes: Vec<String>,
}

/// One end of the question, resolved as far as the data allows.
struct Endpoint {
    device_id: Option<String>,
    ip: Option<String>,
    label: String,
}

fn resolve(app: &App, collected: &Collected, text: &str) -> AppResult<Endpoint> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::invalid("give a device or an address"));
    }
    if let Ok(device) = nocturne_app::inventory::find(app, text) {
        let ip = pick_address(collected, &device.id).or_else(|| {
            Some(device.address.clone()).filter(|a| a.parse::<std::net::IpAddr>().is_ok())
        });
        return Ok(Endpoint {
            label: device.name.clone(),
            device_id: Some(device.id),
            ip,
        });
    }
    if text.parse::<std::net::IpAddr>().is_ok() {
        let lowered = text.to_ascii_lowercase();
        return Ok(Endpoint {
            device_id: collected.device_with_address(&lowered).map(str::to_string),
            ip: Some(lowered),
            label: text.to_string(),
        });
    }
    Err(AppError::not_found(format!(
        "`{text}` is neither a device in the inventory nor an IP address"
    )))
}

/// The address of a device a route would be matched against: its management
/// address if it has one, else a loopback, else the lowest interface address.
fn pick_address(collected: &Collected, device_id: &str) -> Option<String> {
    let aged = collected.interfaces.get(device_id)?;
    let managed = collected
        .device(device_id)
        .map(|row| row.device.address.to_ascii_lowercase())
        .filter(|address| address.parse::<std::net::IpAddr>().is_ok());
    let mut loopbacks: Vec<String> = Vec::new();
    let mut others: Vec<String> = Vec::new();
    for interface in &aged.rows {
        for cidr in &interface.ipv4 {
            let Some(host) = host_of(cidr) else { continue };
            if ifname::long(&interface.name).starts_with("Loopback") {
                loopbacks.push(host);
            } else {
                others.push(host);
            }
        }
    }
    loopbacks.sort();
    others.sort();
    if let Some(managed) = managed
        && (loopbacks.contains(&managed) || others.contains(&managed))
    {
        return Some(managed);
    }
    loopbacks
        .into_iter()
        .next()
        .or_else(|| others.into_iter().next())
}

/// Whether a route is in the table a packet with no VRF would be looked up in.
fn is_default_vrf(route: &Route) -> bool {
    route.vrf.is_empty() || route.vrf.eq_ignore_ascii_case("default")
}

/// The routes of a device that match an address, longest prefix first.
///
/// The default VRF is looked at first; another one is only used when nothing
/// in the default table matches, and the caller says so in a note.
fn best_routes(routes: &[Route], destination: &str) -> (Vec<Route>, Option<u8>) {
    let in_default: Vec<Route> = routes
        .iter()
        .filter(|route| is_default_vrf(route))
        .cloned()
        .collect();
    if !in_default.is_empty() {
        let found = matching(&in_default, destination);
        if found.1.is_some() {
            return found;
        }
    }
    matching(routes, destination)
}

fn matching(routes: &[Route], destination: &str) -> (Vec<Route>, Option<u8>) {
    let mut matches: Vec<(u8, Route)> = Vec::new();
    for route in routes {
        let Ok(info) = nocturne_tools::subnet::info(&route.prefix) else {
            continue;
        };
        if nocturne_tools::subnet::contains(&info.cidr, destination).unwrap_or(false) {
            matches.push((info.prefix, route.clone()));
        }
    }
    let Some(longest) = matches.iter().map(|(prefix, _)| *prefix).max() else {
        return (Vec::new(), None);
    };
    let mut best: Vec<Route> = matches
        .into_iter()
        .filter(|(prefix, _)| *prefix == longest)
        .map(|(_, route)| route)
        .collect();
    // Among equals: lowest administrative distance, then lowest metric, then
    // a stable order so the same data always gives the same path.
    best.sort_by(|a, b| {
        a.distance
            .unwrap_or(0)
            .cmp(&b.distance.unwrap_or(0))
            .then(a.metric.unwrap_or(0).cmp(&b.metric.unwrap_or(0)))
            .then(a.next_hop.cmp(&b.next_hop))
            .then(a.interface.cmp(&b.interface))
    });
    let lowest = (best[0].distance.unwrap_or(0), best[0].metric.unwrap_or(0));
    best.retain(|route| (route.distance.unwrap_or(0), route.metric.unwrap_or(0)) == lowest);
    (best, Some(longest))
}

fn is_connected(route: &Route) -> bool {
    let protocol = route.protocol.to_ascii_lowercase();
    route.next_hop.trim().is_empty()
        || protocol.starts_with("connected")
        || protocol.starts_with("local")
        || protocol.starts_with("direct")
}

fn describe(route: &Route, prefix: Option<u8>) -> String {
    let mut detail = match prefix {
        Some(0) => format!("default route {}", route.prefix),
        _ => format!("{} matches", route.prefix),
    };
    if !route.protocol.is_empty() {
        detail.push_str(&format!(" ({})", route.protocol));
    }
    if !route.next_hop.trim().is_empty() {
        detail.push_str(&format!(", next hop {}", route.next_hop.trim()));
    }
    if !route.interface.trim().is_empty() {
        detail.push_str(&format!(
            ", out of {}",
            ifname::long(route.interface.trim())
        ));
    }
    if !route.vrf.is_empty() && route.vrf != "default" {
        detail.push_str(&format!(", in VRF {}", route.vrf));
    }
    detail
}

pub fn trace(app: &App, params: &Params) -> AppResult<Trace> {
    let config: Settings = settings::get(app, SETTINGS_SECTION)?;
    let collected = Collected::load(app)?;
    let from = resolve(app, &collected, &params.from)?;
    let to = resolve(app, &collected, &params.to)?;

    let mut trace = Trace {
        from: from.label.clone(),
        to: to.label.clone(),
        destination: to.ip.clone(),
        hops: Vec::new(),
        complete: false,
        stopped_because: None,
        notes: Vec::new(),
    };

    let Some(mut current) = from.device_id.clone() else {
        trace.stopped_because = Some(format!(
            "{} is not a device Nocturne manages, so there is no routing table to start from. Give \
             a device as `from`",
            from.label
        ));
        return Ok(trace);
    };
    let Some(destination) = to.ip.clone() else {
        trace.stopped_because = Some(format!(
            "no IP address is known for {}. Collect `interfaces` from it, or give an address",
            to.label
        ));
        return Ok(trace);
    };

    let mut in_interface: Option<String> = None;
    let mut visited: BTreeSet<String> = BTreeSet::new();
    for _ in 0..MAX_HOPS {
        if !visited.insert(current.clone()) {
            trace.stopped_because = Some(format!(
                "the path returns to {}, which is a routing loop in the collected data",
                collected.name_of(&current)
            ));
            return Ok(trace);
        }
        let name = collected.name_of(&current);

        // Arrived: the destination device itself, or an address it owns.
        let owns = collected
            .addresses_of(&current)
            .iter()
            .any(|address| address.eq_ignore_ascii_case(&destination));
        if to.device_id.as_deref() == Some(current.as_str()) || owns {
            trace.hops.push(Hop {
                node: name,
                device_id: Some(current.clone()),
                in_interface: in_interface.clone(),
                out_interface: None,
                via: "connected".to_string(),
                detail: format!("{destination} is an address of this device"),
            });
            trace.complete = true;
            return Ok(trace);
        }

        let Some(routes) = collected.routes.get(&current) else {
            trace.hops.push(Hop {
                node: name.clone(),
                device_id: Some(current.clone()),
                in_interface: in_interface.clone(),
                out_interface: None,
                via: "route".to_string(),
                detail: "no routing table has been collected from this device".to_string(),
            });
            trace.stopped_because = Some(format!(
                "no route data collected for {name}. Run a collection of `routes` on it and ask \
                 again"
            ));
            return Ok(trace);
        };

        let (best, prefix) = best_routes(&routes.rows, &destination);
        let Some(route) = best.first().cloned() else {
            trace.hops.push(Hop {
                node: name.clone(),
                device_id: Some(current.clone()),
                in_interface: in_interface.clone(),
                out_interface: None,
                via: "route".to_string(),
                detail: format!(
                    "no route to {destination} among the {} routes collected",
                    routes.rows.len()
                ),
            });
            trace.stopped_because = Some(format!("{name} has no route to {destination}"));
            return Ok(trace);
        };
        if best.len() > 1 {
            let choices: Vec<String> = best
                .iter()
                .map(|route| {
                    let hop = route.next_hop.trim();
                    if hop.is_empty() {
                        ifname::long(route.interface.trim())
                    } else {
                        hop.to_string()
                    }
                })
                .collect();
            trace.notes.push(format!(
                "{name} has {} equal-cost paths to {destination} (via {}); the first is followed",
                best.len(),
                choices.join(", ")
            ));
        }
        if !is_default_vrf(&route) {
            trace.notes.push(format!(
                "nothing in the default routing table of {name} matches {destination}; the route \
                 used is in VRF {}",
                route.vrf
            ));
        }

        if !is_connected(&route) {
            let next_hop = route.next_hop.trim().to_ascii_lowercase();
            let out = Some(route.interface.trim())
                .filter(|name| !name.is_empty())
                .map(ifname::long)
                .or_else(|| interface_towards(&collected, &current, &next_hop));
            trace.hops.push(Hop {
                node: name.clone(),
                device_id: Some(current.clone()),
                in_interface: in_interface.clone(),
                out_interface: out,
                via: "route".to_string(),
                detail: describe(&route, prefix),
            });
            let Some(next) = collected.device_with_address(&next_hop) else {
                trace.stopped_because = Some(format!(
                    "the next hop {next_hop} is not an address of any device Nocturne manages (or \
                     `interfaces` has not been collected from it), so the path cannot be followed \
                     past {name}"
                ));
                return Ok(trace);
            };
            let next = next.to_string();
            in_interface = interface_with_address(&collected, &next, &next_hop);
            current = next;
            continue;
        }

        // Connected: the destination is on one of this device's own subnets.
        let out = Some(route.interface.trim())
            .filter(|name| !name.is_empty())
            .map(ifname::long)
            .or_else(|| interface_towards(&collected, &current, &destination));
        trace.hops.push(Hop {
            node: name.clone(),
            device_id: Some(current.clone()),
            in_interface: in_interface.clone(),
            out_interface: out.clone(),
            via: "connected".to_string(),
            detail: describe(&route, prefix),
        });
        return Ok(last_hop(
            &collected,
            &config,
            trace,
            &current,
            &destination,
            out,
        ));
    }
    trace.stopped_because = Some(format!(
        "the path is longer than {MAX_HOPS} hops, which is not a path any more"
    ));
    Ok(trace)
}

/// From the connected device to the host: ARP for the MAC, then the MAC tables
/// for the port.
fn last_hop(
    collected: &Collected,
    config: &Settings,
    mut trace: Trace,
    device_id: &str,
    destination: &str,
    out: Option<String>,
) -> Trace {
    let name = collected.name_of(device_id);
    let entry = collected
        .arp
        .get(device_id)
        .and_then(|aged| {
            aged.rows
                .iter()
                .find(|entry| entry.ip.trim().eq_ignore_ascii_case(destination))
                .cloned()
        })
        .and_then(|entry| {
            crate::data::bare_mac(&entry.mac).map(|mac| (mac, entry.interface.clone()))
        });
    let Some((mac, arp_interface)) = entry else {
        trace.stopped_because = Some(format!(
            "{destination} is on a network connected to {name}{}, but no ARP entry for it has been \
             collected there, so the port it is on is unknown",
            out.as_ref()
                .map(|out| format!(" through {out}"))
                .unwrap_or_default()
        ));
        return trace;
    };
    trace.hops.push(Hop {
        node: destination.to_string(),
        device_id: None,
        in_interface: Some(ifname::long(arp_interface.trim())).filter(|name| !name.is_empty()),
        out_interface: None,
        via: "arp".to_string(),
        detail: format!(
            "{name} has {destination} at {} in its ARP table",
            crate::data::pretty(&mac)
        ),
    });

    let scope: BTreeSet<String> = collected
        .devices
        .iter()
        .map(|row| row.device.id.clone())
        .collect();
    let reported = physical::build(collected, &scope);
    let seen = endpoints::sightings(
        collected,
        &scope,
        &reported.network_ports,
        config.endpoint_max_macs as usize,
    );
    let sightings = seen.get(&mac).cloned().unwrap_or_default();
    match endpoints::best_sighting(&sightings) {
        Some(index) => {
            let sighting = &sightings[index];
            trace.hops.push(Hop {
                node: destination.to_string(),
                device_id: Some(sighting.device_id.clone()),
                in_interface: Some(sighting.interface.clone()),
                out_interface: None,
                via: "mac".to_string(),
                detail: format!(
                    "learnt on {} {} ({}): {}",
                    collected.name_of(&sighting.device_id),
                    sighting.interface,
                    sighting.kind.as_str(),
                    sighting.why
                ),
            });
            trace.complete = true;
            if sighting.kind != PortKind::Access {
                trace.notes.push(
                    "the last hop is a trunk or an uplink, not an edge port: collect `mac_table` \
                     from the access switches to find the port itself"
                        .to_string(),
                );
            }
        }
        None => {
            trace.complete = true;
            trace.notes.push(format!(
                "{} is in no collected MAC table, so the final switch port is unknown",
                crate::data::pretty(&mac)
            ));
        }
    }
    trace
}

/// The interface of a device whose subnet contains an address.
fn interface_towards(collected: &Collected, device_id: &str, address: &str) -> Option<String> {
    let aged = collected.interfaces.get(device_id)?;
    for interface in &aged.rows {
        for cidr in &interface.ipv4 {
            let Ok(info) = nocturne_tools::subnet::info(cidr) else {
                continue;
            };
            if nocturne_tools::subnet::contains(&info.cidr, address).unwrap_or(false) {
                return Some(ifname::long(&interface.name));
            }
        }
    }
    None
}

/// The interface of a device that has exactly this address.
fn interface_with_address(collected: &Collected, device_id: &str, address: &str) -> Option<String> {
    let aged = collected.interfaces.get(device_id)?;
    aged.rows
        .iter()
        .find(|interface| {
            interface
                .ipv4
                .iter()
                .any(|cidr| host_of(cidr).as_deref() == Some(address))
        })
        .map(|interface| ifname::long(&interface.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(prefix: &str, protocol: &str, next_hop: &str, metric: u64) -> Route {
        Route {
            prefix: prefix.to_string(),
            protocol: protocol.to_string(),
            next_hop: next_hop.to_string(),
            metric: Some(metric),
            ..Route::default()
        }
    }

    #[test]
    fn the_longest_prefix_wins_over_the_default_route() {
        let routes = vec![
            route("0.0.0.0/0", "static", "10.0.0.1", 1),
            route("10.1.0.0/16", "ospf", "10.0.0.5", 20),
            route("10.1.10.0/24", "ospf", "10.0.0.9", 20),
        ];
        let (best, prefix) = best_routes(&routes, "10.1.10.55");
        assert_eq!(prefix, Some(24));
        assert_eq!(best.len(), 1);
        assert_eq!(best[0].next_hop, "10.0.0.9");

        let (best, prefix) = best_routes(&routes, "192.0.2.1");
        assert_eq!(prefix, Some(0));
        assert_eq!(best[0].next_hop, "10.0.0.1");
        assert!(describe(&best[0], prefix).starts_with("default route"));

        assert_eq!(best_routes(&[], "10.0.0.1").0.len(), 0);
    }

    #[test]
    fn equal_cost_routes_are_all_reported() {
        let routes = vec![
            route("10.1.0.0/16", "ospf", "10.0.0.5", 20),
            route("10.1.0.0/16", "ospf", "10.0.0.9", 20),
            route("10.1.0.0/16", "ospf", "10.0.0.13", 30),
        ];
        let (best, _) = best_routes(&routes, "10.1.2.3");
        assert_eq!(best.len(), 2, "the metric-30 path is not equal cost");
        assert_eq!(best[0].next_hop, "10.0.0.13".replace("13", "5"));
    }

    #[test]
    fn connected_routes_are_recognised_however_they_are_spelt() {
        assert!(is_connected(&route("10.1.10.0/24", "connected", "", 0)));
        assert!(is_connected(&route("10.1.10.0/24", "local", "", 0)));
        assert!(is_connected(&route("10.1.10.0/24", "", "", 0)));
        assert!(!is_connected(&route("10.1.10.0/24", "ospf", "10.0.0.1", 0)));
    }
}
