//! Ansible interoperability — and exactly what it is worth.
//!
//! Nocturne does not use Ansible as its engine (`docs/ARCHITECTURE.md`, "Why
//! not Ansible as the engine?"). It interoperates, because teams have
//! playbooks and those playbooks are real work.
//!
//! # What this path gives you
//!
//! * the inventory generated from *this* inventory, so a playbook runs against
//!   the devices Nocturne knows about, chosen by the same filters;
//! * credentials passed **only through the child process's environment**,
//!   referenced from the inventory as `{{ lookup('env', 'NOCTURNE_PW_1') }}` —
//!   never on a command line, never written to a file, and scrubbed from the
//!   output that is streamed into the job log;
//! * a temporary inventory, mode 0600 in a 0700 directory, deleted when the
//!   run ends however it ends;
//! * `--check` by default, the process streamed live into a job you can cancel
//!   (which kills it), and a per-device result parsed from `PLAY RECAP` — a
//!   host Ansible could not reach is recorded `unreached`, like any other
//!   device Nocturne could not reach.
//!
//! # What this path does not give you
//!
//! **None of the change pipeline's guarantees apply to what a playbook does.**
//! A native change is: hierarchical diff → predicted configuration → risk and
//! protected-resource assessment → a human approving *that exact plan* by its
//! hash → backup → dead-man switch → send → verify → roll back on failure. A
//! playbook has none of that:
//!
//! * no backup is taken first and no configuration version is recorded, so
//!   there is nothing to compare against or restore from afterwards;
//! * no drift check: Nocturne does not know whether the device still looked
//!   the way it did when anybody last read it;
//! * no plan, no `plan_hash`, no approval — nobody signed off on the lines
//!   that will actually be sent, because Nocturne never computed them;
//! * no verification and **no rollback**: if a task half-succeeds, the device
//!   stays half-changed and the only record is the text Ansible printed;
//! * no protected-resource or risk assessment, so a playbook will happily
//!   reconfigure a management VLAN on a device marked protected;
//! * idempotence and `--check` support are the module's business, not
//!   Nocturne's. `--check` on a network module usually means "say what you
//!   would do"; on some it means nothing at all;
//! * host-key checking is Ansible's, with Ansible's configuration. Nocturne's
//!   known-hosts store is not consulted;
//! * what the playbook changed is known only from its own output. The
//!   inventory, the collected data and the configuration history in Nocturne
//!   are not updated by it.
//!
//! Every run says this: the caveat is the first line of the job log and it is
//! stored on the run record. The honest summary is that this is a convenient
//! way to run what you already have, and that a native change is the safe way
//! to change a device.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs::{self, JobCtx, JobSpec};
use nocturne_app::model::{Device, Transport};
use nocturne_app::{App, AppBuilder, NoParams, credentials};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::model::{AnsibleStatus, DeviceResult, DeviceState, Run, RunKind, Selector, StepResult};
use crate::runner::Fanout;
use crate::settings::Settings;
use crate::state::AutomationState;
use crate::{runner, select, templates};

/// Said on every run, stored on every run record.
pub const CAVEAT: &str = "This runs `ansible-playbook` outside Nocturne's change pipeline: \
     no backup is taken, no drift check is made, no plan is approved, nothing is verified and \
     nothing can be rolled back. Whatever the playbook changes is the playbook's business. \
     A native change (changes.create) is the path that gives those guarantees.";

fn caveats() -> Vec<String> {
    vec![
        CAVEAT.to_string(),
        "Credentials reach the playbook only through the process environment, and the \
         temporary inventory refers to them with `lookup('env', …)`."
            .to_string(),
        "Per-device outcomes are parsed from Ansible's `PLAY RECAP`, so a playbook run with a \
         custom stdout callback may report nothing per host."
            .to_string(),
        "`--check` is on by default, but what check mode means is each module's business."
            .to_string(),
        "Host-key checking is Ansible's own; Nocturne's known-hosts store is not used.".to_string(),
        "A device with an enable secret is given `ansible_become_method: enable`, and a \
         device type Nocturne can map is given `ansible.netcommon.network_cli`. Both come \
         from the `ansible.netcommon` collection: without it installed, the play fails."
            .to_string(),
    ]
}

// ------------------------------------------------------------------ status

/// Find `ansible-playbook`: the configured path, or the first one on `PATH`.
fn find_executable(settings: &Settings) -> Option<PathBuf> {
    let configured = settings.ansible_path.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        return path.is_file().then_some(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["ansible-playbook.exe", "ansible-playbook"]
    } else {
        &["ansible-playbook"]
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            names
                .iter()
                .map(|name| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
    })
}

