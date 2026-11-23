# nocturne-logs — build notes

Spec: `docs/dev/features/logs.md`. Conventions: `docs/dev/features/README.md`.

## State

Syslog reception, the SNMP trap receiver, storage, search, live tail,
pattern rules (thresholds, per-instance throttling, alerts that clear, the
security built-ins, the trap built-ins), correlation, device log buffers,
`logs.ingest` for lab devices, forwarding to a log platform (syslog over
UDP, TCP and TLS as RFC 5424 or JSON; JSON over HTTPS to an event
collector) and the flow collector (NetFlow v5, v9 and IPFIX into one-minute
buckets of the biggest talkers, in memory, kept a day; `flows.top`,
`flows.exporters`): done.

The flow collector has met softflowd 1.1.0 (v5, v9, IPFIX, with and
without sampling) and nfdump 1.7.3's `nfreplay` (v5, v9), both run on this
machine against traffic written by hand as a capture file, and nothing
else: no datagram in its tests was sent by a router or a switch, and no
exporter in them reports interface numbers.

The forwarders have met listeners written in their own tests and nothing
else: no Splunk, no rsyslog, no syslog-ng, no Graylog has received a byte
from them. The HTTP event collector envelope and the `Authorization:
Splunk` header are written from what that interface is documented to take.

The trap receiver has met net-snmp's `snmptrap` and `snmpinform` and
nothing else: no datagram in its tests was sent by a switch or a router.

276 tests (120 unit, 156 through `App::rpc` and real sockets — two of
those run inside another test, in a process of their own),
`cargo clippy -p nocturne-logs --all-targets -- -D warnings` clean,
`cargo fmt -p nocturne-logs --check` clean, no `unsafe`.

```
src/
  lib.rs        init(): settings, tables, state, the RPC surface, on_start
  settings.rs   the `logs` section (off by default, unprivileged ports)
  model.rs      status / histogram / top / burst / rule shapes (contract)
  db.rs         own tables (logs_syslog, logs_sources), batch insert, retention
  state.rs      one block per receiver (sockets, queue, counters), the
                forwarders' block, retention counters, follows, the rules'
                memory
  receiver.rs   the supervisor: every receiver follows the settings
  syslog.rs     the syslog receiver: UDP/TCP sockets, bounded queue, writer
  flows/
    mod.rs      the flow collector: the engine's socket and bounded queue,
                the counting task, its status; flows.top, flows.exporters
    table.rs    what is remembered: per exporter, a bucket a minute, the
                biggest talkers six ways, `other`, and every ceiling
    label.rs    the service behind a port, the name of a marking (from
                nocturne-tools' reference tables)
  traps/
    mod.rs      the trap receiver: one UDP socket, decode → attribute →
                community → bounded queue → writer → acknowledge informs
    admit.rs    the community check against the vault, the scrub of
                communities out of bindings, the list of rejected sources
    render.rs   a notification → message, body and index columns
  ingest.rs     prepare (syslog message → row), commit (the half every
                receiver shares: policy, transaction, tail, rules),
                from_lab (`logs.ingest`), the retention task
  attribute.rs  address → device, and a line's interface → the device's own
  guard.rs      the redaction prefilter in front of the shared redactor
  query.rs      the shared filter, query / histogram / top / bursts / sources
  tail.rs       logs.appended batching and sampling; logs.follow
  rules/
    mod.rs      evaluate (threshold → throttle → signal), clearing, the
                sweep, the ordered queue of calls to the monitor,
                logs.rules.*
    builtin.rs  33 built-in rules: 29 by platform and 4 on the standard
                notifications, each with its sourced line
    track.rs    what a rule remembers per (device, instance), and its caps
  forward/
    mod.rs      forwarders: the documents, the secret in the vault, the tap
                (`offer`), the reader of the event and audit streams, start
                and stop, the status rows, logs.forward.*
    wire.rs     a stored entry as an RFC 5424 message, a JSON document, an
                event collector's envelope; the limits and what is cut
    deliver.rs  one forwarder's task: batch, send, retry, count; the HTTPS
                client and what an error of its may say
  correlate.rs  logs.around: the line next to what else was known
  pull.rs       logs.pull: `show logging` / `/log print` into the same store
  setup.rs      logs.device_setup: the syslog and trap lines that would
                point a device here (a table that belongs in the profiles)
tests/
  receiving.rs  real datagrams and connections, attribution, redaction
  searching.rs  filters, paging, histogram, top, bursts, retention, clear
  signals.rs    rules, throttling, the alert hand-off, the tail, logs.around
  thresholds.rs counting rules and their edges, the security built-ins,
                alerts that clear, and what a hostile sender cannot do
                (50,000 made-up keys, a capture as long as the line, a
                made-up user name, opposites for made-up neighbours, a
                monitor that has stopped answering)
  lab.rs        logs.ingest: refused for hardware however the request is
                spelled, a datagram's equal for a lab, a simulated
                port-security violation end to end
  devices.rs    pulling a buffer, de-duplication, device_setup per platform
  forwarding.rs a UDP socket, a TCP listener, a TLS listener and an HTTPS
                endpoint (certificates made in the test) as collectors:
                what arrives is the stored row, the audit trail and a lab's
                lines only when asked for, the secret searched for
                everywhere, a collector that stops reading (ingest timed
                against the same lines with no forwarder), one that is down
  flood.rs      load: simulator-generated lines over a real socket
  traps.rs      datagrams captured from net-snmp replayed into a real
                socket, and net-snmp's tools themselves where installed:
                the community (rejected, counted, found nowhere), informs,
                SNMPv3, interface names, the trap rules, a storm, a forged
                flood, the trap lines of device_setup
  flows.rs      export datagrams captured from softflowd and nfreplay
                replayed into a real socket (the captures are the engine's,
                `nocturne-netprobe/src/flow/tests/captures.rs`, included by
                path), softflowd itself where installed, and datagrams
                written by hand for what softflowd cannot send: top-N six
                ways, coverage, sampling, a set before its template,
                exporters known and unknown, interface names, ageing, a
                flood of made-up keys, a storm, binding
```

