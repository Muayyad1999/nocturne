# Automation screen — working notes

Author: screens-automation slice.
Owned paths: `ui/src/pages/automation/`, `ui/tests/automation.spec.ts`,
`ui/tests/automation.shots.ts`. `ui/src/api/types/automation.ts` was written by
the backend's author; two shapes were added to it (below).

## Shape

```
index.tsx          the screen: gating, the tabs, ?run=<id>, the status item,
                   the palette commands, and the one place a finished run is
                   reported
data.ts            the reads every tab shares
given.ts           "did the backend actually give us this?"   (see below)
vocab.ts           the four outcomes, run kinds, origins, when a schedule
                   fires, in words                            (unit tested)
selector.ts        the filter as a form, and in a sentence    (unit tested)
compare.ts         the headline of a comparison, and its diffs (unit tested)
playbook.ts        where a playbook's steps start, for the gutter (unit tested)
useRun.ts          starting a run and following its job to the end
RunTab.tsx         the composer, the run history, the tally
DeviceSelect.tsx   the filter editor + `tasks.devices` under it
CodeEditor.tsx     CodeMirror with line numbers and problem markers
RunView.tsx        one run: live, then compare / devices / search
Compare.tsx        the point of the screen
RunDevices.tsx     per-device results, grouped by outcome
Tasks.tsx          saved tasks
TaskDrawer.tsx     the task editor (filter + one of four actions)
Templates.tsx      the list, the editor, the live render, the apply
Variables.tsx      a template's variables: the declarations and the values
Playbooks.tsx      the YAML editor, its validation, and playbook runs
Schedules.tsx      schedules and triggers, with the firing preview
Ansible.tsx        status, the caveats, export, run
Caveat.tsx         the caveats, as a banner and as a line
```

Methods used: `runner.run/.runs/.get/.compare/.search/.export/.delete`;
`templates.list/.get/.save/.delete/.duplicate/.validate/.render/.apply`;
`playbooks.list/.save/.delete/.validate/.run/.runs`;
`tasks.list/.save/.delete/.run/.devices`;
`schedules.list/.save/.delete/.run_now/.preview`;
`triggers.list/.save/.delete/.fire_now`;
`ansible.status/.export_playbook/.run`; plus the core's `app.methods`,
`devices.list`, `devices.facets`, `jobs.get` and `jobs.cancel`.
Events: `automation.runs.changed`, `tasks.changed`, `templates.changed`,
`playbooks.changed`, `schedules.changed`, `schedules.fired`,
`triggers.changed`, `triggers.fired`, `jobs.changed`, `jobs.log`,
`devices.changed`, `devices.state`.

Four methods are not called, and why: `templates.get`, `playbooks.get`,
`tasks.get` and `triggers.get` each answer with the record the matching
`*.list` already carried, and every list on this screen is kept fresh by its
`*.changed` event, so a second read would only add a way for the two to
disagree. `playbooks.run_get` answers the same `RunDetail` as `runner.get`;
one run view reads runs one way.

## Decisions

* **Four outcomes, four words, three of which are not "it broke".** `ok`,
  `failed`, `unreached` and `skipped` have their own label, tone, icon and
  one-sentence explanation in `vocab.ts`, and the per-device view groups by
  them with that sentence above each group. A device that was never reached
  says *"No session could be opened, so nothing was sent at all. This is not a
  failed command: the device never heard one"*; a skipped one says *"Nothing
  was sent and nothing failed"* and carries the backend's own reason, which
  names the run holding it. The tally in the run header and in every history
  row counts them separately for the same reason: "2 failed" and "2 could not
  be reached" are different facts and one number would hide which.
* **The comparison is the screen's reason to exist**, so it is the first tab
  of a finished run, and it is honest in three ways: the trimmed patterns are
  listed (agreement after trimming is not agreement), `+`/`−` are drawn as
  signs as well as colours, and the devices with *no* output are shown with
  their state and their reason rather than left out of the denominator.
