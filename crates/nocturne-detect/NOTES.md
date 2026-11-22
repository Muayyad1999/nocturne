# nocturne-detect — working notes

Spec: `docs/dev/features/detect.md` (conventions: `docs/dev/features/README.md`).

Build / test, always scoped to this crate:

```bash
cargo test -p nocturne-detect
cargo clippy -p nocturne-detect --all-targets -- -D warnings
cargo fmt -p nocturne-detect -- --check
```

## Status

Built: the seven detections, the `detect` settings section,
`detections.list` / `.set_enabled` / `.recent`, the events `detections.hit`
and `detections.changed`. Not registered in `nocturne-suite` by this crate's
author; the suite's end-to-end test (a lab switch changed from its console)
is still to be written there.

```
src/catalogue.rs        the seven entries, and why one cannot run in a build
src/sources.rs          the other features by method name: data | absent | failed
src/planned.rs          the one definition of "no approved change"
src/checks/*            one module per source: config_change, reload, host_key, audit, protected
src/hits.rs             a finding → alerts.raise → the `event` stream → detections.hit; reading hits back
src/watch.rs            listener (filters the bus) and judge (one event at a time, in order)
src/rpc.rs              the three methods and `detections.changed`
src/tests/*             behaviour tests through App::rpc, with fakes for the other features
```

## Decisions

* **Two tasks, one judge.** The listener only filters topics, so it does not
  fall behind a busy bus; the judge takes matched events one at a time. In
  order matters: the counting detections read the hits already stored to
  know which audit entries a hit has used up, and two judges could both
  count the same five.
* **A hit is one sighting.** An edit seen in the log and again by the backup
  is two hits and one alert (same dedup key). Only an identical finding is
  folded before it becomes a hit: the same host key on every reconnect (24 h,
  in memory), and a change announced twice in the same state.
* **A backup is judged over a stretch, a log line at a moment.** See
  `planned::Span`. Judging `config.changed` at the moment it is published
  reports every approved change that saved to startup, at the next scheduled
  backup: the pipeline keeps a post-change copy of the running configuration
  only (`nocturne-changes/src/apply.rs`, `store_after`).
* **The stretch starts where the earlier version was last seen.** Not where
  it was first stored: the review found that with `taken_at` as the start,
  the change that produced a version (whose post-change copy *is* that
  version, stored seconds before the change is saved as applied) explained
  every later edit by hand, so the detection never fired from a backup on a
  device the pipeline had touched. `last_seen` lives in the config feature's
  own table, so it is asked for by name (`config.versions`).
* **A dead-man switch is the pipeline's doing.** A target left in
  `guard_fired` reloads (or undoes its commit) up to the switch's minutes
  after the change is written up as failed. `planned.rs` gives such a
  device an hour beyond the grace, after the change only.
* **Only jobs that can write explain a restart.** `changes.apply`,
  `changes.rollback`, `automation.ansible`. The automation feature's command
  runs and playbooks are read-only (`automation.runner`,
  `automation.playbook`) and scheduled across every device; counted, they
  would explain every restart.
* **A lab's own restarts are the lab's.** `labs.reset` and
  `labs.node.config_set` boot simulated nodes, which log `SYS-5-RESTART`
  like hardware (`nocturne-netsim/src/sim/edit.rs`, `boot`). The audit entry
  names the lab, and the device knows its lab, so the restart is explained
  within a minute of it. `labs.start` is listed too although the simulator
  discards boot events when a lab is built (`Lab::from_spec`), so that it
  stays right if that changes. Faults and power switches are left to fire.
* **The same line twice is one sighting.** Folded by a fingerprint of the
  line as received (`body.raw`), per detection and device, for ten seconds.
  Nothing of the text is kept.
* **`failed` runs, but does not touch.** For "does a change explain this" a
  `failed` change counts (leaning towards not accusing). For "did it touch
  protected resources" a `failed` target does not (leaning towards not
  claiming something that may not have happened). The change summary cannot
  tell "never connected" from "session lost half-way".
* **No `notify` on `alerts.raise`**, including for lab devices: the monitor
  owns that rule, and `notify: []` would override `notify_for_lab`.
