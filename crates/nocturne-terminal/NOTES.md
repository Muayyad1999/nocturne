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
- [x] tests (spec "Tests" section), clippy, fmt
- [x] `ui/src/api/types/terminal.ts` (first version of the surface)
- [x] snippets, second version (`docs/dev/spec-capture.md`): variables,
      variants, `render`, `send`, `export`, `import`
- [x] redacted reads that no window can cut open; `redact: false` audited
- [ ] the interface for the above, and its TypeScript types: not this crate's

## Tests

`cargo test -p nocturne-terminal`: unit tests in `src/` plus nine
integration files.

```
tests/common/mod.rs  the app, an event recorder that checks the output
                     protocol as it reads, and scripted consoles (which keep
                     every byte they are sent, and can go deaf)
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
tests/workbench.rs    6  settings defaults, snippets CRUD, classify, keys, idle
tests/snippets.rs    31  variables and where a fault is, variants and the
                         choice for a session, strict rendering, sending
                         (bytes, hash, all-or-none, bracketing, the audit
                         entry, a session that dies under it), export, import
                         in each mode and as a dry run, hostile documents,
                         old stored snippets; the audit of `redact: false`;
                         what a value may not carry, a snippet changed after
                         it was confirmed, a document brought twice, a dry
                         run against every row of the store, racing writers
tests/redaction.rs   10  a private key in a snapshot and in a session log,
                         with the edge of the window in every place — a
                         log read from every byte, a snapshot of every
                         length; a secret behind what looks like a prompt;
                         reads into and through lines longer than a page
tests/sftp.rs         3  against a private OpenSSH `sshd` on a free high port
```

`NOCTURNE_WIRE=<file> cargo test -p nocturne-terminal --test snippets`
appends every call that test file makes — parameters and answer, as they
went over `App::rpc` — to the file. That is where the interface's types for
these methods are meant to be written from.

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
snippets.rs   terminal.snippets.*: the record, the save rules, the choice
              among variants, render
  template.rs   `{{name}}` / `{{name=default}}`: parse, render, what a value
                may hold, the bytes a keyboard would have sent
  send.rs       terminal.snippets.send: re-render, compare, check, type, audit
  transfer.rs   export and import (one transaction, or nothing)
pastemode.rs  DECSET 2004 followed in a session's output
logs.rs       the session-log index + terminal.logs.*
sessionlog.rs the writer: timestamped plain text, escapes resolved
sftp.rs       sftp.* (sessions held in the backend; transfers are jobs)
keys.rs       keys.generate
ssh.rs        SSH parameters for the connections this crate makes itself
hostkey.rs    turning a host-key refusal into an error with both fingerprints
platform.rs   which profile applies to a target
text.rs       plain text from raw output; redacting a window of a
              transcript, private keys the window cuts included
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
* **Snippet variables are derived, not stored.** The record is what it was
  (`{id, name, text, profile, tags}` + timestamps); `variables` is read out
  of the text on every list. So there is nothing to migrate, and a snippet
  from before the rules is judged by them when it is read: listed with
  `problem` if it fails, never dropped and never rewritten behind anyone's
  back.
* **`\{{` is the escape**, as in Handlebars, because the spec gives none
  and without one a Go or Jinja template could no longer be stored at all.
  `\\{{name}}` is a literal backslash and then the variable, for Windows
  paths.
* **Control characters are refused in a snippet's text**, not only in a
  value. The spec says so for values; for the text it follows from "an
  import is hostile input" and from the confirmation having to show what is
  typed. Tabs and line breaks stay.
* **An import is all or nothing.** The spec's result has `problems` beside
  the counts and says "nothing is half-imported"; read together: any problem
  means zero counts and `applied: false`. `renamed`, `applied` and `dry_run`
  are additions to the result.
* **`send` answers `{sent, failed}`**, not just `sent`: a connection can die
  between the check and the write, and then the answer has to say who got
  the text. It also refuses a snippet for one device type in a session known
  to be another, which the spec does not ask for.
* **Bracketed paste is followed in the backend** (`pastemode.rs`) so that a
  snippet sent by RPC is typed as the interface's paste funnel types a paste.
  Same two exceptions: the end marker is stripped from the text, and a send
  to several sessions is never bracketed.
* **A log page is whole lines** even when that means starting before
  `offset` or returning more than `limit` (one line at most). The answer has
  `offset` so the caller can tell. Without this, paging with a small `limit`
  defeated every redaction rule, not only the one for keys.
* **Key context** is the whole scrollback for a snapshot and 256 KiB either
  side of a page for a log. A key is followed across up to three lines in a
  row that are not key material. A bare word within that distance of a
  cut-off key is hidden with it: over-redaction is the safe failure.
* **`terminal.view_unredacted`** is the audit action of a `redact: false`
  read (snapshot or log), one entry per read that returned text. The
  detections feature counts `config.view_unredacted`; whether it should
  count this one too is that crate's decision.
* **One writer at a time for the snippet library** (`snippets::Library`, a
  mutex in the app's state held by `save` and `import` from their look at
  the library to their write). The rules a save checks — one snippet per
  name and device type, at most 2,000 — are read-then-write, and RPC calls
  run on several threads. `tests/snippets.rs` races sixteen saves for one
  name.
* **`values` is taken as any JSON** and checked by hand. Typed as a map,
  serde's own message for a wrong shape (`invalid type: string "…"`)
  repeated what was sent, and that message is logged.
* **Parsing a snippet is linear** in its text and a render is measured
  before it is built: `list` parses every snippet on every call, and one
  variable in fifty thousand places with a long value was 200 MB put
  together to be refused.
* **`rename` remembers itself.** An entry an earlier import brought in as
  `name (n)` is skipped the next time, or a document imported twice that
  way left `name (2)` and `name (3)` behind.
* **The start of a line is looked for as far back as a page reaches**
  (1 MiB less four bytes), one byte first — a page asked for where the last
  one ended starts a line — and then in steps. It was 128 KiB in one read,
  on the grounds that the writer breaks a line at 64 KiB; but that is 64 KiB
  of *output*, counted a read at a time, and a byte that is not text is
  three in the file. A read that fell further into such a line started in
  the middle of it, and half a line cannot be redacted. The four bytes are
  what keeps paging a line longer than a page from going round in a circle:
  a piece ends up to three bytes short of a full page, and the next read
  must not reach back to the start. `tests/redaction.rs` has both cases.
* **A transcript is redacted twice.** First what follows each prompt, so
  that the rules anchored to the start of a configuration line find it;
  then each line with its prompt back in place. The prompt pattern is a
  guess, and `root:$6$…` (a line of `/etc/shadow`) starts exactly like one:
  with a single pass the `$` went to the "prompt", the hash lost its first
  character and no rule knew it.
* **The import writes `docs` rows itself**, in one transaction, with the
  statement `Store::put` runs. The store has no way to put several documents
  atomically; if it grows one, `transfer.rs` should use it.
