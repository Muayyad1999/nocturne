//! Structured data Nocturne has already read, and stored configurations.

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use super::boundary;
use super::{DEVICE_ARG, Tool, ToolCtx, ToolOk, params, schema};
use crate::shape::{self, Table};
use crate::view::Link;

const KINDS: &[&str] = &[
    "facts",
    "interfaces",
    "neighbors",
    "vlans",
    "mac_table",
    "arp",
    "routes",
    "health",
    "inventory",
];

// ----------------------------------------------------------- collect_data

pub const COLLECT_DATA: Tool = Tool {
    name: "collect_data",
    title: "Collect data",
    description: "Read structured data from devices now, over their CLI, and store it: facts, \
                  interfaces, neighbours, VLANs, MAC table, ARP table, routes, health, hardware \
                  inventory. Use it when what is stored is too old to answer the question; use \
                  get_collected first to see what is already there. Takes time — one session \
                  per device.",
    brief: "Read fresh structured data (interfaces, VLANs, neighbours, routes …) from devices.",
    required: &[boundary::COLLECT_RUN, boundary::JOBS_GET],
    optional: &[],
    core: false,
    touches_device: true,
    parallel_safe: false,
    schema: || {
        schema(
            json!({
                "devices": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 50,
                            "description": "Device names, addresses or ids."},
                "kinds": {"type": "array", "items": {"type": "string", "enum": KINDS},
                          "description": "What to read. Everything the device type supports when left out."}
            }),
            &["devices"],
        )
    },
    run: |ctx, input| Box::pin(collect_data(ctx, input)),
};

#[derive(Deserialize)]
struct CollectParams {
    devices: Vec<String>,
    #[serde(default)]
    kinds: Option<Vec<String>>,
}

