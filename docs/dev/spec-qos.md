# Quality of service

What Nocturne knows and can do about QoS: read per-class statistics from a
device, chart and alert on them, notice a policy that is wrong, and change a
policy through the same pipeline as any other configuration. It crosses
several crates, so the contract is here and each feature's own specification
points at it.

**What this is not.** It is not "every QoS feature". There is no NBAR or
application recognition, no WRED curve or buffer tuning view, no PFC/ETS, no
wireless WMM, no QoS over SNMP yet (below), and no QoS read on a platform
without a profile. `docs/STATUS.md` states the coverage per platform and
which reads rest on real captures.

## The read: a `qos` kind (`nocturne-drivers`)

`Kind::Qos` is a table like `interfaces`. One row per class per direction per
interface:

```rust
struct QosClass {
    interface: String,              // canonical interface name, as everywhere else
    direction: Direction,           // in | out
    policy: String,                 // IOS: the policy-map. RouterOS: the menu the queue is in, "queue simple" or "queue tree"
    class: String,                  // class name (RouterOS: the queue); "class-default" is a class like any other
    parent: Option<String>,         // the parent class or queue, for hierarchical policies
    rate_bps: Option<u64>,          // configured: shape rate, policer CIR, priority rate, bandwidth guarantee or max-limit
    packets: Option<u64>,           // classified into this class
    bytes: Option<u64>,
    dropped_packets: Option<u64>,   // every drop, whatever the reason
    dropped_bytes: Option<u64>,
    queue_dropped_packets: Option<u64>,   // tail/WRED drops: congestion
    police_dropped_packets: Option<u64>,  // policer drops: the policy doing its job
    queued_packets: Option<u64>,    // queue depth at the moment of the read
    offered_rate_bps: Option<u64>,  // the DEVICE's own averages, kept apart from
    drop_rate_bps: Option<u64>,     // anything Nocturne computes from counters
}
```

Every number is optional and uses the lenient deserialisers: a field the
platform does not print is absent, never zero. The row is flat because a
profile's `map` allows one level of nesting only. In JSON an absent number is
`null`, like every other optional field of the model; `direction` is `"in"`
or `"out"`. A row whose direction cannot be told is dropped and counted as a
`partial` problem, never filed under a default. `interface` is empty where
the platform attaches a policy to something that is not an interface.

*A policy is a tree.* A class that holds a child policy is the `parent` of
that policy's classes, and its counters include theirs: consumers add up one
level, never the whole table.

*What the two platforms fill.*

| Field | `cisco_ios` | `mikrotik_routeros` |
|---|---|---|
| `interface` | the interface the policy is attached to | a simple queue's `target` when that is one interface; empty for addresses and for every tree queue (the listing names a queue's immediate parent only, not the interface at the root) |
| `direction` | the policy's (`input` → `in`) | simple queue: two rows, upload → `in`, download → `out` (seen from the router, as the target sends and receives); tree queue: `out` |
| `policy`, `class` | policy-map, class | `queue simple` / `queue tree`, the queue's name |
| `parent` | the class the nested policy sits in | the queue's `parent` as printed — another queue, an interface or `global` — when printed and not `none` |
| `rate_bps` | the first of: shaper rate, policer CIR, priority rate, bandwidth guarantee | never: the statistics listing does not print `max-limit` |
| `queue_dropped_packets` | the queue's `total drops`, plus a priority class's `b/w exceed drops` (it is held to its rate only while the interface is congested) | never: RouterOS counts one kind of drop |
| `police_dropped_packets` | the packets of each kind (conformed, exceeded, violated) whose action is `drop`; absent without a policer, and where the policer counts bytes only (Catalyst 6500) | never |
| `dropped_packets` | both of the above together, when every counter the class prints is known | `dropped` |
| `dropped_bytes` | only where the one thing in the class that drops counts bytes (a policer; before HQF the priority queue) | never |
| `queued_packets` | `queue depth` | `queued-packets` |
| `offered_rate_bps`, `drop_rate_bps` | the class's own `offered rate` / `drop rate` | never: RouterOS's `rate` is what passes the queue, not what is offered to it |

