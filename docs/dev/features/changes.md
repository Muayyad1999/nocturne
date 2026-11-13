# Feature: changes (`nocturne-changes`)

The one path by which Nocturne changes a device. A person, a template, a
compliance fix, a restore or the AI assistant all end up here with the same
thing — "these devices should get this configuration" — and from then on the
pipeline is the same: **plan** (exact commands, predicted diff, risk),
**approve** (bound to that exact plan), **apply** (backup, dead-man switch,
send, verify), and **roll back** when verification fails.

This is why Nocturne is allowed to touch production: nothing reaches a
device that a human has not seen in its final form, every change can be
undone, and a change that cuts off its own management access undoes itself.
Correctness here matters more than anywhere else in the product.

Engines: `nocturne-drivers` (`hier::{remediate, ConfigTree, text_diff}`,
`policy`, `session::CliSession::{configure, save, fetch_config, exec}`,
profile `config.guard`), core `connect::cli` (+ `Cli::session()` — this is
the only crate allowed to use it), `collect`, `jobs`. Config versions are
stored through `store.config_insert`; pin them with RPC `config.pin` when
that method exists (tolerate its absence).

## Model (contract)

```rust
struct Change {
    id, title, description,
    origin: user|assistant|template|compliance|restore|drift|playbook,
    state: ChangeState,
    targets: Vec<Target>,
    options: Options,
    approval: Option<Approval>,
    created_at, updated_at, created_by: String,       // "user" | "assistant" | …
    ticket: Option<String>,                           // free-text reference (change-request number)
}
enum ChangeState { Draft, Planned, Approved, Applying, Applied, Failed, RolledBack, PartiallyApplied, Cancelled }

struct Target {
    device_id,
    intent: Intent,
    plan: Option<Plan>,
    result: Option<TargetResult>,
}
enum Intent {                                        // #[serde(tag = "type", rename_all = "snake_case")]
    /// Configuration-mode lines exactly as they would be typed (section headers included).
    Lines { lines: Vec<String> },
    /// Desired configuration (whole file or fragment): the commands are computed by hier diff against the running-config.
    Intended { config: String, /// true: the device must equal `config` (remove what is not in it); false: `config` must be present (additive)
               exact: bool },
}

struct Plan {
    commands: Vec<String>,               // exactly what will be sent in configuration mode, in order
    rollback: Vec<String>,               // what undoes it, computed from the same diff (empty = cannot be computed; see `warnings`)
    diff: TextDiff,                      // predicted running-config before → after
    risk: Risk,
    protected_hits: Vec<ProtectedHit>,   // why this needs a typed confirmation
    warnings: Vec<String>,
    base_version_id: i64,                // the running-config backup the plan was computed against
    base_sha256: String,
    guard: Option<GuardPlan>,            // the dead-man switch that will be armed, if any
    plan_hash: String,                   // sha256 over device, commands, rollback, base_sha256, options
    planned_at,
}
struct Risk { level: low|medium|high|critical, reasons: Vec<String> }
struct ProtectedHit { kind: String, subject: String, command: String, why: String }
struct GuardPlan { name, minutes, arm: Vec<String>, disarm: Vec<String>, effect }

struct Options {                                     // #[serde(default)]
    save: bool,                    // persist to startup after a verified apply (default true)
    guard_minutes: Option<u32>,    // arm the platform's dead-man switch; None = decide from risk (see below)
    parallel: u32,                 // devices at a time (default 1 — one at a time, in order)
    stop_on_failure: bool,         // default true: a failed device stops the remaining ones
    verify: Verify,
    rehearse: bool,                // reserved for the lab twin (ignore for now)
}
struct Verify {                                      // #[serde(default)]
    config: bool,                  // default true: re-read the running-config and check it is what the plan predicted
    neighbors: bool,               // default true: no LLDP/CDP neighbour that existed before may be gone afterwards
    interfaces: bool,              // default true: no interface that was up before may be down afterwards (except ones the change shuts down)
    routes: bool,                  // default false
    pings: Vec<String>,            // addresses the device itself must still be able to ping afterwards
    settle_s: u32,                 // wait before verifying (default 5; 0 for lab devices)
}

struct Approval { by, at, plan_hashes: Vec<String>, note, confirmed_devices: Vec<String> }

struct TargetResult {
    state: pending|applying|applied|failed|rolled_back|rollback_failed|skipped|guard_fired,
    started_at, finished_at,
    steps: Vec<Step { name, ok, detail, at }>,       // connect, drift check, backup, arm guard, apply, verify, rollback, disarm guard, save, final backup
    sent: Vec<CommandOutput>,                         // every configuration line with the device's answer
    failed_line: Option<usize>, error: Option<String>,
    before_version_id: Option<i64>, after_version_id: Option<i64>,
    verification: Vec<Check { name, ok, detail }>,
    transcript: String,                               // redacted session transcript
}
```

## RPC (contract)

