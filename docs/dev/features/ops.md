# Feature: operations record (`nocturne-ops`)

The paperwork of a NOC, generated from what actually happened: the overview
a shift starts with, incidents with a timeline, the handover note at the end
of a shift, reports for the people who ask for reports, runbook notes, and a
global search over all of it.

Everything here is a view over real records (alerts, changes, jobs, audit,
backups, samples). Where another feature is not present in the build or has
no data, a section says "not available" / "no data yet" — it never invents
a number.

Engines: core only. Other features are reached by RPC and must be optional:
call `app.methods()` once and treat missing methods as "feature not
installed".

## Overview (contract)

`overview.get {}` → one document for the home screen, each part
independently `null` when its source is unavailable:

```rust
{ devices: { total, up, down, unknown, lab, by_site: [{site, total, down}] },
  alerts: { open, by_severity: {critical, high, medium, low, info}, newest: [..5] }?,       // alerts.list
  changes: { awaiting_approval, applying, applied_24h, failed_24h, recent: [..5] }?,        // changes.list
  backups: { devices_with_backup, devices_without, changed_24h, oldest_backup_at }?,        // config.devices
  compliance: { score_pct, failing_devices, run_at }?,                                      // compliance.results
  jobs: { running: [..], failed_24h },
  activity: [ {ts, kind, text, device_id?, ref?} ..20 ],                                    // merged audit + alerts + changes, newest first
  insights: { open, top: [..5] }?,                                                           // insights.list
  generated_at }
```

Cheap enough to call every 30 s; cache internally for 5 s.

## Incidents (contract)

```rust
struct Incident { id, title, severity: sev1|sev2|sev3|sev4, state: open|mitigated|resolved,
    started_at, mitigated_at?, resolved_at?, device_ids, alert_ids, change_ids,
    summary, impact, root_cause, follow_ups: [{text, done}],
    timeline: [Entry { id, ts, kind: note|alert|change|command|status|log, text, ref?: {type, id}, author }],
    created_at, updated_at }
```

| Method | Params → Result |
|---|---|
| `incidents.list` | `{state?, limit?}` |
| `incidents.get` / `.create` / `.update` / `.delete` | CRUD; state changes stamp the times and add a `status` timeline entry |
| `incidents.note` | `{id, text, ts?}` → adds a timeline note |
| `incidents.link` | `{id, alert_ids?, change_ids?, device_ids?}` → links and pulls the linked items' own events into the timeline (alert fired/acked/resolved, change applied/rolled back) at their real timestamps |
| `incidents.suggest_links` | `{id}` → alerts, changes and config changes on the incident's devices inside its time window that are not linked yet — "this switch's config changed 4 minutes before the outage" is the single most useful thing an incident tool can say |
| `incidents.export` | `{id, format: markdown}` → a post-incident review document (summary, impact, timeline table, root cause, follow-ups) |

Event: `incidents.changed`.

## Shift handover (contract)

`handover.generate {since, until?: now}` → a structured summary of the
period, every item a real record with its id:

```rust
{ period: {since, until},
  alerts: { fired, resolved, still_open: [..] }?,
  incidents: { opened: [..], resolved: [..], open: [..] },
  changes: { applied: [..], failed_or_rolled_back: [..], awaiting_approval: [..] }?,
  config_changes: [ {device, at, lines_added, lines_removed} ]?,          // changes nobody made through Nocturne are the interesting ones: mark `unplanned` when no applied change covers the device at that time
  devices: { added: [..], removed: [..], went_down: [..], unreachable_now: [..] },
  backups: { failed: [..] }?,
  jobs_failed: [..],
  notes: "" }
```

`handover.save {handover, notes}` / `handover.list` / `handover.get` /
`handover.export {id, format: markdown}`. The assistant may be asked to
polish the prose of `notes`; the data sections are never model-written.

## Reports (contract)

`reports.kinds` → what is available in this build; `reports.generate
{kind, since?, until?, filter?, format: markdown|csv|html|json}` → `{title,
content, generated_at, rows}`; `reports.save` / `.list` / `.get` /
`.delete` for keeping a generated report. Kinds:

* `inventory` — devices with site, role, type, model, serial, OS version,
  uptime (from collected facts; blank where never collected, with a count of
  such devices in the header).
* `software` — OS versions per model, and devices that differ from the most
  common version of their model.
* `availability` — per device over the period from `icmp` reachability
  samples: % up, outages (count, longest), with the sampling coverage stated;
  devices with < 50 % coverage are listed separately, not given a percentage.
* `changes` — applied / failed / rolled back, by origin, with tickets.
* `backups` — coverage, age of newest backup, devices never backed up.
* `compliance` — latest run by policy and by device.
* `alerts` — counts by rule/device/severity, mean time to acknowledge and
  resolve.

HTML output is self-contained (inline CSS, no scripts), prints well, and
carries the generation time and the filter in its header.

## Notes / runbooks (contract)

Markdown notes attachable to devices, sites or tags: `notes.list
{device_id?, site?, tag?, query?}`, `notes.get/save/delete`, with
`[[device name]]` links resolved to device ids on save. These are what the
assistant searches when asked "how do we fail over the WAN link?".

## Global search (contract)

`search.query {q, limit?: 8 per kind}` → `{devices, notes, incidents,
changes, config: [..]?, addresses: [..]?, logs: [..]?}` — fans out to
`config.search`, `ipam.search`, `logs.query` when present (each with a
300 ms budget; a slow or failing source is reported in `partial: [..]`,
not waited for). A query that looks like an IP or MAC address goes to
`locate.host` when present and comes back as `location`.

## Tests

Each view against an app with stub RPC methods registered in the test
(returning fixed, realistic payloads) **and** with those methods absent
(→ `null` sections, never zeros); incident lifecycle and timeline ordering
with linked items; `suggest_links` window logic; handover period boundaries
and `unplanned` detection; every report kind in every format (CSV escaping,
HTML escaping of hostile device names/descriptions — `<script>` in an
interface description must come out inert), availability maths on hand-made
sample series incl. gaps; notes links; search fan-out with a slow and a
failing source.
