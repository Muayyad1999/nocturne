# Status

What is built, and how it has been checked. Written to be read sceptically:
where something has only been tested against the simulator or against
recorded device output, it says so, and where a part is not written yet it
says that rather than nothing.

Every count here is a number that passed on this machine, not an estimate.
**2,542 Rust tests across 163 targets, 864 interface tests and 186
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
| `nocturne-monitor` | **done** | 125 | Polling, time series, alert rules with hysteresis and damping, maintenance windows, five notification channels. Rates are computed from device time, never the poller's clock. |
| `nocturne-assistant` | **done** | 75 | 43 tools behind a 45-method allow-list the model cannot name, redaction at the gate, approvals, four provider kinds including local models. |
| `nocturne-insights` | **done** | 109 | 34 checks with measured precision and recall on a planted campus, and no false positive on a clean 500-device fleet. |
| `nocturne-topology` | **done** | 83 | Link matching with stated confidence, three layouts, path tracing that says why it stopped, host location, five export formats. |
| `nocturne-discovery` | **done** | 50 | Sweep, crawl, preview before anything joins the inventory, an ignore list. |
| `nocturne-automation` | **done** | 94 | The command runner with comparison, templates, playbooks, saved tasks, schedules, event triggers, Ansible interoperability — which states on every run which of the change pipeline's guarantees do not apply to it. |
| `nocturne-logs` | **done** | 68 | A syslog receiver and the log store, queries, histograms, live follow, rules. |
| `nocturne-ops` | **done** | 96 | The overview, incidents with a timeline, shift handover, reports, notes, audit and job history. |
| `nocturne-ipam` | **done** | 48 | A plan and a network kept in different tables that never merge: records written only by a person, observations only by a sync, and `reconcile` reporting what it refuses to decide. |
| `nocturne-toolbox` | **done** | 78 | 52 methods: offline calculators, probes from this machine and from a device, a TFTP server, a syslog staging window, and a history. `reversible` and `decodable` are answered separately, because Cisco type 6 is the first and not the second. |

`nocturne-suite` assembles the core and all fourteen features: **406 RPC
methods across 46 namespaces, with no duplicate name**, and its tests assert
every feature's methods are registered. One of them joins the two halves
nothing else can: a prompt reaches the real assistant, which calls the real
change engine, which configures a simulated switch — and the switch is read
back.

## Interface

**Done** — 864 unit tests, a production build with no warnings, design
tokens, both themes, about fifty components, the app shell (nav, header,
status bar, assistant dock, command palette), and the RPC and event client
with Tauri and HTTP transports.

**No route is a stub.** Every screen in `docs/dev/spec-ui-pages.md` is built:
Overview, Devices, Terminal, Topology, Monitoring (with Charts, Insights and
Logs as tabs), Configuration, Changes, Automation, Lab, IPAM, Tools,
Operations, Assistant and Settings.

186 end-to-end tests in 19 files run the real interface against a real
`nocturne-server`. They build real state rather than fixtures — a real alert
raised by a real rule, a real change planned and applied, real syslog
datagrams on the wire, a real lab collected from — and every one of them
asserts the browser console is clean.

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
- A child profile *replaces* an inherited list rather than extending it, so
  adding one pattern means restating all of them — which is exactly how the
  secret-pattern fault above got in. An append form would remove that class.
- Junos and IOS-XR have the ideal dead-man switch (`commit confirmed`) and
  cannot use it: `config.guard` runs its commands before configuration mode is
  entered, and `commit confirmed` is only valid inside it.
- The licence file is a placeholder that grants nothing. The choice is the
  owner's to make.
