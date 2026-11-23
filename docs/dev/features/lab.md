# Feature: lab (`nocturne-lab`)

A network you can break. Drag routers, switches and hosts onto a canvas,
cable them, open their consoles, watch packets move hop by hop — and, because
every lab device is also an inventory device, point every other Nocturne
feature at it: monitor it, back it up, push a change through the real
pipeline, let the assistant loose on it. It is how a user tries Nocturne
with no hardware, how a change is rehearsed, and how a design is proven
before anything is bought.

Engines: `nocturne-netsim` (read `docs/dev/spec-netsim.md` — `Lab`,
`LabSpec`, `catalog()`, consoles, probes, traces, flows, faults,
`demo_campus()`, `examples()`), `nocturne-design` (`docs/dev/spec-design.md`:
requirements → lab + plan), `nocturne-transport` (`Connection::pair`,
`serve::{serve_ssh, serve_telnet}`), core `connect::SimConnector`,
`inventory`, `collect::store`, `store.log_append`.

The simulator is a deterministic library with a `&mut self` API and its own
clock; this crate gives it a life: real-time ticking, consoles as
`Connection`s, persistence, and inventory sync.

## Runtime

* A lab is stored as its `LabSpec` (+ id, timestamps) in collection `labs`.
  **Running** means: a `Lab` in memory and a ticker task advancing it —
  every 50 ms of real time by `50 ms × speed` (speed 0 = paused, 0.25–16×).
  After loading, `settle()` so STP/OSPF/DHCP are converged before the user
  looks. Stopped labs cost nothing. At most 8 labs run at once (setting).
* All access to a `Lab` goes through one lock with short critical sections;
  nothing awaits while holding it.
* **Consoles**: this crate implements `connect::SimConnector` and registers
  it at init (`builder.get::<ConnectState>().set_sim_connector(..)`):
  `open(LabRef)` → `console_open` → `Connection::pair`; input is applied
  immediately and followed by a short `advance` + output drain so typing and
  automation are not quantised to the tick; `Input::Resize` →
  `console_resize`; closing either side closes the console. Opening a
  console on a lab that is not running starts it (and says so in the job/
  session title), unless the lab was deleted.
* **Inventory sync**: every node with a CLI is an inventory device named
  `<lab name>/<node>` with `lab: {lab_id, node}`, `transport: sim`, site
  `Lab: <lab name>`; network devices use profile `cisco_ios` and the role
  matching their kind; hosts use a `nocturne_sim_host` profile that this
  crate ships and registers (ask for a core hook in your report if
  `profiles` has no way to register an extra built-in — the lead will add
  `profiles::register`), role `host`, monitoring off. Created on lab
  create/import and node add; renamed with the node; removed (with their
  collected data) with the node or lab. No credential is needed unless the
  node's config sets one.
