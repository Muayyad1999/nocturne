# Feature: toolbox (`nocturne-toolbox`)

The small tools a network engineer reaches for twenty times a day, in one
place and scriptable: calculators and decoders (instant, offline) and probes
run from the workstation or from a device.

Engines: `nocturne-tools` (pure calculators — read `docs/dev/spec-tools.md`),
`nocturne-netprobe` (active probes and small servers — read
`docs/dev/spec-netprobe.md`), `nocturne-transport` (`generate_keypair`),
core `connect::exec`, `jobs`.

This crate is a thin, well-validated RPC façade: the engines do the work.
Do not re-implement what they provide; if one is missing something, put it
in your report.

## Calculators (contract) — synchronous RPC, no device or network access

One method per tool, params and results mirroring the engine's types
(look them up in the crates; names here are the RPC names):

`tools.subnet {input}` (address/prefix, address + mask, or wildcard → network,
broadcast, host range, counts, masks, class, scope, binary) ·
`tools.subnet.split {cidr, new_len | count}` · `tools.vlsm {base, sizes:
[{name, hosts}]}` · `tools.supernet {cidrs}` (summarise) · `tools.contains
{a, b}` / `tools.overlaps {cidrs}` · `tools.wildcard {input}` (mask ↔
wildcard, ACL line helper) · `tools.ip.convert {input}` (dotted, decimal,
hex, binary, IPv4-mapped; IPv6 expand/compress; reverse-DNS name) ·
`tools.mac {input}` (normalise to every vendor format, OUI vendor, flags
unicast/multicast/local, EUI-64 link-local) · `tools.secret.decode
{kind: cisco7|junos9, text}` and `.encode` (clearly labelled: these are
obfuscation, not encryption) · `tools.secret.identify {text}` (what kind of
hash/type is this line: type 5, 7, 8, 9, …) · `tools.convert {value, from,
to}` (bits/bytes, rates, dBm/mW, time) · `tools.transfer_time {size,
rate, overhead_pct?}` · `tools.mtu {link_mtu, encapsulations: [..]}`
(overhead of GRE/IPsec/VXLAN/PPPoE/MPLS/… → payload MTU and TCP MSS) ·
`tools.reference {table: ports|dscp|ethertypes|protocols|admin_distance|
icmp_types|cable_pinouts|wifi_channels|voice_codecs|voice_links|…, query?}` · `tools.regex {pattern,
text, flags}` (matches with groups; for testing profile and log patterns) ·
`tools.diff {a, b, context?}` (text diff) · `tools.password {length,
classes, count}` and `tools.passphrase` (CSPRNG) · `tools.keys.generate
{kind, comment}` · `tools.cron {expr}` (next runs — only if the engine has
it) · `tools.list` → the catalogue `[{id, title, group, description,
offline: bool}]` so the UI and the assistant can discover tools.

Bad input returns `invalid` with the engine's message (which says what was
wrong with the input) — never a default answer.

### QoS calculators (contract)

The cross-crate QoS contract is `docs/dev/spec-qos.md` ("Tools"); the shapes
are here. Both are in group `qos` of `tools.list`, offline, not jobs, and
audit nothing (they change nothing). Engine: `nocturne_tools::qos`.

| Method | Params → Result |
|---|---|
| `tools.qos.marking` | `{value: string \| number, as: "dscp" \| "tos" \| "precedence" \| "name"}` → `Marking` |
| `tools.qos.voice` | `{codec, calls, link, crtp?: false, vad?: false, packet_ms?, l2_bytes?}` → `VoiceBandwidth` |

```rust
struct Marking { dscp, name?,                     // name null: a value the registry has not named
    dscp_binary, dscp_hex,                        // "101110", "0x2E"
    tos, tos_hex, tos_binary,                     // 184, "0xB8", "10111000"
    precedence, precedence_name,                  // 5, "CRITIC/ECP" (RFC 791)
    ecn, ecn_binary, ecn_name, ecn_meaning,       // 0, "00", "Not-ECT" | "ECT(1)" | "ECT(0)" | "CE" (RFC 3168)
    phb, typical_use?,                            // from the dscp reference table; typical_use null when unnamed
    notes: [string] }

struct VoiceBandwidth { codec: VoiceCodec, link: VoiceLink, calls, crtp,
    packet_ms, packets_per_second,                // 20, 50.0   (30 → 33.333)
    payload_bytes,
    packet: [{name, bytes}], packet_bytes,        // layer 2, IP/UDP/RTP or cRTP, payload — and their sum
    per_call_bps, total_bps, per_call, total,     // 11600, 116000, "11.6 kbps", "116 kbps"
    vad?: { saving_pct, per_call_bps, total_bps, per_call, total, enough_calls, caveat },
    arithmetic: [string], notes: [string], sources: [string] }
struct VoiceCodec { id, name, bit_rate_bps, sample_bytes, sample_ms, packet_ms, payload_bytes, mos? }
struct VoiceLink  { id, name, overhead_bytes, parts, crtp, source, note }
```

* **`tools.qos.marking`** — `as` is required: the same number is a different
  marking as a DSCP, a ToS byte and a precedence, so it is never guessed.
  `value` may be a JSON number or text (`46`, `"0x2e"`, `"0b101110"`, `"ef"`).
  `dscp` takes 0–63 or a code point name; `name` a name only; `tos` 0–255,
  whose bottom two bits come back as the ECN field; `precedence` 0–7 or
  `routine` … `network`, answered with its class selector and a note that
  the other bits were not given. A number out of the form's range is
  `invalid` — `184` as a DSCP is not quietly read as the ToS byte.
