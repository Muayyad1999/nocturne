//! Incidents: what is broken, who is on it, and what happened when.
//!
//! An incident is a thin record — a title, a severity, a state, the devices
//! it touches — around a **timeline** that gathers itself. The timeline is
//! the point: every entry on it is a record that exists somewhere else and
//! says where it came from (see [`crate::timeline`]). Nothing here reaches a
//! device; the whole module reads.
//!
//! Every change to a stored incident reads it and writes it back inside one
//! transaction ([`modify`]). The ones that first ask other features what
//! happened — `update`, `link`, `sync` — do the asking before, on a copy,
//! and then apply their change to whatever the incident has become in the
//! meantime: a note written while a link was gathering its events is still
//! there afterwards.

use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence;
use crate::model::{Entry, EntryKind, FollowUp, Incident, IncidentHead, Severity, State};
use crate::records::{self, Kind, Query, Record, Tx};
use crate::refs::Devices;
use crate::render::{Cell, Document, Exhibit, Format, Section, Table};
use crate::settings;
use crate::sources::{self, Severity as AlertSeverity};
use crate::text;
use crate::timefmt::{self, Zone};
use crate::timeline;

const WHAT: &str = "incident";

/// The facet a list filters on: both state and severity, so either (or both)
/// can be narrowed in SQL.
fn facet(state: State, severity: Severity) -> String {
    format!("{}:{}", state.as_str(), severity.as_str())
}

fn record(incident: &Incident) -> AppResult<Record> {
    Ok(Record {
        id: incident.id.clone(),
        sort: incident.started_at,
        facet: facet(incident.state, incident.severity),
        head: serde_json::to_string(&incident.head())?,
        search: incident.search_text(),
        body: serde_json::to_string(incident)?,
        links: incident.links(),
    })
}

pub(crate) fn parse(body: &str) -> AppResult<Incident> {
    serde_json::from_str(body).map_err(AppError::from)
}

pub fn load(app: &App, id: &str) -> AppResult<Incident> {
    let body = records::body(app.store(), Kind::Incident, id)?
        .ok_or_else(|| AppError::not_found(format!("{WHAT} `{id}`")))?;
    parse(&body)
}

/// Store a new incident and announce it. An incident that exists already is
/// changed with [`modify`], never written over with a copy read earlier.
fn save_new(app: &App, incident: &mut Incident) -> AppResult<()> {
    incident.updated_at = now_ms();
    incident.sort_timeline();
    records::save(app.store(), Kind::Incident, &record(incident)?)?;
    app.publish("incidents.changed", &incident.head());
    Ok(())
}

/// Read the incident, change it and write it back, as part of a step that
/// may move other records too. `change` sees the incident as it is stored
/// now; when it refuses, nothing is written.
pub(crate) fn modify_in<T>(
    tx: &Tx<'_>,
    id: &str,
    change: impl FnOnce(&mut Incident) -> AppResult<T>,
) -> AppResult<(Incident, T)> {
    let body = tx
        .body(Kind::Incident, id)?
        .ok_or_else(|| AppError::not_found(format!("{WHAT} `{id}`")))?;
    let mut incident = parse(&body)?;
    let value = change(&mut incident)?;
    incident.updated_at = now_ms();
    incident.sort_timeline();
    tx.put(Kind::Incident, &record(&incident)?)?;
    Ok((incident, value))
}

/// [`modify_in`] as a step of its own, announced once it is committed.
pub(crate) fn modify<T>(
    app: &App,
    id: &str,
    change: impl FnOnce(&mut Incident) -> AppResult<T>,
) -> AppResult<(Incident, T)> {
    let (incident, value) = records::transaction(app.store(), |tx| modify_in(tx, id, change))?;
    app.publish("incidents.changed", &incident.head());
    Ok((incident, value))
}

/// An incident as `get` returns it: the record, with its devices resolved
/// against the inventory as it is now.
fn detailed(app: &App, incident: &Incident) -> AppResult<Value> {
    let mut value = serde_json::to_value(incident)?;
    let devices = Devices::load(app)?;
    value["devices"] = serde_json::to_value(
        incident
            .device_ids
            .iter()
            .map(|id| devices.reference(id))
            .collect::<Vec<_>>(),
    )?;
    value["gaps"] = json!(
        incident
            .timeline
            .iter()
            .filter(|entry| entry.kind == EntryKind::Gap)
            .count()
    );
    Ok(value)
}

/// Ask every source what happened to the things `incident` points at.
///
/// This waits for other features, so it is never done inside a transaction:
/// the entries are gathered for the incident as the caller means it to be,
/// and absorbed afterwards into the incident as it then is.
async fn gather(app: &App, incident: &Incident) -> Vec<Entry> {
    let devices = Devices::load(app).unwrap_or_else(|_| Devices::empty());
    timeline::gather(app, incident, &devices, now_ms())
        .await
        .all()
}

fn gaps(incident: &Incident) -> usize {
    incident
        .timeline
        .iter()
        .filter(|entry| entry.kind == EntryKind::Gap)
        .count()
}

