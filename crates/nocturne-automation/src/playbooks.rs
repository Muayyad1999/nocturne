//! Playbooks: a list of steps run per device, in Nocturne's own YAML.
//!
//! Deliberately close to what an Ansible user expects to read, and
//! deliberately not Ansible: the steps are the things this application can do
//! safely — read, collect, assert, back up, render a template — and a step
//! that would change configuration does not send it. Template lines are
//! accumulated per device and become **one change at the end of the run**,
//! which the change pipeline then plans, risks, and asks a human about.
//!
//! A failed step stops that device. Everything else carries on: forty switches
//! must not be left half-done because the forty-first is unreachable.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::connect::{self, Cli, CliOptions};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs::{self, JobCtx};
use nocturne_app::model::Device;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, collect, profiles};
use nocturne_drivers::model::Kind;
use nocturne_drivers::policy::CommandClass;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::{
    CommandResult, DeviceResult, DeviceState, Playbook, RunKind, Selector, StepResult,
};
use crate::runner::{AfterRun, DeviceWork, Fanout};
use crate::settings::Settings;
use crate::{db, render, runner, select, templates};

// ------------------------------------------------------------------- model

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub vars: Map<String, Value>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub ignore_errors: bool,
    #[serde(default)]
    pub register: Option<String>,
    #[serde(default)]
    pub collect: Option<Vec<String>>,
    #[serde(default)]
    pub commands: Option<Vec<String>>,
    #[serde(default)]
    pub assert: Option<Assert>,
    #[serde(default)]
    pub backup: Option<bool>,
    #[serde(default)]
    pub template: Option<TemplateStep>,
    #[serde(default)]
    pub set: Option<Map<String, Value>>,
    #[serde(default)]
    pub pause: Option<Pause>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assert {
    pub that: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateStep {
    pub id: String,
    #[serde(default)]
    pub vars: Value,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
pub enum Pause {
    Seconds(u64),
    Spec {
        #[serde(default)]
        seconds: u64,
    },
}

impl Pause {
    fn seconds(self) -> u64 {
        match self {
            Pause::Seconds(seconds) => seconds,
            Pause::Spec { seconds } => seconds,
        }
        .min(3600)
    }
}

impl Step {
    /// `collect`, `commands`, … — the one thing this step does.
    fn kind(&self) -> Option<&'static str> {
        let kinds = [
            ("collect", self.collect.is_some()),
            ("commands", self.commands.is_some()),
            ("assert", self.assert.is_some()),
            ("backup", self.backup.is_some()),
            ("template", self.template.is_some()),
            ("set", self.set.is_some()),
            ("pause", self.pause.is_some()),
        ];
        let mut found = None;
        for (name, present) in kinds {
            if present {
                if found.is_some() {
                    return None;
                }
                found = Some(name);
            }
        }
        found
    }

    fn title(&self, index: usize) -> String {
        self.name.clone().unwrap_or_else(|| {
            format!(
                "step {} ({})",
                index + 1,
                self.kind().unwrap_or("nothing to do")
            )
        })
    }
}

pub fn parse(yaml: &str) -> AppResult<Source> {
    serde_yaml_ng::from_str::<Source>(yaml)
        .map_err(|err| AppError::invalid(format!("this is not a playbook: {err}")))
}

// -------------------------------------------------------------- validation

#[derive(Debug, Serialize)]
pub struct Problem {
    /// 1-based step number, when the problem is in a step.
    pub step: Option<usize>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Validation {
    pub ok: bool,
    pub problems: Vec<Problem>,
    pub steps: usize,
}

/// `true` when at least one device type in this build would class the command
/// as a read. The per-device gate still decides at run time; this only catches
/// the obvious `reload` in a playbook before anybody runs it.
fn read_only_somewhere(app: &App, command: &str) -> bool {
    profiles::registry(app)
        .platforms()
        .any(|platform| platform.policy().classify(command).class == CommandClass::ReadOnly)
}

pub fn validate(app: &App, yaml: &str) -> Validation {
    let mut problems = Vec::new();
    let source = match parse(yaml) {
        Ok(source) => source,
        Err(err) => {
            return Validation {
                ok: false,
                problems: vec![Problem {
                    step: None,
                    message: err.message,
                }],
                steps: 0,
            };
        }
    };
    if source.steps.is_empty() {
        problems.push(Problem {
            step: None,
            message: "a playbook needs at least one step".into(),
        });
    }
    let expressions = profiles::expressions(app);
    for (index, step) in source.steps.iter().enumerate() {
        let at = Some(index + 1);
        let problem = |message: String| Problem { step: at, message };
        match step.kind() {
            None => problems.push(problem(
                "a step does exactly one thing: collect, commands, assert, backup, template, \
                 set or pause"
                    .into(),
            )),
            Some("collect") => {
                for name in step.collect.clone().unwrap_or_default() {
                    if kind_from_name(&name).is_none() {
                        problems.push(problem(format!(
                            "`{name}` is not a kind of data to collect"
                        )));
                    }
                }
            }
            Some("commands") => {
                for command in step.commands.clone().unwrap_or_default() {
                    if command.trim().is_empty() {
                        problems.push(problem("a command is empty".into()));
                    } else if !read_only_somewhere(app, command.trim()) {
                        problems.push(problem(format!(
                            "`{}` is not a read-only command on any device type in this build; \
                             a playbook cannot send it. Use a `template` step, which becomes a \
                             change.",
                            command.trim()
                        )));
                    }
                }
            }
            Some("assert") => {
                if let Some(assert) = &step.assert
                    && let Err(err) = expressions.check(&assert.that)
                {
                    problems.push(problem(format!(
                        "`{}` does not compile: {err}",
                        assert.that
                    )));
                }
            }
            Some("template") => {
                if let Some(wanted) = &step.template
                    && templates::get(app, &wanted.id).is_err()
                {
                    problems.push(problem(format!("there is no template `{}`", wanted.id)));
                }
            }
            Some("set") => {
                for (name, expression) in step.set.clone().unwrap_or_default() {
                    if let Some(expression) = expression.as_str()
                        && let Err(err) = expressions.check(expression)
                    {
                        problems.push(problem(format!("`{name}`: {err}")));
                    }
                }
            }
            _ => {}
        }
        if let Some(when) = &step.when
            && let Err(err) = expressions.check(when)
        {
            problems.push(problem(format!("`when: {when}` does not compile: {err}")));
        }
    }
    Validation {
        ok: problems.is_empty(),
        problems,
        steps: source.steps.len(),
    }
}

fn kind_from_name(name: &str) -> Option<Kind> {
    Kind::ALL
        .into_iter()
        .find(|kind| kind.as_str().eq_ignore_ascii_case(name.trim()))
}

// ------------------------------------------------------------------ running

/// One device's state while its steps run.
struct Walk<'a> {
    app: &'a App,
    ctx: &'a JobCtx,
    device: Device,
    context: Map<String, Value>,
    vars: Map<String, Value>,
    cli: Option<Cli>,
    lines: Vec<String>,
    steps: Vec<StepResult>,
    cap: usize,
    dry_run: bool,
}

impl Walk<'_> {
    fn value(&self) -> Value {
        Value::Object(self.context.clone())
    }

    /// Refresh the collected parts of the context from the store.
    fn refresh(&mut self) -> AppResult<()> {
        let fresh = render::context(self.app, &self.device, &self.vars)?;
        if let Value::Object(object) = fresh.value {
            for (key, value) in object {
                self.context.insert(key, value);
            }
        }
        Ok(())
    }

    async fn session(&mut self) -> AppResult<&mut Cli> {
        if self.cli.is_none() {
            self.cli = Some(connect::cli(self.app, &self.device.id, CliOptions::default()).await?);
        }
        Ok(self.cli.as_mut().expect("just opened"))
    }

    async fn release(&mut self) {
        if let Some(cli) = self.cli.take() {
            cli.close().await;
        }
    }
}

