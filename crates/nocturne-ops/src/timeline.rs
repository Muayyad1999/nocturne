//! Gathering an incident's timeline from what really happened.
//!
//! Nothing in here writes a sentence of its own about the network. Every
//! entry is one record read from one place — an alert from the monitor, a
//! change from the change pipeline, a configuration version from the core's
//! snapshot table, a line of the audit trail — and carries a
//! [`Provenance`](crate::model::Provenance) saying which record and which of
//! its fields it came from, so the timeline can be checked rather than
//! believed.
//!
//! Two rules decide the shape of this module:
//!
//! * **Nothing is invented.** A fact with no timestamp in its source is not
//!   placed at a guessed time; it is left out, and the source's own field
//!   names are quoted in the entry's `evidence`.
//! * **Nothing is lost quietly.** A source that could not be read becomes an
//!   entry of kind `gap` at the time of the attempt, saying what is therefore
//!   possibly missing. Gaps are identified by source and reason, so a source
//!   that keeps failing leaves one mark, not one per attempt.

use nocturne_app::App;
use nocturne_app::store::{LogEntry, LogQuery};
use serde_json::json;

use crate::configs::{self, ConfigChange};
use crate::model::{Entry, EntryKind, Incident, Provenance};
use crate::refs::{Devices, Period, Ref};
use crate::sources::{self, Alert, ChangeDetail, Gap, Unavailable};

/// What one pass over the sources produced.
#[derive(Debug, Default)]
pub struct Gathered {
    /// Entries for things that happened.
    pub entries: Vec<Entry>,
    /// Entries of kind `gap`, one per source that could not be read.
    pub gaps: Vec<Entry>,
}

impl Gathered {
    fn gap(&mut self, source: &str, what: &str, gap: &Gap, at: i64) {
        self.gaps.push(Entry {
            id: format!("gap:{source}:{}", gap.reason.as_str()),
            ts: at,
            kind: EntryKind::Gap,
            text: format!(
                "{what} could not be read, so this timeline may be missing entries: {}",
                gap.message
            ),
            reference: None,
            author: String::new(),
            source: Provenance::read(source, gap.message.clone(), at),
        });
    }

    /// Everything, gaps included, which is what goes onto a timeline.
    pub fn all(self) -> Vec<Entry> {
        let mut all = self.entries;
        all.extend(self.gaps);
        all
    }
}

/// Read everything the incident points at and turn it into timeline entries.
///
/// `now` is the moment of the read: gap entries and every entry's
/// `gathered_at` are stamped with it.
pub async fn gather(app: &App, incident: &Incident, devices: &Devices, now: i64) -> Gathered {
    let mut out = Gathered::default();
    alerts_of(app, incident, devices, now, &mut out).await;
    changes_of(app, incident, devices, now, &mut out).await;
    configs_of(app, incident, devices, now, &mut out);
    audit_of(app, incident, devices, now, &mut out);
    out
}

// ------------------------------------------------------------------- alerts

/// The entries one alert contributes: it fired, it was acknowledged, it
/// resolved. Only the times the alert itself carries.
pub fn alert_entries(alert: &Alert, devices: &Devices, now: i64) -> Vec<Entry> {
    // The inventory's name is the current one; the alert's own is what it
    // recorded at the time, and is used when the device is gone.
    let on = alert
        .device_id
        .as_deref()
        .and_then(|id| devices.get(id).map(|row| row.device.name.clone()))
        .or_else(|| alert.device.clone())
        .or_else(|| alert.device_id.clone())
        .map(|name| format!(" on {name}"))
        .unwrap_or_default();
    let rule = alert
        .rule_name
        .as_deref()
        .map(|rule| format!(" [rule: {rule}]"))
        .unwrap_or_default();
    let reference = Some(Ref::new("alert", alert.id.clone()));
    let mut entries = vec![Entry {
        id: format!("alert:{}:fired", alert.id),
        ts: alert.started_at,
        kind: EntryKind::Alert,
        text: format!(
            "{} alert fired: {}{on}{rule}",
            alert.severity.as_str(),
            alert.title
        ),
        reference: reference.clone(),
        author: "nocturne".into(),
        source: Provenance::read(
            "alerts.get",
            format!("alert `{}`: started_at, severity, title", alert.id),
            now,
        ),
    }];
    if let Some(at) = alert.acked_at {
        entries.push(Entry {
            id: format!("alert:{}:acknowledged", alert.id),
            ts: at,
            kind: EntryKind::Alert,
            text: format!("Alert acknowledged: {}{on}", alert.title),
            reference: reference.clone(),
            author: String::new(),
            source: Provenance::read("alerts.get", format!("alert `{}`: acked_at", alert.id), now),
        });
    }
    if let Some(at) = alert.resolved_at {
        entries.push(Entry {
            id: format!("alert:{}:resolved", alert.id),
            ts: at,
            kind: EntryKind::Alert,
            text: format!("Alert resolved: {}{on}", alert.title),
            reference,
            author: "nocturne".into(),
            source: Provenance::read(
                "alerts.get",
                format!("alert `{}`: resolved_at", alert.id),
                now,
            ),
        });
    }
    entries
}