fn severity_from_alert(severity: AlertSeverity) -> Severity {
    match severity {
        AlertSeverity::Critical => Severity::Sev1,
        AlertSeverity::High => Severity::Sev2,
        AlertSeverity::Medium => Severity::Sev3,
        AlertSeverity::Low | AlertSeverity::Info => Severity::Sev4,
    }
}

// --------------------------------------------------------------------- list

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    /// `open`, `mitigated`, `resolved`, or `unresolved` for the two that are
    /// not resolved.
    pub state: Option<String>,
    pub severity: Option<Severity>,
    pub device_id: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

fn states_of(state: Option<&str>) -> AppResult<Vec<State>> {
    Ok(match state {
        None | Some("") | Some("any") => vec![State::Open, State::Mitigated, State::Resolved],
        Some("unresolved") | Some("active") => vec![State::Open, State::Mitigated],
        Some("open") => vec![State::Open],
        Some("mitigated") => vec![State::Mitigated],
        Some("resolved") => vec![State::Resolved],
        Some(other) => {
            return Err(AppError::invalid(format!(
                "`state` must be open, mitigated, resolved, unresolved or any, not `{other}`"
            )));
        }
    })
}

#[derive(Debug, Serialize)]
pub struct Listed {
    pub incidents: Vec<IncidentHead>,
    pub counts: Counts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Counts {
    pub total: u64,
    pub by_state: serde_json::Map<String, Value>,
    pub by_severity: serde_json::Map<String, Value>,
}

/// Counts by state and by severity, from the facet index.
pub fn counts(app: &App) -> AppResult<Counts> {
    let mut counts = Counts::default();
    for state in [State::Open, State::Mitigated, State::Resolved] {
        counts.by_state.insert(state.as_str().into(), json!(0));
    }
    for severity in Severity::ALL {
        counts
            .by_severity
            .insert(severity.as_str().into(), json!(0));
    }
    for (facet, count) in records::facet_counts(app.store(), Kind::Incident)? {
        counts.total += count;
        let (state, severity) = facet.split_once(':').unwrap_or((facet.as_str(), ""));
        for (map, key) in [
            (&mut counts.by_state, state),
            (&mut counts.by_severity, severity),
        ] {
            let total = map.get(key).and_then(Value::as_u64).unwrap_or(0) + count;
            map.insert(key.to_string(), json!(total));
        }
    }
    Ok(counts)
}

pub fn list(app: &App, params: &ListParams) -> AppResult<Listed> {
    let states = states_of(params.state.as_deref())?;
    let severities = match params.severity {
        Some(severity) => vec![severity],
        None => Severity::ALL.to_vec(),
    };
    let facets = states
        .iter()
        .flat_map(|state| {
            severities
                .iter()
                .map(move |severity| facet(*state, *severity))
        })
        .collect();
    let page = records::page(
        app.store(),
        Kind::Incident,
        &Query {
            facets,
            words: params
                .query
                .as_deref()
                .map(records::words)
                .unwrap_or_default(),
            links: params
                .device_id
                .iter()
                .map(|id| format!("device:{id}"))
                .collect(),
            cursor: params.cursor.clone(),
            limit: params.limit.unwrap_or(50).clamp(1, 500),
        },
    )?;
    let devices = Devices::load(app)?;
    let incidents: Vec<IncidentHead> = page
        .heads
        .iter()
        .map(|head| {
            let mut head: IncidentHead = serde_json::from_str(head)?;
            head.devices = head
                .device_ids
                .iter()
                .map(|id| devices.reference(id))
                .collect();
            Ok(head)
        })
        .collect::<AppResult<_>>()?;
    Ok(Listed {
        incidents,
        counts: counts(app)?,
        next_cursor: page.next_cursor,
    })
}

// ------------------------------------------------------------------- create

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreateParams {
    pub title: Option<String>,
    pub severity: Option<Severity>,
    pub started_at: Option<i64>,
    pub device_ids: Vec<String>,
    pub alert_ids: Vec<String>,
    pub change_ids: Vec<String>,
    pub summary: String,
    pub impact: String,
    /// Open the incident from an alert: its title, time, severity and device
    /// become the incident's, and the alert is linked.
    pub from_alert: Option<String>,
    /// Who is opening it. The core has no notion of a user, so this is
    /// whatever the caller says, and empty when it says nothing.
    pub author: String,
    pub assignee: String,
}

pub async fn create(app: &App, params: CreateParams) -> AppResult<Value> {
    text::prose("an incident's summary", &params.summary)?;
    text::prose("an incident's impact", &params.impact)?;
    let now = now_ms();
    let mut incident = Incident {
        id: new_id(),
        title: params.title.clone().unwrap_or_default(),
        severity: params.severity.unwrap_or(Severity::Sev3),
        state: State::Open,
        started_at: params.started_at.unwrap_or(now),
        device_ids: params.device_ids,
        alert_ids: params.alert_ids,
        change_ids: params.change_ids,
        summary: params.summary,
        impact: params.impact,
        assignee: params.assignee,
        opened_by: params.author.clone(),
        created_at: now,
        updated_at: now,
        ..Incident::default()
    };
    let mut from = "by hand".to_string();
    if let Some(alert_id) = &params.from_alert {
        let alert = sources::alert(app, alert_id)
            .await
            .map_err(sources::Gap::into_error)?
            .ok_or_else(|| AppError::not_found(format!("alert `{alert_id}`")))?;
        if incident.title.is_empty() {
            incident.title = alert.title.clone();
        }
        if params.severity.is_none() {
            incident.severity = severity_from_alert(alert.severity);
        }
        if params.started_at.is_none() {
            incident.started_at = alert.started_at;
        }
        if let Some(device_id) = &alert.device_id
            && !incident.device_ids.contains(device_id)
        {
            incident.device_ids.push(device_id.clone());
        }
        if !incident.alert_ids.contains(alert_id) {
            incident.alert_ids.push(alert_id.clone());
        }
        from = format!("from alert `{alert_id}`");
    }
    if incident.title.trim().is_empty() {
        return Err(AppError::invalid(
            "an incident needs a title, or an alert to take one from",
        ));
    }
    incident.timeline.push(timeline::status_entry(
        &format!("{}:opened", incident.id),
        &format!(
            "Incident opened {from} as {}: {}",
            incident.severity.as_str(),
            incident.title
        ),
        &params.author,
        now,
    ));
    let gathered = gather(app, &incident).await;
    incident.absorb(gathered);
    let gaps = gaps(&incident);
    save_new(app, &mut incident)?;
    app.audit(
        "incidents.create",
        incident.device_ids.first().map(String::as_str),
        format!(
            "Opened {} incident `{}`",
            incident.severity.as_str(),
            incident.title
        ),
        json!({
            "incident_id": incident.id,
            "severity": incident.severity,
            "device_ids": incident.device_ids,
            "alert_ids": incident.alert_ids,
            "timeline_entries": incident.timeline.len(),
            "timeline_gaps": gaps,
        }),
    );
    detailed(app, &incident)
}

// ------------------------------------------------------------------- update

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct UpdateParams {
    pub id: String,
    pub title: Option<String>,
    pub severity: Option<Severity>,
    pub state: Option<State>,
    pub summary: Option<String>,
    pub impact: Option<String>,
    pub root_cause: Option<String>,
    pub follow_ups: Option<Vec<FollowUp>>,
    pub assignee: Option<String>,
    pub device_ids: Option<Vec<String>>,
    pub started_at: Option<i64>,
    /// Why, for a state change that wants a reason (reopening).
    pub note: Option<String>,
    pub author: String,
}

/// Apply the changes, stamp the times a state change implies, and leave a
/// `status` entry for each thing that moved.
fn apply_update(
    incident: &mut Incident,
    params: &UpdateParams,
    now: i64,
) -> AppResult<Vec<String>> {
    for (what, prose) in [
        ("an incident's summary", &params.summary),
        ("an incident's impact", &params.impact),
        ("an incident's root cause", &params.root_cause),
        ("the reason for a state change", &params.note),
    ] {
        if let Some(prose) = prose {
            text::prose(what, prose)?;
        }
    }
    let mut moved: Vec<String> = Vec::new();
    if let Some(title) = &params.title {
        if title.trim().is_empty() {
            return Err(AppError::invalid("an incident needs a title"));
        }
        if *title != incident.title {
            moved.push(format!("Retitled from `{}` to `{title}`", incident.title));
            incident.title = title.clone();
        }
    }
    if let Some(severity) = params.severity
        && severity != incident.severity
    {
        moved.push(format!(
            "Severity changed from {} to {}",
            incident.severity.as_str(),
            severity.as_str()
        ));
        incident.severity = severity;
    }
    if let Some(assignee) = &params.assignee
        && *assignee != incident.assignee
    {
        moved.push(if assignee.trim().is_empty() {
            "Unassigned".to_string()
        } else {
            format!("Assigned to {assignee}")
        });
        incident.assignee = assignee.clone();
    }
    if let Some(summary) = &params.summary {
        incident.summary = summary.clone();
    }
    if let Some(impact) = &params.impact {
        incident.impact = impact.clone();
    }
    if let Some(root_cause) = &params.root_cause {
        incident.root_cause = root_cause.clone();
    }
    if let Some(follow_ups) = &params.follow_ups {
        incident.follow_ups = follow_ups.clone();
    }
    if let Some(device_ids) = &params.device_ids {
        let mut ids = device_ids.clone();
        ids.sort();
        ids.dedup();
        incident.device_ids = ids;
    }
    if let Some(started_at) = params.started_at
        && started_at != incident.started_at
    {
        moved.push("The start time was corrected".to_string());
        incident.started_at = started_at;
    }
    if let Some(state) = params.state
        && state != incident.state
    {
        let was = incident.state;
        let why = params
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(|note| format!(": {note}"))
            .unwrap_or_default();
        match state {
            State::Mitigated => {
                incident.mitigated_at = Some(now);
                incident.resolved_at = None;
            }
            State::Resolved => {
                if incident.root_cause.trim().is_empty() {
                    return Err(AppError::invalid(
                        "an incident is resolved with a cause: set `root_cause` in the same call",
                    ));
                }
                incident.mitigated_at = incident.mitigated_at.or(Some(now));
                incident.resolved_at = Some(now);
            }
            State::Open => {
                // Reopening: it is happening again, so neither time holds.
                incident.mitigated_at = None;
                incident.resolved_at = None;
            }
        }
        incident.state = state;
        moved.push(format!("{} → {}{why}", was.as_str(), state.as_str()));
    }
    // The position in the timeline is part of the id, so that two updates
    // landing in the same millisecond — resolved and reopened at once, in a
    // test or on a fast machine — cannot collide and lose one of the two.
    let already = incident.timeline.len();
    for (index, what) in moved.iter().enumerate() {
        incident.timeline.push(timeline::status_entry(
            &format!("{}:{now}:{}", incident.id, already + index),
            what,
            &params.author,
            now,
        ));
    }
    Ok(moved)
}

pub async fn update(app: &App, params: UpdateParams) -> AppResult<Value> {
    let now = now_ms();
    // On a copy first: it refuses what cannot be done before anything is
    // asked of another feature, and it says which devices to gather for — a
    // device added to the incident brings its own history with it.
    let mut draft = load(app, &params.id)?;
    apply_update(&mut draft, &params, now)?;
    let gathered = gather(app, &draft).await;
    let (incident, (moved, gaps)) = modify(app, &params.id, |incident| {
        let moved = apply_update(incident, &params, now)?;
        incident.absorb(gathered);
        Ok((moved, gaps(incident)))
    })?;
    app.audit(
        "incidents.update",
        incident.device_ids.first().map(String::as_str),
        if moved.is_empty() {
            format!("Edited incident `{}`", incident.title)
        } else {
            format!(
                "Updated incident `{}`: {}",
                incident.title,
                moved.join("; ")
            )
        },
        json!({
            "incident_id": incident.id,
            "state": incident.state,
            "severity": incident.severity,
            "changed": moved,
            "timeline_gaps": gaps,
        }),
    );
    detailed(app, &incident)
}

// ---------------------------------------------------------- the usual verbs

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct IdParams {
    pub id: String,
    pub author: String,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AssignParams {
    pub id: String,
    /// Whom to. An empty string unassigns.
    pub to: String,
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, Deserialize)]
pub struct ResolveParams {
    pub id: String,
    pub root_cause: String,
    #[serde(default)]
    pub follow_ups: Option<Vec<FollowUp>>,
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, Deserialize)]
pub struct ReopenParams {
    pub id: String,
    /// Why it is being reopened. Required: a reopened incident whose reason
    /// nobody wrote down is the one nobody can explain a week later.
    pub why: String,
    #[serde(default)]
    pub author: String,
}

