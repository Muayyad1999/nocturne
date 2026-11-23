# Tools — build notes

Screen: `docs/dev/spec-ui-pages.md` → "Tools". Backend: `docs/dev/features/toolbox.md`,
`crates/nocturne-toolbox`, types in `ui/src/api/types/toolbox.ts`.

## State

45 panels, every tool in the engine's catalogue covered. `pnpm --dir ui test`
(193 tests in this folder), `pnpm --dir ui build` (whole-repo typecheck) and
`ui/tests/tools.spec.ts` (18 tests against a real `nocturne-server`) are green;
`ui/tests/tools.shots.ts` writes 31 pictures per theme, 18 of them the two QoS
calculators and the reference tables that lead to them (`-g QoS` takes those
alone).

```
index.tsx        the screen: gating, routing, the drawer and the panel
catalogue.ts     tools.list → kinds, panels, folding, grouping, the filter
specs.ts         what each tool asks for, and the parameters that come of it;
                 which field a refusal is about; where a reference row leads;
                 what a picker says in each state of its table
field-kinds.ts   the field vocabulary (types only, so specs.ts stays pure)
facts.ts         every result shape → sections of facts, tables, lines, text, notes
probes.ts        what a run did: outcome, failure words, ping arithmetic, merging
secret-answers.ts  reversible vs decodable, in two statements
data.ts          the reads, the writes, the debounce, the remembered form, and
                 the engine's tables behind a form's pickers
Answer.tsx       a form and one answer (calculators, generators, reply probes)
JobProbe.tsx     the four streamed probes, with re-attachment
Secrets.tsx  Reference.tsx  Servers.tsx  History.tsx   the panels with a shape of their own
Sections.tsx  Fields.tsx  ToolList.tsx  ToolPanel.tsx  Unavailable.tsx
```

The reasoning is in the `.ts` files and has unit tests (`catalogue.test.ts`,
`specs.test.ts`, `facts.test.ts`, `probes.test.ts`, `secret-answers.test.ts`,
`data.test.ts`); the `.tsx` files render what those decide.

## The two things this screen exists to get right

**`reversible` and `decodable` are different questions.** `secret-answers.ts` turns
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

## The two QoS calculators

`tools.qos.marking` and `tools.qos.voice` (`docs/dev/features/toolbox.md`,
"QoS calculators") are ordinary calculators here: a spec, a `sectionsFor`
case, `LiveAnswer`. What they needed was added to the shared machinery rather
than built beside it, so the next tool that needs it has it:

* **A picker can be the engine's table** (`Field.source`). The codec and link
  choices are the rows of `voice_codecs` and `voice_links`, read through
  `tools.reference` by `Fields` itself (`useTables`), so every form gets it
  without being told. Not read yet is a disabled "Reading the engine's
  table…", not an empty list; a table that could not be read says so in the
  engine's words under the field, and the list then reads "Nothing to choose
  from" rather than going on saying it is being read. In every state the
  list shows what the form holds — a codec remembered from last time is asked
  about at once, table or no table, and a list reading "Nothing to choose
  from" above an answer for G.729 was the picture that showed it (a held
  value with no row is listed by its id). The states are one pure function
  (`pickerOf`), and the failed read is seen in a browser by cutting that one
  request on the wire (`tools.spec.ts`). `custom` is the one choice written in the
  spec, because the engine's table has no row for it. A chosen option's own
  figures replace the field's hint (`FieldOption.hint`).
