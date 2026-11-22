# Feature: logs and traps (`nocturne-logs`)

What the network says about itself: a syslog receiver, an SNMP trap
receiver and a flow collector, stored, searchable, tailed live, and turned
into signals the rest of the product reacts to (an interface flapping, a
login failure, "someone changed the configuration").

Engines: `nocturne-netprobe` (`syslog` server/parser, `flow` collector —
read `docs/dev/spec-netprobe.md`), `nocturne-snmp` (`decode_trap`, `names`),
core `store.log_*`.

## Receivers

Settings section `logs`:

```rust
{ syslog: { enabled: false, udp_port: 5514, tcp_port: 5514, bind: "0.0.0.0" },
  traps:  { enabled: false, port: 1162, bind: "0.0.0.0", require_known_community: true },
  flows:  { enabled: false, port: 2055, bind: "0.0.0.0" },
  retention_days: 30, max_rows: 2_000_000, unknown_sources: "store" /* or "drop" */ }
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
* Traps: `decode_trap`, resolve OIDs to names, stream `trap`, message = the
  trap name plus its most useful varbinds (`linkDown ifIndex=3
  ifDescr=Gi0/3`), `body` = `{version, trap, oid, uptime?, varbinds: [{oid,
  name, value}]}` built field by field. v2c informs are acknowledged.
  * **A community is a secret.** It is never stored, published, logged or
    returned — not in the body, not in settings. With
    `require_known_community` (the default) a v1/v2c trap is kept only when
    its community equals the SNMP community of the device its source address
    belongs to, read from the vault; anything else is counted under
    `traps.rejected` and dropped. Switching it off accepts any community
    from a known device; an unknown source follows `unknown_sources`.
  * v1 and v2c traps are unauthenticated UDP: whoever can reach the port can
    forge one. The interface says so where the receiver is switched on, and
    nothing derived from a trap ever changes a device.
  * SNMPv3 notifications that need authentication or privacy are counted
    under `traps.unauthenticated` and **not** stored as if valid; decoding
    them is not built yet.
  * `ifIndex` varbinds are shown with the interface name when the monitor
    has an index map for the device (`monitor.ifindex`), and as `ifIndex N`
    when it has not.
* Batched inserts (≤ 250 ms or 500 rows), bounded queue: under a storm the
  receiver drops and **counts** (shown in `logs.status`) rather than growing
  without bound or blocking the socket.
* Retention task: age and row cap, in small batches.
* Flows: NetFlow v5, v9 and IPFIX (no sFlow). Aggregated in memory per
  exporter into 1-minute buckets of the top talkers, kept 24 h. No per-flow
  storage, so everything is lost on a restart and every answer states what
  it covers. Buckets are keyed by the time of receipt (a v5 or v9 timestamp
  is relative to the exporter's boot). Byte and packet counts are what the
  exporter sent: a sampling rate is reported beside them, never multiplied
  in silently. Exporters are attributed like syslog sources, and an unknown
  exporter is listed, not dropped without trace.
* Lab devices: a simulated device has no socket to send from, so
  `nocturne-lab` hands its syslog lines to `logs.ingest`. They then take the
  same path as a datagram — index row, rules, signals, tail, redaction — and
  are marked `via: "lab"`. Lab devices send no traps and no flows; the
  status says so rather than showing an idle receiver.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `logs.query` | `{stream?: syslog|trap|audit|event, device_id?, max_severity?, text?, since?, until?, before_id?, limit?: 200}` → `{entries: [LogEntry], has_more}` newest first (`before_id` pages back) |
| `logs.histogram` | `{stream?, device_id?, text?, since, until, buckets?: 60}` → `[{t, count, by_severity: [8]}]` |
| `logs.top` | `{since, until, by: device|mnemonic|severity, limit?}` → `[{key, count}]` |
| `logs.status` | → `{syslog: {enabled, listening: [..], error?, received, stored, dropped, last_at}, traps: {.., rejected, unauthenticated}, flows: {.., records, sets_without_template, exporters}, forwarders: [{id, name, state, sent, dropped, last_error?, last_at?}], store: {..}}` |
| `logs.restart` | → the status, after re-applying the receiver settings |
| `logs.bursts` | `{..filter, min_count?: 5, limit?: 50}` → `[Burst]` — the same message from the same device many times, folded into one row with its count, first and last time |
| `logs.around` | `{entry_id? \| at, device_id?, window_s?: 300, limit?: 100}` → what else happened around one moment: log lines, alerts and changes on that device, in time order |
| `logs.follow` / `logs.unfollow` | `{filter, follow_id?}` → `{id}` / `{id}` → `{closed}` — a live tail with a filter applied on the server |
| `logs.pull` | `{device_id}` → reads the device's own log buffer over the CLI (read-only) and stores the lines it has not seen |
| `logs.ingest` | `{device_id, lines: [String]}` → `{stored}` — for `nocturne-lab` only: refused unless the device is a lab device, so it is never a way to write log lines for hardware |
| `logs.sources` | → `[{address, device_id?, last_at, count}]` |
| `logs.rules.list` / `.save` / `.delete` / `.test` | pattern rules, below |
| `logs.clear` | `{stream, before?}` — refused for `audit`: the audit trail is evidence, and the API that is audited does not get to erase it. It ages out by retention only |
| `flows.top` | `{device_id?, minutes?: 15, by: source\|destination\|conversation\|application\|interface\|dscp, limit?: 20}` → `{rows: [{key, label, bytes, packets, flows}], coverage: {since, records, sets_without_template, sampling?: [u32], exporters}}` — `application` is protocol and well-known port; `dscp` is the marking on the wire |
| `flows.exporters` | → `[{address, device_id?, version, last_at, records, templates}]` |
| `logs.forward.list` / `.save` / `.delete` / `.test` | forwarders, below |
| `logs.device_setup` | `{device_id}` → `{lines: [..], note}` — the configuration lines that would point this device's syslog/traps at this Nocturne instance (for the profile's platform), ready to hand to `changes.create`; it does not apply them |

Events (contract): `logs.appended {entries: [LogEntry]}` — batched, at most
4 per second, capped at 200 entries per event with a `dropped` count (the
live tail must not melt the UI); `logs.status_changed`.

## Pattern rules → signals

```rust
struct LogRule { id, name, enabled, builtin, stream, pattern: regex (named groups allowed),
    min_severity?, scope?,
    threshold?: { count, window_s, group_by: [capture group] },   // below
    clear_after_s?,                     // raise through alerts.raise, clear it after this long with no match
    emit: Signal,                       // what to raise
    throttle_s: 60 }                    // at most one signal per (rule, device, instance) per window; count the rest
