# Monitoring screen — working notes

Author: screens-monitoring slice.
Owned paths: `ui/src/pages/monitoring/`, `ui/tests/monitoring.spec.ts`,
`ui/tests/monitoring.shots.ts`.

## Shape

```
index.tsx            the screen: gating, the status strip, the tabs, Poll now
data.ts              the reads every tab shares, and the one write
StatusStrip.tsx      what is watched right now + monitor.status's problems
Alerts.tsx           the alert table, its filters and the three row actions
AlertDrawer.tsx      one alert, with the series it was measured on
Health.tsx           every monitored device, problems first
Device.tsx           one device: cause, health, sources, charts, ports
Interfaces.tsx       the network-wide interface table (also one device's)
InterfaceDrawer.tsx  one port: facts and three charts
Charts.tsx           the metric explorer
Rules.tsx            alert rules + maintenance windows + channels
RuleDrawer.tsx       the rule editor, with "test against what is stored"
MaintenanceDrawer.tsx  one window
ChannelDrawer.tsx    one channel; its secret is write-only
SecretField.tsx      "Set / Not set", Replace, Clear          (a copy, see below)
MetricChart.tsx      TimeSeriesChart + the gap bookkeeping
RangePicker.tsx      how far back the charts under it look
series.ts            answers → one axis, with the holes kept   (unit tested)
vocab.ts             metrics, conditions, severities in words  (unit tested)
datetime.ts          datetime-local ↔ ms, window state         (unit tested)
```

Methods used: `app.methods`, `monitor.status`, `monitor.device`,
`monitor.interfaces`, `monitor.poll_now`, `metrics.query_many`,
`metrics.series`, `metrics.top`, `alerts.list`, `alerts.ack`, `alerts.resolve`,
`alerts.snooze`, `alerts.summary`, `alerts.rules.list/.save/.delete/.test`,
`maintenance.list/.save/.delete`, `notify.channels.list/.save/.delete/.test`,
plus the core's `devices.list`, `devices.get`, `devices.update` and
`settings.*` (read by other screens). Events: `monitor.polled`,
`monitor.interfaces`, `monitor.status_changed`, `alerts.changed`,
`devices.state`, `devices.changed`.

`alerts.get` and `metrics.query` are the two methods of the feature this screen
does not call: the list already carries the whole alert, and every chart draws
more than one series, so `metrics.query_many` serves both.

## Decisions

* **A gap is a hole, and it is also said in words.** `metrics.query` answers
  with only the buckets that have something in them, so the points alone would
  draw a straight line across an outage. `series.ts` puts the answers on one
  axis, gives a series with no sample at a timestamp `null`, and inserts one
  extra timestamp wherever the axis itself jumps — that is what makes uPlot
  lift the pen. The same pass returns the gaps, and `MetricChart` prints
  "nothing was recorded then" under the chart: a break in a line is easy to
  miss, and the difference between *nothing happened* and *nothing was
  measured* is the whole point. `series.test.ts` has the cases; the
  end-to-end spec proves it against a device that really stopped answering,
  through the chart's table view (a row with a dash).
* **How far apart samples normally are is taken from the data**, as the lower
  median distance between consecutive samples, floored at the bucket width the
  backend answered with; more than 1.5× that is a hole. Not from the device's
  poll interval, which is per device while a chart can span several. The known
  limit: where the bucket width is about the polling interval, one missed poll
  is within the jitter and may not be drawn as a hole — an outage of two
  intervals or more always is. Short ranges ask for a step well under a
  minute, which is where this matters least.
* **A device that is down reads as one cause.** The backend holds back the
  interface and threshold alerts of a device that is not answering, so the
  device page leads with the one alert and says in a sentence why the thirty
  dead ports below it are not thirty problems.
* **Nothing counts alerts twice.** The tiles use `alerts.summary` only. The
  per-device column on Health narrows the *same* open-alert list the Alerts tab
  shows, grouped by device, so the two can never disagree; nothing is derived
  from interface rows or thresholds.
* **Four different silences, four different words.** "Up", "Down", "Not
  polled" (monitored, nothing yet) and "Not monitored" are separate states
  with separate tones, and a rate that could not be measured says "Not
  measured" rather than 0 b/s. `vocab.ts` owns the wording so a cell cannot
  drift from a tooltip.
* **The whole screen is gated on `app.methods`.** Monitoring is a feature
  crate; without it the screen says so in one sentence and nothing else
  breaks.
* **Health reads the newest value of a metric with `metrics.top`, once per
  metric**, instead of `monitor.device` per row: one SQL query answers for
  every device. The round-trip sparklines come from one `metrics.query_many`
  over the rows on screen, capped at the 200 series the backend accepts; a
  device that is not pinged says so instead of showing a flat line at zero.
* **The interface table filters, sorts and pages in the backend** (it is the
  one list that can be a hundred thousand rows). Only the alert list is
  narrowed locally, by a search box that says it searches what was fetched.
* **The rule editor tests what is on screen, not what is stored**: it sends
  the rule as a `rule` object with no `id`, because `alerts.rules.test` prefers
  an `id` and would then evaluate the saved version.
* **One device page, reached by `?device=<id>`** rather than a route of its
  own: `monitoring/:tab?` has one parameter and the route table belongs to the
  shell. It keeps the link shareable.

## Things the design system and the shell might want

