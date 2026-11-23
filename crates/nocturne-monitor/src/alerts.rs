//! Alerts: raising them, counting repeats, resolving them, and the RPC
//! surface around them.
//!
//! An alert is keyed by its `dedup_key` — (rule, device, instance) for a rule,
//! whatever the caller passes for `alerts.raise`. While an alert with that key
//! is open, another sighting counts a repeat; once it resolves, the next
//! sighting is a new alert with a new id, so the history says "down three
//! times this week" rather than "down, still down".
//!
//! Two things here are easy to get subtly wrong, and were:
//!
//! * **One key, one open alert** is only true if looking for the open alert
//!   and writing the new one are a single visit to the store. Polls and
//!   service checks finish on different threads and all end up here, so
//!   every change to an alert below reads and writes inside one transaction.
//! * **Whether to tell anybody is decided when there is something to tell**,
//!   not once when the alert is raised. An alert raised inside a maintenance
//!   window still owes its announcement, and pays it at the first look
//!   that finds it unmuted ([`Alert::notified_at`]); and an alert that was
//!   announced gets its "resolved" whatever window or snooze has come along
//!   since, because that message closes something people already have open.
//!
//!   A look is a sighting, an evaluation — or, for the alert that is raised
//!   once and never looked at again (a configuration change, a log rule, a
//!   detection), the scheduler's own pass over what is still owed
//!   ([`announce_owed`]). All of them ask one function, [`announce_if_due`],
//!   and that is also the one place where an alert about a lab device is
//!   kept from every channel.

use std::collections::HashMap;

use nocturne_app::model::{Device, Transport};
use nocturne_app::store::LogEntry;
use nocturne_app::{App, AppError, AppResult, inventory};
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{Alert, AlertRule, AlertState, Severity};
use crate::notify::{Audience, Stage};
use crate::settings::MonitorSettings;
use crate::state::MonitorState;
use crate::{maintenance, notify, poll};

/// What a caller knows when it wants an alert raised.
#[derive(Clone, Debug, Default)]
pub struct AlertSpec {
    pub dedup_key: String,
    pub rule_id: Option<String>,
    pub rule_name: String,
    pub device_id: Option<String>,
    pub instance: Option<String>,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub value: Option<f64>,
    /// The channels of the rule that raises this; empty means every enabled
    /// channel.
    pub notify: Vec<String>,
    /// What a caller of `alerts.raise` said about channels, which the alert
    /// keeps ([`Alert::notify`]) and which wins over the rule's: `Some` of a
    /// list is those and no others, of an empty list nobody.
    pub only: Option<Vec<String>>,
    /// When the problem was observed, where that was not just now: a rule
    /// that judges a service check from what it last found passes the time
    /// of that run. Looking at the same observation again — after the next
    /// poll, after another check on the device has run — is then not another
    /// sighting, and an observation older than the alert's last word on the
    /// matter cannot reopen it. `None` means this call is the observation.
    pub observed_at: Option<i64>,
}

// ------------------------------------------------------------------ store

fn row_to_alert(row: &Row<'_>) -> rusqlite::Result<Alert> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn one(found: rusqlite::Result<Alert>) -> rusqlite::Result<Option<Alert>> {
    match found {
        Ok(alert) => Ok(Some(alert)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(other) => Err(other),
    }
}

// The four statements below take a connection rather than the app, so that
// the lifecycle can string them together inside one transaction. There is
// deliberately no way to write an alert outside one: a bare "save" is how a
// copy read a moment ago overwrites what happened since.

fn write(conn: &Connection, alert: &Alert) -> rusqlite::Result<()> {
    let body = serde_json::to_string(alert)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT OR REPLACE INTO monitor_alerts
         (id, dedup_key, state, severity, device_id, started_at, last_seen_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            alert.id,
            alert.dedup_key,
            alert.state.as_str(),
            alert.severity.rank(),
            alert.device_id,
            alert.started_at,
            alert.last_seen_at,
            body,
        ],
    )?;
    Ok(())
}

fn by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<Alert>> {
    one(conn.query_row(
        "SELECT body FROM monitor_alerts WHERE id = ?1",
        params![id],
        row_to_alert,
    ))
}

fn open_with_key(conn: &Connection, dedup_key: &str) -> rusqlite::Result<Option<Alert>> {
    one(conn.query_row(
        "SELECT body FROM monitor_alerts WHERE dedup_key = ?1 AND state != 'resolved'
         ORDER BY started_at DESC LIMIT 1",
        params![dedup_key],
        row_to_alert,
    ))
}