## Where a new receiver plugs in

`traps/` is the worked example of every step; `flows/` is a receiver that
does steps 1, 2, 3 and 5 and has no rows, so no step 4 or 6. The steps are
in the module documentation of `receiver.rs`; in short:

1. settings: a `#[serde(default)]` block in `settings::LogsSettings`;
2. state: a block beside `state::SyslogState` (socket slot,
   `ReceiverCounters`, bounded queue) and a field in `LogsState`;
3. supervisor: an `apply(&App)` in the receiver's module, called from
   `receiver::apply` — that line makes it follow the settings and
   `logs.restart`;
4. pipeline: build `db::Prepared` rows (attribute with `attribute::Index`,
   pass every stored string through `guard::clean`) and hand them to
   `ingest::commit` with the receiver's own counters. `commit` applies the
   unknown-source policy, writes the batch, offers it to the tail and to
   the forwarders (`forward::offer`, after `db::insert_batch`) and runs the
   rules — so a new receiver's rows are forwarded without doing anything,
   provided its stream is one a forwarder can name (`forward::STREAMS`);
5. status: a field of `model::LogsStatus`, filled in by `status::snapshot`;
6. rules for the new stream: one more function in `rules/builtin.rs`
   chained into `builtins()`, with `stream` set and its own platform label.
   Rules already match on `entry.stream`, read `body.raw` or the message,
   and take the instance from a capture named `interface` or from
   `threshold.group_by`.

`db::Prepared` and `logs_syslog` carry syslog-shaped columns (facility,
app, mnemonic, host, interface, fingerprint); every one is optional. A trap
row uses `mnemonic` for the notification's name and `interface` for the
interface, so the existing filters, `logs.top` and `logs.bursts` work on
it, and leaves the rest empty.

