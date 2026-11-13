//! Configuration templates.
//!
//! A `config` template renders to configuration lines and hands them to
//! `changes.create`; it never sends anything. A `commands` template renders to
//! read-only commands and becomes a runner run. The difference is declared on
//! the template, not decided at apply time.

use std::sync::Arc;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::model::Device;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::{
    DeviceResult, DeviceState, Selector, Template, TemplateKind, Variable, VariableKind,
};
use crate::runner::{DeviceWork, Fanout};
use crate::settings::Settings;
use crate::{builtins, db, render, runner, select};

/// Built-ins first, then saved ones, both by name.
pub fn list(app: &App) -> AppResult<Vec<Template>> {
    let mut stored = db::stored_templates(app)?;
    stored.sort_by_key(|template| template.name.to_lowercase());
    let mut all = builtins::all();
    all.sort_by_key(|template| template.name.to_lowercase());
    all.extend(stored);
    Ok(all)
}

pub fn get(app: &App, id: &str) -> AppResult<Template> {
    if let Some(template) = builtins::get(id) {
        return Ok(template);
    }
    db::stored_template(app, id)?.ok_or_else(|| AppError::not_found(format!("template `{id}`")))
}

fn editable(id: &str) -> AppResult<()> {
    if builtins::get(id).is_some() {
        return Err(AppError::forbidden(format!(
            "`{id}` is a built-in template and cannot be changed or deleted; duplicate it and \
             edit the copy"
        )));
    }
    Ok(())
}

// ------------------------------------------------------ variable validation

