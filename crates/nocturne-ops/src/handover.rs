//! The shift handover: what happened on my shift, what is still open, what
//! the next person has to watch.
//!
//! Readable in two minutes, every item a real record with its id. The period
//! is half-open, `[since, until)`, so two consecutive handovers neither
//! overlap nor leave a minute unaccounted for.
//!
//! The prose (`notes`) is the only part a person — or a model asked to tidy
//! the wording — may write. `handover.save` therefore **regenerates the data
//! sections from the period** instead of storing what the client sent: a
//! saved handover's numbers are always the server's own.

use nocturne_app::jobs::{self, JobFilter, JobState};
use nocturne_app::store::LogQuery;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, AppError, AppResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::configs::{self, ConfigChange};
use crate::evidence;
use crate::incidents;
use crate::model::{HandoverHead, SavedHandover};
use crate::reach;
use crate::records::{self, Kind, Query, Record};
use crate::refs::{AlertRef, ChangeRef, DeviceRef, Devices, JobRef, Period};
use crate::render::{Cell, Document, Format, Section, Table, missing};
use crate::settings;
use crate::sources::{self, Reason, Unavailable};
use crate::text;
use crate::timefmt::{self, Zone};

const WHAT: &str = "handover";
const SCAN: usize = 1_000;

#[derive(Debug, Deserialize)]
pub struct GenerateParams {
    /// The start of the shift, in ms since the epoch.
    pub since: i64,
    /// The end; now when it is left out.
    #[serde(default)]
    pub until: Option<i64>,
}

/// Collects sections and the reasons the missing ones are missing.
struct Sections {
    object: serde_json::Map<String, Value>,
    unavailable: Vec<Unavailable>,
}

impl Sections {
    fn set(&mut self, name: &str, value: Value) {
        self.object.insert(name.to_string(), value);
    }

    fn missing(&mut self, name: &str, gap: sources::Gap) {
        self.object.insert(name.to_string(), Value::Null);
        self.unavailable.push(gap.for_section(name));
    }

    fn failed(&mut self, name: &str, message: String) {
        self.object.insert(name.to_string(), Value::Null);
        self.unavailable.push(Unavailable {
            section: name.to_string(),
            reason: Reason::Failed,
            message,
        });
    }
}

/// A configuration change as the handover reports it.
#[derive(Debug, serde::Serialize)]
pub struct ReportedConfigChange {
    pub device: String,
    #[serde(flatten)]
    pub change: ConfigChange,
    /// No applied change covers this device at this time. These are the
    /// interesting ones: somebody was on the console.
    pub unplanned: bool,
    /// `false` when there was no change list to check against, so
    /// `unplanned` could not be decided.
    pub checked: bool,
}

/// A device that stopped answering during the shift.
#[derive(Debug, serde::Serialize)]
pub struct WentDown {
    #[serde(flatten)]
    pub device: DeviceRef,
    pub at: i64,
    pub down_ms: i64,
    /// Still down when the period ended.
    pub ongoing: bool,
    /// Which samples this was read from.
    pub from: String,
}

