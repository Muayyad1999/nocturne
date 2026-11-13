//! Reports for the people who ask for reports.
//!
//! Seven kinds, four formats, one document model ([`crate::render`]). The
//! rules that matter more than the layout:
//!
//! * A value that was never collected is blank, and the header says how many
//!   devices it is blank for. It is never a zero and never a guess.
//! * A measurement states its own coverage. The availability report refuses
//!   to give a percentage for a device whose reachability was observed for
//!   less than half the period; those devices are listed separately.
//! * A kind whose source is not in this build is a report that says so,
//!   rather than an empty table.

use std::collections::BTreeMap;

use nocturne_app::inventory::DeviceFilter;
use nocturne_app::model::DeviceRow;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, AppError, AppResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::configs;
use crate::model::{ReportHead, SavedReport};
use crate::reach;
use crate::records::{self, Kind as RecordKind, Query, Record};
use crate::refs::{Devices, Period};
use crate::render::{
    Cell, Document, Format, Section, Table, filter_fact, generated_fact, missing, period_fact,
};
use crate::settings;
use crate::sources::{self, Reason};
use crate::timefmt::Zone;

const WHAT: &str = "report";
const WEEK_MS: i64 = 7 * 86_400_000;
/// Coverage below which a device is listed without a percentage.
const MIN_COVERAGE: f64 = 0.5;

/// The report kinds, with what each needs.
#[derive(Debug)]
pub struct KindInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// The RPC method it cannot be made without, if any.
    pub needs: Option<&'static str>,
    /// Whether it covers a period rather than the present.
    pub period: bool,
}

pub const KINDS: [KindInfo; 7] = [
    KindInfo {
        id: "inventory",
        title: "Inventory",
        description: "Every device with its site, role, type, model, serial, OS version and uptime, as last collected.",
        needs: None,
        period: false,
    },
    KindInfo {
        id: "software",
        title: "Software versions",
        description: "OS versions per model, and the devices that differ from the most common version of their model.",
        needs: None,
        period: false,
    },
    KindInfo {
        id: "availability",
        title: "Availability",
        description: "Per device over the period, from reachability samples: share of observations up, outages, and the coverage behind the figure.",
        needs: None,
        period: true,
    },
    KindInfo {
        id: "changes",
        title: "Changes",
        description: "Changes applied, failed and rolled back in the period, by origin, with their tickets.",
        needs: Some("changes.list"),
        period: true,
    },
    KindInfo {
        id: "backups",
        title: "Configuration backups",
        description: "Backup coverage, the age of the newest backup per device, and the devices never backed up.",
        needs: None,
        period: false,
    },
    KindInfo {
        id: "compliance",
        title: "Compliance",
        description: "The latest compliance run, by policy and by device.",
        needs: Some("compliance.results"),
        period: false,
    },
    KindInfo {
        id: "alerts",
        title: "Alerts",
        description: "Alerts that fired in the period by rule, device and severity, with the mean time to acknowledge and to resolve.",
        needs: Some("alerts.list"),
        period: true,
    },
];