enum Signal { ConfigChanged, InterfaceDown { interface_group }, InterfaceUp { .. }, LoginFailure, Reload,
              Alert { severity, title /* template with {groups} */ }, }
```

Signals are published as events other features subscribe to:
`logs.config_changed {device_id, by?, from?}` (the config feature backs the
device up), `logs.signal {rule_id, kind, device_id, severity, title, entry_id,
count}`; and `Alert` signals also call `alerts.raise` when that method is
registered (tolerate its absence). Built-in rules for IOS/IOS-XE
(`%SYS-5-CONFIG_I`, `%LINK-3-UPDOWN`, `%LINEPROTO-5-UPDOWN`,
`%SEC_LOGIN-4-LOGIN_FAILED`, `%SYS-5-RESTART`, `%SPANTREE-2-*`,
`%CDP-4-NATIVE_VLAN_MISMATCH`, `%CDP-4-DUPLEX_MISMATCH`, `%PM-4-ERR_DISABLE`,
`%DUAL-5-NBRCHANGE`, `%OSPF-5-ADJCHG`, `%BGP-5-ADJCHANGE`, `%SW_MATM-4-MACFLAP_NOTIF`,
`%ENVMON-*`/`%PLATFORM_ENV-*`) and RouterOS (`system,info ... changed by`,
`login failure`, `link down`/`link up`, `ospf ... state change`), each with a
test line taken from vendor documentation. `logs.rules.test {rule, line}`
shows what a rule would do with a pasted line.

The throttle key is (rule, device, instance), where the instance is the
interface or the group key: a second port going down inside the window is a
second signal, not a swallowed one.

**Thresholds.** A rule with a `threshold` fires when `count` matching lines
arrive inside `window_s` for one device and one value of the `group_by`
captures — eight failed logins from one address in a minute, not one. It is
evaluated where every line passes, because a throttled signal cannot be
counted afterwards. The counters live in memory (a restart starts them
again, and the rule says so), and a rule tracks at most 10,000 group keys so
that a flood of spoofed sources cannot grow it without bound; beyond that
the oldest are forgotten and the status counts them.

Security built-ins, each for a stated platform and each with a documented
test line: a burst of failed logins from one address (IOS
`%SEC_LOGIN-4-LOGIN_FAILED`, RouterOS `login failure`), a login that
succeeds straight after such a burst (IOS `%SEC_LOGIN-5-LOGIN_SUCCESS`), a
port-security violation (`%PORT_SECURITY-2-PSECURE_VIOLATION`), DHCP
snooping and dynamic ARP inspection drops (`%DHCP_SNOOPING-5-…`,
`%SW_DAI-4-…`), an interface flap storm (link down more than N times in a
window), and from traps: `authenticationFailure` bursts and `linkDown` /
`linkUp`, `coldStart` / `warmStart`. Other platforms are not covered, and the
rule list says which platform each rule is for.

An `Alert` raised by a rule is cleared again — `alerts.clear` with the same
dedup key — when its opposite arrives (`linkUp` after `linkDown`, a
neighbour coming back) or after `clear_after_s` without a match. An alert
for a lab device never reaches a notification channel.

## Forwarding to a log platform

Nocturne is a network event collector for the people who run the network,
with thirty days on one SQLite file. It is not a SIEM — no normalisation
across hundreds of source types, no years of retention, no threat
intelligence, no case management — and it does not pretend to be. What it
does instead is hand everything to one.

```rust
struct Forwarder { id, name, enabled,
    kind: syslog_udp | syslog_tcp | syslog_tls | https_json,
    host, port,                          // syslog kinds
    format: rfc5424 | json,
    streams: [syslog | trap | event | audit],   // audit is opt-in, never a default
    min_severity?,
    has_secret: bool }                   // https_json: the URL and token live in the vault
