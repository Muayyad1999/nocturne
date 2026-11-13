# Feature: monitoring (`nocturne-monitor`)

Is everything up, how loaded is it, and what needs a human right now.
Polls devices (ICMP, SNMP, and the CLI where there is nothing else), keeps
time series, evaluates alert rules, and notifies.

Engines: `nocturne-netprobe` (`ping::Pinger`, `tcp::check`), `nocturne-snmp`
(`Client`, `Auth`, `oids`), core `collect` (CLI/sim fallback), `store`
time-series API (`series_id`, `samples_insert`, `samples_query`,
`sample_latest`, `samples_maintain`).

## Polling

One scheduler task; each device with `monitor.enabled` is polled every
`monitor.interval_s` (jittered, spread across the interval; global
concurrency cap from settings, default 32 probes in flight; a slow device
never delays others).

* **Reachability** (`monitor.icmp`): 3 echoes (`Pinger`), loss and RTT.
  Where ICMP is not possible on this machine or the device does not answer
  ping but its management port does, fall back to a TCP connect to the
  management port and say which was used (`via`). Lab devices: reachability
  is "the node is powered and has a console" — ask the simulator through
  `connect::open` cheaply or `collect`; never ping a lab address on the real
  network.
* **Up/down with damping**: `down` after `down_after` consecutive failed
  polls (default 2), `up` on the first success. Transitions update
  `DeviceState` (`inventory::update_state`: `reachable`, `rtt_ms`,
  `last_seen`, `last_checked`, `last_error`) — which publishes
  `devices.state` — and are recorded in the `event` log stream.
