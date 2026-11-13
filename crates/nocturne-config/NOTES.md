# nocturne-config — working notes (resume file)

Spec: `docs/dev/features/config.md`, conventions: `docs/dev/features/README.md`.
Cargo: always `CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo <cmd> -p nocturne-config`
(fmt too). No git commands. Only touch this crate and `ui/src/api/types/config.ts`.
Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/config/`.

## Status

### Done
- read the specs and the core / drivers / redactor source

### In progress
- crate skeleton, storage (own tables), outline parser

### Next
1. versions + backups (job, events, retention, pins, unsaved, change-event debounce)
2. reading: get/latest/diff/compare/search/missing/export/delete
3. compliance: model, evaluation, built-in packs, runs, remediate, test
4. intended / drift / restore (hand-off to `changes.create`)
5. tests per spec, `ui/src/api/types/config.ts`, clippy, fmt, report

## Decisions (keep; they go into the final report)

(see bottom of file, appended as they are made)
