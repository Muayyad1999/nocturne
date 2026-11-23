# nocturne-changes — build log

Spec: `docs/dev/features/changes.md`. Conventions: `docs/dev/features/README.md`.

## Layout

```
src/model.rs      the contract types (Change, Target, Intent, Plan, Risk, …)
src/db.rs         persistence, summaries, listing, the per-device apply lock,
                  restart recovery
src/settings.rs   the `changes` section and `assistant.autonomy`
src/abbrev.rs     abbreviation / case refusal for `Lines` intents
src/compare.rs    comparing configurations as sets of lines per section
src/redact.rs     what text of a change may leave the engine, and in what
                  form: the view a method answers, and the hider everything
                  said about a change passes
src/protect.rs    the protected-resource survey and detector
src/risk.rs       risk level and the reasons behind it
src/plan.rs       planning over one CLI session (the only reader of a device)
src/verify.rs     the post-change checks
src/apply.rs      the apply pipeline
src/rollback.rs   undoing an applied change on demand: the undo as a change
                  (which devices, its plan's warnings, marking the original)
src/autonomy.rs   the gate in front of `changes.assistant_apply`
src/rpc.rs        the RPC surface
tests/            planning, applying, approval, orchestration, property,
                  secrets, undo
```

## Decisions

* **Prediction** (`ConfigTree::apply`, added to `nocturne-drivers/src/hier.rs`)
  models a device strictly: a leaf line goes into the section that is open;
  there is no "a global command silently closes the section" rule, because a
  real IOS has none. Plans built from `Intended` always carry explicit
  `exit`s (remediate emits them); `Lines` intents are validated, and an
  unambiguously global command typed inside an open section is refused with
  "put an `exit` first" rather than guessed.
* **Comparing configurations** (verification, the rollback check, the property
  test) is done on the *set* of lines per section path, not on rendered text:
  a device inserts a new `vlan 30` among the other VLANs while the prediction
  appends it, and that is not a difference anybody cares about. Differences
  are reported as "missing" (predicted, not on the device) and "unexpected".
* **Abbreviations**: a configuration keyword in a `Lines` intent that is a
  strict prefix of a known keyword but not a keyword itself (`sw tr al v 10`)
  is refused, with the full form shown; so is a keyword that is not in lower
  case, because the prediction is compared with what the device prints. Only
  the leading keyword run of a line is checked, so free text (`description to
  core sw`) and names (`WEB`, `bob`) are left alone. A word that is not a
  prefix of anything known is *unknown*, not abbreviated: the line is kept, a
  warning is raised and the change can never be `low` risk.
* **`interface range` / `default interface`**: `interface range` is refused in
  `Lines` (its effect on several ports cannot be predicted, verified or rolled
  back); `default interface` is modelled as "remove every line of that
  interface" and warned about. Both are still understood by the
  protected-resource detector, so neither can be used to slip past it.
* Session lost mid-apply, device reachable again, rollback succeeds → the
  target state is `rolled_back` (the states in §7 of the spec), with `error`
  saying the session was lost. The spec's test line calls this "failed +
  reconnect + rollback"; "failed" there is the apply, not the final state.
* A job that owns the change record waits for a one-shot signal before its
  first read: the RPC that started it writes `applying` / the job id first.
  Without that the job's early writes could be clobbered by the starter.
* Cancelling an apply makes the *job* end cancelled (not "succeeded"), after
  the change record has been written.
* A change that `failed` or was `rolled_back` may be planned again (the drift
  message tells people to do exactly that); one whose devices were changed may
  not, because its results are the record of what happened and the rollback
  still needs them.
* **The assistant and an approved change** (r9, A). `changes.assistant_apply`
  used to start an approved change without asking the autonomy gate at all:
  with autonomy `draft`, a protected device, a protected resource, high risk
  or an unverified device type, a person's approval let the assistant start
  it. It now asks `autonomy::decide` for an approved change exactly as for a
  planned one, checks the approval still covers the stored plans, keeps the
  person's approval (`approval.by` is not overwritten) and audits the start as
  the assistant's, naming who approved it. The refusal for an approved change
  says the person has to press Apply rather than "approve it there".
