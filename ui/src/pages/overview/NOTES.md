# Overview screen — working notes

Author: screens-overview slice.
Owned paths: `ui/src/pages/overview/`, `ui/tests/overview.spec.ts`,
`ui/tests/overview.shots.ts`. Nothing outside them was touched.

## Shape

```
index.tsx            the screen: states, toolbar, freshness, first run
overview.ts          all the reasoning, with no React in it          (unit tested)
overview.test.ts     46 tests, most of them about null-versus-zero
data.ts              the one read, the events, the three writes
Tiles.tsx            the five readings, or the reasons there are none
NeedsAttention.tsx   the merged, ranked list, and what it could not look at
Running.tsx          jobs running now, with progress and a stop
Activity.tsx         the merged audit / alert / change feed
FirstRun.tsx         an empty inventory: two doors, no zeroes
Unavailable.tsx      one way of saying "there is no number here", and why
```

Methods: `overview.get` (everything), `devices.list` (only for the *names* of
the devices that are down), `app.methods` (is there a simulator in this
build?), `alerts.ack`, `jobs.cancel`, `labs.create` + `labs.start` (the demo
door). Events: `devices.state`, `devices.changed`, `alerts.changed`,
`alerts.raised`, `changes.changed`, `jobs.changed`, `config.changed`,
`insights.changed`, `incidents.changed`, `audit.appended`.

## Decisions

* **`null` and `0` are different shapes, not different numbers.**
  `readPart(overview, 'alerts')` returns either `{present: true, value}` or
  `{present: false, reason, message}`, and a tile built from the second has no
  `value` field at all. A missing source therefore *cannot* be formatted as a
  zero by mistake: there is nothing to format. On screen the difference is a
  figure and a green "Nothing firing" against a dashed tile with no figure,
  the headline "Not in this build" / "Nothing recorded yet" / "Could not be
  read" and the backend's own sentence underneath. The reasons get five
  different headlines and two tones (a feature left out of the build is a fact
  about the build; one that failed or timed out is something to look into).
* **The specification says an unavailable tile is "hidden, not zeroed"; this
  one is shown, with its reason.** Hiding it keeps the no-zero promise but
  loses everything else: a person cannot tell a compliance score of 100 % from
  a build that cannot measure compliance, and the most useful thing the tile
  can say ("no compliance run has been made yet") is the thing hiding throws
  away. The figure is never drawn, which is what the rule is for.
* **A capped count says "at least".** `truncated: true` puts a marker in front
  of the figure with the reason on hover, and — this one matters more — the
  "worst severity" line changes from "Worst: critical" to "Worst **seen**:
  medium (at least 500)" plus "Counted from a capped list; there may be
  worse", because the severities were counted from the part of the list the
  backend read. Claiming the worst of a sample is the worst of the estate
  would be the same lie in a quieter voice. The attention list gets a row for
  everything open that it does not show ("at least 500 further open alerts").
* **Each fact has exactly one home.** The first draft said "4 devices have
  never been backed up" on the backups tile *and* as a row in Needs attention
  *and*, once the insight scan had run, as four per-device rows. The tiles now
  carry the totals; the list carries individual things with an action. The
  compliance row is the one summary row that stayed, because its tile shows a
  score rather than what is failing.
* **A device that is down and an alert saying so are one problem.** When an
  open critical or high alert names a device the inventory reports down, the
  device row is dropped: the alert carries the same fact plus its own words,
  its time and an Acknowledge. A device that is down with nothing firing about
  it keeps its row — which is also how "nobody is watching this" looks.
* **The time the data was read is on screen three ways**: in the toolbar as a
  live relative time with the absolute time on hover, in the status bar, and —
  past `STALE_AFTER_MS` (two missed refetches) — as a banner saying the
  picture may have stopped moving. This is the only way this screen can really
  mislead, so it is the one thing stated more than once.
