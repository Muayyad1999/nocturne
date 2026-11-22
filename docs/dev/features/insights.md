# Feature: insights (`nocturne-insights`)

Issue identification: Nocturne reads what it has already collected —
interface state and counters, neighbours, VLANs, routes, configurations,
logs, metrics — and points at the things an experienced engineer would spot
walking the network: a duplex mismatch, a port that flaps, a trunk whose two
ends disagree, a switch that was never saved, two devices with one IP.

Every insight is a finding with evidence, not a vibe. If the data to decide
is missing, the check does not run for that device and says so.

Engines: core `collect` (`latest`, `latest_all`, `typed`), `store`
(samples, logs, config versions), `inventory`; `nocturne-drivers`
(`hier::ConfigTree`, `model::*`, `ifname`); `nocturne-tools` (subnet maths).
Reads other features through RPC when present (`topology.get` for links,
`config.latest`, `alerts.raise`). No device access.

## Model and RPC (contract)

```rust
struct Insight { id,                               // stable: hash of (check, device, subject) — the same problem keeps its id across scans
    check: String,                                 // "duplex_mismatch"
    category: physical|l2|l3|config|security|capacity|hygiene,
    severity: info|low|medium|high|critical,
    device_id, device, subject,                    // "Gi0/3", "VLAN 20", "10.0.0.5"
    title,                                         // one sentence a person can act on
    detail,                                        // why it matters, in two or three sentences
    evidence: Vec<{label, value}>,                 // the actual lines / counters / timestamps it was decided from
    suggestion: Option<{text, config_lines?: Vec<String>, commands?: Vec<String>}>,   // optional fix as config lines (for changes.create) or read-only commands to confirm
    related: Vec<{device_id, subject}>,            // the other end of the link, the other holder of the IP
    state: open|dismissed|resolved,
    first_seen, last_seen, resolved_at?, dismissed_until?, lab: bool }
```

| Method | Params → Result |
|---|---|
| `insights.scan` | `{device_ids?, checks?}` → `{job_id}` (also runs automatically, debounced, after `collect.updated` and `config.changed` for the affected device) |
| `insights.list` | `{state?: open, severity?, category?, check?, device_id?, query?, limit?, cursor?}` → `{insights, counts: {by_severity, by_category}}` |
| `insights.get` | `{id}` |
| `insights.dismiss` | `{id, days?: null /* forever */, note?}` / `insights.reopen {id}` |
| `insights.checks` | → `[{id, title, category, default_severity, needs: [kinds], enabled, description}]` — the catalogue, with what data each needs |
| `insights.settings` | via settings section `insights`: `{enabled_checks: {id: bool}, thresholds: {.., qos_drop_ps: 10.0}, include_lab: true, raise_alerts_from: "high"}` |
| `insights.fix` | `{id}` → `{change_id}` — `changes.create` with the suggestion's config lines (origin `user`, titled from the insight); `invalid` when the insight has no config fix |
| `insights.coverage` | → per check: devices evaluated / skipped for missing data — so "no findings" is never mistaken for "checked" |

Insights that disappear from two consecutive scans of their device become
`resolved`. Open insights at or above `raise_alerts_from` call
`alerts.raise` (dedup key = insight id) when the monitor is present.

Event (contract): `insights.changed {device_id?, opened, resolved}`.

## Checks to ship

Each check is a small function over typed data with a table-driven test of
positive, negative and not-enough-data cases. Only ship a check whose logic
you can defend against a real network; say in `description` what it cannot
see.

Physical / interface
* `duplex_mismatch` — a link (topology) whose ends report different duplex,
  or one end half-duplex at ≥ 100 Mb/s; also late collisions > 0.
* `speed_mismatch`, `mtu_mismatch` — the two ends of a link disagree.
* `interface_errors` — input errors/CRC growing between the last two
  collections (not merely non-zero since boot); rate over the threshold.
* `interface_flapping` — ≥ N up/down transitions in an hour (from
  `if.oper` samples or `logs.signal` link events), default 4.
* `err_disabled` — ports in err-disabled state (with the cause when the
  platform prints it).
* `high_utilisation` — sustained > 80 % (samples), with the hours it peaks.
* `half_duplex_uplink`, `down_uplink` — an interface with a known neighbour
  (seen in an earlier collection) that is now down.

