//! Helpers shared by the test suites: boot a design, prove its checks, and
//! assert the invariants every design has to satisfy.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use nocturne_design::{
    Addressing, Design, Expect, InternetEdge, LinkKind, Redundancy, Requirements, Routing,
    Services, Site, WanTopology, generate,
};
use nocturne_netsim::{Lab, LabSpec, catalog};

pub const SECOND: u64 = 1_000_000;
/// Long enough for OSPF, HSRP and spanning tree on the biggest lab here, and
/// cheap when the control plane has already gone quiet.
pub const SETTLE: u64 = 240 * SECOND;

/// Boot a design, requiring that every configuration line was accepted.
pub fn boot(design: &Design, label: &str) -> Lab {
    let mut lab = Lab::from_spec(&design.lab)
        .unwrap_or_else(|e| panic!("{label}: the lab would not load: {e}"));
    let mut rejected: Vec<String> = Vec::new();
    for node in lab.snapshot().nodes {
        for (line, why) in lab.startup_errors(node.id) {
            rejected.push(format!("{}: {line} -> {}", node.name, why.trim()));
        }
    }
    assert!(
        rejected.is_empty(),
        "{label}: the simulator rejected {} configuration line(s):\n{}",
        rejected.len(),
        rejected.join("\n")
    );
    lab.settle(SETTLE);
    lab
}

/// Probe every check the design declares. Returns the failures.
pub fn failures(design: &Design, lab: &mut Lab) -> Vec<String> {
    let mut out = Vec::new();
    for check in &design.plan.checks {
        match probe_detail(lab, &check.from, &check.to, check.port) {
            Ok((ok, reason)) if ok != (check.expect == Expect::Reachable) => out.push(format!(
                "{} -> {} ({}) {}: expected {:?}, got {}{}",
                check.from,
                check.to,
                check.to_name,
                match check.port {
                    Some(p) => format!("tcp/{p}"),
                    None => "ping".to_string(),
                },
                check.expect,
                if ok { "reachable" } else { "blocked" },
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({reason})")
                }
            )),
            Err(problem) => out.push(problem),
            _ => {}
        }
    }
    out
}

/// `Ok((reachable, reason))`, or a complaint about the check itself.
pub fn probe(
    lab: &mut Lab,
    from: &str,
    to: &str,
    port: Option<u16>,
) -> Result<(bool, String), String> {
    probe_detail(lab, from, to, port)
}

/// The same, with the path the probe took appended to the reason.
pub fn probe_detail(
    lab: &mut Lab,
    from: &str,
    to: &str,
    port: Option<u16>,
) -> Result<(bool, String), String> {
    let node = lab
        .node_by_name(from)
        .ok_or_else(|| format!("no node called {from} in the lab"))?;
    let addr: Ipv4Addr = to
        .parse()
        .map_err(|_| format!("{to} is not an address ({from} -> {to})"))?;
    let result = match port {
        Some(port) => lab.probe_tcp(node, addr, port),
        None => lab.probe_ping(node, addr),
    };
    let mut reason = result.reason.clone();
    if !result.ok && !result.hops.is_empty() {
        reason.push_str("\n      path:");
        for hop in &result.hops {
            reason.push_str(&format!(
                "\n        {} in={} out={}: {}",
                hop.name, hop.in_if, hop.out_if, hop.note
            ));
        }
    }
    Ok((result.ok, reason))
}

