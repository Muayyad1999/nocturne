# Nocturne: guide for contributors and coding agents

One Rust application that does the daily work of a network operations engineer:
terminal, discovery, monitoring, configuration backup and compliance, safe
configuration changes, automation, topology, IPAM, a virtual lab, and an AI
assistant that can use all of it. It runs as a desktop application (Tauri 2
around a web interface) and, with the same backend, as a headless server used
from a browser.

Read `docs/ARCHITECTURE.md` first — it is the map, and every box in it has a
specification under `docs/dev/`. `docs/STATUS.md` says what is actually built
and how it was checked; keep it honest when you change things.

## Non-negotiable rules

* **One RPC surface, one event stream.** Everything a person can do is a method
  on `App::rpc(method, params)`; everything they should see is an event
  `{topic, payload, at}`. The desktop shell forwards both over Tauri IPC, the
  server over HTTP and a WebSocket, tests call them directly. Never add a second
  way in — not for the assistant, not for scripting, not "just for this screen".
* **Features do not link to each other.** A feature crate depends on the core
  and the engines it needs, never on another feature. To reach one, call its
  method by name and cope when it is absent (`unknown_method`). This is what
  keeps the assistant's reach an explicit list of names, and it is why a feature
  can be built and tested alone.
* **Nothing changes a device outside the change pipeline.** Reads go through
  `connect::Cli::read`, which sends a command only when the platform's policy
  classifies it read-only — default deny. Configuration goes through
  `nocturne-changes`: hierarchical diff with make-before-break ordering →
  predicted configuration → risk and protected resources → a human approves
  *that exact plan* (bound to its hash) → backup → dead-man switch → send →
  verify → roll back on failure. Never apply configuration from a router
  handler, a collector, or an AI tool.
* **The model proposes; the pipeline decides.** The assistant may draft and plan
  changes. Whether one may be applied is decided by policy inside the change
  engine, never by the model and never by the prompt.
* **No fabricated data.** A read that failed is a recorded failure. "Unknown"
  and "empty" are different values everywhere, and a report states its coverage.
  Nothing in a crate may return invented, placeholder or demo data — if you want
  something to try the application against, build a lab (below), which is real.
* **Secrets stay in the vault.** Credentials are encrypted at rest
  (XChaCha20-Poly1305) and appear in no document, event, log line, job output,
  error message or model context. A webhook URL that carries a token *is* a
  secret. Before a model sees a configuration it goes through
  `nocturne-ai::redact`; by default it is redacted for a human too.
* **A device type is data.** Supporting a platform means a YAML profile — prompts,
  paging, config mode, which commands are read-only and which are dangerous, how
  to turn `show` output into the common model, how to detect it. See
  `docs/profiles.md`. Do not hard-code a vendor in Rust.
* **Lab devices are devices.** A simulated router is an inventory device with
  `transport: sim`; monitoring, backup, the change pipeline and the assistant
  treat it exactly as hardware. The simulator (`nocturne-netsim`) is
  deterministic: every random value is a pure function of the world's seed and of
  what it describes, so a lab replays identically. Keep it that way — no clock
  reads, no running generator.
* **Nothing is deployed to run a lab.** No containers, no Docker socket, no root.
  A lab is simulated in process.

## Layout

```
crates/
  nocturne-app/        core: RPC router, event bus, tasks, SQLite store, vault,
                       settings, credentials, inventory, profiles, connect,
                       collect, jobs, testing fakes
  nocturne-suite/      assembles the core and every feature: start(config)
  feature crates       each exposes `pub fn init(&mut AppBuilder)`:
                       terminal config changes lab monitor assistant topology
                       insights ops discovery automation logs ipam toolbox
  engines (no app knowledge):
  nocturne-drivers     profiles, CLI session driver, collectors, detection,
                       hierarchical config diff and remediation, command policy
  nocturne-transport   SSH (legacy crypto, jump hosts, SFTP), Telnet, serial
  nocturne-snmp        v1/v2c/v3 client, trap decoding, OID names
  nocturne-textfsm     TextFSM engine + bundled ntc-templates
  nocturne-netprobe    ping, sweep, traceroute, port scan, DNS, TLS, HTTP, NTP…
  nocturne-tools       subnet maths, MAC/OUI, secret decoders, converters
  nocturne-netsim      deterministic simulator with an IOS-style CLI
  nocturne-design      requirements → a network design, proven in the simulator
  nocturne-ai          model providers, agent loop, secret redaction
  nocturne-server      axum: HTTP + WebSocket over the RPC surface, embedded UI
apps/desktop/          Tauri 2 shell (crate `nocturne-desktop`, binary `nocturne`)
ui/                    React 19 + TypeScript + Vite (pages/, components/, api/)
docs/dev/              the specifications; features/ has one per feature crate
```

