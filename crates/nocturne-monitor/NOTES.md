# nocturne-monitor — build notes

Spec: `docs/dev/features/monitor.md`. Conventions: `docs/dev/features/README.md`.

## State: complete

123 tests, `cargo clippy -p nocturne-monitor --all-targets --no-deps -- -D warnings`
clean, `cargo fmt` applied, no `unsafe`.

```
src/
  lib.rs          init(): settings, tables, state, routers, on_start
  settings.rs     the `monitoring` section
  model.rs        Alert, AlertRule/Condition/Scope, MaintenanceWindow, Channel,
                  IfStatus, DeviceHealth/Sources/DeviceRecord  (contract shapes)
  clock.rs        Clock trait; SystemClock + VirtualClock (follows paused tokio time)
  probe.rs        Reachability trait (ping + tcp) over nocturne-netprobe
  snmp.rs         SnmpSession / SnmpTransport traits, RealSnmp, per-device session cache
  read.rs         one `Reading` shape from either SNMP or the CLI collector
  rates.rs        counter deltas over uptime deltas: wrap, reboot, unusable
  poll.rs         one poll: reach, read, rates, damping, samples, events; poll_now job
  db.rs           own tables (monitor_interfaces, monitor_alerts) + collections
  interfaces.rs   the interface table, filtered/sorted/paged in SQL
  alerts.rs       lifecycle keyed on dedup_key; alerts.* RPC
  rules.rs        built-ins, tri-state evaluation, for_s, suppression, alerts.rules.*
  maintenance.rs  windows (one-off and weekly)
  scope.rs        which devices a rule or window covers
  metrics.rs      metrics.* RPC, glob matching, resolution choice
  notify.rs       channels, payload per kind, retries, notify.* RPC
  smtp.rs         a small SMTP submission client on tokio + tokio-rustls
  scheduler.rs    the scheduler, the hourly maintenance pass, the event listener
  status.rs       monitor.status / .device / .interfaces / .poll_now
  testing.rs      fake probe, fake SNMP agents, Harness
  tests/          polling, counters, health, lifecycle, scheduling, notifications,
                  live (in-process SNMP Agent + CLI), snmpd (real net-snmp), rpc
```

## Decisions worth remembering

* **`Condition::RttBaseline` was added** to the condition enum. The built-in
  "RTT above 5× the 24-hour median" rule the spec asks for cannot be written
  with the five conditions it lists. Everything else in the enum is as
  specified. Also in `ui/src/api/types/monitor.ts`.
* **`Threshold.metric` accepts a glob**, so the built-in "interface errors"
  rule can be one rule over `if.*_err_ps`. `instance` was already a glob.
* **`notify` empty on a rule = every enabled channel.** A channel also has a
  `min_severity`.
* **Built-in rules cannot be deleted, only disabled** (`forbidden`, with the
  reason). Deleting one would only bring it back at the next start.
* `monitor.interfaces` and `alerts.list` return plain arrays, as the contract
  says, so `cursor` is "rows already received" as a decimal string.
* `alerts.list`'s `severity` filter means *at least* this severe.
* SNMP interfaces are handed to `collect::store(.., Interfaces, .., "snmp")`
  at the CLI cadence, not every poll: the collected store keeps one previous
  generation for "what changed since last time", and rewriting it every
  minute would make that useless.
* A failed ICMP probe still records `icmp.loss_pct` (it is a measurement); a
  probe that could not be *sent* records nothing but `up = 0`. A TCP-fallback
  handshake time goes to `DeviceState.rtt_ms` but never to `icmp.rtt_ms`.
* The scheduler re-reads the inventory at most every 5 s and ticks every 1 s;
  reading a few thousand devices out of the store four times a second cost
  more than the polling did.
* Everything that needs "now" asks the `Clock` on `MonitorState`, including
  `metrics.*`, so a paused-timer test has one consistent notion of time.
* **The scheduler's jitter is hashed from the device id, and the core hands
  out a random id per device**, so *when* a device is first polled is random
  from a test's point of view. Any test that asserts how often something was
  polled in a window pins the phase first with `Harness::due_now`; the test
  that is about the spread does not pin, and checks the observed count
  against `scheduler::jitter` for those very ids instead of against a fixed
  range. `one_slow_device_delays_nothing_else` used to fail about a third of
  the time for exactly this reason (its 60 s slot fell outside the 40 s
  window), which is what the pinning fixed.

## Test hazards found by CI on other platforms

* **A metric window must be a whole number of steps.** The store buckets raw
  samples by `span / max_points`, and `max_points` is `span / step` rounded
  down, so a 301 s window at a 60 s step asks for buckets of 60.2 s — wider
  than the 60 s between samples. Two samples then share a bucket whenever the
  first falls in the opening 800 ms of a 60.2 s cycle: 800 phases in 60,200,
  about one run in eighty. `a_metric_window_of_whole_steps_gives_one_point_
  per_sample` pins that exact phase and checks several others.
* **A paused tokio runtime must not be mixed with real I/O.** It fast-forwards
  to the next armed timer whenever nothing is ready to run, and waiting on a
  socket is exactly that: measured here, one loopback request with a 15 s HTTP
  timeout armed advanced a paused clock by the full 15 s when the peer was
  300 ms slow. The back-off schedule is therefore timed with no network in the
  path, and every test that talks to a listener runs on real time.
* **A test must not assert the behaviour of a peer it does not control.** The
  wrong-community test now establishes that the agent is up (by polling it
  with the right community, which also replaces a fixed 400 ms wait that could
  expire before `snmpd` had bound) and skips with a reason if that `snmpd`
  answers any community, as some builds do.

## Needed from the core (worked around for now)

* `metrics.top` and the retention sweep read and write the core's `series`,
  `samples` and `rollups` tables with `store.with(...)`. There is no API for
  "latest value of every series with this metric" or for a retention policy,
  and `samples_maintain`'s retention is hard-coded (48 h raw / 30 d / 400 d),
  so `retention_days_raw` can only be honoured when it is stricter.
* `store.samples_query` decides raw-vs-roll-up against `util::now_ms()`
  rather than a caller-supplied time, which is why the tests' virtual clock
  has to start at the real present.
