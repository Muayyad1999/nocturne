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

**Cadence does not depend on the time of day.** Whether a poll, a check, a
CLI read or the next look at the inventory is due is a difference of two
readings of a clock that only goes forwards (`Clock::monotonic_ms`), never of
two timestamps. The time of day (`Clock::now_ms`) is what samples, alerts and
history rows are *stamped* with, and it can be stepped — by NTP, by a person,
by a machine that boots an hour fast — in either direction without a poll or
a run being skipped or bunched. (A rule's `for_s` wait is still measured in
timestamps; if the time of day is set back during one, the wait starts over
rather than lasting for the size of the step.)

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
  and IPAM see interface state on SNMP-only devices — only from a reading
  SNMP itself produced, never a copy of what the CLI read.
* **The ifIndex map** (`monitor.ifindex`). A trap says `ifIndex=3` and a
  flow record says "input interface 3"; the poller is what knows that 3 is
  `GigabitEthernet0/3`. After an SNMP poll the agent's interface-name
  columns are read by row index and the pairing is stored per device
  (collection `monitor_ifindex`), so it is there after a restart, before
  the device has been polled again. That request is the last thing a poll
  does, after its samples, the device's state and `monitor.polled` are
  out: it is for other features, and an agent that stops answering between
  two requests must not make the poll's own results wait for it. A pair is in the map only if the agent
  gave the row a name *and* the poll knows an interface by that name,
  spelled as `monitor.interfaces` and the metrics spell it; a row the agent
  left nameless (shown on screen as `ifIndex 7`) is not in it. The map is
  read again when the device restarted (uptime went backwards — the one
  moment an agent may renumber), when the poll read other interface names
  than last time, and otherwise every five minutes; `at` is when it was
  last read. A read that fails leaves the stored map as it was and never
  fails the poll. A device that has never answered SNMP has no map: `at` is
  absent and the list is empty, which is a different answer from a map read
  off an agent with no interfaces (`at` set, list empty). Deleted with the
  device.
* **CLI fallback** (devices with CLI access but no SNMP, devices whose SNMP
  is switched on but not answering, and lab devices): `collect::run` for
  `interfaces` + `health` at a slower cadence (`cli_interval_s`, default
  300; lab devices 30) and derive the same metrics from counters.
* **A counter that goes down** while the uptime went forwards: a 32-bit
  counter wrapped (modulo 2^32). A 64-bit counter wrapped only if it had
  reached the top of its range (the wrapped difference is at most 2^56);
  otherwise it was started again on the device — statistics cleared, a line
  card re-seated — and the interval is a gap. A counter read off CLI text
  has no known width and never wraps: `clear counters` is a gap, not
  10^17 bit/s.
* A failed SNMP/CLI poll is a recorded gap, not zeros. The counters held
  from the last reading are dropped when the source that produced them was
  asked again and did not answer — not when the other source failed, and
  not on a poll at which the CLI was simply not due.

Metrics (series = device + metric + instance):
`icmp.rtt_ms`, `icmp.loss_pct`, `up` (1/0), `uptime_s`, `cpu.pct`,
`mem.pct`, `temp.c` (instance = sensor), and per interface (instance =
canonical interface name) `if.in_bps`, `if.out_bps`, `if.util_pct`,
`if.in_err_ps`, `if.out_err_ps`, `if.in_disc_ps`, `if.out_disc_ps`,
`if.oper` (1/0).

Settings section `monitoring`: `{enabled: true, concurrency: 32,
down_after: 2, cli_interval_s: 300, retention_days_raw: 7,
retention_days_rollup: 365, ping_timeout_ms: 1000, checks_enabled: true,
check_history_days: 90, cert_warn_days: 14, notify_for_lab: false}`. A
maintenance task calls `samples_maintain` hourly and prunes service-check
history. `notify_for_lab` decides whether an alert about a lab device is
announced at all (see Notifications).

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
| `icmp` | `ping::ping` through the poller's `Reachability` | `count` (1–16, default 3), `loss_pct_down` (0–100, default 100; `0` means *any* loss is down, never that no loss is) |
| `tcp` | `tcp::check` | `port` |
| `dns` | `dns::lookup`, asked **of the target** | `name`, `record` (default `A`), `expect`, `port` (53) |
| `http` | `http::check` | `path`, `port` (80), `method`, `expect_status` (default any 2xx/3xx), `expect_text`, `follow_redirects` (3) |
| `https` | `http::check` over TLS | the same, plus `verify_tls` (true), `warn_days`, `port` (443) |
| `tls` | `tls::inspect` | `port` (443), `sni`, `require_trusted` (true), `require_hostname` (true), `warn_days` |
| `ntp` | `ntp::query` | `port` (123), `max_offset_ms` |
| `snmp` | the device's own SNMP session | `oid` (default `sysUpTime`); no `host` |

