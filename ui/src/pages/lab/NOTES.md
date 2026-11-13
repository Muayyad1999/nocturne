# Lab screen — working notes

Running log for whoever picks this up next.

Owned paths: `ui/src/pages/lab/`, `ui/tests/lab.spec.ts`, `ui/tests/lab.shots.ts`,
`ui/src/api/types/lab.ts` (written by the backend's author; nothing needed adding).

## Shape

```
index.tsx           the route: `lab/:id?` → the list or one lab
LabList.tsx         the labs, create / start / stop / delete
NewLabDialog.tsx    an empty canvas or one of `labs.examples`
TwinDrawer.tsx      `labs.twin`: pick real devices, watch the job, read the fidelity report
Editor.tsx          one lab: toolbar, palette, canvas, inspector, console dock
Canvas.tsx          the SVG scene on `design/Viewport`: notes, cables, nodes, dragging
Palette.tsx         `labs.catalog`, and the cable tool
Inspector.tsx       the selected node or cable, and what can be done to it
CableDialog.tsx     cabling with the ports named by hand
ConsoleDock.tsx     tabs of `terminal.*` sessions, xterm lent from the Terminal screen
Reachability.tsx    `labs.probe` (path + why it died) and `labs.connectivity` (matrix)
TablesDrawer.tsx    `labs.node.tables` as nine tabs
model.ts            geometry, icons, lights, addresses, the sim clock   (unit tested)
cabling.ts          the two-click cable gesture and its refusals        (unit tested)
consoles.ts         which `terminal.list` sessions belong to this lab   (unit tested)
```

Tests: `model.test.ts` / `cabling.test.ts` / `consoles.test.ts` (43 unit tests),
`ui/tests/lab.spec.ts` (7, against a real engine and the real simulator),
`ui/tests/lab.shots.ts` (`pnpm shots`, 20 PNGs into `tests/shots/lab/`).
Both Playwright files build the UI and start a real `nocturne-server`: the
simulator is the thing being tested, so there is nothing worth mocking.

## Decisions

* **The backend owns everything a person could care about.** The canvas is
  `labs.get` (invalidated by `labs.changed`), the lights are `labs.tick` laid
  over that snapshot, and the open consoles are `terminal.list` filtered to
  this lab's devices. Close the window and reopen it and all three come back
  as the engine has them. The only thing in `localStorage` is how tall the
  console dock is.
* **A console is just a terminal session**, so `pages/terminal/sessions.ts`
  owns the xterm instance (outside React) and `terminal.attach` brings the
  scrollback back with no gap and nothing twice. This screen imports that
  registry, `appearance.ts` and `terminal/tokens.css` rather than copying
  them — see "things the foundation might want" below.
* **Tabs are derived, never remembered.** `tabsForLab` turns `terminal.list`
  into the tab bar; opening a console reuses the session a node already has.
  A session opened here is held in a ref until the refetch lands, or the
  reconciling effect would pull the user back to the previous tab (it did,
  and the screenshots caught it).
* **The canvas is a sibling of the console dock, not its child.** A
  `Splitter` around both would remount the canvas the moment a console
  opened, losing the pan and zoom; the dock is a sibling with a
  `ResizeHandle` between them instead.
* **Dragging is local until the pointer is let go**, then `labs.node.move`;
  the local position is dropped as soon as the backend reports the same one.
  `labs.node.move` publishes no event by design, so nothing refetches per
  pixel.
* **Two ways to cable.** The tool is one click on each device and the
  simulator picks the first free port that suits the cable (`labs.link.add`
  with no interface named) — the common case in one gesture, with a toast
  saying which ports it used. `Cable…` in the inspector names both ports.
* **Refusals are the simulator's own sentences.** "`PC1` has no free
  interface left", "R1 GigabitEthernet0/0 is already cabled", "none of these
  devices can be twinned yet: collect from them and back them up first". The
  only refusals this screen writes itself are the two it can see from the
  snapshot (no free port, both ends the same node), so the user is not made
  to wait for a round trip to learn them.
* **Nothing is invented.** An empty lab says it is empty. A lab with no
  addressed node says there is nowhere to send a packet from, rather than
  showing `labs.connectivity`'s "100 %" for an empty matrix (which is what it
  honestly returns for no nodes). A stopped lab has no tables, no probes and
  no lights, and says so with the backend's own `unavailable` message.
* **The probe is the point.** `labs.probe` returns every hop with the
  device's reasoning — "The switch's inbound ACL GUEST-IN on Vl20 matches
  line 20 `deny ip …`: the packet is dropped" — so the drawer prints the
  whole path and the reason rather than a red cross. The matrix is the same
  data in bulk, and selecting a cell shows that pair's sentence.
