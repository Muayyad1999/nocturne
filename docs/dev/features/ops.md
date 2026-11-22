# Feature: operations record (`nocturne-ops`)

The paperwork of a NOC, generated from what actually happened: the overview
a shift starts with, incidents with a timeline, the handover note at the end
of a shift, reports for the people who ask for reports, runbook notes,
evidence — what a device printed, kept with its source — and a global search
over all of it.

Everything here is a view over real records (alerts, changes, jobs, audit,
backups, samples). Where another feature is not present in the build or has
no data, a section says "not available" / "no data yet" — it never invents
a number.

Engines: `nocturne-ai`, for the redactor every piece of evidence passes
before it is stored (see Evidence). Other features are reached by RPC and
must be optional: call `app.methods()` once and treat missing methods as
"feature not installed".

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
    timeline: [Entry { id, ts, kind: note|alert|change|command|status|log|gap|evidence, text, ref?: {type, id}, author,
                       source: {from, evidence, gathered_at} }],
    created_at, updated_at }
```

`gap` is a source that could not be read, at the time of the attempt.
`evidence` is an attached evidence item, at the time it was captured: `id`
is `evidence:<evidence id>`, `text` is the item's title, `ref` is `{type:
"evidence", id}` — a reference, never the item's text. An `evidence` entry
without a `ref` is a tombstone: the item was deleted, and `text` says when
(see Evidence).

| Method | Params → Result |
|---|---|
| `incidents.list` | `{state?, limit?}` |
| `incidents.get` / `.create` / `.update` / `.delete` | CRUD; state changes stamp the times and add a `status` timeline entry |
| `incidents.note` | `{id, text, ts?}` → adds a timeline note. At most 64 KiB (see "Free text is capped") |
| `incidents.link` | `{id, alert_ids?, change_ids?, device_ids?}` → links and pulls the linked items' own events into the timeline (alert fired/acked/resolved, change applied/rolled back) at their real timestamps |
| `incidents.suggest_links` | `{id}` → alerts, changes and config changes on the incident's devices inside its time window that are not linked yet — "this switch's config changed 4 minutes before the outage" is the single most useful thing an incident tool can say |
| `incidents.export` | `{id, format: markdown\|csv\|html\|json}` → a post-incident review document (summary, impact, timeline table, root cause, follow-ups) ending in **Appendix: evidence**: every attached item with its id, kind, capture time, source, session, device, lab label, line count, size and SHA-256, and its content — text preformatted and escaped for the format, a drawing as an `<img>` with a `data:` address in HTML (described, not embedded, elsewhere), a chart as one row per series. In CSV a title, a fact or a paragraph that a spreadsheet would take for a formula is quoted out, like a line of output. At most 4 MiB of evidence goes into one document; an item past that is listed with its facts and a statement that its content was left out |

Event: `incidents.changed`.

Every change to a stored incident (`note`, `update` and the verbs built on
it, `link`, `sync`, `acknowledge`, and `evidence.attach` / `.detach` /
`.rename` / `.delete`) reads the incident and writes it back inside one
transaction. `update`, `link` and `sync` ask other features what happened
*before* that, on a copy, and then apply their change to the incident as it
is by then: a note added while a link is gathering its events is not
overwritten by a stale copy.

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
  evidence: { captured: [Evidence without body ..] },                     // what was captured in the period, oldest first: real records with ids and hashes, never their content
  notes: "" }
```

`handover.save {handover, notes}` / `handover.list` / `handover.get` /
`handover.export {id, format: markdown}`. The assistant may be asked to
polish the prose of `notes`; the data sections are never model-written.
`notes` is at most 64 KiB, in `handover.save` and in `handover.export`.

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

The interface holds it to that. An HTML document is previewed in a frame
with scripting off and an origin of its own, and printed
(`ui/src/pages/ops/PrintFrame.tsx`) from a second frame that also has an
origin of its own and may load nothing at all: a stylesheet, image or font
that is not inline or a `data:` address would be missing on paper, and a
pragma (`<meta http-equiv>`) is dropped. Neither frame is ever given
`allow-same-origin`: the document is made from device data.

## Notes / runbooks (contract)

Markdown notes attachable to devices, sites or tags: `notes.list
{device_id?, site?, tag?, query?}`, `notes.get/save/delete`, with
`[[device name]]` links resolved to device ids on save. These are what the
assistant searches when asked "how do we fail over the WAN link?". A note's
body is at most 64 KiB.

