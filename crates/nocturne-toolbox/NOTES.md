# nocturne-toolbox — build notes

Spec: `docs/dev/features/toolbox.md`. Conventions: `docs/dev/features/README.md`.

## State: complete

77 tests, `cargo clippy -p nocturne-toolbox --all-targets -- -D warnings` clean,
`cargo fmt -p nocturne-toolbox --check` clean, no `unsafe`, no new third-party
dependency (`nocturne-snmp` was added to this crate for the OID dictionary
behind `tools.oid`; everything else was already declared).

```
src/
  lib.rs        init(): settings, state, routers; the rules of engagement in prose
  settings.rs   the `toolbox` section: probe defaults, the sweep's leash, the servers' ports
  errors.rs     ToolError / ProbeError -> AppError, in one place
  catalogue.rs  tools.list — the whole surface, kept honest by a test in both directions
  state.rs      the only runtime state there is: the two servers, when one runs
  calc/         the offline half, one module per group
    addressing  subnet, split, vlsm, supernet/summarise, contains, overlaps, range, convert, wildcard, acl
    identity    mac, oui (both ways), oid, interface names
    secrets     decode/encode/identify, passwords, key pair
    units       convert (sizes, rates, durations, dBm), transfer_time, mtu
    reference   the tables, with the engine's own search where it has one
    text        regex, diff, hashes, base64
  probe/        the half that touches the network
    known       Known (address -> device), Target, and the host-name allow-list
    history     the probe runs: begin / succeeded / failed, list, get, delete, retention
    ping        probe.ping, the echo stream, and the until-cancelled mode
    trace       probe.traceroute, hops labelled with the inventory as they arrive
    ports       probe.ports
    sweep       probe.sweep and the plan it refuses on
    quick       probe.dns / .whois / .tls / .http / .ntp — answer in the reply, still recorded
    local       probe.local, probe.wol
    device      probe.device.ping / .traceroute: the per-family command, the gate, the parsers
  servers/      tftp.* and probe.syslog.* — a job each, off unless asked
  tests/        calculators, catalogue, probes (loopback), device, servers, history
```

## What the feature claims, and how

* **Calculators are the engine, wired up.** Every method is `nocturne-tools`
  (or `nocturne_drivers::hier::text_diff` for `tools.diff`, and
  `nocturne_snmp::names` for `tools.oid`). The façade adds parameter reading,
  the `invalid` mapping that keeps the engine's own message, and the two
  parameter shapes the contract asks for that the engine deliberately does not
  guess at (`subnet.split` by `count`, `convert` by `from`/`to`).
* **Probes say where they ran and how.** `from` is `this computer` or
  `the Nocturne server`; a ping also carries the socket method the operating
  system allowed, a traceroute its implementation, a device probe the command.
* **A probe that cannot run is a refusal, not a result.** `errors::probe` maps
  `ProbeError::Permission` to `forbidden`, and the history record of a failed
  run has `ok: false`, `result: null` and an `error_code`. The one test that
  cannot be deterministic on an arbitrary machine
  (`a_ping_of_loopback_either_reports_or_says_it_could_not_run`) asserts both
  branches and prints why it took the second.
* **It knows the inventory.** Any probe can be aimed at `device` (id, name or
  address), and `known::Known` labels addresses that come back — sweep hosts,
  traceroute hops, DNS answers, the local neighbour table — with the device
  they belong to. Only exact address matches: an address inside a device's
  subnet is not that device's.
* **It keeps a history.** Every probe writes a record before it runs and
  finishes it afterwards, so a running ping is in the list and a process that
  dies leaves a record that says so (`close_interrupted`).

## Decisions worth remembering

* **The quick probes answer in the reply and still leave a record.** The
  contract's shapes are `probe.dns -> DnsAnswer` and so on, so the engine's
  report is `#[serde(flatten)]`ed and `from`, `run_id` and the device hints sit
  beside it. Only the four that can take a while (ping, traceroute, ports,
  sweep) are jobs with `{job_id, run_id}`.
* **`count: 0` drives the engine one echo at a time.** `ping::ping` has no
  until-cancelled mode and returns its report only at the end, so a continuous
  ping calls it with `count: 1` in a loop: resolution, the socket method and
  the "this machine cannot send at all" judgement stay the engine's, and the
  loop only counts and paces. The summary of such a run (and of a cancelled
  one) is computed here — `report_from` — which is the one piece of engine
  arithmetic this crate repeats, and the reason it does is in the report.
* **A cancelled ping succeeds with a partial report** rather than failing. It
  ran, it was stopped, and what it saw is the honest answer; the job's state
  says it was the user who stopped it.
* **A long run keeps its newest echoes.** An hour at one echo a second is 3600
  of them; `MAX_KEPT_ECHOES` is 2000 and `echoes_dropped` says how many went.
* **`probe.sweep` carries its own leash** (`sweep::plan`) rather than calling
  discovery's: features do not depend on each other, and this is a smaller
  promise than `discovery.scan` — what answered, and nothing about what it is.
  The two implementations of "vet these targets" are near-identical; see below.
* **Only the families whose ping syntax is known get a command.** `IosRepeat`,
  `IosCount` (NX-OS, IOS-XR), `Junos`, `RouterOs`, `Vrp` and `Linux`, decided
  from what the *profile* says about itself (`os`, `vendor`, `config.syntax`),
  never from a vendor hard-coded against a device id. Anything else is
  `unsupported` and names the profile. `FortiOS` is the reason the matching is
  by whole word and not by `contains("ios")`.
* **The target of a device probe is allow-listed, not sanitised.** Letters,
  digits, `.`, `-`, `_`, `:` and nothing else, and the same for the source
  interface and the VRF. `tests::device` asserts that after eight injection
  attempts the fake device received *nothing at all*.