async fn collect_data(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: CollectParams = params(input)?;
    let mut request = json!({"device_ids": input.devices, "origin": "assistant"});
    if let Some(kinds) = &input.kinds {
        request["kinds"] = json!(kinds);
    }
    let started = ctx.rpc(boundary::COLLECT_RUN, request).await?;
    let job_id = shape::text_of(&started, "job_id");
    ctx.audit(
        "assistant.collect",
        None,
        format!(
            "Assistant collected data from {} device(s)",
            input.devices.len()
        ),
        json!({"devices": input.devices, "kinds": input.kinds, "job_id": job_id}),
    );
    if job_id.is_empty() {
        return Ok(ToolOk::new(
            "The collection was started.",
            "collection started",
        ));
    }
    let job = ctx.wait_for_job(&job_id, "the collection").await?;
    let result = &job["result"];
    let mut table = Table::new(&["device", "collected", "problems"], ctx.caps().rows);
    for outcome in result["devices"].as_array().unwrap_or(&Vec::new()) {
        let problems = match outcome["error"].as_str() {
            Some(error) => format!("failed: {error}"),
            None => outcome["problems"]
                .as_array()
                .map(|problems| {
                    problems
                        .iter()
                        .map(|problem| {
                            format!(
                                "{}: {}",
                                shape::text_of(problem, "kind"),
                                shape::text_of(problem, "message")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default(),
        };
        table.push([
            shape::text_of(outcome, "name"),
            outcome["collected"]
                .as_array()
                .map(|kinds| {
                    kinds
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
            problems,
        ]);
    }
    // A problem is the collector's account of what a device answered, and
    // may quote it: it travels as data like the rest of what a device said.
    let body = if table.is_empty() {
        format!(
            "The collection finished and said nothing per device:\n\n{}",
            shape::data_block(
                "what the collection job reported",
                &shape::one_line(&result.to_string(), 400)
            )
        )
    } else {
        format!(
            "Collected. Read it back with get_collected.\n\n{}",
            shape::data_block(
                "what was collected, and what could not be",
                &table.render_within(ctx.caps().chars)
            )
        )
    };
    Ok(ToolOk::new(
        body,
        format!("{} device(s)", input.devices.len()),
    ))
}

// ---------------------------------------------------------- get_collected

pub const GET_COLLECTED: Tool = Tool {
    name: "get_collected",
    title: "Stored data",
    description: "What was last read from a device, without talking to it: interfaces, \
                  neighbours, VLANs, MAC table, ARP, routes, facts, health or hardware \
                  inventory. Always says when it was collected — say so too if the answer \
                  depends on it. Ask for `previous: true` to see the generation before, which \
                  is how you answer 'what changed since last time'.",
    brief: "The interfaces, VLANs, neighbours or routes last read from a device, with when they \
            were read.",
    required: &[boundary::COLLECT_GET],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "device": {"type": "string", "description": DEVICE_ARG},
                "kind": {"type": "string", "enum": KINDS},
                "previous": {"type": "boolean", "description": "The generation before the latest."},
                "filter": {"type": "string", "description": "Keep only rows containing this text (an interface name, a VLAN id, a MAC …)."}
            }),
            &["device", "kind"],
        )
    },
    run: |ctx, input| Box::pin(get_collected(ctx, input)),
};

#[derive(Deserialize)]
struct CollectedParams {
    device: String,
    kind: String,
    #[serde(default)]
    previous: bool,
    #[serde(default)]
    filter: Option<String>,
}

async fn get_collected(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: CollectedParams = params(input)?;
    let stored = ctx
        .rpc(
            boundary::COLLECT_GET,
            json!({"id": input.device, "kind": input.kind, "previous": input.previous}),
        )
        .await?;
    if stored.is_null() {
        return Ok(ToolOk::new(
            format!(
                "Nothing of kind `{}` has been collected from `{}`{}. Use collect_data to read it now.",
                input.kind,
                input.device,
                if input.previous {
                    " before the latest generation"
                } else {
                    ""
                }
            ),
            "nothing stored",
        ));
    }
    let device_id = shape::text_of(&stored, "device_id");
    ctx.link(Link::device(&device_id));
    let rows: Vec<Value> = stored["rows"].as_array().cloned().unwrap_or_default();
    let rows: Vec<Value> = match &input.filter {
        Some(filter) if !filter.trim().is_empty() => rows
            .into_iter()
            .filter(|row| {
                row.to_string()
                    .to_lowercase()
                    .contains(&filter.to_lowercase())
            })
            .collect(),
        _ => rows,
    };
    let header = format!(
        "{} from device {device_id}, collected {} by {}.",
        input.kind,
        stored["collected_at"]
            .as_i64()
            .map(|at| shape::iso_ago(at, now_ms()))
            .unwrap_or_else(|| "at an unknown time".into()),
        shape::text_of(&stored, "source"),
    );
    let body = shape::data_block(
        &format!("{} of device {device_id}", input.kind),
        &render_rows(&input.kind, &rows, ctx.caps().rows),
    );
    Ok(ToolOk::new(
        format!("{header}\n\n{body}"),
        format!("{} {} row(s)", rows.len(), input.kind),
    ))
}

/// One table per kind, with the columns that matter for that kind.
fn render_rows(kind: &str, rows: &[Value], limit: usize) -> String {
    let columns: &[(&str, &str)] = match kind {
        "interfaces" => &[
            ("name", "name"),
            ("description", "description"),
            ("admin", "admin_up"),
            ("oper", "oper_up"),
            ("mode", "mode"),
            ("vlan", "vlan"),
            ("ipv4", "ipv4"),
            ("speed", "speed_bps"),
        ],
        "neighbors" => &[
            ("local", "local_interface"),
            ("device", "device"),
            ("remote", "remote_interface"),
            ("platform", "platform"),
            ("address", "address"),
            ("via", "protocol"),
        ],
        "vlans" => &[
            ("id", "id"),
            ("name", "name"),
            ("status", "status"),
            ("interfaces", "interfaces"),
        ],
        "mac_table" => &[
            ("vlan", "vlan"),
            ("mac", "mac"),
            ("interface", "interface"),
            ("kind", "kind"),
        ],
        "arp" => &[
            ("ip", "ip"),
            ("mac", "mac"),
            ("interface", "interface"),
            ("age", "age"),
        ],
        "routes" => &[
            ("prefix", "prefix"),
            ("via", "protocol"),
            ("next hop", "next_hop"),
            ("interface", "interface"),
            ("metric", "metric"),
            ("vrf", "vrf"),
        ],
        "inventory" => &[
            ("name", "name"),
            ("description", "description"),
            ("part", "part_id"),
            ("serial", "serial"),
        ],
        // Facts and health are one row about the device: a list reads better.
        _ => {
            return rows
                .iter()
                .map(|row| {
                    row.as_object()
                        .map(|map| {
                            map.iter()
                                .filter(|(_, value)| !value.is_null())
                                .map(|(key, value)| {
                                    format!("{key}: {}", shape::one_line(&render_cell(value), 120))
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_else(|| row.to_string())
                })
                .collect::<Vec<_>>()
                .join("\n---\n");
        }
    };
    let headers: Vec<&str> = columns.iter().map(|(header, _)| *header).collect();
    let mut table =
        Table::new(&headers, limit).narrow_with("use `filter` to pick out the rows you need");
    for row in rows {
        table.push(
            columns
                .iter()
                .map(|(_, field)| render_cell(row.get(field).unwrap_or(&Value::Null))),
        );
    }
    table.render()
}

fn render_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "up" } else { "down" }.to_string(),
        Value::Array(items) => items
            .iter()
            .map(render_cell)
            .filter(|cell| !cell.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------- get_config

pub const GET_CONFIG: Tool = Tool {
    name: "get_config",
    title: "Read configuration",
    description: "The stored configuration of a device (the latest backup, not a live read). \
                  Secrets are replaced by stable placeholders such as `<secret:3>`: the same \
                  value always gets the same placeholder, so you can say two devices share a \
                  key, and you can reuse a placeholder in a change — it is put back for real \
                  before anything is sent. Use `section` on a large configuration: whole \
                  configurations eat the context you need for the answer.",
    brief: "The stored configuration of a device, secrets replaced by placeholders. Use \
            `section` to read only part of it.",
    required: &[boundary::CONFIG_LATEST],
    optional: &[],
    core: true,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "device": {"type": "string", "description": DEVICE_ARG},
                "kind": {"type": "string", "enum": ["running", "startup"]},
                "section": {"type": "string", "description": "Keep only top-level sections whose header contains this text, with their sub-lines (e.g. `interface GigabitEthernet0/1`, `router bgp`, `vlan`)."}
            }),
            &["device"],
        )
    },
    run: |ctx, input| Box::pin(get_config(ctx, input)),
};

#[derive(Deserialize)]
struct ConfigParams {
    device: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    section: Option<String>,
}

async fn get_config(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: ConfigParams = params(input)?;
    let kind = input.kind.unwrap_or_else(|| "running".into());
    let stored = ctx
        .rpc(
            boundary::CONFIG_LATEST,
            json!({"device_id": input.device, "kind": kind, "redact": true}),
        )
        .await
        .map_err(|err| {
            if err.code == "not_found" {
                AppError::not_found(format!(
                    "no stored {kind} configuration for `{}` — back it up first with backup_config",
                    input.device
                ))
            } else {
                err
            }
        })?;
    let version = &stored["version"];
    let device_id = shape::text_of(version, "device_id");
    if !device_id.is_empty() {
        ctx.link(Link::device(&device_id));
    }
    let text = shape::text_of(&stored, "text");
    let (text, note) = match &input.section {
        Some(section) if !section.trim().is_empty() => {
            let filtered = sections_matching(&text, section);
            if filtered.trim().is_empty() {
                return Ok(ToolOk::new(
                    format!(
                        "The {kind} configuration of `{}` has no section matching `{section}`.",
                        input.device
                    ),
                    "no such section",
                ));
            }
            (filtered, format!(" (sections matching `{section}`)"))
        }
        _ => (text, String::new()),
    };
    let header = format!(
        "{kind} configuration of `{}`{note}, backed up {} ({} lines in full).",
        input.device,
        version["last_seen"]
            .as_i64()
            .or_else(|| version["first_seen"].as_i64())
            .map(|at| shape::iso_ago(at, now_ms()))
            .unwrap_or_else(|| "at an unknown time".into()),
        version["lines"].as_u64().unwrap_or_default(),
    );
    Ok(ToolOk::new(
        format!(
            "{header}\n\n{}",
            shape::data_block(
                &format!("{} {kind}-config", input.device),
                &shape::cap_text(&text, ctx.caps().chars)
            )
        ),
        format!("{} lines", text.lines().count()),
    ))
}

/// Top-level blocks whose header contains `needle`, with their indented lines.
fn sections_matching(config: &str, needle: &str) -> String {
    let needle = needle.to_lowercase();
    let mut out = String::new();
    let mut keeping = false;
    for line in config.lines() {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            keeping = line.to_lowercase().contains(&needle);
        }
        if keeping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// --------------------------------------------------------- search_configs

pub const SEARCH_CONFIGS: Tool = Tool {
    name: "search_configs",
    title: "Search configurations",
    description: "Search the stored configuration of every device for a line. This is how you \
                  answer 'which switches still have VLAN 40', 'who points at the old NTP \
                  server', 'where is this ACL applied'. Returns the matching line with the \
                  device and the section it sits in. Always redacted.",
    brief: "Search every stored configuration for a line and return device, section and the \
            matching line.",
    required: &[boundary::CONFIG_SEARCH],
    optional: &[],
    core: true,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "query": {"type": "string", "description": "Text, or a regular expression when `regex` is true."},
                "regex": {"type": "boolean"},
                "site": {"type": "string", "description": "Search only this site."},
                "role": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500}
            }),
            &["query"],
        )
    },
    run: |ctx, input| Box::pin(search_configs(ctx, input)),
};

