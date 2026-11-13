//! Running the same thing on many devices, and keeping what each one said.
//!
//! Everything in this crate that touches more than one device goes through
//! [`fan_out`]: it is where the bounded concurrency, the cancellation, the
//! per-device lease and the honest tally live. A device that was selected
//! always has a result — `ok`, `failed`, `unreached` or `skipped` — so the
//! number of results always equals the number of devices the selector picked.
//! A run never shrinks quietly.

use std::sync::Arc;
use std::time::Instant;

use futures::future::BoxFuture;
use nocturne_app::connect::{self, CliOptions};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::model::Device;
use nocturne_app::util::new_id;
use nocturne_app::{App, AppBuilder, inventory, profiles};
use nocturne_drivers::policy::CommandClass;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{CommandResult, DeviceResult, DeviceState, Run, RunKind, Selector};
use crate::settings::Settings;
use crate::state::AutomationState;
use crate::{compare, db, select};

/// What is about to be run, before any device is touched.
#[derive(Clone, Debug, Default)]
pub struct Fanout {
    pub kind: RunKind,
    pub name: String,
    /// `user`, `schedule`, `trigger` or `assistant`.
    pub origin: String,
    pub task_id: Option<String>,
    pub playbook_id: Option<String>,
    pub commands: Vec<String>,
    pub dry_run: bool,
    /// Printed first in the job log and kept on the run record. This is how
    /// an Ansible run says, every time, that it is not a native change.
    pub caveat: Option<String>,
}

/// The per-device work. It is handed the device and returns its result;
/// reporting an error is returning a result, never `Err`.
pub type DeviceWork =
    Arc<dyn Fn(JobCtx, Device) -> BoxFuture<'static, DeviceResult> + Send + Sync + 'static>;

/// Work to do once every device is done and before the run is tallied — how a
/// playbook turns the lines it accumulated into one change.
pub type AfterRun = Arc<
    dyn Fn(App, Vec<DeviceResult>) -> BoxFuture<'static, Vec<DeviceResult>> + Send + Sync + 'static,
>;

/// Start a run: create its record, start a job, work through the devices with
/// bounded concurrency, and store a result for every one of them.
pub fn fan_out(app: &App, fan: Fanout, devices: Vec<Device>, work: DeviceWork) -> AppResult<Value> {
    fan_out_then(app, fan, devices, work, None)
}