A check's `host` is empty by default, meaning the device's own address; set
it for a service on a virtual address or under a name. An `snmp` check cannot
have one: it goes over the device's own session, to the device's address and
SNMP port with the device's credential, so a host on it would be a target the
check claimed and never contacted. `checks.save` refuses it (`invalid`); a
host left on a stored SNMP check by a version that accepted one is ignored
when the check runs (it does not get the check past the simulated-device
guard either) and dropped when the check is next saved. Interval 5 s–24 h
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

Keeping only the changes makes every figure an inference — "up from nine to
ten" rests on the runs in between having happened — so the history has rules
for where the inference stops. **Time nobody measured is `unknown`, never the
last status:**

* **A watch that was broken is closed where it broke.** When a run begins
  more than `2 × interval + timeout + 30 s` after the previous one ended
  (one run may be late or skipped; two may not), nobody was looking in
  between: the application was closed, the machine asleep, the check or its
  device's monitoring switched off. The run first appends a row
  `previous → unknown` dated at the **last run** (code `not_measured`), then
  records itself with `previous = unknown`, `held_s = null` and `since` =
  now. So `down` at 18:00, application closed, `up` at 09:00 reads as an
  outage seen until 18:00 and fifteen hours unknown, not as fifteen hours of
  downtime. A status found again after such a gap is a new change (a `down`
  before and after it counts twice in `down_count`: nothing says it was one
  outage). The device's timeline gets a line for the run only if it found
  something other than what was last heard.
* **An edit is not a hole, and does not hide one.** The silence is counted
  from the last run or, if it is later, from the last time the check was
  saved while switched on: a save is when a cadence begins (the scheduler
  gives a saved check a fresh slot in its interval), so an hourly check made
  a one-minute check forty minutes after it ran has not gone forty minutes
  unmeasured, and its `since` does not start again. That is only sound
  because `checks.save` closes a hole that is already there when it is
  called, judged by the cadence the check had until then: it appends the
  same `previous → unknown` row at the last run and leaves the state
  `unknown` (keeping `last_ok_at` / `last_down_at`) until the next run. A
  check renamed in the morning before its first run does not vouch for the
  night. A save that leaves a check switched off moves nothing: a check
  that is off has no cadence to restart.
* **The stretch after the last change** in a window is closed by the next
  change on record when there is one after the window: the status lasted
  until then, as between any two rows, whatever the check is doing now (a
  check repointed last week still says how it spent the week before).
  Where there is none the summary closes it itself, since no row does: the
  status counts up to `until` only while the check is being watched (below)
  and its last run — or last save, as above — is no older than that same
  allowance, and otherwise up to the last run. A check switched off for
  three days has three days of `unknown`, not three days of whatever it last
  said.
* **Pointing a check somewhere else** — a different kind, host, port or
  device — drops what it found and appends `previous → unknown` (code
  `repointed`) at its last run. The history rows stay with the check; when
  it moves to another device they are re-homed to it.
* **Pruning keeps what a window needs.** The hourly pass deletes changes
  older than `check_history_days` except, per check, the newest of them:
  that row is the status the retention opens on. (It also deletes rows whose
  check no longer exists.) Where a window still opens before the oldest row
  — an installation pruned by an earlier version, a window longer than the
  retention — the oldest row's own `previous` and `held_s` are used, and
  with no row at all the check's `since`: a check that has been up for a
  year reads as up, not as unknown.
* **The summary is added up from every change in the window**, whatever
  `limit` is. `events` is the newest `limit` changes (default 500, at most
  5,000), oldest first, with `events_total` and `truncated` beside it; to
  page back, ask again with `until` just before the oldest row returned.

