//! What the traffic is doing: per-class QoS on a device or an interface, and
//! who is filling a link according to the flows the exporters sent.
//!
//! Both read figures Nocturne holds rather than asking a device, and both
//! have answers that are easy to misread: a class with no rate is not a class
//! with no traffic, and a collector that is not listening hears no traffic.
//! So each says what its figures cover before anything else — whether the
//! read worked, whether it has met a real device, how much of the table is
//! shown, what the exporters said about sampling — and every name a device or
//! an exporter supplied travels inside a data block.

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::now_ms;
use serde::Deserialize;
use serde_json::{Value, json};

use super::boundary;
use super::{DEVICE_ARG, Tool, ToolCtx, ToolOk, device_ref, params, schema};
use crate::shape::{self, Table};
use crate::view::Link;

/// A rate in bits per second the way a person says it.
fn bits(bps: f64) -> String {
    let (value, unit) = match bps.abs() {
        rate if rate >= 1e9 => (bps / 1e9, "Gbit/s"),
        rate if rate >= 1e6 => (bps / 1e6, "Mbit/s"),
        rate if rate >= 1e3 => (bps / 1e3, "kbit/s"),
        _ => (bps, "bit/s"),
    };
    format!("{} {unit}", significant(value))
}

/// A count of bytes the way a person says it (powers of ten, as links are
/// sold).
fn bytes(count: u64) -> String {
    let count = count as f64;
    let (value, unit) = match count {
        size if size >= 1e12 => (count / 1e12, "TB"),
        size if size >= 1e9 => (count / 1e9, "GB"),
        size if size >= 1e6 => (count / 1e6, "MB"),
        size if size >= 1e3 => (count / 1e3, "kB"),
        _ => return format!("{count} B"),
    };
    format!("{} {unit}", significant(value))
}

/// Three significant figures, without a trailing `.0`.
fn significant(value: f64) -> String {
    let text = match value.abs() {
        size if size >= 100.0 => format!("{value:.0}"),
        size if size >= 10.0 => format!("{value:.1}"),
        _ => format!("{value:.2}"),
    };
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

// --------------------------------------------------------------- qos_status

pub const QOS_STATUS: Tool = Tool {
    name: "qos_status",
    title: "QoS classes",
    description: "Per-class QoS on a device or one interface, as monitoring last read it: each \
                  class of each attached policy with its configured rate, the traffic in it, its \
                  drops per second (congestion drops apart where the device separates them from \
                  a policer's), the share dropped and the queue depth. Rates are measured \
                  between two reads; `not measured` means the interval could not be (the first \
                  read, a restart, a failed read) or the device does not print that counter, and \
                  is never a zero. `problem_only` keeps the classes losing packets to \
                  congestion: a policer that drops is the policy working, and a class whose \
                  drops were not measured is `unknown`, never among them. It also says whether \
                  each device can be read at all, when it was, whether that read has been \
                  checked against a real device of its type, and how many classes it holds. For \
                  a class's history: get_metrics with `qos.bps`, `qos.drop_ps`, \
                  `qos.queue_drop_ps`, `qos.drop_pct` or `qos.queue_len` and the class's series.",
    brief: "Per-class QoS traffic, drops and queue depth on a device or an interface.",
    required: &[boundary::MONITOR_QOS],
    optional: &[boundary::DEVICES_GET],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "device": {"type": "string", "description": DEVICE_ARG},
                "interface": {"type": "string", "description": "Any spelling (Gi0/1)."},
                "problem_only": {"type": "boolean", "description": "Only classes dropping to congestion."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            }),
            &[],
        )
    },
    run: |ctx, input| Box::pin(qos_status(ctx, input)),
};

#[derive(Deserialize)]
struct QosParams {
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    interface: Option<String>,
    #[serde(default)]
    problem_only: bool,
    #[serde(default)]
    limit: Option<usize>,
}

/// A rate from the table: `not measured` when the field is absent or
/// `null`, because that is a gap and never a zero.
fn measured(row: &Value, key: &str, show: impl Fn(f64) -> String) -> String {
    row[key]
        .as_f64()
        .map(show)
        .unwrap_or_else(|| "not measured".into())
}

