# Changes screen — working notes

Author: screens-changes slice.
Owned paths: `ui/src/pages/changes/`, `ui/tests/changes.spec.ts`,
`ui/tests/changes.shots.ts`. `ui/src/api/types/changes.ts` was already written
by the backend's author and is complete; nothing here needed to change it.

## Shape

```
index.tsx          the route: the list, or one change by :id
ChangeList.tsx     every change, with its state, risk, devices and reference
ChangePage.tsx     one change: banners, device cards, approve/apply, history
DeviceCard.tsx     one device: commands, predicted diff, risk, protected hits,
                   dead-man switch, rollback — and afterwards the steps,
                   the checks, the answers and the transcript
ApproveBar.tsx     the signature: typed confirmations, the review gate
ApplyStepper.tsx   the live pipeline per device, and the checks
CommandList.tsx    commands exactly as sent, numbered, with the device's answers
PlanDiff.tsx       the engine's own diff, rendered as it was given
LinesEditor.tsx    the CodeMirror box (line numbers, nothing clever)
NewChange.tsx      the three-step wizard: devices → what to do → review and plan
TemplatePicker.tsx `templates.*`, when a build has them (this one does not)
change.ts          states, tones, words, the stepper          (unit tested)
approval.ts        the approval gate and the hash binding     (unit tested)
useChangeJob.ts    following plan / apply / rollback to the end
methods.ts         `app.methods`, to leave out what a build cannot do
```

Methods used: `changes.list`, `changes.get`, `changes.create`, `changes.plan`,
`changes.preview`, `changes.approve`, `changes.unapprove`, `changes.apply`,
`changes.rollback`, `changes.cancel`, `changes.delete`; `jobs.get`,
`jobs.cancel`; `devices.list`; `audit.list`; `app.methods`; and
`templates.list` / `templates.render` only where they exist.
Events: `changes.changed`, `changes.progress`, `jobs.changed`,
`devices.changed`, `audit.appended`.

## Decisions

* **The typed confirmation names the device, and the screen agrees with the
  engine character for character.** `typedConfirmationOk` trims and compares
  case-insensitively, which is exactly what `rpc.rs` does; a screen that
  accepted less would send approvals the engine then refuses, and one that
  accepted more would be lying. "yes", a tick, and a near miss are all
  refused, and the unit test says so.
* **The approval is bound to the plans on screen.** `approvalRequest` sends
  the `plan_hash` of the plans that were rendered — not the ones the engine
  holds at that instant. If they have diverged, the engine refuses, which is
  the point of a hash.
* **The screen refuses to apply an approval that no longer covers the stored
  plans** (`approval.binding`), shows which device's plan moved and offers
  *Plan again*. It never re-plans by itself: that would be the screen deciding
  what a person signed for. The engine checks this too; this is the guard in
  front of it, and `changes.apply` is not called at all when it trips.
* **A plan is read by opening it, and only by opening it.** A plan that is
  replaced while its card happens to be open is *not* read — the card reports
  nothing, the review resets, and the page says "these plans were worked out
  again while this page was open". Re-planning an unchanged device gives the
  same `plan_hash`, so that case correctly keeps the review: it is the same
  plan, and the person did read it.
* **Refusals are quoted, never paraphrased.** Every engine message (drift,
  `forbidden` from the autonomy gate, a rejected line, a failed check) is
  rendered as it arrived. Only the engine knows why it said no; the screen's
  job is to put its sentence where the person will read it.
* **The diff is the engine's.** `PlanDiff` renders `plan.diff.lines` rather
  than re-diffing `before`/`after` with `design/DiffView`. What verification
  compares the device against is this prediction, so this is what has to be on
  screen — line numbers included.
* **An absent `guard` does not mean the platform has none.** The engine arms
  the dead-man switch when the risk is high or the change asks for it, so the
  card says "none will be armed for this change" and points at the warnings,
  where a platform that really has none says so. The first draft claimed the
  platform had none; the screenshots caught it.
* **Make-before-break is annotated from the command text**: a
  `switchport trunk allowed vlan add` line is marked "made before break" and
  the matching `remove` "broken after make", so the reason the order is safe
  is on the line itself rather than in a caption.
* **History comes from `audit.list`.** The audit `text` filter matches only
  the message and the source, not the body, so the page reads the `changes.`
  sources and filters on `body.change_id` in the browser.
* **`templates.*` is not in this build.** The source is offered disabled with
  that as its reason. `TemplatePicker` exists for builds that have the
  methods and reads their answers defensively, because no spec in this
  repository pins their shapes down; if a template library lands, pin them in
  `api/types/` and tighten it.

## Things the foundation (or another slice) might want

* `plural()` is now in three page folders (devices, terminal, changes). It
  belongs in `lib/format.ts`.
* `useHasMethod` is duplicated from `pages/devices/methods.ts` for the same
  reason; it belongs in `api/`.
* `design/Toolbar` wraps rather than condensing, so a page with status pills
  *and* actions wraps to two rows at 1280 with the dock open. This screen
  moved its pills out of the toolbar; a `Toolbar` that could hide labels under
  a container query (as the Terminal's does by hand) would help everyone.
* Playwright's `getByRole(..., {name})` matches a *substring*, so
  "Approve & apply to 1 device" matches `{name: 'Apply to 1 device'}` and
  "Rollback commands for X" matches `{name: 'Commands for X'}`. Both bit here;
  both need `exact: true`. Worth adding to the testing traps the Devices notes
  collect.
* CodeMirror's content element is `role="textbox"` with the `aria-label` from
  `EditorView.contentAttributes`, and Playwright's `fill()` works on it. That
  is the only way to type into the Lines box from a test.
* The simulator (`nocturne-netsim`) accepts `vlan 30` / `name GUEST` but does
  not print VLAN definitions in `show running-config`, so a change that
  creates a VLAN plans and applies cleanly and then fails verification and
  rolls back. That is the pipeline working; it does mean lab-based tests of a
  *successful* apply have to stay inside what the simulator renders
  (interface-level configuration does).

## State at hand-off

* `pnpm --dir ui test`: green; 30 of them are new here
  (`change.test.ts`, `approval.test.ts`).
* `tests/changes.spec.ts`: 9 tests against a real `nocturne-server` with a
  real simulated IOS switch behind it (`labs.create` / `labs.node.add` /
  `labs.start` — a lab node is an inventory device with `transport: sim`).
  One of them plans, approves, applies, verifies and rolls back for real, and
  checks the device's running configuration before and after each step. Every
  test asserts the console is clean.
* `tests/changes.shots.ts`: 16 PNGs into `tests/shots/changes/` (both themes,
  1440×900 and 1280×720, dock open and closed).
* Typecheck of this folder is clean. At hand-off `pnpm --dir ui build` depends
  on the assistant and monitoring slices, which were mid-flight.
