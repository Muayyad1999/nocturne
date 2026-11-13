# nocturne-automation — working notes (resume file)

Spec: `docs/dev/features/automation.md`, conventions: `docs/dev/features/README.md`.
Cargo: always `CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo <cmd> -p nocturne-automation`
(fmt too). No git commands. Only touch this crate and `ui/src/api/types/automation.ts`.
Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/automation/`.

## Status

### Done
- read the specs, the core, drivers (`expr`, `policy`, `profile`, `hier`, `model`), textfsm `ntc`, minijinja 3.1 source

### In progress
- design + crate skeleton

### Next
1. `render` (strict MiniJinja env delegating filters to `Expressions`, tracked context)
2. runner (gate, job, storage, compare, search, export, retention)
3. templates (model, variables, CRUD, validate, render, apply, built-ins)
4. playbooks (YAML model, validate, run, history)
5. schedules (When, next-run, scheduler with injected clock)
6. ansible (status, export, run)
7. tests per spec, `ui/src/api/types/automation.ts`, clippy, fmt, report

## Decisions (keep; they go into the final report)

(appended as they are made)
