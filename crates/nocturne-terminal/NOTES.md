# nocturne-terminal — working notes

Spec: `docs/dev/features/terminal.md`. Conventions: `docs/dev/features/README.md`.

## Status

- [x] settings, model, ring buffer
- [x] sessions: registry, pump, coalesced output, attach/snapshot
- [x] open: device + ad-hoc (ssh/telnet/serial/local), host-key details
- [x] auto-login
- [x] session logs (writer, index, list/read/delete)
- [x] snippets, classify, serial ports, keys
- [x] SFTP (+ transfer jobs)
- [x] tests (spec "Tests" section), clippy, fmt — 80 tests, all green
- [x] `ui/src/api/types/terminal.ts`

## Tests

`cargo test -p nocturne-terminal`: 31 unit tests in `src/` plus seven
integration files.

```
tests/common/mod.rs  the app, an event recorder that checks the output
                     protocol as it reads, and scripted consoles
tests/sessions.rs    12  open/type/resize/close/list, attach mid-stream,
                         snapshot, the far end hanging up, the 64 cap,
                         shutdown
tests/autologin.rs    9  the login dialogue end to end: banners that say
                         "Password:", a console that echoes, one that asks
                         twice, prompts that are not a login, the setting off
tests/coalescing.rs   3  a 5 MB burst, 12 MB with nothing attached, one
                         flooding session beside a quiet one
tests/adhoc.rs       10  SSH to `serve_ssh` (right and wrong password, host
                         key learned then changed), Telnet to `serve_telnet`,
                         the local shell, local-access refusals, closing a
                         session while it connects
tests/logs.rs         6  the file on disk, names, paging, deletion, the
                         setting and the toggle, redaction, no data directory
tests/workbench.rs    6  settings defaults, snippets, classify, keys, idle
tests/sftp.rs         3  against a private OpenSSH `sshd` on a free high port
```

`tests/sftp.rs` starts `sshd` itself (its own host key, `authorized_keys`,
config and directory) and stops it again. It needs `/run/sshd` to exist,
which a container image often lacks; it creates it if it can and otherwise
prints why it is skipping and passes.

## Layout

```
lib.rs        init: settings, state, routers, idle reapers, shutdown hook
model.rs      SessionInfo, Adhoc, event payloads
settings.rs   the `terminal` section
ring.rs       the scrollback ring (newest N bytes, resumes at a line start)
session.rs    registry, the per-session pump, coalescing, attach, close
open.rs       terminal.open: plan → dial → start
autologin.rs  answering login prompts, once, timidly
api.rs        input/input_many/resize/send_break/attach/snapshot/close/list/
              logging/classify/serial_ports
snippets.rs   terminal.snippets.*
logs.rs       the session-log index + terminal.logs.*
sessionlog.rs the writer: timestamped plain text, escapes resolved
sftp.rs       sftp.* (sessions held in the backend; transfers are jobs)
keys.rs       keys.generate
ssh.rs        SSH parameters for the connections this crate makes itself
hostkey.rs    turning a host-key refusal into an error with both fingerprints
platform.rs   which profile applies to a target
text.rs       plain text from raw output; redacting a transcript
```

## Decisions (where the spec left a choice)

* **Attach has no gap and no duplicate** because the ring, the pending buffer
  and `seq` move under one lock. `absorb` appends to the ring and to
  `pending`; `announce` (same lock) empties `pending` into an event and bumps
  `seq`. So at any instant with the lock held, "bytes in the ring that have
  not been announced" is exactly `pending.len()`. `attach` returns
  `ring.snapshot(pending.len())` with the current `seq`: a client that
  subscribed first and then attached applies everything with `seq >` that.
* **Coalescing**: flush at most every 8 ms, and at once when 32 KiB has piled
  up (also the largest event). Output after a quiet moment is announced
  immediately so a keystroke's echo does not wait 8 ms.
* **Back-pressure** is the ring: the pump never waits for a UI. Memory per
  session is bounded by `scrollback_kib` (clamped to 64 KiB…16 MiB) + 32 KiB.
