# Operations screen — working notes

Author: the screens-ops slice.
Owned paths: `ui/src/pages/ops/`, `ui/tests/ops.spec.ts`, `ui/tests/ops.shots.ts`,
`ui/src/api/types/ops.ts` (which the backend's author had already written and
which needed no change — it is complete and it is right).

## Shape

```
index.tsx          the route: six tabs, and `?incident=<id>` for one incident
ops.ts             the reasoning, with no React in it           (unit tested)
ops.test.ts        30 tests, about the four things that can mislead
data.ts            every read and write, and the topics that refresh them
author.ts          who is writing this, remembered in the browser
Unavailable.tsx    the three kinds of nothing, told apart
IncidentList.tsx   the list, counts, filters
NewIncident.tsx    opening one — from an alert, or by hand
IncidentPage.tsx   one incident: state, prose, follow-ups, links, export
Timeline.tsx       the centrepiece: merged sources, evidence, the note box
Handover.tsx       the generated summary, the prose, the saved copies
Reports.tsx        seven kinds, four formats, and a statement of coverage
Notes.tsx          runbooks, with `[[device]]` links as the backend resolved them
Audit.tsx          the record: filterable, linked, not editable
Jobs.tsx           the full history, with one job's log in a drawer
ExportDialog.tsx   a rendered document before it becomes a file
PrintFrame.tsx     printing that document without giving it this page's origin
printSelf.js       the one script the print frame runs (a file: see below)
```

Methods used: `incidents.*` (list, get, create, update, delete, note, link,
sync, acknowledge, resolve, reopen, suggest_links, export), `handover.*`
(generate, save, list, get, delete, export), `reports.*` (kinds, generate,
save, list, get, delete), `notes.*`, `audit.search`, `audit.actions`,
`jobs.list/get/cancel`, `devices.list`, and — only where `app.methods` says
they exist — `alerts.list`, `changes.list`, `ai.status`, `ai.chats.create`,
`ai.send`.
Events: `incidents.changed`, `handover.changed`, `notes.changed`,
`reports.changed`, `audit.appended`, `jobs.changed`, `alerts.changed`,
`changes.changed`, `config.changed`, `devices.changed`, `ai.event`.

## Decisions

* **The timeline merges a second source, and says which lines are only an
  offer.** The backend's timeline is already merged and sorted;
  `incidents.suggest_links` is the other half of the story and belongs *on*
  the timeline, at each item's own time — "this switch's configuration
  changed four minutes before the outage" is only useful next to the outage.
  `mergeTimeline` puts the two in one order (ts, then recorded facts before
  offers, then id) and the offers are drawn dashed, labelled *not linked*,
  with the button that adopts them. A suggestion whose key matches an entry
  already on the timeline is dropped rather than shown twice.
* **The note box is not behind a button.** It is pinned under a scrolling
  timeline, so it is on screen whatever the length of the record, and it
  takes a note on the button or Ctrl/⌘+Enter. The view follows the newest
  line only when the reader is already at the bottom: somebody reading the
  start of an outage must not be yanked away by a note from the next room.
  Backdating is one click away (`It happened earlier…`) because a note
  written at 03:40 about 03:05 belongs at 03:05.
* **Every line of the timeline can be checked.** The provenance the backend
  attaches is behind a disclosure on each line (`alerts.get`, the evidence
  string, when it was gathered, and how long after the fact). Collapsed,
  because nobody reads it until they doubt a line — and then it is the only
  thing they want.
* **A gap is drawn, never filtered.** An entry of kind `gap` gets the warn
  border and a banner above the timeline saying the lines around it may be
  incomplete. The same for `suggest_links`'s own `unavailable` list: an empty
  suggestion list with a reason beside it is not "nothing to suggest".
