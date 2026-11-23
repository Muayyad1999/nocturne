# Feature: detections (`nocturne-detect`)

A short, fixed catalogue of things worth telling somebody about that no
single log line and no single metric shows — because each needs two sources
put side by side: a configuration that changed while no approved change was
running, an SSH host key that is not the one from yesterday, somebody
reading unredacted configurations again and again.

It is not a rule language and not a SIEM. Each detection is a few lines of
Rust with a stated source, a stated blind spot and a test, and the list is
meant to stay short enough to read. Conditions the other features already
judge are left to them: `interface_flapping`, `err_disabled`, `mac_flapping`
(insights) and everything a log rule can say by itself (`logs`).

Engines: core only. It links to no feature. It subscribes to events, asks
other features by method name, and copes when one is absent: a detection
whose source is missing is listed as unavailable with the reason, and stays
silent.

## What a detection does

* It only **tells**. A hit is appended to the `event` stream with its
  evidence, published as `detections.hit`, and raised through `alerts.raise`
  with dedup key `detect:<id>:<device or "-">:<key>` — into the one alert
  list, with the one set of notification channels, maintenance windows and
  snoozes. There is no second inbox. `<key>` is what tells two findings of
  one detection apart: `config`, `reload`, `<host>:<port>:<presented
  fingerprint>`, `views`, `credential_saved` / `credential_deleted` /
  `master_password`, `vault` / `server`, and the change id. The alert is
  raised as `{rule: "Detection: <name>", device_id?, severity, title, detail,
  dedup_key}`; `detail` is the sentence saying what it was decided from.
* It never changes a device, never drafts a change by itself, and never
  triggers a write. A person who wants to act on one opens the device.
* It says what it was decided from. Syslog and v1/v2c traps are
  unauthenticated, so a detection built on them carries that sentence in
  its evidence (`caution`) — unless the line behind it was handed over by
  the lab in process (`via: lab`) or read back over Nocturne's own session
  (`via: pull`; the log rules do not run over pulled lines today). Evidence
  is ids and counts, never the text of a log line, a configuration or
  anything that was typed — apart from the two words a device says about a
  configuration change, `by` and `from`, cut to 64 printable characters.
* Lab devices are devices: detections run for them too, and their hits say
  `lab: true`. One thing is the lab's own doing and is not reported: a lab
  that is started or reset boots its nodes, and a node given a new startup
  configuration is reloaded into it, so a simulated device's restart within
  a minute of such an entry for its lab in the audit trail (`labs.start`,
  `labs.reset`, `labs.node.config_set {lab_id}`) is explained. A fault
  injected into a lab, a node switched off and on, `reload` typed at a
  simulated console are not: that is somebody breaking a network to see
  what notices. Their alerts reach no notification channel — by the
  monitor's rule (below), not by anything passed from here: `alerts.raise`
  is called without `notify`, because `notify: []` would silence a lab
  alert even where `notify_for_lab` is switched on.
* A check that could not be made is not a check that found nothing. When a
  source fails, or an event is lost because the application was too busy to
  read it, nobody is accused and the catalogue entry carries `last_error`
  until the next check of that detection goes through. The same holds for a
  check that crashes — a panic in this crate, or in the method of another
  feature that it called: each event is judged in a task of its own, so the
  crash is recorded (`last_error` on every entry, since it is not known
  which one would have judged it) and the next event is judged as usual.
  Without that, one fault would silence every detection until a restart.

## The catalogue