* **On-demand undo** (r9, B). `changes.rollback` used to derive commands from
  the device and send them in the same call: nobody saw them, nothing
  classified them, no hash, no approval, no dead-man switch. It now creates a
  change (`undoes`, origin `restore`) whose intent is the backed-up
  configuration from before as an exact intended configuration, and starts
  planning it; everything after that is the ordinary pipeline. The
  `rollback::run` that sent commands is gone. Decisions in the spec §
  On-demand undo. Two things that are only true of an undo: its targets
  cannot be replaced by `changes.update` (otherwise it is not the way back),
  and it is exempt from `allow_unverified_writes` (the exemption the on-demand
  rollback always had; the assistant is still held to the device type).
* **The undo links after a second look** (r9 review). An undo of an undo is
  refused: it would make the original change again under the undo's
  exemption, with the original's record still reading rolled back; the
  original is planned again instead. Planning again keeps the links honest:
  a finished change planned again loses its results and its `undone_by` and
  is a `draft` at once; it is refused while an undo of it waits; an undo is
  not planned again while another undo of the same change waits, and when
  it is, the original names it again. `changes.rollback` and `changes.plan`
  read the change with `ChangesState::undoing` held — `changes.rollback` used
  to read the original before taking it and write it back inside, which could
  write an older copy over what a finishing undo had just marked rolled back.
  The unused `rollback::candidates` is gone; `undo_of` is the rule.
* **An exact intended configuration on a grammar nothing predicts** is
  checked against itself (`verify::config_check`). Found while moving the
  undo onto the pipeline: `mikrotik_routeros` is `verified`, its configuration
  cannot be predicted, so its configuration check always failed — and the
  undo, applied as a change, would have rolled itself back every time where
  the old rollback compared the device with the version from before. A
  restore or a drift fix (`config.restore`, `config.drift.fix`, both exact)
  on RouterOS or Junos used to fail the same way and now is held to the
  configuration it names.

