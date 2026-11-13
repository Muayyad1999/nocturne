//! VLANs, trunks and the MAC table.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_drivers::hier::format_vlan_ranges;

use crate::checks::{NO_LINK, link_ends, owner, skip_link, skip_rest};
use crate::model::{Finding, Severity, Suggestion};
use crate::norm::{self, Allowed, Mode};
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{DeviceData, World, canonical};

/// Every VLAN id a switch can have. An allowed list holding all of them is
/// the same statement as having no allowed list at all.
fn is_every_vlan(vlans: &BTreeSet<u16>) -> bool {
    vlans.len() == 4094 && vlans.contains(&1) && vlans.contains(&4094)
}

fn allowed_of(data: &DeviceData, interface: &str) -> Allowed {
    match norm::trunk_allowed(data, interface) {
        Allowed::List(vlans) if is_every_vlan(&vlans) => Allowed::All,
        other => other,
    }
}

// ------------------------------------------------------- native VLAN mismatch

/// The two ends of a trunk disagree about which VLAN untagged frames belong to.
///
/// Look-alikes this stays quiet about:
///
/// * **One end with no `native vlan` line.** On an IOS-style trunk that means
///   VLAN 1 — a fact, not a guess — so `native vlan 1` on one end and nothing
///   on the other is *not* a mismatch.
/// * **A port that is not known to be a trunk.** An access port has no native
///   VLAN to compare, and a platform whose mode was neither collected nor
///   configured in a grammar this reads is not evaluated.
pub fn native_vlan_mismatch(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for link in world.target_links() {
        let (a, a_port, b, b_port) = match link_ends(world, link) {
            Ok(ends) => ends,
            Err(reason) => {
                skip_link(report, link, reason);
                continue;
            }
        };
        let (Some(a_native), Some(b_native)) =
            (norm::native_vlan(a, a_port), norm::native_vlan(b, b_port))
        else {
            skip_link(
                report,
                link,
                "one end is not known to be a trunk, or its native VLAN is neither collected nor \
                 in a configuration this reads",
            );
            continue;
        };
        report.ok(a.id());
        report.ok(b.id());
        if a_native == b_native {
            continue;
        }
        // The end that is on VLAN 1 only because nobody set it is the end that
        // was forgotten, so it owns the finding and gets the fix. Otherwise
        // the lower device id owns it, so the finding keeps its identity.
        let a_implicit = a_native == 1 && !norm::native_vlan_is_explicit(a, &a_port.name);
        let b_implicit = b_native == 1 && !norm::native_vlan_is_explicit(b, &b_port.name);
        let near_is_a = match (a_implicit, b_implicit) {
            (true, false) => true,
            (false, true) => false,
            _ => owner(a.id(), b.id()).0 == a.id(),
        };
        let (near, near_port, near_native, far, far_port, far_native) = if near_is_a {
            (a, a_port, a_native, b, b_port, b_native)
        } else {
            (b, b_port, b_native, a, a_port, a_native)
        };
        let fixable = near_native == 1 && !norm::native_vlan_is_explicit(near, &near_port.name);
        let mut suggestion = Suggestion::text(format!(
            "Decide which VLAN untagged frames on this trunk belong to and set both ends to it. \
             {} {} is on VLAN {near_native}{}; {} {} is on VLAN {far_native}.",
            near.name(),
            near_port.name,
            if fixable {
                " (the platform default, because nothing is configured)"
            } else {
                ""
            },
            far.name(),
            far_port.name
        ))
        .commands([
            format!("show interfaces {} switchport", near_port.name),
            format!("show interfaces {} switchport", far_port.name),
        ]);
        if fixable {
            suggestion = suggestion.lines([
                format!("interface {}", near_port.name),
                format!("switchport trunk native vlan {far_native}"),
            ]);
        }
        report.found(
            Finding::new(
                "native_vlan_mismatch",
                near.id(),
                &near_port.name,
                format!(
                    "Native VLAN {near_native} on {} {} against {far_native} on {} {}",
                    near.name(),
                    near_port.name,
                    far.name(),
                    far_port.name
                ),
            )
            .detail(
                "Untagged frames entering one end come out in a different VLAN at the other, \
                 which bridges two VLANs into one broadcast domain. Spanning tree runs on the \
                 native VLAN, so this is also how a loop gets past it.",
            )
            .evidence(
                format!("{} {}", near.name(), near_port.name),
                format!("native VLAN {near_native}"),
            )
            .evidence(
                format!("{} {}", far.name(), far_port.name),
                format!("native VLAN {far_native}"),
            )
            .related(far.id(), far.name(), &far_port.name)
            .suggest(suggestion),
        );
    }
    skip_rest(world, report, NO_LINK);
}