/// Everything that has to be true of any design, whatever was asked for.
pub fn assert_invariants(design: &Design, label: &str) {
    let plan = &design.plan;

    // -- the lab is loadable in principle: names and interfaces exist --------
    let models: BTreeMap<String, Vec<String>> = catalog()
        .into_iter()
        .map(|m| (m.id, m.interfaces))
        .collect();
    let mut names: Vec<&str> = design.lab.nodes.iter().map(|n| n.name.as_str()).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "{label}: two nodes share a name");
    for node in &design.lab.nodes {
        assert!(
            !node.name.is_empty()
                && node.name.len() <= 63
                && node
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
            "{label}: {:?} is not a usable node name",
            node.name
        );
        let interfaces = models
            .get(&node.model)
            .unwrap_or_else(|| panic!("{label}: {} has no catalogue model", node.name));
        for link in &design.lab.links {
            for end in [&link.a, &link.b] {
                if end.node == node.name {
                    assert!(
                        interfaces.contains(&end.interface),
                        "{label}: {} has no interface {}",
                        node.name,
                        end.interface
                    );
                }
            }
        }
    }

    // -- one cable per port ------------------------------------------------
    let mut ends: Vec<(String, String)> = Vec::new();
    for link in &design.lab.links {
        for end in [&link.a, &link.b] {
            let key = (end.node.clone(), end.interface.clone());
            assert!(
                !ends.contains(&key),
                "{label}: {} {} is cabled twice",
                end.node,
                end.interface
            );
            ends.push(key);
        }
        assert_ne!(link.a.node, link.b.node, "{label}: a cable loops on itself");
    }

    // -- no two subnets overlap --------------------------------------------
    let mut blocks: Vec<(String, Net)> = Vec::new();
    for vlan in &plan.vlans {
        blocks.push((
            format!("VLAN {} at {}", vlan.id, vlan.site),
            Net::parse(&vlan.subnet)
                .unwrap_or_else(|| panic!("{label}: {} is not a block", vlan.subnet)),
        ));
    }
    for link in &plan.links {
        if link.subnet.is_empty() || link.kind == LinkKind::Internet {
            continue;
        }
        blocks.push((
            format!("the link {} - {}", link.a, link.b),
            Net::parse(&link.subnet)
                .unwrap_or_else(|| panic!("{label}: {} is not a block", link.subnet)),
        ));
    }
    for i in 0..blocks.len() {
        for j in (i + 1)..blocks.len() {
            assert!(
                !blocks[i].1.overlaps(blocks[j].1),
                "{label}: {} ({}) overlaps {} ({})",
                blocks[i].0,
                blocks[i].1,
                blocks[j].0,
                blocks[j].1
            );
        }
    }

    // -- no address is used twice -------------------------------------------
    let mut used: BTreeMap<Ipv4Addr, String> = BTreeMap::new();
    let mut claim = |addr: Ipv4Addr, who: String| {
        if let Some(other) = used.get(&addr) {
            panic!("{label}: {addr} is used by both {other} and {who}");
        }
        used.insert(addr, who);
    };
    for dev in &plan.devices {
        for iface in &dev.interfaces {
            if iface.ip.is_empty() {
                continue;
            }
            let Some((addr, _)) = iface.ip.split_once('/') else {
                continue;
            };
            let Ok(addr) = addr.parse::<Ipv4Addr>() else {
                continue;
            };
            // A device's own ports and its SVIs are listed once each, but a
            // host's single port repeats its address, so skip those.
            if iface.mode == "host" {
                continue;
            }
            claim(addr, format!("{} {}", dev.name, iface.name));
        }
    }
    for node in &design.lab.nodes {
        let Some(host) = &node.host else { continue };
        if let Some(ip) = &host.ip
            && let Ok(addr) = ip.parse::<Ipv4Addr>()
        {
            claim(addr, node.name.clone());
        }
    }

    // -- the two ends of every routed link share its subnet ------------------
    let by_name: BTreeMap<&str, &nocturne_design::DevicePlan> =
        plan.devices.iter().map(|d| (d.name.as_str(), d)).collect();
    for link in &plan.links {
        let (Some(a), Some(b)) = (by_name.get(link.a.as_str()), by_name.get(link.b.as_str()))
        else {
            panic!(
                "{label}: the link {} - {} names a device that is not in the plan",
                link.a, link.b
            );
        };
        let find = |dev: &nocturne_design::DevicePlan, iface: &str| {
            dev.interfaces.iter().find(|i| i.name == iface).cloned()
        };
        let (Some(pa), Some(pb)) = (find(a, &link.a_if), find(b, &link.b_if)) else {
            panic!(
                "{label}: the link {} {} - {} {} is missing from one end's interface list",
                link.a, link.a_if, link.b, link.b_if
            );
        };
        assert!(
            !pa.description.is_empty() && !pb.description.is_empty(),
            "{label}: the link {} {} - {} {} has an end with no description",
            link.a,
            link.a_if,
            link.b,
            link.b_if
        );
        if link.subnet.is_empty() {
            match link.kind {
                LinkKind::Host => assert!(
                    pb.mode.starts_with("access") || pb.mode == "host",
                    "{label}: {} {} should be an access port, it is {:?}",
                    link.b,
                    link.b_if,
                    pb.mode
                ),
                _ => assert!(
                    pa.mode.starts_with("trunk") && pb.mode.starts_with("trunk"),
                    "{label}: the switched link {} {} - {} {} is not a trunk at both ends \
                     ({:?} / {:?})",
                    link.a,
                    link.a_if,
                    link.b,
                    link.b_if,
                    pa.mode,
                    pb.mode
                ),
            }
            continue;
        }
        let net = Net::parse(&link.subnet).expect("checked above");
        for (dev, port) in [(&link.a, &pa), (&link.b, &pb)] {
            let Some((addr, _)) = port.ip.split_once('/') else {
                if link.kind == LinkKind::Internet {
                    continue;
                }
                panic!(
                    "{label}: {dev} {} has no address on the routed link {}",
                    port.name, link.subnet
                );
            };
            let addr: Ipv4Addr = addr.parse().expect("an address");
            assert!(
                net.contains(u32::from(addr)),
                "{label}: {dev} {} is {addr}, outside the link subnet {net}",
                port.name
            );
        }
    }

    // -- the plan agrees with itself ----------------------------------------
    for vlan in &plan.vlans {
        let net = Net::parse(&vlan.subnet).expect("a block");
        let gateway: Ipv4Addr = vlan.gateway.parse().expect("an address");
        assert!(
            net.contains(u32::from(gateway)),
            "{label}: the gateway {gateway} of VLAN {} is outside {net}",
            vlan.id
        );
        assert!(
            vlan.hosts_available >= vlan.hosts_needed,
            "{label}: VLAN {} at {} is sized for {} hosts but holds {}",
            vlan.id,
            vlan.site,
            vlan.hosts_needed,
            vlan.hosts_available
        );
        if !vlan.dhcp_first.is_empty() {
            for addr in [&vlan.dhcp_first, &vlan.dhcp_last] {
                let addr: Ipv4Addr = addr.parse().expect("an address");
                assert!(
                    net.contains(u32::from(addr)),
                    "{label}: the DHCP pool of VLAN {} leaves {net}",
                    vlan.id
                );
            }
        }
    }
    for dev in &plan.devices {
        assert!(
            design.lab.nodes.iter().any(|n| n.name == dev.name),
            "{label}: {} is in the plan but not in the lab",
            dev.name
        );
    }

    // -- no secret may look like a secret ------------------------------------
    assert_no_real_secrets(design, label);
}