pub fn acknowledge(app: &App, params: IdParams) -> AppResult<Value> {
    let now = now_ms();
    let (incident, ()) = modify(app, &params.id, |incident| {
        if incident.acked_at.is_some() {
            return Err(AppError::conflict(format!(
                "this incident was already acknowledged{}",
                match incident.acked_by.as_str() {
                    "" => String::new(),
                    who => format!(" by {who}"),
                }
            )));
        }
        let who = match params.author.as_str() {
            "" => String::new(),
            who => format!(" by {who}"),
        };
        incident.acked_at = Some(now);
        incident.acked_by = params.author.clone();
        if incident.assignee.trim().is_empty() && !params.author.trim().is_empty() {
            incident.assignee = params.author.clone();
        }
        let note = params
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(|note| format!(": {note}"))
            .unwrap_or_default();
        incident.timeline.push(timeline::status_entry(
            &format!("{}:acknowledged", incident.id),
            &format!("Acknowledged{who}{note}"),
            &params.author,
            now,
        ));
        Ok(())
    })?;
    app.audit(
        "incidents.acknowledge",
        incident.device_ids.first().map(String::as_str),
        format!("Acknowledged incident `{}`", incident.title),
        json!({"incident_id": incident.id, "by": params.author}),
    );
    detailed(app, &incident)
}