`cisco_ios` does not read the queue counters printed under `queue stats for
all priority classes` (they belong to no class), nor a policy attached below
an interface (a Frame Relay DLCI, an ATM VC, a service instance): that
output fails the read with an error that says so, as does a class whose place
in the tree cannot be told from its indentation. So does an answer that is
not a listing at all (text before the first interface other than the lines
of `exec prompt timestamp` or a log message), a policy without a name and a
class before its interface's policy is named: the read never files a class
under a name left over from another interface, and never reads text it does
not understand as "no policy attached". A policer's drops are read only
where the answer says what it does with both the conforming and the
exceeding packets. `mikrotik_routeros` fails the read of a queue listing
that is printed as a table instead of `key=value`.

*Unknown and empty stay different.* A device with no policy attached answers
with zero rows and that is stored as collected-and-empty. A failed read
stores nothing and is recorded as a failure.

A profile reads it with ordinary `collect.qos` steps — YAML, no Rust:

| Profile | Command | Parser |
|---|---|---|
| `cisco_ios` | `show policy-map interface` | inline TextFSM in the profile (`ntc: auto` resolves to the *configured policy* template, which swallows statistics silently — do not use it) |
| `mikrotik_routeros` | `/queue simple print stats detail without-paging`, `/queue tree print stats detail without-paging` | `kv` |
| `juniper_junos`, `cisco_nxos`, `arista_eos`, `cisco_xr` | not in this round | — |

### A read that has not met a real device

No recording of per-class statistics from a real device exists in this
repository or in the upstream template corpus, and it cannot be produced
from inside a build environment. The two profiles above are `verified`, and
that word must stay true. So a profile may say, per kind, that a read is not
yet checked:

```yaml
unverified: [qos]      # kinds whose parser has not met output recorded from a real device
```

* A capture says what its files are, in `expect.yml`: `origin: recorded`
  (the default), `documented` — a sample printed in the vendor's own
  documentation, copied as it stands, with the page's address in `SOURCE.md` —
  or `synthetic` (written by hand, or printed by the simulator). Only
  `recorded` is real device output. The parsers here are tested against the
  samples in Cisco's command reference and MikroTik's documentation, against
  the simulator's output and against hand-written output, each labelled as
  what it is.
* The profile test harness accepts documented and synthetic captures for a
  kind listed there, requires at least one the read succeeds on, and **fails
  if a recorded capture exists for a kind still listed** — the flag comes off
  the day a real recording arrives, not later. A documented sample does not
  lift it.
* `collect.get` carries `unverified: true` on such a document and
  `collect.status` on its entry (the key is absent otherwise; it is said of
  the read that produced the rows, when it was made, so rows handed to the
  store by another feeder never carry it), `profiles.list` lists the kinds
  per profile (`unverified: ["qos"]`), `monitor.qos` carries it per device,
  and every screen that shows the data says "this read has not been checked
  against a real <platform> device" above it. In Rust:
  `Profile::is_unverified(kind)`, and `nocturne_app::collect::unverified(app,
  device_id, kind)` for a device that has nothing stored yet.
* `maturity` is untouched: lowering it would take write access away from the
  two platforms that have earned it.

Recording the three commands from a real IOS/IOS-XE and a real RouterOS
device is read-only and already possible (`devices.exec`, or the terminal);
`crates/nocturne-drivers/tests/profiles/<id>/captures/SOURCE.md` says how to
anonymise and cite one.

## Monitoring (`nocturne-monitor`)

QoS is read over the CLI on its own cadence — `qos_interval_s` (default 300,
30 for lab devices) — **also for devices whose interfaces are polled over
SNMP**, because SNMP carries no QoS yet. Only devices whose profile defines
the kind are asked. A failed read is a gap in the series, not zeros.

Per class, from counter deltas over the device-uptime interval (the same
rule as interface rates: a reboot or a counter that went backwards throws
the interval away):

| Series | Instance | Meaning |
|---|---|---|
| `qos.bps` | `<interface>\|<in\|out>\|<policy>/<class>` | traffic in the class |
| `qos.drop_ps` | same | all drops per second |
| `qos.queue_drop_ps` | same | congestion drops per second — only where the row separates them |
| `qos.drop_pct` | same | drops as a share of packets offered |
| `qos.queue_len` | same | queue depth, a gauge |

`qos.drop_pct` is drops over the packets classified into the class in the
interval; it is not written for an interval in which nothing was offered,
nor where more was dropped than offered.