| Id | Severity | Decided from | Says | Cannot see |
|---|---|---|---|---|
| `unplanned_config_change` | high | `logs.config_changed {device_id, by?, from?, entry_id, at}` and `config.changed {device_id, kind, version_id, previous_id, added, removed}` (with when each version was seen, from `config.versions`), each against `changes.list {device_id}` | "<by> changed <device> from <from> and no approved change was being applied" (only the parts the device said); from a backup, "<device>'s <kind> configuration changed between two backups and no approved change was being applied" | a change made on a device that sends no syslog and is not backed up before the next schedule; who, when the device does not log it; a change by hand made between the same two backups as an approved one — including one that the pipeline's own backup, taken as an approved change starts, is the first to see (that copy is stored without `config.changed`) |
| `unplanned_reload` | medium | `logs.signal` with `kind: "reload"`, against `changes.list {device_id}`, the core's jobs of a kind that can change a device (`changes.apply`, `changes.rollback`, `automation.ansible`) and, for a simulated device, the lab's own record of restarting it (audit entries `labs.start`, `labs.reset`, `labs.node.config_set` naming its lab) | "<device> restarted and nothing Nocturne was doing explains it" | a reload on a device that sends no syslog; why it restarted |
| `host_key_changed` | critical | `connect.host_key_changed {host, port, known, presented}` | "<device> presented a different SSH host key" with both fingerprints | a key that changed between two connections to a device nobody connects to; whether the device was re-keyed or the connection intercepted |
| `unredacted_config_views` | medium | the audit trail: `config.view_unredacted` entries | "<n> unredacted configuration views in <m> minutes" | who: there are no users (below); an unredacted export to disk, which is one `config.export` entry however many devices it covers |
| `secrets_changed` | info | the audit trail: `credential.save {change, secrets_added, secrets_replaced, secrets_removed}`, `credential.delete`, `vault.set_password {change}` | the secrets of a stored credential were changed (added, replaced or removed) or a credential was deleted; the vault's master password was set, changed or removed (`high` for removed). A new credential is not a hit | a save recorded before the audit entry said what it changed |
| `failed_unlocks` | high | the audit trail: `vault.wrong_password` entries, and `server.token_refused {bad_token}` entries | "<n> wrong vault passwords in <m> minutes", or "<n> requests to the server with a wrong access token in <m> minutes" — counted apart | an attacker who guesses right the first time; requests with no token at all (recorded, not counted: a scanner is not a guess) |
| `protected_change_outside_window` | info | `changes.changed` for a finished change with protected hits, against `maintenance.list` | "<change> touched protected resources on <device> outside any maintenance window" | a device whose change `failed`: the summary does not say whether anything was sent to it; when each device of a long change was done: the change is judged at the moment all of it finished |

Every source above was read from the code that publishes it, and each
check's module says which file. Where the first draft of this table assumed
something that is not so, it was corrected:

* `connect.host_key_changed` names a host and a port, not a device. The hit
  is filed under the inventory device with that address and SSH port when
  there is exactly one; with none or several it is filed under no device and
  the evidence lists the candidates. The core publishes the event on every
  connection attempt, so the same key from the same host is one hit per
  24 hours (remembered in memory: a restart can repeat it once).
* `credential.save` used to be written for a new credential and for a
  replaced one alike, with no detail, so the detection fired on both and
  every first device anybody added came with an alert. The entry now carries
  `{change: created | updated, secrets_added, secrets_replaced,
  secrets_removed}` (the names of the parts, never a value), and only an
  `updated` entry that names a secret is a hit. An entry without the detail
  is not guessed at.
* `vault.set_password` used to carry no detail; it now carries `{change:
  set | changed | removed | unchanged}` (below), which is what "`high` for
  removed" is decided from.
* A cleared audit trail is not a detection: `logs.clear` refuses the `audit`
  stream (`logs.md`).

"No approved change" has one definition, here (`planned.rs`). A change
explains something that happened to a device when it names that device and
either

* the pipeline is working on it now — the change is `applying`, or a job
  holds a change that has already run (a rollback in progress; a planning
  job also sets `job_id`, but only on a change that has not run); or
* it has run — `applied`, `rolled_back`, `failed` or `partially_applied`,
  every state in which the pipeline has had its hands on a device — and its
  `updated_at` (stamped on every save, so the time it reached that state) is
  within `grace_minutes` (15) either side of the event.

A device the change skipped or never reached (target state `skipped` or
`pending`) is not explained by it — once the change is over. While it is
still `applying` it explains every device it names, including ones it has
finished with or not yet reached: the summary carries no time per device,
so "the pipeline has left this one" cannot be told from "it is on it now",
and the definition leans towards not accusing. A device the change lost and left to its
dead-man switch (target state `guard_fired`) is explained for an hour
longer after the change, and no longer before it: the switch — a timed
reload, a commit that undoes itself — fires up to its minutes after the
change has been written up as failed, a device that reloads takes minutes
more to say so, and the summary does not carry the minutes (the default is
ten). Without this the pipeline's own safety net is reported as an
unplanned restart.

The time of a syslog line is the time it was received, so the window needs
that slack. For a backup the "event" is not a moment: `config.changed` says
when the difference was noticed, and it was made after the earlier version
was last fetched unchanged and before the later one was first fetched, so a
change anywhere in that stretch (plus the grace either side) explains it.
Otherwise every approved change would be reported by the scheduled backup
that follows it — the pipeline keeps its own post-change copy of the running
configuration, but not of the startup one.

The stretch starts at the earlier version's `last_seen`, not at the time it
was first stored. The two are asked of the config feature (`config.versions
{device_id, kind, before_id, limit: 1}` → `first_seen`, `last_seen`), which
is the only place `last_seen` is kept. A version can be a device's
configuration for months and was very often stored by the approved change
that made it; measured from when it was first stored, that old change would
explain whatever is done to the device by hand afterwards, and the
detection would never fire on a device the pipeline has touched. When
either version is no longer stored (or the device has left the inventory)
the stretch cannot be bounded, and the event is not judged (and says so,
below) rather than judged at the moment of the backup. A `config.changed`
that itself counts no line added and none removed is not reported: the
bytes differed, the configuration did not.