pub async fn assign(app: &App, params: AssignParams) -> AppResult<Value> {
    update(
        app,
        UpdateParams {
            id: params.id,
            assignee: Some(params.to),
            author: params.author,
            ..UpdateParams::default()
        },
    )
    .await
}

pub async fn resolve(app: &App, params: ResolveParams) -> AppResult<Value> {
    if params.root_cause.trim().is_empty() {
        return Err(AppError::invalid(
            "say what the cause was: an incident resolved without one teaches nobody anything",
        ));
    }
    update(
        app,
        UpdateParams {
            id: params.id,
            state: Some(State::Resolved),
            root_cause: Some(params.root_cause),
            follow_ups: params.follow_ups,
            author: params.author,
            ..UpdateParams::default()
        },
    )
    .await
}

pub async fn reopen(app: &App, params: ReopenParams) -> AppResult<Value> {
    if params.why.trim().is_empty() {
        return Err(AppError::invalid(
            "say why it is being reopened; it goes on the timeline",
        ));
    }
    update(
        app,
        UpdateParams {
            id: params.id,
            state: Some(State::Open),
            note: Some(params.why),
            author: params.author,
            ..UpdateParams::default()
        },
    )
    .await
}

pub fn delete(app: &App, id: &str) -> AppResult<Value> {
    // The evidence that was attached stays, and stops saying it is attached
    // to an incident that is gone.
    let (incident, removed, detached) = records::transaction(app.store(), |tx| {
        let body = tx
            .body(Kind::Incident, id)?
            .ok_or_else(|| AppError::not_found(format!("{WHAT} `{id}`")))?;
        let detached = evidence::forget_incident(tx, id)?;
        Ok((parse(&body)?, tx.delete(Kind::Incident, id)?, detached))
    })?;
    app.publish("incidents.changed", &json!({"id": id, "deleted": true}));
    for item in &detached {
        app.publish("evidence.changed", item);
    }
    app.audit(
        "incidents.delete",
        incident.device_ids.first().map(String::as_str),
        format!("Deleted incident `{}`", incident.title),
        json!({"incident_id": id, "state": incident.state, "entries": incident.timeline.len()}),
    );
    Ok(json!({"deleted": removed}))
}