/// A counter or a gauge as last read: absent is "not printed", not zero.
fn counted(row: &Value, key: &str) -> String {
    row[key]
        .as_u64()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".into())
}

/// The `dropping` column. Monitoring sets the flag only from a drop rate it
/// measured, so `false` beside two drop rates nobody measured (a first read,
/// a read that failed, a class whose drops the device does not print) is
/// not "not dropping": it is not known.
fn dropping(row: &Value) -> &'static str {
    if row["dropping"].as_bool() == Some(true) {
        "yes"
    } else if row["drop_ps"].as_f64().is_none() && row["queue_drop_ps"].as_f64().is_none() {
        "unknown"
    } else {
        "no"
    }
}

/// What one device's entry in the coverage says, in words. `named` is how
/// the device is called in it.
fn device_coverage(entry: &Value, named: &str, now: i64) -> String {
    let at = entry["at"]
        .as_i64()
        .map(|at| shape::iso_ago(at, now))
        .unwrap_or_else(|| "at an unknown time".into());
    let classes = entry["classes"].as_u64().unwrap_or_default();
    let series = entry["series"].as_u64().unwrap_or_default();
    let error = shape::one_line(&shape::text_of(entry, "error"), 300);
    let mut parts = vec![match entry["state"].as_str() {
        Some("ok") if classes == 0 => format!("read {at}: no policy is attached"),
        Some("ok") => format!("read {at}: {classes} class(es)"),
        Some("failed") => format!(
            "unknown now — the last read failed {at}{}; {}",
            if error.is_empty() {
                String::new()
            } else {
                format!(" ({error})")
            },
            if classes == 0 {
                "no class is held from an earlier read".to_string()
            } else {
                format!(
                    "the {classes} class(es) held are the ones the last read that worked found, \
                     without rates"
                )
            }
        ),
        Some("never") => "unknown — it can be read and has not been yet".into(),
        Some("unsupported") => "unknown — QoS is not read on this device: its device type has \
                                no QoS read, or it has no command line"
            .into(),
        _ => "unknown — monitoring gave no state for it".into(),
    }];
    if entry["state"] == "ok" && !error.is_empty() {
        parts.push(format!("the read left something out: {error}"));
    }
    if entry["series_capped"].as_bool() == Some(true) {
        parts.push(format!(
            "it holds {classes} classes and only {series} have series and alert rules: the rest \
             are listed but not charted or alerted on"
        ));
    }
    if entry["unverified"].as_bool() == Some(true) {
        parts.push(
            "this read has not been checked against output from a real device of its type".into(),
        );
    }
    format!("{named}: {}", parts.join("; "))
}

