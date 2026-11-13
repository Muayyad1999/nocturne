# Feature: discovery (`nocturne-discovery`)

Turn "we have a network" into an inventory: sweep address ranges, identify
what answers (SNMP, SSH banner, open ports), crawl outwards from known
devices through their LLDP/CDP neighbours, and add what the user picks.

Engines: `nocturne-netprobe` (`sweep`, `tcp`, `dns`, `local`),
`nocturne-snmp`, `nocturne-tools` (MAC vendor lookup), `nocturne-drivers`
(`detect::{from_snmp, from_banner}` or `Registry::detect_*`), core
`collect`, `inventory`, `jobs`.

Scanning is intrusive on someone else's network. Nothing scans unless a user
(or a schedule a user created) asks; targets are validated; rates are bounded.

## RPC (contract)

| Method | Params → Result |
|---|---|
| `discovery.suggest` | → `{networks: [{cidr, interface, hosts}], gateways: [..]}` — the workstation's own connected subnets, as starting points (local access only; server mode returns the server's) |
| `discovery.scan` | `ScanRequest` → `{job_id, scan_id}` |
| `discovery.scans` | `{limit?}` → `[{id, targets, started_at, finished_at?, state, found, identified, new}]` |
| `discovery.scan_get` | `{scan_id, only_new?, query?}` → `{scan, hosts: [Found]}` (also while running — hosts appear as they answer; event `discovery.found`) |
| `discovery.add` | `{scan_id, addresses: [..], defaults: {site?, role?, tags?, credential_id?, transport?, monitor?}}` → `{created: [device_id], skipped: [{address, reason}]}` |
| `discovery.ignore` | `{addresses}` / `discovery.ignored` / `discovery.unignore` — addresses the user never wants offered again |
| `discovery.scan_delete` | `{scan_ids}` |
| `discovery.crawl` | `{seed_device_ids?: all, depth?: 2, credential_ids?: [..], add?: false}` → `{job_id, scan_id}` |

```rust
struct ScanRequest {                               // #[serde(default)]
    targets: String,                 // "10.0.0.0/24, 10.0.1.5-10.0.1.40, core-sw.example"
    icmp: bool,                      // default true
    ports: String,                   // TCP ports to check on live hosts, default "22,23,80,443,161,8291" ("" = none)
    snmp_credential_ids: Vec<String>,// communities / v3 users to try, in order
    identify_ssh: bool,              // read the SSH identification string (no login) — default true
    resolve_names: bool,             // reverse DNS — default true
    max_hosts: u32,                  // default 4096, hard cap 65536
    rate: u32,                       // probes per second, default 500
}
struct Found {
    address, alive: bool, rtt_ms?, via,               // "icmp" | "tcp/22" | "arp"
    hostname?, mac?, vendor?,                         // vendor from the MAC OUI
    open_ports: [{port, service, banner?}],
    snmp: Option<{credential_id, sys_name, sys_descr, sys_object_id, uptime_s?}>,
    ssh_banner?,
    profile?: {id, name, method: snmp|banner, evidence},   // detected device type, if any
    suggested: {name, transport: ssh|telnet|none, role?},  // what `discovery.add` would create
    device_id?,                                       // already in the inventory (matched by address or by a collected interface IP)
    ignored: bool,
}
```

## Behaviour

* `targets` are expanded with `sweep::expand_targets` and refused when they
  exceed `max_hosts`, include multicast/broadcast/unspecified addresses, or
  are not private/link-local **unless** `allow_public: true` is passed
  explicitly (and the job log says so): sweeping the Internet from a NOC
  workstation is never what the user meant.
* Pipeline per live host, bounded concurrency: port check → SNMP (first
  credential that answers; `sysDescr/sysObjectID/sysName/sysUpTime`) →
  SSH identification string when 22 is open → reverse DNS → MAC from the
  workstation's neighbour table + OUI vendor. Each step failing is normal;
  the record says what is known and how.
* Device type: `detect::from_snmp` then `from_banner`. No match → no profile
  (the UI shows "unknown device", the terminal still works with `generic`).
  Role suggestion from sysDescr/profile where honest (a profile may say what
  it is; do not guess from open ports).
* `discovery.add`: creates devices through `inventory::create` (name =
  sysName, else reverse-DNS short name, else the address; duplicates by
  name get the address appended), sets `profile`, `monitor.snmp` + the SNMP
  credential that answered, transport `ssh` when 22 is open else `telnet`
  when 23 is open else `none`. It does not log in anywhere.
* **Crawl**: for each seed device, `collect::run` neighbours (and
  `arp` when `depth > 1` is asked for edge devices only if cheap); each
  neighbour's management address not yet in the inventory becomes a
  candidate, probed like a scan host; with `add: true` candidates that are
  positively identified as network devices are added (with the seed's
  credential and site) and crawled in turn until `depth`. Never crawls into
  a device it could not identify, never tries credentials the user did not
  list, records the path (`via: [seed, neighbour…]`) for every found device.
* Results are stored per scan (documents; keep the last 20 scans).
* Events: `discovery.found {scan_id, host: Found}` (throttled),
  `discovery.finished {scan_id, summary}`.

## Tests

Target validation (ranges, caps, public refusal, IPv6 literal); the pipeline
against local fixtures: a TCP listener that prints an SSH banner, the
in-crate SNMP `Agent` answering sysDescr of a known platform (register a test
profile with `detect:` rules), a dead address; `device_id` matching by
address and by collected interface IP; `discovery.add` naming, duplicates,
transport choice, monitor/SNMP wiring, ignored addresses; crawl over three
fake devices whose collected neighbours point at each other (depth limits,
no loops, path recording, `add: false` vs `true`) — give the fakes canned
`show cdp neighbors detail` / `show lldp neighbors detail` outputs and a
test profile that parses them; cancellation mid-scan; rate limiting (count
probes per second with an injected probe layer).
