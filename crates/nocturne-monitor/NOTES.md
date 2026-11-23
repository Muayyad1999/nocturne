# nocturne-monitor — build notes

Spec: `docs/dev/features/monitor.md`. Conventions: `docs/dev/features/README.md`.

## State: complete

263 tests, `cargo clippy -p nocturne-monitor --all-targets -- -D warnings`
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
  scheduler.rs    the scheduler (devices and service checks), the hourly
                  maintenance pass, the event listener
  status.rs       monitor.status / .device / .interfaces / .poll_now
  checks/         per-host service checks
    model.rs      CheckKind/CheckSpec/CheckStatus/CheckState/CheckFacts (contract)
    outcome.rs    ProbeError → did the check measure the service, or not run?
    run.rs        aim one check, drive its engine, judge the answer, samples
    store.rs      monitor_checks + monitor_check_events, state, history, summary
    rpc.rs        checks.* (list/get/save/delete/run_now/history/kinds)
  testing.rs      fake probe, fake SNMP agents, Harness
  tests/          polling, counters, health, lifecycle, scheduling, notifications,
                  live (in-process SNMP Agent + CLI), snmpd (real net-snmp), rpc,
                  checks (the five states, history, alerting, scheduling),
                  check_history (what the history may claim), check_definitions
                  (limits, hosts, stale runs, moves), check_watching
                  (monitoring switched off),
                  check_engines (real listeners), listeners + certificates
                  (the servers those tests start)