async fn qos_status(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: QosParams = params(input)?;
    let rows = input.limit.unwrap_or(ctx.caps().rows).min(ctx.caps().rows);
    let (device_id, device_name) = match input
        .device
        .as_deref()
        .map(str::trim)
        .filter(|device| !device.is_empty())
    {
        Some(reference) => {
            let (id, name) = device_ref(ctx, reference).await?;
            (Some(id), shape::one_line(&name, 80))
        }
        None => (None, String::new()),
    };
    let interface = input
        .interface
        .as_deref()
        .map(str::trim)
        .filter(|interface| !interface.is_empty());
    let page = ctx
        .rpc(
            boundary::MONITOR_QOS,
            json!({
                "device_id": device_id, "interface": interface,
                "problem_only": input.problem_only,
                // One more than is shown, to know whether the list was cut.
                "limit": rows + 1,
            }),
        )
        .await?;
    if let Some(id) = &device_id {
        ctx.link(Link::device(id));
    }
    let now = now_ms();
    let found = page["rows"].as_array().cloned().unwrap_or_default();
    let shown = &found[..found.len().min(rows)];
    let cut = found.len() > rows;
    let coverage = page["coverage"]["devices"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let filtered = interface.is_some() || input.problem_only;
    let held: u64 = coverage
        .iter()
        .filter_map(|entry| entry["classes"].as_u64())
        .sum();
    let scope = if device_id.is_some() {
        format!(" on {device_name}")
    } else {
        " on every device monitoring reads".to_string()
    };

    // What the coverage says, per device: in full for a device that was
    // named; otherwise counted by state, with the devices that are not
    // simply read and fine listed by id, as far as there is room.
    let coverage_text = if device_id.is_some() {
        coverage
            .iter()
            .map(|entry| device_coverage(entry, &device_name, now))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let mut states: Vec<(String, usize)> = Vec::new();
        for entry in &coverage {
            let state = shape::one_line(&shape::text_of(entry, "state"), 20);
            match states.iter_mut().find(|(known, _)| *known == state) {
                Some((_, count)) => *count += 1,
                None => states.push((state, 1)),
            }
        }
        let notable: Vec<String> = coverage
            .iter()
            // A failed read, or one that worked and still left something
            // out or was capped. Devices never read, or with no QoS read at
            // all, are counted above and not listed one by one.
            .filter(|entry| {
                !["ok", "unsupported", "never"].contains(&entry["state"].as_str().unwrap_or(""))
                    || entry["series_capped"].as_bool() == Some(true)
                    || !shape::text_of(entry, "error").is_empty()
            })
            .map(|entry| {
                device_coverage(
                    entry,
                    &format!(
                        "device id {}",
                        shape::one_line(&shape::text_of(entry, "device_id"), 40)
                    ),
                    now,
                )
            })
            .collect();
        // Monitoring has four states; a list of more than a handful is not
        // one to read out in full, whatever a feature answered.
        const STATES: usize = 6;
        let mut text = format!(
            "{} device(s): {}{}.",
            coverage.len(),
            states
                .iter()
                .take(STATES)
                .map(|(state, count)| format!("{state} {count}"))
                .collect::<Vec<_>>()
                .join(", "),
            match states.len().saturating_sub(STATES) {
                0 => String::new(),
                more => format!(", and {more} other state(s)"),
            }
        );
        if !notable.is_empty() {
            let room = (ctx.caps().chars / 4).max(300);
            let mut used = 0;
            let mut listed = 0;
            for line in &notable {
                let line = shape::truncate(line, 500);
                used += line.chars().count() + 1;
                if used > room && listed > 0 {
                    break;
                }
                text.push('\n');
                text.push_str(&line);
                listed += 1;
            }
            if notable.len() > listed {
                text.push_str(&format!(
                    "\n[{} more device(s) with a failed read, a cut or a problem not shown — name \
                     a device]",
                    notable.len() - listed
                ));
            }
        }
        text
    };

    let unverified = coverage
        .iter()
        .any(|entry| entry["unverified"].as_bool() == Some(true))
        || shown
            .iter()
            .any(|row| row["unverified"].as_bool() == Some(true));
    let unmeasured = shown
        .iter()
        .filter(|row| row["bps"].as_f64().is_none())
        .count();
    let gaps = shown.iter().any(|row| {
        ["bps", "drop_ps", "queue_drop_ps", "drop_pct"]
            .iter()
            .any(|key| row[*key].as_f64().is_none())
    });
    let undecided = shown
        .iter()
        .filter(|row| dropping(row) == "unknown")
        .count();

    if shown.is_empty() {
        // Why there is nothing: a filter that matched nothing, no policy, a
        // read that never happened or failed, or a device type that has no
        // QoS read. Only the second is an answer about the device.
        let lead = if coverage.is_empty() {
            format!(
                "No QoS class is held{scope}, and no device is covered: none is monitored, so \
                 none is read. That is unknown, not \"no policy\"."
            )
        } else if filtered {
            let on = interface
                .map(|interface| {
                    format!(
                        " on the interface `{}`",
                        shape::one_line(&shape::neutralize(interface), 60)
                    )
                })
                .unwrap_or_default();
            if input.problem_only {
                // The filter is monitoring's `dropping`, which is set only
                // from a drop rate it measured: an empty answer is "none
                // seen dropping", never "none dropping".
                format!(
                    "No class{scope}{on} was measured dropping to congestion. That is not the same \
                     as none dropping: a class whose drops could not be measured (a first read, a \
                     restart, a read that failed, a device that does not print them) is never \
                     counted as dropping, and a device that was not read has no classes at all. \
                     What the reads cover is below."
                )
            } else {
                format!(
                    "No class{scope}{on} is held. What the reads cover is below: a device that \
                     was not read has no classes to match."
                )
            }
        } else {
            format!("No QoS class is held{scope}. What that means, device by device:")
        };
        return Ok(ToolOk::new(
            format!(
                "{lead}\n\n{}",
                shape::data_block("what the QoS reads cover", &coverage_text)
            ),
            "no QoS classes",
        ));
    }

    let many_devices = device_id.is_none();
    let mut headers = Vec::new();
    if many_devices {
        headers.push("device id");
    }
    headers.extend([
        "interface",
        "dir",
        "policy/class",
        "parent",
        "rate",
        "traffic",
        "drops/s",
        "congestion/s",
        "dropped",
        "queue",
        "dropping",
        "series",
    ]);
    let mut table = Table::new(&headers, rows)
        .narrow_with("name a device or an interface, or ask for problem_only");
    for row in shown {
        let mut cells = Vec::new();
        if many_devices {
            cells.push(shape::text_of(row, "device_id"));
        }
        cells.extend([
            match shape::text_of(row, "interface") {
                interface if interface.is_empty() => "(no interface)".to_string(),
                interface => interface,
            },
            shape::text_of(row, "direction"),
            format!(
                "{}/{}",
                shape::text_of(row, "policy"),
                shape::text_of(row, "class")
            ),
            match shape::text_of(row, "parent") {
                parent if parent.is_empty() => "-".to_string(),
                parent => parent,
            },
            row["rate_bps"]
                .as_f64()
                .map(bits)
                .unwrap_or_else(|| "-".into()),
            measured(row, "bps", bits),
            measured(row, "drop_ps", |rate| format!("{}/s", significant(rate))),
            measured(row, "queue_drop_ps", |rate| {
                format!("{}/s", significant(rate))
            }),
            measured(row, "drop_pct", |share| format!("{}%", significant(share))),
            counted(row, "queue_len"),
            dropping(row).to_string(),
            match shape::text_of(row, "instance") {
                instance if instance.is_empty() => "none".to_string(),
                instance => instance,
            },
        ]);
        table.push(cells);
    }

    let mut lead = vec![format!(
        "{} QoS class(es){scope}{}{}.",
        shown.len(),
        if cut {
            if !filtered && held > 0 {
                format!(
                    " — only the first {rows} of the {held} held are shown, in the table's own order"
                )
            } else {
                format!(" — only the first {rows} are shown and there are more")
            }
        } else if !filtered && held as usize == shown.len() {
            " — every class held".to_string()
        } else {
            String::new()
        },
        if input.problem_only {
            ", dropping to congestion"
        } else {
            ""
        },
    )];
    if unmeasured > 0 {
        lead.push(format!(
            "{unmeasured} of them have no measured rate: the interval could not be measured (the \
             first read, a restart, a failed read) or the device does not print the counter."
        ));
    }
    if gaps {
        lead.push(
            "`not measured` is never a zero; `-` is a figure the device does not print.".into(),
        );
    }
    if undecided > 0 {
        lead.push(format!(
            "Whether {undecided} of them are dropping cannot be told — no drop rate was measured — \
             so `dropping` says `unknown` for them, not `no`."
        ));
    }
    if unverified {
        lead.push(
            "Some of this comes from a read that has not been checked against output from a real \
             device of its type: say so when you rely on it."
                .into(),
        );
    }
    lead.push(
        "`dropping` is congestion as far as the device can tell; a class whose drops are all a \
         policer's is the policy working. A class holding a child policy counts its children's \
         traffic and drops."
            .into(),
    );
    Ok(ToolOk::new(
        format!(
            "{}\n\n{}",
            lead.join(" "),
            shape::data_block(
                "QoS classes as the devices name them, and what the reads cover",
                &format!(
                    "{}\n\nWhat the reads cover:\n{coverage_text}",
                    table.render_within(ctx.caps().chars)
                )
            )
        ),
        format!("{} QoS class(es)", shown.len()),
    ))
}

// -------------------------------------------------------------- top_talkers

pub const TOP_TALKERS: Tool = Tool {
    name: "top_talkers",
    title: "Top talkers",
    description: "Who is filling a link, from the flow records exporters sent Nocturne: the \
                  biggest talkers by source, destination, conversation, application (protocol \
                  and service port), interface or DSCP over the last minutes, with bytes, \
                  packets, flow records and their share. It says what the answer covers: \
                  whether the collector is listening, since when it has data, how many records \
                  and exporters it adds up, data that arrived before its template, and which \
                  exporters sample — sampled figures are as exported, never multiplied up. Flow \
                  export is unauthenticated: these are what exporters said, not what Nocturne \
                  measured.",
    brief: "Top flows by source, destination, conversation, application, interface or DSCP.",
    required: &[boundary::FLOWS_TOP],
    optional: &[boundary::DEVICES_GET],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "by": {"type": "string", "enum": ["source", "destination", "conversation", "application", "interface", "dscp"]},
                "device": {"type": "string", "description": DEVICE_ARG},
                "exporter": {"type": "string", "description": "An exporter's address."},
                "minutes": {"type": "integer", "minimum": 1, "maximum": 1440, "description": "Default 15."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100}
            }),
            &["by"],
        )
    },
    run: |ctx, input| Box::pin(top_talkers(ctx, input)),
};