fn kind_info(id: &str) -> AppResult<&'static KindInfo> {
    KINDS.iter().find(|kind| kind.id == id).ok_or_else(|| {
        AppError::invalid(format!(
            "there is no report kind `{id}`; this build has {}",
            KINDS
                .iter()
                .map(|kind| kind.id)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

/// What this build can report on, and why it cannot report on the rest.
pub fn kinds(app: &App) -> Value {
    json!({
        "kinds": KINDS
            .iter()
            .map(|kind| {
                let available = kind.needs.is_none_or(|method| sources::has(app, method));
                json!({
                    "id": kind.id,
                    "title": kind.title,
                    "description": kind.description,
                    "period": kind.period,
                    "needs": kind.needs,
                    "available": available,
                    "unavailable_reason": (!available).then(|| format!(
                        "`{}` is not part of this build",
                        kind.needs.unwrap_or_default()
                    )),
                })
            })
            .collect::<Vec<_>>(),
        "formats": ["markdown", "csv", "html", "json"],
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GenerateParams {
    pub kind: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Which devices the report is about (the core's device filter).
    pub filter: Option<Value>,
    #[serde(default = "markdown")]
    pub format: Format,
    pub utc_offset_minutes: Option<i32>,
}

fn markdown() -> Format {
    Format::Markdown
}

/// The devices a report covers, and the filter as a header fact.
fn scope(app: &App, filter: &Option<Value>) -> AppResult<(Devices, Option<String>)> {
    let Some(filter) = filter else {
        return Ok((Devices::load(app)?, None));
    };
    let parsed: DeviceFilter = serde_json::from_value(filter.clone())
        .map_err(|err| AppError::invalid(format!("that device filter cannot be read: {err}")))?;
    Ok((Devices::matching(app, &parsed)?, filter_fact(filter)))
}

pub async fn generate(app: &App, params: &GenerateParams) -> AppResult<Value> {
    let info = kind_info(&params.kind)?;
    let zone = Zone::resolve(params.utc_offset_minutes)?;
    let now = now_ms();
    let until = params.until.unwrap_or(now);
    let since = params.since.unwrap_or(until - WEEK_MS);
    if until <= since {
        return Err(AppError::invalid(
            "a report period needs `until` after `since`",
        ));
    }
    let period = Period { since, until };
    let (devices, filter) = scope(app, &params.filter)?;

    let mut document = match info.id {
        "inventory" => inventory(&devices, &zone),
        "software" => software(&devices),
        "availability" => availability(app, &devices, period, &zone)?,
        "changes" => changes(app, &devices, period, &zone).await,
        "backups" => backups(app, &devices, &zone, now)?,
        "compliance" => compliance(app, &devices, &zone).await,
        "alerts" => alerts(app, &devices, period, &zone).await,
        other => {
            return Err(AppError::internal(format!(
                "report kind `{other}` has no builder"
            )));
        }
    };
    if info.period {
        document.fact("Period", period_fact(&zone, since, until));
    }
    document
        .fact("Devices covered", devices.rows().len().to_string())
        .fact("Times shown in", zone.label())
        .fact("Generated", generated_fact(&zone, now));
    if let Some(filter) = &filter {
        document.fact("Filter", filter.clone());
    }
    Ok(json!({
        "kind": info.id,
        "title": document.title,
        "format": params.format.as_str(),
        "filename": format!(
            "{}-{}.{}",
            info.id,
            zone.stamp(now),
            params.format.extension()
        ),
        "content": document.render(params.format),
        "rows": document.rows(),
        "unavailable": document.unavailable(),
        "period": if info.period { Some(period) } else { None },
        "generated_at": now,
    }))
}

// ---------------------------------------------------------------- inventory

/// The device's facts, when something has ever collected them.
fn facts(row: &DeviceRow) -> Option<&nocturne_drivers::model::Facts> {
    row.state.facts.as_ref()
}

fn blank_or(text: &str) -> Cell {
    if text.trim().is_empty() {
        Cell::unknown()
    } else {
        Cell::text(text)
    }
}

fn inventory(devices: &Devices, zone: &Zone) -> Document {
    let mut document = Document::new("Inventory");
    let mut table = Table::new(&[
        ("device", "Device"),
        ("address", "Address"),
        ("site", "Site"),
        ("role", "Role"),
        ("profile", "Type"),
        ("model", "Model"),
        ("serial", "Serial"),
        ("os", "OS"),
        ("os_version", "Version"),
        ("uptime", "Uptime"),
        ("collected_at", "Facts collected"),
        ("lab", "Lab"),
    ]);
    let mut never = 0usize;
    for row in devices.rows() {
        let facts = facts(row);
        if facts.is_none() {
            never += 1;
        }
        table.push(vec![
            Cell::text(&row.device.name),
            Cell::text(&row.device.address),
            blank_or(&row.device.site),
            blank_or(&row.device.role),
            blank_or(&row.device.profile),
            Cell::maybe(facts.map(|facts| blank_or(&facts.model))),
            Cell::maybe(facts.map(|facts| blank_or(&facts.serial))),
            Cell::maybe(facts.map(|facts| blank_or(&facts.os))),
            Cell::maybe(facts.map(|facts| blank_or(&facts.os_version))),
            Cell::maybe(
                facts
                    .and_then(|facts| facts.uptime_s)
                    .map(|seconds| Cell::duration(seconds as i64 * 1000)),
            ),
            Cell::maybe(row.state.facts_at.map(|at| Cell::time(zone, at))),
            Cell::text(if row.device.lab.is_some() {
                "yes"
            } else {
                "no"
            }),
        ]);
    }
    document.section(Section::table("Devices", table).with_note(format!(
        "{never} of {} device(s) have never had their facts collected; their model, serial, OS and uptime are blank rather than zero.",
        devices.rows().len()
    )));
    document
}

// ----------------------------------------------------------------- software

fn software(devices: &Devices) -> Document {
    let mut document = Document::new("Software versions");
    // model → version → device names
    let mut by_model: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    let mut unknown: Vec<&str> = Vec::new();
    for row in devices.rows() {
        match facts(row).filter(|facts| !facts.os_version.trim().is_empty()) {
            Some(facts) => {
                let model = if facts.model.trim().is_empty() {
                    "(model not collected)"
                } else {
                    facts.model.as_str()
                };
                by_model
                    .entry(model)
                    .or_default()
                    .entry(facts.os_version.as_str())
                    .or_default()
                    .push(row.device.name.as_str());
            }
            None => unknown.push(row.device.name.as_str()),
        }
    }

    let mut table = Table::new(&[
        ("model", "Model"),
        ("version", "Version"),
        ("devices", "Devices"),
        ("most_common", "Most common for this model"),
    ]);
    let mut odd = Table::new(&[
        ("device", "Device"),
        ("model", "Model"),
        ("version", "Version"),
        ("expected", "Most common for its model"),
    ]);
    for (model, versions) in &by_model {
        // The most common version of the model — only when one version
        // really is more common than every other. Two versions on two
        // devices each means the model has no standard version, and calling
        // one of them the standard would be this report's own invention.
        let most = versions
            .values()
            .map(|names| names.len())
            .max()
            .unwrap_or(0);
        let leaders: Vec<&str> = versions
            .iter()
            .filter(|(_, names)| names.len() == most)
            .map(|(version, _)| *version)
            .collect();
        let common = (leaders.len() == 1).then(|| leaders[0]);
        for (version, names) in versions {
            table.push(vec![
                Cell::text(*model),
                Cell::text(*version),
                Cell::count(names.len()),
                match common {
                    Some(common) if *version == common => Cell::text("yes"),
                    Some(_) => Cell::text("no"),
                    None => Cell::text("no single version is the most common"),
                },
            ]);
            if let Some(common) = common
                && *version != common
            {
                for name in names {
                    odd.push(vec![
                        Cell::text(*name),
                        Cell::text(*model),
                        Cell::text(*version),
                        Cell::text(common),
                    ]);
                }
            }
        }
    }
    document.section(Section::table("Versions by model", table));
    document.section(Section::table("Devices out of step with their model", odd).with_note(
        "A device whose OS version is not the most common one for its model. Models with one device never appear here.",
    ));
    if !unknown.is_empty() {
        document.section(
            Section::list(
                "Devices whose version is not known",
                unknown.iter().map(|name| (*name).to_string()).collect(),
            )
            .with_note("Nothing has collected an OS version for these devices."),
        );
    }
    document
}

// ------------------------------------------------------------- availability

fn availability(app: &App, devices: &Devices, period: Period, zone: &Zone) -> AppResult<Document> {
    let limit = settings::get(app)?.availability_max_devices;
    let mut document = Document::new("Availability");
    let mut table = Table::new(&[
        ("device", "Device"),
        ("site", "Site"),
        ("availability_pct", "Up"),
        ("outages", "Outages"),
        ("longest_outage", "Longest"),
        ("down", "Down in total"),
        ("coverage_pct", "Coverage"),
        ("observations", "Observations"),
        ("resolution", "Samples"),
    ]);
    let mut thin = Table::new(&[
        ("device", "Device"),
        ("site", "Site"),
        ("coverage_pct", "Coverage"),
        ("observations", "Observations"),
        ("why", "Why there is no figure"),
    ]);
    let mut never: Vec<String> = Vec::new();
    for row in devices.rows().iter().take(limit) {
        let Some((series, analysis)) =
            reach::of_device(app.store(), &row.device, period.since, period.until)?
        else {
            never.push(row.device.name.clone());
            continue;
        };
        if analysis.coverage < MIN_COVERAGE || analysis.availability.is_none() {
            thin.push(vec![
                Cell::text(&row.device.name),
                blank_or(&row.device.site),
                Cell::percent(analysis.coverage),
                Cell::int(analysis.observations as i64),
                Cell::text(if analysis.observations == 0 {
                    "no reachability samples in this period".to_string()
                } else {
                    format!(
                        "{} of the {} expected observations",
                        analysis.observations, analysis.expected
                    )
                }),
            ]);
            continue;
        }
        table.push(vec![
            Cell::text(&row.device.name),
            blank_or(&row.device.site),
            Cell::maybe(analysis.availability.map(Cell::percent)),
            Cell::count(analysis.outages.len()),
            Cell::maybe(
                analysis
                    .longest_outage()
                    .map(|outage| Cell::duration(outage.down_ms)),
            ),
            Cell::duration(analysis.down_ms()),
            Cell::percent(analysis.coverage),
            Cell::int(analysis.observations as i64),
            Cell::text(series.resolution.label()),
        ]);
    }
    document.section(
        Section::table("Measured devices", table).with_note(format!(
            "The percentage is the share of observations in which the device answered, not of the period: a gap in the samples is a gap, never an assumption. Coverage is how much of {} was observed at all.",
            period_fact(zone, period.since, period.until)
        )),
    );
    document.section(Section::table("Too little data to measure", thin).with_note(format!(
        "Observed for less than {:.0}% of the period. These devices are deliberately given no percentage.",
        MIN_COVERAGE * 100.0
    )));
    if !never.is_empty() {
        document.section(
            Section::list("Never monitored", never).with_note(
                "Nothing has ever recorded these devices' reachability, so nothing can be said about them.",
            ),
        );
    }
    if devices.rows().len() > limit {
        document.section(missing(
            "Devices beyond the measurement limit",
            "availability",
            Reason::NoData,
            &format!(
                "{} of {} devices were not measured: this report measures at most {limit} (the `ops.availability_max_devices` setting).",
                devices.rows().len() - limit,
                devices.rows().len()
            ),
        ));
    }
    Ok(document)
}

// ------------------------------------------------------------------ changes

async fn changes(app: &App, devices: &Devices, period: Period, zone: &Zone) -> Document {
    let mut document = Document::new("Changes");
    let changes = match sources::changes(app, json!({"limit": 1000})).await {
        Ok(changes) => changes,
        Err(gap) => {
            document.section(Section::missing("Changes", gap.for_section("changes")));
            return document;
        }
    };
    let covered = |change: &sources::ChangeSummary| match change.devices() {
        Some(ids) => ids.iter().any(|id| devices.contains(id)),
        // A change list that does not say which devices it touched cannot be
        // filtered by device, so it is kept and the note says so.
        None => true,
    };
    let inside: Vec<&sources::ChangeSummary> = changes
        .iter()
        .filter(|change| period.contains(change.updated_at) && covered(change))
        .collect();

    let mut table = Table::new(&[
        ("state", "State"),
        ("title", "Change"),
        ("origin", "Origin"),
        ("ticket", "Ticket"),
        ("created_by", "By"),
        ("devices", "Devices"),
        ("created_at", "Created"),
        ("updated_at", "Finished"),
    ]);
    let mut by_origin: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    for change in &inside {
        let origin = change
            .origin
            .clone()
            .unwrap_or_else(|| "(not recorded)".into());
        let counts = by_origin.entry(origin.clone()).or_insert([0, 0, 0]);
        if change.is_applied() {
            counts[0] += 1;
        } else if change.state == "rolled_back" {
            counts[2] += 1;
        } else if change.is_failure() {
            counts[1] += 1;
        }
        table.push(vec![
            Cell::text(&change.state),
            Cell::text(&change.title),
            Cell::text(&origin),
            Cell::maybe(change.ticket.clone().map(Cell::text)),
            Cell::maybe(change.created_by.clone().map(Cell::text)),
            Cell::maybe(change.devices().map(|ids| {
                Cell::text(
                    ids.iter()
                        .map(|id| devices.name(id))
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            })),
            Cell::time(zone, change.created_at),
            Cell::time(zone, change.updated_at),
        ]);
    }
    let mut summary = Table::new(&[
        ("origin", "Origin"),
        ("applied", "Applied"),
        ("failed", "Failed"),
        ("rolled_back", "Rolled back"),
    ]);
    for (origin, counts) in &by_origin {
        summary.push(vec![
            Cell::text(origin),
            Cell::count(counts[0]),
            Cell::count(counts[1]),
            Cell::count(counts[2]),
        ]);
    }
    document.section(Section::table("By origin", summary));
    document.section(Section::table("Changes", table).with_note(format!(
        "{} change(s) ended in the period{}.",
        inside.len(),
        if changes.len() >= 1000 {
            "; only the newest 1000 changes were read, so older ones are not counted"
        } else {
            ""
        }
    )));
    document
}

// ------------------------------------------------------------------ backups

fn backups(app: &App, devices: &Devices, zone: &Zone, now: i64) -> AppResult<Document> {
    let mut document = Document::new("Configuration backups");
    let mut table = Table::new(&[
        ("device", "Device"),
        ("site", "Site"),
        ("newest_backup_at", "Newest backup"),
        ("age", "Age"),
        ("lines", "Lines"),
        ("versions", "Versions kept"),
    ]);
    let mut never: Vec<String> = Vec::new();
    let mut with = 0usize;
    for row in devices.rows() {
        let versions = app
            .store()
            .config_versions(&row.device.id, Some("running"))?;
        match versions.first() {
            Some(newest) => {
                with += 1;
                table.push(vec![
                    Cell::text(&row.device.name),
                    blank_or(&row.device.site),
                    Cell::time(zone, newest.taken_at),
                    Cell::duration(now - newest.taken_at),
                    Cell::int(i64::from(newest.lines)),
                    Cell::count(versions.len()),
                ]);
            }
            None => never.push(row.device.name.clone()),
        }
    }
    let total = devices.rows().len();
    document.section(Section::table("Coverage", {
        let mut coverage = Table::new(&[("what", "What"), ("count", "Devices")]);
        coverage.push(vec![Cell::text("with a backup"), Cell::count(with)]);
        coverage.push(vec![
            Cell::text("never backed up"),
            Cell::count(total - with),
        ]);
        coverage.push(vec![Cell::text("in scope"), Cell::count(total)]);
        coverage
    }));
    document.section(Section::table("Newest backup per device", table).with_note(
        "Read from the stored configuration snapshots. A snapshot is only kept when its content differs from the one before it.",
    ));
    if !never.is_empty() {
        document.section(Section::list("Never backed up", never));
    }
    // What moved recently, which is the other half of the question.
    let ids: Vec<String> = devices
        .rows()
        .iter()
        .map(|row| row.device.id.clone())
        .collect();
    let (recent, _) = configs::changes_in(
        app.store(),
        &ids,
        Period {
            since: now - WEEK_MS,
            until: now + 1,
        },
    )?;
    let mut moved = Table::new(&[
        ("device", "Device"),
        ("at", "When"),
        ("lines_added", "+"),
        ("lines_removed", "−"),
        ("source", "Taken by"),
        ("note", "Note"),
    ]);
    for change in recent.iter().filter(|change| !change.first) {
        moved.push(vec![
            Cell::text(devices.name(&change.device_id)),
            Cell::time(zone, change.at),
            Cell::maybe(change.lines_added.map(Cell::count)),
            Cell::maybe(change.lines_removed.map(Cell::count)),
            Cell::text(&change.source),
            blank_or(&change.note),
        ]);
    }
    document.section(
        Section::table("Configurations that changed in the last 7 days", moved)
            .with_note("Counted from the stored snapshots, whoever took them."),
    );
    Ok(document)
}

// --------------------------------------------------------------- compliance

async fn compliance(app: &App, devices: &Devices, zone: &Zone) -> Document {
    let mut document = Document::new("Compliance");
    let results = match sources::compliance(app, None).await {
        Ok(results) => results,
        Err(gap) => {
            document.section(Section::missing(
                "Latest run",
                gap.for_section("compliance"),
            ));
            return document;
        }
    };
    let policies = sources::compliance_policies(app).await.unwrap_or_default();
    let policy_name = |id: &str| {
        policies
            .iter()
            .find(|policy| policy.id == id)
            .and_then(|policy| policy.name.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let rule_name = |policy_id: &str, rule_id: &str| {
        policies
            .iter()
            .find(|policy| policy.id == policy_id)
            .and_then(|policy| policy.rules.iter().find(|rule| rule.id == rule_id))
            .and_then(|rule| rule.title.clone())
            .unwrap_or_else(|| rule_id.to_string())
    };

    let mut summary = Table::new(&[("what", "What"), ("value", "Value")]);
    summary.push(vec![
        Cell::text("score"),
        Cell::maybe(
            results
                .summary
                .score_pct
                .map(|score| Cell::percent(score / 100.0)),
        ),
    ]);
    summary.push(vec![
        Cell::text("devices evaluated"),
        Cell::maybe(results.summary.devices.map(|count| Cell::int(count as i64))),
    ]);
    summary.push(vec![
        Cell::text("rules evaluated"),
        Cell::maybe(results.summary.rules.map(|count| Cell::int(count as i64))),
    ]);
    summary.push(vec![
        Cell::text("run at"),
        Cell::maybe(results.run_at.map(|at| Cell::time(zone, at))),
    ]);
    document.section(Section::table("Latest run", summary).with_note(
        if results.run_at.is_none() {
            "The compliance feature did not say when the run happened, so the time is blank."
        } else {
            ""
        },
    ));

    let mut by_policy: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    let mut by_device: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    let mut failures = Table::new(&[
        ("device", "Device"),
        ("policy", "Policy"),
        ("rule", "Rule"),
        ("detail", "Detail"),
    ]);
    for finding in &results.findings {
        if !devices.contains(&finding.device_id) && !devices.rows().is_empty() {
            continue;
        }
        *by_policy
            .entry(finding.policy_id.as_str())
            .or_default()
            .entry(finding.status.as_str())
            .or_default() += 1;
        *by_device
            .entry(finding.device_id.as_str())
            .or_default()
            .entry(finding.status.as_str())
            .or_default() += 1;
        if finding.status == "fail" {
            failures.push(vec![
                Cell::text(devices.name(&finding.device_id)),
                Cell::text(policy_name(&finding.policy_id)),
                Cell::text(rule_name(&finding.policy_id, &finding.rule_id)),
                Cell::maybe(finding.detail.clone().map(Cell::text)),
            ]);
        }
    }
    let status_table = |rows: BTreeMap<&str, BTreeMap<&str, usize>>,
                        first: &str,
                        name: &dyn Fn(&str) -> String| {
        let mut table = Table::new(&[
            ("subject", first),
            ("pass", "Pass"),
            ("fail", "Fail"),
            ("not_applicable", "Not applicable"),
            ("not_evaluated", "Not evaluated"),
        ]);
        for (subject, counts) in rows {
            let get = |status: &str| counts.get(status).copied().unwrap_or(0);
            table.push(vec![
                Cell::text(name(subject)),
                Cell::count(get("pass")),
                Cell::count(get("fail")),
                Cell::count(get("not_applicable")),
                Cell::count(get("not_evaluated")),
            ]);
        }
        table
    };
    document.section(Section::table(
        "By policy",
        status_table(by_policy, "Policy", &|id| policy_name(id)),
    ));
    document.section(Section::table(
        "By device",
        status_table(by_device, "Device", &|id| devices.name(id)),
    ));
    document.section(Section::table("Failures", failures).with_note(if results.truncated {
        "The findings were cut off by the compliance feature's own paging, so this table may be incomplete."
    } else {
        ""
    }));
    document
}

// ------------------------------------------------------------------- alerts

async fn alerts(app: &App, devices: &Devices, period: Period, zone: &Zone) -> Document {
    let mut document = Document::new("Alerts");
    let alerts = match sources::alerts(app, json!({"since": period.since, "limit": 5000})).await {
        Ok(alerts) => alerts,
        Err(gap) => {
            document.section(Section::missing("Alerts", gap.for_section("alerts")));
            return document;
        }
    };
    let in_scope: Vec<&sources::Alert> = alerts
        .iter()
        .filter(|alert| period.contains(alert.started_at))
        .filter(|alert| match &alert.device_id {
            Some(id) => devices.contains(id) || devices.rows().is_empty(),
            None => true,
        })
        .collect();

    // Mean time to acknowledge and to resolve, over the alerts that were.
    let mut ack: Vec<i64> = Vec::new();
    let mut resolve: Vec<i64> = Vec::new();
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_device: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_severity: BTreeMap<&str, usize> = BTreeMap::new();
    let mut still_open = 0usize;
    for alert in &in_scope {
        if let Some(at) = alert.acked_at {
            ack.push(at - alert.started_at);
        }
        match alert.resolved_at {
            Some(at) => resolve.push(at - alert.started_at),
            None => still_open += 1,
        }
        *by_rule
            .entry(
                alert
                    .rule_name
                    .clone()
                    .or_else(|| alert.rule_id.clone())
                    .unwrap_or_else(|| "(no rule)".into()),
            )
            .or_default() += 1;
        *by_device
            .entry(match &alert.device_id {
                Some(id) => devices.name(id),
                None => "(no device)".into(),
            })
            .or_default() += 1;
        *by_severity.entry(alert.severity.as_str()).or_default() += 1;
    }
    let mean = |values: &[i64]| {
        (!values.is_empty()).then(|| values.iter().sum::<i64>() / values.len() as i64)
    };

    let mut summary = Table::new(&[("what", "What"), ("value", "Value"), ("of", "Counted over")]);
    summary.push(vec![
        Cell::text("alerts fired"),
        Cell::count(in_scope.len()),
        Cell::text(period_fact(zone, period.since, period.until)),
    ]);
    summary.push(vec![
        Cell::text("still open"),
        Cell::count(still_open),
        Cell::text("now"),
    ]);
    summary.push(vec![
        Cell::text("mean time to acknowledge"),
        Cell::maybe(mean(&ack).map(Cell::duration)),
        Cell::text(format!("{} acknowledged alert(s)", ack.len())),
    ]);
    summary.push(vec![
        Cell::text("mean time to resolve"),
        Cell::maybe(mean(&resolve).map(Cell::duration)),
        Cell::text(format!("{} resolved alert(s)", resolve.len())),
    ]);
    document.section(Section::table("Summary", summary).with_note(
        "The means are over the alerts that were acknowledged and resolved; alerts still firing are counted separately rather than treated as instant.",
    ));

    let counted = |rows: BTreeMap<String, usize>, heading: &str, first: &str| {
        let mut table = Table::new(&[("subject", first), ("alerts", "Alerts")]);
        let mut rows: Vec<(String, usize)> = rows.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (subject, count) in rows {
            table.push(vec![Cell::text(subject), Cell::count(count)]);
        }
        Section::table(heading, table)
    };
    document.section(counted(by_rule, "By rule", "Rule"));
    document.section(counted(
        by_device,
        "Devices that cost the most attention",
        "Device",
    ));
    let mut severity = Table::new(&[("severity", "Severity"), ("alerts", "Alerts")]);
    for level in sources::Severity::MOST_SEVERE_FIRST {
        severity.push(vec![
            Cell::text(level.as_str()),
            Cell::count(by_severity.get(level.as_str()).copied().unwrap_or(0)),
        ]);
    }
    document.section(
        Section::table("By severity", severity).with_note(if alerts.len() >= 5000 {
            "Only the newest 5000 alerts were read, so older ones in the period are not counted."
        } else {
            ""
        }),
    );
    document
}

// ---------------------------------------------------------- saved reports

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    pub kind: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub filter: Option<Value>,
    #[serde(default = "markdown")]
    pub format: Format,
    pub utc_offset_minutes: Option<i32>,
    /// A title of your own; the report's own otherwise.
    pub title: Option<String>,
    pub notes: String,
    pub author: String,
}

/// Generate the report here and store what was generated. A client cannot
/// hand in content of its own.
pub async fn save(app: &App, params: SaveParams) -> AppResult<Value> {
    let generated = generate(
        app,
        &GenerateParams {
            kind: params.kind.clone(),
            since: params.since,
            until: params.until,
            filter: params.filter.clone(),
            format: params.format,
            utc_offset_minutes: params.utc_offset_minutes,
        },
    )
    .await?;
    let now = now_ms();
    let saved = SavedReport {
        id: new_id(),
        kind: params.kind.clone(),
        title: params
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                generated["title"]
                    .as_str()
                    .unwrap_or(&params.kind)
                    .to_string()
            }),
        format: params.format.as_str().to_string(),
        content: generated["content"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        rows: generated["rows"].as_u64().unwrap_or(0) as usize,
        period: serde_json::from_value(generated["period"].clone()).unwrap_or(None),
        filter: params.filter.clone(),
        notes: params.notes.clone(),
        author: params.author.clone(),
        unavailable: serde_json::from_value(generated["unavailable"].clone()).unwrap_or_default(),
        generated_at: generated["generated_at"].as_i64().unwrap_or(now),
        created_at: now,
    };
    records::save(
        app.store(),
        RecordKind::Report,
        &Record {
            id: saved.id.clone(),
            sort: saved.generated_at,
            facet: saved.kind.clone(),
            head: serde_json::to_string(&saved.head())?,
            search: saved.search_text(),
            body: serde_json::to_string(&saved)?,
            links: Vec::new(),
        },
    )?;
    app.publish("reports.changed", &saved.head());
    app.audit(
        "reports.save",
        None,
        format!("Saved the {} report `{}`", saved.kind, saved.title),
        json!({"report_id": saved.id, "kind": saved.kind, "format": saved.format, "rows": saved.rows}),
    );
    Ok(serde_json::to_value(&saved)?)
}

pub fn load(app: &App, id: &str) -> AppResult<SavedReport> {
    let body = records::body(app.store(), RecordKind::Report, id)?
        .ok_or_else(|| AppError::not_found(format!("{WHAT} `{id}`")))?;
    serde_json::from_str(&body).map_err(AppError::from)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    pub kind: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

pub fn list(app: &App, params: &ListParams) -> AppResult<Value> {
    let page = records::page(
        app.store(),
        RecordKind::Report,
        &Query {
            facets: params.kind.iter().cloned().collect(),
            words: params
                .query
                .as_deref()
                .map(records::words)
                .unwrap_or_default(),
            cursor: params.cursor.clone(),
            limit: params.limit.unwrap_or(50).clamp(1, 500),
            ..Query::default()
        },
    )?;
    let reports: Vec<ReportHead> = page
        .heads
        .iter()
        .map(|head| serde_json::from_str(head).map_err(AppError::from))
        .collect::<AppResult<_>>()?;
    Ok(json!({"reports": reports, "next_cursor": page.next_cursor}))
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

pub fn delete(app: &App, id: &str) -> AppResult<Value> {
    let saved = load(app, id)?;
    let removed = records::delete(app.store(), RecordKind::Report, id)?;
    app.publish("reports.changed", &json!({"id": id, "deleted": true}));
    app.audit(
        "reports.delete",
        None,
        format!("Deleted the saved report `{}`", saved.title),
        json!({"report_id": id, "kind": saved.kind}),
    );
    Ok(json!({"deleted": removed}))
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add(
        "reports.kinds",
        |app: App, _: nocturne_app::NoParams| async move { Ok(kinds(&app)) },
    );
    router.add(
        "reports.generate",
        |app: App, params: GenerateParams| async move { generate(&app, &params).await },
    );
    router.add("reports.save", |app: App, params: SaveParams| async move {
        save(&app, params).await
    });
    router.add("reports.list", |app: App, params: ListParams| async move {
        list(&app, &params)
    });
    router.add("reports.get", |app: App, params: IdParams| async move {
        serde_json::to_value(load(&app, &params.id)?).map_err(AppError::from)
    });
    router.add("reports.delete", |app: App, params: IdParams| async move {
        delete(&app, &params.id)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::model::{Device, DeviceState};
    use nocturne_drivers::model::Facts;
    use pretty_assertions::assert_eq;

    fn row(name: &str, model: &str, version: &str) -> DeviceRow {
        DeviceRow {
            device: Device {
                id: format!("id-{name}"),
                name: name.into(),
                site: "HQ".into(),
                ..Device::default()
            },
            state: DeviceState {
                facts: (!version.is_empty()).then(|| Facts {
                    model: model.into(),
                    os_version: version.into(),
                    os: "IOS".into(),
                    serial: format!("SN-{name}"),
                    uptime_s: Some(3_600),
                    ..Facts::default()
                }),
                facts_at: (!version.is_empty()).then_some(1_000),
                ..DeviceState::default()
            },
        }
    }

    #[test]
    fn the_inventory_report_counts_what_it_does_not_know() {
        let devices =
            Devices::from_rows(vec![row("SW1", "C9300-24T", "17.9.4"), row("SW2", "", "")]);
        let zone = Zone::resolve(Some(0)).unwrap();
        let document = inventory(&devices, &zone);
        let markdown = document.render(Format::Markdown);
        assert!(markdown.contains("| SW1 |"));
        assert!(markdown.contains("C9300-24T"));
        assert!(markdown.contains("1 h"), "uptime is shown as a duration");
        assert!(
            markdown.contains("1 of 2 device(s) have never had their facts collected"),
            "{markdown}"
        );
        // The uncollected device's columns are blank, not zero.
        let value: Value = serde_json::from_str(&document.render(Format::Json)).unwrap();
        let rows = value["sections"][0]["rows"].as_array().unwrap();
        assert_eq!(rows[1]["device"], "SW2");
        assert_eq!(rows[1]["model"], Value::Null);
        assert_eq!(rows[1]["serial"], Value::Null);
        assert_eq!(rows[1]["uptime"], Value::Null);
        assert_eq!(rows[1]["collected_at"], Value::Null);
        assert_eq!(document.rows(), 2);
    }

    #[test]
    fn the_software_report_finds_the_odd_one_out() {
        let devices = Devices::from_rows(vec![
            row("SW1", "C9300-24T", "17.9.4"),
            row("SW2", "C9300-24T", "17.9.4"),
            row("SW3", "C9300-24T", "16.12.1"),
            row("RTR1", "ISR4331", "17.6.5"),
            row("SW9", "", ""),
        ]);
        let document = software(&devices);
        let value: Value = serde_json::from_str(&document.render(Format::Json)).unwrap();
        let versions = value["sections"][0]["rows"].as_array().unwrap();
        assert_eq!(versions.len(), 3);
        let odd = value["sections"][1]["rows"].as_array().unwrap();
        assert_eq!(odd.len(), 1);
        assert_eq!(odd[0]["device"], "SW3");
        assert_eq!(odd[0]["version"], "16.12.1");
        assert_eq!(odd[0]["expected"], "17.9.4");
        // A model with one device is never "out of step".
        assert!(
            !value
                .to_string()
                .contains("RTR1\",\"model\":\"ISR4331\",\"version\":\"17.6.5\",\"expected")
        );
        // And a device with no facts is listed as unknown, not as a version.
        assert_eq!(value["sections"][2]["items"], json!(["SW9"]));
    }

    #[test]
    fn the_kind_list_says_what_this_build_cannot_report_on() {
        assert_eq!(KINDS.len(), 7);
        assert!(kind_info("availability").is_ok());
        let bad = kind_info("nonsense").unwrap_err();
        assert_eq!(bad.code, "invalid");
        assert!(bad.message.contains("inventory"), "{}", bad.message);
    }
}
