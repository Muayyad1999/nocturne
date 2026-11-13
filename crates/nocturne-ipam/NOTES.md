# nocturne-ipam — working notes

Spec: `docs/dev/features/ipam.md` (conventions: `docs/dev/features/README.md`).

Build / test (shared target directory, always scoped to this crate):

```bash
export CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/main
cargo test -p nocturne-ipam
cargo clippy -p nocturne-ipam --all-targets -- -D warnings
cargo fmt -p nocturne-ipam
# the measurements, printed by the test itself:
cargo test --release -p nocturne-ipam --test performance -- --nocapture
```

## Status: complete

```
src/model.rs       the record types, the observation type, the result types, the settings
src/net.rs         the numeric form of what `nocturne-tools` returns (keys, host ranges, counts)
src/db.rs          two own tables: `ipam_address` (records) and `ipam_observation` (evidence)
src/prefixes.rs    the tree, utilisation, free runs, free child space, save/delete, splits
src/addresses.rs   `compose` (the one place a record and its evidence meet), the grid, next free
src/collected.rs   collected interfaces and ARP tables -> observations, with per-device coverage
src/conflicts.rs   duplicate address, overlap, outside its prefix, gateway; virtual-MAC recognition
src/sync.rs        sync (evidence only), reconcile (reports, decides nothing), learn (preview first)
src/vlans.rs       VLAN records merged with what the devices report, per device name
src/portability.rs CSV import (preview then commit) and export
src/api.rs         the 22 methods, the `ipam.changed` event, the debounced watcher
tests/common/      a real campus: the simulator's devices, collected through the real profile
tests/rpc.rs       the surface and its failure modes (15)
tests/arithmetic.rs  /31, /32, /30, /126, /127, /64, /0, splits, VLSM, next free (5)
tests/reconcile.rs   reconciliation against genuinely configured devices (17)
tests/performance.rs 50,000 addresses in 200 prefixes, timed (1)
ui/src/api/types/ipam.ts
```

67 tests green; clippy clean with `-D warnings`; no `unsafe`.

## The one distinction

A **record** is what a person wrote (`ipam_address`, the `ipam_prefixes` and `ipam_vlans`
documents). An **observation** is what a device said (`ipam_observation`: device, interface,
`collected_at`, `first_seen`, `last_seen`, `current`). They are separate tables, separate types
and separate fields on the wire; `addresses::compose` is the only place they meet, and it keeps
`documented`, `observed`, `observations[]` and `disagreements[]` so no screen has to guess.

* `ipam.sync` writes **evidence only**. The one record it creates is a prefix with
  `source: discovered`, which says in itself that nobody has confirmed it.
* `ipam.reconcile` **reports** and writes nothing. Its `refused` field lists, in words, the
  decisions it will not make.
* `ipam.learn` is the only call that turns evidence into records, and `dry_run` defaults to
  `true`.

The automatic sync after `collect.updated` is debounced by `sync_delay_ms` (1.5 s) and can be
turned off with `sync_on_collect`; both are settings so the tests can drive it, which is also why
the reconciliation tests disable it and one test turns it back on to check it works.

## Decisions

- **Own tables, not documents.** A campus has a quarter of a million addresses: a prefix's
  addresses are a `BETWEEN` on `(scope, family, ip_key)` where `ip_key` is fixed-width hex, so
  SQLite's string order *is* address order and a range is one index scan. Prefixes and VLANs stay
  documents — there are thousands, not millions, and the tree is built in memory anyway.
- **Namespaces.** `scope` is `""` for production and `lab:<lab id>` for a lab, taken from
  `device.lab`; a lab prefix carries it as its `site`. Nothing in a lab can be counted towards a
  production prefix, which the tests check. A production `site` is only a filter — containment is
  containment, so a branch's /24 still nests inside the estate-wide /8.
- **`free` is "not seen".** `Status::Free` means no record and nothing has ever answered. Every
  answer that rests on absence carries its coverage: `ipam.prefix.get` has `coverage` and a
  `note`, `ipam.next_address` has `basis` and a breakdown of what it skipped, `ipam.reconcile`
  has `coverage` and, per unobserved address, `had_coverage` — "nobody looked" and "nothing is
  there" are different sentences.