pub async fn generate(app: &App, params: &GenerateParams) -> AppResult<Value> {
    let now = now_ms();
    let until = params.until.unwrap_or(now);
    if until <= params.since {
        return Err(AppError::invalid(
            "a handover period needs `until` after `since`",
        ));
    }
    let period = Period {
        since: params.since,
        until,
    };
    let config = settings::get(app)?;
    let grace = i64::from(config.unplanned_grace_minutes) * 60_000;
    let devices = Devices::load(app)?;
    let mut sections = Sections {
        object: serde_json::Map::new(),
        unavailable: Vec::new(),
    };
    sections.set("period", serde_json::to_value(period)?);

    let (alerts, changes, backups) = tokio::join!(
        sources::alerts(app, json!({"since": period.since, "limit": SCAN})),
        sources::changes(app, json!({"limit": SCAN})),
        sources::config_devices(app, None),
    );

    match alerts {
        Ok(alerts) => {
            let reference = |alert: &sources::Alert| AlertRef::new(alert, &devices);
            sections.set(
                "alerts",
                json!({
                    "fired": alerts.iter().filter(|alert| period.contains(alert.started_at)).count(),
                    "resolved": alerts
                        .iter()
                        .filter(|alert| alert.resolved_at.is_some_and(|at| period.contains(at)))
                        .count(),
                    "still_open": alerts
                        .iter()
                        .filter(|alert| alert.is_open())
                        .map(reference)
                        .collect::<Vec<_>>(),
                    "truncated": alerts.len() >= SCAN,
                }),
            );
        }
        Err(gap) => sections.missing("alerts", gap),
    }

    // Our own records: always there.
    let incidents = incidents::in_period(app, period.since, period.until)?;
    sections.set(
        "incidents",
        json!({
            "opened": incidents
                .iter()
                .filter(|incident| period.contains(incident.started_at))
                .collect::<Vec<_>>(),
            "resolved": incidents
                .iter()
                .filter(|incident| incident.resolved_at.is_some_and(|at| period.contains(at)))
                .collect::<Vec<_>>(),
            "open": incidents
                .iter()
                .filter(|incident| incident.state.is_open())
                .collect::<Vec<_>>(),
        }),
    );

    // The windows in which a change was working on a device, for deciding
    // whether a configuration change was planned.
    let mut applied_windows: Vec<(Vec<String>, i64, i64)> = Vec::new();
    let mut have_changes = false;
    match changes {
        Ok(changes) => {
            have_changes = true;
            for change in &changes {
                if change.was_executed() {
                    applied_windows.push((
                        change.devices().unwrap_or_default(),
                        change.created_at,
                        change.updated_at,
                    ));
                }
            }
            let keep = |want: fn(&sources::ChangeSummary) -> bool| {
                changes
                    .iter()
                    .filter(|change| want(change) && period.contains(change.updated_at))
                    .map(ChangeRef::from)
                    .collect::<Vec<_>>()
            };
            sections.set(
                "changes",
                json!({
                    "applied": keep(sources::ChangeSummary::is_applied),
                    "failed_or_rolled_back": keep(sources::ChangeSummary::is_failure),
                    // Waiting now, whenever it was planned: this is what the
                    // next shift has to deal with.
                    "awaiting_approval": changes
                        .iter()
                        .filter(|change| change.is_awaiting_approval())
                        .map(ChangeRef::from)
                        .collect::<Vec<_>>(),
                    "truncated": changes.len() >= SCAN,
                }),
            );
        }
        Err(gap) => sections.missing("changes", gap),
    }

    let device_ids: Vec<String> = devices
        .rows()
        .iter()
        .map(|row| row.device.id.clone())
        .collect();
    match configs::changes_in(app.store(), &device_ids, period) {
        Ok((changes, _truncated)) => {
            let reported: Vec<ReportedConfigChange> = changes
                .into_iter()
                .map(|change| {
                    let planned = applied_windows.iter().any(|(devices, from, to)| {
                        devices.contains(&change.device_id)
                            && change.at >= from - grace
                            && change.at <= to + grace
                    });
                    ReportedConfigChange {
                        device: devices.name(&change.device_id),
                        unplanned: have_changes && !planned,
                        checked: have_changes,
                        change,
                    }
                })
                .collect();
            sections.set("config_changes", serde_json::to_value(reported)?);
        }
        Err(err) => sections.failed("config_changes", err.message),
    }

    match devices_section(app, &devices, period, config.availability_max_devices) {
        Ok(value) => sections.set("devices", value),
        Err(err) => sections.failed("devices", err.message),
    }

    match backups {
        Ok(rows) => sections.set(
            "backups",
            json!({
                "failed": rows
                    .iter()
                    .filter(|row| row.last_error.as_deref().is_some_and(|error| !error.is_empty()))
                    .map(|row| json!({
                        "device_id": row.device_id,
                        "device": row.name.clone().unwrap_or_else(|| devices.name(&row.device_id)),
                        "error": row.last_error,
                        "last_backup_at": row.running.as_ref().map(|head| head.fetched_at),
                    }))
                    .collect::<Vec<_>>(),
            }),
        ),
        Err(gap) => sections.missing("backups", gap),
    }

    match jobs::list(
        app,
        &JobFilter {
            state: Some("finished".into()),
            limit: Some(SCAN),
            ..JobFilter::default()
        },
    ) {
        Ok(finished) => sections.set(
            "jobs_failed",
            serde_json::to_value(
                finished
                    .iter()
                    .filter(|job| job.state == JobState::Failed)
                    .filter(|job| job.finished_at.is_some_and(|at| period.contains(at)))
                    .map(JobRef::from)
                    .collect::<Vec<_>>(),
            )?,
        ),
        Err(err) => sections.failed("jobs_failed", err.message),
    }

    // Our own records again: what was captured during the shift. The items
    // themselves, with their ids and hashes, not their content — a handover
    // is read in two minutes and points at what there is to read.
    match evidence::captured_in(app, period) {
        Ok(captured) => sections.set("evidence", json!({"captured": captured})),
        Err(err) => sections.failed("evidence", err.message),
    }

    sections.set("notes", json!(""));
    sections.set("unavailable", serde_json::to_value(&sections.unavailable)?);
    sections.set("generated_at", json!(now));
    Ok(Value::Object(sections.object))
}

