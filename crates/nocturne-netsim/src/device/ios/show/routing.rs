//! `show` commands for layer 3: ARP, the routing table, access lists.

use std::fmt::Write as _;
use std::net::Ipv4Addr;

use crate::api::SimTime;
use crate::device::cli::grammar::Args;
use crate::device::config::AclItem;
use crate::sim::acl::{hits, rule_show_text};
use crate::sim::node::Node;
use crate::sim::rib::{Route, Source};
use crate::util::clock::age_compact;
use crate::util::net::Prefix;

pub(crate) fn arp(node: &Node, now: SimTime) -> String {
    let mut rows: Vec<(Ipv4Addr, String, String, String)> = Vec::new();
    for (iface, addr) in &node.rib.addrs {
        if iface.kind != crate::ifname::IfKind::Loopback {
            rows.push((
                addr.ip,
                "-".to_string(),
                node.mac_for(*iface).to_string(),
                iface.name(),
            ));
        }
    }
    for (ip, entry) in node.arp.rows(now) {
        let minutes = now.saturating_sub(entry.learned) / 60_000_000;
        rows.push((
            ip,
            minutes.to_string(),
            entry.mac.to_string(),
            entry.iface.name(),
        ));
    }
    rows.sort_by_key(|(ip, ..)| *ip);
    let mut out =
        String::from("Protocol  Address          Age (min)  Hardware Addr   Type   Interface\n");
    for (ip, age, mac, iface) in rows {
        let _ = writeln!(
            out,
            "Internet  {:<15}{age:>11}  {mac}  ARPA   {iface}",
            ip.to_string()
        );
    }
    out
}

const CODES: &str = "Codes: L - local, C - connected, S - static, R - RIP, M - mobile, B - BGP
       D - EIGRP, EX - EIGRP external, O - OSPF, IA - OSPF inter area
       N1 - OSPF NSSA external type 1, N2 - OSPF NSSA external type 2
       E1 - OSPF external type 1, E2 - OSPF external type 2
       i - IS-IS, su - IS-IS summary, L1 - IS-IS level-1, L2 - IS-IS level-2
       ia - IS-IS inter area, * - candidate default, U - per-user static route
       o - ODR, P - periodic downloaded static route, H - NHRP, l - LISP
       a - application route
       + - replicated route, % - next hop override, p - overrides from PfR
";

/// What follows the prefix on a route line, one string per next hop.
fn hops(route: &Route, now: SimTime) -> Vec<String> {
    let age = age_compact(now.saturating_sub(route.since));
    route
        .hops
        .iter()
        .map(|hop| match (route.source, hop.via, hop.iface) {
            (Source::Connected | Source::Local, _, Some(iface)) => {
                format!("is directly connected, {}", iface.name())
            }
            (Source::Static, None, Some(iface)) => {
                format!("is directly connected, {}", iface.name())
            }
            (Source::Static, Some(via), Some(iface)) => {
                format!(
                    "[{}/{}] via {via}, {}",
                    route.distance,
                    route.metric,
                    iface.name()
                )
            }
            (Source::Static, Some(via), None) => {
                format!("[{}/{}] via {via}", route.distance, route.metric)
            }
            (_, Some(via), Some(iface)) => {
                format!(
                    "[{}/{}] via {via}, {age}, {}",
                    route.distance,
                    route.metric,
                    iface.name()
                )
            }
            (_, Some(via), None) => {
                format!("[{}/{}] via {via}, {age}", route.distance, route.metric)
            }
            (_, None, _) => "is directly connected".to_string(),
        })
        .collect()
}

fn route_lines(out: &mut String, route: &Route, now: SimTime, code_width: usize, with_len: bool) {
    let prefix = if with_len {
        route.prefix.to_string()
    } else {
        route.prefix.net.to_string()
    };
    let head = format!("{:<code_width$}{prefix} ", route.code());
    for (index, hop) in hops(route, now).iter().enumerate() {
        if index == 0 {
            let _ = writeln!(out, "{head}{hop}");
        } else {
            let _ = writeln!(out, "{:<width$}{hop}", "", width = head.len());
        }
    }
}

fn wanted(route: &Route, a: &Args) -> bool {
    (!a.has("static") || route.source == Source::Static)
        && (!a.has("connected") || matches!(route.source, Source::Connected | Source::Local))
        && (!a.has("ospf") || route.source.is_ospf())
        && (!a.has("rip") || route.source == Source::Rip)
}