A run records against the definition it was started from. If the check was
edited, repointed or deleted while its probe was out, the result is dropped:
no state, no history row, no sample, no log line, no rule evaluation.
`checks.run_now` reports such a check with an `error`.

Scheduling is the existing poller's: the same one-second tick, the same
`jitter` hashed from the id so a thousand checks do not all run on the first
tick, and the same concurrency semaphore, so service checks cannot escape
the cap by being a different kind of work. A check already running is not
started again.

**A check is watched** when it is switched on, its device has
`monitor.enabled`, and `monitoring.enabled` and `monitoring.checks_enabled`
are on (`checks::watched` — the one place this is written). Switching
monitoring off for a device switches off everything about it, and everything
that asks whether a check is live asks the same question:

* the scheduler does not run an unwatched check;
* `checks.run_now` does not either: it lists the check with `skipped`
  (`disabled`, `monitoring_off` or `checks_off`) and a sentence, and counts
  only what it ran — a run by hand would record a verdict and raise an alert
  that no later run would ever take back;
* `monitor.status` counts it under `disabled`, whatever it last found, and
  it is not among the `checks` / `checks_blind` problems;
* `checks.history` stops crediting its last status with time (above);
* the rules say nothing about a device whose monitoring is off, and what
  they had open on it is resolved when it is switched off (see Evaluation).

What a check last found stays on the check (`checks.list` still shows it,
with its `last_run_at`); it is a record, not a claim about now.

Alerting goes through the rules engine, with its hysteresis, `for_s` damping,
maintenance windows and device-down suppression. There is no second alerting
path.