/// What a step did. `Err` carries the message and whether the device was never
/// reached, which is a different thing from a step that failed on it.
struct StepOutcome {
    detail: String,
    outputs: Vec<CommandResult>,
    registered: Option<Value>,
}

async fn run_step(walk: &mut Walk<'_>, step: &Step) -> Result<StepOutcome, (String, bool)> {
    let fail = |message: String| (message, false);
    let unreached = |message: String| (message, true);
    let mut outputs = Vec::new();
    let mut registered = None;

    let detail = match step.kind() {
        Some("collect") => {
            let names = step.collect.clone().unwrap_or_default();
            let kinds: Vec<Kind> = names
                .iter()
                .filter_map(|name| kind_from_name(name))
                .collect();
            if walk.dry_run {
                format!("would collect {}", names.join(", "))
            } else {
                let app = walk.app.clone();
                let cli = walk.session().await.map_err(|err| unreached(err.message))?;
                let snapshot = collect::run_on(&app, cli, &kinds)
                    .await
                    .map_err(|err| fail(err.message))?;
                walk.refresh().map_err(|err| fail(err.message))?;
                let failed: Vec<String> = snapshot
                    .problems
                    .iter()
                    .map(|problem| problem.message.clone())
                    .collect();
                if failed.is_empty() {
                    format!("collected {}", names.join(", "))
                } else {
                    return Err(fail(failed.join("; ")));
                }
            }
        }
        Some("commands") => {
            let commands = step.commands.clone().unwrap_or_default();
            if walk.dry_run {
                format!("would run {}", commands.join("; "))
            } else {
                let cap = walk.cap;
                let cli = walk.session().await.map_err(|err| unreached(err.message))?;
                let mut failure = None;
                for command in &commands {
                    match cli.read(command).await {
                        Ok(output) => {
                            let text = truncate(output.output, cap);
                            if let Some(error) = &output.error {
                                failure.get_or_insert(format!("`{command}`: {error}"));
                            }
                            outputs.push(CommandResult {
                                command: command.clone(),
                                output: text,
                                error: output.error,
                                elapsed_ms: output.elapsed_ms,
                                ..CommandResult::default()
                            });
                        }
                        Err(err) => {
                            failure.get_or_insert(format!("`{command}`: {}", err.message));
                            outputs.push(CommandResult {
                                command: command.clone(),
                                error: Some(err.message),
                                ..CommandResult::default()
                            });
                            break;
                        }
                    }
                }
                registered = Some(json!(
                    outputs
                        .iter()
                        .map(|output| json!({
                            "command": output.command,
                            "output": output.output,
                            "error": output.error,
                        }))
                        .collect::<Vec<_>>()
                ));
                if let Some(failure) = failure {
                    return Err(fail(failure));
                }
                format!("{} command(s)", commands.len())
            }
        }
        Some("assert") => {
            let assert = step.assert.clone().unwrap_or_default();
            let expressions = profiles::expressions(walk.app);
            let context = walk.value();
            let holds = expressions
                .truthy(&assert.that, &context)
                .map_err(|err| fail(format!("`{}` could not be evaluated: {err}", assert.that)))?;
            if !holds {
                let message = match &assert.message {
                    Some(message) => expressions
                        .render(message, &context)
                        .unwrap_or_else(|_| message.clone()),
                    None => format!("`{}` is not true", assert.that),
                };
                return Err(fail(message));
            }
            format!("`{}` holds", assert.that)
        }
        Some("backup") if step.backup == Some(false) => "backup not asked for".to_string(),
        Some("backup") => {
            if walk.dry_run {
                "would back up the configuration".to_string()
            } else {
                // The core allows one automated session per device, so ours
                // has to go before `config.backup` opens its own.
                walk.release().await;
                let started = walk
                    .app
                    .rpc(
                        "config.backup",
                        json!({"device_ids": [walk.device.id], "origin": "automation"}),
                    )
                    .await
                    .map_err(|err| match err.code {
                        "unknown_method" => {
                            fail("this build has no configuration backup".to_string())
                        }
                        _ => fail(err.message),
                    })?;
                match started.get("job_id").and_then(Value::as_str) {
                    Some(job_id) => {
                        let job = jobs::wait(walk.app, job_id)
                            .await
                            .map_err(|err| fail(err.message))?;
                        match job.state {
                            jobs::JobState::Succeeded => "backed up".to_string(),
                            _ => {
                                return Err(fail(
                                    job.error
                                        .unwrap_or_else(|| "the backup did not finish".into()),
                                ));
                            }
                        }
                    }
                    None => "backed up".to_string(),
                }
            }
        }
        Some("template") => {
            let wanted = step.template.clone().unwrap_or_default();
            let template = templates::get(walk.app, &wanted.id).map_err(|err| fail(err.message))?;
            let mut vars = walk.vars.clone();
            if let Value::Object(given) = &wanted.vars {
                for (key, value) in given {
                    vars.insert(key.clone(), value.clone());
                }
            }
            let bound = templates::bind(&template.variables, &Value::Object(vars))
                .map_err(|err| fail(err.message))?;
            let context =
                render::context(walk.app, &walk.device, &bound).map_err(|err| fail(err.message))?;
            let lines =
                render::render(&template.body, &context).map_err(|err| fail(err.message))?;
            let count = lines.len();
            walk.lines.extend(lines);
            format!("{count} configuration line(s) for the change at the end of the run")
        }
        Some("set") => {
            let expressions = profiles::expressions(walk.app);
            let context = walk.value();
            let assignments = step.set.clone().unwrap_or_default();
            let mut named = Vec::new();
            for (name, expression) in assignments {
                let value = match expression.as_str() {
                    Some(text) => expressions
                        .eval(text, &context)
                        .map_err(|err| fail(format!("`{name}`: {err}")))?,
                    None => expression,
                };
                walk.vars.insert(name.clone(), value.clone());
                walk.context.insert(name.clone(), value);
                named.push(name);
            }
            walk.context
                .insert("vars".into(), Value::Object(walk.vars.clone()));
            format!("set {}", named.join(", "))
        }
        Some("pause") => {
            let seconds = step.pause.map(Pause::seconds).unwrap_or_default();
            if walk.dry_run {
                format!("would wait {seconds}s")
            } else {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
                    _ = walk.ctx.token().cancelled() => return Err(fail("the run was stopped".into())),
                }
                format!("waited {seconds}s")
            }
        }
        _ => return Err(fail("this step does nothing".into())),
    };

    Ok(StepOutcome {
        detail,
        outputs,
        registered,
    })
}

