# Status

What is built, and how it has been checked. Written to be read sceptically:
where something has only been tested against the simulator or against
recorded device output, it says so, and where a part is not written yet it
says that rather than nothing.

A test count is the number that passed on this machine, not an estimate.
**2,068 Rust tests and 333 interface tests** pass as this is written.

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
| `nocturne-snmp` | **done** | 157 | Live against net-snmp: v1, v2c, and all 43 SNMPv3 authentication × privacy combinations this build supports. Traps and informs decoded from `snmptrap`/`snmpinform`. |
| `nocturne-transport` | **done** | 217 | Live against OpenSSH 9.6 (password, keyboard-interactive, public key, exec, SFTP, jump hosts, keepalives) and against legacy-only algorithm sets, plus GNU telnetd. Back-pressure tests verified to fail when the pump is made naive. |
| `nocturne-tools` | **done** | 188 | Cross-checked against Python's `ipaddress` and the system tools. |
| `nocturne-netprobe` | **done** | 298 | A network-namespace lab (client–router–router–target), cross-checks against nmap and the system tools. Its TLS fixtures are generated around the real clock, because the platform verifiers on macOS and Windows judge a certificate against the system clock and refuse one valid for more than 398 days. |
| `nocturne-netsim` | **done** | 94 | The demo campus converges, survives the loss of a distribution switch, and all five teaching labs behave as documented. |
| `nocturne-ai` | **done** | 65 | Provider wire formats against local HTTP fixtures; the agent loop and the secret redactor against fixed scripts, including a configuration with one line per rule that must redact identically twice over. |
| `nocturne-design` | **done** | 54 | Every design in the suite is booted in the simulator and every check it declares is probed. A 240-case matrix of sites × redundancy × WAN × routing, head-counts from 5 to 2,000. Redundancy proved by removing a core. |

## Core

| Crate | Status | Tests | |
|---|---|---|---|
| `nocturne-app` | **done** | 53 | The RPC router and event bus, SQLite store, the encrypted vault, settings, credentials, the device inventory with CSV / SSH-config / Ansible import and export, jobs, the device-type registry, the connection layer (host-key trust, jump hosts, the read-only command gate, probe-based identification) and the collected-data store. |
| `nocturne-drivers` | **done** | 103 | Collectors, platform detection, hierarchical configuration diff and remediation, the command policy, four built-in profiles. |

Device types shipped: `cisco_ios` (IOS and IOS-XE, verified, 9 kinds collected,
20 capture cases), `mikrotik_routeros` (v6 and v7, verified, 9 kinds, 10 capture
cases), `linux` (community, 6 kinds), `generic` (community — logs in and probes
an unidentified device). Together 33 capture cases over 165 model rows, 158
prompt lines, 253 command classifications and 63 detection cases. About
twenty-five more platforms are planned; `docs/profiles.md` is the guide.

**Not verified against real hardware.** Everything above rests on recorded
output and the simulator. Nobody has pointed this at a production switch yet.

## Features

| Crate | Status | Tests | Notes |
|---|---|---|---|
| `nocturne-terminal` | **done** | 79 | Sessions live in the backend with scrollback and re-attach; SFTP tested against a real sshd it starts itself. |
| `nocturne-config` | **done** | 80 | Backups, diff, network-wide search, two built-in hardening policies, intended-state drift. |
| `nocturne-changes` | **done** | 58 | Plan, approve against a plan hash, apply, verify, roll back; risk and protected resources. |
| `nocturne-lab` | **done** | 39 | Virtual labs on the simulator, consoles, exposure as ordinary devices, the digital twin, and the `design.*` surface. |
| `nocturne-monitor` | **done** | 123 | Polling, time series, alert rules with hysteresis and damping, maintenance windows, five notification channels. Rates are computed from device time, never the poller's clock. |
| `nocturne-assistant` | **done** | 75 | 43 tools behind a 45-method allow-list the model cannot name, redaction at the gate, approvals, four provider kinds including local models. |
| `nocturne-insights` | **done** | 93 | 34 checks with measured precision and recall on a planted campus, and no false positive on a clean 500-device fleet. |
| `nocturne-topology` | **done** | 83 | Link matching with stated confidence, three layouts, path tracing that says why it stopped, host location, five export formats. |
| `nocturne-ops` | **partial** | — | Written but not finished or wired in. |
| `nocturne-discovery`, `-automation`, `-logs` | being built | — | Specified under `docs/dev/features/`. |
| `nocturne-ipam`, `-toolbox` | **planned** | — | Specified under `docs/dev/features/`. |

`nocturne-suite` assembles the core with terminal, config, changes, lab,
topology, monitor, insights and assistant, and its test asserts every one of
their methods is registered.

## Interface

**Foundation done** — 297 unit tests, a production build with no warnings,
design tokens, both themes, about fifty components, the app shell (nav,
header, status bar, assistant dock, command palette), and the RPC and event
client with Tauri and HTTP transports.

| Screen | Status | |
|---|---|---|
| Devices | **done** | List, facets, import preview, export, connection test with the stage that failed, the device page with three distinct kinds of "nothing here". 2,000 devices stay usable. |
| Settings | **done** | Sections filtered by `app.methods`; vault, host keys, credentials, device types with a validating YAML editor. |
| Terminal | **done** | Panes, tabs, SFTP, snippets, broadcast, and re-attach proved with no gap and nothing twice. |
| Monitoring, Changes, Assistant, Lab | being built | |
| Overview, Topology, Config, Insights, Automation, IPAM, Tools, Ops | route stubs | Each says what it will be; none shows invented data. |

36 end-to-end tests run the real interface against a real `nocturne-server`:
8 shell, 13 terminal, 15 devices and settings. Every one asserts the browser
console is clean.

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

TLS is rustls with the ring provider and nothing else: `aws-lc-rs`,
`aws-lc-sys`, `openssl-sys` and `native-tls` are all absent from the
dependency graph, and exactly one version of rustls is linked. The project
therefore builds on Windows, macOS and Linux without a C toolchain beyond
what ring and SQLite need.

## Verified together

The whole stack has been run end to end on this machine: the server with the
real interface embedded, a credential and a device created over HTTP, the
device types listed, and the application opened in a browser — no console
errors. A design generated from requirements becomes a lab that boots, whose
devices appear in the inventory and answer on the connection layer like
hardware.

## Continuous integration

CI runs on every push: formatting, clippy with `-D warnings`, the whole test
suite on Linux, macOS and Windows, the interface's unit tests and build, the
end-to-end suite against a real server, and a compile of the desktop
shell. Installers are built by a separate workflow on a tag.

It has found real faults rather than only confirming ours, which is the point
of it: a build break on a platform without raw sockets, certificates too
long-lived for macOS, a closed UDP port reported differently on Windows, and
a path alias that resolved in a working tree but not in a fresh checkout.

## Known gaps worth stating plainly

- No part of this has talked to real production hardware.
- `nocturne-ops` exists only in part; IPAM and the toolbox are not written.
- Eight screens are still honest stubs.
- The licence file is a placeholder that grants nothing. The choice is the
  owner's to make.
