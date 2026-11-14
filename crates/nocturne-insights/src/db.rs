//! Where findings live between scans.
//!
//! An insight has a life: it appears, it is seen again, somebody dismisses it
//! for a fortnight, it comes back, it goes away. That history is the whole
//! value of a stable id, so it is kept in a table of this feature's own —
//! with the columns a list needs to filter, count and page in SQL, because a
//! campus produces thousands of findings and the UI asks for fifty.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::store::Store;
use nocturne_app::{AppError, AppResult};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::model::{Category, Insight, Severity, State};
use crate::scan::Coverage;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS insights (
    id              TEXT PRIMARY KEY,
    check_id        TEXT NOT NULL,
    category        TEXT NOT NULL,
    severity        INTEGER NOT NULL,
    device_id       TEXT NOT NULL,
    subject         TEXT NOT NULL,
    state           TEXT NOT NULL,
    first_seen      INTEGER NOT NULL,
    last_seen       INTEGER NOT NULL,
    resolved_at     INTEGER,
    dismissed_until INTEGER,
    lab             INTEGER NOT NULL DEFAULT 0,
    -- Consecutive scans in which the check was evaluated for the device and
    -- did not find this problem. Two of them resolve it.
    clean_scans     INTEGER NOT NULL DEFAULT 0,
    -- `,id,id,` of the devices at the other end, so a device's list finds the
    -- findings it is only half of.
    related         TEXT NOT NULL DEFAULT ',',
    -- Lowercased device name, subject and title, for free-text search.
    search          TEXT NOT NULL DEFAULT '',
    body            TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS insights_state ON insights (state, severity DESC, last_seen DESC);
CREATE INDEX IF NOT EXISTS insights_device ON insights (device_id);
CREATE INDEX IF NOT EXISTS insights_check ON insights (check_id, device_id);
";

pub(crate) fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

fn row_to_insight(row: &Row<'_>) -> rusqlite::Result<Insight> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn search_text(insight: &Insight) -> String {
    format!(
        "{} {} {} {}",
        insight.device, insight.subject, insight.title, insight.check
    )
    .to_lowercase()
}

fn related_text(insight: &Insight) -> String {
    let mut text = String::from(",");
    for related in &insight.related {
        text.push_str(&related.device_id);
        text.push(',');
    }
    text
}

fn write(conn: &Connection, insight: &Insight, clean_scans: u32) -> rusqlite::Result<()> {
    let body = serde_json::to_string(insight)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT INTO insights (id, check_id, category, severity, device_id, subject, state,
             first_seen, last_seen, resolved_at, dismissed_until, lab, clean_scans, related, search, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT (id) DO UPDATE SET
             severity = excluded.severity, state = excluded.state, subject = excluded.subject,
             first_seen = excluded.first_seen, last_seen = excluded.last_seen,
             resolved_at = excluded.resolved_at, dismissed_until = excluded.dismissed_until,
             lab = excluded.lab, clean_scans = excluded.clean_scans, related = excluded.related,
             search = excluded.search, body = excluded.body",
        params![
            insight.id,
            insight.check,
            insight.category.as_str(),
            insight.severity.rank(),
            insight.device_id,
            insight.subject,
            insight.state.as_str(),
            insight.first_seen,
            insight.last_seen,
            insight.resolved_at,
            insight.dismissed_until,
            insight.lab as i64,
            clean_scans,
            related_text(insight),
            search_text(insight),
            body,
        ],
    )?;
    Ok(())
}

/// One check's result for one scan, ready for the ledger.
pub(crate) struct Outcome {
    pub check: &'static str,
    pub found: Vec<Insight>,
    /// Devices the check reached a verdict for. Only these may have a finding
    /// of theirs counted as clean — a device that dropped out of collection
    /// never resolves its own open findings.
    pub evaluated: BTreeSet<String>,
}

/// What a scan changed, for the event and for alert raising.
#[derive(Debug, Default)]
pub(crate) struct Changed {
    pub opened: Vec<Insight>,
    pub resolved: Vec<Insight>,
    /// Open findings after the scan, for the alert threshold.
    pub open: Vec<Insight>,
}

/// Fold a scan's results into the ledger: new findings appear, findings seen
/// again keep their identity and history, and findings that two consecutive
/// evaluated scans did not see resolve.
/// Store what a scan found, and the coverage it found it under, together.
///
/// The coverage is written in the same transaction on purpose. It used to be
/// written after this function returned, which left a window — the whole of
/// it — in which `insights.list` already answered with findings while
/// `insights.coverage` still said nothing had ever been scanned. That is the
/// one contradiction this crate exists to prevent: the screen tells "nothing
/// is wrong" from "nobody looked" by exactly that field, so for a moment it
/// could show findings from a scan that never happened. An event-driven
/// reader never saw it, because `insights.changed` is published after both
/// writes, but anything that polled could. CI caught it.
pub(crate) fn reconcile(
    store: &Store,
    now: i64,
    outcomes: &[Outcome],
    coverage: &Coverage,
) -> AppResult<Changed> {
    let mut changed = Changed::default();
    let coverage = serde_json::to_string(coverage)?;
    store.transaction(|tx| {
        tx.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params!["insights.coverage", coverage],
        )?;
        for outcome in outcomes {
            let found_ids: BTreeSet<&str> =
                outcome.found.iter().map(|insight| insight.id.as_str()).collect();

            for insight in &outcome.found {
                let existing: Option<(String, i64, Option<i64>, u32)> = tx
                    .query_row(
                        "SELECT state, first_seen, dismissed_until, clean_scans FROM insights WHERE id = ?1",
                        params![insight.id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()?;
                let mut insight = insight.clone();
                insight.last_seen = now;
                match existing {
                    None => {
                        insight.first_seen = now;
                        insight.occurrences = 1;
                        insight.state = State::Open;
                        write(tx, &insight, 0)?;
                        changed.opened.push(insight.clone());
                    }
                    Some((state, first_seen, dismissed_until, _)) => {
                        let previous = State::parse(&state).unwrap_or(State::Open);
                        let occurrences: u32 = tx
                            .query_row(
                                "SELECT body FROM insights WHERE id = ?1",
                                params![insight.id],
                                |row| row.get::<_, String>(0),
                            )
                            .ok()
                            .and_then(|body| serde_json::from_str::<Insight>(&body).ok())
                            .map(|stored| stored.occurrences)
                            .unwrap_or(1);
                        insight.first_seen = first_seen;
                        let dismissal_holds = previous == State::Dismissed
                            && dismissed_until.is_none_or(|until| until > now);
                        if dismissal_holds {
                            insight.state = State::Dismissed;
                            insight.dismissed_until = dismissed_until;
                            insight.occurrences = occurrences;
                            write(tx, &insight, 0)?;
                        } else {
                            // A resolved problem that is back, or a dismissal
                            // that has run out, is a new occurrence.
                            let returning = previous != State::Open;
                            insight.state = State::Open;
                            insight.resolved_at = None;
                            insight.dismissed_until = None;
                            insight.occurrences = if returning {
                                occurrences.saturating_add(1)
                            } else {
                                occurrences
                            };
                            write(tx, &insight, 0)?;
                            if returning {
                                changed.opened.push(insight.clone());
                            }
                        }
                    }
                }
            }

            // Findings of this check, on devices this scan decided for, that
            // were not found this time.
            let mut statement = tx.prepare(
                "SELECT body, clean_scans FROM insights
                 WHERE check_id = ?1 AND state != 'resolved'",
            )?;
            let rows = statement
                .query_map(params![outcome.check], |row| {
                    Ok((row_to_insight(row)?, row.get::<_, u32>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (mut insight, clean_scans) in rows {
                if found_ids.contains(insight.id.as_str())
                    || !outcome.evaluated.contains(&insight.device_id)
                {
                    continue;
                }
                let clean = clean_scans + 1;
                if clean >= 2 {
                    insight.state = State::Resolved;
                    insight.resolved_at = Some(now);
                    write(tx, &insight, clean)?;
                    changed.resolved.push(insight);
                } else {
                    write(tx, &insight, clean)?;
                }
            }
        }
        Ok(())
    })?;

    changed.open = store.with(|conn| {
        let mut statement = conn.prepare(
            "SELECT body FROM insights WHERE state = 'open' ORDER BY severity DESC, last_seen DESC",
        )?;
        let rows = statement.query_map([], row_to_insight)?;
        rows.collect()
    })?;
    Ok(changed)
}

pub(crate) fn get(store: &Store, id: &str) -> AppResult<Insight> {
    let found: Option<Insight> = store.with(|conn| {
        conn.query_row(
            "SELECT body FROM insights WHERE id = ?1",
            params![id],
            row_to_insight,
        )
        .optional()
    })?;
    found.ok_or_else(|| AppError::not_found(format!("insight `{id}`")))
}

pub(crate) fn save(store: &Store, insight: &Insight) -> AppResult<()> {
    store.with(|conn| {
        let clean: u32 = conn
            .query_row(
                "SELECT clean_scans FROM insights WHERE id = ?1",
                params![insight.id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        write(conn, insight, clean)
    })
}

/// What a list is being narrowed to.
#[derive(Clone, Debug, Default)]
pub(crate) struct Filter {
    pub state: Option<String>,
    pub severity: Option<Severity>,
    pub category: Option<Category>,
    pub check: Option<String>,
    pub device_id: Option<String>,
    pub query: Option<String>,
    pub include_lab: bool,
}

fn where_clause(filter: &Filter) -> (String, Vec<SqlValue>) {
    let mut sql = String::from(" WHERE 1=1");
    let mut args: Vec<SqlValue> = Vec::new();
    if let Some(state) = &filter.state {
        sql.push_str(" AND state = ?");
        args.push(state.clone().into());
    }
    if let Some(severity) = filter.severity {
        sql.push_str(" AND severity >= ?");
        args.push(severity.rank().into());
    }
    if let Some(category) = filter.category {
        sql.push_str(" AND category = ?");
        args.push(category.as_str().to_string().into());
    }
    if let Some(check) = &filter.check {
        sql.push_str(" AND check_id = ?");
        args.push(check.clone().into());
    }
    if let Some(device_id) = &filter.device_id {
        // The owner of the finding, or the device at the other end of it.
        sql.push_str(" AND (device_id = ? OR related LIKE '%,' || ? || ',%')");
        args.push(device_id.clone().into());
        args.push(device_id.clone().into());
    }
    if let Some(query) = filter
        .query
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        sql.push_str(" AND search LIKE ? ESCAPE '\\'");
        let pattern = format!(
            "%{}%",
            query
                .trim()
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        args.push(pattern.into());
    }
    if !filter.include_lab {
        sql.push_str(" AND lab = 0");
    }
    (sql, args)
}

/// A page of findings, worst first, with the cursor for the next page.
pub(crate) fn page(
    store: &Store,
    filter: &Filter,
    cursor: Option<&str>,
    limit: usize,
) -> AppResult<(Vec<Insight>, Option<String>)> {
    let (mut sql, mut args) = where_clause(filter);
    if let Some(cursor) = cursor {
        let parts: Vec<&str> = cursor.split(':').collect();
        let (Some(severity), Some(last_seen), Some(id)) = (
            parts.first().and_then(|part| part.parse::<i64>().ok()),
            parts.get(1).and_then(|part| part.parse::<i64>().ok()),
            parts.get(2),
        ) else {
            return Err(AppError::invalid("that cursor is not one of ours"));
        };
        sql.push_str(
            " AND (severity < ? OR (severity = ? AND (last_seen < ? OR (last_seen = ? AND id > ?))))",
        );
        args.push(severity.into());
        args.push(severity.into());
        args.push(last_seen.into());
        args.push(last_seen.into());
        args.push((*id).to_string().into());
    }
    sql.push_str(" ORDER BY severity DESC, last_seen DESC, id ASC LIMIT ?");
    args.push((limit as i64 + 1).into());

    let mut rows: Vec<Insight> = store.with(|conn| {
        let mut statement = conn.prepare(&format!("SELECT body FROM insights{sql}"))?;
        let rows = statement.query_map(params_from_iter(args.iter()), row_to_insight)?;
        rows.collect()
    })?;
    let next = (rows.len() > limit).then(|| {
        rows.truncate(limit);
        let last = rows.last().expect("a page with a last row");
        format!("{}:{}:{}", last.severity.rank(), last.last_seen, last.id)
    });
    Ok((rows, next))
}

/// How many findings there are per severity and per category.
pub(crate) fn counts(
    store: &Store,
    filter: &Filter,
) -> AppResult<(BTreeMap<String, u64>, BTreeMap<String, u64>)> {
    let (sql, args) = where_clause(filter);
    let mut by_severity: BTreeMap<String, u64> = Severity::ALL
        .iter()
        .map(|severity| (severity.as_str().to_string(), 0))
        .collect();
    let mut by_category: BTreeMap<String, u64> = Category::ALL
        .iter()
        .map(|category| (category.as_str().to_string(), 0))
        .collect();
    let rows: Vec<(i64, String, u64)> = store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT severity, category, COUNT(*) FROM insights{sql} GROUP BY severity, category"
        ))?;
        let rows = statement.query_map(params_from_iter(args.iter()), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, i64>(2)?.max(0) as u64,
            ))
        })?;
        rows.collect()
    })?;
    for (severity, category, count) in rows {
        *by_severity
            .entry(Severity::from_rank(severity).as_str().to_string())
            .or_insert(0) += count;
        *by_category.entry(category).or_insert(0) += count;
    }
    Ok((by_severity, by_category))
}

/// Drop every finding about a device (when it leaves the inventory).
pub(crate) fn forget_device(store: &Store, device_id: &str) -> AppResult<usize> {
    store.with(|conn| {
        conn.execute(
            "DELETE FROM insights WHERE device_id = ?1 OR related LIKE '%,' || ?1 || ',%'",
            params![device_id],
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Finding, Related};

    fn insight(check: &'static str, device_id: &str, subject: &str, severity: Severity) -> Insight {
        let finding = Finding::new(check, device_id, subject, format!("{subject} is wrong"));
        crate::scan::to_insight(&finding, Category::Physical, severity, "SW1", false, 100)
    }

    fn store() -> Store {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        store
    }

    fn coverage_at(scanned_at: i64) -> Coverage {
        Coverage {
            scanned_at,
            devices: 1,
            link_source: "neighbors".into(),
            links: 0,
            checks: Vec::new(),
            missing_sources: Vec::new(),
            took_ms: 1,
        }
    }

    /// The tests below are about findings, so they say nothing about the
    /// coverage that goes in with them; this shadows the real one to keep
    /// them reading as they did.
    fn reconcile(store: &Store, now: i64, outcomes: &[Outcome]) -> AppResult<Changed> {
        super::reconcile(store, now, outcomes, &coverage_at(now))
    }

    /// Findings and the coverage they were found under are stored together,
    /// so no reader can ever see one without the other. Before they shared a
    /// transaction, `insights.list` answered with findings while
    /// `insights.coverage` still said nothing had been scanned — the exact
    /// contradiction between "nothing is wrong" and "nobody looked" that this
    /// crate is built to avoid.
    #[test]
    fn the_coverage_lands_with_the_findings_and_not_after_them() {
        let store = store();
        assert!(
            store
                .kv_get::<Coverage>("insights.coverage")
                .unwrap()
                .is_none(),
            "nothing has been scanned yet"
        );

        let found = insight("duplex_mismatch", "a", "Gi0/1", Severity::High);
        let changed = super::reconcile(
            &store,
            1_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: vec![found.clone()],
                evaluated: [String::from("a")].into_iter().collect(),
            }],
            &coverage_at(1_000),
        )
        .unwrap();
        assert_eq!(changed.opened.len(), 1);

        let stored: Coverage = store
            .kv_get("insights.coverage")
            .unwrap()
            .expect("the coverage is there the moment the findings are");
        assert_eq!(stored.scanned_at, 1_000);
        assert_eq!(get(&store, &found.id).unwrap().state, State::Open);
    }

    #[test]
    fn a_finding_keeps_its_identity_and_resolves_after_two_clean_scans() {
        let store = store();
        let found = insight("duplex_mismatch", "a", "Gi0/1", Severity::High);
        let evaluated: BTreeSet<String> = [String::from("a")].into_iter().collect();

        let first = reconcile(
            &store,
            1_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: vec![found.clone()],
                evaluated: evaluated.clone(),
            }],
        )
        .unwrap();
        assert_eq!(first.opened.len(), 1);
        assert_eq!(first.open.len(), 1);
        let id = found.id.clone();
        assert_eq!(get(&store, &id).unwrap().first_seen, 1_000);

        // Seen again: same id, same first_seen, new last_seen, nothing opened.
        let again = reconcile(
            &store,
            2_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: vec![found.clone()],
                evaluated: evaluated.clone(),
            }],
        )
        .unwrap();
        assert!(again.opened.is_empty() && again.resolved.is_empty());
        let stored = get(&store, &id).unwrap();
        assert_eq!((stored.first_seen, stored.last_seen), (1_000, 2_000));
        assert_eq!(stored.occurrences, 1);

        // Gone once: still open, because one clean scan is not two.
        let clean = Outcome {
            check: "duplex_mismatch",
            found: Vec::new(),
            evaluated: evaluated.clone(),
        };
        let first_clean = reconcile(&store, 3_000, &[clean]).unwrap();
        assert!(first_clean.resolved.is_empty());
        assert_eq!(get(&store, &id).unwrap().state, State::Open);

        let second_clean = reconcile(
            &store,
            4_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: Vec::new(),
                evaluated: evaluated.clone(),
            }],
        )
        .unwrap();
        assert_eq!(second_clean.resolved.len(), 1);
        let stored = get(&store, &id).unwrap();
        assert_eq!(stored.state, State::Resolved);
        assert_eq!(stored.resolved_at, Some(4_000));

        // Back again: the same insight, counted as a second occurrence.
        let returned = reconcile(
            &store,
            5_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: vec![found.clone()],
                evaluated: evaluated.clone(),
            }],
        )
        .unwrap();
        assert_eq!(returned.opened.len(), 1);
        let stored = get(&store, &id).unwrap();
        assert_eq!(
            (stored.state, stored.occurrences, stored.first_seen),
            (State::Open, 2, 1_000)
        );
    }

    #[test]
    fn a_device_that_was_not_evaluated_never_resolves_its_own_findings() {
        let store = store();
        let found = insight("duplex_mismatch", "a", "Gi0/1", Severity::High);
        reconcile(
            &store,
            1_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: vec![found.clone()],
                evaluated: [String::from("a")].into_iter().collect(),
            }],
        )
        .unwrap();
        for at in [2_000, 3_000, 4_000] {
            // The scan could not decide for `a` (no data), so the finding stands.
            reconcile(
                &store,
                at,
                &[Outcome {
                    check: "duplex_mismatch",
                    found: Vec::new(),
                    evaluated: BTreeSet::new(),
                }],
            )
            .unwrap();
        }
        assert_eq!(get(&store, &found.id).unwrap().state, State::Open);
    }

    #[test]
    fn a_dismissal_holds_until_it_expires() {
        let store = store();
        let found = insight("telnet_enabled", "a", "line vty", Severity::High);
        let evaluated: BTreeSet<String> = [String::from("a")].into_iter().collect();
        reconcile(
            &store,
            1_000,
            &[Outcome {
                check: "telnet_enabled",
                found: vec![found.clone()],
                evaluated: evaluated.clone(),
            }],
        )
        .unwrap();

        let mut dismissed = get(&store, &found.id).unwrap();
        dismissed.state = State::Dismissed;
        dismissed.dismissed_until = Some(5_000);
        save(&store, &dismissed).unwrap();

        // Still there, still dismissed, and not reported as newly opened.
        let during = reconcile(
            &store,
            2_000,
            &[Outcome {
                check: "telnet_enabled",
                found: vec![found.clone()],
                evaluated: evaluated.clone(),
            }],
        )
        .unwrap();
        assert!(during.opened.is_empty());
        assert!(during.open.is_empty(), "a dismissed finding is not open");
        assert_eq!(get(&store, &found.id).unwrap().state, State::Dismissed);

        // After it expires the next sighting opens it again.
        let after = reconcile(
            &store,
            6_000,
            &[Outcome {
                check: "telnet_enabled",
                found: vec![found.clone()],
                evaluated,
            }],
        )
        .unwrap();
        assert_eq!(after.opened.len(), 1);
        assert_eq!(get(&store, &found.id).unwrap().state, State::Open);
    }

    #[test]
    fn lists_filter_count_and_page_in_sql() {
        let store = store();
        let mut findings = Vec::new();
        for (index, severity) in [
            Severity::Critical,
            Severity::High,
            Severity::Medium,
            Severity::Low,
            Severity::Info,
        ]
        .into_iter()
        .enumerate()
        {
            let mut insight = insight("duplex_mismatch", "a", &format!("Gi0/{index}"), severity);
            insight.related.push(Related {
                device_id: "b".into(),
                subject: "Gi1/0/1".into(),
                device: "SW2".into(),
            });
            findings.push(insight);
        }
        let mut lab = insight("unused_vlan", "c", "VLAN 40", Severity::Info);
        lab.lab = true;
        lab.category = Category::Hygiene;
        findings.push(lab);
        reconcile(
            &store,
            1_000,
            &[Outcome {
                check: "duplex_mismatch",
                found: findings.clone(),
                evaluated: BTreeSet::new(),
            }],
        )
        .unwrap();

        let open = Filter {
            state: Some("open".into()),
            include_lab: true,
            ..Filter::default()
        };
        let (page_one, cursor) = page(&store, &open, None, 2).unwrap();
        assert_eq!(page_one.len(), 2);
        assert_eq!(page_one[0].severity, Severity::Critical);
        let (page_two, _) = page(&store, &open, cursor.as_deref(), 10).unwrap();
        assert_eq!(page_two.len(), 4);
        assert!(
            page_two
                .iter()
                .all(|insight| insight.severity < Severity::High)
        );

        // Lab devices are excluded unless asked for.
        let (without_lab, _) = page(
            &store,
            &Filter {
                state: Some("open".into()),
                ..Filter::default()
            },
            None,
            10,
        )
        .unwrap();
        assert_eq!(without_lab.len(), 5);

        // The far end of a link finds the finding too.
        let (far, _) = page(
            &store,
            &Filter {
                device_id: Some("b".into()),
                include_lab: true,
                ..Filter::default()
            },
            None,
            10,
        )
        .unwrap();
        assert_eq!(far.len(), 5);

        let (severity, category) = counts(
            &store,
            &Filter {
                include_lab: true,
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(severity["critical"], 1);
        assert_eq!(severity["info"], 2);
        assert_eq!(category["physical"], 5);
        assert_eq!(category["hygiene"], 1);

        // Free text searches the device, subject, title and check.
        let (hits, _) = page(
            &store,
            &Filter {
                query: Some("gi0/3".into()),
                include_lab: true,
                ..Filter::default()
            },
            None,
            10,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);

        assert_eq!(forget_device(&store, "a").unwrap(), 5);
        assert_eq!(
            page(
                &store,
                &Filter {
                    include_lab: true,
                    ..Filter::default()
                },
                None,
                10
            )
            .unwrap()
            .0
            .len(),
            1
        );
        assert_eq!(get(&store, "nope").unwrap_err().code, "not_found");
        assert_eq!(
            page(&store, &Filter::default(), Some("rubbish"), 10)
                .unwrap_err()
                .code,
            "invalid"
        );
    }
}