## Free text is capped (contract)

`incidents.note`, a note's body and a handover's notes — and an incident's
`summary`, `impact`, `root_cause` and the `note` of a state change, which
are the same kind of field — hold at most 64 KiB (65,536 bytes). They are
stored whole inside their record, copied into its search text and never
redacted; the only thing that makes one that long is pasted device output.
Past the limit the call is refused with `invalid`, a message that points at
`evidence.capture_terminal` and `evidence.add_text`, and `details: {limit:
"prose_bytes", max: 65536, actual, instead: [..]}`. Nothing is truncated.

## Evidence (contract)

The contract is `docs/dev/spec-capture.md`, section "Evidence": the
`Evidence` type, the methods, the rules (redacted always, provenance
recorded by the engine, stated limits, absent features). This section is
what that one leaves to the implementation, and it is contract too.

| Method | Params → Result |
|---|---|
| `evidence.capture_terminal` | `{session_id, lines?: 200, title?}` → `Evidence` with `body`. `lines` is 1 to 10,000 |
| `evidence.capture_log` | `{name, offset?: 0, limit?: 262144, title?}` → `Evidence` with `body`. `limit` is at most 262,144 bytes; `source.note` says which bytes of the log were read |
| `evidence.capture_topology` | `{view?, layer?, filter?, include_endpoints?, title?}` → `Evidence` with `body` |
| `evidence.capture_chart` | `{series: [{device_id, metric, instance?}], since, until, step_s?, title?}` → `Evidence` with `body`. Every device must be in the inventory |
| `evidence.add_text` | `{text, title, session_id?, device_id?, note?}` → `Evidence` with `body`; `source.from` is `operator`. `session_id` is recorded whether or not the session can still be found, so it has to be shaped like an id (at most 64 letters, digits, `-`, `_`, `.`, `:`) or the call is `invalid` |
| `evidence.list` | `{kind?, device_id?, incident_id?, query?, limit?: 50, cursor?}` → `{items: [Evidence without body], next_cursor?}`, newest first, at most 200 a page. `query` is matched against the title, the device, the session and the note, not the content |
| `evidence.get` | `{id}` → `Evidence` with `body` |
| `evidence.rename` | `{id, title}` → `Evidence` without body. The timeline entries that point at the item follow |
| `evidence.attach` / `evidence.detach` | `{id, incident_id, author?}` → `Evidence` without body, with its `incident_ids` as they now are. Attaching twice is the same state, not a second entry |
| `evidence.delete` | `{id, author?}` → `{deleted: true, tombstones: [incident_id ..]}` |

Event: `evidence.changed` — the `Evidence` without body, or `{id, deleted:
true}`. Attach, detach, rename and delete also publish `incidents.changed`
for each incident whose timeline moved.

```rust
// On the wire, `body` is one of:
{ type: "text", text }                                   // kind terminal | log | text
{ type: "svg", svg }                                     // kind topology
{ type: "chart",                                         // kind chart
  series: [..],                                          // exactly what `metrics.query_many` returned, holes included
  coverage: { since, until, points,
              series: [{ device_id, device, lab, metric, instance, points,
                         first_at?, last_at?, longest_gap_ms?, step_s?, rollup? }] } }
```

* `lines` is present for the three text kinds only; `bytes` and `sha256` are
  of the body as stored (for a chart, of its JSON).
* **Redaction.** `nocturne_ai::Redactor`'s rules, the `config.secrets`
  patterns of every loaded device type, the secrets of every stored
  credential wherever they appear, and every value the rules recognised
  wherever else it occurs in the same item. The title, the note, the name
  and the far end of the session, the names and strings of a chart and the
  recorded parameters pass it too. A device type's pattern is applied one
  line at a time (it is written for a line of configuration; across a line
  break a pattern for `password: value` takes the word after a `Password:`
  prompt), and a placeholder already in the text is never cut in two by a
  later rule. An unknown parameter such as `redact: false` changes nothing.
  * **A private key** is replaced whole when both its markers are there, in
    a transcript, in a string of a chart and in the text of a drawing. One
    that the edge of a capture cut off — the last lines of a session, a
    piece of a log, a selection — has no second marker for a rule to find:
    every line of key material from the marker that is there towards that
    edge reads `<hidden>`, with or without a session log's timestamp in
    front of it, and past the odd line that is something else (a pager's
    `--More--`).
  * **A placeholder a device printed** is text. One with a number no
    redactor could have made (`<secret:4294967295>`) is not counted on from.
  * **In a drawing** the text between tags, the values of attributes,
    comments and CDATA sections are read as the text they stand for
    (`&quot;` is a quote), searched, and written back escaped: a placeholder
    there reads `&lt;secret:N&gt;` (in a comment or a CDATA section, as it
    is), and the SVG stays well formed. The names of elements and attributes
    are never touched. The drawing is gone through twice, so a value
    recognised in its last label is hidden in its first.