async fn alerts_of(
    app: &App,
    incident: &Incident,
    devices: &Devices,
    now: i64,
    out: &mut Gathered,
) {
    for id in &incident.alert_ids {
        match sources::alert(app, id).await {
            Ok(Some(alert)) => out.entries.extend(alert_entries(&alert, devices, now)),
            // The alert is linked but the monitor no longer has it. That is a
            // hole in the evidence and is said so, at the time of the read.
            Ok(None) => out.gaps.push(Entry {
                id: format!("gap:alert:{id}"),
                ts: now,
                kind: EntryKind::Gap,
                text: format!(
                    "Alert `{id}` is linked to this incident but the alert store no longer has it, so when it fired and resolved cannot be shown."
                ),
                reference: Some(Ref::new("alert", id.clone())),
                author: String::new(),
                source: Provenance::read("alerts.get", format!("alert `{id}`: not found"), now),
            }),
            Err(gap) => {
                out.gap("alerts.get", "The alert store", &gap, now);
                // One unreadable feed is one gap, not one per alert.
                break;
            }
        }
    }
}

// ------------------------------------------------------------------ changes

/// The entries one change contributes, from the times the change pipeline
/// stamped itself. A target with no result has not run and gets no entry.
pub fn change_entries(change: &ChangeDetail, devices: &Devices, now: i64) -> Vec<Entry> {
    let reference = Some(Ref::new("change", change.id.clone()));
    let origin = change
        .origin
        .as_deref()
        .map(|origin| format!(" (origin {origin})"))
        .unwrap_or_default();
    let ticket = change
        .ticket
        .as_deref()
        .filter(|ticket| !ticket.is_empty())
        .map(|ticket| format!(" [{ticket}]"))
        .unwrap_or_default();
    let evidence = |field: &str| format!("change `{}`: {field}", change.id);
    let mut entries = vec![Entry {
        id: format!("change:{}:created", change.id),
        ts: change.created_at,
        kind: EntryKind::Change,
        text: format!("Change created: {}{origin}{ticket}", change.title),
        reference: reference.clone(),
        author: change.created_by.clone().unwrap_or_default(),
        source: Provenance::read("changes.get", evidence("created_at"), now),
    }];
    if let Some(approval) = &change.approval
        && let Some(at) = approval.at.filter(|at| *at > 0)
    {
        entries.push(Entry {
            id: format!("change:{}:approved", change.id),
            ts: at,
            kind: EntryKind::Change,
            text: format!("Change approved: {}", change.title),
            reference: reference.clone(),
            author: approval.by.clone().unwrap_or_default(),
            source: Provenance::read("changes.get", evidence("approval.at"), now),
        });
    }
    let mut touched = false;
    for target in &change.targets {
        let Some(result) = &target.result else {
            continue;
        };
        if !result.touched_device() {
            continue;
        }
        let name = devices.name(&target.device_id);
        if let Some(at) = result.started() {
            touched = true;
            entries.push(Entry {
                id: format!("change:{}:{}:started", change.id, target.device_id),
                ts: at,
                kind: EntryKind::Change,
                text: format!("Started applying `{}` on {name}", change.title),
                reference: reference.clone(),
                author: change.created_by.clone().unwrap_or_default(),
                source: Provenance::read(
                    "changes.get",
                    evidence(&format!("targets[{}].result.started_at", target.device_id)),
                    now,
                ),
            });
        }
        if let Some(at) = result.finished() {
            touched = true;
            let what = match result.state.as_str() {
                "applied" => format!("Applied `{}` on {name}", change.title),
                "rolled_back" => format!("Rolled back `{}` on {name}", change.title),
                "rollback_failed" => format!(
                    "Rollback of `{}` FAILED on {name}: {}",
                    change.title,
                    result.error.as_deref().unwrap_or("no reason recorded")
                ),
                "guard_fired" => format!(
                    "The dead-man guard fired on {name} during `{}`",
                    change.title
                ),
                "failed" => format!(
                    "Applying `{}` failed on {name}: {}",
                    change.title,
                    result.error.as_deref().unwrap_or("no reason recorded")
                ),
                other => format!("`{}` finished on {name} as {other}", change.title),
            };
            entries.push(Entry {
                id: format!("change:{}:{}:{}", change.id, target.device_id, result.state),
                ts: at,
                kind: EntryKind::Change,
                text: what,
                reference: reference.clone(),
                author: change.created_by.clone().unwrap_or_default(),
                source: Provenance::read(
                    "changes.get",
                    evidence(&format!(
                        "targets[{}].result.{{state, finished_at}}",
                        target.device_id
                    )),
                    now,
                ),
            });
        }
    }
    // A change that reached a device but whose targets carry no times still
    // moved; its own `updated_at` is the only time there is, and the entry
    // says that is where it comes from.
    if !touched && change.state != "draft" && change.state != "planned" {
        entries.push(Entry {
            id: format!("change:{}:state:{}", change.id, change.state),
            ts: change.updated_at,
            kind: EntryKind::Change,
            text: format!("Change `{}` is {}", change.title, change.state),
            reference,
            author: change.created_by.clone().unwrap_or_default(),
            source: Provenance::read(
                "changes.get",
                evidence("state, updated_at (no per-device times recorded)"),
                now,
            ),
        });
    }
    entries
}

