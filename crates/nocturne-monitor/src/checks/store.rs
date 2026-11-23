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
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{
    CheckEvent, CheckKind, CheckState, CheckStatus, CheckSummary, ServiceCheck, ServiceCheckView,
};
use super::outcome::Outcome;

/// What the scheduler's own granularity can add between two runs of a check
/// that is being run on time: its tick, the wait for a free slot behind the
/// concurrency cap, a list of checks that is a few seconds old.
const SCHEDULING_SLACK_MS: i64 = 30_000;

/// How long after a run nothing more has to have been heard for the watch to
/// count as broken.
///
/// The history keeps changes, not runs, so "it was up from nine to ten" is an
/// inference: it rests on the runs in between having happened and having
/// said the same thing. They are taken to have happened while the next run
/// begins within two intervals of the last one ending — one run may be
/// skipped or late — plus the time a run may take and what the scheduler can
/// add. Longer than that and nobody was looking: the application was closed,
/// the machine was asleep, the check or its device's monitoring was switched
/// off. What the service did then is not known, and the last status heard is
/// not an answer.
fn silence_ms(check: &ServiceCheck) -> i64 {
    2 * i64::from(check.interval_s.max(5)) * 1_000
        + i64::try_from(check.timeout_ms).unwrap_or(120_000)
        + SCHEDULING_SLACK_MS
}

/// The moment a check's silence is counted from: its last run, or the last
/// time its definition was saved if that came later.
///
/// A save is not a measurement, but it is when a cadence begins: the
/// scheduler gives a saved check a fresh slot inside its (possibly new)
/// interval. Counted from the last run alone, an hourly check made a
/// one-minute check forty minutes after it ran would read as forty minutes
/// nobody measured, because forty minutes is more than a one-minute check
/// may go unrun — and "up since last week" would start again at the edit.
///
/// This is only sound because [`save`] closes a hole that is already there
/// when it is called, judged by the cadence the check had until then: what
/// is left to count from the save is the new cadence alone. A check that is
/// switched off has no cadence, so a save that left it off moves nothing.
fn silent_since(check: &ServiceCheck, last_run: i64) -> i64 {
    if check.enabled {
        last_run.max(check.updated_at)
    } else {
        last_run
    }
}

/// Until when the status a check holds can be taken to have lasted, as seen
/// from `at`: up to `at` while the watch is unbroken, and otherwise only up
/// to the last run, which is the last moment anything was measured.
fn measured_until(check: &ServiceCheck, state: &CheckState, watched: bool, at: i64) -> i64 {
    match state.last_run_at {
        Some(last) if at <= last => at,
        Some(last) if watched && at - silent_since(check, last) <= silence_ms(check) => at,
        Some(last) => last,
        // Never run, or pointed somewhere else since: nothing is measured.
        None => i64::MIN,
    }
}

/// What recording one run did to the check's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub state: CheckState,
    /// What the check said before this run. `unknown` after a stretch in
    /// which it was not run, whatever it had said before that.
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
///
/// The device is part of it: a check with no host of its own watches
/// whatever device it is on, and even one that names a host raises its
/// alerts under the device's name. The host is not part of it for an SNMP
/// check, which only ever asks the device's own agent.
fn identity(check: &ServiceCheck) -> String {
    let host = match check.kind() {
        CheckKind::Snmp => String::new(),
        _ => check.host.trim().to_lowercase(),
    };
    format!(
        "{}|{}|{}|{}",
        check.device_id,
        check.kind().as_str(),
        host,
        check
            .spec
            .port()
            .map(|port| port.to_string())
            .unwrap_or_default()
    )
}

fn read(conn: &Connection, id: &str) -> rusqlite::Result<Option<(ServiceCheck, CheckState)>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM monitor_checks WHERE id = ?1"),
        params![id],
        row_to_pair,
    )
    .optional()
}

/// The fields of one history row that are not the same for every row.
struct Change<'a> {
    at: i64,
    status: CheckStatus,
    previous: CheckStatus,
    held_s: Option<i64>,
    code: &'a str,
    message: &'a str,
}

