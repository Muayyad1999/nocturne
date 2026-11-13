//! Search: the filter every method in this crate shares, and the SQL behind
//! `logs.query`, `logs.histogram`, `logs.top`, `logs.bursts` and
//! `logs.sources`.
//!
//! A campus keeps millions of lines, so nothing here loads rows to throw
//! them away: every filter is a column, every count is done by SQLite, and
//! paging is by id (`before_id`) rather than by offset, which stays correct
//! while new lines arrive underneath the reader.

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use rusqlite::types::Value as Sql;
use serde::{Deserialize, Serialize};

use crate::db::{self, Prepared};
use crate::model::{Bucket, Burst, TopRow};

/// What a search is looking for. Shared by the query methods and by a live
/// tail, so that "follow what I am looking at" means exactly the same filter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// `syslog` (the default), `audit`, `event`, … Empty means every stream.
    pub stream: Option<String>,
    pub device_id: Option<String>,
    /// Several devices at once (a site, a selection on screen).
    pub device_ids: Vec<String>,
    /// Keep entries at least this severe (numerically ≤, as syslog counts:
    /// 3 means error and worse).
    pub max_severity: Option<u8>,
    /// Syslog facility number, 0…23.
    pub facility: Option<u8>,
    /// Vendor mnemonic, e.g. `LINK-3-UPDOWN`. A trailing `*` matches a prefix.
    pub mnemonic: Option<String>,
    /// An interface of the device the line is about, in any spelling.
    pub interface: Option<String>,
    /// The sending address.
    pub source: Option<String>,
    /// Words that must appear in the message or the source. A word with a
    /// leading `-` must not appear; `"two words"` is one phrase.
    pub text: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// `true`: only lines no device could be attributed to.
    pub unattributed: Option<bool>,
}

/// One term of a text query.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    text: String,
    negated: bool,
}

/// `foo -bar "two words"` → three terms.
fn terms(text: &str) -> Vec<Term> {
    let mut terms = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let negated = rest.starts_with('-') && rest.len() > 1;
        if negated {
            rest = &rest[1..];
        }
        let (word, tail) = if let Some(after) = rest.strip_prefix('"') {
            match after.split_once('"') {
                Some((word, tail)) => (word, tail),
                None => (after, ""),
            }
        } else {
            match rest.split_once(char::is_whitespace) {
                Some((word, tail)) => (word, tail),
                None => (rest, ""),
            }
        };
        if !word.is_empty() {
            terms.push(Term {
                text: word.to_string(),
                negated,
            });
        }
        rest = tail.trim_start();
    }
    terms
}

