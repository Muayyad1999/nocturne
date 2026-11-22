# Topology screen — working notes

Author: screens-topology slice.
Owned paths: `ui/src/pages/topology/`, `ui/tests/topology.spec.ts`,
`ui/tests/topology.shots.ts`. `ui/src/api/types/topology.ts` was already
written by the backend's author and is complete; nothing here needed changing.

## Shape

```
index.tsx         the route: toolbar, warning band, canvas, side panel, overlays
Canvas.tsx        the SVG scene on `design/Viewport`: groups, links, path, nodes, dragging
graph.ts          geometry and, more importantly, `evidence()`       (unit tested)
trace.ts          a `topology.path` answer laid over the drawing      (unit tested)
search.ts         what the one search box does with what is typed     (unit tested)
Legend.tsx        the key: the three confidences, the four sources, the node kinds
Warnings.tsx      the band above the canvas: the backend's own caveats, and the age
Inspector.tsx     one node (with `topology.neighbors`) or one link (leading with how it is known)
PathPanel.tsx     two ends, the hops, and the sentence that says where the data ran out
HostPanel.tsx     `locate.host`: the port it decided on, the sightings it dismissed, the reasoning
ExportDialog.tsx  the five formats, with the text on screen before it is saved
ManualLinks.tsx   the links a person drew: list, draw, edit, delete
```

Methods used: `topology.get`, `topology.views`, `topology.view_save`,
`topology.view_delete`, `topology.layout.save`, `topology.autolayout`,
`topology.neighbors`, `topology.path`, `locate.host`, `topology.export`,
`topology.manual.list` / `.save` / `.delete`; `settings.get` (the `topology`
section, for the staleness limit); `collect.run` and `jobs.get` for "Refresh
data"; `devices.create` for adding an unmanaged neighbour to the inventory.
Events: `topology.changed` (every read is invalidated by it) and
`jobs.changed` while a collection runs.

`topology.changes` is **not** used. The screen it belongs on is a timeline of
what appeared and disappeared, and this screen had no honest place to put one
that was not a second, quieter history screen; it is the obvious next piece.

## Decisions

* **The three confidences are three different claims, so they are drawn three
  different ways and said in three different sentences.** The dash patterns
  are the ones the backend's own SVG export uses (solid, `7 4`, `2 3`), so a
  drawing exported from this screen reads like the screen. On top of that,
  colour carries *where the claim came from* — a neighbour protocol (grey), a
  subnet or a MAC table (blue), a person (accent), the lab (purple) — and
  every link that is not one both ends reported carries the words `one end
  only`, `inferred`, `drawn by hand` or `lab wiring` on the line itself. A
  dash pattern alone is a legend lookup, and this is the part a reader must
  not get wrong.
* **A one-sided link never says which end reported it**, because the backend
  cannot say: `physical.rs` sorts a link's two ends by node and interface
  before it stores them, so `a` is not the reporter. The claim is "only one
  end reported this link; the other has not — it is unmanaged, silent, or has
  never been collected from", and a unit test asserts the sentence does not
  name an end.
* **A link a person drew is `confidence: confirmed`** (the person is sure), so
  going by confidence alone would draw it exactly like a link two devices
  agreed on. `evidence()` therefore looks at `kind` first: manual and lab
  links get their own source, colour and words. There is a unit test whose
  name is that mistake.
* **The legend only explains what is on screen**, and it is a strip under the
  drawing rather than a card floating over a corner of it. The first version
  floated, and the first screenshot showed it covering three devices and the
  one-sided link they were attached to — a key that hides the thing it
  explains. It also leaves out a row that a confidence already covers (a
  subnet link is always `inferred`), and condenses to names and samples alone
  when the canvas is narrow.
* **Positions: every node is sent, not only the one that moved.**
  `topology.get` lays out a node that has no stored position "next to its
  neighbours", so saving one position alone makes the other thirteen nodes
  *new* and they are re-placed around it. Saving one node moved everything
  else on the first try; `positionsOf(nodes, moved)` sends the whole picture,
  with the dragged node pinned (a position a person chose survives an
  automatic layout, which is what `pinned` is for).
