# Logs — build notes

The Monitoring screen's **Logs** tab (`/monitoring/logs`). Specifications:
`docs/dev/spec-ui-pages.md` (§ Monitoring), `docs/dev/features/logs.md` and
`docs/dev/features/detect.md`. Backends: `crates/nocturne-logs` and
`crates/nocturne-detect` (read their `NOTES.md` first — most of the decisions
here are the other half of one of theirs). Types: `src/api/types/logs.ts` and
`src/api/types/detect.ts`.

## What it is

Six panes over five engine pieces, each pane offered only when the method that
fills it is registered (`panes.ts`):

```
index.tsx           AREA, <Logs/> (the tab body: the pane tabs) and the default export
panes.ts            what this build can do, from app.methods: one capability per method
data.ts             every query and mutation the tab makes, in one place

Lines               the store: search, histogram, list, live tail, every stream
  LinesPane.tsx     the pane (what index.tsx was before the panes)
  query.ts          severity vocabulary and bands, the query language, spans, streams
  coverage.ts       gap-or-zero, what a severity filter hides, which receiver fills a stream
  stream.ts         the list's model: a page, older pages, the tail, the holes
  useStream.ts      the live tail (logs.follow) and paging back
  entry.ts          reading one stored line defensively (two clocks, three bodies)
  trap.ts           reading one stored trap: no severity, the claimed agent address
  Histogram.tsx     stacked bars, gaps as gaps, what the filter hides as outlines
  LogList.tsx       the virtualised list and the stick-to-the-bottom behaviour
  EntryDrawer.tsx   one line in full with both clocks; a trap field by field
  SetupHint.tsx     the receiver behind the stream: off, failed, silent — and the addresses no device claims
  DeviceSetup.tsx   logs.device_setup: syslog lines and trap lines, the community a placeholder

Receivers           syslog, traps, flows
  ReceiversPane.tsx a card each, the store, what a device has to be told
  receivers.ts      the cards from logs.status without adding a number; the port form

Rules               patterns, thresholds, clearing
  RulesPane.tsx     the list and what the rules hold in memory
  RuleDrawer.tsx    the editor; a built-in's definition and its test line; the test box
  rules.ts          draft ↔ params, which field an engine refusal is about, the phrases

Forwarding          handing what is stored to a log platform
  ForwardingPane.tsx, ForwarderDrawer.tsx, forward.ts, WriteOnlyField.tsx

Flows               top talkers from the collector's memory
  FlowsPane.tsx, flows.ts

Detections          the catalogue and its hits
  DetectionsPane.tsx, detections.ts

fixtures.ts         answers a running nocturne-server gave, for the unit tests only
```

Every module with reasoning in it has a `*.test.ts` beside it. None of them
touch React. `fixtures.ts` is imported by tests and by nothing else; each
answer in it is checked against its type with `satisfies`, so a field the
type lacks fails the build.

## Decisions worth remembering

* **A control exists only when its method does.** `capabilitiesOf(app.methods)`
  gives one flag per method this folder calls, and every pane, card, switch,
  button and row action reads its flag. A build without `flows.*` has no
  Flows tab, one without `logs.forward.save` has a read-only forwarder list,
  and nothing on screen can answer `unknown_method`. `panes.test.ts` checks
  every combination of the six pane methods and every single missing method.
* **A counter the engine does not give is not shown.** `receivers.ts` reads
  each counter by name and yields a row only when the status holds a number
  there. The flow collector has no `stored` (nothing of a flow is), so its
  card has no "Stored"; a forwarder the status does not list is "Not
  reported", not zero. The queues' sizes in bytes (`queued_bytes` against
  `max_queued_bytes`, and a forwarder's `queued`) arrived in the engine while
  this was being built: they are optional in the types and shown where the
  status has them, so an engine without them loses a row and nothing else.
* **Off, failed and absent are three states**, on the cards and in the Lines
  pane's hint: switched off is a choice, enabled-and-not-listening carries the
  engine's own reason, `implemented: false` is a build without it. The hint
  follows the stream: the trap stream is about the trap receiver.
* **A trap has no severity.** Not "info", not the dash of a syslog line that
  came without a priority: the cell is empty (`data-severity="none"`) and the
  drawer says "none — an SNMP notification carries no severity". A v1 trap's
  agent address is shown as what the agent *claims*, beside where the datagram
  came from.
* **The community in `trap_lines` is a placeholder** and the setup block says
  so before the engine's own note. Nothing on any pane holds a community: the
  e2e test searches the page for the right one and the wrong one.
* **Write-only means never shown.** A forwarder's URL and token are typed into
  `WriteOnlyField`, masked, sent once and never read back; editing says that
  one is stored and offers Replace / Remove / Keep. A new URL does not inherit
  the stored token, and the form says so before the save (`tokenFate`).
* **The engine's refusal stands beside the field it is about**, unchanged.
  `fieldOfRuleError` and `fieldOfForwardError` recognise the engine's
  sentences by how they begin; one they do not recognise goes above the form,
  whole. The only thing refused before sending is a number field that does
  not hold a number.
* **Sampled flow figures are labelled, never multiplied.** The coverage stands
  beside the table, not under it: collecting or not, since when, records, sets
  without a template, the rates exporters said and how many records came with
  one. By interface every flow is counted in and out, so those rows get no
  share of the total.