async fn first_line(path: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(|line| line.trim().to_string())
}

/// Collections `ansible-galaxy` reports, or `None` when it could not be asked
/// — which is not the same as "none installed".
async fn collections(playbook: &Path) -> Option<Vec<String>> {
    let galaxy = playbook.with_file_name(if cfg!(windows) {
        "ansible-galaxy.exe"
    } else {
        "ansible-galaxy"
    });
    if !galaxy.is_file() {
        return None;
    }
    let output = tokio::process::Command::new(&galaxy)
        .args(["collection", "list"])
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.contains('.') && !line.starts_with('#') && !line.starts_with("---"))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.contains('.') && !name.contains('/'))
        .map(str::to_string)
        .collect();
    found.sort();
    found.dedup();
    Some(found)
}

pub async fn status(app: &App) -> AppResult<AnsibleStatus> {
    let settings = Settings::load(app);
    if !app.local_access() {
        return Ok(AnsibleStatus {
            unsupported: Some(
                "running a playbook needs a local process, which the server mode of Nocturne \
                 does not have. Use the desktop app on the machine where Ansible is installed."
                    .into(),
            ),
            caveats: caveats(),
            ..AnsibleStatus::default()
        });
    }
    let Some(path) = find_executable(&settings) else {
        return Ok(AnsibleStatus {
            unsupported: Some(
                "`ansible-playbook` was not found on PATH. Install Ansible (`pipx install \
                 ansible-core`) or set `automation.ansible_path` to the executable. Nothing \
                 else in Nocturne needs it."
                    .into(),
            ),
            caveats: caveats(),
            ..AnsibleStatus::default()
        });
    };
    let version = first_line(&path, &["--version"]).await;
    if version.is_none() {
        return Ok(AnsibleStatus {
            path: Some(path.display().to_string()),
            unsupported: Some(format!(
                "`{}` is there but would not say its version, so it cannot be used",
                path.display()
            )),
            caveats: caveats(),
            ..AnsibleStatus::default()
        });
    }
    Ok(AnsibleStatus {
        installed: true,
        version,
        collections: collections(&path).await,
        path: Some(path.display().to_string()),
        unsupported: None,
        caveats: caveats(),
    })
}

// ------------------------------------------------------------- the mapping

/// Nocturne profile → the Ansible network OS and the module that sends
/// configuration lines. Only the platforms whose mapping is certain are here;
/// everything else is honestly `unsupported`.
const MAPPING: &[(&str, &str, &str, &str)] = &[
    // profile, ansible_network_os, config module, the module's lines argument
    (
        "cisco_ios",
        "cisco.ios.ios",
        "cisco.ios.ios_config",
        "lines",
    ),
    (
        "cisco_nxos",
        "cisco.nxos.nxos",
        "cisco.nxos.nxos_config",
        "lines",
    ),
    (
        "cisco_xr",
        "cisco.iosxr.iosxr",
        "cisco.iosxr.iosxr_config",
        "lines",
    ),
    (
        "arista_eos",
        "arista.eos.eos",
        "arista.eos.eos_config",
        "lines",
    ),
    ("vyos", "vyos.vyos.vyos", "vyos.vyos.vyos_config", "lines"),
    (
        "juniper_junos",
        "junipernetworks.junos.junos",
        "junipernetworks.junos.junos_config",
        "lines",
    ),
    (
        "mikrotik_routeros",
        "community.routeros.routeros",
        "community.routeros.command",
        "commands",
    ),
];

fn mapping_for(profile: &str) -> Option<(&'static str, &'static str, &'static str)> {
    MAPPING
        .iter()
        .find(|(id, ..)| *id == profile)
        .map(|(_, os, module, argument)| (*os, *module, *argument))
}

/// The Ansible network OS for a profile, for the inventory alone.
fn network_os(profile: &str) -> Option<&'static str> {
    MAPPING
        .iter()
        .find(|(id, ..)| *id == profile)
        .map(|(_, os, ..)| *os)
}

/// An inventory host name: Ansible-safe and unique.
fn host_names(devices: &[Device]) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(devices.len());
    for device in devices {
        let cleaned: String = device
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let base = if cleaned.trim_matches('_').is_empty() {
            format!("device_{}", &device.id[..device.id.len().min(8)])
        } else {
            cleaned
        };
        let mut candidate = base.clone();
        let mut suffix = 2;
        while names.iter().any(|taken| taken == &candidate) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        names.push(candidate);
    }
    names
}

