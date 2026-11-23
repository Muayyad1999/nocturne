//! Work Nocturne did on somebody's instruction: what a discovery scan found,
//! and how the saved automation tasks last ran.
//!
//! Both are read only. Adding what a scan found to the inventory, and running
//! a task, stay a person's click: neither method is on the allow-list.
//! Starting a scan is in `records.rs`, because it asks first.

use nocturne_app::error::AppResult;
use nocturne_app::util::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use super::boundary;
use super::{Tool, ToolCtx, ToolOk, not_in_build, params, schema, unread};
use crate::shape::{self, Table};
use crate::view::{Link, LinkKind};

// ------------------------------------------------------- discovery_results

pub const DISCOVERY_RESULTS: Tool = Tool {
    name: "discovery_results",
    title: "Discovery results",
    description: "What discovery scans found. Without a scan id: the recent scans and what the \
                  newest one found. With one: that scan's finds — address, what proved it \
                  alive, the name and device type where something identified them, open ports, \
                  and whether it is already in the inventory. By default only what is not in \
                  the inventory yet. A find with no device type was not identified: do not \
                  guess one from a port or a vendor. A scan that is still running shows what it \
                  has so far. Adding a find to the inventory is the user's click.",
    brief: "What a discovery scan found, and what is not in the inventory yet.",
    required: &[boundary::DISCOVERY_SCANS, boundary::DISCOVERY_SCAN_GET],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "scan_id": {"type": "string", "description": "A scan's id. Default: the newest scan."},
                "only_new": {"type": "boolean", "description": "Leave out finds already in the inventory. Default true."},
                "text": {"type": "string", "description": "Matched against address, names, vendor, device type and banners."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            }),
            &[],
        )
    },
    run: |ctx, input| Box::pin(discovery_results(ctx, input)),
};