What a check found **before its device came back from being down** is not a
verdict on the device that came back. While a device is down its checks go
`down` too and are recorded without an alert (one cause, one alert); the
poller is then usually first to see the device answer, and judging the
stored `down` at that moment would raise a service-down alert per check
after the outage is over. So a check whose `last_run_at` is older than the
device's last down→up transition (`DeviceRecord.up_since`, stamped when the
device is declared up) yields no verdict — it neither raises nor resolves —
until the check's own next run, on its own schedule.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `monitor.status` | → `{enabled, devices_monitored, polls_per_min, last_cycle_ms, ping_method, checks: {total, up, warning, down, unknown, cannot_run, disabled}, problems: [..]}` — `problems` has `checks` for what is down and `checks_blind` for what cannot run, counted apart because a blind spot is not a failure; `disabled` is every check that is not being watched (switched off itself, or by its device's or the installation's monitoring switch), and those are in none of the other figures |
| `monitor.poll_now` | `{device_ids}` → `{job_id}` |
| `monitor.device` | `{device_id}` → `{state: DeviceState, health: {cpu_pct?, mem_pct?, temp_c?, uptime_s?, at?}, interfaces: [IfStatus], checks: [ServiceCheckView], sources: {icmp, snmp, cli}}` |
| `monitor.interfaces` | `{device_id?, filter?, problem_only?, sort?, limit?, cursor?}` → `[IfStatus{device_id, device, name, description, admin_up, oper_up, speed_bps, in_bps, out_bps, util_pct, in_err_ps, out_err_ps, at}]` |
| `monitor.ifindex` | `{device_id}` → `{at?, interfaces: [{index, name}]}` — the SNMP ifIndex of each interface the agent named, sorted by index, with when the map was last read. `at` is absent (not null) and the list empty for a device that was never read over SNMP. `not_found` for an unknown device. For the trap and flow decoders |
| `metrics.query` | `{device_id, metric, instance?, since, until?, step_s?}` → `{points: [[t, v]..], step_s, rollup: raw|5m|1h}` (gaps are gaps) |
| `metrics.query_many` | `{series: [{device_id, metric, instance?}], since, until?, step_s?}` |
| `metrics.series` | `{device_id}` → `[{metric, instance, latest: [t, v]?}]` |
| `metrics.top` | `{metric, limit?: 10, filter?}` → highest current values across devices/interfaces ("top talkers", "hottest", "busiest CPU") |
| `alerts.list` | `{state?: firing|acknowledged|resolved|open, severity?, device_id?, since?, limit?, cursor?}` → `[Alert]` |
| `alerts.get` / `alerts.ack {id, note?}` / `alerts.resolve {id, note?}` / `alerts.snooze {id, minutes}` | `alerts.resolve` resolves the alert with that id (not whatever is open under its key) |
| `alerts.raise` | `{rule?: String, device_id?, instance?, severity, title, detail?, dedup_key, notify?: [channel_id]}` → `Alert` — for other features (log rules, detections, insights, config). `notify` absent or `null` is the usual routing (every enabled channel), `[]` is nobody, a list is those channels and no others; `not_found` for an id that is not a channel, and then nothing is raised |
| `alerts.clear` | `{dedup_key, note?}` → `{cleared: bool}` — the condition another feature raised an alert for is over: resolves the open alert (firing or acknowledged) under that key, with `note` as its `detail`. `false` when nothing is open under the key. `forbidden` when the open alert belongs to an alert rule. Audited as `alert.clear` when it closed something |
| `alerts.rules.list` / `.save` / `.delete` / `.test` | |
| `checks.list` | `{device_id?, status?: up|warning|down|unknown|cannot_run|problem, kind?, filter?, enabled?, limit?, cursor?}` → `[ServiceCheckView{..ServiceCheck, kind, device, state: CheckState}]` |
| `checks.get` | `{id}` → `ServiceCheckView` |
| `checks.save` | `{id?, device_id, name, spec: CheckSpec, host?, enabled?, interval_s?, timeout_ms?}` → `ServiceCheckView`; `conflict` on a duplicate name, `invalid` for a timeout longer than the interval, a `loss_pct_down` outside 0–100, or a `host` on an SNMP check. Repointing a check (kind, host, port, or moving it to another `device_id`) drops what it found at the old target. A save made after the check had already gone unrun for longer than its cadence allows closes that stretch in the history and returns `state.status = unknown`. The check rules are evaluated at once for its device (and for the one it was moved from), so switching a check off or moving it closes its alert without waiting for a poll. |
| `checks.delete` | `{id}` → `{deleted}`; the rules engine then resolves the alerts it had raised |
| `checks.run_now` | `{id? , ids?, device_id?}` → `{ran, checks: [{check_id, name, kind, status, previous, changed, code, message, rtt_ms, facts} \| {check_id, name, kind, skipped, error} \| {check_id, name, kind, error}]}` — runs inline, bounded by `concurrency`, at most 50. `ran` counts the checks that ran and were recorded. A check that is not being watched is not run: `skipped` is `disabled`, `monitoring_off` or `checks_off` and `error` says which switch. A check edited or deleted while it ran has `error` alone. |
| `checks.history` | `{id, since?, until?, limit?}` → `{current: CheckState, events: [{at, status, previous, held_s?, code, message}], events_total, truncated, summary: {up_s, warning_s, down_s, cannot_run_s, unknown_s, measured_s, availability_pct?, down_count, last_down_at?, last_down_for_s?}, series}` — `events` is the newest `limit` changes in the window, oldest first; `summary` is computed from all `events_total` of them |
| `checks.kinds` | → what a form can offer: each kind with its default timeout and whether it sees certificates |
| `alerts.summary` | → `{open, by_severity, newest}` |
| `maintenance.list` / `.save` / `.delete` | windows that mute alerts: `{id, name, scope, starts_at, ends_at, recurring?: weekly}` |
| `notify.channels.list` / `.save` / `.delete` / `.test` | |

