# Status

What is built, and how it has been checked. Written to be read sceptically:
where something has only been tested against the simulator or against
recorded device output, it says so, and where a part is not written yet it
says that rather than nothing.

Every count here is a number that passed on this machine, not an estimate.
**2,548 Rust tests across 163 targets, 864 interface tests and 186
end-to-end tests** pass as this is written, with no failures.

## Legend

| | |
|---|---|
| **done** | implemented, tested, and the tests pass here |
| **partial** | usable, with named gaps |
| **planned** | specified in `docs/dev/`, not yet written |

## Engines

These have no knowledge of the application; each was built against a written
specification in `docs/dev/` and tested against something real.

| Crate | Status | Tests | How it was checked |
|---|---|---|---|
| `nocturne-textfsm` | **done** | 67 | The full upstream ntc-templates corpus (1,897 cases) parses identically to the Python reference, and a regex oracle of 543,000 evaluations taken from CPython agrees. |
| `nocturne-snmp` | **done** | 169 | Live against net-snmp: v1, v2c, and all 43 SNMPv3 authentication × privacy combinations this build supports. Traps and informs decoded from `snmptrap`/`snmpinform`. |
| `nocturne-transport` | **done** | 217 | Live against OpenSSH 9.6 (password, keyboard-interactive, public key, exec, SFTP, jump hosts, keepalives) and against legacy-only algorithm sets, plus GNU telnetd. Back-pressure tests verified to fail when the pump is made naive. |
| `nocturne-tools` | **done** | 181 | Cross-checked against Python's `ipaddress` and the system tools. |
| `nocturne-netprobe` | **done** | 289 | A network-namespace lab (client–router–router–target), cross-checks against nmap and the system tools. Certificate validity is judged from the moment the caller asked about, in both the inspecting and the verifying path, because the platform verifiers on macOS and Windows judge against the *system* clock and cannot be told otherwise. |
| `nocturne-netsim` | **done** | 94 | The demo campus converges, survives the loss of a distribution switch, and all five teaching labs behave as documented. |
| `nocturne-ai` | **done** | 71 | Provider wire formats against local HTTP fixtures; the agent loop and the secret redactor against fixed scripts, including a configuration with one line per rule that must redact identically twice over, and a proof that a careless pattern from any one device type changes nothing. |
| `nocturne-design` | **done** | 70 | Every design in the suite is booted in the simulator and every check it declares is probed. A 240-case matrix of sites × redundancy × WAN × routing, head-counts from 5 to 2,000. Redundancy proved by removing a core. |

## Core

| Crate | Status | Tests | |
|---|---|---|---|
| `nocturne-app` | **done** | 53 | The RPC router and event bus, SQLite store, the encrypted vault, settings, credentials, the device inventory with CSV / SSH-config / Ansible import and export, jobs, the device-type registry, the connection layer (host-key trust, jump hosts, the read-only command gate, probe-based identification) and the collected-data store. |
| `nocturne-drivers` | **done** | 103 | Collectors, platform detection, hierarchical configuration diff and remediation, the command policy, nine built-in profiles. |

### Device types

Nine, and what each claims is deliberately different:

| Profile | Maturity | Captures | Model rows | Command classifications |
|---|---|---|---|---|
| `cisco_ios` (IOS and IOS-XE) | **verified** | 20 | 83 | 54 |
| `mikrotik_routeros` (v6 and v7) | **verified** | 10 | 62 | 84 |
| `cisco_nxos` | community | 8 | 38 | 61 |
| `arista_eos` | community | 7 | 34 | 51 |
| `cisco_xr` | community | 6 | 36 | 54 |
| `hp_procurve` / Aruba ProVision | community | 5 | 22 | 63 |
| `juniper_junos` | community | 5 | 21 | 116 |
| `linux` | community | 3 | 20 | 87 |
| `generic` | community | — | — | 28 |

Together 64 capture cases over 316 model rows, 339 prompt lines, 598 command
classifications, 203 error samples and 134 detection cases. Every capture is
a verbatim copy of real device output from the ntc-templates corpus, cited
file by file; the running-configuration fixtures for the seven community
profiles were written by hand, and say so.

A tenth device type, `nocturne_sim_host`, is shipped by `nocturne-lab` rather
than by the profile pack: it is what a simulated end host in a lab is, and it
has no hardware to be verified against.

`verified` means this repository's tests check the profile against recorded
output of that platform. `community` means it was written from documentation
and a real corpus but nobody has pointed it at a device — and `community`
profiles get no write access by default. `docs/profiles.md` is the guide for
adding more; about twenty platforms remain.