// -------------------------------------------------------------------- notes

#[derive(Debug, Deserialize)]
pub struct NoteParams {
    pub id: String,
    pub text: String,
    /// When what is being noted happened; defaults to now.
    #[serde(default)]
    pub ts: Option<i64>,
    #[serde(default)]
    pub author: String,
}

pub fn note(app: &App, params: NoteParams) -> AppResult<Value> {
    if params.text.trim().is_empty() {
        return Err(AppError::invalid("a note needs some text"));
    }
    text::prose("a note on an incident", &params.text)?;
    let now = now_ms();
    let (incident, ()) = modify(app, &params.id, |incident| {
        incident.timeline.push(timeline::note_entry(
            &new_id(),
            params.text.trim(),
            &params.author,
            params.ts.unwrap_or(now),
            now,
        ));
        Ok(())
    })?;
    app.audit(
        "incidents.note",
        incident.device_ids.first().map(String::as_str),
        format!("Added a note to incident `{}`", incident.title),
        json!({"incident_id": incident.id, "by": params.author}),
    );
    detailed(app, &incident)
}

// --------------------------------------------------------------------- link

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LinkParams {
    pub id: String,
    pub alert_ids: Vec<String>,
    pub change_ids: Vec<String>,
    pub device_ids: Vec<String>,
    pub author: String,
}

/// Add the ids that are not linked yet; returns what was added, in words.
fn apply_links(incident: &mut Incident, params: &LinkParams) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
    for (ids, into, what) in [
        (&params.alert_ids, &mut incident.alert_ids, "alert"),
        (&params.change_ids, &mut incident.change_ids, "change"),
        (&params.device_ids, &mut incident.device_ids, "device"),
    ] {
        for id in ids {
            if !into.contains(id) {
                into.push(id.clone());
                added.push(format!("{what} `{id}`"));
            }
        }
    }
    added
}

pub async fn link(app: &App, params: LinkParams) -> AppResult<Value> {
    // What the linked things have to say is read for the incident as it
    // will be, without holding anything: other features are being asked.
    let mut draft = load(app, &params.id)?;
    apply_links(&mut draft, &params);
    let gathered = gather(app, &draft).await;
    let (incident, (added, entries, gaps)) = modify(app, &params.id, |incident| {
        let added = apply_links(incident, &params);
        let entries = incident.absorb(gathered);
        Ok((added, entries, gaps(incident)))
    })?;
    if !added.is_empty() {
        app.audit(
            "incidents.link",
            incident.device_ids.first().map(String::as_str),
            format!(
                "Linked {} to incident `{}`",
                added.join(", "),
                incident.title
            ),
            json!({
                "incident_id": incident.id,
                "linked": added,
                "timeline_entries_added": entries,
                "timeline_gaps": gaps,
            }),
        );
    }
    let mut value = detailed(app, &incident)?;
    value["linked"] = json!(added);
    value["timeline_entries_added"] = json!(entries);
    Ok(value)
}

/// Read every source again without linking anything new.
pub async fn resync(app: &App, id: &str) -> AppResult<Value> {
    let gathered = gather(app, &load(app, id)?).await;
    let (incident, added) = modify(app, id, |incident| Ok(incident.absorb(gathered)))?;
    let mut value = detailed(app, &incident)?;
    value["timeline_entries_added"] = json!(added);
    Ok(value)
}

// ----------------------------------------------------------- suggest links

pub async fn suggest_links(app: &App, id: &str) -> AppResult<Value> {
    let incident = load(app, id)?;
    let config = settings::get(app)?;
    let devices = Devices::load(app)?;
    let suggestions = timeline::suggest(
        app,
        &incident,
        &devices,
        now_ms(),
        i64::from(config.suggest_lead_minutes) * 60_000,
        i64::from(config.unplanned_grace_minutes) * 60_000,
    )
    .await;
    Ok(serde_json::to_value(suggestions)?)
}

// ------------------------------------------------------------------- export

#[derive(Debug, Deserialize)]
pub struct ExportParams {
    pub id: String,
    #[serde(default = "markdown")]
    pub format: Format,
    #[serde(default)]
    pub utc_offset_minutes: Option<i32>,
}