// -------------------------------------------------------- trunk VLAN mismatch

/// Both ends of a trunk restrict what they carry, and the lists differ.
///
/// Look-alikes this stays quiet about:
///
/// * **One end allowing everything.** A trunk with no allowed list carries
///   every VLAN; the other end pruning it to the VLANs it needs is how a
///   trunk is normally built, not a fault. Only two *explicit* lists are
///   compared.
/// * **VLANs that exist nowhere.** A list that allows 100–200 while the other
///   allows 100–150 is only a problem for VLANs that actually exist, so the
///   difference is filtered against both switches' VLAN databases. Without
///   those lists the check does not run.
pub fn trunk_vlan_mismatch(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for link in world.target_links() {
        let (a, a_port, b, b_port) = match link_ends(world, link) {
            Ok(ends) => ends,
            Err(reason) => {
                skip_link(report, link, reason);
                continue;
            }
        };
        let (Allowed::List(a_allowed), Allowed::List(b_allowed)) =
            (allowed_of(a, &a_port.name), allowed_of(b, &b_port.name))
        else {
            skip_link(
                report,
                link,
                "one end has no explicit allowed VLAN list (or its configuration is not stored in \
                 a grammar this reads)",
            );
            continue;
        };
        let (Some(a_vlans), Some(b_vlans)) = (a.vlans.as_ref(), b.vlans.as_ref()) else {
            skip_link(
                report,
                link,
                "no VLAN list collected for one end, so a real VLAN cannot be told from a gap in \
                 the allowed list",
            );
            continue;
        };
        report.ok(a.id());
        report.ok(b.id());
        let defined: BTreeSet<u16> = a_vlans
            .rows
            .iter()
            .chain(b_vlans.rows.iter())
            .filter_map(|vlan| vlan.id)
            .filter(|id| *id <= 4094)
            .map(|id| id as u16)
            .collect();
        let only_a: Vec<u16> = a_allowed
            .difference(&b_allowed)
            .copied()
            .filter(|vlan| defined.contains(vlan))
            .collect();
        let only_b: Vec<u16> = b_allowed
            .difference(&a_allowed)
            .copied()
            .filter(|vlan| defined.contains(vlan))
            .collect();
        if only_a.is_empty() && only_b.is_empty() {
            continue;
        }
        let near_is_a = owner(a.id(), b.id()).0 == a.id();
        let (near, near_port, far, far_port, near_only, far_only, near_allowed, far_allowed) =
            if near_is_a {
                (
                    a, a_port, b, b_port, &only_a, &only_b, &a_allowed, &b_allowed,
                )
            } else {
                (
                    b, b_port, a, a_port, &only_b, &only_a, &b_allowed, &a_allowed,
                )
            };
        let as_ranges =
            |vlans: &BTreeSet<u16>| format_vlan_ranges(&vlans.iter().copied().collect::<Vec<_>>());
        let mut finding = Finding::new(
            "trunk_vlan_mismatch",
            near.id(),
            &near_port.name,
            format!(
                "The trunk {} {} ↔ {} {} does not carry the same VLANs at both ends",
                near.name(),
                near_port.name,
                far.name(),
                far_port.name
            ),
        )
        .detail(
            "Each end drops the VLANs the other allows and it does not, so hosts in those VLANs \
             reach everything on their own switch and nothing beyond it — which looks like a \
             host problem, not a trunk problem.",
        )
        .evidence(
            format!("{} {} allows", near.name(), near_port.name),
            as_ranges(near_allowed),
        )
        .evidence(
            format!("{} {} allows", far.name(), far_port.name),
            as_ranges(far_allowed),
        )
        .related(far.id(), far.name(), &far_port.name);
        if !near_only.is_empty() {
            finding = finding.evidence(
                format!("Only on {}", near.name()),
                format_vlan_ranges(near_only),
            );
        }
        if !far_only.is_empty() {
            finding = finding.evidence(
                format!("Only on {}", far.name()),
                format_vlan_ranges(far_only),
            );
        }
        // Adding the missing VLANs to this end is safe and exact: `add` edits
        // the list in place, where restating it would drop every other VLAN
        // for the instant between the two commands.
        if !far_only.is_empty() {
            finding = finding.suggest(
                Suggestion::text(format!(
                    "If {} should carry {}, add them here — `add` edits the list in place rather \
                     than restating it, which would cut the trunk for an instant. If they should \
                     not be carried, remove them at the other end instead.",
                    near.name(),
                    format_vlan_ranges(far_only)
                ))
                .lines([
                    format!("interface {}", near_port.name),
                    format!(
                        "switchport trunk allowed vlan add {}",
                        format_vlan_ranges(far_only)
                    ),
                ])
                .commands([
                    format!("show interfaces {} trunk", near_port.name),
                    format!("show interfaces {} trunk", far_port.name),
                ]),
            );
        }
        report.found(finding);
    }
    skip_rest(world, report, NO_LINK);
}