* **Three kinds of nothing, three shapes** (`Unavailable.tsx`): a section that
  was not read (with the engine's own reason and a headline per reason), a
  section that was read and was empty (its own sentence, in `--ok`), and a
  measured zero (an ordinary figure). `readSection` returns different *shapes*
  for the first and the others, so a component cannot render a missing
  section as a zero by accident.
* **Every handover item carries a link to the record it was made from**
  (`handoverSections`), and the unit test asserts that invariant over a
  handover with something in every section. A removed device — which leaves
  no record but the audit trail — links to the audit trail filtered by that
  device, because that is the only thing left to check it against.
* **`handover.save` regenerates the data server-side, so the prose box is the
  only thing a person writes.** *Ask the assistant to tidy my notes* therefore
  sends the model the prose and nothing else, and what comes back is a
  proposal in an `--ai` panel with *Use this* / *Discard*: the model's words
  never reach the record without a person pressing something. The button only
  exists when `ai.send` does.
* **Reports state their coverage in sentences** (`reportCoverage`), because
  "no rows because nothing happened" and "no rows because a source could not
  be read" are different documents and must not read alike. HTML output is
  previewed in a sandboxed frame rather than as source — it is self-contained
  with no scripts, and the point of that format is that it prints.
* **Print does not go through the preview.** The preview frame is allowed
  nothing, so its `print()` cannot be reached from this page (a SecurityError
  in every engine), and `allow-same-origin` — which would make it reachable —
  would put a document made from device data on the application's origin.
  `usePrintFrame` hands the same text to a second, hidden frame that has an
  origin of its own and one script, `printSelf.js`, which takes the document
  in a message, parses it where nothing runs, and prints itself. That script
  is a file because the application's policy only runs scripts from its own
  address, and it is imported `?url&no-inline` because the build would
  otherwise turn a file that small into a `data:` address, which is not.
  `tests/print.spec.ts` prints a document that tries to run script and shows
  that it cannot, on screen or in the print frame. The export dialog and the
  Reports tab both print this way, and neither preview is allowed anything.
* **A print job ends with its frame.** The export dialog stays mounted while
  it is closed, so what `usePrintFrame` remembers outlives what it renders. A
  job kept across that would be handed a new frame when the dialog came back
  and print again, unasked. The frame therefore reports leaving the document
  and the job goes with it. On the Reports tab, Print is offered for what is
  on screen — a kept report is in the format it was kept in, whatever the
  selector says.
* **The audit tab does not mask anything.** A secret must never reach the
  trail; masking one here would hide the defect in whichever feature wrote it
  while leaving the secret in the database, in `audit.search`'s answer and in
  every export. So the detail is rendered verbatim and the end-to-end test
  asserts that a real credential's password appears neither on the page nor
  in `audit.search`'s answer. It does not, today.
* **`resolved: false` on an audit reference is not "deleted".** The backend
  can only look up titles for its own records (incidents, notes, handovers,
  reports); for a change, a job or a lab it always says unresolved. The screen
  shows the *deleted* badge only for the kinds that can really be looked up
  (`RESOLVABLE_KINDS`), because telling a user their job record is gone when
  it is not would be a lie the trail does not support.
* **`undefined` from `useHasMethod` means "not known yet", and each call site
  decides.** For the whole screen it shows a skeleton (a wrong "not
  installed" page would be the worst answer). For the tab bar it hides
  nothing until the answer arrives, so the bar does not shuffle. For a
  control inside a tab (*open from an alert*, *tidy my notes*) it waits for
  `=== true`, so a control never flashes in and out.
* **The incident page is `?incident=<id>`, not a route segment.** The route
  table gives this screen one optional segment and it is spent on the tab;
  the route table is not mine to edit. It is still a link one can send, and
  the header names the incident.
* **Who wrote it is free text, remembered per browser** (`author.ts`). The
  core has no user directory and `audit(...)` has nowhere to put an actor, so
  every incident call takes an `author`; the field is editable wherever it is
  used and the timeline shows exactly what was sent. Nothing here pretends to
  be an identity.

## Deliberately not called

* **`search.query`** — the global search belongs to the command palette and
  the header's search button, which are the shell's, not this screen's.
  Adding a second search box here would make two places to look.
* **`overview.get`** — the Overview screen owns it.
* **`notes.list {incident_id}`** — notes can be attached to an incident, but
  nothing in this build writes that link, so a panel for it would always be
  empty. The incident's links panel points at the audit trail instead.
* **`incidents.assign`** — it is `incidents.update` with one field, and the
  assignee is edited in place in the Who panel; a second method for the same
  write would be a second thing to keep in step.
* **`reports.save {title, notes}`** — a report is kept with the engine's own
  title. Letting a person retitle a generated document is a small step
  towards a document that no longer says what it is.
* **Deleting an audit entry** — there is no method, and there should not be.

## What the backend or the design system made awkward

* **A defect: `nocturne-ops` cannot see which devices a change touched.**
  `crates/nocturne-ops/src/sources.rs:405–423` parses a change summary's
  devices from `device_ids` or `targets[].device_id`, but `changes.list`
  returns neither: `crates/nocturne-changes/src/db.rs:82–97` builds a
  `ChangeSummary` whose field is `devices: Vec<DeviceSummary>` (model.rs:388,
  each with `device_id`). So `ChangeSummary::devices()` is always `None` in a
  real build, and four things downstream are wrong:
  - `timeline.rs:625` — `touches` is always false, so **no change is ever
    suggested** for an incident, however obviously it caused it;
  - `timeline.rs:621` — the applied-change windows are empty, so every
    suggested configuration change says *no change of ours covers it*;
  - `handover.rs:176` — the same, so **every configuration change in a shift
    is reported `unplanned: true` with `checked: true`**, which is the false
    accusation the crate's own NOTES.md says it set out to avoid. It is
    visible in the screenshots: the pre- and post-change backups of a change
    Nocturne itself applied are listed as unplanned;
  - `reports.rs:494` — the changes report's per-device coverage.
  One line in `sources.rs` (reading `devices[].device_id` as a third
  alternative) fixes all four. The screen shows what the backend says and does
  not paper over it.
* **The audit trail records no actor**, so "who did this" is empty on every
  line gathered from it, and the incident methods take an `author` per call
  instead. A `by` field on `App::audit(...)` would fix both; the crate's own
  notes say the same.
* **`alerts` have no page of their own**, so every alert reference on this
  screen can only point at the device's health page (`/monitoring/health?device=…`),
  and an alert with no device points at the alert list. A `/monitoring/alerts?alert=<id>`
  route would make the handover's alert section and the audit trail's alert
  references land on the thing itself.
* **A configuration version cannot be addressed** from the Configuration
  screen, so configuration movement links to the device's history and leaves
  the reader to find the version. `/config?device=…&version=…` would close it.
* **Controls are `width: 100%`** (`design/control.css`), which in a wrapping
  toolbar makes every filter take a row of its own. Each one here carries a
  local `max-width` class to clamp its flex base size; a `width`-sizable
  control, or a Toolbar that handles it, would save every screen the same
  workaround.
* **`Panel` takes no arbitrary DOM props**, so a handover section is wrapped
  in a `div[data-section]` purely so a test can address it without depending
  on a heading's wording.
* **`TimeAgo` renders `0` as "56 years ago"**, since `0` is a finite number.
  Nothing on this screen passes a raw timestamp to it: `instant()` turns `0`,
  negatives and non-finite values into `null` first. A guard inside `TimeAgo`
  would be better than one in every screen.
* **There is no shared device picker** (the page spec names one). The new
  incident dialog uses a `Combobox` plus chips; the Changes screen built its
  own as well. Two implementations of the same control are one too many.

## File names

Two files in one directory must not differ only in case once the extension
is taken off: `Evidence.tsx` beside `evidence.ts`, or `PrintFrame.tsx`
beside `printFrame.js`. On Linux each import finds its own file; on Windows
and macOS `./Evidence` finds whichever the filesystem offers first, and the
build or the tests fail there on an export that "is not a function" — which
is how the print frame's tests failed on Windows while passing here. A
component keeps its own name; the React-free module beside it is named for
what it does (`comparison.ts` beside `Compare.tsx`). Run
`./scripts/check-case-collisions.sh` before calling work in this directory
finished: it also looks at files that are not committed yet.