#[derive(Deserialize)]
struct TalkerParams {
    by: String,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    exporter: Option<String>,
    #[serde(default)]
    minutes: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

const WAYS: [&str; 6] = [
    "source",
    "destination",
    "conversation",
    "application",
    "interface",
    "dscp",
];

async fn top_talkers(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: TalkerParams = params(input)?;
    let by = input.by.trim().to_ascii_lowercase();
    if !WAYS.contains(&by.as_str()) {
        return Err(AppError::invalid(format!(
            "flows are added up by one of {}",
            WAYS.join(", ")
        )));
    }
    let minutes = input.minutes.unwrap_or(15).clamp(1, 1_440);
    let rows = input.limit.unwrap_or(ctx.caps().rows).min(ctx.caps().rows);
    let (device_id, on_device) = match input
        .device
        .as_deref()
        .map(str::trim)
        .filter(|device| !device.is_empty())
    {
        Some(reference) => {
            let (id, name) = device_ref(ctx, reference).await?;
            (Some(id), format!(" from {}", shape::one_line(&name, 80)))
        }
        None => (None, String::new()),
    };
    let exporter = input
        .exporter
        .as_deref()
        .map(str::trim)
        .filter(|exporter| !exporter.is_empty());
    let top = ctx
        .rpc(
            boundary::FLOWS_TOP,
            json!({
                "by": by, "device_id": device_id, "exporter": exporter, "minutes": minutes,
                "limit": rows + 1,
            }),
        )
        .await?;
    if let Some(id) = &device_id {
        ctx.link(Link::device(id));
    }
    let coverage = &top["coverage"];
    let found = top["rows"].as_array().cloned().unwrap_or_default();
    let shown = &found[..found.len().min(rows)];
    let cut = found.len() > rows;
    let total_bytes = top["total"]["bytes"].as_u64().unwrap_or_default();
    let window_start = coverage["window_start"].as_i64();
    let until = coverage["until"].as_i64();
    let seconds = match (window_start, until) {
        (Some(start), Some(end)) if end > start => (end - start) as f64 / 1_000.0,
        _ => 0.0,
    };
    let scope = format!(
        "{on_device}{}",
        exporter
            .map(|address| format!(
                " from the exporter {}",
                shape::one_line(&shape::neutralize(address), 60)
            ))
            .unwrap_or_default()
    );
    let window = format!(
        "the last {minutes} minute(s) and the one running{}",
        match (window_start, until) {
            (Some(start), Some(end)) => format!(" ({} to {})", shape::iso(start), shape::iso(end)),
            _ => String::new(),
        }
    );
    let collecting = coverage["collecting"].as_bool();
    let records = coverage["records"].as_u64().unwrap_or_default();

    // What the answer covers, from the coverage alone: numbers and times. An
    // answer that carries none says nothing about whether anything arrived,
    // and reading its absent numbers as zeros would turn "not said" into "no
    // traffic".
    let mut covers = Vec::new();
    let accounted = coverage.is_object();
    if !accounted {
        covers.push(
            "What this answer covers is unknown: the flow collector gave no account of it \
             (whether it is listening, since when it has data, how many records it adds up, what \
             the exporters sample)."
                .to_string(),
        );
    } else {
        covers.push(match collecting {
            Some(true) => "The collector is listening now.".to_string(),
            Some(false) => "The flow collector is not listening now (switched off in the log \
                            collector's settings, or it could not bind), so nothing new is being \
                            counted."
                .to_string(),
            None => "Whether the collector is listening is unknown.".to_string(),
        });
        let since = coverage["since"].as_i64();
        covers.push(match since {
            Some(since) if window_start.is_some_and(|start| since > start) => format!(
                "Data in the window starts at {}: nothing earlier in it was received or kept \
                 (flows are kept in memory for a day, and a restart loses them).",
                shape::iso(since)
            ),
            Some(_) => "Data covers the whole window.".to_string(),
            None => "Nothing arrived in the window.".to_string(),
        });
        let exporters = coverage["exporters"].as_u64().unwrap_or_default();
        covers.push(format!(
            "{records} flow record(s) from {exporters} exporter(s){}.",
            if exporters > 1 {
                "; a conversation two exporters saw is counted by both"
            } else {
                ""
            }
        ));
        let without_template = coverage["sets_without_template"]
            .as_u64()
            .unwrap_or_default();
        if without_template > 0 {
            covers.push(format!(
                "{without_template} data set(s) arrived before their template: what they held is \
                 in no row and not in the total."
            ));
        }
        let rates: Vec<u64> = coverage["sampling"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .take(8)
            .collect();
        let sampled = coverage["sampled_records"].as_u64().unwrap_or_default();
        covers.push(if rates.is_empty() {
            "No exporter in the window said it samples (one that samples without saying so \
             cannot be told apart)."
                .to_string()
        } else {
            format!(
                "{sampled} of the {records} record(s) came from exporters sampling 1 in {}: their \
                 bytes and packets are as exported and NOT multiplied, so they stand for about \
                 that many times more traffic.",
                rates
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", 1 in ")
            )
        });
    }
    covers.push(
        "Flow export is unauthenticated: anything that reaches the collector can report traffic, \
         so these are what exporters said."
            .into(),
    );

    if shown.is_empty() {
        let (lead, summary) = match collecting {
            _ if !accounted => (
                format!(
                    "Unknown — the flow collector answered with no rows{scope} and no account of \
                     what it covers. That is not the same as no traffic."
                ),
                "flow coverage unknown",
            ),
            Some(false) if records == 0 => (
                format!(
                    "Unknown — no flow is being counted{scope}: the collector is not listening. \
                     That is not the same as no traffic."
                ),
                "flow collector not listening",
            ),
            _ if records == 0 => (
                format!(
                    "None in the window: no flow record arrived{scope} over {window}. Either \
                     nothing exports flows to Nocturne{scope}, or the exporters sent nothing — \
                     this is not a measurement of zero traffic."
                ),
                "no flow records in the window",
            ),
            _ => (
                format!(
                    "Every record{scope} over {window} is under no key: each minute keeps only \
                     its biggest talkers. In all: {} in {records} record(s).",
                    bytes(total_bytes)
                ),
                "every record under no key",
            ),
        };
        return Ok(ToolOk::new(
            format!("{lead}\n\n{}", covers.join("\n")),
            summary,
        ));
    }

    let mut table = Table::new(
        &["what", "bytes", "share", "packets", "flows", "average"],
        rows,
    )
    .narrow_with("shorten the window, or name a device or an exporter");
    for row in shown {
        // A figure the answer does not carry is `-`, never a zero.
        let row_bytes = row["bytes"].as_u64();
        table.push([
            match shape::text_of(row, "label") {
                label if label.is_empty() => shape::text_of(row, "key"),
                label => label,
            },
            row_bytes.map(bytes).unwrap_or_else(|| "-".into()),
            match row_bytes {
                Some(row_bytes) if total_bytes > 0 => format!(
                    "{}%",
                    significant(row_bytes as f64 * 100.0 / total_bytes as f64)
                ),
                _ => "-".into(),
            },
            counted(row, "packets"),
            counted(row, "flows"),
            match row_bytes {
                Some(row_bytes) if seconds > 0.0 => bits(row_bytes as f64 * 8.0 / seconds),
                _ => "-".into(),
            },
        ]);
    }
    let mut body = table.render_within(ctx.caps().chars);
    let other = &top["other"];
    match other["bytes"].as_u64() {
        Some(other_bytes) => body.push_str(&format!(
            "\nunder no key (smaller talkers, added up minute by minute): {}{}",
            bytes(other_bytes),
            if total_bytes > 0 {
                format!(
                    ", {}% of the total",
                    significant(other_bytes as f64 * 100.0 / total_bytes as f64)
                )
            } else {
                String::new()
            }
        )),
        None => body.push_str("\nunder no key: not given with this answer"),
    }
    let total = &top["total"];
    match total["bytes"].as_u64() {
        Some(total_bytes) => body.push_str(&format!(
            "\ntotal: {}, {} packet(s), {} flow record(s){}",
            bytes(total_bytes),
            counted(total, "packets"),
            counted(total, "flows"),
            if seconds > 0.0 {
                format!(
                    " — {} on average over the window",
                    bits(total_bytes as f64 * 8.0 / seconds)
                )
            } else {
                String::new()
            }
        )),
        None => body.push_str("\ntotal: not given with this answer"),
    }
    let mut lead = format!(
        "The {} biggest by {by}{scope} over {window}{}. A row is a floor — never more than was \
         seen — and the average is the row's bytes over the whole window.",
        shown.len(),
        if cut {
            format!(" — only the first {rows} are shown and there are more")
        } else {
            String::new()
        }
    );
    if by == "interface" {
        lead.push_str(
            " Each flow is counted under the interface it came in by and the one it left by, so \
             these rows add up to twice the total.",
        );
    }
    Ok(ToolOk::new(
        format!(
            "{lead}\n\n{}\n\nWhat this covers:\n{}",
            shape::data_block("flow records, as the exporters reported them", &body),
            covers.join("\n")
        ),
        format!("{} talker(s) by {by}", shown.len()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_are_written_as_a_person_says_them() {
        assert_eq!(bits(11_788_013.25), "11.8 Mbit/s");
        assert_eq!(bits(254_523.76), "255 kbit/s");
        assert_eq!(bits(512.0), "512 bit/s");
        assert_eq!(bits(6_000_000.0), "6 Mbit/s");
        assert_eq!(bytes(37_876), "37.9 kB");
        assert_eq!(bytes(860), "860 B");
        assert_eq!(bytes(0), "0 B");
        assert_eq!(significant(49.393), "49.4");
        assert_eq!(significant(0.0), "0");
    }

    #[test]
    fn a_rate_nobody_measured_is_not_a_zero() {
        let row = json!({"bps": null, "drop_ps": 0.0});
        assert_eq!(measured(&row, "bps", bits), "not measured");
        assert_eq!(measured(&row, "missing", bits), "not measured");
        assert_eq!(measured(&row, "drop_ps", bits), "0 bit/s");
        assert_eq!(counted(&json!({"queue_len": null}), "queue_len"), "-");
    }
}