* **SNMP** (`monitor.snmp`, credential from `monitor.snmp_credential_id` or
  the device credential's SNMP half; one long-lived `Client` per device):
  `sysUpTime`; IF-MIB `ifName/ifDescr/ifAlias/ifOperStatus/ifAdminStatus/
  ifHighSpeed` + HC octet counters (32-bit fallback on v1 or when the HC
  columns are absent), errors and discards; health from the profile's
  `snmp:` OIDs (CPU, memory, temperature). Rates are computed from counter
  deltas over `sysUpTime` deltas, with counter wrap and device reboot
  (uptime went backwards → discard the interval, never a spike) handled.
  Also feeds `collect::store(.., Kind::Interfaces, .., "snmp")` so topology
  and IPAM see interface state on SNMP-only devices.
* **CLI fallback** (devices with CLI access but no SNMP, and lab devices):
  `collect::run` for `interfaces` + `health` at a slower cadence
  (`cli_interval_s`, default 300; lab devices 30) and derive the same
  metrics from counters.
* A failed SNMP/CLI poll is a recorded gap, not zeros.

Metrics (series = device + metric + instance):
`icmp.rtt_ms`, `icmp.loss_pct`, `up` (1/0), `uptime_s`, `cpu.pct`,
`mem.pct`, `temp.c` (instance = sensor), and per interface (instance =
canonical interface name) `if.in_bps`, `if.out_bps`, `if.util_pct`,
`if.in_err_ps`, `if.out_err_ps`, `if.in_disc_ps`, `if.out_disc_ps`,
`if.oper` (1/0).

Settings section `monitoring`: `{enabled: true, concurrency: 32,
down_after: 2, cli_interval_s: 300, retention_days_raw: 7,
retention_days_rollup: 365, ping_timeout_ms: 1000}`. A maintenance task
calls `samples_maintain` hourly.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `monitor.status` | → `{enabled, devices_monitored, polls_per_min, last_cycle_ms, ping_method, problems: [..]}` |
| `monitor.poll_now` | `{device_ids}` → `{job_id}` |
| `monitor.device` | `{device_id}` → `{state: DeviceState, health: {cpu_pct?, mem_pct?, temp_c?, uptime_s?, at?}, interfaces: [IfStatus], sources: {icmp, snmp, cli}}` |
| `monitor.interfaces` | `{device_id?, filter?, problem_only?, sort?, limit?, cursor?}` → `[IfStatus{device_id, device, name, description, admin_up, oper_up, speed_bps, in_bps, out_bps, util_pct, in_err_ps, out_err_ps, at}]` |
| `metrics.query` | `{device_id, metric, instance?, since, until?, step_s?}` → `{points: [[t, v]..], step_s, rollup: raw|5m|1h}` (gaps are gaps) |
| `metrics.query_many` | `{series: [{device_id, metric, instance?}], since, until?, step_s?}` |
| `metrics.series` | `{device_id}` → `[{metric, instance, latest: [t, v]?}]` |
| `metrics.top` | `{metric, limit?: 10, filter?}` → highest current values across devices/interfaces ("top talkers", "hottest", "busiest CPU") |
| `alerts.list` | `{state?: firing|acknowledged|resolved|open, severity?, device_id?, since?, limit?, cursor?}` → `[Alert]` |
| `alerts.get` / `alerts.ack {id, note?}` / `alerts.resolve {id, note?}` / `alerts.snooze {id, minutes}` | |
| `alerts.raise` | `{rule?: String, device_id?, instance?, severity, title, detail?, dedup_key}` → `Alert` — for other features (log rules, insights, config) |
| `alerts.rules.list` / `.save` / `.delete` / `.test` | |
| `alerts.summary` | → `{open, by_severity, newest}` |
| `maintenance.list` / `.save` / `.delete` | windows that mute alerts: `{id, name, scope, starts_at, ends_at, recurring?: weekly}` |
| `notify.channels.list` / `.save` / `.delete` / `.test` | |

```rust
struct Alert { id, rule_id?, rule_name, device_id?, device?, instance?, severity: info|low|medium|high|critical,
    state: firing|acknowledged|resolved, title, detail, value?, started_at, last_seen_at, resolved_at?,
    acked_at?, ack_note?, snoozed_until?, count, muted_by_maintenance: bool, lab: bool }
struct AlertRule { id, name, enabled, builtin, severity, scope: {sites, roles, tags, device_ids, include_lab: false},
    condition: Condition, for_s, notify: [channel_id] }
enum Condition {                                   // #[serde(tag = "type", rename_all = "snake_case")]
    DeviceDown,
    InterfaceDown { only: uplinks|described|all },  // admin up + oper down; `uplinks` = has a collected LLDP/CDP neighbour
    Threshold { metric, instance?: glob, op: gt|lt, value, clear_value? },   // hysteresis via clear_value
    NoData { metric, for_s },
    ConfigChanged,                                  // on the event `config.changed`
}
```

Built-in rules (editable copies, can be disabled): device down (critical,
for 0 s after damping), uplink down (high), CPU > 90 % for 5 min, memory
> 90 % for 10 min, temperature > 70 °C, interface errors > 10/s for 5 min,
interface utilisation > 90 % for 10 min, packet loss > 20 % for 3 min,
RTT > 5× the device's 24 h median for 5 min, configuration changed (info).

Evaluation: after each poll for the polled device. An alert is keyed by
(rule, device, instance): it fires once, counts repeats, resolves itself when
the condition clears (with `clear_value` hysteresis), and re-fires as a new
alert after a resolve. When a device is down, its interface/threshold alerts
are not raised (one cause, one alert). Maintenance windows and snoozes mute
notifications but still record. Lab devices alert only when a rule opts in.

Notifications: channel kinds `webhook` (generic JSON POST), `slack`,
`teams`, `discord` (their incoming-webhook formats), `email` (SMTP with
STARTTLS — only if it can be done with rustls + ring and no C dependency;
otherwise leave it out and say so), plus the event `alerts.changed` that the
desktop shell turns into a system notification. URLs and SMTP passwords live
in the vault (`monitor:channel:<id>:secret`), never in documents. Firing
and resolved are both sent; a notification failure is retried with back-off
(3×) and recorded, never blocks polling. `notify.channels.test` sends a
clearly marked test message.

Events (contract): `devices.state` (from the core), `monitor.polled
{device_id, at, ok}`, `monitor.interfaces {device_id}` (interface table
changed), `alerts.changed {alert}`, `monitor.status_changed`.

## Tests

With an injected clock and fake probes (make the ping and SNMP layers
traits so tests drive them): damping and transitions; jitter and the
concurrency cap; counter-delta maths incl. wrap (32 and 64 bit) and reboot;
gap handling; each condition incl. hysteresis, `for_s`, `NoData`, device-down
suppression of child alerts, maintenance, snooze, re-fire after resolve;
`alerts.raise` dedup; notification payloads per channel kind against a local
HTTP listener, retries, secrets not in documents/events. Live: SNMP polling
against the in-crate `nocturne_snmp::Agent` (and a private `snmpd` if
installed), ICMP to 127.0.0.1 where permitted, CLI fallback against a fake
device with canned `show interfaces` output; a 2,000-device simulated cycle
stays within the concurrency cap and finishes.
