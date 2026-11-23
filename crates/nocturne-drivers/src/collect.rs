//! Collecting structured data: run a profile's `collect:` steps and map what
//! the device prints onto the common [`model`](crate::model).
//!
//! A profile lists, per kind of data (`facts`, `interfaces`, `routes`, …), the
//! steps that produce it. [`Collector::collect`] runs them over anything that
//! can run a command — a live [`CliSession`], or [`Recorded`] output for
//! tests and "paste the output here" tools — and returns a [`Snapshot`].
//!
//! # What a step does
//!
//! 1. **Run** `command`. A command that several steps (of the same kind or of
//!    different kinds) use is run once per call.
//! 2. **Parse** the output with the step's `parser` into rows.
//! 3. **Filter**: keep a row only when `when` is truthy.
//! 4. **Map**: build a model row from `map` (model field → expression). The
//!    parsed row's fields are the variables, plus `row` (the whole parsed
//!    row) and `facts` (when facts were collected earlier in the same call —
//!    facts are always collected first). TextFSM value names are available
//!    as written (`HARDWARE`) and lower-cased (`hardware`). A dotted field
//!    name fills a nested field (`counters.in_errors`, `extra.image`). A
//!    field whose expression gives nothing — undefined, none, an empty
//!    string or an empty list — is left out, never set to `""`. An empty
//!    `map` passes the parsed row through; fields the model does not have are
//!    dropped.
//!
//! # How steps combine
//!
//! Steps of a kind run in order.
//!
//! * A step **without** `merge_on` creates rows: its rows are appended.
//! * A step **with** `merge_on: <field>` enriches rows: each of its rows is
//!   merged into the existing rows that have the same value in that field
//!   (interface names are compared the way [`ifname::same`] does). It never
//!   creates rows, so a row that matches nothing is dropped.
//! * Merging keeps what is already there: a field that is present and not
//!   empty is kept, a missing one is filled. Fields that are lists in the
//!   model (`ipv4`, `ipv6`, a VLAN's `interfaces`) collect the items of every
//!   step instead.
//! * `facts` and `health` are a single row: every row of every step is merged
//!   into it, first value wins. A step that could not add anything — every
//!   field it maps is already filled — is not run and reported as skipped.
//!
//! # When things go wrong
//!
//! Nothing is invented and nothing fails silently:
//!
//! * A kind the profile has no steps for is [`ProblemCode::Unsupported`].
//! * A failing step with `optional: true` is recorded in
//!   [`Snapshot::steps`] and collection goes on.
//! * If a required step fails but the rows could still be created, the kind
//!   is kept and a [`ProblemCode::Partial`] problem says what is missing.
//! * If no row-creating step succeeded, the kind is `None` and
//!   [`ProblemCode::Failed`] quotes every step's error.
//! * A row that cannot be read into the model is dropped, and a field whose
//!   expression fails is left out; each is counted in a `Partial` problem
//!   that quotes the first error.
//! * The collector only runs commands the platform's policy classifies as
//!   read-only. Anything else is a bug in the profile and fails the step
//!   without touching the device.
//! * Once the session is lost (timeout, disconnect), the remaining steps are
//!   not attempted.
//!
//! Interface names are canonicalised with [`ifname::long`] in every model
//! field that holds one of the device's own interfaces, so that `Gi1/0/1`
//! from a MAC table joins `GigabitEthernet1/0/1` from `show interfaces` —
//! unless the profile says `interface_names: verbatim`. A neighbour's
//! `remote_interface` belongs to another device, possibly of another
//! platform, and is kept as it was advertised; compare it with
//! [`ifname::same`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use indexmap::IndexMap;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::expr::{Context, Expressions};
use crate::ifname;
use crate::model::{
    ArpEntry, Facts, Health, Interface, InventoryItem, Kind, MacEntry, Neighbor, QosClass, Route,
    Vlan,
};
use crate::parse::{self, Row};
use crate::policy::CommandClass;
use crate::profile::{CollectStep, InterfaceNames, Parser, Platform};
use crate::session::{CliSession, DriverError};

/// Anything that can run one read-only command and return its cleaned output.
#[async_trait]
pub trait Runner: Send {
    async fn run(&mut self, command: &str) -> Result<String, DriverError>;
}

#[async_trait]
impl Runner for CliSession {
    /// Runs the command with [`CliSession::exec`]. Output that matches one of
    /// the platform's error patterns becomes [`DriverError::Rejected`].
    async fn run(&mut self, command: &str) -> Result<String, DriverError> {
        let output = self.exec(command).await?;
        match output.error {
            Some(error) => Err(DriverError::Rejected {
                command: command.to_string(),
                error,
            }),
            None => Ok(output.output),
        }
    }
}

/// A [`Runner`] over recorded output, for tests and for "paste the output" tools.
///
/// A command without recorded output is rejected, the way a device rejects a
/// command it does not know.
#[derive(Clone, Debug, Default)]
pub struct Recorded {
    outputs: IndexMap<String, String>,
    run: Vec<String>,
}

impl Recorded {
    pub fn new() -> Recorded {
        Recorded::default()
    }

    /// Record what `command` prints.
    pub fn with(mut self, command: &str, output: &str) -> Recorded {
        self.outputs
            .insert(command.trim().to_string(), output.to_string());
        self
    }

    /// Every command asked for so far, in order, whether or not it had output.
    pub fn commands_run(&self) -> &[String] {
        &self.run
    }
}

#[async_trait]
impl Runner for Recorded {
    async fn run(&mut self, command: &str) -> Result<String, DriverError> {
        let command = command.trim();
        self.run.push(command.to_string());
        self.outputs
            .get(command)
            .cloned()
            .ok_or_else(|| DriverError::Rejected {
                command: command.to_string(),
                error: "no output was recorded for this command".into(),
            })
    }
}

/// A command as a file name, the way recorded output is stored for
/// [`Recorded`] sessions and the profile tests: `show mac address-table` →
/// `show_mac_address-table`, `/ip route print terse without-paging` →
/// `ip_route_print_terse_without-paging`, `cat /proc/meminfo` →
/// `cat_proc_meminfo`.
pub fn command_slug(command: &str) -> String {
    let mut slug = String::with_capacity(command.len());
    for ch in command.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '-') {
            slug.push(ch);
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    slug.trim_matches('_').to_string()
}

/// What happened to one step of a collection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    pub kind: Kind,
    pub command: String,
    pub ok: bool,
    /// Rows this step contributed after `when` filtering.
    pub rows: usize,
    pub error: Option<String>,
    pub elapsed_ms: u64,
    /// An optional step that failed, or a step that was not run because it had nothing to add.
    pub skipped: bool,
}

/// What a collection produced.
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
    /// `Some(vec![])` is a device with no policy attached; `None` is a read
    /// that was not made or did not work.
    // Snapshots are stored (job results, the change pipeline's evidence), and
    // one written before this field existed must still be readable.
    #[serde(default)]
    pub qos: Option<Vec<QosClass>>,
    pub problems: Vec<Problem>,
    pub steps: Vec<StepReport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub kind: Kind,
    pub code: ProblemCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemCode {
    /// The profile defines no steps for this kind.
    Unsupported,
    /// No step that creates rows succeeded.
    Failed,
    /// Some steps, rows or fields could not be collected or mapped.
    Partial,
}

impl Snapshot {
    /// Whether the kind was collected.
    pub fn has(&self, kind: Kind) -> bool {
        match kind {
            Kind::Facts => self.facts.is_some(),
            Kind::Interfaces => self.interfaces.is_some(),
            Kind::Neighbors => self.neighbors.is_some(),
            Kind::Vlans => self.vlans.is_some(),
            Kind::MacTable => self.mac_table.is_some(),
            Kind::Arp => self.arp.is_some(),
            Kind::Routes => self.routes.is_some(),
            Kind::Health => self.health.is_some(),
            Kind::Inventory => self.inventory.is_some(),
            Kind::Qos => self.qos.is_some(),
        }
    }

    /// The collected rows of one kind as JSON (single-row kinds give a one-element array).
    pub fn rows(&self, kind: Kind) -> Option<Vec<Value>> {
        fn one<T: Serialize>(row: &Option<T>) -> Option<Vec<Value>> {
            row.as_ref()
                .map(|row| vec![serde_json::to_value(row).unwrap_or(Value::Null)])
        }
        fn many<T: Serialize>(rows: &Option<Vec<T>>) -> Option<Vec<Value>> {
            rows.as_ref().map(|rows| {
                rows.iter()
                    .map(|row| serde_json::to_value(row).unwrap_or(Value::Null))
                    .collect()
            })
        }
        match kind {
            Kind::Facts => one(&self.facts),
            Kind::Interfaces => many(&self.interfaces),
            Kind::Neighbors => many(&self.neighbors),
            Kind::Vlans => many(&self.vlans),
            Kind::MacTable => many(&self.mac_table),
            Kind::Arp => many(&self.arp),
            Kind::Routes => many(&self.routes),
            Kind::Health => one(&self.health),
            Kind::Inventory => many(&self.inventory),
            Kind::Qos => many(&self.qos),
        }
    }

    /// The problems recorded for one kind.
    pub fn problems_for(&self, kind: Kind) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(move |problem| problem.kind == kind)
    }
}

/// Runs collection steps and maps their output. See the [module docs](self).
pub struct Collector {
    expressions: Arc<Expressions>,
}

/// One step's rows after parsing, filtering and mapping.
struct Mapped {
    rows: Vec<Row>,
    /// How many field expressions failed, and the first failure.
    field_errors: usize,
    first_field_error: Option<String>,
}

impl Collector {
    /// `expressions` is normally [`Registry::expressions`](crate::profile::Registry::expressions).
    pub fn new(expressions: Arc<Expressions>) -> Collector {
        Collector { expressions }
    }

    /// Collect several kinds over one session. A command that appears in the
    /// steps of several kinds is run once. Facts are collected first, whatever
    /// their place in `kinds`, so that other kinds can refer to them.
    pub async fn collect(
        &self,
        runner: &mut dyn Runner,
        platform: &Platform,
        kinds: &[Kind],
    ) -> Snapshot {
        let mut order: Vec<Kind> = Vec::with_capacity(kinds.len());
        for kind in kinds {
            if !order.contains(kind) {
                order.push(*kind);
            }
        }
        order.sort_by_key(|kind| *kind != Kind::Facts);

        let mut snapshot = Snapshot::default();
        let mut outputs = Outputs {
            runner,
            cache: HashMap::new(),
            lost: None,
        };
        let mut facts: Option<Value> = None;
        for kind in order {
            let Some(rows) = self
                .collect_kind(&mut outputs, platform, kind, facts.as_ref(), &mut snapshot)
                .await
            else {
                continue;
            };
            store(&mut snapshot, kind, rows);
            if kind == Kind::Facts {
                facts = snapshot
                    .facts
                    .as_ref()
                    .and_then(|facts| serde_json::to_value(facts).ok());
            }
        }
        // These messages quote what the device printed — the line no rule
        // could place, the value that did not fit — and they travel on into
        // job results and events. A device can print a line of any length.
        for problem in &mut snapshot.problems {
            bound(&mut problem.message);
        }
        for error in snapshot
            .steps
            .iter_mut()
            .filter_map(|step| step.error.as_mut())
        {
            bound(error);
        }
        snapshot
    }

    /// Offline: parse and map one command's output with one step. Used by the
    /// profile editor's live preview and by the tests.
    ///
    /// The rows are what the step contributes before it is merged with other
    /// steps: model field → value, without interface-name canonicalisation
    /// (that depends on the kind the step belongs to). Unlike a collection,
    /// which leaves out a field whose expression fails, this reports the
    /// first such failure as an error, so that a preview shows it.
    pub fn map_output(
        &self,
        platform: &Platform,
        step: &CollectStep,
        output: &str,
    ) -> Result<Vec<Row>, String> {
        let mapped = self.map_rows(platform, step, output, None)?;
        match mapped.first_field_error {
            Some(error) => Err(error),
            None => Ok(mapped.rows),
        }
    }

