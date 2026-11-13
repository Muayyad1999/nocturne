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
icmp_types|cable_pinouts|wifi_channels|…, query?}` · `tools.regex {pattern,
text, flags}` (matches with groups; for testing profile and log patterns) ·
`tools.diff {a, b, context?}` (text diff) · `tools.password {length,
classes, count}` and `tools.passphrase` (CSPRNG) · `tools.keys.generate
{kind, comment}` · `tools.cron {expr}` (next runs — only if the engine has
it) · `tools.list` → the catalogue `[{id, title, group, description,
offline: bool}]` so the UI and the assistant can discover tools.

Bad input returns `invalid` with the engine's message (which says what was
wrong with the input) — never a default answer.

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
