# Logs — build notes

The Monitoring screen's **Logs** tab. Specification: `docs/dev/spec-ui-pages.md`
(§ Monitoring) and `docs/dev/features/logs.md`. Backend: `crates/nocturne-logs`
(read its `NOTES.md` first — most of the decisions here are the other half of
one of its). Types: `src/api/types/logs.ts`.

## What it is

```
index.tsx       AREA, <Logs/> (the tab body) and the default export (a page)
data.ts         every query and mutation the screen makes, in one place
query.ts        severity vocabulary, the severity bands, the query language, spans
coverage.ts     gap-or-zero, what a severity filter hides, what "nothing" means
stream.ts       the list's model: a page, older pages, the tail, the holes
entry.ts        reading one stored line defensively (two clocks, three bodies)
useStream.ts    the live tail (logs.follow) and paging back
Histogram.tsx   stacked bars, gaps as gaps, what the filter hides as outlines
LogList.tsx     the virtualised list and the stick-to-the-bottom behaviour
EntryDrawer.tsx one line in full, with both clocks
SetupHint.tsx   the receiver's state, the exact device commands, unclaimed addresses
```

Four unit-tested modules carry the reasoning (`query.test.ts`,
`coverage.test.ts`, `stream.test.ts` — 45 tests): parsing the query bar,
deciding whether an empty histogram bucket is a zero or a gap, deciding whether
"nothing here" means quiet or not configured, and keeping the list honest when
the tail samples or the view fills up. None of them touch React.

## Decisions worth remembering

* **An empty bucket is only a zero where the store can prove it.**
  `logs.histogram` returns every bucket of the range, including the empty ones,
  and cannot say *why* one is empty. `bucketReach` decides from `logs.status`
  alone: a bucket older than `store.oldest_at` is a gap (retention took it, or
  nothing was being sent yet), a bucket while nothing is listening is a gap, a
  bucket after now is "not yet", and everything else is a measured zero — drawn
  as a tick on the baseline, so a measurement reads as one. The hatched gaps
  are counted in a sentence under the chart, the way `pages/monitoring` says
  how much of a line is missing.
* **The histogram is fetched without the severity filter.** It is the only way
  the screen can say "180 lines in this range · 90 shown (warning and worse)"
  and mean it, and it lets the chart draw what the filter hides as an outline
  in the band's own colour instead of quietly shrinking. A severity filter also
  hides every line that *gave* no severity — `severity <= 4` is false for NULL
  in SQL — so those are counted separately and named in the sentence.
* **The list is oldest-at-the-top.** `logs.query` answers newest first because
  that is what paging back wants; a tail that sticks to the bottom wants the
  opposite. `stream.ts` reverses once, at the edge.
* **Pinning is derived from one number.** How far the viewport is from the
  bottom. Scrolling up unpins, scrolling back down pins again, and the code
  that follows the tail never has to know whether a scroll was the reader's or
  its own. Prepending older lines compensates `scrollTop` by the rows added, so
  the reader's place does not move.
* **Switching the tail on re-reads the newest page.** A follow only ever
  delivers lines that arrive *after* it opens; starting a live tail on top of a
  page read ten minutes ago would leave a hole nobody could see.
* **A page adopts what the tail delivered while it was being read** (`adopt`).
  A page is a snapshot of the store when the query ran; a line stored a moment
  later is in neither the page nor a view that simply replaced itself with it.
  A page of a *different* search adopts nothing — those lines were chosen by a
  filter that has just changed. (This was a real hole: the end-to-end test for
  the live tail caught it as a line that arrived and vanished.)
* **The tail is filtered by the engine** (`logs.follow`), not in the browser.
  At twenty thousand lines a minute the difference is a tail and a frozen tab.
  The follow is renewed every 45 s against a two-minute lease and closed on
  unmount; a tab that crashes stops costing the engine a filter pass per line
  by itself.
* **What the tail sampled away gets a row of its own.** Over 200 lines in a
  quarter of a second the engine sends an even sample plus a count. A list that
  dropped that count would make a storm look like a trickle.
* **The list filter has no `until` on a relative range.** The same filter is
  the follow's, and an upper time bound on a live tail is a tail that silently
  stops delivering. The chart keeps its `until` — rounded *up* to the end of
  the running bucket, so a line that arrives a second after the screen opened
  is still in the picture.
* **Two clocks, never mixed.** `ts` is when Nocturne received the line; what
  the device wrote is text in `body.timestamp`. A pulled buffer is the
  exception the backend documents (`body.ts_source === 'device'`), and those
  rows are marked in the list (amber time, a `d`) and spelled out in the
  drawer. A simulated switch pulled in the Playwright suite thinks it is March,
  which is exactly the wrong-clock case a real estate is full of.