* **`tools.qos.voice`** — `codec` and `link` are ids of the reference tables
  `voice_codecs` and `voice_links` (below), in any common spelling (`G.729A`,
  `g729r8`); `link: "custom"` takes the layer-2 bytes in `l2_bytes` (0 to
  256), and `l2_bytes` with any other link is refused. `calls` is required,
  1 to 1 000 000. `packet_ms` is the voice per packet when not the codec's
  default; it must hold whole samples and be at most 240 ms. The figure is
  one direction of the call, voice packets only. `per_call_bps` and
  `total_bps` are always the full figure:
  `vad: true` adds the `vad` object — the source's "up to 35 %", with
  `enough_calls` (more than 24) and its warning — beside it and never
  instead of it. Its figures are 65 % of the `per_call_bps` and `total_bps`
  shown, so the last line of `arithmetic` multiplies out. `crtp: true` on a
  link whose row says `crtp: false` (`ethernet`, `dot1q`, `wlan`, `mpls`:
  the sources have no compressed RTP there) is `invalid`, and the message
  points to `custom`.
* Every codec and link figure is copied from a published Cisco table and the
  result's `sources` names it; `docs/dev/spec-tools.md` ("Data") lists the
  two documents, where they disagree, and what is left out (ATM, SRTP, IPv6,
  codecs neither tabulates).
* `tools.reference` gains two tables: `voice_codecs` (aliases `codecs`,
  `voice`) and `voice_links` (aliases `links`, `voice_overheads`), whose rows
  are `VoiceCodec` and `VoiceLink`. A picker is built from them; `custom` is
  not a row because it has no figure of its own.

## Probes from this machine (contract)

Local access rules: in server mode these run on the server, which is what
the user means ("can the NOC server reach it"); the result always says where
it ran (`from: "this computer" | "the Nocturne server"`).

| Method | Params → Result |
|---|---|
| `probe.ping` | `{host, count?, interval_ms?, timeout_ms?, payload?}` → `{job_id}`; echoes stream as `probe.ping.echo {job_id, echo}`; job result = `PingReport`. `count: 0` = until cancelled (cap 1 hour). |
| `probe.traceroute` | `{host, max_hops?, probes?, timeout_ms?, resolve_names?}` → `{job_id}`; hops stream as `probe.trace.hop`; result `TraceReport` |
| `probe.ports` | `{host, ports: "22,80,443" | "top100" | "common", timeout_ms?, banners?}` → `{job_id}`; open ports stream as `probe.port`; result `[PortResult]` |
| `probe.sweep` | `{targets, …}` → `{job_id}` (same safety rules as discovery: private ranges unless `allow_public`, host cap) |
| `probe.dns` | `{name, kind?, server?}` → `DnsAnswer` |
| `probe.whois` | `{query}` → `WhoisAnswer` |
| `probe.tls` | `{host, port?, sni?}` → `TlsReport` |
| `probe.http` | `{url, method?, headers?, follow_redirects?, verify_tls?}` → `HttpReport` |
| `probe.ntp` | `{server}` → `NtpReport` |
| `probe.local` | → `{interfaces, gateways, neighbours}` |
| `probe.wol` | `{mac, broadcast?}` → `{sent_to: [..]}` |

## Probes from a device (contract)

`probe.device.ping {device_id, target, count?, source?, vrf?}` and
`probe.device.traceroute {device_id, target, …}` → `{command, output,
summary?: {sent, received, loss_pct, min_ms?, avg_ms?, max_ms?}}` — builds
the platform's own ping/traceroute command for the device's profile (only
for platform families you can build it correctly for: IOS-like, RouterOS,
Junos, Linux, EOS/NX-OS, VRP/Comware; others → `unsupported`), validates the
target (address or hostname characters only — this text goes into a device
command line), runs it through `connect::exec` (so the platform policy still
decides whether it is read-only) and parses the summary when the format is
recognised. "Can the core switch reach it?" is a different question from
"can I?", and usually the right one.

## TFTP server (contract)

For firmware and config transfers to gear that speaks nothing else:
`tftp.start {root, port?: 6969, allow_write?: false, allow_overwrite?:
false}` → `{address}` (local access only; `root` must be an existing
directory; port 69 only when it can be bound), `tftp.stop`, `tftp.status` →
`{running, address?, root?, transfers: [..]}`; transfer events as
`tftp.transfer {..TftpEvent}`. Stops with the app.

## Tests

Every calculator method: one happy path and one bad-input path through
`app.rpc` (values hand-checked, not copied from the engine's output) —
the point is the façade's wiring, shapes and error mapping, not re-testing
the engine. Probes against loopback fixtures (a TCP listener, a UDP DNS
responder, the in-crate TFTP server with a client written in the test,
ping to 127.0.0.1 where permitted else skipped); streaming events arrive in
order and the job result matches; cancel stops a continuous ping; public
target refusal; device ping/traceroute command building per platform family
with injection attempts (`8.8.8.8; reload`, backticks, newlines) refused,
and output parsing from recorded outputs; TFTP lifecycle and the root
confinement.