* **A run is not a change, and the screen never lets that blur.** The
  composer says so above the button and again in the confirmation; a
  template's apply panel says "this drafts a change and touches nothing" and
  explains what the change pipeline will then do; the backend's up-front
  refusal of a non-read-only command is quoted verbatim, because only the
  engine knows why it said no.
* **The Ansible caveats are not decoration.** `Caveat.tsx` renders the five
  guarantees a change gives and this path does not (Backup, Drift check,
  Approved plan, Verification, Rollback) as five struck-through chips above
  the backend's own sentences, is not dismissible, and appears on the Ansible
  tab, in the task editor as soon as the action is `ansible`, and as a line on
  any run that carries `Run.caveat`. The one-line version leads with "No
  backup, no drift check, no verification, no rollback".
* **Where something runs is a filter, not a list.** `DeviceSelect` edits the
  filter and asks `tasks.devices` what it currently matches — the same method
  for a saved task and for one being written — so the count under the form is
  the backend's answer to the exact selection on screen. Devices named by
  hand are a union on top of it, and an open filter says in words that it
  means the whole inventory.
* **The run view is the only place a run is shown**, live or finished. While
  the job runs the screen shows what the *job* says (progress, and one line
  per device at the level the backend gave it) because that is all that is
  known: per-device output is written to the run record when the whole run
  ends. Nothing infers a device's state from the text of a log line.
* **The outcome is reported once, from the run record**, by a component at the
  page level (`Reporter`) rather than inside the composer, so the toast still
  arrives if the person has moved to another tab. Same pattern as
  `useChangeJob.ts` and `useCollect.ts`.
* **A template's variables are declared as well as filled.** The declarations
  editor is driven by `templates.validate`, which names what the body reads
  and cannot provide: one button declares exactly those. Built-ins are
  read-only and offer "Duplicate to edit", as the backend requires.
* **The majority's answer is folded away, the odd ones out are open.** What
  you came to the comparison for is the difference; the agreed answer is long,
  and one click away. With a single group (everybody agreed) that group opens,
  because then its text is the whole point.
* **Schedules and triggers share a tab** because they answer the same
  question ("what runs without me?") and the allow-listed method is the same
  object in both. The next five firings come from `schedules.preview` and are
  shown with the machine's UTC offset, since that is the clock the backend
  walks.

## What the backend made awkward

* **`Run.job_id` is lost when a run finishes.** `runner::fan_out` clones the
  record into the job closure *before* setting `job_id`, so the stored run has
  it while it is running and an empty string afterwards (`finish()` saves the
  stale clone). The live view uses it while it lasts; a finished run therefore
  cannot link to its job log, which is where a cancelled run's detail is.
  One line in `finish()` (`run.job_id` is already known at `open_run`) would
  fix it. The Ansible path does not have this problem, which is how the screen
  can follow an `ansible.run` job to the run it opened.
* **Optional fields are `null` on the wire, not absent.** The types say an
  optional field is "genuinely missing from the response, not `null`", and
  `CommandResult.parsed` really is; but `Run.caveat`, `Run.task_id`,
  `DeviceResult.error`, `Schedule.next_run_at`, `TemplateProblem.line`,
  `Filter.lab` and the rest arrive as explicit nulls. A `!== undefined` test
  therefore passes for a field the backend said it had nothing for — which is
  how a plain command run first showed the Ansible caveat, and how a filter
  with no lab preference first read as "real devices only". `given.ts` is the
  guard; the types would be better written `?: T | null`.
* **Per-device output only exists when the whole run has ended.**
  `db::save_results` runs once, in `finish`, so a fifty-device run shows
  nothing of what device 1 said until device 50 is done. The job log carries
  one line per device as it finishes, which is what the live view uses, but
  "what came back from the devices that are done" cannot be shown. An
  incremental `results` write (or a `jobs.log` line carrying the output) would
  make the live view what it wants to be.
* **A run record does not keep the selector it ran from.** It keeps the
  resolved devices, so "run again" can only offer the same *list*; the screen
  says so rather than pretending it re-resolves the filter.