"No approved change" is never concluded from a change engine that is absent
(the detection is unavailable) or that failed to answer (the check is
recorded as not made, `last_error`). The handover report and the
`config_changed_outside_nocturne` insight keep their own wording but are to
converge on this setting.

One edit can be seen more than once — the device says so in its log, the
next backup of the running configuration differs, and so does the startup
one after a save — and each sighting is a hit with its own evidence. They
share a dedup key, so however often an edit is seen it is one alert with a
count. The same log line received twice is not two sightings: a line with
the same text from the same device within ten seconds of the one last
reported (a duplicated datagram, a device that logs to two addresses which
both end here) is folded into it. The text is compared by a fingerprint
kept in memory and is stored nowhere. The two detections that count (`unredacted_config_views`,
`failed_unlocks`) start again from nothing after a hit: the sixth view after
a hit on the fifth is the first of the next five.

**There are no users.** Nocturne has one operator identity per instance, so
"who" in a detection is whatever the device said (`by`, `from`), never a
Nocturne account, and there is no per-person baseline. This is stated on the
screen, not hidden.

Deliberately absent: "a new MAC address on a port". It needs a baseline of
the MAC table and is noise without 802.1X context; if it is ever built it is
a scan check in insights, not a reaction to a log line.

## RPC (contract)

Settings section `detect`: `{ enabled: true, grace_minutes: 15,
unredacted_views: {count: 5, window_min: 10}, failed_unlocks: {count: 5,
window_min: 5}, disabled: [id] }`. Read with values that would make a
detection meaningless brought into range: a count of at least 1, a window of
1 to 1440 minutes, a grace of 0 to 1440. It is read key by key:
`settings.set` stores whatever it is given, and a value of the wrong type
takes that key's default without touching the others — one mistyped number
must not switch back on what was switched off.

| Method | Params → Result |
|---|---|
| `detections.list` | → `[{id, name, description, severity, decided_from, cannot_see, enabled, available, unavailable_reason?, last_hit_at?, hits_24h, last_error?: {at, message}}]`, in catalogue order |
| `detections.set_enabled` | `{id, enabled}` → the entry; audited (`detections.set_enabled {id, enabled}`) when it changes something. `invalid` for an id that is not in the catalogue; `conflict` for switching one on while `enabled` is off as a whole (the entry would still read `enabled: false`) |
| `detections.recent` | `{id?, device_id?, since?, limit?: 100}` → `[Hit]` newest first; `limit` 1–1000; `invalid` for an unknown `id` |

```rust
struct Hit { id, detection, at, device_id?, severity, title,
    evidence: Value,          // the records it was decided from, by id: log entry, change, audit entry
    alert_id?,                // absent when alerts.raise is not in the build, or refused
    lab: bool }
```

`enabled` is the switch (`detect.enabled` and not in `disabled`);
`available` is whether this build can run it. They are independent: a
detection can be switched on and unavailable. `severity` in the list is the
usual one; a hit carries its own (`secrets_changed` is `high` for a removed
master password). `at` is when what it was decided from happened, not when
it was judged. `last_hit_at` survives the hit ageing out of the `event`
stream; `hits_24h` is counted from the stream, up to 100,000.

Availability is decided from the method table, once: `changes.list`,
`maintenance.list` and `alerts.raise` by name, the logs feature by
`logs.rules.list` (the rules are what publish `logs.signal` and
`logs.config_changed`) and the config feature by `config.backup` (the backup
is what publishes `config.changed`, and the same feature writes
`config.view_unredacted`). `unplanned_config_change` needs `changes.list`
and at least one of the two that report a change (a backup that differs is
judged with `config.versions`, which the same feature registers; were it
missing, the check is recorded as not made); `unplanned_reload` needs
the logs feature and `changes.list`; `unredacted_config_views` the config
feature; `protected_change_outside_window` `changes.list` and
`maintenance.list`. The other three need only the core. `alerts.raise` is
where a hit goes, never what it is decided from: no detection is unavailable
for lack of it.

Evidence, by detection (fields with nothing to say are left out):

