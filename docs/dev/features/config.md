# Feature: configuration management (`nocturne-config`)

Every device's configuration, versioned; what changed and when; search across
the whole network; compliance against written-down rules; drift against what
was intended. Read-only towards devices: anything that would change one is
handed to the change pipeline (`changes.create`).

Engines: `nocturne-drivers` (`session::CliSession::fetch_config`,
`hier::{ConfigTree, remediate, text_diff, Syntax}`), `nocturne-ai`
(`Redactor`), core `connect`, `jobs`, `store.config_*`.

## Backups

* `config.backup {device_ids?: [..], filter?: DeviceFilter, kinds?: ["running","startup"]}`
  → `{job_id}`. Per device: open a CLI session, fetch each kind (the profile
  cleans volatile lines), `store.config_insert` (identical content is not
  stored twice — the version's `last_seen` moves instead). Devices without
  CLI access or whose profile defines no config command are reported as
  skipped with the reason, not failed. Job result:
  `{backed_up, unchanged, changed, skipped: [{device_id, reason}], failed: [{device_id, error}]}`.
* A changed running-config publishes `config.changed {device_id, kind,
  version_id, previous_id, added, removed}` and audits it. First-ever backup
  publishes `config.first_backup`.
* `config.unsaved` → devices whose latest running and startup differ
  (ignoring what the profile ignores), with the diff size. Only meaningful
  where both kinds were fetched; say so otherwise.
* Setting `config`: `{keep_versions: 200, backup_startup: true,
  backup_on_change_event: true, concurrency: 6}`. Retention prunes the oldest
  versions per device beyond `keep_versions` but never the newest, and never
  one referenced by a change record (`changes` marks those through
  `config.pin {version_id, reason}` / `config.unpin`).
* When `backup_on_change_event` is on, a `logs.config_changed {device_id}`
  event (the log feature raises it from syslog like `%SYS-5-CONFIG_I`) queues
  a backup of that device, debounced 60 s.

## Reading (contract)

| Method | Params → Result |
|---|---|
| `config.devices` | `{filter?}` → `[{device_id, name, profile, running: {version_id, fetched_at, lines, sha256}?, startup: {..}?, versions, last_change_at?, last_error?}]` — the backup coverage table |
| `config.versions` | `{device_id, kind?: "running", limit?, before_id?}` → `[ConfigVersion{id, device_id, kind, sha256, lines, bytes, first_seen, last_seen, origin, pinned}]` |
| `config.get` | `{version_id, redact?: true}` → `{version, text, redacted: bool}` |
| `config.latest` | `{device_id, kind?, redact?}` → same, or `not_found` with "no backup yet" |
| `config.diff` | `{a: version_id, b: version_id, context?: 3, redact?: true}` or `{device_id, kind?}` (latest vs previous) → `{a, b, diff: TextDiff, sections: [{path: [..], added, removed}]}` — `sections` groups the change by the configuration section it is in (interface, router, line…) using the profile's syntax |
| `config.compare` | `{a: {device_id}|{version_id}, b: …, redact?}` → diff between two devices (for "why does SW2 behave differently from SW1") |
| `config.search` | `{query, regex?: false, case?: false, filter?, kind?: "running", context?: 1, limit?: 500}` → `{matches: [{device_id, name, version_id, line_no, section: [..], line, before: [..], after: [..]}], devices_searched, devices_matched, truncated}` — over the latest version of every device. Invalid regex → `invalid` with the regex error. Always redacted. |
| `config.missing` | `{query, regex?, filter?}` → devices whose latest config does **not** contain the line/pattern |
| `config.export` | `{dir, filter?, redact?: false}` → writes `<dir>/<device name>.<kind>.cfg`; local access only; refuses a non-empty directory that is not a previous export (marker file) |
| `config.delete` | `{device_id}` or `{version_ids}` → removes stored versions (not pinned ones) |

Redaction: `Redactor` with the profile's `config.secrets` patterns added.
`redact: false` returns the stored text and audits "viewed unredacted
configuration of X".

## Compliance

A **policy** is a named list of rules with a scope; a **run** evaluates
policies against the latest stored running-config of each device in scope
(no device access — it is instant and works offline; a device with no backup
is `not_evaluated`, never `pass`).

```rust
struct Policy { id, name, description, enabled, builtin: bool,
    scope: Scope { profiles: [..], roles: [..], sites: [..], tags: [..] },   // empty lists = any
    rules: Vec<Rule> }
struct Rule { id, title, severity: info|low|medium|high|critical, rationale,
    check: Check, remediation: Option<Vec<String>> /* config lines, may use {{ }} over the device */ }
enum Check {                                   // #[serde(tag = "type", rename_all = "snake_case")]
    Present { line },                          // exact global line (after whitespace normalisation)
    Absent { line },
    Matches { pattern },                       // regex must match some line
    NotMatches { pattern },
    Section { parent: pattern, each: bool, must: [line|pattern..], must_not: [..] },   // e.g. every `interface .*` that is an access port has `spanning-tree portfast`
    SectionWhen { parent, when: pattern, must: [..], must_not: [..] },                 // only sections containing `when`
    Snippet { config: text },                  // a golden block: hier-diff it, pass when nothing is missing
}
struct Finding { policy_id, rule_id, device_id, status: pass|fail|not_applicable|not_evaluated, evidence: [{line_no, line}], missing: [..], detail }
```

| Method | Params → Result |
|---|---|
| `compliance.policies.list` / `.get` / `.save` / `.delete` / `.duplicate` | CRUD; built-ins are read-only (duplicate to edit), can be disabled |
| `compliance.policies.validate` | `{policy}` → `{ok, problems}` (regex errors, empty checks…) |
| `compliance.run` | `{policy_ids?, device_ids?, filter?}` → `{run_id, summary}` and stores the run |
| `compliance.results` | `{run_id?: latest, status?, severity?, device_id?, policy_id?, limit, cursor}` → `{findings, summary: {devices, rules, pass, fail, not_applicable, not_evaluated, score_pct, by_severity}}` |
| `compliance.runs` | history of runs (id, at, summary), so the score can be charted over time |
| `compliance.remediate` | `{run_id, findings: [{device_id, rule_id}]}` → `{change_id}` — renders the rules' remediation lines per device and calls `changes.create` (origin `compliance`). Never applies anything itself. |
| `compliance.test` | `{rule, text | device_id}` → the finding for one rule against pasted config (rule editor preview) |

Ship built-in policy packs as data (YAML/JSON embedded): a hardening
baseline for IOS/IOS-XE (password encryption, no HTTP server, SSH v2 only,
VTY transport input ssh + exec-timeout, logging host, NTP, banner, no
source-route, AAA new-model, no SNMP `public`/`private`, enable secret not
enable password, console timeout, unused ports shut — whatever can be judged
from the config text alone) and one for RouterOS `/export` (telnet/ftp/www
services disabled, strong SSH crypto, no default `admin` without password is
*not* decidable from an export — do not pretend it is; neighbour discovery
restricted, DNS allow-remote-requests off on edge…). Every built-in rule
needs a test with a passing and a failing config. Rules must be right about
the platform — a wrong hardening rule is worse than none.

Events: `compliance.finished {run_id, summary}`.

## Intended configuration and drift

* `config.intended.get/set/clear {device_id, text}` — the configuration (or
  the part of it) the device should have. Stored as a document; full files or
  fragments are both fine.
* `config.drift {device_id}` → `{supported, commands: [..], warnings, diff}`:
  `hier::remediate(latest running, intended, syntax)` — what would have to be
  sent to make the device match. `config.drift.fix {device_id}` →
  `{change_id}` via `changes.create` (intent `intended`).
* `config.restore {version_id}` → `{change_id}`: a change whose intent is
  "make the running-config equal to this stored version" — computed by diff,
  reviewed and applied by the change pipeline; never a blind overwrite.

## Tests

Backups against fake devices (first backup, unchanged second, changed third →
event + audit + diff; startup vs running → `config.unsaved`; a device that
fails; a monitor-only device skipped; cancellation mid-job; concurrency
bound); retention incl. pinned versions; search (plain, regex, section path,
context lines, redaction of secrets in results, invalid regex); diff sections;
compare; export/refusal; every `Check` variant with pass/fail/not-applicable
cases incl. `each` semantics and whitespace normalisation; every built-in
rule; `compliance.remediate`/`config.restore`/`config.drift.fix` produce the
right `changes.create` call (register a stub `changes.create` in the test
app and assert on what it received); redaction on by default and audited
when turned off.
