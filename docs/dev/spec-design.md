# Spec: `nocturne-design` — automated network design

"We are opening a branch with 120 people, voice, guest Wi-Fi, two uplinks —
what do I build?" This crate answers with a complete, working design: the
topology, the VLANs and addressing plan, the device list, and the full
configuration of every device — as a lab that boots in the simulator and
proves itself by pinging across it.

A pure library: requirements in, design out. Deterministic (same input, same
design, byte for byte). No I/O, no clocks, no randomness beyond the seed in
the requirements. Depends on `nocturne-netsim` (for `LabSpec`, `catalog()`,
and to validate its own output in tests) and `nocturne-tools` (VLSM and
subnet arithmetic).

## Public API (contract)

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Requirements {
    pub name: String,                       // "Acme branch"
    pub sites: Vec<Site>,                   // at least one
    pub redundancy: Redundancy,             // None | Uplinks | Full
    pub wan: Wan,                           // how sites interconnect + Internet
    pub addressing: Addressing,
    pub routing: Routing,                   // Static | Ospf
    pub growth_pct: u32,                    // head-room on ports and subnets (default 25)
    pub access_ports: u32,                  // ports per access switch: 24 | 48 (default 24)
    pub services: Services,                 // dhcp: bool, nat: bool, ntp/syslog/snmp targets (strings, optional)
    pub hosts_per_vlan: u32,                // sample end hosts to place per user VLAN for testing (default 1, max 4)
    pub seed: u64,
}
pub struct Site { pub name: String, pub users: u32, pub voice: bool, pub guest_wifi: bool, pub servers: u32, pub printers: u32,
                  pub vlans: Vec<VlanReq> /* optional explicit VLANs: {name, purpose, hosts, id?} — empty = derive from the counts */ }
pub enum Redundancy { None, Uplinks, Full }
pub struct Wan { pub topology: WanTopology /* None | HubAndSpoke | FullMesh */, pub hub: Option<String>, pub internet: InternetEdge /* None | AtHub | PerSite */ }
pub struct Addressing { pub base: String /* "10.20.0.0/16" */, pub per_site_len: Option<u8>, pub p2p_len: u8 /* 30 | 31 */, pub mgmt_vlan: u16 /* default 99 */ }

pub struct Design {
    pub lab: nocturne_netsim::LabSpec,              // nodes with full startup-configs, links, positions, notes
    pub plan: Plan,
    pub decisions: Vec<Decision>,                   // what was chosen and why, in order ("2 access switches at HQ: 61 ports needed incl. 25 % growth, 24 per switch → 3 … ")
    pub warnings: Vec<String>,                      // requirements that could not be met exactly and what was done instead
}
pub struct Plan {
    pub sites: Vec<SitePlan>,                       // per site: block, devices, vlans
    pub vlans: Vec<VlanPlan>,                       // site, id, name, purpose, subnet, gateway, dhcp range, hosts needed / available
    pub links: Vec<LinkPlan>,                       // a, a_if, b, b_if, kind: access_uplink|core_interlink|wan|internet, subnet (for routed links)
    pub devices: Vec<DevicePlan>,                   // name, site, role: core|distribution|access|edge|host, model, mgmt_ip, loopback, interfaces used
    pub bill_of_materials: Vec<BomLine>,            // role/class, quantity, ports — generic classes, never invented part numbers or prices
    pub checks: Vec<Check>,                         // what a correct build must satisfy: {from, to, expect: reachable|blocked, why}
}

pub fn generate(requirements: &Requirements) -> Result<Design, DesignError>;
/// Problems with the requirements, all at once, in user terms ("the base block 10.0.0.0/24 holds 254 hosts but the sites need 418").
pub fn validate(requirements: &Requirements) -> Vec<String>;
/// The plan as a Markdown document (addressing table, VLAN table, cabling list, BOM, decisions).
pub fn document(design: &Design) -> String;
pub fn examples() -> Vec<(String, Requirements)>;   // "Small office", "Campus with two buildings", "HQ and three branches"
```

All structs `Serialize + Deserialize + Clone + Debug`, enums `snake_case`,
`#[serde(default)]` with sensible defaults so a UI or an AI model can send
partial JSON.