    fn map_rows(
        &self,
        platform: &Platform,
        step: &CollectStep,
        output: &str,
        facts: Option<&Value>,
    ) -> Result<Mapped, String> {
        let parsed = parse_output(platform, step, output)?;
        // TextFSM names its values in capitals; let expressions use either spelling.
        let lower_case_aliases = matches!(step.parser, Parser::Ntc(_) | Parser::Textfsm(_));
        let when = step
            .when
            .as_deref()
            .map(|condition| self.expressions.compile(condition))
            .transpose()
            .map_err(|err| format!("when: {err}"))?;
        let fields = step
            .map
            .iter()
            .map(|(field, expression)| {
                self.expressions
                    .compile(expression)
                    .map(|compiled| (field.as_str(), compiled))
                    .map_err(|err| format!("map.{field}: {err}"))
            })
            .collect::<Result<Vec<_>, String>>()?;

        let mut mapped = Mapped {
            rows: Vec::with_capacity(parsed.len()),
            field_errors: 0,
            first_field_error: None,
        };
        for (number, parsed_row) in parsed.into_iter().enumerate() {
            let variables = variables(parsed_row, lower_case_aliases, facts);
            let context = Context::new(&variables);
            if let Some(when) = &when
                && !when
                    .truthy(&context)
                    .map_err(|err| format!("when: {err} (row {})", number + 1))?
            {
                continue;
            }
            let mut row = Row::new();
            if fields.is_empty()
                && let Value::Object(variables) = variables
            {
                row.extend(variables.into_iter().filter(|(name, value)| {
                    !matches!(name.as_str(), "row" | "facts") && !is_blank(value)
                }));
            }
            for (field, expression) in &fields {
                match expression.eval(&context) {
                    Ok(value) => {
                        if let Some(value) = meaningful(value) {
                            set_path(&mut row, field, value);
                        }
                    }
                    Err(err) => {
                        mapped.field_errors += 1;
                        mapped.first_field_error.get_or_insert_with(|| {
                            format!("map.{field}: {err} (row {})", number + 1)
                        });
                    }
                }
            }
            if !row.is_empty() {
                mapped.rows.push(row);
            }
        }
        Ok(mapped)
    }

    /// Run the steps of one kind. `None` when the kind could not be collected
    /// (the reason is added to `snapshot.problems`).
    async fn collect_kind(
        &self,
        outputs: &mut Outputs<'_>,
        platform: &Platform,
        kind: Kind,
        facts: Option<&Value>,
        snapshot: &mut Snapshot,
    ) -> Option<Vec<Row>> {
        let steps = match platform.profile.collect.get(&kind) {
            Some(steps) if !steps.is_empty() => steps,
            _ => {
                snapshot.problems.push(Problem {
                    kind,
                    code: ProblemCode::Unsupported,
                    message: format!(
                        "the `{}` profile does not define how to collect {kind}",
                        platform.id()
                    ),
                });
                return None;
            }
        };
        let model = kind.empty_row();
        let names = Names::for_kind(kind, platform.profile.interface_names);
        let single = kind.is_single_row();

        let mut rows: Vec<Row> = Vec::new();
        // Whether a step that creates rows has succeeded.
        let mut created = false;
        let mut failures: Vec<String> = Vec::new();
        let mut required_failures: Vec<String> = Vec::new();
        let mut field_errors = 0;
        let mut first_field_error: Option<String> = None;
        // Steps that read rows and placed none of them.
        let mut unmerged: Vec<String> = Vec::new();

        for (index, step) in steps.iter().enumerate() {
            let command = step.command.trim();
            let mut report = StepReport {
                kind,
                command: command.to_string(),
                ok: false,
                rows: 0,
                error: None,
                elapsed_ms: 0,
                skipped: false,
            };
            if single
                && !step.map.is_empty()
                && rows
                    .first()
                    .is_some_and(|row| step.map.keys().all(|field| has_path(row, field)))
            {
                report.ok = true;
                report.skipped = true;
                snapshot.steps.push(report);
                continue;
            }

            let started = Instant::now();
            let outcome = match outputs.get(platform, command).await {
                Ok(output) => self.map_rows(platform, step, &output, facts),
                Err(error) => Err(error),
            };
            report.elapsed_ms = started.elapsed().as_millis() as u64;
            match outcome {
                Ok(mut mapped) => {
                    for row in &mut mapped.rows {
                        names.canonicalise(row);
                    }
                    report.ok = true;
                    report.rows = if single {
                        let count = mapped.rows.len();
                        if rows.is_empty() && count > 0 {
                            rows.push(Row::new());
                        }
                        for row in mapped.rows {
                            fill(&mut rows[0], row, &model);
                        }
                        created = true;
                        count
                    } else if let Some(key) = step.merge_on.as_deref() {
                        let (merged, offered) = merge(&mut rows, mapped.rows, key, &names, &model);
                        if merged == 0 && offered > 0 {
                            unmerged.push(format!(
                                "`{command}` read {} but none of them matched an existing row on `{key}`",
                                plural(offered, "row")
                            ));
                        }
                        merged
                    } else {
                        let count = mapped.rows.len();
                        rows.extend(mapped.rows);
                        created = true;
                        count
                    };
                    if mapped.field_errors > 0 {
                        field_errors += mapped.field_errors;
                        first_field_error = first_field_error.or(mapped
                            .first_field_error
                            .map(|error| format!("collect.{kind}[{index}].{error}")));
                    }
                }
                Err(error) => {
                    let described = format!("`{command}`: {error}");
                    failures.push(described.clone());
                    if step.optional {
                        report.skipped = true;
                    } else {
                        required_failures.push(described);
                    }
                    report.error = Some(error);
                }
            }
            snapshot.steps.push(report);
        }

        if !created {
            let message = if failures.is_empty() {
                // Only possible when every step has `merge_on`; profile validation rejects that.
                "none of the profile's steps creates rows (every step has `merge_on`)".to_string()
            } else {
                failures.join("; ")
            };
            snapshot.problems.push(Problem {
                kind,
                code: ProblemCode::Failed,
                message,
            });
            return None;
        }
        if single && rows.first().is_none_or(Row::is_empty) {
            snapshot.problems.push(Problem {
                kind,
                code: ProblemCode::Failed,
                message: "the commands ran, but none of the fields the profile maps had a value in their output"
                    .into(),
            });
            return None;
        }
        // A step that enriches rows and matched none of them added nothing at
        // all. The collection is still good — this is `Partial`, not
        // `Failed` — but it must not pass for complete, because every field
        // that step was there to fill is missing and nothing else says so.
        if !unmerged.is_empty() {
            snapshot.problems.push(Problem {
                kind,
                code: ProblemCode::Partial,
                message: format!(
                    "{} added nothing: {}",
                    plural(unmerged.len(), "step"),
                    unmerged.join("; ")
                ),
            });
        }
        if !required_failures.is_empty() {
            snapshot.problems.push(Problem {
                kind,
                code: ProblemCode::Partial,
                message: format!(
                    "{} failed, so what {} would have added is missing: {}",
                    plural(required_failures.len(), "step"),
                    if required_failures.len() == 1 {
                        "it"
                    } else {
                        "they"
                    },
                    required_failures.join("; ")
                ),
            });
        }
        if let Some(first) = first_field_error {
            snapshot.problems.push(Problem {
                kind,
                code: ProblemCode::Partial,
                message: format!(
                    "{} could not be computed and {} left out; first: {first}",
                    plural(field_errors, "field value"),
                    if field_errors == 1 { "was" } else { "were" },
                ),
            });
        }
        Some(rows)
    }
}

/// The most characters of a problem's message, or of a step's error, that
/// are kept.
const MESSAGE_LIMIT: usize = 2_000;

/// Cut a message down to [`MESSAGE_LIMIT`] characters, saying how much of it
/// was left out.
fn bound(message: &mut String) {
    let Some((at, _)) = message.char_indices().nth(MESSAGE_LIMIT) else {
        return;
    };
    let left_out = message[at..].chars().count();
    message.truncate(at);
    message.push_str(&format!("… ({left_out} more characters)"));
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Runs each command once and remembers what it printed.
struct Outputs<'a> {
    runner: &'a mut dyn Runner,
    cache: HashMap<String, Result<String, String>>,
    /// The command during which the session was lost, once that has happened.
    lost: Option<String>,
}

impl Outputs<'_> {
    async fn get(&mut self, platform: &Platform, command: &str) -> Result<String, String> {
        let verdict = platform.policy.classify(command);
        if verdict.class != CommandClass::ReadOnly {
            return Err(format!(
                "not run: collectors only run read-only commands, and {}. This is a mistake in the `{}` profile: fix the command or the profile's `policy.read_only` list",
                verdict.reason,
                platform.id()
            ));
        }
        if let Some(cached) = self.cache.get(command) {
            return cached.clone();
        }
        if let Some(lost) = &self.lost {
            return Err(format!(
                "not run: the session was lost while running `{lost}`"
            ));
        }
        let result = match self.runner.run(command).await {
            Ok(output) => Ok(output),
            Err(err) => {
                if matches!(
                    err,
                    DriverError::Timeout { .. }
                        | DriverError::Closed { .. }
                        | DriverError::Channel(_)
                ) {
                    self.lost = Some(command.to_string());
                }
                Err(match err {
                    DriverError::Rejected { error, .. } => {
                        format!("the device rejected the command: {error}")
                    }
                    other => other.to_string(),
                })
            }
        };
        self.cache.insert(command.to_string(), result.clone());
        result
    }
}

fn parse_output(platform: &Platform, step: &CollectStep, output: &str) -> Result<Vec<Row>, String> {
    match &step.parser {
        Parser::Ntc(name) => parse::parse_ntc(
            platform.profile.ntc_platform.as_deref(),
            step.command.trim(),
            name,
            output,
        ),
        Parser::Textfsm(template) => parse::parse_textfsm(template, output),
        Parser::Regex(pattern) => Regex::new(pattern)
            .map(|pattern| parse::parse_regex(&pattern, output))
            .map_err(|err| format!("invalid regular expression: {err}")),
        Parser::Kv => {
            let rows = parse::parse_kv(output);
            // No rows from a listing that does list something: it is printed
            // in a form this parser does not read (a table). That is a read
            // that failed, and it must not pass for "the device has none".
            match parse::kv_unread_item(output).filter(|_| rows.is_empty()) {
                Some(line) => Err(format!(
                    "the listing has items, but not in the `key=value` form this parser reads (is it printed as a table?); first: {line}"
                )),
                None => Ok(rows),
            }
        }
        Parser::Colon => Ok(parse::parse_colon(output)),
        Parser::Json(path) => parse::parse_json(output, path),
        Parser::Raw => Ok(parse::parse_raw(output)),
    }
}

/// The variables a step's expressions see for one parsed row.
fn variables(parsed: Row, lower_case_aliases: bool, facts: Option<&Value>) -> Value {
    let mut variables = Row::with_capacity(parsed.len() + 2);
    for (name, value) in &parsed {
        if lower_case_aliases {
            let lower = name.to_ascii_lowercase();
            if lower != *name && !parsed.contains_key(&lower) {
                variables.insert(lower, value.clone());
            }
        }
        variables.insert(name.clone(), value.clone());
    }
    if let Some(facts) = facts {
        variables.insert("facts".into(), facts.clone());
    }
    variables.insert("row".into(), Value::Object(parsed));
    Value::Object(variables)
}

/// Nothing worth storing: null, an empty or blank string, an empty list or map.
fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(items) => items.iter().all(is_blank),
        Value::Object(map) => map.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

