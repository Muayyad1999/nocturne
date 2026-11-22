# nocturne-ops: working notes

Task: the operations record — overview, incidents, shift handover, reports,
notes, evidence, global search, audit view.
Spec: `docs/dev/features/ops.md`; evidence: `docs/dev/spec-capture.md`;
conventions: `docs/dev/features/README.md`.

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
src/records.rs    own tables (ops_records, ops_links): facet + search + link filtering in SQL,
                  and `transaction` — several records read and written as one step
src/evidence.rs   evidence items: the five ways to make one, list/get/rename, attach/detach,
                  delete with tombstones, the appendix of a review; bodies in ops_evidence_bodies
src/redact.rs     the redactor every piece of evidence passes before it is stored
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
src/text.rs       escaping (html, markdown, csv formulas, code fences, control characters),
                  excerpts, slugs, the 64 KiB cap on free text
src/timefmt.rs    one fixed UTC offset per document, durations
tests/common/     the lab: core + ops + stubs for the features ops gathers from
tests/common/     … and a stand-in for the terminal that holds a real connection to the
                  fake device (`open_session`), `topology.export`, `metrics.query_many`
tests/{overview,incidents,handover,reports,notes_search,absent,evidence}.rs
ui/src/api/types/ops.ts
```

Clippy clean with `-D warnings`, `cargo fmt --check` clean, no `unsafe`.

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

## Evidence: decisions

- **What an item is and what it contains are stored apart.** The item
  (title, source, hash, the incidents it is on) is a record like the others,
  so paging, the kind facet, word search and "which items are about this
  device / on this incident" are the record store's. The body is one row of
  `ops_evidence_bodies`, read by `evidence.get` and by an export and by
  nothing else.
- **`records::update` got its callers, and a sibling.** Every incident
  mutation goes through `incidents::modify` (read, change, write in one
  transaction). `records::transaction` is the same thing for a step that
  moves two records — an item and an incident — and rolls back whatever it
  wrote when it is refused half way. The store is one connection behind a
  lock, so nothing inside a transaction waits: `update`, `link` and `sync`
  gather from other features first, on a copy, and re-apply their change to
  the incident as it is when they get the lock.
- **The redactor is the engine's parts, put together again here.** The
  assistant builds the same thing for a chat and the terminal has the
  transcript handling; features do not link, so `redact.rs` is a third copy
  of the construction (not of any rule). It belongs in the core — which
  already depends on `nocturne-ai` and owns the profiles and the vault — as
  one function all three would call. Until then, what is here differs from
  a chat's redactor in three deliberate ways, each found by a test on a real
  session of the fake device:
  - a device type's pattern is applied **one line at a time**. On a whole
    transcript `linux.yaml`'s pattern for `password: value` reads the
    device's `Password:` prompt, crosses the line break and takes `SW1#show`
    for the password;
  - a **placeholder is never cut in two**. The engine applies its rules in
    sequence to the same text, and the same pattern finds `secret: value`
    inside `<secret:1>`, leaving `<secret:<secret:2>` (and a table entry
    `"1>"`). Placeholders are set aside while rules run, and a pattern that
    can match inside one gets a pass of its own;
  - what follows a prompt or a session log's timestamp is offered to the
    rules **as the start of a line**, because `evidence.add_text` gets text
    that has not been through the terminal's own transcript handling.
- **What a second reading of the redactor found, each with a test that
  failed first** (`redact.rs`, `tests/evidence.rs`):
  - a key **cut off in a session log** was left standing: every line of a
    log has a timestamp in front, and a line with one is not "only base64".
    The timestamp is set aside before the question is asked, and a cut-off
    key is hidden towards the edge that cut it past the odd line that is
    something else (a pager's `--More--`);
  - a key of **several lines inside one string** — a chart's label, the
    text of a drawing — was searched a line at a time and never seen whole;
  - a drawing was cut at every `>`, so a vault value with a `>` in it,
    standing in an attribute, was in two pieces and found in neither. A
    drawing is now taken apart as a parser takes it (`pieces`): text,
    quoted attribute values, comments, CDATA. Each is read as the text it
    stands for, so a rule that takes a quoted value whole sees the quote
    and not `&quot;`; names of elements and attributes are never touched
    (a rule that took `stroke-width` for a password would otherwise unmake
    the drawing); and it is gone through twice, so a value recognised late
    is hidden where it stood earlier;
  - `<secret:4294967295>` printed by a device set the engine's counter to
    its last value and the next placeholder overflowed it (a panic in a
    debug build, numbering from zero again in a release one);
  - a chart's `metric` and `instance`, the session's name and its far end
    were stored as given while everything beside them was redacted.
- **What a client names is stored, so it is bounded.** `add_text`'s
  `session_id` is kept whether or not the session can be found; it now has
  to be shaped like an id. What a chart or a drawing was asked for is part
  of the item's head (lists, events, handovers), so it is at most 64 KiB
  (`params_bytes`), refused before the other feature is asked.
- **A document does not fall over a source's numbers.** A gap between two
  sample times is computed saturating and `timefmt::duration` formats
  `i64::MAX`: a series with times at the ends of the number line is kept
  and exported.
- **In CSV, everything that is somebody's words is guarded**, not only the
  lines of an exhibit: its title, its facts, and the paragraphs and list
  items of the document (`text::cell_line`). `text::visible` also shows
  direction overrides (U+202A–202E, U+2066–2069, U+200E/F, U+061C), which
  are not control characters to Unicode and do the same job.
- **Only stored credentials are "what the vault knows".** `Vault` has no way
  to list what it holds, so a webhook URL's token or a model provider's key
  is hidden only where a rule recognises it. A core redactor could know all
  of it.
- **Stored configurations are not read to prime the redactor** (a chat's
  does, once per chat): it would cost megabytes of regex per capture.
- **A drawing in a document is an image.** In HTML an `<img>` with a `data:`
  address, where a browser runs nothing; in Markdown, CSV and JSON it is not
  rendered at all. The stored SVG is what `topology.export` returned and is
  never placed into a page as markup by this crate.
- **`lab` for a drawing errs towards saying so.** An item is labelled
  without being opened, and a saved view's filter is the topology feature's
  to know.
- **Not built:** the assistant cannot write evidence (the timeline is kept
  by the engine and by people); nothing stores pixels or files.

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