fn truncate(output: String, cap: usize) -> String {
    if output.len() <= cap {
        return output;
    }
    let mut cut = cap;
    while cut > 0 && !output.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… [output cut at {cap} bytes]\n", &output[..cut])
}

/// Walk one device through the playbook.
async fn run_on_device(
    app: &App,
    ctx: &JobCtx,
    device: Device,
    source: &Source,
    vars: &Map<String, Value>,
    cap: usize,
    dry_run: bool,
) -> (DeviceResult, Vec<String>) {
    let context = match render::context(app, &device, vars) {
        Ok(context) => context,
        Err(err) => {
            return (
                DeviceResult {
                    error: Some(err.message),
                    ..DeviceResult::default()
                }
                .with_state(DeviceState::Failed),
                Vec::new(),
            );
        }
    };
    let mut walk = Walk {
        app,
        ctx,
        device,
        context: context.value.as_object().cloned().unwrap_or_default(),
        vars: vars.clone(),
        cli: None,
        lines: Vec::new(),
        steps: Vec::new(),
        cap,
        dry_run,
    };

    let expressions = profiles::expressions(app);
    let mut state = DeviceState::Ok;
    let mut error = None;

    for (index, step) in source.steps.iter().enumerate() {
        let title = step.title(index);
        if ctx.is_cancelled() {
            walk.steps.push(StepResult {
                name: title,
                state: "skipped".into(),
                detail: "the run was stopped".into(),
                outputs: Vec::new(),
            });
            continue;
        }
        if let Some(when) = &step.when {
            let context = walk.value();
            match expressions.truthy(when, &context) {
                Ok(false) => {
                    walk.steps.push(StepResult {
                        name: title,
                        state: "skipped".into(),
                        detail: format!("`{when}` is not true here"),
                        outputs: Vec::new(),
                    });
                    continue;
                }
                Err(err) => {
                    walk.steps.push(StepResult {
                        name: title,
                        state: "failed".into(),
                        detail: format!("`when: {when}` could not be evaluated: {err}"),
                        outputs: Vec::new(),
                    });
                    state = DeviceState::Failed;
                    error = Some(format!("`when: {when}` could not be evaluated: {err}"));
                    break;
                }
                Ok(true) => {}
            }
        }

        match run_step(&mut walk, step).await {
            Ok(outcome) => {
                if let (Some(name), Some(value)) = (&step.register, outcome.registered) {
                    walk.vars.insert(name.clone(), value.clone());
                    walk.context.insert(name.clone(), value);
                    walk.context
                        .insert("vars".into(), Value::Object(walk.vars.clone()));
                }
                walk.steps.push(StepResult {
                    name: title,
                    state: "ok".into(),
                    detail: outcome.detail,
                    outputs: outcome.outputs,
                });
            }
            Err((message, was_unreached)) => {
                let ignored = step.ignore_errors;
                walk.steps.push(StepResult {
                    name: title.clone(),
                    state: if ignored {
                        "ok".into()
                    } else {
                        "failed".into()
                    },
                    detail: if ignored {
                        format!("{message} (ignored)")
                    } else {
                        message.clone()
                    },
                    outputs: Vec::new(),
                });
                if !ignored {
                    state = if was_unreached {
                        DeviceState::Unreached
                    } else {
                        DeviceState::Failed
                    };
                    error = Some(format!("`{title}`: {message}"));
                    break;
                }
            }
        }
    }

    walk.release().await;
    let lines = walk.lines.clone();
    (
        DeviceResult {
            steps: walk.steps,
            error,
            ..DeviceResult::default()
        }
        .with_state(state),
        lines,
    )
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
    yaml: String,
}