| Method | Params → Result |
|---|---|
| `changes.list` | `{state?, device_id?, origin?, limit?, cursor?}` → `[ChangeSummary]` (no plans/results/transcripts) newest first |
| `changes.get` | `{id}` → `Change` |
| `changes.create` | `{title, description?, origin?, ticket?, targets: [{device_id, intent}], options?, plan?: bool}` → `Change` (state `draft`; `plan: true` also starts planning and returns `{change, job_id}`) |
| `changes.update` | `{id, title?, description?, ticket?, targets?, options?}` → `Change`. Only in `draft`/`planned`; any change to targets or options discards the plans and the approval. |
| `changes.plan` | `{id}` → `{job_id}`; fills `target.plan` for every target, state → `planned` |
| `changes.approve` | `{id, plan_hashes: [..], confirm?: {device_name: typed_text}, note?}` → `Change` (state `approved`) |
| `changes.unapprove` | `{id}` |
| `changes.apply` | `{id}` → `{job_id}` — requires `approved` with hashes still matching |
| `changes.rollback` | `{id, device_ids?}` → `{job_id}` — undo an applied change on demand (restores the `before` version by diff) |
| `changes.cancel` | `{id}` — draft/planned/approved → cancelled; applying → cancels the job (the device in flight completes or rolls back; none is left half-configured) |
| `changes.delete` | `{id}` — not while applying; the audit trail keeps the record that it existed |
| `changes.preview` | `{device_id, intent}` → `Plan` without creating anything (the "what would this do" behind the quick-configure box) |
| `changes.assistant_apply` | `{id}` → `{job_id}` or `forbidden` — see Autonomy |

Events (contract): `changes.changed {ChangeSummary}` on every state change;
`changes.progress {change_id, device_id, step, ok, detail}` while applying.

## Planning

For each target, over one CLI session:

1. Fetch the running-config; store it as a version (`base_version_id`).
2. Build the command list:
   * `Lines`: the lines as given, validated against the tree: each must be
     parseable into the platform's hierarchy (a sub-command with no section
     header before it is an error, not a guess).
   * `Intended`: `hier::remediate(running, intended, syntax)`; for
     `exact: false` drop the removals.
   On platforms where `remediate` reports `supported: false`, `Intended` is
   refused with the reason; `Lines` still works.
3. Predict the result by applying the commands to the tree (`ConfigTree`
   — add what you need to `nocturne-drivers::hier` if it lacks "apply these
   commands to this tree"; that crate is yours to extend in `hier.rs` only,
   with tests) and diff. The predicted config is what `Verify.config`
   compares against after the apply.
4. Compute `rollback` = `remediate(predicted, running)`. If it cannot be
   computed, say so in `warnings` and raise the risk one level: a change that
   cannot be undone automatically is not low risk.
5. Classify every command with the platform policy. A `Dangerous` command
   (`reload`, `erase`, …) is refused outright at planning: the pipeline
   configures, it does not operate.
6. Risk and protected resources — rules below.
7. `plan_hash`.

