# Status

What is built, and how it has been checked. Written to be read sceptically:
where something has only been tested against the simulator or against
recorded device output, it says so, and where a part is not written yet it
says that rather than nothing.

Last updated while the first screens were being built.

## Legend

| | |
|---|---|
| **done** | implemented, tested, and the tests pass here |
| **partial** | usable, with named gaps |
| **planned** | specified in `docs/dev/`, not yet written |

A test count is the number that passed on this machine, not an estimate.

## Engines

These have no knowledge of the application; each was built against a written
specification in `docs/dev/` and tested against something real.

| Crate | Status | Tests | How it was checked |
|---|---|---|---|
| `nocturne-textfsm` | **done** | 67 | The full upstream ntc-templates corpus (1,897 cases) parses identically to the Python reference, and a regex oracle of 543,000 evaluations taken from CPython agrees. |
| `nocturne-snmp` | **done** | 157 | Live against net-snmp: v1, v2c, and all 43 SNMPv3 authentication × privacy combinations this build supports. Traps and informs decoded from `snmptrap`/`snmpinform`. |
| `nocturne-transport` | **done** | 217 | Live against OpenSSH 9.6 (password, keyboard-interactive, public key, exec, SFTP, jump hosts, keepalives) and against legacy-only algorithm sets, plus GNU telnetd. Back-pressure tests verified to fail when the pump is made naive. |
| `nocturne-tools` | **done** | 188 | Cross-checked against Python's `ipaddress` and the system tools. One pinned-output test still carries placeholder values. |
| `nocturne-netprobe` | **done** | 297 | A network-namespace lab (client–router–router–target), cross-checks against nmap and the system tools. |
| `nocturne-netsim` | **done** | 94 | The demo campus converges, survives the loss of a distribution switch, and all five teaching labs behave as documented. |
| `nocturne-ai` | **done** | 65 | Provider wire formats against local HTTP fixtures; the agent loop and the secret redactor against fixed scripts. |
| `nocturne-design` | **planned** | — | Specified in `docs/dev/spec-design.md`. |

## Core (`nocturne-app`)

**done** — 49 tests. The RPC router and event bus, SQLite store (documents,
logs, time series, configuration versions), the encrypted vault, settings,
credentials, the device inventory with CSV / SSH-config / Ansible import and
export, jobs, the device-type registry, the connection layer (host-key trust,
jump hosts, the read-only command gate, probe-based identification of unknown
devices) and the collected-data store.

## Device types (`nocturne-drivers`)

**done** for what is there — 103 tests. The collector, platform detection,
hierarchical configuration diff and remediation, the command policy, and four
built-in profiles:

| Profile | Maturity | Collects | Evidence |
|---|---|---|---|
| `cisco_ios` (IOS and IOS-XE) | verified | 9 kinds | 20 capture cases from real recorded output |
| `mikrotik_routeros` (v6 and v7) | verified | 9 kinds | 10 capture cases, 8 of them verbatim real output |
| `linux` | community | 6 kinds | 3 capture cases recorded from a real kernel in a network namespace |
| `generic` | community | none, deliberately | logs in and probes an unidentified device |

Together: 33 capture cases over 165 model rows, 158 prompt lines, 253 command
classifications and 63 detection cases. About twenty-five more platforms are
planned; `docs/profiles.md` is the guide for writing them, and its 14 examples
are loaded by a test.

**Not verified against real hardware.** Everything above rests on recorded
output and the simulator. Nobody has pointed this at a production switch yet.

## Features

| Crate | Status | Tests | Notes |
|---|---|---|---|
| `nocturne-terminal` | **done** | 80 | Sessions live in the backend with scrollback and re-attach; SFTP tested against a real sshd it starts itself. |
| `nocturne-config` | **done** | 80 | Backups, diff, network-wide search, two built-in hardening policies (every rule tested against a hardened and a box-fresh configuration), intended-state drift. |
| `nocturne-changes` | in progress | — | The change pipeline: plan, approve, apply, verify, roll back. |
| `nocturne-monitor` | in progress | — | Polling, time series, alert rules, notifications. |
| `nocturne-lab` | in progress | — | Virtual labs on the simulator. |
| `nocturne-ops` | **partial** | — | Written but not finished or wired in. |
| `nocturne-insights` | **partial** | — | Skeleton only. |
| `nocturne-automation`, `-logs`, `-discovery`, `-topology`, `-ipam`, `-toolbox`, `-assistant` | **planned** | — | Each has a specification under `docs/dev/features/`. |

## Interface

**Foundation done** — 262 unit tests, 9 end-to-end specs plus 2 against a
real server, a production build with no warnings, 46 screenshots in both
themes. Design tokens, both themes, about fifty components, the app shell
(nav, header, status bar, assistant dock, command palette), and the RPC and
event client with Tauri and HTTP transports.

**Screens**: the Devices, Settings and Terminal screens are being built. The
others are honest route stubs that say what the screen will do — not mock
data.

## Shells

| | Status | Notes |
|---|---|---|
| `nocturne-server` | **done** | 123 tests. HTTP + WebSocket over the RPC surface, token authentication, Host and Origin checks, TLS with rustls, the interface embedded in the binary, and a `call` subcommand for scripting. Four integration modules (binary, lifecycle, TLS, UI) are declared but not yet written. |
| Desktop (Tauri 2) | **partial** | Builds and runs the backend in process with one `rpc` command and one event channel; icons and bundling configured. Not yet exercised under a real window here. |

## Checked by hand, not only by its own tests

The server holds the credentials of a network, so its refusals were tried
directly against a running instance:

| | |
|---|---|
| A public bind without TLS | refused, with the reason and three ways forward |
| `server.token`, `vault.key` | mode `600` |
| No token / a wrong token / the right one | `401` / `401` / `200` |
| A request carrying another site's `Origin` | `403` — a page the operator visits cannot use the API |
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
device types listed, and the application opened in a browser — 103 RPC
methods, "Engine online", no console errors.

## Known gaps worth stating plainly

- No part of this has talked to real production hardware.
- The AI assistant is specified and its engine is built, but the feature that
  gives a model tools is not written yet.
- `nocturne-ops` and `nocturne-insights` exist only in part.
- The licence file is a placeholder that grants nothing.
- Continuous integration is configured but has never run, because the
  repository has not been pushed anywhere yet.
