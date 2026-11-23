//! Rendering a template to lines.
//!
//! MiniJinja with **strict undefined**: a name the template uses but the
//! context does not have is an error that says which name, not an empty
//! string that quietly produces `ntp server` with no address.
//!
//! The filters a network engineer expects (`ifname`, `cidr`, `mac`, …) are the
//! ones device profiles use, so a template and a profile mapping read the same
//! way. They are re-registered here rather than borrowed: the drivers crate's
//! `Expressions` owns its environment privately and renders with *chainable*
//! undefined, which is right for parsing a device's output and wrong for
//! writing configuration.
//!
//! A template can also refuse what it was given: `{{ fail("…") }}` stops the
//! rendering with that message. A variable's kind says what one value is; it
//! cannot say that three percentages must leave something over, or that a
//! guaranteed rate may not be above its limit — and lines like those are the
//! dangerous ones, because a device accepts them and then queues wrongly.
//!
//! And a rendering is bounded ([`DEVICE_BYTES`], [`TOTAL_BYTES`],
//! [`DEVICE_FUEL`]). A template is text somebody writes, run over whatever the
//! inventory holds: one loop over ten thousand interfaces with a megabyte in
//! it is ten gigabytes, and a process that runs out of memory takes the
//! schedule that started it down with it, on every start.

use std::collections::BTreeMap;
use std::io;

use minijinja::value::{Serde, Value as JValue};
use minijinja::{Environment, ErrorKind, UndefinedBehavior};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::model::{Device, DeviceState as Known};
use nocturne_app::{App, collect, inventory};
use nocturne_drivers::expr;
use nocturne_drivers::model::Kind;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Map, Value, json};

/// Collected data a template may ask for, and the kind to collect when it is missing.
const COLLECTED: [(&str, Kind); 4] = [
    ("facts", Kind::Facts),
    ("interfaces", Kind::Interfaces),
    ("neighbors", Kind::Neighbors),
    ("vlans", Kind::Vlans),
];

fn text(value: &JValue) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

/// Marks an error as the template's own `fail("…")`, so that its message is
/// passed on as written instead of being reported as a broken template.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the template refused its variables")
    }
}

impl std::error::Error for Refused {}

fn refused(err: &minijinja::Error) -> bool {
    std::error::Error::source(err).is_some_and(|source| source.is::<Refused>())
}

/// The longest refusal that is passed on, in characters.
const REFUSAL_LIMIT: usize = 400;

/// What `fail("…")` was given, as one line of bounded length.
///
/// A template usually quotes the value it refuses, and a value is whatever
/// the caller sent: it can be a megabyte long and hold line breaks or escape
/// sequences. The message goes to a screen, into an error and possibly a log
/// line, once per device, so it is cut and its control characters become
/// spaces. A message that fits is passed on exactly as written.
fn refusal(message: &str) -> String {
    let mut text: String = message
        .chars()
        .take(REFUSAL_LIMIT)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if message.chars().nth(REFUSAL_LIMIT).is_some() {
        text.push('…');
    }
    text
}

/// Is `name` something the environment itself provides (`fail`, `range`,
/// `namespace`…)? MiniJinja's static analysis lists those among the names a
/// template reads, and they are neither variables nor missing.
fn provided(env: &Environment<'_>, name: &str) -> bool {
    env.globals().any(|(global, _)| global == name)
}

