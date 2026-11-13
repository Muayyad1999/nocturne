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
src/catalogue.rs  the 34 checks: needs, description, not_evaluated, default severity
src/report.rs     per device, per check: a verdict or the reason there is none
src/world.rs      one bulk load of everything a scan reasons over + the parsed-config cache
src/links.rs      topology.get when present, else a narrow neighbour matcher
src/norm.rs       duplex / speed / mode / trunk / native VLAN / VLAN-name normalisation
src/net.rs        CIDR maths; HSRP/VRRP/GLBP and anycast recognition
src/metrics.rs    metric history, aggregated in SQL (the one place that reads core tables)
src/logs.rs       link up/down and err-disable events from the stored log
src/checks/*      physical (9), l2 (6), l3 (4), config (3), security (7), fleet (5)
src/db.rs         own `insights` table: stable ids, resolve, dismissal, SQL filter/count/page
src/scan.rs       the scan, the coverage report, alert raising, the debounced watcher
src/rpc.rs        scan / list / get / checks / coverage / dismiss / reopen / fix
tests/rpc.rs      the whole surface through app.rpc (13)
tests/planted.rs  a campus with 8 planted faults and 20 look-alikes: precision and recall (3)
tests/performance.rs  500 devices x 48 interfaces, timed (1)
ui/src/api/types/insights.ts
```

93 tests green; clippy clean with `-D warnings`; no `unsafe`.

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

## Measurements in this workspace

- Planted network: 7 devices, 8 faults, 20 look-alikes → precision 1.00, recall 1.00, scan 6 ms.
- 500 devices x 48 interfaces: see the test's printout (release is the number that matters; the
  debug budget is looser because most of the time goes into serde).
- Loading dominates a scan; the two things that made it worse were compiling the platform's
  `config.ignore` regexes per device and cloning every collected row to deserialise it.