```rust
struct Alert { id, rule_id?, rule_name, device_id?, device?, instance?, severity: info|low|medium|high|critical,
    state: firing|acknowledged|resolved, title, detail, value?, started_at, last_seen_at, resolved_at?,
    acked_at?, ack_note?, snoozed_until?, count, muted_by_maintenance: bool, notified_at?,
    notify?: [channel_id],   // what `alerts.raise` named: null = the usual routing, [] = nobody
    lab: bool,               // about a simulated device (a lab's, or any with `transport: sim`)
    dedup_key }
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
stops a blind spot from reading as a recovery. For the same reason a `down`
run yields no verdict for a `warning` rule: a timeout while the web server
restarts has learned nothing about the certificate. The exception is
`tls_expired`, a run that did see the certificate: it is no longer *about*
to expire, so the warning clears and the down rule takes over.

Evaluation: after each poll for the polled device, and after each service
check for that device's check rules. An alert is keyed by (rule, device,
instance): it fires once, counts repeats, resolves itself when the condition
clears (with `clear_value` hysteresis), and re-fires as a new alert after a
resolve. When a device is down, its interface/threshold alerts are not raised
(one cause, one alert). Lab devices alert only when a rule opts in — and
whoever raises it, an alert about a lab device is announced only when
`notify_for_lab` says so (see Notifications).

**The rules stand down for a device whose monitoring is off**
(`rules::stand_down`). An alert is a claim that something is wrong now, kept
true by the next poll or run, and such a device has no next one. When
`monitor.enabled` is switched off (the `devices.changed` event), every alert
the rules have open on the device is resolved with a note that says so —
its resolved notice goes out if its firing notice did — and any evaluation
that still reaches the device (`monitor.poll_now` polls whatever it is given;
`checks.save` evaluates) raises nothing and resolves the same way. Alerts
raised by other features through `alerts.raise`, and configuration-change
alerts (a record of something that happened), are left alone. Switching
monitoring back on starts from the next poll and the next run.

**One key, one open alert** holds under concurrency: looking for the open
alert and writing the new one (or the counted one, the acknowledged one, the
resolved one) is a single store transaction. Polls and checks finish on
different threads; `alerts.raise` can be called from anywhere.

**A sighting is a measurement.** `count` goes up once per observation, and
`last_seen_at` is when the condition was last observed. A check rule judges
a stored verdict, so its findings carry the run's time (`observed_at`):
evaluating the same run again — the next poll, another check on the device
finishing — counts nothing and publishes nothing (it can still send a firing
notice that was being held back, below), an all-clear older than the alert's
last sighting does not resolve it, and a sighting older than the last resolve
does not raise it again. So an alert resolved by hand stays resolved until
the problem is *measured* again, not until the next poll. An alert resolves
as of the observation that cleared it when that is later than the
evaluation's own time (a poll's evaluation is dated when the poll set out,
and a check may have run since): `resolved_at` is never earlier than the run
that cleared it. `alerts.raise` has no `observed_at`: every call is a
sighting.

**Muting is decided when a notification is due, not when the alert is
raised.** Maintenance windows and snoozes hold back the *firing* notice and
never the record:

* the firing notice goes out the first time the alert is looked at (the
  raise, any later evaluation that still finds the condition, whether or
  not it counts as a new sighting, or the scheduler's pass below) while it
  is still `firing`, no window covers its device and it is not snoozed;
  `notified_at` records when. An outage that starts inside a window and
  outlasts it is therefore announced at the first look after the window
  closes — once. For a check rule that is the device's next poll or the
  next run of any of its checks, not the check's own next run, which can be
  an hour or a day away. Deleting a window or cancelling a snooze has the
  same effect at the next look. An alert somebody acknowledged while it was
  muted is not announced afterwards: a person has it.
* **a look is about the alert's own device.** `alerts.raise` takes any key
  with any `device_id`, so a call can land on an open alert while naming
  another device, or none. That call is a sighting — counted, with its
  detail and severity — and decides no notice: the windows and the lab
  setting it asked about are not the alert's. `muted_by_maintenance` stays
  as it was, and the scheduler's pass, which asks about the device the
  alert is about, pays what is owed. Without this a second raise that left
  the device out announced an alert a window over that device was holding
  back — or one about a lab device.
* **an alert nothing looks at again still gets its notice.** A
  configuration change is its own alert per version; a log rule or a
  detection raises once; a threshold alert sitting between its two
  thresholds is evaluated without being sighted. So every 15 s the
  scheduler passes over the alerts that are open, `firing` and never
  announced, and sends the notice of each one that nothing holds back any
  more — at most 15 s after the window closes or the snooze runs out,
  whether or not anything else ever looks. It asks the same question a
  sighting asks, in the same place (`alerts::announce_if_due`), sets
  `notified_at` and publishes `alerts.changed` with `notification:
  "firing"` like any other announcement, and goes to the channels the
  alert would have gone to: the ones `alerts.raise` named, else its rule's
  as the rule stands now. The cadence is a difference of two readings of
  the clock that cannot be set back, like the rest of the scheduler, and
  it runs with `monitoring.enabled` off too (other features still raise
  alerts then). Its cost is bounded: it reads from an index that holds
  only the alerts still owing a notice, and at most 200 of them a pass,
  carrying on from where the last pass stopped — so a thousand lab alerts
  held back for as long as `notify_for_lab` is off cost a pass 200 rows
  and delay another alert's notice by a few passes at most. The pass names
  that index, so it can never quietly become a scan; for the same reason
  the index is made again at start when the one in the database was
  created with another condition (a pass that cannot use its index fails,
  and would leave every held-back notice unsent).
* one alert waits for more than that: a rule's alert last seen **before the
  application started**, about a device the scheduler polls and has not
  polled yet since. What the rules knew when the application was closed can
  be hours old; the device's first poll either sights the problem again
  (which announces it) or finds it gone and resolves it silently, and the
  pass does not announce it a few seconds earlier on yesterday's knowledge.
  An alert raised since the start, an alert no rule owns and an alert about
  a device nothing polls do not wait.
* the resolved notice goes out exactly when the firing notice did
  (`notified_at` is set), whatever window or snooze covers the alert by then:
  it closes an incident people already have open in a chat or a pager. An
  alert that was never announced resolves silently.
* `muted_by_maintenance` is a record of the last sighting, announcement or
  resolve, not a live answer; in between it can be out of date.

**An alert about a lab device reaches no channel unless `notify_for_lab` is
on** (default off). A lab is where things are broken on purpose. The alert
is raised, counted, listed and shown like any other (`lab: true`); only its
notice is held back, by the same function that holds one back for a window
or a snooze — the one place this is decided, for every source: the
monitor's own rules, `alerts.raise` from the log rules, the detections and
the insights. A lab device is an inventory device with `lab` set or with
`transport: sim`. A `notify` list on `alerts.raise` does not get round it,
nor does a rule's own channel list, a window closing, a snooze running out,
a restart, or a later raise under the same key that names another device or
none (above); and an alert that was never announced resolves without a
notice, so its end is no way round either.
Like a window, the setting is asked when a notice is due, not when the alert
is raised: switching it on announces, at the next look or the next pass, the
lab alerts that are still firing and were never announced, and switching it
off leaves an alert that *was* announced with its resolved notice. While it
is off, `alerts.changed` carries `notification: null` for a lab alert at
every change, so the desktop shell — which shows a system notification for
the one event whose `notification` is `"firing"` — shows none; with it on,
a lab alert's events are those of any other alert.

**Who is told** (`alerts.raise {notify}`). Absent or `null`: the usual
routing, every enabled channel whose `min_severity` admits the alert. A
list: those channels and no others (still subject to `enabled` and
`min_severity`); an id that is not a channel is refused, because an alert
that quietly reached fewer people than its caller meant is found out during
the outage it was for. `[]`: nobody — the alert is recorded and shown,
`notified_at` stays null for good, `alerts.changed` never carries a
`notification` for it (so the desktop shell stays quiet too), and it
resolves without a notice. The choice is kept on the alert (`notify`),
because its notice can be due long after the call that named the channels:
when a window closes, and again when `alerts.clear` — which names none —
resolves it. Until the alert has been announced the latest raise decides;
after that the audience stays, so the resolved notice reaches the people
who got the firing one. A channel deleted since is simply no longer sent
to; the alert does not fall back to everybody. A raise under a key whose
open alert belongs to an alert rule counts a sighting of it, as it always
has, and cannot change who that alert is announced to.

**Taking an alert back** (`alerts.clear`). A feature that raised an alert
because of something that has an opposite — a neighbour lost and found, a
link down and up — clears it under the same `dedup_key`. That resolves the
open alert, firing or acknowledged, with the note as its detail and its
resolved notice if its firing notice went out; the audit entry
(`alert.clear {alert_id, severity}`) says a condition ended, not that a
person resolved it, and holds neither the title, the key nor the note,
which can all carry text a device wrote. It answers `{cleared: false}`
when nothing is open under the key — a clear that arrives twice, or for
an alert a person already resolved, is not an error. **It cannot close an
alert an alert rule owns** (`rule_id` set): a rule's alert resolves when
the rule's own condition clears, and a feature that could close it by
naming its key (`rule:<rule>:<device>:<instance>`) would hide an outage the
rule still sees. That is refused with `forbidden`, checked again inside the
transaction that resolves. A raise and a clear of one key that arrive
together are each one store transaction: every raise is a sighting of
exactly one alert, every `cleared: true` closed exactly one.

Notifications: channel kinds `webhook` (generic JSON POST), `slack`,
`teams`, `discord` (their incoming-webhook formats), `email` (SMTP with
STARTTLS — only if it can be done with rustls + ring and no C dependency;
otherwise leave it out and say so), plus the event `alerts.changed` that the
desktop shell turns into a system notification. URLs and SMTP passwords live
in the vault (`monitor:channel:<id>:secret`), never in documents. A
notification failure is retried with back-off (3×) and recorded, never blocks
polling. `notify.channels.test` sends a clearly marked test message.

Events (contract): `devices.state` (from the core), `monitor.polled
{device_id, at, ok}`, `monitor.interfaces {device_id}` (interface table
changed), `alerts.changed {alert, notification}`, `monitor.status_changed`
(`what` is `rules`, `maintenance`, `channel` or `checks`), and `checks.ran
{check_id, device_id, name, kind, at, status, previous, changed, code,
rtt_ms}` after every service-check run.

`alerts.changed` is published for every change to an alert (raised, counted,
announced late, acknowledged, snoozed, resolved). `notification` is
`"firing"` on the one event at which the firing notice became due and was
handed to the channels
(whether or not any channel is configured), `"resolved"` on the one at which
the resolved notice was, and `null` otherwise — so a consumer that shows its
own notification (the desktop shell) honours maintenance windows, snoozes,
late announcements, the lab setting and `notify: []` by looking at this
field alone, rather than at `count == 1`, at `muted_by_maintenance` or at
`lab`. An alert raised for a list of channels is announced to the desktop
shell as well: the event says a notice went out, not to whom.

Alerts stored before `notified_at` existed are given one at the first start
with this version: the time they were raised, unless their last sighting was
inside a maintenance window. That is what the earlier code did, and it keeps
an upgrade from announcing every open alert a second time.

## Tests

With an injected clock and fake probes (make the ping and SNMP layers
traits so tests drive them): damping and transitions; jitter and the
concurrency cap; counter-delta maths incl. wrap (32 and 64 bit) and reboot;
gap handling; each condition incl. hysteresis, `for_s`, `NoData`, device-down
suppression of child alerts, maintenance, snooze, re-fire after resolve;
`alerts.raise` dedup, also under concurrent callers on a multi-threaded
runtime, with rule evaluations of one device overlapping and with
acknowledgements landing among sightings (several rounds each: one round
lets a non-atomic raise through too often on a busy machine); a stale
all-clear and a stale sighting arriving after the alert has moved on;
`alerts.resolve` by id with two alerts under one key; a notice held back by
a window or a snooze and sent at the first evaluation after it (also when a
slow check has not run again), a resolved notice paired with its firing
notice, and the `notification` field of `alerts.changed`; an alert raised
once inside a window (through `alerts.raise`, and a configuration change)
announced by the scheduler's pass after it with nothing else looking, to
the channels its rule names, once, and its end announced too; a snooze
running out the same way; nothing announced for an alert acknowledged,
resolved or cleared while it was held back; a rule's alert from before the
application started waiting for its device's first poll, which announces
it if the problem is still there and resolves it silently if not, while a
log rule's alert of the same night does not wait; the scheduler making the
pass with polling switched off and the time of day set back an hour; one
pass reading 200 of 451 owing alerts and the next carrying on; the query
plan of the pass (the index of owing alerts, not a scan) and what that
index holds, and an index of that name left with another condition made
again at the next start; `alerts.raise {notify}` reaching the named channels and no
others at raise and at resolve, `[]` reaching nobody ever, an unknown
channel refused with nothing raised, the audience following the latest
raise until somebody has been told, and a raise "for nobody" under a rule's
key leaving the rule's alert to be announced; `alerts.clear` resolving by key with
its note, answering `false` when nothing is open, closing an acknowledged
alert, refusing a rule's alert, audited without the title, the key or the
note, and holding under concurrent raises and clears of one key (every
raise one sighting, every `cleared: true` one alert closed); an alert
about a lab device — raised through `alerts.raise`, with and without a
`notify` list, for a device in a lab and one with `transport: sim`, and
raised by the monitor's own rule (which names the channel itself) —
reaching a recording channel only with
`notify_for_lab` on, with the event's `notification` null until then; a lab
alert raised in a window, snoozed, seen again, raised again under its key
with no device, another device and a channel named, then cleared and
resolved, reaching no channel and no `notification` at any of it; a second
raise that names no device or another one leaving an alert held back by a
window over its own device unannounced until the pass after the window; a
snooze made longer moving the notice and one taken off letting it through;
a notice owed when the application is closed paid once by the next one, and
not again by the one after, with what was announced before staying
announced and a lab alert staying quiet across both; an alert cleared and
raised again being a new alert, announced again, whatever was done to the
first (snoozed, acknowledged); a clear landing among an acknowledgement and
a snooze of the same alert leaving it closed; what resolves for
`alerts.clear` refusing a rule's alert by itself; the index map asked for
only after `monitor.polled`;
`monitor.ifindex` against a fake agent (the agent's names as the interface
table spells them, a nameless row and a row the poll did not read left
out, no map for a device never read
over SNMP against an empty map for an agent with no interfaces, read again
on a restart, on a new port and after five minutes, kept when the read
fails, and still there after the application is restarted on the same data
directory); a counter cleared
on the device
(CLI text and 64-bit SNMP) as a gap; the CLI fallback keeping its counters
while SNMP is on and failing; notification payloads per channel kind against a local
HTTP listener, retries, secrets not in documents/events. Service checks:
every state against servers the test starts on loopback on an
OS-chosen port — a TCP listener, an HTTP server, a hand-built DNS responder,
an SNTP responder whose clock is deliberately wrong, and TLS servers whose
certificates are generated when the test runs (expiring in nine days,
expired three days ago) — plus, through the existing fakes, a probe that
cannot be attempted and an SNMP check with no credential; that `cannot_run`
writes no sample and does not resolve an outage alert; the history,
`held_s` and an availability figure that excludes what it could not measure;
that time in which nothing ran (a night with the application closed, a
check switched off for days) is `unknown` and never the last status, while
one late run breaks nothing; that a watch resuming on what it last heard
is a row in the history and no line on the timeline; that tightening a
check's interval is not a hole, an edit after a night of silence does not
hide the night, and edits to a switched-off check do not keep its watch
alive; that a window in the past says the same whatever became of the
check afterwards; that pruning keeps the row a window opens on (also when newer
changes follow it inside the retention),
drops the rest and the rows of deleted checks, and that a history already
pruned bare still reads from the oldest row and from the check's `since`;
that the summary is the same whatever `limit` is and a cut list keeps the
newest changes and says it was cut; `loss_pct_down: 0` on a host that
answers every echo, and values outside 0–100 refused; a host refused on an
SNMP check, ignored on a stored one (also on a simulated device) and
cleared without resetting the state; a run that outlives a repoint or a
delete leaving nothing behind; a check moved to another device starting
from `unknown` there, closing its alert on the old device and taking its
history with it; monitoring switched off for a device closing its alerts
(but not one raised through `alerts.raise`, nor a configuration-change
alert) and taking its checks out of the counts, `checks.run_now` leaving such checks
(and switched-off ones) alone and saying so, and a poll by hand raising
nothing about such a device; switching a check off closing its alert at
once; the scheduler, the CLI cadence and a `for_s` wait all carrying on
when the time of day is set back an hour;
per-check alerts through the rules engine with `for_s` damping, maintenance
muting and device-down suppression; no alert from a verdict that predates
the device's recovery (including one recorded while the poll that found the
device back was still out), and one sighting per run however often it is
evaluated; a certificate warning that survives a run which never saw the
certificate; the scheduler running each check on its
own interval. Live: SNMP polling
against the in-crate `nocturne_snmp::Agent` (and a private `snmpd` if
installed), ICMP to 127.0.0.1 where permitted, CLI fallback against a fake
device with canned `show interfaces` output; a 2,000-device simulated cycle
stays within the concurrency cap and finishes.