async fn changes_of(
    app: &App,
    incident: &Incident,
    devices: &Devices,
    now: i64,
    out: &mut Gathered,
) {
    for id in &incident.change_ids {
        match sources::change(app, id).await {
            Ok(Some(change)) => out.entries.extend(change_entries(&change, devices, now)),
            Ok(None) => out.gaps.push(Entry {
                id: format!("gap:change:{id}"),
                ts: now,
                kind: EntryKind::Gap,
                text: format!(
                    "Change `{id}` is linked to this incident but the change pipeline no longer has it, so what it did and when cannot be shown."
                ),
                reference: Some(Ref::new("change", id.clone())),
                author: String::new(),
                source: Provenance::read("changes.get", format!("change `{id}`: not found"), now),
            }),
            Err(gap) => {
                out.gap("changes.get", "The change pipeline", &gap, now);
                break;
            }
        }
    }
}

// ------------------------------------------------------------------ configs

/// The entry a configuration version contributes.
pub fn config_entry(change: &ConfigChange, devices: &Devices, now: i64) -> Entry {
    Entry {
        id: format!("config:{}:{}", change.device_id, change.version_id),
        ts: change.at,
        kind: EntryKind::Log,
        // The first snapshot of a device is a baseline, not a change, and
        // must not be reported as one.
        text: if change.first {
            format!(
                "Running configuration of {} recorded for the first time: {} lines ({})",
                devices.name(&change.device_id),
                change.lines,
                change.source
            )
        } else {
            format!(
                "Running configuration of {} changed: {}",
                devices.name(&change.device_id),
                change.describe()
            )
        },
        reference: Some(Ref::new("config_version", change.version_id.to_string())),
        author: String::new(),
        source: Provenance::read(
            "store.configs",
            format!(
                "configuration version {} of device `{}`: taken_at, source, note{}",
                change.version_id,
                change.device_id,
                match change.previous_version_id {
                    Some(previous) => format!("; line counts diffed against version {previous}"),
                    None => String::new(),
                }
            ),
            now,
        ),
    }
}

fn configs_of(app: &App, incident: &Incident, devices: &Devices, now: i64, out: &mut Gathered) {
    let window = incident.window(now);
    match configs::changes_in(app.store(), &incident.device_ids, window) {
        Ok((changes, truncated)) => {
            out.entries
                .extend(changes.iter().map(|change| config_entry(change, devices, now)));
            for device_id in truncated {
                out.gaps.push(Entry {
                    id: format!("gap:configs:{device_id}"),
                    ts: now,
                    kind: EntryKind::Gap,
                    text: format!(
                        "{} has more than {} stored configuration versions in this window; only the newest {} are on the timeline.",
                        devices.name(&device_id),
                        configs::MAX_PER_DEVICE,
                        configs::MAX_PER_DEVICE
                    ),
                    reference: None,
                    author: String::new(),
                    source: Provenance::read(
                        "store.configs",
                        format!("device `{device_id}`: versions truncated"),
                        now,
                    ),
                });
            }
        }
        Err(err) => out.gaps.push(Entry {
            id: "gap:configs:failed".into(),
            ts: now,
            kind: EntryKind::Gap,
            text: format!(
                "The stored configurations could not be read, so configuration changes may be missing from this timeline: {}",
                err.message
            ),
            reference: None,
            author: String::new(),
            source: Provenance::read("store.configs", err.message.clone(), now),
        }),
    }
}