#[derive(Deserialize)]
struct SearchParams {
    query: String,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    site: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn search_configs(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: SearchParams = params(input)?;
    let limit = input.limit.unwrap_or(ctx.caps().rows).min(500);
    let found = ctx
        .rpc(
            boundary::CONFIG_SEARCH,
            json!({
                "query": input.query,
                "regex": input.regex,
                "filter": {"site": input.site, "role": input.role},
                "limit": limit,
            }),
        )
        .await?;
    let matches = found["matches"].as_array().cloned().unwrap_or_default();
    let mut table = Table::new(&["device", "line", "section"], ctx.caps().rows)
        .narrow_with("make the query more specific, or filter by site or role");
    for found in &matches {
        table.push([
            shape::text_of(found, "name"),
            shape::text_of(found, "line"),
            found["section"]
                .as_array()
                .map(|path| {
                    path.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default(),
        ]);
    }
    let header = format!(
        "{} match(es) on {} of {} device(s) searched{}.",
        matches.len(),
        found["devices_matched"].as_u64().unwrap_or_default(),
        found["devices_searched"].as_u64().unwrap_or_default(),
        if found["truncated"].as_bool() == Some(true) {
            " — the search itself stopped at its limit, so there may be more"
        } else {
            ""
        }
    );
    Ok(ToolOk::new(
        format!(
            "{header}\n\n{}",
            shape::data_block("matching configuration lines", &table.render())
        ),
        format!("{} match(es)", matches.len()),
    ))
}

// ------------------------------------------------------------ diff_config

pub const DIFF_CONFIG: Tool = Tool {
    name: "diff_config",
    title: "Compare configuration",
    description: "What changed in a device's configuration between its last two backups (or \
                  between two versions you name). Use it for 'what changed on this switch'.",
    brief: "What changed between a device's last two stored configurations.",
    required: &[boundary::CONFIG_DIFF],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "device": {"type": "string", "description": DEVICE_ARG},
                "kind": {"type": "string", "enum": ["running", "startup"]},
                "a": {"type": "integer", "description": "Version id of the older side, if you are comparing two specific versions."},
                "b": {"type": "integer", "description": "Version id of the newer side."}
            }),
            &[],
        )
    },
    run: |ctx, input| Box::pin(diff_config(ctx, input)),
};

