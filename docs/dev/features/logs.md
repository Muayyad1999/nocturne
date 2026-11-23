# Feature: logs and traps (`nocturne-logs`)

What the network says about itself: a syslog receiver, an SNMP trap
receiver and a flow collector, stored, searchable, tailed live, and turned
into signals the rest of the product reacts to (an interface flapping, a
login failure, "someone changed the configuration").

Engines: `nocturne-netprobe` (`syslog` server/parser, RFC 5424 formatter
and sender, `flow` collector —
read `docs/dev/spec-netprobe.md`), `nocturne-snmp` (`decode_trap`, `names`),
core `store.log_*`.

## Receivers

Settings section `logs`:

```rust
{ syslog: { enabled: false, udp_port: 5514, tcp_port: 5514, bind: "0.0.0.0" },
  traps:  { enabled: false, port: 1162, bind: "0.0.0.0", require_known_community: true },
  flows:  { enabled: false, port: 2055, bind: "0.0.0.0" },
  retention_days: 30, max_rows: 2_000_000, unknown_sources: "store" /* or "drop" */,
  queue_limit: 50_000 /* what may wait to be written, per receiver; for flows: flow records waiting to be counted */ }
```

Off by default (opening a listening port is the user's decision). The
standard ports 514/162 need privileges on Unix: the defaults are unprivileged
and the UI shows the device-side command for the chosen port; if binding
fails the receiver reports *why* (`logs.status`) instead of dying silently.
Receivers restart when their settings change.

* Each datagram/line is parsed (RFC 3164, RFC 5424, and the Cisco
  `%FACILITY-SEV-MNEMONIC:` and RouterOS `topic,topic message` shapes inside
  them), attributed to a device by source address (match `device.address`,
  then collected interface IPs via `collect::latest`), and appended to
  the `syslog` stream (`store.log_append`) with `body` = `{facility, app,
  mnemonic?, host, raw}`. Unattributed sources are kept with `device_id:
  null` (or dropped, per setting) and listed in `logs.sources` so the user
  can add them.
* Traps: one UDP socket. `decode_trap`, resolve OIDs to names, attribute by
  source address like a syslog line, stream `trap`, `severity: null` (a
  notification has no syslog severity and is given none), `source` = the
  address the datagram came from. A v1 trap's `agent_addr` is kept in the
  body as what the agent claims; it never decides the device.
  * **The message** is the notification's name and its bindings by name:
    `linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)`. A
    notification the dictionary has no name for is shown by its OID, and so
    is a binding. A binding's instance is left out where it says nothing
    (`.0` of a scalar, or the one interface the notification is about) and
    kept where it does (`bgpPeerState.10.0.0.2`). `ifAdminStatus` and
    `ifOperStatus` read `up(1)`, `down(2)` … as RFC 2863 numbers them; a
    value with a space in it is quoted. At most 12 bindings, 160
    characters of each name (a name nobody has is its OID, and that is as
    long as its sender liked) and 200 of each value; the rest are counted
    (`(+N more)`) and are in the body.
  * **The body** is built field by field — never the decoded trap
    serialised, which carries the community: `{version: "v1" | "v2c" |
    "v3", trap: name | null, oid: dotted | null, uptime?: seconds,
    inform?: true, enterprise?, generic?, specific?, agent_addr? (v1 only),
    varbinds: [{oid, name: text | null, value: text, interface?}],
    varbinds_omitted?, if_index?, interface?}`. `name` is null when the
    dictionary does not know the object. At most 64 bindings, 1,000
    characters of each value and 16 KiB of bindings in all (OIDs, names
    and values); what did not fit is counted in `varbinds_omitted`. The
    body takes bindings in the order they were sent and is closed by the
    first that does not fit — it is not searched further for a smaller
    one, which would be naming and redacting thousands of bindings for a
    row that shows sixty-four. Of a datagram only the first 128 bindings
    are kept where it is decoded (a datagram can be six thousand, each
    several times its size on the wire once decoded, for every place in
    the queue); all of them are counted, in `(+N more)` and in
    `varbinds_omitted`. The index columns a syslog line has
    are filled where they mean something: `mnemonic` is the notification's
    name (so `logs.top` and a text search work on traps), `interface` the
    interface.
  * **A community is a secret.** It is never stored, published, logged or
    returned — not in the body, not in settings, not in an error, an audit
    entry or a rejection's reason. With `require_known_community` (the
    default) a v1/v2c trap is kept only when its community equals the SNMP
    community of the device its source address belongs to — the credential
    the monitor polls it with — read from the vault through the core's
    credentials; anything else is counted under `traps.rejected` and
    dropped, before it takes a place in the queue, so that a flood of
    forged traps cannot crowd real ones out. What the vault said is kept
    for at most 30 s and let go when a credential, a device or the vault
    changes. Switching it off accepts any community from a known device; an
    unknown source follows `unknown_sources`. The community is compared
    where the datagram is decoded and dropped there. It also arrives in
    bindings: a proxy that forwards a trap adds `snmpTrapCommunity.0` (RFC
    3584), so that object and `snmpCommunityName` are stored as
    `<community>` whatever they hold, and the datagram's own community is
    replaced by `<community>` in every string binding that repeats it (as a
    whole value at any length, inside a longer one from four characters).
    There is no list of accepted communities in the settings, which
    `settings.get` returns.
  * **Why a source was rejected** is in the status, because a count alone
    cannot be acted on: `traps.rejections`, the most recent 20 of at most 64
    addresses held in memory, each `{address, device_id?, reason, count,
    last_at}` — "the community is not the one stored for `SW1`", "the SNMP
    credential of `SW1` holds no SNMPv1/v2c community", "no device in the
    inventory has the address …". A rejected source is not written to
    `logs.sources`: anybody can make one up.
  * v1 and v2c traps are unauthenticated UDP: whoever can reach the port can
    forge one, and the community travels in the clear. The interface says
    so where the receiver is switched on, and nothing derived from a trap
    ever changes a device.
  * **Informs.** An SNMPv2c inform is acknowledged — after it is stored,
    never before, and not at all when it was not kept (rejected, dropped
    with the queue full, or from an unknown source under `unknown_sources:
    "drop"`), so that a sender which keeps an inform until it is answered
    loses nothing here. The same inform arriving again (same sender, same
    bytes, within five minutes — its answer was lost or late) is answered
    again and stored once; it is counted under `traps.duplicates`. At most
    4,096 informs are remembered; past that a repeat is stored again.
  * SNMPv3 notifications that need authentication or privacy are counted
    under `traps.unauthenticated` and **not** stored as if valid; verifying
    them is not built yet. One without authentication carries a user name
    and nothing else: with `require_known_community` it is kept only from a
    device whose stored SNMP credential is SNMPv3, that user, and no
    authentication, and rejected otherwise. An SNMPv3 inform is not
    acknowledged (that takes an authoritative engine); a sender's engine
    discovery request is not a notification and is counted under
    `traps.malformed` with everything else that is not one.
  * `ifIndex` bindings are shown with the interface name when the monitor
    has an index map for the device (`monitor.ifindex {device_id}` →
    `{interfaces: [{index, name}]}`, asked for at most once a minute per
    device, and only for a notification that names an interface):
    `ifIndex=3 (GigabitEthernet0/3)`, and the body's `interface`. Without a
    map — the monitor is not in the build, or has not polled the device, or
    does not know that index — it is `ifIndex=3` and no name is made up.
    The name is the device's own collected spelling when it has that
    interface collected.
  * Every datagram taken off the socket (`received`) ends in exactly one of
    `stored`, `dropped`, `rejected`, `unauthenticated`, `malformed` and
    `duplicates` — also across a restart: a receiver that is switched off
    or rebound still writes what it had already received. A trap the
    database would not take is `dropped` (and an inform among them is not
    acknowledged).
  * Only `enabled`, `port` and `bind` rebind the socket.
    `require_known_community` is applied to the running receiver.