/// The alert that most recently had something to say about this key,
/// resolved or not.
fn last_with_key(conn: &Connection, dedup_key: &str) -> rusqlite::Result<Option<Alert>> {
    one(conn.query_row(
        "SELECT body FROM monitor_alerts WHERE dedup_key = ?1
         ORDER BY last_seen_at DESC LIMIT 1",
        params![dedup_key],
        row_to_alert,
    ))
}

/// Which alerts still owe their firing notice, as SQL. The same text is the
/// condition of the index `monitor_alerts_owed` ([`crate::db`]): SQLite
/// only answers from a partial index when the query repeats its condition
/// term for term.
///
/// An alert raised for nobody (`notify: []`) owes nothing and never will, so
/// it is left out here rather than read and skipped on every pass.
pub(crate) const OWED: &str = "state = 'firing' \
     AND json_extract(body, '$.notified_at') IS NULL \
     AND json_extract(body, '$.notify') IS NOT '[]'";

/// The page of [`owed`]: `?3` alerts after the place (`?1`, `?2`) in the
/// order (started_at, id).
///
/// The index is named because the planner, left to choose, takes the one on
/// (state, started_at) instead — which also answers the question, by going
/// through every firing alert there is and looking inside each. Named, a
/// change that stops the index from fitting is an error the first time the
/// statement is prepared, not a pass that has quietly become a scan.
pub(crate) fn owed_query() -> String {
    format!(
        "SELECT body FROM monitor_alerts INDEXED BY monitor_alerts_owed
         WHERE {OWED} AND (started_at, id) > (?1, ?2)
         ORDER BY started_at, id LIMIT ?3"
    )
}

/// Up to `limit` alerts that owe their firing notice, oldest first, starting
/// after `after` — a place in that order, as (started_at, id).
fn owed(
    conn: &Connection,
    after: Option<&(i64, String)>,
    limit: usize,
) -> rusqlite::Result<Vec<Alert>> {
    let (started_at, id) = match after {
        Some((started_at, id)) => (*started_at, id.as_str()),
        // Before every alert there is.
        None => (i64::MIN, ""),
    };
    let mut statement = conn.prepare_cached(&owed_query())?;
    statement
        .query_map(params![started_at, id, limit as i64], row_to_alert)?
        .collect()
}

pub fn get(app: &App, id: &str) -> AppResult<Alert> {
    app.store()
        .with(|conn| by_id(conn, id))?
        .ok_or_else(|| AppError::not_found(format!("alert `{id}`")))
}

/// The alert with this key that has not resolved yet, if any.
pub fn open_by_key(app: &App, dedup_key: &str) -> AppResult<Option<Alert>> {
    app.store().with(|conn| open_with_key(conn, dedup_key))
}

/// Every alert that has not resolved yet for a device.
pub fn open_of_device(app: &App, device_id: &str) -> AppResult<Vec<Alert>> {
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM monitor_alerts WHERE device_id = ?1 AND state != 'resolved'
             ORDER BY started_at DESC",
        )?;
        statement
            .query_map(params![device_id], row_to_alert)?
            .collect()
    })
}

pub fn forget_device(app: &App, device_id: &str) -> AppResult<()> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_alerts WHERE device_id = ?1",
            params![device_id],
        )
    })?;
    Ok(())
}

/// Drop resolved alerts older than `before`.
pub fn prune(app: &App, before: i64) -> AppResult<usize> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_alerts WHERE state = 'resolved' AND last_seen_at < ?1",
            params![before],
        )
    })
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AlertQuery {
    /// `firing`, `acknowledged`, `resolved` or `open` (firing + acknowledged).
    pub state: Option<String>,
    /// At least this severe.
    pub severity: Option<Severity>,
    pub device_id: Option<String>,
    /// Only alerts seen since this time.
    pub since: Option<i64>,
    pub limit: Option<u32>,
    /// Rows already received, as a decimal string.
    pub cursor: Option<String>,
}