/// Devices Ansible cannot reach at all, and why.
fn unreachable_by_design(device: &Device) -> Option<&'static str> {
    match device.transport {
        Transport::Sim => Some(
            "a simulated device has no SSH service for Ansible to connect to; it is only a \
             device to Nocturne",
        ),
        Transport::Serial => Some("Ansible has no serial connection plugin"),
        Transport::None => Some("this device has no CLI access configured"),
        Transport::Ssh | Transport::Telnet => None,
    }
}

/// The secret for a device, as an environment variable name and value.
struct Secret {
    variable: String,
    value: String,
}

/// Build the inventory. `secrets` is filled with the environment variables the
/// child process must be given; the inventory itself only names them.
fn inventory_for(
    app: &App,
    devices: &[Device],
    names: &[String],
    secrets: &mut Vec<Secret>,
    with_secrets: bool,
) -> AppResult<String> {
    let mut hosts = Map::new();
    let mut groups: std::collections::BTreeMap<String, Map<String, Value>> = Default::default();
    for (index, (device, host)) in devices.iter().zip(names).enumerate() {
        let mut vars = Map::new();
        vars.insert("ansible_host".into(), json!(device.address));
        if let Some(port) = device.port {
            vars.insert("ansible_port".into(), json!(port));
        }
        if device.transport == Transport::Telnet {
            vars.insert(
                "ansible_connection".into(),
                json!("ansible.netcommon.telnet"),
            );
        } else if let Some(os) = network_os(&device.profile) {
            vars.insert("ansible_network_os".into(), json!(os));
            vars.insert(
                "ansible_connection".into(),
                json!("ansible.netcommon.network_cli"),
            );
        }
        if let Some(credential_id) = &device.credential_id {
            let resolved = credentials::resolve(app, credential_id)?;
            if !resolved.username.is_empty() {
                vars.insert("ansible_user".into(), json!(resolved.username));
            }
            // The value never goes in the inventory, only the name of the
            // environment variable that will hold it.
            if let Some(password) = &resolved.password {
                let variable = format!("NOCTURNE_PW_{}", index + 1);
                vars.insert(
                    "ansible_password".into(),
                    json!(format!("{{{{ lookup('env', '{variable}') }}}}")),
                );
                if with_secrets {
                    secrets.push(Secret {
                        variable,
                        value: password.to_string(),
                    });
                }
            }
            if let Some(enable) = &resolved.enable_secret {
                let variable = format!("NOCTURNE_EN_{}", index + 1);
                vars.insert("ansible_become".into(), json!(true));
                vars.insert("ansible_become_method".into(), json!("enable"));
                vars.insert(
                    "ansible_become_password".into(),
                    json!(format!("{{{{ lookup('env', '{variable}') }}}}")),
                );
                if with_secrets {
                    secrets.push(Secret {
                        variable,
                        value: enable.to_string(),
                    });
                }
            }
        }
        hosts.insert(host.clone(), Value::Object(vars));
        let group = |prefix: &str, value: &str| -> String {
            let cleaned: String = value
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            format!("{prefix}_{}", cleaned.trim_matches('_'))
        };
        for name in [
            (!device.site.is_empty()).then(|| group("site", &device.site)),
            (!device.role.is_empty()).then(|| group("role", &device.role)),
            (!device.profile.is_empty()).then(|| group("os", &device.profile)),
        ]
        .into_iter()
        .flatten()
        {
            groups
                .entry(name)
                .or_default()
                .insert(host.clone(), json!({}));
        }
    }

    let mut all = Map::new();
    all.insert("hosts".into(), Value::Object(hosts));
    if !groups.is_empty() {
        let children: Map<String, Value> = groups
            .into_iter()
            .map(|(name, hosts)| (name, json!({"hosts": hosts})))
            .collect();
        all.insert("children".into(), Value::Object(children));
    }
    let document = json!({"all": all});
    let yaml = serde_yaml_ng::to_string(&document)
        .map_err(|err| AppError::internal(format!("the inventory could not be written: {err}")))?;
    Ok(format!(
        "# Generated by Nocturne. No secret is in this file: passwords are read from the \n\
         # environment of the `ansible-playbook` process Nocturne starts.\n{yaml}"
    ))
}

// ----------------------------------------------------------------- exports