Layer 2
* `native_vlan_mismatch` — trunk ends with different native VLANs.
* `trunk_vlan_mismatch` — a VLAN allowed on one end of a trunk and not the
  other (list the VLANs).
* `vlan_without_ports` / `vlan_not_on_trunk` — a VLAN with access ports on
  a switch whose uplink trunks do not carry it (hosts black-holed).
* `unused_vlan` (info), `vlan_name_inconsistent` — the same VLAN id named
  differently across switches.
* `mac_flapping` — the same MAC learned on different ports between
  collections on the same switch (possible loop).
* `stp_root_unexpected` — root bridge for a VLAN is an access-role device.

Layer 3
* `duplicate_ip` — one IP on interfaces of two devices, or one IP with two
  MACs across ARP tables.
* `subnet_mask_mismatch` — interfaces in the same L2 segment (link) with
  overlapping but different prefixes.
* `no_default_route` — a routed device with no default route (info on core).
* `overlapping_subnets` on one device.

Configuration (from the latest stored running-config)
* `unsaved_config` — running differs from startup.
* `no_recent_backup` — newest backup older than N days (default 7) or none.
* `config_changed_outside_nocturne` — a `config.changed` event with no
  applied change covering that device and time (when the changes feature is
  present).
* `ntp_not_configured`, `no_syslog_host`, `telnet_enabled`,
  `http_server_enabled`, `snmp_default_community`, `weak_enable_password`
  (enable password / type 7 present), `no_vty_acl` — plain reads of the
  config tree for the platform's syntax; skip platforms whose syntax the
  check does not understand.
* `legacy_ssh_crypto` — the device only negotiated legacy SSH algorithms
  (from `DeviceState`/connection info if recorded).

Fleet
* `software_inconsistent` — devices of one model running different OS
  versions (lists the outliers against the majority).
* `uptime_reset` — uptime went backwards since the last collection
  (unexpected reboot), with the reboot reason when collected.
* `high_cpu`, `high_memory`, `high_temperature` sustained (samples).

Quality of service (contract: `docs/dev/spec-qos.md`, "Insights")
* `qos_policy_missing` (config, medium) — an interface or a class
  references a policy-map the same stored running-config does not define.
  One finding per policy, quoting every line that names it (the first
  eight, then a count). Judged only where the grammar is known:
  `service-policy input|output NAME` under an `interface` (at any depth, and
  inside `ip nhrp map group … service-policy output NAME`) and
  `service-policy NAME` in a class of an untyped `policy-map`. A reference
  from `control-plane`, a template or a wireless profile is not judged, a
  generated name (`AutoQos…`, `AutoConf-…`, `system-cpp-policy`,
  `port_child_policy`) is never missing, and neither is a name defined as
  `policy-map type … NAME`. A device normally refuses such a line, so on
  hardware a finding usually means the stored configuration is not the
  whole of what the device runs; the description says so.
* `qos_policy_unattached` (hygiene, info) — a `policy-map` that nothing
  references. Anything that follows `service-policy` on any line of the
  configuration counts as use: an attachment, a child policy named from a
  class (also the class of a policy that is itself unattached), the control
  plane, a template, a map-class, an NHRP group. Generated policies are not
  reported. It cannot see a policy RADIUS or a controller applies by name,
  so a device whose sessions RADIUS authorises — an `interface
  Virtual-Template…`, an IOS-XR `dynamic-template`, `ip subscriber` on an
  interface, `aaa authorization network` — is not evaluated.