pub fn list(app: &App, query: &AlertQuery) -> AppResult<Vec<Alert>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 2_000) as i64;
    let offset = match &query.cursor {
        Some(cursor) if !cursor.is_empty() => cursor.parse::<i64>().map_err(|_| {
            AppError::invalid("the cursor is the number of alerts already received")
        })?,
        _ => 0,
    };
    let mut sql = String::from("SELECT body FROM monitor_alerts WHERE 1 = 1");
    match query.state.as_deref() {
        Some("open") => sql.push_str(" AND state != 'resolved'"),
        Some("firing") => sql.push_str(" AND state = 'firing'"),
        Some("acknowledged") => sql.push_str(" AND state = 'acknowledged'"),
        Some("resolved") => sql.push_str(" AND state = 'resolved'"),
        Some(other) => {
            return Err(AppError::invalid(format!(
                "`{other}` is not an alert state (firing, acknowledged, resolved or open)"
            )));
        }
        None => {}
    }
    if query.severity.is_some() {
        sql.push_str(" AND severity >= :severity");
    }
    if query.device_id.is_some() {
        sql.push_str(" AND device_id = :device_id");
    }
    if query.since.is_some() {
        sql.push_str(" AND last_seen_at >= :since");
    }
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT :limit OFFSET :offset");

    let severity = query.severity.map(Severity::rank);
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&sql)?;
        let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":limit", &limit), (":offset", &offset)];
        if let Some(severity) = &severity {
            bound.push((":severity", severity));
        }
        if let Some(device_id) = &query.device_id {
            bound.push((":device_id", device_id));
        }
        if let Some(since) = &query.since {
            bound.push((":since", since));
        }
        statement
            .query_map(bound.as_slice(), row_to_alert)?
            .collect()
    })
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub open: u64,
    pub by_severity: std::collections::BTreeMap<String, u64>,
    pub newest: Option<Alert>,
}

pub fn summary(app: &App) -> AppResult<Summary> {
    let open = list(
        app,
        &AlertQuery {
            state: Some("open".into()),
            limit: Some(2_000),
            ..AlertQuery::default()
        },
    )?;
    let mut by_severity = std::collections::BTreeMap::new();
    for alert in &open {
        *by_severity
            .entry(alert.severity.as_str().to_string())
            .or_insert(0) += 1;
    }
    Ok(Summary {
        open: open.len() as u64,
        newest: open.first().cloned(),
        by_severity,
    })
}

// -------------------------------------------------------------- lifecycle

/// What one call to [`fire`] turned out to be.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sighting {
    /// Nothing was open under the key: a new alert.
    Raised,
    /// Another sighting of the alert already open.
    Counted,
    /// An observation the key's alert has already taken into account.
    AlreadySeen,
}

/// A simulated device: one a lab made, or one reached through the simulator.
fn is_lab(device: &Device) -> bool {
    device.lab.is_some() || device.transport == Transport::Sim
}

/// What one look at an alert knows that the alert does not record itself.
#[derive(Clone, Copy, Debug)]
struct Look {
    now: i64,
    /// A maintenance window covers the alert's device.
    in_maintenance: bool,
    /// The alert is about a lab device and those are not being announced.
    quiet_lab: bool,
}

impl Look {
    /// Read before the transaction that uses it, not inside: the windows,
    /// the settings and the inventory live in the same store, behind the
    /// same lock.
    fn at(app: &App, device: Option<&Device>, now: i64) -> AppResult<Look> {
        Ok(Look {
            now,
            in_maintenance: maintenance::covers(app, device, now)?,
            // The setting is only read for the devices it is about.
            quiet_lab: device.is_some_and(is_lab) && !MonitorSettings::load(app).notify_for_lab,
        })
    }
}

/// Mark the alert announced if its firing notice is owed and may go out now.
///
/// Every notice is decided here — at a raise, at a later sighting or
/// evaluation, and on the scheduler's pass over what is still owed — so that
/// each reason to keep quiet is written once:
///
/// * it has gone out already: owed means never sent;
/// * the alert is no longer firing — somebody acknowledging it has taken it
///   on, and telling everyone else about work that is in hand is noise;
/// * a maintenance window covers it, or it is snoozed;
/// * it is about a lab device and `notify_for_lab` is off. A lab is where
///   things are broken on purpose; which feature raised the alert and which
///   channels it named make no difference to that;
/// * it was raised for nobody (`notify: []`).
///
/// All but the last can change, and the notice goes out at the first look
/// after they have.
fn announce_if_due(alert: &mut Alert, look: &Look) -> bool {
    let held = alert.notified_at.is_some()
        || alert.state != AlertState::Firing
        || alert.muted(look.now)
        || look.quiet_lab
        || alert.notify.as_ref().is_some_and(Vec::is_empty);
    if held {
        return false;
    }
    alert.notified_at = Some(look.now);
    true
}

/// Who hears about an alert: whoever `alerts.raise` named for it, and
/// otherwise the channels of its rule.
fn audience(alert: &Alert, rule_channels: &[String]) -> Audience {
    match &alert.notify {
        Some(only) => Audience::Only(only.clone()),
        None => Audience::of_rule(rule_channels),
    }
}

