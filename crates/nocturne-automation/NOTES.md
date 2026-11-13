# nocturne-automation — build notes

Spec: `docs/dev/features/automation.md`. Conventions: `docs/dev/features/README.md`.

## State: complete

93 tests (19 unit + 74 through `App::rpc`), `cargo clippy -p nocturne-automation
--all-targets -- -D warnings` clean, `cargo fmt -p nocturne-automation --check`
clean, no `unsafe`. A real `ansible-playbook` was run during development (see
"Ansible" below).

```
src/
  lib.rs        init(): settings, state, routers, on_start (scheduler + triggers)
  model.rs      contract shapes: Selector/Filter, Run/DeviceResult/CommandResult/
                StepResult, Task/Action, Template/Variable, Playbook, Schedule/When,
                Trigger, ActionCall, AnsibleStatus
  settings.rs   the `automation` section
  state.rs      the device lease and the "already firing" guard — the no-overlap rule
  clock.rs      Clock trait; SystemClock + VirtualClock (follows paused tokio time)
  select.rs     Selector → Vec<Device>, resolved at run time
  db.rs         collections, run history, retention
  runner.rs     fan_out (bounded concurrency, cancel, lease, tally), the read-only
                gate, run_commands, runner.* RPC
  compare.rs    grouping with volatile-line trimming, an LCS line diff, search, export
  render.rs     strict MiniJinja + the networking filters; the render context
  templates.rs  variable kinds, CRUD, validate, render, apply
  builtins.rs   16 built-in templates
  playbooks.rs  the YAML model, validation, the per-device walk, the one change
  tasks.rs      saved tasks; tasks.run is the one name a schedule/trigger points at
  cron.rs       five-field cron and `daily`, in local time, DST included
  schedules.rs  the allow-list, the scheduler task, catch-up-once
  triggers.rs   event topics, payload filters, debounce, no overlap
  ansible.rs    status, export_playbook, run (env-only credentials, 0600 inventory,
                streamed output, PLAY RECAP per host) and the caveats
tests/
  common/       harness: core + this feature + stubs for changes.create/get and
                config.backup, a virtual clock, paused-time tick helpers
  runner.rs templates.rs playbooks.rs schedules.rs triggers.rs tasks.rs ansible.rs
```

## Decisions worth remembering

* **One device belongs to one run at a time** (`state.rs`). `fan_out` claims a
  lease per device before the work and drops it after; a device that is taken
  becomes a `skipped` result naming the run that holds it, and no second
  session is opened. The same lease is taken by the Ansible path before it
  starts its process. This is also what makes "a schedule that fires while the
  previous run is still going" safe twice over: the schedule itself is also
  guarded (`start_firing`), and `fire` **waits for the job** the action
  started, so a `runner.run` that returns a job id at once does not let the
  next tick start the same work again.
* **A run never shrinks.** `results.len() == devices selected`, always:
  `ok` / `failed` / `unreached` / `skipped`, with `unreached` reserved for "a
  session could not be opened" and `skipped` carrying a `reason`. A cancelled
  run marks the devices it never got to as `skipped`, and the job ends as
  `cancelled` rather than `succeeded` (returning `Ok` from a cancelled job
  would make the core record it as a success — see `jobs::finish`).
* **Selection is a filter resolved at run time**, never a stored list of ids.
  `exclude_device_ids` is tolerant (an excluded device that no longer exists is
  fine); a named device that does not exist is an error, because a typo must
  not quietly shrink a run.
* **No built-in template takes a secret.** The specification asks for a local
  user and an SNMPv3 user; both would need a password or an auth key, and a
  rendered line goes into a change document, into job output and onto a screen.
  Rule 3 wins: those two are not shipped. A test asserts that no built-in
  mentions `password`, `secret`, `community`, `passphrase` or `private-key`.
  (If they are wanted later, the vault would have to render into the change at
  apply time, redacted everywhere else — a change-engine feature, not a
  template feature.)
* **Strict undefined needs its own MiniJinja environment.** `nocturne-drivers`
  `expr::Expressions` keeps its environment private and uses *chainable*
  undefined, which is right for parsing device output and wrong for writing
  configuration. `render.rs` builds a strict environment and re-registers the
  networking filters over the drivers crate's public helpers
  (`parse_duration`, `parse_bps`, `parse_bytes`, `normalize_mac`, `to_cidr`,
  `ifname::long`), plus a `wildcard` filter for IOS-style ACL masks.
  Playbook `when`/`assert`/`set` expressions do use the drivers' own
  `Expressions`, where chainable undefined is what you want.
* **The variable that is undefined is named** by checking
  `Template::undeclared_variables` against the context *before* rendering.
  MiniJinja's strict-undefined error cannot say which name it was without the
  crate's `debug` feature, which the workspace does not enable.
* **`cron` and `daily` walk local wall-clock minutes**, not absolute ones. That
  is what settles the two odd days of the year: the hour that does not exist in
  spring is skipped (a 02:30 job does not run that day), and the hour that
  happens twice in autumn fires once, at its first occurrence. Both `daily` and
  `cron` go through one `Spec`, and `dom`/`dow` use Vixie cron's OR rule when
  both are restricted.
* **Catch up once, from now.** A schedule that was due while the process was
  not running fires once at the next tick and then computes its next run from
  *now*; a week of missed nightly backups is one backup. Past the
  `catch_up_within_s` window it does not run at all, it just moves on.