/// [`fan_out`] with a step that runs once every device has finished.
pub fn fan_out_then(
    app: &App,
    fan: Fanout,
    devices: Vec<Device>,
    work: DeviceWork,
    after: Option<AfterRun>,
) -> AppResult<Value> {
    let state = app.state::<AutomationState>();
    let settings = Settings::load(app);
    let device_ids: Vec<String> = devices.iter().map(|device| device.id.clone()).collect();
    let run = Run {
        id: new_id(),
        kind: fan.kind,
        name: fan.name.clone(),
        task_id: fan.task_id.clone(),
        origin: if fan.origin.is_empty() {
            "user".into()
        } else {
            fan.origin.clone()
        },
        job_id: String::new(),
        commands: fan.commands.clone(),
        playbook_id: fan.playbook_id.clone(),
        started_at: state.now_ms(),
        finished_at: None,
        devices: devices.len(),
        dry_run: fan.dry_run,
        caveat: fan.caveat.clone(),
        ..Run::default()
    };

    let title = if fan.dry_run {
        format!("{} (dry run, {})", fan.name, select::describe(&devices))
    } else {
        format!("{} ({})", fan.name, select::describe(&devices))
    };
    let kind = match fan.kind {
        RunKind::Commands => "automation.runner",
        RunKind::Playbook => "automation.playbook",
        RunKind::Ansible => "automation.ansible",
    };

    let run_id = run.id.clone();
    let run_name = run.name.clone();
    let app_for_job = app.clone();
    let mut record = run.clone();
    let job = jobs::start(
        app,
        JobSpec::new(kind, title)
            .origin(&run.origin)
            .devices(&device_ids),
        move |ctx| async move {
            if let Some(caveat) = &fan.caveat {
                ctx.warn(caveat.clone());
            }
            if fan.dry_run {
                ctx.info("Dry run: nothing is sent to a device and no change is created.");
            }
            let state = app_for_job.state::<AutomationState>();
            let concurrency = settings.concurrency();
            let work = work.clone();
            let outcomes = ctx
                .for_each(devices, concurrency, move |ctx, device| {
                    let work = work.clone();
                    let state = state.clone();
                    let run_id = run_id.clone();
                    let run_name = run_name.clone();
                    async move {
                        let started = Instant::now();
                        // The one place two runs are kept off one device.
                        let lease = match state.claim(&device.id, &run_id, &run_name) {
                            Ok(lease) => lease,
                            Err(busy) => {
                                let reason = format!(
                                    "`{}` is already being worked on by the run `{}` that started {}s ago",
                                    device.name,
                                    busy.run_name,
                                    (state.now_ms() - busy.since).max(0) / 1000
                                );
                                ctx.device(&device.id, "warn", reason.clone());
                                return Ok(DeviceResult {
                                    device_id: device.id.clone(),
                                    name: device.name.clone(),
                                    reason: Some(reason),
                                    ..DeviceResult::default()
                                }
                                .with_state(DeviceState::Skipped));
                            }
                        };
                        let mut result = work(ctx.clone(), device.clone()).await;
                        drop(lease);
                        result.device_id = device.id.clone();
                        result.name = device.name.clone();
                        result.ok = result.state == DeviceState::Ok;
                        result.elapsed_ms = started.elapsed().as_millis() as u64;
                        match result.state {
                            DeviceState::Ok => ctx.device(&device.id, "info", format!("{} ok", device.name)),
                            DeviceState::Unreached => ctx.device(
                                &device.id,
                                "error",
                                format!(
                                    "{} could not be reached: {}",
                                    device.name,
                                    result.error.clone().unwrap_or_else(|| "no reason given".into())
                                ),
                            ),
                            DeviceState::Failed => ctx.device(
                                &device.id,
                                "error",
                                format!(
                                    "{} failed: {}",
                                    device.name,
                                    result.error.clone().unwrap_or_else(|| "no reason given".into())
                                ),
                            ),
                            DeviceState::Skipped => {}
                        }
                        Ok(result)
                    }
                })
                .await;

            // A device the job never got to (cancelled) is still in the run.
            let mut results: Vec<DeviceResult> = Vec::with_capacity(outcomes.len());
            for (index, outcome) in outcomes.into_iter().enumerate() {
                match outcome {
                    Ok(result) => results.push(result),
                    Err(err) => {
                        let device_id = device_ids.get(index).cloned().unwrap_or_default();
                        let name = inventory::get(&app_for_job, &device_id)
                            .map(|device| device.name)
                            .unwrap_or_else(|_| device_id.clone());
                        results.push(
                            DeviceResult {
                                device_id,
                                name,
                                reason: Some(if err.code == "cancelled" {
                                    "the run was stopped before this device was reached".into()
                                } else {
                                    err.message.clone()
                                }),
                                ..DeviceResult::default()
                            }
                            .with_state(DeviceState::Skipped),
                        );
                    }
                }
            }

            let mut results = match &after {
                Some(after) => after(app_for_job.clone(), results).await,
                None => results,
            };
            for result in &mut results {
                result.ok = result.state == DeviceState::Ok;
            }
            record.cancelled = ctx.is_cancelled();
            // From the job itself, not from the copy taken before `jobs::start`
            // returned: that one was cloned while the id was still empty, so
            // storing it here is what loses a finished run's link to its log.
            record.job_id = ctx.id().to_string();
            finish(&app_for_job, &mut record, &results)?;
            ctx.info(format!(
                "{} ok, {} failed, {} unreached, {} skipped",
                record.ok, record.failed, record.unreached, record.skipped
            ));
            // The record is already stored, so the job can end as what it was:
            // a run that somebody stopped is cancelled, not succeeded.
            if record.cancelled {
                return Err(jobs::cancelled());
            }
            Ok(json!({
                "run_id": record.id,
                "devices": record.devices,
                "ok": record.ok,
                "failed": record.failed,
                "unreached": record.unreached,
                "skipped": record.skipped,
                "change_ids": record.change_ids,
            }))
        },
    );

    let mut run = run;
    run.job_id = job.id.clone();
    db::save_run(app, &run)?;
    app.publish("automation.runs.changed", &run);
    Ok(json!({"job_id": job.id, "run_id": run.id}))
}

