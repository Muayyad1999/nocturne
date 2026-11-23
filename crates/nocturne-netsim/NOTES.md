# nocturne-netsim — working notes (for resuming after an interruption)

Spec: `/home/user/nocturne/docs/dev/spec-netsim.md` (public API section is a contract).
Build: `CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/netsim cargo test -p nocturne-netsim`

## Architecture decisions (settled)

* `Lab` (public facade, `lab.rs`) owns a `World` (`sim/`), the consoles, flows.
* `World` is `Clone`. **Instant probes run the real event loop on a scratch clone** of the
  world (a traced PDU, retried while the only failure is "ARP still resolving"), so the timed
  simulation and the walker share every forwarding function by construction.
* Node logic is local: `fn(&mut Node, &mut Fx, &Env, ..)`. `Fx` collects effects (frames to
  transmit, timers, syslog, packet notes); the world applies them (links, queue).
* Cross-node "analytic" state (link carrier, DTP, EtherChannel compat, OSPF SPF) is computed
  at world level in `reconcile` / `ospf::recompute`.
* Config model (`device/config`) is personality-neutral; the IOS front end (`device/ios`)
  parses commands into it and renders running-config from it. Boot = apply startup-config
  text through the same command path. Hosts derive the same model from `HostConfig`.
* CLI grammar: pattern DSL compiled to small NFAs, simulated in parallel across all commands
  of a mode (gives abbreviations, `?` help, Tab, caret position, ambiguity for free).
* Long-running commands (ping/traceroute/telnet connect) are node-level `Proc`s driven by
  events; consoles only render their output.
* ARP miss drops the packet everywhere (hosts too) -> first ping `.!!!!`.
* Timers: STP short by default (fwd delay 1s+1s, max age 6s); `set_realistic_timers(true)`
  gives 15s+15s / 20s and OSPF wait = dead interval.

## Status

(see bottom of file — updated as work proceeds)

## Done (all compiled and tested unless noted)

- [x] util, api, catalog, ifname, device/config (model), device/hostcfg, device/host (shell)
- [x] sim core: frame, fx, syslog, node, mod (World), reconcile, port, switching, stp, arp,
      rib, ip, acl, procs, probe, flows, faults, edit, health
