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
src/protect.rs    the protected-resource survey and detector
src/risk.rs       risk level and the reasons behind it
src/plan.rs       planning over one CLI session (the only reader of a device)
src/verify.rs     the post-change checks
src/apply.rs      the apply pipeline
src/rollback.rs   undoing an applied change on demand
src/autonomy.rs   the gate in front of `changes.assistant_apply`
src/rpc.rs        the RPC surface
tests/            planning, applying, approval, orchestration, property
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

Done: everything in the spec. 58 tests (23 unit, 35 through `app.rpc`),
`cargo clippy -p nocturne-changes --all-targets -- -D warnings` clean,
`cargo fmt -p nocturne-changes` applied, no `unsafe`.
`ui/src/api/types/changes.ts` mirrors the surface (type-checks with `tsc
--strict`).
