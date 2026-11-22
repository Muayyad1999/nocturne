# nocturne-insights — working notes

Spec: `docs/dev/features/insights.md` (conventions: `docs/dev/features/README.md`).

Build / test (shared feature target dir, always scoped to this crate):

```bash
export CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features
cargo test -p nocturne-insights
cargo clippy -p nocturne-insights --all-targets --no-deps -- -D warnings
cargo fmt -p nocturne-insights
# the measurements, printed by the tests themselves:
cargo test -p nocturne-insights --test planted -- --nocapture
cargo test --release -p nocturne-insights --test performance -- --nocapture
```

## Status: complete

```
src/model.rs      Insight / Finding / Severity / Category / stable ids   (from the skeleton)
src/settings.rs   enabled_checks (from the catalogue) + every threshold
src/catalogue.rs  the 37 checks: needs, description, not_evaluated, default severity
src/report.rs     per device, per check: a verdict or the reason there is none
src/world.rs      one bulk load of everything a scan reasons over + the parsed-config cache
src/links.rs      topology.get when present, else a narrow neighbour matcher
src/norm.rs       duplex / speed / mode / trunk / native VLAN / VLAN-name normalisation
src/net.rs        CIDR maths; HSRP/VRRP/GLBP and anycast recognition
src/metrics.rs    metric history, aggregated in SQL (the one place that reads core tables)
src/logs.rs       link up/down and err-disable events from the stored log
src/checks/*      physical (9), l2 (6), l3 (4), config (3), security (7), fleet (5), qos (3)
src/db.rs         own `insights` table: stable ids, resolve, dismissal, SQL filter/count/page
src/scan.rs       the scan, the coverage report, alert raising, the debounced watcher
src/rpc.rs        scan / list / get / checks / coverage / dismiss / reopen / fix
tests/rpc.rs      the whole surface through app.rpc (13)
tests/qos.rs      the three QoS checks through app.rpc (5)
tests/planted.rs  a campus with 8 planted faults and 20 look-alikes: precision and recall (3)
tests/performance.rs  500 devices x 48 interfaces, timed (1)
ui/src/api/types/insights.ts
```

Clippy clean with `-D warnings`; no `unsafe`. Run the suite for the count.

## Decisions (they are in the final report too)

- Insights live in an own SQLite table (`insights`) through `store.with`: lists must filter, count
  and page in SQL. Coverage is one kv document.
- One insight per problem. Cross-device problems are owned by one end — the lower device id, except
  `duplex_mismatch` (the half-duplex end) and `native_vlan_mismatch` (the end left on the platform
  default), because that is the end the fix belongs on. The other end is in `related`, and
  `insights.list {device_id}` matches both.
- "Two consecutive scans" counts only scans that *evaluated* the check for that device. A device
  that drops out of collection never resolves its own findings.
- A suggested fix carries `config_lines` only where the fix is exactly right for that platform and
  that device; otherwise text and read-only commands. Trunk edits only ever use
  `switchport trunk allowed vlan add …`, never a restated list.
- `metrics.rs` is the only place that reads core tables (`series`, `samples`) directly, read-only,
  because the public API would be one query per interface. It wants a core API; see the report.
- Refused to ship: `stp_root_unexpected` (nothing collects STP state) and `legacy_ssh_crypto`
  (`ConnectionInfo.legacy_crypto` is never persisted). `vlan_without_ports` from the spec is folded
  into `unused_vlan`. Late collisions are not in `model::Counters`, so the duplex checks do not use
  them.
- Health (`Kind::Health`) is deliberately not loaded: one reading can never be "sustained", so the
  capacity checks use monitoring samples and say "not evaluated" without them.
- Severities chosen so the list stays readable on a real estate: `no_vty_acl` and an ageing backup
  are Low, because on many estates they are true of every device.

## QoS checks (contract: `docs/dev/spec-qos.md`, "Insights")