* **Availability is read from the method table.** An event has no
  registration to look for, so a feature is recognised by a method it always
  registers (`logs.rules.list`, `config.backup`). If one of those is ever
  renamed, the detections that lean on it turn unavailable — loudly, with the
  name in the reason — rather than silently never firing.
* **Maintenance scope and recurrence are restated**, not linked
  (`checks/protected.rs`). A recurrence other than `weekly` is a failed
  check, not a one-off window.

## Known limits (also in the spec)

* An event the bus dropped (a lagging receiver) is not recovered. Every
  detection then carries `last_error` until its next check goes through; the
  audit-trail detections catch up by themselves, because each check re-reads
  the window from the store, but a lost `logs.config_changed` is lost.
* The repeat memory for host keys and judged changes is in memory, capped at
  4,096 entries: a restart can repeat a host-key hit once.
* `hits_24h` reads the `event` stream through `Store::log_query` (substring
  filter, exact source checked here, a page at a time) and stops at 100,000
  rows. A dedicated index on `source` would make it exact at any volume; it
  did not seem worth a table of this crate's own. Measured by the review
  (debug build, in-memory database): `detections.list` takes 24 ms with
  100,000 other events in the last day and no hits, 120 ms with 2,000 hits
  as well, 1.8 s with 22,000. Only a forged flood of log lines makes that
  many, and the log feature stores two rows a line itself before this
  feature adds a third.
* A panic while judging an event is caught per event (`watch::judge`) and
  shows as `last_error` on every entry; which detection it was is in the
  application log, not in the list.
* A lab node switched off and on from the lab screen (`labs.node.update`
  with `powered`) is reported by `unplanned_reload`, on purpose. The other
  way round, giving one node a new startup configuration explains a restart
  of *any* node of that lab for a minute: the audit entry names the lab,
  not the node.
* `protected_change_outside_window` judges every device of a change at the
  moment the whole change finished (`updated_at`): the summary carries no
  per-device time. A device done inside a window early in a change that ran
  on past the window plus the grace is reported.
* The alert of a lab device reaches no channel only once the monitor has
  its lab rule (`notify_for_lab`, see the spec). At the time of the review
  `alerts.raise` in the tree notified for lab devices like for any other.
* A terminal session is the human's own hands and is outside the pipeline by
  definition: a change typed there is reported. Nothing in the evidence says
  "a Nocturne terminal was open on this device at the time"; `terminal.open`
  is audited and could be added as context.

## How it was checked

99 tests: the pure parts by themselves (`planned`, the window and scope
arithmetic, the catalogue's availability table, the settings' ranges) and
every detection through `App::rpc` and published events. `src/tests/wire.rs`
pins the field names of every method and event (`-- --nocapture` prints one
real example of each).

Broken on purpose, one at a time, to see a test fail — by the builder
(twelve) and again by the reviewer (twenty-eight, listed in the review's
report): a backup judged from where the earlier version was first stored;
`applying` no longer explaining; the grace ignored; a failed `changes.list`
read as "no changes"; a read-only automation run explaining a restart; a
lab reset not explaining one; the dead-man switch having no reach; the same
line twice being two hits; every lost event announced; one unreadable
setting resetting the section; a backup with no line changed reported; the
same host key reported on every reconnect; a reported run of audit entries
counted again; requests with no token counted as guesses; a window's scope
ignored; a disabled detection still running; an unavailable one still
running; a removed master password being only information; a hit not
stored; a lab hit not labelled; a lab line carrying the caution;
`set_enabled` not audited; a change still rolling back judged; a failed
target counted as touched; a direction override surviving into a title;
the dedup key losing its device; a check that panics taking the judge down;
a backup judged without `config.versions` to ask.

Not checked here, because it cannot be: anything against the real log,
config, change, lab and monitor features. Their events and results were
read from their source and the fakes copy those shapes; nothing ran against
them.

## Test hook

`watch::SYNC` / `SYNCED` exist only under `cfg(test)`: a marker the tests
send down the same path as every event, so "stayed silent" can be told from
"has not been judged yet" without sleeping.