A receiver that owes its sender an answer (an inform's acknowledgement)
asks `ingest::keeps` whether `commit` kept a row, rather than having an
unknown-source rule of its own.

## The flow collector: decisions

* **Counts, not rows.** Nothing of a flow is written anywhere: not to the
  database, the tail, an event, a forwarder, a rule or a log line. That is
  what the specification asks ("no per-flow storage") and it is also what
  keeps the collector from being a second way for unauthenticated UDP to
  raise an alert. The cost: a restart forgets everything, so every answer
  carries its coverage (`collecting`, `since`, `records`, `exporters`).
* **A minute keeps its 20 biggest talkers per way of looking, not all of
  them.** An answer over fifteen minutes adds up fifteen top-20 lists, so
  a talker that is 21st in every minute is in no row although it may be
  3rd overall. That is the price of a day of history in bounded memory,
  and it is why `other` and `total` are in every answer: the screen can
  say "these rows are 82% of the traffic". What a row says is a floor,
  never an estimate (`table.rs`, module documentation).
* **The running minute lets the smaller half go at 512 keys** rather than
  refusing new keys. Refusing would mean a flood of made-up sources early
  in a minute hides a real talker that starts late in it; halving keeps
  whoever is big so far and lets newcomers in. No estimating algorithm
  (space-saving and its relatives) is used, because those report an upper
  bound as if it were a count.
* **`interface` is two keys per flow** (in through, out by). "Who is
  filling this uplink" is about traffic that *leaves* by it, and the input
  interface alone — what the engine's own `flow::top` uses — cannot say
  that. The price is that interface rows add up to twice `total`; the
  specification says so.
* **`application` is "the port a service is on"**, with a rule for which
  side that is (`label::service_port`). The engine's rule (the lower port)
  makes thousands of applications out of two client ports. The names come
  from `nocturne-tools`' reference table — an engine, so the dependency is
  allowed — and a label always carries the numbers it was derived from.
* **The engine's counters are read, not copied.** `received`, `malformed`,
  `records`, `sets_without_template` and `drops.queue_full` are the
  engine's own (`FlowCounters`), added to what earlier collectors of this
  process had counted when one is stopped. The feature counts what only it
  knows: `counted`, `drops.no_room`, `drops.unclaimed`.
* **Attribution is by index stamp.** An exporter is attributed when it is
  first heard and again whenever the attribution index is a newer one
  (`attribute_all` before every answer, and per datagram in `add`), so a
  device given the exporter's address owns what that address sent before —
  the buckets are per address and the device is looked up, not stored with
  the traffic.
* **Nothing runs on a timer.** Minutes are finished, aged out and trimmed
  when a datagram arrives in a new minute or when somebody asks
  (`logs.status`, `flows.top`, `flows.exporters`). An idle collector holds
  what it holds until then, which is bounded.
* **`unknown_sources: "drop"` reaches the collector at the next `apply`**
  (a settings change, `logs.restart`), through an atomic it reads per
  datagram, without rebinding.
* **A test must not hold the buckets' lock across an await.** The counting
  task then blocks a runtime thread on it, and on the multi-threaded test
  runtime that can be the thread that was driving timers and sockets: the
  test's own `sleep` never ends. The storm test therefore overruns the
  queue the way the trap receiver's does — on the single-threaded runtime,
  where the socket is read in bursts.

Not built: sFlow; per-flow search; a filter inside `flows.top` (the
talkers *of one interface*, or of one application); scaling by the
sampling rate even as an option; alerts or insights from flows; remembering
anything across a restart; an event when a new exporter appears (a screen
asks again); naming an interface from the exporter's own option data
(softflowd sends `interfaceName`; the decoder skips it).

## The trap receiver: decisions

* **The community dies in `traps::take`.** `nocturne_snmp::decode_trap`
  returns a structure that carries it (and serialises it). It is taken
  apart there, the community is compared (`admit::check`), cut out of the
  bindings (`admit::scrub`) and dropped; what goes on is a
  `traps::Notification`, which has no field for one. Nothing after that
  point could store or log a community if it tried. The decode error is
  not logged either: at that point it is the only thing that knows what
  was in the datagram.
* **A community also arrives in bindings.** A proxy that forwards a trap
  adds `snmpTrapCommunity.0` (RFC 3584), and the shared redactor does not
  know that name (`snmpTrapCommunity=public` passes through it untouched —
  checked). So `snmpTrapCommunity` and `snmpCommunityName` are emptied by
  definition, and the datagram's own community is replaced wherever a
  string binding repeats it. The placeholder is `<community>`, not the
  redactor's `<secret:n>`: the redactor keeps every value it replaces in a
  table (so a model's answer can be given it back), and a community must
  not be kept in a table either.
* **What the vault said is cached for 30 s**, inside
  `credentials::Resolved` (which wipes itself and prints only whether a
  field is set) with everything but the community dropped at once. The
  cache is emptied on `credentials.changed`, `devices.changed`,
  `vault.changed` and when the supervisor falls behind the event bus.
  Reading the vault per datagram would let a flood of traps from a known
  address turn into a flood of decryptions.
* **The check is before the queue.** A forged trap is turned away in the
  socket task and never takes a queue slot, so a flood of them cannot make
  the receiver drop real ones. The cost is that attribution and the cache
  lookup run in the socket task; both are a map lookup.
* **Rejected sources are remembered in memory, not in `logs_sources`.** The
  table is written per stored or policy-dropped row; a rejected datagram's
  source is whatever its sender made up, and one row per made-up address is
  an unbounded table. `traps.rejections` holds 64 addresses and shows 20.
* **An inform is acknowledged after `commit`.** The writer sends the
  answer, through a `Weak` to the socket so that it cannot keep a port open
  that the supervisor has closed. Because the answer can therefore be
  late, and because any answer can be lost, a repeat is recognised (sender
  address and port, FNV of the datagram: a sender repeats an inform byte
  for byte) and not stored again — otherwise one lost acknowledgement would
  count as two authentication failures towards a burst.
* **SNMPv3 without authentication** is decodable and proves nothing. It is
  treated as a community would be: kept only from a device whose stored
  credential is SNMPv3, that user and no authentication. A device stored
  with authentication never legitimately sends one without.
* **`require_known_community` is applied to the running socket** (an
  atomic the socket task reads). Only `enabled`, `port` and `bind` rebind,
  and `apply` runs one at a time and waits for the old socket task to end
  before binding, since the port may be the same.
* **A stopped receiver's writer drains its queue.** The trap writer ends
  when its queue is closed and empty, not when the receiver is replaced:
  what was taken off the socket before a restart is written, and the
  counters still add up. (The syslog writer stops at once and loses what
  was queued; that predates this and was left alone.)
* **`logs.restart` retries a trap receiver that could not bind.** The
  syslog receiver does not (`syslog::apply` returns early while an error is
  recorded and the settings are unchanged); that predates this and was left
  alone.
* **The link rule is keyed by ifIndex, not by name.** The name is only in
  the message when the monitor has polled the device, and a map that
  arrives between a down and an up must not make them two instances. The
  optional ` (name)` is captured by a group that can match nothing, so the
  title reads `Link down: ifIndex 3 (Gi0/3)` or `Link down: ifIndex 3`
  without a placeholder left in it.
* **Trap rows count towards `max_rows` and age out with `retention_days`.**
  A trap flood is a device-row flood like any other; the audit trail is
  still outside the cap.
* **`logs.device_setup` returns a placeholder for the community.** The
  lines cannot be handed to `changes.create` as they are, and the note says
  so. Returning the community would make this the one RPC that reads a
  secret back out of the vault.
* **The setup lines are a table (`setup::RECIPES`), keyed by syntax.** That
  is device-type knowledge in Rust, where it does not belong, and `syntax:
  ios` is also NX-OS, EOS and AOS-CX, whose commands are not these. The
  table has the shape of the profile section it should become. Nothing was
  added to it but the trap lines of the two platforms it already had.

## Forwarding: decisions

* **The tap is two taps.** Device lines and traps are offered where this
  crate writes them (`ingest::commit`, `pull::store`). The `event` and
  `audit` streams are written by every feature through `store.log_append`,
  where this crate sees nothing, so a task reads those rows back by id
  twice a second (`forward::records`). Both are "after it is stored"; the
  second is also why nothing outside this crate had to change. The cursor
  is unset while no forwarder names either stream, so an installation that
  forwards only syslog never runs the query.
* **A forwarder starts at the newest row** (`Running.after`). Without it a
  forwarder saved a moment ago was offered whatever the reader's next page
  held — including audit entries from before anyone asked for the audit
  trail to leave. A test caught exactly that.
* **`offer` is all the ingest path does**: for each entry and each
  forwarder that wants it, a `try_send` of an `Arc` of the row, or a
  counter. The row is cloned once, and only if some queue has room.
  Formatting, name lookups, connecting and waiting are the forwarder's own
  task. Measured below.
* **A new URL does not inherit the stored token.** The URL and token are
  one vault entry, write-only. If changing the URL kept the token, anyone
  who may save a forwarder could point it at a listener of their own and
  read the token off the wire. So `url` without `token` starts with none
  (the same URL sent again keeps it).
* **What an HTTPS error may say.** `reqwest`'s message contains the URL, so
  the reason is built from the error's kind — and from the rustls error in
  its source chain when there is one, because "could not be reached" for a
  certificate nobody signed sends people to the firewall. A refusal's body
  is not quoted either (an error page repeats the URL; a collector may
  repeat the token): only an integer `code` is read out of it.
* **The HTTP client's own log lines.** hyper-util writes the host and the
  socket address it connects to at debug and trace level, and reqwest the
  host at debug. The request future therefore runs under
  `tracing::subscriber::NoSubscriber`, and — because the lines about kept
  connections are written from tasks hyper-util spawns, where that does not
  reach — no connection is kept (`pool_max_idle_per_host(0)`, HTTP/1.1
  only). The cost is a TLS handshake per batch of up to 200 entries. The
  test searches the log at trace level; it found the lines before this.
* **RFC 5424 without an enterprise number.** A private structured-data
  element needs `name@<enterprise number>` and Nocturne has none
  registered; 32473 is the documentation number and not ours to ship. So
  the only element is the registered `origin` (`ip`, or `software` and
  `swVersion`), the device goes in `HOSTNAME` and the kind of message in
  `MSGID`. Everything else is what the JSON format is for.
* **`HOSTNAME` is the device an entry is about**, for all four streams, and
  `APP-NAME` says who wrote the text (`nocturne` for what it wrote itself).
  Strictly, Nocturne is the originator of an alert about SW1; but a person
  at a log platform looks for SW1, and the alert's text does not name it.
* **No severity is `user.notice`** in RFC 5424 (what RFC 3164 §4.3.3 has a
  relay write for a message without a priority) and `null` in JSON. A
  severity floor does not hold such an entry back — the rules' and the
  query's `min_severity` do the opposite, deliberately not followed here:
  a trap has no severity by nature, and a floor that silently took every
  trap out of a forwarder that names `trap` would be a trap of its own.
  The audit trail is exempt for the same kind of reason: `App::audit`
  writes every entry as informational, so any floor above that would send
  none of it, for a forwarder that asked for it by name.
* **A forwarder pointed at this application's own syslog receiver is
  refused** when it is saved (an address of this machine, a port the
  receiver has bound). Its lines would come back as new lines and go round
  for ever. Only the direct case is seen: not a loop through a relay, not a
  receiver switched on afterwards.
* **A stopped task says nothing more.** A forwarder saved again shares its
  counters with its successor (so that renaming does not zero them), and a
  task still in the middle of a send would otherwise report a failure
  against settings that are gone. Stopping interrupts the send and counts
  the batch as dropped.
* **Framing**: octet counting for RFC 5424 over TLS (RFC 5425 requires
  it); a message to a line everywhere else, because that is what a plain
  TCP input and anything reading JSON lines expects, and a number in front
  of every event is what such an input would show otherwise.
* **Lab entries stay unless `lab: true`**: a log platform would take a
  lab's port-security violation for a real one. Decided when the entry is offered, from the
  attribution index; the reader refreshes the index when a row names a
  device it does not know, since a lab's devices are created and start
  talking in the same breath.
* **The send buffer is the operating system's.** A collector that accepts
  and never reads is "sent to" until the buffers between fill (about 4 MB
  per connection here), and again after each reconnection. `sent` is
  documented as "handed to the network" for that reason. Capping
  `SO_SNDBUF` would make the number more honest at the price of throughput
  on a long link; not done.
* **Not persisted**: the queue and the counters. A restart loses what was
  waiting and starts the numbers at zero, like the receivers'.

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
  first flap immediately, not after a minute of aggregation. The key is
  (rule, device, instance): the interface, or a threshold's group key.
* **One rule speaks for at most 256 instances of a device per window.** The
  specification asks for per-instance throttling; on its own that turns a
  sender who invents a port name per line into a signal, a stored event and
  an alert per line (before, it was one a minute per rule and device). Past
  256 the matches are held back and counted (`rules.held_back`), and each
  instance speaks on its next match after the window.
* **What a pattern captured is cut before it is used.** An instance is at
  most 200 characters, a title 300, every captured value a signal carries
  2,000 (its `groups`, so also an alert's `detail` and the `by` of
  `logs.config_changed`), and control characters and direction overrides in
  them become spaces (`rules::plain`). A signal goes onto the event bus,
  whose ring is resident, and into the `event` stream; the line it is
  about is already in the store, whole.
  A capture can be most of a 64 KiB line, a rule holds ten thousand keys
  and each open alert keeps its key and title: without the cut the cap on
  keys bounded their number and not their size. A title also goes to
  notification channels, where an escape sequence is not text.
* **The login rules read the address from the end of the line.** The user
  name is typed by whoever is at the prompt; a pattern that stops at the
  first `]` or the first ` from ` lets a name such as
  `x] [Source: 192.0.2.9` choose the address the attempt is counted under,
  a different one each time, and the burst never adds up.
* **The calls to the monitor go through one bounded queue, in order**
  (`rules::Outbox`, 4,096 calls, one task). A task per call let a raise
  overtake the clear decided just before it — a neighbour back and lost
  again within a batch — and left the alert closed on a device that ended
  down. Past the bound a call is dropped and counted
  (`rules.undelivered`), like a line the writer cannot take.
* **An opposite for an alert nobody here knows is asked for at most 256
  instances of a device a window**, the same cap as for signals, and makes
  no record when it is not passed on. Before, every made-up "neighbour up"
  was a call to the monitor.
* **A rule that stops watching forgets.** Switched off, or saved with
  anything changed but its name and throttle, a rule drops its counts and
  the alerts it had open (as `delete` does). Otherwise the sweep would
  report "quiet for 30 minutes" for a rule that had merely stopped looking.
* **Only an alert its rule can clear is waited on.** One with neither an
  opposite nor a quiet period is the monitor's from the moment it is
  raised; it is not in `rules.open_alerts`, and its record idles out like
  any other. Counting it as open for ever would be a number that only
  grows.
* **`logs.ingest` wants a lab *and* the simulator.** `is_simulated` is
  `lab` set and `transport: sim`. Whether an alert about such a device
  reaches a channel is not decided here: the rules pass no `notify`, and
  the monitor holds a lab's alert back unless `notify_for_lab` is on — one
  rule for every source of alerts (it was `notify: []` from here once,
  which made a log rule's lab alert silent whatever that setting said).
  An empty `device_id` is refused before the inventory is
  asked, because `inventory::find` also matches by address and a simulated
  device has none.
* **A threshold fires and starts again.** `count` lines inside `window_s`
  raise the signal and empty the count, so a burst that goes on fires every
  `count` lines and the throttle turns that into one signal a window with
  the number of lines. A sliding "still above the threshold" would need a
  timer per key to say when it stopped.
* **`threshold {count: 1}` is how a rule gets one instance per neighbour.**
  The specification gives a rule two sources of instance — the interface
  and the group key — and a BGP neighbour is neither an interface nor
  something to count to eight. Rather than a third field, the neighbour
  rules group by `neighbor` with a count of one.
* **Two clocks.** What is decided when a line arrives (throttle, threshold,
  whether an opposite is inside the window) is arithmetic on line times;
  what is decided when no line arrives (a quiet period, the end of a hold)
  is the wall clock, in `rules::sweep`. In the application they are the same
  clock. Keeping them apart is what lets tests store lines at times of their
  choosing without the background sweep clearing things under them, and
  move the wall clock by calling `sweep` with a later time.
* **An alert is held for its throttle window.** An opposite inside the
  window is remembered and applied when the window ends, unless the rule
  matched again. Clearing at once and letting the throttle swallow the next
  `down` would leave an alert closed on a neighbour that ended up down.
* **`after` is the open alert of another rule**, not a time window of its
  own: "a login that succeeds straight after a burst" is a success from an
  address whose burst alert is still open, and the burst's `clear_after_s`
  is what "straight after" means.
* **What is open is not persisted.** After a restart an opposite still
  clears (the monitor is asked blind, once a window per instance); a quiet
  period does not, because nobody knows when the last match was. Adopting
  open `logs:` alerts from `alerts.list` at start would close that gap and
  is not built.
* **Retention ages the audit trail too**, and `logs.clear` refuses it. The
  row cap is for what devices sent (`syslog` and `trap` together). The
  `event` stream is not aged.
* **A lab line has no source address.** `logs.ingest` leaves `source`
  empty rather than writing one the device never sent from, so nothing is
  added to `logs.sources`; `body.via` says `lab`. It is audited once per
  device per run — a lab hands lines over many times a second.
* **Rules do not run over pulled lines.** A buffer is history, and
  `logs.config_changed` means "back this device up now".
* **Built-in rules can be switched off, not deleted or edited** (a delete
  would come back at the next start; `logs.rules.save` on a built-in keeps
  only `enabled`, `scope` and `throttle_s`). Those three are kept across a
  restart: `ensure_builtins` used to keep `enabled` alone and rewrite every
  rule at every start, so a scope or a throttle somebody had set lasted
  until the application was next started.
* **An alert rule needs a title** when it is saved. The monitor refuses an
  alert without one, and it would do so where nobody sees it: when a line
  matches.
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
| 16,000 lines of about 1 KB stored in two applications, batch by batch in turn: one with no forwarder, one with a TCP forwarder whose collector accepted the connection and never read from it (machine under load, two other builds running) | 36.4 s with no forwarder, 35.0 s with the stalled one; of the 16,000 offered to it, 1,311 were written before it stalled, 4,489 were dropped and counted, 10,000 were waiting (the queue's limit) and 200 were in the task's hand |

A release build does the same work about ten times faster: 20,000 lines
through `prepare` + insert + rules took 290 ms (≈69,000 lines/s), split
roughly evenly between parsing/attribution/redaction, the three inserts per
row, and the 21 rule patterns.

## Second review of the receivers: what was found and changed

Looked for with a throw-away test that sent traps with one community and
a wrong one, saved forwarders with a token in their URL against a dead
port and against a listener whose certificate does not check out, called
every `logs.*` and `flows.*` method, and then searched every table, every
event, every result and error and every log line (a global subscriber at
trace level, on a runtime with several threads, so that lines written from
other tasks and threads were seen too) for those strings. Nothing was
found. What was found was elsewhere:

* **One datagram could cost seconds.** The flow decoder did four things
  once per record that looked at more than the record: it searched every
  template for the oldest when the table was full, read the rest of a
  template set to see whether it was padding, looked through every
  template for each IPFIX "withdraw all", and walked a thousand zero-length
  fields for each one-byte record. A datagram written for it took 0.5 to
  3 s in a debug build; each now takes what its size takes (engine tests
  `a_datagram_of_thousands_of_templates_costs_what_its_size_is` and its
  neighbours, which fail on the old code by a factor of ten or more).
  The trap row had the same shape of problem: a body left a few bytes
  short was searched to the end of the datagram for a binding that might
  still fit, naming and redacting each (8 s for one datagram). The body is
  now closed by the first binding that does not fit.
* **A queued trap is what a row can show.** `traps::take` keeps the first
  128 bindings and the count. Six thousand null bindings are 54 KB on the
  wire and about ten times that decoded, for each of `queue_limit` places.
* **`flows.exporters` claimed templates a rebound collector did not
  have.** The templates are the decoder's and the decoder is the socket's.
  `templates` is now zero from the moment the collector stops until the
  exporter sends them again, and a counting task that is still draining
  the old queue cannot put the old number back (`FlowState::stopped`).
* **A refused certificate read as "could not be reached"** for
  `https_json`. The HTTP client hands the TLS error on inside two I/O
  errors, and `std::io::Error::source()` is the source of what it wraps,
  not what it wraps; `deliver::describe` unwraps by hand now.
* **`ca_pem` with a private key in it was accepted and returned.** A
  certificate parser reads past a key block without a word, and the text
  is kept in the forwarder's document. Anything but certificate blocks is
  refused.
* **A trap the database refused was in no count.** It is `dropped`.

Left as they were at the end of that review, and closed since (next
section): queues bounded in entries only; the sampling rates and templates
a rebind forgot; the syslog writer's batch the database refused.

## Hardening: what the second review left open

* **Every queue has a byte budget beside its entry limit** (`budget.rs`):
  the syslog and trap receivers `queue_limit_bytes` (a setting, 64 MiB,
  1 MiB to 1 GiB, applied to a running receiver), the flow collector the
  same setting read when it starts (the engine's `QueueLimits`), each
  forwarder 16 MiB (`forward::QUEUE_BYTES`). What does not fit is dropped
  under the counter a full queue already used. A queued entry carries a
  `Held` that gives its bytes back when it is dropped — written, sent,
  refused, or thrown away with its queue — so a writer's batch in hand
  counts, and no path can forget to give them back. What an entry weighs
  is its size plus the capacities of what it owns; a JSON object's entries
  are counted at half as much again as key and value, for the index and
  the hash. The flow queue's bytes are the engine's and leave the queue
  when the counting task takes a batch: one datagram (up to ~11 MiB of
  one-byte records) is in its hands and in no number.
* **Measured** (tests/memory.rs, debug build, each flood in a process of
  its own, VmRSS): with a 2 MiB budget the process grew by 11–12 MiB
  (600 syslog datagrams of 65,507 bytes), 11–12 MiB (1,000 traps of that
  size), 40–53 MiB (20 flow datagrams of 65,480 one-byte records — two
  datagrams held, one being decoded, and what each thread's allocator
  arena keeps of the decoding) and under 1 MiB (1,000 rows of 128 KiB
  offered to a forwarder whose collector is gone: the queue's 16 MiB fit
  in memory the allocator had kept from start-up). With the budget at
  1 GiB — that is, bounded by entries as before — the same floods grew it
  by 85, 71 and 211 MiB. A flood is so many datagrams; nothing is timed.
  The flow flood runs on two runtime threads: on four, the arenas of the
  threads the collector happened to decode on made the figure wander
  between 50 and 69 MiB.
* **A lab's alert is the monitor's to silence.** No `notify` is passed;
  `notify_for_lab` decides, for log rules as for everything else.
* **The syslog writer counts what it loses** — a batch the database
  refused, or the attribution index could not be read for — as `dropped`
  (`ingest::commit` returns `Unwritten {error, rows}`; the trap writer
  uses the same count now instead of working it out again), and it no
  longer stops when the receiver is restarted: it drains what was taken
  off the sockets, as the trap writer does. The relay counts what it
  could not queue at shutdown.
* **An https_json forwarder uses no proxy** (`no_proxy()` on its client),
  and **refuses a credential over `http://`**: a token, or a user name or
  password in the URL, at save (`invalid`) and again before sending, for
  a forwarder stored before. Decided: refused, not acknowledged — a token
  in the clear is one anybody on the path holds, and a collector that
  wants one can have https:// with `ca_pem`.
* **One flow decoder for the life of the application** (`SharedDecoder`
  in `FlowState`), handed to every collector: templates and announced
  sampling rates survive a rebind and a switch-off. The collector's
  counters are what it decoded (`DecoderStats::since`), so `past` plus the
  running one still adds up. The earlier repair that zeroed `templates`
  on a rebind is gone with the reason for it.
* **Tests that hold a writer** hold a lock from a thread of their own
  (`support::hold_the_rules`, `hold_the_index`), let go after 30 s at the
  latest, and wait without the runtime's timers (`wait_held`), nudging
  the sleeping runtime threads: a held thread may be the one that was
  minding the sockets and the clock, and with it blocked a
  `tokio::time::sleep` never ended — the switch-off test hung that way.

## The review of that hardening

* **A datagram larger than the flow queue no longer shuts it to everyone
  else.** One such datagram (65,480 one-byte records, about 11 MiB decoded,
  more than the default 50,000 records) is let onto an empty queue as
  before; what arrives while it waits is now measured against the limits
  without it (the engine's `Alone` note, read and cleared together with the
  counts). Before, every other exporter's datagram was refused for as long
  as one waited, which under a flood of them is all the time: beside such a
  flood a quiet exporter sending one v5 record ten times a second had 1 of
  123 datagrams counted and 86 refused as `queue_full` (default limits).
  Now none is refused at the queue; 88 of 123 were counted, the rest lost
  in the kernel's socket buffer while a debug build decoded the flood. The
  queue holds at most the limits and that one datagram.
* **Measured by the review** (debug build, each flood in a process of its
  own, `unknown_sources: drop` for the flood so the in-memory database
  does not grow with it, 8 MiB `queue_limit_bytes`): 6 s of 64 KiB syslog
  datagrams at 500/s grew the process by 28 MiB (queue at most 7.9 MiB);
  the same with the budget at 1 GiB grew it by 358 MiB (2,275 lines,
  341 MiB queued). Received was stored plus every kind of dropped, to the
  datagram, for syslog, traps, flows and two stalled forwarders (sent plus
  dropped equal to what was offered), with large and with tiny datagrams.
  With the writer held, 400 of 400 large datagrams were taken off each
  socket. The process grows by about the budget, the batch's working copies
  (redacted text, the row's body) and some 20 MiB besides.
* **The flow flood of `tests/memory.rs` waits for the first datagram to
  leave the queue** before it sends the second. The counting task, woken
  behind the socket task, may not get a turn while the socket has datagrams
  to decode; the first then waits in the queue the whole flood, everything
  after it is refused, and one datagram is counted where the test expects
  two (2 runs of 5 here).
* **Left as it is: a full queue drops from its tail, whoever sent.** Beside
  a flood of tiny datagrams (20,000 a second from one address), a quiet
  source's lines over TCP were stored 57% (syslog), 91% (traps) and 50%
  (syslog with a stalled forwarder) of the time; beside a flood of 64 KiB
  datagrams, all of them (a small line fits where a large one does not). It
  was so with the entry limit before the byte budget. A share of the queue
  per source would keep one noisy device from crowding out the others, and
  would not help against sources that are made up.

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
* **The flow engine keeps `FlowRecord` addresses as text**, so `table.rs`
  parses two strings per record back into addresses. A record with typed
  addresses would save that; it is the engine's public shape and was left
  alone.
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
* **`nocturne_ai::Redactor`'s configuration-command rules used to be
  anchored to the start of a line** (they were written for configuration
  files), and a log line *quotes* a command in the middle of itself — IOS
  `%PARSER-5-CFGLOG_LOGGEDCMD`, FortiGate `msg="… set psksecret ENC …"`.
  This crate once carried unanchored copies; the fix went upstream and
  `guard.rs` now uses the shared redactor with nothing added.
* **The syslog parser does not read two shapes the documentation prints.**
  A line stamped with an uptime and no priority (`00:22:51: %PORT_SECURITY-2-…`,
  `service timestamps log uptime`) and a RouterOS line as `/log print`
  shows it (`dec/10/2007 10:40:06 system,error,critical …`) both come back
  with no mnemonic and no severity. The rules still match (they read the
  raw line), but such a line is stored as "severity unknown" — and
  `logs.pull` stores RouterOS rows in exactly the second shape: run against
  a `/log print` row, `pull::store` kept it with no severity, no mnemonic
  and `ts_source: received` (the device's own date unused). Not fixed here;
  the fix belongs in `nocturne_netprobe::syslog::parse` or in
  `pull::buffer_lines`.
* **`nocturne_app::testing::lab` creates devices with no address**, which is
  exactly what a log receiver needs one for; every test here sets it with
  `devices.update` first. A `lab_with_addresses` helper, or an address per
  fake device, would save that step. `FakeDevice` also has no log buffer of
  its own, so `show logging` is a canned output in `tests/devices.rs`.
