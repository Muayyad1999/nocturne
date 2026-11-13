# nocturne-discovery — build notes

Spec: `docs/dev/features/discovery.md`. Conventions: `docs/dev/features/README.md`.

## State: complete

49 tests, `cargo clippy -p nocturne-discovery --all-targets -- -D warnings`
clean, `cargo fmt -p nocturne-discovery --check` clean, no `unsafe`, no new
third-party dependency (`ipnet`, already in the workspace, was added to this
crate for CIDR containment and block sizes).

```
src/
  lib.rs       init(): settings, state, routers; the rules of engagement in prose
  settings.rs  the `discovery` section — the leash: max_hosts, rate, concurrency,
               timeout, ports, allow_public, never_scan, keep_scans, max_crawl_depth
  model.rs     ScanRequest, Found, Evidence, Suggested, ToldBy, Scan, Ignored (contract shapes)
  targets.rs   target expansion and every refusal a sweep makes before it sends
  probe.rs     Probes trait (echo / ports / snmp / reverse DNS / local tables),
               RealProbes over netprobe + snmp, Rate (token schedule), ProbeLayer
  snmp.rs      the scan's SNMP credentials, resolved once, secrets kept in Auth
  identify.rs  the per-address pipeline, the profile-registry match, the suggestion,
               the evidence, and `Known` (inventory match by address or collected IP)
  db.rs        scans and finds as documents, the live set of a running scan,
               the ignore list, retention, restart clean-up
  scan.rs      discovery.scan / .scans / .scan_get / .scan_delete, the sweep job
  crawl.rs     discovery.crawl: neighbour, ARP and route tables of managed devices
  add.rs       discovery.preview / .add / .ignore / .unignore / .ignored
  suggest.rs   discovery.suggest — this machine's own networks and gateways
  tests/       targets, sweeping, adding, crawling, records, live (real packets)
```

## Rules of engagement (the point of the crate)

Written in full at the top of `lib.rs`; in short:

* Nothing scans unless a person asks. No background task, nothing at start-up.
* The settings bound every scan and a request may only ask for less.
  `never_scan` blocks are dropped from the plan whatever the request says.
* Multicast, broadcast and unspecified addresses are never probed. Public
  address space needs `allow_public` on the request **and** in the settings,
  and the job log says so.
* A scan is a job: visible, logged, cancellable; cancelling stops the sending
  instead of draining the queue (`tests::sweeping::cancelling_a_scan_…`).
* No login anywhere in the crate. SNMP uses only the credentials the request
  names; the SSH banner is what the server volunteers; the only CLI sessions
  are to devices already in the inventory, through `collect::run`, during a
  crawl the user asked for.
* `discovery.add` is the one mutation, it goes through `inventory::create`,
  and it still logs in nowhere.

## Decisions worth remembering

* **`ScanRequest.ports` is `Option<String>`**, not `String`. The contract
  wants "absent = the usual ports, `""` = none", and serde cannot tell an
  absent `String` from an empty one. Absent now means "the ports in the
  settings", which is also how the default moved from a constant in the
  request to a setting a person can change.
* **`Found` carries `evidence: Vec<{field, source, detail}>`** beyond the
  contract's shape, and `suggested` carries `profile`, `site` and
  `snmp_credential_id`. The feature's whole claim is "here is why I think
  this", and a `profile.evidence` string alone could not carry it. The
  evidence also becomes the created device's notes.
* **`via` stayed a string** (`icmp`, `tcp/22`, `arp`, `snmp`, `neighbor`) as
  the contract says; the crawl's path went into **`path: Vec<String>`** and
  **`told_by`** instead, which say which device told us and through which
  table and interface.
* **A role is only ever suggested from a neighbour's reported capabilities**
  (`ToldBy.capabilities`, from CDP/LLDP). Nothing is inferred from open ports
  or from `sysDescr` — the profile has no role field, so there was nothing
  honest to read there, and port 8291 is a hint, not a fact.
* **An already-managed crawl candidate is still reported as a find** (with
  `device_id` set) and becomes a seed for the next level. "That is SW2, which
  you already have" is what makes a crawl's coverage readable, and nothing is
  sent to it.