* Batched inserts (≤ 250 ms or 500 rows), bounded queue: under a storm the
  receiver drops and **counts** (shown in `logs.status`) rather than growing
  without bound or blocking the socket. Each receiver has a queue of its
  own, of `queue_limit` entries.
* Retention task: age and row cap, in small batches. The age limit
  (`retention_days`, 0 = keep) applies to the `syslog` and `trap` streams
  and to the `audit` trail — which nothing else may delete, see
  `logs.clear` — and the row cap to what devices sent, `syslog` and `trap`
  counted together and the oldest of either going first, so that a flood of
  device lines or of traps cannot push the audit trail out. `store.rows` in
  the status is that count. The `event` stream is not aged; it can be
  cleared.
* Flows: NetFlow v5, v9 and IPFIX (no sFlow), one UDP socket, decoded by
  the engine. Aggregated in memory per exporter into 1-minute buckets of
  the top talkers, kept 24 h. No per-flow storage — no flow is written to
  the database, the tail, an event, a forwarder, a rule or a log line — so
  everything is lost on a restart and every answer states what it covers.
  * **What is counted.** Each flow record is counted under six ways of
    looking at it: its `source`, its `destination`, its `conversation`
    (both directions of an address pair are one key), its `application`,
    its `interface`s and its `dscp`. `application` is the IP protocol and,
    for TCP, UDP and SCTP, the port the service is on: a port the
    reference table (`nocturne-tools`) has a name for, whichever side it is
    on; failing that the lower port when it is below 32768, where clients
    are given theirs from; failing that none (`tcp, no service port`), so
    that two client ports are not thousands of applications. `interface` is
    two keys per flow — the ifIndex it came in through and the one it left
    by — so the interface rows add up to twice `total`; ifIndex 0 is "the
    exporter did not say". `dscp` is the upper six bits of the ToS /
    Traffic Class byte as the exporter saw it.
  * **Time.** Buckets are keyed by the time of receipt (a v5 or v9
    timestamp is relative to the exporter's boot). A flow's bytes are
    therefore in the minute its record arrived, which is when the
    exporter's cache let it go — up to its active timeout after the
    traffic. A clock that steps back does not make a bucket in the past:
    what arrives is counted in the newest minute there is, and a minute is
    kept once.
  * **Sampling.** Byte and packet counts are what the exporter sent: a
    sampling rate is reported beside them (`coverage.sampling`,
    `sampled_records`), never multiplied in. The rate is read from the v5
    header, from v9 options (`samplingInterval`, `samplerRandomInterval`)
    and from IPFIX options the PSAMP way (`samplingPacketInterval` and
    `samplingPacketSpace`, or `samplingSize` and `samplingPopulation`).
  * **Memory is bounded whatever arrives** — a flow export is a UDP
    datagram anybody who can reach the port can write. The minute that is
    running holds at most 512 keys per way of looking; one key more and the
    smaller half is added up into `other` and let go. A minute that is
    over keeps its 20 biggest and adds the rest to `other`. All finished
    minutes of all exporters hold at most 2,000,000 keys (about 130 MB at
    the very worst); past that the oldest minute of any exporter goes
    first, the memory reaches back less than a day, and the status
    (`memory.evicted_minutes`, `memory.oldest_at`) and every answer
    (`coverage.since`) say so. What was folded is never guessed at
    afterwards: a key that was let go and turns up again starts from
    nothing, so a row is a floor — never more than was seen — `other` is
    what is under no key, and `total` is exact. At most 256 exporters are
    followed; a full table gives up the unclaimed exporter that has been
    silent longest to a newcomer a device claims, and never to another
    stranger (what a refused one sends is counted, `drops.no_room`). The
    engine's queue between the socket and the buckets holds at most
    `queue_limit` flow records; what does not fit is decoded (templates
    are still learned), dropped and counted (`drops.queue_full`). The
    decoder keeps at most 4,096 templates and 8,192 announced sampling
    rates.
  * **What a datagram costs to read is in proportion to its size.** One
    datagram holds sixteen thousand four-byte template records, or sixty
    thousand one-byte data records, so nothing is done once per record
    that looks at the whole set or at every template there is: the oldest
    template is found without looking at the others, where a set's padding
    starts is found once, an IPFIX "withdraw all" looks through the
    templates once per datagram unless one was learned in between, and a
    template field of length zero — which holds no value — is left out, so
    that a record is never more fields than it is bytes.
  * **Exporters** are attributed like syslog sources, by the address the
    datagram came from, and attributed again when the inventory changes. An
    unknown exporter is listed (`flows.exporters`, with the reason), not
    dropped without trace: with `unknown_sources: "store"` its traffic is
    counted and can be asked for by its address; with `"drop"` it stays
    listed with what was not kept (`dropped`). A datagram that is not a
    flow export is counted (`malformed`) and makes nobody an exporter.
    Exporters are not written to `logs.sources`: nothing of a flow is
    stored.
  * **A set before its template** (v9, IPFIX: after an exporter restarts,
    or when the template datagram was lost) cannot be read. It is counted —
    in the status, per exporter, and in the coverage of every answer whose
    window it arrived in — and nothing is made of its bytes.
  * **What a flow is not.** Flow export is unauthenticated UDP: whoever can
    reach the port can report traffic that never was, from any address. So
    nothing derived from a flow ever changes a device or raises an alert.
    A conversation that crosses two exporters is reported by both, and an
    answer over several exporters adds both up (`coverage.exporters` says
    how many it is over).
  * Only `enabled`, `port` and `bind` rebind the socket; `unknown_sources`
    is applied to the running collector. What has been counted stays when
    the collector is rebound or switched off: the buckets belong to the
    application, not to the socket, and `coverage.collecting` says whether
    anything new is coming in.
* Lab devices: a simulated device has no socket to send from, so
  `nocturne-lab` hands its syslog lines to `logs.ingest`. A lab device is
  one that belongs to a lab **and** is reached through the simulator
  (`lab` set and `transport: sim`); a record that names a lab and is still
  reached over SSH is hardware, for `logs.ingest` and for who an alert about
  it notifies. The lines then take the
  same path as a datagram — index row, rules, signals, tail, redaction — and
  are marked `via: "lab"`. The entry's `source` is empty (no address is made
  up for a device that sent from none, and none appears in `logs.sources`),
  and the status counts them under `syslog.from_lab`, apart from `received`,
  which is what came off a socket. Lab devices send no traps and no flows;
  the status says so (`traps.lab`, `flows.lab`) rather than showing an idle
  receiver.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `logs.query` | `{stream?: syslog|trap|audit|event, device_id?, max_severity?, text?, since?, until?, before_id?, limit?: 200}` → `{entries: [LogEntry], has_more}` newest first (`before_id` pages back) |
