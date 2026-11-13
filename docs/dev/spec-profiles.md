# Spec: collectors, platform detection and the built-in device profiles

Nocturne supports a device type through a **profile**: a YAML document that
says how to talk to it and how to map what it prints onto the common model.
The profile format, its validation, the CLI session driver, the parsers and
the expression language already exist in `crates/nocturne-drivers` (read
`src/profile.rs`, `src/session.rs`, `src/parse.rs`, `src/expr.rs`,
`src/model.rs`, `src/policy.rs`, `src/hier.rs` first — they are the ground
truth for everything below).

What is missing, and what this spec covers:

1. `src/collect.rs` — run a profile's `collect:` steps and produce model data.
2. `src/detect.rs` — work out which profile an unknown device needs.
3. The built-in profile pack under `crates/nocturne-drivers/profiles/`, with
   tests grounded in real device output.
4. A user-facing guide, `docs/profiles.md`: how to add a device type.

Project rule that shapes all of it: **no fabricated data**. A read that fails
or a kind a platform does not define is reported as such — never filled with
placeholders, never silently empty.

## 1. `collect.rs`

```rust
/// Anything that can run one read-only command and return its cleaned output.
#[async_trait::async_trait]
pub trait Runner: Send {
    async fn run(&mut self, command: &str) -> Result<String, DriverError>;
}
impl Runner for CliSession { /* exec(); a device-side rejection becomes DriverError::Rejected */ }

/// A `Runner` over recorded output, for tests and for "paste the output" tools.
pub struct Recorded { /* command → output; unknown command → DriverError::Rejected */ }
impl Recorded {
    pub fn new() -> Recorded;
    pub fn with(self, command: &str, output: &str) -> Recorded;
    pub fn commands_run(&self) -> &[String];
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepReport {
    pub kind: Kind,
    pub command: String,
    pub ok: bool,
    /// Rows this step contributed after `when` filtering.
    pub rows: usize,
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub skipped: bool,          // optional step that failed, or `when`-less step not needed
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    // `None` = not collected (not requested, not defined for the platform, or failed — see `problems`).
    pub facts: Option<Facts>,
    pub interfaces: Option<Vec<Interface>>,
    pub neighbors: Option<Vec<Neighbor>>,
    pub vlans: Option<Vec<Vlan>>,
    pub mac_table: Option<Vec<MacEntry>>,
    pub arp: Option<Vec<ArpEntry>>,
    pub routes: Option<Vec<Route>>,
    pub health: Option<Health>,
    pub inventory: Option<Vec<InventoryItem>>,
    pub problems: Vec<Problem>,
    pub steps: Vec<StepReport>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Problem { pub kind: Kind, pub code: ProblemCode, pub message: String }
#[serde(rename_all = "snake_case")]
pub enum ProblemCode { Unsupported /* the profile defines no steps for this kind */, Failed /* every required step failed */, Partial /* some rows or fields could not be mapped */ }

impl Snapshot {
    pub fn has(&self, kind: Kind) -> bool;
    /// The collected rows of one kind as JSON (single-row kinds give a one-element array).
    pub fn rows(&self, kind: Kind) -> Option<Vec<serde_json::Value>>;
}

pub struct Collector { /* expressions: Arc<Expressions> */ }
impl Collector {
    pub fn new(expressions: Arc<Expressions>) -> Collector;      // Registry::expressions()
    /// Collect several kinds over one session. A command that appears in the
    /// steps of several kinds is run once.
    pub async fn collect(&self, runner: &mut dyn Runner, platform: &Platform, kinds: &[Kind]) -> Snapshot;
    /// Offline: parse and map one command's output with one step. Used by the
    /// profile editor's live preview and by the tests.
    pub fn map_output(&self, platform: &Platform, step: &CollectStep, output: &str) -> Result<Vec<Row>, String>;
}
```

Semantics (write them into the module docs and `docs/profiles.md`):

* Steps of a kind run in order. Each step: run the command → parse with
  `step.parser` → evaluate `when` per row → build the model row from `map`
  (model field → expression; the parsed row's fields are the variables, plus
  `row` holding the whole parsed row, and `facts` when facts were collected
  earlier in the same call). A field whose expression evaluates to
  undefined/none/empty is left out of the row rather than set to `""`.
  An empty `map` passes the parsed row through unchanged (fields that the
  model does not know are dropped at deserialisation).
* `parser: {ntc: auto}` resolves the template from `profile.ntc_platform` +
  the command through `nocturne_textfsm::ntc` (`find`/`parse`; read
  `docs/dev/spec-textfsm.md` — the bundle has ~980 templates). `{ntc: name}`
  names one explicitly. ntc value names are upper-case (`HARDWARE`,
  `SERIAL`); expose them to expressions lower-cased **and** as written. A
  template that cannot be found is a step error.