// ------------------------------------------------------------ VLAN plumbing

struct Trunks {
    /// VLANs carried by at least one trunk.
    carried: BTreeSet<u16>,
    /// A trunk carries everything, so no VLAN can be missing.
    any_unrestricted: bool,
    /// A trunk whose allowed list cannot be read: no verdict is possible.
    any_unknown: bool,
    /// `(interface, allowed)` of the trunks with an explicit list.
    explicit: Vec<(String, BTreeSet<u16>)>,
}

fn trunks_of(data: &DeviceData) -> Trunks {
    let mut trunks = Trunks {
        carried: BTreeSet::new(),
        any_unrestricted: false,
        any_unknown: false,
        explicit: Vec::new(),
    };
    for port in data.interface_rows() {
        if norm::mode(data, port) != Some(Mode::Trunk) {
            continue;
        }
        match allowed_of(data, &port.name) {
            Allowed::All => trunks.any_unrestricted = true,
            Allowed::Unknown => trunks.any_unknown = true,
            Allowed::List(vlans) => {
                trunks.carried.extend(vlans.iter().copied());
                trunks.explicit.push((port.name.clone(), vlans));
            }
        }
    }
    trunks
}

/// Hosts are in a VLAN that none of the switch's trunks carry.
///
/// Look-alikes this stays quiet about:
///
/// * **A trunk with no allowed list** carries the VLAN, so nothing is missing.
/// * **VLANs with no live access port.** A VLAN whose only ports are shut is
///   not black-holing anybody; that is `unused_vlan`'s business.
pub fn vlan_not_on_trunk(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for data in world.targets() {
        let Some(vlans) = data.vlans.as_ref() else {
            report.skip(data.id(), "no VLAN list collected");
            continue;
        };
        if data.interfaces.is_none() {
            report.skip(data.id(), "no interface data collected");
            continue;
        }
        let trunks = trunks_of(data);
        if trunks.any_unknown {
            report.skip(
                data.id(),
                "a trunk's allowed VLAN list cannot be read from the stored configuration",
            );
            continue;
        }
        if trunks.explicit.is_empty() && !trunks.any_unrestricted {
            report.skip(
                data.id(),
                "the switch has no trunk, so there is no uplink a VLAN could be missing from",
            );
            continue;
        }
        report.ok(data.id());
        if trunks.any_unrestricted {
            continue;
        }
        for vlan in &vlans.rows {
            let Some(id) = vlan.id.filter(|id| *id <= 4094) else {
                continue;
            };
            let ports: Vec<String> = data
                .interface_rows()
                .iter()
                .filter(|port| {
                    port.admin_up != Some(false) && norm::access_vlan(data, port) == Some(id)
                })
                .map(|port| port.name.clone())
                .collect();
            if ports.is_empty() || trunks.carried.contains(&(id as u16)) {
                continue;
            }
            let trunk_names: Vec<String> = trunks
                .explicit
                .iter()
                .map(|(name, vlans)| {
                    format!(
                        "{name} allows {}",
                        format_vlan_ranges(&vlans.iter().copied().collect::<Vec<_>>())
                    )
                })
                .collect();
            let mut suggestion = Suggestion::text(format!(
                "Either VLAN {id} should leave this switch — then add it to the uplink trunk — or \
                 those ports are in the wrong VLAN. Check which before changing the trunk.",
            ))
            .commands(vec!["show interfaces trunk".to_string()]);
            // Only offered when there is one trunk it could mean; with
            // several, adding a VLAN to the wrong one is its own outage.
            if let [(only_trunk, _)] = trunks.explicit.as_slice() {
                suggestion = suggestion.lines([
                    format!("interface {only_trunk}"),
                    format!("switchport trunk allowed vlan add {id}"),
                ]);
            }
            report.found(
                Finding::new(
                    "vlan_not_on_trunk",
                    data.id(),
                    format!("VLAN {id}"),
                    format!(
                        "VLAN {id} has {} port(s) on {} but no trunk carries it",
                        ports.len(),
                        data.name()
                    ),
                )
                .detail(
                    "Hosts in this VLAN can reach each other and nothing else: no gateway, no \
                     DHCP server, no other switch. From the host's side it looks like a broken \
                     network card.",
                )
                .evidence(
                    format!("VLAN {id}"),
                    format!(
                        "{} ({})",
                        if vlan.name.is_empty() {
                            "no name".into()
                        } else {
                            vlan.name.clone()
                        },
                        vlan.status
                    ),
                )
                .evidence("Access ports", ports.join(", "))
                .evidence("Trunks", trunk_names.join("; "))
                .suggest(suggestion),
            );
        }
    }
    skip_rest(world, report, "no VLAN list collected");
}