* **With `add: false` a crawl can only read devices already in the
  inventory**, so that is as deep as it goes; `depth` beyond that needs
  `add: true`. Reading a device Nocturne has not been allowed to add would
  mean logging into something the user never approved.
* **ARP and route tables are capped per device** (256 ARP entries, 128 next
  hops, 4096 candidates per crawl) and the job log says when a cap bit. A
  distribution switch knows thousands of workstations; a crawl is for finding
  the network.
* **The rate limit wraps the call sites, not the probe implementation**, so it
  bounds the real probes and anything a test injects, and it is scheduled on
  tokio's clock — which is how
  `tests::sweeping::the_probe_rate_is_what_the_scan_was_given` measures the
  pacing exactly under `start_paused`. One permit = one echo, one TCP
  connection or one SNMP GET; reverse DNS is not counted (it goes to the
  resolver, not to the target) and `Scan.probes_sent` is the permit count.
* **`discovery.found` is capped at 200 events per scan**; after that the job
  log says so and `discovery.progress {probed, found, streaming: false}`
  carries the counters while the finds stay in `discovery.scan_get`. The
  event bus is shared with every other window.
* **A running scan keeps its finds in memory and flushes them every 750 ms**;
  `scan_get` serves the live set while it runs and the stored document after,
  so a reader cannot tell the difference. Rewriting one document per
  answering host would be quadratic.
* **`scan_get` pages** (`limit`, `cursor` as "rows already received", plus
  `total`), because a /16 can answer tens of thousands of times. The contract
  named no paging; the defaults are generous enough that a caller written
  against it still works.
* **`device_id` and `ignored` are re-evaluated on every read** of a scan, and
  on `preview`/`add`. A find is a record of what answered, not of what
  Nocturne managed at the time.
* **A scan deleted while it runs stays deleted**: the job updates its record
  in place (`db::update_scan`) and drops its results when the record is gone.
  Scans left `running` by a killed process are marked `failed` at start-up,
  the way the core does with jobs.
* Scans are documents, 20 kept (`keep_scans`), and deleting a scan deletes
  its finds with it.

## Needed from the core (worked around for now)

* **SNMP credential → `nocturne_snmp::Auth` is duplicated.** `snmp.rs` here
  and `nocturne-monitor`'s `snmp::target` both turn a `Credential` plus its
  resolved secrets into an `Auth`, with the same protocol tables and nearly
  the same error messages. Features may not depend on each other, so there is
  no third place to put it: it belongs in the core beside
  `credentials::resolve` (something like `credentials::snmp_auth(app, id)`).
* **`nocturne_app::testing` has no device that can be reached at an arbitrary
  address.** `FakeDevice` is served through the simulator hook, which matches
  on `LabRef.node`, so a device *discovery created* cannot then be read —
  which is exactly what a crawl does at depth ≥ 2 with `add: true`. The
  crawl's "go through what you added" path is therefore tested by pre-creating
  the intermediate device in the inventory
  (`tests::crawling::a_crawl_goes_through_the_devices_it_reaches…`); the
  "added, then crawled" sequence is covered only one level deep. A fake that
  the connect layer resolves by address (or a `FakeSim` that can be told "this
  address is that node") would close the gap.
* **`FakeDevice.drop_on` only applies to configuration lines**, so there is no
  way to make a fake device fail a *read*. The "a seed that cannot be read is
  reported and the crawl carries on" test uses a device whose profile does not
  exist instead, which exercises the same error path but for a different
  reason than a dying session would.
* `FakeDevice` renders `Capabilities: Switch IGMP` with no way to set it, so
  the crawl tests patch the canned `show cdp neighbors detail` output to get
  a router's capabilities. A settable `capabilities` on `FakeNeighbor` would
  be tidier.
* No core API lists "every address Nocturne knows about": `identify::Known`
  reads the device list plus `collect::latest_all(Interfaces)` and builds the
  map itself. That is cheap at campus scale and wrong at ten thousand
  devices; an indexed lookup in the core would serve the topology and IPAM
  features too.