/// The value without its blank parts, or `None` when nothing is left.
fn meaningful(value: Value) -> Option<Value> {
    match value {
        Value::Array(items) => {
            let items: Vec<Value> = items.into_iter().filter(|item| !is_blank(item)).collect();
            (!items.is_empty()).then_some(Value::Array(items))
        }
        Value::Object(map) => {
            let map: Row = map
                .into_iter()
                .filter_map(|(key, value)| meaningful(value).map(|value| (key, value)))
                .collect();
            (!map.is_empty()).then_some(Value::Object(map))
        }
        other => (!is_blank(&other)).then_some(other),
    }
}

/// Set `counters.in_errors`-style paths, creating the objects on the way.
fn set_path(row: &mut Row, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            row.insert(path.to_string(), value);
        }
        Some((head, rest)) => {
            let slot = row
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(Row::new()));
            if !slot.is_object() {
                *slot = Value::Object(Row::new());
            }
            if let Value::Object(inner) = slot {
                set_path(inner, rest, value);
            }
        }
    }
}

/// Whether the row already has a non-blank value at the (possibly dotted) path.
fn has_path(row: &Row, path: &str) -> bool {
    let (head, rest) = match path.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (path, None),
    };
    match (row.get(head), rest) {
        (Some(Value::Object(inner)), Some(rest)) => has_path(inner, rest),
        (Some(value), None) => !is_blank(value),
        _ => false,
    }
}

/// A value as the text two rows are matched on.
fn key_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Merge `new` into `existing`: what is there stays, what is missing is
/// filled, lists (per the model) collect the items of both.
fn fill(existing: &mut Row, new: Row, model: &Row) {
    for (field, value) in new {
        let is_list = matches!(model.get(&field), Some(Value::Array(_)));
        match existing.get_mut(&field) {
            Some(current) if is_list => extend_list(current, value),
            Some(Value::Object(current)) => {
                if let Value::Object(incoming) = value {
                    let nested = match model.get(&field) {
                        Some(Value::Object(nested)) => nested.clone(),
                        _ => Row::new(),
                    };
                    fill(current, incoming, &nested);
                }
            }
            Some(current) if !is_blank(current) => {}
            _ => {
                existing.insert(field, value);
            }
        }
    }
}

fn extend_list(current: &mut Value, incoming: Value) {
    if !current.is_array() {
        let single = std::mem::take(current);
        *current = Value::Array(if is_blank(&single) {
            Vec::new()
        } else {
            vec![single]
        });
    }
    let Value::Array(items) = current else { return };
    let incoming = match incoming {
        Value::Array(incoming) => incoming,
        other => vec![other],
    };
    for item in incoming {
        if !is_blank(&item) && !items.iter().any(|known| key_text(known) == key_text(&item)) {
            items.push(item);
        }
    }
}

/// Merge each of `new` into the rows that have the same value in `key`.
/// Returns how many of the new rows found a row to merge into.
///
/// Usually one row carries a key, and every new row with that key goes into
/// it (several addresses for one interface). When the key occurs several
/// times on both sides and equally often — the same listing parsed twice —
/// the rows are paired in order instead, so that each keeps its own values.
/// Enrich existing rows from `new`, matching on `key`.
///
/// Returns how many of `new`'s rows landed somewhere and how many were
/// offered. The second number is the one that matters when the first is zero:
/// a step whose rows match nothing is a silent no-op today, and that is
/// exactly how a step can look fine for months while contributing nothing —
/// the key is wrong, or the names normalise differently, or the output came
/// from a different device than the rows it was meant to enrich.
fn merge(rows: &mut [Row], new: Vec<Row>, key: &str, names: &Names, model: &Row) -> (usize, usize) {
    let mut targets: HashMap<String, Vec<usize>> = HashMap::new();
    for (position, row) in rows.iter().enumerate() {
        if let Some(value) = row.get(key).filter(|value| !is_blank(value)) {
            targets
                .entry(names.match_key(key, value))
                .or_default()
                .push(position);
        }
    }
    let offered = new.len();
    let mut incoming: IndexMap<String, Vec<Row>> = IndexMap::new();
    for row in new {
        if let Some(value) = row.get(key).filter(|value| !is_blank(value)) {
            incoming
                .entry(names.match_key(key, value))
                .or_default()
                .push(row);
        }
    }
    let mut merged = 0;
    for (value, group) in incoming {
        let Some(positions) = targets.get(&value) else {
            continue;
        };
        merged += group.len();
        if group.len() == positions.len() {
            for (position, row) in positions.iter().zip(group) {
                fill(&mut rows[*position], row, model);
            }
        } else {
            for row in group {
                for position in positions {
                    fill(&mut rows[*position], row.clone(), model);
                }
            }
        }
    }
    (merged, offered)
}

/// How interface names are treated for one kind of row.
struct Names {
    fields: &'static [&'static str],
    canonical: bool,
}

impl Names {
    fn for_kind(kind: Kind, style: InterfaceNames) -> Names {
        Names {
            fields: kind.interface_fields(),
            canonical: style == InterfaceNames::Cisco,
        }
    }