/// A VLAN that exists on the switch and is used by nothing on it.
///
/// Look-alike: a VLAN **any trunk carries** may be transiting for a switch
/// further down the chain, which is entirely normal — so a switch with an
/// unrestricted trunk is not evaluated at all, and a VLAN on an explicit
/// allowed list is never reported.
pub fn unused_vlan(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for data in world.targets() {
        let Some(vlans) = data.vlans.as_ref() else {
            report.skip(data.id(), "no VLAN list collected");
            continue;
        };
        if data.interfaces.is_none() {
            report.skip(data.id(), "no interface data collected");
            continue;
        }
        let trunks = trunks_of(data);
        if trunks.any_unrestricted || trunks.any_unknown {
            report.skip(
                data.id(),
                "a trunk carries every VLAN (or its list cannot be read), so a VLAN may be \
                 transiting rather than unused",
            );
            continue;
        }
        report.ok(data.id());
        for vlan in &vlans.rows {
            let Some(id) = vlan.id.filter(|id| *id > 1 && *id <= 4094) else {
                continue;
            };
            if trunks.carried.contains(&(id as u16)) {
                continue;
            }
            let used_by_port = data
                .interface_rows()
                .iter()
                .any(|port| norm::access_vlan(data, port) == Some(id));
            let has_svi = data.interface(&format!("Vlan{id}")).is_some();
            if used_by_port || has_svi {
                continue;
            }
            report.found(
                Finding::new(
                    "unused_vlan",
                    data.id(),
                    format!("VLAN {id}"),
                    format!(
                        "VLAN {id}{} exists on {} but nothing uses it",
                        if vlan.name.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", vlan.name)
                        },
                        data.name()
                    ),
                )
                .detail(
                    "No access port is in it, there is no interface for it, and no trunk carries \
                     it. Leftover VLANs make every later change harder to reason about, and an \
                     unused VLAN that is still allowed somewhere is a place for traffic to hide.",
                )
                .evidence(
                    format!("VLAN {id}"),
                    format!("{} ({})", vlan.name, vlan.status),
                )
                .evidence("Access ports", "none".to_string())
                .evidence("Carried by a trunk", "no".to_string())
                .suggest(
                    Suggestion::text(
                        "Confirm nothing is planned for it, then remove it. Removing a VLAN that \
                         is in use takes its hosts off the network, so check the other switches \
                         first.",
                    )
                    .commands(vec!["show vlan brief".to_string()]),
                ),
            );
        }
    }
    skip_rest(world, report, "no VLAN list collected");
}