fn like(text: &str) -> String {
    format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// The `WHERE` clause of a filter, and whether it needs the index table.
struct Clause {
    sql: String,
    args: Vec<Sql>,
    needs_meta: bool,
}

fn clause(filter: &Filter) -> Clause {
    let mut sql = String::from(" WHERE 1=1");
    let mut args: Vec<Sql> = Vec::new();
    let mut needs_meta = false;
    let add = |sql: &mut String, text: &str, value: Sql, args: &mut Vec<Sql>| {
        sql.push_str(text);
        args.push(value);
    };
    match filter.stream.as_deref() {
        Some("") | Some("*") => {}
        Some(stream) => add(
            &mut sql,
            " AND l.stream = ?",
            stream.to_string().into(),
            &mut args,
        ),
        None => add(
            &mut sql,
            " AND l.stream = ?",
            db::STREAM.to_string().into(),
            &mut args,
        ),
    }
    let devices: Vec<&String> = filter
        .device_id
        .iter()
        .chain(filter.device_ids.iter())
        .filter(|id| !id.is_empty())
        .collect();
    if !devices.is_empty() {
        let places = std::iter::repeat_n("?", devices.len())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND l.device_id IN ({places})"));
        args.extend(devices.into_iter().map(|id| Sql::from(id.clone())));
    }
    if filter.unattributed == Some(true) {
        sql.push_str(" AND l.device_id IS NULL");
    }
    if let Some(severity) = filter.max_severity {
        add(
            &mut sql,
            " AND l.severity <= ?",
            i64::from(severity).into(),
            &mut args,
        );
    }
    if let Some(source) = filter.source.as_deref().filter(|text| !text.is_empty()) {
        add(
            &mut sql,
            " AND l.source = ?",
            source.to_string().into(),
            &mut args,
        );
    }
    if let Some(since) = filter.since {
        add(&mut sql, " AND l.ts >= ?", since.into(), &mut args);
    }
    if let Some(until) = filter.until {
        add(&mut sql, " AND l.ts <= ?", until.into(), &mut args);
    }
    if let Some(facility) = filter.facility {
        needs_meta = true;
        add(
            &mut sql,
            " AND s.facility = ?",
            i64::from(facility).into(),
            &mut args,
        );
    }
    if let Some(mnemonic) = filter.mnemonic.as_deref().filter(|text| !text.is_empty()) {
        needs_meta = true;
        match mnemonic.strip_suffix('*') {
            Some(prefix) => add(
                &mut sql,
                " AND s.mnemonic LIKE ? ESCAPE '\\'",
                format!(
                    "{}%",
                    prefix
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
                .into(),
                &mut args,
            ),
            None => add(
                &mut sql,
                " AND s.mnemonic = ?",
                mnemonic.to_string().into(),
                &mut args,
            ),
        }
    }
    if let Some(interface) = filter.interface.as_deref().filter(|text| !text.is_empty()) {
        needs_meta = true;
        // Stored canonically, asked for in whatever spelling the user typed.
        let long = nocturne_drivers::ifname::long(interface);
        sql.push_str(" AND (s.interface = ? COLLATE NOCASE OR s.interface = ? COLLATE NOCASE)");
        args.push(interface.to_string().into());
        args.push(long.into());
    }
    for term in filter
        .text
        .as_deref()
        .map(terms)
        .unwrap_or_default()
        .into_iter()
        .take(16)
    {
        // The vendor's mnemonic is not part of the message column, and a
        // person searching for `UPDOWN` means the mnemonic, so a word is
        // looked for in the message, the sending address and the mnemonic.
        needs_meta = true;
        let pattern = like(&term.text);
        if term.negated {
            sql.push_str(
                " AND l.message NOT LIKE ? ESCAPE '\\' AND COALESCE(s.mnemonic, '') NOT LIKE ? ESCAPE '\\'",
            );
            args.push(pattern.clone().into());
            args.push(pattern.into());
        } else {
            sql.push_str(
                " AND (l.message LIKE ? ESCAPE '\\' OR l.source LIKE ? ESCAPE '\\'
                       OR COALESCE(s.mnemonic, '') LIKE ? ESCAPE '\\')",
            );
            args.push(pattern.clone().into());
            args.push(pattern.clone().into());
            args.push(pattern.into());
        }
    }
    Clause {
        sql,
        args,
        needs_meta,
    }
}

fn from(needs_meta: bool) -> &'static str {
    if needs_meta {
        // Left, not inner: the `audit` and `event` streams have no row in
        // this feature's index table and must still be searchable.
        " FROM logs l LEFT JOIN logs_syslog s ON s.log_id = l.id"
    } else {
        " FROM logs l"
    }
}