/// Devices added, removed, that went down, and that are down now.
fn devices_section(
    app: &App,
    devices: &Devices,
    period: Period,
    max_measured: usize,
) -> AppResult<Value> {
    let added: Vec<DeviceRef> = devices
        .rows()
        .iter()
        .filter(|row| period.contains(row.device.created_at))
        .map(DeviceRef::from)
        .collect();

    // A deleted device leaves no record but the audit trail, so that is
    // where "removed" comes from, name and all.
    let removed: Vec<Value> = app
        .store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            since: Some(period.since),
            until: Some(period.until - 1),
            text: Some("device.delete".into()),
            limit: Some(500),
            ..LogQuery::default()
        })?
        .iter()
        .filter(|entry| entry.source == "device.delete")
        .map(|entry| {
            json!({
                "device_id": entry.device_id,
                "name": quoted(&entry.message).unwrap_or_else(|| entry
                    .device_id
                    .clone()
                    .unwrap_or_default()),
                "at": entry.ts,
                "from": format!("audit entry {}", entry.id),
            })
        })
        .collect();

    let mut went_down: Vec<WentDown> = Vec::new();
    let mut not_measured = 0usize;
    for row in devices.rows().iter().take(max_measured) {
        match reach::of_device(app.store(), &row.device, period.since, period.until)? {
            Some((series, analysis)) => {
                for outage in &analysis.outages {
                    if period.contains(outage.start) {
                        went_down.push(WentDown {
                            device: DeviceRef::from(row),
                            at: outage.start,
                            down_ms: outage.down_ms,
                            ongoing: outage.ongoing,
                            from: format!(
                                "`{}` samples of device `{}` ({})",
                                reach::METRIC,
                                row.device.id,
                                series.resolution.label()
                            ),
                        });
                    }
                }
            }
            // Nothing has ever recorded this device's reachability, so
            // whether it went down is unknown — not "no".
            None => not_measured += 1,
        }
    }
    went_down.sort_by_key(|down| down.at);

    Ok(json!({
        "added": added,
        "removed": removed,
        "went_down": went_down,
        "unreachable_now": devices
            .rows()
            .iter()
            .filter(|row| row.state.reachable == Some(false))
            .map(DeviceRef::from)
            .collect::<Vec<_>>(),
        // Honesty about the measurement: these devices have no reachability
        // samples at all, so they cannot appear in `went_down`.
        "without_reachability_samples": not_measured,
        "not_measured": devices.rows().len().saturating_sub(max_measured),
    }))
}

/// The text between the first pair of backticks, which is how the core's
/// audit messages carry a device's name.
fn quoted(message: &str) -> Option<String> {
    let (_, rest) = message.split_once('`')?;
    let (name, _) = rest.split_once('`')?;
    (!name.is_empty()).then(|| name.to_string())
}

// --------------------------------------------------------------------- save

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    /// The generated document. Only its `period` is read: the data sections
    /// are generated again here, so nothing a client or a model wrote can
    /// end up in them.
    pub handover: Option<Value>,
    /// The period, when the whole document is not being sent back.
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// The human part of the handover. The only field that is stored as sent.
    pub notes: String,
    pub author: String,
    /// Update a handover saved earlier instead of making a new one.
    pub id: Option<String>,
}

