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
  snoozes. There is no second inbox.
* It never changes a device, never drafts a change by itself, and never
  triggers a write. A person who wants to act on one opens the device.
* It says what it was decided from. Syslog and v1/v2c traps are
  unauthenticated, so a detection built on them carries that sentence in
  its evidence.
* Lab devices are devices: detections run for them too. Their alerts reach
  no notification channel (the monitor's rule, below).

## The catalogue

| Id | Severity | Decided from | Says | Cannot see |
|---|---|---|---|---|
| `unplanned_config_change` | high | `logs.config_changed {device_id, by?, from?}` and the config feature's "a new version differs" event, against `changes.list {device_id}` | "<by> changed <device> from <from> and no approved change was being applied" | a change made on a device that sends no syslog and is not backed up before the next schedule; who, when the device does not log it |
| `unplanned_reload` | medium | the `Reload` log signal against `changes.list` and running jobs | "<device> restarted and nothing Nocturne was doing explains it" | a reload on a device that sends no syslog |
| `host_key_changed` | critical | `connect.host_key_changed` | "<device> presented a different SSH host key" with both fingerprints | a key that changed between two connections to a device nobody connects to |
| `unredacted_config_views` | medium | the audit trail | "<n> unredacted configuration views in <m> minutes" | who: there are no users (below) |
| `secrets_changed` | info | the audit trail | a credential was replaced or deleted; the vault's master password was set, changed or removed (`high` for removed) | — |
| `failed_unlocks` | high | the audit trail | repeated wrong vault passwords, or repeated requests to the server with a bad token | an attacker who guesses right the first time |
| `protected_change_outside_window` | info | a change applied with protected hits, against `maintenance.list` | "<change> touched protected resources outside any maintenance window" | — |

"No approved change" has one definition, here: no change for that device was
`applying`, or was `applied` or `rolled_back`, within `grace_minutes` (15)
either side of the event. The time of a syslog line is the time it was
received, so the window needs that slack. The handover report and the
`config_changed_outside_nocturne` insight keep their own wording but are to
converge on this setting.

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
window_min: 5}, disabled: [id] }`.

| Method | Params → Result |
|---|---|
| `detections.list` | → `[{id, name, description, severity, decided_from, cannot_see, enabled, available, unavailable_reason?, last_hit_at?, hits_24h}]` |
| `detections.set_enabled` | `{id, enabled}` → the entry; audited |
| `detections.recent` | `{id?, device_id?, since?, limit?: 100}` → `[Hit]` newest first |

```rust
struct Hit { id, detection, at, device_id?, severity, title,
    evidence: Value,          // the records it was decided from, by id: log entry, change, audit entry
    alert_id?,                // absent when alerts.raise is not in the build
    lab: bool }
```

Event: `detections.hit {hit}`; `detections.changed` when the catalogue's
state changes.

## What it needs from the rest (and adds to them)

* `nocturne-monitor`: `alerts.raise` gains `notify?: [channel_id]` (absent =
  the usual routing, `[]` = nobody). `alerts.clear {dedup_key, note?}` →
  `{cleared}` resolves the open alert with that key and records that the
  condition cleared, not that a person resolved it. An alert for a lab
  device is recorded and shown and **sent to no channel** unless the monitor
  setting `notify_for_lab` is switched on. `monitor.ifindex {device_id}` →
  `{at?, interfaces: [{index, name}]}`, the index map of the last SNMP poll,
  kept across restarts.
* The core and the server: a wrong vault password and a request with a bad
  or missing token are audited (they were recorded nowhere). Bad-token
  requests are folded into one audit entry per minute with a count, so the
  audit trail cannot be flooded through the front door, and the entry never
  contains the token that was tried.

## Tests

Each detection through `App::rpc` and real events, with fakes standing in
for the other features: it fires on its evidence and names the records; it
stays silent inside the grace window and when a change explains the event;
it reports itself unavailable, with the reason, when its source method is
absent; a hit lands in the `event` stream; `alerts.raise` being absent
loses nothing but the alert. A lab device's hit raises an alert that
notifies nobody. End to end in the suite: a lab switch whose configuration
is changed from its console, with no change in the pipeline, raises
`unplanned_config_change`.
