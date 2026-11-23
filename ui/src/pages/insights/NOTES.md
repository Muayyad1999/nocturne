# Insights screen — working notes

Author: screens-insights slice.
Owned paths: `ui/src/pages/insights/`, `ui/tests/insights.spec.ts`,
`ui/tests/insights.shots.ts`. `ui/src/api/types/insights.ts` was written by the
backend's author; the screen added `SettingsProblem` and
`InsightsSettingsRefusal`, the refusal `settings.set` gives for this section
(`docs/dev/features/insights.md`, Settings).

## Shape

```
index.tsx          `Insights`: the mountable body — toolbar, the three views,
                   the scan, and the status-bar item that always says how old
                   the scan is. The default export is the same thing with a
                   `Page` and a page title around it, for a standalone route.
CoverageBand.tsx   coverage above every view — the thing that cannot be missed
Coverage.tsx       the Coverage view: every check of the last scan accounted for
Findings.tsx       the findings: filters, severity groups, the three empty states
FindingCard.tsx    one finding: evidence on the card, actions under it
FindingDrawer.tsx  one finding in full, plus the catalogue entry that raised it
DismissDialog.tsx  dismiss with a reason and an expiry
Checks.tsx         the catalogue, the thresholds and how scans run
Facets.tsx         the filter chips
ScanBanner.tsx     the scan while it runs, and its failure afterwards
insight.ts         severities, categories, verdicts, coverage sums,
                   reading and judging thresholds              (unit tested)
useScan.ts         following `insights.scan`'s job to its end
data.ts            every query and mutation, and the device index
```

Methods used: `insights.list`, `insights.get`, `insights.coverage`,
`insights.checks`, `insights.scan`, `insights.dismiss`, `insights.reopen`,
`insights.fix`; `jobs.get`, `jobs.cancel`; `devices.list`; `settings.get`,
`settings.set` (section `insights`); `app.methods`.
Events: `insights.changed`, `jobs.changed`, `settings.changed`,
`devices.changed`.

## Decisions

* **Coverage is a band, not a tab.** The findings cannot be read without what
  was not examined, so `CoverageBand` sits above all three views: the number of
  checks and devices, the age of the scan, four tiles (findings / evaluated /
  not evaluated / checks off) and the engine's own top reasons. The Coverage
  view behind it accounts for all 37 checks. The band is one landmark named
  "Coverage of the last scan" in every state it can be in, including the error
  and the never-scanned ones.
* **Five answers, five words, in one place.** `insight.ts` owns
  `checkVerdict`/`verdict`: `N findings` (crit), `Clear` (ok),
  `Clear on 1 of 3` (warn), `Not evaluated` (idle), `Off` (idle). A check that
  could not run is never shown as a check that passed, and a check that is
  switched off is a decision rather than a gap — so "the check is switched off"
  is left out of the reason list and lives in the catalogue instead.
* **Reasons are the backend's sentences, verbatim**, aggregated by how many
  device-by-check answers they account for (`reasonTotals`). Paraphrasing them
  would be the one lie this screen cannot afford.
* **Answers, not devices.** Coverage is summed as device-by-check pairs
  ("102 evaluated, 33 not evaluated of 135 asked for"): a check that could not
  decide for three devices is three missing answers.
* **The age of the scan is in three places**: the band, the status bar (always,
  while the screen is open) and the toast after a scan. A stale scan is the
  main way this screen could mislead, so the band's left edge turns warn after
  an hour and crit after a day.
* **The last scan is not always the whole network.** `insights.coverage` is the
  coverage of the *last* scan, which may have been one device (the debounced
  automatic scan is per device). The band compares `coverage.devices` with the
  inventory and says so, with a "Scan every device" action, rather than letting
  "3 devices" read as "the network".
* **Evidence is on the card.** A finding with no evidence on screen is an
  accusation; where a finding carries none, the card says so in `--warn` rather
  than showing an empty box.