#[derive(Debug, Deserialize)]
pub struct ExportParams {
    #[serde(default)]
    pub change_id: Option<String>,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(flatten)]
    pub select: Selector,
    #[serde(default)]
    pub vars: Value,
}

/// A playbook that would send the same lines, plus the matching inventory.
pub async fn export(app: &App, params: ExportParams) -> AppResult<Value> {
    let (title, per_device): (String, Vec<(Device, Vec<String>)>) =
        match (&params.change_id, &params.template_id) {
            (Some(change_id), _) if !change_id.trim().is_empty() => {
                lines_from_change(app, change_id.trim()).await?
            }
            (_, Some(template_id)) if !template_id.trim().is_empty() => {
                lines_from_template(app, template_id.trim(), &params).await?
            }
            _ => {
                return Err(AppError::invalid(
                    "give a `change_id`, or a `template_id` with devices",
                ));
            }
        };

    let mut plays = Vec::new();
    let mut unsupported = Vec::new();
    for (device, lines) in &per_device {
        let Some((_, module, argument)) = mapping_for(&device.profile) else {
            unsupported.push(format!(
                "{} ({})",
                device.name,
                if device.profile.is_empty() {
                    "device type unknown"
                } else {
                    &device.profile
                }
            ));
            continue;
        };
        let names = host_names(std::slice::from_ref(device));
        plays.push(json!({
            "name": format!("{} — {}", names[0], title),
            "hosts": names[0],
            "gather_facts": false,
            "tasks": [{
                "name": title,
                module: {argument: lines},
            }],
        }));
    }
    if !unsupported.is_empty() {
        return Err(AppError::new(
            "unsupported",
            format!(
                "Nocturne will not guess which Ansible module configures {}. Export is only \
                 offered for the platforms whose module and argument are certain: {}.",
                unsupported.join(", "),
                MAPPING
                    .iter()
                    .map(|(profile, ..)| *profile)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let devices: Vec<Device> = per_device.into_iter().map(|(device, _)| device).collect();
    let names = host_names(&devices);
    let mut secrets = Vec::new();
    let inventory = inventory_for(app, &devices, &names, &mut secrets, false)?;
    let playbook = serde_yaml_ng::to_string(&Value::Array(plays))
        .map_err(|err| AppError::internal(format!("the playbook could not be written: {err}")))?;
    Ok(json!({
        "playbook": format!("# Generated by Nocturne.\n# {CAVEAT}\n{playbook}"),
        "inventory": inventory,
        "caveat": CAVEAT,
    }))
}

async fn lines_from_change(
    app: &App,
    change_id: &str,
) -> AppResult<(String, Vec<(Device, Vec<String>)>)> {
    let change = app
        .rpc("changes.get", json!({"id": change_id}))
        .await
        .map_err(|err| match err.code {
            "unknown_method" => {
                AppError::unavailable("this build has no change pipeline to export from")
            }
            _ => err,
        })?;
    let title = change
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Nocturne change")
        .to_string();
    let targets = change
        .get("targets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut per_device = Vec::new();
    for target in targets {
        let device_id = target
            .get("device_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let device = nocturne_app::inventory::get(app, device_id)?;
        // The planned commands are what would really be sent; the intent's own
        // lines are the fallback for a change nobody has planned yet.
        let lines = target
            .pointer("/plan/commands")
            .and_then(Value::as_array)
            .or_else(|| target.pointer("/intent/lines").and_then(Value::as_array))
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if lines.is_empty() {
            return Err(AppError::invalid(format!(
                "there are no commands to export for `{}`: plan the change first (an \
                 `intended` change only becomes commands when it is planned against the \
                 device's running configuration)",
                device.name
            )));
        }
        per_device.push((device, lines));
    }
    if per_device.is_empty() {
        return Err(AppError::invalid("that change has no devices"));
    }
    Ok((title, per_device))
}

async fn lines_from_template(
    app: &App,
    template_id: &str,
    params: &ExportParams,
) -> AppResult<(String, Vec<(Device, Vec<String>)>)> {
    let template = templates::get(app, template_id)?;
    let devices = select::resolve(app, &params.select)?;
    let vars = templates::bind(&template.variables, &params.vars)?;
    let rendered = templates::render_for(app, &template.body, &devices, &vars);
    let mut per_device = Vec::new();
    for item in rendered {
        let device = devices
            .iter()
            .find(|device| device.id == item.device_id)
            .cloned()
            .expect("rendered device is in the selection");
        match item.lines {
            Some(lines) if !lines.is_empty() => per_device.push((device, lines)),
            _ => {
                return Err(AppError::invalid(format!(
                    "`{}`: {}",
                    item.name,
                    item.error
                        .unwrap_or_else(|| "the template rendered nothing".into())
                )));
            }
        }
    }
    Ok((template.name, per_device))
}

// --------------------------------------------------------------------- run

#[derive(Debug, Deserialize)]
pub struct RunParams {
    pub playbook_path: String,
    #[serde(flatten)]
    pub select: Selector,
    /// Passed to `--limit` on top of the generated inventory.
    #[serde(default)]
    pub limit: Option<String>,
    #[serde(default = "check_by_default")]
    pub check: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    /// Report what would be run — the inventory, the command line, the
    /// devices — and start no process.
    #[serde(default)]
    pub dry_run: bool,
}

fn check_by_default() -> bool {
    true
}

/// Arguments that would break the promises this module makes.
fn refuse_argument(argument: &str) -> Option<&'static str> {
    let head = argument.split('=').next().unwrap_or(argument);
    match head {
        "-i" | "--inventory" | "--inventory-file" => {
            Some("Nocturne supplies the inventory; use `device_ids`/`filter` to choose devices")
        }
        "-k" | "--ask-pass" | "-K" | "--ask-become-pass" | "--ask-vault-pass" => {
            Some("there is no terminal to type a password into; credentials come from the vault")
        }
        "-l" | "--limit" => {
            Some("use `limit` instead, which is applied to the generated inventory")
        }
        _ => None,
    }
}

pub async fn run(app: &App, params: RunParams) -> AppResult<Value> {
    app.require_local_access("running an Ansible playbook")?;
    let settings = Settings::load(app);
    let playbook = PathBuf::from(params.playbook_path.trim());
    if params.playbook_path.trim().is_empty() {
        return Err(AppError::invalid("name the playbook to run"));
    }
    if !playbook.is_file() {
        return Err(AppError::not_found(format!(
            "the playbook `{}`",
            playbook.display()
        )));
    }
    for argument in &params.extra_args {
        if let Some(reason) = refuse_argument(argument) {
            return Err(AppError::invalid(format!(
                "`{argument}` is not passed on: {reason}"
            )));
        }
    }
    let executable = match find_executable(&settings) {
        Some(executable) => executable,
        None => {
            // Not a failure: this machine simply has no Ansible.
            return Err(AppError::new(
                "unsupported",
                "`ansible-playbook` was not found on PATH, so an Ansible playbook cannot be \
                 run here. Install Ansible (`pipx install ansible-core`) or set \
                 `automation.ansible_path`. Nothing else in Nocturne needs it.",
            ));
        }
    };

    let selected = select::resolve(app, &params.select)?;
    let state = app.state::<AutomationState>();
    let run_name = format!(
        "{}{}",
        playbook
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "playbook".into()),
        if params.check { " (--check)" } else { "" }
    );

    // One process covers every device, so the leases are taken before it
    // starts rather than per device inside a fan-out.
    let fan = Fanout {
        kind: RunKind::Ansible,
        name: run_name.clone(),
        origin: params.origin.unwrap_or_else(|| "user".into()),
        task_id: params.task_id,
        dry_run: params.dry_run,
        caveat: Some(CAVEAT.to_string()),
        ..Fanout::default()
    };

    let device_ids: Vec<String> = selected.iter().map(|device| device.id.clone()).collect();
    let app_for_job = app.clone();
    let total = selected.len();
    let check = params.check;
    let extra_args = params.extra_args.clone();
    let user_limit = params.limit.clone();
    let dry_run = params.dry_run;
    let timeout = settings.ansible_timeout_s.clamp(10, 24 * 3600);

    let job = jobs::start(
        app,
        JobSpec::new("automation.ansible", format!("Ansible: {run_name}"))
            .origin(&fan.origin)
            .devices(&device_ids)
            .total(1),
        move |ctx| async move {
            let mut record = runner::open_run(&app_for_job, &fan, ctx.id(), total);
            ctx.warn(CAVEAT);
            let results = execute(
                &app_for_job,
                &ctx,
                &record,
                &executable,
                &playbook,
                selected,
                check,
                &extra_args,
                user_limit.as_deref(),
                dry_run,
                timeout,
            )
            .await;
            let results = match results {
                Ok(results) => results,
                Err(err) => {
                    record.cancelled = ctx.is_cancelled();
                    runner::finish(&app_for_job, &mut record, &[])?;
                    return Err(err);
                }
            };
            record.cancelled = ctx.is_cancelled();
            runner::finish(&app_for_job, &mut record, &results)?;
            if record.cancelled {
                // The record is stored; the job ends as what it was.
                return Err(jobs::cancelled());
            }
            Ok(json!({
                "run_id": record.id,
                "ok": record.ok,
                "failed": record.failed,
                "unreached": record.unreached,
                "skipped": record.skipped,
                "outside_change_pipeline": true,
                "caveat": CAVEAT,
            }))
        },
    );
    let _ = state;
    app.audit(
        "ansible.run",
        None,
        format!(
            "Ran the Ansible playbook `{}` against {} device(s){}",
            params.playbook_path.trim(),
            total,
            if params.check { " in check mode" } else { "" }
        ),
        json!({"check": params.check, "outside_change_pipeline": true}),
    );
    Ok(json!({"job_id": job.id}))
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    app: &App,
    ctx: &JobCtx,
    record: &Run,
    executable: &Path,
    playbook: &Path,
    selected: Vec<Device>,
    check: bool,
    extra_args: &[String],
    user_limit: Option<&str>,
    dry_run: bool,
    timeout_s: u64,
) -> AppResult<Vec<DeviceResult>> {
    let state = app.state::<AutomationState>();
    let mut results: Vec<DeviceResult> = Vec::with_capacity(selected.len());
    let mut runnable: Vec<Device> = Vec::new();
    let mut leases = Vec::new();

    for device in selected {
        if let Some(reason) = unreachable_by_design(&device) {
            ctx.device(&device.id, "warn", format!("{}: {reason}", device.name));
            results.push(
                DeviceResult {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    reason: Some(reason.to_string()),
                    ..DeviceResult::default()
                }
                .with_state(DeviceState::Skipped),
            );
            continue;
        }
        match state.claim(&device.id, &record.id, &record.name) {
            Ok(lease) => {
                leases.push(lease);
                runnable.push(device);
            }
            Err(busy) => {
                let reason = format!(
                    "`{}` is already being worked on by the run `{}`",
                    device.name, busy.run_name
                );
                ctx.device(&device.id, "warn", reason.clone());
                results.push(
                    DeviceResult {
                        device_id: device.id.clone(),
                        name: device.name.clone(),
                        reason: Some(reason),
                        ..DeviceResult::default()
                    }
                    .with_state(DeviceState::Skipped),
                );
            }
        }
    }
    if runnable.is_empty() {
        ctx.warn("no device in this selection can be reached by Ansible");
        return Ok(results);
    }

    let names = host_names(&runnable);
    let mut secrets = Vec::new();
    let inventory = inventory_for(app, &runnable, &names, &mut secrets, true)?;

    let limit = match user_limit {
        Some(limit) if !limit.trim().is_empty() => limit.trim().to_string(),
        _ => names.join(","),
    };
    let mut arguments: Vec<String> = vec!["-i".into(), "<inventory>".into()];
    if check {
        arguments.push("--check".into());
    }
    arguments.push("--limit".into());
    arguments.push(limit.clone());
    arguments.extend(extra_args.iter().cloned());
    arguments.push(playbook.display().to_string());

    if dry_run {
        ctx.info(format!(
            "would run: {} {}",
            executable.display(),
            arguments.join(" ")
        ));
        ctx.info(format!(
            "the generated inventory would be:\n{}",
            inventory.trim_end()
        ));
        if !secrets.is_empty() {
            ctx.info(format!(
                "{} credential(s) would be passed through the environment as {}",
                secrets.len(),
                secrets
                    .iter()
                    .map(|secret| secret.variable.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for (device, host) in runnable.iter().zip(&names) {
            results.push(
                DeviceResult {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    reason: Some(format!("would be inventory host `{host}`")),
                    ..DeviceResult::default()
                }
                .with_state(DeviceState::Ok),
            );
        }
        return Ok(results);
    }

    // A directory of our own, 0700, holding a 0600 inventory, removed on drop
    // however this function ends — including a cancel and a panic.
    let directory = tempfile::Builder::new()
        .prefix("nocturne-ansible-")
        .tempdir()
        .map_err(|err| {
            AppError::internal(format!("a temporary directory could not be made: {err}"))
        })?;
    let inventory_path = directory.path().join("inventory.yml");
    write_private(&inventory_path, &inventory)?;

    let secret_values: Vec<String> = secrets
        .iter()
        .map(|secret| secret.value.clone())
        .filter(|value| value.len() >= 4)
        .collect();

    let mut command = tokio::process::Command::new(executable);
    command
        .arg("-i")
        .arg(&inventory_path)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if check {
        command.arg("--check");
    }
    command.arg("--limit").arg(&limit);
    command.args(extra_args);
    command.arg(playbook);
    // Credentials: here and nowhere else.
    for secret in &secrets {
        command.env(&secret.variable, &secret.value);
    }
    // Ansible must not stop to ask about a host key or a password: there is
    // nobody at the other end of this process's stdin.
    command.env("ANSIBLE_NOCOLOR", "1");
    command.env("ANSIBLE_FORCE_COLOR", "0");
    command.env("PYTHONUNBUFFERED", "1");

    ctx.info(format!(
        "{} {} --limit {} {}",
        executable.display(),
        if check { "--check" } else { "" },
        limit,
        playbook.display()
    ));

    let mut child = command.spawn().map_err(|err| {
        AppError::internal(format!(
            "`{}` could not be started: {err}",
            executable.display()
        ))
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let transcript = Arc::new(parking_lot::Mutex::new(String::new()));

    let pump = |stream: Option<tokio::process::ChildStdout>,
                err_stream: Option<tokio::process::ChildStderr>,
                ctx: JobCtx,
                transcript: Arc<parking_lot::Mutex<String>>,
                secrets: Vec<String>| async move {
        // Two readers, one line handler.
        let handle = |line: String,
                      level: &str,
                      ctx: &JobCtx,
                      transcript: &Arc<parking_lot::Mutex<String>>| {
            let mut line = line;
            for secret in &secrets {
                if line.contains(secret.as_str()) {
                    line = line.replace(secret.as_str(), "********");
                }
            }
            transcript.lock().push_str(&line);
            transcript.lock().push('\n');
            match level {
                "error" => ctx.error(line),
                _ => ctx.info(line),
            }
        };
        let out = async {
            if let Some(stream) = stream {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    handle(line, "info", &ctx, &transcript);
                }
            }
        };
        let errs = async {
            if let Some(stream) = err_stream {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    handle(line, "error", &ctx, &transcript);
                }
            }
        };
        tokio::join!(out, errs);
    };

    let streaming = pump(
        stdout,
        stderr,
        ctx.clone(),
        transcript.clone(),
        secret_values,
    );
    let waiting = async {
        tokio::select! {
            status = child.wait() => Some(status),
            _ = ctx.token().cancelled() => {
                // Cancelling the job kills the process; `kill_on_drop` is the
                // backstop if this somehow does not land.
                let _ = child.kill().await;
                None
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_s)) => {
                let _ = child.kill().await;
                Some(Err(std::io::Error::other(format!(
                    "the playbook was still running after {timeout_s}s and was stopped"
                ))))
            }
        }
    };
    let (status, ()) = tokio::join!(waiting, streaming);

    let text = transcript.lock().clone();
    let recap = parse_recap(&text);
    let cancelled = status.is_none();
    let exit = match status {
        Some(Ok(status)) => Some(status),
        Some(Err(err)) => {
            ctx.error(err.to_string());
            None
        }
        None => None,
    };

    for (device, host) in runnable.iter().zip(&names) {
        let result = match recap.get(host) {
            Some(counts) => {
                let state = if counts.unreachable > 0 {
                    DeviceState::Unreached
                } else if counts.failed > 0 {
                    DeviceState::Failed
                } else {
                    DeviceState::Ok
                };
                DeviceResult {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    error: (state != DeviceState::Ok).then(|| {
                        format!(
                            "Ansible reported failed={} unreachable={} for `{host}`",
                            counts.failed, counts.unreachable
                        )
                    }),
                    steps: vec![StepResult {
                        name: "ansible-playbook".into(),
                        state: match state {
                            DeviceState::Ok if counts.changed > 0 => "ok".into(),
                            DeviceState::Ok => "ok".into(),
                            _ => "failed".into(),
                        },
                        detail: format!(
                            "ok={} changed={} unreachable={} failed={} skipped={}",
                            counts.ok,
                            counts.changed,
                            counts.unreachable,
                            counts.failed,
                            counts.skipped
                        ),
                        outputs: Vec::new(),
                    }],
                    ..DeviceResult::default()
                }
                .with_state(state)
            }
            None => {
                let reason = if cancelled {
                    "the run was stopped before Ansible reported on this host".to_string()
                } else {
                    format!(
                        "`{host}` is not in Ansible's PLAY RECAP: the playbook's own `hosts:` \
                         pattern or a `--limit` left it out, so nothing was done to it"
                    )
                };
                DeviceResult {
                    device_id: device.id.clone(),
                    name: device.name.clone(),
                    reason: Some(reason),
                    ..DeviceResult::default()
                }
                .with_state(DeviceState::Skipped)
            }
        };
        results.push(result);
    }

    if let Some(exit) = exit
        && !exit.success()
    {
        ctx.warn(format!(
            "ansible-playbook exited with {}",
            exit.code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "a signal".into())
        ));
    }
    // The leases and the temporary directory go here, together.
    drop(leases);
    drop(directory);
    Ok(results)
}

/// Write a file only this user can read.
fn write_private(path: &Path, contents: &str) -> AppResult<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| {
        AppError::internal(format!(
            "the temporary inventory {} could not be created: {err}",
            path.display()
        ))
    })?;
    file.write_all(contents.as_bytes())
        .map_err(|err| AppError::internal(format!("the temporary inventory: {err}")))?;
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
pub struct Counts {
    pub ok: u32,
    pub changed: u32,
    pub unreachable: u32,
    pub failed: u32,
    pub skipped: u32,
}

/// Per-host counts out of Ansible's `PLAY RECAP`.
///
/// This is the only per-host answer the default output gives, which is why the
/// caveats say a custom stdout callback means no per-host result.
pub fn parse_recap(text: &str) -> HashMap<String, Counts> {
    let mut found = HashMap::new();
    let mut in_recap = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("PLAY RECAP") {
            in_recap = true;
            continue;
        }
        if !in_recap || line.is_empty() {
            continue;
        }
        let Some((host, rest)) = line.split_once(':') else {
            continue;
        };
        let host = host.trim();
        if host.is_empty() || !rest.contains('=') {
            continue;
        }
        let mut counts = Counts::default();
        for pair in rest.split_whitespace() {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let value: u32 = value.trim().parse().unwrap_or(0);
            match key {
                "ok" => counts.ok = value,
                "changed" => counts.changed = value,
                "unreachable" => counts.unreachable = value,
                "failed" => counts.failed = value,
                "skipped" => counts.skipped = value,
                _ => {}
            }
        }
        found.insert(host.to_string(), counts);
    }
    found
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("ansible.status", |app: App, _: NoParams| async move {
        status(&app).await
    });

    router.add(
        "ansible.export_playbook",
        |app: App, params: ExportParams| async move { export(&app, params).await },
    );

    router.add("ansible.run", |app: App, params: RunParams| async move {
        run(&app, params).await
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_recap_is_read_per_host() {
        let text = "\nPLAY [all] ****\n\nTASK [config] ****\nok: [SW1]\n\nPLAY RECAP ****\n\
            SW1                        : ok=2    changed=1    unreachable=0    failed=0    skipped=0    rescued=0    ignored=0\n\
            SW2                        : ok=0    changed=0    unreachable=1    failed=0    skipped=0    rescued=0    ignored=0\n";
        let recap = parse_recap(text);
        assert_eq!(
            recap.get("SW1"),
            Some(&Counts {
                ok: 2,
                changed: 1,
                ..Counts::default()
            })
        );
        assert_eq!(recap["SW2"].unreachable, 1);
        assert_eq!(recap.len(), 2);
        // Lines before the recap are not hosts.
        assert!(!recap.contains_key("ok"));
    }

    #[test]
    fn host_names_are_ansible_safe_and_unique() {
        let devices = vec![
            Device {
                id: "a".into(),
                name: "SW 1".into(),
                ..Device::default()
            },
            Device {
                id: "b".into(),
                name: "SW/1".into(),
                ..Device::default()
            },
            Device {
                id: "c".into(),
                name: "core-1.lab".into(),
                ..Device::default()
            },
        ];
        assert_eq!(host_names(&devices), vec!["SW_1", "SW_1_2", "core-1.lab"]);
    }

    #[test]
    fn arguments_that_would_break_the_promises_are_refused() {
        assert!(refuse_argument("-i").is_some());
        assert!(refuse_argument("--inventory=hosts.yml").is_some());
        assert!(refuse_argument("--ask-pass").is_some());
        assert!(refuse_argument("--limit").is_some());
        assert!(refuse_argument("--diff").is_none());
        assert!(refuse_argument("-vv").is_none());
    }
}
