//! Protected resources: the things a change may not touch without a human
//! typing the device's name.
//!
//! The detector works on the commands that will actually be sent, with the
//! configuration section each one lands in, and compares them against what
//! the device looks like now: which interface carries the address Nocturne
//! reaches it on, which ports have neighbours, which VLANs have users, which
//! access lists guard the management session. It is deliberately
//! case-insensitive, whitespace-insensitive and blind to the order of the
//! commands, because every one of those is a way to hide a line from a
//! simpler check.
//!
//! What it cannot see is written down in [`Survey::notes`] and ends up in the
//! plan's warnings: a device whose management address is not in its own
//! configuration (reached by hostname, or through NAT), a platform whose
//! configuration grammar is not modelled, and neighbours that were never
//! collected.

use std::collections::{BTreeSet, HashMap, HashSet};

use nocturne_app::model::Device;
use nocturne_drivers::hier::{ConfigTree, Node, Syntax, parse_vlan_ranges};
use nocturne_drivers::ifname;
use nocturne_drivers::model::{Interface, Neighbor};

use crate::model::ProtectedHit;

const TRUNK_ALLOWED: &str = "switchport trunk allowed vlan ";

/// Kinds that mean "this can take the device away from us": they are the
/// difference between `high` and `critical`.
pub fn is_management_or_access(kind: &str) -> bool {
    kind.starts_with("management_")
        || kind.starts_with("access_")
        || matches!(kind, "default_route" | "default_gateway" | "ip_routing")
}

/// What is worth protecting on one device, read off its running configuration
/// and its collected data.
#[derive(Clone, Debug, Default)]
pub struct Survey {
    /// Canonical name of the interface carrying the management address.
    pub management_interface: Option<String>,
    pub management_address: String,
    pub management_vlan: Option<u16>,
    pub management_vrf: Option<String>,
    /// Canonical interface name → what is on the other end.
    pub neighbors: HashMap<String, String>,
    /// Interfaces that carry more than one host: trunks, port-channels, routed links.
    pub uplinks: HashMap<String, String>,
    /// Interfaces that are trunks today.
    pub trunks: HashSet<String>,
    /// Allowed-VLAN list of each trunk; `None` = no list, so every VLAN is allowed.
    pub trunk_allowed: HashMap<String, Option<BTreeSet<u16>>>,
    /// Interfaces that exist in the running configuration.
    pub interfaces: HashSet<String>,
    /// VLAN → why removing it would hurt.
    pub used_vlans: HashMap<u16, String>,
    /// Access lists applied to the VTY lines or to the management interface.
    pub guarded_acls: HashSet<String>,
    /// What could not be determined, in words for the plan's warnings.
    pub notes: Vec<String>,
}

fn key(name: &str) -> String {
    ifname::long(name.trim()).to_ascii_lowercase()
}

/// Everything after the first word, trimmed.
fn rest<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix).map(str::trim)
}

