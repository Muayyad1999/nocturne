# nocturne-lab — working notes

Spec: `docs/dev/features/lab.md` (the RPC table and the events are a contract).
Conventions: `docs/dev/features/README.md`. Engine: `crates/nocturne-netsim` (fixed; never edit).

Build (always scoped, always this target directory):

```bash
CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo test -p nocturne-lab
CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo clippy -p nocturne-lab --all-targets --no-deps -- -D warnings
CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/features cargo fmt -p nocturne-lab
```

## Layout

| file | what is in it |
|---|---|
| `lib.rs` | `init`: settings, the host profile, state, RPC, the console connector |
| `model.rs` | `LabRecord` (the stored lab), `Settings`, `LabView`/`LabSummary`, store helpers, name rules, `LabError` → `AppError` |
| `runtime.rs` | `LabsState`, `RunningLab`, start/stop, the ticker, the capture ring, autosave, "unsaved" tracking, `edit` |
| `console.rs` | `SimConnector` (as `Opener`) and the task that bridges a console to a `Connection` |
| `devices.rs` | the `nocturne_sim_host` profile and inventory sync |
| `expose.rs` | `labs.expose`/`exposed`/`unexpose` over the transport's SSH and Telnet servers |
| `twin.rs` | `labs.twin`: a lab from collected data and backups, with the fidelity accounting |
| `api.rs` | every `labs.*` method |

## Decisions

* **Syslog goes to `logs.ingest` by name.** The ticker groups the simulator's syslog
  lines by device and calls `logs.ingest {device_id, lines}` (awaited in the ticker, so
  lines keep their order). `unknown_method` → today's direct `store.log_append`; any
  other error is logged by code only (the message is another feature's and may quote
  the line) and nothing is written around it. `tests/network.rs` registers a stand-in
  `logs.ingest` on the builder's router; do not add a dependency on `nocturne-logs`.
  A device's lines go over in calls of at most `INGEST_LINES_PER_CALL` (the pipeline
  refuses more than 10,000 in one call whole); the unit tests beside `deliver_syslog`
  in `runtime.rs` cover the flood and the line of a node that has no device.
* **A flow's `dscp` needs no code here.** `labs.flow.*`, import and export pass
  `nocturne_netsim::FlowSpec` through as it is; the field is `Option<u8>` and is not
  serialised when unset, so old lab documents round-trip byte for byte.

* **No `App` in module state.** `ConnectState` and this crate's state live in the
  app's state map, so an `App` held there would make `Arc<Inner>` immortal and the
  data-directory lock would never be released (the persistence test restarts an app
  on the same directory). The console connector therefore holds an mpsc sender, and
  a task started in `on_start` owns the `App` and answers open requests. The expose
  reaper holds only `Arc<ExposeState>` and a `CancellationToken`.
* **One `parking_lot::Mutex<Lab>` per running lab**, reached only through
  `RunningLab::with`. The guard is `!Send`, so "never held across an await" is
  guaranteed by the compiler rather than by discipline.
* **Ticker**: one task per running lab; `tokio::time::interval(tick_ms)` advances
  `tick_ms × speed × 1000` µs of simulated time, drains `take_events`, then does all
  the I/O (syslog, events, autosave) with the lock released. Consoles are woken by a
  `watch` channel bumped at the end of every tick.
* **Console latency**: input is applied to the simulator at once, followed by 2 ms of
  simulated time (unless the lab is paused) and an immediate output drain, so an echo
  never waits for a tick.
* **"Unsaved" means "a restart would lose this"**: a node is unsaved when its
  running-config differs from the *baseline* — what it rendered just after it last
  booted from its startup-config. Comparing running against startup text does not
  work, because a lab that ships a hand-written `startup_config` renders a fuller
  running-config that is not an edit. The baseline is re-taken when the startup-config
  changes (`write memory`) and when a node reboots. `write memory` is not an event the
  simulator announces, so the nodes somebody typed on are re-measured each tick, and
  every node every 40 ticks.
* **Stopped labs are edited through a temporary `Lab`** (load from spec, apply,
  `to_spec`, store): all of the simulator's validation applies and no running
  configuration is invented. Running labs are edited in place and written back.
* **After an edit, `runtime::settle_devices` writes the running lab back first** and
  only then syncs the inventory — otherwise a node added to a running lab would get no
  device (the stored spec would not know about it yet).
* **`labs.node.config_set`** does what an engineer would, because the simulator has no
  setter for a startup-config: `erase startup-config`, `reload`, type the configuration
  in, `write memory`. The by-product is a per-line accepted/rejected report.
* `labs.changed` is debounced (250 ms), `labs.tick` is at most 4/s and only when a
  link's state, lights or rates changed, `labs.packets` batches at most 10/s.
* Lab names are unique: device names are `<lab>/<node>` and must not collide.
* Lab devices carry no address and are never polled (`monitor.icmp`/`snmp` off): a lab
  address is not a real address.

## Status

Done and tested: settings, store record, runtime (start/stop/ticker/autosave/capture
ring/unsaved), consoles (`SimConnector` + bridge + per-node close), inventory sync and
the `nocturne_sim_host` profile, every `labs.*` method, expose over SSH and Telnet,
`labs.twin`, the four events, `ui/src/api/types/lab.ts`, 38 tests.

Left out on purpose: `design.generate`, `design.create_lab`, `design.export` — the
`nocturne-design` crate is an empty stub.

Wanted from elsewhere (worked around, not patched):

* `nocturne-netsim`: `Lab::set_name`, `Lab::set_startup_config(node, text)`, and a
  public read of a node's `unsaved` flag (the simulator already tracks it).
* core: nothing to carry "this lab was started for your session" into a session title.
