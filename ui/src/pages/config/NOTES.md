# Configuration screen — working notes

Author: screens-config slice.
Owned paths: `ui/src/pages/config/`, `ui/tests/config.spec.ts`,
`ui/tests/config.shots.ts`. `ui/src/api/types/config.ts` was already written
by the backend's author and is complete and accurate; nothing here needed to
change it.

## Shape

```
index.tsx          the route: five tabs, the URL state, the palette commands
data.ts            every query and mutation this screen makes, in one place
methods.ts         `app.methods`, to leave out what a build cannot do
vocab.ts           words, tones, capture states, the diff projection   (unit tested)
redaction.ts       finding and describing `<secret:N>` placeholders    (unit tested)
find.ts            searching inside one configuration, locally         (unit tested)
Backups.tsx        coverage table, unsaved changes, per-device job outcomes
History.tsx        one device: versions, the text, two versions compared
ConfigView.tsx     the read-only CodeMirror viewer                     (lazy)
IntendedEditor.tsx the editable twin of it                            (lazy)
editor.ts          theme, highlighter, secret markers, search marks   (lazy, shared)
VersionDiff.tsx    the engine's diff, unified or projected into columns
NetworkSearch.tsx  `config.search` / `config.missing` across the network
Compliance.tsx     score, policies, rules, findings
RuleDetail.tsx     one rule: check, rationale, fix, and a live test
Intended.tsx       intended configuration, drift, hand-over to changes
Redacted.tsx       the "secrets hidden" note and the inline marker
Quoted.tsx         the engine's backticked spans, as code                (unit tested)
DevicePicker.tsx   choosing a device, including ones with nothing stored
reveal.ts          asking once per session to see secrets
```

Methods used: `config.devices`, `config.unsaved`, `config.backup`,
`config.versions`, `config.get`, `config.diff`, `config.search`,
`config.missing`, `config.pin`, `config.unpin`, `config.restore`,
`config.intended.get` / `.set` / `.clear`, `config.drift`,
`config.drift.fix`; `compliance.policies.list`, `compliance.policies.save`,
`compliance.run`, `compliance.results`, `compliance.runs`,
`compliance.remediate`, `compliance.test`; `jobs.get`, `jobs.cancel`;
`devices.list`; `app.methods`.
Events: `config.changed`, `config.first_backup`, `config.deleted`,
`config.pinned`, `config.intended_changed`, `compliance.finished`,
`compliance.policies_changed`, `jobs.changed`, `devices.changed`.

Not used here: `config.export` (a local-filesystem path belongs in a dialog
that can browse one, which the design system has no component for yet) and
`config.delete` (deleting captures is retention's job, and a button for it
next to "Restore" invites the wrong click). Both are noted below.

## Decisions

* **Four states, not one.** "Never captured", "captured and empty", "could
  not be read" and "not backed up on purpose" are four different answers and
  `captureState` keeps them apart everywhere: the coverage table says
  *never* rather than a zero, the history tab of a never-captured device is
  an empty state that says in words that this is **not** an empty
  configuration, and a stored version with no lines is a third screen that
  shows its checksum and its timestamp, because it *was* read. The backend
  agrees: `config.latest` answers `not_found` with "has no running-config
  backup yet", and that sentence is quoted rather than paraphrased.
* **The diff is the engine's, and so is the side-by-side.** `VersionDiff`
  renders `config.diff`'s lines, numbers, `…` gaps and counts as they
  arrived — the Changes screen's reason applies here too. Side by side is
  therefore a *projection* of that same unified diff into two columns
  (`sideBySide` in `vocab.ts`), pairing each run of removals with the run of
  additions that follows it. `@codemirror/merge` would have re-diffed the two
  texts in the browser, and a second opinion that disagreed with the engine
  would be believed because it is the prettier one. The unit test asserts
  every rendered line is one the engine gave us.