/// A strict environment with the networking filters.
pub fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_filter("duration", |value: JValue| {
        expr::parse_duration(&text(&value))
            .map(JValue::from)
            .unwrap_or_default()
    });
    env.add_filter("bps", |value: JValue| {
        expr::parse_bps(&text(&value))
            .map(JValue::from)
            .unwrap_or_default()
    });
    env.add_filter("bytes", |value: JValue| {
        expr::parse_bytes(&text(&value))
            .map(JValue::from)
            .unwrap_or_default()
    });
    env.add_filter("mac", |value: JValue| expr::normalize_mac(&text(&value)));
    env.add_filter("ifname", |value: JValue| {
        nocturne_drivers::ifname::long(&text(&value))
    });
    env.add_filter("cidr", |value: JValue, mask: String| {
        expr::to_cidr(&text(&value), &mask)
    });
    env.add_filter("regex_search", |value: JValue, pattern: String| {
        Regex::new(&pattern)
            .ok()
            .and_then(|regex| {
                let haystack = text(&value);
                regex.captures(&haystack).map(|caps| {
                    caps.get(1)
                        .or_else(|| caps.get(0))
                        .map(|found| found.as_str().to_string())
                        .unwrap_or_default()
                })
            })
            .map(JValue::from)
            .unwrap_or_default()
    });
    env.add_filter(
        "regex_replace",
        |value: JValue, pattern: String, replacement: String| match Regex::new(&pattern) {
            Ok(regex) => regex
                .replace_all(&text(&value), replacement.as_str())
                .into(),
            Err(_) => text(&value),
        },
    );
    // `10.0.0.0/24` → `0.0.0.255`, for the platforms whose ACLs take a
    // wildcard mask rather than a prefix length.
    env.add_filter("wildcard", |value: JValue| wildcard(&text(&value)));
    env.add_test("matching", |value: JValue, pattern: String| {
        Regex::new(&pattern).is_ok_and(|regex| regex.is_match(&text(&value)))
    });
    env.add_function(
        "fail",
        |message: String| -> Result<JValue, minijinja::Error> {
            Err(
                minijinja::Error::new(ErrorKind::InvalidOperation, refusal(&message))
                    .with_source(Refused),
            )
        },
    );
    env
}

/// The IOS-style wildcard mask of a CIDR prefix, or the input when it is not one.
fn wildcard(prefix: &str) -> String {
    let Some((_, length)) = prefix.split_once('/') else {
        return prefix.to_string();
    };
    let Ok(length) = length.trim().parse::<u32>() else {
        return prefix.to_string();
    };
    if length > 32 {
        return prefix.to_string();
    }
    let mask: u32 = if length == 0 {
        u32::MAX
    } else {
        u32::MAX >> length
    };
    std::net::Ipv4Addr::from(mask).to_string()
}

/// Everything a template may see about one device.
#[derive(Clone, Debug)]
pub struct Context {
    pub value: Value,
    /// Collected kinds this device has nothing stored for.
    pub missing: Vec<&'static str>,
    pub device_name: String,
}

impl Context {
    pub fn top_level_names(&self) -> Vec<String> {
        self.value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// The device type a template is rendered for: the one the device is set to,
/// or else the one detection found when it was first reached. `None` while
/// neither has said — which is not the same as "any".
pub fn profile_of(device: &Device, state: &Known) -> Option<String> {
    if !device.profile.is_empty() {
        return Some(device.profile.clone());
    }
    state
        .detected_profile
        .clone()
        .filter(|profile| !profile.is_empty())
}

/// Build the render context for one device: what it is, what was collected
/// from it, and the variables the caller supplied.
pub fn context(app: &App, device: &Device, vars: &Map<String, Value>) -> AppResult<Context> {
    let state = inventory::state_of(app, &device.id)?;
    let mut object = Map::new();
    object.insert(
        "device".into(),
        json!({
            "id": device.id,
            "name": device.name,
            "address": device.address,
            "site": device.site,
            "role": device.role,
            "tags": device.tags,
            "profile": profile_of(device, &state).unwrap_or_default(),
            "notes": device.notes,
            "protected": device.protected,
            "is_lab": device.lab.is_some(),
        }),
    );

    let mut missing = Vec::new();
    for (name, kind) in COLLECTED {
        match collect::latest(app, &device.id, kind)? {
            Some(stored) => {
                let value = if kind == Kind::Facts {
                    stored.rows.into_iter().next().unwrap_or(Value::Null)
                } else {
                    Value::Array(stored.rows)
                };
                object.insert(name.to_string(), value);
            }
            // `facts` may also be on the device record even when nothing was
            // collected through a session (monitoring writes it).
            None if kind == Kind::Facts && state.facts.is_some() => {
                object.insert(name.to_string(), serde_json::to_value(&state.facts)?);
            }
            None => missing.push(name),
        }
    }

    object.insert("vars".into(), Value::Object(vars.clone()));
    for (key, value) in vars {
        object.insert(key.clone(), value.clone());
    }
    Ok(Context {
        value: Value::Object(object),
        missing,
        device_name: device.name.clone(),
    })
}

/// A problem with a template, located where it can be.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Problem {
    pub line: Option<usize>,
    pub message: String,
}

/// The names a template reads, as MiniJinja sees them.
pub fn used_variables(body: &str) -> Result<Vec<String>, Problem> {
    let env = environment();
    let template = env.template_from_str(body).map_err(problem)?;
    let mut names: Vec<String> = template
        .undeclared_variables(false)
        .into_iter()
        .filter(|name| !provided(&env, name))
        .collect();
    names.sort();
    Ok(names)
}

fn problem(err: minijinja::Error) -> Problem {
    Problem {
        line: err.line(),
        message: match err.detail() {
            Some(detail) => format!("{}: {detail}", err.kind()),
            None => err.kind().to_string(),
        },
    }
}

// ------------------------------------------------- what a rendering may do

/// The most one rendering writes for one device: 1 MiB. A device's whole
/// configuration is a fraction of that; a template that goes past it is a
/// loop that runs too often or a value that is far too long, and what it
/// wrote is not kept.
pub const DEVICE_BYTES: usize = 1 << 20;

/// The most one call renders for all its devices together: 16 MiB, which is
/// also the most the server takes in one request, so a preview always fits
/// back into one. Devices past it are not rendered, and say so.
pub const TOTAL_BYTES: usize = 16 << 20;

/// The work one rendering may do for one device, in MiniJinja's fuel: a step
/// is one instruction, or 64 bytes of text the template makes on the way,
/// kept or not. It bounds how long a rendering takes and what it can hold in
/// memory while it runs (about 32 MB).
pub const DEVICE_FUEL: u64 = 500_000;

/// What the devices of one call may still render between them.
///
/// The devices of a playbook run render at the same time, so each takes its
/// share before it starts and gives back what it did not write: two of them
/// cannot both spend the last megabyte.
#[derive(Debug)]
pub struct Budget {
    bytes: Mutex<usize>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new()
    }
}