/// Lower-case, single-spaced, for matching.
fn norm(line: &str) -> String {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Read a device's running configuration and collected data.
pub fn survey(
    device: &Device,
    running: &ConfigTree,
    syntax: Syntax,
    neighbors: &[Neighbor],
    interfaces: &[Interface],
) -> Survey {
    let mut survey = Survey {
        management_address: device.address.trim().to_ascii_lowercase(),
        ..Survey::default()
    };
    for neighbor in neighbors {
        if neighbor.local_interface.trim().is_empty() {
            continue;
        }
        survey.neighbors.insert(
            key(&neighbor.local_interface),
            format!(
                "{} is on the other end of it",
                if neighbor.device.trim().is_empty() {
                    "another device".to_string()
                } else {
                    neighbor.device.clone()
                }
            ),
        );
    }
    if neighbors.is_empty() {
        survey.notes.push(
            "No LLDP/CDP neighbours have been collected from this device, so Nocturne cannot tell which of its ports are uplinks."
                .into(),
        );
    }

    match syntax {
        Syntax::Ios | Syntax::Vrp => survey_hierarchical(&mut survey, running),
        Syntax::Routeros => survey_routeros(&mut survey, running),
        _ => survey.notes.push(
            "This platform's configuration grammar is not modelled, so Nocturne cannot tell from the configuration which lines protect its own management access."
                .into(),
        ),
    }

    // The configuration may not hold the address at all (reached by hostname,
    // by DHCP, or through NAT); the collected interface data may.
    if survey.management_interface.is_none() {
        let address = survey.management_address.clone();
        if let Some(interface) = interfaces.iter().find(|interface| {
            interface.ipv4.iter().chain(&interface.ipv6).any(|cidr| {
                cidr.split('/')
                    .next()
                    .unwrap_or(cidr)
                    .eq_ignore_ascii_case(&address)
            })
        }) {
            survey.management_interface = Some(key(&interface.name));
            survey.management_vlan = vlan_of(&interface.name);
        }
    }
    if survey.management_interface.is_none() {
        survey.notes.push(format!(
            "Nocturne reaches this device at `{}`, which is not configured on any of its interfaces, so it cannot tell which interface carries its own session; check the change for anything that would cut it off.",
            device.address
        ));
    }
    survey
}

/// `Vlan10` → 10.
fn vlan_of(name: &str) -> Option<u16> {
    let long = ifname::long(name.trim());
    long.strip_prefix("Vlan")?.parse().ok()
}

/// Read an indented configuration (IOS or VRP): which interface carries the
/// management address, which ports are uplinks, which VLANs are in use, and
/// which access lists guard the way in.
fn survey_hierarchical(survey: &mut Survey, running: &ConfigTree) {
    let interface_word = "interface ";
    for node in &running.roots {
        let line = norm(&node.text);
        if let Some(name) = rest(&line, interface_word) {
            let name = key(name);
            survey.interfaces.insert(name.clone());
            if let Some(vlan) = vlan_of(&name) {
                survey
                    .used_vlans
                    .insert(vlan, format!("VLAN {vlan} has a switch interface (SVI)"));
            }
            survey_interface(survey, &name, node);
            continue;
        }
        if line.starts_with("line vty") || line.starts_with("line con") {
            for child in &node.children {
                if let Some(acl) = rest(&norm(&child.text), "access-class ")
                    && let Some(name) = acl.split_whitespace().next()
                {
                    survey.guarded_acls.insert(name.to_string());
                }
            }
        }
    }
}

fn survey_interface(survey: &mut Survey, name: &str, node: &Node) {
    let mut has_address = false;
    for child in &node.children {
        let line = norm(&child.text);
        if let Some(address) = rest(&line, "ip address ")
            .and_then(|rest| rest.split_whitespace().next())
            .filter(|address| *address != "dhcp")
        {
            has_address = true;
            if address == survey.management_address {
                survey.management_interface = Some(name.to_string());
                survey.management_vlan = vlan_of(name);
            }
        }
        if let Some(vrf) =
            rest(&line, "vrf forwarding ").or_else(|| rest(&line, "ip vrf forwarding "))
            && survey.management_interface.as_deref() == Some(name)
        {
            survey.management_vrf = Some(vrf.to_string());
        }
        if line == "switchport mode trunk" {
            survey.trunks.insert(name.to_string());
            survey
                .uplinks
                .insert(name.to_string(), "it is a trunk".into());
        }
        if let Some(list) = rest(&line, TRUNK_ALLOWED) {
            let list = list.strip_prefix("add ").unwrap_or(list);
            let parsed = if list == "all" {
                None
            } else {
                parse_vlan_ranges(list)
            };
            let entry = survey
                .trunk_allowed
                .entry(name.to_string())
                .or_insert(Some(BTreeSet::new()));
            match (entry.as_mut(), parsed) {
                (Some(current), Some(more)) => current.extend(more),
                _ => *entry = None,
            }
            for vlan in parse_vlan_ranges(list).unwrap_or_default() {
                survey
                    .used_vlans
                    .entry(vlan)
                    .or_insert_with(|| format!("VLAN {vlan} is carried by a trunk"));
            }
        }
        if let Some(vlan) = rest(&line, "switchport access vlan ").and_then(|id| id.parse().ok()) {
            survey
                .used_vlans
                .insert(vlan, format!("VLAN {vlan} has access ports"));
        }
        if line.starts_with("channel-group ") {
            survey
                .uplinks
                .insert(name.to_string(), "it is a member of a port-channel".into());
        }
    }
    if name.starts_with("port-channel") {
        survey
            .uplinks
            .insert(name.to_string(), "it is a port-channel".into());
    }
    if has_address {
        survey
            .uplinks
            .entry(name.to_string())
            .or_insert_with(|| "it is a routed interface".into());
    }
    if survey.management_interface.as_deref() == Some(name) {
        for child in &node.children {
            if let Some(acl) = rest(&norm(&child.text), "ip access-group ")
                .and_then(|rest| rest.split_whitespace().next())
            {
                survey.guarded_acls.insert(acl.to_string());
            }
        }
    }
}

fn survey_routeros(survey: &mut Survey, running: &ConfigTree) {
    for node in &running.roots {
        let line = norm(&node.text);
        if line.starts_with("/ip address")
            && let Some(address) = line
                .split_whitespace()
                .find_map(|word| word.strip_prefix("address="))
            && address
                .split('/')
                .next()
                .is_some_and(|address| address == survey.management_address)
        {
            let interface = line
                .split_whitespace()
                .find_map(|word| word.strip_prefix("interface="))
                .unwrap_or("the management interface");
            survey.management_interface = Some(interface.to_string());
        }
    }
}

// ------------------------------------------------------------------ walking

/// One command with the sections it lands in (several for `interface range`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    /// Section headers, lower-cased; empty at the top level. More than one
    /// when the section is an `interface range`.
    pub sections: Vec<String>,
    /// The command, lower-cased and single-spaced.
    pub line: String,
    /// The command as it will be sent.
    pub raw: String,
}

