# nocturne-ops: working notes

Task: the operations record — overview, incidents, shift handover, reports,
notes, global search, audit view.
Spec: `docs/dev/features/ops.md`; conventions: `docs/dev/features/README.md`.

Cargo (always; the target directory is shared with other agents):

```bash
export CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/main
cargo test -p nocturne-ops
cargo clippy -p nocturne-ops --all-targets --no-deps -- -D warnings
cargo fmt -p nocturne-ops            # never a bare `cargo fmt`
```

Only `crates/nocturne-ops/**` and `ui/src/api/types/ops.ts` are this crate's
to touch. No git commands.

## Status: complete

```
src/lib.rs        init(): settings, state, own tables, routers
src/settings.rs   the `ops` section (cache, budgets, windows, limits)
src/model.rs      Incident / Entry / Provenance / Note / SavedHandover / SavedReport
src/records.rs    own tables (ops_records, ops_links): facet + search + link filtering in SQL
src/sources.rs    every other feature ops reads, and the Gap/Unavailable vocabulary
src/refs.rs       Period, Ref, DeviceRef/AlertRef/ChangeRef/InsightRef/JobRef, Devices
src/configs.rs    configuration movement read from the core's snapshot table
src/reach.rs      coverage / availability / outages from the `up` sample series
src/timeline.rs   gathering a timeline with provenance, gaps, and suggest_links
src/incidents.rs  the incident surface and the post-incident review document
src/overview.rs   the home screen, independently nullable part by part
src/handover.rs   handover.generate / save / list / get / delete / export
src/reports.rs    7 kinds x 4 formats, saved reports
src/notes.rs      notes with `[[device name]]` resolution
src/search.rs     the global search fan-out
src/audit.rs      the core's audit trail, filtered and joined
src/render.rs     one document model -> markdown / csv / html / json
src/text.rs       escaping (html, markdown, csv formulas), excerpts, slugs
src/timefmt.rs    one fixed UTC offset per document, durations
tests/common/     the lab: core + ops + stubs for the features ops gathers from
tests/{overview,incidents,handover,reports,notes_search,absent}.rs
ui/src/api/types/ops.ts
```

95 tests (58 unit, 37 integration through `App::rpc`), clippy clean with
`-D warnings`, `cargo fmt --check` clean, no `unsafe`.

## Decisions

- **Storage**: two own tables (`ops_records`, `ops_links`) rather than the
  document store, because a list must not parse bodies (timelines, report
  text) and a search must filter in SQL. An incident's facet is
  `<state>:<severity>`, so narrowing by either stays a SQL `IN`.
- **Provenance on every timeline entry** (`source: {from, evidence,
  gathered_at}`). `ts` is when the thing happened; `gathered_at` is when ops
  read it. Entry ids are derived from the fact (`alert:a1:fired`,
  `change:c1:<device>:applied`, `config:<device>:<version>`, `audit:<id>`),
  so re-gathering is idempotent and nothing is duplicated or lost.
- **A gap is a timeline entry** of kind `gap` (an addition to the spec's list
  of kinds) at the moment of the failed read, identified by source and
  reason, so a source that keeps failing leaves one mark rather than one per
  attempt. Gaps persist: that nothing was known for a while is itself part of
  the record.
- **A first backup is a baseline, not a change.** It is on the timeline as
  "recorded for the first time" and is never counted as configuration
  movement or as `unplanned`.
- **`unplanned` is only claimed when it can be checked.** Without a change
  list, `checked: false` and `unplanned: false`.
- **Backups are counted from the core's own snapshot table**
  (`store.config_versions`), not from `config.devices`: every backup is
  stored there by whoever took it, so the coverage count is as true in a
  build without the configuration feature as in one with it. `config.devices`
  is used for the one thing only it knows — that a backup was *attempted and
  failed* — and the handover's `backups` section is `null` without it.
- **`alerts.summary` first, `alerts.list` as the fallback**, because only the
  summary knows how many alerts are open without listing them. The overview
  says which it used (`from`), and a count taken from a capped list carries
  `truncated: true`.
- **Drift on the overview is the latest compliance run**, not `config.drift`:
  that method is per device, and an overview that is polled every 30 s cannot
  make one call per device.
- **`handover.save` and `reports.save` regenerate server-side.** The client
  sends a period (or the generated document, of which only `period` is read);
  the data sections are made again here. Only `notes` is stored as sent, so
  the assistant can be asked to polish the prose and still cannot write a
  number.
- **Periods are half-open `[since, until)`**, so consecutive handovers
  neither overlap nor leave a gap.
- **Availability states its own coverage**, and a device observed for less
  than half the period is listed separately with no percentage. A device with
  no samples at all is in a third list: unknown is not 0 %.
- **No single most-common version, no "out of step".** Two versions on equal
  numbers of a model means the model has no standard version, and naming one
  would be the report's own invention.
- **CSV cells that start with `=`, `+` or `@` are quoted out**, because a
  spreadsheet runs them and interface descriptions are written by whoever has
  the console.
- Times in Markdown/HTML are shown in one fixed UTC offset, stated in the
  header (`utc_offset_minutes`; default the server's). CSV and JSON carry the
  raw values.
- This crate has no method that reaches a device, and no dependency that
  could: it reads the core store, the inventory, the job list and other
  features' RPC.

## Discarded from the half-written crate

- `records::linked` ("ids of the records that refer to this target") and its
  assertions: the same question is answered by the link filter in
  `records::page`, which also pages and combines with the other filters.
- `refs::Period::ms` and a `Gathered::unavailable` helper written early and
  never used; gaps are reported as timeline entries instead.

Everything else that was there — `records.rs`, `sources.rs`, `refs.rs`,
`reach.rs`, `text.rs`, `timefmt.rs` — was kept and built on.

## Needed from the core (worked around)

- **The audit trail records no actor.** `App::audit(action, device_id,
  message, detail)` has nowhere to put *who*, so a timeline entry gathered
  from the trail has an empty `author`, and the incident methods take an
  `author` parameter per call instead. A `by` field on `audit(...)` would fix
  both.
- **A deleted device leaves no record but the audit message.** The handover's
  "removed" list parses the device's name out of the backticks in
  `Deleted device \`NAME\`` (`device.delete`, whose detail is `null`). A
  tombstone, or that name in the detail, would make it reliable.
- `store.config_versions` is one query per device, so backup coverage over a
  large estate is N queries; there is no "newest version of every device".
- `reach::of_device` cannot be tested for a period before the process
  started, because a device's `created_at` is set by the core. The tests move
  it back through the store (`tests/common/mod.rs::backdate`).
