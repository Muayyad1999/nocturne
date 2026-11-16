//! Where checks, their current state and their changes are kept.
//!
//! A table rather than documents, for the same reason the interface table is
//! one: a campus with a few thousand devices and a handful of checks each has
//! too many to filter, sort and count in memory, and "which checks are down
//! right now" has to be one query.
//!
//! The definition and the state share a row but not a column. The row's
//! `status`, `since` and `last_run_at` are there to be queried; the whole
//! [`CheckState`] travels in `state`, which is `NULL` until the first run —
//! and a `NULL` there is what [`CheckStatus::Unknown`] means, rather than a
//! row pretending to a verdict it has never reached.

use nocturne_app::store::LogEntry;
use nocturne_app::{App, AppError, AppResult};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{
    CheckEvent, CheckKind, CheckState, CheckStatus, CheckSummary, ServiceCheck, ServiceCheckView,
};
use super::outcome::Outcome;

/// What recording one run did to the check's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub state: CheckState,
    pub previous: CheckStatus,
    /// Whether this run changed the status (and so appended to the history).
    pub changed: bool,
    /// How long the previous status had lasted, when that is known.
    pub held_s: Option<i64>,
}

fn row_to_check(row: &Row<'_>) -> rusqlite::Result<ServiceCheck> {
    let body: String = row.get("body")?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn row_to_state(row: &Row<'_>) -> rusqlite::Result<CheckState> {
    let stored: Option<String> = row.get("state")?;
    match stored {
        Some(body) => serde_json::from_str(&body).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        }),
        // Never run: the default is `unknown`, which is the truth.
        None => Ok(CheckState {
            check_id: row.get("id")?,
            device_id: row.get("device_id")?,
            ..CheckState::default()
        }),
    }
}

fn row_to_pair(row: &Row<'_>) -> rusqlite::Result<(ServiceCheck, CheckState)> {
    Ok((row_to_check(row)?, row_to_state(row)?))
}

const COLUMNS: &str = "id, device_id, name, kind, enabled, interval_s, status, since,
                       last_run_at, state, body";

// ------------------------------------------------------------- definitions

/// What makes a stored verdict about *this* check rather than another one:
/// change any of it and the state is no longer about the same thing.
fn identity(check: &ServiceCheck) -> String {
    format!(
        "{}|{}|{}",
        check.kind().as_str(),
        check.host.trim().to_lowercase(),
        check
            .spec
            .port()
            .map(|port| port.to_string())
            .unwrap_or_default()
    )
}

/// Create or replace a check's definition, keeping whatever it has found so
/// far — unless it has been pointed somewhere else, in which case the old
/// verdict is about a different service and is dropped rather than carried
/// over.
pub fn save(app: &App, check: &ServiceCheck) -> AppResult<bool> {
    let existing = get(app, &check.id)?;
    let repointed = existing
        .as_ref()
        .is_some_and(|(stored, _)| identity(stored) != identity(check));
    let body = serde_json::to_string(check)?;
    app.store().with(|conn| {
        conn.execute(
            "INSERT INTO monitor_checks
                (id, device_id, name, kind, enabled, interval_s, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                device_id = excluded.device_id, name = excluded.name, kind = excluded.kind,
                enabled = excluded.enabled, interval_s = excluded.interval_s,
                body = excluded.body, updated_at = excluded.updated_at",
            params![
                check.id,
                check.device_id,
                check.name,
                check.kind().as_str(),
                check.enabled,
                i64::from(check.interval_s),
                body,
                check.created_at,
                check.updated_at,
            ],
        )
    })?;
    if repointed {
        forget_state(app, &check.id)?;
    }
    Ok(repointed)
}

/// A check with the same name on the same device, if there is one.
pub fn by_name(app: &App, device_id: &str, name: &str) -> AppResult<Option<ServiceCheck>> {
    app.store().with(|conn| {
        conn.query_row(
            &format!(
                "SELECT {COLUMNS} FROM monitor_checks
                 WHERE device_id = ?1 AND name = ?2 COLLATE NOCASE"
            ),
            params![device_id, name],
            row_to_check,
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    })
}

pub fn get(app: &App, id: &str) -> AppResult<Option<(ServiceCheck, CheckState)>> {
    app.store().with(|conn| {
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM monitor_checks WHERE id = ?1"),
            params![id],
            row_to_pair,
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    })
}

pub fn require(app: &App, id: &str) -> AppResult<(ServiceCheck, CheckState)> {
    get(app, id)?.ok_or_else(|| AppError::not_found(format!("service check `{id}`")))
}

/// Every check that is switched on, for the scheduler.
pub fn enabled(app: &App) -> AppResult<Vec<ServiceCheck>> {
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM monitor_checks WHERE enabled = 1 ORDER BY id"
        ))?;
        statement.query_map([], row_to_check)?.collect()
    })
}