/// Attribute every command to the section it will land in, whatever tricks
/// were played with the order, the case or the spacing.
pub fn locate(commands: &[String], syntax: Syntax) -> Vec<Located> {
    let mut located = Vec::with_capacity(commands.len());
    let mut sections: Vec<String> = Vec::new();
    for raw in commands {
        let line = norm(raw);
        if line.is_empty() || line.starts_with('!') {
            continue;
        }
        if line == "exit" || line == "quit" || line == "end" || line == "return" {
            sections.clear();
            located.push(Located {
                sections: Vec::new(),
                line,
                raw: raw.trim().to_string(),
            });
            continue;
        }
        if let Some(range) = rest(&line, "interface range ") {
            sections = expand_range(range)
                .into_iter()
                .map(|name| format!("interface {name}"))
                .collect();
            located.push(Located {
                sections: sections.clone(),
                line,
                raw: raw.trim().to_string(),
            });
            continue;
        }
        if nocturne_drivers::hier::opens_section(&line, syntax) {
            // A nested section (`address-family` under `router bgp`) is added
            // to the stack; anything else starts a new one. Either way the
            // rules below run for every section the line is inside.
            if nocturne_drivers::hier::opens_nested(&line, syntax) && !sections.is_empty() {
                sections.push(line.clone());
            } else {
                sections = vec![line.clone()];
            }
        }
        located.push(Located {
            sections: sections.clone(),
            line,
            raw: raw.trim().to_string(),
        });
    }
    located
}

/// `gi0/1 - 4` / `gi0/1-4, gi0/9` → the ports it names.
fn expand_range(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for part in text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let part = part
            .replace(" - ", "-")
            .replace(" -", "-")
            .replace("- ", "-");
        match part.rsplit_once('-') {
            Some((first, last)) if last.chars().all(|c| c.is_ascii_digit()) => {
                let head = first.trim();
                names.push(head.to_string());
                let (prefix, start) = head.split_at(
                    head.rfind(|c: char| !c.is_ascii_digit())
                        .map(|at| at + 1)
                        .unwrap_or(0),
                );
                if let (Ok(start), Ok(last)) = (start.parse::<u32>(), last.parse::<u32>())
                    && last > start
                    && last - start < 256
                {
                    for index in start + 1..=last {
                        names.push(format!("{prefix}{index}"));
                    }
                }
            }
            _ => names.push(part.to_string()),
        }
    }
    names
}

// ----------------------------------------------------------------- detecting