| Detection | `evidence` |
|---|---|
| `unplanned_config_change` (log) | `{decided_from: "log", log_entry_id, stream?, via?, by?, from?, changes_checked, nearest_change?: {change_id, state, updated_at}, grace_minutes, caution?}` |
| `unplanned_config_change` (backup) | `{decided_from: "backup", kind, version_id, previous_id, between: {from, to}, lines_added, lines_removed, changes_checked, nearest_change?, grace_minutes}` — `from` is when the earlier version was last fetched unchanged, `to` when the later one was first fetched |
| `unplanned_reload` | `{decided_from: "log", log_entry_id, rule_id, stream?, via?, changes_checked, nearest_change?, jobs_checked, grace_minutes, caution?}` |
| `host_key_changed` | `{decided_from: "connect.host_key_changed", host, port, known: {algorithm, fingerprint, first_seen}, presented: {algorithm, fingerprint}, device_ids: [..], host_key_policy, connection: refused \| went_ahead}` |
| `unredacted_config_views` | `{decided_from: "audit", action, count, window_min, audit_entry_ids: [..50], through_audit_id, device_ids: [..], who}` |
| `secrets_changed` | `{decided_from: "audit", action, audit_entry_id, who}` |
| `failed_unlocks` | `{decided_from: "audit", what: vault \| server, action, count, window_min, audit_entry_ids: [..50], through_audit_id, from?: [address]}` |
| `protected_change_outside_window` | `{decided_from: "changes.changed", change_id, state, finished_at, devices: [{device_id, protected_hits}], devices_no_longer_in_inventory: [..], windows_checked, grace_minutes}` |

With `host_key_policy: accept` the core announces a changed key and then
connects anyway; the hit says so in its title and `connection: went_ahead`.

`protected_change_outside_window` judges a change once it has finished
(`applied`, `partially_applied`, `rolled_back`, `failed`, and no job on it),
for the devices the pipeline sent something to (target state `applied`,
`rolled_back`, `rollback_failed`, `guard_fired`) that have protected hits. A
window counts when its scope covers the device — the monitor's rule for
windows, restated here because the crates do not link — and it was open when
the change finished or closed no more than `grace_minutes` before. A later
rollback by hand is judged again, under the same key.

Events: `detections.hit {hit}`; `detections.changed {id?}` when the
catalogue's state changes — a switch moved (through `detections.set_enabled`
or the settings, once either way), or a `last_error` appeared, changed or
cleared. A check that keeps failing the same way, and a flood of events
that could not all be judged, are announced once, not once an event.
Without `id`, refetch the list.

## What it needs from the rest (and adds to them)

* `nocturne-monitor`: `alerts.raise` gains `notify?: [channel_id]` (absent =
  the usual routing, `[]` = nobody). `alerts.clear {dedup_key, note?}` →
  `{cleared}` resolves the open alert with that key and records that the
  condition cleared, not that a person resolved it. An alert for a lab
  device is recorded and shown and **sent to no channel** unless the monitor
  setting `notify_for_lab` is switched on. `monitor.ifindex {device_id}` →
  `{at?, interfaces: [{index, name}]}`, the index map of the last SNMP poll,
  kept across restarts.
* The core (`nocturne-app/src/system.rs`): a wrong vault password is
  audited as `vault.wrong_password {method: "vault.unlock" |
  "vault.set_password"}`, never with the password that was tried. A locked
  vault refuses `vault.set_password` before looking at the current password,
  so `vault.unlock` is the one place a locked vault tests a guess — and that
  place is audited. `vault.set_password` carries `{change: set | changed |
  removed | unchanged}`.
* The server (`nocturne-server/src/auth.rs`): requests refused for a wrong
  or missing token are counted, and each minute that had any becomes one
  audit entry, `server.token_refused {count, bad_token, missing_token, from:
  [up to 8 addresses], more_sources, window_s: 60}` — so the audit trail
  cannot be flooded through the front door, and the entry never contains
  the token that was tried. The request path takes a lock and adds one; the
  entry is written by a task when the minute is over, or at once when the
  server stops. Requests refused after the stop has begun (what the open
  connections had already sent) are gathered for 250 ms at a time rather
  than written one by one. A detection therefore hears of a guess up to a
  minute late. Measured in a debug build: 10,000 requests with a wrong
  token take about 0.3 s in all (30 µs each, against 20 µs for an accepted
  request to the same route) and leave one entry.

## Tests

Each detection through `App::rpc` and real events, with fakes standing in
for the other features: it fires on its evidence and names the records; it
stays silent inside the grace window and when a change explains the event
(and does not when the change that explains it is only the one that made
the earlier backup, a month before); the same line twice is one hit; a
read-only automation run explains no restart, a dead-man switch and a lab
reset do; a flood that outruns the judge is announced once; a setting of
the wrong type switches nothing back on;
it reports itself unavailable, with the reason, when its source method is
absent; a source that fails is a recorded failed check and accuses nobody;
a hit lands in the `event` stream; `alerts.raise` being absent loses nothing
but the alert. A lab device's hit is labelled `lab` and raised for that
device with no `notify` of its own. The core and the server test their own
halves: a wrong master password is audited and what was tried is not; a
flood of refused requests is one entry a minute.

End to end in the suite, once this crate is registered there: a lab switch
whose configuration is changed from its console, with no change in the
pipeline, raises `unplanned_config_change`; and its alert notifies nobody.
Neither can be shown in this crate, which has no log feature, no lab and no
monitor to show it with.