* **`refresh: true` on every read, and the screen throttles itself.** The
  backend caches an overview for five seconds so several panes cost one read.
  This screen is the only pane, and it re-reads *because an event said
  something changed* — a cached copy from before that event is precisely the
  wrong answer, and would leave the screen a minute behind. So `data.ts` asks
  for a fresh read, collapses an event burst into one read (400 ms) and never
  reads more often than every 3 s.
* **One slow refetch, for the things that change with the clock.** 60 s, and
  the comment in `data.ts` says what it is for: the rolling 24-hour counters
  (`applied_24h`, `failed_24h`, `changed_24h`), the age of the last compliance
  run, and the age of the reading itself. No event announces any of them;
  there is no `compliance.changed` topic at all.
* **The inventory is a convenience, not a dependency.** `devices.list` is read
  only to name and link the devices that are down. If it fails the screen does
  not: the list says "N devices not answering — the inventory could not be
  read, so they cannot be named here", and says so in the gaps section too.
* **Sites are named only when something is down there.** The toolbar line used
  to say "4 sites, none with a device down", which reads as "all well" on an
  estate where nothing has been polled. It now says where the trouble is, or
  nothing at all; up, down and never-polled are the devices tile's job.
* **Rows link, buttons act.** "Open" and "Review" are real `<a>` elements with
  an `aria-label` carrying the row's title, because a NOC engineer opens three
  of them in tabs before touching the first. Only Acknowledge and Cancel are
  buttons, and Cancel asks first, saying that work already done is not undone.

## What is missing, and why

* **The topology thumbnail** `spec-ui-pages.md` asks for at the bottom is not
  here. `topology.get` would answer it, but drawing it means a second
  renderer for a picture that is already a nav click away; the space is spent
  on the attention list instead. If it is wanted, it belongs in a
  `Thumbnail.tsx` reading `topology.get` with `invalidateOn: ['topology.changed']`.
* **The third first-run door, *Discover my network*, is not offered.** The
  engine has the whole `discovery.*` surface, but this build's interface has
  no discovery screen — no route in `app/routes.tsx`, no page folder — so the
  door would lead nowhere. Two doors that work beat three with a dead one. The
  moment a `/discovery` screen lands, the door is four lines in `FirstRun.tsx`.
* **No per-alert deep link.** Monitoring takes `?device=`, not `?alert=`, so an
  alert row opens that device's monitoring page (or the Alerts tab when the
  alert names no device).
* **Insights rows cannot open the finding.** There is no insights route in this
  build either (`pages/insights` exists with no route), so an insight row opens
  its device.
* **No stale-banner screenshot.** It needs a reading 150 s old, which can only
  be staged by letting a browser tab sit still for two and a half minutes or by
  lying about the clock. The unit test covers `freshness`.

## What the backend made awkward

* **`alerts.summary` is unreadable to `nocturne-ops`.** The monitor answers
  `{open, by_severity, newest: Alert | null}` — one alert, or null — while
  `sources::alert_summary` expects `newest: [Alert]`, so the parse fails as
  `malformed` and the overview silently falls back to `alerts.list`. The visible
  consequence: `from` is always `alerts.list` in this build, and the open count
  becomes a capped floor at 500 instead of the exact number the summary knows.
  Everything stays honest — the screen says "at least" — but an exact count is
  available and is not being used. One of the two shapes should move.
* **`compliance` reports `no_data` with the message "no compliance run has been
  made yet" after a run that found nothing to check.** `sources::compliance`
  treats "no score and no findings" as "never ran", so an estate whose only
  device has no backup gets a sentence that is not true. The screen quotes it,
  because paraphrasing another feature's reason is worse. A run that happened
  and decided nothing is a third state and the message should say so.
* **The overview's five newest alerts are the newest, not the worst.** With
  more than five open, the critical one can be missing from `newest` entirely
  (the capped screenshot shows exactly that: 500 medium alerts push both loud
  ones out). The screen says how many are not shown, but a `worst: [AlertRef]`
  beside `newest` would let the list show what actually matters.
