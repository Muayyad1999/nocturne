# nocturne-logs — build notes

Spec: `docs/dev/features/logs.md`. Conventions: `docs/dev/features/README.md`.

## State

Syslog reception, storage, search, live tail, pattern rules, correlation and
device log buffers: done. **SNMP traps and flow collection are not built** —
`logs.status` reports them as `implemented: false` rather than as switched
off, and `flows.top` is not registered (callers get `unknown_method`).

67 tests (27 unit, 40 through `App::rpc` and real sockets),
`cargo clippy -p nocturne-logs --all-targets -- -D warnings` clean,
`cargo fmt -p nocturne-logs --check` clean, no `unsafe`.

```
src/
  lib.rs        init(): settings, tables, state, the RPC surface, on_start
  settings.rs   the `logs` section (off by default, unprivileged ports)
  model.rs      status / histogram / top / burst / rule shapes (contract)
  db.rs         own tables (logs_syslog, logs_sources), batch insert, retention
  state.rs      counters, receiver slot, rate window, follows, throttles
  receiver.rs   the UDP/TCP receiver and the supervisor that follows settings
  ingest.rs     parse → attribute → redact → batch write; retention task
  attribute.rs  address → device, and a line's interface → the device's own
  guard.rs      the redaction prefilter and this crate's extra rules
  query.rs      the shared filter, query / histogram / top / bursts / sources
  tail.rs       logs.appended batching and sampling; logs.follow
  rules.rs      21 built-in rules, throttling, signals, logs.rules.*
  correlate.rs  logs.around: the line next to what else was known
  pull.rs       logs.pull: `show logging` / `/log print` into the same store
  setup.rs      logs.device_setup: the lines that would point a device here
tests/
  receiving.rs  real datagrams and connections, attribution, redaction
  searching.rs  filters, paging, histogram, top, bursts, retention, clear
  signals.rs    rules, throttling, the alert hand-off, the tail, logs.around
  devices.rs    pulling a buffer, de-duplication, device_setup per platform
  flood.rs      load: simulator-generated lines over a real socket
```

## Decisions worth remembering

* **`ts` is the time of receipt, not the device's.** A device's clock is its
  own business; `body.timestamp` keeps what it wrote, verbatim. The one
  exception is a pulled buffer, where stamping a week of history at the
  moment of the pull would be worse — there the device's date is used when it
  parses, and `body.ts_source` says `device` or `received`.
* **Attribution is by source address only** (`device.address`, then a
  collected interface address, then a device whose `address` is a host name
  matching the source literally). Never by the host name *inside* the
  message: two switches called `SW1` in different sites is normal, and a line
  filed against the wrong one is worse than one filed against none. An
  address two devices claim is a refusal naming both, not a coin toss.
* **`logs_syslog`** holds facility, app, mnemonic, host, the resolved
  interface and a line fingerprint, keyed on the core row's id. Without it,
  "only the `%LINK-3-UPDOWN` lines about `Gi1/0/3`" and a per-severity
  histogram are `json_extract` over the whole table.
* **`logs_sources`** is the other half of attribution: every address that has
  sent, the device that claims it, and the reason when none does — including
  addresses whose lines are all dropped (`unknown_sources: "drop"`), because
  a setting that makes a device invisible has to be visible itself.
* **A text term is looked for in the message, the sending address and the
  mnemonic.** The mnemonic is not part of the message column, and a person
  searching `UPDOWN` means the mnemonic. A term may be negated (`-up`) and
  quoted (`"changed state to down"`).
* **The live tail samples.** Over 200 entries in a window it sends an even
  sample across the window plus the newest line, with `sampled: true` and the
  number left out. The first 200 of 18,000 would be a lie about the shape of
  a storm. `logs.follow` adds server-side filtered streams (`logs.tail`) so a
  screen does not receive the whole firehose to filter in the browser.
* **One tail event is capped in bytes as well as entries** (64 KiB of log
  text). An entry can be 64 KiB on its own, and the event bus keeps its whole
  ring resident, so "200 entries" alone would allow megabytes in one event.
  The newest lines that fit are sent and the rest counted; the full lines are
  in the store, which is where a screen reads them from anyway.
* **Rules match the raw line**, mnemonic and all, because that is what vendor
  documentation shows and what a user pastes into `logs.rules.test`.
* **Throttling publishes the first match of a window at once** and counts the
  rest; the next signal after the window carries that count. A NOC wants the
  first flap immediately, not after a minute of aggregation.
* **Rules do not run over pulled lines.** A buffer is history, and
  `logs.config_changed` means "back this device up now".
* **Built-in rules can be switched off, not deleted or edited** (a delete
  would come back at the next start; `logs.rules.save` on a built-in keeps
  only `enabled`, `scope` and `throttle_s`).
* **Signals are stored as well as published** (stream `event`), so a
  correlation can be made afterwards — an event nobody was listening for is
  otherwise gone.