* **Events out of the simulator** (`take_events` each tick): syslog lines →
  the log pipeline, by method name: `logs.ingest {device_id, lines}` with the
  node's device id and the lines as the device formats them (`*Mar  1
  00:00:22.010: %LINK-3-UPDOWN: Interface …`), one call per device per tick,
  in the order they were logged (a tick that brought a device more than 5,000
  lines hands them over in several calls, in order: the pipeline refuses a
  call of more than 10,000 whole). From there they take the path a datagram
  from hardware takes — parsing, rules, signals, the live tail
  (`docs/dev/features/logs.md`). This crate does not link the logs feature:
  when the method is unknown (a build without it) the lines are written
  straight to the log store (`store.log_append`, stream `syslog`, body
  `{line, lab_id, sim_time}`), so a lab's messages are still there to read.
  Only then: when the pipeline is there and refuses, nothing is written
  around it (and the rest of that device's lines of the tick are not tried).
  The one line that is written directly in either build is a line from a
  node that has no inventory device at that moment (one that is just being
  added or removed): the pipeline takes lines for a device only, so it is
  kept as before, unattributed. Link state and `NodeChanged` → debounced
  `labs.changed` and a spec autosave (a `write memory` on a console must
  survive a restart; unsaved running-config is lost on stop, as on real
  gear — say so when stopping a lab with unsaved nodes); packet events →
  capture buffer (ring, 20k events) + `labs.packets` batches while capture
  is on.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `labs.list` | → `[{id, name, description, nodes, links, running, updated_at}]` |
| `labs.create` | `{name, example?: id}` → `LabView` (examples from `nocturne_netsim::examples()`; `demo` = `demo_campus()`) |
| `labs.get` | `{id}` → `LabView {id, name, description, running, speed, sim_time, snapshot: LabSnapshot, devices: {node_name: device_id}, unsaved_nodes: [..], capture: bool}` |
| `labs.update` | `{id, name?, description?, notes?}` |
| `labs.delete` / `labs.duplicate {id, name}` | |
| `labs.import` | `{name?, spec: LabSpec | text (JSON or YAML)}` → `LabView`; validation errors name the node/link |
| `labs.export` | `{id, format: json|yaml}` → `{text}` (a `LabSpec`, shareable) |
| `labs.start` / `labs.stop` / `labs.speed {id, speed}` / `labs.reset {id}` (reboot every node from its startup-config) | |
| `labs.catalog` | → `[ModelInfo]` |
| `labs.examples` | → `[{id, name, description, nodes, teaches}]` |
| `labs.node.add` | `{lab_id, model, name?, x, y}` → `NodeView` |
| `labs.node.update` | `{lab_id, node, name?, x?, y?, powered?, host?: HostConfig}` |
| `labs.node.move` | `{lab_id, positions: {node: {x, y}}}` (drag; debounced autosave, no event storm) |
| `labs.node.remove` | `{lab_id, node}` |
| `labs.node.config` | `{lab_id, node, kind: running|startup}` → `{text}`; `labs.node.config_set {.., startup: text}` (replaces the startup-config and reboots the node) |
| `labs.node.tables` | `{lab_id, node}` → `NodeTables`; `labs.node.metrics` → `NodeMetrics` |
| `labs.link.add` | `{lab_id, a: {node, interface?}, b: {node, interface?}, cable?}` → `LinkView` — a missing interface picks the first free one that suits the cable; refusals explain (no free port, same node, interface busy) |
| `labs.link.remove` | `{lab_id, link}` |
| `labs.links` | `{lab_id?}` → links of running/stored labs as `{a: {device_id, interface}, b: {..}, up}` (the topology feature's source of truth for lab nodes) |
| `labs.probe` | `{lab_id, from, to, kind: ping|tcp, port?}` → `ProbeResult` (instant; `to` = node name or IP) |
| `labs.connectivity` | `{lab_id, nodes?: [..]}` → `{nodes: [..], matrix: [[{ok, reason}]], reachable_pct}` — every host/SVI to every other, the before/after check for a rehearsal |
| `labs.capture` | `{lab_id, on}`; `labs.trace {lab_id, from, to, pdu: icmp|tcp|udp, port?}` → `{trace_id}`; `labs.packets {lab_id, after_seq?, limit?}` → `{events: [PacketEvent], next_seq}` |
| `labs.flow.add` | `{lab_id, flow: FlowSpec}` → `{flow_id}` — background traffic (moves counters, lights up utilisation). `FlowSpec` = `{name, src, dst_ip, bps, pattern?: "constant" \| {sine: {period_s}} \| {burst: {on_s, off_s}}, enabled?: true, dscp?: 0-63}`. `dscp` is the marking the traffic carries (46 = EF), which a simulated device's QoS policy classifies on (`docs/dev/spec-qos.md`); leave it out for traffic that states none. It is absent from every result and from the lab document when unset — not `null`, not 0 — so documents written before it existed load and export unchanged. A value above 63 is refused, here and in `labs.import`, naming the flow |
| `labs.flow.remove` | `{lab_id, flow_id}` |
| `labs.flow.list` | `{lab_id}` → `[{id, flow: FlowSpec, state: {active, current_bps, delivered_octets, path: [node], reason}}]` — `current_bps` is what the flow offers; `delivered_octets` is what arrived, after whatever a policy or a full interface on the path dropped |
| `labs.fault.inject` / `.clear` / `.list` | `Fault` as the simulator defines it (link down, packet loss, node power) |
| `labs.expose` | `{lab_id, node, protocol: ssh|telnet, port?: 0, bind?: "127.0.0.1"}` → `{address}` — a real TCP port bridged to the node's console so PuTTY, Ansible or a monitoring tool can connect; `labs.exposed`, `labs.unexpose`. Non-loopback bind needs local access and is audited. |
| `design.generate` | `Requirements` → `Design` (see `spec-design.md`; pure, nothing is created) |
| `design.create_lab` | `{requirements | design, name?}` → `LabView` — creates the lab, starts it, runs `labs.connectivity`, and returns the validation result with it |
| `design.export` | `{requirements, format: markdown|configs}` → the whole design as one document (what to change before deploying, sites, addressing, links, bill of materials, tests, every configuration, and why it is built this way), or `{configs: {device: config}, without_a_configuration: [...]}`. Takes the **requirements**, not a `Design`: generating is pure, so the design is reproduced rather than carried back and forth — a `Design` holds every configuration line of every device, which is exactly why `design.generate` does not return its lab. The `configs` form names the nodes that have no command line (hosts) instead of leaving them out. |
| `labs.twin` | `{device_ids, name?}` → `{job_id}` — build a lab from real devices: nodes from collected facts/interfaces, links from collected neighbours, each node's startup-config from its latest backup **as far as the simulator accepts it**; the result reports, per node, lines accepted / rejected (with the rejected lines), so the user knows exactly how faithful the twin is. Never claims fidelity it does not have; refuses (with the reason) devices with no collected data or backup. |

Events (contract): `labs.changed {lab_id, reason: created|updated|deleted|
started|stopped|topology|node|config}`, `labs.tick {lab_id, sim_time,
links: [{id, up, a_light, b_light, bps_ab, bps_ba}]}` (≤ 4 per second per
running lab, only while something changed), `labs.packets {lab_id, events,
next_seq}` (capture on; batched ≤ 10 per second), `labs.trace_done
{lab_id, trace_id, ok, summary}`.

## Tests

Lifecycle (create from each example, start, settle, stop, restart, delete)
with inventory sync verified at each step; persistence (edit, `write
memory` on a console, stop, start: config survives; unsaved does not, and
`unsaved_nodes` said so); console through `connect::cli` with the
`cisco_ios` profile if it exists in the build (else a minimal test profile)
— login, `show version`, a config change, the automation gate (one session
at a time); `show policy-map interface` through that driver on a device with
a policy and on one without (an answer of nothing, not an error); two
interactive consoles on one node; typing latency (input →
echo without waiting for a tick); `labs.probe`/`connectivity` on the demo
campus (expected reachability from the example's own description, then
inject a fault and see it change); capture + trace events in order with
monotonic `seq`; syslog lines handed to `logs.ingest` (a stand-in method
registered by the test, since features do not link) with the right device
and not also written directly, a refusal from it not worked around, a flood
of more lines than one call takes arriving whole and in order, and
without the method the lines landing in the log store with the right
device; a flow's `dscp` through `labs.flow.add`/`.list`, export and import
(JSON and YAML), a document without the field, and a value out of range;
expose over real SSH and Telnet on port 0 and drive a session with
the transport client; link/add/remove validation messages; import/export
round trip; ticker behaviour with speed 0 and 16×; 8-lab cap; a lab deleted
while a console is open closes it cleanly; twin from hand-built collected
data + a small config, with the accepted/rejected accounting checked.
