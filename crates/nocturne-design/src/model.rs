//! Reading the requirements: normalise what was asked for, size the site, and
//! carve the address space. Everything downstream works from the result.
//!
//! [`prepare`] is the single place requirements are judged, so
//! [`validate`](crate::validate) and [`generate`](crate::generate) can never
//! disagree about what is buildable.

use nocturne_tools::vlsm;

use crate::design::Decision;
use crate::requirements::{InternetEdge, Purpose, Redundancy, Requirements, VlanReq, WanTopology};
use crate::util::{Net, fmt_ip, one_line, prefix_for_block, prefix_for_hosts, tag, with_growth};

/// Sites above this are a programme of work, not one design pass.
pub(crate) const MAX_SITES: usize = 12;
/// Host ports on the simulator's access switch (`switch`: 24 × FE + 2 × GE).
pub(crate) const SIM_ACCESS_PORTS: u32 = 24;
/// Downlink ports on a distribution switch (`switch-l3`, keeping two for uplinks).
pub(crate) const DIST_DOWNLINKS: u32 = 22;
/// Ports on a core switch (`switch-l3`: 24 × GE, the 10G ports are kept for the peer link).
pub(crate) const CORE_PORTS: u32 = 24;
/// Smallest subnet any routed VLAN gets, so the first ten addresses can stay
/// static and a DHCP pool still has room.
pub(crate) const MIN_VLAN_HOSTS: u32 = 30;
/// Addresses at the bottom of every user subnet kept out of the DHCP pool.
pub(crate) const STATIC_RESERVE: u32 = 10;
/// Wireless users one access point is planned for.
pub(crate) const USERS_PER_AP: u32 = 40;
/// A broadcast domain larger than this is worth splitting up.
pub(crate) const BIG_VLAN_HOSTS: u32 = 500;
/// The public block the lab's Internet links come from (RFC 5737).
pub(crate) const PUBLIC_BLOCK: &str = "198.51.100.0/24";
/// The well-known address on the simulated Internet.
pub(crate) const INTERNET_HOST: &str = "8.8.8.8";
/// Ports on the simulator's `cloud` model.
pub(crate) const CLOUD_PORTS: usize = 4;

/// One VLAN, sized and addressed.
#[derive(Clone, Debug)]
pub(crate) struct VlanModel {
    pub(crate) id: u16,
    pub(crate) name: String,
    pub(crate) purpose: Purpose,
    /// Hosts asked for, before growth.
    pub(crate) hosts_raw: u32,
    /// Hosts planned for, growth included.
    pub(crate) hosts_needed: u32,
    pub(crate) subnet: Net,
    /// First usable address: the gateway, real or virtual.
    pub(crate) gateway: u32,
    /// DHCP pool, when this kind of VLAN gets one.
    pub(crate) pool: Option<(u32, u32)>,
}

impl VlanModel {
    /// The address a test host with index `n` (from 0) gets.
    pub(crate) fn test_host(&self, n: u32) -> Option<u32> {
        // .1 gateway, .2/.3 the real core addresses, .5.. the test hosts,
        // .11 onwards the DHCP pool.
        let addr = self.subnet.first_host().checked_add(4 + n)?;
        (addr <= self.subnet.last_host()).then_some(addr)
    }
}

/// How many of each kind of box a site needs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counts {
    pub(crate) cores: u32,
    pub(crate) distribution: u32,
    pub(crate) access: u32,
    pub(crate) edges: u32,
    pub(crate) aps: u32,
    /// Servers (and the management station) hang off their own switch.
    pub(crate) host_switch: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SiteModel {
    pub(crate) name: String,
    pub(crate) tag: String,
    pub(crate) index: usize,
    pub(crate) users: u32,
    pub(crate) servers: u32,
    pub(crate) printers: u32,
    pub(crate) is_hub: bool,
    pub(crate) internet: bool,
    pub(crate) vlans: Vec<VlanModel>,
    pub(crate) block: Net,
    pub(crate) counts: Counts,
    /// Host ports dealt to access switch `k`.
    pub(crate) ports_per_access: Vec<u32>,
    /// The point-to-point subnet of the transit VLAN between two cores.
    pub(crate) transit: Option<Net>,
    /// This site's slice of the infrastructure block: its loopbacks and its
    /// point-to-point links, so one prefix summarises the whole site.
    pub(crate) infra: Net,
    /// Loopbacks, in the order the devices are created.
    pub(crate) loopbacks: Vec<u32>,
    pub(crate) next_loopback: usize,
    /// Point-to-point blocks: the core peer transit first, then one per edge
    /// router, then the WAN links this site owns.
    pub(crate) p2p: Vec<Net>,
    pub(crate) next_p2p: usize,
}