* **No position is ever computed here.** The backend's layout guarantees cards
  do not overlap, working in 190 × 64 world units per node, so the drawn card
  is 184 × 52 and that is the whole of the geometry contract. The only things
  worked out in the browser are where a line leaves a card, how far parallel
  links fan apart, and when a line has to bow round a card it is not connected
  to — the backend's own "the access point's link ran straight through two
  cards it was not connected to" mistake, which is a lie about what is on the
  path. `overlapping()` checks the guarantee and warns in the console in
  development; the end-to-end suite checks it over the real positions after an
  automatic layout.
* **The warnings are quoted, never paraphrased.** The band says how many
  things the drawing cannot tell you and opens to the backend's sentences,
  word for word; the end-to-end test reads `topology.get` and asserts each
  one is on screen exactly. The age of the data is in the same band, with
  Refresh next to it, because "this is six hours old" has the same remedy.
* **An incomplete path is not a failure.** `topology.path` stops rather than
  guessing, and its reason is the valuable part: the panel gives
  `stopped_because` its own heading and quotes it, every hop carries the
  device's own `detail` line, and the drawing ends in a dashed stub that goes
  nowhere, labelled "the path stops here". `notes` (equal-cost choices, a
  non-default VRF) are shown, never dropped.
* **`locate.host` is shown with its workings.** The port it decided on, the
  sightings it dismissed under the backend's own words ("the path frames take,
  not where it is plugged in"), and `how` printed in full. When `best` is
  absent the panel says no port could be decided on instead of picking the
  first sighting.
* **"Refresh data" is a `collect.run`**, because the topology feature touches
  no device. It collects from the devices in the drawing, follows the job, and
  lets `topology.changed` bring the new graph in.
* **Adding an unmanaged neighbour** uses `devices.create` behind a confirm that
  names what will be created and says that nothing is connected to until it has
  credentials.
* **The side panel is a column, not a drawer**: a modal over the canvas would
  hide the thing being asked about. It has three tabs — Selection, Path, Find
  host — and the canvas is fitted again when the layer or the view changes.

## Things the foundation (or another slice) might want

* `design/Viewport` fits once on mount, which is always before the first
  `topology.get` has arrived, so the canvas fits again by hand when the
  drawing first has nodes and when the view, layer or endpoint setting
  changes. The Lab screen's notes ask for a `fitKey` prop; this screen is a
  second vote for it.
* ~~`/terminal` takes no device, so the "Terminal" action the page spec lists
  for a node is not offered.~~ **Done in the second pass**, and without a
  route parameter: the map calls `terminal.open` itself, keeps the session in
  the dock under the drawing, and hands it over to the Terminal screen
  through that screen's own layout store when asked. A `/terminal?device=<id>`
  route would still be a tidier way in from anywhere else.
* There is no way to put something in the assistant's context from another
  page (`pages/terminal/assistant.ts` is a page-local store), so "Ask
  assistant" is not offered either.
* `design/Toolbar` wraps rather than condensing — the same note the Changes
  screen left. This screen keeps its toolbar short by using icon-only buttons
  for Arrange, Links drawn by hand and Export, and by hiding the Site and Role
  filters when the drawing has only one of each.
* `plural(count, word)` includes the count. Four places here read
  `{n} {plural(n, 'thing')}` at first and rendered "1 1 thing"; the
  screenshots caught it. Worth a line in its doc comment.

## What looking at it changed

The drawing was rendered, looked at, and fixed — twice. What the pictures
caught, in the order they were found:

1. `plural(n, word)` already contains the count, so four places read
   "1 1 thing this drawing cannot tell you" and "13 13 devices".
2. Every `Select` in the toolbar was full width (the design system's controls
   are `width: 100%`), so the toolbar was three rows tall and the drawing
   started halfway down the screen.
3. The legend floated over the bottom-left corner of the canvas and covered
   three devices and the one-sided link between two of them. It is a strip
   under the drawing now.
4. Interface names were drawn in full: `GigabitEthernet1/0/11` four times
   round one switch, overlapping each other and the links. They are the short
   forms now (`Gi1/0/11`), set off to one side of the line — the mistake the
   backend's notes describe making and fixing in its own SVG.