* **A drawing is stored as `topology.export` returned it and is hostile
  input**: it may hold a `<script>`, an `onload`, a `<foreignObject>`, an
  address to fetch. It is a string in `body.svg` and nothing in this
  feature ever makes markup of it; the interface must not either. Show it
  as an image and only as an image — an `<img>` whose `src` is a `data:` or
  `blob:` address made from the string — where a browser runs no script and
  fetches nothing. Never `innerHTML`, never `<object>`, `<embed>` or
  `<iframe>`, never a window or tab opened on the address.
* **Text is stored as it was printed**, control characters included: a
  body can hold an escape sequence. Show it as text (preformatted), never
  write it into a terminal emulator. The documents this feature makes show
  every control character and every direction override as a picture of
  itself.
* **Lab.** `source.lab` is the session's own `lab` for a terminal capture;
  for a log, the log's transport (`sim`) or its device; for a chart, whether
  any of its devices is simulated; for a drawing, whether the filter that
  was sent could show a simulated device (a saved view's own filter is not
  known here, so a drawing made from a view alone is labelled lab whenever
  the inventory has a lab device).
* **Refusals.** Past a limit: `invalid` with `details: {limit: "text_bytes"
  | "svg_bytes" | "points" | "items" | "params_bytes", max, actual}`. Text is
  measured as it would be stored: a placeholder can be longer than what it
  replaces, and text that only fits before redaction is refused, not cut.
  `params_bytes` is what was asked for (a chart's series, a drawing's
  filter): it is recorded with the item and copied into every list, event
  and handover that shows it, so it is at most 64 KiB, and the other
  feature is not asked. A feature that is not in the build, does not answer
  in 20 s, or answers with something that is not what was asked for:
  `unavailable`, naming the method. A refusal of the other feature's own
  (no such session, no such log) is passed on with its code. In every case
  nothing is stored.
* **Tombstone.** Deleting an item leaves, on each incident it was attached
  to, the same entry at the same time with no `ref`, `text: "<title> —
  evidence deleted on <YYYY-MM-DD> (UTC)"` and `source.from:
  "evidence.delete"`; the SHA-256 stays in `source.evidence`. Deleting an
  incident removes it from the `incident_ids` of the items attached to it
  (each announced with `evidence.changed`).
* **Audit.** `evidence.capture_terminal` / `.capture_log` /
  `.capture_topology` / `.capture_chart` / `.add_text` / `.rename` /
  `.attach` / `.detach` / `.delete`, each with `evidence_id` and `sha256`,
  the counts that apply (`lines`, `points`, `bytes`) and never the text.
* Bodies are in the table `ops_evidence_bodies`; what an item is, in
  `ops_records` like every other record of this feature.

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

Evidence (`tests/evidence.rs`), through `App::rpc`: the tests' stand-in for
the terminal feature holds a real connection to the fake device and hides
nothing itself, so a secret that is absent from an item was hidden here.
What `docs/dev/spec-capture.md` lists under Tests, plus: a device type's
pattern and a vault value hidden in a real session; every capture method
`unavailable` in a build without the feature it reads from; a note and an
attachment made while `link`, `sync` or `update` is waiting for another
feature are still there afterwards; the free-text caps. And the checks a
second reader asked for: an enable secret, an SNMP community, a type-7
password and a private key (whole, and cut off by the edge of a capture)
through every way an item is made, then looked for in every result, every
event, the audit trail, the review and the handover in every format, and
every row of every table; one incident changed by notes, attachments,
links, syncs, updates and renames at once with nothing lost; escape
sequences, a line of 100,000 characters, fences, formulas, a direction
override and a drawing with a script in it, inert in every format; each
limit at exactly its edge; a feature that never answers; an item after a
restart, and the database file searched byte for byte.