* **The twin shows its accounting.** Lines accepted of lines tried, every
  rejected line with what the device said, the interfaces no model has, and
  the devices refused with the reason. A twin that does not state its
  fidelity is a twin nobody should trust.

## Not built (and why)

The feature spec's lab section is larger than this slice. Left out
deliberately, with no stub and no fake: packet capture and the
simulation-mode event list (`labs.capture` / `labs.trace` / `labs.packets`),
traffic flows (`labs.flow.*`), faults (`labs.fault.*`), import/export,
duplicate, canvas notes editing, and the design wizard (`design.*` is not in
this build at all — `nocturne-design` is a stub, so there is nothing to call).
`labs.expose` is not surfaced either: it belongs next to the Terminal
screen's "connect with your own client", not on the canvas.

## Simulator behaviour the screen has to live with

* **A port that has just come up does not forward straight away.** Spanning
  tree holds it in listening and learning for about two simulated seconds, so
  a probe taken the instant a cable or an address is configured can honestly
  fail. The link lights show it (amber), the inspector names it, and the
  end-to-end test waits for the simulator to agree the path is up before it
  asks the screen about it rather than asserting on a clock.
* **`labs.connectivity` answers "100 %" for a lab with nothing in it**, which
  is true of the empty set and useless to a person, so the drawer checks for
  addressable nodes itself and says there is nowhere to send a packet instead.
* **A probe needs a running lab**; tables and metrics too. The backend's
  `unavailable` message says so and the screen offers Start rather than
  showing empty tables.
* **A twin cannot be made of a lab device** (the backend refuses: "this is
  already a simulated device"), so the picker only offers real ones.

## Things the foundation might want to look at

* `pages/terminal/sessions.ts`, `appearance.ts` and `tokens.css` are now used
  by two screens, so they belong in `lib/` and `design/tokens.css`. This
  screen imports them across page folders, which the layout rules say should
  not last.
* `design/Viewport` fits once on mount. When a lab's nodes arrive after the
  first paint the scene is already fitted to nothing; the canvas works around
  it by passing `contentBounds` as soon as there are nodes. A `fitKey` prop,
  or fitting again when the bounds change from empty, would be better.
* `design/Table` has no row identity for rows that have none of their own
  (the simulator's table rows), so `TablesDrawer` adds a `_key` field.

## Log

* Read the lab and netsim specs, `crates/nocturne-lab/src/api.rs`, the
  terminal screen's notes and registry, then probed a real `nocturne-server`
  over HTTP to learn the live shapes: the catalog, the examples, a built lab,
  `labs.probe` on the demo campus, a console through `terminal.open`, and
  `labs.twin` both ways (refused, and — by exposing a lab node over real SSH
  and collecting from it — succeeding).
* Built the canvas, the rails, the dock and the drawers against that server
  the whole way.
* Made the screen hold at 1280×720 with the assistant dock open — the
  toolbar wrapped onto a second row and the palette's names squeezed the
  canvas to nothing. A container query at 900px of content now takes the
  secondary buttons' words and the palette's names away (their accessible
  names and tooltips stay), the cable tool is pinned below the scrolling
  model list, and the inspector gives up 60px.
* Looked at the screenshots and fixed: a palette that was one truncated
  column (two columns made every name an ellipsis, so it is one wide column
  with the catalog's own description in a tooltip), port labels colliding
  with node names (the kind line under a node is gone — the icon says it —
  the labels sit a third of the way along the cable and 8px to the side of
  it, with a halo), two palette entries both reading "Router", a Stop icon
  that read as a checkbox, and a Delete button louder than Start.
* Found and fixed four real defects that way, three of them things only a
  real backend and a real browser would show:
  - opening a console while another was open put the new tab on screen for a
    frame and then snapped back to the old one (`terminal.list` had not
    refetched yet);
  - xterm's own stylesheet is imported by the Terminal screen, so on this
    screen xterm's hidden character-measuring element was painted over the
    output as a row of repeated glyphs — this screen imports it too now;
  - fitting a lab with one device in it zoomed to 800 % (the Viewport's
    `maxZoom`), after which the next device from the palette landed outside
    the view; the canvas caps the zoom at 250 % and fits again after an add;
  - when the console dock opened it covered the bottom of the canvas, so a
    device down there could not be clicked; the scene is fitted again when
    the dock appears or goes away (a drag of the handle is left alone).