/// The channels of the rule an alert belongs to, as the rule stands now;
/// none (which reads as every channel) when it has no rule or the rule is
/// gone.
fn rule_channels(app: &App, alert: &Alert) -> Vec<String> {
    alert
        .rule_id
        .as_deref()
        .and_then(|id| {
            app.store()
                .get::<AlertRule>(crate::db::collections::RULES, id)
                .ok()
                .flatten()
        })
        .map(|rule| rule.notify)
        .unwrap_or_default()
}

/// Tell every screen that an alert changed, and — for the desktop shell,
/// which turns this event into a system notification and cannot work out
/// windows and snoozes for itself — whether this change is one the
/// notification channels were told about, and as what.
fn publish(app: &App, alert: &Alert, notification: Option<Stage>) {
    app.publish(
        "alerts.changed",
        &json!({"alert": alert, "notification": notification.map(Stage::as_str)}),
    );
}

/// Raise an alert, or count another sighting of one already open.
///
/// Returns the alert and whether this call is what raised it. A call that
/// brings an observation the key's alert has already seen counts nothing
/// and returns that alert as it stands — unless the alert's firing notice
/// was being held back and no longer is, in which case this call sends it.
pub fn fire(app: &App, spec: &AlertSpec, now: i64) -> AppResult<(Alert, bool)> {
    let device = spec
        .device_id
        .as_deref()
        .and_then(|id| inventory::get(app, id).ok());
    let look = Look::at(app, device.as_ref(), now)?;
    let muted = look.in_maintenance;
    let seen_at = spec.observed_at.unwrap_or(now);
    // What was read above — the windows, whether the device is a lab's — is
    // about the device this call names. The alert open under the key may be
    // about another: `alerts.raise` takes any key with any `device_id`, or
    // none. Such a call is still a sighting, but it is not a look at that
    // alert's device, and deciding a notice on it would let a second raise
    // that leaves the device out announce what a window, or the lab setting,
    // is holding back. The scheduler's pass asks about the alert's own
    // device and pays what is owed.
    let about = |alert: &Alert| alert.device_id == spec.device_id;

    let (alert, sighting, announce) = app.store().transaction(|tx| {
        let (mut alert, sighting) = match open_with_key(tx, &spec.dedup_key)? {
            Some(seen)
                if spec
                    .observed_at
                    .is_some_and(|observed| observed <= seen.last_seen_at) =>
            {
                // Nothing new was measured, so nothing is counted. But a
                // notice held back by a window or a snooze is owed from the
                // moment that ends, not from the check's next run — which
                // for an hourly check, or a daily look at a certificate, is
                // a long time to say nothing about an outage that is still
                // on. The alert is open and this is the latest anybody
                // knows of it, so this look pays what is owed.
                let mut alert = seen.clone();
                alert.muted_by_maintenance = muted;
                if !about(&alert) || !announce_if_due(&mut alert, &look) {
                    return Ok((seen, Sighting::AlreadySeen, false));
                }
                write(tx, &alert)?;
                return Ok((alert, Sighting::AlreadySeen, true));
            }
            Some(mut alert) => {
                alert.count += 1;
                alert.last_seen_at = alert.last_seen_at.max(seen_at);
                alert.value = spec.value;
                alert.detail = spec.detail.clone();
                alert.severity = spec.severity;
                if about(&alert) {
                    alert.muted_by_maintenance = muted;
                }
                // Who is to be told follows the latest call until somebody
                // has been; after that it stays, so that the "resolved"
                // reaches the people who got the "firing". Never for a
                // rule's alert: a caller of `alerts.raise` that used a
                // rule's key could otherwise route the rule's notice to
                // nobody.
                if alert.notified_at.is_none() && alert.rule_id.is_none() {
                    alert.notify = spec.only.clone();
                }
                (alert, Sighting::Counted)
            }
            None => {
                // An observation from before the key's last alert resolved
                // is what that alert was about, not a new problem. Without
                // this, an evaluation that read a check's `down` a moment
                // before the run that cleared it raises the alert again
                // behind the resolve, and an alert resolved by hand is back
                // at the next poll with nothing new having been measured.
                if let Some(observed) = spec.observed_at
                    && let Some(last) = last_with_key(tx, &spec.dedup_key)?
                    && observed < last.last_seen_at
                {
                    return Ok((last, Sighting::AlreadySeen, false));
                }
                let alert = Alert {
                    id: nocturne_app::util::new_id(),
                    rule_id: spec.rule_id.clone(),
                    rule_name: spec.rule_name.clone(),
                    device_id: spec.device_id.clone(),
                    device: device.as_ref().map(|device| device.name.clone()),
                    instance: spec.instance.clone(),
                    severity: spec.severity,
                    state: AlertState::Firing,
                    title: spec.title.clone(),
                    detail: spec.detail.clone(),
                    value: spec.value,
                    // It cannot have started after the sighting that raised it.
                    started_at: now.min(seen_at),
                    last_seen_at: seen_at,
                    resolved_at: None,
                    acked_at: None,
                    ack_note: None,
                    snoozed_until: None,
                    count: 1,
                    muted_by_maintenance: muted,
                    notified_at: None,
                    notify: spec.only.clone(),
                    lab: device.as_ref().is_some_and(is_lab),
                    dedup_key: spec.dedup_key.clone(),
                };
                (alert, Sighting::Raised)
            }
        };
        let announce = about(&alert) && announce_if_due(&mut alert, &look);
        write(tx, &alert)?;
        Ok((alert, sighting, announce))
    })?;

    match sighting {
        Sighting::AlreadySeen if !announce => return Ok((alert, false)),
        Sighting::Raised => record(app, &alert, "raised"),
        Sighting::AlreadySeen | Sighting::Counted => {}
    }
    publish(app, &alert, announce.then_some(Stage::Firing));
    if announce {
        notify::send(app, &alert, Stage::Firing, &audience(&alert, &spec.notify));
    }
    Ok((alert, sighting == Sighting::Raised))
}