impl SiteModel {
    pub(crate) fn take_loopback(&mut self) -> Option<u32> {
        let addr = self.loopbacks.get(self.next_loopback).copied();
        self.next_loopback += 1;
        addr
    }

    pub(crate) fn take_p2p(&mut self) -> Option<Net> {
        let net = self.p2p.get(self.next_p2p).copied();
        self.next_p2p += 1;
        net
    }

    pub(crate) fn vlan(&self, purpose: Purpose) -> Option<&VlanModel> {
        self.vlans.iter().find(|v| v.purpose == purpose)
    }

    pub(crate) fn mgmt(&self) -> Option<&VlanModel> {
        self.vlan(Purpose::Management)
    }
}

/// One WAN adjacency between two sites.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WanLink {
    pub(crate) a: usize,
    pub(crate) b: usize,
    pub(crate) subnet: Net,
}

/// The requirements, made concrete.
#[derive(Clone, Debug)]
pub(crate) struct Model {
    pub(crate) req: Requirements,
    pub(crate) sites: Vec<SiteModel>,
    pub(crate) base: Net,
    pub(crate) infra: Net,
    pub(crate) parking_vlan: u16,
    /// Carried on the core peer link only: the routed adjacency between the
    /// two cores of a site, so routing never runs over a user VLAN.
    pub(crate) transit_vlan: u16,
    pub(crate) wan_links: Vec<WanLink>,
    pub(crate) hub: usize,
    pub(crate) decisions: Vec<Decision>,
    pub(crate) warnings: Vec<String>,
}

impl Model {
    /// Every address the design uses internally, as one ACL-friendly block.
    pub(crate) fn internal(&self) -> Net {
        self.base
    }
}

// ---------------------------------------------------------------- the pass