```

* A forwarder taps a batch **after it is stored**, so only the text that is
  kept — already redacted — ever leaves.
* Each has a bounded queue. A slow or dead collector never slows ingest: it
  drops and counts, and `logs.status` shows sent, dropped and the last error.
* For `https_json` the URL and the token are one secret, stored as
  `logs:forward:<id>:secret`, write-only over RPC, and absent from every
  document, event, error message and log line (an error is built from the
  kind of failure, never from the HTTP client's message, which contains the
  URL). The destination is checked like a notification channel's.
* `logs.forward.test {id}` sends one event that says it is a test.
* Every save and delete is audited. Forwarding the `audit` stream sends
  operator activity off the machine, so it is switched on per forwarder and
  the screen says what it means.

TLS is rustls with ring, like everything else.

## Tests

Parsers against a corpus of real-shaped lines per format (incl. malformed
input, non-UTF-8, huge lines, missing PRI); live receivers on port 0:
send over UDP and TCP (octet-counted and newline framing), `snmptrap` if
installed else crafted PDUs; attribution by address and by interface IP;
storm behaviour (100k datagrams: bounded memory, counted drops, no hang);
retention; query paging and filters; histogram bucketing; rule matching,
throttling and each signal; `logs.appended` batching; receiver restart on
settings change and a clear error when the port is taken; flows top-N from
crafted NetFlow v5/v9/IPFIX packets.

Traps: `snmptrap` from net-snmp where it is installed (skipping with a
reason where it is not), else crafted PDUs; a wrong community is rejected
and counted and appears nowhere; an inform is acknowledged; a v3
authenticated notification is counted and not stored. Thresholds: N-1 lines
raise nothing and the Nth does, per group; the group-key cap holds under a
flood. Forwarders: a local syslog listener and a local HTTPS server receive
what was stored, redacted; a dead collector costs ingest nothing; the secret
appears in no RPC result, event or error. Flows: top-N per key, a bucket
ageing out, the coverage block, and a set that arrives before its template
counted rather than guessed at. `logs.ingest` is refused for hardware, and a
port-security violation on a lab switch raises its alert end to end.

**Not verified:** the flow decoder has only met datagrams built by hand and
by this repository's own encoder, never a real exporter. `docs/STATUS.md`
says so until one has been tested.