* **Secrets** (`src/redact.rs`, spec § Secrets). Two kinds of text, two
  rules. The *lines* (intent, commands, rollback, diff, a hit's command) are
  kept exactly as they will be sent and are only ever answered through a
  view: placeholders numbered per answer by default, whole on an audited
  `redact: false`. *Everything said about them* (warnings, reasons, step and
  check details, errors, sent lines, transcript, events, job log, audit
  details) goes through a `Hider` as it is made and holds `<hidden>`, with
  no number: that text is stored, the table that numbered it is not, and two
  tables' numbers side by side in one answer would claim that different
  values are the same. That is also why the transcript, which used to keep
  the engine's numbered placeholders, now says `<hidden>`.
  * The hider reads a line as a line. Several of the engine's rules know a
    secret only by the word its line begins with (`password …`, `key …`), so
    a path (`line vty 0 4 / password …`) is hidden part by part, a line
    quoted between backticks is hidden as a line (a rule that takes "the rest
    of the word" would otherwise take the backtick), and in a transcript
    whatever follows a prompt character is offered to the rules again.
  * A path, or a quoted line, is read whole *before* it is cut at ` / `. In
    a flat configuration (RouterOS, Junos `set`, `key=value`) every line is
    its own path, and a ` / ` in it is inside a quoted value: cut first,
    `password="qqvv / zzkk"` left `zzkk` to no rule, and a RouterOS or Junos
    rollback report published it (tests `a_routeros_value_…`,
    `a_junos_value_…`).
  * A device type's patterns are compiled with `(?mR)`, so one pass over a
    whole configuration or transcript reads each line the way a `^`/`$`
    pattern was written for. The first version read every multi-line text
    whole and then again line by line; measured on a 20,000-line IOS
    configuration with 4,150 secrets (debug build), that made planning 1.4×
    and an apply 2× slower, a whole-file intended change up to 2.5× slower,
    and a `changes.get` of one 230–390 ms. Now planning is as before, an
    apply about 150 ms slower, and that `changes.get` 75–110 ms (the view
    reads an intended configuration whole and gives its hider the values it
    just read). A text is still read a line at a time first when a line of
    it leaves a quote open, because a rule that reads to the closing quote
    would otherwise run on past the next line's value.
  * A careless device-type pattern can read the engine's own placeholder as
    a secret: the Linux type's `secret\s*[=:]` matches `<secret:3>` and
    leaves `<secret:<secret:4>`. Nothing of the value is left; what is left
    of the marker is folded into `<hidden>` (and may take a closing quote
    with it). No marker the engine treats as hidden can avoid this, since
    each contains `secret:`.
  * While the rules run, what is already hidden reads as `<secret:0>`: the
    engine leaves anything containing one of its placeholders alone, but
    knows `<hidden>` only when it stands by itself, and would take
    `<hidden>`,` for a new secret. Without this, hiding a text twice did not
    give the same text — and a stored record is hidden again every time it
    is read, for the sake of records written before any of this.
  * `abbrev::inspect` is given the line with its values hidden. A community
    made of lower-case letters is a value, not an unknown keyword; before,
    it was reported as one and ended up in the reasons for the risk level.
  * A refusal at planning points at the line (`details.index`) and quotes it
    hidden; a refusal *because the keywords were not understood* does not
    quote it at all, since those keywords are what the rules go by.
  * Not covered: the vault's own credentials are not looked for (the
    transcript never was, and the core does not echo what it types at a
    password prompt); `title`, `description`, `ticket` and an approval's
    `note` are a person's words and are answered and audited as written.

## What the protected-resource detector cannot see

Written into the plan's warnings rather than left unsaid:

* a device whose management address is not configured on any of its own
  interfaces (reached by hostname, by DHCP, or through NAT) — the
  management-path rules then have nothing to match against;
* a platform whose configuration grammar is not modelled (`Syntax::Flat`);
* uplinks on a device whose neighbours have never been collected.

## Known fidelity gaps of the fake device (for the core's owner)

* `nocturne_app::testing::FakeState` models only four global commands where
  setting a new value replaces the old one (`hostname`, `enable secret`,
  `ip default-gateway`, `ip domain-name`). A real IOS replaces many more
  (`snmp-server location`, `snmp-server contact`, `clock timezone`,
  `logging buffered`, `vtp mode`, …), so a change to one of those leaves two
  lines on the fake where a device would have one. The pipeline spots the
  difference and rolls the change back, which is why the property test's
  generator leaves those commands out.
* The fake treats a global command typed inside a section as closing it
  (`GLOBALS`); a real IOS either refuses it or applies it to the section.
  Nocturne refuses such lines at planning, so the two never disagree in
  practice.
* `default interface …` is not modelled by the fake at all.

## State

Done: everything in the spec, the secrets contract and the on-demand undo
included. 104 tests (39 unit, 65 through `app.rpc`), `cargo clippy -p
nocturne-changes --all-targets -- -D warnings` clean, `cargo fmt -p
nocturne-changes -- --check` clean, no `unsafe`.

Not done here, because it is not this crate's:

* `ui/src/api/types/changes.ts` does not yet know `redacted`, `hidden`, a
  result's `redacted`, or the `redact` parameter of `changes.get` and
  `changes.preview`, and the Changes screens have no way to ask for the
  hidden values (spec § Secrets, "What this asks of a caller").
* `nocturne-automation`'s playbook export reads `plan.commands` from
  `changes.get` with default parameters and would now export placeholders.
* The assistant's gate does not yet treat the `changes.*` answers as
  redacted elsewhere.
* A result recorded before values were hidden stays as it was in the store
  and is hidden each time it is read; nothing rewrites it.