fn append(conn: &Connection, check: &ServiceCheck, change: &Change<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO monitor_check_events
            (check_id, device_id, at, status, previous, held_s, code, message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            check.id,
            check.device_id,
            change.at,
            change.status.as_str(),
            change.previous.as_str(),
            change.held_s,
            change.code,
            change.message,
        ],
    )?;
    Ok(())
}

/// End the stretch of history in which `state.status` held, at `at`: from
/// there on the check's status is `unknown` until a run says otherwise.
fn close_span(
    conn: &Connection,
    check: &ServiceCheck,
    state: &CheckState,
    at: i64,
    code: &str,
    message: &str,
) -> rusqlite::Result<()> {
    if state.status == CheckStatus::Unknown {
        return Ok(());
    }
    append(
        conn,
        check,
        &Change {
            at,
            status: CheckStatus::Unknown,
            previous: state.status,
            held_s: state.since.map(|since| (at - since).max(0) / 1_000),
            code,
            message,
        },
    )
}

/// Record that nobody was watching a check after its run at `last`: the
/// stretch in which its status held ends there in the history, and what is
/// returned is the state it is left with. The status is forgotten; when it
/// was last seen up and last seen down are not, being dates rather than
/// claims about now.
fn lapse(
    conn: &Connection,
    check: &ServiceCheck,
    state: &CheckState,
    last: i64,
) -> rusqlite::Result<CheckState> {
    close_span(
        conn,
        check,
        state,
        last,
        "not_measured",
        "The check stopped being run here. What the service did until its next run is not known.",
    )?;
    Ok(CheckState {
        check_id: check.id.clone(),
        device_id: check.device_id.clone(),
        last_ok_at: state.last_ok_at,
        last_down_at: state.last_down_at,
        ..CheckState::default()
    })
}

fn write_state(conn: &Connection, state: &CheckState) -> rusqlite::Result<()> {
    let body = serde_json::to_string(state)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "UPDATE monitor_checks SET status = ?2, since = ?3, last_run_at = ?4, state = ?5
         WHERE id = ?1",
        params![
            state.check_id,
            state.status.as_str(),
            state.since,
            state.last_run_at,
            body
        ],
    )?;
    Ok(())
}

/// Create or replace a check's definition, keeping whatever it has found so
/// far — unless it has been pointed somewhere else, in which case the old
/// verdict is about a different service and is dropped rather than carried
/// over. Returns whether it was.
///
/// One transaction: a run that finishes while this is under way is recorded
/// against the definition it ran from or not at all ([`record`]), and that
/// only works if the definition and the state change together.
pub fn save(app: &App, check: &ServiceCheck) -> AppResult<bool> {
    let body = serde_json::to_string(check)?;
    app.store().transaction(|tx| {
        let existing = read(tx, &check.id)?;
        tx.execute(
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
        )?;
        let Some((stored, state)) = existing else {
            return Ok(false);
        };
        if stored.device_id != check.device_id {
            // The history is the check's and goes where the check goes.
            // Left under the old device it would be deleted with that
            // device, and survive the deletion of the one it is now on.
            tx.execute(
                "UPDATE monitor_check_events SET device_id = ?2 WHERE check_id = ?1",
                params![check.id, check.device_id],
            )?;
        }
        if identity(&stored) == identity(check) {
            // Still the same service, so what was found stands — unless
            // nobody has been looking. From here on the check's silence is
            // counted from this save ([`silent_since`]), so a hole that is
            // already there has to be closed now, by the cadence the check
            // had until now. Otherwise renaming a check in the morning,
            // before its first run, would vouch for the night.
            if let Some(last) = state.last_run_at
                && check.updated_at - silent_since(&stored, last) > silence_ms(&stored)
            {
                let forgotten = lapse(tx, check, &state, last)?;
                write_state(tx, &forgotten)?;
            }
            return Ok(false);
        }
        // The old verdict stops here in the history too. Without a row to
        // say so, the time until the new target's first run would be added
        // to whatever the old target was last doing.
        // It stops at the last run, which is the last moment anything was
        // measured about the old target.
        close_span(
            tx,
            check,
            &state,
            state.last_run_at.unwrap_or(check.updated_at),
            "repointed",
            "The check was pointed at something else, so what it had found no longer applies.",
        )?;
        tx.execute(
            "UPDATE monitor_checks
             SET status = 'unknown', since = NULL, last_run_at = NULL, state = NULL
             WHERE id = ?1",
            params![check.id],
        )?;
        Ok(true)
    })
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
    app.store().with(|conn| read(conn, id))
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

/// How many checks are in each state, for `monitor.status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub total: u64,
    pub up: u64,
    pub warning: u64,
    pub down: u64,
    pub unknown: u64,
    pub cannot_run: u64,
    /// Checks that are not being watched — switched off themselves, or on a
    /// device (or in an installation) whose monitoring is — and so are not
    /// counted above.
    pub disabled: u64,
}