* **A Linux source interface is refused rather than sent.** `ping -I eth0` is
  not on the Linux profile's read-only list, and rightly so; refusing with
  `unsupported` is better than sending a line the policy will stop.
* **The servers are jobs.** `tftp.start` binds first (so a port that cannot be
  had is an error on the button) and then hands the server to a job that owns
  it; `tftp.stop` cancels that job, and shutdown ends it the same way. Nothing
  can leave a socket open because there is nowhere for it to hide.
* **The syslog listener lives under `probe.syslog.*`, not `syslog.*`.** The
  brief asked for a staging receiver; `nocturne-logs` already owns syslog
  collection (its own receiver, settings, ingest rules and `logs.status`), so
  this one keeps its messages in memory, writes nothing to the log store,
  defaults to 5515 rather than the 5514 that receiver uses, and says so in
  `probe.syslog.status.note`. The global namespace is left to the feature that
  owns the subject.
* **`tools.keys.generate` delegates to `keys.generate`** when the terminal
  feature is in the build — one implementation of the fingerprint — and falls
  back to `nocturne_transport::generate_keypair` with `fingerprint: null` when
  it is not. Tested both ways by registering a stub method in the test builder.
* **Audit covers the mutations, not the reads.** A probe is a read, but one
  that leaves this machine, so a sweep and a Wake-on-LAN packet are audited the
  way `discovery.scan` is, and so is starting or stopping a server.
  `probe.history.delete` is the one record-changing method and it is audited
  too. The calculators audit nothing: they have no side effect, and a decoded
  secret must not end up in the trail.
* **No secret is written anywhere.** `tools.secret.decode` returns the secret
  to its caller and writes no audit line, no job log and no history record;
  `probe.http` records the URL and the response but never the request headers,
  because one of them may be a token.
* **The reference tables are not copied.** `cable_pinouts` is in the contract's
  list of possible tables but no engine has one, so it is not offered;
  `tools.reference.tables` says what there is and an unknown table is
  `not_found` naming them.

## Needed from the engines (worked around for now)

* **`nocturne_tools::mac` cannot be searched, only looked up.** The registry is
  private and `mac::vendor` takes a prefix, so "which blocks does this vendor
  own?" — the other half of "OUI lookup both ways" — has no engine call behind
  it. Rather than keep a second copy of the IEEE registry here, `calc::identity`
  derives a reverse index from the engine itself: every prefix the IEEE could
  have assigned (the first octet's multicast and local bits are always clear in
  an assignment, which rules out three quarters of the space) is looked up once,
  4.2 M calls, about a third of a second of one core in release, built lazily on
  a blocking thread the first time somebody searches by name. It finds the
  24-bit MA-L assignments; the answer's `note` says that MA-M, MA-S and IAB
  blocks are prefix-only. **A `mac::search(name) -> Vec<(prefix, name)>` in the
  engine would delete all of this.**
* **No optical power conversion.** `convert` has sizes, rates, durations and
  MTU but nothing for dBm and milliwatts, and its number parser refuses the
  negative numbers an optical level is written with. `calc::units` does that one
  family itself (three lines of logarithm) and says so in its own docs; it
  belongs in `nocturne_tools::convert`.
* **`convert` parses every unit but emits only its own choice.** Turning bytes
  back into MiB needs the divisor, so `calc::units` keeps a table of factors for
  the emitting side only. `convert::to_unit(canonical, "MiB")` in the engine
  would replace it.
* **`ping::ping` has no until-cancelled mode and no partial report**, which is
  why `report_from` recomputes loss and min/avg/max/stddev here (see above).
  An `on_echo`-driven `PingRun` that could be stopped and asked for its report
  would be better, and would make the same numbers come from one place.
* **No word list, so no `tools.passphrase`.** The contract lists it beside
  `tools.password`, but `secrets` has no word list and inventing one here would
  be a second, unreviewed copy of data the engine should own. Not registered;
  `secrets::generate_passphrase(words, seed)` would be the place.
* **No cron.** `tools.cron` is in the contract "only if the engine has it", and
  the only cron in the workspace is `nocturne-automation::cron` — another
  feature crate, which this one may not depend on. Not registered.
* **No core "whose address is this?"** `known::Known` builds the map from the
  device list plus `collect::latest_all(Interfaces)` on every probe. That is
  cheap at campus scale and wrong at ten thousand devices, and
  `nocturne-discovery::identify::Known` does the same thing a second time. An
  indexed lookup in the core would serve this crate, discovery, topology and
  IPAM.
* **Target vetting is duplicated.** `probe::sweep::plan` and
  `nocturne-discovery::targets::plan` both expand a target expression and refuse
  multicast, broadcast and public space. The rules belong somewhere both can
  reach — the core beside `connect`, or the netprobe engine.
* **`nocturne_app::testing` has no device that fails a read.** `FakeDevice`
  rejects configuration lines but always answers a `show`, so "the device
  session broke half way" is untested here; the gate test uses a profile whose
  policy refuses the command instead, which exercises a different path.
* **`nocturne_netprobe::tls::test_support` is `pub(crate)`.** It builds local
  TLS servers with self-signed and expired certificates with `rcgen`, which is
  exactly what a `probe.tls` test wants; from outside the crate there is no way
  to stand one up without adding `rcgen` here. The TLS test therefore covers the
  failure path only (a handshake against something that is not TLS) and asserts
  that it fails rather than returning an empty certificate. Making that module
  public behind a `testing` feature would let this crate check the interesting
  half: a chain described in full with `trusted: false`.

## Not done

* `tools.passphrase` and `tools.cron` (above).
* The `tools.oui` name search covers MA-L only (above).
* `probe.tls` against a real certificate is not tested from this crate (above).