* **Nothing is chosen for the reader where the engine refuses to choose.**
  `as` has no default at the engine ("the same number is a different marking
  as each") and `calls` has none ("one call for a link sized for sixty"), so
  both fields start empty and the form asks nothing until they are filled. A
  preselected "DSCP" would have been the interface doing the guessing the
  engine was written not to do. The form is remembered, so this costs one
  click, once.
* **A refusal goes beside the field it is about** (`Spec.blame`, `blamed`).
  The engine's refusal carries no field — `details` is `{reason}` — but its
  sentence opens with what the input was supposed to be (``invalid link type
  `ethernet`: …``), and that opening phrase is all that is matched, never the
  reason. The sentence takes the hint's place through the design system's
  `Field error`, and where the answer would be the box only points at it, so
  the sentence is on screen once. A refusal that names no input (a negative
  number of calls is refused by the router: "expected u32") is shown whole
  where the answer would be, as for every other tool. The refusals for a
  parameter that is missing altogether (``give the number of simultaneous
  calls in `calls` ``) are worded by the RPC method and name the parameter
  instead; they are matched too, although no form here sends such a request.
  `specs.test.ts` holds one sentence of every reason either method refused
  with on a running server, each with the field it belongs beside.
* **Compressed RTP on Ethernet is refused beside "Link type", not beside the
  switch.** The engine calls the *link* wrong, and the way out it names
  (`custom`) is in that field. The link's hint says "Compressed RTP is not
  counted on it" before the switch is tried; the switch is not disabled,
  because the engine's sentence is the one that explains why.
* **The figures are the engine's.** `voiceSections` lays out `per_call`,
  `total`, `packet`, `arithmetic`, `notes` and `sources` and works nothing
  out; a unit test feeds it a result whose total is *not* calls × per call
  and asserts the engine's total is what is shown. `phbParts` cuts the
  engine's per-hop-behaviour sentence where the engine put its comma and its
  bracket ("Assured Forwarding class 4" · "low" · "RFC 2597") and keeps any
  sentence that is not in that shape whole.
* **Every field of the answer is on screen.** A marking shows all sixteen
  the engine returns, the ECN field as its bits *and* its number (`01 (1) —
  ECT(1)`: ECT(1) is 1 and ECT(0) is 2, so the name alone invites reading it
  the wrong way round). A voice answer shows the packet that was worked out
  (`packet_ms`, `payload_bytes` of the result), not the codec's default one;
  the codec's own row — bit rate, sample, default packet — is under its
  picker, where it was chosen.
* **The arithmetic and the sources are a new section kind, `lines`**: the
  engine's sentences, one per row, numbered when their order is the argument.
* **A reference table leads to the calculator that takes its rows**
  (`leadOf`, `leadValues`, `prefill`). Enter on a DSCP row, or its menu, opens
  the marking calculator on that value — by number, because a searched-for
  unnamed value has a row whose "name" is `DSCP 47`. The two voice tables
  lead to the voice calculator the same way, keeping what its form already
  remembered. The row action exists only when `tools.list` has the
  calculator. The DSCP table did not grow hexadecimal or ECN columns.

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

* **Nothing leads back from the marking calculator to the DSCP table**, and
  there is no command-palette entry for either QoS tool: the drawer's filter
  finds both by `dscp`, `tos`, `codec` or `voice`.
* **A voice refusal by the router** (`calls: -3`, `calls: 1.5`) is passed on
  as the router wrote it. The form could keep such a number from being sent,
  but then the interface would be deciding what a valid number of calls is.

## What the backend or the design system made awkward

* **A calculator's refusal does not say which parameter it is about.**
  `errors::tool` puts only `{reason}` in `details`, although `ToolError::
  Invalid` knows its `what`. `Spec.blame` therefore reads the opening phrase
  of the message. One more key in `details` (`what`, or better the parameter
  name) would let the interface stop reading prose; until then a reworded
  `what` ("link type") moves that refusal back to where the answer would be,
  which is the old behaviour and not a wrong one.

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
11. Choosing a codec pushed "Simultaneous calls" down the screen, and a
    refusal under "Link type" pushed "Voice per packet" down by four lines:
    the form grid was bottom-aligned, so a hint that grew moved the control
    beside it. The grid is top-aligned now, for every tool; the Reference
    panel's "Search" used to sit a line below its "Table" for the same reason.
12. The DSCP table's headings read "DS…" and "Preced…", and its PHB column
    was nine characters of monospace. Columns are as wide as their headings
    need, and the PHB is prose.
13. The codec table ran off the panel's right edge and lost its MOS column
    (the VLSM table's mistake again): eight columns at the widths their long
    headings needed. The headings are the units now (`B/sample`,
    `ms/packet`).
14. The "what the value is" hints wrapped to two lines beside a one-line hint.
    Each is one line.
15. The link table cut `frame-relay` to `frame-re…`: an id that cannot be
    read off cannot be typed. The id column is as wide as the longest id.
16. A reference table's cell is one line, so the parts, source and note of a
    link, a PHB and a "typically for" are cut at the column's edge. The whole
    text is on the cell (`title`) for whoever points at it, and the row's
    calculator shows it in full; the table itself still does not wrap,
    because the design system's table does not.
17. At 500 px the form is one column and the drawer is on top; the refusal
    stays under its own field there (`qos-voice-narrow-refused`). The focus
    ring on a switch, which no `Field` frames, is in `qos-voice-focus`.
18. With the codec table unreadable and G.729 remembered, the list read
    "Nothing to choose from" while the engine was being asked about G.729
    underneath it. The list shows the codec the form holds
    (`qos-voice-unread`).