/// Every protected resource the commands would touch.
pub fn detect(
    device: &Device,
    survey: &Survey,
    commands: &[String],
    syntax: Syntax,
) -> Vec<ProtectedHit> {
    let mut hits: Vec<ProtectedHit> = Vec::new();
    if device.protected {
        hits.push(ProtectedHit {
            kind: "protected_device".into(),
            subject: device.name.clone(),
            command: String::new(),
            why: format!(
                "`{}` is marked as a protected device: every change to it needs a human to confirm it by name.",
                device.name
            ),
        });
    }
    for item in locate(commands, syntax) {
        if syntax == Syntax::Routeros {
            routeros_rules(survey, &item, &mut hits);
            continue;
        }
        if item.sections.is_empty() {
            global_rules(survey, &item, &mut hits);
        }
        for section in &item.sections {
            section_rules(survey, section, &item, &mut hits);
        }
    }
    hits.dedup_by(|a, b| a.kind == b.kind && a.subject == b.subject && a.command == b.command);
    hits
}

fn push(hits: &mut Vec<ProtectedHit>, kind: &str, subject: &str, item: &Located, why: String) {
    hits.push(ProtectedHit {
        kind: kind.to_string(),
        subject: subject.to_string(),
        command: item.raw.clone(),
        why,
    });
}

/// The negated form of a line, and the line itself with the negation removed.
fn negation(line: &str) -> (bool, &str) {
    match line
        .strip_prefix("no ")
        .or_else(|| line.strip_prefix("undo "))
    {
        Some(rest) => (true, rest.trim()),
        None => (false, line),
    }
}

fn global_rules(survey: &Survey, item: &Located, hits: &mut Vec<ProtectedHit>) {
    let (negated, bare) = negation(&item.line);
    if bare.starts_with("ip route 0.0.0.0 0.0.0.0") || bare.starts_with("ipv6 route ::/0") {
        push(
            hits,
            "default_route",
            "the default route",
            item,
            format!(
                "This {} the default route. Everything Nocturne reaches through it — including this device, if its session is routed — depends on it.",
                if negated { "removes" } else { "changes" }
            ),
        );
    }
    if bare.starts_with("ip default-gateway") {
        push(
            hits,
            "default_gateway",
            "the default gateway",
            item,
            "This changes the default gateway, which is how a switch without routing answers anything outside its own subnet — including Nocturne.".into(),
        );
    }
    if negated && (bare == "ip routing" || bare.starts_with("ip routing")) {
        push(
            hits,
            "ip_routing",
            "routing",
            item,
            "This turns routing off: every address the device does not have a direct interface on becomes unreachable.".into(),
        );
    }
    for (prefix, kind, what) in [
        ("username ", "access_user", "a login account"),
        ("enable secret", "access_enable", "the enable secret"),
        ("enable password", "access_enable", "the enable password"),
        ("aaa ", "access_aaa", "how logins are authenticated"),
        ("ip ssh ", "access_ssh", "the SSH server"),
        ("crypto key", "access_crypto", "the SSH host key"),
        ("ip http ", "access_http", "the web management server"),
    ] {
        if bare.starts_with(prefix) {
            push(
                hits,
                kind,
                what,
                item,
                format!("This changes {what}, which is how Nocturne logs in to this device."),
            );
        }
    }
    if let Some(acl) = rest(bare, "ip access-list ")
        .and_then(|rest| rest.split_whitespace().nth(1))
        .filter(|name| survey.guarded_acls.contains(*name))
    {
        push(
            hits,
            "access_acl",
            acl,
            item,
            format!(
                "The access list `{acl}` is applied to the management session; changing its entries can lock Nocturne out."
            ),
        );
    }
    if let Some(number) = rest(bare, "access-list ")
        .and_then(|rest| rest.split_whitespace().next())
        .filter(|number| survey.guarded_acls.contains(*number))
    {
        push(
            hits,
            "access_acl",
            number,
            item,
            format!(
                "Access list {number} is applied to the management session; changing its entries can lock Nocturne out."
            ),
        );
    }
    if negated && let Some(vlan) = rest(bare, "vlan ").and_then(|id| id.parse::<u16>().ok()) {
        if let Some(why) = survey.used_vlans.get(&vlan) {
            push(
                hits,
                "vlan_removal",
                &format!("VLAN {vlan}"),
                item,
                format!("{why}; removing it takes them off the network."),
            );
        }
        if survey.management_vlan == Some(vlan) {
            push(
                hits,
                "management_vlan",
                &format!("VLAN {vlan}"),
                item,
                format!("VLAN {vlan} carries the address Nocturne reaches this device on."),
            );
        }
    }
    if bare.starts_with("spanning-tree mode") || bare.starts_with("spanning-tree vlan") {
        push(
            hits,
            "spanning_tree",
            "spanning tree",
            item,
            "Changing the spanning-tree mode or priority re-converges the topology: links block and unblock across the whole domain.".into(),
        );
    }
    if negated && bare.starts_with("router ") {
        push(
            hits,
            "routing_removal",
            bare,
            item,
            format!(
                "This removes the routing process `{bare}` and every route it learned or advertised."
            ),
        );
    }
    if bare.starts_with("hostname ") || bare.starts_with("sysname ") {
        push(
            hits,
            "hostname",
            bare.split_whitespace().nth(1).unwrap_or(bare),
            item,
            "Renaming the device changes its prompt in the middle of the session; Nocturne follows it, but anything matching on the old name (monitoring, syslog, other tools) will not.".into(),
        );
    }
    // Removing or defaulting an interface from the top level.
    let defaulted = rest(&item.line, "default interface ");
    let removed = negated.then(|| rest(bare, "interface ")).flatten();
    if let Some(name) = defaulted.or(removed) {
        let interface = key(name);
        interface_hits(survey, &interface, item, hits, true);
    }
}