pub async fn save(app: &App, params: SaveParams) -> AppResult<Value> {
    text::prose("a handover's notes", &params.notes)?;
    let from_document = params
        .handover
        .as_ref()
        .and_then(|document| document.get("period").cloned())
        .and_then(|period| serde_json::from_value::<Period>(period).ok());
    let period = match (from_document, params.since, params.until) {
        (Some(period), _, _) => period,
        (None, Some(since), until) => Period {
            since,
            until: until.unwrap_or_else(now_ms),
        },
        (None, None, _) => {
            return Err(AppError::invalid(
                "say which period to save: send the generated handover back, or `since` and `until`",
            ));
        }
    };
    // Generated here, never taken from the request.
    let data = generate(
        app,
        &GenerateParams {
            since: period.since,
            until: Some(period.until),
        },
    )
    .await?;
    let now = now_ms();
    let existing = match &params.id {
        Some(id) => Some(load(app, id)?),
        None => None,
    };
    let mut saved = SavedHandover {
        id: params.id.clone().unwrap_or_else(new_id),
        period,
        data,
        notes: params.notes.clone(),
        author: params.author.clone(),
        created_at: existing.as_ref().map_or(now, |saved| saved.created_at),
        updated_at: now,
    };
    if saved.author.is_empty()
        && let Some(existing) = &existing
    {
        saved.author = existing.author.clone();
    }
    store(app, &saved)?;
    app.publish("handover.changed", &saved.head());
    app.audit(
        "handover.save",
        None,
        format!(
            "Saved the shift handover for {} to {}",
            saved.period.since, saved.period.until
        ),
        json!({
            "handover_id": saved.id,
            "period": saved.period,
            "data_regenerated": true,
        }),
    );
    Ok(serde_json::to_value(&saved)?)
}

fn store(app: &App, saved: &SavedHandover) -> AppResult<()> {
    records::save(
        app.store(),
        Kind::Handover,
        &Record {
            id: saved.id.clone(),
            sort: saved.period.until,
            facet: String::new(),
            head: serde_json::to_string(&saved.head())?,
            search: saved.search_text(),
            body: serde_json::to_string(saved)?,
            links: Vec::new(),
        },
    )
}