**Not verified against real hardware.** Everything above rests on recorded
output and the simulator. Nobody has pointed this at a production switch yet.

## Features

| Crate | Status | Tests | Notes |
|---|---|---|---|
| `nocturne-terminal` | **done** | 79 | Sessions live in the backend with scrollback and re-attach; SFTP tested against a real sshd it starts itself. |
| `nocturne-config` | **done** | 80 | Backups, diff, network-wide search, two built-in hardening policies, intended-state drift. |
| `nocturne-changes` | **done** | 58 | Plan, approve against a plan hash, apply, verify, roll back; risk and protected resources. |
| `nocturne-lab` | **done** | 40 | Virtual labs on the simulator, consoles, exposure as ordinary devices, the digital twin, and the `design.*` surface including `design.export`. |
| `nocturne-monitor` | **done** | 195 | Polling, time series, alert rules with hysteresis and damping, maintenance windows, five notification channels. Rates are computed from device time, never the poller's clock. Per-host service checks (ICMP, TCP, DNS, HTTP, HTTPS, TLS, NTP, SNMP) on the same scheduler, each with its own state, history and alert; a check that *could not run* is a fifth state, never an outage, and tested against servers the tests start on loopback — including certificates generated to expire in nine days and three days ago. |
| `nocturne-assistant` | **done** | 75 | 43 tools behind a 45-method allow-list the model cannot name, redaction at the gate, approvals, four provider kinds including local models. |
| `nocturne-insights` | **done** | 109 | 34 checks with measured precision and recall on a planted campus, and no false positive on a clean 500-device fleet. |
| `nocturne-topology` | **done** | 83 | Link matching with stated confidence, three layouts, path tracing that says why it stopped, host location, five export formats. |
| `nocturne-discovery` | **done** | 50 | Sweep, crawl, preview before anything joins the inventory, an ignore list. |
| `nocturne-automation` | **done** | 94 | The command runner with comparison, templates, playbooks, saved tasks, schedules, event triggers, Ansible interoperability — which states on every run which of the change pipeline's guarantees do not apply to it. |
| `nocturne-logs` | **done** | 68 | A syslog receiver and the log store, queries, histograms, live follow, rules. |
| `nocturne-ops` | **done** | 96 | The overview, incidents with a timeline, shift handover, reports, notes, audit and job history. |
| `nocturne-ipam` | **done** | 48 | A plan and a network kept in different tables that never merge: records written only by a person, observations only by a sync, and `reconcile` reporting what it refuses to decide. |
| `nocturne-toolbox` | **done** | 78 | 52 methods: offline calculators, probes from this machine and from a device, a TFTP server, a syslog staging window, and a history. `reversible` and `decodable` are answered separately, because Cisco type 6 is the first and not the second. |

`nocturne-suite` assembles the core and all fourteen features: **413 RPC
methods across 46 namespaces, with no duplicate name**, and its tests assert
every feature's methods are registered. One of them joins the two halves
nothing else can: a prompt reaches the real assistant, which calls the real
change engine, which configures a simulated switch — and the switch is read
back.

## The ISP edition

One feature crate exists outside that set, for an internet provider rather
than an engineer looking after one network. It is **not in the free build at
all** — not disabled, absent — and `docs/dev/spec-editions.md` says why that
is a distribution decision rather than a licence check in the code.

| Crate | Status | Tests | How it was checked |
|---|---|---|---|
| `nocturne-isp` | **done** | 58 | Subscribers, tariffs, subscriptions, invoices, payments, credit notes, arrears ageing, revenue, usage against what was sold, oversubscription and margin per megabit. 38 of the tests go through `App::rpc`. |

What has been checked about it, specifically:

* **Money cannot become a float.** There is no float field; the deserialiser
  refuses a JSON float (`1000.0` as firmly as `25.5`); no schema column is
  `REAL`, which a test reads back through `pragma_table_info`; and a test
  greps the crate's own source and fails if `f64` appears on a money path.
* **IQD has no minor digits and USD has two**, so `1000` is a thousand dinars
  or ten dollars depending on the currency beside it. Each customer is billed
  in their own currency.
* **A total across currencies is not a number**, so there is nowhere to put
  one: the reports return one amount per currency with no scalar, and a test
  asserts the arrears result has no `total_minor` or `grand_total` field. One
  figure requires rates passed in the request, and the answer says it is a
  conversion rather than the ledger.
* **A payment keeps the rate it settled at.** A 33,000 IQD payment against a
  $25 invoice at 1,320 still reads 1,320 after a later payment uses 1,500.
