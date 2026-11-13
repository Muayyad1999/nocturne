# nocturne-ops: working notes (resume file)

Task: the operations record (overview, incidents, handover, reports, notes, search).
Spec: `docs/dev/features/ops.md`; conventions: `docs/dev/features/README.md`.

Cargo (always, fmt too):
`CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo <cmd> -p nocturne-ops`
No git commands. Only touch `crates/nocturne-ops/` and `ui/src/api/types/ops.ts`.
Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/ops/`.

## Status

### Done
- Read the conventions, the ops spec, the core source and the specs of the features ops reads
  (changes, config, logs, monitor, insights, ipam, topology/locate).
- Stub crate builds in the shared target dir.

### In progress
- Foundation: `records.rs` (own tables), `sources.rs` (optional upstream features), shared refs/text/time.

### Next
1. overview  2. incidents (+ timeline sync, suggest, export)  3. handover  4. reports (7 kinds x 4 formats)
5. notes  6. search  7. integration tests per spec  8. `ui/src/api/types/ops.ts`  9. clippy/fmt, report.

## Decisions (go into the final report)

- Storage: two own tables (`ops_records`, `ops_links`) instead of the document store, because
  lists must not parse bodies (timelines, report contents) and search must filter in SQL.
- Optional features: `app.methods()` is read once; every upstream read goes through `sources.rs`,
  which holds the minimal shapes ops reads. Rows are parsed strictly: a required field missing
  makes the section unavailable (with the reason), never a guess.
- Every nullable section is explained in a sibling `unavailable: [{section, reason, message}]`
  (`not_installed | failed | timeout | malformed | no_data`).
- Periods are half-open `[since, until)` so consecutive handovers neither overlap nor leave gaps.
- Reachability series read by the availability report and the handover: metric `up`, instance ``
  (1 = answered, 0 = did not), as `docs/dev/features/monitor.md` names it.
- Alert times follow the monitor spec: `started_at`, `acked_at`, `resolved_at`.
- `handover.save` / `reports.save` persist the server's own generated copy (looked up by id), so
  data sections can never be written by a client or a model.
- Times in Markdown/HTML are shown in one fixed UTC offset, stated in the header
  (`utc_offset_minutes` param; default the machine's local offset). CSV: RFC 3339; JSON: ms.