5. The site group and the lab group hold exactly the same devices, so two
   outlines were drawn on the same rectangle with their labels on top of each
   other. Groups with the same members are one outline with both names.
6. The word on a short link (`one end only`, between two adjacent access
   switches) was drawn half under one of the cards, because links are drawn
   before nodes. The words are their own layer, after the nodes.
7. `Campus/Guest-Laptop1` and `Campus/Guest-Laptop2` were both clipped by the
   card to `Campus/Guest-Laptop`: two different devices with the same name on
   them. Names are cut with an ellipsis now, in the middle, so what tells two
   devices apart survives.
8. The stub that marks where a path stopped pointed off the top-right corner
   and was clipped away by the edge of the canvas — the reason went with it.
   The fit includes it.
9. A host found in a MAC table is named after its address, so its card read
   `10.10.30.100` twice.
10. The warning band had a second "Refresh data" button a few pixels under the
    toolbar's one. It only appears now when the band is about the age of the
    data, and it says "Collect again".
11. At 1280 with the assistant dock open the legend was three rows tall and
    took the height the drawing needed; it condenses there.
12. A drawn link's panel said `Confidence: confirmed` under "A person drew
    this link" — next to links two devices really did report, that word reads
    as the devices confirming it. It now says what it was confirmed by.

## Link load, and a session from the map (second pass)

Two things The Dude does that this drawing did not: live utilisation on each
line, and a command line on the device under the pointer. Both are here now.

```
load.ts           the reasoning: joining links to rates, unknown vs idle, live vs stale,
                  what shows at which zoom                               (unit tested)
session.ts        whether a node can be given a terminal, and why not    (unit tested)
SessionDock.tsx   the terminal itself, borrowed from `pages/terminal/sessions.ts`
```

Methods added: `monitor.interfaces` (the whole table, one call, refetched on
`monitor.interfaces` and `monitor.polled`), `devices.list`, `settings.get`
(the `monitoring` section, for `cli_interval_s`), `terminal.list`,
`terminal.open`, `terminal.close`, and `labs.get` for the one lab a selected
device belongs to. Nothing new was asked of the backend, and nothing was
added to it.

(Superseded for the rates: the screen now asks `topology.rates`, not
`monitor.interfaces`. See "What the review found" below.)

### The four states of a rate, and how each one reads

A direction of a link is in exactly one of four states, and they are four
different facts — so each is drawn a different *shape*, not a different
colour, because at the zoom where the numbers disappear hue would be all that
is left:

| state | on the line | in words | in the panel |
|---|---|---|---|
| measured, with a speed | a channel with a bar in it, as long as the load | `42%` (nothing under 0.1 %) | `24 Mb/s of 1 Gb/s · sent by Gi1/0/1` |
| measured, and zero | an empty channel | `idle` | `0 b/s of 1 Gb/s` |
| measured, no speed | a *dashed* channel, no bar | `24 Mb/s` | "the port did not say how fast it is, so there is no percentage" |
| not measured | nothing at all | `no rate` (italic) | "no rate has been measured for this direction" |

* **Zero is not unknown.** An empty channel means monitoring looked and
  nothing was moving; *no* channel means nobody looked. The legend says both
  in those words ("nothing has been measured — not the same as idle"), and an
  e2e test asserts that the panel for an unmeasured link contains neither
  `0%` nor `idle`.
* **A bar is never shorter than 1.6 %** (a hundred kilobits on a ten-gigabit
  port would otherwise be an invisible bar, i.e. indistinguishable from
  idle), and a measured zero gets **no** bar, so "idle" and "a trickle" are
  not two shades of one shape. A bar is also not drawn at all on a line under
  52 px long on screen: there is nothing there to draw it on.
* **The length carries the number**, through SVG `pathLength="100"`: the dash
  pattern is literally the percentage. That is what the e2e test asserts —
  the first figure of `stroke-dasharray` equals `out_bps / speed_bps × 100`
  as the backend measured it.
