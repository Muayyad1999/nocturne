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

## Done

- [x] util (net, mac, rng, regex, crypt, clock, text) — unit tested
- [x] api.rs / catalog.rs / ifname.rs / device/config.rs (config model) / device/hostcfg.rs
- [x] sim core written (NOT YET COMPILED TOGETHER): frame, fx, syslog, node, mod (World),
      reconcile, port, switching, stp, arp, rib, ip, acl, procs (ping/connect/traceroute/dns),
      probe (traces + instant probes on scratch clone), flows, faults, edit
- [x] placeholders (to be replaced): sim/{nat,dhcp,neighbors,ospf,rip,hsrp}.rs
- [x] device/cli/grammar.rs (pattern DSL -> NFA; parse/help/complete) + tests
- [x] device/cli/mod.rs (Console, Session, line discipline, pager, nested sessions)
- [x] device/ios/mod.rs (IosShell: modes, prompts, login, dispatch, output filters, help)
- [ ] device/ios/{exec,config,config_if,config_sub,help,runcfg,boot}.rs + show/*
- [ ] device/host.rs (host shell)
- [ ] lab.rs (public API), spec.rs, views (snapshot/tables/metrics)
- [ ] first full compile + tier-1 scenario tests
- [ ] Tier 2: OSPF, DHCP, NAT, EtherChannel show, CDP/LLDP, tracing polish, demo
- [ ] Tier 3: RIP, HSRP, port security shows, long-tail shows
- [ ] fuzz + determinism tests, tests/ntc_validate.py, examples/dump.rs

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

Tier-1 code compiles; tests green so far: tests/tier1_l2.rs (5), tier1_l3.rs (5),
tier1_cli.rs (7), unit tests (36). `examples/console.rs` = scratch console driver
(`cargo run --example console -- "en" "show vlan" ...`, NODE=R1|PC1 selects device).

Next: tests/tier1_lab.rs (config round trip, power cycle, spec round trip, flows, faults,
traces, determinism, host shell, API errors). Then replace placeholders: neighbors
(CDP/LLDP), dhcp, nat, ospf, rip, hsrp + show/protocols.rs (table entries were removed
from show/mod.rs until then), demo.rs; fuzz test; ntc_validate.py + examples/dump.rs;
clippy + fmt (-p nocturne-netsim only!).
