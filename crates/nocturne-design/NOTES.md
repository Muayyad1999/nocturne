# nocturne-design — working notes

Spec: `/home/user/nocturne/docs/dev/spec-design.md`. The simulator's contract
(what the generated configurations may use) is `docs/dev/spec-netsim.md`,
section *CLI: command coverage*; the worked examples in
`crates/nocturne-netsim/src/demo.rs` are the dialect that is known to work.

```
CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/design cargo test -p nocturne-design
CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/design cargo clippy -p nocturne-design --all-targets --no-deps -- -D warnings
CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/design cargo run -p nocturne-design --example dump -- <branch|0|1|2|big N full>
```

`examples/dump.rs` is the iteration harness: it generates, boots, settles and
probes one design and prints what went wrong. `SHOWCFG=1` prints every
configuration, `SHOWDOC=1` the Markdown document, `SHOWCHECKS=1` the check
table with pass/fail.

## Module map

| File | What it does |
|---|---|
| `requirements.rs` | the input types, all with `Default` |
| `design.rs` | the output types: `Design`, `Plan`, `Check`, `Decision` |
| `model.rs` | `prepare`: normalise, size the site, carve the addresses. The one place requirements are judged, so `validate` and `generate` can never disagree |
| `topology.rs` | devices, ports, cables, trunk pruning, the picture |
| `config.rs` | the IOS-style configuration text |
| `checks.rs` | the reachability matrix, derived from the design |
| `assemble.rs` | `LabSpec` + `Plan` + the decisions and warnings |
| `document.rs` | the Markdown document |
| `util.rs` | IPv4 arithmetic and the sanitising of anything a person typed |

## Settled decisions

* Address plan: the infrastructure block sits at the top of the base block and
  is **sliced per site** (loopbacks, then the core-peer transit, then one block
  per edge router, then the WAN links the site owns). A single global block
  cannot be summarised with static routes without creating a loop.
* Every routed VLAN is at least a /27, so `.1` can be the gateway, `.2`/`.3`
  the two cores, `.5…` the test hosts and `.11…` the DHCP pool.
* Two cores: HSRP v2, alternating the active core VLAN by VLAN, with the
  spanning-tree root priority following the same split.
* A dedicated transit VLAN between a pair of cores, so routing never runs over
  a user VLAN.
* The WAN lands on the first edge router of a site only; the second edge
  router is a second Internet exit. A warning says so under `Full`.
* Trunk `allowed vlan` lists are computed *after* the end systems are placed,
  bottom up, so a trunk carries exactly what the far end needs.
* Device management addresses come from a cursor above the static reserve of
  the management subnet, which is sized for the device count.
* The lab places a sample of end systems (`hosts_per_vlan`), not thousands of
  workstations; the plan still sizes the VLANs for the real counts.

## Simulator limitations found (reported)

`cargo run -p nocturne-design --example simbug` reproduces both of them.

* `Dot11Radio0` is listed by `free_interfaces` but `IfId::parse` splits the
  type word at the first digit, so `add_link` rejects it: nothing can be
  cabled to an access point's radio. Access points are therefore cabled by
  their `GigabitEthernet0` and the lab's guest clients sit on their own access
  port in the guest VLAN.
* `probe_*` stops retrying as soon as the worst drop of an attempt is not
  `ArpPending`, so a flooded ARP frame that dies on an unrelated branch fails
  the whole probe even though the timed simulation delivers it. The branch
  that bites is a workstation on a port with `switchport voice vlan`: it
  cannot decode the tagged voice VLAN and drops it. In the reproduction the
  console `ping` answers `.!!!` while `probe_ping` reports a failure. The
  ports the lab attaches a test workstation to are therefore data only, and a
  warning says so.
* The catalogue has no 48-port access switch, so `access_ports: 48` is clamped
  to 24 with a warning.

## Status: done

Green: 5 unit, 12 arithmetic, 5 document, 5 hostile, 8 lab, 17 matrix, 1
doctest. Clippy clean with `-D warnings`, `cargo fmt` applied, no `unsafe`.
