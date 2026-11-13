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

- [x] util (net, mac, rng, regex, crypt, clock, text) — unit tested, all green
- [x] api.rs public types / catalog.rs / ifname.rs
- [x] device/config.rs — config model (personality-neutral)
- [x] sim/frame.rs (frames/packets + inspector text), sim/fx.rs (effects), sim/syslog.rs,
      sim/node.rs (Node/IfState), sim/mod.rs (World, queue, transmit/arrive, traces)
- [ ] sim/reconcile.rs (link refresh + Node::derive), sim/port.rs (rx dispatch, l3 tx)
- [ ] sim/switching.rs, sim/stp.rs
- [ ] sim/arp.rs, sim/rib.rs, sim/ip.rs (+icmp), sim/procs.rs (ping etc.)
- [ ] placeholder state for tier 2/3 modules so the tree compiles:
      acl, dhcp, nat, neighbors, ospf, rip, hsrp, flows, faults, probe
- [ ] CLI engine (device/cli: grammar, line editor, pager, modifiers)
- [ ] IOS exec/config/show (tier 1), runcfg round trip, host shell
- [ ] lab.rs (public API), spec.rs, snapshot/tables/metrics
- [ ] Tier 2: OSPF, DHCP, NAT, ACL, EtherChannel, CDP/LLDP, traceroute, tracing, demo
- [ ] Tier 3: RIP, HSRP, port security, telnet/ssh, long-tail shows
- [ ] tests (scenario, fuzz, determinism), tests/ntc_validate.py, examples/dump.rs

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

writing sim/reconcile.rs, port.rs, switching.rs, stp.rs (tree does not compile until the
remaining sim modules exist)