fn count(results: &[DeviceResult], state: DeviceState) -> usize {
    results
        .iter()
        .filter(|result| result.state == state)
        .count()
}

/// Close a run: tally it, store it with its results, prune and announce.
///
/// Shared with the Ansible path, which runs one process for every device at
/// once and so cannot use [`fan_out`], but must still leave the same record.
pub fn finish(app: &App, run: &mut Run, results: &[DeviceResult]) -> AppResult<()> {
    let state = app.state::<AutomationState>();
    run.finished_at = Some(state.now_ms());
    run.devices = results.len().max(run.devices);
    run.ok = count(results, DeviceState::Ok);
    run.failed = count(results, DeviceState::Failed);
    run.unreached = count(results, DeviceState::Unreached);
    run.skipped = count(results, DeviceState::Skipped);
    run.change_ids = results
        .iter()
        .filter_map(|result| result.change_id.clone())
        .collect();
    db::save_results(app, &run.id, results)?;
    db::save_run(app, run)?;
    db::prune_runs(app)?;
    app.publish("automation.runs.changed", run);
    Ok(())
}

/// A run record as it starts, for a path that does its own fanning out.
pub fn open_run(app: &App, fan: &Fanout, job_id: &str, devices: usize) -> Run {
    let state = app.state::<AutomationState>();
    Run {
        id: new_id(),
        kind: fan.kind,
        name: fan.name.clone(),
        task_id: fan.task_id.clone(),
        origin: if fan.origin.is_empty() {
            "user".into()
        } else {
            fan.origin.clone()
        },
        job_id: job_id.to_string(),
        commands: fan.commands.clone(),
        playbook_id: fan.playbook_id.clone(),
        started_at: state.now_ms(),
        devices,
        dry_run: fan.dry_run,
        caveat: fan.caveat.clone(),
        ..Run::default()
    }
}

// ------------------------------------------------------- the command runner

/// The device type a command will be classified against, when one is known in
/// advance. `None` means nothing says what the device is until it answers.
pub fn known_platform(
    app: &App,
    device: &Device,
) -> Option<Arc<nocturne_drivers::profile::Platform>> {
    if !device.profile.is_empty() {
        return profiles::platform(app, &device.profile).ok();
    }
    inventory::state_of(app, &device.id)
        .ok()
        .and_then(|state| state.detected_profile)
        .and_then(|id| profiles::registry(app).get(&id))
}

/// Refuse the whole run up front if any command is a change on any target.
///
/// `Cli::read` refuses it again on the wire — this is default-deny twice over,
/// so that nothing is sent to device 1 before device 40 is found to be the one
/// where `clear counters` is not a read.
pub fn check_read_only(app: &App, devices: &[Device], commands: &[String]) -> AppResult<()> {
    for device in devices {
        let Some(platform) = known_platform(app, device) else {
            continue;
        };
        for command in commands {
            let verdict = platform.policy().classify(command.trim());
            if verdict.class != CommandClass::ReadOnly {
                return Err(AppError::forbidden(format!(
                    "`{}` is not a read-only command on {} ({}), which is what `{}` is. \
                     Nothing was sent. Commands that change a device go through a change, \
                     where they are reviewed, backed up and can be rolled back.",
                    command.trim(),
                    platform.profile.name,
                    verdict.reason,
                    device.name,
                ))
                .with_details(json!({
                    "command": command.trim(),
                    "device_id": device.id,
                    "device": device.name,
                    "class": verdict.class,
                    "reason": verdict.reason,
                })));
            }
        }
    }
    Ok(())
}

fn cap(output: String, limit: usize) -> (String, bool) {
    if output.len() <= limit {
        return (output, false);
    }
    let mut cut = limit;
    while cut > 0 && !output.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = output.len() - cut;
    let mut text = output[..cut].to_string();
    text.push_str(&format!(
        "\n… [{dropped} more bytes were not kept: this output is over the {limit}-byte cap]\n"
    ));
    (text, true)
}