* **`logs.around` reports, it does not conclude.** Logs, bursts, this crate's
  signals, and alerts and changes borrowed from the other features by RPC
  name. Each borrowed list carries `state: ok | unavailable | failed`, so an
  empty list cannot be read as "nothing happened" when it means "that feature
  is not in this build".
* **`queue_limit` was added to the settings section** (default 50,000). The
  spec does not list it, but it is the memory the receiver may use to absorb
  a burst, it is the number the drop counter is about, and without it the
  drop behaviour can only be tested by overwhelming a machine.
* **`tcp_port: 0` means "no TCP listener"**, not "ephemeral". Tests that want
  an ephemeral TCP port ask the OS for a free one and retry if it was taken
  in between (`tests/support/mod.rs`).
* **Everything kept is redacted** (`guard.rs`) through
  `nocturne_ai::Redactor`. Forty-odd regular expressions per line is too much
  at twenty thousand lines a minute, so a cheap case-insensitive scan for the
  words those rules key on decides whether they run at all. The trigger list
  must stay a superset of what the rules need — a word missing there is a
  secret stored in the clear, which is why it has a test per secret shape.

## Measured

Debug build (how `cargo test` runs), in-memory store, one sender on loopback:

| | |
|---|---|
| sustained 8,000 lines at ~5,000/s (TCP) | 8,000 stored, 0 dropped |
| 100,000 lines pushed in under a second (TCP) | 100,000 received, 55,500 stored, 44,500 counted as dropped, 8.9 s, ~6,200 lines/s written |
| 20,000 lines with a 200-line queue and the writer blocked 300 ms | 20,000 received, 1,400 stored, 18,600 counted as dropped |
| 20,000 datagrams as fast as `send_to` takes them (UDP) | ~330 arrive; the rest never reach the process (see below) |

A release build does the same work about ten times faster: 20,000 lines
through `prepare` + insert + rules took 290 ms (≈69,000 lines/s), split
roughly evenly between parsing/attribution/redaction, the three inserts per
row, and the 21 rule patterns.

## Needed from the core and the engines (worked around here)

* **`store.log_append` is one row per call**, each taking the connection lock
  and its own implicit transaction. A storm needs batches, so `db.rs` writes
  the core's `logs` table with its own prepared statements inside one
  `store.transaction`. A `log_append_many(&[LogEntry]) -> Vec<i64>` on the
  store would let this crate drop that SQL.
* **`store.log_query` cannot answer what a log screen asks** (no facility, no
  mnemonic, no counts, no aggregation), so `query.rs` has its own SQL over
  `logs` joined to this crate's index table. That is the intended division —
  but it means the core's `logs` table now has two writers and two readers,
  and `LogQuery` is unused by this feature.
* **Retention is per feature.** `store.log_prune(stream, before)` deletes by
  age only, with no row cap and no batching, and it knows nothing about this
  crate's index rows, so `db::prune` does both in batches itself.
* **`nocturne_netprobe::syslog::SyslogServer` hands messages over an
  *unbounded* channel**, so the bound has to be re-imposed one hop later
  (`receiver::relay` into a bounded queue). The relay does nothing but count
  and forward, so the engine's channel does not build up — but the engine
  deciding its own bound, or taking a `Sender`, would be cleaner.
* **The engine's UDP socket uses the default receive buffer** and one receive
  loop. A sender blasting 20,000 datagrams at loopback loses about 98% of
  them inside the kernel before this process sees them. Real devices do not
  send that way, and the counters stay honest (what the kernel dropped is not
  counted as received), but `SO_RCVBUF` on that socket — socket2 is already a
  workspace dependency — would raise the ceiling a long way. It cannot be
  done from here.
* **`SyslogMessage` has no received-at field**, so the time of receipt is
  taken in this crate when the batch is written, not when the datagram
  arrived. Under a storm that moves a line's `ts` by up to the queue delay.
* **No profile field says how to read a device's log buffer.** `pull.rs` maps
  `config.syntax` to `show logging` (IOS-like) or `/log print without-paging`
  (RouterOS) and refuses anything else with `unsupported`. A
  `commands: {log_buffer: …}` key in the profile would make this data, as
  `docs/profiles.md` says a device type should be. The same goes for the
  syslog configuration lines in `setup.rs`.
* **`nocturne_ai::Redactor`'s configuration-command rules are anchored to the
  start of a line** (they were written for configuration files). A log line
  *quotes* a command in the middle of itself — IOS
  `%PARSER-5-CFGLOG_LOGGEDCMD`, FortiGate `msg="… set psksecret ENC …"` — so
  the anchored form misses the secret. `guard::EXTRA_RULES` adds the
  unanchored forms; the fix belongs upstream.
* **`nocturne_app::testing::lab` creates devices with no address**, which is
  exactly what a log receiver needs one for; every test here sets it with
  `devices.update` first. A `lab_with_addresses` helper, or an address per
  fake device, would save that step. `FakeDevice` also has no log buffer of
  its own, so `show logging` is a canned output in `tests/devices.rs`.