* **A placeholder is drawn as a placeholder.** `<secret:3>` is marked
  wherever it appears — in the editor (a CodeMirror decoration), in a search
  hit, in a diff line, in compliance evidence, in a drift command — and the
  note above every configuration says how many secrets were taken out, in how
  many places, and how many were well-known defaults (`:weak`). "Redacted
  with nothing to hide" is a different sentence from "3 secrets hidden",
  because it tells the reader nothing was kept from them.
* **Revealing asks once per session and says it is audited.** `reveal.ts`
  holds that agreement, so the viewer, a future export dialog and anything
  else agree about whether it has been given. The backend does the auditing;
  the screen only tells the truth about it.
* **Search is a field, not a form.** 180 ms of debounce, no Enter, no submit
  button; the query, the regex and case flags live in the query string so a
  result can be pasted into a ticket. `/` focuses it from any tab (and jumps
  to the Search tab from the others).
* **A search says what it is an answer about.** `devices_searched` versus
  `devices_without_backup` is the difference between "nothing is configured
  that way" and "eleven devices have never been read", so the coverage line
  carries both and the empty state spells it out. The same applies to
  compliance: `devices_without_backup` and `not_evaluated` are shown next to
  the score, and `FINDING_SENTENCE.not_evaluated` says in as many words that
  it is not a pass.