impl Budget {
    pub fn new() -> Budget {
        Budget {
            bytes: Mutex::new(TOTAL_BYTES),
        }
    }

    fn take(&self) -> usize {
        let mut left = self.bytes.lock();
        let share = (*left).min(DEVICE_BYTES);
        *left -= share;
        share
    }

    fn give_back(&self, bytes: usize) {
        *self.bytes.lock() += bytes;
    }
}

/// What one rendering took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Bytes written, before blank lines are dropped.
    pub bytes: usize,
    /// Steps of work, as [`DEVICE_FUEL`] counts them.
    pub fuel: u64,
}

/// Where a rendering writes. The write that would take it past its share is
/// refused, which stops the rendering there: the text is never built whole.
struct Capped {
    text: Vec<u8>,
    share: usize,
    over: bool,
}

impl io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.text.len() + buf.len() > self.share {
            self.over = true;
            return Err(io::Error::other("the rendering is over its share"));
        }
        self.text.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn too_long() -> AppError {
    AppError::invalid(
        "the template renders more than 1 MiB for this one device, which no device's \
         configuration comes near; nothing of it was kept. Look for a loop that runs too \
         often, or a value that is far too long",
    )
}

fn too_much_altogether() -> AppError {
    AppError::invalid(
        "not rendered: with the devices before it, this would come to more than 16 MiB, the \
         most one call renders for all its devices together. Render fewer devices at a time",
    )
}

fn too_much_work() -> AppError {
    AppError::invalid(format!(
        "the template did more than {DEVICE_FUEL} steps of work for this one device and was \
         stopped; nothing of it was kept. Look for a loop that runs far too often"
    ))
}

/// Render a template body for one device and return the configuration (or
/// command) lines: trailing whitespace trimmed, blank lines dropped.
pub fn render(body: &str, context: &Context, budget: &Budget) -> AppResult<Vec<String>> {
    render_with_usage(body, context, budget).map(|(lines, _)| lines)
}