The instance reads back to exactly its four names. It is split at the
first two pipes and the last slash, so a `|` in an interface name is
written `%7C`, a `/` in a class name `%2F` and a `%` in either `%25`; the
policy is spelled as the device spells it, slashes included. Where one
policy sits under two classes of a parent on the same interface, the
classes are told apart by that parent, written before the policy
(`Gi0/1|out|VLAN10/PER-VLAN/VOICE`). A row carries its own instance
(`QosStatus.instance`), so nothing has to rebuild the spelling. The rule,
and what happens to classes nothing tells apart, are in
`docs/dev/features/monitor.md` ("QoS").

A device with a queue per subscriber can have thousands of classes. Series
are written for at most `qos_max_series` classes per device (default 256,
taken in a stable order: interface, direction, policy, class); the current
table always holds every class, and `monitor.qos` says when series were
capped. Nothing is dropped silently.

| Method | Params → Result |
|---|---|
| `monitor.qos` | `{device_id?, interface?, problem_only?, limit?: 200, cursor?}` → `{rows: [QosStatus], next_cursor?, coverage: {devices: [{device_id, state: ok\|unsupported\|failed\|never, at?, unverified, series_capped, error?, classes, series}]}}` |

```rust
struct QosStatus { device_id, interface, direction, policy, class, parent?, rate_bps?,
    instance?,                                                // of the class's series; None = it has none
    bps?, drop_ps?, queue_drop_ps?, drop_pct?,                // None = the interval could not be measured
    queue_len?, packets?, dropped_packets?,                   // as last read
    dropping: bool,                                           // what `problem_only` selects
    at, unverified: bool }
```

Optional fields are `null` in JSON, `next_cursor` included (after the last
page). `classes` and `series` are how many classes the device has and how
many of them have series. `error` is why a read `failed`; with `ok` it is
what a read that worked could not place and left out. `dropping` is
congestion as far as the device can tell: `queue_drop_ps` above zero, or
`drop_ps` above zero where the row has nothing to tell congestion from
policing; a class whose drops are all a policer's is not. A failed read
keeps the rows and takes their rates away.

Event: `monitor.qos.updated {device_id}`.

One built-in, editable rule: **"QoS class is dropping (congestion)"** — a
threshold on `qos.queue_drop_ps` above 10/s for 300 s (clearing below 1/s),
severity `medium`.
There is deliberately no built-in rule on policer drops: a policer that
drops is the policy working.

## Insights (`nocturne-insights`)

| Check | Category | From | Says |
|---|---|---|---|
| `qos_policy_missing` | config | latest stored running-config | an interface or a class references a policy-map the same configuration does not define |
| `qos_policy_unattached` | hygiene | latest stored running-config | a policy-map nothing references (a child policy referenced from a class counts as used) |
| `qos_class_dropping` | capacity | the `qos.*` series | a class whose congestion drops stayed above the threshold for most of the window, with interface, policy, class, drop rate, share of offered traffic and peak hour |

The two configuration checks understand IOS syntax only and say so in
coverage; a device whose stored configuration is partial (an interface-only
capture) is not judged. `qos_class_dropping` is "not evaluated" with the
reason when a device has no QoS samples, and where a platform cannot tell
congestion drops from policer drops the finding says that instead of
guessing.

## The simulator (`nocturne-netsim`)

So that a lab can show all of the above honestly:

* `class-map [match-any|match-all] NAME` with `match dscp …` and `match
  access-group …`; `policy-map NAME` with `class NAME` and `priority
  [kbps]`, `bandwidth {kbps|percent N}`, `police <bps>`, `shape average
  <bps>`, `set dscp …`, `service-policy CHILD`; `service-policy {input|output}
  NAME` on an interface; all rendered in `show running-config`.
* A flow gains an optional `dscp` (default: none, which matches no `match
  dscp` — not even `default` — and so falls to `class-default` unless a
  `match access-group` catches it by address), so traffic can be classified.