* **Nothing arriving is four different states.** `receiverVerdict` separates
  switched off, enabled but unable to bind (with the engine's own error), never
  been sent anything, and nothing since this process started — and only then
  does the screen offer the setup hint. A filter that matches nothing is not a
  setup problem and says something else entirely.
* **The setup hint shows real commands for a real device**
  (`logs.device_setup`): the platform's own syntax, this installation's port,
  and the address of this machine as that device would reach it. They are not
  applied and the note says so. Where a device cannot send, the same panel
  offers `logs.pull`, which reads the buffer it keeps for itself.
* **`undefined` from `useHasMethod` waits.** A screen that flashes "not in this
  build" and then draws itself is worse than a skeleton for one round trip.

## What I deliberately did not call

* **`logs.clear`** — deleting a stream is a destructive, audited act that
  belongs with retention in Settings, not behind a button on a reading screen.
* **`logs.top` and `logs.bursts`** — "this happened 240 times" is the right way
  to read a storm, but the specification's brief for this tab is a query bar, a
  histogram, a list and a tail. A *Bursts* view is the obvious next addition and
  both methods are already typed in `api/types/logs.ts`.
* **`logs.around`** — the drawer's "Show the minute around this" sets the time
  range instead, which needs no second list and keeps one screen. `logs.around`
  earns its keep when alerts and changes are shown beside the lines (its
  `Borrowed` states are made for that), which is an Alerts-drawer job.
* **`logs.rules.*`** — log rules are the Monitoring screen's **Rules** tab, not
  this one. Nothing here edits them.
* **`logs.restart`** — the receiver follows its settings by itself, so the
  switch-on button is a `settings.set` and the engine does the rest. A
  "restart" button would only paper over a bug.
* **`flows.top`** — not registered in this build (the crate says so), and the
  screen says traps and flows are *absent* rather than switched off.
* **Severity as a multi-select.** `max_severity` is a threshold in the backend,
  and offering a set of checkboxes that cannot be sent would be a lie about
  what the filter does.

## What the backend or the design system made awkward

* **`logs.histogram` cannot say why a bucket is empty.** Everything in
  `coverage.ts` exists to infer it from `store.oldest_at`, `store.rows` and
  `syslog.listening`, and the inference is weaker than the truth: a receiver
  that was off for an hour yesterday and is on now is indistinguishable from a
  quiet hour. A `received_from`/`listening_since` in `logs.status` (or a
  `covered_since` on the histogram) would make the judgement exact instead of
  conservative. **Worth asking for.**
* **Lab syslog bypasses this feature's ingest.**
  `crates/nocturne-lab/src/runtime.rs:516` writes a simulated device's lines
  straight into the core `logs` table with `source = "%FACILITY-SEV-MNEMONIC"`
  and `body = {line, lab_id, sim_time}`. Those rows have no `logs_syslog` index
  row, so the mnemonic, facility and interface filters never match them,
  `logs.bursts` (an inner join) cannot see them, and the body has no `raw`,
  `via` or `mnemonic`. `entry.ts` reads every field defensively because of it
  (`rawOf` falls back to `body.line`, `viaOf` reports `lab`), and
  `api/types/logs.ts` now documents that `SyslogBody.raw` is not guaranteed.
  The honest fix is in the lab crate: send those lines through
  `nocturne_logs::ingest::prepare` like any other. **A defect, not a quirk.**
* **`logs.status` has no configured port while the receiver is off.**
  `listening` is empty, which is correct, but the hint has to name the port it
  is about to open — so the screen reads `settings.get {section: 'logs'}` as
  well. A `configured: {udp_port, tcp_port, bind}` block in the status would
  save every caller that round trip.
* **The follow lease is shorter than a coffee break.** Two minutes with a 45 s
  renew is fine for a focused tab; a backgrounded tab whose timers are throttled
  loses its follow and silently stops receiving. The screen would catch it on
  the next renew, but an expiry event (or a longer lease while the socket is
  connected) would be better than polling.
* **No bar chart in the design system.** `TimeSeriesChart` is uPlot and lines;
  a histogram of discrete buckets with a stacked severity breakdown, hatched
  gaps and clickable bars is a different instrument, so this folder draws its
  own with flex boxes and tokens. If a second screen ever wants one it should
  move to `design/`, not be copied.
* **`Table` is sorted and virtualised but not append-friendly**, which is why
  the line list is its own `@tanstack/react-virtual` list: it needs fixed rows,
  no sorting, prepend-without-jumping and stick-to-bottom, none of which belong
  in a general table.
* **One Playwright file may not import another**, so the lab, the harness and
  the line shapes are duplicated between `tests/logs.spec.ts` and
  `tests/logs.shots.ts` — the same duplication `insights.shots.ts` carries.

## Needed from outside my paths

* **Mount the tab.** `pages/monitoring/index.tsx` should add
  `{ id: 'logs', label: AREA.label, icon: AREA.icon }` to `TABS` and
  `{current === 'logs' && <Logs />}` to the panel, importing
  `{ AREA as LOGS, Logs } from '@/pages/logs'`, exactly as it does for
  Insights, and offer the tab only when `useHasMethod('logs.query') !== false`.
  Until then the Playwright specs mount `/logs` through their own harness
  (`buildHarness`); when the tab lands, both files can drop the harness and use
  `startBackend({ uiDir: 'dist' })` with `/monitoring/logs`.
* Nothing else. The screen uses only existing design-system components, the
  shared hooks and `lib/`.