/// Nothing in a generated configuration may be mistakable for a password
/// somebody might keep.
pub fn assert_no_real_secrets(design: &Design, label: &str) {
    for node in &design.lab.nodes {
        let Some(config) = &node.startup_config else {
            continue;
        };
        for line in config.lines() {
            let trimmed = line.trim();
            for keyword in ["secret ", "password ", "community "] {
                let Some(rest) = trimmed.split_once(keyword).map(|(_, r)| r) else {
                    continue;
                };
                if trimmed.starts_with("ip access-list") || trimmed.starts_with("remark") {
                    continue;
                }
                let value = rest.split_whitespace().next().unwrap_or("");
                assert!(
                    value.starts_with("CHANGEME-"),
                    "{label}: {} has a secret that does not announce itself as a placeholder: \
                     {trimmed:?}",
                    node.name
                );
            }
        }
    }
}

/// A tiny IPv4 block, so the tests do their own arithmetic rather than
/// trusting the crate's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Net {
    pub addr: u32,
    pub len: u8,
}

impl Net {
    pub fn parse(text: &str) -> Option<Net> {
        let (addr, len) = text.split_once('/')?;
        let addr: Ipv4Addr = addr.parse().ok()?;
        let len: u8 = len.parse().ok()?;
        (len <= 32).then_some(Net {
            addr: u32::from(addr),
            len,
        })
    }
    pub fn last(self) -> u32 {
        if self.len == 0 {
            u32::MAX
        } else {
            self.addr | (u32::MAX >> self.len)
        }
    }
    pub fn contains(self, addr: u32) -> bool {
        addr >= self.addr && addr <= self.last()
    }
    pub fn overlaps(self, other: Net) -> bool {
        self.addr <= other.last() && other.addr <= self.last()
    }
}

impl std::fmt::Display for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", Ipv4Addr::from(self.addr), self.len)
    }
}

// ------------------------------------------------------------------ matrix

/// One cell of the requirements matrix.
pub struct Case {
    pub label: String,
    pub requirements: Requirements,
    /// Generate twice and compare. Done on a quarter of the matrix; the
    /// dedicated determinism tests cover the rest more cheaply.
    pub check_determinism: bool,
}