    /// Rewrite every interface name in the row to its long form.
    fn canonicalise(&self, row: &mut Row) {
        if !self.canonical {
            return;
        }
        for field in self.fields {
            match row.get_mut(*field) {
                Some(Value::String(name)) => *name = ifname::long(name),
                Some(Value::Array(items)) => {
                    for item in items {
                        if let Value::String(name) = item {
                            *name = ifname::long(name);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The text rows are matched on: equal exactly when [`ifname::same`]
    /// holds for interface names, plain equality for everything else.
    fn match_key(&self, field: &str, value: &Value) -> String {
        let text = key_text(value);
        if self.canonical && self.fields.contains(&field) {
            ifname::long(&text).to_ascii_lowercase()
        } else {
            text
        }
    }
}

/// Read rows into the model and put them in the snapshot. Rows that do not
/// fit are dropped and counted in one `Partial` problem.
fn store(snapshot: &mut Snapshot, kind: Kind, rows: Vec<Row>) {
    fn typed<T: DeserializeOwned>(rows: Vec<Row>, model: &Row) -> (Vec<T>, usize, Option<String>) {
        let mut out = Vec::with_capacity(rows.len());
        let mut dropped = 0;
        let mut first = None;
        for mut row in rows {
            stringify_free_form(&mut row, model);
            let shown = Value::Object(row.clone()).to_string();
            match serde_json::from_value::<T>(Value::Object(row)) {
                Ok(item) => out.push(item),
                Err(err) => {
                    dropped += 1;
                    first.get_or_insert_with(|| {
                        let shown: String = shown.chars().take(300).collect();
                        format!("{err} in {shown}")
                    });
                }
            }
        }
        (out, dropped, first)
    }

    let model = kind.empty_row();
    let total = rows.len();
    macro_rules! many {
        ($field:ident) => {{
            let (items, dropped, first) = typed(rows, &model);
            snapshot.$field = Some(items);
            (dropped, first)
        }};
    }
    macro_rules! one {
        ($field:ident) => {{
            let (mut items, dropped, first) = typed(rows, &model);
            snapshot.$field = items.pop();
            (dropped, first)
        }};
    }
    let (dropped, first) = match kind {
        Kind::Facts => one!(facts),
        Kind::Health => one!(health),
        Kind::Interfaces => many!(interfaces),
        Kind::Neighbors => many!(neighbors),
        Kind::Vlans => many!(vlans),
        Kind::MacTable => many!(mac_table),
        Kind::Arp => many!(arp),
        Kind::Routes => many!(routes),
        Kind::Inventory => many!(inventory),
        Kind::Qos => many!(qos),
    };
    if let Some(first) = first {
        let single = kind.is_single_row();
        snapshot.problems.push(Problem {
            kind,
            code: if single {
                ProblemCode::Failed
            } else {
                ProblemCode::Partial
            },
            message: if single {
                format!("the collected values do not fit the model: {first}")
            } else {
                format!(
                    "{dropped} of {} did not fit the model and {} dropped; first: {first}",
                    plural(total, "row"),
                    if dropped == 1 { "was" } else { "were" },
                )
            },
        });
    }
}

/// Free-form string maps (`extra`) take whatever the expressions produced, as text.
fn stringify_free_form(row: &mut Row, model: &Row) {
    for (field, default) in model {
        if !matches!(default, Value::Object(inner) if inner.is_empty()) {
            continue;
        }
        if let Some(Value::Object(values)) = row.get_mut(field) {
            for value in values.values_mut() {
                *value = Value::String(match &*value {
                    Value::String(text) => text.trim().to_string(),
                    Value::Array(items) => {
                        items.iter().map(key_text).collect::<Vec<_>>().join(", ")
                    }
                    other => key_text(other),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;
    use crate::profile::{Origin, Registry};

    /// A small IOS-flavoured profile that exercises every combination rule.
    const PROFILE: &str = r#"
id: acme
name: Acme
ntc_platform: cisco_ios
policy:
  read_only: ['^show\b']
  dangerous: ['^reload\b']
collect:
  facts:
    - command: show version
      parser: {regex: 'Acme (?P<model>\S+) running (?P<version>\S+)(?: serial (?P<serial>\S+))?'}
      map:
        vendor: "'Acme'"
        model: model
        os_version: version
        serial: serial
        extra.image: "row.model ~ '.bin' if row.model else none"
    - command: show inventory
      parser: {regex: 'SN: (?P<serial>\S+)'}
      map: {serial: serial, model: "'never used'"}
      optional: true
    - command: show version
      parser: {regex: 'uptime is (?P<uptime>.+)'}
      map: {uptime_s: "uptime | duration"}
  interfaces:
    - command: show interfaces
      parser:
        regex: '(?m)^(?P<name>\S+) is (?P<state>up|down|administratively down), mtu (?P<mtu>\d+)(?:, address (?P<ip>[\d./]+))?(?:, (?P<errors>\S+) input errors)?'
      map:
        name: name
        admin_up: "state != 'administratively down'"
        oper_up: "state == 'up'"
        mtu: mtu
        ipv4: "[ip] if ip else []"
        description: "missing_field"
        counters.in_errors: "errors | num"
    - command: show interfaces description
      parser: {regex: '(?m)^(?P<port>\S+)\s+(?P<descr>.+)$'}
      map: {name: port, description: descr, mtu: "'9999'"}
      merge_on: name
    - command: show ip addresses
      parser: {regex: '(?m)^(?P<port>\S+) (?P<address>\S+)$'}
      map: {name: port, ipv4: address}
      merge_on: name
      optional: true
  neighbors:
    - command: show cdp neighbors
      parser: {regex: '(?m)^(?P<device>\S+) (?P<local>\S+) (?P<remote>\S+)$'}
      map: {device: device, local_interface: local, remote_interface: remote, protocol: "'cdp'"}
      optional: true
    - command: show lldp neighbors
      parser: {regex: '(?m)^(?P<device>\S+) (?P<local>\S+) (?P<remote>\S+)$'}
      map: {device: device, local_interface: local, remote_interface: remote, protocol: "'lldp'"}
      optional: true
  vlans:
    - command: show vlan
      parser: {regex: '(?m)^(?P<id>\d+)\s+(?P<name>\S+)\s+(?P<status>\S+)'}
  mac_table:
    - command: show mac address-table
      parser: {regex: '(?m)^(?P<vlan>\d+) (?P<mac>\S+) (?P<port>\S+)$'}
      map: {vlan: vlan, mac: "mac | mac", interface: port}
      when: "port != 'CPU'"
  health:
    - command: show version
      parser: {regex: 'uptime is (?P<uptime>.+)'}
      map: {uptime_s: "uptime | duration"}
    - command: show cpu
      parser: {regex: 'cpu (?P<cpu>\d+)%'}
      map: {cpu_pct: cpu}
  routes:
    - command: show ip route
      parser: {regex: '(?m)^(?P<code>\S) (?P<prefix>\S+) via (?P<hop>\S+) on (?P<port>\S+) hosts (?P<hosts>\S+)$'}
      map:
        prefix: prefix
        next_hop: hop
        interface: port
        metric: "(hosts | num) * 2"
        protocol: "'on ' ~ facts.model if facts else 'no facts'"
"#;

    const VERSION: &str =
        "Acme X-1000 running 7.1 serial ABC123\nSW1 uptime is 1 hour, 5 minutes\n";
    const INTERFACES: &str = "\
Gi0/1 is up, mtu 1500, address 10.0.0.1/24, 1,204 input errors
Gi0/2 is administratively down, mtu 1500
Vlan10 is down, mtu 9000, address 10.0.10.1/24
";

    fn platform() -> (Collector, Arc<Platform>) {
        let mut registry = Registry::empty();
        let platform = registry.add(PROFILE, Origin::Builtin).unwrap();
        (Collector::new(registry.expressions()), platform)
    }

    fn codes(snapshot: &Snapshot, kind: Kind) -> Vec<ProblemCode> {
        snapshot
            .problems_for(kind)
            .map(|problem| problem.code)
            .collect()
    }

    #[tokio::test]
    async fn steps_merge_into_rows_and_never_overwrite() {
        let (collector, platform) = platform();
        let mut device = Recorded::new()
            .with("show interfaces", INTERFACES)
            .with(
                "show interfaces description",
                "Gi0/1   uplink to core\ngigabitethernet0/2   spare\nGi0/9   not in the list\n",
            )
            .with(
                "show ip addresses",
                "Gi0/1 10.0.0.1/24\nGi0/1 10.0.1.1/24\nVl10 10.0.20.1/24\n",
            );
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Interfaces])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        let interfaces = snapshot.interfaces.as_ref().unwrap();
        assert_eq!(interfaces.len(), 3, "an enrichment row never adds a row");

        // Names are canonicalised; rows are matched across spellings.
        let first = &interfaces[0];
        assert_eq!(first.name, "GigabitEthernet0/1");
        assert_eq!(first.description, "uplink to core");
        assert_eq!((first.admin_up, first.oper_up), (Some(true), Some(true)));
        assert_eq!(first.mtu, Some(1500), "a value that is present is kept");
        assert_eq!(
            first.ipv4,
            vec!["10.0.0.1/24", "10.0.1.1/24"],
            "lists collect"
        );
        assert_eq!(first.counters.in_errors, Some(1204));

        let second = &interfaces[1];
        assert_eq!(second.name, "GigabitEthernet0/2");
        assert_eq!(second.description, "spare");
        assert_eq!(
            (second.admin_up, second.oper_up),
            (Some(false), Some(false))
        );
        assert_eq!(second.ipv4, Vec::<String>::new());
        assert_eq!(second.counters.in_errors, None);

        let third = &interfaces[2];
        assert_eq!(third.name, "Vlan10");
        assert_eq!(third.description, "", "undefined is left out, not set");
        assert_eq!(third.ipv4, vec!["10.0.10.1/24", "10.0.20.1/24"]);

        let reports: Vec<(&str, bool, usize, bool)> = snapshot
            .steps
            .iter()
            .map(|step| (step.command.as_str(), step.ok, step.rows, step.skipped))
            .collect();
        assert_eq!(
            reports,
            vec![
                ("show interfaces", true, 3, false),
                ("show interfaces description", true, 2, false),
                ("show ip addresses", true, 3, false),
            ]
        );
        assert_eq!(
            snapshot.rows(Kind::Interfaces).unwrap()[0]["name"],
            "GigabitEthernet0/1"
        );
        assert!(snapshot.has(Kind::Interfaces) && !snapshot.has(Kind::Facts));
        assert_eq!(snapshot.rows(Kind::Facts), None);
    }

    #[tokio::test]
    async fn single_row_kinds_fill_gaps_and_skip_steps_with_nothing_to_add() {
        let (collector, platform) = platform();
        let mut device = Recorded::new()
            .with("show version", VERSION)
            .with("show inventory", "SN: FROM-INVENTORY\n");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Facts])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        let facts = snapshot.facts.as_ref().unwrap();
        assert_eq!(
            (
                facts.vendor.as_str(),
                facts.model.as_str(),
                facts.os_version.as_str()
            ),
            ("Acme", "X-1000", "7.1")
        );
        assert_eq!(facts.serial, "ABC123", "the first value wins");
        assert_eq!(facts.uptime_s, Some(3900));
        assert_eq!(facts.extra["image"], "X-1000.bin");
        assert_eq!(facts.hostname, "", "nothing maps it, so it stays empty");
        // The inventory step maps only fields that were already filled: not run.
        assert_eq!(device.commands_run(), ["show version"]);
        let inventory_step = &snapshot.steps[1];
        assert!(inventory_step.ok && inventory_step.skipped && inventory_step.error.is_none());

        // Without a serial in `show version` the second step is needed and fills the gap.
        let mut device = Recorded::new()
            .with("show version", "Acme X-1000 running 7.1\n")
            .with("show inventory", "SN: FROM-INVENTORY\n");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Facts])
            .await;
        let facts = snapshot.facts.as_ref().unwrap();
        assert_eq!(
            (facts.serial.as_str(), facts.model.as_str()),
            ("FROM-INVENTORY", "X-1000")
        );
        assert_eq!(facts.uptime_s, None);
        assert_eq!(device.commands_run(), ["show version", "show inventory"]);
        assert_eq!(snapshot.problems, vec![]);
        assert_eq!(
            snapshot.rows(Kind::Facts).unwrap()[0]["serial"],
            "FROM-INVENTORY"
        );
    }

    #[tokio::test]
    async fn a_command_shared_by_several_kinds_runs_once_and_facts_come_first() {
        let (collector, platform) = platform();
        let mut device = Recorded::new()
            .with("show version", VERSION)
            .with("show cpu", "cpu 17%\n")
            .with(
                "show ip route",
                "S 0.0.0.0/0 via 10.0.0.254 on Gi0/1 hosts 3\n",
            );
        let snapshot = collector
            .collect(
                &mut device,
                &platform,
                &[Kind::Routes, Kind::Health, Kind::Facts, Kind::Health],
            )
            .await;
        assert_eq!(snapshot.problems, vec![]);
        assert_eq!(
            device.commands_run(),
            ["show version", "show ip route", "show cpu"],
            "facts first; `show version` once for facts (twice) and health"
        );
        let health = snapshot.health.as_ref().unwrap();
        assert_eq!((health.uptime_s, health.cpu_pct), (Some(3900), Some(17.0)));
        let route = &snapshot.routes.as_ref().unwrap()[0];
        assert_eq!(
            route.protocol, "on X-1000",
            "`facts` is visible to later kinds"
        );
        assert_eq!(route.interface, "GigabitEthernet0/1");
        assert_eq!(route.metric, Some(6));

        // Without facts in the same call the variable is simply absent.
        let mut device = Recorded::new().with(
            "show ip route",
            "S 0.0.0.0/0 via 10.0.0.254 on Gi0/1 hosts 3\n",
        );
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Routes])
            .await;
        assert_eq!(snapshot.routes.as_ref().unwrap()[0].protocol, "no facts");
    }

    #[tokio::test]
    async fn optional_and_required_failures() {
        let (collector, platform) = platform();

        // An optional step that fails is recorded and nothing else happens.
        let mut device = Recorded::new()
            .with("show interfaces", INTERFACES)
            .with("show interfaces description", "Gi0/1   uplink to core\n");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Interfaces])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        let last = snapshot.steps.last().unwrap();
        assert!(!last.ok && last.skipped);
        assert!(
            last.error.as_deref().unwrap().contains("rejected"),
            "{:?}",
            last.error
        );

        // A required enrichment step that fails: the rows are kept, the gap is reported.
        let mut device = Recorded::new().with("show interfaces", INTERFACES);
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Interfaces])
            .await;
        assert_eq!(snapshot.interfaces.as_ref().unwrap().len(), 3);
        assert_eq!(
            codes(&snapshot, Kind::Interfaces),
            vec![ProblemCode::Partial]
        );
        let message = &snapshot.problems[0].message;
        assert!(
            message.contains("1 step failed") && message.contains("`show interfaces description`"),
            "{message}"
        );