pub fn of_device(app: &App, device_id: &str) -> AppResult<Vec<(ServiceCheck, CheckState)>> {
    list(
        app,
        &CheckQuery {
            device_id: Some(device_id.to_string()),
            limit: Some(1_000),
            ..CheckQuery::default()
        },
    )
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CheckQuery {
    pub device_id: Option<String>,
    /// `up`, `warning`, `down`, `unknown`, `cannot_run`, or `problem` for
    /// everything that is not up.
    pub status: Option<String>,
    pub kind: Option<CheckKind>,
    /// Free text over the check's name and the device's.
    pub filter: Option<String>,
    /// Only checks that are switched on (or, with `false`, only those off).
    pub enabled: Option<bool>,
    pub limit: Option<u32>,
    /// Rows already received, as a decimal string.
    pub cursor: Option<String>,
}

pub fn list(app: &App, query: &CheckQuery) -> AppResult<Vec<(ServiceCheck, CheckState)>> {
    let limit = query.limit.unwrap_or(200).clamp(1, 5_000) as i64;
    let offset = match &query.cursor {
        Some(cursor) if !cursor.is_empty() => cursor
            .parse::<i64>()
            .map_err(|_| AppError::invalid("the cursor is the number of rows already received"))?,
        _ => 0,
    };
    let mut sql = format!("SELECT {COLUMNS} FROM monitor_checks WHERE 1 = 1");
    if query.device_id.is_some() {
        sql.push_str(" AND device_id = :device_id");
    }
    if query.kind.is_some() {
        sql.push_str(" AND kind = :kind");
    }
    if query.enabled.is_some() {
        sql.push_str(" AND enabled = :enabled");
    }
    match query.status.as_deref().map(str::trim) {
        Some("problem") => sql.push_str(" AND status NOT IN ('up', 'unknown')"),
        Some(status) if !status.is_empty() => {
            if CheckStatus::parse(status).is_none() {
                return Err(AppError::invalid(format!(
                    "`{status}` is not a check status (up, warning, down, unknown, cannot_run, or problem for everything that is not up)"
                )));
            }
            sql.push_str(" AND status = :status");
        }
        _ => {}
    }
    let text = query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| format!("%{}%", text.replace('%', "\\%").replace('_', "\\_")));
    if text.is_some() {
        sql.push_str(" AND name LIKE :text ESCAPE '\\'");
    }
    sql.push_str(" ORDER BY device_id, name COLLATE NOCASE LIMIT :limit OFFSET :offset");

    let kind = query.kind.map(|kind| kind.as_str());
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&sql)?;
        let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":limit", &limit), (":offset", &offset)];
        if let Some(device_id) = &query.device_id {
            bound.push((":device_id", device_id));
        }
        if let Some(kind) = &kind {
            bound.push((":kind", kind));
        }
        if let Some(enabled) = &query.enabled {
            bound.push((":enabled", enabled));
        }
        if let Some(status) = &query.status
            && status.trim() != "problem"
            && !status.trim().is_empty()
        {
            bound.push((":status", status));
        }
        if let Some(text) = &text {
            bound.push((":text", text));
        }
        statement
            .query_map(bound.as_slice(), row_to_pair)?
            .collect()
    })
}

