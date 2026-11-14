# Tools — build notes

Screen: `docs/dev/spec-ui-pages.md` → "Tools". Backend: `docs/dev/features/toolbox.md`,
`crates/nocturne-toolbox`, types in `ui/src/api/types/toolbox.ts`.

## State

43 panels, every tool in the engine's catalogue covered. `pnpm --dir ui test`
(111 tests in this folder), `pnpm --dir ui build` (whole-repo typecheck) and
`ui/tests/tools.spec.ts` (14 tests against a real `nocturne-server`) are green;
`ui/tests/tools.shots.ts` writes 13 pictures per theme.

```
index.tsx        the screen: gating, routing, the drawer and the panel
catalogue.ts     tools.list → kinds, panels, folding, grouping, the filter
specs.ts         what each tool asks for, and the parameters that come of it
fields.ts        the field vocabulary (types only, so specs.ts stays pure)
facts.ts         every result shape → sections of facts, tables, text, notes
probes.ts        what a run did: outcome, failure words, ping arithmetic, merging
secrets.ts       reversible vs decodable, in two statements
data.ts          the reads, the writes, the debounce and the remembered form
Answer.tsx       a form and one answer (calculators, generators, reply probes)
JobProbe.tsx     the four streamed probes, with re-attachment
Secrets.tsx  Reference.tsx  Servers.tsx  History.tsx   the panels with a shape of their own
Sections.tsx  Fields.tsx  ToolList.tsx  ToolPanel.tsx  Unavailable.tsx
```

The reasoning is in the four `.ts` files and has unit tests
(`catalogue.test.ts`, `specs.test.ts`, `facts.test.ts`, `probes.test.ts`,
`secrets.test.ts`); the `.tsx` files render what those decide.

## The two things this screen exists to get right

