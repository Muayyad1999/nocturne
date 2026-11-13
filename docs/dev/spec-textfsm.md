# Spec: `nocturne-textfsm` — TextFSM engine + bundled ntc-templates

Nocturne turns raw CLI output from ~70 network operating systems into structured
records. Rather than hand-writing parsers, it runs the community
[ntc-templates](https://github.com/networktocode/ntc-templates) library
(Apache-2.0, ~980 templates, each tested upstream against real device output)
on a TextFSM engine written in Rust. This crate is that engine plus the bundled
library.

## Public API (contract)

```rust
/// A compiled TextFSM template.
pub struct Template { /* … */ }

impl Template {
    pub fn parse(source: &str) -> Result<Template, TemplateError>;
    /// Value names in declaration order (the table header).
    pub fn header(&self) -> &[String];
    /// Run the state machine over CLI output and return the recorded rows.
    pub fn run(&self, input: &str) -> Result<Vec<Record>, RunError>;
}

/// One output row: value name (as written in the template, usually UPPERCASE) → value.
pub type Record = indexmap::IndexMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Text(String),
    List(Vec<String>),
}
impl Value {
    pub fn as_str(&self) -> &str;            // "" for List
    pub fn as_list(&self) -> &[String];      // empty slice for Text
    pub fn is_empty(&self) -> bool;
}

/// The bundled ntc-templates library.
pub mod ntc {
    /// Platforms present in the index, e.g. "cisco_ios", "arista_eos", "mikrotik_routeros".
    pub fn platforms() -> &'static [String];
    /// All bundled template names (file stem, e.g. "cisco_ios_show_version").
    pub fn template_names() -> impl Iterator<Item = &'static str>;
    pub fn source(name: &str) -> Option<&'static str>;
    /// Compiled lazily, cached for the life of the process.
    pub fn template(name: &str) -> Result<Option<std::sync::Arc<super::Template>>, super::TemplateError>;
    /// Resolve via the ntc `index` file: platform + the command as typed
    /// (abbreviations such as `sh ip int br` must resolve exactly as upstream does).
    pub fn find(platform: &str, command: &str) -> Option<&'static str>;
    /// Convenience: find + run. `Ok(None)` when no template covers the command.
    pub fn parse(platform: &str, command: &str, output: &str) -> Result<Option<Vec<super::Record>>, super::Error>;
    /// Upstream commit the bundle was taken from.
    pub fn upstream_revision() -> &'static str;
}
```

`Template` must be `Send + Sync`. Errors carry the template line number and a
readable message (`thiserror`).

## Engine semantics

Implement the TextFSM language exactly as the Python reference implementation
(`google/textfsm`, `textfsm/parser.py`) behaves — the upstream test corpus is
the arbiter. Points that are easy to get wrong:

* **Value** lines: `Value [Options] NAME (regex)`; options `Filldown`, `Key`,
  `Required`, `List`, `Fillup` (comma separated). The regex must be wrapped in
  parentheses; nested groups are allowed. A `List` value whose regex contains
  nested *named* groups records each match as a map in Python — represent that
  row item as a compact `key=value` joined string **only if** a bundled
  template needs it; otherwise ignore the feature.
* **States** start with `Start` (required); rule lines begin with one or two
  spaces/tab then `^`. `${NAME}` and `$NAME` substitute `(?P<NAME>regex)`;
  `$$` is end-of-line. Comment lines start with `#`.
* **Rule actions**: `-> [LineOp][.RecordOp] [NewState]` with line ops `Next`
  (default), `Continue`, `Error ["message"]`, and record ops `NoRecord`
  (default), `Record`, `Clear`, `Clearall`. `Continue` may not change state.
  `Error` aborts with the message. Special states `End` (stop immediately) and
  `EOF` (implicit `^.* -> Record` at end of input unless the template declares
  an empty `EOF` state).
* Matching is `re.match` semantics: anchored at the start of the line, not
  necessarily at the end. Lines are split on `\n`; strip a trailing `\r`.
* **Record**: a row is emitted only if every `Required` value is non-empty and
  at least one value is non-empty; after recording, non-`Filldown` values are
  cleared; `Fillup` back-fills earlier rows; `Clear`/`Clearall` semantics as
  upstream. `List` values accumulate across matches until recorded.
* Python `re` vs Rust: use the `regex` crate when a pattern compiles there and
  fall back to `fancy-regex` for look-around/back-references. Translate the
  Python-only constructs that actually occur in the bundled templates
  (`(?P<n>…)` is fine in both; check `\Z`, `(?i)` placement, possessive/atomic
  oddities, `\d` being Unicode in `regex` — force ASCII classes to match
  Python's behaviour on ASCII device output, and make `$` match only at the
  true end of line).

## The bundle

* Copy `ntc_templates/templates/` (every `*.textfsm` and the `index` file)
  from `/home/user/.cache/vendor/ntc-templates` into
  `crates/nocturne-textfsm/templates/`, with upstream `LICENSE` and a short
  `README.md` stating origin, licence and commit hash (`git -C … rev-parse HEAD`).
* Embed at compile time (no runtime file IO). Keep the binary cost small:
  a `build.rs` that packs all templates + index into one compressed blob is
  preferred over ~1000 separate `include_str!`s (you may add `flate2` or
  `zstd` as a build + runtime dependency in this crate's `Cargo.toml`, or
  `include_dir` if you measure that compression is not worth it — say which
  and why in the report).
* **Index resolution**: the `index` file is CSV-like with header `Template,
  Hostname, Platform, Command`; `Platform` and `Command` are regexes; the
  command column uses `[[…]]` to mark optional completions (`sh[[ow]] ip
  int[[erface]] br[[ief]]` expands to nested optional groups, as in
  `clitable.py`). First matching row wins. Some `Template` cells list several
  templates joined by `:` — use the first.

## Conformance (the important part)

The upstream repo ships a test corpus: `tests/<platform>/<command>/<case>.raw`
(real device output) with `<case>.yml` holding `parsed_sample:` — the expected
rows with **lower-cased** keys. Build a conformance runner:

* `tests/conformance.rs`: when env `NTC_TEMPLATES_DIR` is set (use
  `/home/user/.cache/vendor/ntc-templates`), walk every case, resolve the
  template through `ntc::find(platform, command-from-directory-name with `_`→space)`
  (fall back to the directory-derived template name when the index needs the
  original punctuation), run it, and compare to the YAML. Print a per-platform
  pass/fail table and the list of failing cases with the first differing
  row/field; fail the test if the pass rate drops below the level you reach
  (record it as a constant so regressions are caught). When the env var is
  unset the test is skipped with a note.
* Target: **≥ 99 % of cases pass**; every `cisco_ios`, `cisco_nxos`,
  `cisco_xr`, `cisco_asa`, `arista_eos`, `juniper_junos`, `mikrotik_routeros`,
  `huawei_vrp`, `hp_comware`, `hp_procurve`, `aruba_aoscx`, `fortinet`,
  `paloalto_panos`, `linux` case should pass. Investigate every failure to a
  root cause; fix engine bugs; for a genuine upstream-data problem, list the
  case in a documented skip-list with the reason.
* Also commit a small always-on fixture set (so `cargo test` is meaningful
  without the corpus): copy ~80 representative `.raw`/`.yml` pairs (≤ 600 KB
  total) into `crates/nocturne-textfsm/tests/fixtures/<platform>/<command>/`
  covering the platforms above and the commands Nocturne relies on most
  (version, interfaces, ip interface brief, vlan, mac table, arp, route,
  cdp/lldp neighbors detail, inventory, spanning-tree, etherchannel, ospf/bgp
  neighbors; RouterOS `interface print detail`, `ip address print`, `ip route
  print`, `ip neighbor print detail`, `ip arp print`…), and a test that runs
  them all.
* Unit tests for the engine semantics above (each option/action/state rule).

## Constraints

* Dependencies already declared: `regex`, `fancy-regex`, `indexmap`, `serde`,
  `serde_json`, `thiserror`; dev: `pretty_assertions`, `serde_yaml_ng`.
* No `unsafe`; `cargo clippy -p nocturne-textfsm -- -D warnings` clean.
* Performance: compiling a template once and parsing a 5,000-line `show
  interfaces` should take well under 100 ms in release; include a simple
  timing check in the report (not a flaky assertion).