* **Per-device backup outcomes come from the job's log.** The result counts
  outcomes but names only the devices it skipped or failed on, so
  `backupRows` reads the per-device lines the job wrote ("first backup
  stored", "configuration unchanged", "skipped: …") and keeps everything
  else about that device as a note. That is how the simulator's
  "could not read the startup-config: startup-config is not present" ends up
  on screen as a warning next to a successful capture rather than vanishing.
* **The screen never sends anything to a device.** *Restore this version*,
  *Fix the drift* and *Fix findings* all call methods that draft a change,
  and each one says so — naming the device — before it is pressed. The
  confirmations say "nothing is sent until somebody approves that plan",
  which is what the pipeline actually guarantees.
* **CodeMirror is lazy.** `ConfigView`, `IntendedEditor` and the shared
  `editor.ts` are reached only through `React.lazy`, so the 212 kB
  CodeMirror chunk is not in the Configuration route's own chunk (67 kB) —
  the build output confirms it is only in the dynamic-import dependency map.
* **Highlighting is regex-based on purpose.** Nocturne reads IOS, RouterOS,
  VRP and set-style configurations; a grammar per platform is a dependency
  and a lie for the fifth one. Comments, the leading keyword, `no`,
  addresses, numbers, quoted strings and secret placeholders are common to
  all of them, and that is what the eye uses. No new dependency was added.
* **Compliance runs are not jobs.** The spec for this screen asked for "run a
  policy as a job", but `compliance.run` reads only stored configurations, so
  the backend answers with the finished summary rather than a job id. The
  screen follows the backend: a button that goes pending and a toast with the
  outcome, no job drawer. See below.
* **A built-in policy's switch is the only thing the screen writes.** The
  backend accepts `enabled` on a built-in and nothing else, so the policy
  list offers a Switch and says "Built in" with a tooltip explaining that
  editing means duplicating. The custom-rule editor the page spec describes
  is not here (see below) — and it is not offered disabled either, because
  this project does not do "coming soon".

## What the backend made awkward

* **`compliance.results` filters by device *id*, while `config.latest`,
  `config.versions`, `config.drift` and friends all accept an id, a name or
  an address.** Passing a device name to `compliance.results` silently
  returns zero findings with a full summary, which reads exactly like "this
  device has no findings". The screen only ever passes ids, so it is fine,
  but the next caller will lose an hour to it. Either resolve the name there
  too, or answer `invalid`.
* **A finished `config.backup` job's `result` names only the skipped and
  failed devices.** Which device was *unchanged* and which stored a new
  version has to be recovered from the log lines, which are prose
  ("configuration unchanged"). A per-device array in the result — id, state,
  version id — would make this reliable instead of nearly reliable.
* **`jobs.list` omits `result`,** so a page that wants outcomes has to
  `jobs.get` each job. Fine for one followed job, awkward for a history.
* **`config.drift` returns a `TextDiff` without the two `ConfigVersion`
  records** that `config.diff` returns, so rendering it through the same
  component needs two placeholder version records. Returning the same
  `DiffResult` shape (with `sections`) would let one component serve both.
* **Setting an `enable secret` on a lab node locks Nocturne out of it.** Not
  a bug — the device's credential has no enable secret — but it means the
  obvious way to get a redactable secret into a test configuration breaks the
  next backup, with a good error message. The tests use
  `username … password` and an SNMP community instead, and say so.
* **`compliance.run` is not a job**, although the page spec asks for "run a
  policy as a job". It reads stored configurations only, so it answers with
  the finished summary — which is better, and the screen follows the backend
  rather than inventing a job to follow.
* **`config.export` takes a directory path and nothing else.** There is no
  file-picker component in the design system and a text field for an absolute
  path on the server's filesystem is a trap, so export is not offered yet. A
  `Dialog` with a path field plus the engine's "refuses a non-empty directory
  that is not a previous export" message would be a small, self-contained
  follow-up.

## Still to do (said plainly, not shipped as an empty tab)

* The **custom-rule editor** from `spec-ui-pages.md` (`compliance.policies.save`
  on a duplicated policy, with `compliance.policies.validate` and
  `compliance.test` as the live preview). `RuleDetail` already does the
  "test a rule against a device" half of it, and `data.ts` has the mutations.
* **Compliance score over time** from `compliance.runs` — the data is there
  (the screen already reads it to know whether anything has ever been run); a
  Sparkline in the score Stat is the whole job.
* **`config.compare`** (two different devices, "why does SW2 behave
  differently from SW1") — the `VersionDiff` component takes its result shape
  unchanged, so this is a device picker and a call.

## What looking at it changed

* **The findings table ran under the status bar.** The Compliance tab is a
  long document (score, a note, the policy list, filters, the table) and it
  was built as a fixed-height pane: `flex: 1; min-height: 0` with a `fill`
  table inside a scrolling `Page`. When the content outgrew the viewport the
  table's own box extended below the window and its last rows could not be
  scrolled clear of the status bar — a row you could see and could not click.
  The end-to-end test caught it before the screenshots did (Playwright
  reported the footer intercepting the click for five minutes). Tabs that are
  documents — Backups, Search, Compliance — now scroll with the page, and
  their tables carry their own `maxHeight` instead of `fill`. The tabs that
  really are panes (History's splitter, Intended's two editors) keep the
  fixed-height layout, and the drift column scrolls inside itself.
* **The policy list opens collapsed.** The two built-in policies are 27 rules
  between them; expanded, they pushed the findings — what the tab is for —
  below the fold. It is one click, and `Panel` remembers the choice.
* **Columns truncated to nothing while the table had room to spare.** Both
  tables let every column share the width equally, so "Campus/access-sw-1"
  became "Campus/a…" next to an empty half-screen. The fixed-shape columns
  (verdict, severity, counts, times) now carry a `width` and the text
  columns take what is left.
* **Native selects stretch to fill their flex line**, so the Compliance
  filters, the configuration-kind picker and the Search scope each sat on a
  row of their own, full width. They are given explicit widths now and the
  controls read as one row.
* **"1 device … were not judged"** — the summary sentence did not agree with
  itself for one device.
* **"Back up all  now…"** with two spaces, when nothing is backable. Now it
  reads "Back up every device…" and stays disabled. The label also lost the
  word "now" so the toolbar does not wrap at 1280 with the dock open.
* **The compliance stat tiles stacked one per row** at 1280 with the dock
  open; they are `flex: 1 1 110px` now and stay on one line.
* **The engine's prose is full of backticks** — "Checked: the global line
  `service password-encryption`" — and they were being shown raw. `Quoted`
  renders those spans in the monospace face without changing a word.
* **The drift panel's "Fix the drift…" button was full width** (it looked
  like a banner, not an action) and its diff grew to fill the column however
  little it had to say. Both are bounded now.
* **The lazily loaded editor showed a bare spinner** while its chunk
  arrived; the screenshots caught it mid-flight. It is a Skeleton shaped
  like the editor now, as the page conventions ask.
* **The primary action jumped to the left** when the toolbar wrapped at
  1280 with the dock open, because a `ToolbarSpacer` takes the whole first
  row. The action pushes itself right with `margin-left: auto` instead, so
  it stays the top-right action on one row or two.
* **An ellipsised evidence cell cut a redaction marker in half** — `<sec`
  with a border round it reads as a broken value. The cell is a block with
  a real ellipsis now.
* **A diff's skipped-lines row showed `⋯` and an em dash**, which reads as
  two separate symbols; it says "unchanged lines" instead. An empty section
  list no longer prints "No section changed" next to a drift diff, where the
  statement was meaningless.

## Things the foundation might want

* `Combobox` has no way to render a label with two parts (a device's name and
  a quiet note), so `DevicePicker` joins them with an em dash into one string.
  An optional `description` per option exists but is only shown in the list,
  not in the closed field.
* `Table`'s `rowActions` entries have no `id`, so two actions with the same
  label in one menu are indistinguishable to a test. Not a problem here.
* Every row checkbox in a `Table` is labelled "Select row", so a test has to
  find the row first. `aria-label` per row (from `rowKey`, or a column) would
  make selection tests read much better.
* `plural()` is in `lib/format.ts` now (thank you), and this screen uses it.
  A one-line `app.methods` hook is now in three page folders (devices,
  changes, config); it belongs in `api/`.
* A `Panel` whose header can hold a progress bar would save this screen a
  small layout; not important.

## Testing traps met here (on top of the ones Devices and Changes collected)

* `getByText('never')` inside a table row matches "Never captured" and
  "Never/SW1" as well: substring matching applies to `getByText` too, so the
  coverage assertions pass `{ exact: true }`.
* Lab devices stay in the inventory after their lab is stopped, so a test
  that backs up "everything" counts the devices of every test before it. The
  backup test selects its four rows instead.
* CodeMirror's content element is `role="textbox"` with the `aria-label` from
  `EditorView.contentAttributes`, so the viewer is
  `getByRole('textbox', { name: /Running configuration, version/ })` and
  `fill()` works on the intended editor. (Confirmed again from the Changes
  notes; it is the only way in.)
* The `RuleDetail` drawer and `confirm()` are both `role="dialog"`, so a test
  that has one open while the other appears must filter
  (`.filter({ hasText: 'Why it matters' })`).

## State at hand-off

* `pnpm --dir ui test`: the suite passes, 41 of them new here
  (`vocab.test.ts`, `redaction.test.ts`, `find.test.ts`, `Quoted.test.ts`).
* `tests/config.spec.ts`: 6 tests against a real `nocturne-server` with real
  simulated IOS switches behind it. They back up a device that had never been
  captured, back up four at once (one of them unreachable, so its failure and
  the engine's reason are asserted), change a switch **at its own console**
  and compare the two resulting versions, search the whole network, run the
  built-in hardening policy and read its findings, and hand a drift to the
  change pipeline — then check through the API that the device's running
  configuration is byte-for-byte unchanged. Every test asserts the console is
  clean.
* `tests/config.shots.ts`: 28 PNGs into `tests/shots/config/` — 14 per
  theme, at 1440×900 and 1280×720, with the dock open and closed.
* `pnpm --dir ui build` is clean (typecheck of both projects, then the
  bundle). The Configuration route's own chunk is 68 kB (21 kB gzipped) and
  the CodeMirror chunk (212 kB) is reachable only through the dynamic
  imports, as the build's dependency map shows.