- **An interface table is authoritative, an ARP table ages.** An interface address that is no
  longer reported is *withdrawn* at the next sync (the whole kind is replaced per device, keeping
  `first_seen`); an ARP sighting that leaves the table keeps its `last_seen`, stops being
  `current`, and is forgotten once it is older than `arp_stale_days`. A device that was never
  collected has nothing replaced: a failed read is not an empty table.
- **Adoption instead of a duplicate.** Documenting a block a sync discovered updates that record
  and makes it `manual` (`adopted: true` in the result) rather than refusing as a duplicate. The
  `discovered_from` note is kept as provenance and is never refreshed again, because a sync only
  refreshes prefixes whose `source` is still `discovered`.
- **Conflicts are findings.** Nothing is ever resolved by picking a side. `duplicate_ip` comes
  from two devices with one address configured, or two MACs answering within
  `mac_change_window_minutes`; the well-known HSRP/VRRP/GLBP virtual MAC ranges are recognised and
  excluded. `gateway_mismatch` is only raised where something *is* configured in the prefix —
  claiming a mismatch in a block nobody has read would be inventing a fault.
- **An interface mask that disagrees with its prefix** (`/25` configured inside a documented
  `/24`) is reported by `ipam.reconcile` as `mask_mismatches`, not as a `Conflict`: the contract
  fixes the four conflict kinds and the UI switches on them.
- **A reservation never writes over anything.** An address that already has a record, or that a
  device has configured or has answered on, is a `conflict` error naming the holder, with the
  sightings in `details`.
- **Lists scale.** A whole prefix tree is two queries per namespace, not two per row: the record
  and sighting keys are read once and each row's range is found by binary search
  (`prefixes::Bulk`). A search composes its rows from two batched queries. Both were written after
  the 50,000-address test showed half-second answers.
- **No records are invented from text.** An interface address with no mask yields an observation
  with `prefix_len: None` and no prefix is discovered from it; `/32` would have been a guess.
  Link-local, loopback and unspecified addresses are skipped and counted in `skipped_addresses`.

## How the tests get their evidence

`tests/common/mod.rs` builds `nocturne_netsim::demo_campus()` — the reference campus: an edge
router with NAT, two layer-3 distribution switches with SVIs, HSRP, OSPF and DHCP, three access
switches, and hosts that really take leases — lets it settle, and then puts each device in the
inventory with the real `cisco_ios` profile. Each collection takes what the simulated device
prints for `show interfaces`, `show ip arp`, `show vlan` and `show version` and runs it through
the real collectors and ntc-templates. So the "collected" rows the reconciliation tests read are
what a device said about itself, and `Campus::configure` changes a device through its own CLI
(a duplicate address, a /25 mask, a loopback that goes away) and reads it again.

`nocturne-netsim` is an engine, so this breaks no layering rule; it is a dev-dependency only.

## Measurements in this workspace

50,000 address records in 200 prefixes under one /16, 5,000 of them also seen in ARP, debug
build on a shared machine:

| call | time |
|---|---|
| `ipam.prefixes` (201 rows, utilisation each) | 43 ms |
| `ipam.search` by host name / partial address / MAC | 12 / 57 / 13 ms |
| `ipam.prefix.get` (a /24 grid of 254 rows) | 9 ms |
| `ipam.next_address` | 4 ms |
| `ipam.summary` | 88 ms |

Before the two batching changes above the same calls were 65 / 515 / 789 / 517 / 4 / 495 ms.

## Wants from the core

* `collect::Stored` has no per-row timestamp, so an observation's `collected_at` is the time of
  the whole table. An ARP entry's own age (the `age` column) is text the profiles do not
  normalise, so it is not used.
* Nothing in the core says a device's interface addresses were read but came back empty versus the
  device having none — `Snapshot` says it per run, but it is not persisted, so coverage here is
  "the table was read at T and carried no address".
* A simulated device can only be reached as a lab device (`Transport::Sim` requires a `LabRef`),
  so every test that collects real output works in the `lab:` namespace. Production-namespace
  behaviour is covered by records and by unit tests.
* `nocturne-netsim`'s HSRP is a placeholder, so no lab produces a virtual MAC. The recognition of
  the HSRP/VRRP/GLBP ranges is unit-tested against the values in the standards; the conflict path
  that uses it has no end-to-end test.
* `nocturne_tools::subnet` is string-in, string-out (its `ip` module is `pub(crate)`), so
  `net.rs` converts canonical output to numbers for the hot paths. Its tests check every edge
  against `subnet::info` so the two cannot drift.