* **"Draft a change", never "Fix".** `insights.fix` creates a draft in the
  change pipeline. The button's tooltip, the toast and the drawer all say that
  nothing is sent to the device and that a person still has to approve it. The
  end-to-end test reads the simulated switch's running configuration before and
  after to prove it.
* **A repeat is not a discovery.** `occurrences > 1` shows a `Repeat · N`
  badge, and the drawer says "N separate times — it came back". That is the
  backend's counter: a resolved finding found again, or a dismissal that ran
  out, counts one more.
* **Dismissal asks for a reason and an end**, and says what happens when the
  end arrives (it comes back as occurrence N+1). The reason is kept with the
  finding and read back in the drawer.
* **Severity is a floor.** `insights.list {severity}` means "at least this
  severe", so choosing one shows a line saying so; otherwise the chip would
  read as an exact match.
* **Site is filtered in the browser.** There is no `site` parameter on
  `insights.list` and no multi-device filter, so a site narrows the loaded
  findings; when more findings match than were loaded, the footer says exactly
  that instead of quietly under-reporting.
* **The `insights` settings section has no home in Settings** (`sections.ts`
  does not list it), so the Checks view owns it: the per-check switches, the
  sixteen thresholds, `include_lab`, `auto_scan` and `raise_alerts_from`. A
  threshold is part of what a check means, so each check also shows the
  numbers it compares against (`CHECK_THRESHOLDS`).
* **A threshold is checked before it is sent, and the engine's answer is
  said beside it.** Eleven thresholds are whole numbers in the engine (`u32`,
  `interface_error_min_delta` a `u64`): their fields step by one and refuse a
  fraction, a negative number and one too large to hold, beside the field,
  sending nothing (`readThreshold`). An engine from before writes were
  checked stored `7.5` days as given and then could not read its own
  section: the catalogue, the list and every scan failed. An emptied field —
  or text a number field cannot read, `1e` — is no change, never a zero
  (zero makes most checks match everything). A refusal from `settings.set`
  goes beside the field that was refused (its own line of
  `details.problems`), not into a toast; each field has its own mutation, so
  each gets its own answer. Escape puts the stored value back.
* **The settings never depend on the catalogue.** `settings.get` hands back
  what is stored even when the engine cannot read it as the section's type,
  so "How insights run" and the thresholds stay on screen when
  `insights.checks` fails; only the catalogue shows its error. A stored value
  that is wrong (`storedThresholdProblem`) is said beside its field, and the
  thresholds panel opens by itself while one is: nothing else in the
  application edits this section.
* **No "Ask the assistant" button.** The dock has no API for attaching context
  yet (`pages/assistant` exports only the page), and a button that does nothing
  would be worse than none. One line here once the assistant slice offers one.

## How it is mounted

Insights is the Monitoring screen's **Insights** tab, and
`ui/src/pages/monitoring/` belongs to another slice, so this folder exports the
body and lets that screen mount it:

```ts
import { Insights } from '@/pages/insights';
// in Monitoring's TABS: { id: 'insights', label: 'Insights', icon: ScanSearch }
// in its tab panel:     {current === 'insights' && <Insights />}
```

`Insights` takes **no props**. It renders no `Page`, no page header and no
router of its own: it is a flex column (`.screen`) that fits inside
Monitoring's `TabPanel`. It brings its own `Toolbar` with its three sub-views
(Findings / Coverage / Checks) and its own primary action, *Scan now*; its
sub-view is the `view` search parameter (`?view=coverage`), which does not
collide with Monitoring's `:tab` route parameter or its `?device=`. It
registers two commands in the palette (`insights.scan`, `insights.coverage`)
and one status-bar item (`insights.scan`, the age of the last scan) while it is
mounted, and it gates itself on `app.methods` as well, so mounting it in a
build without the crate is safe.

