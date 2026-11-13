# Feature crates: shared conventions

Nocturne's backend is a small **core** (`crates/nocturne-app`) plus one crate
per feature. A feature crate depends on the core and the engine crates it
needs, never on another feature crate: features talk to each other through
the RPC surface (`app.rpc("config.backup", …)`) and events, so they can be
built, tested and replaced independently. `crates/nocturne-suite` wires them
together; the desktop shell and the server only see the suite.

Each feature has a spec in this directory. Sections a spec marks **contract**
(RPC names, parameter and result shapes, events) are fixed — the UI and the
assistant are written against them. Everything else is the author's call.

## Shape of a feature crate

```
crates/nocturne-<feature>/
  Cargo.toml          # workspace = true deps; nocturne-app; dev-dep nocturne-app with features = ["testing"]
  src/lib.rs          # pub fn init(builder: &mut AppBuilder)  + pub mod …
  NOTES.md            # running log while building (done / next / decisions)
```

```rust
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(builder, "<section>", &Settings::default());   // if it has settings
    builder.state(MyState::default());                                              // runtime state, read with app.state::<MyState>()
    builder.router.add("area.verb", |app: App, params: Params| async move { … });  // Params: Deserialize, result: Serialize
    builder.on_start(|app| app.spawn("area.worker", worker(app.clone())));          // background tasks (stopped at shutdown)
}
```

The lead engineer adds the crate to the workspace `Cargo.toml` and to
`nocturne-suite` — do not edit those. To run your crate's tests you only need
`cargo test -p nocturne-<feature>`.

## Core API you build on (`nocturne_app`)

Read the source for details; these are the entry points.

```rust
// app.rs
App: Clone;  app.rpc(method, json).await -> AppResult<Value>;  app.publish(topic, &payload);  app.subscribe() -> broadcast::Receiver<Event>;
app.store() -> &Arc<Store>;  app.vault() -> &Arc<Vault>;  app.data_dir() -> Option<&Path>;  app.state::<T>() -> Arc<T>;
app.audit(action, device_id: Option<&str>, message, detail: Value);            // every user-visible mutation
app.spawn(name, future);  app.shutdown_token();  app.local_access() / app.require_local_access(what)   // false in server mode
AppError::{not_found, invalid, conflict, unavailable, timeout, unauthorized, forbidden, internal, new(code, msg)}.with_details(json)

// store.rs — documents (JSON by collection + id), key/value, logs, time series, config versions
store.put(collection, id, &T) / get::<T> / require::<T>(collection, id, "what") / list::<T>(collection) / delete / count
store.kv_get::<T>(key) / kv_set / kv_delete
store.log_append(&LogEntry{ts, stream, device_id, severity, source, message, body}) / log_query(&LogQuery) / log_prune(stream, before)
store.series_id(device_id, metric, instance) / samples_insert(&[(series_id, ts_ms, value)]) / samples_query(..) / sample_latest / series_of(device_id)
store.config_insert(..) / config_versions(device_id, kind, limit) / config_latest(device_id, kind) / config_get(id) -> (ConfigVersion, String)
store.with(|conn: &rusqlite::Connection| …) / store.transaction(..)            // only for a feature's own tables; prefer documents

// inventory.rs / model.rs
inventory::{get(app, id), find(app, id_or_name_or_address), list(app, &DeviceFilter), state_of(app, id), update_state(app, id, |state| …), create, update, delete}
model::{Device{id,name,address,profile,transport,port,credential_id,jump_device_id,site,role,tags,notes,monitor,lab,protected,..}, DeviceState, DeviceRow, Transport, LabRef, Credential}

// credentials.rs / vault.rs — secrets never leave through RPC, events, logs or job output
credentials::resolve(app, credential_id) -> AppResult<Resolved>;   app.vault().put(id, secret) / get(id) / delete(id)   // ids "<feature>:<thing>:<part>"

// profiles.rs — device types
profiles::{registry(app) -> Arc<Registry>, platform(app, profile_id) -> AppResult<Arc<Platform>>, expressions(app)}

// connect.rs — the only way to reach a device
connect::open(app, &device, &OpenOptions{cols, rows, term}) -> AppResult<nocturne_transport::Connection>      // raw byte stream
connect::cli(app, device_id, CliOptions::default()) -> AppResult<Cli>                                           // logged in, enabled, paging off
Cli::{read(cmd) -> CommandOutput   // refuses anything the platform policy does not class read-only
     classify(cmd) -> Classification{class: ReadOnly|Change|Dangerous, reason},
     session() -> &mut nocturne_drivers::session::CliSession   // exec / configure / save / fetch_config — unchecked: for the change engine only
     hostname(), take_transcript(), close().await, device, platform, info}
connect::exec(app, device_id, &[commands]) -> AppResult<Vec<CommandOutput>>    // read-only batch;  connect::verifier(app) for ad-hoc SSH
// one automated session per device at a time (configurable): a second `cli()` waits, then fails with `unavailable`.

// jobs.rs — anything longer than a click
let job = jobs::start(app, JobSpec::new("config.backup", "Back up 12 devices").devices(&ids).origin("user"), |ctx| async move {
    let results = ctx.for_each(ids, 4, |ctx, id| async move { … }).await;       // bounded concurrency, advances progress, honours cancel
    ctx.info("…"); ctx.device(&id, "warn", "…"); ctx.check()?;                   // log lines; Err(cancelled) once cancelled
    Ok(json!({ … }))                                                            // becomes job.result
});
return Ok(jobs::started(&job));                                                 // RPC result {job_id}; UI follows `jobs.changed` / `jobs.log`

// collect.rs — structured device data (facts, interfaces, neighbors, vlans, mac_table, arp, routes, health, inventory)
collect::run(app, device_id, &[Kind]) -> AppResult<Snapshot>       // over a new CLI session; stores what it read, publishes `collect.updated`
collect::run_on(app, &mut cli, &[Kind]) -> AppResult<Snapshot>     // over a session you already hold
collect::latest(app, device_id, Kind) -> AppResult<Option<Stored{device_id, kind, collected_at, source, rows: Vec<Value>}>>
collect::previous(app, device_id, Kind)                            // the generation before `latest` — for "what changed since last time"
collect::latest_all(app, Kind) -> AppResult<Vec<Stored>>;  stored.typed::<T>() -> Vec<T>            // T = nocturne_drivers::model::{Interface, Neighbor, …}
collect::store(app, device_id, Kind, rows, source) / store_many(..) // for feeders other than the CLI (SNMP, the lab); tests use it to hand-build data
RPC: collect.get {id, kind, previous?} / collect.status {id} / collect.run {device_ids, kinds?} -> {job_id};  event collect.updated {device_id, kinds, at, source}

// settings.rs
settings::declare(builder, "section", &Defaults);  settings::get::<T>(app, "section");  RPC settings.get/set/all;  event settings.changed

// util.rs
util::{now_ms(), new_id(), matches_query(haystack, query)}
```

