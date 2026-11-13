# Nocturne architecture

Nocturne is one Rust application that does the daily work of a network
operations engineer: terminal, discovery, monitoring, configuration backup
and compliance, safe configuration changes, automation, topology, IP address
management, a virtual lab, and an AI assistant that can use all of it. It runs
as a desktop app (Tauri shell around a web UI) and, with the same backend, as
a headless server used from a browser.

This document is the map. Each box below has its own specification under
`docs/dev/`.

## Shape

```
                 ┌──────────────── UI (React + TypeScript, ui/) ────────────────┐
                 │  shell · devices · terminal · topology · monitoring · config │
                 │  changes · automation · lab · IPAM · tools · ops · assistant │
                 └───────────────▲──────────────────────────────▲──────────────┘
        Tauri IPC (desktop)      │  rpc(method, params)          │  events {topic, payload, at}
        HTTP + WebSocket (server)│                               │
┌────────────────────────────────┴───────────────────────────────┴───────────────┐
│ apps/desktop (Tauri 2)                      crates/nocturne-server (axum)       │  thin adapters
├─────────────────────────────────────────────────────────────────────────────────┤
│ crates/nocturne-suite — assembles the core and every feature: start(config)      │
├─────────────────────────────────────────────────────────────────────────────────┤
│ Feature crates (each: pub fn init(&mut AppBuilder); talk only via RPC + events)  │
│  terminal  discovery  monitor  logs  insights  topology  ipam  toolbox           │
│  config    changes    automation    lab    assistant    ops                      │
├─────────────────────────────────────────────────────────────────────────────────┤
│ Core — crates/nocturne-app                                                       │
│  App (RPC router, event bus, tasks) · Store (SQLite: documents, logs, series,    │
│  config versions) · Vault (XChaCha20-Poly1305, optional master password) ·       │
│  settings · credentials · inventory · profiles · connect · collect · jobs        │
├─────────────────────────────────────────────────────────────────────────────────┤
│ Engines (no app knowledge)                                                       │
│  drivers   device profiles (YAML), CLI session driver, collectors, detection,    │
│            hierarchical config diff/remediation, command policy                  │
│  transport SSH (incl. legacy crypto, jump hosts, SFTP), Telnet, serial, local    │
│  snmp      v1/v2c/v3 client, trap decoding, OID names                            │
│  textfsm   TextFSM engine + bundled ntc-templates (≈980 parsers)                 │
│  netprobe  ping, sweep, traceroute, port scan, DNS, WHOIS, TLS, HTTP, NTP,       │
│            TFTP/syslog/flow servers                                              │
│  tools     subnet/VLSM maths, MAC/OUI, secret decoders, converters, references   │
│  netsim    deterministic network simulator with an IOS-style CLI                 │
│  design    requirements → complete network design, proven in the simulator       │
│  ai        model providers (Anthropic, OpenAI-compatible, Ollama), agent loop,   │
│            secret redaction                                                      │
└─────────────────────────────────────────────────────────────────────────────────┘
```

## Principles

**One RPC surface, one event stream.** Everything a user can do is a method
on `App::rpc(method, params)`; everything they should see happen is an event.
The desktop shell forwards both over Tauri IPC, the server over HTTP and a
WebSocket, tests call the same methods directly. There is no second API for
the assistant or for scripting — `nocturne-server call devices.list` and the
AI's tools go through the same door as the UI.

**Features do not link to each other.** A feature crate depends on the core
and the engines it needs. When the configuration feature wants a fix applied
it calls `changes.create` by name; when the overview wants alert counts it
calls `alerts.summary` and copes when that method does not exist. This keeps
compile units small, lets features be built and tested in isolation, and
makes "what can the assistant reach" an explicit list of method names.

**A device type is data.** Supporting a platform means a YAML profile: how
its prompts look, how to disable paging, how to enter configuration mode and
save, which commands are read-only and which are dangerous, how to turn its
`show` output into the common model (through bundled TextFSM templates,
regexes, key/value or JSON parsers and small expressions), and how to detect
it. Users and the assistant can add profiles without rebuilding. See
`docs/profiles.md`.

**Nothing changes a device outside the change pipeline.** Reads go through
`connect::Cli::read`, which sends a command only when the platform's policy
classifies it as read-only (default deny). Configuration goes through
`nocturne-changes`: the exact commands are computed (hierarchical diff with
make-before-break ordering), the resulting configuration is predicted, risk
and protected resources are assessed, a human approves that exact plan, and
the apply is backup → dead-man switch → send → verify → rollback on failure.
The AI assistant can draft and plan changes; whether it may apply one is
decided by policy inside the change engine, never by the model.

**No fabricated data.** A read that failed is a recorded failure. "Unknown"
and "empty" are different values everywhere, and reports state their
coverage.

**Secrets stay in the vault.** Credentials are encrypted at rest; they are
not in documents, events, logs, job output or anything sent to a model.
Device configuration is redacted (stable placeholders) before a model sees
it, and by default before a human does.

**Lab devices are devices.** A simulated router is an inventory device with
`transport: sim`. Monitoring, backup, the change pipeline and the assistant
work on it exactly as on hardware — which is how Nocturne can be tried,
demonstrated and tested end to end without a single real switch.

## Why not Ansible as the engine?

Ansible's ideas are here — idempotent configuration by diffing intended
against running state, check mode with a diff, templates, playbooks — but
not Ansible itself: it has no native Windows control node, is a heavyweight
Python dependency for a desktop application, and its network collections are
GPL-licensed. Nocturne instead interoperates: it imports and exports Ansible
inventories, exports a planned change as an Ansible playbook, and can run an
existing playbook when `ansible-playbook` is installed (credentials passed
only through the environment).

## Data

A single SQLite database in the data directory:

| What | How |
|---|---|
| Documents (devices, credentials metadata, changes, labs, policies, …) | JSON by collection + id |
| Secrets | encrypted blobs, key in `vault.key` (optionally wrapped with Argon2id from a master password) |
| Logs (syslog, traps, audit, events) | append-only table with indexed stream / device / time |
| Time series | raw samples with 5-minute and 1-hour roll-ups, retention per tier |
| Configuration versions | compressed, de-duplicated by hash |

## Process model

Async Rust on tokio. The core owns the event bus and a cancellation token
every background task observes; long operations are jobs (progress, log,
cancel, history). Device access is serialised per device (one automated CLI
session at a time by default) and bounded globally by each feature. TLS is
rustls with the ring provider throughout — no OpenSSL, no C toolchain beyond
what ring and SQLite need — so the same code builds on Windows, macOS and
Linux.

## Repository layout

```
crates/            engines, core, features, suite, server (one crate each)
apps/desktop/      Tauri 2 shell
ui/                React + TypeScript interface (also embedded in the server)
docs/              ARCHITECTURE.md, profiles.md (user guide to device types)
docs/dev/          specifications: spec-*.md (engines, UI, server), features/*.md
```

## Verification

Engines are tested against the real thing where one exists on a build
machine: the SSH transport against OpenSSH (modern and legacy-only
configurations), SNMP against net-snmp (every v3 auth/priv combination), the
TextFSM engine against the upstream ntc-templates corpus (1,897 cases) and
the Python reference implementation, the simulator's output against the same
templates. Device profiles are tested against recorded device output.
Features are tested through the RPC surface against a stateful fake device
and the simulator; the UI is tested with Playwright against the real backend
in server mode.