impl Filter {
    /// The same filter, applied to an entry on its way past — what a live
    /// tail needs so that following and searching agree.
    pub fn matches(&self, item: &Prepared) -> bool {
        let entry = &item.entry;
        match self.stream.as_deref() {
            Some("") | Some("*") => {}
            Some(stream) if stream != entry.stream => return false,
            None if entry.stream != db::STREAM => return false,
            _ => {}
        }
        let devices: Vec<&String> = self
            .device_id
            .iter()
            .chain(self.device_ids.iter())
            .filter(|id| !id.is_empty())
            .collect();
        if !devices.is_empty()
            && !entry
                .device_id
                .as_ref()
                .is_some_and(|id| devices.contains(&id))
        {
            return false;
        }
        if self.unattributed == Some(true) && entry.device_id.is_some() {
            return false;
        }
        if let Some(max) = self.max_severity
            && !entry.severity.is_some_and(|severity| severity <= max)
        {
            return false;
        }
        if let Some(facility) = self.facility
            && item.facility != Some(facility)
        {
            return false;
        }
        if let Some(source) = self.source.as_deref().filter(|text| !text.is_empty())
            && entry.source != source
        {
            return false;
        }
        if let Some(wanted) = self.mnemonic.as_deref().filter(|text| !text.is_empty()) {
            let mnemonic = item.mnemonic.as_deref().unwrap_or_default();
            let ok = match wanted.strip_suffix('*') {
                Some(prefix) => mnemonic.starts_with(prefix),
                None => mnemonic == wanted,
            };
            if !ok {
                return false;
            }
        }
        if let Some(wanted) = self.interface.as_deref().filter(|text| !text.is_empty()) {
            let interface = item.interface.as_deref().unwrap_or_default();
            if !nocturne_drivers::ifname::same(interface, wanted) || interface.is_empty() {
                return false;
            }
        }
        if let Some(since) = self.since
            && entry.ts < since
        {
            return false;
        }
        if let Some(until) = self.until
            && entry.ts > until
        {
            return false;
        }
        for term in self.text.as_deref().map(terms).unwrap_or_default() {
            let mnemonic = item.mnemonic.as_deref().unwrap_or_default();
            let found = contains_fold(&entry.message, &term.text)
                || contains_fold(mnemonic, &term.text)
                || (!term.negated && contains_fold(&entry.source, &term.text));
            if found == term.negated {
                return false;
            }
        }
        true
    }
}

fn contains_fold(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntry> {
    let body: Option<String> = row.get(7)?;
    Ok(LogEntry {
        id: row.get(0)?,
        ts: row.get(1)?,
        stream: row.get(2)?,
        device_id: row.get(3)?,
        severity: row.get(4)?,
        source: row.get(5)?,
        message: row.get(6)?,
        body: body.and_then(|body| serde_json::from_str(&body).ok()),
    })
}

const COLUMNS: &str =
    " SELECT l.id, l.ts, l.stream, l.device_id, l.severity, l.source, l.message, l.body";

/// `logs.query`: a page of entries, newest first.
pub fn page(
    app: &App,
    filter: &Filter,
    before_id: Option<i64>,
    after_id: Option<i64>,
    limit: u32,
) -> AppResult<(Vec<LogEntry>, bool)> {
    let limit = limit.clamp(1, 1_000);
    let mut clause = clause(filter);
    if let Some(before) = before_id {
        clause.sql.push_str(" AND l.id < ?");
        clause.args.push(before.into());
    }
    if let Some(after) = after_id {
        clause.sql.push_str(" AND l.id > ?");
        clause.args.push(after.into());
    }
    // One more than asked for says whether there is another page.
    let sql = format!(
        "{COLUMNS}{}{} ORDER BY l.id DESC LIMIT {}",
        from(clause.needs_meta),
        clause.sql,
        limit + 1
    );
    let mut entries: Vec<LogEntry> = app.store().with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(clause.args.iter()), row_to_entry)?;
        rows.collect()
    })?;
    let has_more = entries.len() > limit as usize;
    entries.truncate(limit as usize);
    Ok((entries, has_more))
}