/// How many alerts one pass of [`announce_owed`] looks at.
const OWED_PAGE: usize = 200;

/// Send the firing notices that were being held back and no longer are.
///
/// An alert raised while a maintenance window covers it, or snoozed before
/// anybody was told, owes its announcement, and until this existed only
/// another look at the alert paid it: the next sighting, the next
/// evaluation. Plenty of alerts never get one. A configuration change is
/// its own alert per version; a log rule or a detection raises once; a
/// threshold alert sitting between its two thresholds is evaluated and not
/// sighted. Those stayed unannounced for good once the window had closed.
/// So the scheduler comes by ([`crate::scheduler`]) and asks, of every alert
/// that is open, firing and never announced, the question a sighting would
/// have asked — through [`announce_if_due`], like every other notice.
///
/// The cost is bounded twice over. Only alerts that still owe a notice are
/// read at all (an index holds exactly those, and in a quiet installation
/// it is empty); and one pass reads at most [`OWED_PAGE`] of them, going on
/// from where the last pass stopped. A lab with a thousand alerts that are
/// held back for as long as `notify_for_lab` is off costs a pass two hundred
/// rows, not a thousand, and delays the others by a few passes at most.
///
/// Returns the alerts it announced.
pub fn announce_owed(app: &App, now: i64) -> AppResult<Vec<Alert>> {
    let state = app.state::<MonitorState>();
    let after = state.runtime.lock().owed_after.clone();
    let page = app
        .store()
        .with(|conn| owed(conn, after.as_ref(), OWED_PAGE))?;
    // A short page is the end of the list: start again from the oldest.
    state.runtime.lock().owed_after = (page.len() == OWED_PAGE)
        .then(|| page.last().map(|last| (last.started_at, last.id.clone())))
        .flatten();
    if page.is_empty() {
        return Ok(Vec::new());
    }

    let settings = MonitorSettings::load(app);
    let windows = maintenance::list(app)?;
    let mut devices: HashMap<String, Option<Device>> = HashMap::new();
    let mut announced = Vec::new();
    for owing in page {
        // The two that need nothing read, first.
        if owing.snoozed_until.is_some_and(|until| until > now)
            || (owing.lab && !settings.notify_for_lab)
        {
            continue;
        }
        let device = owing.device_id.as_ref().and_then(|id| {
            devices
                .entry(id.clone())
                .or_insert_with(|| inventory::get(app, id).ok())
                .clone()
        });
        let look = Look {
            now,
            in_maintenance: maintenance::covers_among(&windows, device.as_ref(), now),
            quiet_lab: (owing.lab || device.as_ref().is_some_and(is_lab))
                && !settings.notify_for_lab,
        };
        if look.in_maintenance
            || look.quiet_lab
            || awaits_its_first_poll(&state, &settings, &owing, device.as_ref())
        {
            continue;
        }
        let paid = app.store().transaction(|tx| {
            // As it is now, not as the page had it: it may have been
            // acknowledged, snoozed, resolved or announced since.
            let Some(mut alert) = by_id(tx, &owing.id)? else {
                return Ok(None);
            };
            alert.muted_by_maintenance = look.in_maintenance;
            if !announce_if_due(&mut alert, &look) {
                return Ok(None);
            }
            write(tx, &alert)?;
            Ok(Some(alert))
        })?;
        if let Some(alert) = paid {
            publish(app, &alert, Some(Stage::Firing));
            let channels = rule_channels(app, &alert);
            notify::send(app, &alert, Stage::Firing, &audience(&alert, &channels));
            announced.push(alert);
        }
    }
    Ok(announced)
}