/// Check and normalise the variables a template was given.
///
/// Returns the values to render with: declared variables first (with their
/// defaults filled in), then anything else the caller passed, so a template
/// can still be handed an undeclared value on purpose.
pub fn bind(variables: &[Variable], vars: &Value) -> AppResult<Map<String, Value>> {
    let given = match vars {
        Value::Null => Map::new(),
        Value::Object(object) => object.clone(),
        _ => return Err(AppError::invalid("`vars` must be a JSON object")),
    };
    let mut bound = Map::new();
    for variable in variables {
        let value = match given.get(&variable.name) {
            Some(value) if !is_blank(value) => value.clone(),
            _ => match &variable.default {
                Some(default) => default.clone(),
                None if variable.required => {
                    return Err(AppError::invalid(format!(
                        "`{}` is required",
                        variable.title()
                    )));
                }
                None => Value::Null,
            },
        };
        if variable.required && is_blank(&value) {
            return Err(AppError::invalid(format!(
                "`{}` is required",
                variable.title()
            )));
        }
        bound.insert(variable.name.clone(), coerce(variable, value)?);
    }
    for (key, value) in given {
        bound.entry(key).or_insert(value);
    }
    Ok(bound)
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

fn coerce(variable: &Variable, value: Value) -> AppResult<Value> {
    let label = variable.title();
    let bad = |what: &str| AppError::invalid(format!("`{label}` {what}"));
    // An unset optional variable stays unset; strict rendering catches a
    // template that uses it anyway.
    if is_blank(&value) && !variable.required {
        return Ok(match variable.kind {
            VariableKind::List => Value::Array(Vec::new()),
            VariableKind::Bool => Value::Bool(false),
            _ => value,
        });
    }
    match variable.kind {
        VariableKind::String | VariableKind::Choice | VariableKind::Interface => {
            let text = match &value {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string(),
            };
            if variable.kind == VariableKind::Interface && text.is_empty() {
                return Err(bad("must name an interface"));
            }
            if variable.kind == VariableKind::Choice
                && !variable
                    .choices
                    .iter()
                    .any(|choice| choice.eq_ignore_ascii_case(&text))
            {
                return Err(bad(&format!(
                    "must be one of {}",
                    variable.choices.join(", ")
                )));
            }
            Ok(Value::String(text))
        }
        VariableKind::Int => as_i64(&value)
            .map(|number| json!(number))
            .ok_or_else(|| bad("must be a whole number")),
        VariableKind::Vlan => match as_i64(&value) {
            Some(id) if (1..=4094).contains(&id) => Ok(json!(id)),
            _ => Err(bad("must be a VLAN id between 1 and 4094")),
        },
        VariableKind::Bool => match &value {
            Value::Bool(flag) => Ok(json!(flag)),
            Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Ok(json!(true)),
                "false" | "no" | "0" => Ok(json!(false)),
                _ => Err(bad("must be true or false")),
            },
            _ => Err(bad("must be true or false")),
        },
        VariableKind::Ip => {
            let text = value.as_str().unwrap_or_default().trim().to_string();
            text.parse::<std::net::IpAddr>()
                .map(|_| Value::String(text.clone()))
                .map_err(|_| bad("must be an IP address"))
        }
        VariableKind::Cidr => {
            let text = value.as_str().unwrap_or_default().trim().to_string();
            match text.split_once('/') {
                Some((address, length)) => {
                    let address = address.parse::<std::net::IpAddr>().ok();
                    let length = length.parse::<u32>().ok();
                    let limit = match address {
                        Some(std::net::IpAddr::V4(_)) => 32,
                        Some(std::net::IpAddr::V6(_)) => 128,
                        None => return Err(bad("must be an address with a prefix length")),
                    };
                    match length {
                        Some(length) if length <= limit => Ok(Value::String(text)),
                        _ => Err(bad("must be an address with a prefix length")),
                    }
                }
                None => Err(bad(
                    "must be an address with a prefix length, e.g. 10.0.0.0/24",
                )),
            }
        }
        VariableKind::List => match value {
            Value::Array(items) => Ok(Value::Array(items)),
            // A comma- or space-separated string is what a text box gives.
            Value::String(text) => Ok(Value::Array(
                text.split([',', ' ', ';'])
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| Value::String(item.to_string()))
                    .collect(),
            )),
            other => Ok(Value::Array(vec![other])),
        },
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

// ------------------------------------------------------------------ render

/// One device's preview.
#[derive(Debug, Serialize)]
pub struct Rendered {
    pub device_id: String,
    pub name: String,
    pub ok: bool,
    pub lines: Option<Vec<String>>,
    pub error: Option<String>,
}

/// Render a body for every device, reporting per device rather than failing
/// the lot: one switch with nothing collected must not hide the other forty.
pub fn render_for(
    app: &App,
    body: &str,
    devices: &[Device],
    vars: &Map<String, Value>,
) -> Vec<Rendered> {
    devices
        .iter()
        .map(|device| {
            let outcome = render::context(app, device, vars)
                .and_then(|context| render::render(body, &context));
            match outcome {
                Ok(lines) => Rendered {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    ok: true,
                    lines: Some(lines),
                    error: None,
                },
                Err(err) => Rendered {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    ok: false,
                    lines: None,
                    error: Some(err.message),
                },
            }
        })
        .collect()
}

/// Devices the template is not written for.
fn wrong_profile<'a>(template: &Template, devices: &'a [Device]) -> Vec<&'a Device> {
    if template.profiles.is_empty() {
        return Vec::new();
    }
    devices
        .iter()
        .filter(|device| {
            !device.profile.is_empty()
                && !template
                    .profiles
                    .iter()
                    .any(|profile| profile.eq_ignore_ascii_case(&device.profile))
        })
        .collect()
}