- [x] device/cli (grammar NFA, Console/Session line discipline, pager, nested sessions)
- [x] device/ios (shell, exec, config, config_if, config_sub, help, runcfg, boot, show/*)
- [x] lab.rs (public API), views.rs (snapshot/tables/metrics)
- [x] Tier 2 real: neighbors (CDP/LLDP), dhcp, acl, nat (tests/tier2_nat.rs)
- [x] OSPF: sim/ospf.rs (hello/DR/adjacency machine) + sim/ospf_spf.rs (analytic SPF) +
      show ip ospf [neighbor [detail] | interface [brief] | database] — compiled, unit tests
      for SPF pass, SCENARIO TESTS NOT WRITTEN YET (tests/tier2_ospf.rs)
- [ ] sim/rip.rs, sim/hsrp.rs are still PLACEHOLDERS (commands exist in grammar + runcfg;
      `show ip rip database`, `show standby` not registered) -> implement or remove
- [ ] demo.rs is a placeholder (empty demo_campus/examples)

## Module map (planned)

    src/lib.rs api.rs catalog.rs ifname.rs lab.rs spec.rs demo.rs
    src/util/{net,mac,rng,regex,crypt,clock,text}.rs
    src/device/config.rs            config model
    src/device/cli/...              grammar engine, line editor, sessions
    src/device/ios/...              IOS personality: exec, config, show/*, runcfg
    src/device/host.rs              host shell
    src/sim/mod.rs                  World + event loop
    src/sim/{frame,fx,node,syslog,reconcile,port,switching,stp,arp,rib,ip,procs,
             acl,nat,dhcp,ospf,rip,hsrp,neighbors,flows,faults,probe}.rs

## Key internal conventions

* Node logic: `impl Node { fn xxx(&mut self, fx: &mut Fx, ...) }`; world applies `Fx`.
* `fx.note(node, kind, &frame, iface, why, || text)` records a PacketEvent only when the
  frame is watched (capture on or traced); returns seq to store in `frame.meta.via`.
* `World::with_node(id, |n, fx| ...)` = run node logic + apply effects.
* `fx.dirty = true` -> world reconciles the node (link refresh + `Node::derive`).
* Timers carry tokens; node `epoch` invalidates all timers on power change.
* No identifier may be called `gen` (reserved in edition 2024).

## In progress / next

GREEN (88 tests): unit 40, tier1_l2 5, tier1_l3 5, tier1_cli 7, tier1_lab 8,
tier2_services 5, tier2_nat 2, tier2_ospf 7, tier2_switching 3, tier3_rip 4, tier3_hsrp 2.

DONE since last note: RIP (sim/rip.rs real: v1/v2, split horizon, poison, triggered updates,
`show ip rip database`), HSRP (sim/hsrp.rs real: RFC 2281 state machine, vMAC, resign,
`show standby [brief]`), EtherChannel compat/suspend, port-security sticky conversion.

OSPF notes: SPF is analytic per "flooding component" (routers joined by mutual FULL
adjacencies); a router keeps stale adverts (`node.ospf.learned`) of routers it lost silently
until its dead timer fires. `ospf_converging(now)` is true while a neighbour missed a hello.

Next, in order: telnet/ssh nested session test (tests/tier3_sessions.rs), full-feature
running-config round-trip test, demo.rs (demo_campus + 5 examples), fuzz test,
tests/ntc_validate.py + examples/dump.rs, dead-code cleanup, clippy -D warnings,
fmt (-p nocturne-netsim only!), final report via SubagentHandback.

Known decisions to report: ARP miss drops everywhere; `show ip dhcp binding` expiry is one
token (template limitation); NodeSpec gained vlan_dat + rsa_key_bits; PacketEvent gained
interface + parent; exec() returns device error text as Ok; real IOS ordering nat->route->acl;
single OSPF process; no L3 port-channels; console cable uses pseudo-ports Console/RS232;
channel members that find no partner do not forward (like `port-channel standalone-disable`);
RIP has no holddown (poison + triggered updates instead); HSRP timers fixed 3/10 s.

## Quality of service (added later)

Spec: `docs/dev/spec-netsim.md` (*Quality of service*) and `docs/dev/spec-qos.md`.

* Model: `device/config.rs` (`ClassMap`, `PolicyMap`, `PolicyClass`, `IfConfig::policy_in/out`).
  Command line: `device/ios/config_qos.rs` (modes `config-cmap`, `config-pmap`,
  `config-pmap-c`; the `class` command is in both policy modes because sub-modes only
  fall back to global configuration). Rendering: `runcfg.rs` (`policy_order`: children
  before the policy that uses them). Statistics: `show/qos.rs`.
* Engine: `sim/qos.rs` is pure (`plan` → `classify` → `evaluate`/`fifo` → `schedule`);
  `sim/flows.rs::tick` lays every flow out as a chain of stages (in/out of every hop,
  then the return traffic back), lets every point decide what it passes, and repeats
  until the rates stop moving (a drop upstream is not offered downstream; bounded by
  `MAX_ROUNDS` for paths that cross in a circle). Counters live in `Node::qos`, as
  fractions of octets; `Node::qos_account` folds a tick in and prunes what is no longer
  configured.
* Format: every line of `show policy-map interface` has a counterpart in Cisco's command
  reference; the spec has the table and the three lines built by analogy. The page text
  used is not in the repository (it is Cisco's); re-fetch it from the URL in the spec.
* Tests: `tests/tier3_qos.rs` (the two exact-text tests are what the `cisco_ios` profile's
  `qos` step is written against), `tests/demo.rs` (the campus edge), unit tests in
  `sim/qos.rs`. Float fuzz: counters are sums of products, so display and the
  interface drop counter round with a tiny margin (`show/qos.rs::whole`).
* Review round: the reference's own hierarchical example is now a test
  (`the_policies_of_the_reference_s_example_print_as_the_reference_prints_them`), which is
  where the priority burst's 50-byte step comes from; conservation is a generated-policy
  unit test (`every_octet_offered_is_sent_on_or_counted_as_exactly_one_kind_of_drop`);
  a policy is bounded at `POLICY_CLASSES` written out (three levels multiply, and `plan`
  writes a policy out on every tick); the flow tick keeps running after the last flow is
  removed until nothing shows its traffic (`World::traffic_lingers`).
* When breaking code on purpose to see a test fail: restore the file with a fresh
  modification time (or `touch` it). Cargo decides by mtime, and a file put back with its
  old one leaves the *broken* build in the shared target directory for every crate that
  depends on this one.