fn section_rules(survey: &Survey, section: &str, item: &Located, hits: &mut Vec<ProtectedHit>) {
    if let Some(name) = rest(section, "interface ") {
        let interface = key(name);
        if item.line == *section {
            return;
        }
        interface_section_rules(survey, &interface, item, hits);
        return;
    }
    if section.starts_with("line vty") || section.starts_with("line con") {
        if item.line == *section {
            return;
        }
        push(
            hits,
            "access_vty",
            section,
            item,
            format!(
                "`{section}` is how Nocturne (and everybody else) logs in; changing what is configured there can close the door behind this change."
            ),
        );
        return;
    }
    if let Some(name) = rest(section, "ip access-list ")
        .and_then(|rest| rest.split_whitespace().nth(1))
        .filter(|name| survey.guarded_acls.contains(*name))
        && item.line != *section
    {
        push(
            hits,
            "access_acl",
            name,
            item,
            format!(
                "The access list `{name}` guards the management session; this line changes what it lets through."
            ),
        );
        return;
    }
    if section.starts_with("router ") && item.line != *section {
        let (negated, bare) = negation(&item.line);
        if negated && (bare.starts_with("network ") || bare.starts_with("neighbor ")) {
            push(
                hits,
                "routing_removal",
                section,
                item,
                format!("This removes `{bare}` from `{section}`: the routes it carried disappear."),
            );
        }
    }
}

fn interface_section_rules(
    survey: &Survey,
    interface: &str,
    item: &Located,
    hits: &mut Vec<ProtectedHit>,
) {
    let (negated, bare) = negation(&item.line);
    let is_management = survey.management_interface.as_deref() == Some(interface);
    let pretty = ifname::long(interface);

    if is_management
        && (bare == "shutdown"
            || bare.starts_with("ip address")
            || bare.starts_with("vrf forwarding")
            || bare.starts_with("ip vrf forwarding")
            || bare.starts_with("switchport")
            || bare.starts_with("ip access-group"))
    {
        push(
            hits,
            "management_interface",
            &pretty,
            item,
            format!(
                "`{pretty}` carries the address `{}` Nocturne reaches this device on; this line changes it.",
                survey.management_address
            ),
        );
    }
    if !negated && bare == "shutdown" {
        interface_hits(survey, interface, item, hits, false);
    }
    if bare.starts_with(TRUNK_ALLOWED.trim_end()) {
        trunk_rules(survey, interface, item, negated, bare, hits);
    }
    if is_management && bare.starts_with("ip access-group") {
        // Already reported above as a management-interface hit.
    }
}

