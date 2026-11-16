# Feature: monitoring (`nocturne-monitor`)

Is everything up, how loaded is it, and what needs a human right now.
Polls devices (ICMP, SNMP, and the CLI where there is nothing else), keeps
time series, evaluates alert rules, and notifies.

Engines: `nocturne-netprobe` (`ping::Pinger`, `tcp::check`, and for service
checks `dns::lookup`, `http::check`, `tls::inspect`, `ntp::query`),
`nocturne-snmp`
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
retention_days_rollup: 365, ping_timeout_ms: 1000, checks_enabled: true,
check_history_days: 90, cert_warn_days: 14}`. A maintenance task calls
`samples_maintain` hourly and prunes service-check history.

## Service checks

One reachability verdict per device is not enough: "the web server on
10.0.0.5 is down but the box answers ping" is a different fact, and a
different alert. A **service check** is a named check attached to a device,
with its own parameters, interval, timeout, state, history and alert. A
device acquires none by itself — a check exists because somebody asked for
it.

Kinds and parameters (`CheckSpec`, tagged by `kind`):

| Kind | Engine | Parameters |
|---|---|---|
| `icmp` | `ping::ping` through the poller's `Reachability` | `count` (1–16, default 3), `loss_pct_down` (default 100) |
| `tcp` | `tcp::check` | `port` |
| `dns` | `dns::lookup`, asked **of the target** | `name`, `record` (default `A`), `expect`, `port` (53) |
| `http` | `http::check` | `path`, `port` (80), `method`, `expect_status` (default any 2xx/3xx), `expect_text`, `follow_redirects` (3) |
| `https` | `http::check` over TLS | the same, plus `verify_tls` (true), `warn_days`, `port` (443) |
| `tls` | `tls::inspect` | `port` (443), `sni`, `require_trusted` (true), `require_hostname` (true), `warn_days` |
| `ntp` | `ntp::query` | `port` (123), `max_offset_ms` |
| `snmp` | the device's own SNMP session | `oid` (default `sysUpTime`) |

A check's `host` is empty by default, meaning the device's own address; set
it for a service on a virtual address or under a name. Interval 5 s–24 h
(default 60); timeout defaults per kind and may not exceed the interval. No
request headers are stored: one of them would eventually carry a token.

**Five states** (`CheckStatus`), because three cannot tell the truth:

* `unknown` — it has never run. Nothing is known.
* `up` — it ran and the service answered properly.
* `warning` — it ran, the service answered, and something it measures is
  inside its warning margin: a certificate that expires in nine days.
  Available now, and **counted as up** in every availability figure.
* `down` — it ran and the service did not answer, or answered wrongly (a
  503, an `NXDOMAIN` from the server that should serve the name, a
  certificate that expired yesterday, a stratum-16 time server). The
  measurement is kept: a 503 still has a response time and a status.
* `cannot_run` — the check could not be attempted at all: no credential
  (`no_credential`), nothing to aim at (`no_address`), a name that will not
  resolve (`dns_resolve`), a machine that may not send ICMP (`permission`),
  a simulated device (`sim`), parameters that are not valid (`invalid`), an
  object the agent does not have (`snmp_no_such_object`). This says nothing
  about the service. It is **never** an outage, it writes **no** `check.up`
  sample, and it does **not** resolve an open outage alert — "we cannot tell
  any more" is not "it recovered".

Each state carries a stable `code` and the engine's own sentence as
`message`, plus the measurements the run produced (`CheckFacts`: loss, HTTP
status and final URL, DNS rcode and record count, NTP offset and stratum,
certificate subject / issuer / `not_after` / days left / trusted, TLS
protocol, the SNMP value). Every field is `null` until something measured it.

Metrics, with the **check's id** as the instance so that renaming a check
does not split its series: `check.up` (1/0, only for a run that reached a
verdict), `check.rtt_ms`, `check.cert_days`, `check.loss_pct`. Charted
through `metrics.query` like anything else.

History is the **changes**, not the runs: one row per transition with the
previous status and how long it had lasted (`held_s`), which is what answers
"when did it last go down and for how long" without keeping every run for
ever. `checks.history` adds a summary whose five figures are kept apart —
`up_s`, `warning_s`, `down_s`, `cannot_run_s`, `unknown_s` — and
`availability_pct = (up + warning) / (up + warning + down)`, which is `null`
when nothing in the window was measured. Time that could not be measured is
excluded from it rather than counted as either.

Scheduling is the existing poller's: the same one-second tick, the same
`jitter` hashed from the id so a thousand checks do not all run on the first
tick, and the same concurrency semaphore, so service checks cannot escape
the cap by being a different kind of work. A check already running is not
started again. A check runs only while its device has `monitor.enabled`:
switching monitoring off for a device switches off everything about it.

Alerting goes through the rules engine, with its hysteresis, `for_s` damping,
maintenance windows and device-down suppression. There is no second alerting
path.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `monitor.status` | → `{enabled, devices_monitored, polls_per_min, last_cycle_ms, ping_method, checks: {total, up, warning, down, unknown, cannot_run, disabled}, problems: [..]}` — `problems` has `checks` for what is down and `checks_blind` for what cannot run, counted apart because a blind spot is not a failure |
| `monitor.poll_now` | `{device_ids}` → `{job_id}` |
| `monitor.device` | `{device_id}` → `{state: DeviceState, health: {cpu_pct?, mem_pct?, temp_c?, uptime_s?, at?}, interfaces: [IfStatus], checks: [ServiceCheckView], sources: {icmp, snmp, cli}}` |
| `monitor.interfaces` | `{device_id?, filter?, problem_only?, sort?, limit?, cursor?}` → `[IfStatus{device_id, device, name, description, admin_up, oper_up, speed_bps, in_bps, out_bps, util_pct, in_err_ps, out_err_ps, at}]` |
| `metrics.query` | `{device_id, metric, instance?, since, until?, step_s?}` → `{points: [[t, v]..], step_s, rollup: raw|5m|1h}` (gaps are gaps) |
| `metrics.query_many` | `{series: [{device_id, metric, instance?}], since, until?, step_s?}` |
| `metrics.series` | `{device_id}` → `[{metric, instance, latest: [t, v]?}]` |
| `metrics.top` | `{metric, limit?: 10, filter?}` → highest current values across devices/interfaces ("top talkers", "hottest", "busiest CPU") |
| `alerts.list` | `{state?: firing|acknowledged|resolved|open, severity?, device_id?, since?, limit?, cursor?}` → `[Alert]` |
| `alerts.get` / `alerts.ack {id, note?}` / `alerts.resolve {id, note?}` / `alerts.snooze {id, minutes}` | |
| `alerts.raise` | `{rule?: String, device_id?, instance?, severity, title, detail?, dedup_key}` → `Alert` — for other features (log rules, insights, config) |
| `alerts.rules.list` / `.save` / `.delete` / `.test` | |
| `checks.list` | `{device_id?, status?: up|warning|down|unknown|cannot_run|problem, kind?, filter?, enabled?, limit?, cursor?}` → `[ServiceCheckView{..ServiceCheck, kind, device, state: CheckState}]` |
| `checks.get` | `{id}` → `ServiceCheckView` |
| `checks.save` | `{id?, device_id, name, spec: CheckSpec, host?, enabled?, interval_s?, timeout_ms?}` → `ServiceCheckView`; `conflict` on a duplicate name, `invalid` for a timeout longer than the interval. Repointing a check (kind, host or port) drops what it found at the old target. |
| `checks.delete` | `{id}` → `{deleted}`; the rules engine then resolves the alerts it had raised |
| `checks.run_now` | `{id? , ids?, device_id?}` → `{ran, checks: [{check_id, name, kind, status, previous, changed, code, message, rtt_ms, facts}]}` — runs inline, bounded by `concurrency`, at most 50 |
| `checks.history` | `{id, since?, until?, limit?}` → `{current: CheckState, events: [{at, status, previous, held_s?, code, message}], summary: {up_s, warning_s, down_s, cannot_run_s, unknown_s, measured_s, availability_pct?, down_count, last_down_at?, last_down_for_s?}, series}` |
| `checks.kinds` | → what a form can offer: each kind with its default timeout and whether it sees certificates |
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
    CheckState { check?: glob, kinds?: [CheckKind], state: down|warning|cannot_run },
    ConfigChanged,                                  // on the event `config.changed`
}
struct ServiceCheck { id, device_id, name, spec: CheckSpec, host, enabled, interval_s, timeout_ms }
struct CheckState { check_id, device_id, status: unknown|up|warning|down|cannot_run, code, message,
    rtt_ms?, facts: CheckFacts, since?, last_run_at?, last_ok_at?, last_down_at?, runs }
```