* **Auto-login** answers at most one user-name prompt and one password
  prompt, only while the device has settled on that prompt for 250 ms, only
  before the first command prompt, only in the first minute and first 64 KiB,
  and never once the user has typed anything but Return. A password prompt
  whose line also matches `new|old|current|confirm|retype|…|enable|sudo` is
  treated as foreign and ends the watch. SSH and the local shell never get
  auto-login (SSH authenticates in the protocol).
* **`terminal.open` returns once connected.** The session is registered first
  (a `Reservation` that unregisters itself if the attempt is abandoned), and
  `terminal.opened` / the audit entry only go out once there is something to
  type into. Closing a connecting session cancels the attempt.
* **Sizes** are clamped to 20–500 columns and 5–200 rows; defaults 120 × 40.
* **Snapshots and session logs are redacted by default** (`redact: false` to
  get the text verbatim), per convention 3. The live byte stream never is:
  it is the user's own view of the device.
* **Logs** are `<data_dir>/sessions/YYYY-MM/<title>-<YYYYmmdd-HHMMSS>.log`,
  the title sanitised to `[A-Za-z0-9.\-_@+=]` (so no traversal, no reserved
  Windows device name — `COM3` only ever appears as `COM3-<stamp>.log`), with
  `-2`, `-3`… when two sessions to one target start in the same second. The
  index is this feature's own table; `terminal.logs.read` only accepts
  `YYYY-MM/<file>.log`.
* **Extra methods beyond the RPC table** (the table is a floor):
  `terminal.logging` (the `terminal.changed` event's "logging toggled"),
  `sftp.sessions`. `terminal.logs.read` and `terminal.snapshot` take
  `redact`; `terminal.logs.list` takes `before` for paging.
* **Extra events**: `terminal.removed {session_id}` (the backend has
  forgotten a session — closed, or evicted when the table of 64 was full),
  `terminal.logs.changed`, `terminal.snippets.changed`, `sftp.opened` /
  `sftp.closed`.
* **`terminal.classify`** takes an optional `device_id` (the generic policy
  without one) and returns one entry per line, blank lines included, so the
  UI can line them up with what the user pasted. At most 20 000 lines.
* **SFTP sessions** live in the backend under an id like terminal sessions,
  are capped at 16, and are dropped after 30 minutes unused (each one holds
  an SSH connection on the device). Transfers are jobs with byte progress and
  need local access; the local path must be absolute.
* **Ad-hoc passwords** arrive as `model::Secret`, which is not `Serialize`
  (so it cannot leave through a result, an event or an audit entry), says
  `<secret>` when printed, and wipes itself when the call that carried it
  ends. From there the password goes only into `SshAuth` (the transport's own
  type) or into `AutoLogin`, which holds it as `Zeroizing` and drops it the
  moment it has answered or given up.

## What I needed from the core and worked around

* `connect::open` gives a `Connection`, not an `SshSession`, and takes only
  inventory devices — SFTP needs the session and ad-hoc needs a free-form
  target, so `ssh.rs` rebuilds the same parameters (credential →
  authentication methods, jump chain, `connections` settings, shared
  known-hosts verifier). If the core ever grows
  `connect::open_ssh_session(app, &device)` and an ad-hoc variant, `ssh.rs`
  goes away.
* The host-key verifier reports the two fingerprints only as an event, so
  `hostkey.rs` listens for it around each attempt to put them in the error's
  `details`.
* `App::publish` keeps the last 4096 events in the broadcast channel whether
  or not anybody is reading (tokio drops them only when there is no receiver
  at all). A `show tech` at 32 KiB an event is therefore up to ~170 MB held
  by the bus when a UI is connected but slow. Nothing this crate can do from
  the outside; worth a bound on retained payload size in the core.
* `nocturne_app::testing::FakeDevice` has no login dialogue — it prints a
  prompt straight away — so the auto-login tests script their own consoles
  over `Connection::pair` behind a `SimConnector` (`tests/common/mod.rs`). A
  `FakeDevice` option for "ask for a user name and password first" would let
  them go away.