fn markdown() -> Format {
    Format::Markdown
}

/// The post-incident review document. `evidence` is the appendix: the items
/// on the timeline, each with its content (see [`evidence::exhibits`]).
pub fn review(
    incident: &Incident,
    devices: &Devices,
    evidence: Vec<Exhibit>,
    zone: &Zone,
    now: i64,
) -> Document {
    let mut document = Document::new(format!("Incident review: {}", incident.title));
    document
        .fact("Incident", incident.id.clone())
        .fact("Severity", incident.severity.as_str())
        .fact("State", incident.state.as_str())
        .fact("Started", zone.datetime(incident.started_at));
    if let Some(at) = incident.mitigated_at {
        document.fact("Mitigated", zone.datetime(at));
    }
    match incident.resolved_at {
        Some(at) => {
            document
                .fact("Resolved", zone.datetime(at))
                .fact("Duration", timefmt::duration(at - incident.started_at));
        }
        None => {
            document.fact("Open for", timefmt::duration(now - incident.started_at));
        }
    }
    if !incident.assignee.trim().is_empty() {
        document.fact("Assigned to", incident.assignee.clone());
    }
    let device_names: Vec<String> = incident
        .device_ids
        .iter()
        .map(|id| devices.name(id))
        .collect();
    document
        .fact(
            "Devices",
            if device_names.is_empty() {
                "none recorded".to_string()
            } else {
                device_names.join(", ")
            },
        )
        .fact("Times shown in", zone.label())
        .fact("Generated", crate::render::generated_fact(zone, now));

    document.section(Section::text("Summary", paragraphs(&incident.summary)));
    document.section(Section::text("Impact", paragraphs(&incident.impact)));

    let mut table = Table::new(&[
        ("ts", "Time"),
        ("kind", "Kind"),
        ("text", "What happened"),
        ("author", "Who"),
        ("source", "Evidence"),
    ]);
    for entry in &incident.timeline {
        table.push(vec![
            Cell::time(zone, entry.ts),
            Cell::text(entry.kind.as_str()),
            Cell::text(&entry.text),
            if entry.author.is_empty() {
                Cell::unknown()
            } else {
                Cell::text(&entry.author)
            },
            Cell::text(format!("{} — {}", entry.source.from, entry.source.evidence)),
        ]);
    }
    document.section(Section::table("Timeline", table).with_note(
        "Every line is one record read from one place; the evidence column says which, so each line can be checked.",
    ));

    let gaps: Vec<String> = incident
        .timeline
        .iter()
        .filter(|entry| entry.kind == EntryKind::Gap)
        .map(|entry| format!("{} — {}", zone.datetime(entry.ts), entry.text))
        .collect();
    if !gaps.is_empty() {
        document.section(Section::list("Gaps in the evidence", gaps).with_note(
            "A source that could not be read. The timeline above may be missing entries from it.",
        ));
    }

    document.section(Section::text(
        "Root cause",
        paragraphs(&incident.root_cause),
    ));

    let mut follow_ups = Table::new(&[("done", "Done"), ("text", "Follow-up")]);
    for follow_up in &incident.follow_ups {
        follow_ups.push(vec![
            Cell::text(if follow_up.done { "yes" } else { "no" }),
            Cell::text(&follow_up.text),
        ]);
    }
    document.section(Section::table("Follow-ups", follow_ups));

    let mut links: Vec<String> = Vec::new();
    for id in &incident.alert_ids {
        links.push(format!("alert {id}"));
    }
    for id in &incident.change_ids {
        links.push(format!("change {id}"));
    }
    if !links.is_empty() {
        document.section(Section::list("Linked records", links));
    }
    document.section(Section::exhibits("Appendix: evidence", evidence).with_note(
        "What was captured and attached to this incident, as it is stored: redacted before it was kept, with its source, the time it was read and the SHA-256 of the stored content. Device output is shown exactly as it was printed.",
    ));
    document
}

fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn export(app: &App, params: &ExportParams) -> AppResult<Value> {
    let incident = load(app, &params.id)?;
    let zone = Zone::resolve(params.utc_offset_minutes)?;
    let now = now_ms();
    let devices = Devices::load(app)?;
    let evidence = evidence::exhibits(app, &incident, &zone)?;
    let document = review(&incident, &devices, evidence, &zone, now);
    Ok(json!({
        "title": document.title,
        "format": params.format.as_str(),
        "filename": format!(
            "incident-{}-{}.{}",
            crate::text::slug(&incident.title, 40),
            zone.stamp(incident.started_at),
            params.format.extension()
        ),
        "content": document.render(params.format),
        "rows": document.rows(),
        "generated_at": now,
    }))
}

// --------------------------------------------------------- for other views

/// The unresolved incidents, for the overview and the handover.
pub fn unresolved(app: &App) -> AppResult<Vec<IncidentHead>> {
    Ok(list(
        app,
        &ListParams {
            state: Some("unresolved".into()),
            limit: Some(500),
            ..ListParams::default()
        },
    )?
    .incidents)
}