#[derive(Deserialize)]
struct DiffParams {
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    a: Option<i64>,
    #[serde(default)]
    b: Option<i64>,
}

async fn diff_config(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: DiffParams = params(input)?;
    let request = match (input.a, input.b, &input.device) {
        (Some(a), Some(b), _) => json!({"a": a, "b": b, "redact": true}),
        (_, _, Some(device)) => {
            json!({"device_id": device, "kind": input.kind.unwrap_or_else(|| "running".into()), "redact": true})
        }
        _ => {
            return Err(AppError::invalid(
                "give either a device, or the two version ids to compare",
            ));
        }
    };
    let diff = ctx.rpc(boundary::CONFIG_DIFF, request).await?;
    let lines = diff["diff"]["lines"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let changed: Vec<String> = lines
        .iter()
        .filter(|line| shape::text_of(line, "tag") != " ")
        .map(|line| {
            format!(
                "{}{}",
                shape::text_of(line, "tag"),
                shape::text_of(line, "text")
            )
        })
        .collect();
    let body = if changed.is_empty() {
        "No difference.".to_string()
    } else {
        shape::data_block(
            "configuration diff",
            &shape::cap_text(&changed.join("\n"), ctx.caps().chars),
        )
    };
    Ok(ToolOk::new(
        format!(
            "{} line(s) added, {} removed.\n\n{body}",
            diff["diff"]["added"].as_u64().unwrap_or_default(),
            diff["diff"]["removed"].as_u64().unwrap_or_default(),
        ),
        format!("{} changed line(s)", changed.len()),
    ))
}

// ---------------------------------------------------------- backup_config

pub const BACKUP_CONFIG: Tool = Tool {
    name: "backup_config",
    title: "Back up configuration",
    description: "Fetch and store the current configuration of devices. Use it before \
                  proposing a change if the stored copy is old, or when the user asks for a \
                  backup. Reports which devices were backed up, which were unchanged and which \
                  could not be reached.",
    brief: "Fetch and store the current configuration of devices.",
    required: &[boundary::CONFIG_BACKUP, boundary::JOBS_GET],
    optional: &[],
    core: false,
    touches_device: true,
    parallel_safe: false,
    schema: || {
        schema(
            json!({"devices": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 50,
                               "description": "Device names, addresses or ids."}}),
            &["devices"],
        )
    },
    run: |ctx, input| Box::pin(backup_config(ctx, input)),
};

