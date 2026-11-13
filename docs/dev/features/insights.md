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
| `insights.settings` | via settings section `insights`: `{enabled_checks: {id: bool}, thresholds: {..}, include_lab: true, raise_alerts_from: "high"}` |
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

## Tests

Table-driven per check with hand-built collected rows and configs; the
"not enough data" path of each; stable ids across scans; resolve after two
clean scans; dismiss with expiry; debounced auto-scan on events; coverage
report; `insights.fix` → `changes.create` payload; alert raising threshold;
a scan over 500 devices × 48 interfaces finishes in well under a second
(these run after every collection).