Built-in rules (editable copies, can be disabled): device down (critical,
for 0 s after damping), uplink down (high), CPU > 90 % for 5 min, memory
> 90 % for 10 min, temperature > 70 °C, interface errors > 10/s for 5 min,
interface utilisation > 90 % for 10 min, packet loss > 20 % for 3 min,
RTT > 5× the device's 24 h median for 5 min, configuration changed (info),
service check down (high), certificate about to expire (low), service check
cannot run (medium, after 10 min — long enough that saving a credential does
not wake anybody).

A `check_state` rule is keyed on (rule, device, **check name**), so two
services failing on one device are two alerts, and a rule can name one check
(`check: "web"`) or one kind. It is judged from the checks' stored state, so
the same code answers after a poll, after a single check has run, and for
`alerts.rules.test`. A check that goes `cannot_run` yields *no verdict* for
a `down` or `warning` rule — neither firing nor resolving — which is what
stops a blind spot from reading as a recovery.

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
changed), `alerts.changed {alert}`, `monitor.status_changed` (`what` is
`rules`, `maintenance`, `channel` or `checks`), and `checks.ran
{check_id, device_id, name, kind, at, status, previous, changed, code,
rtt_ms}` after every service-check run.

## Tests

With an injected clock and fake probes (make the ping and SNMP layers
traits so tests drive them): damping and transitions; jitter and the
concurrency cap; counter-delta maths incl. wrap (32 and 64 bit) and reboot;
gap handling; each condition incl. hysteresis, `for_s`, `NoData`, device-down
suppression of child alerts, maintenance, snooze, re-fire after resolve;
`alerts.raise` dedup; notification payloads per channel kind against a local
HTTP listener, retries, secrets not in documents/events. Service checks:
every state against servers the test starts on loopback on an
OS-chosen port — a TCP listener, an HTTP server, a hand-built DNS responder,
an SNTP responder whose clock is deliberately wrong, and TLS servers whose
certificates are generated when the test runs (expiring in nine days,
expired three days ago) — plus, through the existing fakes, a probe that
cannot be attempted and an SNMP check with no credential; that `cannot_run`
writes no sample and does not resolve an outage alert; the history,
`held_s` and an availability figure that excludes what it could not measure;
per-check alerts through the rules engine with `for_s` damping, maintenance
muting and device-down suppression; the scheduler running each check on its
own interval. Live: SNMP polling
against the in-crate `nocturne_snmp::Agent` (and a private `snmpd` if
installed), ICMP to 127.0.0.1 where permitted, CLI fallback against a fake
device with canned `show interfaces` output; a 2,000-device simulated cycle
stays within the concurrency cap and finishes.