pub fn load(app: &App, id: &str) -> AppResult<SavedHandover> {
    let body = records::body(app.store(), Kind::Handover, id)?
        .ok_or_else(|| AppError::not_found(format!("{WHAT} `{id}`")))?;
    serde_json::from_str(&body).map_err(AppError::from)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

pub fn list(app: &App, params: &ListParams) -> AppResult<Value> {
    let page = records::page(
        app.store(),
        Kind::Handover,
        &Query {
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
    let handovers: Vec<HandoverHead> = page
        .heads
        .iter()
        .map(|head| serde_json::from_str(head).map_err(AppError::from))
        .collect::<AppResult<_>>()?;
    Ok(json!({"handovers": handovers, "next_cursor": page.next_cursor}))
}

pub fn delete(app: &App, id: &str) -> AppResult<Value> {
    let saved = load(app, id)?;
    let removed = records::delete(app.store(), Kind::Handover, id)?;
    app.publish("handover.changed", &json!({"id": id, "deleted": true}));
    app.audit(
        "handover.delete",
        None,
        "Deleted a saved shift handover",
        json!({"handover_id": id, "period": saved.period}),
    );
    Ok(json!({"deleted": removed}))
}

// ------------------------------------------------------------------- export

#[derive(Debug, Deserialize)]
pub struct ExportParams {
    /// A saved handover, or `since`/`until` to export one without saving it.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub until: Option<i64>,
    #[serde(default = "markdown")]
    pub format: Format,
    #[serde(default)]
    pub utc_offset_minutes: Option<i32>,
    #[serde(default)]
    pub notes: Option<String>,
}

fn markdown() -> Format {
    Format::Markdown
}

pub async fn export(app: &App, params: &ExportParams) -> AppResult<Value> {
    if let Some(notes) = &params.notes {
        text::prose("a handover's notes", notes)?;
    }
    let zone = Zone::resolve(params.utc_offset_minutes)?;
    let (period, data, notes) = match &params.id {
        Some(id) => {
            let saved = load(app, id)?;
            (saved.period, saved.data, saved.notes)
        }
        None => {
            let since = params.since.ok_or_else(|| {
                AppError::invalid("say which handover to export: an `id`, or `since` and `until`")
            })?;
            let data = generate(
                app,
                &GenerateParams {
                    since,
                    until: params.until,
                },
            )
            .await?;
            let period = serde_json::from_value(data["period"].clone())?;
            (period, data, params.notes.clone().unwrap_or_default())
        }
    };
    let now = now_ms();
    let document = document(&data, period, &notes, &zone, now);
    Ok(json!({
        "title": document.title,
        "format": params.format.as_str(),
        "filename": format!(
            "handover-{}.{}",
            zone.stamp(period.until),
            params.format.extension()
        ),
        "content": document.render(params.format),
        "rows": document.rows(),
        "generated_at": now,
    }))
}

/// The handover as a document: short, in order of what the next person needs.
pub fn document(data: &Value, period: Period, notes: &str, zone: &Zone, now: i64) -> Document {
    let mut document = Document::new("Shift handover");
    document
        .fact(
            "Shift",
            crate::render::period_fact(zone, period.since, period.until),
        )
        .fact("Times shown in", zone.label())
        .fact("Generated", crate::render::generated_fact(zone, now));

    document.section(Section::text(
        "What the next shift needs to know",
        notes
            .split("\n\n")
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
    ));

    // -------- incidents
    let mut table = Table::new(&[
        ("state", "State"),
        ("severity", "Severity"),
        ("title", "Incident"),
        ("started_at", "Started"),
        ("assignee", "With"),
    ]);
    for incident in array(data, "/incidents/open") {
        table.push(vec![
            Cell::text(string(&incident, "state")),
            Cell::text(string(&incident, "severity")),
            Cell::text(string(&incident, "title")),
            Cell::maybe(
                incident
                    .get("started_at")
                    .and_then(Value::as_i64)
                    .map(|at| Cell::time(zone, at)),
            ),
            Cell::text(string(&incident, "assignee")),
        ]);
    }
    document.section(Section::table("Still open", table).with_note(format!(
        "{} opened and {} resolved during the shift.",
        array(data, "/incidents/opened").len(),
        array(data, "/incidents/resolved").len()
    )));

    // -------- alerts
    match data.get("alerts") {
        Some(Value::Null) | None => {
            document.section(unavailable_section(data, "Alerts", "alerts"));
        }
        Some(alerts) => {
            let mut table = Table::new(&[
                ("severity", "Severity"),
                ("title", "Alert"),
                ("device", "Device"),
                ("started_at", "Firing since"),
                ("state", "State"),
            ]);
            for alert in alerts
                .get("still_open")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                table.push(vec![
                    Cell::text(string(&alert, "severity")),
                    Cell::text(string(&alert, "title")),
                    Cell::text(string(&alert, "device")),
                    Cell::maybe(
                        alert
                            .get("started_at")
                            .and_then(Value::as_i64)
                            .map(|at| Cell::time(zone, at)),
                    ),
                    Cell::text(string(&alert, "state")),
                ]);
            }
            document.section(
                Section::table("Alerts still firing", table).with_note(format!(
                    "{} fired and {} resolved during the shift.",
                    number(alerts, "fired"),
                    number(alerts, "resolved")
                )),
            );
        }
    }

    // -------- changes
    match data.get("changes") {
        Some(Value::Null) | None => {
            document.section(unavailable_section(data, "Changes", "changes"));
        }
        Some(changes) => {
            let mut table = Table::new(&[
                ("state", "State"),
                ("title", "Change"),
                ("origin", "Origin"),
                ("ticket", "Ticket"),
                ("updated_at", "When"),
            ]);
            for key in ["applied", "failed_or_rolled_back", "awaiting_approval"] {
                for change in changes
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    table.push(vec![
                        Cell::text(string(&change, "state")),
                        Cell::text(string(&change, "title")),
                        Cell::text(string(&change, "origin")),
                        Cell::text(string(&change, "ticket")),
                        Cell::maybe(
                            change
                                .get("updated_at")
                                .and_then(Value::as_i64)
                                .map(|at| Cell::time(zone, at)),
                        ),
                    ]);
                }
            }
            document.section(Section::table("Changes", table));
        }
    }

    // -------- configuration movement
    match data.get("config_changes") {
        Some(Value::Null) | None => {
            document.section(unavailable_section(
                data,
                "Configuration changes",
                "config_changes",
            ));
        }
        Some(_) => {
            let changes = array(data, "/config_changes");
            let unplanned = changes
                .iter()
                .filter(|change| change["unplanned"] == Value::Bool(true))
                .count();
            let mut table = Table::new(&[
                ("device", "Device"),
                ("at", "When"),
                ("lines_added", "+"),
                ("lines_removed", "−"),
                ("source", "Taken by"),
                ("unplanned", "Unplanned"),
            ]);
            for change in &changes {
                table.push(vec![
                    Cell::text(string(change, "device")),
                    Cell::maybe(
                        change
                            .get("at")
                            .and_then(Value::as_i64)
                            .map(|at| Cell::time(zone, at)),
                    ),
                    Cell::maybe(
                        change
                            .get("lines_added")
                            .and_then(Value::as_i64)
                            .map(Cell::int),
                    ),
                    Cell::maybe(
                        change
                            .get("lines_removed")
                            .and_then(Value::as_i64)
                            .map(Cell::int),
                    ),
                    Cell::text(string(change, "source")),
                    Cell::text(
                        match (change["unplanned"].as_bool(), change["checked"].as_bool()) {
                            (Some(true), _) => "yes",
                            (_, Some(false)) => "not checked",
                            _ => "no",
                        },
                    ),
                ]);
            }
            document.section(
                Section::table("Configuration changes", table).with_note(format!(
                    "{unplanned} of {} are not covered by an applied change.",
                    changes.len()
                )),
            );
        }
    }

    // -------- devices
    let mut table = Table::new(&[
        ("device", "Device"),
        ("what", "What"),
        ("at", "When"),
        ("detail", "Detail"),
    ]);
    for device in array(data, "/devices/went_down") {
        table.push(vec![
            Cell::text(string(&device, "name")),
            Cell::text("went down"),
            Cell::maybe(
                device
                    .get("at")
                    .and_then(Value::as_i64)
                    .map(|at| Cell::time(zone, at)),
            ),
            Cell::text(format!(
                "down for {}{}",
                timefmt::duration(device["down_ms"].as_i64().unwrap_or(0)),
                if device["ongoing"] == Value::Bool(true) {
                    ", still down"
                } else {
                    ""
                }
            )),
        ]);
    }
    for device in array(data, "/devices/added") {
        table.push(vec![
            Cell::text(string(&device, "name")),
            Cell::text("added to the inventory"),
            Cell::unknown(),
            Cell::text(string(&device, "site")),
        ]);
    }
    for device in array(data, "/devices/removed") {
        table.push(vec![
            Cell::text(string(&device, "name")),
            Cell::text("removed from the inventory"),
            Cell::maybe(
                device
                    .get("at")
                    .and_then(Value::as_i64)
                    .map(|at| Cell::time(zone, at)),
            ),
            Cell::text(string(&device, "from")),
        ]);
    }
    let unreachable = array(data, "/devices/unreachable_now");
    let unmeasured = data
        .pointer("/devices/without_reachability_samples")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    document.section(
        Section::table("Devices", table).with_note(format!(
            "{} unreachable right now. {unmeasured} device(s) have no reachability samples at all, so whether they went down is not known.",
            unreachable.len(),
        )),
    );
    if !unreachable.is_empty() {
        document.section(Section::list(
            "Unreachable right now",
            unreachable
                .iter()
                .map(|device| {
                    format!(
                        "{}{}",
                        string(device, "name"),
                        match string(device, "site").as_str() {
                            "" => String::new(),
                            site => format!(" ({site})"),
                        }
                    )
                })
                .collect(),
        ));
    }

    // -------- backups and jobs
    match data.get("backups") {
        Some(Value::Null) | None => {
            document.section(unavailable_section(data, "Backups", "backups"));
        }
        Some(_) => {
            document.section(Section::list(
                "Backups that failed",
                array(data, "/backups/failed")
                    .iter()
                    .map(|row| format!("{}: {}", string(row, "device"), string(row, "error")))
                    .collect(),
            ));
        }
    }
    document.section(Section::list(
        "Jobs that failed",
        array(data, "/jobs_failed")
            .iter()
            .map(|job| format!("{} — {}", string(job, "title"), string(job, "error")))
            .collect(),
    ));

    // -------- evidence
    match data.get("evidence") {
        // A handover saved before evidence existed has no such section, and
        // that is not the same as a shift in which nothing was captured.
        None => {}
        Some(Value::Null) => {
            document.section(unavailable_section(data, "Evidence captured", "evidence"));
        }
        Some(_) => {
            let mut table = Table::new(&[
                ("captured_at", "Captured"),
                ("kind", "Kind"),
                ("title", "Evidence"),
                ("device", "Device"),
                ("from", "Source"),
                ("incidents", "Attached to"),
                ("sha256", "SHA-256"),
                ("id", "Id"),
            ]);
            for item in array(data, "/evidence/captured") {
                let source = item.get("source").cloned().unwrap_or(Value::Null);
                let device = match (
                    string(&source, "device").as_str(),
                    source["lab"] == Value::Bool(true),
                ) {
                    ("", _) => Cell::unknown(),
                    (name, true) => Cell::text(format!("{name} (lab)")),
                    (name, false) => Cell::text(name),
                };
                table.push(vec![
                    Cell::maybe(
                        item.get("captured_at")
                            .and_then(Value::as_i64)
                            .map(|at| Cell::time(zone, at)),
                    ),
                    Cell::text(string(&item, "kind")),
                    Cell::text(string(&item, "title")),
                    device,
                    Cell::text(string(&source, "from")),
                    Cell::count(
                        item.get("incident_ids")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len),
                    ),
                    Cell::text(string(&item, "sha256")),
                    Cell::text(string(&item, "id")),
                ]);
            }
            document.section(Section::table("Evidence captured", table).with_note(
                "What was kept during the shift, by its id and the hash of what is stored. The content is in the item, redacted; it is not copied into the handover.",
            ));
        }
    }
    document
}