/// The same VLAN id named differently on different switches.
///
/// Look-alikes: **default names** (`default`, `VLAN0010`) say nothing, and
/// differences of **case, spaces, hyphens and underscores** (`Voice-VLAN`
/// against `voice_vlan`) are not differences anybody needs to know about.
pub fn vlan_name_inconsistent(world: &World, _settings: &Settings, report: &mut CheckReport) {
    // VLAN id → name key → (spelling, devices)
    let mut names: BTreeMap<u64, BTreeMap<String, (String, Vec<String>)>> = BTreeMap::new();
    let mut any_named = false;
    for data in &world.devices {
        let Some(vlans) = data.vlans.as_ref() else {
            continue;
        };
        for vlan in &vlans.rows {
            let Some(id) = vlan.id else { continue };
            if norm::is_default_vlan_name(id, &vlan.name) {
                continue;
            }
            any_named = true;
            let entry = names
                .entry(id)
                .or_default()
                .entry(norm::vlan_name_key(&vlan.name))
                .or_insert_with(|| (vlan.name.clone(), Vec::new()));
            entry.1.push(data.device.id.clone());
        }
    }

    for data in world.targets() {
        if data.vlans.is_none() {
            report.skip(data.id(), "no VLAN list collected");
        } else if !any_named {
            report.skip(
                data.id(),
                "no switch has a VLAN name of its own, so there is nothing to compare",
            );
        } else {
            report.ok(data.id());
        }
    }

    for (id, spellings) in names {
        if spellings.len() < 2 {
            continue;
        }
        // The majority spelling; on a tie, the alphabetically first key, so
        // that the same switch is reported in every scan.
        let majority = spellings
            .iter()
            .max_by_key(|(key, (_, devices))| (devices.len(), std::cmp::Reverse((*key).clone())))
            .map(|(key, (spelling, devices))| (key.clone(), spelling.clone(), devices.len()));
        let Some((majority_key, majority_spelling, majority_count)) = majority else {
            continue;
        };
        for (key, (spelling, devices)) in &spellings {
            if *key == majority_key {
                continue;
            }
            for device_id in devices {
                if !world.is_target(device_id) {
                    continue;
                }
                let Some(data) = world.device(device_id) else {
                    continue;
                };
                let others: Vec<crate::model::Related> = spellings
                    .get(&majority_key)
                    .map(|(_, devices)| devices.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|other| world.device(other))
                    .map(|other| crate::model::Related {
                        device_id: other.id().to_string(),
                        device: other.name().to_string(),
                        subject: format!("VLAN {id}"),
                    })
                    .collect();
                let mut finding = Finding::new(
                    "vlan_name_inconsistent",
                    data.id(),
                    format!("VLAN {id}"),
                    format!(
                        "VLAN {id} is `{spelling}` on {} but `{majority_spelling}` on {majority_count} other switch(es)",
                        data.name()
                    ),
                )
                .detail(
                    "One VLAN with two names makes every search, every report and every change \
                     ambiguous — and it is usually the first sign that two people built the same \
                     VLAN twice for different reasons.",
                )
                .evidence(format!("On {}", data.name()), spelling.clone())
                .evidence("Elsewhere", majority_spelling.clone())
                .suggest(
                    Suggestion::text(format!(
                        "Pick one name. Renaming a VLAN changes nothing about how it forwards, so \
                         this is safe to correct: `vlan {id}` then `name {majority_spelling}`.",
                    ))
                    .lines([format!("vlan {id}"), format!("name {majority_spelling}")])
                    .commands(vec!["show vlan brief".to_string()]),
                );
                for related in others {
                    finding = finding.related(&related.device_id, &related.device, related.subject);
                }
                report.found(finding);
            }
        }
    }
}

// ------------------------------------------------------------- MAC flapping

