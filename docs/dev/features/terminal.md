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
| `terminal.snapshot` | `{session_id, lines?: 200}` → `{text}` — the last N lines as plain text (escape sequences removed with `nocturne_drivers::term`), for "explain this" and copy |
| `terminal.close` | `{session_id}` → `{}` |
| `terminal.list` | → `[SessionInfo]` |
| `terminal.classify` | `{device_id, text}` → `[{line, class, reason}]` — what the device's policy thinks of each line, so the UI can warn before a paste that reloads a switch |
| `terminal.serial_ports` | → `[SerialPortInfo]` (local access only) |
| `terminal.snippets.list` / `.save` / `.delete` | saved commands: `{id?, name, text, profile?, tags}`. `save` with an `id` **replaces the whole record**: a field that is left out is erased (`tags`) or reset (`profile` → any device), so a client sends back every field, including the ones it does not show |
| `terminal.logs.list` | `{device_id?, query?, limit}` → `[{name, device_id, title, started_at, size}]` |
| `terminal.logs.read` | `{name, offset?, limit?}` → `{text, size, eof}` |
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
timestamps and stripped escapes; snippets CRUD; `terminal.classify`; SFTP
against a real `sshd` if one can be started privately on a high port, else
skipped with a note; output coalescing under a 5 MB burst (bounded event
count, bounded memory, bytes intact).

Interface (`ui/src/pages/terminal/*.test.ts`, `ui/tests/terminal.spec.ts`
against a real `nocturne-server` and a console that records what it ran and
the bytes it was sent): a browser paste is held until answered and sent once;
"Cancel" sends nothing; CRLF arrives as one CR per line; a program that asked
for bracketed paste gets brackets and runs nothing; a hole in the numbering
stops the screen and re-attaches; editing a snippet keeps its tags and device
type. Two of the end-to-end tests change what the browser gives the page,
because the fault they guard against needs conditions a plain browser does
not have. One loses `terminal.output` frames at the browser's door, on a
socket that stays open, and then hands over the engine's `events.lagged`
notice: the screen must end with every answer, once and in order, both for a
session that prints again and for one that has gone quiet. The other presents
the page as the desktop shell does — `__TAURI_INTERNALS__`, a clipboard a
script may not read, a `window.open` that goes nowhere — and requires the
Paste button, the context menu and a link in the output to go through the two
plugin commands the shell lends (`docs/dev/spec-desktop.md`).