/// `logs.histogram`: counts per bucket, including the empty buckets — a
/// histogram with the quiet hours left out is a lie about the shape.
pub fn histogram(
    app: &App,
    filter: &Filter,
    since: i64,
    until: i64,
    buckets: u32,
) -> AppResult<Vec<Bucket>> {
    if until <= since {
        return Err(AppError::invalid(
            "a histogram needs a time range: `until` must be after `since`",
        ));
    }
    let buckets = buckets.clamp(1, 1_000) as i64;
    let width = ((until - since) as f64 / buckets as f64).ceil() as i64;
    let width = width.max(1);
    let mut filter = filter.clone();
    filter.since = Some(since);
    filter.until = Some(until);
    let clause = clause(&filter);
    let severities = (0..8)
        .map(|severity| format!("SUM(CASE WHEN l.severity = {severity} THEN 1 ELSE 0 END)"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT (l.ts - {since}) / {width} AS bucket, COUNT(*), {severities},
                SUM(CASE WHEN l.severity IS NULL THEN 1 ELSE 0 END)
         {}{} GROUP BY bucket",
        from(clause.needs_meta),
        clause.sql
    );
    let counted: Vec<(i64, i64, [i64; 8], i64)> = app.store().with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(clause.args.iter()), |row| {
            let mut by_severity = [0i64; 8];
            for (index, slot) in by_severity.iter_mut().enumerate() {
                *slot = row.get(2 + index)?;
            }
            Ok((row.get(0)?, row.get(1)?, by_severity, row.get(10)?))
        })?;
        rows.collect()
    })?;
    let mut out: Vec<Bucket> = (0..buckets)
        .map(|index| Bucket {
            t: since + index * width,
            count: 0,
            by_severity: [0; 8],
            no_severity: 0,
        })
        .collect();
    for (index, count, by_severity, no_severity) in counted {
        if let Some(bucket) = usize::try_from(index)
            .ok()
            .and_then(|index| out.get_mut(index))
        {
            bucket.count = count;
            bucket.by_severity = by_severity;
            bucket.no_severity = no_severity;
        }
    }
    Ok(out)
}

/// `logs.top`: the biggest talkers of a window, by whichever key.
pub fn top(app: &App, filter: &Filter, by: &str, limit: u32) -> AppResult<Vec<TopRow>> {
    let (column, needs_meta) = match by {
        "device" => ("COALESCE(l.device_id, '')", false),
        "source" => ("l.source", false),
        "severity" => ("COALESCE(CAST(l.severity AS TEXT), '')", false),
        "mnemonic" => ("COALESCE(s.mnemonic, '')", true),
        "facility" => ("COALESCE(CAST(s.facility AS TEXT), '')", true),
        "interface" => ("COALESCE(s.interface, '')", true),
        other => {
            return Err(AppError::invalid(format!(
                "`{other}` is not something to count by (device, source, severity, mnemonic, facility, interface)"
            )));
        }
    };
    let clause = clause(filter);
    let sql = format!(
        "SELECT {column} AS key, COUNT(*) AS count {}{} GROUP BY key HAVING key != ''
         ORDER BY count DESC, key LIMIT ?",
        from(clause.needs_meta || needs_meta),
        clause.sql
    );
    let mut args = clause.args;
    args.push(i64::from(limit.clamp(1, 500)).into());
    let rows: Vec<(String, i64)> = app.store().with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect()
    })?;
    let index = crate::attribute::refresh(app, now_ms(), 30_000)?;
    Ok(rows
        .into_iter()
        .map(|(key, count)| {
            let label = match by {
                "device" => index.name_of(&key),
                "severity" => key
                    .parse::<u8>()
                    .map(nocturne_netprobe::syslog::severity_name)
                    .unwrap_or_default()
                    .to_string(),
                "facility" => key
                    .parse::<u8>()
                    .map(nocturne_netprobe::syslog::facility_name)
                    .unwrap_or_default()
                    .to_string(),
                _ => String::new(),
            };
            TopRow { key, label, count }
        })
        .collect())
}