#[derive(Debug, Deserialize)]
struct ValidateParams {
    yaml: String,
}

#[derive(Debug, Deserialize)]
pub struct RunParams {
    #[serde(default)]
    pub playbook_id: Option<String>,
    #[serde(default)]
    pub yaml: Option<String>,
    #[serde(flatten)]
    pub select: Selector,
    #[serde(default)]
    pub vars: Value,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RunsParams {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RunGetParams {
    run_id: String,
}

#[derive(Serialize)]
struct RunDetail {
    run: crate::model::Run,
    results: Vec<DeviceResult>,
}

pub fn start(app: &App, params: RunParams) -> AppResult<Value> {
    let (yaml, playbook_id, name) = match (&params.playbook_id, &params.yaml) {
        (Some(id), _) if !id.trim().is_empty() => {
            let playbook = db::playbook(app, id.trim())?;
            (playbook.yaml, Some(playbook.id), playbook.name)
        }
        (_, Some(yaml)) if !yaml.trim().is_empty() => {
            let source = parse(yaml)?;
            let name = if source.name.trim().is_empty() {
                "Playbook".to_string()
            } else {
                source.name.clone()
            };
            (yaml.clone(), None, name)
        }
        _ => return Err(AppError::invalid("give a `playbook_id` or a `yaml` to run")),
    };

    let checked = validate(app, &yaml);
    if !checked.ok {
        return Err(AppError::invalid(format!(
            "this playbook does not validate: {}",
            checked
                .problems
                .iter()
                .map(|problem| match problem.step {
                    Some(step) => format!("step {step}: {}", problem.message),
                    None => problem.message.clone(),
                })
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let source = parse(&yaml)?;
    let devices = select::resolve(app, &params.select)?;

    let mut vars = source.vars.clone();
    if let Value::Object(given) = &params.vars {
        for (key, value) in given {
            vars.insert(key.clone(), value.clone());
        }
    }

    let settings = Settings::load(app);
    let cap = settings.output_cap();
    let dry_run = params.dry_run;
    // Shared between the per-device work and the step that turns the lines
    // into one change once every device has had its turn.
    let accumulated: Arc<Mutex<HashMap<String, Vec<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let app_for_work = app.clone();
    let source = Arc::new(source);
    let vars = Arc::new(vars);
    let bucket = accumulated.clone();
    let work: DeviceWork = Arc::new(move |ctx, device| {
        let app = app_for_work.clone();
        let source = source.clone();
        let vars = vars.clone();
        let bucket = bucket.clone();
        Box::pin(async move {
            let device_id = device.id.clone();
            let (result, lines) =
                run_on_device(&app, &ctx, device, &source, &vars, cap, dry_run).await;
            if !lines.is_empty() {
                bucket.lock().insert(device_id, lines);
            }
            result
        })
    });

    let title = name.clone();
    let after: AfterRun = Arc::new(move |app: App, results: Vec<DeviceResult>| {
        let accumulated = accumulated.clone();
        let title = title.clone();
        Box::pin(async move {
            let lines = accumulated.lock().clone();
            if lines.is_empty() || dry_run {
                return results;
            }
            make_change(&app, &title, lines, results).await
        })
    });

    runner::fan_out_then(
        app,
        Fanout {
            kind: RunKind::Playbook,
            name,
            origin: params.origin.unwrap_or_else(|| "user".into()),
            task_id: params.task_id,
            playbook_id,
            dry_run,
            ..Fanout::default()
        },
        devices,
        work,
        Some(after),
    )
}

/// Turn the lines a playbook accumulated into one change for the pipeline.
/// Nothing is applied: `changes.create` leaves it in `draft`.
async fn make_change(
    app: &App,
    title: &str,
    lines: HashMap<String, Vec<String>>,
    mut results: Vec<DeviceResult>,
) -> Vec<DeviceResult> {
    // Only devices that got through their steps: a device that failed half
    // way must not have its half-rendered lines in a change.
    let targets: Vec<Value> = results
        .iter()
        .filter(|result| result.state == DeviceState::Ok)
        .filter_map(|result| {
            lines.get(&result.device_id).map(|lines| {
                json!({
                    "device_id": result.device_id,
                    "intent": {"type": "lines", "lines": lines},
                })
            })
        })
        .collect();
    if targets.is_empty() {
        return results;
    }
    let created = app
        .rpc(
            "changes.create",
            json!({
                "title": title,
                "description": "Configuration a playbook produced. Nothing has been sent: \
                                this change is planned, risked and approved like any other.",
                "origin": "playbook",
                "targets": targets,
            }),
        )
        .await;
    match created {
        Ok(created) => {
            let change_id = created
                .get("change_id")
                .or_else(|| created.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            for result in &mut results {
                if result.state == DeviceState::Ok && lines.contains_key(&result.device_id) {
                    result.change_id = Some(change_id.clone());
                }
            }
        }
        Err(err) => {
            let message = if err.code == "unknown_method" {
                "this build has no change pipeline, so the lines this playbook produced have \
                 nowhere to go"
                    .to_string()
            } else {
                format!("the change could not be created: {}", err.message)
            };
            for result in &mut results {
                if lines.contains_key(&result.device_id) {
                    result.state = DeviceState::Failed;
                    result.error = Some(message.clone());
                }
            }
        }
    }
    results
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add(
        "playbooks.list",
        |app: App, _: nocturne_app::NoParams| async move { db::playbooks(&app) },
    );

    router.add("playbooks.get", |app: App, params: IdParams| async move {
        db::playbook(&app, &params.id)
    });

    router.add(
        "playbooks.save",
        |app: App, params: SaveParams| async move {
            let name = db::require_name(&params.name, "playbook")?;
            let checked = validate(&app, &params.yaml);
            if !checked.ok {
                return Err(AppError::invalid(format!(
                    "this playbook does not validate: {}",
                    checked
                        .problems
                        .iter()
                        .map(|problem| problem.message.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                )));
            }
            let playbook = Playbook {
                id: params
                    .id
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or_else(new_id),
                name,
                description: params.description,
                yaml: params.yaml,
                updated_at: now_ms(),
            };
            app.store().put(db::PLAYBOOKS, &playbook.id, &playbook)?;
            app.audit(
                "playbooks.save",
                None,
                format!("Saved the playbook `{}`", playbook.name),
                json!({"playbook_id": playbook.id}),
            );
            app.publish("playbooks.changed", &json!({"id": playbook.id}));
            Ok(playbook)
        },
    );

    router.add(
        "playbooks.delete",
        |app: App, params: IdParams| async move {
            let playbook = db::playbook(&app, &params.id)?;
            app.store().delete(db::PLAYBOOKS, &params.id)?;
            app.audit(
                "playbooks.delete",
                None,
                format!("Deleted the playbook `{}`", playbook.name),
                json!({"playbook_id": params.id}),
            );
            app.publish("playbooks.changed", &json!({"id": params.id}));
            Ok(json!({"deleted": true}))
        },
    );

    router.add(
        "playbooks.validate",
        |app: App, params: ValidateParams| async move { Ok(validate(&app, &params.yaml)) },
    );

    router.add("playbooks.run", |app: App, params: RunParams| async move {
        start(&app, params)
    });

    router.add(
        "playbooks.runs",
        |app: App, params: RunsParams| async move {
            let mut runs = db::runs(&app, params.limit.unwrap_or(50))?;
            runs.retain(|run| run.kind == RunKind::Playbook);
            Ok(runs)
        },
    );

    router.add(
        "playbooks.run_get",
        |app: App, params: RunGetParams| async move {
            Ok(RunDetail {
                run: db::get_run(&app, &params.run_id)?,
                results: db::results(&app, &params.run_id)?,
            })
        },
    );
}