Engines (read their specs under `docs/dev/spec-*.md`): `nocturne-drivers`
(profiles, CLI session, `hier` config diff/remediation, `policy`, `model`,
collectors), `nocturne-transport`, `nocturne-snmp`, `nocturne-textfsm`,
`nocturne-netprobe`, `nocturne-tools`, `nocturne-netsim`, `nocturne-ai`.

## Rules every feature follows

1. **No fabricated data.** A read that failed is reported as failed. Empty
   and "unknown" are different things and the result types must be able to
   say which. No sample data, no placeholders, no optimistic defaults.
2. **Nothing changes a device outside the change pipeline.** Only
   `nocturne-changes` may call `Cli::session()` to configure. Everything else
   uses `Cli::read` / `connect::exec` (policy-checked read-only) or creates a
   change through `changes.create`. The terminal is the human's own hands and
   is exempt.
3. **Secrets stay in the vault.** Never in documents, events, logs, job
   output, error messages or anything sent to an AI model. Device output that
   may contain secrets (configurations!) is redacted with
   `nocturne_ai::Redactor` before it is shown to a model, and by default
   before it is shown at all (`redact: false` must be asked for explicitly).
4. **Everything a user can do is an RPC method**; everything they should see
   happen is an event. No hidden state the UI cannot reach. Method names are
   `area.verb` / `area.thing.verb`, snake_case fields, timestamps in ms since
   the epoch, ids are opaque strings (`util::new_id()`).
5. **Lists scale.** A campus has thousands of interfaces and millions of log
   lines: paginate (`limit`, cursor), filter in SQL or while streaming, never
   load a whole table to return 50 rows.
6. **Background work is polite.** Bounded concurrency towards devices
   (default 4–8 in parallel), back-off on failure, no polling tighter than the
   setting says, everything stops at `app.shutdown_token()`.
7. **Errors are for the person at the keyboard**: what happened, to which
   device, and what to do about it. `AppError.code` is machine-stable.
8. **Lab devices are devices.** Anything that works on a real device works on
   a simulated one (`Transport::Sim`), and results always carry enough to
   label lab data as lab (`device.lab.is_some()`).
9. **Audit**: every mutation that a user would want to find later calls
   `app.audit(...)` with a sentence in the past tense.
10. **Protected devices** (`device.protected`) are never changed by automation
    or the assistant without an explicit, per-change human approval.

## Tests

`nocturne_app::testing` (enable the `testing` feature in dev-dependencies)
gives an in-memory app and fake devices:

```rust
let mut builder = nocturne_app::testing::builder();      // core modules registered
my_feature::init(&mut builder);
let lab = nocturne_app::testing::lab(builder, &["SW1", "SW2"]).await;   // devices in inventory, profile `test_ios`, credential `lab`
let id = lab.id("SW1");
lab.fake("SW1").with(|device| { device.outputs.insert("show ip route".into(), "…".into()); device.reject.push("bogus".into()); });
lab.app.rpc("my.method", json!({ "device_id": id })).await?;
assert!(lab.fake("SW1").running_config().contains("ntp server 192.0.2.1"));
```

`FakeDevice` is an IOS-flavoured switch with modes, an enable secret, a real
hierarchical running-config that configuration lines change (including
`switchport trunk allowed vlan add/remove`), startup-config and `write
memory`, a reload timer, `ping`, canned outputs, rejected lines
(`reject`), and a line that cuts the session (`drop_on`). Read
`crates/nocturne-app/src/testing.rs`. If you need a behaviour it lacks, say so
in your report rather than faking it inside your own crate.

Every RPC method has at least one test through `app.rpc(...)` (the real
wire shape), including its failure modes. Tests must not need the network,
fixed ports, real time (`tokio::time::pause` where timing matters) or root.

## TypeScript types

Write `ui/src/api/types/<area>.ts` mirroring every param and result struct of
your RPC surface (field names as the backend's, `snake_case`; optional = `?`;
timestamps `number`), with a one-line doc comment per method listing its
name, params type and result type. The UI authors work from that file. Do
not touch anything else under `ui/`.
