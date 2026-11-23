# Feature: topology (`nocturne-topology`)

The network drawn from what the devices themselves report: who is connected
to whom (LLDP/CDP/MNDP), which subnets they share, where a host is plugged
in, and the path between two points. Automatic, laid out sensibly, exportable
— and honest about what it inferred versus what it was told.

Engines: core `collect` (`latest_all` of neighbors, interfaces, mac_table,
arp, routes, vlans, facts), `inventory`; `nocturne-tools` (subnet maths, MAC
vendor); `nocturne-drivers::ifname`. Pure computation: no device access here
(a "Refresh" in the UI calls `collect.run`).

## Graph (contract)

`topology.get {view?: "default", layer?: physical|l3, filter?: {site, tag,
role, lab_id, device_ids}, include_endpoints?: false}` →

```rust
struct Graph { nodes: Vec<Node>, links: Vec<Link>, groups: Vec<Group>, warnings: Vec<String>,
               stats: {devices, links, unmanaged, stale_devices}, generated_at, data_age: {oldest_at, newest_at} }
struct Node { id, kind: router|switch|l3_switch|firewall|access_point|server|host|cloud|unknown,
    device_id?,                // absent for unmanaged neighbours and inferred endpoints
    name, address?, site?, role?, profile?, model?, lab: bool,
    status: up|down|unknown,   // from DeviceState
    unmanaged: bool,           // seen by a neighbour protocol but not in the inventory
    x, y, pinned: bool,        // position in this view
    alerts?: u32 }
struct Link { id, a: End, b: End,
    kind: lldp|cdp|mndp|subnet|lab|manual|mac,       // how we know
    confidence: confirmed|one_sided|inferred,        // both ends report it | only one end | derived
    status: up|down|unknown, speed_bps?, util_pct?,  // from collected interfaces / monitor metrics when present
                                                     // speed: the slower end; a folded port-channel: its members added up
    util_at?,                                        // when monitoring measured util_pct (the busier end's poll)
    vlans?: String, aggregate?: String,              // trunk allowed list, port-channel name
    label? }
struct End { node, interface? /* canonical name */, ip? }
struct Group { id, kind: site|subnet|lab, label, nodes: [node id] }
```

Building rules:

* **Physical**: one link per pair of interfaces. LLDP and CDP reports of the
  same link from both ends merge (`confirmed`); match remote devices to
  inventory by collected hostname/sysName, management address, or chassis
  MAC — never by name similarity. Member links of a port-channel are folded
  into one link with `aggregate` (and a count) when both ends agree. The
  folded link's `speed_bps` is what its members can carry together: the sum
  over the members of the slower of each member's two ports. It is never the
  two ends added up — the two ends are the two aggregate interfaces, and each
  already speaks for the whole bundle (or, on some platforms, for one
  member). When a member's speed was not collected the sum is not known, and
  the link takes the smaller of what the two aggregate interfaces report, or
  no speed at all.
  Neighbours not in the inventory become `unmanaged` nodes (one per distinct
  chassis/name), which the UI offers to add. Lab links come from the lab
  feature (`labs.links` RPC if present) and are authoritative for lab nodes.
* **L3**: interfaces sharing a subnet (from collected interface IPs) are
  joined through a `subnet` group; point-to-point /30 and /31 become direct
  links. HSRP/VRRP virtual addresses and loopbacks are not links.
* **Endpoints** (`include_endpoints`): MAC-table entries on access ports
  (ports with no network neighbour and ≤ 3 MACs) become `host` nodes named
  from ARP/IPAM/DNS when known, else by vendor + MAC suffix; `kind: mac`,
  `confidence: inferred`.
* Stale data is flagged, not hidden: a device whose neighbour data is older
  than 24 h (setting) contributes links marked in `warnings`/`stale_devices`.
* Manual links and annotations fill what discovery cannot see (unmanaged
  switches, WAN circuits): `topology.manual.save/delete`.
* What other features know is asked for by name and read **to the end**.
  `monitor.interfaces` (for `util_pct` and `topology.rates`) and
  `alerts.list` (for `Node.alerts`) page by offset and answer with at most a
  few thousand rows a call; the first page alone would make a port further
  down the table look unmeasured and a device further down look quiet.
  Monitoring's rows are read per device, for the devices the links end on. A
  row an offset cursor returns twice because the list moved between pages is
  taken once (an alert by `id`, a port by name). A build without the feature
  (`unknown_method`) is carried on without.
* A feature that is there and **refuses** is said to have, in `warnings`,
  because what it would have added looks like nothing when it is missing:
  `open alerts could not be read (…)` (no node then has `alerts`, and a node
  without them may still have alerts open), `link utilisation could not be
  read from monitoring (…)` (no link has `util_pct`), and `the lab feature's
  wiring could not be read (…)` (links between lab devices are drawn from
  what they report). These name no device, and `topology.neighbors` passes
  them on with the warnings that name its device.
* `util_pct` is monitoring's last reading of the busier end, whatever its
  age — monitoring keeps a silent device's last row — so it comes with
  `util_at`, the time of that poll; age it against `generated_at`, as
  `topology.rates` readings are aged against its `at`. A reading monitoring
  gave no time has no `util_at`.

## Layout