#[derive(Deserialize)]
struct DiscoveryParams {
    #[serde(default)]
    scan_id: Option<String>,
    #[serde(default)]
    only_new: Option<bool>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// A scan in words: what it was asked, how far it got and what it counted.
fn scan_text(scan: &Value) -> String {
    let state = shape::text_of(scan, "state");
    shape::fields(&[
        ("scan", shape::text_of(scan, "id")),
        ("kind", shape::text_of(scan, "kind")),
        ("targets", shape::text_of(scan, "targets")),
        ("note", shape::text_of(scan, "note")),
        (
            "state",
            match state.as_str() {
                "running" => "running — what follows is what it has found so far".to_string(),
                _ => state,
            },
        ),
        ("error", shape::text_of(scan, "error")),
        (
            "started",
            scan["started_at"]
                .as_i64()
                .map(shape::iso)
                .unwrap_or_default(),
        ),
        (
            "finished",
            scan["finished_at"]
                .as_i64()
                .map(shape::iso)
                .unwrap_or_default(),
        ),
        (
            "covered",
            format!(
                "{} of {} address(es) probed",
                shape::text_of(scan, "probed"),
                shape::text_of(scan, "planned")
            ),
        ),
        (
            "found",
            format!(
                "{} answered, {} identified as a known device type, {} not in the inventory",
                shape::text_of(scan, "found"),
                shape::text_of(scan, "identified"),
                shape::text_of(scan, "new")
            ),
        ),
        (
            "public address space",
            if scan["allow_public"].as_bool() == Some(true) {
                "included".into()
            } else {
                String::new()
            },
        ),
    ])
}

async fn discovery_results(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: DiscoveryParams = params(input)?;
    let rows = input.limit.unwrap_or(ctx.caps().rows).min(ctx.caps().rows);
    let only_new = input.only_new.unwrap_or(true);
    let mut out = Vec::new();

    let scan_id = match input.scan_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            let scans = ctx
                .rpc(boundary::DISCOVERY_SCANS, json!({"limit": 10}))
                .await?;
            let scans = scans.as_array().cloned().unwrap_or_default();
            let Some(newest) = scans.first() else {
                return Ok(ToolOk::new(
                    "No discovery scan has been run yet, so there is nothing found to report.",
                    "no scans yet",
                ));
            };
            let mut table = Table::new(
                &[
                    "started", "kind", "targets", "state", "probed", "found", "new", "scan id",
                ],
                10,
            );
            for scan in &scans {
                table.push([
                    scan["started_at"]
                        .as_i64()
                        .map(shape::iso)
                        .unwrap_or_default(),
                    shape::text_of(scan, "kind"),
                    shape::text_of(scan, "targets"),
                    shape::text_of(scan, "state"),
                    format!(
                        "{}/{}",
                        shape::text_of(scan, "probed"),
                        shape::text_of(scan, "planned")
                    ),
                    shape::text_of(scan, "found"),
                    shape::text_of(scan, "new"),
                    shape::text_of(scan, "id"),
                ]);
            }
            out.push(format!(
                "The {} most recent scan(s), newest first:\n{}",
                scans.len(),
                table.render()
            ));
            shape::text_of(newest, "id")
        }
    };

    let found = ctx
        .rpc(
            boundary::DISCOVERY_SCAN_GET,
            json!({
                "scan_id": scan_id, "only_new": only_new, "query": input.text, "limit": rows,
            }),
        )
        .await?;
    let scan = &found["scan"];
    let hosts = found["hosts"].as_array().cloned().unwrap_or_default();
    // Finds matching the filters before the page was cut.
    let total = found["total"]
        .as_u64()
        .map(|total| total as usize)
        .unwrap_or(hosts.len());
    out.push(scan_text(scan));

    let which = if only_new {
        "not in the inventory (and not ignored)"
    } else {
        "in all"
    };
    if hosts.is_empty() {
        out.push(format!(
            "Finds {which}: none{}.",
            if shape::text_of(scan, "state") == "running" {
                " so far — the scan is still running"
            } else {
                ""
            }
        ));
    } else {
        let mut table = Table::new(
            &[
                "address",
                "name",
                "device type",
                "alive via",
                "vendor",
                "open ports",
                "in inventory",
            ],
            rows,
        )
        .narrow_with("add a text filter");
        for host in &hosts {
            let snmp_name = shape::text_of(&host["snmp"], "sys_name");
            table.push([
                shape::text_of(host, "address"),
                // What the device called itself, else what DNS calls it.
                match (snmp_name.is_empty(), shape::text_of(host, "hostname")) {
                    (false, _) => snmp_name,
                    (true, hostname) => hostname,
                },
                match shape::text_of(&host["profile"], "id") {
                    id if id.is_empty() => "not identified".to_string(),
                    id => format!("{id} (by {})", shape::text_of(&host["profile"], "method")),
                },
                shape::text_of(host, "via"),
                shape::text_of(host, "vendor"),
                host["open_ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|port| shape::text_of(port, "port"))
                    .collect::<Vec<_>>()
                    .join(","),
                match (
                    shape::text_of(host, "device_name"),
                    host["ignored"].as_bool(),
                ) {
                    (name, _) if !name.is_empty() => format!("yes: {name}"),
                    (_, Some(true)) => "no (ignored)".to_string(),
                    _ => "no".to_string(),
                },
            ]);
        }
        out.push(format!(
            "Finds {which}: {total}{}.\n{}",
            if total > hosts.len() {
                format!(
                    ", of which the first {} are shown — add a text filter to see others",
                    hosts.len()
                )
            } else {
                String::new()
            },
            table.render()
        ));
    }
    let job_id = shape::text_of(scan, "job_id");
    if !job_id.is_empty() {
        ctx.link(Link::new(LinkKind::Job, job_id));
    }
    Ok(ToolOk::new(
        // Names, banners and vendors are what strangers' machines said.
        shape::data_block(
            "discovery: what answered a scan, in its own words",
            &shape::cap_text(&out.join("\n\n"), ctx.caps().chars),
        ),
        format!("{total} find(s) {which}"),
    ))
}