#[derive(Deserialize)]
struct BackupParams {
    devices: Vec<String>,
}

async fn backup_config(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: BackupParams = params(input)?;
    let started = ctx
        .rpc(
            boundary::CONFIG_BACKUP,
            json!({"device_ids": input.devices}),
        )
        .await?;
    ctx.audit(
        "assistant.backup",
        None,
        format!(
            "Assistant backed up the configuration of {} device(s)",
            input.devices.len()
        ),
        json!({"devices": input.devices}),
    );
    let job_id = shape::text_of(&started, "job_id");
    if job_id.is_empty() {
        return Ok(ToolOk::new("The backup was started.", "backup started"));
    }
    let job = ctx.wait_for_job(&job_id, "the backup").await?;
    let result = &job["result"];
    let failed = result["failed"].as_array().cloned().unwrap_or_default();
    let skipped = result["skipped"].as_array().cloned().unwrap_or_default();
    let mut body = shape::fields(&[
        (
            "backed up",
            result["backed_up"].as_u64().unwrap_or_default().to_string(),
        ),
        (
            "unchanged",
            result["unchanged"].as_u64().unwrap_or_default().to_string(),
        ),
        (
            "changed",
            result["changed"].as_u64().unwrap_or_default().to_string(),
        ),
    ]);
    for (label, rows) in [("skipped", &skipped), ("failed", &failed)] {
        for row in rows.iter().take(ctx.caps().rows) {
            body.push_str(&format!(
                "\n{label}: {} — {}{}",
                shape::text_of(row, "device_id"),
                shape::text_of(row, "reason"),
                shape::text_of(row, "error"),
            ));
        }
    }
    Ok(ToolOk::new(
        body,
        format!(
            "{} backed up",
            result["backed_up"].as_u64().unwrap_or_default()
        ),
    ))
}