pub(crate) fn ip_route(node: &Node, now: SimTime, a: &Args) -> String {
    if !node.routes_packets() {
        // A layer-2 switch has no routing table, only a default gateway.
        let gateway = node
            .cfg
            .default_gateway
            .map_or("not set".to_string(), |g| g.to_string());
        return format!(
            "Default gateway is {gateway}\n\nHost               Gateway           Last Use    Total Uses  Interface\nICMP redirect cache is empty\n"
        );
    }
    if let Some(ip) = a.ip("prefix") {
        return route_detail(node, now, ip);
    }
    if a.has("summary") {
        return route_summary(node);
    }
    let mut routes: Vec<&Route> = node.rib.routes.iter().collect();
    routes.sort_by_key(|r| (r.prefix.net, r.prefix.len));
    let mut out = String::from(CODES);
    out.push('\n');
    match node.rib.default_route().and_then(|r| r.hops.first()) {
        Some(hop) => {
            let via = hop.via.unwrap_or(Ipv4Addr::UNSPECIFIED);
            let _ = writeln!(out, "Gateway of last resort is {via} to network 0.0.0.0\n");
        }
        None => out.push_str("Gateway of last resort is not set\n\n"),
    }
    let mut index = 0;
    while index < routes.len() {
        let route = routes[index];
        let major = Prefix::classful(route.prefix.net);
        // Supernets and the default route stand alone.
        if route.prefix.len < major.len || route.prefix.len == 0 {
            if wanted(route, a) {
                route_lines(&mut out, route, now, 6, true);
            }
            index += 1;
            continue;
        }
        let group: Vec<&Route> = routes[index..]
            .iter()
            .copied()
            .take_while(|r| r.prefix.len >= major.len && major.contains(r.prefix.net))
            .collect();
        index += group.len();
        // The heading describes the whole major network even when a filter
        // (`show ip route static`) hides some of its subnets.
        let shown: Vec<&Route> = group.iter().copied().filter(|r| wanted(r, a)).collect();
        if shown.is_empty() {
            continue;
        }
        if group.len() == 1 && route.prefix == major {
            route_lines(&mut out, route, now, 6, true);
            continue;
        }
        let mut masks: Vec<u8> = group.iter().map(|r| r.prefix.len).collect();
        masks.sort_unstable();
        masks.dedup();
        if masks.len() == 1 {
            let _ = writeln!(
                out,
                "      {}/{} is subnetted, {} subnets",
                major.net,
                masks[0],
                group.len()
            );
        } else {
            let _ = writeln!(
                out,
                "      {major} is variably subnetted, {} subnets, {} masks",
                group.len(),
                masks.len()
            );
        }
        for r in shown {
            route_lines(&mut out, r, now, 9, masks.len() > 1);
        }
    }
    out
}

fn source_name(node: &Node, source: Source) -> String {
    match source {
        Source::Connected | Source::Local => "connected".to_string(),
        Source::Static => "static".to_string(),
        Source::Rip => "rip".to_string(),
        Source::OspfIntra | Source::OspfInter | Source::OspfExternal => {
            format!("ospf {}", node.cfg.ospf.as_ref().map_or(1, |o| o.pid))
        }
    }
}

fn route_detail(node: &Node, now: SimTime, ip: Ipv4Addr) -> String {
    let Some(route) = node.rib.lookup(ip) else {
        return "% Network not in table\n".to_string();
    };
    let age = age_compact(now.saturating_sub(route.since));
    let mut out = format!("Routing entry for {}", route.prefix);
    if route.prefix.len == 0 {
        out.push_str(", supernet");
    }
    out.push('\n');
    let kind = match route.source {
        Source::OspfIntra => ", type intra area",
        Source::OspfInter => ", type inter area",
        Source::OspfExternal => ", type extern 2, forward metric 1",
        _ => "",
    };
    let known = if matches!(route.source, Source::Connected | Source::Local) {
        format!(
            "  Known via \"connected\", distance 0, metric 0 (connected{})",
            if route.source == Source::Local {
                ", via interface"
            } else {
                ", via interface"
            }
        )
    } else {
        format!(
            "  Known via \"{}\", distance {}, metric {}{}{kind}",
            source_name(node, route.source),
            route.distance,
            route.metric,
            if route.prefix.len == 0 {
                ", candidate default path"
            } else {
                ""
            }
        )
    };
    let _ = writeln!(out, "{known}");
    out.push_str("  Routing Descriptor Blocks:\n");
    for hop in &route.hops {
        match (hop.via, hop.iface) {
            (Some(via), Some(iface)) => {
                let _ = writeln!(out, "  * {via}, {age} ago, via {}", iface.name());
            }
            (Some(via), None) => {
                let _ = writeln!(out, "  * {via}");
            }
            (None, Some(iface)) => {
                let _ = writeln!(out, "  * directly connected, via {}", iface.name());
            }
            (None, None) => {}
        }
        let _ = writeln!(
            out,
            "      Route metric is {}, traffic share count is 1",
            route.metric
        );
    }
    out
}