* **`runner.compare` takes one command**, so "did anything differ anywhere in
  this run" needs one call per command. Fine for the five commands a run
  usually sends; worth knowing before someone sends fifty.
* **Built-in templates have `updated_at: 0`**, which a relative time renders
  as "56 years ago". The column says "Never" for them instead.
* **`templates.render` refuses the whole call when a required variable is
  missing** (`invalid`), rather than answering per device like it does for a
  render failure. The preview therefore holds the call back until the required
  variables have values, and says which are missing.
* **`schedules.run_now` and `triggers.fire_now` answer with the action's own
  result**, which for `runner.run` is `{job_id, run_id}` and for others is
  something else entirely. The screen reports that it started rather than
  opening it, because it cannot know what it got.

## Things the design system and the shell might want

* `design/Table`'s header is cut to "O." for a 64px column with a sort
  control: the header needs about 78px before a two-letter name fits.
* A `StatusPill` used as a group heading would like a `size` smaller than its
  one size; the state groups set their own type size around it.
* `design/Switch` and `design/RadioGroup` keep their known testing traps
  (`pages/monitoring/NOTES.md`); every switch here has label text, which is
  what makes it clickable from a test.
* A row's switch makes `getByRole('cell', {name})` ambiguous: the switch's
  accessible name is "<row name> is on", so a substring match finds *that*
  cell first and double-clicking it toggles the row instead of opening it.
  `exact: true` picks the name cell — unless that cell also holds a badge
  ("Built in", a tag), where `.first()` is the way. Both are in the specs
  here, next to the traps the other screens recorded.
* `plural()` and `formatCount()` carried this screen; nothing new was needed
  in `lib/format.ts`.
* `IdParams` and `Deleted` were added to `api/types/automation.ts` because
  nothing named the `{id}` / `{deleted}` shapes the method list documents.
  `monitor.ts` declares the same two for the same reason: a third screen
  wanting them is the point at which they belong in `core.ts`.

## Log

* Read the specs, the automation crate and its notes, then drove a real
  `nocturne-server` from the shell to learn the live shapes before writing a
  component — which is how the `null`-not-absent problem, the lost `job_id`
  and the "no results until the run ends" rule were found rather than
  guessed.
* Built the logic modules first (`vocab.ts`, `selector.ts`, `compare.ts`,
  `playbook.ts`) with their tests, then the tabs, then the editors.
* Looked at the screenshots in both themes and fixed what they showed: a
  plain command run wore the Ansible caveat and an empty "From the task"
  row (both `null` read as "given"); the comparison said "nothing to compare"
  for a run whose results landed after it was asked (no `invalidateOn`); the
  trimmed-pattern line read "6 line patterns was left out"; built-in
  templates were "changed 56 years ago" (`updated_at: 0`); the devices a
  selection matches were cut to "Ca…" because the address and role columns
  took the room (both are now one click away in the column chooser); the
  schedules table's "On" header was cut to "O."; the live view's tally read
  "of 2…" because a run's counts are all zero until it ends; the run header
  was a column of five rows that pushed the comparison below the fold at
  1280; the majority's long agreed output was open by default while the odd
  one out was off-screen; and the diff wrapped every device line in half
  instead of scrolling sideways.

## State at hand-off

* `pnpm --dir ui test`: green — 614 tests, 45 of them new here
  (`vocab.test.ts`, `selector.test.ts`, `compare.test.ts`,
  `playbook.test.ts`).
* `tests/automation.spec.ts`: 16 tests against a real `nocturne-server` with a
  real lab (two simulated switches and a router), a real unreachable device
  and a real playbook holding a device's lease. The comparison test is the
  one that matters: two switches really print the same interface table, the
  router really prints a different one, and the screen really groups them.
  Every test asserts the console is clean.
* `tests/automation.shots.ts`: 21 PNGs per theme (42 in all) into
  `tests/shots/<night|day>/` — 1440×900 and 1280×720, dock open and closed.
* `pnpm --dir ui build` (typecheck + vite) clean.