// --------------------------------------------------------------------- rpc

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SaveParams {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    kind: TemplateKind,
    body: String,
    #[serde(default)]
    profiles: Vec<String>,
    #[serde(default)]
    variables: Vec<Variable>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DuplicateParams {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValidateParams {
    body: String,
    #[serde(default)]
    variables: Vec<Variable>,
}

#[derive(Debug, Serialize)]
struct Validation {
    ok: bool,
    problems: Vec<render::Problem>,
    used_variables: Vec<String>,
    undeclared: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RenderParams {
    #[serde(default)]
    template_id: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    variables: Vec<Variable>,
    #[serde(flatten)]
    select: Selector,
    #[serde(default)]
    vars: Value,
}

#[derive(Debug, Deserialize)]
pub struct ApplyParams {
    pub template_id: String,
    #[serde(flatten)]
    pub select: Selector,
    #[serde(default)]
    pub vars: Value,
    #[serde(default)]
    pub title: Option<String>,
    /// Render and report, create nothing and send nothing.
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

/// `templates.apply`: a `config` template becomes one change (not applied), a
/// `commands` template becomes a run.
pub async fn apply(app: &App, params: ApplyParams) -> AppResult<Value> {
    let template = get(app, &params.template_id)?;
    let devices = select::resolve(app, &params.select)?;
    let mismatched = wrong_profile(&template, &devices);
    if !mismatched.is_empty() {
        return Err(AppError::invalid(format!(
            "`{}` is written for {} and these devices are not: {}",
            template.name,
            template.profiles.join(", "),
            mismatched
                .iter()
                .map(|device| format!("{} ({})", device.name, device.profile))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let vars = bind(&template.variables, &params.vars)?;
    let rendered = render_for(app, &template.body, &devices, &vars);
    if let Some(failed) = rendered.iter().find(|item| !item.ok) {
        return Err(AppError::invalid(format!(
            "`{}`: {}",
            failed.name,
            failed.error.clone().unwrap_or_default()
        )));
    }

    let title = params
        .title
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| template.name.clone());

    match template.kind {
        TemplateKind::Config => {
            if params.dry_run {
                return Ok(json!({
                    "dry_run": true,
                    "would_create": "one change with a `lines` intent per device",
                    "title": title,
                    "rendered": rendered,
                }));
            }
            let targets: Vec<Value> = rendered
                .iter()
                .map(|item| {
                    json!({
                        "device_id": item.device_id,
                        "intent": {"type": "lines", "lines": item.lines.clone().unwrap_or_default()},
                    })
                })
                .collect();
            let created = app
                .rpc(
                    "changes.create",
                    json!({
                        "title": title,
                        "description": format!("Rendered from the template `{}`.", template.name),
                        "origin": "template",
                        "targets": targets,
                    }),
                )
                .await
                .map_err(|err| match err.code {
                    "unknown_method" => AppError::unavailable(
                        "this build has no change pipeline, so a configuration template has \
                         nowhere to go; `templates.render` still previews the lines",
                    ),
                    _ => err,
                })?;
            let change_id = created
                .get("change_id")
                .or_else(|| created.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            app.audit(
                "templates.apply",
                None,
                format!(
                    "Created a change from the template `{}` for {} device(s)",
                    template.name,
                    devices.len()
                ),
                json!({"template_id": template.id, "change_id": change_id}),
            );
            Ok(json!({"change_id": change_id}))
        }
        TemplateKind::Commands => {
            // Every device gets the commands rendered for it, so the gate runs
            // per device rather than over one shared list.
            let mut per_device: Vec<(String, Vec<String>)> = Vec::new();
            for item in &rendered {
                let commands = item.lines.clone().unwrap_or_default();
                let device = devices
                    .iter()
                    .find(|device| device.id == item.device_id)
                    .expect("rendered device is in the selection");
                runner::check_read_only(app, std::slice::from_ref(device), &commands)?;
                per_device.push((item.device_id.clone(), commands));
            }
            let settings = Settings::load(app);
            let app_for_work = app.clone();
            let lookup = per_device.clone();
            let dry_run = params.dry_run;
            let work: DeviceWork = Arc::new(move |_ctx, device| {
                let app = app_for_work.clone();
                let commands = lookup
                    .iter()
                    .find(|(id, _)| id == &device.id)
                    .map(|(_, commands)| commands.clone())
                    .unwrap_or_default();
                let cap = settings.output_cap();
                Box::pin(async move {
                    if dry_run {
                        return DeviceResult {
                            reason: Some(format!("would run: {}", commands.join("; "))),
                            ..DeviceResult::default()
                        }
                        .with_state(DeviceState::Ok);
                    }
                    runner::run_commands(&app, &device, &commands, false, cap).await
                })
            });
            let commands = per_device
                .first()
                .map(|(_, commands)| commands.clone())
                .unwrap_or_default();
            runner::fan_out(
                app,
                Fanout {
                    kind: crate::model::RunKind::Commands,
                    name: title,
                    origin: params.origin.unwrap_or_else(|| "user".into()),
                    task_id: params.task_id,
                    commands,
                    dry_run: params.dry_run,
                    ..Fanout::default()
                },
                devices,
                work,
            )
        }
    }
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add(
        "templates.list",
        |app: App, _: nocturne_app::NoParams| async move { list(&app) },
    );

    router.add("templates.get", |app: App, params: IdParams| async move {
        get(&app, &params.id)
    });

    router.add(
        "templates.save",
        |app: App, params: SaveParams| async move {
            let name = db::require_name(&params.name, "template")?;
            if params.body.trim().is_empty() {
                return Err(AppError::invalid("a template needs a body"));
            }
            let id = match params.id {
                Some(id) if !id.trim().is_empty() => {
                    editable(id.trim())?;
                    id.trim().to_string()
                }
                _ => new_id(),
            };
            for variable in &params.variables {
                if variable.name.trim().is_empty() {
                    return Err(AppError::invalid("every variable needs a name"));
                }
                if variable.kind == VariableKind::Choice && variable.choices.is_empty() {
                    return Err(AppError::invalid(format!(
                        "`{}` is a choice but lists no choices",
                        variable.title()
                    )));
                }
            }
            let declared: Vec<String> = params
                .variables
                .iter()
                .map(|variable| variable.name.clone())
                .collect();
            let (problems, ..) = render::check(&params.body, &declared);
            if let Some(problem) = problems.first() {
                return Err(AppError::invalid(format!(
                    "the template does not compile: {}",
                    problem.message
                )));
            }
            let template = Template {
                id,
                name,
                description: params.description,
                kind: params.kind,
                body: params.body,
                builtin: false,
                profiles: params.profiles,
                variables: params.variables,
                tags: params.tags,
                updated_at: now_ms(),
            };
            app.store().put(db::TEMPLATES, &template.id, &template)?;
            app.audit(
                "templates.save",
                None,
                format!("Saved the template `{}`", template.name),
                json!({"template_id": template.id}),
            );
            app.publish("templates.changed", &json!({"id": template.id}));
            Ok(template)
        },
    );

    router.add(
        "templates.delete",
        |app: App, params: IdParams| async move {
            editable(&params.id)?;
            let template = get(&app, &params.id)?;
            app.store().delete(db::TEMPLATES, &params.id)?;
            app.audit(
                "templates.delete",
                None,
                format!("Deleted the template `{}`", template.name),
                json!({"template_id": params.id}),
            );
            app.publish("templates.changed", &json!({"id": params.id}));
            Ok(json!({"deleted": true}))
        },
    );

    router.add(
        "templates.duplicate",
        |app: App, params: DuplicateParams| async move {
            let source = get(&app, &params.id)?;
            let copy = Template {
                id: new_id(),
                name: params
                    .name
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("{} (copy)", source.name)),
                builtin: false,
                updated_at: now_ms(),
                ..source
            };
            app.store().put(db::TEMPLATES, &copy.id, &copy)?;
            app.audit(
                "templates.duplicate",
                None,
                format!("Duplicated the template `{}`", copy.name),
                json!({"template_id": copy.id, "from": params.id}),
            );
            app.publish("templates.changed", &json!({"id": copy.id}));
            Ok(copy)
        },
    );

    router.add(
        "templates.validate",
        |_app: App, params: ValidateParams| async move {
            let declared: Vec<String> = params
                .variables
                .iter()
                .map(|variable| variable.name.clone())
                .collect();
            let (problems, used, undeclared) = render::check(&params.body, &declared);
            Ok(Validation {
                ok: problems.is_empty(),
                problems,
                used_variables: used,
                undeclared,
            })
        },
    );

    router.add(
        "templates.render",
        |app: App, params: RenderParams| async move {
            let (body, variables) = match (&params.template_id, &params.body) {
                (Some(id), _) if !id.trim().is_empty() => {
                    let template = get(&app, id.trim())?;
                    (template.body, template.variables)
                }
                (_, Some(body)) if !body.trim().is_empty() => {
                    (body.clone(), params.variables.clone())
                }
                _ => {
                    return Err(AppError::invalid(
                        "give a `template_id` or a `body` to render",
                    ));
                }
            };
            let devices = select::resolve(&app, &params.select)?;
            let vars = bind(&variables, &params.vars)?;
            Ok(render_for(&app, &body, &devices, &vars))
        },
    );

    router.add(
        "templates.apply",
        |app: App, params: ApplyParams| async move { apply(&app, params).await },
    );
}