/// Normalise, size and address the requirements, or explain what is wrong.
pub(crate) fn prepare(req: &Requirements) -> Result<Model, Vec<String>> {
    let mut problems: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut decisions: Vec<Decision> = Vec::new();
    let mut req = req.clone();

    // -- the obvious scalars ------------------------------------------------
    if req.sites.is_empty() {
        problems.push("there are no sites to design; add at least one".to_string());
    }
    if req.sites.len() > MAX_SITES {
        problems.push(format!(
            "{} sites were given; this designs up to {MAX_SITES} in one pass, so split the project \
             into regions and design each one",
            req.sites.len()
        ));
    }
    if req.growth_pct > 400 {
        warnings.push(format!(
            "{} % growth head-room is a lot; the subnets and the switch count grow with it",
            req.growth_pct
        ));
    }
    if !matches!(req.access_ports, 24 | 48) {
        problems.push(format!(
            "access_ports is {}; an access switch has either 24 or 48 host ports",
            req.access_ports
        ));
    } else if req.access_ports != SIM_ACCESS_PORTS {
        warnings.push(format!(
            "{}-port access switches were asked for, but the simulator's catalogue only has a \
             24-port access switch, so the design uses 24-port switches and needs more of them; \
             halve the count when you buy {}-port hardware",
            req.access_ports, req.access_ports
        ));
        req.access_ports = SIM_ACCESS_PORTS;
    }
    if req.hosts_per_vlan > 4 {
        warnings.push(format!(
            "hosts_per_vlan was {}; the lab places at most 4 test hosts per VLAN",
            req.hosts_per_vlan
        ));
        req.hosts_per_vlan = 4;
    }
    if !matches!(req.addressing.p2p_len, 30 | 31) {
        problems.push(format!(
            "point-to-point links are /{}; use /30, or /31 (RFC 3021) to halve the waste",
            req.addressing.p2p_len
        ));
    }
    if !(1..=4094).contains(&req.addressing.mgmt_vlan) {
        problems.push(format!(
            "the management VLAN id is {}; VLAN ids run from 1 to 4094",
            req.addressing.mgmt_vlan
        ));
    } else if req.addressing.mgmt_vlan == 1 {
        warnings.push(
            "the management VLAN is VLAN 1, the default VLAN every unconfigured port lands in; \
             a dedicated id such as 99 keeps a mis-cabled port out of the management network"
                .to_string(),
        );
    }
    req.name = one_line(&req.name, 60);
    if req.name.is_empty() {
        req.name = "Network".to_string();
    }

    // -- site names ---------------------------------------------------------
    let mut seen: Vec<String> = Vec::new();
    for (i, site) in req.sites.iter().enumerate() {
        let name = one_line(&site.name, 40);
        if name.is_empty() {
            warnings.push(format!(
                "site {} has no name; it is called \"Site {}\" in the design",
                i + 1,
                i + 1
            ));
        }
        let key = name.to_ascii_lowercase();
        if !key.is_empty() && seen.contains(&key) {
            problems.push(format!(
                "two sites are called \"{name}\"; site names have to be different so the devices \
                 and the addresses can be told apart"
            ));
        }
        seen.push(key);
        if site.users > 100_000 {
            problems.push(format!(
                "site \"{}\" has {} users; above 100,000 this is a campus programme, not one site",
                if name.is_empty() { "unnamed" } else { &name },
                site.users
            ));
        }
    }

    // -- the base block -----------------------------------------------------
    let base = match Net::parse(&req.addressing.base) {
        Some(net) if net.len <= 30 => Some(net),
        Some(net) => {
            problems.push(format!(
                "the base block {net} is a /{}; a network needs room for subnets, so use /24 or shorter",
                net.len
            ));
            None
        }
        None => {
            problems.push(format!(
                "\"{}\" is not an IPv4 block; write it as an address and a prefix length, \
                 e.g. 10.20.0.0/16 (this design is IPv4 only)",
                one_line(&req.addressing.base, 40)
            ));
            None
        }
    };

    // -- the WAN ------------------------------------------------------------
    let hub = match &req.wan.hub {
        None => 0,
        Some(name) => {
            let wanted = one_line(name, 40).to_ascii_lowercase();
            match req
                .sites
                .iter()
                .position(|s| one_line(&s.name, 40).to_ascii_lowercase() == wanted)
            {
                Some(i) => i,
                None => {
                    problems.push(format!(
                        "the hub site \"{}\" is not one of the sites",
                        one_line(name, 40)
                    ));
                    0
                }
            }
        }
    };
    if req.wan.internet == InternetEdge::AtHub
        && req.sites.len() > 1
        && req.wan.topology == WanTopology::None
    {
        problems.push(
            "the Internet connection is at the hub, but no WAN joins the sites, so the other \
             sites would have no way to reach it; connect the sites or break out per site"
                .to_string(),
        );
    }

    if !problems.is_empty() {
        return Err(problems);
    }
    // Unreachable: a base that would not parse is already in `problems`.
    let Some(base) = base else {
        return Err(vec![
            "the base address block could not be read; write it as an address and a prefix              length, e.g. 10.20.0.0/16"
                .to_string(),
        ]);
    };

    // -- VLANs --------------------------------------------------------------
    let (parking_vlan, transit_vlan) = housekeeping_vlans(&req);
    let mut sites: Vec<SiteModel> = Vec::new();
    for (index, site) in req.sites.iter().enumerate() {
        let name = {
            let n = one_line(&site.name, 40);
            if n.is_empty() {
                format!("Site {}", index + 1)
            } else {
                n
            }
        };
        let mut site_tag = tag(&name, 10);
        if site_tag.is_empty() {
            site_tag = format!("SITE{}", index + 1);
        }
        while sites.iter().any(|s: &SiteModel| s.tag == site_tag) {
            site_tag = format!("{}{}", tag(&name, 8), index + 1);
            if sites.iter().any(|s: &SiteModel| s.tag == site_tag) {
                site_tag = format!("SITE{}", index + 1);
            }
        }
        let vlans = match site_vlans(
            site,
            &req,
            &[parking_vlan, transit_vlan],
            &mut problems,
            &mut warnings,
            &name,
        ) {
            Some(vlans) => vlans,
            None => continue,
        };
        sites.push(SiteModel {
            name,
            tag: site_tag,
            index,
            users: site.users,
            servers: site.servers,
            printers: site.printers,
            is_hub: index == hub,
            internet: match req.wan.internet {
                InternetEdge::None => false,
                InternetEdge::AtHub => index == hub,
                InternetEdge::PerSite => true,
            },
            vlans,
            block: base,
            counts: Counts::default(),
            ports_per_access: Vec::new(),
            transit: None,
            infra: base,
            loopbacks: Vec::new(),
            next_loopback: 0,
            p2p: Vec::new(),
            next_p2p: 0,
        });
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    // -- how many boxes -----------------------------------------------------
    let wan_links = wan_adjacencies(&req, hub, sites.len());
    for site in &mut sites {
        let wan_here = wan_links
            .iter()
            .filter(|l| l.a == site.index || l.b == site.index)
            .count() as u32;
        size_site(site, &req, wan_here, &mut problems, &mut decisions);
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    // -- the management VLAN can only be sized once the boxes are known -----
    for site in &mut sites {
        let devices = site.counts.cores
            + site.counts.distribution
            + site.counts.access
            + site.counts.edges
            + u32::from(site.counts.host_switch)
            + site.counts.aps
            + u32::from(site.is_hub); // the management station itself
        if let Some(mgmt) = site
            .vlans
            .iter_mut()
            .find(|v| v.purpose == Purpose::Management)
            && mgmt.hosts_raw == 0
        {
            // The bottom of the subnet is reserved for the gateway, the cores
            // and the test hosts, so the devices start above it.
            mgmt.hosts_raw = devices + STATIC_RESERVE + 4;
            mgmt.hosts_needed = with_growth(u64::from(mgmt.hosts_raw), req.growth_pct)
                .min(u64::from(u32::MAX)) as u32;
        }
    }

    // -- addresses ----------------------------------------------------------
    let (infra, wan_links) = carve(
        &req,
        base,
        &mut sites,
        wan_links,
        &mut problems,
        &mut decisions,
    )?;
    if !problems.is_empty() {
        return Err(problems);
    }

    for site in &sites {
        for vlan in &site.vlans {
            if vlan.hosts_needed > BIG_VLAN_HOSTS {
                warnings.push(format!(
                    "VLAN {} ({}) at {} is sized for {} hosts ({}); broadcast domains above {} \
                     hosts are normally split per floor or per building",
                    vlan.id, vlan.name, site.name, vlan.hosts_needed, vlan.subnet, BIG_VLAN_HOSTS
                ));
            }
        }
    }

    Ok(Model {
        req,
        sites,
        base,
        infra,
        parking_vlan,
        transit_vlan,
        wan_links,
        hub,
        decisions,
        warnings,
    })
}

/// Two high VLAN ids nothing else uses: one to park unused ports in, one for
/// the routed link between a pair of cores. (1002-1005 are reserved on real
/// switches, so both stay below that.)
fn housekeeping_vlans(req: &Requirements) -> (u16, u16) {
    let mut taken: Vec<u16> = vec![req.addressing.mgmt_vlan];
    for site in &req.sites {
        for vlan in &site.vlans {
            if let Some(id) = vlan.id {
                taken.push(id);
            }
        }
    }
    let mut free = (900..=998).rev().filter(|id| !taken.contains(id));
    (free.next().unwrap_or(999), free.next().unwrap_or(998))
}

/// The VLANs of one site: the ones asked for, or the ones its counts imply.
fn site_vlans(
    site: &crate::requirements::Site,
    req: &Requirements,
    reserved: &[u16],
    problems: &mut Vec<String>,
    warnings: &mut Vec<String>,
    site_name: &str,
) -> Option<Vec<VlanModel>> {
    let mut drafts: Vec<(Purpose, String, u32, Option<u16>)> = Vec::new();
    if site.vlans.is_empty() {
        if site.users > 0 {
            drafts.push((Purpose::Users, String::new(), site.users, None));
            if site.voice {
                drafts.push((Purpose::Voice, String::new(), site.users, None));
            }
        } else if site.voice {
            warnings.push(format!(
                "{site_name} asked for voice but has no users, so there is no voice VLAN"
            ));
        }
        if site.guest_wifi {
            let guests = site.users.div_ceil(4).max(25);
            drafts.push((Purpose::Guest, String::new(), guests, None));
        }
        if site.servers > 0 {
            drafts.push((Purpose::Servers, String::new(), site.servers, None));
        }
        if site.printers > 0 {
            drafts.push((Purpose::Printers, String::new(), site.printers, None));
        }
    } else {
        for (i, vlan) in site.vlans.iter().enumerate() {
            let VlanReq {
                name,
                purpose,
                hosts,
                id,
            } = vlan;
            if let Some(id) = id
                && !(1..=4094).contains(id)
            {
                problems.push(format!(
                    "VLAN {} of {site_name} has id {id}; VLAN ids run from 1 to 4094",
                    i + 1
                ));
            }
            drafts.push((*purpose, one_line(name, 30), *hosts, *id));
        }
    }
    // Management always exists; its size is worked out once the devices are known.
    if !drafts.iter().any(|(p, ..)| *p == Purpose::Management) {
        drafts.push((
            Purpose::Management,
            String::new(),
            0,
            Some(req.addressing.mgmt_vlan),
        ));
    }
    drafts.sort_by_key(|(purpose, ..)| {
        Purpose::ORDER
            .iter()
            .position(|p| p == purpose)
            .unwrap_or(Purpose::ORDER.len())
    });

    let mut vlans: Vec<VlanModel> = Vec::new();
    let mut used: Vec<u16> = reserved.to_vec();
    let mut names: Vec<String> = Vec::new();
    for (purpose, name, hosts, id) in drafts {
        let id = match id {
            Some(id) if used.contains(&id) => {
                problems.push(format!(
                    "two VLANs of {site_name} are both given id {id}; each VLAN needs its own id"
                ));
                continue;
            }
            Some(id) => id,
            None => {
                let wanted = if purpose == Purpose::Management {
                    req.addressing.mgmt_vlan
                } else {
                    purpose.default_id()
                };
                if used.contains(&wanted) {
                    match (2..=4094).find(|id| !used.contains(id)) {
                        Some(id) => id,
                        None => {
                            problems.push(format!("{site_name} has run out of VLAN ids"));
                            continue;
                        }
                    }
                } else {
                    wanted
                }
            }
        };
        used.push(id);
        let mut vlan_name = tag(&name, 20);
        if vlan_name.is_empty() {
            vlan_name = purpose.default_name().to_string();
        }
        while names.contains(&vlan_name) {
            vlan_name = format!("{}-{id}", tag(&vlan_name, 14));
        }
        names.push(vlan_name.clone());
        let needed = with_growth(u64::from(hosts), req.growth_pct).min(u64::from(u32::MAX)) as u32;
        vlans.push(VlanModel {
            id,
            name: vlan_name,
            purpose,
            hosts_raw: hosts,
            hosts_needed: needed,
            subnet: Net::new(0, 32),
            gateway: 0,
            pool: None,
        });
    }
    Some(vlans)
}

/// Access points, access switches, distribution switches, cores and edges.
fn size_site(
    site: &mut SiteModel,
    req: &Requirements,
    wan_links: u32,
    problems: &mut Vec<String>,
    decisions: &mut Vec<Decision>,
) {
    let aps: u32 = site
        .vlans
        .iter()
        .filter(|v| v.purpose == Purpose::Guest)
        .map(|v| v.hosts_raw.div_ceil(USERS_PER_AP).max(1))
        .sum();
    let host_ports: u32 = site
        .vlans
        .iter()
        .map(|v| match v.purpose {
            Purpose::Users | Purpose::Printers | Purpose::Other => v.hosts_raw,
            _ => 0,
        })
        .fold(0u32, |a, b| a.saturating_add(b))
        .saturating_add(aps);

    let with_room = with_growth(u64::from(host_ports), req.growth_pct);
    let access = with_room
        .div_ceil(u64::from(req.access_ports.max(1)))
        .min(4096) as u32;
    let cores = if req.redundancy == Redundancy::Full {
        2
    } else {
        1
    };
    let internet_edges = if site.internet {
        if req.redundancy == Redundancy::Full {
            2
        } else {
            1
        }
    } else {
        0
    };
    let edges = if internet_edges > 0 {
        internet_edges
    } else {
        u32::from(wan_links > 0)
    };

    // Servers and the management station: on the core when there is one core,
    // on a switch of their own when there are two, so one core going away does
    // not take them with it.
    let core_hosts = site.servers.min(req.hosts_per_vlan) + u32::from(site.is_hub);
    let host_switch = core_hosts > 0 && cores == 2;
    let core_host_ports = if host_switch { 0 } else { core_hosts };

    // Does the access layer fit straight onto the core?
    let links_per_child = if req.redundancy == Redundancy::Uplinks {
        2
    } else {
        1
    };
    let core_free = CORE_PORTS
        .saturating_sub(1) // the edge router
        .saturating_sub(core_host_ports)
        .saturating_sub(u32::from(host_switch) * links_per_child);
    let direct_capacity = core_free / links_per_child;
    let total_children = access + u32::from(host_switch);
    let distribution = if total_children <= direct_capacity {
        0
    } else {
        let per_dist = DIST_DOWNLINKS / links_per_child;
        let groups = access.div_ceil(per_dist.max(1));
        if req.redundancy == Redundancy::Full {
            groups * 2
        } else {
            groups
        }
    };
    if distribution > 0 && distribution * links_per_child > core_free {
        problems.push(format!(
            "{} needs {access} access switches, which is more than a collapsed core can carry \
             even behind a distribution layer ({distribution} distribution switches would need \
             {} core ports, and the core has {core_free} free); split the site into buildings",
            site.name,
            distribution * links_per_child
        ));
    }

    if access > 0 {
        decisions.push(Decision::site(
            &site.name,
            "Access switches",
            format!(
                "{access} access switch{} at {}: {host_ports} host ports needed \
                 ({} users + {} printers + {aps} access point{}), {}% growth head-room \
                 makes {with_room}, {} ports per switch.",
                if access == 1 { "" } else { "es" },
                site.name,
                site.users,
                site.printers,
                if aps == 1 { "" } else { "s" },
                req.growth_pct,
                req.access_ports,
            ),
        ));
    }
    if distribution > 0 {
        decisions.push(Decision::site(
            &site.name,
            "Distribution layer",
            format!(
                "{distribution} distribution switch{} at {}: the core has {core_free} free \
                 downlink ports and {access} access switches need {}, so the access layer is \
                 aggregated first.",
                if distribution == 1 { "" } else { "es" },
                site.name,
                access * links_per_child
            ),
        ));
    }

    // Spread the host ports evenly so every switch keeps the same head-room.
    let mut per_access = Vec::new();
    if let Some(share) = host_ports.checked_div(access) {
        let rem = host_ports % access;
        for i in 0..access {
            per_access.push(share + u32::from(i < rem));
        }
    }

    site.counts = Counts {
        cores,
        distribution,
        access,
        edges,
        aps,
        host_switch,
    };
    site.ports_per_access = per_access;
}

/// Which sites get a link to which.
fn wan_adjacencies(req: &Requirements, hub: usize, sites: usize) -> Vec<WanLink> {
    let mut links = Vec::new();
    let zero = Net::new(0, 32);
    match req.wan.topology {
        WanTopology::None => {}
        WanTopology::HubAndSpoke => {
            for i in 0..sites {
                if i != hub {
                    links.push(WanLink {
                        a: hub,
                        b: i,
                        subnet: zero,
                    });
                }
            }
        }
        WanTopology::FullMesh => {
            for i in 0..sites {
                for j in (i + 1)..sites {
                    links.push(WanLink {
                        a: i,
                        b: j,
                        subnet: zero,
                    });
                }
            }
        }
    }
    links
}

type Carved = (Net, Vec<WanLink>);

/// Split the base block: an infrastructure block at the top for loopbacks and
/// point-to-point links, equal site blocks below it, then VLSM inside each site.
fn carve(
    req: &Requirements,
    base: Net,
    sites: &mut [SiteModel],
    mut wan_links: Vec<WanLink>,
    problems: &mut Vec<String>,
    decisions: &mut Vec<Decision>,
) -> Result<Carved, Vec<String>> {
    // Each site gets its own slice of the infrastructure block, so its
    // loopbacks and transit links summarise to one prefix the same way its
    // VLANs do. A WAN link belongs to the lower-numbered of the two sites.
    let p2p_size = 1u64 << (32 - u32::from(req.addressing.p2p_len));
    let needs: Vec<(u32, u32)> = sites
        .iter()
        .map(|site| {
            let loopbacks = site.counts.cores + site.counts.edges;
            let p2p = u32::from(site.counts.cores == 2)
                + site.counts.edges
                + wan_links
                    .iter()
                    .filter(|l| l.a.min(l.b) == site.index)
                    .count() as u32;
            (loopbacks, p2p)
        })
        .collect();
    let per_site_infra = needs
        .iter()
        .map(|(loopbacks, p2p)| {
            let lo = u64::from(*loopbacks)
                .max(1)
                .next_power_of_two()
                .max(p2p_size);
            let links = (u64::from(*p2p).max(1) * p2p_size).next_power_of_two();
            lo + links
        })
        .max()
        .unwrap_or(1)
        .next_power_of_two();
    let infra_len = prefix_for_block(per_site_infra * sites.len() as u64);
    let infra_size = 1u64 << (32 - u32::from(infra_len));

    // Site blocks are sized for the largest site, so every site is the same
    // shape and summarises to one prefix.
    let per_site_need: u64 = sites
        .iter()
        .map(|site| {
            site.vlans
                .iter()
                .map(|v| {
                    let hosts = v.hosts_needed.max(MIN_VLAN_HOSTS);
                    prefix_for_hosts(u64::from(hosts))
                        .map(|len| 1u64 << (32 - u32::from(len)))
                        .unwrap_or(1u64 << 32)
                })
                .fold(0u64, u64::saturating_add)
        })
        .max()
        .unwrap_or(0)
        .max(1);
    let site_len = match req.addressing.per_site_len {
        Some(len) if len > 30 => {
            problems.push(format!(
                "per_site_len is /{len}; a site block has to hold its VLANs, so it needs to be /30 or shorter"
            ));
            return Err(problems.clone());
        }
        Some(len) => {
            let have = 1u64 << (32 - u32::from(len));
            if have < per_site_need {
                problems.push(format!(
                    "per_site_len is /{len} ({have} addresses) but the largest site needs \
                     {per_site_need}; use /{} or let the design size the blocks",
                    prefix_for_block(per_site_need)
                ));
                return Err(problems.clone());
            }
            len
        }
        None => prefix_for_block(per_site_need),
    };
    let site_size = 1u64 << (32 - u32::from(site_len));
    let needed = site_size * sites.len() as u64 + infra_size;
    if site_len < base.len || needed > base.size() {
        problems.push(format!(
            "the base block {base} holds {} addresses but this design needs {needed}: \
             {} site{} of {site_size} addresses (a /{site_len} each, sized for the largest site) \
             and {infra_size} for loopbacks and point-to-point links. Use {} or shorter.",
            base.size(),
            sites.len(),
            if sites.len() == 1 { "" } else { "s" },
            Net::new(base.addr, prefix_for_block(needed))
        ));
        return Err(problems.clone());
    }
    let infra = Net::new((base.last() as u64 + 1 - infra_size) as u32, infra_len);
    decisions.push(Decision::global(
        "Addressing",
        format!(
            "{base} is split into {} site block{} of /{site_len} from the bottom and {infra} at \
             the top, sliced per site for loopbacks and point-to-point links, so every site \
             summarises to two prefixes and the infrastructure addresses stay out of the user \
             space.",
            sites.len(),
            if sites.len() == 1 { "" } else { "s" },
        ),
    ));

    // Loopbacks first, then the point-to-point blocks, inside each site's own
    // slice of the infrastructure block.
    for (i, site) in sites.iter_mut().enumerate() {
        let slice = Net::new(
            (infra.addr as u64 + per_site_infra * i as u64) as u32,
            prefix_for_block(per_site_infra),
        );
        let (loopbacks, p2p) = needs[i];
        let lo_region = u64::from(loopbacks)
            .max(1)
            .next_power_of_two()
            .max(p2p_size);
        site.infra = slice;
        site.loopbacks = (0..loopbacks).filter_map(|n| slice.nth(n)).collect();
        site.p2p = (0..u64::from(p2p))
            .map(|n| {
                Net::new(
                    (slice.addr as u64 + lo_region + n * p2p_size) as u32,
                    req.addressing.p2p_len,
                )
            })
            .filter(|n| slice.contains(n.last()))
            .collect();
        if site.loopbacks.len() < loopbacks as usize || site.p2p.len() < p2p as usize {
            problems.push(format!(
                "the infrastructure slice {slice} of {} cannot hold {loopbacks} loopbacks and \
                 {p2p} point-to-point links; widen the base block",
                site.name
            ));
        }
    }
    if !problems.is_empty() {
        return Err(problems.clone());
    }
    // A WAN link's subnet comes out of the lower-numbered site's slice, after
    // that site's own transit and edge links.
    let mut taken: Vec<usize> = vec![0; sites.len()];
    for link in wan_links.iter_mut() {
        let owner = link.a.min(link.b);
        let offset = usize::from(sites[owner].counts.cores == 2)
            + sites[owner].counts.edges as usize
            + taken[owner];
        taken[owner] += 1;
        match sites[owner].p2p.get(offset) {
            Some(net) => link.subnet = *net,
            None => problems.push(format!(
                "the infrastructure slice of {} has no block left for the WAN link to {}",
                sites[owner].name,
                sites[link.a.max(link.b)].name
            )),
        }
    }
    if !problems.is_empty() {
        return Err(problems.clone());
    }

    // Inside each site, largest VLAN first.
    for (i, site) in sites.iter_mut().enumerate() {
        let block = Net::new((base.addr as u64 + site_size * i as u64) as u32, site_len);
        site.block = block;
        let requests: Vec<vlsm::Request> = site
            .vlans
            .iter()
            .map(|v| vlsm::Request {
                name: format!("vlan{}", v.id),
                hosts: u64::from(v.hosts_needed.max(MIN_VLAN_HOSTS)),
            })
            .collect();
        let plan = match vlsm::plan(&block.to_string(), &requests, false) {
            Ok(plan) => plan,
            Err(error) => {
                problems.push(format!(
                    "the VLANs of {} do not fit in {block}: {error}",
                    site.name
                ));
                continue;
            }
        };
        for vlan in &mut site.vlans {
            let key = format!("vlan{}", vlan.id);
            let Some(allocation) = plan.allocations.iter().find(|a| a.name == key) else {
                problems.push(format!(
                    "VLAN {} of {} was not given a subnet",
                    vlan.id, site.name
                ));
                continue;
            };
            let Some(net) = Net::parse(&allocation.cidr) else {
                problems.push(format!(
                    "VLAN {} of {} was given the subnet \"{}\", which is not a block",
                    vlan.id, site.name, allocation.cidr
                ));
                continue;
            };
            vlan.subnet = net;
            vlan.gateway = net.first_host();
            if req.services.dhcp && vlan.purpose.wants_dhcp() {
                let first = net.first_host().saturating_add(STATIC_RESERVE);
                let last = net.last_host();
                if first <= last {
                    vlan.pool = Some((first, last));
                }
            }
        }
        // Every VLAN of the site must have landed somewhere sane.
        for a in 0..site.vlans.len() {
            for b in (a + 1)..site.vlans.len() {
                if site.vlans[a].subnet.overlaps(site.vlans[b].subnet) {
                    problems.push(format!(
                        "VLANs {} and {} of {} were given overlapping subnets ({} and {})",
                        site.vlans[a].id,
                        site.vlans[b].id,
                        site.name,
                        site.vlans[a].subnet,
                        site.vlans[b].subnet
                    ));
                }
            }
        }
        decisions.push(Decision::site(
            &site.name,
            "Site block",
            format!(
                "{} gets {block}: {} VLAN{} carved largest-first, {} addresses still free.",
                site.name,
                site.vlans.len(),
                if site.vlans.len() == 1 { "" } else { "s" },
                block.size() - plan.used_addresses,
            ),
        ));
    }
    if !problems.is_empty() {
        return Err(problems.clone());
    }
    let _ = fmt_ip(0);
    Ok((infra, wan_links))
}