fn array(data: &Value, pointer: &str) -> Vec<Value> {
    data.pointer(pointer)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn string(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(value: &Value, field: &str) -> i64 {
    value.get(field).and_then(Value::as_i64).unwrap_or(0)
}

/// The section for a part whose source was not available, with the reason the
/// generated document recorded.
fn unavailable_section(data: &Value, heading: &str, section: &str) -> Section {
    let reported: Vec<Unavailable> = data
        .get("unavailable")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    match reported.into_iter().find(|entry| entry.section == section) {
        Some(entry) => Section::missing(heading, entry),
        None => missing(
            heading,
            section,
            Reason::NotInstalled,
            "this part of the handover has no source in this build",
        ),
    }
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add(
        "handover.generate",
        |app: App, params: GenerateParams| async move { generate(&app, &params).await },
    );
    router.add("handover.save", |app: App, params: SaveParams| async move {
        save(&app, params).await
    });
    router.add("handover.list", |app: App, params: ListParams| async move {
        list(&app, &params)
    });
    router.add(
        "handover.get",
        |app: App, params: GetByIdParams| async move {
            serde_json::to_value(load(&app, &params.id)?).map_err(AppError::from)
        },
    );
    router.add(
        "handover.delete",
        |app: App, params: GetByIdParams| async move { delete(&app, &params.id) },
    );
    router.add(
        "handover.export",
        |app: App, params: ExportParams| async move { export(&app, &params).await },
    );
}

#[derive(Debug, Deserialize)]
pub struct GetByIdParams {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn a_device_name_is_recovered_from_the_audit_message() {
        assert_eq!(
            quoted("Deleted device `ACC-SW7`").as_deref(),
            Some("ACC-SW7")
        );
        assert_eq!(quoted("Deleted device").as_deref(), None);
        assert_eq!(quoted("Deleted device ``").as_deref(), None);
    }

    fn generated() -> Value {
        json!({
            "period": {"since": 0, "until": 28_800_000},
            "alerts": {"fired": 4, "resolved": 3, "still_open": [
                {"id": "a1", "severity": "critical", "title": "SW1 is down", "device": "SW1",
                 "state": "firing", "started_at": 1_000}
            ]},
            "incidents": {
                "opened": [{"id": "i1", "title": "HQ outage", "severity": "sev1", "state": "open",
                            "started_at": 1_000, "assignee": "mo"}],
                "resolved": [],
                "open": [{"id": "i1", "title": "HQ outage", "severity": "sev1", "state": "open",
                          "started_at": 1_000, "assignee": "mo"}]
            },
            "changes": {"applied": [
                {"id": "c1", "title": "Shut Gi0/3", "state": "applied", "origin": "user",
                 "ticket": "CHG-9", "created_at": 10, "updated_at": 2_000}
            ], "failed_or_rolled_back": [], "awaiting_approval": []},
            "config_changes": [
                {"device": "SW2", "device_id": "d2", "version_id": 7, "at": 3_000,
                 "lines_added": 2, "lines_removed": 0, "lines": 40, "source": "scheduled",
                 "note": "", "first": false, "unplanned": true, "checked": true}
            ],
            "devices": {"added": [], "removed": [], "went_down": [
                {"id": "d1", "name": "SW1", "site": "HQ", "lab": false, "at": 1_000,
                 "down_ms": 600_000, "ongoing": true, "from": "`up` samples of device `d1` (raw samples)"}
            ], "unreachable_now": [{"id": "d1", "name": "SW1", "site": "HQ", "lab": false}],
               "without_reachability_samples": 2, "not_measured": 0},
            "backups": {"failed": [{"device_id": "d3", "device": "SW3", "error": "login failed"}]},
            "jobs_failed": [{"id": "j1", "title": "Back up 12 devices", "error": "2 devices failed"}],
            "notes": "",
            "unavailable": [],
        })
    }

    #[test]
    fn the_document_leads_with_what_the_next_shift_must_watch() {
        let zone = Zone::resolve(Some(0)).unwrap();
        let period = Period {
            since: 0,
            until: 28_800_000,
        };
        let document = document(
            &generated(),
            period,
            "Watch SW1.\n\nThe optic is ordered.",
            &zone,
            28_800_000,
        );
        let markdown = document.render(Format::Markdown);
        assert!(markdown.starts_with("# Shift handover\n"));
        assert!(markdown.contains("**Shift:** 1970-01-01 00:00 → 1970-01-01 08:00 (8 h)"));
        // The prose comes first, before any table.
        let prose = markdown.find("Watch SW1.").unwrap();
        assert!(prose < markdown.find("## Still open").unwrap());
        assert!(markdown.contains("The optic is ordered."));
        assert!(markdown.contains("| open | sev1 | HQ outage |"));
        assert!(markdown.contains("1 opened and 0 resolved during the shift."));
        assert!(markdown.contains("4 fired and 3 resolved during the shift."));
        assert!(markdown.contains("| applied | Shut Gi0/3 | user | CHG-9 |"));
        assert!(
            markdown.contains("| SW2 | 1970-01-01 00:00 | 2 | 0 | scheduled | yes |"),
            "{markdown}"
        );
        assert!(markdown.contains("1 of 1 are not covered by an applied change."));
        assert!(markdown.contains("| SW1 | went down |"));
        assert!(markdown.contains("down for 10 min, still down"));
        assert!(markdown.contains("SW3: login failed"));
        assert!(markdown.contains("Back up 12 devices — 2 devices failed"));
    }

    #[test]
    fn a_section_without_a_source_says_which_and_why() {
        let mut data = generated();
        data["alerts"] = Value::Null;
        data["changes"] = Value::Null;
        data["unavailable"] = json!([
            {"section": "alerts", "reason": "not_installed",
             "message": "`alerts.list` is not part of this build"},
        ]);
        let zone = Zone::resolve(Some(0)).unwrap();
        let document = document(&data, Period { since: 0, until: 1 }, "", &zone, 1);
        let markdown = document.render(Format::Markdown);
        assert!(
            markdown.contains(
                "## Alerts\n\n*Not available — \\`alerts.list\\` is not part of this build*"
            ),
            "{markdown}"
        );
        // A section with no recorded reason still says it has no source
        // rather than showing an empty table.
        assert!(markdown.contains(
            "## Changes\n\n*Not available — this part of the handover has no source in this build*"
        ));
        assert!(!markdown.contains("| applied |"), "no fabricated rows");
        let value: Value = serde_json::from_str(&document.render(Format::Json)).unwrap();
        let alerts = value["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|section| section["heading"] == "Alerts")
            .unwrap();
        assert_eq!(alerts["kind"], "unavailable");
        assert_eq!(alerts["reason"], "not_installed");
    }

    #[test]
    fn evidence_that_was_never_looked_for_is_not_reported_as_none_captured() {
        let zone = Zone::resolve(Some(0)).unwrap();
        let period = Period { since: 0, until: 1 };
        let render = |data: &Value| document(data, period, "", &zone, 1).render(Format::Markdown);

        // Saved before evidence existed: there is no such section, and the
        // document does not make one up to say it is empty.
        let before = render(&generated());
        assert!(!before.contains("Evidence captured"), "{before}");

        // Looked for and not readable: said, with the reason.
        let mut unreadable = generated();
        unreadable["evidence"] = Value::Null;
        unreadable["unavailable"] = json!([
            {"section": "evidence", "reason": "failed", "message": "the table could not be read"},
        ]);
        let unreadable = render(&unreadable);
        assert!(
            unreadable
                .contains("## Evidence captured\n\n*Not available — the table could not be read*"),
            "{unreadable}"
        );

        // Looked for and nothing there: an empty table, which is a fact.
        let mut none = generated();
        none["evidence"] = json!({"captured": []});
        let none = render(&none);
        assert!(none.contains("## Evidence captured\n"), "{none}");
        assert!(!none.contains("Not available — the table"), "{none}");

        // And one item: by its id and hash, with the lab mark on its device.
        let mut one = generated();
        one["evidence"] = json!({"captured": [{
            "id": "e1", "kind": "terminal", "title": "show interfaces", "captured_at": 60_000,
            "source": {"from": "terminal.snapshot", "device": "SW1", "lab": true},
            "sha256": "ab".repeat(32), "incident_ids": ["i1", "i2"],
        }]});
        let one = render(&one);
        assert!(
            one.contains(&format!(
                "| 1970-01-01 00:01 | terminal | show interfaces | SW1 (lab) | terminal.snapshot | 2 | {} | e1 |",
                "ab".repeat(32)
            )),
            "{one}"
        );
    }
}