* Multi-row kinds: rows of later steps are **merged** into existing rows when
  `merge_on` is set (rows are matched on that model field; for interface
  names compare with `ifname::same`); fields already present and non-empty
  are kept, missing ones are filled. Rows with no match are appended only if
  `merge_on` is unset. Single-row kinds (`facts`, `health`) merge every step
  into the one row the same way.
* A failing step: `optional: true` → recorded in `steps`, collection goes on.
  Otherwise the kind fails only if **no** step produced rows; if an earlier
  step succeeded the kind is kept and a `Partial` problem recorded.
* Rows become model structs through serde (`model.rs` has lenient
  deserialisers). A row that does not deserialise is dropped and counted in
  one `Partial` problem per kind that quotes the first offending row's error.
* Interface names are canonicalised (`ifname::long`) in every model field
  that holds one (`Interface.name`, `Neighbor.local_interface`,
  `MacEntry.interface`, `ArpEntry.interface`, `Route.interface`, …) so the
  topology builder can join them.
* The collector never runs a command the platform's policy does not classify
  as read-only (`Platform::policy().is_read_only(cmd)`); such a step is a
  profile bug: fail it with a message that says so. Profile validation
  (`Registry::add`) should reject it up front — add that check there too.
* `Registry::add` must also check every `map`/`when` expression compiles and
  every explicitly named ntc template exists, and report all problems at once.

## 2. `detect.rs`

A user adds "10.0.0.1" and nothing else. Nocturne has three chances to
identify it, cheapest first:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Detection {
    pub profile_id: String,
    pub method: DetectMethod,        // Snmp | Banner | Probe
    pub evidence: String,            // the text that matched, trimmed to ~200 chars
}
/// From SNMP sysDescr / sysObjectID (the app asks SNMP; this crate only matches).
pub fn from_snmp(registry: &Registry, sysdescr: &str, sysobjectid: &str) -> Option<Detection>;
/// From the SSH identification string, pre-auth banner and login screen.
pub fn from_banner(registry: &Registry, text: &str) -> Option<Detection>;
/// Log in with the permissive `generic` profile, run the registry's probe
/// commands (each distinct command once, in priority order, stopping at the
/// first profile whose probe matches), then re-bind the session to the
/// detected platform: its prompt patterns, enable, and init commands.
pub async fn probe_session(
    channel: Box<dyn Channel>, registry: &Registry, credentials: &Credentials,
    options: &SessionOptions, banner_hint: Option<&str>,
) -> Result<(CliSession, Detection), DriverError>;
```

`probe_session` needs `CliSession` to support re-binding to another platform
(`pub async fn rebind(&mut self, platform: Arc<Platform>, credentials, options)`
or similar) — add it to `session.rs` with tests. A probe command the device
rejects must not poison the session (resynchronise on the prompt). When
nothing matches, return the session bound to `generic` and a `Detection` for
`generic` with evidence saying nothing matched — the user can still use the
terminal and run read-only commands.

## 3. Profiles

Add a field to `Profile`: `maturity` — `verified` (commands, prompts and
parsers checked against recorded output of the real platform in this
repository's tests), `community` (written from vendor documentation and
other tools' definitions; not yet checked against a device) or `experimental`.
The UI shows it; `community`/`experimental` profiles never get write access by
default. Be honest when assigning it.

The first wave (this task): **`generic`**, **`cisco_ios`** (IOS and IOS-XE),
**`mikrotik_routeros`** (v6 and v7), **`linux`** (iproute2 with `-j` JSON,
`/proc`, `uname`, `hostnamectl`; shell prompt handling incl. `sudo -n` for
privileged reads is out of scope — read-only as the login user). An abstract
base (`_cli_ios_like`) that IOS-style platforms extend is encouraged where it
removes repetition.

Every non-abstract profile fills, as far as the platform allows:

* `detect`: SSH banner patterns, `sysDescr` patterns, `sysObjectID` prefixes
  (enterprise numbers), at least one probe command + pattern, a priority.
* `cli`: prompt / user-mode / config-mode patterns that match **real**
  prompts including awkward hostnames (dots, dashes, VRF/context decorations,
  `(config-if-range)#`, RouterOS `[admin@MikroTik] /interface>`, Junos
  `user@host>` / `#`, HA suffixes) and do not match ordinary output lines;
  login prompts; `init` commands that disable paging and widen the terminal;
  a pager pattern as a fallback; exec error patterns; `enable` where it
  exists; interactive `answers` for known questions (e.g. `[confirm]`).