**`reversible` and `decodable` are different questions.** `secrets.ts` turns
one `tools.secret.identify` answer into two statements that are always both on
screen: what the *format* is ("Reversible by design" for a Cisco type 6, "One
way: a hash" for a type 5) and what *Nocturne* can do with it ("Nocturne cannot
recover it — not because the format is strong, but because the key belongs to
the device"). A single tick would have taught the reader that a type 6 is as
safe as a hash. Both the unit tests and the end-to-end test assert that the two
differ for a type 6.

**A probe that could not run is not silence.** The screen never draws a figure
for a run that produced none: `probes.ts` turns a job and its history record
into an outcome whose `failed` case carries no result at all, and
`ProbeFailure` renders the engine's own sentence under a headline that keeps
`forbidden` ("This machine may not run that probe") apart from `timeout`
("Nothing answered in time"), says whether any packet left the machine, and
prints the code. 100 % loss only ever comes from a report that exists. The
ping test asserts both branches — a real report, or the real refusal with no
`… ms` anywhere on screen — because whether this machine may send ICMP is not
something a test can decide.

## Decisions worth remembering

* **The drawer is `tools.list`, not a list kept here.** A tool added to the
  crate appears without a line of TypeScript. `FORMED` in `catalogue.ts` is the
  set this screen has a form for; anything else renders as "No form for this
  tool yet" rather than being hidden, and a unit test checks `FORMED` against
  the ids in `crates/nocturne-toolbox/src/catalogue.rs` so adding one there is
  a failing test here rather than a silent gap.
* **Three methods, one panel.** `tftp.start/stop/status`, the four
  `probe.syslog.*`, the three `probe.history*` and `tools.reference` with
  `tools.reference.tables` are each one thing on screen; `FOLDED` says which
  method belongs to which panel and the panel header lists what it folded in.
* **Calculators are queries, generators and probes are mutations.** Typing is
  the question for arithmetic (debounced, and React Query caches by parameters
  so going back to an earlier input costs nothing). A password, a key pair, an
  encoded secret and every probe are mutations: a query would hand back the
  cached previous answer, which for a generator is wrong and for a probe is a
  lie about when it ran.
* **The secret tools are asked, not typed at, where the input is a new secret.**
  `tools.secret.encode`, `tools.password` and `tools.keys.generate` take or
  make a secret, so they have a button: answering as you type would send every
  prefix of a password somebody is choosing, and would throw the answer away on
  the next keystroke. Decoding and identifying stay live — the input there is
  ciphertext that is already in a configuration.
* **Secrets are never remembered.** Every other tool's last inputs are written
  to `localStorage` (the brief asks for it); `Spec.secret` marks the five that
  are not, and the end-to-end test reads `localStorage` back to prove that
  neither the pasted line nor the decoded value is in it.
* **The three families never look alike.** `FAMILY` maps each kind to one of
  three frames — arithmetic (quiet), a probe from here (`--info`), something
  standing: on a device or a running server (`--accent`) — and the panel header
  carries the family's colour, a word, and one sentence about what using it
  will do, *before* it is used.
* **A run belongs to the backend.** The four job-shaped probes start a job and
  then read everything back: `jobs.get` for progress and the report, the
  feature's topics for what it is finding, and `probe.history.get` for the
  failure code. Leaving the screen does not stop a ping, and coming back
  attaches to the run rather than starting a second one.
* **Events that arrive before the job id are held, not dropped** (`hold`/`take`
  in `probes.ts`). `probe.ping` starts the job and *then* answers, so the first
  echo can reach the screen before it knows which job it belongs to; without
  holding, the live count was one short of the report. Same rule as the
  Terminal screen's attach.
* **The ping chart has no fixed scale.** Full height is the tallest round trip
  drawn, and the label says what that is. A 1 ms floor was tried first: on a
  loopback ping of 0.08 ms every bar became a sliver one pixel high, which
  reads as "nothing came back" — the one thing the chart must never say by
  accident. A lost echo is a full-height wash with a mark at the baseline, and
  an answered one is never shorter than 4 px.
* **The two servers are gated on `app.info.local_access`**, so the screen says
  "only in the desktop app" in a sentence instead of offering a button that is
  always refused. A refusal that happens anyway is still shown in the engine's
  words.
* **The filter matches titles fuzzily and descriptions as plain text.** The
  command palette's fuzzy match over forty sentences of prose matched more than
  half the drawer for `mss`; a filter that never narrows is not a filter.
  Groups keep their fixed order while nothing is typed and are ordered by their
  best match once something is.

## What I deliberately did not call

* **`probe.traceroute` and `probe.sweep` have panels but no end-to-end test.**
  Both are built and streamed exactly like the ping and the port check (the
  merging rules are unit-tested), but a traceroute from a container usually
  needs a raw socket and a sweep of loopback space is three addresses of very
  little; neither would have asserted anything the ping and port tests do not.
  They are in the shots only through the drawer.
* **`probe.whois` is in the drawer and is never called by a test**: it needs
  the internet, which the suite does not have.
* **`probe.tls` against a real certificate** is not exercised either, for the
  same reason the crate's own tests do not (`nocturne_netprobe::tls::
  test_support` is `pub(crate)`). The panel renders the chain from the engine's
  shape; the failure path is the one the tests cover.
* **`tools.reference.tables` is not a panel of its own**, and neither are the
  `stop`/`status` methods: they are how the panels they belong to work.
* **No "ask the assistant" button.** The shared `--ai` affordance belongs to
  the screens that have something to hand over; a subnet calculation is not it.

## What the backend or the design system made awkward

* **A running probe's history record has no `job_id`** — a defect, and the
  reason this screen re-attaches through `jobs.list` instead. `Run::new` leaves
  `job_id: None`, `history::begin` saves that record, and the job then sets
  `run.job_id` on its own copy, which is written only when the run ends:
  `crates/nocturne-toolbox/src/probe/ping.rs:199`, `ports.rs:130`,
  `sweep.rs:241`, `trace.rs:132`. So while a probe is running, nothing in
  `probe.history` says which job it is, and `probe.history.get` on a running
  ping cannot be joined to `jobs.get`. One `history::begin`-style save after
  `run.job_id = Some(...)` would fix it. The same goes for `method`, which is
  `null` until the run finishes.
* **`AppInfo` in `api/types/core.ts` does not declare `local_access`**, though
  `app.info` returns it (and the two servers are refused without it). This
  screen reads it through a narrow local type in `data.ts`; the field belongs
  in `core.ts`, which is not mine to edit.
* **`tools.secret.decode` and `.encode` have no "what kind is this" in their
  result** beyond the `kind` and the note, so the decode panel cannot show the
  strength verdict beside the value without a second call to
  `tools.secret.identify`. It links to that tool instead. Not worth a change.
* **The design system's `Table` gives every column 120 px unless told
  otherwise**, and the header text is ellipsised at `width - 44 px` (padding
  plus the sort affordance). A column of two-digit numbers beside a column of
  prose therefore has to be measured by hand; `facts.ts` carries the widths.
  A `fit` or `min` width that sized to the header would save every screen that
  work.
* **A container query cannot measure its own container.** The first version put
  `container: tools / inline-size` on the grid it was meant to reflow, so the
  drawer never stacked when the assistant dock took 380 px; it needed a wrapper
  (`.frame`). Worth a line in `spec-ui.md` for the next screen that reflows on
  the dock rather than on the window.
* **`Button` with `loading` hides its label**, so a slow probe's button becomes
  an unlabelled spinner. Left as the design system has it.

## Looking at the screenshots changed

1. The drawer said "Offline" on 25 of 43 rows — the badge now appears only for
   the tools that are *not* plain arithmetic, which is the information the row
   actually carries.
2. The ping chart was invisible. With a 1 ms floor a loopback ping drew twelve
   one-pixel slivers; the scale now comes from the data and an answered echo is
   never thinner than 4 px. Capping the bar width was tried too and reverted:
   a dozen thin bars in the corner of an empty box read as "nothing came back".
3. The finished ping showed the live figures, the rolling echo list, the
   report's facts *and* the full echo table — two of them saying the same
   thing twice. The strip and the rolling list now belong to a run in flight
   and give way to the report when it lands.
4. The rolling list did not follow its own newest line; it scrolls now.
5. Table headers were clipped ("Round t…", "T…", "Answered i…") and the VLSM
   table ran off the panel's right edge, losing its "Spare" column. Widths are
   set per column in `facts.ts`, and the VLSM table dropped its netmask column
   (the prefix is in the network beside it).
6. The port grid told open from closed by colour alone; every chip now carries
   a shape as well.
7. The secret verdict put the strength word and its explanation in two columns,
   where the explanation wrapped badly in a column meant for values. One row.
8. The history panel's own sentence repeated the catalogue's description of it
   almost word for word.
9. `Directory in the settings` wrapped the label column in the TFTP panel; it
   is `Directory`.
10. At 1280 px with the assistant dock open the drawer was still beside the
    panel, leaving 416 px for the tool (see the container-query note above).
    It stacks now, and the stacked drawer is 220 px so the tool is on screen
    under it.