Positions are stored per view (`topology.layout.save {view, positions:
{node_id: {x, y, pinned}}}`); nodes without one are placed by
`topology.autolayout {view, algorithm?: layered|force|radial, filter?}` →
positions. Deterministic (same graph → same picture). `layered` is the
default for campus networks: rank by role (core/edge at the top, then
distribution, access, endpoints), order within ranks to minimise crossings
(barycentre sweeps), sites as columns; fall back to graph distance from the
highest-degree node when roles are missing. `force` for meshes; `radial`
around a chosen root. New nodes appearing later are placed near their
neighbours without moving pinned ones.

## Other RPC (contract)

| Method | Params → Result |
|---|---|
| `topology.views` / `.view_save` / `.view_delete` | named views `{id, name, layer, filter, include_endpoints}` |
| `topology.neighbors` | `{device_id}` → `{device_id, device, rows: [{link_id, local_interface?, local_ip?, peer: Node, peer_interface?, peer_ip?, kind, confidence, status, speed_bps?, util_pct?, util_at?, vlans?, aggregate?, label?, protocols, members}], warnings}` — the device's links with both ends resolved (the device page's "connected to" table); `warnings` are the graph's that name the device, and those about every device (a feature that could not answer) |
| `topology.rates` | `{ports: [{device_id, interface}]}` → `{at, monitoring: bool, ports: [{device_id, interface, name, in_bps?, out_bps?, speed_bps?, at?}]}` — what monitoring last measured on the ports asked about (the map asks about the ports its links end on, and draws the link load from the answer). One entry per port monitoring has a row for, under the `interface` spelling it was **asked with**; `name` is monitoring's own. The join is made here, with `ifname` on both names: a link end is always canonical (`TenGigabitEthernet0/0/0/1`) while monitoring keeps the device's spelling for profiles with `interface_names: verbatim` (`TenGigE0/0/0/1`), so a caller comparing the two as text loses the port. The canonical form finds a port spelled differently; it does not merge two: a port is looked up by the name monitoring keeps first, and by canonical form only when exactly one of the device's ports has it, so `Ethernet0` on a device that also has `eth0` is answered with its own row, and a spelling that could mean either is not answered. A rate that could not be measured is absent, never `0`. Top-level `at` is the server's clock when it answered: a reading's age is its own `at` taken from that, never from the caller's clock. `monitoring: false` means the monitor feature is not in the build (no rates to be had), which is not the same as nothing having been measured (`monitoring: true`, `ports: []`); any other refusal by the monitor is this method's error |
| `topology.path` | `{from, to}` (device ids, names or IP addresses) → `{hops: [{node, device_id?, in_interface?, out_interface?, via: route|arp|mac|connected, detail}], complete: bool, stopped_because?}` — walks collected routing tables (longest prefix match), then ARP and MAC tables for the last hop; stops honestly ("no route data collected for R3") instead of guessing |
| `locate.host` | `{query}` (IP, MAC, or hostname) → `{mac?, ip?, vendor?, locations: [{device_id, device, interface, vlan?, kind: access|trunk|uplink, seen_at}], best?: index, how: [..]}` — "which switch port is this plugged into": resolve IP→MAC through collected ARP tables, find the MAC in MAC tables, prefer the port with no network neighbour (the edge), report the trunk sightings as the path |
| `topology.changes` | `{since}` → links and nodes that appeared/disappeared between stored snapshots (a snapshot of the physical graph is stored whenever it changes after a collect) |
| `topology.export` | `{view?, format: svg|drawio|mermaid|dot|json}` → `{text}` — SVG is a clean standalone drawing (icons as simple shapes, labels, a legend, lab/unmanaged styling); draw.io XML opens in diagrams.net with editable shapes |

Event (contract): `topology.changed {reason}` (after `collect.updated`,
inventory or manual-link changes; debounced 1 s).

## Tests

Hand-built collected data (`collect::store`) for: a three-tier campus with
LLDP on both ends, CDP on one end only, a port-channel, an unmanaged
neighbour, hostname vs FQDN vs chassis-MAC matching, interface-name forms
(`Gi0/1` vs `GigabitEthernet0/1`), duplicate reports; L3 view with shared
/24s, /30s, loopbacks, secondary addresses; endpoints; staleness. A
port-channel's speed for bundles whose aggregate interfaces report the whole
bundle, one member, or nothing, and whose two ends disagree. `topology.rates` against a stand-in
`monitor.interfaces` that keeps the real one's paging contract: a port whose
device sorts past the first five thousand rows, a device with more rows than
a page, the IOS XR and other spellings `ifname` rewrites, two ports of one
device with one canonical form, a build without monitoring, a monitor that
fails (and the drawing's warning), a table that never ends, a reading twenty
hours old (`util_at`). `alerts.list` and `labs.links` that refuse: a warning,
never a clean card. Layout:
determinism, no overlapping nodes, pinned nodes do not move, a 500-node graph
lays out in under a second. Path: connected, static, default route, ECMP
(report the first with a note), missing data stops. `locate.host` across a
core + two access switches with the MAC seen on trunks and one access port.
Exports parse (SVG as XML, draw.io as XML, DOT syntax, Mermaid by a simple
grammar check) and contain every node; hostile names are escaped.