/// Several MAC addresses moving between the same two ports between two
/// collections — what a bridging loop looks like from the MAC table.
///
/// Look-alikes this stays quiet about, and this is the check where they
/// matter most:
///
/// * **One address moving** is a laptop at another desk or a wireless client
///   roaming. The threshold is several addresses moving the same way.
/// * **Moves involving a trunk or a port with a network neighbour.** A
///   spanning-tree reconvergence moves every address from one uplink to the
///   other, by design. Only moves between two ordinary access ports are
///   reported.
/// * **Static entries**, which do not move.
pub fn mac_flapping(world: &World, settings: &Settings, report: &mut CheckReport) {
    let minimum = settings.thresholds.mac_flap_min_macs.max(2);
    for data in world.targets() {
        let (Some(now), Some(before)) = (data.mac_table.as_ref(), data.previous_mac_table.as_ref())
        else {
            report.skip(
                data.id(),
                "only one MAC-table collection, so nothing can be seen to move",
            );
            continue;
        };
        report.ok(data.id());
        let mut was: BTreeMap<(u64, String), String> = BTreeMap::new();
        for entry in &before.rows {
            if entry.kind.eq_ignore_ascii_case("static") {
                continue;
            }
            if let (Some(vlan), Some(mac)) = (entry.vlan, crate::net::normalise_mac(&entry.mac)) {
                was.insert((vlan, mac), canonical(&entry.interface));
            }
        }
        // (vlan, from, to) → the addresses that moved
        let mut moves: BTreeMap<(u64, String, String), Vec<String>> = BTreeMap::new();
        for entry in &now.rows {
            if entry.kind.eq_ignore_ascii_case("static") {
                continue;
            }
            let (Some(vlan), Some(mac)) = (entry.vlan, crate::net::normalise_mac(&entry.mac))
            else {
                continue;
            };
            let to = canonical(&entry.interface);
            let Some(from) = was.get(&(vlan, mac.clone())) else {
                continue;
            };
            if *from == to {
                continue;
            }
            moves
                .entry((vlan, from.clone(), to))
                .or_default()
                .push(entry.mac.clone());
        }
        for ((vlan, from, to), macs) in moves {
            if macs.len() < minimum as usize {
                continue;
            }
            if !is_edge_port(data, &from) || !is_edge_port(data, &to) {
                continue;
            }
            let from_name = port_name(data, &from);
            let to_name = port_name(data, &to);
            let examples: Vec<String> = macs.iter().take(5).cloned().collect();
            report.found(
                Finding::new(
                    "mac_flapping",
                    data.id(),
                    format!("VLAN {vlan} {from_name} ↔ {to_name}"),
                    format!(
                        "{} addresses in VLAN {vlan} moved from {from_name} to {to_name} between two collections",
                        macs.len()
                    ),
                )
                .severity(Severity::High)
                .detail(
                    "When many addresses in one VLAN appear on a second port at once, the same \
                     frames are arriving twice — which is what a cable patched between two wall \
                     sockets, or a small unmanaged switch wired back into the network, does. \
                     Both ports are edge ports, so this is not a spanning-tree reconvergence.",
                )
                .evidence("Addresses", format!("{} moved", macs.len()))
                .evidence("Examples", examples.join(", "))
                .evidence(
                    "Collections",
                    format!("{} → {}", before.at, now.at),
                )
                .suggest(
                    Suggestion::text(format!(
                        "Look at what is plugged into {from_name} and {to_name}. Until it is \
                         found, shutting one of the two ports stops the loop — but shut the right \
                         one: it takes whatever is on it off the network.",
                    ))
                    .commands([
                        format!("show mac address-table interface {from_name}"),
                        format!("show mac address-table interface {to_name}"),
                        "show spanning-tree detail".to_string(),
                    ]),
                ),
            );
        }
    }
    skip_rest(world, report, "no MAC-table history");
}

/// A port with no network device on it: not a trunk, no collected neighbour.
fn is_edge_port(data: &DeviceData, canonical_name: &str) -> bool {
    let Some(port) = data.interface(canonical_name) else {
        return false;
    };
    if norm::mode(data, port) == Some(Mode::Trunk) {
        return false;
    }
    if !norm::is_physical(&port.name) {
        return false;
    }
    data.neighbors_of(&port.name).next().is_none()
}

fn port_name(data: &DeviceData, canonical_name: &str) -> String {
    data.interface(canonical_name)
        .map(|port| port.name.clone())
        .unwrap_or_else(|| canonical_name.to_string())
}

#[cfg(test)]
#[path = "l2_tests.rs"]
mod tests;