// -------------------------------------------------------------------- audit

/// Audit entries made against a device by hand are commands; everything else
/// is a record of something Nocturne did.
fn audit_kind(action: &str) -> EntryKind {
    if action.starts_with("terminal.") || action == "devices.exec" || action.ends_with(".exec") {
        EntryKind::Command
    } else {
        EntryKind::Log
    }
}

/// How many audit entries one device may contribute to one timeline.
const AUDIT_PER_DEVICE: u32 = 200;

/// Whether an audit entry is this incident's own paperwork.
fn about_this_incident(entry: &LogEntry, incident_id: &str) -> bool {
    entry
        .body
        .as_ref()
        .and_then(|detail| detail.get("incident_id"))
        .and_then(|id| id.as_str())
        == Some(incident_id)
}

fn audit_entry(entry: &LogEntry, devices: &Devices, now: i64) -> Entry {
    let on = entry
        .device_id
        .as_deref()
        .map(|id| format!(" ({})", devices.name(id)))
        .unwrap_or_default();
    Entry {
        id: format!("audit:{}", entry.id),
        ts: entry.ts,
        kind: audit_kind(&entry.source),
        text: format!("{}{on}", entry.message),
        // Only what the entry's own detail names, and the first of those.
        reference: crate::audit::refs_of(entry).into_iter().next(),
        // `App::audit` records no actor, so there is nobody to name here.
        author: String::new(),
        source: Provenance::read(
            "store.logs",
            format!("audit entry {} (`{}`)", entry.id, entry.source),
            now,
        ),
    }
}

fn audit_of(app: &App, incident: &Incident, devices: &Devices, now: i64, out: &mut Gathered) {
    let window = incident.window(now);
    for device_id in &incident.device_ids {
        let query = LogQuery {
            stream: Some("audit".into()),
            device_id: Some(device_id.clone()),
            since: Some(window.since),
            until: Some(window.until),
            limit: Some(AUDIT_PER_DEVICE + 1),
            ..LogQuery::default()
        };
        match app.store().log_query(&query) {
            Ok(entries) => {
                let cut = entries.len() as u32 > AUDIT_PER_DEVICE;
                out.entries.extend(
                    entries
                        .iter()
                        .take(AUDIT_PER_DEVICE as usize)
                        // This incident's own audit entries say the same
                        // things as its `status` entries, from one step
                        // further away. Another incident's belong here.
                        .filter(|entry| !about_this_incident(entry, &incident.id))
                        .map(|entry| audit_entry(entry, devices, now)),
                );
                if cut {
                    out.gaps.push(Entry {
                        id: format!("gap:audit:{device_id}"),
                        ts: now,
                        kind: EntryKind::Gap,
                        text: format!(
                            "More than {AUDIT_PER_DEVICE} audit entries were recorded against {} in this window; only the newest {AUDIT_PER_DEVICE} are on the timeline.",
                            devices.name(device_id)
                        ),
                        reference: None,
                        author: String::new(),
                        source: Provenance::read(
                            "store.logs",
                            format!("audit entries of `{device_id}`: truncated"),
                            now,
                        ),
                    });
                }
            }
            Err(err) => {
                out.gaps.push(Entry {
                    id: "gap:audit:failed".into(),
                    ts: now,
                    kind: EntryKind::Gap,
                    text: format!(
                        "The audit trail could not be read, so what people did may be missing from this timeline: {}",
                        err.message
                    ),
                    reference: None,
                    author: String::new(),
                    source: Provenance::read("store.logs", err.message.clone(), now),
                });
                break;
            }
        }
    }
}

// ------------------------------------------------------- operator's own words

/// A note somebody wrote on the incident.
pub fn note_entry(id: &str, text: &str, author: &str, ts: i64, now: i64) -> Entry {
    Entry {
        id: format!("note:{id}"),
        ts,
        kind: EntryKind::Note,
        text: text.to_string(),
        reference: None,
        author: author.to_string(),
        source: Provenance::operator("written on the incident through `incidents.note`", now),
    }
}

