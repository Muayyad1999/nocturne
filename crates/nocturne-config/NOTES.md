# nocturne-config — working notes (resume file)

Spec: `docs/dev/features/config.md`, conventions: `docs/dev/features/README.md`.
Cargo: always `CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo <cmd> -p nocturne-config`
(fmt too). No git commands. Only touch this crate and `ui/src/api/types/config.ts`.
Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/config/`.

## Status

### Done
- read the specs and the core / drivers / redactor source
- storage (`db.rs`), outline parser (`outline.rs`), redaction wrapper (`redact.rs`)
- backups (`backup.rs`): job, events, retention, change-event debounce, status
- versions / pins / delete (`versions.rs`), coverage + unsaved (`coverage.rs`)
- diff + compare (`diff.rs`), search + missing (`search.rs`), export (`export.rs`)
- intended / drift / restore (`intended.rs`), change hand-off (`handoff.rs`)
- `lib.rs` wires every module (it was an empty stub: nothing was compiled before)
- compliance (`compliance.rs` + `compliance/`): model, evaluation of every
  `Check`, validation, runs + results + history, remediation hand-off,
  `compliance.test`, two built-in packs (IOS, RouterOS) as embedded YAML
- unit tests of outline / redaction / export / built-in packs

- tests (80): `tests/{backups,redaction,reading,drift,compliance_checks,
  compliance_builtins,compliance_runs}.rs` + `tests/support/mod.rs`, and the
  unit tests in `outline.rs`, `redact.rs`, `export.rs`, `compliance/builtin.rs`
- `ui/src/api/types/config.ts`
- clippy (`--no-deps -- -D warnings`) clean, `cargo fmt -p nocturne-config` applied

### Next
Nothing outstanding in this crate. Open items for others are in the report:
a redaction gap worked around here (`src/redact.rs` `GAP_RULES`), and
`cargo clippy -p nocturne-config --all-targets -- -D warnings` currently fails
in `nocturne-drivers` (two `collapsible_if` lints), so it has to be run with
`--no-deps` until that crate is fixed.

## Decisions (keep; they go into the final report)

- `db.rs` keeps this feature's own tables beside the core's `configs`:
  `config_meta` (last_seen, bytes), `config_pins`, `compliance_runs`,
  `compliance_findings`. SQLite has no unsigned integers, so sizes round-trip
  through `i64`.
- Compliance evaluates the configuration **as stored**, never the redacted
  form — a rule about a community called `public` could not fire against
  `<secret:1:weak>`. Only the evidence that leaves the crate is redacted, and
  because `redact.rs` keeps the line count, quoted line numbers still line up.
- `Scope` got two lists the spec does not name: `profiles` matches the
  device's profile *or one it extends*, and `os` matches the device type's
  `os` field. Without them a built-in pack could not say "Cisco IOS and
  IOS-XE, not EOS or NX-OS" (there is no RouterOS profile in the build at
  all, so the RouterOS pack is scoped by `os` alone).
- Built-in policies are read-only: `compliance.policies.save` on a built-in
  accepts a change of `enabled` and nothing else (the off-switch is a kv list
  of ids, so the pack stays the file it shipped as).
- Rules I refused to ship because the configuration text cannot answer them:
  "unused ports are shut" (nothing in the text says a port is unused),
  per-interface PortFast/BPDU-guard (a global `spanning-tree portfast
  default` makes the per-port line unnecessary, so the rule would fail a
  correct device), and anything about RouterOS's default `admin` user (not in
  an export). Every rule shipped states in its rationale exactly which line
  it looks for.
- `compliance.remediate` renders a rule's remediation once per offending
  section when the lines mention `{{ section }}` (the author writes the
  section header), otherwise once, globally. Undefined variables are an
  error, never an empty substitution.
- Run retention is by constants, not settings (findings of the newest 20
  runs, summaries of the newest 500), so the settings section stays the four
  keys the spec names.
- `nocturne_ai::Redactor` reads `ntp authentication-key 1 md5 <key> 7` wrongly:
  the generic `authentication-key` rule matches `md5` as the secret and leaves
  the key in the clear. `src/redact.rs` adds the right pattern in front of the
  built-in rules (extra rules run first) and `tests/redaction.rs` holds it to
  it. Fix belongs upstream.