/// Parse one command's output with the device type's ntc-templates platform.
/// `None` is "no template covers this command", never "nothing matched".
fn parse_output(
    platform: Option<&nocturne_drivers::profile::Platform>,
    command: &str,
    output: &str,
) -> Option<Vec<Value>> {
    let ntc = platform?.profile.ntc_platform.as_deref()?;
    let rows = nocturne_textfsm::ntc::parse(ntc, command, output).ok()??;
    rows.into_iter()
        .map(|row| serde_json::to_value(row).ok())
        .collect()
}

/// Run read-only commands on one device over one session.
pub async fn run_commands(
    app: &App,
    device: &Device,
    commands: &[String],
    parse: bool,
    cap_bytes: usize,
) -> DeviceResult {
    let base = DeviceResult {
        device_id: device.id.clone(),
        name: device.name.clone(),
        ..DeviceResult::default()
    };
    let mut cli = match connect::cli(app, &device.id, CliOptions::default()).await {
        Ok(cli) => cli,
        Err(err) => {
            return DeviceResult {
                error: Some(err.message),
                ..base
            }
            .with_state(DeviceState::Unreached);
        }
    };
    let platform = cli.platform.clone();
    let mut outputs = Vec::with_capacity(commands.len());
    let mut failure: Option<String> = None;
    for command in commands {
        match cli.read(command).await {
            Ok(output) => {
                let (text, truncated) = cap(output.output, cap_bytes);
                let parsed = if parse {
                    parse_output(Some(&platform), command, &text)
                } else {
                    None
                };
                if let Some(error) = &output.error {
                    failure.get_or_insert(format!("`{command}`: {error}"));
                }
                outputs.push(CommandResult {
                    command: command.clone(),
                    output: text,
                    error: output.error,
                    elapsed_ms: output.elapsed_ms,
                    truncated,
                    parsed,
                });
            }
            Err(err) => {
                failure.get_or_insert(format!("`{command}`: {}", err.message));
                outputs.push(CommandResult {
                    command: command.clone(),
                    error: Some(err.message),
                    ..CommandResult::default()
                });
                // The session is not to be trusted after a refusal or a break.
                break;
            }
        }
    }
    cli.close().await;
    let result = DeviceResult {
        outputs,
        error: failure.clone(),
        ..base
    };
    result.with_state(if failure.is_some() {
        DeviceState::Failed
    } else {
        DeviceState::Ok
    })
}

// -------------------------------------------------------------------- rpc

#[derive(Debug, Deserialize)]
pub struct RunParams {
    #[serde(flatten)]
    pub select: Selector,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parse: bool,
    /// Say what would run, send nothing.
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub origin: Option<String>,
    /// Set when the run comes from a saved task.
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RunsParams {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    kind: Option<RunKind>,
}

#[derive(Debug, Deserialize)]
struct GetParams {
    run_id: String,
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Serialize)]
struct RunDetail {
    run: Run,
    results: Vec<DeviceResult>,
}

#[derive(Debug, Deserialize)]
struct CompareParams {
    run_id: String,
    command: String,
    /// Extra regular expressions whose lines are ignored when grouping.
    #[serde(default)]
    ignore: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    run_id: String,
    query: String,
    #[serde(default)]
    regex: bool,
}

#[derive(Debug, Deserialize)]
struct ExportParams {
    run_id: String,
    #[serde(default)]
    format: String,
}

#[derive(Debug, Deserialize)]
struct DeleteParams {
    run_ids: Vec<String>,
}