        // The step that creates the rows fails: nothing to enrich, the kind failed.
        let mut device = Recorded::new().with("show interfaces description", "Gi0/1   uplink\n");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Interfaces])
            .await;
        assert!(snapshot.interfaces.is_none());
        assert_eq!(
            codes(&snapshot, Kind::Interfaces),
            vec![ProblemCode::Failed]
        );
        assert!(
            snapshot.problems[0].message.contains("`show interfaces`:"),
            "{}",
            snapshot.problems[0].message
        );

        // Every step optional and every step failing is still a failure, not an empty table.
        let mut device = Recorded::new();
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Neighbors])
            .await;
        assert!(snapshot.neighbors.is_none());
        assert_eq!(codes(&snapshot, Kind::Neighbors), vec![ProblemCode::Failed]);
        let message = &snapshot.problems[0].message;
        assert!(
            message.contains("`show cdp neighbors`") && message.contains("`show lldp neighbors`"),
            "{message}"
        );

        // One of two alternative sources is enough, and a table that is really empty is empty.
        let mut device = Recorded::new().with("show lldp neighbors", "core1 Gi0/1 Te1/1/1\n");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Neighbors])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        let neighbor = &snapshot.neighbors.as_ref().unwrap()[0];
        assert_eq!(
            (
                neighbor.protocol.as_str(),
                neighbor.local_interface.as_str(),
                neighbor.remote_interface.as_str()
            ),
            ("lldp", "GigabitEthernet0/1", "Te1/1/1"),
            "the neighbour's own port keeps the spelling it was advertised with"
        );
        let mut device = Recorded::new().with("show cdp neighbors", "");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Neighbors])
            .await;
        assert_eq!(snapshot.neighbors, Some(vec![]));
        assert_eq!(snapshot.problems, vec![]);
    }

    #[tokio::test]
    async fn unsupported_kinds_and_single_rows_without_values() {
        let (collector, platform) = platform();
        let mut device = Recorded::new()
            .with("show version", "nothing recognisable\n")
            .with("show cpu", "\n");
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Arp, Kind::Health])
            .await;
        assert!(snapshot.arp.is_none() && snapshot.health.is_none());
        assert_eq!(codes(&snapshot, Kind::Arp), vec![ProblemCode::Unsupported]);
        assert!(
            snapshot.problems[0]
                .message
                .contains("`acme` profile does not define how to collect arp"),
            "{}",
            snapshot.problems[0].message
        );
        assert_eq!(codes(&snapshot, Kind::Health), vec![ProblemCode::Failed]);
        assert!(
            snapshot.problems[1].message.contains("none of the fields"),
            "{}",
            snapshot.problems[1].message
        );
        assert!(
            device
                .commands_run()
                .iter()
                .all(|command| command != "show arp")
        );
    }

    #[tokio::test]
    async fn rows_are_filtered_passed_through_and_field_errors_reported() {
        let (collector, platform) = platform();
        let mut device = Recorded::new()
            .with(
                "show mac address-table",
                "10 aabb.cc00.0100 Gi0/1\n10 0100.0ccc.cccc CPU\n20 AABB.CC00.0200 Po1\n",
            )
            .with("show vlan", "1    default  active\n30   Voice    active\n")
            .with(
                "show ip route",
                "S 0.0.0.0/0 via 10.0.0.254 on Gi0/1 hosts 3\nC 10.0.0.0/24 via 0.0.0.0 on Vl10 hosts many\n",
            );
        let snapshot = collector
            .collect(
                &mut device,
                &platform,
                &[Kind::MacTable, Kind::Vlans, Kind::Routes],
            )
            .await;

        let macs = snapshot.mac_table.as_ref().unwrap();
        assert_eq!(macs.len(), 2, "`when` dropped the CPU entry");
        assert_eq!(
            (
                macs[1].vlan,
                macs[1].mac.as_str(),
                macs[1].interface.as_str()
            ),
            (Some(20), "aa:bb:cc:00:02:00", "Port-channel1")
        );
        assert_eq!(snapshot.steps[0].rows, 2);

        // An empty `map` passes the parsed row through: the groups are named like the model's fields.
        let vlans = snapshot.vlans.as_ref().unwrap();
        assert_eq!(
            (
                vlans[1].id,
                vlans[1].name.as_str(),
                vlans[1].status.as_str()
            ),
            (Some(30), "Voice", "active")
        );

        // `"many" | num` is none, and none * 2 cannot be computed: the field is left out and counted.
        let routes = snapshot.routes.as_ref().unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!((routes[0].metric, routes[1].metric), (Some(6), None));
        assert_eq!(routes[1].interface, "Vlan10");
        assert_eq!(codes(&snapshot, Kind::Routes), vec![ProblemCode::Partial]);
        let message = &snapshot.problems[0].message;
        assert!(
            message.contains("1 field value could not be computed")
                && message.contains("collect.routes[0].map.metric")
                && message.contains("(row 2)"),
            "{message}"
        );
        assert_eq!(snapshot.problems.len(), 1);
    }

    #[tokio::test]
    async fn commands_that_are_not_read_only_are_never_sent() {
        // Validation rejects such a profile, so build the situation by hand:
        // the platform's policy is tightened after the profile was loaded.
        let mut registry = Registry::empty();
        let strict = PROFILE.replace(
            r"read_only: ['^show\b']",
            r"read_only: ['^show (version|cpu)\b']",
        );
        let error = registry.add(&strict, Origin::Builtin).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains(
                "collect.interfaces[0].command: collectors may only run read-only commands"
            ) && text.contains("`show interfaces` is not on this platform's read-only list"),
            "{text}"
        );

        let loaded = registry.add(PROFILE, Origin::Builtin).unwrap();
        registry.remove("acme");
        let platform = Platform {
            policy: crate::policy::Policy::new(&[r"^show (version|cpu)\b"], &["^show ip route"])
                .unwrap(),
            ..Arc::try_unwrap(loaded).ok().unwrap()
        };
        let collector = Collector::new(registry.expressions());
        let mut device = Recorded::new()
            .with("show version", VERSION)
            .with("show interfaces", INTERFACES)
            .with(
                "show ip route",
                "S 0.0.0.0/0 via 10.0.0.254 on Gi0/1 hosts 3\n",
            );
        let snapshot = collector
            .collect(
                &mut device,
                &platform,
                &[Kind::Facts, Kind::Interfaces, Kind::Routes],
            )
            .await;
        assert_eq!(device.commands_run(), ["show version"]);
        assert!(
            snapshot.facts.is_some() && snapshot.interfaces.is_none() && snapshot.routes.is_none()
        );
        assert_eq!(
            codes(&snapshot, Kind::Interfaces),
            vec![ProblemCode::Failed]
        );
        let message = &snapshot.problems[0].message;
        assert!(
            message.contains("not run: collectors only run read-only commands")
                && message.contains("mistake in the `acme` profile"),
            "{message}"
        );
        assert!(
            snapshot.problems[1]
                .message
                .contains("disruptive or destructive"),
            "{}",
            snapshot.problems[1].message
        );
    }

    /// A runner whose session dies on a chosen command.
    struct Dying {
        inner: Recorded,
        dies_on: &'static str,
    }

    #[async_trait]
    impl Runner for Dying {
        async fn run(&mut self, command: &str) -> Result<String, DriverError> {
            if command == self.dies_on {
                self.inner.run.push(command.to_string());
                return Err(DriverError::Timeout {
                    waiting_for: format!("running `{command}`"),
                    seconds: 30,
                    last_output: String::new(),
                });
            }
            self.inner.run(command).await
        }
    }

    #[tokio::test]
    async fn nothing_more_is_sent_once_the_session_is_lost() {
        let (collector, platform) = platform();
        let mut device = Dying {
            inner: Recorded::new()
                .with("show version", VERSION)
                .with("show cpu", "cpu 17%\n")
                .with("show vlan", "1 default active\n"),
            dies_on: "show interfaces",
        };
        let snapshot = collector
            .collect(
                &mut device,
                &platform,
                &[Kind::Facts, Kind::Interfaces, Kind::Vlans, Kind::Health],
            )
            .await;
        assert_eq!(
            device.inner.commands_run(),
            ["show version", "show interfaces"]
        );
        assert!(
            snapshot.facts.is_some(),
            "what was collected before is kept"
        );
        assert_eq!(
            codes(&snapshot, Kind::Interfaces),
            vec![ProblemCode::Failed]
        );
        assert!(
            snapshot.problems[0].message.contains("no response for 30s"),
            "{}",
            snapshot.problems[0].message
        );
        assert_eq!(codes(&snapshot, Kind::Vlans), vec![ProblemCode::Failed]);
        assert!(
            snapshot.problems[1]
                .message
                .contains("not run: the session was lost while running `show interfaces`"),
            "{}",
            snapshot.problems[1].message
        );
        // `show version` was answered before the loss, so health still has its uptime;
        // `show cpu` could not be asked for.
        let health = snapshot.health.as_ref().unwrap();
        assert_eq!((health.uptime_s, health.cpu_pct), (Some(3900), None));
        assert_eq!(codes(&snapshot, Kind::Health), vec![ProblemCode::Partial]);
    }

    #[test]
    fn map_output_previews_one_step() {
        let (collector, platform) = platform();
        let step = &platform.profile.collect[&Kind::Interfaces][0];
        let rows = collector.map_output(&platform, step, INTERFACES).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            Value::Object(rows[0].clone()),
            json!({
                "name": "Gi0/1",
                "admin_up": true,
                "oper_up": true,
                "mtu": "1500",
                "ipv4": ["10.0.0.1/24"],
                "counters": {"in_errors": 1204},
            }),
            "mapped but not yet canonicalised or merged"
        );
        assert_eq!(
            Value::Object(rows[1].clone()),
            json!({"name": "Gi0/2", "admin_up": false, "oper_up": false, "mtu": "1500"})
        );

        // A field that cannot be computed is an error here, so a preview shows it.
        let routes = &platform.profile.collect[&Kind::Routes][0];
        let error = collector
            .map_output(
                &platform,
                routes,
                "C 10.0.0.0/24 via 0.0.0.0 on Vl10 hosts many\n",
            )
            .unwrap_err();
        assert!(
            error.starts_with("map.metric:") && error.ends_with("(row 1)"),
            "{error}"
        );

        // ntc-templates through the profile's `ntc_platform`, upper- and lower-case names.
        let ntc: CollectStep = serde_yaml_ng::from_str(
            "command: show ip interface brief\nparser: {ntc: auto}\nmap: {name: INTERFACE, ipv4: ip_address, oper_up: \"row.PROTO == 'up'\"}\nwhen: \"STATUS != 'deleted'\"\n",
        )
        .unwrap();
        let rows = collector
            .map_output(
                &platform,
                &ntc,
                "Interface              IP-Address      OK? Method Status                Protocol\nVlan1                  10.0.0.2        YES NVRAM  up                    up\nVlan2                  unassigned      YES unset  deleted               down\n",
            )
            .unwrap();
        assert_eq!(
            rows.into_iter().map(Value::Object).collect::<Vec<_>>(),
            vec![json!({"name": "Vlan1", "ipv4": "10.0.0.2", "oper_up": true})]
        );
        let missing: CollectStep =
            serde_yaml_ng::from_str("command: show nonsense\nparser: {ntc: auto}\n").unwrap();
        let error = collector.map_output(&platform, &missing, "").unwrap_err();
        assert!(error.contains("no template for `show nonsense`"), "{error}");
        let bad_json: CollectStep =
            serde_yaml_ng::from_str("command: show x\nparser: {json: 'a.*'}\n").unwrap();
        assert!(
            collector
                .map_output(&platform, &bad_json, "not json")
                .unwrap_err()
                .contains("no JSON")
        );
    }

    #[test]
    fn rows_that_do_not_fit_the_model_are_dropped_and_counted() {
        let mut snapshot = Snapshot::default();
        let rows: Vec<Row> = [
            json!({"name": "Gi0/1", "counters": {"in_errors": "7"}}),
            json!({"name": "Gi0/2", "counters": "not an object"}),
            json!({"name": "Gi0/3", "counters": 5}),
        ]
        .into_iter()
        .filter_map(|row| row.as_object().cloned())
        .collect();
        store(&mut snapshot, Kind::Interfaces, rows);
        let interfaces = snapshot.interfaces.as_ref().unwrap();
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].counters.in_errors, Some(7));
        assert_eq!(
            codes(&snapshot, Kind::Interfaces),
            vec![ProblemCode::Partial]
        );
        let message = &snapshot.problems[0].message;
        assert!(
            message.starts_with("2 of 3 rows did not fit the model and were dropped; first: ")
                && message.contains("Gi0/2"),
            "{message}"
        );

        // `extra` takes anything, as text.
        let mut snapshot = Snapshot::default();
        let facts =
            json!({"hostname": "sw1", "extra": {"ports": 48, "stack": ["a", "b"], "poe": true}});
        store(
            &mut snapshot,
            Kind::Facts,
            vec![facts.as_object().cloned().unwrap()],
        );
        let extra = &snapshot.facts.as_ref().unwrap().extra;
        assert_eq!(
            (
                extra["ports"].as_str(),
                extra["stack"].as_str(),
                extra["poe"].as_str()
            ),
            ("48", "a, b", "true")
        );
        assert_eq!(snapshot.problems, vec![]);
    }

    #[test]
    fn rows_that_share_a_key_are_paired_in_order() {
        let model = Kind::Interfaces.empty_row();
        let names = Names::for_kind(Kind::Interfaces, InterfaceNames::Cisco);
        let rows = |values: serde_json::Value| -> Vec<Row> {
            values
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row.as_object().cloned())
                .collect()
        };
        // The same listing parsed twice: two rows are called Gi0/2 and each keeps its own counters.
        let mut existing = rows(json!([{"name": "Gi0/1"}, {"name": "Gi0/2"}, {"name": "Gi0/2"}]));
        let merged = merge(
            &mut existing,
            rows(json!([
                {"name": "Gi0/2", "mtu": 1500},
                {"name": "Gi0/1", "mtu": 9000},
                {"name": "Gi0/2", "mtu": 1400},
                {"name": "Gi0/7", "mtu": 1},
            ])),
            "name",
            &names,
            &model,
        );
        assert_eq!(
            merged,
            (3, 4),
            "three of the four rows offered found a home"
        );
        assert_eq!(
            existing
                .iter()
                .map(|row| row.get("mtu").cloned())
                .collect::<Vec<_>>(),
            vec![Some(json!(9000)), Some(json!(1500)), Some(json!(1400))]
        );

        // One row per key on one side only: every new row goes into every row with that key.
        let mut existing = rows(json!([{"name": "Vlan10"}, {"name": "Vlan10"}]));
        let merged = merge(
            &mut existing,
            rows(json!([{"name": "Vl10", "ipv4": ["10.0.0.1/24"]}])),
            "name",
            &names,
            &model,
        );
        assert_eq!(merged, (1, 1));
        assert!(
            existing
                .iter()
                .all(|row| row["ipv4"] == json!(["10.0.0.1/24"]))
        );
    }

    #[test]
    fn commands_become_file_names() {
        for (command, slug) in [
            ("show mac address-table", "show_mac_address-table"),
            (
                "/ip route print terse without-paging",
                "ip_route_print_terse_without-paging",
            ),
            ("cat /proc/meminfo", "cat_proc_meminfo"),
            ("ip -j -s addr show", "ip_-j_-s_addr_show"),
            ("show ip route vrf *", "show_ip_route_vrf"),
            (
                "  show processes cpu | include CPU  ",
                "show_processes_cpu_include_CPU",
            ),
        ] {
            assert_eq!(command_slug(command), slug);
        }
    }

    /// The built-in profile whose QoS read these tests are about.
    fn cisco_ios() -> (Collector, Arc<Platform>) {
        let (registry, errors) = Registry::builtin();
        assert_eq!(errors, vec![], "the built-in profiles load");
        let platform = registry.get("cisco_ios").expect("cisco_ios is built in");
        (Collector::new(registry.expressions()), platform)
    }

    #[tokio::test]
    async fn no_policy_is_an_empty_table_and_a_refused_read_is_no_table() {
        let (collector, ios) = cisco_ios();

        // A device with nothing attached answers with nothing. That is a
        // read that worked: zero rows, and no problem.
        let mut bare = Recorded::new().with("show policy-map interface", "");
        let snapshot = collector.collect(&mut bare, &ios, &[Kind::Qos]).await;
        assert_eq!(snapshot.qos, Some(vec![]));
        assert_eq!(snapshot.rows(Kind::Qos), Some(vec![]));
        assert!(snapshot.has(Kind::Qos));
        assert_eq!(snapshot.problems, vec![]);

        // A device that refuses the command has told us nothing about its
        // policies, and the snapshot must not look as if it had none.
        let mut refusing = Recorded::new();
        let snapshot = collector.collect(&mut refusing, &ios, &[Kind::Qos]).await;
        assert_eq!(snapshot.qos, None);
        assert_eq!(snapshot.rows(Kind::Qos), None);
        assert!(!snapshot.has(Kind::Qos));
        assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Failed]);

        // A platform without the read is a third thing again.
        let (collector, acme) = platform();
        let mut any = Recorded::new();
        let snapshot = collector.collect(&mut any, &acme, &[Kind::Qos]).await;
        assert_eq!(snapshot.qos, None);
        assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Unsupported]);
        assert!(any.commands_run().is_empty());
    }

    #[tokio::test]
    async fn output_the_qos_template_cannot_place_fails_the_read_whole() {
        let (collector, platform) = cisco_ios();
        // The second class starts where nothing in a policy does, so its
        // place in the tree cannot be told. Reading on would file its
        // counters under a guess, so the read stops — and the class that
        // could be read is not kept either, because a table that leaves a
        // class out without saying so is worse than none.
        let output = " GigabitEthernet0/1\n\n  Service-policy output: WAN\n\n    Class-map: VOICE (match-all)\n      10 packets, 800 bytes\n  Class-map: VIDEO (match-all)\n      20 packets, 1600 bytes\n";
        let mut device = Recorded::new().with("show policy-map interface", output);
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(snapshot.qos, None);
        assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Failed]);
        let message = &snapshot.problems[0].message;
        assert!(
            message.contains("at an indentation this profile does not know")
                && message.contains("Class-map: VIDEO"),
            "{message}"
        );
    }

    /// What `cisco_ios` makes of an answer to `show policy-map interface`.
    async fn read_policies(output: &str) -> Snapshot {
        let (collector, platform) = cisco_ios();
        let mut device = Recorded::new().with("show policy-map interface", output);
        collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await
    }

    /// The one problem a failed QoS read reports.
    fn failure(snapshot: &Snapshot) -> &str {
        assert_eq!(snapshot.qos, None, "{:?}", snapshot.problems);
        assert_eq!(codes(snapshot, Kind::Qos), vec![ProblemCode::Failed]);
        &snapshot.problems[0].message
    }

    #[tokio::test]
    async fn an_answer_that_is_not_a_listing_of_policies_is_not_read_as_no_policy() {
        // All hand-written. A device that says something this profile does
        // not understand has not said that nothing is attached.
        let message = read_policies("QoS statistics are not available on this platform\n").await;
        assert!(
            failure(&message).contains("does not begin with the name of an interface"),
            "{:?}",
            message.problems
        );

        // What may stand before the first interface without being one: the
        // two lines of `exec prompt timestamp`, and a log message that
        // reached the terminal.
        let preamble = "Load for five secs: 1%/0%; one minute: 1%; five minutes: 1%\nTime source is NTP, 10:11:12.123 UTC Mon Nov 23 2026\n\n*Nov 23 10:11:12.001: %LINK-3-UPDOWN: Interface GigabitEthernet0/2, changed state to up\n";
        let nothing_attached = read_policies(preamble).await;
        assert_eq!(nothing_attached.qos, Some(vec![]));
        assert_eq!(nothing_attached.problems, vec![]);
        let attached = format!(
            "{preamble} GigabitEthernet0/1\n\n  Service-policy output: WAN\n\n    Class-map: class-default (match-any)\n      10 packets, 800 bytes\n"
        );
        let snapshot = read_policies(&attached).await;
        assert_eq!(snapshot.problems, vec![]);
        assert_eq!(snapshot.qos.as_ref().map(Vec::len), Some(1));
        // A router whose clock was never set words the second line otherwise.
        let unset_clock = read_policies(
            "Load for five secs: 0%/0%; one minute: 0%; five minutes: 0%\nNo time source, *00:11:35.455 UTC Mon Mar 1 1993\n",
        )
        .await;
        assert_eq!(unset_clock.qos, Some(vec![]));
        assert_eq!(unset_clock.problems, vec![]);

        // A log message is no part of the listing wherever it lands, also
        // between a policer's counter and the action on the line after it.
        let whole = include_str!(
            "../tests/profiles/cisco_ios/captures/qos-lab-policed-input/show_policy-map_interface.txt"
        );
        let interrupted = whole.replace(
            "          drop\n",
            "*Nov 23 10:11:12.001: %SYS-5-CONFIG_I: Configured from console by admin on vty0\n          drop\n",
        );
        assert_ne!(interrupted, whole);
        assert_eq!(
            read_policies(&interrupted).await.qos,
            read_policies(whole).await.qos
        );
    }

    #[tokio::test]
    async fn a_class_is_never_filed_under_a_name_it_was_not_printed_under() {
        // All hand-written. Each answer is missing one of the three lines
        // that say where a class belongs; before, the classes were filed
        // under whatever had been named last (or under no interface at all).
        let class = "    Class-map: B (match-all)\n      2 packets, 200 bytes\n";
        let mut wrong: Vec<String> = Vec::new();
        let first = " GigabitEthernet0/1\n\n  Service-policy output: FIRST\n\n    Class-map: A (match-all)\n      1 packets, 100 bytes\n\n";
        for (output, expected) in [
            (
                format!("  Service-policy output: FIRST\n\n{class}"),
                "does not begin with the name of an interface",
            ),
            (
                format!("{first} GigabitEthernet0/2\n\n{class}"),
                "a class before the policy of its interface is named",
            ),
            (
                format!("{first}  Service-policy input:\n\n{class}"),
                "without the name of a policy",
            ),
            (
                format!(" GigabitEthernet0/2\n\n  Service-policy input:\n\n{class}"),
                "followed by something other than `Service-policy input|output: NAME`",
            ),
            (
                format!("{first}      Service-policy :\n\n    {class}"),
                "a `Service-policy :` line without the name of a policy",
            ),
            (
                // Four tabs are not four columns: a tab has no column.
                format!("{first}\t\t\t\tClass-map: B (match-all)\n"),
                "at an indentation this profile does not know",
            ),
            (
                // The line ends before it says how the class matches.
                format!("{first}    Class-map: B"),
                "a class whose line is incomplete",
            ),
        ] {
            let snapshot = read_policies(&output).await;
            let refused = snapshot.qos.is_none()
                && snapshot
                    .problems
                    .iter()
                    .any(|problem| problem.message.contains(expected));
            if !refused {
                wrong.push(format!(
                    "expected `{expected}`, got {:?} and {:?}",
                    snapshot.qos, snapshot.problems
                ));
            }
        }
        // All of them, so that one that is read does not hide the next.
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[tokio::test]
    async fn a_number_is_not_read_from_a_line_that_ends_inside_it() {
        // The simulator's shaped uplink (synthetic), cut in the middle of
        // the shaper's rate: `cir 5000` of `cir 5000000, bc 20000, be 20000`.
        let whole = include_str!(
            "../tests/profiles/cisco_ios/captures/qos-lab-shaped-uplink/show_policy-map_interface.txt"
        );
        let at = whole.find("cir 5000000,").expect("the shaper's line") + "cir 5000".len();
        let snapshot = read_policies(&whole[..at]).await;
        assert_eq!(snapshot.problems, vec![]);
        let classes = snapshot.qos.as_ref().unwrap();
        assert_eq!(classes.len(), 1);
        assert_eq!(
            classes[0].rate_bps, None,
            "5000 is the beginning of 5000000"
        );
        // What stood on whole lines before the cut is read as it is.
        assert_eq!(classes[0].queue_dropped_packets, Some(2031));
        assert_eq!(
            read_policies(whole).await.qos.unwrap()[0].rate_bps,
            Some(5_000_000)
        );
    }

    #[tokio::test]
    async fn a_class_further_in_than_any_policy_announced_for_it_is_the_attached_policys() {
        // Hand-written. The first interface has three levels; on the second
        // the attached policy is shown as far in as the innermost of them,
        // and nothing announces a policy inside it. Its class used to be
        // filed under the innermost policy of the interface before.
        let output = " GigabitEthernet0/1\n\n  Service-policy output: TOP\n\n    Class-map: A (match-all)\n      1 packets, 100 bytes\n      Service-policy : MID\n        Class-map: B (match-all)\n          2 packets, 200 bytes\n          Service-policy : LEAF\n            Class-map: C (match-all)\n              3 packets, 300 bytes\n\n GigabitEthernet0/2\n\n  Service-policy output: OTHER\n\n            Class-map: D (match-all)\n              4 packets, 400 bytes\n\n GigabitEthernet0/3\n\n  Service-policy output: THIRD\n\n    Class-map: E (match-all)\n      5 packets, 500 bytes\n      Service-policy : INNER\n            Class-map: F (match-all)\n              6 packets, 600 bytes\n";
        let snapshot = read_policies(output).await;
        assert_eq!(snapshot.problems, vec![]);
        let place = |class: &str, policy: &str, parent: Option<&str>| {
            (
                class.to_string(),
                policy.to_string(),
                parent.map(str::to_string),
            )
        };
        assert_eq!(
            tree(&snapshot),
            [
                place("A", "TOP", None),
                place("B", "MID", Some("A")),
                place("C", "LEAF", Some("B")),
                place("D", "OTHER", None),
                place("E", "THIRD", None),
                // Announced one level in, shown two levels in: still INNER's.
                place("F", "INNER", Some("E")),
            ]
        );
        assert_eq!(
            snapshot.qos.as_ref().unwrap()[3].interface,
            "GigabitEthernet0/2"
        );
    }

    #[tokio::test]
    async fn a_class_that_counts_packets_alone_does_not_take_the_bytes_of_a_match_statement() {
        // Hand-written: the class line of the Catalyst 6500 sample in
        // Cisco's reference (`N packets`, no bytes) above match statements
        // that count both, as older releases print them for match-any.
        let output = " GigabitEthernet3/1\n\n  Service-policy input: in1\n\n    Class-map: p1 (match-any)\n      12 packets\n      Match:  precedence 1\n        7 packets, 700 bytes\n        5 minute rate 0 bps\n      Match:  precedence 2\n        5 packets, 500 bytes\n        5 minute rate 0 bps\n";
        let snapshot = read_policies(output).await;
        assert_eq!(snapshot.problems, vec![]);
        let classes = snapshot.qos.as_ref().unwrap();
        assert_eq!(
            (classes[0].packets, classes[0].bytes),
            (Some(12), None),
            "700 is the first match statement's, not the class's"
        );
    }

    #[tokio::test]
    async fn counters_in_the_upper_half_of_64_bits_are_read_exactly_or_not_at_all() {
        // Hand-written, with numbers no interface has ever counted: what
        // matters is that none of them comes out as a different number.
        let output = " TenGigabitEthernet1/1/1\n\n  Service-policy output: BIG\n\n    Class-map: HUGE (match-all)\n      18446744073709551615 packets, 18446744073709551614 bytes\n      5 minute offered rate 9999999999 bps, drop rate 4294967296 bps\n      Match: any\n      Queueing\n      queue limit 64 packets\n      (queue depth/total drops/no-buffer drops) 4294967296/9300000000000000001/0\n      (pkts output/bytes output) 1/1\n      shape (average) cir 100000000000, bc 400000000, be 400000000\n      police:\n          cir 40000000000 bps, bc 1250000000 bytes\n        conformed 9007199254740993 packets, 9007199254740995 bytes; actions:\n          transmit\n        exceeded 9300000000000000003 packets, 18446744073709551001 bytes; actions:\n          drop\n        conformed 65000 bps, exceed 98000 bps\n\n    Class-map: class-default (match-any)\n      4294967296 packets, 281474976710656 bytes\n      5 minute offered rate 0 bps, drop rate 0 bps\n      Match: any\n      Priority: 40000000 kbps, burst bytes 1500, b/w exceed drops: 9007199254740993\n";
        let snapshot = read_policies(output).await;
        let classes = snapshot.qos.as_ref().expect("the rows are kept");
        let huge = &classes[0];
        assert_eq!(huge.packets, Some(u64::MAX));
        assert_eq!(huge.bytes, Some(u64::MAX - 1));
        assert_eq!(huge.rate_bps, Some(100_000_000_000));
        assert_eq!(huge.queued_packets, Some(4_294_967_296));
        // Both beyond 2^63, where a float is 2048 wide.
        assert_eq!(huge.queue_dropped_packets, Some(9_300_000_000_000_000_001));
        assert_eq!(huge.police_dropped_packets, Some(9_300_000_000_000_000_003));
        // Their sum does not fit in 64 bits. It is left out and reported,
        // not replaced by the largest number that does fit.
        assert_eq!(huge.dropped_packets, None);
        assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Partial]);
        assert!(
            snapshot.problems[0]
                .message
                .contains("map.dropped_packets: number out of range"),
            "{:?}",
            snapshot.problems
        );
        let default = &classes[1];
        assert_eq!(default.rate_bps, Some(40_000_000_000));
        // 2^53 + 1, which a float cannot hold.
        assert_eq!(default.queue_dropped_packets, Some(9_007_199_254_740_993));
        assert_eq!(default.dropped_packets, Some(9_007_199_254_740_993));
    }

    #[tokio::test]
    async fn the_rows_of_an_old_wred_table_are_not_taken_for_a_circuit_below_the_interface() {
        // Hand-written after the first sample of Cisco's reference (the
        // layout before hierarchical queueing, WRED table at the left
        // margin), with a table by DSCP instead of by precedence: its rows
        // begin `af11`, `cs1`, which look like the name of an interface.
        let output = " Serial3/1\n  Service-policy output: mypolicy\n    Class-map: silver (match-all)\n      0 packets, 0 bytes\n      5 minute offered rate 0 bps, drop rate 0 bps\n      Match: ip precedence 1\n      Weighted Fair Queueing\n        Output Queue: Conversation 266\n        Bandwidth 80 (kbps)\n        (pkts matched/bytes matched) 0/0\n        (depth/total drops/no-buffer drops) 0/7/0\n         exponential weight: 9\n         mean queue depth: 0\ndscp      Transmitted       Random drop      Tail drop    Minimum Maximum  Mark\n          pkts/bytes        pkts/bytes       pkts/bytes    thresh  thresh  prob\naf11          0/0               3/300            4/400         20      40  1/10\ncs1           0/0               0/0              0/0           24      40  1/10\ndefault       0/0               0/0              0/0           26      40  1/10\n    Class-map: class-default (match-any)\n      9 packets, 900 bytes\n      5 minute offered rate 0 bps, drop rate 0 bps\n      Match: any\n";
        let snapshot = read_policies(output).await;
        assert_eq!(snapshot.problems, vec![]);
        let classes = snapshot.qos.as_ref().unwrap();
        assert_eq!(
            classes
                .iter()
                .map(|class| (
                    class.class.as_str(),
                    class.interface.as_str(),
                    class.rate_bps,
                    class.queue_dropped_packets,
                    class.packets
                ))
                .collect::<Vec<_>>(),
            vec![
                ("silver", "Serial3/1", Some(80_000), Some(7), Some(0)),
                ("class-default", "Serial3/1", None, None, Some(9)),
            ]
        );
        // A real circuit below the interface is still refused.
        let dlci = read_policies(" Serial3/1.1: DLCI 201 -\n  Service-policy output: X\n").await;
        assert!(
            failure(&dlci).contains("a policy attached below the interface"),
            "{:?}",
            dlci.problems
        );
    }

    /// Every number of a row, in a fixed order.
    fn numbers(class: &QosClass) -> [Option<u64>; 10] {
        [
            class.rate_bps,
            class.packets,
            class.bytes,
            class.dropped_packets,
            class.dropped_bytes,
            class.queue_dropped_packets,
            class.police_dropped_packets,
            class.queued_packets,
            class.offered_rate_bps,
            class.drop_rate_bps,
        ]
    }

    /// Whether `cut`, read from an answer that was cut short, says anything
    /// the whole answer does not: every class it has must stand where the
    /// whole answer puts it, and every number must be the whole answer's or
    /// be absent.
    fn says_nothing_else(cut: &[QosClass], whole: &[QosClass]) -> Result<(), String> {
        if cut.len() > whole.len() {
            return Err(format!(
                "{} classes, the whole has {}",
                cut.len(),
                whole.len()
            ));
        }
        for (cut, whole) in cut.iter().zip(whole) {
            let place = |class: &QosClass| {
                (
                    class.interface.clone(),
                    class.direction,
                    class.policy.clone(),
                    class.class.clone(),
                    class.parent.clone(),
                )
            };
            if place(cut) != place(whole) {
                return Err(format!("{:?} is not {:?}", place(cut), place(whole)));
            }
            for (cut_number, whole_number) in numbers(cut).iter().zip(numbers(whole)) {
                if cut_number.is_some() && *cut_number != whole_number {
                    return Err(format!(
                        "{}: {cut_number:?} where the whole answer has {whole_number:?}",
                        cut.class
                    ));
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn an_answer_cut_short_never_turns_into_a_different_number() {
        // The session only hands over an answer that ended in a prompt, so
        // this should not happen; if it ever does, a wrong counter must not
        // be what comes of it. The samples are the simulator's two and the
        // hand-written three-level one (all synthetic; see their SOURCE.md).
        let samples = [
            include_str!(
                "../tests/profiles/cisco_ios/captures/qos-lab-policed-input/show_policy-map_interface.txt"
            ),
            include_str!(
                "../tests/profiles/cisco_ios/captures/qos-lab-shaped-uplink/show_policy-map_interface.txt"
            ),
            include_str!(
                "../tests/profiles/cisco_ios/captures/qos-synthetic-three-levels/show_policy-map_interface.txt"
            ),
        ];
        let (collector, platform) = cisco_ios();
        let mut failed = 0;
        let mut read = 0;
        for sample in samples {
            let mut device = Recorded::new().with("show policy-map interface", sample);
            let whole = collector
                .collect(&mut device, &platform, &[Kind::Qos])
                .await
                .qos
                .expect("the whole sample is read");
            // Cut after every line, and (for the first, shortest sample)
            // after every character.
            let every_character = sample.len() < 1_000;
            for (at, character) in sample.char_indices() {
                if !every_character && character != '\n' {
                    continue;
                }
                let prefix = &sample[..at];
                let mut device = Recorded::new().with("show policy-map interface", prefix);
                let snapshot = collector
                    .collect(&mut device, &platform, &[Kind::Qos])
                    .await;
                let Some(cut) = &snapshot.qos else {
                    assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Failed]);
                    failed += 1;
                    continue;
                };
                read += 1;
                if let Err(difference) = says_nothing_else(cut, &whole) {
                    panic!(
                        "cut after {at} characters ({:?}): {difference}",
                        prefix.lines().last().unwrap_or("")
                    );
                }
            }
        }
        // Both things happen: a cut in the middle of a line that cannot be
        // read is an error, a cut between lines leaves rows with less in them.
        assert!(failed > 20 && read > 100, "{failed} failed, {read} read");
    }

    #[tokio::test]
    async fn a_policer_whose_action_did_not_arrive_has_not_dropped_nothing() {
        // The simulator's policer sample (synthetic), cut after the line
        // that counts the exceeding packets and before the one that says
        // they are dropped. Before, this read as a policer that drops
        // nothing: 0 where the whole answer says 4687.
        let whole = include_str!(
            "../tests/profiles/cisco_ios/captures/qos-lab-policed-input/show_policy-map_interface.txt"
        );
        let at = whole.find("          drop\n").expect("the action line");
        let snapshot = read_policies(&whole[..at]).await;
        assert_eq!(snapshot.problems, vec![]);
        let class = &snapshot.qos.as_ref().unwrap()[0];
        assert_eq!(class.packets, Some(7812));
        assert_eq!(class.police_dropped_packets, None);
        assert_eq!(class.dropped_packets, None);
        assert_eq!(class.dropped_bytes, None);
        // The whole answer, for comparison.
        let snapshot = read_policies(whole).await;
        let class = &snapshot.qos.as_ref().unwrap()[0];
        assert_eq!(class.police_dropped_packets, Some(4687));
        assert_eq!(class.dropped_bytes, Some(3_750_000));
    }

    #[tokio::test]
    async fn what_a_device_printed_is_quoted_in_a_problem_only_up_to_a_limit() {
        // A class line no rule can place is quoted in the error. This one is
        // 300 000 characters long.
        let line = format!("Class-map: {} (match-all)", "A".repeat(300_000));
        let output = format!(" GigabitEthernet0/0\n\n  Service-policy output: X\n\n{line}\n");
        let snapshot = read_policies(&output).await;
        let message = failure(&snapshot);
        assert!(
            message.contains("Input Line: Class-map: AAAA"),
            "{}",
            &message[..300]
        );
        assert!(
            message.ends_with(" more characters)"),
            "{}",
            &message[message.len() - 60..]
        );
        assert!(message.chars().count() < MESSAGE_LIMIT + 40);
        let step_error = snapshot.steps[0].error.as_deref().expect("the step failed");
        assert!(step_error.chars().count() < MESSAGE_LIMIT + 40);

        // A message that fits is left as it is, whatever it is made of.
        let mut short = "é".repeat(MESSAGE_LIMIT);
        bound(&mut short);
        assert_eq!(short.chars().count(), MESSAGE_LIMIT);
        let mut long = "é".repeat(MESSAGE_LIMIT + 3);
        bound(&mut long);
        assert!(
            long.ends_with("é… (3 more characters)"),
            "{}",
            &long[long.len() - 40..]
        );
    }

    #[tokio::test]
    async fn a_queue_listing_printed_as_a_table_is_a_failed_read_not_an_empty_one() {
        // Hand-written. `mikrotik_routeros` asks for `print stats detail`;
        // should a release answer with a table all the same, its rows carry
        // no `key=value` and the parser reads none of them.
        let (registry, errors) = Registry::builtin();
        assert_eq!(errors, vec![]);
        let platform = registry.get("mikrotik_routeros").expect("built in");
        let collector = Collector::new(registry.expressions());
        let simple = "/queue simple print stats detail without-paging";
        let tree = "/queue tree print stats detail without-paging";
        let table = "Flags: X - disabled, I - invalid \n #   NAME    PARENT  PACKET-MARK  RATE  PACKET-RATE  QUEUED-BYTES  QUEUED-PACKETS  BYTES  PACKETS  DROPPED\n 0   wan-up  ether1  all          0     0            0             0               1000   10       1\n";
        let legend = "Flags: X - disabled, I - invalid \n";

        let mut device = Recorded::new().with(simple, table).with(tree, table);
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(
            snapshot.qos, None,
            "queues exist, so this is not `no queues`"
        );
        assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Failed]);
        assert!(
            snapshot.problems[0]
                .message
                .contains("not in the `key=value` form this parser reads")
                && snapshot.problems[0].message.contains("0   wan-up  ether1"),
            "{:?}",
            snapshot.problems
        );

        // No queues at all is still an empty table and no problem.
        let mut device = Recorded::new().with(simple, legend).with(tree, legend);
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(snapshot.qos, Some(vec![]));
        assert_eq!(snapshot.problems, vec![]);
    }

    #[tokio::test]
    async fn neither_half_is_read_from_a_queue_counter_that_is_not_a_pair() {
        // Hand-written. A simple queue counts `upload/download`; a value of
        // any other shape used to give up its first or its last number as
        // one of the halves.
        let (registry, errors) = Registry::builtin();
        assert_eq!(errors, vec![]);
        let platform = registry.get("mikrotik_routeros").expect("built in");
        let collector = Collector::new(registry.expressions());
        let simple = " 0    name=\"odd\" target=ether3 bytes=1 packets=3/4/5 dropped=/6 queued-packets=7/ \n 1    name=\"even\" target=ether4 bytes=10/20 packets=30/40 dropped=50/60 queued-packets=70/80 \n";
        let mut device = Recorded::new()
            .with("/queue simple print stats detail without-paging", simple)
            .with(
                "/queue tree print stats detail without-paging",
                "Flags: X - disabled, I - invalid \n",
            );
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        let read: Vec<_> = snapshot
            .qos
            .as_ref()
            .unwrap()
            .iter()
            .map(|queue| {
                (
                    queue.class.as_str(),
                    queue.direction.as_str(),
                    [
                        queue.bytes,
                        queue.packets,
                        queue.dropped_packets,
                        queue.queued_packets,
                    ],
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                ("odd", "in", [None; 4]),
                ("even", "in", [Some(10), Some(30), Some(50), Some(70)]),
                ("odd", "out", [None; 4]),
                ("even", "out", [Some(20), Some(40), Some(60), Some(80)]),
            ]
        );
    }

    /// (class, policy, parent) of every row, in order.
    fn tree(snapshot: &Snapshot) -> Vec<(String, String, Option<String>)> {
        snapshot
            .qos
            .as_ref()
            .unwrap_or_else(|| panic!("no qos table: {:?}", snapshot.problems))
            .iter()
            .map(|class| {
                (
                    class.class.clone(),
                    class.policy.clone(),
                    class.parent.clone(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn the_policy_tree_is_read_the_same_whichever_column_the_output_starts_in() {
        let (collector, platform) = cisco_ios();
        // Hand-written, three levels deep; see the case's SOURCE.md.
        let standard = include_str!(
            "../tests/profiles/cisco_ios/captures/qos-synthetic-three-levels/show_policy-map_interface.txt"
        );
        let mut device = Recorded::new().with("show policy-map interface", standard);
        let expected = tree(
            &collector
                .collect(&mut device, &platform, &[Kind::Qos])
                .await,
        );
        assert_eq!(
            expected[..6],
            [
                ("SITE-A", "TOP", None),
                ("DATA", "MID", Some("SITE-A")),
                ("GOLD", "LEAF", Some("DATA")),
                ("class-default", "LEAF", Some("DATA")),
                ("class-default", "MID", Some("SITE-A")),
                ("class-default", "TOP", None),
            ]
            .map(|(class, policy, parent)| (
                class.to_string(),
                policy.to_string(),
                parent.map(str::to_string)
            ))
        );

        // IOS does not start every policy in the same column (a port-channel
        // subinterface is shown one further in), so everything below the
        // interface line is moved as a block: one column out, up to three in.
        for shift in [-1_i32, 1, 2, 3] {
            let mut device =
                Recorded::new().with("show policy-map interface", &moved(standard, shift));
            let snapshot = collector
                .collect(&mut device, &platform, &[Kind::Qos])
                .await;
            // Three columns in, the innermost of the three policies leaves
            // the columns a policy is looked for in. That is an error, not
            // a guess.
            if shift == 3 {
                assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Failed]);
                assert!(
                    snapshot.problems[0]
                        .message
                        .contains("nested deeper than three levels"),
                    "{:?}",
                    snapshot.problems
                );
                continue;
            }
            assert_eq!(snapshot.problems, vec![], "moved by {shift}");
            assert_eq!(tree(&snapshot), expected, "moved by {shift}");
        }
    }

    /// `output` with everything below each interface line moved `shift`
    /// columns to the right (to the left when negative).
    fn moved(output: &str, shift: i32) -> String {
        output
            .lines()
            .map(|line| {
                let is_interface = line.starts_with(' ') && !line.starts_with("  ");
                match shift {
                    _ if is_interface || line.is_empty() => line.to_string(),
                    -1 => line[1..].to_string(),
                    _ => format!("{}{line}", " ".repeat(shift as usize)),
                }
            })
            .map(|line| line + "\n")
            .collect()
    }

    #[tokio::test]
    async fn an_attached_policy_shown_further_in_is_not_taken_for_a_nested_one() {
        let (collector, platform) = cisco_ios();
        // The simulator's sample of a shaper with a queueing policy inside
        // it (synthetic), as it stands…
        let nested = include_str!(
            "../tests/profiles/cisco_ios/captures/qos-lab-shaped-uplink/show_policy-map_interface.txt"
        );
        // …and a flat policy on another interface.
        let flat = include_str!(
            "../tests/profiles/cisco_ios/captures/qos-lab-policed-input/show_policy-map_interface.txt"
        );
        let place = |class: &str, policy: &str, parent: Option<&str>| {
            (
                class.to_string(),
                policy.to_string(),
                parent.map(str::to_string),
            )
        };
        let nested_tree = [
            place("class-default", "WAN-EDGE", None),
            place("VOICE", "WAN-QUEUE", Some("class-default")),
            place("VIDEO", "WAN-QUEUE", Some("class-default")),
            place("class-default", "WAN-QUEUE", Some("class-default")),
        ];
        let flat_tree = [
            place("USERS", "LIMIT-USERS", None),
            place("class-default", "LIMIT-USERS", None),
        ];

        // Three columns further in, the attached policy's classes stand
        // where a nested policy's would. Nothing announced a nested policy,
        // so they are the attached policy's — and the policy that *is*
        // announced inside one of them is still told apart.
        let mut device = Recorded::new().with("show policy-map interface", &moved(nested, 3));
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        assert_eq!(tree(&snapshot), nested_tree);

        // The same holds right after a policy that did have one inside it:
        // the name of that inner policy is not left standing for the next
        // attached policy to be filed under.
        let both = format!("{nested}\n{}", moved(flat, 3));
        let mut device = Recorded::new().with("show policy-map interface", &both);
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        assert_eq!(tree(&snapshot), [&nested_tree[..], &flat_tree[..]].concat());
        let classes = snapshot.qos.as_ref().unwrap();
        assert_eq!(classes[4].interface, "GigabitEthernet0/0");
        assert_eq!(classes[3].interface, "GigabitEthernet0/1");
    }

    #[tokio::test]
    async fn a_priority_class_gives_its_drops_however_its_rate_is_worded() {
        let (collector, platform) = cisco_ios();
        // Hand-written. The second class's line is how IOS XE is remembered
        // to word `priority` without a rate; it is in no reference and no
        // recording, which is the point: a wording the profile has not seen
        // must not cost the class its drop counter.
        let output = " GigabitEthernet0/1\n\n  Service-policy output: WAN\n\n    Class-map: VOICE (match-any)\n      10 packets, 800 bytes\n      Priority: 10% (5000 kbps), burst bytes 125000, b/w exceed drops: 3\n\n    Class-map: SIGNAL (match-any)\n      20 packets, 1600 bytes\n      Priority: Strict, b/w exceed drops: 7\n";
        let mut device = Recorded::new().with("show policy-map interface", output);
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        assert_eq!(snapshot.problems, vec![]);
        let classes = snapshot.qos.as_ref().unwrap();
        assert_eq!(
            classes
                .iter()
                .map(|class| (
                    class.class.as_str(),
                    class.rate_bps,
                    class.queue_dropped_packets,
                    class.dropped_packets
                ))
                .collect::<Vec<_>>(),
            vec![
                ("VOICE", Some(5_000_000), Some(3), Some(3)),
                // No rate was printed, so none is claimed.
                ("SIGNAL", None, Some(7), Some(7)),
            ]
        );
    }

    #[tokio::test]
    async fn a_qos_row_that_does_not_say_which_way_is_dropped_and_counted() {
        const DIRECTIONLESS: &str = r#"
id: qos_os
name: QoS OS
policy: {read_only: ['^show\b']}
collect:
  qos:
    - command: show queues
      parser: {regex: '(?m)^(?P<name>\S+) (?P<way>\S+) (?P<packets>\d+)$'}
      map: {class: name, direction: way, packets: packets}
"#;
        let mut registry = Registry::empty();
        let platform = registry.add(DIRECTIONLESS, Origin::User).unwrap();
        let collector = Collector::new(registry.expressions());
        let mut device = Recorded::new().with(
            "show queues",
            "voice out 10\nvideo sideways 20\nbulk input 30\n",
        );
        let snapshot = collector
            .collect(&mut device, &platform, &[Kind::Qos])
            .await;
        let classes = snapshot.qos.as_ref().expect("the other rows are kept");
        assert_eq!(
            classes
                .iter()
                .map(|class| (class.class.as_str(), class.direction, class.packets))
                .collect::<Vec<_>>(),
            vec![
                ("voice", crate::model::Direction::Out, Some(10)),
                ("bulk", crate::model::Direction::In, Some(30)),
            ]
        );
        assert_eq!(codes(&snapshot, Kind::Qos), vec![ProblemCode::Partial]);
        let message = &snapshot.problems[0].message;
        assert!(
            message.contains("1 of 3 rows did not fit the model") && message.contains("sideways"),
            "{message}"
        );
    }

    #[test]
    fn verbatim_interface_names_are_left_alone() {
        let names = Names::for_kind(Kind::Interfaces, InterfaceNames::Verbatim);
        let mut row = json!({"name": "vlan10"}).as_object().cloned().unwrap();
        names.canonicalise(&mut row);
        assert_eq!(row["name"], "vlan10");
        assert_ne!(
            names.match_key("name", &json!("vlan10")),
            names.match_key("name", &json!("Vlan10"))
        );

        let names = Names::for_kind(Kind::Vlans, InterfaceNames::Cisco);
        let mut row = json!({"name": "Gi0/1", "interfaces": ["Gi0/1", "Fa0/2"]})
            .as_object()
            .cloned()
            .unwrap();
        names.canonicalise(&mut row);
        assert_eq!(
            Value::Object(row),
            json!({"name": "Gi0/1", "interfaces": ["GigabitEthernet0/1", "FastEthernet0/2"]}),
            "a VLAN's own name is not an interface name"
        );
        let names = Names::for_kind(Kind::Interfaces, InterfaceNames::Cisco);
        assert_eq!(
            names.match_key("name", &json!("gi 0/1")),
            names.match_key("name", &json!("GigabitEthernet0/1"))
        );
    }
}