* **A detection's lab hit is not promised silence.** Who is told is the
  monitor's `notify_for_lab`, decided in one place for every source of
  alerts; the hit drawer says "sent to no notification channel, unless
  Monitoring is set to notify about lab devices".
* **Only a scroll upwards unpins the list** (`scrollVerdict` in `stream.ts`).
  A scroll event is read when it is dispatched, which can be after the tab
  above the list grew (an alert badge, the "lines have arrived" banner): the
  list's own jump to its end then reads as "8 px, 30 px from the end" and, with
  pinning derived from distance alone, the screenshots run opened the tab
  unpinned now and then. A pinned list whose end moved without the reader
  goes back to it.
* **The list takes what the tab leaves it.** Inside Monitoring (status strip,
  two rows of tabs) a 260 px minimum pushed the list past the bottom of a
  1280×800 window, so the newest line of a live tail was below the fold. The
  list's minimum is 160 px, the filter bar is narrow enough not to wrap at
  that width, and the chart is 72 px tall on a short screen.
* **A switch keeps the focus while its change is on its way.** The receiver,
  community, rule and detection switches used to be `disabled` until the
  engine answered, and disabling the control that has the focus drops the
  keyboard's focus to the page: a second Space went nowhere, and a keyboard
  user started again from the top. They stay enabled, are marked
  `aria-busy`, look busy (`.busySwitch`) and ignore input until the answer.
  "Replace…" on a write-only field likewise puts the focus in the new box,
  not on "Keep what is stored", which takes its place in the tree.
* **What the form hides is not sent.** A rule switched from an alert to
  another signal keeps what its quiet period and its opposite held, for a
  change back, and sends neither: the engine would refuse the rule for text
  that is no longer on the screen. A number too large for the engine's
  32 bits is said to be too large, not "not written in digits".
* **In a narrow window the column that matters stays in view.** A
  forwarder's State is the second column, because at 1280 with the dock the
  table scrolls sideways. A sortable header takes 52 px beside its text, and
  the flows tables' numeric columns are their headers' width and no more, so
  no header is cut off at 1440 or at 1280 with the dock.
* Everything from the earlier notes still holds: an empty bucket is a zero
  only where the store can prove it, the histogram is fetched without the
  severity filter, the list is oldest-at-the-top, the tail re-reads the newest
  page when switched on and is filtered by the engine,
  what the tail sampled away has a row, two clocks are never mixed.

## What I deliberately did not call

* **`logs.clear`** — destructive and audited; it belongs with retention in
  Settings, not on a reading screen.
* **`logs.top`, `logs.bursts`, `logs.around`** — typed, not used. A Bursts
  view is the obvious next addition.
* **`logs.ingest`** — the lab's, not a person's. The e2e test gets lab lines
  the way a lab does: by typing at a simulated switch's console.
* **A per-alert link.** A hit links to the alert list (`/monitoring/alerts`);
  that tab has no address for one alert to open.

## What the backend made awkward

* **A second edit on one device within a minute is no detection hit.** The
  rule that turns `%SYS-5-CONFIG_I` into `logs.config_changed` has a 60 s
  throttle per device, and `unplanned_config_change` is decided from that
  signal. Every line is stored; the hit list shows one per device per minute.
  The e2e test configures a second lab switch for its detection case.
* **`logs.histogram` cannot say why a bucket is empty**, and `logs.status`
  has no `listening_since`: a receiver that was off for an hour yesterday is
  indistinguishable from a quiet hour. `coverage.ts` infers conservatively.
* **No event for flows.** The collector publishes nothing per minute, so the
  Flows pane asks again every 30 s and on demand.
* **`detections.changed` carries no catalogue**, so the list is asked again;
  a hit arrives whole on `detections.hit` and goes into the cached list.

## Tests

* Unit (`vitest`): every pure module here.
* End to end (`tests/logs.spec.ts`, real `nocturne-server`, no stubbed
  answer): syslog over UDP from the test process, a lab switch configured from
  its console (lines through `logs.ingest`), net-snmp's captured traps to the
  port the engine reports, hand-written NetFlow v5/v9 datagrams, a TCP
  collector in the test process, and the detection on a lab switch. Three
  checks at the end: after a forwarder is saved, its URL, its token and any
  community are in no RPC answer or event the page receives, no pane's
  markup or fields, and neither storage; a log line, a hand-built trap and an
  exporter's device name full of markup and a 50,000-character run stay text
  and open in well under a second; and two builds that lack features — no
  `flows.*`, `detections.*` or `logs.forward.*`, and every list with no
  write — offer only what they have and never call what they lack. Those two
  are the one place `app.methods` is not the server's answer as it came: the
  absent names are taken out of it on the way to the page, because a build
  without them cannot be started here; every other call is the engine's.
* Shots (`tests/logs.shots.ts`): every pane in both themes, at 1440 and at
  1280 with the dock open, under `tests/shots/logs/{night,day}`.

## Needed from outside my paths

* Monitoring's Alerts tab: an address that opens one alert
  (`?alert=<id>`), so a detection hit can link to its alert and not only to
  the list.
* `docs/dev/features/logs.md` says `logs.follow` takes `{filter, follow_id?}`
  and answers `{id}`. The engine takes the filter's fields at the top level
  and answers `{follow_id, expires_at, event}`; `types/logs.ts` follows the
  engine, and the specification's line should be corrected to it.
* `types/core.ts`: `Device` has no `state`, which `devices.list` returns.