```

## Decisions worth remembering

### Service checks

* **Five states, not three.** `unknown` (never run), `up`, `warning`, `down`
  and `cannot_run`. The last is the whole point: a check with no credential,
  nothing to aim at, a name that will not resolve or a machine that may not
  send ICMP has not found an outage — it has failed to look. It writes no
  `check.up` sample (a gap, like a failed poll), it is counted apart on
  `monitor.status` (`checks_blind`), it has its own alert rule at a lower
  severity after ten minutes, and — the part that matters most — it yields
  **no verdict** for a `down` rule, so it can neither raise an outage nor
  resolve one. "We cannot tell any more" must never read as a recovery.
  `checks::outcome::from_probe_error` is the only place that decides which
  side of that line an engine error falls on, and every variant is tested.
* **A certificate about to expire is `warning`, not `down`.** It is serving
  today; the date it stops is known. A warning counts as **up** everywhere
  (`CheckStatus::is_up`), including in `availability_pct`, because saying a
  working service is down is how service-down alerts get ignored. Expiry is
  judged before trust, since an expired certificate also makes the chain
  untrusted and `tls_expired` with a number of days says far more than "the
  chain could not be verified". The days left are their own series
  (`check.cert_days`), so "what expires next month" is a query.
* **The series instance is the check's id, not its name.** A rename must not
  split a history; the id is also what `checks.history` takes. The UI maps
  id to name from `checks.list`. An alert's `instance`, by contrast, is the
  **name**, because that is what a person reads in "web on SW1 is down" —
  which is why `conditionMetric` in the UI cannot resolve a chart from a
  check rule alone.
* **History is the changes, not the runs.** One row per transition with the
  previous status and how long it had lasted (`held_s`). That answers "when
  did it last go down and for how long" without keeping every run for ever,
  and the response times are already in the time series. The price is that
  every figure is an inference from two rows, and the inference has to stop
  somewhere: see "What a check's history may claim" below.
* **Availability excludes what could not be measured.** `up_s`, `warning_s`,
  `down_s`, `cannot_run_s` and `unknown_s` are kept apart, and
  `availability_pct` is `null` when nothing in the window was measured. A
  flattering 100 % for a window nobody watched is the exact failure this
  crate's first rule forbids.
* **A check on a simulated device reports `sim` and sends nothing.** A lab
  device's `address` field is either empty or belongs to a stranger on the
  real network — the same hazard `poll::plan` guards. The check still exists
  and is still treated like any other (lab devices are devices); it just
  says truthfully that a console is not a service. Naming a host on the
  check overrides this, because that is a deliberate act.
* **Severity lives on the rule, not on the check.** The Dude puts it on the
  service; here it would be a second place for the same decision, and the
  rules engine is the only thing allowed to decide who gets woken. "This
  check is critical" is a rule with `check: "<name>"` and a scope — which is
  also how one check gets a different severity from the rest.
* **`Condition::CheckState` was added** rather than bending `Threshold` at
  `check.up`. A threshold cannot say "this named check on this device", and
  cannot tell `down` from `cannot_run` at all — `check.up` has no value for a
  check that did not run, which is the point. It is evaluated from the
  checks' stored state, so one code path answers after a poll, after a single
  check has run (`rules::evaluate_checks`) and for `alerts.rules.test`.
* **No new network code and no new scheduler.** ICMP goes through the
  existing `Reachability` trait (whose contract already separates "could not
  be attempted" from "no answer", which is exactly the distinction needed),
  SNMP through the existing per-device session, and TCP/DNS/HTTP/TLS/NTP
  straight to `nocturne-netprobe`. Runs are driven by the device poller's own
  tick, jitter and concurrency semaphore.
* **`nocturne-toolbox`'s `ProbeError` mapping was left where it is.** It
  looks similar but answers a different question — "how should this RPC call
  be refused" — and `Resolve` is `not_found` there and `cannot_run` here. Two
  functions, deliberately, with the shared strings noted in both.
* **`Harness::samples` asked the wrong window.** It queried from the epoch,
  and `store.samples_query` reasonably answers such a span from hourly
  roll-ups, which a test's seconds-old samples are not in — so it returned
  empty for *everything*, and every existing use of it happened to be an
  assertion of absence. Now a day wide, which keeps it on the raw table.
  Worth knowing before writing another test that asserts a sample exists.
* **One line of `ui/src/pages/monitoring/vocab.ts` is ours.** Adding
  `check_state` to the `Condition` union breaks two exhaustive switches
  there, so each gained a case. `CONDITION_KINDS` was deliberately **not**
  extended: that drives the rule editor's dropdown, and offering a condition
  with no form for its parameters would be worse than not offering it.

### What a check's history may claim (after the review findings)

* **Time nobody measured is `unknown`, never the last status.** One bound,
  `store::silence_ms` (2 × interval + timeout + 30 s: one run may be late or
  skipped, two may not), used in three places through `measured_until`:
  `record` closes a broken watch with a `→ unknown` row at the *last run*
  before it records the new one from `unknown`; `summarise` closes the tail
  the same way, since no row does; a repoint closes at the last run too. It
  is measured from the end of one run to the *start* of the next
  (`record` takes both times), because a probe can legitimately take much
  longer than its interval (ten redirects, sixteen echoes) and that is not a
  hole. A purely elapsed-time rule was chosen over explicit markers (a row
  at shutdown, at disable, …) because it is the only one that also covers a
  crash and a laptop lid.
* **Silence is counted from the last run or the last save, whichever is
  later** (`store::silent_since`), and `store::save` closes a hole that is
  already there. Counted from the last run alone, shortening a check's
  interval read as a hole nearly every time (an hourly check made a
  one-minute check 40 minutes after it ran: 40 minutes is more than a
  one-minute check may go unrun), wrote a false `not_measured` row and
  restarted `since`. Counting from the save is only sound if the save
  cannot vouch for silence before it, so `save` judges that stretch by the
  cadence the check had *until then* and, if it was already broken, writes
  the `→ unknown` row and leaves the state `unknown` until the next run.
  That is the one place a stored verdict is forgotten without a run or a
  repoint; the dates (`last_ok_at`, `last_down_at`) and the history keep
  what it was. A save that leaves the check switched off moves nothing, or
  a chain of edits to a disabled check would keep its watch "alive". No new
  stored field was needed: the definition's `updated_at` is the save.
* **A window's last stretch is closed by the next row, when there is one.**
  `summarise` first asks whether any change is on record after `until`; if
  so the status the window ends in lasted through it, exactly as between
  two rows inside the window. Only otherwise does the check's present state
  (`measured_until`) decide. Judged from the present state alone, a check
  that had been repointed (no `last_run_at`) answered `unknown` for every
  earlier window's tail, although its own rows said what it was doing.
* **The row after a gap is real, the timeline line is not always.** After a
  gap `previous` is `unknown` and `changed` is true even if the status is
  what it was — the history did change. The device's `event` log only gets a
  line when the status differs from what was *last heard*, or every start of
  the desktop application would write one line per check.
* **`summarise` reads the rows itself.** It used to be handed the list the
  caller was about to return, which is cut to `limit`; everything after the
  cut was charged to the last row that made it. It now streams every change
  in the window inside one visit to the store, and `events` returns the
  newest `limit` (the end of a flapping episode, not its first minutes) with
  `events_total` / `truncated`.
* **Pruning keeps one old row per check** — the newest before the cut-off,
  which is the status the retention opens on. `store::opening` also
  copes without it (the oldest row's `previous` + `held_s`, then the check's
  own `since`), because installations pruned by the earlier code have
  already lost it and a window can be longer than the retention.
* **A run is recorded against the definition it ran from, or not at all.**
  `record` re-reads the check inside its transaction and drops the result
  unless the stored definition is *equal* to the one the run was given
  (`updated_at` alone is not enough under a paused test clock, and equality
  is what is actually meant). `store::save` is one transaction for the same
  reason. `run_and_record` turns a dropped result into a `conflict`, which
  is how `checks.run_now` reports it. The scheduler also re-reads its list
  at the first tick after any `forget_check` (`Runtime::checks_changed`)
  instead of running a five-second-old copy once more.
* **`identity` includes the device** (and, for SNMP, not the host). Keeping
  the state across a move when the check names its own host was considered
  and dropped: the alert key, the series and the rule scope all change with
  the device, and "starts from `unknown`" is never wrong.

### One rule for "is this check live" (`checks::watched`)

Switched on, device monitored, monitoring on, checks on. The scheduler,
`checks.run_now`, the `monitor.status` counts and the history's tail all call
it; the alerts follow from the rules' side (`rules::stand_down`, called from
the `devices.changed` listener and from any evaluation that reaches an
unmonitored device). Decided deliberately:

* `checks.run_now` **skips** an unwatched check rather than running it
  without recording. A dry run would be a second kind of result in the
  contract, and "monitoring is off for this device" should also mean nothing
  is sent to it because an assistant asked.
* The state is **not** wiped when monitoring is switched off. What the check
  last found is a record with a date on it; only the derived views (counts,
  alerts, history tail) stop treating it as current.
* The two installation-wide switches stop `checks.run_now` as well. That
  goes one step beyond the finding (which was about the per-device switch)
  for the same reason: with `monitoring.enabled` off a run by hand raises an
  alert nothing looks at again until it is switched back on. If on-demand
  runs are wanted while the scheduler is off, it is one match arm in
  `rpc::run_now`.
* Only the **per-device** switch closes alerts. Turning `monitoring.enabled`
  or `checks_enabled` off stops the runs, takes every check out of the
  up/down counts and stops `run_now`, but leaves open alerts open: that
  switch is usually thrown for an hour, and resolving every alert in the
  installation (with notices) for it would be worse than leaving them.
* `monitor.poll_now` still polls an unmonitored device when asked; the rules
  just do not act on what it finds.

### Cadence is not the time of day

`Clock::monotonic_ms` beside `Clock::now_ms`. Every "is it due?" — device
polls, check runs, the inventory refresh, the CLI read, the hand-over to the
collected store, polls per minute, cycle time — is a difference of monotonic
readings; `DeviceRuntime::last_*_mono` and `CheckRuntime::last_run_mono` are
`Option`s so that no reading is a sentinel. The reasoning is in the comment
in `scheduler::scheduler`. Left on timestamps: a rule's `for_s` wait (it now
starts over if the time of day goes back during it, rather than lasting for
the step) and `rates` with `TimeBase::Wall`, i.e. a device that reports no
uptime of its own — a backwards step there is an unusable interval (a gap),
a forwards one stretches the interval and understates one rate. Not fixed.

### Alert lifecycle (after the review findings)

* **Every change to an alert is one store transaction** (`alerts::fire`,
  `resolve`, `amend`). Look-up and insert used to be two visits to the store,
  and polls, checks and `alerts.raise` all arrive on different threads: two
  callers found nothing open and both raised. There is deliberately no
  `alerts::save` any more — a bare save is how a copy read a moment ago
  overwrites what happened since (an acknowledgement losing a sighting, a
  sighting undoing an acknowledgement). Maintenance windows and the device
  are read *before* the transaction: they live in the same store behind the
  same non-reentrant lock.
* **A sighting is a measurement** (`AlertSpec::observed_at`,
  `Finding::observed_at`). A check rule judges a stored verdict, and that
  verdict is looked at after every poll and after every other check on the
  device has run. Counting each look made `count` meaningless (12 checks on a
  device → 12 per round) and published an event per look. Three comparisons
  against the alert's `last_seen_at` do all the work: an observation it has
  already seen is not counted; an all-clear older than its last sighting
  does not resolve it; a sighting older than the last resolve does not raise
  it again. The last two also close a real race — an evaluation that read
  `down` just before the run that cleared it used to re-raise the alert
  behind the resolve. For that to hold, a resolve is dated no earlier than
  the observation that cleared it: a poll's evaluation carries the time the
  poll set out, which can be before a sighting the alert already counted.
* **The firing notice is owed until it is paid** (`Alert::notified_at`), and
  **the resolved notice is paired with it**. Muting used to be frozen into
  the alert at the raise, so an outage that began inside a maintenance window
  and outlasted it was never announced at all. It is paid at the first
  *evaluation* that finds the alert unmuted, counted or not: an evaluation
  that looks again at a run it has already counted still sends the notice,
  or a check that runs hourly would announce an outage an hour after the
  window closed. The resolved notice follows
  the firing one *regardless* of windows and snoozes: the alternative leaves
  an incident open for ever in a chat or a pager, and "resolved" is not a
  message that wakes anybody. It is not sent for an alert that was never
  announced. `notify::send` therefore no longer looks at muting at all.
* **`alerts.changed` carries `notification`** (`firing` / `resolved` /
  `null`), because the desktop shell shows its own notification from this
  event and its old test (`count == 1`) cannot see a late announcement.
* **What a check found before its device came back is not known**
  (`DeviceRecord::up_since`). Stamped when the device is *declared* up, not
  when the poll set out. Deliberately not done: making the device's checks
  due at once on recovery. A box answers ping well before its services are
  up after a reboot, so that would trade the false alert for a real but
  equally useless one; the check's own next run speaks instead.
* **Alerts open across the upgrade** have no `notified_at`; `db::init`
  writes the answer the old code implies, once (see `BACKFILL_NOTIFIED_AT`).

* **The concurrency tests run several rounds, and count.** Two alerts under
  one key is the rare symptom of a raise that visits the store twice; a lost
  sighting is the common one. `checks_that_finish_together…` can only see
  the first (one run is one sighting) and, with the transaction taken out,
  passed 10 times in 10 on a loaded machine. `evaluations_of_one_device_
  that_overlap…` (poll rules, every evaluation a sighting) and the
  `alerts.raise` / `alerts.ack` tests failed every time, loaded or not.

Still left alone: `checks::run_and_record` has the rules look at *every*
check of the device after each run. It is harmless (the other checks'
verdicts are already seen), but it is `checks × rules` store reads per run
where one check's would do. A check that runs between a host dying and the
poller declaring the device down (the damping window) still raises its own
alert beside the device's. And a run whose probe was sent before the device
came back but recorded after `up_since` is still trusted: `record` now knows
when a run started, but `CheckState` does not keep it.

### Counters

* **A counter that goes down is only sometimes a wrap** (`rates::delta`).
  32-bit: always (they really do, every few minutes). 64-bit: only when the
  wrapped difference is at most 2^56, i.e. when the counter had actually
  reached the top — otherwise it was cleared on the device and the interval
  is a gap. Text off the CLI (`Width::Text`): never. The 64-bit rule keeps a
  genuine wrap working and is what `clear counters` / Junos `clear interfaces
  statistics` / a re-seated line card needed.
* **Counters remember which source left them** (`DeviceRuntime::
  counters_from`), and are dropped only when *that* source was asked again
  and failed. With SNMP on but dead, every poll between two CLI reads used
  to drop the CLI's counters, so the fallback never produced a rate. The
  same knowledge stops a CLI reading being re-filed in the collected store
  as `snmp` (it used to be, with most of its fields gone).

### Polling and alerts

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