## Design rules

* **Per site**: a collapsed core. `Redundancy::None` → one L3 switch;
  `Uplinks` → one L3 core, every access switch dual-homed with an
  EtherChannel if the simulator supports it, else two uplinks with STP;
  `Full` → two L3 cores with an interlink, access switches dual-homed, a
  first-hop redundancy protocol on the SVIs **only if the simulator
  implements it** (check the catalog/CLI; otherwise say so in `warnings` and
  build the STP-based fallback). Access switches = ceil(ports needed ×
  (1 + growth) / access_ports), ports needed = users (+ users again when
  `voice` uses separate ports is *not* assumed: phones daisy-chain) +
  printers + APs (one per 40 guest/wireless users, minimum one when
  `guest_wifi`). Servers hang off the core (or a server access switch when
  more than a core's free ports).
* **VLANs** (when not given): users, voice (if `voice`), guest (if
  `guest_wifi`), servers (if any), printers (if > 0), management. Stable
  ids (10, 20, 30, 40, 50, mgmt) per site; names say what they are.
* **Addressing**: carve the base block per site (equal power-of-two blocks
  sized for the largest site with growth, or `per_site_len`), then VLSM
  inside the site largest-first with `growth_pct` head-room; gateways are
  the first usable address; DHCP ranges leave the first 10 addresses static;
  loopbacks and point-to-point links come from a dedicated block at the end
  of the base. Everything is checked for overlap; running out of space is a
  `validate` error that states how much is needed.
* **Routing**: SVIs on the core; `Static` → default routes towards the edge
  and summary statics back; `Ospf` → area 0 on core/edge links and WAN
  links, passive on user SVIs, router-ids from loopbacks.
* **WAN / Internet**: sites connect router-to-router over /30 (or /31)
  links per `topology`; the Internet edge does NAT overload for the internal
  blocks towards an `internet` cloud node with a public-looking documentation
  address (`198.51.100.0/24`, `203.0.113.0/24`), and guest VLANs are
  ACL-blocked from internal blocks.
* **Hardening baseline** on every device: hostname, enable secret, a local
  user, SSH-only VTYs with a management ACL, banner, `service
  password-encryption`, NTP/syslog/SNMP when targets are given, unused
  access ports shut and parked in an unused VLAN, portfast + BPDU guard on
  host ports, descriptions on every used interface naming the far end.
  Passwords in generated configs are obvious placeholders the document tells
  the user to replace — never a real-looking secret.
* **Configs are for the simulator's IOS dialect** — only commands it
  implements (read `docs/dev/spec-netsim.md` → "CLI: command coverage" and
  test against the real thing). A design the simulator cannot run is a bug.
* **Layout**: positions computed so the picture reads top-down (edge, core,
  access, hosts), sites side by side, no overlaps, a note box per site.
* **Checks**: derive the expected reachability matrix from the design
  itself (user ↔ gateway, inter-VLAN, inter-site, Internet, guest isolation).

## Validation you must do

1. Unit tests for the arithmetic: switch counts, VLSM carving, block
   exhaustion, p2p allocation, determinism (two runs equal; changing `seed`
   changes only what the seed is documented to affect).
2. **Every example and a generated matrix of requirements** (1–4 sites ×
   each redundancy × each WAN topology × static/OSPF × voice/guest on/off,
   users from 5 to 2,000): `generate`, load `design.lab` with
   `nocturne_netsim::Lab::from_spec`, `settle`, and evaluate every `Check`
   with `probe_ping`/`probe_tcp`. All must hold. Also assert every config
   line was accepted by the simulator (`configure`/boot reports no rejected
   lines), no duplicate IPs, no overlapping subnets, every link's two ends
   in the same subnet/VLAN set.
3. `document` renders tables that match the plan (spot-check by parsing the
   Markdown tables back).
4. `cargo clippy -p nocturne-design --all-targets -- -D warnings`, fmt, no
   `unsafe`, no panics on any `Requirements` (fuzz it: zero users, 0 sites,
   absurd counts, base blocks that are too small, /32, IPv6 base → clear
   `validate` errors).
