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
* `/terminal` takes no device, so the "Terminal" action the page spec lists for
  a node is not offered — a button that opened the Terminal screen on whatever
  it happened to be showing would be a lie. `/terminal?device=<id>` (or a
  `terminal.open` the screen could call) would let it exist.
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

## What the backend made awkward

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

## State at hand-off

* `pnpm --dir ui test`: 522 green, 53 of them new here (`graph.test.ts`,
  `search.test.ts`, `trace.test.ts`).
* `pnpm --dir ui exec vite build`: clean, and the screen is its own 54 kB
  chunk. `pnpm --dir ui build` additionally typechecks `tests/`, which at
  hand-off fails on `tests/config.spec.ts` (an unused `tab` helper in the
  Config slice, in flight at the same time) — nothing in this slice.
* `tests/topology.spec.ts`: 16 tests against a real `nocturne-server` with the
  demo campus lab really collected from, seeded so that all three
  confidences, a drawn link, an endpoint and a node that is not in the
  inventory are on the drawing. One of them adds a second device that answers
  to a name a neighbour reported and checks that the backend's refusal to
  guess is on screen. Every test asserts the console is clean.
  Run: `CARGO_TARGET_DIR=~/.cache/nocturne-target/main pnpm exec playwright
  test --project=e2e topology.spec.ts`.
* `tests/topology.shots.ts`: 34 PNGs into `tests/shots/topology/` (both
  themes, 1440×900 and 1280×720, dock open and closed).
