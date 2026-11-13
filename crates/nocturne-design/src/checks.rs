//! The reachability matrix a correct build has to satisfy, derived from the
//! design itself.
//!
//! Every check names a node that exists in the lab and an address the plan
//! allocated, so the test suite can probe all of them without knowing
//! anything about how the design was made.

use crate::design::{Check, DeviceRole, Expect};
use crate::model::{INTERNET_HOST, Model};
use crate::requirements::{InternetEdge, Purpose, WanTopology};
use crate::topology::Topology;
use crate::util::fmt_ip;

/// A design with a hundred switches must not produce a thousand checks, so the
/// management probes name every device that carries the site — its cores and
/// its edge routers — and then a sample of the access layer.
const MANAGED_SAMPLE_PER_SITE: usize = 2;

pub(crate) fn derive(model: &Model, t: &Topology) -> Vec<Check> {
    let mut checks: Vec<Check> = Vec::new();
    let mut push = |from: &str, to: String, to_name: String, port, expect, why: String| {
        checks.push(Check {
            from: from.to_string(),
            to,
            to_name,
            port,
            expect,
            why,
        });
    };

    for (s, site) in model.sites.iter().enumerate() {
        let hosts: Vec<usize> = (0..t.devs.len())
            .filter(|&i| t.devs[i].site == s && t.devs[i].role == DeviceRole::Host)
            .collect();
        let host_in = |purpose: Purpose| -> Option<usize> {
            let vlan = site.vlan(purpose)?;
            hosts
                .iter()
                .copied()
                .find(|&i| t.devs[i].vlan == Some(vlan.id) && t.devs[i].model_id != "nms")
        };

        // Every test host reaches its own gateway: the VLAN exists, the SVI is
        // up, and the host's addressing matches it.
        for &i in &hosts {
            let Some(vlan_id) = t.devs[i].vlan else {
                continue;
            };
            let Some(vlan) = site.vlans.iter().find(|v| v.id == vlan_id) else {
                continue;
            };
            push(
                &t.devs[i].name,
                fmt_ip(vlan.gateway),
                format!("{} default gateway at {}", vlan.name, site.name),
                None,
                Expect::Reachable,
                format!(
                    "a host in VLAN {vlan_id} has to reach its own gateway, or nothing else works"
                ),
            );
        }

        // Between VLANs: the core really does route.
        let users = host_in(Purpose::Users);
        for (purpose, label) in [
            (Purpose::Servers, "the servers"),
            (Purpose::Printers, "a printer"),
            (Purpose::Voice, "a telephone"),
        ] {
            if let (Some(a), Some(b)) = (users, host_in(purpose)) {
                push(
                    &t.devs[a].name,
                    t.devs[b].mgmt.map(fmt_ip).unwrap_or_default(),
                    t.devs[b].name.clone(),
                    None,
                    Expect::Reachable,
                    format!("users have to reach {label} across the VLAN boundary"),
                );
            }
        }

        // Visitors: out to the Internet, nowhere inside, in either direction.
        if let (Some(guest), Some(gvlan)) = (host_in(Purpose::Guest), site.vlan(Purpose::Guest)) {
            let gname = t.devs[guest].name.clone();
            if let Some(users_vlan) = site.vlan(Purpose::Users) {
                push(
                    &gname,
                    fmt_ip(users_vlan.gateway),
                    format!("{} gateway at {}", users_vlan.name, site.name),
                    None,
                    Expect::Blocked,
                    "the guest network must not reach the internal networks".to_string(),
                );
            }
            if let Some(u) = users {
                push(
                    &gname,
                    t.devs[u].mgmt.map(fmt_ip).unwrap_or_default(),
                    t.devs[u].name.clone(),
                    None,
                    Expect::Blocked,
                    "a visitor must not reach a workstation".to_string(),
                );
                push(
                    &t.devs[u].name,
                    t.devs[guest].mgmt.map(fmt_ip).unwrap_or_default(),
                    gname.clone(),
                    None,
                    Expect::Blocked,
                    "and the isolation holds the other way round too: an internal host gets no \
                     answer from the guest network"
                        .to_string(),
                );
            }
            if let Some(core) = t.cores[s].first().and_then(|&c| t.devs[c].mgmt) {
                push(
                    &gname,
                    fmt_ip(core),
                    "the core switch's management address".to_string(),
                    Some(22),
                    Expect::Blocked,
                    "a visitor must not even see the management plane".to_string(),
                );
            }
            let _ = gvlan;
        }

        // The way out.
        let internet_here = site.internet || model.req.wan.internet != InternetEdge::None;
        if internet_here {
            for purpose in [Purpose::Users, Purpose::Guest] {
                if let Some(i) = host_in(purpose) {
                    push(
                        &t.devs[i].name,
                        INTERNET_HOST.to_string(),
                        "a public host on the Internet".to_string(),
                        None,
                        Expect::Reachable,
                        match purpose {
                            Purpose::Guest => {
                                "visitors get Internet access, which is the whole point of the \
                                 guest network"
                                    .to_string()
                            }
                            _ => "users reach the Internet through the edge router's \
                                  translation"
                                .to_string(),
                        },
                    );
                }
            }
            if let Some(i) = host_in(Purpose::Users) {
                push(
                    &t.devs[i].name,
                    INTERNET_HOST.to_string(),
                    "a public web server".to_string(),
                    Some(80),
                    Expect::Reachable,
                    "and TCP gets through the translation, not just ping".to_string(),
                );
            }
        }

        // The host that takes its address from DHCP proves the pools work.
        if let Some(i) = hosts
            .iter()
            .copied()
            .find(|&i| t.devs[i].host.as_ref().is_some_and(|h| h.dhcp))
            && let Some(users_vlan) = site.vlan(Purpose::Users)
        {
            push(
                &t.devs[i].name,
                fmt_ip(users_vlan.gateway),
                format!("{} gateway at {}", users_vlan.name, site.name),
                None,
                Expect::Reachable,
                "this host has no address of its own: reaching the gateway proves the DHCP pool \
                 leases a working address, mask and gateway"
                    .to_string(),
            );
        }
    }

    // Site to site.
    let reaches = |a: usize, b: usize| -> bool {
        match model.req.wan.topology {
            WanTopology::None => false,
            WanTopology::FullMesh => a != b,
            WanTopology::HubAndSpoke => a != b,
        }
    };
    for a in 0..model.sites.len() {
        for b in 0..model.sites.len() {
            if a == b || !reaches(a, b) {
                continue;
            }
            let Some(from) = (0..t.devs.len()).find(|&i| {
                t.devs[i].site == a
                    && t.devs[i].role == DeviceRole::Host
                    && t.devs[i].vlan == model.sites[a].vlan(Purpose::Users).map(|v| v.id)
                    && t.devs[i].mgmt.is_some()
            }) else {
                continue;
            };
            if let Some(vlan) = model.sites[b].vlan(Purpose::Users) {
                checks.push(Check {
                    from: t.devs[from].name.clone(),
                    to: fmt_ip(vlan.gateway),
                    to_name: format!("{} gateway at {}", vlan.name, model.sites[b].name),
                    port: None,
                    expect: Expect::Reachable,
                    why: format!(
                        "{} and {} are joined by the WAN, so their users share the network",
                        model.sites[a].name, model.sites[b].name
                    ),
                });
            }
            // Only once per pair for the guest isolation, which is global.
            if a < b
                && let (Some(guest), Some(vlan)) = (
                    (0..t.devs.len()).find(|&i| {
                        t.devs[i].site == a
                            && t.devs[i].role == DeviceRole::Host
                            && t.devs[i].vlan == model.sites[a].vlan(Purpose::Guest).map(|v| v.id)
                    }),
                    model.sites[b].vlan(Purpose::Users),
                )
            {
                checks.push(Check {
                    from: t.devs[guest].name.clone(),
                    to: fmt_ip(vlan.gateway),
                    to_name: format!("{} gateway at {}", vlan.name, model.sites[b].name),
                    port: None,
                    expect: Expect::Blocked,
                    why: "guest isolation covers the whole address space, not just the local site"
                        .to_string(),
                });
            }
        }
    }

    // The management plane.
    if let Some(nms) = t.nms {
        let name = t.devs[nms].name.clone();
        let home = t.devs[nms].site;
        // A site with no WAN is an island: its devices cannot be managed from
        // another site, and saying they can would be a lie the lab exposes.
        let joined = model.req.wan.topology != WanTopology::None;
        let mut managed: Vec<usize> = Vec::new();
        for s in 0..model.sites.len() {
            if s != home && !joined {
                continue;
            }
            let here = |role: DeviceRole| {
                (0..t.devs.len()).filter(move |&i| {
                    t.devs[i].site == s && t.devs[i].role == role && t.devs[i].mgmt.is_some()
                })
            };
            managed.extend(here(DeviceRole::Core));
            managed.extend(here(DeviceRole::Edge));
            managed.extend(
                here(DeviceRole::Distribution)
                    .take(MANAGED_SAMPLE_PER_SITE)
                    .chain(here(DeviceRole::Access).take(MANAGED_SAMPLE_PER_SITE)),
            );
        }
        for i in managed {
            let addr = fmt_ip(t.devs[i].mgmt.unwrap_or(0));
            checks.push(Check {
                from: name.clone(),
                to: addr.clone(),
                to_name: format!("{} ({})", t.devs[i].name, t.devs[i].role.label()),
                port: Some(22),
                expect: Expect::Reachable,
                why: "every device has to be reachable over SSH from the management network, or \
                      it cannot be backed up or changed"
                    .to_string(),
            });
            checks.push(Check {
                from: name.clone(),
                to: addr,
                to_name: format!("{} telnet", t.devs[i].name),
                port: Some(23),
                expect: Expect::Blocked,
                why: "telnet is turned off everywhere: the lines accept SSH only".to_string(),
            });
        }
    }

    checks.retain(|c| !c.to.is_empty() && c.to != "0.0.0.0" && !c.from.is_empty());
    checks
}
