# Feature: terminal (`nocturne-terminal`)

The thing a network engineer has open all day. Interactive sessions to
inventory devices and ad-hoc hosts over SSH, Telnet and serial, a local shell,
SFTP, saved snippets, and session logs — with the session living in the
backend so the UI can come and go.

Engines: `nocturne-transport` (read `docs/dev/spec-transport.md`), core
`connect`.

## Model

A **session** is a `Connection` owned by the backend plus a scrollback ring
buffer (raw bytes, default 512 KiB, setting) and counters. The UI attaches by
id, receives output as events and sends input by RPC. Closing the window does
not close sessions; `terminal.list` finds them again.

```rust
struct SessionInfo {
    id, title,                       // "SW1" / "admin@10.0.0.1" / "COM3" / "Local shell"
    kind: ssh|telnet|serial|local|sim,
    device_id: Option<String>, lab: bool,
    opened_at, state: connecting|open|closed, close_reason: Option<String>,
    cols, rows,
    info: ConnectionInfo,            // peer, banner, ciphers, legacy_crypto, via (jump hosts)
    bytes_in, bytes_out, logging: Option<String /* log file name */>,
}
```

## RPC (contract)

| Method | Params → Result |
|---|---|
| `terminal.open` | `{device_id?, adhoc?: Adhoc, cols, rows, log?: bool}` → `SessionInfo`. Exactly one of `device_id` / `adhoc`. Returns once connected (errors carry the transport's code: `auth`, `host_key`, `refused`, …). |
| `terminal.input` | `{session_id, data: string}` (UTF-8 keystrokes/paste) → `{}` |
| `terminal.input_many` | `{session_ids: [..], data}` → `{sent: n}` (broadcast typing to several sessions) |
| `terminal.resize` | `{session_id, cols, rows}` → `{}` |
| `terminal.send_break` | `{session_id}` → `{}` |
| `terminal.attach` | `{session_id}` → `{info: SessionInfo, scrollback: base64, seq}`; the client then applies `terminal.output` events with `seq` greater than this, each exactly one more than the last (see "Output numbering") |
| `terminal.snapshot` | `{session_id, lines?: 200, redact?: true}` → `{text, lines, redacted}` — the last N lines (at most 10,000) as plain text (escape sequences removed with `nocturne_drivers::term`), for "explain this", copy and capture. Device secrets are hidden unless `redact: false`, and such a read is audited (see "What is read back") |
| `terminal.close` | `{session_id}` → `{}` |
| `terminal.list` | → `[SessionInfo]` |
| `terminal.classify` | `{device_id, text}` → `[{line, class, reason}]` — what the device's policy thinks of each line, so the UI can warn before a paste that reloads a switch |
| `terminal.serial_ports` | → `[SerialPortInfo]` (local access only) |
| `terminal.snippets.list` | `{profile?, session_id?, query?}` → `[Snippet]`: `{id, name, text, profile, tags, created_at, updated_at, variables: [{name, default?}], problem?, chosen?, chosen_instead?}`, by name. `session_id` (or `profile`, not both) asks for one session's snippets and adds `chosen` (see "Snippets") |
| `terminal.snippets.save` | `{id?, name, text, profile?, tags}` → the snippet as `list` gives it. With an `id` it **replaces the whole record**: a field that is left out is erased (`tags`) or reset (`profile` → any device), so a client sends back every field, including the ones it does not show |
| `terminal.snippets.delete` | `{id?, ids?}` → `{deleted}` |
| `terminal.snippets.render` | `{id, values: {name: text}}` → `{text, lines, sha256, variables}` — strict; types nothing |
| `terminal.snippets.send` | `{id, values, session_ids: [..], expect_sha256}` → `{sent: [{session_id, lines}], failed: [{session_id, code, message}]}` — renders again, refuses a text that is not the confirmed one, types it into every session or none, and audits it |
| `terminal.snippets.export` | → `{format: "nocturne.snippets/1", snippets: [{name, text, profile, tags}]}` |
| `terminal.snippets.import` | `{document, on_conflict?: skip \| replace \| rename, dry_run?}` → `{created, replaced, skipped, renamed, problems: [{index, name?, problem}], applied, dry_run}` — the whole document or none of it |
| `terminal.logs.list` | `{device_id?, query?, limit}` → `[{name, device_id, title, started_at, size}]` |
| `terminal.logs.read` | `{name, offset?: 0, limit?: 262144, redact?: true}` → `{text, offset, size, eof, next_offset, redacted}` — a page of whole lines: it starts with the line `offset` falls in (the answer's `offset` says where that is) and holds as many lines as fit in `limit` bytes (at most 1 MiB), never fewer than one. Continue from `next_offset`. Redacted unless `redact: false`, which is audited (see "What is read back") |
| `terminal.logs.delete` | `{names}` |
| `sftp.open` | `{device_id}` → `{sftp_id, home}` (SSH devices only) |
| `sftp.list` / `.stat` / `.mkdir` / `.remove` / `.rename` / `.close` | as the transport's `Sftp` |
| `sftp.read_text` / `.write_text` | `{sftp_id, path}` → `{text}` (≤ 1 MiB, UTF-8 only; says so otherwise) / `{sftp_id, path, text}` |
| `sftp.download` / `.upload` | `{sftp_id, remote, local}` → job with byte progress (local access only) |
| `keys.generate` | `{kind: ed25519|rsa3072|rsa4096, comment}` → `{private_key, public_key}` (never stored by this call) |

```rust
struct Adhoc {                                   // #[serde(default)]
    transport: ssh|telnet|serial|local,
    host, port, username,
    password: Option<String>,                   // used for this session only, never stored or logged
    credential_id: Option<String>,              // or take everything from a stored credential
    jump_device_id: Option<String>,
    serial: {baud, data_bits, parity, stop_bits, flow},
    local: {program, args, cwd},
    title: Option<String>,
}
```

Events (contract): `terminal.opened {SessionInfo}`, `terminal.output
{session_id, seq, data: base64}` (coalesce bursts: flush at most every
~8 ms or 32 KiB so a `show tech` does not become ten thousand events),
`terminal.closed {session_id, reason}`, `terminal.changed {SessionInfo}`
(resize, logging toggled).

### Output numbering

`seq` starts at 1 for a session and goes up by exactly one with every
`terminal.output` event; `terminal.attach` answers with the number of the last
event its scrollback includes. That is the whole hand-over, and it is also how
a client knows it has lost output:

* The event stream drops events for a client that has fallen too far behind
  (`EVENT_BACKLOG` in the core) and says so with `events.lagged {missed}` —
  **on the same connection, which stays open**. Nothing reconnects.
* So an event whose `seq` is not the next one is a hole. The client must not
  write it: it stops, calls `terminal.attach` again, holds what arrives in the
  meantime, and rewrites the screen from the scrollback. Writing around the
  hole would show output the device never sent in that form, and "Copy" and
  "Explain" would hand it on.
* A session that printed nothing after the lost events never shows a hole, so
  on `events.lagged` (and when the stream comes back after a break) every
  terminal attaches again; the answer is used only where the backend's `seq`
  has run ahead of the screen.

The scrollback ring is the bound on what can be recovered: a burst larger
than `scrollback_kib` comes back without its beginning, never with a silent
gap in its middle.

## Behaviour

* Device sessions go through `connect::open` (stored credential, jump hosts,
  host-key policy). Ad-hoc SSH uses `connect::verifier(app)` — the same
  known-hosts store. A changed host key surfaces as error code `host_key`
  with both fingerprints in `details`.
* **Auto-login** (setting `terminal.auto_login`, default on) for Telnet,
  serial and simulated devices that have a credential: answer the profile's
  username/password prompts once, at the start, then get out of the way. It
  must never type a password at anything that is not a login prompt (no
  echoing a secret into a shell).
* The local shell and serial ports need `app.require_local_access(..)`.
* **Session logging** (setting `terminal.log_sessions`, default off; per
  session `log: true`): plain text, escape sequences stripped, each line
  prefixed with an ISO timestamp, to `<data_dir>/sessions/YYYY-MM/<title>-<started>.log`
  (sanitise the file name). In-memory apps (no data dir) keep no logs. What
  the user types into a password prompt is not echoed by the device and so is
  not logged; do not log input separately.
* A session whose UI went away keeps running; idle sessions close after the
  `terminal.idle_timeout_min` setting (0 = never, default 0). Cap sessions at
  64. Closing the app closes all.
* Back-pressure: a device printing faster than the UI drains must not grow
  memory without bound (the ring buffer is the bound) and must not stall
  other sessions.
* Settings section `terminal`: `{scrollback_kib: 512, log_sessions: false,
  auto_login: true, idle_timeout_min: 0, keepalive: true}`.

## Snippets

The contract between the features is in `docs/dev/spec-capture.md`
("Snippets"); this is all of it for this crate, including what that leaves
open.

What is stored is `{id, name, text, profile, tags, created_at, updated_at}`
and nothing else — as before variables existed, so nothing is migrated. A
snippet's variables are read out of its text each time it is listed.

**What a snippet may be.** These rules are checked by `save` and, entry by
entry, by `import`; there is no other way in.

* A name of at most 200 characters, a text of at most 256 KiB that is not
  blank, at most 32 tags of at most 64 characters, at most 2,000 snippets.
  More tags than that is refused (it used to be cut to 32 without a word).
* The name and the tags are plain text on one line. The text may hold line
  breaks and tabs and no other control character, and neither may hold a
  text-direction mark (U+202A–202E, U+2066–2069) or a line separator
  (U+2028/2029). A control character is a keystroke, not text — Ctrl+C,
  Escape, a backspace over what the confirmation just showed — and a snippet
  that presses keys is a macro, which is not built.
* `profile` names a device type that is loaded here, or is absent for any
  device.
* No second snippet of the same name **and** the same `profile`: `conflict`,
  with the other one's `id` in `details`. Names are compared without regard
  to case and to spaces around them. Saves and imports write one at a time,
  so two that arrive together cannot both take a name, or both the last
  free place.
* Every placeholder is well formed (next).

**Variables.** `{{name}}` and `{{name=default}}`. A name is ASCII letters,
digits and `_`, at most 64 characters, with nothing around it (`{{ name }}`
is refused, not trimmed: with a default it would be a guess where the
default starts). The default is everything between the first `=` and the
`}}`, exactly as written; it follows the rules of a value (below), and it may
be empty — `{{descr=}}` may be left out, `{{descr}}` may not. A placeholder
opens and closes on one line. A variable may stand in several places; a
default given in one of them holds for all, and two different defaults are
refused. At most 32 variables.

To have two braces typed as they are, write `\{{` (a Go template, a Jinja
expression). The backslash is only special there: `\\{{name}}` is a backslash
of the text's own, as in a Windows path, followed by the variable. `}}`
without a `{{` before it, a single brace and any other backslash are text.

A text that breaks these rules is refused with where: the message says
`line L, column C:` (both from 1, the column in characters) and `details` is
`{line, column}`. The message never repeats the text.

`list` gives each snippet's `variables: [{name, default?}]` in the order they
first appear. `default` is absent when there is none and `""` when it is
empty: the two are different.

**Variants.** Two snippets may share a name when their `profile` differs.
Which one a session gets is decided here and nowhere else:
`terminal.snippets.list {session_id}` answers for that session.

* The session's *device type* is the profile its inventory device is set to,
  or else the one detection remembered for it. A session that is not to an
  inventory device, and a device nothing was detected on, have none — the
  generic profile such a device is driven with is a fallback, not a type.
* With a device type: snippets for another device type are left out. Every
  row has `chosen`. It is `true` for a snippet of that device type, and for a
  snippet for any device unless a snippet of the same name exists for that
  device type; that one wins, the generic row has `chosen: false` and names
  the winner in `chosen_instead`.
* With no device type known: nothing is left out. Snippets for any device
  have `chosen: true`; those for a device type have `chosen: false` and no
  `chosen_instead` — the person may know what they are connected to, Nocturne
  does not, and so it does not say that one is right.
* `{profile}` asks the same question for a device type instead of a session.
  Without either there is no `chosen` at all.
* `query` narrows the answer after the choice is made: a generic snippet is
  still second to a variant the words do not match.

`render` and `send` take the `id` they are given. `send` never swaps in
another variant — the text that was confirmed is the text that is typed — but
it refuses a snippet for one device type in a session whose device type is
known to be another (`invalid`).

**Rendering.** `terminal.snippets.render {id, values}` puts the values in and
returns `{text, lines, sha256, variables}`:

* `text` is what will be typed. Every line ending (CRLF, a bare CR, LF) is
  one `\n`, and the one line break at the very end of a snippet is dropped:
  the last line is typed and left at the prompt, the Enter is the person's. A
  blank line left above it is theirs too and stays.
* `lines` is the number of lines in `text` (0 for an empty one); `sha256` is
  the SHA-256 of `text` as UTF-8, in lower-case hex.
* It is strict. `invalid`, with `details: {variables: [{name, problem}]}`
  naming every variable that is wrong, when a variable has neither a value
  nor a default (`missing`), a value is given for a variable the snippet does
  not have (`unknown`), a value is not a JSON string (`not_text`), contains a
  line break (`line_break`: LF, CR, U+2028, U+2029), contains any other
  control character — a tab included — or a text-direction mark
  (`control_character`), or is longer than 1,024 characters (`too_long`). The
  message says the same in words and never repeats a value.
* A value goes in once. One that looks like a placeholder is typed as it is.
* `values` is an object, or left out (`null` is the same) when there are
  none to give. Anything else is `invalid`, in words that do not repeat it:
  what was sent as a value may be a password.
* A text of more than 256 KiB once filled in is `invalid`, and is measured
  before it is put together.

**Sending.** `terminal.snippets.send {id, values, session_ids,
expect_sha256}`, after the paste confirmation has shown the person the exact
lines of `render`'s `text` — the interface's part; this method cannot see a
confirmation, only the hash of what was confirmed.

1. `session_ids` holds 1 to 64 sessions, none twice (`invalid`).
2. The snippet is read and rendered again, by the code `render` runs.
3. `expect_sha256` must be the hash of that text (hex, either case).
   Otherwise `conflict` — the snippet was edited or a value differs — with
   the current hash in `details.sha256`, and nothing is typed. A text that
   renders to nothing is `invalid`.
4. Every session is checked before anything is typed: it exists
   (`not_found`), is open (`closed`; `unavailable` while it connects) and is
   not known to be a device of another type than the snippet's (`invalid`).
   If any is refused nothing is sent to any of them; the error carries the
   first refusal's code and `details.refused: [{session_id, code, message}]`.
5. The text is typed down the road `terminal.input` takes, in the order of
   `session_ids`, as the interface's paste funnel would have typed it: one
   carriage return per line break and nothing after the last line. It is
   bracketed (`ESC[200~` … `ESC[201~`, any `ESC[201~` inside removed first)
   only when it goes to a single session whose program has asked for that
   (DECSET 2004, which the backend follows in each session's output); a send
   to several is never bracketed.
6. `sent` lists the sessions that took it. `failed` is empty unless a
   connection stalled or dropped between the check and the typing; what the
   others received cannot be taken back, so the answer says who got it.
7. One audit entry, `terminal.snippet.send`: the snippet's id, name and
   device type, `lines`, `sha256`, `sessions: [{session_id, title, device_id,
   lab, sent}]` and `device_ids`. The entry's device is set when everything
   went to one inventory device. Never the text and never the values. This
   entry is the record of what was sent where — and of where it was not:
   when no session took the text the summary says so instead of "Sent".

It types into a person's terminal, so it belongs on the assistant's
never-reachable list.

**Taking the library out and in.** `terminal.snippets.export` returns
`{format: "nocturne.snippets/1", snippets: [{name, text, profile, tags}]}` —
what a person wrote, without ids and timestamps, by name.

`terminal.snippets.import {document, on_conflict?, dry_run?}` reads such a
document: the object itself, or its JSON text as read from a file (a
byte-order mark in front of it is skipped). It is hostile input and is judged
whole before anything is written.

* Refused outright, `invalid`: a document of more than 4 MiB (as JSON text),
  one that is not JSON, whose `format` is not exactly `nocturne.snippets/1`,
  that has no `snippets` list or more than 2,000 entries, and a `document`
  that is neither an object nor text. `conflict` when the snippets here plus
  those it would add pass 2,000.
* Every entry must be `{name: text, text: text, profile?: text | null, tags?:
  [text]}` (other fields are ignored), pass the rules above, and not share
  its name and device type with an earlier entry of the same document. What
  fails is listed in `problems: [{index, name?, problem}]` — `index` from 0;
  `name` only when the entry has one, cut to 80 characters and with anything
  that would not show replaced.
* **All or nothing.** While `problems` is not empty nothing is imported, all
  four counts are 0 and `applied` is `false` — with `dry_run` or without.
* `on_conflict` says what happens to an entry whose name and device type a
  snippet here already has: `skip` (the default — it loses nothing) keeps
  the one here; `replace` gives it the entry's name, text and tags and keeps
  its `id` and `created_at` (a problem when two snippets here share that name
  and device type, since it is not clear which is meant); `rename` keeps both
  and brings the entry in as `name (2)`, or the first number that is free.
  An entry that says word for word what the snippet here says is `skipped` in
  every mode — and so is, with `rename`, one that an earlier import already
  brought in as `name (n)` — so importing a document twice adds nothing.
* `created` counts what was added (`renamed` of them under a new name),
  `replaced` and `skipped` the rest; `applied` is `true` when the snippets
  now are as the answer says.
* `dry_run: true` returns exactly what the import would, and writes nothing,
  audits nothing and publishes nothing.
* What is written is written in one transaction, then audited once
  (`terminal.snippet.import`: the counts and the mode) and announced once
  (`terminal.snippets.changed {saved: [ids]}`).

**Snippets saved before variables existed** are read by today's rules. Plain
text has no variables and is typed as before. `{{name}}` in an old text is
now a variable. A text that today's rules refuse — braces that are not a
placeholder, a control character — is still listed, with `problem` saying
what is wrong and where and with `variables: []`; `render` and `send` refuse
it (`invalid`) until it has been corrected and saved again, rather than
guessing what was meant. Two old snippets that share a name and a device
type both stay and work; each can be edited under the name it has, and only
taking such a name anew is refused.

Audit: `terminal.snippet.save`, `.delete`, `.send`, `.import`. Event:
`terminal.snippets.changed {saved: [ids]}` / `{deleted: [ids]}`.

## What is read back

The byte stream of a live session is its user's own view of the device and
is never altered. A session log on disk is what the device printed, like a
stored configuration. What is *read back* from either — `terminal.snapshot`,
`terminal.logs.read` — has device secrets hidden (the redaction engine's
rules plus the `config.secrets` patterns of the device's profile, applied
behind the prompt and the log's timestamp), and says so with `redacted`.
Each line is redacted a second time with its prompt in place, because not
everything that starts like a prompt is one: a line of `/etc/shadow` starts
`root:$`, and that `$` is where the hash begins.

**A read is a window, and no window may cut a secret open.** A snapshot is
the last lines of a scrollback and a page is a piece of a file.

* Both are made of whole lines, whatever was asked for. `terminal.logs.read`
  starts with the line its `offset` falls in and ends at a line break, and
  returns one whole line when `limit` is smaller than that: half of `enable
  secret Sup3rS3cret` is a word nothing recognises. Only a line of more than
  1 MiB is returned in pieces — the log writer ends a line itself after
  64 KiB of output without a line break — and a piece never splits a
  character.
* A private key is the one secret longer than a line, so a window can hold
  lines of one and neither of its markers. Whether a line is key material is
  therefore decided on more than the window: all of the scrollback for a
  snapshot, and up to 256 KiB of the file on either side of a page. That
  context is read and never returned.
* A key's first marker claims the lines after it and its last marker the
  lines before it, across at most three lines in a row that are something
  else (a pager's `--More--`, a line the device logged) and never past
  another marker. A line of base64, a blank line and — above the body only —
  a PEM header count as key material; in a session log each is judged
  without its timestamp, and keeps it. The first marker stays, to say what
  was there; every other line of the key reads `<hidden>`.

**`redact: false`** returns the text as the device printed it and is audited
as an unredacted configuration view is: action `terminal.view_unredacted`,
the entry's device the session's or the log's, summary `Viewed unredacted
terminal output of <title>` / `Viewed unredacted session log of <title>`,
detail `{source: "snapshot", session_id, title, lines}` or `{source: "log",
name, offset, bytes}` — which output and how much, never what it said. One
entry per read that returned text; a read that returned nothing (past the
end of a log, a session that has printed nothing) is not a view.

## The interface: one road for pasted text

`terminal.input` takes whatever it is given and sends it. Everything that
makes a paste safe is therefore the interface's job, and it is done in one
place (`ui/src/pages/terminal/pasteFunnel.ts`, reached through
`terminals.paste` in `sessions.ts`) for every screen that shows a terminal —
the Terminal screen, a lab's consoles, the session docked on the map.

**Every door leads to it.** Text can arrive by the browser's own paste
(Shift+Insert, Ctrl+Shift+V, ⌘V, a middle click on Linux, the WebView's
Edit ▸ Paste), from the toolbar, the context menu, a snippet, or as text
dropped on the terminal. xterm.js would send a browser paste to the device
the instant the event arrives, so the registry takes the `paste` event in the
capture phase on the element xterm lives in and xterm never sees one. No
other code calls `terminal.input` with pasted text.

**What the funnel does, in order:**

1. Normalises line endings: CRLF, a bare CR and LF each become one line
   break. (`terminal.classify` splits on LF only; text with bare carriage
   returns would otherwise be judged as a single line.)
2. Asks `terminal.classify` with the session's device. If the policy cannot
   be asked, the paste is not blocked; the question says so.
3. Asks the person when the text has more than one line, when any line is
   not read-only, or when typing is being broadcast to several sessions.
   One read-only line goes straight through. The Terminal screen shows the
   sheet with every line and its class; other screens a plain confirm dialog.
4. Sends once, after the answer — never before it, and nothing at all on
   "Cancel". Pastes are handled strictly one after another.

**What is sent.** One carriage return per line break, which is what the Enter
key sends. A Windows clipboard's CRLF used to reach the device as CR then LF,
and the LF is a second Enter: after `reload` it answers `[confirm]`.

**Bracketed paste.** When the program at the far end has asked for it
(DECSET 2004 — bash, zsh, vim; network operating systems do not), the paste
is sent between `ESC[200~` and `ESC[201~`, as a terminal should: the shell
then holds the lines instead of running each one, and an editor does not
re-indent them. Two exceptions, both deliberate: any `ESC[201~` inside the
text is removed first, so the text cannot close the bracket and have its
remainder run as typed; and a paste that is being broadcast is never
bracketed, because the sessions have not all asked, and to one that has not
the markers are six characters typed at its prompt.

**Typing on the heels of a paste.** Judging a paste takes a round trip. What
is typed into the same session meanwhile — the Enter after a pasted command —
is held and sent behind the paste, not ahead of it; if the paste is refused it
is dropped with it. Once a question is on screen nothing is held: what
arrives for the session then is the terminal answering its device.

**Reading the clipboard.** A browser paste carries its own text, so the
keyboard needs no permission. Only the toolbar button and the context menu
read the clipboard: `navigator.clipboard` in a browser (which may refuse, and
does not exist over plain HTTP — the person is told, and told which keys work
without it), and the shell's clipboard plugin on the desktop
(`docs/dev/spec-desktop.md`).

**Snippets** go the same way with one difference: the line break that ends a
snippet is not sent. The Enter is always the person's.

With an engine that has `terminal.snippets.render` and `.send`, a snippet is
not typed from the window at all, and the road is the same one with its last
step moved into the engine. The text is the engine's (`render`): a snippet
with variables shows it in the drawer beside the form, with a refused value's
sentence at the field it is about, and nothing is checked on this side. The
funnel then asks its question about exactly that text (`PasteFunnel.confirm`:
the same sheet, in the same queue behind any paste that is waiting, by the
same rule — several lines, a line that is not read-only on any of the devices
it would reach, or more than one session) and hands nothing to
`terminal.input`. After a yes, `terminal.snippets.send` types it, held to the
hash of the render that was asked about; a snippet that changed in between is
refused and the drawer shows what it renders to now. One read-only line into
one session is typed without a question, as when it is pasted. With an engine
that lacks the two methods a snippet goes down the funnel as before.

**Clips** (text copied earlier through a control that opted in; see
`docs/dev/spec-capture.md`) typed into a session are a paste by another door:
`terminals.paste`, asked about like any other.

**Links** in the output (`http`, `https` only) open outside the application:
a new tab from a browser, the system browser from the desktop shell.

## Tests

Sessions against `nocturne_app::testing` fake devices (open, type a command,
see echoed output events in order with increasing `seq`, attach mid-stream
and get scrollback + later events with no gap and no duplicate, resize,
close from either side, list); auto-login against a `Connection::pair`
scripted Telnet-style login (and proof that it does not type the password at
a non-login prompt); ad-hoc SSH against `nocturne_transport::serve::serve_ssh`
on port 0 (password ok / wrong, host key learned then changed); local shell
(`sh -c`) incl. refusal without local access; logging on disk with
timestamps and stripped escapes; snippets (CRUD; variables found and
checked on save with the place of a fault; the choice among variants for a
session; strict rendering; a send typed with one CR per line and no Enter of
its own, refused on another hash, reaching every session or none, bracketed
for the one session that asked, and audited without its text or values;
export; import in each mode, as a dry run, and refused whole on one bad
entry; snippets stored in the old shape); a private key hidden in a snapshot
and in a log wherever the window or the page cuts it; the audit entry of a
read with `redact: false`; `terminal.classify`; SFTP
against a real `sshd` if one can be started privately on a high port, else
skipped with a note; output coalescing under a 5 MB burst (bounded event
count, bounded memory, bytes intact).

Interface (`ui/src/pages/terminal/*.test.ts`, `ui/tests/terminal.spec.ts`
against a real `nocturne-server` and a console that records what it ran and
the bytes it was sent): a browser paste is held until answered and sent once;
"Cancel" sends nothing; CRLF arrives as one CR per line; a program that asked
for bracketed paste gets brackets and runs nothing; a hole in the numbering
stops the screen and re-attaches; editing a snippet keeps its tags and device
type; a snippet with variables is typed only as the text that was shown and
confirmed, once, with no Enter of its own, into the sessions that were
ticked, and leaves an audit entry without its text or values; the drawer
shows the variant the engine chose; an import is checked before it is
applied; a session log is read page by page, redacted until asked, and the
asking comes before the audited read; the last lines and a selection are kept
as evidence, the first read by the engine and the second labelled the
operator's; a clip lives in memory only and is asked about before it is
typed; and with the newer methods taken out of the engine's own
`app.methods` answer no control for them is drawn and none of them is called.
Two of the other end-to-end tests change what the browser gives the page,
because the fault they guard against needs conditions a plain browser does
not have. One loses `terminal.output` frames at the browser's door, on a
socket that stays open, and then hands over the engine's `events.lagged`
notice: the screen must end with every answer, once and in order, both for a
session that prints again and for one that has gone quiet. The other presents
the page as the desktop shell does — `__TAURI_INTERNALS__`, a clipboard a
script may not read, a `window.open` that goes nowhere — and requires the
Paste button, the context menu and a link in the output to go through the two
plugin commands the shell lends (`docs/dev/spec-desktop.md`).
