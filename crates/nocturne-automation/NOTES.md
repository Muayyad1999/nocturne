# nocturne-automation — build notes

Spec: `docs/dev/features/automation.md`. Conventions: `docs/dev/features/README.md`.

## State: complete

145 tests (20 unit + 125 integration, all but one through `App::rpc`), `cargo clippy -p nocturne-automation
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
  render.rs     strict MiniJinja + the networking filters + `fail(…)`; the
                render context; the bounds on one rendering and one call
  templates.rs  variable kinds, CRUD, validate, render, apply
  builtins.rs   16 built-in templates, plus
  builtins/qos.rs  the 5 QoS ones (tag `qos`)
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
  qos_templates.rs   the QoS built-ins: defaults, refusals, hostile values,
                     the simulator against the planner's prediction
  render_bounds.rs   what the built-ins take on a 48-port switch, against the bounds
```

## Decisions worth remembering

* **A rendering is bounded** (`render.rs`): 1 MiB written and 500 000 steps of
  MiniJinja fuel per device, 16 MiB for one call (`Budget`, shared by a
  playbook run's devices, which render at the same time: a share is taken
  before and the unused part given back). Before, a 1 MiB variable over ten
  thousand interfaces was ten gigabytes in one `String`, then copied, on a
  runtime worker. The output goes through a writer that refuses the write
  past its share, so the text is never built whole; fuel also charges for the
  data a template makes on the way (one step per 64 bytes), which bounds what a
  `{% set %}` or a macro can hold. The workspace's `minijinja` does not enable
  `fuel`; this crate adds the feature on its own dependency line (features
  are additive, `Cargo.lock` does not record them). A failed rendering keeps
  no state to read its fuel from, which is why there is no per-call fuel
  budget: the time a call takes is bounded per device instead. Every render
  runs on the blocking pool. Measured on a 48-port switch: 3 935 steps and
  3 447 bytes at most among the built-ins (`tests/render_bounds.rs`).
* **A run never writes back the copy it started with.** Schedules, triggers
  and tasks record a run when it starts and again when it ends, each time by
  reading the stored record under `AutomationState::records` and changing
  only the run's own fields; save and delete take the same lock. Before, a
  schedule deleted or switched off during a 20-minute backup was put back,
  enabled, when the backup ended.
* **The slot is spent before the action is called** (schedules; for triggers
  the burst and the count). A process killed by its own action (out of
  memory) used to come back to a schedule that was still due, fire it within
  the first tick and die again, every start for the catch-up week.
* **What the last process left half-done is closed at start**, before the
  scheduler and the triggers: a run with no `finished_at` (the core has
  already failed its job) gets every device `skipped` with
  `runner::CUT_SHORT`, and `cancelled`; a schedule recorded as started with
  no outcome gets `last_ok: false` and `schedules::INTERRUPTED`; a task whose
  last run that was gets `last_ok: false`. A trigger's open debounce window
  is on disk (`automation.trigger_pending`) and comes back.
* **A copy of a live data directory is a crash.** The restart tests hold an
  action open, copy the data directory (the SQLite WAL with it, not the lock
  file) and open a second app on the copy: exactly what a process that dies
  at that moment leaves, without killing anything.
* **Job events are automation's own.** Every allowed action starts a job and
  the core publishes `jobs.changed` for it; a trigger on `jobs.changed` with
  no filter or `{state: succeeded}` fired four times a second. `jobs.` is in
  `OWN_TOPICS`, a pattern that covers any own prefix (`*`, `t*`) is refused,
  and the listener ignores own topics whatever a stored trigger says. Not
  caught: a trigger whose action publishes what it watches by another route
  (`devices.state` → `collect.run`, which updates the device's state). The
  bus carries no cause, and a burst that arrives during a run is held for
  after it (below), so such a trigger runs back to back until it is switched
  off. Telling those events apart needs a cause on the event, which is the
  core's to add.
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
  `playbooks.`) or job events (`jobs.`): that is a loop, and it is refused at
  save time. `pass_device` reads an event's `/id` only on `devices.*` topics.
* **A value is one line.** A `string` (unless `multiline`, which only the
  banner is), a `choice`, an `interface` and every `list` item refuse control
  characters in `coerce`: a line break in a value was a second configuration
  line (`vlan_name: "USERS\ninterface …\n shutdown"` rendered three). One
  line can still be two words, so the older built-ins check the shape of what
  they put into a command (`builtins::WORDS`), as the QoS ones always did.
* **The device type a template checks is the one it renders with**: the
  device's own profile, else the detected one (`render::profile_of`); a device
  with neither is refused by a template that names profiles. `templates.apply`,
  a playbook's `template` step and `ansible.export_playbook` share
  `templates::written_for`. Before, a device left to detection took the IOS and
  the RouterOS QoS templates alike, and a playbook step checked nothing.
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

## QoS templates (`builtins/qos.rs`)

Contract: `docs/dev/spec-qos.md` ("Changing QoS") and the table in
`docs/dev/features/automation.md`. Five built-ins, tag `qos`.