* **A trigger holds its burst rather than losing it.** If a trigger's previous
  run is still going when its debounce window closes, the pending burst is
  re-armed instead of dropped. A trigger may not watch the topics this crate
  publishes (`automation.`, `schedules.`, `triggers.`, `tasks.`, `templates.`,
  `playbooks.`): that is a loop, and it is refused at save time.
* **A playbook's `template` steps accumulate per device and become one change
  at the end of the run**, with a target only for the devices that finished —
  a device that failed half way does not get its half-rendered lines into a
  change. If `changes.create` is not in the build, those devices are marked
  `failed` with that as the reason rather than quietly succeeding.
* **The playbook walk drops its CLI session before a `backup` step**, because
  the core allows one automated session per device and `config.backup` opens
  its own. It then waits for that job before going on.
* `runner.compare` trims "volatile" lines (uptime, timestamps, `Building
  configuration`, …) before grouping, and returns the patterns it used, so
  nobody is fooled by a difference that was trimmed away. It also returns
  `missing`: the devices in the run that have no output to compare, and why.
* The line diff is a longest-common-subsequence over the part that differs,
  with a size guard (`DIFF_LIMIT`); beyond that the answer is "all of it
  changed", which is true and cheap. `similar` is in the workspace but adding
  it here would have been the only new entry in `Cargo.lock`, and this is 60
  lines.

## Ansible: what it can and cannot promise

`ansible.rs`'s module documentation is the long version; the short version is
that the interop is worth having and is not a change.

**It does give you**: the inventory generated from this inventory (same
filters), credentials passed **only** through the child process's environment
and referenced as `{{ lookup('env', 'NOCTURNE_PW_1') }}`, a temporary inventory
at mode 0600 in a 0700 directory that is removed however the run ends,
`--check` by default, live output streamed into a cancellable job (cancelling
kills the process), the known secrets scrubbed out of that output, and a
per-device result parsed from `PLAY RECAP` — so a host Ansible could not reach
is `unreached` like any other device Nocturne could not reach.

**It does not give you** any of the change pipeline's guarantees: no backup, no
drift check, no computed plan and no approval bound to its hash, no
verification, **no rollback**, no risk or protected-resource assessment. What
the playbook changed is known only from what it printed; Nocturne's inventory,
collected data and configuration history are not updated by it. Idempotence and
what `--check` means are the module's business. Host-key checking is Ansible's,
with Ansible's configuration — the known-hosts store here is not consulted.
`ansible_become_method: enable` and `ansible.netcommon.network_cli` both come
from the `ansible.netcommon` collection, so a device with an enable secret or a
mapped device type needs that collection installed. Every run says all of this:
`CAVEAT` is the first line of the job log, it is on the run record, and it is in
`ansible.status`.

`ansible.export_playbook` maps only the platforms whose module and argument are
certain (`cisco_ios`, `cisco_nxos`, `cisco_xr`, `arista_eos`, `vyos`,
`juniper_junos`, `mikrotik_routeros`); anything else is an `unsupported` error
naming the platform rather than a guess. A missing `ansible-playbook` is a clean
`unsupported` with the reason too, never a failure.

## Needed from the core (worked around for now)

* **No way to ask "is this command read-only on this device" without a
  session.** `connect::exec` and `Cli::read` both gate correctly, but the
  up-front refusal ("nothing was sent to device 1 because device 40 is where
  `clear counters` is a change") has to reach for
  `profiles::platform(...).policy().classify(...)` itself, and for a device
  whose type is unknown until it answers there is nothing to classify against —
  those commands are only gated when the session opens. A
  `connect::classify(app, device_id, command)` in the core would make this one
  call and one rule.
* **`nocturne_drivers::expr::Expressions` cannot be extended or borrowed.** Its
  `Environment` is private and its undefined behaviour is fixed, so a strict
  environment means re-registering the filters (`render.rs`). Either a
  constructor taking `UndefinedBehavior`, or a public
  `expr::register_filters(&mut Environment)`, would remove the duplication —
  and would stop the two environments drifting apart, which is the real risk.
* **MiniJinja's `debug` feature is not enabled in the workspace**, so a
  strict-undefined error cannot name the variable; hence the pre-flight check
  in `render.rs`. Worth enabling if another crate ever wants good template
  errors.
* **`jobs::JobCtx::for_each` gives no way to say "this item was not started"**
  other than `Err(cancelled)`, which is also what a real cancellation looks
  like. It works out here (both become `skipped` with a reason) but the
  distinction is lost.
* **A job's `Ok` result beats its cancellation** in `jobs::finish`, so a job
  that wants to record results *and* end as cancelled has to store them itself
  and then return `Err(jobs::cancelled())`. Both the runner and the Ansible
  path do that; it is easy to get wrong.
* `collect::run_on` takes `&App` and `&mut Cli`, which cannot both be borrowed
  out of one struct — `playbooks.rs` clones the `App` first. Minor.
* There is no core helper for "run an RPC method and wait for the job it
  started"; `schedules.rs` does it by hand (look for `job_id`, then
  `jobs::wait`). Several features will want that.

## If you pick this up next

* The assistant can reach all of this by method name already; nothing needs
  registering here.
* `ui/src/api/types/automation.ts` is written and type-checks; no screen exists
  yet. `tasks.devices` is there specifically so a task editor can show what a
  filter currently means before anybody saves it.
* `reports.generate` and `compliance.run` are on the schedule allow-list but
  are not in this build; a schedule naming them is refused at save time with
  that as the reason, and will start working when those features land.