* `design/Switch` draws its track over the real `<input>`, and when the switch
  has no label text beside it (a table cell) Playwright can never click it —
  the thumb intercepts the pointer for ever. The spec toggles it from the
  keyboard instead. `design/RadioGroup` has the same shape of problem
  (`pages/devices/NOTES.md`).
* `SecretField` is now written twice, here and in `pages/settings`. Two
  screens need it, which is the bar `spec-ui.md` sets for moving it into
  `design/`; the two copies are the same component with the same contract
  (`undefined` leaves the stored secret alone, `''` clears it).
* The rail badge and the status-bar items can only be registered by a mounted
  screen, so the open-alert count disappears when you navigate away from
  Monitoring. They belong to the shell (`app/builtins.ts`) or to whichever
  slice is always mounted; everything needed is one `alerts.summary` call.
* An `<input list=…>` has the role `combobox`, not `textbox`: worth a line in
  the design system's notes next to the two Field traps already recorded.

## Known gaps

* **Logs and Insights**, which `spec-ui-pages.md` lists as tabs of this
  screen, are not here: neither crate is in this build (`nocturne-logs`,
  `nocturne-insights` are not registered by the suite), and there is no honest
  way to build a screen for a backend that does not answer. The tab list in
  `index.tsx` is the one place to add them, and both should be gated on
  `app.methods` the way the screen itself is.
* An alert raised by a rule whose metric is a glob (`if.*_err_ps`, which is
  how the built-in interface-errors rule is written) cannot be charted: the
  alert records the instance but not which of the metrics matched. The drawer
  says that rather than guessing between errors in and errors out. The backend
  could carry the metric on the alert.
* `metrics.top` answers for at most 500 series, so on a network larger than
  that the CPU, memory and loss columns on Health are blank for the devices
  that fall outside the top 500 of each metric. The device's own page always
  has its real value. A `metrics.latest` that took a list of devices would fix
  it.
* The Alerts tab pages by growing its `limit` (100 at a time) rather than by
  cursor. `alerts.list` returns a plain array and the cursor is an offset, so
  this is the same number of rows with one query key instead of accumulated
  pages — but it refetches the whole page each time.
* Charts picks series one device at a time (several devices can be overlaid by
  picking each in turn). A "compare these devices on this metric" short cut
  would be better served by a `metrics.series` that takes several devices.

## What the backend made awkward

* **`monitor.poll_now` needs at least one device id**, so "Poll now" has to
  list every monitored device itself. A `{}` meaning "everything monitored"
  would let the button say what it does without the screen computing the set.
* **The simulator reports its uptime in whole minutes**, and a rate is a
  counter delta over an *uptime* delta, so two CLI reads inside the same
  simulated minute measure no interval and are recorded as a gap. A lab device
  therefore has no rates until a simulated minute has turned over, which is
  why both Playwright files wait 65 seconds between the first two polls.
  Correct behaviour, but it makes "poll twice and look" slower than it reads.
* `monitor.status` has no device interval or per-device poll time in it, so
  "how often is this polled" can only be read from the inventory.

## State at hand-off

* `pnpm --dir ui test` green (522 tests at hand-off, 22 of them new here:
  `series.test.ts`, `vocab.test.ts`, `datetime.test.ts`) and `vite build`
  clean. `pnpm --dir ui build` itself is red for a reason outside this slice:
  its `typecheck` step covers `tests/`, and `tests/config.spec.ts` (the Config
  screen, mid-write) has an unused variable. `tsc -p tsconfig.json` and every
  file of this slice are clean.
* `tests/monitoring.spec.ts` (10 tests) drives a real `nocturne-server` with
  the demo campus lab, a really unreachable device and a real local webhook
  listener; every test asserts the console is clean.
* `tests/monitoring.shots.ts` writes 19 PNGs per theme (38 in all) into
  `tests/shots/<night|day>/` — 1440×900 and 1280×720, dock open and closed,
  every tab, both drawers, the device page and a chart with a real hole in it.

## Log

* Read the specs, the monitor crate and its notes, then drove a real
  `nocturne-server` with the demo lab from the shell to learn the live shapes
  before writing any component (which is how the minute-resolution uptime and
  the "a webhook channel cannot exist without its URL" rule were found).
* Built the logic modules first (`series.ts`, `vocab.ts`, `datetime.ts`) with
  their tests, then the tabs, then the editors.
* Looked at the screenshots in both themes and fixed what they showed: the
  header read "Monitoring › Monitoring" over a device instead of the device's
  name; the cause banner ran the device's own error into the next sentence;
  the reachability chart labelled four axis ticks "Down" (an up/down axis has
  two labels, and the ticks between them are now blank); an all-zero error
  chart printed "0.0/s" five times (per-second values under one now keep a
  second decimal); every `Select` stretched the width of the toolbar; the
  "Seen", "Memory" and "On" column headings were cut to "S…", "Me…" and "(";
  the four tiles broke 3-and-1 beside the dock at 1280; the alert drawer told
  a feature-raised alert that "the rule behind this alert is gone" and said
  "1 times"; and the rule's test result was below the fold in the shot.
* **Another agent's build tree is hard-linked to this one** (`/tmp/labbuild`
  shares inodes with `ui/src`), and a stub it wrote for its own build replaced
  `RuleDrawer.tsx`, `MaintenanceDrawer.tsx` and `ChannelDrawer.tsx` through the
  link. They were rewritten and the links in this folder broken (`cp` then
  `mv`) so it cannot happen again from the same direction.