/// Shutting down, removing or defaulting an interface that matters.
fn interface_hits(
    survey: &Survey,
    interface: &str,
    item: &Located,
    hits: &mut Vec<ProtectedHit>,
    removal: bool,
) {
    let pretty = ifname::long(interface);
    let action = if removal {
        "removes the configuration of"
    } else {
        "shuts down"
    };
    if survey.management_interface.as_deref() == Some(interface) {
        push(
            hits,
            "management_interface",
            &pretty,
            item,
            format!(
                "This {action} `{pretty}`, which carries the address `{}` Nocturne reaches this device on.",
                survey.management_address
            ),
        );
    }
    if let Some(why) = survey.neighbors.get(interface) {
        push(
            hits,
            "uplink",
            &pretty,
            item,
            format!("This {action} `{pretty}` — {why}."),
        );
    } else if let Some(why) = survey.uplinks.get(interface) {
        push(
            hits,
            "uplink",
            &pretty,
            item,
            format!(
                "This {action} `{pretty}` — {why}, so it probably carries other devices' traffic."
            ),
        );
    }
}

fn trunk_rules(
    survey: &Survey,
    interface: &str,
    item: &Located,
    negated: bool,
    bare: &str,
    hits: &mut Vec<ProtectedHit>,
) {
    let pretty = ifname::long(interface);
    let is_trunk =
        survey.trunks.contains(interface) || survey.trunk_allowed.contains_key(interface);
    if !is_trunk {
        return;
    }
    let current = survey.trunk_allowed.get(interface).cloned().flatten();
    if negated {
        push(
            hits,
            "trunk_allowed",
            &pretty,
            item,
            format!(
                "This removes the allowed-VLAN list of the trunk `{pretty}`, so it goes back to carrying every VLAN."
            ),
        );
        return;
    }
    let Some(list) = rest(bare, TRUNK_ALLOWED) else {
        return;
    };
    if let Some(removed) = rest(list, "remove ") {
        let removed = parse_vlan_ranges(removed).unwrap_or_default();
        if let Some(vlan) = survey.management_vlan.filter(|vlan| removed.contains(vlan)) {
            push(
                hits,
                "management_vlan",
                &pretty,
                item,
                format!(
                    "This takes VLAN {vlan} — the VLAN Nocturne reaches this device on — off the trunk `{pretty}`."
                ),
            );
        }
        if survey.neighbors.contains_key(interface) {
            push(
                hits,
                "trunk_allowed",
                &pretty,
                item,
                format!(
                    "This takes VLAN(s) {} off the trunk `{pretty}`, which has a neighbour: anything in them stops passing over that link.",
                    removed
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        return;
    }
    if list.starts_with("add ") {
        return;
    }
    // A bare list replaces the whole thing. This is the one that black-holes
    // every VLAN that is not in it.
    let wanted = if list == "none" {
        Some(BTreeSet::new())
    } else if list == "all" {
        None
    } else {
        parse_vlan_ranges(list)
    };
    let narrows = match (&current, &wanted) {
        // The trunk carries every VLAN today: any list at all takes VLANs away.
        (None, _) => true,
        (Some(_), None) => false,
        (Some(current), Some(wanted)) => !current.is_subset(wanted),
    };
    if narrows {
        let lost = match (&current, &wanted) {
            (Some(current), Some(wanted)) => current
                .difference(wanted)
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            _ => "every VLAN not in the list".to_string(),
        };
        push(
            hits,
            "trunk_allowed_replace",
            &pretty,
            item,
            format!(
                "`{}` replaces the allowed-VLAN list of the trunk `{pretty}` instead of editing it: {lost} would stop passing over that link the moment it is sent. Use `switchport trunk allowed vlan add …` / `… remove …` instead.",
                item.raw
            ),
        );
    }
}

fn routeros_rules(survey: &Survey, item: &Located, hits: &mut Vec<ProtectedHit>) {
    let line = &item.line;
    for (prefix, kind, what) in [
        (
            "/ip service",
            "access_service",
            "the management services (SSH, API, Winbox)",
        ),
        ("/user", "access_user", "the login accounts"),
        ("/ip ssh", "access_ssh", "the SSH server"),
    ] {
        if line.starts_with(prefix) {
            push(
                hits,
                kind,
                what,
                item,
                format!("This changes {what}, which is how Nocturne logs in to this router."),
            );
        }
    }
    if line.starts_with("/ip firewall filter") && line.contains("chain=input") {
        push(
            hits,
            "access_firewall",
            "the input firewall chain",
            item,
            "This changes the firewall chain that decides who may reach the router itself.".into(),
        );
    }
    if line.starts_with("/ip route") && line.contains("dst-address=0.0.0.0/0") {
        push(
            hits,
            "default_route",
            "the default route",
            item,
            "This changes the router's default route.".into(),
        );
    }
    if line.starts_with("/ip address") && line.contains(&survey.management_address) {
        push(
            hits,
            "management_interface",
            survey
                .management_interface
                .as_deref()
                .unwrap_or("the management address"),
            item,
            format!(
                "This line touches `{}`, the address Nocturne reaches this router on.",
                survey.management_address
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = "\
hostname SW1
vlan 10
 name users
vlan 20
 name voice
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
 switchport trunk allowed vlan 10,20
interface GigabitEthernet0/2
 switchport mode access
 switchport access vlan 10
interface Vlan1
 ip address 10.0.0.2 255.255.255.0
 ip access-group MGMT in
ip default-gateway 10.0.0.1
ip access-list extended MGMT
 permit ip 10.9.0.0 0.0.0.255 any
line vty 0 4
 access-class VTY in
 transport input ssh
";

    fn device() -> Device {
        Device {
            id: "dev1".into(),
            name: "SW1".into(),
            address: "10.0.0.2".into(),
            ..Device::default()
        }
    }

    fn surveyed() -> Survey {
        let tree = ConfigTree::parse(RUNNING, Syntax::Ios);
        survey(
            &device(),
            &tree,
            Syntax::Ios,
            &[Neighbor {
                local_interface: "GigabitEthernet0/1".into(),
                device: "CORE1".into(),
                ..Neighbor::default()
            }],
            &[],
        )
    }

    fn kinds(lines: &[&str]) -> Vec<String> {
        let commands: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        detect(&device(), &surveyed(), &commands, Syntax::Ios)
            .into_iter()
            .map(|hit| hit.kind)
            .collect()
    }

    #[test]
    fn the_survey_finds_what_matters_on_this_device() {
        let survey = surveyed();
        assert_eq!(survey.management_interface.as_deref(), Some("vlan1"));
        assert_eq!(survey.management_vlan, Some(1));
        assert!(survey.trunks.contains("gigabitethernet0/1"));
        assert!(survey.neighbors.contains_key("gigabitethernet0/1"));
        assert!(survey.uplinks.contains_key("vlan1"), "a routed interface");
        assert_eq!(
            survey.trunk_allowed.get("gigabitethernet0/1"),
            Some(&Some([10u16, 20].into_iter().collect()))
        );
        assert!(survey.used_vlans.contains_key(&10) && survey.used_vlans.contains_key(&20));
        // Names are compared in lower case: `VTY` and `vty` are different
        // access lists on IOS, but treating them as one can only over-protect.
        assert!(survey.guarded_acls.contains("vty") && survey.guarded_acls.contains("mgmt"));
        assert!(survey.notes.is_empty(), "{:?}", survey.notes);
    }

    #[test]
    fn a_device_whose_address_is_nowhere_in_its_configuration_says_so() {
        let mut device = device();
        device.address = "switch.example.com".into();
        let tree = ConfigTree::parse(RUNNING, Syntax::Ios);
        let survey = survey(&device, &tree, Syntax::Ios, &[], &[]);
        assert!(survey.management_interface.is_none());
        assert!(
            survey
                .notes
                .iter()
                .any(|note| note.contains("not configured on any of its interfaces")),
            "{:?}",
            survey.notes
        );
        assert!(
            survey
                .notes
                .iter()
                .any(|note| note.contains("No LLDP/CDP neighbours")),
            "{:?}",
            survey.notes
        );
    }

    #[test]
    fn every_way_of_writing_the_same_dangerous_line_is_caught() {
        // Case, spacing and section distance make no difference.
        for lines in [
            vec!["interface Vlan1", "shutdown"],
            vec!["INTERFACE VLAN1", "SHUTDOWN"],
            vec!["interface   vlan1", "   shutdown  "],
            vec!["interface Vlan1", "description x", "shutdown"],
            vec![
                "interface GigabitEthernet0/2",
                "description a quiet port",
                "exit",
                "interface Vlan1",
                "shutdown",
            ],
            vec!["interface range Vlan1 , GigabitEthernet0/2", "shutdown"],
            vec!["no interface Vlan1"],
            vec!["default interface Vlan1"],
        ] {
            assert!(
                kinds(&lines).contains(&"management_interface".to_string()),
                "{lines:?} → {:?}",
                kinds(&lines)
            );
        }
    }

    #[test]
    fn a_replacing_trunk_list_is_caught_but_a_delta_is_not() {
        let narrowing = kinds(&[
            "interface GigabitEthernet0/1",
            "switchport trunk allowed vlan 10",
        ]);
        assert!(narrowing.contains(&"trunk_allowed_replace".to_string()));

        // Adding and removing explicitly is the safe form.
        assert!(
            !kinds(&[
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan add 30",
            ])
            .contains(&"trunk_allowed_replace".to_string())
        );
        // A list that only widens takes nothing away.
        assert!(
            !kinds(&[
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan 10,20,30",
            ])
            .contains(&"trunk_allowed_replace".to_string())
        );
        // Removing VLANs from a trunk with a neighbour is still worth a look.
        assert!(
            kinds(&[
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan remove 20",
            ])
            .contains(&"trunk_allowed".to_string())
        );
        // A port that is not a trunk today cannot black-hole anything.
        assert!(
            !kinds(&[
                "interface GigabitEthernet0/2",
                "switchport mode trunk",
                "switchport trunk allowed vlan 10",
            ])
            .contains(&"trunk_allowed_replace".to_string())
        );
    }

    #[test]
    fn the_way_in_is_protected_including_the_access_lists_that_guard_it() {
        for (lines, expected) in [
            (vec!["line vty 0 4", "transport input telnet"], "access_vty"),
            (
                vec!["ip access-list extended VTY", "10 deny ip any any"],
                "access_acl",
            ),
            (vec!["ip access-list extended MGMT", "no 10"], "access_acl"),
            (vec!["no username admin"], "access_user"),
            (vec!["enable secret 0 hunter2"], "access_enable"),
            (vec!["aaa new-model"], "access_aaa"),
            (vec!["no ip ssh version 2"], "access_ssh"),
            (vec!["crypto key zeroize rsa"], "access_crypto"),
            (vec!["no ip default-gateway 10.0.0.1"], "default_gateway"),
            (vec!["no ip routing"], "ip_routing"),
            (vec!["no router ospf 1"], "routing_removal"),
            (
                vec!["router ospf 1", "no network 10.0.0.0 0.0.0.255 area 0"],
                "routing_removal",
            ),
            (vec!["no vlan 10"], "vlan_removal"),
            (vec!["spanning-tree vlan 10 priority 4096"], "spanning_tree"),
            (vec!["hostname OTHER"], "hostname"),
        ] {
            assert!(
                kinds(&lines).contains(&expected.to_string()),
                "{lines:?} should be `{expected}`, got {:?}",
                kinds(&lines)
            );
        }
        // An access list nobody has applied to the way in is ordinary work.
        assert!(kinds(&["ip access-list extended GUESTS", "permit ip any any"]).is_empty());
    }

    #[test]
    fn ordinary_access_port_work_hits_nothing() {
        assert!(
            kinds(&[
                "interface GigabitEthernet0/2",
                "description desk 14",
                "switchport access vlan 20",
                "exit",
                "vlan 30",
                "name guests",
                "exit",
                "ntp server 192.0.2.1",
            ])
            .is_empty()
        );
    }

    #[test]
    fn an_interface_range_is_expanded_and_nested_sections_keep_their_parent() {
        let located = locate(
            &[
                "interface range Gi0/1 - 3, Gi0/9".to_string(),
                "shutdown".into(),
            ],
            Syntax::Ios,
        );
        assert_eq!(
            located[1].sections,
            vec![
                "interface gi0/1",
                "interface gi0/2",
                "interface gi0/3",
                "interface gi0/9"
            ]
        );
        let nested = locate(
            &[
                "router bgp 65001".to_string(),
                "address-family ipv4".into(),
                "no network 10.0.0.0 mask 255.255.255.0".into(),
            ],
            Syntax::Ios,
        );
        assert_eq!(
            nested[2].sections,
            vec!["router bgp 65001", "address-family ipv4"]
        );
    }
}