// --------------------------------------------------------- automation_runs

pub const AUTOMATION_RUNS: Tool = Tool {
    name: "automation_runs",
    title: "Automation",
    description: "The saved automation tasks — what each does, on which devices, when it last \
                  ran and whether that run reached every device without a failure — and the \
                  most recent runs with how many devices succeeded, failed or were never \
                  reached. Read only: you cannot run a task; say which one you would run and \
                  the user can start it. A task that has never run has no result: that is not \
                  a success.",
    brief: "Saved automation tasks and how their last runs went. Read only.",
    required: &[boundary::TASKS_LIST],
    optional: &[boundary::RUNNER_RUNS],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "text": {"type": "string", "description": "Only tasks and runs whose name contains this."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100}
            }),
            &[],
        )
    },
    run: |ctx, input| Box::pin(automation_runs(ctx, input)),
};

#[derive(Deserialize)]
struct AutomationParams {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// What a task does, without quoting a playbook or its variables: those are
/// long, and a variable is exactly where somebody keeps a password.
fn action_text(action: &Value) -> String {
    match action["type"].as_str() {
        Some("commands") => {
            let commands = shape::scalar_list(&action["commands"]);
            format!(
                "read-only commands: {}{}",
                commands
                    .iter()
                    .take(3)
                    .map(|command| shape::one_line(command, 60))
                    .collect::<Vec<_>>()
                    .join("; "),
                match commands.len().saturating_sub(3) {
                    0 => String::new(),
                    more => format!(" (+{more})"),
                }
            )
        }
        Some("playbook") => match shape::text_of(action, "playbook_id") {
            id if id.is_empty() => "a playbook written into the task".to_string(),
            id => format!("playbook {id}"),
        },
        Some("template") => format!(
            "template {} → a change for approval",
            shape::text_of(action, "template_id")
        ),
        Some("ansible") => format!(
            "ansible-playbook {}{}",
            shape::one_line(&shape::text_of(action, "playbook_path"), 80),
            if action["check"].as_bool() == Some(false) {
                " (NOT in check mode: outside the change pipeline)"
            } else {
                " (check mode)"
            }
        ),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn selection_text(select: &Value) -> String {
    let ids = select["device_ids"].as_array().map_or(0, Vec::len);
    let mut parts = Vec::new();
    if ids > 0 {
        parts.push(format!("{ids} named device(s)"));
    }
    if let Some(filter) = select["filter"].as_object() {
        let wanted: Vec<String> = filter
            .iter()
            .filter(|(_, value)| match value {
                Value::Null => false,
                Value::String(text) => !text.is_empty(),
                Value::Array(items) => !items.is_empty(),
                _ => true,
            })
            .map(|(key, value)| {
                format!("{key}={}", shape::one_line(&shape::scalar_text(value), 40))
            })
            .collect();
        if !wanted.is_empty() {
            parts.push(format!("devices where {}", wanted.join(", ")));
        }
    }
    let excluded = select["exclude_device_ids"].as_array().map_or(0, Vec::len);
    if parts.is_empty() {
        "no devices selected".to_string()
    } else if excluded > 0 {
        format!(
            "{}, leaving out {excluded} named device(s)",
            parts.join(" + ")
        )
    } else {
        parts.join(" + ")
    }
}

async fn automation_runs(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: AutomationParams = params(input)?;
    let rows = input.limit.unwrap_or(ctx.caps().rows).min(ctx.caps().rows);
    let needle = input
        .text
        .as_deref()
        .map(|text| text.trim().to_lowercase())
        .filter(|text| !text.is_empty());
    let named = |record: &Value| {
        needle.as_ref().is_none_or(|needle| {
            shape::text_of(record, "name")
                .to_lowercase()
                .contains(needle)
        })
    };
    let now = now_ms();

    let tasks = ctx.rpc(boundary::TASKS_LIST, json!({})).await?;
    let tasks: Vec<Value> = tasks
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|task| named(task))
        .collect();
    let mut out = Vec::new();
    if tasks.is_empty() {
        out.push(match &needle {
            Some(needle) => format!("Saved tasks: none whose name contains `{needle}`."),
            None => "Saved tasks: none have been saved.".to_string(),
        });
    } else {
        let mut table = Table::new(
            &["task", "does", "on", "last run", "last result", "id"],
            rows,
        )
        .narrow_with("add a text filter");
        for task in &tasks {
            table.push([
                shape::text_of(task, "name"),
                action_text(&task["action"]),
                selection_text(&task["select"]),
                task["last_run_at"]
                    .as_i64()
                    .map(|at| shape::iso_ago(at, now))
                    .unwrap_or_else(|| "never run".into()),
                match task["last_ok"].as_bool() {
                    Some(true) => "every device ok".to_string(),
                    Some(false) => "a device failed or was not reached".to_string(),
                    // Never run, or a run that has not finished.
                    None => "no result".to_string(),
                },
                shape::text_of(task, "id"),
            ]);
        }
        out.push(format!(
            "Saved tasks ({}):\n{}",
            tasks.len(),
            table.render()
        ));
    }

    out.push(if !ctx.has(boundary::RUNNER_RUNS) {
        not_in_build("Recent runs")
    } else {
        match ctx
            .rpc(boundary::RUNNER_RUNS, json!({"limit": rows.max(20)}))
            .await
        {
            Err(err) => unread("Recent runs", &err),
            Ok(runs) => {
                let runs: Vec<Value> = runs
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|run| named(run))
                    .collect();
                if runs.is_empty() {
                    "Recent runs: none on record.".to_string()
                } else {
                    let mut table = Table::new(
                        &[
                            "started",
                            "run",
                            "kind",
                            "by",
                            "devices",
                            "ok",
                            "failed",
                            "unreached",
                            "skipped",
                            "ended",
                            "run id",
                        ],
                        rows,
                    );
                    for run in &runs {
                        table.push([
                            run["started_at"]
                                .as_i64()
                                .map(shape::iso)
                                .unwrap_or_default(),
                            shape::text_of(run, "name"),
                            format!(
                                "{}{}",
                                shape::text_of(run, "kind"),
                                if run["dry_run"].as_bool() == Some(true) {
                                    " (dry run: nothing sent)"
                                } else {
                                    ""
                                }
                            ),
                            shape::text_of(run, "origin"),
                            shape::text_of(run, "devices"),
                            shape::text_of(run, "ok"),
                            shape::text_of(run, "failed"),
                            shape::text_of(run, "unreached"),
                            shape::text_of(run, "skipped"),
                            match (run["finished_at"].as_i64(), run["cancelled"].as_bool()) {
                                (_, Some(true)) => "cancelled".to_string(),
                                (Some(at), _) => shape::iso(at),
                                (None, _) => "still running".to_string(),
                            },
                            shape::text_of(run, "id"),
                        ]);
                    }
                    // What the feature itself says out loud about a run
                    // that did not go through the change pipeline.
                    let caveats: Vec<String> = runs
                        .iter()
                        .take(rows)
                        .filter_map(|run| {
                            let caveat = shape::one_line(&shape::text_of(run, "caveat"), 240);
                            (!caveat.is_empty()).then(|| {
                                format!(
                                    "  run {}: {caveat}",
                                    shape::one_line(&shape::text_of(run, "id"), 60)
                                )
                            })
                        })
                        .collect();
                    format!(
                        "Recent runs, newest first ({}):\n{}{}",
                        runs.len(),
                        table.render(),
                        if caveats.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\nSaid of these runs by the feature:\n{}",
                                caveats.join("\n")
                            )
                        }
                    )
                }
            }
        }
    });

    Ok(ToolOk::new(
        shape::data_block(
            "automation: saved tasks and recent runs",
            &shape::cap_text(&out.join("\n\n"), ctx.caps().chars),
        ),
        format!("{} task(s)", tasks.len()),
    ))
}