The default export is the same component inside a `Page` with the page title
set, which is what the Playwright specs drive at `#/insights`. Both test files
build the real application from the real `src/` with one route and one nav
destination added (`buildHarness`) and serve it to a real `nocturne-server`;
once the tab is mounted in Monitoring, those specs can navigate to
`#/monitoring/insights` instead and the harness can go.

## What this screen needs from somebody else
* `useHasMethod` is now duplicated in three page folders (devices, changes,
  insights). It belongs in `api/`.
* **The nav rail has no Insights entry and this slice does not ask for one**:
  the count of open findings would have gone there (`useNavBadge('insights')`
  was removed), so while the tab is not on screen nothing says how many
  findings are open. Monitoring's own badge counts open alerts, and findings at
  or above `raise_alerts_from` become alerts, so the serious ones are already
  counted there.
* **`design/Switch` cannot be clicked by Playwright**: the drawn `.track` and
  `.thumb` intercept pointer events over the real `<input>`, exactly as
  `RadioGroup`'s `.dot` does (the Devices notes say Switch does not have this
  problem; it does now). Tests here click the `<label>`. The catalogue's
  switches also carry the word On/Off, which the design rules want anyway.
* **A simulated node hangs on `show startup-config`.** `config.backup` with
  `kinds: ['startup']` (or the default, which includes it) never finishes for a
  `transport: sim` device, and it holds the device's session afterwards, so
  every later backup of that device fails with "busy with another operation".
  Reproduced with nothing else running:

  ```
  labs.import {spec with one `switch` node, startup_config "hostname SW"}
  labs.start
  devices.exec {id: "L/SW", command: "show startup-config"}   # never returns
  devices.exec {id: "L/SW", command: "show running-config"}   # fine
  ```

  Both test files therefore back up `kinds: ['running']` only. The silver
  lining is a genuine "not evaluated" on screen: `unsaved_config` says "no
  startup configuration stored, so there is nothing to compare the running one
  with". Worth a look in `nocturne-netsim`'s CLI or the `cisco_ios` profile's
  `config.startup` command.
* Playwright: `getByText('LAB')` matches "Lab: Campus" and any text containing
  "lab", so the LAB pill needs `{ exact: true }` — one more for the trap list
  the Devices notes collect.
* Two Playwright files may not import each other (the imported file's
  `test(...)` calls register in both projects), so `insights.shots.ts`
  duplicates the harness and the lab from `insights.spec.ts`. A
  `tests/support/` helper would fix that, but that folder is not this slice's.

## What looking at the screenshots changed

* The state filter (`Open`), the catalogue's category filter and *Raise an
  alert from* were each stretching across the whole row: a `Select` fills a
  flex line unless it is told not to. All three now have a width.
* The filter row is hidden when there is nothing to filter and no filter is
  set, so the never-scanned screen is the band, the empty state and nothing
  else.
* The thresholds panel starts collapsed: it was pushing the catalogue — the
  point of that view — below the fold.
* Evidence labels wrapped rather than truncated: at 1280 with the dock open,
  `Campus/ACC1 GigabitEthern…` is half a fact.
* "Verdicts" in the coverage view listed every bucket including the empty
  ones ("0 clear only where they could look"); it now names only the answers
  the scan actually gave.
* The dismissal dialog repeated the finding's title after its device and
  subject; the device and the port are enough.
* The tiles in the band were `<dt>`-value/`<dd>`-label, which is backwards;
  the label is the term now and CSS keeps the number first.

## State at hand-off

* `pnpm --dir ui test`: green; 12 of them are new here (`insight.test.ts`).
* `pnpm --dir ui build`: clean (typecheck and bundle). The insights chunk is
  about 49 kB.
* `tests/insights.spec.ts`: 9 tests against a real `nocturne-server` with three
  real simulated switches behind it, with the faults planted in the
  configurations they boot from — green in 11.5s. Every test asserts the
  browser console is clean.
* `tests/insights.shots.ts`: 20 PNGs into `tests/shots/insights/` (both themes,
  1440×900 and 1280×720, dock open and closed, with findings and with nothing
  examined yet).