| `logs.histogram` | `{stream?, device_id?, text?, since, until, buckets?: 60}` → `[{t, count, by_severity: [8]}]` |
| `logs.top` | `{since, until, by: device|mnemonic|severity, limit?}` → `[{key, count}]` |
| `logs.status` | → `{syslog: {enabled, listening: [..], error?, received, stored, dropped, queued, rate_per_minute, last_at, from_lab}, traps: {enabled, implemented: true, listening: [..], error?, received, stored, dropped, queued, rate_per_minute, last_at, rejected, unauthenticated, malformed, duplicates, acknowledged, require_known_community, rejections: [{address, device_id?, reason, count, last_at}], lab}, flows: {enabled, implemented: true, listening: [..], error?, received, malformed, records, counted, dropped, drops: {queue_full, no_room, unclaimed, unreadable}, queued, rate_per_minute, last_at, sets_without_template, exporters, unattributed_exporters, memory: {minutes, keys, max_keys, evicted_minutes, oldest_at?}, lab}, rules: {tracked, forgotten, max_per_rule, held_back, open_alerts, undelivered}, forwarders: [{id, name, state, sent, dropped, last_error?, last_at?}], store: {rows, oldest_at?, retention_days, max_rows, last_prune_at?, pruned_by_age, pruned_by_rows}, unattributed_sources}` — `rules` is what the pattern rules hold in memory (below). `forwarders` is every forwarder by name, with what it has done since the application started: `state` is `disabled`, `idle` (on, nothing sent or tried yet), `ok` (the last send worked) or `failing` (it did not, and `last_error` says how); `last_at` is when something was last sent (below). `traps.lab` is a sentence: a simulated device sends no traps; `flows.lab` likewise. `flows` counts in two units: `received` and `malformed` are datagrams (`rate_per_minute`: datagrams from followed exporters in the last minute), everything else is flow records. Every record decoded (`records`) ends in exactly one of `counted` (it is in the buckets) and `dropped` (the sum of `drops`: the queue was full; there was no room to follow one more exporter; no device claims the exporter and `unknown_sources` is `drop`; the inventory could not be read), or is still `queued`. `sets_without_template` is data sets that arrived before their template: what they held is in no number. `memory` is what the buckets hold: one-minute buckets over all exporters, the keys in them, the ceiling on keys, buckets let go early to stay under it, and the start of the oldest bucket |
| `logs.restart` | → the status, after re-applying the receiver settings. A trap receiver or a flow collector that could not bind is tried again (whatever held the port may have gone); one that is running is left alone |
| `logs.bursts` | `{..filter, min_count?: 5, limit?: 50}` → `[Burst]` — the same message from the same device many times, folded into one row with its count, first and last time |
| `logs.around` | `{entry_id? \| at, device_id?, window_s?: 300, limit?: 100}` → what else happened around one moment: log lines, alerts and changes on that device, in time order |
| `logs.follow` / `logs.unfollow` | `{filter, follow_id?}` → `{id}` / `{id}` → `{closed}` — a live tail with a filter applied on the server |
| `logs.pull` | `{device_id}` → reads the device's own log buffer over the CLI (read-only) and stores the lines it has not seen |
| `logs.ingest` | `{device_id, lines: [String]}` → `{stored}` — for `nocturne-lab` only: refused (`forbidden`) unless the device is a lab device, so it is never a way to write log lines for hardware. `device_id` may be a device's id or name (or address: it is looked up as everywhere else, and it is the device that is found that is checked and written for — nothing else in the parameters can make a device pass for a lab's; an empty reference is `invalid`). At most 10,000 lines a call (`invalid` beyond, nothing stored); empty lines are skipped; a line may carry a `<PRI>` or not. Audited once per device per run of the application, not per call, and never with the lines |
| `logs.sources` | → `[{address, device_id?, last_at, count}]` |
| `logs.rules.list` / `.save` / `.delete` / `.test` | pattern rules, below. `.list` → `[LogRule]`; `.save` `LogRule` → `LogRule` (`invalid` with the reason when the rule cannot work: a `group_by` the pattern does not capture, a clearing pattern that does not capture it either, `after` naming no rule, an alert with no title); `.delete` `{id}` → `{deleted}`; `.test` `{rule, line}` → `{matched, reason, groups, signal?: {kind, title, severity?, interface?}, parsed, instance, note, clears}` |
| `logs.clear` | `{stream?: syslog, before?}` → `{removed, stream}` — refused (`forbidden`) for `audit`: the audit trail is evidence, and the API that is audited does not get to erase it. It ages out by retention only |
| `flows.top` | `{device_id?, exporter?, minutes?: 15, by: source\|destination\|conversation\|application\|interface\|dscp, limit?: 20}` → `{rows: [{key, label, bytes, packets, flows, interface?: {exporter, device_id?, if_index, name?, direction: in\|out}}], total: {bytes, packets, flows}, other: {bytes, packets, flows}, coverage: {collecting, window_start, until, since?, records, sets_without_template, sampling: [u32], sampled_records, exporters}}` — the biggest talkers by bytes over the last `minutes` whole minutes and the running one (1 to 1,440, `invalid` beyond: a day is what is kept), added up over every exporter, or over those of one device (`device_id`: id, name or address; `not_found` when there is no such device) or one exporter by its address (`exporter`, which is how an exporter no device claims is looked at). `by` is required (`invalid` with the list when it is missing or something else); `limit` is brought into 1 to 500. **Rows.** `key` is stable text — an address, `a <-> b`, `tcp/443`, `tcp` (no service port), `ip/1`, a DSCP number, `<exporter> <ifIndex> <in\|out>` — and `label` is the same for a person: `tcp/443 (https)`, `ICMP (IP protocol 1)`, `EF (46)`, `GigabitEthernet0/3 in`. A label is derived from numbers and carries them: flow export says port 443, never HTTPS. An interface is named through `monitor.ifindex` when that method is there and knows the index (in the device's own collected spelling when it has that interface), `ifIndex N` otherwise and `no interface reported` for index 0; over more than one exporter the label says whose (`SW1 ifIndex 3 in`), and the row's `interface` carries the parts. `bytes` and `packets` are as exported and are a floor (see Flows above). **`total`** is everything the exporters reported in the window, exact; **`other`** is the part of it under no key. **Coverage.** `collecting`: whether the collector is listening now — when it is not, an empty answer means "not collecting", not "no traffic". `window_start` and `until`: what was asked about. `since`: the start of the first minute in the window that holds anything, `null` when none does. `records`: flow records added up. `sets_without_template`: data sets that arrived in the window before their template — traffic the exporters reported that is in no row and not in `total`. `sampling`: the 1-in-N rates the exporters said (ascending, at most eight; empty when none said one) and `sampled_records` how many of `records` came with one; nothing is multiplied. `exporters`: how many exporters have something in the window |
| `flows.exporters` | → `[{address, device_id?, device?, reason, version, first_at, last_at, datagrams, records, dropped, templates, sets_without_template, sampling: [u32]}]`, most recently heard first — every exporter followed (at most 256; one goes when it has been silent for a day). `device` is the device's name; `reason` says why no device claims the address (empty when one does). `version` is what it last spoke (5, 9, 10 for IPFIX). `records` is flow records counted from it, `dropped` those not counted because it is unclaimed and `unknown_sources` is `drop`; `templates` the templates known for it now — none once the collector has been stopped or rebound, until the exporter sends them again (the templates are the decoder's, and the decoder is the socket's; what the exporter sends in between is counted under `sets_without_template`); `sampling` the rates it has said |
| `logs.forward.list` | → `[Forwarder]`, by name. Never a URL or a token: `has_secret` says whether an `https_json` forwarder has its destination |
| `logs.forward.save` | `{id?, name, kind, enabled?, host?, port?, format?, streams?, min_severity?, lab?, ca_pem?, url?, token?}` → `Forwarder` — the document is replaced by what is sent, and what is left out takes its default (below). Two things are kept when left out: `enabled`, and the secret. `url` and `token` are accepted here and returned nowhere. `invalid` with the reason for a forwarder that cannot work, and nothing is written; `not_found` for an `id` that is not there; at most 16 forwarders |
| `logs.forward.delete` | `{id}` → `{deleted}` — stops it, and removes its secret from the vault |
| `logs.forward.test` | `{id}` → `{sent: true}`, or `unavailable` with why it could not be sent — one event that says it is a test, sent at once on a connection of its own, whether the forwarder is switched on or not. It is not stored and not counted in the forwarder's `sent` |
| `logs.device_setup` | `{device_id}` → `{device_id, profile, lines: [..], address, address_note, udp_port, note, trap_lines: [..], trap_port, trap_note}` — the configuration lines that would point this device's syslog (`lines`) and its SNMP notifications (`trap_lines`) at this Nocturne instance, for the profile's platform; it does not apply them. `lines` are ready to hand to `changes.create`. `trap_lines` are not: where the community goes they say `<snmp-community>`, because a community is a secret and is not returned, and `trap_note` says to put the device's own there, whether the receiver would keep what the device then sends (and why not, when not), and that a platform which cannot be told a port (RouterOS) sends to UDP 162 whatever the receiver listens on. Known platforms: `syntax: ios` and `syntax: routeros`, `unsupported` otherwise — this is a table in the feature and belongs in the profiles |

Events (contract): `logs.appended {entries: [LogEntry]}` — batched, at most
4 per second, capped at 200 entries per event with a `dropped` count (the
live tail must not melt the UI); `logs.status_changed`; `logs.signal` and
`logs.config_changed`, below. A stored trap is on `logs.appended` like a
line; nothing published ever contains a community. The flow collector
publishes nothing of what it counts: `logs.status_changed` when it starts,
stops or cannot bind, and no event per flow, per exporter or per minute —
a screen asks `flows.top` again. Starting a receiver is audited
(`logs.receiver.start`, with `receiver: "flows"` and the address bound).

## Pattern rules → signals

```rust
struct LogRule { id, name, enabled, builtin,
    platform,                           // "Cisco IOS / IOS-XE", "MikroTik RouterOS", "Any SNMP agent (SNMPv2-MIB, IF-MIB)"; "" for a rule of one's own
    stream,                             // "syslog" | "trap"
    pattern: regex (named groups allowed),  // over the line as sent; for a trap, over the message written for it
    min_severity?, scope: {device_ids, sites, roles, tags, profiles},
    threshold?: { count, window_s, group_by: [capture group] },   // below
    after?: rule id,                    // only counts while that rule's alert is open for the same device and instance
    clear_pattern?: regex,              // the opposite event: a line matching it clears the alert
    clear_after_s?,                     // raise through alerts.raise, clear it after this long with no match
    emit: Signal,                       // what to raise
    throttle_s: 60,                     // at most one signal per (rule, device, instance) per window; count the rest
    test_line, test_line_source,        // a line the rule matches, and where it came from
    updated_at }
enum Signal { ConfigChanged, InterfaceDown { interface_group }, InterfaceUp { .. }, LoginFailure, Reload,
              Alert { severity, title /* template with {groups} */ }, }
```

Signals are published as events other features subscribe to:
`logs.config_changed {device_id, by?, from?, entry_id, at}` (the config
feature backs the device up), `logs.signal {rule_id, rule, kind, device_id,
device, severity, title, interface, instance, groups, entry_id, count, at}`;
and `Alert` signals also call `alerts.raise {rule, device_id, instance,
severity, title, detail, dedup_key}` when that method is registered
(tolerate its absence). The dedup key is `logs:<rule id>:<device id>:<instance>`.
Built-in rules for IOS/IOS-XE
(`%SYS-5-CONFIG_I`, `%LINK-3-UPDOWN`, `%LINEPROTO-5-UPDOWN`,
`%SEC_LOGIN-4-LOGIN_FAILED`, `%SYS-5-RESTART`, `%SPANTREE-2-*`,
`%CDP-4-NATIVE_VLAN_MISMATCH`, `%CDP-4-DUPLEX_MISMATCH`, `%PM-4-ERR_DISABLE`,
`%DUAL-5-NBRCHANGE`, `%OSPF-5-ADJCHG`, `%BGP-5-ADJCHANGE`, `%SW_MATM-4-MACFLAP_NOTIF`,
`%ENVMON-*`/`%PLATFORM_ENV-*`) and RouterOS (`system,info ... changed by`,
`login failure`, `link down`/`link up`, `ospf ... state change`), each with a
test line taken from vendor documentation. `logs.rules.test {rule, line}`
shows what a rule would do with a pasted line (for a rule on the `trap`
stream, a trap's message; `parsed` then has the notification's name as
`mnemonic` and no severity): whether it matches, what it
captured, the `instance` it would be counted under, a `note` when a match
alone raises nothing (a threshold wants more of them, `after` wants another
rule's alert open), and `clears: true` when the line is the rule's opposite.

The throttle key is (rule, device, instance), where the instance is the
interface or the group key: a second port going down inside the window is a
second signal, not a swallowed one. The interface is the device's own
spelling when the line names one of its collected interfaces, otherwise
what the pattern captured in `interface`; the group key is the values of
the `group_by` captures, joined by a space. `count` in a signal is how many
matching lines it stands for: the ones swallowed since the last signal for
that instance, plus this one.

Instances come out of text a device wrote, on a port anyone can write to,
so these bounds hold whatever arrives. A rule speaks for at most 256
instances of one device inside one throttle window; past that the matches
are held back (`rules.held_back` in the status) and each instance speaks,
with its count, on its next matching line after the window. A rule
remembers at most 10,000 (device, instance) keys — see thresholds. And
what a pattern captured is text, never markup, and never as long as the
line: an instance is cut to 200 characters, a signal's title to 300 and
every captured value a signal carries to 2,000 — its `groups`, and so the
`detail` handed to `alerts.raise` and the `by` and `from` of
`logs.config_changed` (what is cut ends in `…`; the line itself is in the
store, whole) — and control characters and text-direction overrides in
them become spaces.
Two captures that differ only past the cut are one instance. An instance
has no space at either end (it ends the dedup key, and a key is trimmed
where it is used), and a title whose captures captured nothing is the
rule's name rather than empty.

**Thresholds.** A rule with a `threshold` fires when `count` matching lines
arrive inside `window_s` for one device and one value of the `group_by`
captures — eight failed logins from one address in a minute, not one. The
window is half open and slides: a line exactly `window_s` after another is
no longer inside that one's window, so eight lines of which the first and
the last are a full sixty seconds apart are not eight in a minute, and
eight of which they are a millisecond less are. It is
evaluated where every line passes, because a throttled signal cannot be
counted afterwards. Reaching the count raises the signal and starts the
count again from nothing, so a burst that goes on reaches it again every
`count` lines; the throttle then makes that one signal a window, carrying
the number of lines. `count` is 1 to 1,000 and `window_s` 1 to 86,400. A
threshold of `count: 1` raises on every line and is how a rule says "one
instance per neighbour" rather than one per device. The counters live in
memory (a restart starts them again, and the rule says so), and a rule
tracks at most 10,000 group keys so that a flood of spoofed sources cannot
grow it without bound; beyond that the oldest tenth are forgotten — keys
with an alert open last — and the status counts them (`rules.forgotten`).
A forgotten key starts counting again from nothing.

Security built-ins, each for a stated platform and each with a documented
test line: a burst of failed logins from one address (IOS
`%SEC_LOGIN-4-LOGIN_FAILED`, RouterOS `login failure`), a login that
succeeds straight after such a burst (IOS `%SEC_LOGIN-5-LOGIN_SUCCESS`), a
port-security violation (`%PORT_SECURITY-2-PSECURE_VIOLATION`), DHCP
snooping and dynamic ARP inspection drops (`%DHCP_SNOOPING-5-…`,
`%SW_DAI-4-…`), an interface flap storm (link down more than N times in a
window), and from traps: `authenticationFailure` bursts and `linkDown` /
`linkUp`, `coldStart` / `warmStart`. Other platforms are not covered, and the
rule list says which platform each rule is for. The trap rules are no
vendor's: they match the notifications SNMPv2-MIB and IF-MIB define, by the
name the receiver writes at the front of the message, read the `trap`
stream, and are labelled `Any SNMP agent (SNMPv2-MIB, IF-MIB)`.

| Rule id | Raises when | Clears |
|---|---|---|
| `builtin.ios.login_burst`, `builtin.routeros.login_burst` | 8 failed logins from one address in 60 s (high) | 600 s quiet |
| `builtin.ios.login_after_burst` | a login from that address succeeds while the burst's alert is open (critical) | by hand |
| `builtin.ios.port_security` | a violation, per port (high) | 1,800 s quiet |
| `builtin.ios.dhcp_snooping` | a dropped DHCP message, per kind of drop (medium) | 1,800 s quiet |
| `builtin.ios.arp_inspection` | invalid ARP packets, per port (high) | 1,800 s quiet |
| `builtin.ios.flap_storm`, `builtin.routeros.flap_storm` | one port goes down 5 times in 300 s (high) | 900 s quiet |
| `builtin.trap.link_down` | a `linkDown` trap, per ifIndex (low): `Link down: ifIndex 3 (GigabitEthernet0/3)`, or `Link down: ifIndex 3` when the interface has no name here | a `linkUp` trap for the same ifIndex |
| `builtin.trap.auth_failure_burst` | 8 `authenticationFailure` traps from one device in 60 s (high) | 600 s quiet |
| `builtin.trap.cold_start`, `builtin.trap.warm_start` | — a `reload` signal, not an alert | — |

The link rule is keyed by the ifIndex — the number, which is in every such
trap — and not by the interface's name, which is only there once the
monitor has polled the device: a map that arrives between the down and the
up must not make them two ports. The name is in the alert's title and in
the signal's `interface`.

A built-in rule's definition is the application's and is rewritten when a
new version changes it; its switch, its scope and its throttle are the
user's (`logs.rules.save` on a built-in changes those and nothing else)
and survive a restart and an upgrade.

A login line carries one field its sender did not write: the user name,
which is whatever was typed at the prompt. The login rules therefore take
the name greedily and the address from the end of the line, where the
device wrote it (`… [Source: a] [localport: n] [Reason:` on IOS, `… from a
via s` at the end of the line on RouterOS), so that a name typed to look
like the rest of the line cannot move an attempt to an address of the
sender's choosing and keep a burst from adding up.

A test line's source says
whether it is **quoted** from the vendor's documentation (with the page) or
**synthetic** — written by hand to a format the documentation gives. A trap
rule's line is synthetic in another way and says so: it is the message this
receiver writes for a datagram net-snmp's `snmptrap` sent (captured, and
replayed in the tests, which check the message is that line) — not a
device's words, since a trap is numbers and the words are ours. For
IOS the failed-login and ARP-inspection lines are synthetic (Cisco
publishes their formats, not an example); for RouterOS the flap line is
(MikroTik's page gives the message `<interface> link down`, not the topics
in front of it). None is a line recorded from a device.

An `Alert` raised by a rule is cleared again — `alerts.clear {dedup_key,
note}` with the same dedup key — when its opposite arrives (`linkUp` after
`linkDown`, a neighbour coming back) or after `clear_after_s` without a
match. An alert for a lab device never reaches a notification channel:
`alerts.raise` is called with `notify: []` for it, and without the field
for hardware.

* **The opposite** is the rule's `clear_pattern`. It has to capture the
  rule's `group_by` groups, or it could not say which alert it clears. The
  neighbour rules (EIGRP, OSPF, BGP on IOS; OSPF on RouterOS) raise when a
  neighbour is lost and are cleared when it is back; a neighbour coming up
  raises nothing.
* **Inside the throttle window an alert is held.** An opposite that arrives
  before the window the alert was raised in has ended is remembered, and
  the alert is cleared when the window ends unless the rule matched again
  in between: a neighbour that bounces three times in twenty seconds is one
  alert, cleared once, and an alert is never left closed on a device that
  ended up down. After the window an opposite clears at once.
* **Quiet** means no line matching the rule's pattern for that device and
  instance — whether or not the line raised anything — for `clear_after_s`
  (1 s to 30 days), checked every two seconds.
* **A clearing is a signal**: `logs.signal` with `kind: "cleared"`,
  `reason: "opposite" | "quiet"`, the alert's title, `count: 0`, and
  `entry_id` of the line that said so (null when time did). It is stored in
  the `event` stream like any other.
* **What is open is remembered in memory.** After a restart an opposite
  still clears an alert raised before it — the monitor is asked, at most
  once a window per instance and for at most 256 instances of one device a
  window (an opposite for a neighbour nobody saw go down costs a call, and
  a sender can make up a neighbour per line; the rest are counted in
  `rules.held_back` and asked for on their next line after the window), and
  the clearing is recorded only if the monitor had something to clear — but
  the quiet period of such an alert is not known and it stays open until
  its opposite, its next match, or a person.
* **`rules.open_alerts` counts the alerts a rule is waiting to clear** — by
  its opposite or its quiet period — as far as this process knows. An alert
  whose rule has neither is closed by a person, in the monitor, and nothing
  here hears of it: it is not counted, and what the rule remembers of it
  goes after an hour without a match like any other record (until then a
  rule that names it in `after` sees it as open).
* **A rule that stops watching forgets.** Saving a rule switched off, or
  with anything changed that decides which lines it sees, how it counts
  them or how its alert ends (everything but its name and its throttle),
  drops what it was counting and which alerts it had open, exactly as
  deleting it does. Those alerts stay as they are in the alert list: a
  rule that is no longer looking must not report a quiet period, and an
  opposite that still arrives clears them as after a restart.
* **The monitor is told in order, through one bounded queue.** The calls to
  `alerts.raise` and `alerts.clear` are made one after another in the order
  the rules decided them — a neighbour that comes back and is lost again a
  moment later is a clear and then a raise, and made side by side they
  could leave the alert closed on a device that ended up down. At most
  4,096 calls wait; past that a call is dropped and counted
  (`rules.undelivered`), the signal behind it having been stored and
  published all the same.
* `clear_pattern` and `clear_after_s` are refused on a rule that raises no
  alert.

## Forwarding to a log platform

Nocturne is a network event collector for the people who run the network,
with thirty days on one SQLite file. It is not a SIEM — no normalisation
across hundreds of source types, no years of retention, no threat
intelligence, no case management — and it does not pretend to be. What it
does instead is hand everything to one.

```rust
struct Forwarder { id, name, enabled,
    kind: syslog_udp | syslog_tcp | syslog_tls | https_json,
    host, port,                          // syslog kinds; "" and 0 for https_json
    format: rfc5424 | json,              // https_json is always json
    streams: [syslog | trap | event | audit],   // audit is opt-in, never a default
    min_severity?,
    lab: bool,                           // also what simulated devices write; off unless asked for
    ca_pem?,                             // syslog_tls, https_json: the authority the collector's certificate must be signed by
    has_secret: bool,                    // https_json: the URL and token live in the vault
    updated_at }
```

* A forwarder taps a batch **after it is stored**, so only the text that is
  kept — already redacted — ever leaves. Device lines and traps are tapped
  where they are written (the receivers, `logs.ingest`, `logs.pull`). The
  `event` and `audit` streams are written by every feature through the
  core, so they are read back from the store by id, twice a second, which
  is the same promise kept another way. A forwarder sends what is stored
  from the moment it is saved or started; nothing older.
* Each has a bounded queue of 10,000 entries. A slow or dead collector
  never slows ingest: putting an entry on the queue is all the ingest path
  does, and when the queue is full the entry is dropped and counted.
  `logs.status` shows `sent`, `dropped`, the state and the last error.
  * `sent` is what was handed to the network. No syslog transport
    acknowledges anything, so for those it does not mean "arrived" — a
    collector that accepts a connection and never reads from it is counted
    until the buffers between the two machines are full. For `https_json`
    it is what the collector answered 2xx to.
  * `dropped` is what never left: the queue was full, every attempt failed,
    or the forwarder was saved again or deleted while entries waited.
  * Entries are sent in batches of up to 200. A batch that fails is tried
    three times in all, 1 s, 2 s, 4 s … 30 s apart, and then dropped and
    counted. Over a stream a batch that is tried again may arrive twice in
    part; over UDP each datagram is tried once and nothing is tried again.
  * A collector that takes nothing for 5 s has stopped reading
    (`last_error` says so); a connection the collector closed is replaced
    before the next batch is written into it. Connecting may take 10 s, an
    HTTPS request 15 s.
  * `last_error` tells a certificate that was refused from a collector
    that is not there, for `https_json` as for `syslog_tls`: "the TLS
    handshake with the collector failed: its certificate …", not "could
    not be reached".
  * The counters are since the application started, like the receivers'.
    The queue is in memory: what waits when the application stops is lost.
* **Streams.** `syslog`, `trap` and `event` when `streams` is left out.
  **Forwarding the `audit` stream sends operator activity off the
  machine**, so it happens only for a forwarder whose `streams` names it,
  the audit entry for saving that forwarder says that it does
  (`audit: true`, and in words), and the screen says what it means.
* **Severity.** `min_severity` holds back what is less severe. An entry
  with no severity — every trap, a line that came without a priority — is
  not held back: nothing says it is less severe. Nor is the audit trail:
  every audit entry is written with the same severity, so a floor would
  not thin it out but silence it.
* **Labs.** What a lab's simulated devices write, and what is recorded
  about them in `event`, is not forwarded unless the forwarder has
  `lab: true` — a log platform would take a lab's port-security violation
  for a real one, for the reason an alert about a lab device notifies
  nobody. With it, a lab is the way to try a forwarder out. The audit
  trail is what people did, whatever to, and is never held back as a lab's.
* For `https_json` the URL and the token are one secret, stored as
  `logs:forward:<id>:secret`, write-only over RPC, and absent from every
  document, event, error message and log line (an error is built from the
  kind of failure, never from the HTTP client's message, which contains the
  URL — and never from what the collector answers, which may repeat it: an
  HTTP status and the collector's own numeric error code are all that is
  taken). `url` and `token` are parameters of `logs.forward.save` only:
  left out, what is stored stays; an empty `url` clears both. **A new URL
  does not inherit the stored token** — it would be sent to whoever is at
  the new address, which would make a write-only secret readable — so a
  token has to come with it, or there is none. Debug and trace lines of the
  HTTP client name the host it connects to; for these requests they are
  discarded, and so that none can be written elsewhere, each request has a
  connection of its own over HTTP/1.1.
* **The destination is checked like a notification channel's**: a URL has
  to start with `https://` (or `http://` for a collector on one's own
  network) and be a complete URL with a host; a syslog forwarder needs a
  host name or an address, and a port (514 when left out, 6514 for
  `syslog_tls`). Collectors live on internal networks, so no address is
  refused for being private. The rule is written in this crate as it is in
  `nocturne-monitor` (a feature does not link to another); if it grows, its
  place is the core. A redirect is not followed: log text goes to the URL
  that was given and nowhere a reply points to. A syslog forwarder pointed
  at this application's own receiver — an address of this machine and a
  port the receiver is listening on — is refused: it would be sent its own
  lines, store them and send them again, for ever. (A loop through another
  relay, or a receiver switched on afterwards, is not seen.) A refusal
  never repeats the host, the URL or the token it was given.
* **TLS** is rustls with ring, like everything else. The collector's
  certificate is checked against the authorities this machine trusts, or —
  `ca_pem` — against the one authority given, for a collector with a
  certificate from a private CA. A certificate that does not check out ends
  the attempt before any log text is written; there is no way to switch the
  check off, and no client certificate is presented. `ca_pem` is public
  text, kept in the forwarder's document and returned with it — so a
  `ca_pem` that holds anything but certificates (a file with the
  authority's private key in it, which a certificate parser reads past
  without a word) is refused, `invalid`, and nothing of it is stored.
* `logs.forward.test {id}` sends one event that says it is a test.
* Every save, delete and test is audited (`logs.forward.save`,
  `logs.forward.delete`, `logs.forward.test`), never with the URL or the
  token: for an `https_json` forwarder the entry says `secret_changed`, not
  what to.

**What is sent.**

* `rfc5424` — one RFC 5424 message per entry. `HOSTNAME` is the device the
  entry is about, by its inventory name; for a line no device claims, the
  name the line gives itself and then the address it came from; `-` when
  the entry is about no device (a collector then files it under the sender,
  which is this machine). `APP-NAME` says who wrote the text: the device's
  own process for a syslog line, `nocturne` for traps, events and the audit
  trail. `MSGID` is the kind of message — the vendor mnemonic
  (`LINK-3-UPDOWN`), a trap's name, an event's source, an audit action.
  `MSG` is the stored message. The priority is the line's own facility and
  severity; a line that came without a priority is sent as `user.notice`
  (13), which is what RFC 3164 has a relay do — RFC 5424 has no way to say
  "none was given", and the JSON format keeps `null`. Traps and events are
  facility `daemon` (3), the audit trail `log audit` (13). The only
  structured data is the registered `origin` element: `ip` for a device's
  line or trap, `software` and `swVersion` for what Nocturne wrote. Text is
  brought inside what each field allows; a line break or another control
  character in a message is sent as a space, so that a device cannot start
  a second message at the collector with one.
* `json` — one document per entry, on one line: the row as `logs.query`
  returns it, `{id, ts, stream, device_id, severity, source, message,
  body}`, with `device` (the inventory name, or null) and `time` (`ts` as
  RFC 3339) added.
* Framing: a datagram per message over UDP, at most 8,192 bytes; a message
  to a line over TCP; over TLS, RFC 5424 is octet-counted as RFC 5425
  requires and JSON is a document to a line. A message may be 256 KiB on a
  stream or in a request. One that is too long is cut at its end (RFC
  5424), or loses its `body` and says `truncated: true`, and then the end
  of its message (JSON) — it is always valid JSON.
* `https_json` — one `POST` per batch with `Content-Type:
  application/json`: an HTTP event collector's envelope per entry, one to
  a line, `{time: seconds, host?, source: "nocturne", sourcetype:
  "nocturne:<stream>", event: <the JSON document>}`. `host` is as
  `HOSTNAME` above and is left out when there is none. The token is sent
  as `Authorization: Splunk <token>`; a token written with its own scheme
  (`Bearer …`) is sent as written; no token, no header.

**Not built:** CEF or any other format; a client certificate for a
collector that asks for one; indexer acknowledgement; a queue that
survives a restart; a proxy other than the one the environment names.

## Tests

Parsers against a corpus of real-shaped lines per format (incl. malformed
input, non-UTF-8, huge lines, missing PRI); live receivers on port 0:
send over UDP and TCP (octet-counted and newline framing), `snmptrap` if
installed else crafted PDUs; attribution by address and by interface IP;
storm behaviour (100k datagrams: bounded memory, counted drops, no hang);
retention; query paging and filters; histogram bucketing; rule matching,
throttling and each signal; `logs.appended` batching; receiver restart on
settings change and a clear error when the port is taken; flows top-N from
real and crafted NetFlow v5/v9/IPFIX datagrams (below).

Traps: real datagrams on port 0 — datagrams captured from net-snmp's
`snmptrap` and `snmpinform` and replayed (so the receiver is tested where
net-snmp is not installed), and the tools themselves where they are,
skipping with a reason where they are not. A trap with the device's
community is stored by name and attributed; a wrong community is rejected,
counted and appears nowhere — every table of the database, every event on
the bus, the status, the sources, the settings, the audit trail and the log
lines are searched for the wrong one and for the right one; a community in
a binding (a forwarded trap's `snmpTrapCommunity.0`) is withheld; a
community stored a moment ago counts at once; with the check off any
community is taken from a known device and an unknown source follows
`unknown_sources`; an inform is acknowledged once it is stored, a repeat is
answered and stored once, and one that was not kept is not answered
(`snmpinform` exits successfully, and times out with the wrong community);
a v3 authenticated notification is counted and not stored, and one without
authentication is kept only from a device stored that way; an interface is
named when `monitor.ifindex` has it and numbered when not; a storm with the
writer held up is dropped and counted with the queue never over its limit;
a datagram of six thousand bindings (written by hand) is one row of
sixty-four with the rest counted, and one built to leave the body a few
bytes short is not searched to its end; a trap the database would not take
is counted as dropped and the next is stored;
a flood of forged traps takes no place in the queue; the receiver says why
it cannot bind, is retried by `logs.restart`, closes its port when
switched off, and still writes what it had received before that; the built-in rules on real datagrams (link down raised and
cleared by link up, seven authentication failures nothing and the eighth an
alert, cold and warm start a `reload`); each trap rule's test line is the
message the receiver writes for net-snmp's datagram; traps age out and are
cleared with their index rows. Thresholds: N-1 lines
raise nothing and the Nth does, per group; a line a whole window after the
first does not count with it and one a millisecond inside does; the
group-key cap holds under a flood of 50,000 made-up keys and the status
counts every key that was let go; a capture as long as the line is cut
before it is a key or a title; a user name typed to look like the rest of
the line does not move a failed login to another address. Forwarders: a local UDP socket, TCP listener, TLS listener and HTTPS
server (each certificate made in the test) receive what was stored,
redacted, in the format asked for; a collector whose certificate does not
check out is sent nothing, over TLS and over HTTPS, and is said to have a
certificate that was refused rather than to be unreachable; a collector that stops reading costs ingest
nothing (measured against the same lines with no forwarder), the queue
stays at its limit and what did not fit is counted; a dead collector is
tried again and what cannot be sent is counted; the audit trail reaches
only a forwarder that names it, and a lab's lines only one that asks; the
URL and the token are searched for in every table, every event, every
result and error and every log line at trace level; a token is not sent to
a URL it was not given for; an authority pasted together with a private key
is refused and the key is in no table; a line break in a message starts no second
message. Flows: real datagrams on port 0 — the export datagrams softflowd
(v5, v9, IPFIX, each also with `-s 100`) and nfdump's `nfreplay` (v5, v9)
sent for five conversations written out as a capture file, written down
and replayed (so the collector is tested where neither is installed), and
softflowd itself where it is, skipping with a reason where it is not; what
softflowd cannot be made to send (interface numbers, data before its
template, a sampling rate of one's choosing) built by hand from Cisco's v5
format and RFC 3954 and said to be. Top-N by each of the six keys against
what the packets add up to; the coverage block; every version of the same
traffic giving the same talkers; a sampling rate reported in each
version's own way of saying it, with the counts left as sent; a set that
arrives before its template counted — in the status, on the exporter and
in the answer — rather than guessed at; garbage counted and making nobody
an exporter; an interface named when `monitor.ifindex` has it and numbered
when not, and whose it is said over two exporters; an exporter no device
claims listed and followed, or listed and dropped, and attributed once a
device has the address; a bucket ageing out and the answer saying since
when; fifty thousand made-up sources in a minute held as 512 keys while it
runs and 20 once it is over, with every byte in a row or in `other` and
the total exact; a storm dropped and counted with the queue never over its
limit; the collector saying why it cannot bind, retried by `logs.restart`,
moved to another port without forgetting what it had counted — and
without claiming templates its new decoder does not have — and still
answering once it is switched off; datagrams built to be slow (thousands
of templates into a full table, a set of zeros with one byte at its end,
"withdraw all" fifteen thousand times, a thousand empty fields to a
one-byte record) each decoded in the time their size takes (engine
tests); nothing of a flow in any table, event or
log line. `logs.ingest` is refused for hardware however the
request is spelled (id, name, address, parameters by position, a `lab` of
the caller's own, a record that carries a lab's label and is reached over
SSH), and a
port-security violation on a lab switch raises its alert end to end.
Clearing: an opposite after the window clears at once and with the key the
alert was raised with; a bounce inside the window is one alert cleared once;
a quiet period ends on the wall clock; an alert from before a restart is
cleared by its opposite; a build without the monitor loses nothing but the
calls; a flood of opposites for made-up instances is 256 calls and a count;
a clear followed at once by a raise reaches a slow monitor in that order; a
rule that is switched off does not clear its alerts by falling silent;
calls a monitor that has stopped answering cannot take are dropped and
counted. `logs.clear` refuses `audit`, and an audit entry goes when it is
older than the retention and not before.

**Not verified:** the flow collector has met two software exporters on
this machine — softflowd 1.1.0 (NetFlow v5, v9 and IPFIX, with and without
sampling) and nfdump 1.7.3's `nfreplay` (v5 and v9) — reporting traffic
written by hand as a capture file. It has never met a router or a switch:
Cisco Flexible NetFlow, MikroTik Traffic Flow, Juniper inline J-Flow and
whatever fields and option templates they send (sampler tables, VRF and
interface-name options, enterprise fields) are untested, and so is an
exporter that reports interface numbers (softflowd reports 0 when it reads
a file). The rate of a real router's export has not been measured against
the queue. `docs/STATUS.md` should say so. The trap receiver has only met
net-snmp's `snmptrap` and `snmpinform` (5.9.4), never a switch or a router:
what a device puts in its traps beyond the standard bindings, and how it
repeats an inform, is untested. The trap lines of `logs.device_setup` are
written from Cisco's and MikroTik's documentation and have not been applied
to a device.