* The drop model is analytic and deterministic: per tick, a class is offered
  the bytes of the flows that match it; a policer or a shaper drops what
  exceeds its rate integrated over the tick; a `priority` class is served
  first and `bandwidth` classes share what is left on the interface in
  proportion; what an interface cannot carry is dropped from the classes
  over their share, `class-default` first. No clock is read and no generator
  runs: every counter is a pure function of the world's seed, the flows, the
  policy and simulated time. An interface without a policy carries no more
  than its speed either (one queue, tail drop). Queue drops, with or without
  a policy, also count as the interface's output drops; policer drops do not.
  Policies attach to physical interfaces only, because that is where flow
  traffic is accounted. The details, and every line of the output with the
  example in Cisco's reference it comes from, are in `docs/dev/spec-netsim.md`.
* `show policy-map`, `show policy-map interface [NAME]` in IOS format.
  `show policy-map interface` on a device with no policy answers with
  nothing, as hardware does.

The format follows Cisco's published command reference; a capture of the
simulator's own output is synthetic and is marked so.

## Changing QoS (`nocturne-changes`)

There is no QoS write path of its own. A QoS change is configuration lines
through `changes.create`, and goes through plan → risk and protected
resources → approval of that exact plan → backup → apply → verify →
rollback like everything else.

* **Plan.** The `Lines` planner knows QoS vocabulary: a class name is a name
  (`class VOICE`, `class-map match-any VOICE` plan instead of being refused
  as a keyword in the wrong case), and `cos`, `dscp`, `police`, `percent`,
  `remaining`, `average`, `random-detect`, `fair-queue`, `queue-limit`,
  `conform-action`, `exceed-action`, `violate-action`, `precedence`,
  `class-default`, `mls`, `priority-queue`, `rate-limit` are words, not
  abbreviations of something else.
* **Risk and protection.** Two protected kinds: `qos_on_management` — a
  `service-policy`, `police`, `shape` or `rate-limit` landing on the
  interface Nocturne reaches the device through, or a RouterOS `/queue` line
  whose target covers the management address (critical, typed confirmation);
  `qos_on_uplink` (high). Neither is ever on the assistant's low-risk
  allow-list.
* **Verify.** When a plan attaches or changes a policy and the platform
  collects `qos`, the kind is read after the apply and the policy must be
  attached where the plan put it. Where the kind is unsupported the check is
  reported as not run, never as passed.
* **Rollback.** A policy is detached before it is removed. A changed
  policy-map is an ordered section that is rebuilt; that path is tested with
  a policy-map on the fake device, not only with access lists.

Built-in configuration templates (data, in the template library): a
voice/video/critical/best-effort policy for a WAN uplink (IOS MQC), a shaped
uplink (IOS), trusting DSCP on an access port (Catalyst), and a simple queue
per subscriber and a queue tree (RouterOS). A template renders lines; the
lines become a change.

## Flows, and what traffic is actually marked with

The flow collector (`docs/dev/features/logs.md`) reads the ToS byte, so
`flows.top {by: "dscp"}` answers a question no counter can: which markings
are really on the wire.

## Tools (`nocturne-tools`, `nocturne-toolbox`)

* `tools.qos.marking {value, as: dscp|tos|precedence|name}` → the same
  marking in every form: DSCP number and name (`EF`, `AF41`, `CS3`), ToS
  byte, IP precedence, ECN bits, the per-hop behaviour and its usual use.
* `tools.qos.voice {codec, calls, link: ethernet|ppp|mpls|…, crtp?, vad?}` →
  bandwidth per call and in total, with the header arithmetic shown.
  Also optional: `packet_ms?` (voice per packet) and `l2_bytes?` (with
  `link: "custom"`).

Both methods' full parameter and result shapes, and where every codec and
link figure comes from, are in `docs/dev/features/toolbox.md` ("QoS
calculators") and `docs/dev/spec-tools.md` ("Data").

## The interface

No new top-level screen. Monitoring → Interfaces → an interface's drawer
gains a QoS section (one row per class: policy, class, configured rate,
traffic, drops/s, drop %, queue depth; a drops-per-class chart), and the
interface table gains a "Dropping" filter. Devices → a device → Data gains a
QoS table. Three states read differently everywhere: no policy attached; not
collected for this device type; collected, but the interval could not be
measured.

## The assistant

`qos_status` (reads `monitor.qos`) and the `qos` kind in `get_collected`.
QoS changes are proposed with `propose_change` like any other change.

## Later

QoS over SNMP as data: a table mapping in the profile's `snmp:` section, so
RouterOS queue tables and Cisco class-based QoS can be polled at the SNMP
interval. Not before the CLI path is proven.