/// Incidents that started in `[since, until)`, were resolved in it, or were
/// open for the whole of it.
pub fn in_period(app: &App, since: i64, until: i64) -> AppResult<Vec<IncidentHead>> {
    let heads = records::all(app.store(), Kind::Incident, Query::default())?;
    let devices = Devices::load(app)?;
    let mut all: Vec<IncidentHead> = heads
        .iter()
        .map(|head| serde_json::from_str::<IncidentHead>(head).map_err(AppError::from))
        .collect::<AppResult<_>>()?;
    all.retain(|head| {
        let started = (since..until).contains(&head.started_at);
        let resolved = head
            .resolved_at
            .is_some_and(|at| (since..until).contains(&at));
        let spans = head.started_at < until && head.state.is_open();
        started || resolved || spans
    });
    for head in &mut all {
        head.devices = head
            .device_ids
            .iter()
            .map(|id| devices.reference(id))
            .collect();
    }
    Ok(all)
}

// ----------------------------------------------------------------- registry

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add(
        "incidents.list",
        |app: App, params: ListParams| async move { list(&app, &params) },
    );
    router.add("incidents.get", |app: App, params: IdParams| async move {
        detailed(&app, &load(&app, &params.id)?)
    });
    router.add(
        "incidents.create",
        |app: App, params: CreateParams| async move { create(&app, params).await },
    );
    router.add(
        "incidents.update",
        |app: App, params: UpdateParams| async move { update(&app, params).await },
    );
    router.add(
        "incidents.delete",
        |app: App, params: IdParams| async move { delete(&app, &params.id) },
    );
    router.add(
        "incidents.note",
        |app: App, params: NoteParams| async move { note(&app, params) },
    );
    router.add(
        "incidents.link",
        |app: App, params: LinkParams| async move { link(&app, params).await },
    );
    router.add("incidents.sync", |app: App, params: IdParams| async move {
        resync(&app, &params.id).await
    });
    router.add(
        "incidents.acknowledge",
        |app: App, params: IdParams| async move { acknowledge(&app, params) },
    );
    router.add(
        "incidents.assign",
        |app: App, params: AssignParams| async move { assign(&app, params).await },
    );
    router.add(
        "incidents.resolve",
        |app: App, params: ResolveParams| async move { resolve(&app, params).await },
    );
    router.add(
        "incidents.reopen",
        |app: App, params: ReopenParams| async move { reopen(&app, params).await },
    );
    router.add(
        "incidents.suggest_links",
        |app: App, params: IdParams| async move { suggest_links(&app, &params.id).await },
    );
    router.add(
        "incidents.export",
        |app: App, params: ExportParams| async move { export(&app, &params) },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entry, Provenance};
    use crate::render::Body;
    use pretty_assertions::assert_eq;

    fn incident() -> Incident {
        Incident {
            id: "i1".into(),
            title: "Core switch down".into(),
            severity: Severity::Sev2,
            started_at: 1_000,
            ..Incident::default()
        }
    }

    #[test]
    fn a_state_change_stamps_the_time_and_leaves_a_status_entry() {
        let mut incident = incident();
        let moved = apply_update(
            &mut incident,
            &UpdateParams {
                id: "i1".into(),
                state: Some(State::Mitigated),
                author: "mo".into(),
                ..UpdateParams::default()
            },
            5_000,
        )
        .unwrap();
        assert_eq!(moved, ["open → mitigated"]);
        assert_eq!(incident.mitigated_at, Some(5_000));
        assert_eq!(incident.state, State::Mitigated);
        assert_eq!(incident.timeline.len(), 1);
        assert_eq!(incident.timeline[0].kind, EntryKind::Status);
        assert_eq!(incident.timeline[0].author, "mo");

        // Resolving without a cause is refused, and nothing else moves.
        let refused = apply_update(
            &mut incident,
            &UpdateParams {
                id: "i1".into(),
                state: Some(State::Resolved),
                ..UpdateParams::default()
            },
            6_000,
        )
        .unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(
            refused.message.contains("root_cause"),
            "{}",
            refused.message
        );
        assert_eq!(incident.state, State::Mitigated);

        apply_update(
            &mut incident,
            &UpdateParams {
                id: "i1".into(),
                state: Some(State::Resolved),
                root_cause: Some("a cut fibre".into()),
                ..UpdateParams::default()
            },
            7_000,
        )
        .unwrap();
        assert_eq!(incident.resolved_at, Some(7_000));
        assert_eq!(incident.mitigated_at, Some(5_000), "not overwritten");

        // Reopening clears both times and records why.
        apply_update(
            &mut incident,
            &UpdateParams {
                id: "i1".into(),
                state: Some(State::Open),
                note: Some("it is down again".into()),
                ..UpdateParams::default()
            },
            9_000,
        )
        .unwrap();
        assert_eq!((incident.resolved_at, incident.mitigated_at), (None, None));
        incident.sort_timeline();
        assert!(
            incident
                .timeline
                .last()
                .unwrap()
                .text
                .contains("resolved → open: it is down again")
        );
    }

    #[test]
    fn several_edits_in_one_call_each_leave_their_own_entry() {
        let mut incident = incident();
        let moved = apply_update(
            &mut incident,
            &UpdateParams {
                id: "i1".into(),
                title: Some("Core switch SW1 down".into()),
                severity: Some(Severity::Sev1),
                assignee: Some("mo".into()),
                summary: Some("the whole of HQ".into()),
                ..UpdateParams::default()
            },
            5_000,
        )
        .unwrap();
        assert_eq!(moved.len(), 3, "the summary is prose, not a status change");
        assert_eq!(incident.timeline.len(), 3);
        assert_eq!(
            incident
                .timeline
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["status:i1:5000:0", "status:i1:5000:1", "status:i1:5000:2"]
        );
        assert_eq!(incident.summary, "the whole of HQ");
        assert_eq!(
            apply_update(
                &mut incident,
                &UpdateParams {
                    id: "i1".into(),
                    title: Some("  ".into()),
                    ..UpdateParams::default()
                },
                6_000,
            )
            .unwrap_err()
            .code,
            "invalid"
        );
    }

    #[test]
    fn severity_comes_from_the_alert_when_an_incident_is_opened_from_one() {
        assert_eq!(severity_from_alert(AlertSeverity::Critical), Severity::Sev1);
        assert_eq!(severity_from_alert(AlertSeverity::High), Severity::Sev2);
        assert_eq!(severity_from_alert(AlertSeverity::Medium), Severity::Sev3);
        assert_eq!(severity_from_alert(AlertSeverity::Low), Severity::Sev4);
        assert_eq!(severity_from_alert(AlertSeverity::Info), Severity::Sev4);
    }

    #[test]
    fn the_list_facet_narrows_by_state_and_by_severity() {
        assert_eq!(facet(State::Open, Severity::Sev1), "open:sev1");
        assert_eq!(states_of(Some("unresolved")).unwrap().len(), 2);
        assert_eq!(states_of(None).unwrap().len(), 3);
        assert_eq!(states_of(Some("resolved")).unwrap(), [State::Resolved]);
        let bad = states_of(Some("broken")).unwrap_err();
        assert_eq!(bad.code, "invalid");
        assert!(bad.message.contains("`broken`"));
    }

    #[test]
    fn the_review_document_proves_where_each_line_came_from() {
        let mut incident = incident();
        incident.resolved_at = Some(4_600_000);
        incident.root_cause = "A cut fibre between HQ and the data centre.".into();
        incident.summary = "First paragraph.\n\nSecond paragraph.".into();
        incident.follow_ups = vec![FollowUp {
            text: "order a spare".into(),
            done: false,
        }];
        incident.timeline = vec![
            Entry {
                id: "alert:a1:fired".into(),
                ts: 1_000,
                kind: EntryKind::Alert,
                text: "critical alert fired: SW1 is down".into(),
                reference: None,
                author: "nocturne".into(),
                source: Provenance::read("alerts.get", "alert `a1`: started_at", 9_000),
            },
            Entry {
                id: "gap:changes.get:timeout".into(),
                ts: 9_000,
                kind: EntryKind::Gap,
                text: "The change pipeline could not be read".into(),
                reference: None,
                author: String::new(),
                source: Provenance::read("changes.get", "timeout", 9_000),
            },
        ];
        let zone = Zone::resolve(Some(0)).unwrap();
        let document = review(&incident, &Devices::empty(), Vec::new(), &zone, 9_000);
        let markdown = document.render(Format::Markdown);
        assert!(markdown.contains("# Incident review: Core switch down"));
        assert!(markdown.contains("**Severity:** sev2"));
        assert!(markdown.contains("**Duration:** 1 h 16 min"));
        assert!(
            markdown.contains(r"alerts.get — alert \`a1\`: started_at"),
            "{markdown}"
        );
        assert!(
            markdown.contains("## Gaps in the evidence"),
            "a hole in the evidence has its own section: {markdown}"
        );
        assert!(markdown.contains("## Root cause\n\nA cut fibre"));
        assert!(markdown.contains("First paragraph."));
        assert!(markdown.contains("| no | order a spare |"));
        assert!(
            markdown.contains("## Appendix: evidence\n")
                && markdown.trim_end().ends_with("Nothing attached."),
            "an incident without evidence says so: {markdown}"
        );
        // Two timeline rows, one gap line, one follow-up.
        assert_eq!(document.rows(), 2 + 1 + 1);
    }

    #[test]
    fn an_unresolved_incident_shows_how_long_it_has_been_open() {
        let zone = Zone::resolve(Some(0)).unwrap();
        let document = review(&incident(), &Devices::empty(), Vec::new(), &zone, 100_000);
        assert!(
            document
                .meta
                .iter()
                .any(|(name, value)| name == "Open for" && value == "1 min 39 s")
        );
        assert!(
            document
                .meta
                .iter()
                .all(|(name, _)| name != "Resolved" && name != "Duration")
        );
        // Empty prose is said to be empty rather than left out.
        let summary = document
            .sections
            .iter()
            .find(|section| section.heading == "Summary")
            .unwrap();
        assert_eq!(summary.body, Body::Text(Vec::new()));
    }
}