/// Whether an alert is a rule's word from before the application started,
/// about a device whose first poll since then is still to come.
///
/// What the rules knew when the application was closed can be hours old.
/// The device's first poll looks again and either sights the problem — which
/// announces it — or finds it gone and resolves it without a word;
/// announcing it a few seconds before that, on yesterday's knowledge, would
/// wake people for an outage that may have ended overnight. Anything else
/// does not wait: an alert raised since the start, an alert no rule owns,
/// an alert about a device that nothing polls.
fn awaits_its_first_poll(
    state: &MonitorState,
    settings: &MonitorSettings,
    alert: &Alert,
    device: Option<&Device>,
) -> bool {
    let Some(device) = device else {
        return false;
    };
    alert.rule_id.is_some()
        && alert.last_seen_at < state.started_at
        && settings.enabled
        && device.monitor.enabled
        && poll::plan(device).any()
        && !state.device(&device.id, |runtime| runtime.evaluated)
}

/// Which alert a resolve is aimed at.
#[derive(Clone, Copy)]
enum Target<'a> {
    /// Whatever is open under this key.
    Key(&'a str),
    /// This alert and no other.
    Id(&'a str),
    /// Whatever is open under this key, unless a rule owns it: what another
    /// feature may clear (`alerts.clear`).
    Raised(&'a str),
}

fn open_target(conn: &Connection, target: Target<'_>) -> rusqlite::Result<Option<Alert>> {
    match target {
        Target::Key(dedup_key) => open_with_key(conn, dedup_key),
        Target::Id(id) => Ok(by_id(conn, id)?.filter(|alert| alert.state.is_open())),
        Target::Raised(dedup_key) => {
            Ok(open_with_key(conn, dedup_key)?.filter(|alert| alert.rule_id.is_none()))
        }
    }
}

/// Resolve the open alert with this key, if there is one.
///
/// `observed_at` is when whatever says the problem is over was observed, for
/// a caller that judges from stored state (see [`AlertSpec::observed_at`]):
/// an all-clear older than the alert's last sighting is out of date and
/// resolves nothing. `None` resolves unconditionally.
pub fn resolve_key(
    app: &App,
    dedup_key: &str,
    now: i64,
    observed_at: Option<i64>,
    note: Option<&str>,
    channels: &[String],
) -> AppResult<Option<Alert>> {
    resolve(
        app,
        Target::Key(dedup_key),
        now,
        observed_at,
        note,
        channels,
    )
}

fn resolve(
    app: &App,
    target: Target<'_>,
    now: i64,
    observed_at: Option<i64>,
    note: Option<&str>,
    channels: &[String],
) -> AppResult<Option<Alert>> {
    // A first look, only to learn whose maintenance windows apply: they
    // cannot be read inside the transaction (same store, same lock), and an
    // alert's device never changes.
    let Some(open) = app.store().with(|conn| open_target(conn, target))? else {
        return Ok(None);
    };
    let device = open
        .device_id
        .as_deref()
        .and_then(|id| inventory::get(app, id).ok());
    let muted = maintenance::covers(app, device.as_ref(), now)?;

    let resolved = app.store().transaction(|tx| {
        let Some(mut alert) = open_target(tx, target)? else {
            return Ok(None);
        };
        if observed_at.is_some_and(|observed| observed < alert.last_seen_at) {
            return Ok(None);
        }
        // A poll's evaluation is dated when the poll set out, which can be
        // before the run that cleared the alert. Dated that early, the
        // resolve would sit behind a sighting the alert has already
        // counted, and an evaluation still holding that sighting would
        // pass it off as news and raise the alert again.
        let at = observed_at.map_or(now, |observed| observed.max(now));
        alert.state = AlertState::Resolved;
        alert.resolved_at = Some(at);
        alert.last_seen_at = alert.last_seen_at.max(at);
        if let Some(note) = note {
            alert.detail = note.to_string();
        }
        // As of now, like every other sighting: the record should not go on
        // saying "in maintenance" because the last sighting happened to be.
        alert.muted_by_maintenance = muted;
        write(tx, &alert)?;
        Ok(Some(alert))
    })?;
    let Some(alert) = resolved else {
        return Ok(None);
    };
    record(app, &alert, "resolved");
    // The other half of a notification people already have: sent exactly
    // when the firing notice was, whatever mutes the alert by now. Dropping
    // it leaves an incident open in a chat or a pager for ever; sending it
    // for an alert that was never announced would be the first anybody
    // heard of the matter.
    let announced = alert.notified_at.is_some();
    publish(app, &alert, announced.then_some(Stage::Resolved));
    if announced {
        notify::send(app, &alert, Stage::Resolved, &audience(&alert, channels));
    }
    Ok(Some(alert))
}

/// Close what another feature raised under this key: the condition it was
/// raised for is over (`alerts.clear`).
///
/// Never an alert an alert rule owns, and that is decided inside the
/// transaction that resolves, on the alert as it is then — the look the
/// method takes first, to say `forbidden` rather than "nothing was open",
/// can be out of date by the time this runs.
pub(crate) fn clear_raised(
    app: &App,
    dedup_key: &str,
    now: i64,
    note: Option<&str>,
) -> AppResult<Option<Alert>> {
    resolve(app, Target::Raised(dedup_key), now, None, note, &[])
}

/// Change one alert in a single visit to the store, so that a sighting
/// counted while somebody was clicking is not overwritten by the copy their
/// click had read.
fn amend(
    app: &App,
    id: &str,
    change: impl FnOnce(&mut Alert) -> AppResult<()>,
) -> AppResult<Alert> {
    app.store().transaction(|tx| {
        let Some(mut alert) = by_id(tx, id)? else {
            return Ok(Err(AppError::not_found(format!("alert `{id}`"))));
        };
        Ok(match change(&mut alert) {
            Ok(()) => {
                write(tx, &alert)?;
                Ok(alert)
            }
            Err(refused) => Err(refused),
        })
    })?
}

fn record(app: &App, alert: &Alert, what: &str) {
    let _ = app.store().log_append(&LogEntry {
        id: 0,
        ts: alert.last_seen_at,
        stream: "event".into(),
        device_id: alert.device_id.clone(),
        severity: Some(alert.severity.syslog()),
        source: "alert".into(),
        message: format!("Alert {what}: {}", alert.title),
        body: Some(json!({
            "alert_id": alert.id,
            "rule": alert.rule_name,
            "severity": alert.severity,
            "instance": alert.instance,
            "value": alert.value,
            "muted_by_maintenance": alert.muted_by_maintenance,
        })),
    });
}

// -------------------------------------------------------------------- RPC

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NoteParams {
    pub id: String,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SnoozeParams {
    pub id: String,
    pub minutes: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RaiseParams {
    /// The name of whatever is raising this; shown instead of a rule name.
    pub rule: Option<String>,
    pub device_id: Option<String>,
    pub instance: Option<String>,
    pub severity: Severity,
    pub title: String,
    pub detail: Option<String>,
    pub dedup_key: String,
    /// Channel ids. Absent (or `null`) is the usual routing, an empty list
    /// is nobody, a list is those channels and no others.
    pub notify: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ClearParams {
    pub dedup_key: String,
    pub note: Option<String>,
}

impl Default for RaiseParams {
    fn default() -> Self {
        RaiseParams {
            rule: None,
            device_id: None,
            instance: None,
            severity: Severity::Medium,
            title: String::new(),
            detail: None,
            dedup_key: String::new(),
            notify: None,
        }
    }
}

pub fn register(router: &mut nocturne_app::Router) {
    router.add("alerts.list", |app: App, params: AlertQuery| async move {
        list(&app, &params)
    });
    router.add("alerts.get", |app: App, params: IdParams| async move {
        get(&app, &params.id)
    });
    router.add(
        "alerts.summary",
        |app: App, _: nocturne_app::NoParams| async move { summary(&app) },
    );
    router.add("alerts.ack", |app: App, params: NoteParams| async move {
        let now = app.state::<MonitorState>().now();
        let alert = amend(&app, &params.id, |alert| {
            if alert.state == AlertState::Resolved {
                return Err(AppError::conflict(
                    "that alert has already resolved; there is nothing to acknowledge",
                ));
            }
            alert.state = AlertState::Acknowledged;
            alert.acked_at = Some(now);
            alert.ack_note = params.note.clone();
            Ok(())
        })?;
        app.audit(
            "alert.ack",
            alert.device_id.as_deref(),
            format!("Acknowledged the alert `{}`", alert.title),
            json!({"alert_id": alert.id, "note": params.note}),
        );
        publish(&app, &alert, None);
        Ok(alert)
    });
    router.add(
        "alerts.resolve",
        |app: App, params: NoteParams| async move {
            let now = app.state::<MonitorState>().now();
            let alert = get(&app, &params.id)?;
            if alert.state == AlertState::Resolved {
                return Ok(alert);
            }
            // Resolving by hand goes to the channels the rule would have used.
            let channels = rule_channels(&app, &alert);
            // This alert, not whatever is open under its key: the two are
            // the same thing unless duplicates were raised before raising
            // was made atomic, and then it is the one that was clicked.
            let Some(resolved) = resolve(
                &app,
                Target::Id(&alert.id),
                now,
                None,
                params.note.as_deref(),
                &channels,
            )?
            else {
                // It resolved itself between the look and the act.
                return get(&app, &params.id);
            };
            app.audit(
                "alert.resolve",
                resolved.device_id.as_deref(),
                format!("Resolved the alert `{}` by hand", resolved.title),
                json!({"alert_id": resolved.id, "note": params.note}),
            );
            Ok(resolved)
        },
    );
    router.add(
        "alerts.snooze",
        |app: App, params: SnoozeParams| async move {
            let now = app.state::<MonitorState>().now();
            let alert = amend(&app, &params.id, |alert| {
                alert.snoozed_until = if params.minutes == 0 {
                    None
                } else {
                    let minutes = i64::from(params.minutes.min(60 * 24 * 30));
                    Some(now + minutes * 60_000)
                };
                Ok(())
            })?;
            app.audit(
                "alert.snooze",
                alert.device_id.as_deref(),
                format!(
                    "Snoozed the alert `{}` for {} minutes",
                    alert.title, params.minutes
                ),
                json!({"alert_id": alert.id, "until": alert.snoozed_until}),
            );
            publish(&app, &alert, None);
            Ok(alert)
        },
    );
    router.add("alerts.raise", |app: App, params: RaiseParams| async move {
        if params.title.trim().is_empty() {
            return Err(AppError::invalid("an alert needs a title"));
        }
        if params.dedup_key.trim().is_empty() {
            return Err(AppError::invalid(
                "an alert needs a dedup_key, so that the same problem seen twice is one alert",
            ));
        }
        if let Some(device_id) = &params.device_id {
            inventory::get(&app, device_id)?;
        }
        // A channel that does not exist is a mistake in the call, and an
        // alert that quietly reached nobody because of it would be found
        // out during the outage it was for.
        let mut only = params.notify.clone();
        if let Some(wanted) = &mut only {
            if let Some(unknown) = notify::unknown_channel(&app, wanted)? {
                return Err(AppError::not_found(format!(
                    "notification channel `{unknown}`"
                )));
            }
            let mut seen = std::collections::HashSet::new();
            wanted.retain(|id| seen.insert(id.clone()));
        }
        let now = app.state::<MonitorState>().now();
        let spec = AlertSpec {
            dedup_key: params.dedup_key.trim().to_string(),
            rule_id: None,
            rule_name: params
                .rule
                .clone()
                .unwrap_or_else(|| "raised by hand".into()),
            device_id: params.device_id.clone(),
            instance: params.instance.clone(),
            severity: params.severity,
            title: params.title.trim().to_string(),
            detail: params.detail.clone().unwrap_or_default(),
            value: None,
            notify: Vec::new(),
            only,
            // The caller saw it just now: every call is a sighting.
            observed_at: None,
        };
        let (alert, _) = fire(&app, &spec, now)?;
        Ok::<Value, AppError>(serde_json::to_value(alert)?)
    });
    router.add("alerts.clear", |app: App, params: ClearParams| async move {
        let dedup_key = params.dedup_key.trim();
        if dedup_key.is_empty() {
            return Err(AppError::invalid(
                "say which alert is over: the dedup_key it was raised under",
            ));
        }
        // What a rule raised is the rule's to close: it resolves when the
        // rule's own condition clears, and a feature that could close it by
        // naming its key could hide an outage the rule still sees. Refused
        // out loud rather than answered `false`, which would read as
        // "nothing was open".
        if let Some(open) = open_by_key(&app, dedup_key)?
            && open.rule_id.is_some()
        {
            return Err(AppError::forbidden(format!(
                "the alert open under that key belongs to the alert rule `{}`; it resolves when the rule's condition clears, or by hand",
                open.rule_name
            )));
        }
        let now = app.state::<MonitorState>().now();
        let note = params
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty());
        // The ownership is checked again inside the transaction that
        // resolves; the look above only chooses the answer.
        let cleared = clear_raised(&app, dedup_key, now, note)?;
        if let Some(alert) = &cleared {
            // Neither the title, the key nor the note: all three can hold
            // text that came off a device (a log rule builds them from what
            // its pattern captured).
            app.audit(
                "alert.clear",
                alert.device_id.as_deref(),
                format!(
                    "An alert raised by `{}` was cleared: its condition is over",
                    alert.rule_name
                ),
                json!({"alert_id": alert.id, "severity": alert.severity}),
            );
        }
        Ok(json!({"cleared": cleared.is_some()}))
    });
}
