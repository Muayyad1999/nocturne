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
| `terminal.attach` | `{session_id}` → `{info: SessionInfo, scrollback: base64, seq}`; the client then applies `terminal.output` events with `seq` greater than this |
| `terminal.snapshot` | `{session_id, lines?: 200}` → `{text}` — the last N lines as plain text (escape sequences removed with `nocturne_drivers::term`), for "explain this" and copy |
| `terminal.close` | `{session_id}` → `{}` |
| `terminal.list` | → `[SessionInfo]` |
| `terminal.classify` | `{device_id, text}` → `[{line, class, reason}]` — what the device's policy thinks of each line, so the UI can warn before a paste that reloads a switch |
| `terminal.serial_ports` | → `[SerialPortInfo]` (local access only) |
| `terminal.snippets.list` / `.save` / `.delete` | saved commands: `{id?, name, text, profile?, tags}` |
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