// ----------------------------------------------------- compliance_results

pub const COMPLIANCE_RESULTS: Tool = Tool {
    name: "compliance_results",
    title: "Compliance findings",
    description: "Rules that devices fail in the latest compliance run: which device, which \
                  rule, how bad, and what the rule expected. Use it for 'are we compliant', \
                  'what is wrong across the estate', or before an audit.",
    brief: "Compliance rules devices currently fail.",
    required: &[boundary::COMPLIANCE_RESULTS],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "device": {"type": "string", "description": DEVICE_ARG},
                "severity": {"type": "string", "enum": ["info", "low", "medium", "high", "critical"]},
                "status": {"type": "string", "enum": ["fail", "pass", "not_applicable", "not_evaluated"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            }),
            &[],
        )
    },
    run: |ctx, input| Box::pin(compliance_results(ctx, input)),
};

#[derive(Deserialize)]
struct ComplianceParams {
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn compliance_results(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: ComplianceParams = params(input)?;
    let found = ctx
        .rpc(
            boundary::COMPLIANCE_RESULTS,
            json!({
                "device_id": input.device,
                "severity": input.severity,
                "status": input.status.unwrap_or_else(|| "fail".into()),
                "limit": input.limit.unwrap_or(ctx.caps().rows).min(200),
            }),
        )
        .await?;
    let findings = found["findings"].as_array().cloned().unwrap_or_default();
    let mut table = Table::new(&["device", "rule", "severity", "detail"], ctx.caps().rows)
        .narrow_with("filter by device or severity");
    for finding in &findings {
        table.push([
            shape::text_of(finding, "device_name"),
            format!(
                "{} {}",
                shape::text_of(finding, "rule_id"),
                shape::text_of(finding, "title")
            ),
            shape::text_of(finding, "severity"),
            shape::text_of(finding, "detail"),
        ]);
    }
    let summary = &found["summary"];
    Ok(ToolOk::new(
        format!(
            "{} finding(s). Across the run: {} device(s), {} rule(s), {} passed, {} failed.\n\n{}",
            findings.len(),
            summary["devices"].as_u64().unwrap_or_default(),
            summary["rules"].as_u64().unwrap_or_default(),
            summary["pass"].as_u64().unwrap_or_default(),
            summary["fail"].as_u64().unwrap_or_default(),
            shape::data_block("compliance findings", &table.render())
        ),
        format!("{} finding(s)", findings.len()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_filter_keeps_the_block_and_its_children() {
        let config = "hostname SW1\ninterface Gi0/1\n description uplink\n switchport mode trunk\ninterface Gi0/2\n description desk\nvlan 10\n name users\n";
        let filtered = sections_matching(config, "interface Gi0/1");
        assert_eq!(
            filtered,
            "interface Gi0/1\n description uplink\n switchport mode trunk\n"
        );
        assert!(sections_matching(config, "router bgp").is_empty());
        assert_eq!(
            sections_matching(config, "interface").lines().count(),
            5,
            "both interfaces and their children"
        );
    }

    #[test]
    fn rows_render_as_the_table_that_kind_deserves() {
        let rows = vec![json!({
            "name": "GigabitEthernet0/1", "description": "uplink", "admin_up": true,
            "oper_up": false, "mode": "trunk", "ipv4": ["10.0.0.1/24"], "speed_bps": 1000000000
        })];
        let rendered = render_rows("interfaces", &rows, 50);
        assert!(rendered.contains("GigabitEthernet0/1"), "{rendered}");
        assert!(rendered.contains("up"), "{rendered}");
        assert!(rendered.contains("down"), "{rendered}");
        assert!(rendered.contains("10.0.0.1/24"), "{rendered}");
        // Facts are a device, not a table.
        let facts = vec![json!({"hostname": "SW1", "vendor": "Cisco", "serial": null})];
        let rendered = render_rows("facts", &facts, 50);
        assert!(rendered.contains("hostname: SW1"), "{rendered}");
        assert!(!rendered.contains("serial"), "{rendered}");
    }
}