/// The matrix the specification asks for: 1-4 sites × every redundancy level ×
/// every WAN topology × static and OSPF × voice and guest on and off, with
/// user counts from a handful to a couple of thousand.
///
/// `shard` selects every `shards`-th case so the suite can run them in
/// parallel test functions.
pub fn matrix(shard: usize, shards: usize) -> Vec<Case> {
    // A site with thousands of users needs a hundred access switches, and the
    // probes run on a copy of the whole network, so cost grows with the
    // square of the design. The structural cross product is what matters, so
    // the biggest head-counts go to the designs with the fewest sites: one
    // site reaches two thousand users, four sites reach sixty each.
    const USERS: [&[u32]; 4] = [
        &[5, 30, 120, 400, 2000],
        &[5, 30, 120, 300],
        &[5, 25, 90],
        &[5, 20, 60],
    ];
    let mut cases = Vec::new();
    let mut n = 0usize;
    for sites in 1..=4usize {
        for redundancy in [Redundancy::None, Redundancy::Uplinks, Redundancy::Full] {
            for topology in [
                WanTopology::None,
                WanTopology::HubAndSpoke,
                WanTopology::FullMesh,
            ] {
                for routing in [Routing::Static, Routing::Ospf] {
                    for (voice, guest) in
                        [(false, false), (true, false), (false, true), (true, true)]
                    {
                        let k = n;
                        n += 1;
                        if sites == 1 && topology != WanTopology::None {
                            continue; // one site cannot have a WAN
                        }
                        let choices = USERS[sites - 1];
                        let users = choices[k % choices.len()];
                        let internet = if topology == WanTopology::None {
                            [InternetEdge::PerSite, InternetEdge::None][k % 2]
                        } else {
                            [InternetEdge::PerSite, InternetEdge::AtHub][k % 2]
                        };
                        let names = ["Head office", "Harbour", "Northgate", "Old mill"];
                        let requirements = Requirements {
                            name: format!("Matrix {k}"),
                            sites: (0..sites)
                                .map(|i| Site {
                                    name: names[i].to_string(),
                                    users: users / (i as u32 + 1),
                                    voice,
                                    guest_wifi: guest,
                                    servers: if i == 0 { 4 } else { 0 },
                                    printers: (i as u32) % 3,
                                    vlans: Vec::new(),
                                })
                                .collect(),
                            redundancy,
                            wan: nocturne_design::Wan {
                                topology,
                                hub: None,
                                internet,
                            },
                            addressing: Addressing {
                                base: "10.0.0.0/12".to_string(),
                                per_site_len: None,
                                p2p_len: [30, 31][k % 2],
                                mgmt_vlan: [99u16, 900][k % 2],
                            },
                            routing,
                            // Thousands of users already need a hundred
                            // access switches; piling growth on top of that
                            // goes past what a collapsed core can carry, and
                            // the refusal is tested on its own.
                            growth_pct: if users >= 2000 {
                                0
                            } else {
                                [0u32, 25, 50][k % 3]
                            },
                            access_ports: [24u32, 48][k % 2],
                            services: Services {
                                dhcp: k % 4 != 3,
                                nat: true,
                                ntp: None,
                                syslog: None,
                                snmp: None,
                            },
                            // Two test hosts per VLAN on a quarter of the
                            // matrix: it doubles the nodes and the probes, so
                            // it is worth sampling rather than doing always.
                            hosts_per_vlan: 1 + u32::from(k.is_multiple_of(4)),
                            seed: k as u64,
                        };
                        cases.push(Case {
                            check_determinism: k.is_multiple_of(4),
                            label: format!(
                                "case {k}: {sites} site(s), {users} users, {redundancy:?}, \
                                 {topology:?}, {internet:?}, {routing:?}, voice={voice}, \
                                 guest={guest}"
                            ),
                            requirements,
                        });
                    }
                }
            }
        }
    }
    cases
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % shards == shard)
        .map(|(_, case)| case)
        .collect()
}

/// Generate, boot, settle and prove one case.
pub fn run_case(case: &Case) {
    let design = generate(&case.requirements)
        .unwrap_or_else(|e| panic!("{}: generate failed: {e}", case.label));
    assert_invariants(&design, &case.label);
    if case.check_determinism {
        let again = generate(&case.requirements).expect("the second pass must work too");
        assert_eq!(
            serde_json::to_string(&design).unwrap(),
            serde_json::to_string(&again).unwrap(),
            "{}: two runs differ",
            case.label
        );
    }
    let mut lab = boot(&design, &case.label);
    let failures = failures(&design, &mut lab);
    assert!(
        failures.is_empty(),
        "{}: {} check(s) failed:\n{}",
        case.label,
        failures.len(),
        failures.join("\n")
    );
    assert!(
        !design.plan.checks.is_empty(),
        "{}: a design with no checks proves nothing",
        case.label
    );
}

/// A spec round-trips through JSON (the user interface sends it as JSON).
pub fn assert_json_round_trip(spec: &LabSpec, label: &str) {
    let text = serde_json::to_string(spec).expect("a lab serialises");
    let back: LabSpec = serde_json::from_str(&text).expect("and comes back");
    assert_eq!(&back, spec, "{label}: the lab does not round-trip");
}