* **A write-off is never a payment.** A credit note is its own document with
  a required reason, in its own table rather than a negative payment — a
  negative payment would be counted as revenue by every report that sums
  payments. An invoice cleared by one reads `credited`, not `paid`, and one
  part credited with nothing paid is still `issued`, because money is owed
  and nobody has paid any of it. Voiding a credit note puts the debt back.
  There is deliberately **no tax and no refund**: a tax model invented on an
  operator's behalf is worse than none, and this crate has no way to know
  whether money went back out of the business.
* **It is bookkeeping and nothing else.** Arrears are tracked; no customer is
  disconnected. A test greps the crate for every way the core can reach a
  device and for the change pipeline's method names and fails if any appears
  — a billing bug that cuts off a paying customer is a worse failure than a
  late suspension. Usage is read through two method names, both reads.

The edition line itself is checked from both sides: the free build registers
**413** methods and none beginning `isp.` and declares no `isp` settings
section; the same source with `--features isp` registers **447** — the same
413 plus exactly 34. Reading the two binaries agrees: the free one contains
no `isp.` method string and no currency table. A tag builds both and the
release refuses an artefact that is not the edition it is named for.

The screens are built (`ui/src/pages/isp/`): customers, invoices, payments,
arrears, the four reports, and the tariffs and links, with a page per
customer. They keep the edition's rules rather than restating them — an
amount is formatted from its own currency's minor digits and entered as whole
minor units, a total across currencies is several figures and there is no
function in the screen's code that could add them, a row with no usage figure
says which of the nine nothings it is in words, a billing run is a preview
until it is explicitly told otherwise, and nothing anywhere offers to
disconnect a customer. The free edition shows no sign of any of it: the
navigation item declares the method it needs (`isp.customers`) and is left out
when the build does not answer it, which is checked against a method list with
the `isp.*` entries taken out of it.

Checked: 57 unit tests over the screens' React-free modules (money, coverage,
ageing, invoice lifecycle, the words about an account's state) and 9
end-to-end tests against a real `nocturne-server --features isp` seeded
through `/api/rpc` — a dinar customer and a dollar customer shown side by side
and never summed, a run previewed with nothing written, a customer whose
interface nobody has collected saying so in words, an issued invoice that
offers no edit, a void-and-reissue, and a payment taken in dinars against a
dollar invoice keeping the rate it was quoted at. Every screen was looked at
in both themes; that found a duplicated page heading, a customer list that
jumped back to the first page a moment after it opened, truncated coverage
sentences, a device shown by its id, and an invoice row that could only be
opened by double-clicking it (`ui/src/pages/isp/NOTES.md`).