fn route_summary(node: &Node) -> String {
    let mut out = String::from(
        "IP routing table name is default (0x0)\nIP routing table maximum-paths is 32\n",
    );
    out.push_str(
        "Route Source    Networks    Subnets     Replicates  Overhead    Memory (bytes)\n",
    );
    let mut totals = (0usize, 0usize);
    let sources: [(&str, &dyn Fn(Source) -> bool); 4] = [
        ("connected", &|s| {
            matches!(s, Source::Connected | Source::Local)
        }),
        ("static", &|s| s == Source::Static),
        ("ospf", &|s| s.is_ospf()),
        ("rip", &|s| s == Source::Rip),
    ];
    for (name, matches) in sources {
        let routes: Vec<&Route> = node
            .rib
            .routes
            .iter()
            .filter(|r| matches(r.source))
            .collect();
        if routes.is_empty() && matches!(name, "ospf" | "rip") {
            continue;
        }
        let networks = routes
            .iter()
            .filter(|r| r.prefix.len <= Prefix::classful(r.prefix.net).len)
            .count();
        let subnets = routes.len() - networks;
        totals.0 += networks;
        totals.1 += subnets;
        let label = if name == "ospf" {
            source_name(node, Source::OspfIntra)
        } else {
            name.to_string()
        };
        let _ = writeln!(
            out,
            "{label:<16}{networks:<12}{subnets:<12}{:<12}{:<12}{}",
            0,
            routes.len() * 96,
            routes.len() * 304
        );
        if name == "ospf" {
            let count = |s: Source| routes.iter().filter(|r| r.source == s).count();
            let _ = writeln!(
                out,
                "  Intra-area: {} Inter-area: {} External-1: 0 External-2: {}\n  NSSA External-1: 0 NSSA External-2: 0",
                count(Source::OspfIntra),
                count(Source::OspfInter),
                count(Source::OspfExternal)
            );
        }
    }
    let all = totals.0 + totals.1;
    let _ = writeln!(
        out,
        "{:<16}{:<12}{:<12}{:<12}{:<12}{}",
        "internal", 1, "", "", "", 1416
    );
    let _ = writeln!(
        out,
        "{:<16}{:<12}{:<12}{:<12}{:<12}{}",
        "Total",
        totals.0 + 1,
        totals.1,
        0,
        all * 96,
        all * 304 + 1416
    );
    out
}

pub(crate) fn ip_protocols(node: &Node, now: SimTime) -> String {
    let mut out = String::new();
    if node.routes_packets() {
        out.push_str("*** IP Routing is NSF aware ***\n\n");
    }
    out.push_str(&super::protocols::ospf_protocols(node, now));
    out.push_str(&super::protocols::rip_protocols(node, now));
    out
}

pub(crate) fn access_lists(node: &Node, only: Option<&str>) -> String {
    let mut out = String::new();
    for acl in node
        .cfg
        .acls
        .iter()
        .filter(|acl| only.is_none_or(|name| acl.name == name))
    {
        let _ = writeln!(
            out,
            "{} IP access list {}",
            if acl.extended { "Extended" } else { "Standard" },
            acl.name
        );
        for item in &acl.items {
            let AclItem::Rule(rule) = item else { continue };
            let matched = match hits(node, acl, rule.seq) {
                0 => String::new(),
                1 => " (1 match)".to_string(),
                n => format!(" ({n} matches)"),
            };
            let _ = writeln!(
                out,
                "    {} {}{matched}",
                rule.seq,
                rule_show_text(rule, acl.extended)
            );
        }
    }
    out
}