* **Both directions, separately**, because a link is rarely symmetrical. Each
  is read from the *sending* port's `out_bps` where that port is monitored
  (the only one of the two counters the wire cannot have thinned), and falls
  back to the receiving port's `in_bps`. The panel says which it used — `sent
  by` or `received by`, with the port's name. The one exception is age: a
  current reading of what arrived beats a stale reading of what was sent,
  because the question on a map is what is happening *now*.
* **Where the numbers go**: not at the midpoint, where the words that say how
  a link is known already are, but a third of the way along the line from the
  end that is sending — which also means a direction needs no arrowhead to
  say which way it is going.

### Live, stale, and how the screen knows

`IfStatus.at` is when the poll that produced a rate ran. A reading is stale
when it is older than `2.5 ×` the cadence its own device is measured at, with
a 90-second floor, and the cadence is worked out as monitoring works it out:
with SNMP it is the device's `monitor.interval_s`; without SNMP the counters
come from a command-line read, which happens no more often than the
`cli_interval_s` setting however often the device is pinged, so the slower of
the two wins. A stale bar keeps its length (it is still a statement about
something that was measured) and loses its colour, the number is greyed, and
the row in the panel carries a `Stale` pill with the reading's own age and
the cadence of the port it came from — "measured 1m ago, and this port is
read about every 5s" — because "stale" against no yardstick is just a scary
word. The cadence shown is that end's, never the link's: a link between a
device polled every half hour and one polled every five seconds has two
different answers, and the one that matters is the one behind *that* number. The stats strip
under the toolbar says `load measured 40s ago` for the newest reading in the
whole drawing — which is the honest version of a screen that claims to be
live.

Nothing is polled by this screen: the rates refetch on `monitor.interfaces`
and `monitor.polled`, and a 15-second tick re-reckons the *ages* (time
passing is not an event, so nothing would otherwise turn a live reading into
a stale one while the map is open).

Ages are reckoned on the **server's** clock (`serverNow` in `load.ts`), never
the browser's: see "What the review found".

### What shows at which zoom

`loadDetail(zoom, lineLengthOnScreen)`, unit tested, decides: nothing below
0.2 (a card is a smudge), bars only below 0.6, one number — the direction
that matters — up to 1.0, both numbers above it. A line shorter than 90 px on
screen never gets writing however far in the map is zoomed, and one shorter
than 150 px never gets two. At the zoom the campus fits at, the map is bars
only; a measured zero, a loaded link and an unmeasured one are still three
different shapes there. The whole layer has a toolbar toggle ("Load",
remembered per browser), because the one thing a map owes you is a way to
stop it shouting.

`worthLabelling` then throws one more thing away: a percentage too small to
write as a number (`<0.1%`) is never written, at any zoom. That rule came
from the screenshots and is described under "What looking at it changed"
below. A number is drawn beside the middle of its own bar, not near the end
that is sending — also from the screenshots.

### A session from the map

Right-click a card → `Terminal`. It is `terminal.open {device_id}`, the
backend's own session, and the xterm instance belongs to
`pages/terminal/sessions.ts` — the registry the Terminal screen owns and the
Lab screen already borrows. So the session survives being hidden, keeps its
scrollback, and the dock's "open on the Terminal screen" button hands over
*that* session (`useTerminalLayout.open(id)` + navigate) rather than dialling
the device again. A device that already has a session gets that one back: two
prompts on one switch is how a person types into the wrong one.

The dock sits under the drawing rather than floating over it — a terminal on
top of the map covers the thing the map is for.

`session.ts` answers *before* the button is offered, and every refusal is a
sentence plus three or four words for the one line a menu item has:

| case | why |
|---|---|
| not in the inventory (`u:`) | "…add it as a device and give it a credential, and a terminal becomes possible" |
| a host from a MAC table (`h:`) | "not a device Nocturne manages" |
| an end somebody typed (`m:`) | "not a device" |
| `transport: none` | "no command line" |
| no address | "no address" |
| no credential (and not a lab device) | "no credential" |
| the lab is not running | "Start the lab and it comes back" |
| the lab node is powered off | "Power it on and it comes back" |
| no terminal feature in the build | "no terminal here" |

A lab device needs no credential and no address — the simulator is the far
end — but it does need the lab running and the node powered, and only the lab
feature knows that, so `labs.get` is asked for the selected device's lab.
Where the lab (or the inventory) has not answered, that is **not** read as a
refusal: the backend has the last word, and a guess that hid the button would
be worse than letting `terminal.open` say so itself.

### The third thing: a right-click menu

The Dude's third habit is that everything is on the right-click. The same
actions the side panel has — Terminal, Open device, Trace from here, Pin, Add
as a device — are on a context menu on every card, because the pointer is
already there and because a map you have to travel to a panel to act on is a
picture, not a tool. It is a shortcut, never the only path: every item is
also in the panel.

## What looking at the load changed

The drawing was rendered with real rates on it, looked at, and fixed — three
rounds of it. In order:

1. **`<0.1%` seven times over.** A campus at rest is mostly links carrying a
   few hundred kilobits on a gigabit port. Every one of them wrote `<0.1%`,
   and around the two distribution switches they landed on each other and on
   the port names. A percentage too small to write as a number says nothing
   the bar has not; it is not written at any zoom now, and the exact figure is
   one click away in the panel.
2. **`no rate` printed through `Gi0/0`.** The numbers started a third of the
   way along the line from the end that was sending — which is where the port
   names are, on the same side. Numbers moved to the middle of their own bar:
   midpoints are as far apart as the links are, and the side still says which
   direction it is.
3. **"load measured in 11s"** — a measurement from the future. `useNow` floors
   to its own interval so that it is stable between ticks, which puts it up to
   fifteen seconds behind. The tick is the signal to re-reckon; the instant it
   is reckoned against is taken fresh at each tick.
4. **The terminal dock made the cards at the bottom unclickable.** Opening it
   takes a third of the canvas, and the drawing kept its old transform, so the
   bottom row of cards ended up behind the legend — visible, and not
   clickable. The dock is part of the fit key now, so the drawing refits when
   it opens and again when it closes. (Playwright found this one: "`<b>
   Confirmed</b>` from the legend intercepts pointer events".)
5. **The panel contradicted itself on a stale link**: the rows said `Stale`
   and the sentence under them said "these ports are measured about every
   30m", because that sentence took the *slower* of the two ends. The cadence
   belongs to the reading, so it moved into the row that is stale.
6. **Three lines where there was one.** A channel either side of every wire
   made a drawing of cables look like a drawing of pipes. The channel is a
   dimmed version of the wire's own colour now (`--text-3` at 55 %), which
   reads in both themes without turning the map into plumbing.
7. **An idle direction drew a 1.6 % stub**, the same shape as a link carrying
   a trickle, in a different colour. Colour alone is exactly what this layer
   must not rely on: a measured zero draws an *empty* channel now, so idle, a
   trickle, no speed and no rate are four different shapes.

## What the backend made awkward

* **`topology.get`'s `util_pct` is one number for a link, with no direction,
  no age and no speed beside it.** It is `max(in, out)` over both ends, from
  `monitor.interfaces`, and the field says "utilisation" without saying of
  what or when. This pass therefore does the join in the browser instead:
  `monitor.interfaces` for the whole table, `devices.list` for each device's
  cadence, `settings.get {monitoring}` for `cli_interval_s`. It works, but it
  is three reads and a join on every drawing, and two of them are only there
  because the link does not carry what it needs. What would close it, in the
  shape the rest of this type file is in:

  ```rust
  pub struct LinkLoad {
      pub a_to_b_bps: Option<f64>,   // the sending port's own counter
      pub b_to_a_bps: Option<f64>,
      pub speed_bps: Option<u64>,    // already there
      pub measured_at: Option<i64>,  // the poll that produced them
      pub measured_every_s: Option<u32>, // how often this link is measured
      pub read_from: [Option<String>; 2], // which port each number came from
  }
  ```

  `measured_every_s` is the one the screen cannot work out honestly: a lab
  device's counters are read every `LAB_CLI_INTERVAL_S` (30 s, a constant
  inside `nocturne-monitor`), not every `cli_interval_s`, so the UI's
  staleness limit for a lab device is three times more generous than it needs
  to be. It errs towards *not* crying stale, which is the right way round,
  but only the backend can say what the real cadence is. `util_pct` itself
  could stay as it is; nothing here reads it any more.
* ~~**`monitor.interfaces` is asked for 5 000 rows to draw a dozen links.**~~
  Closed: `topology.rates` is the "rates for these (device, interface) pairs"
  query this asked for. It was not merely the wrong shape on a large estate,
  it was wrong: one call is the first 5 000 rows and nothing more.
* **A terminal from a map cannot be offered honestly without the inventory.**
  Whether a device has a credential, an address or a command line is in
  `devices.list`, which this screen now reads for that alone (and for the
  monitoring cadence). A `can_connect: {ok, reason}` on the device row — the
  backend already knows all of it, including whether its lab is running —
  would let every screen say the same sentence instead of each writing its
  own. `pages/topology/session.ts` is that sentence, written here.
* **`labs.get` is the only way to find out whether a node is powered**, so a
  map with lab devices on it has to ask the lab feature per lab. It is asked
  for one lab at a time (the selected device's) and `labs.tick` is
  deliberately *not* subscribed to — four events a second would turn a
  context menu into a refetch storm. A `powered` flag on the device state
  would make this unnecessary.
* **`force` flings disconnected nodes a long way.** With `use_lab_links` off,
  the demo campus has seven hosts that nothing reports as a neighbour; a
  spring embedder pushes them apart and has nothing to pull them back, so
  `topology.autolayout {algorithm: force}` answered with positions spanning
  58 994 × 33 420 world units for fourteen nodes (`layered` spans 1 190 and
  `radial` 814 for the same graph). Fitted into a window that is 2 % zoom and
  the cards are two pixels apart — the separation pass is kept, but the
  drawing is unreadable. The screen cannot fix this without computing
  positions itself, so it says so instead: after a force layout whose spread
  is over 20 000 the toast adds "nodes with no links of their own end up far
  away; press Fit to see them all". Clamping the components into the drawing's
  own box, the way `layered` wraps a too-wide rank, would fix it properly.
* **A graph link does not carry the id of the manual link it came from.** The
  graph's link id is an FNV hash of the link's signature and
  `topology.manual.list` has its own ids, so a drawn link selected on the
  canvas cannot be deleted from there without matching both ends by hand and
  risking the wrong one. The link panel therefore sends the user to the list,
  and says so. A `manual_id` on a `kind: manual` link would close that.
* **`locate.host` of a device name is refused**, with "`SRV1` is not an IP
  address or a MAC address, and no device of that name is in the inventory.
  Nocturne does not resolve names, so give an address was not found" — the
  sentence has a seam in it ("so give an address" meets "was not found"). The
  screen quotes it as it arrives; it would read better fixed in `locate.rs`.
* **`topology.get` has no per-link "which end reported it"**, so one-sided
  links cannot say more than that one end did. That is the honest answer and
  the screen gives it, but a `reported_by: node id` would let the drawing put
  an arrowhead at the end that spoke, which is what an engineer actually wants
  to know.
* **A device's `kind` changes with the layer.** `Campus/AS1` is a `switch` in
  the physical graph and `unknown` in the layer-3 one; `Campus/DS1` is an
  `l3_switch` and then a `router`. The icons on the cards change when the
  layer does, which looks like a bug in the screen and is not one — the kind
  is inferred from what that layer's build happened to see.
* `util_pct` arrives as a percentage (0.095 for an idle gigabit link) while
  `Node.alerts` and `speed_bps` are absent rather than zero; the types say so
  and the screen shows a dash for what is missing, but it is worth knowing
  that a quiet link looks like `0.10%` and not like nothing.

## What the review found (third pass)

Eight defects, all in what the second pass added or left alone.

* **The rates were the first 5 000 rows of monitoring's table, and the join
  was done here.** `monitor.interfaces` returns at most 5 000 rows a call and
  the screen made one call: on 150 access switches the ports of the last
  sixty-odd had "no rate" although monitoring held current rates for them.
  And the join compared names as text through a fifteen-entry table of
  abbreviations in `graph.ts`, while the backend canonicalises with
  `nocturne-drivers::ifname` (`TenGigE` → `TenGigabitEthernet`, `eth` →
  `Ethernet`, `Tw`, `Fi`, …): every IOS XR port was lost. Both are fixed in
  one place, in the backend: `topology.rates {ports}` is asked about the
  ports the links end on (`ratePorts`), reads each device's rows to the end,
  joins with `ifname` on both names, and answers under the spelling it was
  asked with. `interfaceKey` and `LONG_FORMS` are gone; this screen compares
  no interface names any more. (`shortInterface` stays: it abbreviates a
  label and joins nothing.) This is the "narrower query" the list below asks
  for.
* **Staleness compared the server's timestamp with the browser's clock.** A
  laptop four minutes fast greyed the whole map; one four minutes slow kept a
  dead poll live. `topology.rates` and `topology.get` both say what time it
  is on the server (`at`, `generated_at`); `serverNow` carries the latest of
  those forward by how long ago it arrived (`dataUpdatedAt`), so the
  browser's clock is only ever subtracted from itself. The stats line, the
  panel's "Measured …" and the warnings band's "older than the limit" use the
  same time. `TimeAgo` (the design system's) still reckons against the
  browser's clock, here ("Data collected", "from a table collected") and on
  every other screen: that wants a server-clock offset in `api/` or
  `lib/time`, which is not this screen's to add.
* **A second press while the first login was connecting dialled twice.**
  `terminal.open` answers only once the device has been dialled, and
  `terminal.list` — all the screen went by — has nothing until then. The
  page now remembers which devices it is dialling (`sessionAction(existing,
  dialling)` answers `wait`), the button shows it, the menu item says
  "connecting…". `mutateAsync` rather than `mutate` with callbacks: a second
  `mutate` drops the first one's per-call callbacks, which is also why only
  the second session was ever docked.
* **Closing a session from the map kept its xterm.** Hiding parks the
  terminal for later; closing now calls `terminals.forget`, as the Terminal
  screen and the lab's dock do.
* **"Save as a new view…" used `window.prompt`**, which the macOS WebView
  answers `null` without showing anything. `SaveViewDialog` now. Its Enter
  saves, except the Enter that ends an input-method composition
  (`isComposing`), which belongs to the word being typed.
* **The export's Copy said "Copied" whatever the clipboard did.**
* (Backend) a folded port-channel's speed was the sum of its two ends, so
  every percentage on a bundle was wrong; it is the members' sum now, and
  `speedFor` already preferred the link's own speed.

## State at hand-off

* `pnpm --dir ui test`: green; 99 of them in this screen's five files
  (`graph.test.ts`, `search.test.ts`, `trace.test.ts`, and from this pass
  `load.test.ts` and `session.test.ts`). After the third pass: 117 in six
  files — `dialogs.test.tsx` renders the two dialogs in happy-dom.
* `pnpm --dir ui build` (whole-repo typecheck, then the bundle): clean. The
  screen is its own 73 kB chunk, 19 kB more than before this pass.
* `tests/topology.spec.ts`: 24 tests (31 after the third pass: the view
  dialog, the clipboard refusing and accepting, a browser clock two hours
  fast and two hours slow, a double press on Terminal, and a session closed
  from the map) against a real `nocturne-server` with the
  demo campus lab really collected from *and really polled*: the five campus
  switches are read twice a simulated minute apart so the interface rates are
  real, EDGE and the ISP router are left unmonitored so some links honestly
  have no rate at all, and DS2 is taken off monitoring with a five-second
  interval so one link's readings really do go stale while the suite runs.
  Every test asserts the console is clean.
  Run: `NOCTURNE_SERVER_BIN=~/.cache/nocturne-target/main/debug/nocturne-server
  pnpm exec playwright test --project=e2e topology.spec.ts`.
* `tests/topology.shots.ts`: 46 PNGs into `tests/shots/topology/` (23 per
  theme: both themes, 1440×900 and 1280×720, dock open and closed, and from
  this pass the load at a zoom where it is written out, a link's load in the
  panel, a stale one, the right-click menu, a terminal open on a device from
  the map, and a node that cannot be given one).