A plan is only valid for the configuration it was computed against: applying
re-reads the running-config and refuses with `conflict` ("the configuration
of X changed since this was planned — plan again") if its sha256 differs from
`base_sha256`.

### Risk and protected resources

Protected hits (each forces a typed confirmation — the device name — at
approval, whatever the autonomy setting):

* the device is marked `protected`;
* the management path: the interface/SVI that carries the address Nocturne
  reaches the device on (match `device.address` against interface IPs in the
  running-config), its IP address, VLAN, `shutdown`; the default route /
  default gateway; the VRF of that interface;
* how Nocturne gets in: `line vty`, `transport input`, `login`, AAA,
  `username`, `enable secret`, `ip ssh`, `crypto key`, ACLs applied to VTY or
  to the management interface (`access-class`, `ip access-group` there), and
  the entries of those ACLs; RouterOS `/ip service`, `/user`, firewall input
  chain;
* **trunk allowed-VLAN lists**: any `switchport trunk allowed vlan <list>`
  without `add`/`remove` on an interface that currently allows more — it
  replaces the list and can black-hole every other VLAN on the trunk. The
  planner must emit `add`/`remove` forms when the intent is a delta
  (`remediate` does), and flag the replacing form as a hit when a user typed
  it;
* `shutdown` on an interface that has an LLDP/CDP neighbour in the latest
  collected data, or that is a trunk / port-channel / routed uplink;
* removing a VLAN that has ports or an SVI; changing spanning-tree mode or
  priority; `no router …` / removing a routing process or its networks;
  `no ip routing`; changing the hostname (breaks prompt matching mid-session
  — allowed, but the driver must follow it).

Risk level = the worst of: protected hits (→ high; management path or
access → critical); number of sections touched (> 20 → medium); removal of
whole sections (→ medium); cannot compute rollback (+1); platform profile
maturity not `verified` (+1); the device has no dead-man switch (+1 when
level ≥ high). Low is reserved for additive changes confined to access ports
/ descriptions / VLAN names / NTP / logging / SNMP location-type lines.
Every level comes with `reasons` a person can read.

## Applying

Per device (in `options.parallel` lanes, in target order):

1. Connect; **drift check** (above).
2. Snapshot what verification needs (`collect::run_on` for neighbors /
   interfaces / routes as requested) — best effort: a kind the platform
   cannot collect disables that check with a note, it does not fail the change.
3. Store the pre-change running-config (`before_version_id`), pin it.
4. **Guard**: when the profile has a `config.guard` and
   (`options.guard_minutes` is set, or risk ≥ high), arm it (default 10
   minutes) — e.g. `reload in 10` on IOS: if the change cuts the session, the
   device reloads into its saved configuration by itself. Never save while a
   guard is armed. If the platform has no guard and the plan has a
   management-path hit, say so as a warning at planning time.
5. Send the commands (`CliSession::configure`). A rejected line stops the
   device immediately → rollback.
6. Wait `settle_s`, then verify: re-read the running-config and compare with
   the prediction (ignoring lines the profile ignores) — differences are
   listed; run the neighbour / interface / route comparisons and the pings.
   Any failed check → rollback.
7. **Rollback** = send the plan's `rollback` commands, re-read, and require
   the running-config to equal the pre-change version; if the stored rollback
   is not enough, compute `remediate(now, before)` and apply that. Report
   `rolled_back` or — if the device still differs — `rollback_failed` with
   the exact remaining diff. If the session was lost during apply or verify:
   reconnect (3 attempts over ~60 s); if reachable, roll back; if not, the
   result is `guard_fired` when a guard was armed (with what the device will
   do and when) or `failed` with "the device is unreachable and no dead-man
   switch was armed".
8. Disarm the guard; save if `options.save`; store the post-change version
   (`after_version_id`); done.

Change state afterwards: all applied → `applied`; all rolled back →
`rolled_back`; a mix → `partially_applied`; nothing changed and something
failed → `failed`. `stop_on_failure` marks the remaining targets `skipped`.

Nothing is ever applied "blind": no bulk paste, no partial plan, no retry
that re-sends lines without re-reading the device.

Cancelling an apply: finish or roll back the device in flight, skip the
rest. An app shutdown mid-apply is the same; on the next start, changes found
in `applying` are marked `failed` with "Nocturne was closed during the apply
— check these devices" and the devices listed.

## Approval and autonomy

`changes.approve` is the human's signature on specific plans: it must name
the `plan_hash` of every target (so the UI cannot approve something the user
did not see), and for each target with protected hits `confirm` must contain
the device's name typed out. A re-plan invalidates the approval.

`changes.assistant_apply` is the only apply the AI assistant's tool set can
call. It applies without human approval **only** when the `assistant`
settings section (declared by this crate if absent:
`{autonomy: "draft"|"lab"|"low_risk"}`, default `draft`) allows it:

* `draft`: never — returns `forbidden` telling the model to ask the user to
  review the change in the Changes screen;
* `lab`: only when every target is a lab device;
* `low_risk`: lab devices, or real devices when every plan is `low` risk with
  no protected hits and the device is not `protected`.

Whatever the mode, protected hits, protected devices, high/critical risk and
profiles that are not `verified` always need the human. An auto-applied
change records `approval.by = "policy:<mode>"`.

## Audit

Create, plan, approve (with the hashes), apply start/finish per device,
rollback, cancel — each an `app.audit` entry carrying the change id; the
applied commands are part of the change record itself and are never deleted
while the change exists.

## Tests (against `nocturne_app::testing` fake devices unless noted)

Planning: `Lines` and `Intended` (exact and additive) produce the expected
commands, rollback and predicted diff; trunk allowed-VLAN delta uses
`add`/`remove`; a user-typed replacing list is a protected hit; management
SVI / default gateway / VTY hits; `Dangerous` refused; orphan sub-command
refused; unsupported syntax refused for `Intended`. Apply: happy path
(backup before and after, saved, transcript redacted, audit); drift between
plan and apply → `conflict`; a rejected line (`reject`) → rolled back and
the device's running-config equals the before version byte for byte;
verification failure (make the fake lose a neighbour via canned outputs, or a
ping fail via `unreachable`) → rolled back; session cut mid-apply
(`drop_on`) with a guard armed → `guard_fired`, guard not disarmed, not
saved; without a guard → `failed` + reconnect + rollback when the device is
reachable again; guard armed and disarmed around a clean apply and never
saved while armed; `stop_on_failure` over three devices; parallel lanes;
cancel mid-apply; restart recovery of `applying`. Approval: wrong or missing
hash, missing typed confirmation, re-plan invalidation. Autonomy: each mode ×
lab/real × risk/protected matrix. On-demand rollback of an applied change.
Property test: for random pairs of small IOS configs A and B, planning
`Intended{B, exact}` on a device with A and applying it yields B, and rolling
back yields A.
