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
  traps:  { enabled: false, port: 1162, bind: "0.0.0.0", communities: [] /* empty = accept any */ },
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
  ifDescr=Gi0/3`), `body` = the decoded trap. v3 traps that cannot be
  authenticated are counted, not stored as if valid.
* Batched inserts (≤ 250 ms or 500 rows), bounded queue: under a storm the
  receiver drops and **counts** (shown in `logs.status`) rather than growing
  without bound or blocking the socket.
* Retention task: age and row cap, in small batches.
* Flows: aggregate in memory per exporter (device) into 1-minute buckets of
  top talkers; keep 24 h. No per-flow storage.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `logs.query` | `{stream?: syslog|trap|audit|event, device_id?, max_severity?, text?, since?, until?, before_id?, limit?: 200}` → `{entries: [LogEntry], has_more}` newest first (`before_id` pages back) |
| `logs.histogram` | `{stream?, device_id?, text?, since, until, buckets?: 60}` → `[{t, count, by_severity: [8]}]` |
| `logs.top` | `{since, until, by: device|mnemonic|severity, limit?}` → `[{key, count}]` |
| `logs.status` | → `{syslog: {enabled, listening: [..], error?, received, stored, dropped, last_at}, traps: {..}, flows: {..}}` |
| `logs.sources` | → `[{address, device_id?, last_at, count}]` |
| `logs.rules.list` / `.save` / `.delete` / `.test` | pattern rules, below |
| `logs.clear` | `{stream, before?}` |
| `flows.top` | `{device_id?, minutes?: 15, by: source|destination|conversation|port|protocol, limit?: 20}` → `[{key, bytes, packets, flows}]` |
| `logs.device_setup` | `{device_id}` → `{lines: [..], note}` — the configuration lines that would point this device's syslog/traps at this Nocturne instance (for the profile's platform), ready to hand to `changes.create`; it does not apply them |

Events (contract): `logs.appended {entries: [LogEntry]}` — batched, at most
4 per second, capped at 200 entries per event with a `dropped` count (the
live tail must not melt the UI); `logs.status_changed`.

## Pattern rules → signals

```rust
struct LogRule { id, name, enabled, builtin, stream, pattern: regex (named groups allowed),
    min_severity?, scope?, 
    emit: Signal,                       // what to raise
    throttle_s: 60 }                    // at most one signal per device per rule per window; count the rest
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