pub fn delete(app: &App, id: &str) -> AppResult<()> {
    app.store().transaction(|tx| {
        tx.execute("DELETE FROM monitor_checks WHERE id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM monitor_check_events WHERE check_id = ?1",
            params![id],
        )?;
        Ok(())
    })
}

/// Drop everything about a device's checks (it left the inventory).
pub fn forget_device(app: &App, device_id: &str) -> AppResult<()> {
    app.store().transaction(|tx| {
        tx.execute(
            "DELETE FROM monitor_checks WHERE device_id = ?1",
            params![device_id],
        )?;
        tx.execute(
            "DELETE FROM monitor_check_events WHERE device_id = ?1",
            params![device_id],
        )?;
        Ok(())
    })
}

/// Forget what a check found, without forgetting the check.
fn forget_state(app: &App, id: &str) -> AppResult<()> {
    app.store().with(|conn| {
        conn.execute(
            "UPDATE monitor_checks
             SET status = 'unknown', since = NULL, last_run_at = NULL, state = NULL
             WHERE id = ?1",
            params![id],
        )
    })?;
    Ok(())
}

/// How many checks are in each state, for `monitor.status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub total: u64,
    pub up: u64,
    pub warning: u64,
    pub down: u64,
    pub unknown: u64,
    pub cannot_run: u64,
    /// Checks that are switched off, and so are not counted above.
    pub disabled: u64,
}

pub fn counts(app: &App) -> AppResult<Counts> {
    let rows: Vec<(String, bool, i64)> = app.store().with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT status, enabled, COUNT(*) FROM monitor_checks GROUP BY status, enabled",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
    })?;
    let mut counts = Counts::default();
    for (status, enabled, count) in rows {
        let count = count as u64;
        counts.total += count;
        if !enabled {
            counts.disabled += count;
            continue;
        }
        match CheckStatus::parse(&status).unwrap_or_default() {
            CheckStatus::Up => counts.up += count,
            CheckStatus::Warning => counts.warning += count,
            CheckStatus::Down => counts.down += count,
            CheckStatus::Unknown => counts.unknown += count,
            CheckStatus::CannotRun => counts.cannot_run += count,
        }
    }
    Ok(counts)
}

// ------------------------------------------------------------- the recording

/// Record what one run found.
///
/// A run that says the same thing as the one before it updates the state and
/// nothing else; a run that says something different appends to the history,
/// writes a line in the `event` log stream, and starts the clock on the new
/// status. That is what makes "down since 09:14, for 40 minutes" answerable
/// without keeping every run.
pub fn record(
    app: &App,
    check: &ServiceCheck,
    device: &str,
    outcome: &Outcome,
    now: i64,
) -> AppResult<Recorded> {
    let before = get(app, &check.id)?
        .map(|(_, state)| state)
        .unwrap_or_default();
    let previous = before.status;
    let changed = previous != outcome.status;
    let held_s = if changed {
        before.since.map(|since| (now - since).max(0) / 1_000)
    } else {
        None
    };
    let state = CheckState {
        check_id: check.id.clone(),
        device_id: check.device_id.clone(),
        status: outcome.status,
        code: outcome.code.clone(),
        message: outcome.message.clone(),
        rtt_ms: outcome.rtt_ms,
        facts: outcome.facts.clone(),
        since: if changed {
            Some(now)
        } else {
            before.since.or(Some(now))
        },
        last_run_at: Some(now),
        last_ok_at: if outcome.status.is_up() {
            Some(now)
        } else {
            before.last_ok_at
        },
        last_down_at: if outcome.status == CheckStatus::Down {
            Some(now)
        } else {
            before.last_down_at
        },
        runs: if changed {
            1
        } else {
            before.runs.saturating_add(1)
        },
    };
    let body = serde_json::to_string(&state)?;
    app.store().with(|conn| {
        conn.execute(
            "UPDATE monitor_checks SET status = ?2, since = ?3, last_run_at = ?4, state = ?5
             WHERE id = ?1",
            params![
                check.id,
                state.status.as_str(),
                state.since,
                state.last_run_at,
                body
            ],
        )
    })?;
    if changed {
        app.store().with(|conn| {
            conn.execute(
                "INSERT INTO monitor_check_events
                    (check_id, device_id, at, status, previous, held_s, code, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    check.id,
                    check.device_id,
                    now,
                    state.status.as_str(),
                    previous.as_str(),
                    held_s,
                    state.code,
                    state.message,
                ],
            )
        })?;
        log_change(app, check, device, &state, previous, held_s);
    }
    Ok(Recorded {
        state,
        previous,
        changed,
        held_s,
    })
}