* **Where every line comes from.** Read on 2026-11-22 and named in each
  template's `description` and at the top of `tests/qos_templates.rs`: Cisco's
  MQC guide ("Applying QoS Features Using the MQC"), the congestion-management
  guide ("Low Latency Queueing with Priority Percentage Support"), the IOS QoS
  command reference (`bandwidth`, `match dscp`, `priority`, `shape`), the
  Catalyst 3750 and Catalyst 2960/2960-S software configuration guides for
  12.2(55)SE ("Configuring QoS"), and MikroTik's "Queues", "HTB", "Queue
  Burst", "Connection rate", "Mangle", "Common Firewall Matchers and Actions"
  and "Packet Flow in RouterOS". The default code points per class are RFC
  4594's (figure 3), with CS3 added to critical data because Cisco auto-QoS
  marks call signalling with it. **Nothing was checked on a real device.**
* **`fail(…)` rather than more variable kinds.** A percentage over 100 could
  have been a `min`/`max` on the variable; three percentages that leave best
  effort nothing, or a guarantee above its limit, could not. One function in
  the body covers both, keeps `Variable` (and the interface's types) as they
  were, and travels with a duplicated template. The price is that the checks
  are Jinja, at the top of each body, and long.
* **MiniJinja lists its own functions among the names a body reads**
  (`undeclared_variables` "does not special case global variables"). Before
  this, a body that called `range()` or `namespace()` was refused as using an
  unknown name and could not be saved. `render::provided` filters the
  environment's globals out, in the pre-flight check and in `templates.validate`.
* **`exit` between classes.** `nocturne_drivers::hier::ConfigTree::apply`
  pushes a nested opener (`class X`) onto the open path when one is already
  open, so `class A … class B` predicts B *inside* A; a real device (and the
  simulator) treats B as A's sibling. The change pipeline compares the device
  with that prediction after applying, so the WAN policy types `exit` after
  each class, exactly as Cisco's own example does. A test holds this
  (`the_wan_policy_is_predicted_as_a_nested_policy_by_the_config_engine`).
  Hand-written QoS lines without the `exit` will still be mispredicted: that
  is the engine's to fix, not a template's.
* **`priority percent`, never a bare `priority`.** Cisco's command reference
  (`bandwidth`, "Configuring Strict Priority with Bandwidth"): beside a
  strict-priority class no other class may have `bandwidth`. The guide's
  example pairs `priority percent` with `bandwidth percent`; so does this.
* **99, not 100.** The LLQ guide: the percentages "cannot exceed 99 percent of
  the total bandwidth available on the interface". Releases before HQF
  (12.4(20)T) reserve 75 % by default: the command reference says so under
  `max-reserved-bandwidth` (the same page as `match dscp`, qos-m1.html), with
  the device's error message when a policy asks for more. That is not
  enforced, because the template cannot know the release and 75 would be
  wrong for everything newer; it is said in the description and in the help
  of the last share, the device refuses loudly at `service-policy output`,
  and the defaults (10 + 20 + 25) stay under it.
* **A line is rendered the way the device shows it afterwards.** The change
  pipeline's configuration check compares every line of the device, with the
  section it is in, against what `hier` predicted from the lines sent
  (`nocturne-changes`, `compare::lines_of`), and rolls back on a difference.
  The simulated router stores a DSCP as a number and writes it back by name
  where IOS has one, a class map's values ascending: `match dscp ef 44` comes
  back as `match dscp 44 ef`, `set dscp 0` as `set dscp default`. Rendered as
  typed, a change that had worked was rolled back as one that had not. So the
  IOS bodies start with `IOS_DSCP` (the names, `code_of`, `spelled`) and
  render the canonical form; the default policy name of the access-port
  template follows it (`MARK-DSCP-EF` for 46). A test types every IOS
  template into the simulator and requires its configuration to be exactly
  the prediction. **What a real IOS prints was not checked**: the vendor
  pages show what is typed, not what `show running-config` gives back, and
  the reviewer remembers (without a capture to show for it) releases that
  print `match  dscp` with two spaces. If a device differs, the fix is in how
  `hier` normalises such a line, not in a template.
* **The code point, not the spelling, is what is listed twice.** `46` in one
  class and `ef` in another are one marking; comparing texts let that
  through. `code_of` gives the number for both.
* **An IOS interface is its full name** (`builtins::IOS_INTERFACE`, shared by
  every IOS built-in that takes one). The `interface` kind only refuses a
  second line, and `ifname` expands only its table: `Gigabit0/2`,
  `TenGig1/1/1` and `Fast0/1` came out as typed, IOS took them and showed the
  full name, the planner predicted a section that never exists, verify failed
  and the rollback sent `default interface Gigabit0/2` — which reset the real
  uplink. The macro accepts a type written out in full (any case; it is
  written the router's way) and its number, and refuses the rest, naming
  the value. A type missing from its list is refused, not guessed: a copy can
  add it. The kind itself was left alone: a user template may be given
  `range …` on purpose (the planner refuses it later, with its own reason).
  Expanding prefixes in `ifname` itself, and the predictor finding the
  section an abbreviation names, are the drivers crate's to do.
* **One ceiling for every rate, 128 Gbit/s** (`MOST_KBPS`). Without it
  `rate_kbps: 9223372036854775807` rendered `max-limit=9223372036854775807k`
  and guarantees computed in 128-bit arithmetic. It is a sanity bound, not
  the platform's: the command reference gives `shape average` 1 000 to
  1 000 000 000 bit/s on IOS 15.2(1)T (qos-s1.html, `shape (policy-map
  class)`), and IOS XE routers with 10-gigabit ports take more. Enforcing
  1 Gbit/s would refuse lines such a router accepts, so the documented range
  is in the variable's help and the device has the last word.
* **A share of a small circuit can be less than a class may have.** The
  command reference gives `bandwidth` 1 to 2 000 000 kbit/s, 8 at least on
  15.1(1)T, and `priority` 1 to 2 000 000, and says that a `bandwidth
  percent` which comes to less than the range keeps the policy from being
  attached (CSCsy73939: "Configured Percent results in out of range kbps").
  With `shape_kbps` the WAN template knows what a percentage comes to, so it
  refuses 20 % of 39 kbit/s; it takes the stricter 8 because it cannot know
  the release, and a guaranteed class of 5 kbit/s is no loss. The upper end
  is not checked: it is the classic-IOS figure, and a share of a 10-gigabit
  circuit is above it on hardware that takes it.
* **The simple queue takes IPv4 only.** "Queues" shows `target=` with an IPv4
  address and says nothing of IPv6; whether a given RouterOS release takes an
  IPv6 target was not established, so the body refuses one (a copy can drop
  the check).
* **A queue-tree prefix may not be the interface's name** (or `global`). The
  parent queue is called after the prefix and the leaves say `parent=<prefix>`;
  with an uplink renamed `wan` and the default prefix `wan` that names two
  things. How RouterOS resolves it was not tried; refusing costs nothing.
* **A refusal is bounded** (`render::refusal`): 400 characters, control
  characters turned into spaces. `fail(label ~ value)` with a megabyte of
  value gave a megabyte of error per device, line breaks and escape
  sequences included.
* **`mls qos` is never implied.** On the 2960/3750 family the port commands do
  nothing until QoS is on globally, and turning it on makes every untrusted
  port rewrite DSCP to 0. So the access-port template adds `mls qos` only when
  `enable_qos` is set, and a test fails if it ever slips in.
* **Which Catalysts.** Only the families whose guides were read: 2960, 2960-S,
  3750 (12.2(55)SE). The 3560 runs the same software, but its guide now
  redirects to a product page, so it is not named. The 3850 and 9300 guides
  were read only to say what the template is *not* for.
* **Rates are whole kbit/s everywhere**: IOS `shape average` gets `× 1000`
  (it takes bit/s), RouterOS gets the `k` suffix ("Queue Burst" writes
  `1500k`). The 8 kbit/s floor is the 8000 bit/s the command reference gives
  for shaping before 15.2(1)T; the 128 Gbit/s ceiling is only a sanity bound.
* **The queue tree marks everything.** A queue tree on an interface only
  holds packets that reach a leaf. So the first mangle rule marks every packet
  leaving the uplink as best effort (`passthrough=yes`) and the DSCP rules
  re-mark the classes. That replaces packet marks set earlier on that traffic,
  which the description says. Best effort's `limit-at` is the remainder, not a
  rounded share, so the guarantees add up to exactly the parent's rate.
* **No `comment=` on the queues**: it is not on the pages read for
  `/queue simple` and `/queue tree`, and free text needs quoting rules that
  are not there either. **The mangle rules do carry one**, `<prefix>-qos`
  (one word, so no quoting): "Common Firewall Matchers and Actions" lists
  `comment` for a rule. A rule has no name and is added again on every run;
  the queues, whose names are unique, now come first, so a second run is
  refused at its first line before a rule is duplicated, and the comment is
  how the rules of a run that stayed can be found.
* **`place_before` on the subscriber queue.** Simple queues are tried in order
  and a new one goes last, so a subnet or catch-all queue above it takes the
  subscriber's traffic. The variable takes a queue's name, because a number
  exists only after a `print` in the same session ("Command Line Interface").
* **The simulator covers the IOS templates except `mls qos …`** (it has no
  Catalyst commands and answers them with `% Invalid input detected`). The
  RouterOS templates have nothing to be typed into: there is no RouterOS
  simulator.
* **The simulator is not a gate.** Typed by hand it takes `priority percent
  60` beside `bandwidth percent 60`, and `match dscp ef` in two classes,
  without a word. In a lab the template's own checks are the only ones.

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
* **`nocturne_drivers::hier` nests a `class` typed inside a `class`** (see
  "QoS templates" above). The templates work around it with `exit`; lines a
  person types will not.
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
