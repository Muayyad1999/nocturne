# nocturne-topology — running notes

Spec: `docs/dev/features/topology.md`. Pure computation over `collect::latest_all`
plus `inventory`; **no device access at all**. A "Refresh" in the UI is a
`collect.run` followed by a `topology.get`.

## Shape of the crate

| File | What it does |
|---|---|
| `model.rs` | the graph types the UI is written against, and the settings |
| `data.rs` | everything collected, read once, and the identity index |
| `physical.rs` | LLDP/CDP/MNDP reports → links (the hard part) |
| `l3.rs` | interfaces that share a subnet → links and groups |
| `endpoints.rs` | MAC tables on edge ports → host nodes; port classification |
| `graph.rs` | puts it together: nodes, lab links, aggregates, decoration |
| `layout.rs` | layered / force / radial, all deterministic |
| `views.rs`, `manual.rs`, `history.rs` | saved views and positions, drawn links, snapshots |
| `path.rs`, `locate.rs` | the two questions that walk the collected tables |
| `rates.rs`, `paged.rs` | monitoring's rates for the ports links end on; reading another feature's list to its end |
| `export/` | svg, drawio, mermaid, dot, json |
| `api.rs` | the RPC surface and the debounced `topology.changed` watcher |

## Decisions

* **Node ids** are stable strings: managed devices use the inventory
  `device_id`; unmanaged neighbours `u:<key>` (key = first DNS label of the
  reported name, or the bare chassis MAC, or the management address);
  endpoints `h:<bare mac>`; manual-only nodes `m:<slug>`. Link ids are a
  64-bit FNV of the link's signature, so they survive a rebuild.
* **Identity matching** (neighbour → inventory) is exact, never fuzzy, in this
  order: chassis MAC (any notation) against collected interface MACs → full
  name against device name / `facts.hostname` → first DNS label against the
  same → management address against `device.address` or any collected
  interface IP. A name two devices answer to is refused (and warned about)
  rather than guessed. Cisco's `SW1(FOC1234X5)` has the serial stripped first.
* **Link merge**: every neighbour row is one directed observation
  `(from, from_if) → (to, to_if)`. Identical observations merge, so LLDP and
  CDP from one end are one link with two protocols. Exact mirrors are paired
  first (that is what keeps parallel port-channel members matched port to
  port), then looser ones where an end did not name the remote port. Paired =
  `confirmed`, unpaired = `one_sided`.
* **Confidence** is *how we know*, not *how sure we are*.
* **Port-channel folding** only when it can be justified: two or more parallel
  links between the same pair and exactly one aggregate interface
  (`Port-channel*`/`Bundle-Ether*`/`bond*`/`ae*`) at each end that is not
  itself a link end. Otherwise the members stay separate — the collected
  `Interface` has no membership list, and honest beats tidy.
* **A folded link's speed is its members', added up** — computed in
  `fold_aggregates`, where the members are still known. It used to be the sum
  of the link's two ends, which for a folded link are the two aggregate
  interfaces: a 4 x 10G bundle whose Port-channels each report 40 Gb/s came
  out as 80 Gb/s and every percentage on it was halved. What a Port-channel
  row says of its own speed is not dependable either way (the bundle over
  SNMP, one member in some CLI output), so it is only the fallback, and then
  the smaller end, once.
* **The join to monitoring is made here** (`rates.rs`), not by the screen. A
  link end is always `ifname::long`; monitoring keeps a device's own spelling
  for `interface_names: verbatim` profiles. Both names go through
  `ifname::long` (lower-cased, which is `ifname::same` as a map key) and the
  answer is keyed by the spelling that was asked with. The screen had its own
  table of fifteen abbreviations for this and lost every IOS XR port.
  The canonical form finds a port; it must not merge two. SONiC has `eth0`
  (management) and `Ethernet0` (front panel), both `Ethernet0` to `ifname`,
  and joined on that alone the uplink got the management port's rates. So the
  lookup is by monitoring's own name first, and by canonical form only when
  one port of the device has it; an ambiguous spelling is left unanswered.
  (`Collected::interface` still takes the first `ifname::same` match — the
  collected side of the same ambiguity is not handled.)
* **Another feature's list is read to its end** (`paged.rs`). One call of
  `monitor.interfaces` is at most 5 000 rows and of `alerts.list` 2 000; what
  is past that looked like "not measured" and "no alerts". The page size asked
  for is inside the callee's clamp on purpose: a page is only known to be the
  last when it is shorter than what was asked for. A callee that never stops
  returning full pages is refused after 200 of them rather than followed.
  An offset cursor repeats a row when the list moves between two pages (an
  alert fires, a device is polled again), so a row is taken once: an alert by
  its `id`, a port by its name. A row that moved the other way and was never
  returned cannot be seen from here.
* **L3**: an address that appears on more than one device is an HSRP/VRRP
  virtual address: dropped, with a warning. Loopbacks and /32s never make
  links. Two interfaces in a subnet → a link (which covers /30 and /31);
  three or more → a `subnet` group.
* **Staleness** never hides anything: stale devices are counted, named in
  `warnings`, and their links are still drawn.
* **Layout** is a pure function of the graph — ties break on the node *name*,
  never on the (random) id, so `ACC02` comes before `ACC10`. A rank too wide
  for a drawing is wrapped onto several lines, at site boundaries where it
  can; each line is centred, then slid under whatever it connects to, clamped
  to stay inside the drawing.
* **Positions** are per view. `topology.get` uses stored ones, lays the whole
  graph out when there are none, and places new nodes next to their
  neighbours (nearest free lattice point) without moving anything placed.
* Dragging nodes is not audited — a position is not something anyone needs to
  find in the audit trail later. Views and manual links are.

## What the drawing was judged on

The three-tier campus in `scenarios.rs` was exported and rendered to PNG with
Chromium. The first attempt was a 5435 × 758 strip with the access switches in
one row, the names in random order (ties broken by device id), text spilling
out of the cards, labels piled on top of each other around the 12-link
distribution switches, and the access point's link running straight through
two cards it was not connected to. Fixed, in order: name-order tie-breaks,
row wrapping, wider cards with clipping that fits, labels only on ports of
nodes with six links or fewer, links that skip a row bowed away from the
nearest card, group outlines chosen per layer and drawn all-or-nothing for
sites. The result is 1238 × 1058 and readable.

Speed, 500 nodes, release build on four cores: layered 5 ms, force 60 ms,
radial 2 ms. The test asserts a much looser bound because it runs unoptimised
alongside the rest of the suite.

Known limit: dist→access edges still cross a lot. Straight lines between
ranks always do when four nodes fan out to twenty; an orthogonal router would
fix it and is a bigger piece of work than it is worth here.

## Wants from the core

* `Interface` has no aggregate membership, so port-channel folding has to be
  inferred from parallel links plus an unused `Port-channel` interface.
* `Neighbor` has no remote chassis MAC separate from `device`, and no TTL or
  age, so staleness is per collection rather than per neighbour.
* `collect::Stored` has no per-row timestamp, so `locate.host` reports the
  collection time of the whole MAC table as `seen_at`.
* Nothing in the core says which interfaces are members of a VRF or an MST
  instance, which is why `topology.path` only follows the default VRF (it
  notes when it used another).