* Names are compared exactly, as IOS compares them: `Wan-Edge` beside
  `WAN-EDGE` is a second policy. Where a missing or unattached policy
  differs only in case from one that is defined or named, the finding
  carries that as evidence ("Defined under a similar name" / "Named under a
  similar name"). A suggested `show policy-map NAME` is offered only for a
  name made of letters, digits and `-_.:/+@`: a name is device text, and one
  with a pipe or a quote in it is not pasted into a command.
* Both read **IOS-style MQC only** and say so in coverage. Not evaluated,
  each with its own reason: no stored configuration; a grammar other than
  `ios`; typed QoS policies (`policy-map type qos|queuing|network-qos|…`,
  `service-policy type qos …`, `service-policy input type qos …` — NX-OS,
  EOS, OS10); an IOS XR configuration, recognised by the lines only XR
  closes a section with (`end-policy-map`, `end-class-map`, `end-policy`,
  `end-set`); an interface `service-policy` line in a form not read
  (`service-policy "name" in`) or a map defined under any keyword but
  `policy-map NAME` (`policy-map-input NAME`); nesting deeper than 32
  levels; and a
  configuration that is **not whole**, which is one of two things. A
  *partial* one has no `hostname` line, which is what `show running-config
  interface` and `| section` captures are. A *cut-off* one does not close
  with `end`: IOS prints its policy-maps before its interfaces, so a
  listing that stops early has the definitions without the attachments.
  The same rule keeps NX-OS and ProCurve listings out, which print no `end`
  at all — an NX-OS device is therefore never evaluated, with or without
  QoS in its configuration. Which grammar a listing is in is read from the
  listing, never from the device's vendor or profile name. (An EOS
  configuration with no QoS statement in it, and an IOS XR one with neither
  a map nor a route policy nor a set, is evaluated and trivially clean:
  there is nothing in it to misread.)
* `qos_class_dropping` (capacity) — a class whose drops stayed at or above
  `thresholds.qos_drop_ps` (default 10/s, the figure of monitoring's
  built-in rule) for at least 75 % of the samples in the last
  4 × `sustained_minutes` (never less than an hour), with at least five
  samples spanning `sustained_minutes`. Two rules keep it to what was
  measured, both with one length — `sustained_minutes`, at least 15:
  *holes* — two samples further apart than that have a failed read between
  them, and only the run of samples since the last hole is judged, so the
  minutes a finding quotes were measured; *freshness* — a class whose
  newest sample is older than that is not reported, and a device whose
  newest QoS sample is older than that is not evaluated ("the read is
  failing"), neither clean nor dropping. Read from the core's series tables,
  not from `nocturne-monitor`; instance
  `<interface>|<in|out>|<policy>/<class>`, the policy split from the class
  at the last `/`. The finding's subject is that name with spaces for the
  bars (`GigabitEthernet0/0 out WAN-EDGE/VOICE`); the two configuration
  checks' subject is `policy-map NAME`.
  * Decided per device. A device with any `qos.queue_drop_ps` series
    separates congestion from policing: only that series is judged
    (**medium**), and a class with `qos.drop_ps` and no congestion series is
    a class without a queue — a policer doing its job — and is not reported.
  * A device with `qos.drop_ps` only has no congestion counter — its
    platform keeps one drop counter for both, or none of its classes has a
    queue: `qos.drop_ps` is judged, the finding is **low** and says the
    counters cannot tell congestion from a rate limit.
  * Evidence is the figures: drop rate (average, peak, threshold), the
    window (samples, minutes, share over the threshold), the share of
    offered packets dropped (`qos.drop_pct`) and the traffic in the class
    (`qos.bps`), both over the same run of samples as the drops — each
    "not recorded for this class" where the series is absent — and the
    peak hour of the day over `history_hours`. The suggestion carries
    `show policy-map interface <name> <input|output>` only for a device
    whose configuration grammar is `ios` and an interface name that is one.
  * Not evaluated: no `qos.*` drop series for the device; none inside the
    window; samples that stop before now (above); or a
    `thresholds.qos_drop_ps` that is not above zero, which would call every
    measured class a dropping one. Never "not dropping". (A device that
    separates the two and whose classes are measured now but have no
    congestion series is evaluated and clean: no class has a queue to
    overflow.)
  * Not seen: on a hierarchical policy the parent class counts its
    children's drops and the series name carries no parent, so a congested
    child can be reported together with its parent.

## Tests

Table-driven per check with hand-built collected rows and configs; the
"not enough data" path of each; stable ids across scans; resolve after two
clean scans; dismiss with expiry; debounced auto-scan on events; coverage
report; `insights.fix` → `changes.create` payload; alert raising threshold;
a scan over 500 devices × 48 interfaces finishes in well under a second
(these run after every collection).

QoS: the real interface-only capture
(`nocturne-drivers/tests/profiles/cisco_ios/config/asr-subinterfaces.txt`)
is the negative case of both configuration checks, and the same real
sections inside a hand-written whole configuration are the positive one —
and, stopped at four different points, the cut-off one. The repository has
no real configuration with policy-map definitions and no recorded `qos.*`
series; everything else in those tests is written by hand and labelled so,
including the NX-OS, IOS XR, OS10 and ProCurve listings that must not be
judged.
