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
profile `config.guard` and `config.secrets`), `nocturne-ai` (`Redactor`),
core `connect::cli` (+ `Cli::session()` — this is the only crate allowed to
use it), `collect`, `jobs`. Config versions are stored through
`store.config_insert`; pin them with RPC `config.pin` when that method
exists (tolerate its absence).

## Model (contract)

This is the record as it is **kept**. What a method answers is a view of it
— see [Secrets](#secrets-contract) for which fields a view changes.

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
    job_id: Option<String>,                           // the job planning or applying it, or that last did
    undoes: Option<String>,                           // set by `changes.rollback` on the undo it creates: the change it undoes
    undone_by: Option<String>,                        // on that change: its latest undo (see On-demand undo)
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
    risk: Risk,                          // `reasons` hold no secret value (`<hidden>`)
    protected_hits: Vec<ProtectedHit>,   // why this needs a typed confirmation; `command` is a line, `subject` and `why` hold no secret value
    warnings: Vec<String>,               // hold no secret value (`<hidden>`)
    base_version_id: i64,                // the running-config backup the plan was computed against
    base_sha256: String,
    guard: Option<GuardPlan>,            // the dead-man switch that will be armed, if any
    plan_hash: String,                   // sha256 over device, commands, rollback, base_sha256, options
    planned_at,
    predicted_sha256: String,            // sha256 of the predicted configuration ("" = it could not be predicted)
    profile: String, maturity: String,   // the device type the plan was computed for, and how far it has been checked
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

struct TargetResult {                                 // nothing in here holds a secret value: `<hidden>` stands where one was
    state: pending|applying|applied|failed|rolled_back|rollback_failed|skipped|guard_fired,
    started_at, finished_at,
    steps: Vec<Step { name, ok, detail, at }>,       // connect, drift check, backup, arm guard, apply, verify, rollback, disarm guard, save, final backup
    sent: Vec<CommandOutput>,                         // every configuration line that was sent with the device's answer, values hidden (the planned lines are whole in the plan)
    failed_line: Option<usize>, error: Option<String>,
    before_version_id: Option<i64>, after_version_id: Option<i64>,
    verification: Vec<Check { name, ok, detail }>,
    transcript: String,                               // session transcript, values hidden
    redacted: bool,                                   // written down with its values hidden: `true` for every result recorded since the engine hides them, and always `true` in an answer
}
```

## RPC (contract)

| Method | Params → Result |
|---|---|
| `changes.list` | `{state?, device_id?, origin?, limit?, cursor?}` → `[ChangeSummary]` (no plans/results/transcripts, so no configuration lines) newest first |
| `changes.get` | `{id, redact?: true}` → `ChangeView` = `Change` + `{redacted: bool, hidden: N}`. `redact: false` answers the lines exactly as they will be sent and is audited — see [Secrets](#secrets-contract) |
| `changes.create` | `{title, description?, origin?, ticket?, targets: [{device_id, intent}], options?, plan?: bool}` → `ChangeView` (state `draft`; `plan: true` also starts planning and returns `{change: ChangeView, job_id}`). An intent that carries a redaction placeholder is `invalid`. |
| `changes.update` | `{id, title?, description?, ticket?, targets?, options?}` → `ChangeView`. Only in `draft`/`planned`; any change to targets or options discards the plans and the approval. An intent that carries a redaction placeholder is `invalid`. |
| `changes.plan` | `{id}` → `{job_id}`; fills `target.plan` for every target, state → `planned`. A finished change (`failed`, `rolled_back`) planned again starts over: its results and its `undone_by` are cleared and it is a `draft` until its plan is in. `conflict` (`details.change_id`) while an undo of it waits, or, for an undo, while another undo of the same change waits — see [On-demand undo](#on-demand-undo) |
| `changes.approve` | `{id, plan_hashes: [..], confirm?: {device_name: typed_text}, note?}` → `ChangeView` (state `approved`) |
| `changes.unapprove` | `{id}` → `ChangeView` (state `planned`) |
| `changes.apply` | `{id}` → `{job_id}` — requires `approved` with hashes still matching |
| `changes.rollback` | `{id, device_ids?}` → `{change: ChangeView, job_id}` — sends nothing: creates the change that undoes this one (state `draft`, `undoes` set) and starts planning it (`job_id`); that change is approved and applied like any other. `invalid` when `id` is itself an undo (an undo is not undone in turn). See [On-demand undo](#on-demand-undo) |
| `changes.cancel` | `{id}` — draft/planned/approved → cancelled; applying → cancels the job (the device in flight completes or rolls back; none is left half-configured) |
| `changes.delete` | `{id}` — not while applying; the audit trail keeps the record that it existed |
| `changes.preview` | `{device_id, intent, options?, redact?: true}` → `PlanView` = `Plan` + `{redacted: bool, hidden: N}`, without creating anything (the "what would this do" behind the quick-configure box). `redact: false` is audited. |
| `changes.assistant_apply` | `{id}` → `{job_id}` or `forbidden` (`details.needs: "human_approval"`) — a planned or an approved change, the same decision for both; see Autonomy |

Every `ChangeView` answered by a method other than `changes.get {redact:
false}` is the redacted one.

Events (contract): `changes.changed {ChangeSummary}` on every state change;
`changes.progress {change_id, device_id, step, ok, detail}` while applying
(an undo included: it is applied by `changes.apply` like any change). `detail`
is the step's detail as it is recorded: it holds no secret value.

## Secrets (contract)

A configuration is full of secrets that are no vault credential — enable
secrets, communities, routing, RADIUS and TACACS keys, pre-shared keys — and a
change is made of configuration lines. A change that only renames a port
still carries, in three lines of diff context and in the detail of a failed
check, whatever stands next to the line it touches. `src/redact.rs` is the
one place that decides what text of a change leaves the engine, and it leaves
in one of two forms.

**The lines themselves** — `intent.lines` / `intent.config`,
`plan.commands`, `plan.rollback`, `plan.diff.lines[].text` and
`plan.protected_hits[].command` — are kept exactly as they will be sent:
they are what is hashed, approved and sent, and what a re-plan starts from.
They are never answered as they are kept. Every method answers a **view**:

* by default each secret value in those fields is replaced by a placeholder,
  `<secret:3>` (`<secret:3:weak>` for a notorious default such as `public`).
  There is one table per answer: the same number is the same value on every
  line and on every device of that answer, and means nothing in another
  answer. The view says `redacted: true` and `hidden: N`, the number of
  different values hidden in it; `hidden: 0` means the lines shown are
  exactly the lines that will be sent;
* `changes.get {id, redact: false}` and `changes.preview {…, redact: false}`
  answer those fields whole (`redacted: false`, `hidden: 0`). This is how a
  person sees the exact plan they approve, and it is on record: an audit
  entry `changes.view_unredacted` for each device whose lines hold a value
  (in that device's own trail, detail `{change_id, values}`), or one entry
  without a device when nothing in the change was hidden. A preview writes
  one entry for its device (detail `{values}`). No other method answers the
  lines whole;
* `plan_hash` is computed over the kept lines and is the same in both views.
  An approval names that hash whichever view the person read, and what is
  sent is always the kept lines. Reading a change never writes to it.

**Everything said about the lines** holds no secret value at all, in any
view and at rest: `<hidden>` stands where one was. That is
`plan.warnings`, `plan.risk.reasons`, a hit's `subject` and `why`; the whole
of `TargetResult` (`steps[].detail`, `verification[].detail`, `error`,
`sent[]`, `transcript`); every error message and its details; the
`changes.progress` event; job log lines, job results and job errors; audit
details. It is written with the value already hidden — a detail names the
line and its section and says what differed (``missing from the device:
`line vty 0 4 / password <hidden>` ``), never the value — so an unredacted
read does not bring it back: `redact: false` is for the plan, not for the
record of what happened. It is `<hidden>` and not a numbered placeholder
because this text is read long after the table that numbered it is gone,
and a number from one table beside a number from another reads as "the same
value" when it is not. A result says `redacted: true` when it was written
this way; one that does not is a record from before this rule existed, and
is hidden each time it is read (it is not rewritten).

The rules for what is a secret are the engine's (`nocturne_ai::Redactor`)
plus the device type's own `config.secrets` patterns (the type the plan was
computed for; the device's own while there is no plan). Each line is read as
a line (several rules know a secret only by the word its line begins with):
a device type's patterns are applied in multi-line mode (`^` and `$` at every
line, `\r\n` a line end), so a whole configuration is read in one pass. A line
quoted in a sentence and a path such as `line vty 0 4 / password …` are read
whole and then part by part — whole first, because in a flat configuration
(RouterOS, Junos `set`, a file of `key=value`) a ` / ` stands inside a quoted
value (`password="a / b"`) and separates no sections. A command echoed after
a prompt in a transcript is read as if it began a line.
A value the rules recognised in the device's configuration or in the
change's own lines is also hidden where nothing around it says what it is —
in a device's answer, in an error — when it is distinctive enough not to be
an ordinary word. Over-redaction is the safe way to be wrong.

A placeholder is never sent. Hand-written lines that carry one
(`<secret:N>`, `<secret:N:weak>` or `<hidden>`), and an intended
configuration that carries a numbered one, are refused with `invalid` by
`changes.create`, `changes.update` and `changes.preview`: they were copied
out of a view, and planning them would configure the placeholder itself as
the community or the key. Planning refuses, with `invalid`, any computed
command that carries a placeholder of either form — so an intended
configuration read from a device that masks its own secrets may say
`<hidden>` on lines that are not going to change, and may not on one that
would be sent. To edit the lines of an existing change, start from
`changes.get {redact: false}`.

Audit details never quote a device and never hold a value: an apply's entry
carries the plan's hash, the planned commands with their values hidden and
the names of the steps that failed (`failed_steps`), not the error text; a
planning failure carries the device and the error code; a rollback carries
each device's state; a delete carries each plan's hash and its commands with
their values hidden.

What this asks of a caller:

* **A screen** shows the default view, rendering `<secret:N>` as it does for
  a stored configuration. When `redacted && hidden > 0` it offers, beside
  the plan and the approval, to show the hidden values: one
  `changes.get {id, redact: false}` made when the person asks, kept only for
  as long as that plan is on screen and never refetched by itself on a
  `changes.changed` / `changes.progress` event (each such read is an audit
  entry). `plan_hash` for `changes.approve` comes from either view. Steps,
  checks, errors, sent lines and transcripts are shown as they are;
  `<hidden>` in them is all there is.
* **Another feature that needs the real lines** (exporting a change as a
  playbook) must ask for `redact: false`, and must not use the lines of an
  answer that says `redacted: true` with `hidden > 0`: they are not the lines
  that would be sent.
* **The assistant** is given the default view. Its placeholders are numbered
  in this engine's table for that answer, not in the chat's: the assistant's
  gate has to treat `changes.get`, `changes.create`, `changes.update` and
  `changes.preview` as answers redacted elsewhere, so that a number copied
  out of one is never restored to a secret of the chat.

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
   Any failed check → rollback. Where the result could not be predicted (a
   grammar the engine cannot apply commands to: RouterOS, Junos `set`), the
   configuration check fails — unless the intent is an exact intended
   configuration (a restore, an undo), which says what the device must be
   afterwards: then the device is compared with that configuration instead.
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

## On-demand undo

The rollback inside an apply (step 7) is part of the plan that was approved
and is unchanged. Undoing an applied change *later*, on demand, is a change
like any other: weeks after it the device has moved on (other changes,
somebody at the console), and the way back is new configuration in everything
but name. So `changes.rollback` sends nothing. It creates a change that:

* has, for each chosen device, the intent `Intended { config: <the version
  backed up before the original change>, exact: true }`: the planner works the
  commands out from what the device holds *now*, so a line somebody added
  after the original change is taken out as well;
* is titled `Undo: <title>`, `origin: restore`, `created_by: user`, with the
  original's options (so it has to pass the same checks) and ticket (a
  back-out runs under the change request it backs out), `undoes:` the
  original's id; its description names each device and the version it goes
  back to;
* is planned at once by the normal planner (`job_id` in the answer), and then
  read, approved — bound to its hash, with the typed confirmation when it
  touches a protected resource — and applied through `changes.apply` like any
  change: drift check, backup, dead-man switch, send, verify, and a rollback
  of the undo itself if it does not verify.

Its plan says two things no other plan does, as warnings: that the device has
changed since the original change left it (the lines added and removed since,
named without their values), or that Nocturne kept no copy of the device as
the change left it and cannot tell; and, where the result can be predicted,
that the commands do not put the device back exactly as it was. What it puts
on a device is the engine's: `changes.update` refuses new `targets` for an
undo (`invalid`); its title, description, ticket and options can be edited
like any change's.

Decisions:

* **Which devices.** A device the original change left changed — `applied`,
  `rollback_failed` or `guard_fired` — and whose configuration from before was
  backed up. `device_ids` narrows it to those named (by id, name or address).
  A device named that cannot be undone is refused with the reason (`invalid`):
  not one of the change's devices; not left changed (it was skipped, rolled
  back, failed, or never reached); no backup of what it had before was kept;
  that backup can no longer be read. When nobody named it, a device of the
  last two kinds is left out and the undo's description says why; one never
  changed is simply not offered. Nothing to undo at all is `invalid`. A change
  applied to some devices and not others (`partially_applied`, or rolled
  back on some) is undone on the devices it left changed only.
* **A backup that is gone.** The apply pins its backup (`config.pin`, when the
  build has it, "needed to roll back change …"), so retention keeps it. One
  that is gone all the same — unpinned and then pruned by retention, deleted
  by hand, or deleted with the device's history — is the "can no longer be
  read" case above, and nothing is guessed in its place.
* **A device whose configuration moved on** since the original change is the
  reason an undo is a change at all: its commands are worked out from what the
  device holds when the undo is planned, and its plan names what moved. If the
  device moves on again between the undo's planning and its apply, the apply's
  drift check refuses it like any change's; plan the undo again.
* **A device that cannot be reached** when the undo is planned leaves the undo
  in `draft` with the planning error, like any change; plan it again when the
  device answers.
* **One at a time.** While an undo of a change is not finished (`draft`,
  `planned`, `approved`, `applying`), asking for another is refused
  (`conflict`, naming it, `details.change_id`). Once it is cancelled, rolled
  back, failed or applied, another may be asked for. The same holds for
  planning again: an undo that failed or was rolled back may be planned again
  (it then becomes its original's `undone_by`), but not while another undo of
  the same change waits; and the original is not planned again while an undo
  of it waits, because its results are what that undo goes back from. Both
  are `conflict` with `details.change_id` naming the undo that waits. An undo
  planned again is a `draft` from that moment, so it waits from then on.
* **An undo of an undo** is refused (`invalid`): an undo is not undone in turn.
  It would make the original change again under an undo's exemption from
  `allow_unverified_writes` (below), and leave the original's record reading
  rolled back while its lines are back on the device. To have the original
  back, plan it again — a rolled-back change may be, and planned again it
  starts over, its `undone_by` cleared with its results — or create a change
  for what the devices should hold. Either is planned from what the devices
  hold now and approved like any change. The screen offers no "Roll back…" on
  an undo.
* **The original's state.** It stays as it is until its undo is applied. Then
  each device the undo applied reads, in the original, `rolled_back`, with a
  step `undone` naming the undo; the original's state becomes `rolled_back`
  when nothing of it is left on any device, `partially_applied` while a device
  still has it, `failed` otherwise (the rule an apply uses). A device the undo
  did not put back (its own rollback, a failure) stays as the original
  recorded it; the undo's record says what happened there.
* **The links.** The undo's `undoes` names the original; the original's
  `undone_by` names its latest undo (set when the undo is created or planned
  again, cleared when the original is planned again). Deleting an undo removes
  it from the original's `undone_by`. Deleting the original leaves the undo
  standing: it still takes the devices back.
* **The device type.** An undo is not held to `allow_unverified_writes`: a
  gate that can stop a person putting a device back as it was is worse than
  the risk it guards, and the undo's lines cannot be edited, so the exemption
  covers nothing else. The assistant is held to the device type all the same
  (Autonomy below).
* **The assistant** cannot call `changes.rollback` (it is on the never list of
  `nocturne-assistant/src/tools/boundary.rs`), and can start an undo only as
  it can any change: through `changes.assistant_apply`, past the autonomy
  gate.

The answer of `changes.rollback {"id":"6h5vpkt5gfgzwgpd"}`, copied from a
test run (one fake switch, SW1, whose desk port the original change had
renamed; no secrets in it, so `hidden` is 0):

```json
{"change":{"id":"43hw5s4mbp4w2h7o","title":"Undo: test change","description":"Takes `SW1` (version 1) back to the configuration each had before `test change`, by commands worked out from what it holds now. Nothing has been sent: read the plan, then approve and apply it like any other change.","origin":"restore","state":"draft","targets":[{"device_id":"gozte5oil4ehawr2","intent":{"type":"intended","config":"!\nhostname SW1\nip domain-name lab.example\nvlan 10\n name users\n!\nvlan 20\n name voice\n!\ninterface GigabitEthernet0/1\n description uplink\n switchport mode trunk\n!\ninterface GigabitEthernet0/2\n description desk 12\n switchport mode access\n switchport access vlan 10\n!\ninterface Vlan1\n ip address 10.0.0.2 255.255.255.0\n!\nip default-gateway 10.0.0.1\nline vty 0 4\n transport input ssh\n!\nend\n","exact":true},"plan":null,"result":null}],"options":{"save":true,"guard_minutes":null,"parallel":1,"stop_on_failure":true,"verify":{"config":true,"neighbors":true,"interfaces":true,"routes":false,"pings":[],"settle_s":5},"rehearse":false},"approval":null,"created_at":1795424396136,"updated_at":1795424396136,"created_by":"user","ticket":null,"job_id":"png3mlrvmoq4dhnb","undoes":"6h5vpkt5gfgzwgpd","undone_by":null,"redacted":true,"hidden":0},"job_id":"png3mlrvmoq4dhnb"}
```

Once planned (somebody had added `ntp server 192.0.2.99` since), its plan's
commands were `["interface GigabitEthernet0/2","description desk 12","exit","no ntp server 192.0.2.99"]`
with the warning ``"`SW1` has changed since `test change` left it (added since: `ntp server 192.0.2.99`). Undoing the change takes that back as well: what this puts back is the configuration from before that change."``.
The original then read `"undone_by":"43hw5s4mbp4w2h7o"` and stayed
`applied`; after the undo was applied it read `rolled_back`, its device
`rolled_back` with the last step
`{"name":"undone","ok":true,"detail":"undone by change `Undo: test change` (43hw5s4mbp4w2h7o), which put the device back to the configuration it had before this change","at":1795424396564}`.

The refusals, copied from a test run:

```json
{"code":"invalid","message":"`Undo: test change` is itself an undo, and an undo is not undone in turn. To have `test change` back, plan it again (or create a change for what the devices should hold): it is planned from what they hold now and approved like any other change."}
{"code":"conflict","message":"`Undo: test change` is waiting to undo `test change`: apply it, or cancel it, before planning `test change` again","details":{"change_id":"dafb767da2qqw6im"}}
{"code":"conflict","message":"`Undo: test change` already undoes `test change` and is planned: apply it, or cancel it, before planning this undo again","details":{"change_id":"aq7uaqhlejrpqhr3"}}
```

The first is `changes.rollback` of an applied undo; the second `changes.plan` of a
failed change (its own rollback failed) while its undo waits; the third
`changes.plan` of an undo that was rolled back while a later undo of the same
change waits. A rolled-back original planned again read, right after
`changes.plan` answered, `"state":"draft","undone_by":null` and no result,
and `"state":"planned"` once its plan was in.

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

The decision is the same for a change a person has already **approved**: an
approval is consent to that plan, not leave for the assistant to start it,
and grants no more than the setting does (approving and applying are two acts
in the Changes screen; a person approves in the afternoon what they mean to
apply in the evening's window, and the assistant's context holds text
strangers wrote). Refused, the assistant gets the same `forbidden` with
`needs: "human_approval"`, saying that a person has approved it and must
press Apply themselves. Allowed (say `lab` mode and a lab-only, low-risk
change), the approval stays as the person gave it — `approval.by` is not
overwritten — and the audit entry of the start says the assistant started it
and who approved it (`The assistant started applying change `…` (approved by
user)`, detail `{change_id, job_id, origin: "assistant", approved_by}`).

## Audit

Create, plan, approve (with the hashes), apply start (who started it and who
approved it) and finish per device, cancel, delete, and a read that asked for
the lines whole (`changes.view_unredacted`) — each an `app.audit` entry
carrying the change id; the applied commands are part of the change record
itself and are never deleted while the change exists. An on-demand undo
writes `changes.create` for the undo (`{change_id, origin, ticket,
undoes}`), `changes.rollback` for the original when it is asked for
(`{change_id, undo_id, devices: [{device_id, before_version_id}]}`, "…from
which nothing has been sent") and again when the undo's apply has put devices
back (`{change_id, undo_id, devices: [{device_id, state}]}`); a delete names
`undoes`. What an entry's detail may hold is under
[Secrets](#secrets-contract): no secret value and nothing a device said.

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
cancel mid-apply; restart recovery of `applying`; an exact intended
configuration on a grammar nothing predicts (a RouterOS-styled fake) checked
against itself, reached and not reached. Approval: wrong or missing hash,
missing typed confirmation, re-plan invalidation. Autonomy: each mode ×
lab/real × risk/protected matrix; an approved change refused to the
assistant under `draft` and still applied by the person; under the most
permissive mode an approved change on a protected device, with protected
hits, of high risk and on an unverified device type each refused; an
approved lab-only low-risk change started in `lab` mode with the person's
approval kept and the start audited as the assistant's; an approved change
whose approval does not name the stored plan (taken away while it is planned
again, or a stale hash) is started by neither. On-demand undo
(`tests/undo.rs`): asking sends nothing (no configuration session on the
device) and answers a planned change; its plan is the way from now back,
including a line added after the change, and says so; approving and applying
it restores the device byte for byte and makes the original read rolled back
with the links both ways; an undo that fails its checks is itself rolled back
and the original stays applied; an undo that touches a protected resource
needs the device named; the assistant cannot start it past the gate, planned
or approved; one open undo at a time, `device_ids` narrowing, and an undo's
lines not editable; a device the change did not leave changed (a change
applied to some devices and not others), or whose backup retention removed
(the version row deleted), refused by name and otherwise left out with the
reason; from the request until a person applies the undo the device is sent
nothing its device type does not class as a read (every command captured),
and then exactly the approved plan, byte for byte (the session's bytes
captured); an undo is not undone in turn, and the change it undid is made
again by planning it, which clears its `undone_by`; while an undo waits
neither the change nor another undo of it is planned again, and an undo
planned again is the change's undo and waits from that moment; a person
undoes a change on an unverified device type with `allow_unverified_writes`
off, and the assistant may not. Property
test: for random pairs of small IOS configs A and B, planning
`Intended{B, exact}` on a device with A and applying it yields B, and undoing
it on demand (planned, approved and applied) yields A.

Secrets (`tests/secrets.rs`, synthetic configurations): a device whose
configuration holds an enable secret, a type 7 password, a community, a
routing key and a line password beside the lines a change touches; plan,
approve, apply, with somebody else changing the enable secret and adding a
community in the middle so that verification fails on lines that are
secrets, then the rollback — and none of those values in any `changes.*`
answer made with default parameters, any error, any published event
(`changes.*`, `jobs.*`, `audit.appended`), any job record or the audit trail.
The same for a change whose own lines are secrets, applied, then undone on
demand on a device that refuses a line, then deleted; for a line refused at
planning and one rejected by the device; for what a plan says about its
lines as it is kept. The unredacted read returns the lines exactly and is
audited per device; the hash is the same in both views and is the hash of
the kept lines, which are what the device receives; a placeholder in an
intent is refused; one value has one number across the devices of an
answer; a record written before values were hidden is read without them.
The same sweep for a RouterOS export, a Junos `set` listing and a file of
`key=value`, each hidden with the patterns of the device type that ships for
it and holding values with a ` / ` in them, through the rollback the engine
derives from the device when a change to an unmodelled grammar cannot be
checked. What the device is typed in configuration mode is the planned lines
byte for byte (captured from the session); a plan is hashed as it was before
values were hidden (a pinned hash), and an approval stored before then still
applies.