/// [`render`], saying what it took.
pub fn render_with_usage(
    body: &str,
    context: &Context,
    budget: &Budget,
) -> AppResult<(Vec<String>, Usage)> {
    let mut env = environment();
    env.set_fuel(Some(DEVICE_FUEL));
    env.add_template_owned("template", body.to_string())
        .map_err(|err| template_error(err, context))?;
    let template = env
        .get_template("template")
        .map_err(|err| template_error(err, context))?;

    // Strict undefined does report an error, but — without MiniJinja's `debug`
    // feature, which the workspace does not enable — it cannot say which name
    // caused it. Checking the names the template reads against the context
    // first is what lets the message be useful.
    let available: Vec<String> = context.top_level_names();
    let mut unknown: Vec<String> = template
        .undeclared_variables(false)
        .into_iter()
        .filter(|name| !available.contains(name) && !provided(&env, name))
        .collect();
    unknown.sort();
    if let Some(name) = unknown.first() {
        return Err(missing_name(name, context));
    }

    let share = budget.take();
    let mut out = Capped {
        text: Vec::new(),
        share,
        over: false,
    };
    let (fuel, refused) =
        match template.render_captured_to(JValue::from(Serde(&context.value)), &mut out) {
            Ok(captured) => {
                let used = captured.state().fuel_levels().map(|(used, _)| used);
                (used.unwrap_or_default(), None)
            }
            Err(err) => (0, Some(err)),
        };
    let written = if refused.is_some() { 0 } else { out.text.len() };
    budget.give_back(share - written);
    if let Some(err) = refused {
        if out.over {
            // Short of a whole device's share means the call's total ran out.
            return Err(if share < DEVICE_BYTES {
                too_much_altogether()
            } else {
                too_long()
            });
        }
        if err.kind() == ErrorKind::OutOfFuel {
            return Err(too_much_work());
        }
        return Err(template_error(err, context));
    }
    // MiniJinja writes whole `str`s and a write is taken whole or refused, so
    // the bytes kept are always UTF-8.
    let rendered = String::from_utf8(out.text)
        .map_err(|_| AppError::internal("a rendering came out as something other than text"))?;
    Ok((
        rendered
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Usage {
            bytes: written,
            fuel,
        },
    ))
}

/// "collect interfaces for SW1 first" rather than "undefined value".
fn missing_name(name: &str, context: &Context) -> AppError {
    if context.missing.contains(&name) {
        return AppError::invalid(format!(
            "this template needs the {name} of `{}`, which have never been collected — \
             collect {name} for `{}` first",
            context.device_name, context.device_name
        ));
    }
    AppError::invalid(format!(
        "the template uses `{name}`, which is not one of its variables and not part of \
         the render context (device, facts, interfaces, neighbors, vlans, vars)"
    ))
}

fn template_error(err: minijinja::Error, context: &Context) -> AppError {
    if refused(&err) {
        // The template's own words: they name the variable and say what is
        // wrong with it, which "could not be rendered at line 7" does not.
        return AppError::invalid(
            err.detail()
                .unwrap_or("the template refused its variables")
                .to_string(),
        );
    }
    let problem = problem(err);
    let where_ = problem
        .line
        .map(|line| format!(" at line {line}"))
        .unwrap_or_default();
    AppError::invalid(format!(
        "`{}`: the template could not be rendered{where_}: {}",
        context.device_name, problem.message
    ))
}

/// Check a body without a device: syntax, and the names it reads.
pub fn check(body: &str, declared: &[String]) -> (Vec<Problem>, Vec<String>, Vec<String>) {
    let mut problems = Vec::new();
    let used = match used_variables(body) {
        Ok(used) => used,
        Err(problem) => {
            problems.push(problem);
            return (problems, Vec::new(), Vec::new());
        }
    };
    let builtin: BTreeMap<&str, ()> = [
        "device",
        "facts",
        "interfaces",
        "neighbors",
        "vlans",
        "vars",
    ]
    .into_iter()
    .map(|name| (name, ()))
    .collect();
    let undeclared: Vec<String> = used
        .iter()
        .filter(|name| !builtin.contains_key(name.as_str()) && !declared.contains(name))
        .cloned()
        .collect();
    for name in &undeclared {
        problems.push(Problem {
            line: None,
            message: format!(
                "`{name}` is used but not declared as a variable, and is not part of the \
                 render context — rendering will fail with it undefined"
            ),
        });
    }
    (problems, used, undeclared)
}