/// Count the checks by what they are saying.
///
/// `watched` is told a check's device and whether the check is switched on,
/// and answers whether anything is running it ([`super::watched`]). A check
/// nobody runs is counted as `disabled` whatever it last found: the `down`
/// it found before monitoring was switched off for its device is not a
/// service that is down now, and nothing will ever come along to correct it.
pub fn counts(app: &App, watched: impl Fn(&str, bool) -> bool) -> AppResult<Counts> {
    let rows: Vec<(String, String, bool, i64)> = app.store().with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT device_id, status, enabled, COUNT(*) FROM monitor_checks
             GROUP BY device_id, status, enabled",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect()
    })?;
    let mut counts = Counts::default();
    for (device_id, status, enabled, count) in rows {
        let count = count as u64;
        counts.total += count;
        if !watched(&device_id, enabled) {
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
///
/// `check` is the definition the run was started from, `started_at` when it
/// set out and `now` when it came back. Two things are decided here, in the
/// same transaction as the write:
///
/// * **Whether the run still means anything.** A probe can take as long as
///   its timeout, and the check can be edited, pointed somewhere else or
///   deleted meanwhile. A result for a definition that is no longer the
///   stored one is about something else, and is dropped: `Ok(None)`, with
///   nothing written. Recording it would put a verdict on a target that was
///   never probed, or a history row on a check that no longer exists.
/// * **Whether anybody was watching since the last run.** If more time went
///   by than the check's cadence allows ([`silence_ms`], counted from
///   [`silent_since`]), the stretch in which the old status held is closed
///   at the last run, and this run starts from `unknown`: `previous` is
///   `unknown`, nothing is said to have lasted the whole gap, and `since` is
///   now. Otherwise a night with the application closed is added to
///   whatever the check last said.
pub fn record(
    app: &App,
    check: &ServiceCheck,
    device: &str,
    outcome: &Outcome,
    started_at: i64,
    now: i64,
) -> AppResult<Option<Recorded>> {
    let written = app.store().transaction(|tx| {
        let Some((stored, mut before)) = read(tx, &check.id)? else {
            return Ok(None);
        };
        if stored != *check {
            return Ok(None);
        }
        // What it said before nobody was looking, kept for the timeline.
        let last_heard = before.status;
        if let Some(last) = before.last_run_at
            && started_at - silent_since(check, last) > silence_ms(check)
        {
            before = lapse(tx, check, &before, last)?;
        }
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
        write_state(tx, &state)?;
        if changed {
            append(
                tx,
                check,
                &Change {
                    at: now,
                    status: state.status,
                    previous,
                    held_s,
                    code: &state.code,
                    message: &state.message,
                },
            )?;
        }
        Ok(Some((
            Recorded {
                state,
                previous,
                changed,
                held_s,
            },
            last_heard,
        )))
    })?;
    let Some((recorded, last_heard)) = written else {
        return Ok(None);
    };
    // The timeline is for news. A watch that resumes and finds what it had
    // last heard is a new row in the history (the gap is real) but not an
    // event anybody needs to read once per check at every start.
    if recorded.changed && recorded.state.status != last_heard {
        log_change(app, check, device, &recorded, last_heard);
    }
    Ok(Some(recorded))
}

/// One line in the `event` log stream per change, in the same place a device
/// going down is recorded, so one timeline holds both.
///
/// `last_heard` is what the check last said, which is `recorded.previous`
/// unless a stretch in which it was not run lies in between.
fn log_change(
    app: &App,
    check: &ServiceCheck,
    device: &str,
    recorded: &Recorded,
    last_heard: CheckStatus,
) {
    let state = &recorded.state;
    let what = match state.status {
        // "Again" only when it is known to have been otherwise.
        CheckStatus::Up if last_heard == CheckStatus::Unknown => {
            format!("`{}` on `{device}` is answering", check.name)
        }
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
    let detail = match recorded.held_s {
        Some(seconds) => format!(" (it was {} for {seconds} s)", recorded.previous.as_str()),
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
            "previous": recorded.previous.as_str(),
            "code": state.code,
            "held_s": recorded.held_s,
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

/// The changes inside a window: the newest `limit` of them, oldest first,
/// and how many the window holds in all.
///
/// The newest, because a list that has to be cut is cut at the end nobody
/// is looking at: a check that flapped for a day last week and has been
/// quiet since should show the end of the flapping, not its first minutes.
pub fn events(
    app: &App,
    check_id: &str,
    since: i64,
    until: i64,
    limit: u32,
) -> AppResult<(Vec<CheckEvent>, u64)> {
    let limit = i64::from(limit.clamp(1, 5_000));
    app.store().with(|conn| {
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM monitor_check_events
             WHERE check_id = ?1 AND at >= ?2 AND at <= ?3",
            params![check_id, since, until],
            |row| row.get(0),
        )?;
        let mut statement = conn.prepare_cached(&format!(
            "SELECT {EVENT_COLUMNS} FROM monitor_check_events
             WHERE check_id = ?1 AND at >= ?2 AND at <= ?3 ORDER BY at DESC, id DESC LIMIT ?4"
        ))?;
        let mut newest: Vec<CheckEvent> = statement
            .query_map(params![check_id, since, until, limit], row_to_event)?
            .collect::<rusqlite::Result<_>>()?;
        newest.reverse();
        Ok((newest, total as u64))
    })
}

/// The status a check was in when a window opened, and from when inside the
/// window that is known: `(status, from)`, with everything before `from`
/// unknown.
///
/// Normally that is the last change at or before the window's start. The
/// rest is for a history whose older rows are gone — pruning keeps the row a
/// window needs ([`prune`]), but an earlier version did not, and a window can
/// reach back past the retention. A row still says what came before it and
/// for how long (`previous`, `held_s`); and where there is no row at all the
/// check itself says since when it has been in its present state. A check
/// that has been up for a year reads as up, not as unknown, for want of a
/// row that was deleted for being old.
fn opening(
    conn: &Connection,
    check_id: &str,
    state: &CheckState,
    since: i64,
) -> rusqlite::Result<(CheckStatus, i64)> {
    let before = conn
        .query_row(
            &format!(
                "SELECT {EVENT_COLUMNS} FROM monitor_check_events
                 WHERE check_id = ?1 AND at <= ?2 ORDER BY at DESC, id DESC LIMIT 1"
            ),
            params![check_id, since],
            row_to_event,
        )
        .optional()?;
    if let Some(change) = before {
        return Ok((change.status, since));
    }
    let after = conn
        .query_row(
            &format!(
                "SELECT {EVENT_COLUMNS} FROM monitor_check_events
                 WHERE check_id = ?1 AND at > ?2 ORDER BY at, id LIMIT 1"
            ),
            params![check_id, since],
            row_to_event,
        )
        .optional()?;
    Ok(match after {
        Some(next) => match next.held_s {
            Some(held_s) if next.previous != CheckStatus::Unknown => {
                (next.previous, (next.at - held_s * 1_000).max(since))
            }
            _ => (CheckStatus::Unknown, since),
        },
        None => match state.since {
            Some(began) if state.status != CheckStatus::Unknown => (state.status, began.max(since)),
            _ => (CheckStatus::Unknown, since),
        },
    })
}

/// A window being added up, one change at a time.
struct Tally {
    summary: CheckSummary,
    status: CheckStatus,
    at: i64,
    /// The outage that most recently began in the window, and when it was
    /// last seen to be over (or to be no longer watched).
    last_down: Option<(i64, Option<i64>)>,
}

impl Tally {
    fn change(&mut self, at: i64, status: CheckStatus) {
        add(&mut self.summary, self.status, at - self.at);
        if status == CheckStatus::Down {
            self.summary.down_count += 1;
            self.last_down = Some((at, None));
        } else if let Some((start, None)) = self.last_down {
            self.last_down = Some((start, Some(at)));
        }
        self.status = status;
        self.at = at;
    }
}

/// Add up how a check spent a window.
///
/// Built from the changes, not from the runs: between two changes the check
/// was in one status for the whole span, which is the only way to answer
/// "for how long" without keeping every run forever.
///
/// It reads every change in the window for itself, rather than being handed
/// a list: the list a caller shows is cut to a length, and a summary added
/// up from a cut list charges everything after the cut to the last row that
/// made it.
///
/// `watched` says whether anything is running the check now
/// ([`super::watched`]). The stretch after the last change in the window is
/// closed by the next change if there is one on record after the window:
/// the status lasted until then, like any stretch between two rows. Where
/// there is none it is the one part no row closes, so it is closed here:
/// the status is taken to have lasted until `until` only while runs are
/// still arriving, and otherwise until the last run. After that it is
/// unknown.
pub fn summarise(
    app: &App,
    check: &ServiceCheck,
    state: &CheckState,
    watched: bool,
    since: i64,
    until: i64,
) -> AppResult<CheckSummary> {
    let tally = app.store().with(|conn| {
        let (status, from) = opening(conn, &check.id, state, since)?;
        let mut tally = Tally {
            summary: CheckSummary::default(),
            status: CheckStatus::Unknown,
            at: since,
            last_down: None,
        };
        if status != CheckStatus::Unknown {
            // Not `change`: a status the window opened in is not an outage
            // that began inside it.
            let from = from.min(until);
            add(&mut tally.summary, CheckStatus::Unknown, from - since);
            tally.status = status;
            tally.at = from;
        }
        let mut statement = conn.prepare_cached(
            "SELECT at, status FROM monitor_check_events
             WHERE check_id = ?1 AND at >= ?2 AND at <= ?3 ORDER BY at, id",
        )?;
        let mut rows = statement.query(params![check.id, since, until])?;
        while let Some(row) = rows.next()? {
            let status: String = row.get(1)?;
            tally.change(row.get(0)?, CheckStatus::parse(&status).unwrap_or_default());
        }
        let closed_later: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM monitor_check_events WHERE check_id = ?1 AND at > ?2)",
            params![check.id, until],
            |row| row.get(0),
        )?;
        Ok((tally, closed_later))
    })?;
    let (mut tally, closed_later) = tally;

    // What the check is doing *now* only speaks for the end of a window
    // that nothing later on record speaks for. A check repointed last week
    // has no last run at all, and asked about the week before that it would
    // otherwise answer "unknown" for days its own rows say it was up.
    let measured = if closed_later {
        until
    } else {
        measured_until(check, state, watched, until)
            .max(tally.at)
            .min(until)
    };
    add(&mut tally.summary, tally.status, measured - tally.at);
    add(&mut tally.summary, CheckStatus::Unknown, until - measured);
    let mut summary = tally.summary;
    // An outage that is still running is measured to the end of the window,
    // or to the last run that saw it if nothing has looked since.
    if let Some((start, ended)) = tally.last_down {
        summary.last_down_at = Some(start);
        summary.last_down_for_s = Some((ended.unwrap_or(measured) - start).max(0) / 1_000);
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

/// Drop changes older than `before` — except, for each check, the newest of
/// them.
///
/// That one row is what says which status the check was in when the
/// retention begins. Without it a check that went up a year ago and has not
/// changed since has no history at all, and every window asked about reads
/// as `unknown` with no availability, beside a state that says "up since
/// last year".
///
/// Rows whose check is gone go too, whatever their age: there is nothing to
/// ask them through.
pub fn prune(app: &App, before: i64) -> AppResult<usize> {
    app.store().with(|conn| {
        let old = conn.execute(
            "DELETE FROM monitor_check_events
             WHERE at < ?1
               AND EXISTS (
                   SELECT 1 FROM monitor_check_events AS newer
                   WHERE newer.check_id = monitor_check_events.check_id
                     AND newer.at < ?1
                     AND (newer.at > monitor_check_events.at
                          OR (newer.at = monitor_check_events.at
                              AND newer.id > monitor_check_events.id)))",
            params![before],
        )?;
        let orphaned = conn.execute(
            "DELETE FROM monitor_check_events
             WHERE check_id NOT IN (SELECT id FROM monitor_checks)",
            [],
        )?;
        Ok(old + orphaned)
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