/// One line in the `event` log stream per change, in the same place a device
/// going down is recorded, so one timeline holds both.
fn log_change(
    app: &App,
    check: &ServiceCheck,
    device: &str,
    state: &CheckState,
    previous: CheckStatus,
    held_s: Option<i64>,
) {
    let what = match state.status {
        CheckStatus::Up => format!("`{}` on `{device}` is answering again", check.name),
        CheckStatus::Warning => format!(
            "`{}` on `{device}` is answering with a warning: {}",
            check.name, state.message
        ),
        CheckStatus::Down => format!(
            "`{}` on `{device}` is not answering: {}",
            check.name, state.message
        ),
        // Said plainly, because the difference matters to whoever reads it.
        CheckStatus::CannotRun => format!(
            "`{}` on `{device}` could not be checked, so nothing is known about it: {}",
            check.name, state.message
        ),
        CheckStatus::Unknown => format!("`{}` on `{device}` has no state", check.name),
    };
    let detail = match held_s {
        Some(seconds) => format!(" (it was {} for {seconds} s)", previous.as_str()),
        None => String::new(),
    };
    let _ = app.store().log_append(&LogEntry {
        id: 0,
        ts: state.last_run_at.unwrap_or_default(),
        stream: "event".into(),
        device_id: Some(check.device_id.clone()),
        severity: Some(match state.status {
            CheckStatus::Down => 3,
            CheckStatus::Warning | CheckStatus::CannotRun => 4,
            _ => 5,
        }),
        source: "monitor".into(),
        message: format!("{what}{detail}"),
        body: Some(json!({
            "event": "check_state",
            "check_id": check.id,
            "check": check.name,
            "kind": check.kind().as_str(),
            "status": state.status.as_str(),
            "previous": previous.as_str(),
            "code": state.code,
            "held_s": held_s,
        })),
    });
}

// ----------------------------------------------------------------- history

fn row_to_event(row: &Row<'_>) -> rusqlite::Result<CheckEvent> {
    let status: String = row.get(4)?;
    let previous: String = row.get(5)?;
    Ok(CheckEvent {
        id: row.get(0)?,
        check_id: row.get(1)?,
        device_id: row.get(2)?,
        at: row.get(3)?,
        status: CheckStatus::parse(&status).unwrap_or_default(),
        previous: CheckStatus::parse(&previous).unwrap_or_default(),
        held_s: row.get(6)?,
        code: row.get(7)?,
        message: row.get(8)?,
    })
}

const EVENT_COLUMNS: &str = "id, check_id, device_id, at, status, previous, held_s, code, message";