/// The incident itself moved.
pub fn status_entry(id: &str, text: &str, author: &str, at: i64) -> Entry {
    Entry {
        id: format!("status:{id}"),
        ts: at,
        kind: EntryKind::Status,
        text: text.to_string(),
        reference: None,
        author: author.to_string(),
        source: Provenance::operator("recorded by the operations record itself", at),
    }
}

// ------------------------------------------------------------- suggest links

/// Something inside the incident's window that is not linked to it yet.
#[derive(Debug, Default, serde::Serialize)]
pub struct Suggestions {
    pub window: Period,
    pub alerts: Vec<crate::refs::AlertRef>,
    pub changes: Vec<crate::refs::ChangeRef>,
    pub config_changes: Vec<SuggestedConfig>,
    pub unavailable: Vec<Unavailable>,
}

#[derive(Debug, serde::Serialize)]
pub struct SuggestedConfig {
    #[serde(flatten)]
    pub change: ConfigChange,
    pub device: String,
    /// How long before the incident started this happened. Absent when it
    /// happened after the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_start_ms: Option<i64>,
    /// An applied change covers this device at this time, so somebody meant
    /// it. `false` is the interesting case.
    pub explained_by_change: bool,
}

/// Alerts, changes and configuration movement on the incident's devices
/// inside its window (extended `lead_ms` backwards) that are not linked yet.
pub async fn suggest(
    app: &App,
    incident: &Incident,
    devices: &Devices,
    now: i64,
    lead_ms: i64,
    grace_ms: i64,
) -> Suggestions {
    let incident_window = incident.window(now);
    let window = Period {
        since: incident_window.since - lead_ms,
        until: incident_window.until,
    };
    let mut out = Suggestions {
        window,
        ..Suggestions::default()
    };
    let on_device = |id: &Option<String>| match id {
        Some(id) => incident.device_ids.contains(id),
        // An alert with no device is only relevant to an incident that has
        // no devices either.
        None => incident.device_ids.is_empty(),
    };

    match sources::alerts(app, json!({"since": window.since, "limit": 500})).await {
        Ok(alerts) => {
            out.alerts = alerts
                .iter()
                .filter(|alert| !incident.alert_ids.contains(&alert.id))
                .filter(|alert| on_device(&alert.device_id))
                .filter(|alert| alert.active_in(window.since, window.until))
                .map(|alert| crate::refs::AlertRef::new(alert, devices))
                .collect();
            out.alerts.sort_by_key(|alert| alert.started_at);
        }
        Err(gap) => out.unavailable.push(gap.for_section("alerts")),
    }

    let mut applied: Vec<(Vec<String>, i64, i64)> = Vec::new();
    match sources::changes(app, json!({"limit": 500})).await {
        Ok(changes) => {
            for change in &changes {
                let targets = change.devices().unwrap_or_default();
                if change.was_executed() {
                    applied.push((targets.clone(), change.created_at, change.updated_at));
                }
                let touches = match change.devices() {
                    Some(ids) => ids.iter().any(|id| incident.device_ids.contains(id)),
                    // A change list that does not say which devices a change
                    // touched cannot be matched to the incident's devices;
                    // offering it anyway would be a guess.
                    None => false,
                };
                let in_window = window.contains(change.created_at)
                    || window.contains(change.updated_at)
                    // A change that started before the window and is still
                    // going on inside it counts too.
                    || (change.created_at < window.since && change.updated_at >= window.until);
                if touches && in_window && !incident.change_ids.contains(&change.id) {
                    out.changes.push(crate::refs::ChangeRef::from(change));
                }
            }
            out.changes.sort_by_key(|change| change.updated_at);
            out.changes.dedup_by(|a, b| a.id == b.id);
        }
        Err(gap) => out.unavailable.push(gap.for_section("changes")),
    }

    match configs::changes_in(app.store(), &incident.device_ids, window) {
        Ok((changes, _truncated)) => {
            out.config_changes = changes
                .into_iter()
                .filter(|change| !change.first)
                .map(|change| {
                    let explained = applied.iter().any(|(devices, from, to)| {
                        devices.contains(&change.device_id)
                            && change.at >= from - grace_ms
                            && change.at <= to + grace_ms
                    });
                    SuggestedConfig {
                        device: devices.name(&change.device_id),
                        before_start_ms: (change.at < incident_window.since)
                            .then(|| incident_window.since - change.at),
                        explained_by_change: explained,
                        change,
                    }
                })
                .collect();
        }
        Err(err) => out.unavailable.push(Unavailable {
            section: "config_changes".into(),
            reason: sources::Reason::Failed,
            message: err.message,
        }),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Severity;
    use pretty_assertions::assert_eq;

    fn devices() -> Devices {
        Devices::empty()
    }

    fn alert(id: &str) -> Alert {
        Alert {
            id: id.into(),
            severity: Severity::Critical,
            title: "SW1 is down".into(),
            started_at: 1_000,
            acked_at: Some(2_000),
            resolved_at: Some(5_000),
            state: Some("resolved".into()),
            device_id: Some("d1".into()),
            device: Some("SW1".into()),
            rule_id: Some("r1".into()),
            rule_name: Some("Device down".into()),
            lab: Some(false),
        }
    }

    #[test]
    fn an_alert_contributes_one_entry_per_time_it_carries() {
        let entries = alert_entries(&alert("a1"), &devices(), 9_000);
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.id.as_str(), entry.ts))
                .collect::<Vec<_>>(),
            [
                ("alert:a1:fired", 1_000),
                ("alert:a1:acknowledged", 2_000),
                ("alert:a1:resolved", 5_000)
            ]
        );
        assert!(
            entries[0]
                .text
                .contains("critical alert fired: SW1 is down on SW1")
        );
        assert!(entries[0].text.contains("[rule: Device down]"));
        assert_eq!(entries[0].source.from, "alerts.get");
        assert_eq!(
            entries[0].source.evidence,
            "alert `a1`: started_at, severity, title"
        );
        assert_eq!(entries[0].source.gathered_at, 9_000);
        assert_eq!(
            entries[0].reference,
            Some(Ref::new("alert", "a1")),
            "every entry points back at its record"
        );

        // An alert that is still firing contributes only the one entry it can.
        let firing = Alert {
            acked_at: None,
            resolved_at: None,
            ..alert("a2")
        };
        assert_eq!(alert_entries(&firing, &devices(), 9_000).len(), 1);
    }

    #[test]
    fn a_gap_entry_names_the_source_and_what_may_be_missing() {
        let mut gathered = Gathered::default();
        gathered.gap(
            "alerts.get",
            "The alert store",
            &Gap {
                reason: sources::Reason::Timeout,
                message: "`alerts.get` did not answer within 5000 ms".into(),
                code: None,
            },
            7_000,
        );
        let entry = &gathered.gaps[0];
        assert_eq!(entry.id, "gap:alerts.get:timeout");
        assert_eq!(entry.kind, EntryKind::Gap);
        assert_eq!(entry.ts, 7_000);
        assert!(entry.text.starts_with("The alert store could not be read"));
        assert!(entry.text.contains("may be missing entries"));

        // The same source failing the same way again is the same gap.
        let mut again = Gathered::default();
        again.gap(
            "alerts.get",
            "The alert store",
            &Gap {
                reason: sources::Reason::Timeout,
                message: "later".into(),
                code: None,
            },
            9_000,
        );
        assert_eq!(again.gaps[0].id, gathered.gaps[0].id);
    }

    #[test]
    fn audit_entries_keep_their_reference_and_have_no_author() {
        let entry = LogEntry {
            id: 42,
            ts: 3_000,
            stream: "audit".into(),
            device_id: Some("d1".into()),
            severity: Some(6),
            source: "changes.apply".into(),
            message: "Applied change `Shut Gi0/3`".into(),
            body: Some(json!({"change_id": "c7", "devices": 1})),
        };
        let gathered = audit_entry(&entry, &devices(), 8_000);
        assert_eq!(gathered.id, "audit:42");
        assert_eq!(gathered.kind, EntryKind::Log);
        assert_eq!(gathered.reference, Some(Ref::new("change", "c7")));
        assert_eq!(gathered.source.evidence, "audit entry 42 (`changes.apply`)");
        assert_eq!(
            gathered.author, "",
            "the core's audit trail records no actor"
        );
        assert_eq!(audit_kind("terminal.open"), EntryKind::Command);
        assert_eq!(audit_kind("devices.exec"), EntryKind::Command);
        assert_eq!(audit_kind("incidents.create"), EntryKind::Log);
        // A detail without a known reference leaves the entry unreferenced.
        let bare = LogEntry {
            body: Some(json!({"count": 3})),
            ..entry
        };
        assert_eq!(audit_entry(&bare, &devices(), 8_000).reference, None);
    }
}
