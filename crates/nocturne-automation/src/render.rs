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

use std::collections::BTreeMap;

use minijinja::value::{Serde, Value as JValue};
use minijinja::{Environment, UndefinedBehavior};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::model::Device;
use nocturne_app::{App, collect, inventory};
use nocturne_drivers::expr;
use nocturne_drivers::model::Kind;
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
            "profile": if device.profile.is_empty() {
                state.detected_profile.clone().unwrap_or_default()
            } else {
                device.profile.clone()
            },
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
    let mut names: Vec<String> = template.undeclared_variables(false).into_iter().collect();
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

/// Render a template body for one device and return the configuration (or
/// command) lines: trailing whitespace trimmed, blank lines dropped.
pub fn render(body: &str, context: &Context) -> AppResult<Vec<String>> {
    let mut env = environment();
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
        .filter(|name| !available.contains(name))
        .collect();
    unknown.sort();
    if let Some(name) = unknown.first() {
        return Err(missing_name(name, context));
    }

    let rendered = template
        .render(JValue::from(Serde(&context.value)))
        .map_err(|err| template_error(err, context))?;
    Ok(rendered
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
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