/// The changes inside a window, oldest first.
pub fn events(
    app: &App,
    check_id: &str,
    since: i64,
    until: i64,
    limit: u32,
) -> AppResult<Vec<CheckEvent>> {
    let limit = i64::from(limit.clamp(1, 5_000));
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM monitor_check_events
             WHERE check_id = ?1 AND at >= ?2 AND at <= ?3 ORDER BY at, id LIMIT ?4"
        ))?;
        statement
            .query_map(params![check_id, since, until, limit], row_to_event)?
            .collect()
    })
}

/// The status a check was in when a window opened: the last change at or
/// before it. `None` means it had never changed, i.e. it was `unknown`.
pub fn status_at(app: &App, check_id: &str, at: i64) -> AppResult<Option<CheckEvent>> {
    app.store().with(|conn| {
        conn.query_row(
            &format!(
                "SELECT {EVENT_COLUMNS} FROM monitor_check_events
                 WHERE check_id = ?1 AND at <= ?2 ORDER BY at DESC, id DESC LIMIT 1"
            ),
            params![check_id, at],
            row_to_event,
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    })
}

/// Add up how a check spent a window.
///
/// Built from the changes, not from the runs: between two changes the check
/// was in one status for the whole span, which is the only way to answer
/// "for how long" without keeping every run forever.
pub fn summarise(
    app: &App,
    check_id: &str,
    since: i64,
    until: i64,
    events: &[CheckEvent],
) -> AppResult<CheckSummary> {
    let opening = status_at(app, check_id, since)?
        .map(|event| event.status)
        .unwrap_or(CheckStatus::Unknown);
    let mut summary = CheckSummary::default();
    let mut status = opening;
    let mut at = since;
    let mut last_down: Option<(i64, Option<i64>)> = None;
    for event in events {
        add(&mut summary, status, event.at - at);
        if event.status == CheckStatus::Down {
            summary.down_count += 1;
            last_down = Some((event.at, None));
        } else if let Some((start, None)) = last_down {
            last_down = Some((start, Some(event.at)));
        }
        status = event.status;
        at = event.at;
    }
    add(&mut summary, status, until - at);
    // An outage that is still running is measured to the end of the window.
    if let Some((start, ended)) = last_down {
        summary.last_down_at = Some(start);
        summary.last_down_for_s = Some((ended.unwrap_or(until) - start).max(0) / 1_000);
    }
    summary.measured_s = summary.up_s + summary.warning_s + summary.down_s;
    // No figure at all rather than a flattering one: a window in which
    // nothing was measured has no availability.
    summary.availability_pct = (summary.measured_s > 0)
        .then(|| (summary.up_s + summary.warning_s) as f64 / summary.measured_s as f64 * 100.0);
    Ok(summary)
}

fn add(summary: &mut CheckSummary, status: CheckStatus, span_ms: i64) {
    let seconds = span_ms.max(0) / 1_000;
    match status {
        CheckStatus::Up => summary.up_s += seconds,
        CheckStatus::Warning => summary.warning_s += seconds,
        CheckStatus::Down => summary.down_s += seconds,
        CheckStatus::CannotRun => summary.cannot_run_s += seconds,
        CheckStatus::Unknown => summary.unknown_s += seconds,
    }
}

/// Drop changes older than `before`.
pub fn prune(app: &App, before: i64) -> AppResult<usize> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_check_events WHERE at < ?1",
            params![before],
        )
    })
}

// -------------------------------------------------------------------- views

/// A check, its state and its device's name, as the RPC surface returns it.
pub fn view(app: &App, check: ServiceCheck, state: CheckState) -> ServiceCheckView {
    let device = nocturne_app::inventory::get(app, &check.device_id)
        .map(|device| device.name)
        .unwrap_or_default();
    ServiceCheckView {
        kind: check.kind(),
        device,
        check,
        state,
    }
}

pub fn views(app: &App, rows: Vec<(ServiceCheck, CheckState)>) -> Vec<ServiceCheckView> {
    rows.into_iter()
        .map(|(check, state)| view(app, check, state))
        .collect()
}