The workspace's `default-members` is `crates/*`, so a plain `cargo test` skips
the desktop shell and runs on machines without a WebView toolchain.

## Commands

```bash
cargo test
```

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt --all --check
```

Some engine tests talk to real software and skip with a reason when it is
absent. To run them, install `net-snmp`, an OpenSSH server and `telnetd`.

Interface:

```bash
pnpm --dir ui install && pnpm --dir ui test && pnpm --dir ui build
```

```bash
pnpm --dir ui e2e
```

The end-to-end tests build the interface and start a real `nocturne-server` on
a temporary data directory, so they need `cargo build -p nocturne-server` to
have succeeded. `pnpm --dir ui shots` writes screenshots (git-ignored).

Run it:

```bash
cargo run -p nocturne-server -- --open
```

```bash
pnpm --dir ui build && cargo run -p nocturne-desktop
```

The desktop binary embeds `ui/dist`, so build the interface first or you get the
fallback page that says so.

## Conventions

* **Schema changes** are a new string appended to `SCHEMA` in
  `nocturne-app/src/store.rs`; `PRAGMA user_version` carries the number and
  migrations run in order at open. Never edit a migration that has shipped, and
  never renumber.
* **A new RPC method** needs: the handler, a test that calls it through
  `App::rpc` (not the inner function), a line in the feature's specification, and
  a TypeScript type in `ui/src/api/types/` when a screen uses it. Add it to the
  suite's method test if the feature is registered there.
* **A new vendor read**: extend the YAML profile, return the normalized types in
  `nocturne-drivers/src/model.rs`, and add a test against recorded output in the
  crate's fixtures. Output recorded from a real device is worth more than
  anything you can write by hand; say in the test where it came from.
* **A new kind of change** touches `nocturne-changes` in a fixed order:
  `plan.rs` computes the commands, `risk.rs` and `protect.rs` classify them,
  `verify.rs` says how to tell afterwards whether it worked, and `rollback.rs`
  says how to undo it. All four, or it does not ship. Test both the success
  path and verify-failure-then-rollback.
* **A new assistant tool** (`nocturne-assistant/src/tools/`) calls existing RPC
  methods, is registered with the narrowest permission that lets it work,
  returns real data only, and has a test. A tool that can change something must
  go through the change pipeline like everything else.
* **Every user-visible mutation calls `audit(...)`.**
* **Tests name the behaviour, not the function**:
  `a_reboot_throws_the_interval_away_rather_than_inventing_a_spike`. A test that
  would still pass if the feature were deleted is not a test.
* **Comments say why.** The code already says what. A comment that explains a
  non-obvious decision (a sleep, an `allow`, a protocol quirk, an ordering) earns
  its place; one that narrates the next line does not.
* **No `unsafe`.** TLS is rustls with the **ring** provider only — nothing that
  needs a C toolchain beyond what ring and SQLite need, so the project builds on
  Windows, macOS and Linux. Before adding a dependency, check its features:
  `cargo tree -i aws-lc-rs` and `cargo tree -i openssl-sys` must find nothing.
* **CI (`.github/workflows/ci.yml`) must stay green**: fmt, clippy with
  `-D warnings`, `cargo test`, a portability matrix, the interface's unit tests
  and build, the end-to-end suite, and the Docker and desktop builds.