* **The devices part has counts but no names.** `down: 2` with no ids means a
  second call to `devices.list` to say *which* two, which is the one extra read
  on this screen. A handful of `DeviceRef`s for the devices that are down would
  remove it.
* **Nothing says when a compliance run happens.** There is no event topic for
  it, so the compliance tile is the one part that depends on the slow refetch
  (and on `jobs.changed`, which fires because a run is a job — true today, not
  a contract).
* `overview.get` is the only method this screen needs, and that is worth
  saying: every tile, every row and every feed line on it came from one call.

## Things the design system or the shell might want

* **A `Stat` that can say "there is no reading".** Every screen that shows a
  number from a nullable source needs this, and `design/Stat` can only take a
  formatted string — so this folder has its own tile. A `Stat` with an
  `unavailable={{reason, message}}` prop would stop the next screen inventing
  a zero.
* **A link that looks like a button.** `design/Button` always renders a
  `<button>`, so a row action that navigates is either a styled `<a>` (as here
  and in `pages/monitoring`) or a button with `useNavigate`, which breaks
  middle-click. An `as`/`asChild` prop would settle it.
* **`useHasMethod` is now in four page folders** (devices, insights, changes,
  and inlined here as one `useRpc('app.methods')` call). It belongs in `api/`.
* **The shell has no jobs drawer**, although `spec-ui.md` describes one in the
  status bar. Until it does, this screen's Running panel is the only place a
  running job can be seen or stopped — which is why Cancel is here.
* Playwright traps met here, in addition to the ones the other screens record:
  a visually-hidden span inside a link duplicates the row's text and breaks
  `getByText` in strict mode (use `aria-label` instead); and CSS container
  queries in a CSS module obey source order, so a `@container` block written
  above the rule it overrides does nothing.

## State at hand-off

* `pnpm --dir ui test`: 614 passing, 46 of them this slice's
  (`src/pages/overview/overview.test.ts`).
* `pnpm --dir ui build` clean (it was red for a while mid-slice because
  `src/pages/automation` was being written at the same time; that cleared).
* `ui/tests/overview.spec.ts`: 10 tests against a real `nocturne-server` with
  real devices, a real simulated switch, a real alert, change, backup,
  compliance run, insight scan and discovery job. Every figure asserted on
  screen is compared with what `overview.get` itself answered — no number in
  that file is a number a screen is expected to show. Every test asserts the
  console is clean.
* `ui/tests/overview.shots.ts`: 20 PNGs into `tests/shots/overview/<night|day>/`
  — first run, nearly empty, working, running, capped, at 1440×900 and
  1280×720, with the dock open and closed. Two engines, because "nothing has
  ever happened here" cannot be staged on one where something has.

## Log

* Read the specs, `api/types/ops.ts`, `crates/nocturne-ops/{NOTES.md,src/overview.rs,src/sources.rs}`
  and the three built screens' notes, then drove a real server by hand to see
  what `overview.get` actually answers in an empty build, a seeded one and one
  with more alerts than it will count. Every shape on this screen comes from
  one of those answers.
* Built `overview.ts` and its tests first, then the components around it.
* Looked at the screenshots in both themes and both sizes, and changed what
  they showed: the status bar said a reading taken a moment ago was "in 7s"
  (the `useNow` tick is up to ten seconds old, so the age is now measured at
  render); "1 device fail 9 compliance checks"; a down device with a blank time
  where it had never once answered (now "never seen"); the activity feed's
  audit lines headed "Did"; "0 changed in 24 h · oldest just now"; the same
  backup fact in three places at once; a dead device listed twice, once as
  itself and once as the alert about it; "4 sites, none with a device down" on
  an estate nobody was polling; a "Nothing running" empty state that took as
  much room as a problem; tiles whose figures floated in the middle of the box
  when the tile above had more notes; the five tiles taking three rows and
  pushing the list off the screen beside the dock at 1280; and the attention
  rows going three lines tall at the same width.
