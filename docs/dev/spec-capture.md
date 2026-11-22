# Evidence, capture, snippets and clips

"This is what SW1 printed at 03:05" — kept, with where it came from, and
attachable to the incident it explains. Plus the small things an engineer
does all day around it: reusing a command with a different port number,
copying the same three interface names between a terminal, a ticket and a
change, and putting a picture of the map or a graph into an email.

It crosses `nocturne-ops` (evidence), `nocturne-terminal` (snippets, session
logs) and the interface (capture, clips, images), so the contract is here
and each feature's specification points at it.

## Evidence (`nocturne-ops`)

An evidence item is a record of something Nocturne itself read, at a stated
time, from a stated source. It is text or data, never pixels (see "Not
building").

```rust
struct Evidence { id, kind: terminal | log | text | topology | chart,
    title, captured_at,
    source: { from: "terminal.snapshot" | "terminal.logs.read" | "operator"
                  | "topology.export" | "metrics.query_many",
              session_id?, session_title?, device_id?, device?, peer?, lab: bool,
              params?: Value,          // what was asked for: lines, view, series, period
              note?: String },         // "selected in session X; text as sent by the client"
    redacted: true,                    // always: there is no way to store it otherwise
    lines?, bytes, sha256,             // of the body as stored
    incident_ids: [String],
    body: Text(String) | Svg(String) | Chart { series: [..], coverage } }   // not in list results
```

| Method | Params → Result |
|---|---|
| `evidence.capture_terminal` | `{session_id, lines?: 200, title?}` → `Evidence` — ops asks `terminal.snapshot` and `terminal.list` itself; the client never supplies the text |
| `evidence.capture_log` | `{name, offset?, limit?, title?}` → `Evidence` — from a session log, read through `terminal.logs.read` with redaction on |
| `evidence.capture_topology` | `{view?, layer?, filter?, include_endpoints?, title?}` → `Evidence` — the SVG `topology.export` returns |
| `evidence.capture_chart` | `{series: [{device_id, metric, instance?}], since, until, step_s?, title?}` → `Evidence` — the points `metrics.query_many` returns, with their coverage: gaps stay gaps |
| `evidence.add_text` | `{text, title, session_id?, device_id?, note?}` → `Evidence` — for text the person holds (a selection). Redacted on arrival; its source is `operator`, and it says so wherever it is shown |
| `evidence.list` | `{kind?, device_id?, incident_id?, query?, limit?: 50, cursor?}` → `{items: [Evidence without body], next_cursor?}` |
| `evidence.get` | `{id}` → `Evidence` |
| `evidence.rename` | `{id, title}` |
| `evidence.attach` / `evidence.detach` | `{id, incident_id}` — a timeline entry of kind `evidence` at the capture time, holding a reference, never the text |
| `evidence.delete` | `{id}` — an item an incident refers to leaves a tombstone there ("evidence deleted on <date>"), not a hole |

Event: `evidence.changed`.

Rules:

* **Redacted, always.** Text passes the same redactor a model's input does
  (`nocturne-ai`'s rules, the device type's own secret patterns, the values
  the vault knows) before it is stored. There is no parameter that stores it
  raw. What was replaced is not recoverable from the item.
* **Provenance is recorded by the engine, not typed by a person.** What a
  capture method stores is what the source method returned, hashed as
  stored. `add_text` cannot claim that: it is labelled as supplied by the
  operator.
* **Limits, stated.** 256 KiB of text, 2 MiB of SVG, 20,000 points, 5,000
  items; past a limit the call is refused with what to do, nothing is
  truncated silently.
* The body lives in its own table, not in the incident and not in the
  document collection (an incident is rewritten whole on every save and its
  search text concatenates its timeline).
* Every mutation is audited with ids, counts and the hash — never the text.
* A lab device's evidence is labelled lab.
* Absent features: `capture_terminal` without the terminal feature is
  `unavailable` with the reason; nothing is invented.

With evidence available the free-text fields stop being the only place for
device output: `incidents.note`, a note's body and the handover's notes are
capped at 64 KiB, and the refusal points at evidence.

`incidents.export` gains an Evidence appendix (preformatted and escaped for
the format, with device, time, line count and hash), and
`handover.generate` gains an `evidence` section listing what was captured in
the period — real records with ids, like every other section.

Incident mutations (`note`, `update`, `link`, `sync`, `attach`) read and
write inside one transaction: a note added while a link is gathering its
events is not overwritten by a stale copy.

## Snippets (`nocturne-terminal`)

A snippet is still `{id, name, text, profile?, tags}`. What is new:

* **Variables.** `{{name}}` and `{{name=default}}` in the text. Names are
  letters, digits and `_`. They are found and checked on save, and
  `terminal.snippets.list` returns each snippet's `variables: [{name,
  default?}]`. This is plain single-line substitution, not a template
  language: for loops, conditions and running on many devices there is
  Automation, and the drawer links to it.
* **Variants.** Two snippets may share a name when their `profile` differs;
  for a session the one for its device type wins over the generic one.

| Method | Params → Result |
|---|---|
| `terminal.snippets.render` | `{id, values: {name: String}}` → `{text, lines, sha256, variables}` — strict: a missing value without a default, or a value for a variable the snippet does not have, is `invalid`; a value containing a line break or a control character is refused, so a value cannot smuggle in a second command |
| `terminal.snippets.send` | `{id, values, session_ids: [..], expect_sha256}` → `{sent: [{session_id, lines}]}` — renders again on the server, refuses when the hash is not the one the person confirmed, types the text into those sessions, and audits the snippet's name, the sessions, the devices, the line count and the hash — never the text and never the values. That audit entry is the record of what was sent where |
| `terminal.snippets.export` | → `{format: "nocturne.snippets/1", snippets: [..]}` |
| `terminal.snippets.import` | `{document, on_conflict: skip \| replace \| rename, dry_run?}` → `{created, replaced, skipped, problems: [..]}` |

A rendered snippet reaches a device only after the paste confirmation has
shown the exact lines, like any other paste, and nothing presses Enter that
the text does not contain. Sending configuration to many devices at once is
a change and belongs to the change pipeline; there is no "run on all" here.
`terminal.snippets.send` types into a person's terminal, so it is on the
assistant's never-reachable list.

Session logs get a screen: a drawer over `terminal.logs.list` / `.read` /
`.delete` — by device and title, read in pages, redacted, with "keep these
lines as evidence". A session log on disk is what the device printed, like a
stored configuration; it is redacted when it is read, and a read with
`redact: false` (on `terminal.logs.read` or `terminal.snapshot`) is audited,
as an unredacted configuration view is.

## In the interface

* **Terminal.** "Capture" on a selection, in the context menu and on the
  toolbar (the selection, or the last screen, or the last N lines). A dialog
  shows the redacted text that will be stored before it is stored, and
  offers the open incidents. "Save last N lines…" writes a file. The
  snippets drawer gains a form for a snippet's variables, tags with a
  filter, the device-type choice, import and export.
* **Operations → Evidence.** The list, a viewer per kind (text preformatted;
  a drawing shown as an image so nothing in it can run; a chart re-drawn
  from the stored points with its gaps), attach and detach. An incident's
  timeline gets "Attach evidence…".
* **Images, without storing them.** `ui/src/lib/capture.ts`: the backend's
  SVG of the map to PNG; a chart's canvas plus its title and legend to PNG;
  "Copy image" with a download as the fallback where a WebView refuses an
  image on the clipboard; and the numbers behind a chart as CSV. The
  topology export dialog gains PNG, and every chart gains a small export
  menu (PNG, CSV, keep as evidence). The dialog says that the picture is the
  engine's drawing of the same graph, not a pixel copy of the screen.
* **Downloads.** One `ui/src/lib/download.ts` replaces four inline copies
  and reports a failure instead of always toasting "Saved".
* **Clips.** A small panel of the last things copied *through a control that
  opted in* — an interface name, an address, a line of output. It is held in
  memory only: not written to storage, not sent anywhere, gone on reload,
  with a Clear button. From a clip: copy again, type into the focused
  session (through the paste confirmation), keep as evidence
  (`evidence.add_text`), save as a snippet.
  * Recording is opt-in per call site: `copyText(text, {clip: {label,
    source}})`. The default records nothing, so the toolbox's decoded
    secret, generated passwords and private key are excluded by
    construction, not by a filter.
  * The system clipboard is never read to build a history.

## Not building, and why

* **Screenshots of the screen, a window or a region.** It needs a
  screen-capture permission or a second native command in a shell that has
  one door, and pixels cannot be redacted: an image of `show
  running-config` is a secret nobody can find again.
* **Stored terminal screenshots.** For the same reason. Text with a source,
  a time and a hash is better evidence — searchable, checkable, redactable.
* **A clipboard monitor or a clip history on disk.** It would collect
  password-manager copies and revealed secrets into a document.
* **Arbitrary files attached to an incident.** No redaction and no source
  Nocturne can vouch for.
* **Macros that press Enter, and a snippet runner for many devices.** That
  is a change.
* **The assistant writing evidence.** The timeline is a record of what
  happened, kept by the engine and by people.

## Tests

Evidence through `App::rpc` with a real terminal session on the fake device:
a capture stores what `terminal.snapshot` returned, redacted, with the hash
of what is stored; a secret in the text is not in the item, in the audit
entry or in an event; `add_text` is labelled operator-supplied; limits
refuse rather than truncate; attach is one transaction with a concurrent
note; delete leaves a tombstone; export escapes hostile device output in
every format. Snippets: variables found and checked; render strict; a value
with a line break refused; send refuses on a hash mismatch and audits
without the text; import with each conflict mode and `dry_run`. Interface:
the clip store records nothing by default and never for the secrets
toolbox; SVG and chart export produce an image; end to end, a capture from
a lab session appears under Evidence and on an incident's timeline.