/// `logs.bursts`: runs of the same thing, which is what a screen shows
/// instead of four hundred identical lines — and the end a correlation is
/// started from ("what was this switch shouting about at 03:14?").
pub fn bursts(app: &App, filter: &Filter, min_count: i64, limit: u32) -> AppResult<Vec<Burst>> {
    let clause = clause(filter);
    let sql = format!(
        "SELECT l.device_id, s.mnemonic, s.interface, MIN(l.severity), COUNT(*),
                MIN(l.ts), MAX(l.ts), MIN(l.id), MAX(l.id), MIN(l.message)
         FROM logs l JOIN logs_syslog s ON s.log_id = l.id{}
         GROUP BY l.device_id, s.mnemonic, s.interface
         HAVING COUNT(*) >= ?
         ORDER BY COUNT(*) DESC LIMIT ?",
        clause.sql
    );
    let mut args = clause.args;
    args.push(min_count.max(1).into());
    args.push(i64::from(limit.clamp(1, 500)).into());
    let rows: Vec<Burst> = app.store().with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            Ok(Burst {
                device_id: row.get(0)?,
                device: String::new(),
                mnemonic: row.get(1)?,
                interface: row.get(2)?,
                severity: row.get(3)?,
                count: row.get(4)?,
                first_at: row.get(5)?,
                last_at: row.get(6)?,
                first_id: row.get(7)?,
                last_id: row.get(8)?,
                example: row.get(9)?,
            })
        })?;
        rows.collect()
    })?;
    let index = crate::attribute::refresh(app, now_ms(), 30_000)?;
    Ok(rows
        .into_iter()
        .map(|burst| Burst {
            device: burst
                .device_id
                .as_deref()
                .map(|id| index.name_of(id))
                .unwrap_or_default(),
            ..burst
        })
        .collect())
}

/// One address that has sent lines, with the device that claims it now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    #[serde(flatten)]
    pub row: db::SourceRow,
    /// The device's name, when it is attributed.
    pub device: String,
}

/// `logs.sources`: every address that has sent, re-checked against the
/// inventory as it is now — so adding the device the user was told about
/// makes the list say so without waiting for the next line.
pub fn sources(app: &App) -> AppResult<Vec<Source>> {
    let stored = db::sources(app.store())?;
    let index = crate::attribute::refresh(app, now_ms(), 0)?;
    let addresses: Vec<String> = stored.iter().map(|row| row.address.clone()).collect();
    let resolved = index.resolve_all(&addresses);
    let changed: Vec<(String, Option<String>, String)> = resolved
        .iter()
        .zip(&stored)
        .filter(|((_, device_id, reason), row)| {
            *device_id != row.device_id || *reason != row.reason
        })
        .map(|(entry, _)| entry.clone())
        .collect();
    if !changed.is_empty() {
        db::reattribute_sources(app.store(), &changed)?;
    }
    Ok(stored
        .into_iter()
        .zip(resolved)
        .map(|(row, (_, device_id, reason))| Source {
            device: device_id
                .as_deref()
                .map(|id| index.name_of(id))
                .unwrap_or_default(),
            row: db::SourceRow {
                device_id,
                reason,
                ..row
            },
        })
        .collect())
}

/// Delete a stream's entries, optionally only those before a time.
pub fn clear(app: &App, stream: &str, before: Option<i64>) -> AppResult<usize> {
    let before = before.unwrap_or_else(now_ms);
    let removed = app.store().log_prune(stream, before)?;
    if stream == db::STREAM {
        app.store().with(|conn| {
            conn.execute(
                "DELETE FROM logs_syslog WHERE log_id NOT IN (SELECT id FROM logs)",
                [],
            )
        })?;
    }
    app.audit(
        "logs.clear",
        None,
        format!("Deleted {removed} `{stream}` log entries"),
        serde_json::json!({"stream": stream, "before": before, "removed": removed}),
    );
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_query_is_words_phrases_and_exclusions() {
        assert_eq!(
            terms("link -up \"changed state\""),
            vec![
                Term {
                    text: "link".into(),
                    negated: false
                },
                Term {
                    text: "up".into(),
                    negated: true
                },
                Term {
                    text: "changed state".into(),
                    negated: false
                },
            ]
        );
        assert!(terms("   ").is_empty());
    }

    #[test]
    fn like_patterns_escape_what_sqlite_would_read_as_a_wildcard() {
        assert_eq!(like("100%_x"), "%100\\%\\_x%");
    }
}