* `config`: syntax, enter/exit, commit/abort where the platform has a
  candidate config, save, running/startup commands, `ignore` patterns so two
  fetches of an unchanged device compare equal (timestamps, "Last configuration
  change", uptime lines, RouterOS `# jan/02/1970 …` header), config error
  patterns, the platform's dead-man switch as `guard` (`reload in` on IOS,
  `commit confirmed` on Junos, Safe Mode is interactive-only on RouterOS — say
  so instead of faking one), extra `secrets` patterns.
* `policy`: `read_only` and `dangerous` patterns for this CLI (default-deny:
  anything not recognised as read-only is a change). `dangerous` covers what
  can take a device or its config away (`reload`, `erase`, `format`, `delete`
  of system files, `write erase`, `/system reset-configuration`, `debug all`,
  factory reset, …).
* `collect`: as many kinds as the platform exposes — `facts`, `interfaces`
  (state, description, MAC, MTU, speed/duplex, IPs, VLAN/mode where
  applicable, counters), `neighbors` (LLDP and the vendor protocol — CDP,
  MNDP…), `vlans`, `mac_table`, `arp`, `routes`, `health` (CPU, memory,
  temperature, uptime), `inventory` (modules, transceivers, PSUs, fans with
  serials). Prefer one robust command per kind; add optional steps for
  enrichment.
* `snmp`: OIDs for CPU / memory / temperature of that vendor.

Sources to consult for facts (commands, prompts, behaviours — not code to
copy): the vendor's command references as you know them; Netmiko's
per-platform session classes (`pip download netmiko --no-deps`, unzip, read
`netmiko/<vendor>/`; MIT) for paging/enable/config-mode/save behaviour;
NAPALM's getters (Apache-2.0) for which commands produce which facts; and
the ntc-templates index and raw captures at
`/home/user/.cache/vendor/ntc-templates` (Apache-2.0; ~1900 real device
outputs under `tests/<platform>/<command>/*.raw` with the expected parse in
the `.yml` beside each).

## 4. Tests: grounded in real output

A data-driven harness so that adding a profile's tests needs no Rust:

```
crates/nocturne-drivers/tests/profiles/<profile_id>/
  prompts.yml            # lines that MUST be recognised as exec / user / config prompts, and lines that MUST NOT
  errors.yml             # sample rejected-command outputs (exec and config) that MUST be detected, and clean outputs that MUST NOT
  policy.yml             # commands with the class they must get: read_only / change / dangerous
  detect.yml             # banners / sysDescr / sysObjectID / probe outputs that must (or must not) select this profile
  captures/<case>/       # one directory per recorded device
    <command slug>.txt   #   raw output of each command the collectors run (real captures; note the source in SOURCE.md)
    expect.yml           #   expected model data per kind (exact row count; listed fields must match exactly)
  config/<case>.txt      # a real running-config: must survive clean_config idempotently, parse with the profile's syntax,
                         #   and `remediate(cfg, cfg)` must produce no commands
  session/<case>.yml     # optional scripted transcript for a full login → init → exec → config → save run through CliSession
```

One test function walks every directory and reports all failures with the
profile, file and field. Captures come from the ntc-templates corpus wherever
it has the command (copy the `.raw` verbatim and say which file it was in
`SOURCE.md`); where it has none, a capture may be written by hand **only**
from documented output formats, and must be marked `synthetic: true` in
`expect.yml` — a profile whose collectors are only covered by synthetic
captures cannot be `maturity: verified`. Expected values are derived by
reading the raw output, not by running the collector and pasting what it
printed.

Also required: unit tests for the collector semantics (merge, optional steps,
partial failure, policy refusal, command de-duplication, undefined fields),
for detection (priority, probe ordering, rejected probes, re-binding), and
that `Registry::builtin()` loads the whole pack with zero errors.

## 5. `docs/profiles.md` (user-facing)

How to support a new device type without rebuilding Nocturne: the file
format field by field with a worked example built up in steps (generic →
prompts → paging → facts → interfaces), every parser with an example, the
expression language (filters and tests in `expr.rs`), how `extends` merges,
how to test a profile against pasted output, the maturity levels, where user
profiles live, and a checklist for contributing a profile. Write it for a
network engineer, not for a Rust developer. Every example in it must be a
real, loadable profile fragment — add a test that extracts the fenced `yaml`
blocks marked `# profile` and loads them.

## Constraints

* Work only in `crates/nocturne-drivers/` and `docs/profiles.md`. Add
  `nocturne-textfsm.workspace = true` to the crate's dependencies (the crate
  is finished: 1897/1897 upstream conformance).
* Existing public API stays source-compatible (the app crate uses `Registry`,
  `Platform`, `CliSession`, `Channel`, `Credentials`, `SessionOptions`,
  `hier::*`, `policy::*`, `model::*`); additions are fine.
* All 61 existing tests keep passing; `cargo clippy -p nocturne-drivers
  --all-targets -- -D warnings` clean; `cargo fmt -p nocturne-drivers`.
