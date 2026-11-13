# nocturne-insights — working notes

Spec: `docs/dev/features/insights.md` (conventions: `docs/dev/features/README.md`).

Build / test (shared feature target dir, always scoped to this crate):

```bash
export CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features
cargo test -p nocturne-insights
cargo clippy -p nocturne-insights --all-targets -- -D warnings
cargo fmt -p nocturne-insights
```

Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/insights/`.
No git commands. Only this crate and `ui/src/api/types/insights.ts` are ours.

## Status

### Done
- (nothing yet)

### In progress
- design + module skeleton

### Next
- model / settings / world loader / links / config readers
- checks (physical, l2, l3, config, fleet) with table-driven tests
- ledger (own table), scan, worker (debounce), alerts, rpc
- integration tests through `app.rpc`, TS types, clippy/fmt

## Decisions (keep; they go into the final report)

- Insights live in an own SQLite table (`insights`), created through `store.with`: lists must filter,
  count and page in SQL (thousands of rows on a campus). Coverage and per-device memory are documents.
- One insight per problem. Cross-device problems (a link, a duplicate IP) are owned by one end
  (deterministic: lowest device id) and name the other end in `related`; `insights.list {device_id}`
  matches both the owner and `related`.
- "Two consecutive scans of their device": a miss is only counted when the check was evaluated for the
  device in a scan that targeted the device (or a related one). Skipped (missing data) never counts.
- Auto-scans (after `collect.updated`, `config.changed`, link `logs.signal`) are not jobs; only
  `insights.scan` is. Scans run on a blocking thread and are serialised.
- Checks that read metric / log history only run for the devices a scan targets.