- `checks/qos.rs` reads IOS-style MQC out of the parsed configuration and the `qos.*` series out
  of the core's tables through `metrics.rs`. It does not link `nocturne-monitor`; the series
  format is the specification's, and the tests write the series themselves.
- The two configuration checks are lopsided on purpose: *missing* judges only references whose
  grammar is known (an interface, a class); *unattached* treats any word after `service-policy`
  on any line as use.
- A configuration is judged only when it is whole, which is two tests. *Partial*: no `hostname`
  line (`show running-config interface …`, `| section …`). *Cut off*: no closing `end`. IOS
  prints policy-maps before interfaces, so a listing that stops anywhere after them has every
  definition and none of the attachments — every policy would look unattached. The parsed tree
  drops `end` as display, so `world::Config.closed` is computed from the text when it is parsed
  and cached with the tree.
- Which MQC grammar a listing is in is read from the listing, never from the vendor or the
  profile id (a device type is data, and a user's own profile can say `syntax: ios` for
  anything). Typed QoS policies → NX-OS/EOS/OS10 style. `end-policy-map`, `end-class-map`,
  `end-policy`, `end-set` → IOS XR. No `end` → also NX-OS and ProCurve, which print none. An
  EOS listing with no QoS statement, or an XR one with no map, route policy or set, is evaluated
  and trivially clean; that is the gap, and it cannot produce a finding.
- Names are compared exactly (IOS does). A case-only difference is added to the finding as
  evidence rather than being forgiven: `Wan-Edge` beside `WAN-EDGE` is a real leftover.
- A name goes into a suggested `show` command only when it is made of `[A-Za-z0-9-_.:/+@]`:
  it is device text, and the assistant reads suggestions.
- No real configuration with policy-map definitions exists in the repository or in the
  ntc-templates corpus, and no recorded `qos.*` series. The real fixture
  (`nocturne-drivers/tests/profiles/cisco_ios/config/asr-subinterfaces.txt`) is the negative case
  and supplies the interface sections of the positive ones; everything around it is written by
  hand and says so.
- From memory, not from a capture: the names of generated policies (`AutoQos…`, `AutoConf-…`,
  `system-cpp-policy`, `port_child_policy`); the session markers (`interface Virtual-Template`,
  `ip subscriber`, IOS-XR `dynamic-template`, `aaa authorization network` — RADIUS can name a
  policy for a session with `sub-qos-policy-in/out`); that IOS-XE prints a child policy one
  level further in under `shape average`; that NX-OS and ProCurve print no closing `end`; and
  the IOS XR section closers. A wrong memory costs a missed exclusion or a device that is not
  evaluated, never a wrong finding about a policy somebody wrote.
- `qos_class_dropping` decides "does this device count congestion apart" per device, not per
  class: a policed class without a queue has no congestion series, and judging it on `qos.drop_ps`
  would report the policy working. The series name carries no parent class, so a congested child
  of a hierarchical policy can appear together with its parent.
- `qos_class_dropping` judges only what was measured up to now. A failed read writes no sample,
  so: two samples more than `sustained_minutes` (≥ 15) apart have a hole between them and only
  the run since the last hole counts (`metrics::summarise_unbroken`); a class whose newest
  sample is older than that is not reported; a device whose newest QoS sample is older than that
  is *not evaluated* — a read that began failing twenty minutes ago must not leave the device
  "clean", nor a class "still dropping". The share and traffic figures are read over the same
  run as the drops (`metrics::summarise_one`). `high_utilisation` and the fleet checks still use
  the plain window and have the same two blind spots; not changed here.
- A `qos_drop_ps` of zero or less is not evaluated: every measured class is "at or above" zero.

## Measurements in this workspace

- Planted network: 7 devices, 8 faults, 20 look-alikes → precision 1.00, recall 1.00, scan 6 ms.
- 500 devices x 48 interfaces: see the test's printout (release is the number that matters; the
  debug budget is looser because most of the time goes into serde).
- Loading dominates a scan; the two things that made it worse were compiling the platform's
  `config.ignore` regexes per device and cloning every collected row to deserialise it.