/// Everything `runner.run` needs, validated, before a job exists.
pub fn start_command_run(app: &App, params: RunParams) -> AppResult<Value> {
    let commands: Vec<String> = params
        .commands
        .iter()
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .collect();
    if commands.is_empty() {
        return Err(AppError::invalid("give at least one command to run"));
    }
    if commands.len() > 50 {
        return Err(AppError::invalid(
            "at most 50 commands in one run; split it into two",
        ));
    }
    if let Some(bad) = commands
        .iter()
        .find(|command| command.contains(['\n', '\r']))
    {
        return Err(AppError::invalid(format!(
            "one command per entry: `{bad}` contains a line break"
        )));
    }
    let devices = select::resolve(app, &params.select)?;
    check_read_only(app, &devices, &commands)?;

    let name = params
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| commands.join("; "));
    let settings = Settings::load(app);
    let fan = Fanout {
        kind: RunKind::Commands,
        name,
        origin: params.origin.unwrap_or_else(|| "user".into()),
        task_id: params.task_id,
        commands: commands.clone(),
        dry_run: params.dry_run,
        ..Fanout::default()
    };

    let app_for_work = app.clone();
    let parse = params.parse;
    let cap_bytes = settings.output_cap();
    let dry_run = params.dry_run;
    let work: DeviceWork = Arc::new(move |_ctx, device| {
        let app = app_for_work.clone();
        let commands = commands.clone();
        Box::pin(async move {
            if dry_run {
                return dry_run_result(&app, &device, &commands);
            }
            run_commands(&app, &device, &commands, parse, cap_bytes).await
        })
    });
    fan_out(app, fan, devices, work)
}

/// What a device would be asked, and whether that can be checked in advance.
fn dry_run_result(app: &App, device: &Device, commands: &[String]) -> DeviceResult {
    let platform = known_platform(app, device);
    let note = match &platform {
        Some(platform) => format!(
            "would send {} command(s) over {} as {}",
            commands.len(),
            platform.profile.name,
            device.address
        ),
        None => format!(
            "would send {} command(s) to {}; its device type is not known until it answers, \
             so each command is classified when the session opens",
            commands.len(),
            device.address
        ),
    };
    DeviceResult {
        device_id: device.id.clone(),
        name: device.name.clone(),
        reason: Some(note),
        outputs: commands
            .iter()
            .map(|command| CommandResult {
                command: command.clone(),
                ..CommandResult::default()
            })
            .collect(),
        ..DeviceResult::default()
    }
    .with_state(DeviceState::Ok)
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("runner.run", |app: App, params: RunParams| async move {
        start_command_run(&app, params)
    });

    router.add("runner.runs", |app: App, params: RunsParams| async move {
        let mut runs = db::runs(&app, params.limit.unwrap_or(50))?;
        if let Some(task_id) = &params.task_id {
            runs.retain(|run| run.task_id.as_deref() == Some(task_id.as_str()));
        }
        if let Some(kind) = params.kind {
            runs.retain(|run| run.kind == kind);
        }
        Ok(runs)
    });

    router.add("runner.get", |app: App, params: GetParams| async move {
        let run = db::get_run(&app, &params.run_id)?;
        let mut results = db::results(&app, &params.run_id)?;
        if let Some(device_id) = &params.device_id {
            results.retain(|result| &result.device_id == device_id);
        }
        Ok(RunDetail { run, results })
    });

    router.add(
        "runner.compare",
        |app: App, params: CompareParams| async move {
            let settings = Settings::load(&app);
            let results = db::results(&app, &params.run_id)?;
            compare::compare(&results, &params.command, &settings, &params.ignore)
        },
    );

    router.add(
        "runner.search",
        |app: App, params: SearchParams| async move {
            let results = db::results(&app, &params.run_id)?;
            compare::search(&results, &params.query, params.regex)
        },
    );

    router.add(
        "runner.export",
        |app: App, params: ExportParams| async move {
            let run = db::get_run(&app, &params.run_id)?;
            let results = db::results(&app, &params.run_id)?;
            let format = if params.format.trim().is_empty() {
                "text".to_string()
            } else {
                params.format.trim().to_ascii_lowercase()
            };
            Ok(json!({"text": compare::export(&run, &results, &format)?}))
        },
    );

    router.add(
        "runner.delete",
        |app: App, params: DeleteParams| async move {
            let mut removed = 0;
            for id in &params.run_ids {
                if db::delete_run(&app, id)? {
                    removed += 1;
                }
            }
            app.audit(
                "runner.delete",
                None,
                format!("Deleted {removed} automation run(s)"),
                json!({"run_ids": params.run_ids}),
            );
            Ok(json!({"removed": removed}))
        },
    );
}