Driven by hand through a running ISP server, not only through its own tests:
two customers, one billed in dinars and one in dollars; a tariff priced *per
megabit* in each; a billing run that previewed and issued nothing; then an
invoice each. 100 Mbit at 15,000 IQD came to `1500000` and 50 Mbit at
\$12.00 came to `600.00` — the same field, formatted to its own currency's
digits. A 790,000 IQD bank transfer against the \$600 invoice at a rate of
1,320 settled **\$598.48**, said `settled_exact: false` because it had to
round, and left exactly **\$1.52** outstanding; arrears then aged 1,500,000
IQD and \$1.52 in separate reports, each with its own buckets, and the
result carries no field in which a total across them could be put. A payment
with no method was refused ("a payment with no method cannot be reconciled
against anything"), and so was one against an invoice still in draft.

## Interface

**Done** — 970 unit tests, a production build with no warnings, design
tokens, both themes, about fifty components, the app shell (nav, header,
status bar, assistant dock, command palette), and the RPC and event client
with Tauri and HTTP transports.

**No route is a stub.** Every screen in `docs/dev/spec-ui-pages.md` is built:
Overview, Devices, Terminal, Topology, Monitoring (with Charts, Insights and
Logs as tabs), Configuration, Changes, Automation, Lab, IPAM, Tools,
Operations, Assistant and Settings — and, in the ISP edition only, ISP.

203 end-to-end tests in 20 files run the real interface against a real
`nocturne-server`. They build real state rather than fixtures — a real alert
raised by a real rule, a real change planned and applied, real syslog
datagrams on the wire, a real lab collected from — and every one of them
asserts the browser console is clean.

One of them is not yet trustworthy. `terminal.spec.ts`'s "splits into panes
and can type into both at once" passed three times out of three on its own
and failed once in a full-suite run, so it is timing-sensitive under load
rather than broken — which is exactly the failure that wastes a CI run and
teaches people to press re-run. It is written down here rather than left for
somebody to rediscover at a red build.

Every screen was also looked at, in both themes, at 1440×900 and 1280×720
with the assistant dock open and closed. That found about fifty things no
test would have: a panel one word wide, a timeline rendering one word per
line, answered pings drawn as one-pixel slivers, a proven zero drawn like an
unmeasured gap, and a handover box pre-filled with invented example notes.

## Shells

| | Status | Notes |
|---|---|---|
| `nocturne-server` | **done** | 134 tests. HTTP + WebSocket over the RPC surface, token authentication, Host and Origin checks, TLS with rustls, the interface embedded in the binary, and a `call` subcommand for scripting. |
| Desktop (Tauri 2) | **done** | 6 tests. Starts the backend in process, one `rpc` command and one event channel, icons and bundling configured. Run here under a virtual display: the window opens at 1440×900, renders the interface, and the status bar reports the in-process engine online. |

## Checked by hand, not only by its own tests

The server holds the credentials of a network, so its refusals were tried
directly against a running instance:

| | |
|---|---|
| A public bind without TLS | refused, with the reason and three ways forward |
| `server.token`, `vault.key` | mode `600` |
| No token / a wrong token / the right one | `401` / `401` / `200` |
| A request carrying another site's `Origin` | `403` |
| A request carrying another `Host` (DNS rebinding) | `403` |
| `/api/health` without a token | answers, and says nothing about the data |
| Every response | `nosniff`, `DENY`, `no-referrer`, and a strict content-security policy |
| A decoded secret or a generated password | appears nowhere in what the server writes to disk — checked by searching all of it |
| A documented address nothing has ever answered on | reported `documented: false, observed: false`, never "free" |

TLS is rustls with the ring provider and nothing else: `aws-lc-rs`,
`aws-lc-sys`, `openssl-sys` and `native-tls` are all absent from the
dependency graph, and exactly one version of rustls is linked. The project
therefore builds on Windows, macOS and Linux without a C toolchain beyond
what ring and SQLite need.

## Continuous integration

CI runs on every push: formatting, clippy with `-D warnings`, the whole test
suite on Linux, macOS and Windows, the interface's unit tests and build, the
end-to-end suite against a real server, and a compile of the desktop shell.
Installers and server binaries are built on a tag, and the release workflow
starts the server it just built to check that it runs and is carrying the real
interface rather than the fallback page.

It has found real faults rather than only confirming ours, which is the point
of it:

- a build break on a platform without raw sockets;
- certificates too long-lived for macOS, then an extended-key-usage rule, then
  the discovery that a verified handshake was judging validity by the system
  clock rather than the moment it was asked about;
- a closed UDP port reported differently on Windows, and then a junk flood
  that could stall the TFTP server for ten seconds there, because every
  undeliverable reply came back as a receive error and the loop slept on each
  one;
- a path alias that resolved in a working tree but not in a fresh checkout;
- **a real secret reaching a model.** One profile's secret pattern captured a
  key *number* from another vendor's configuration; because the placeholder
  table is numbered in insertion order, that renumbered every placeholder
  after it, so a placeholder the model wrote restored a different secret —
  into a line nothing re-redacts on the way back.

## Known gaps worth stating plainly

- No part of this has talked to real production hardware.
- Seven of the nine device profiles are `community`: written from
  documentation and a real capture corpus, never run against a device.
- `arista_eos` has no `neighbours` collector — no EOS capture or template
  exists — so the topology sees nothing on an EOS device. Its
  `show interfaces status` step, and ProCurve's, are the least-verified things
  in the pack: the only captures available come from different devices than
  the rows they enrich, so no expectation has ever seen a value they wrote.
  The collector now *says* so — a step offered rows that places none is a
  `Partial` problem naming the command and the key, and both capture cases
  expect it — but `mode`, `vlan`, `duplex` and `speed_bps` on those two
  platforms remain unproven.
- A collector expression that fails at *runtime* is silently left out rather
  than reported. An unknown filter is caught when a profile loads, and a
  genuine evaluation error is counted — but an unknown *name* is simply
  absent, which is deliberate (a template that does not capture a field
  should leave it empty) and therefore makes a typo indistinguishable from an
  absent field. Catching it properly means checking a map's names against the
  parser's at load time.
- Junos and IOS-XR have the ideal dead-man switch (`commit confirmed`) and
  cannot use it: `config.guard` runs its commands before configuration mode is
  entered, and `commit confirmed` is only valid inside it.
- Nocturne is proprietary, by the owner's decision: nobody else may use,
  copy or modify it without asking. That is not a gap, but it is worth
  knowing before you read the rest as though it were an open-source project.
  The bundled third-party components keep their own permissive licences
  (`THIRD-PARTY-NOTICES.md`), which also records what those notices require
  should the licensing ever change.
