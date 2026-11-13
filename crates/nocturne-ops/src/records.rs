//! The feature's own tables.
//!
//! Incidents, saved handovers, saved reports and notes are records: a body
//! (the whole thing as JSON), a head (what a list shows), a sort key, one
//! facet and a search text. They sit in separate columns so that a list never
//! parses bodies (timelines, report contents) and a search filters in SQL.
//! What a record refers to (`device:<id>`, `alert:<id>`, …) is kept in a
//! second table, which makes "which records mention this" an index lookup.

use nocturne_app::store::Store;
use nocturne_app::{AppError, AppResult};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS ops_records (
        kind   TEXT NOT NULL,
        id     TEXT NOT NULL,
        sort   INTEGER NOT NULL,
        facet  TEXT NOT NULL DEFAULT '',
        head   TEXT NOT NULL,
        search TEXT NOT NULL DEFAULT '',
        body   TEXT NOT NULL,
        PRIMARY KEY (kind, id)
    );
    CREATE INDEX IF NOT EXISTS ops_records_order ON ops_records (kind, sort);
    CREATE INDEX IF NOT EXISTS ops_records_facet ON ops_records (kind, facet, sort);
    CREATE TABLE IF NOT EXISTS ops_links (
        kind   TEXT NOT NULL,
        target TEXT NOT NULL,
        id     TEXT NOT NULL,
        PRIMARY KEY (kind, target, id)
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS ops_links_record ON ops_links (kind, id);
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Incident,
    Handover,
    Report,
    Note,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Incident => "incident",
            Kind::Handover => "handover",
            Kind::Report => "report",
            Kind::Note => "note",
        }
    }
}

/// What is stored for one record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub id: String,
    /// Lists are ordered by this, largest (newest) first.
    pub sort: i64,
    /// The one value lists filter on (an incident's state, a report's kind).
    pub facet: String,
    /// What a list returns for the record, as JSON.
    pub head: String,
    /// Lower-cased text that searches match against.
    pub search: String,
    /// The record itself, as JSON.
    pub body: String,
    /// What the record refers to, e.g. `device:<id>`.
    pub links: Vec<String>,
}

/// A filter over the records of one kind.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// Keep records whose facet is one of these (empty = any).
    pub facets: Vec<String>,
    /// Every word must occur in the search text (see [`words`]).
    pub words: Vec<String>,
    /// Every one of these links must be present.
    pub links: Vec<String>,
    /// Continue after this position, as returned in [`Page::next_cursor`].
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Default, PartialEq)]
pub struct Page {
    pub heads: Vec<String>,
    /// Present when more records follow.
    pub next_cursor: Option<String>,
}

/// Create the tables. Safe to call at every start.
pub fn ensure_schema(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

/// The words of a typed query, as the search column stores text.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn put(conn: &Connection, kind: Kind, record: &Record) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO ops_records (kind, id, sort, facet, head, search, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (kind, id) DO UPDATE SET sort = excluded.sort, facet = excluded.facet,
             head = excluded.head, search = excluded.search, body = excluded.body",
        params![kind.as_str(), record.id, record.sort, record.facet, record.head, record.search, record.body],
    )?;
    conn.execute(
        "DELETE FROM ops_links WHERE kind = ?1 AND id = ?2",
        params![kind.as_str(), record.id],
    )?;
    let mut insert = conn
        .prepare_cached("INSERT OR IGNORE INTO ops_links (kind, target, id) VALUES (?1, ?2, ?3)")?;
    for link in &record.links {
        insert.execute(params![kind.as_str(), link, record.id])?;
    }
    Ok(())
}

fn read_body(conn: &Connection, kind: Kind, id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT body FROM ops_records WHERE kind = ?1 AND id = ?2",
        params![kind.as_str(), id],
        |row| row.get(0),
    )
    .optional()
}

/// Insert or replace a record together with its links.
pub fn save(store: &Store, kind: Kind, record: &Record) -> AppResult<()> {
    store.transaction(|tx| put(tx, kind, record))
}

pub fn body(store: &Store, kind: Kind, id: &str) -> AppResult<Option<String>> {
    store.with(|conn| read_body(conn, kind, id))
}

pub fn delete(store: &Store, kind: Kind, id: &str) -> AppResult<bool> {
    store.transaction(|tx| {
        tx.execute(
            "DELETE FROM ops_links WHERE kind = ?1 AND id = ?2",
            params![kind.as_str(), id],
        )?;
        let removed = tx.execute(
            "DELETE FROM ops_records WHERE kind = ?1 AND id = ?2",
            params![kind.as_str(), id],
        )?;
        Ok(removed > 0)
    })
}

/// Read a record, change it and store it again in one transaction, so that
/// two writers cannot lose each other's change. `change` gets the stored
/// body and returns the record to store (`None` = leave it as it is).
pub fn update<T>(
    store: &Store,
    kind: Kind,
    id: &str,
    what: &str,
    change: impl FnOnce(&str) -> AppResult<(Option<Record>, T)>,
) -> AppResult<T> {
    store.transaction(|tx| {
        let Some(body) = read_body(tx, kind, id)? else {
            return Ok(Err(AppError::not_found(format!("{what} `{id}`"))));
        };
        Ok(match change(&body) {
            Ok((record, value)) => {
                if let Some(record) = record {
                    put(tx, kind, &record)?;
                }
                Ok(value)
            }
            Err(err) => Err(err),
        })
    })?
}

fn parse_cursor(cursor: &str) -> AppResult<(i64, String)> {
    cursor
        .split_once(':')
        .and_then(|(sort, id)| Some((sort.parse().ok()?, id.to_string())))
        .ok_or_else(|| {
            AppError::invalid("that list position is no longer valid; load the list again from the start")
        })
}

/// One page of heads, newest first.
pub fn page(store: &Store, kind: Kind, query: &Query) -> AppResult<Page> {
    use rusqlite::types::Value;

    let mut sql = String::from("SELECT id, sort, head FROM ops_records WHERE kind = ?");
    let mut args: Vec<Value> = vec![kind.as_str().to_string().into()];
    if !query.facets.is_empty() {
        let marks = vec!["?"; query.facets.len()].join(", ");
        sql.push_str(&format!(" AND facet IN ({marks})"));
        args.extend(query.facets.iter().cloned().map(Value::from));
    }
    for word in &query.words {
        sql.push_str(" AND instr(search, ?) > 0");
        args.push(word.clone().into());
    }
    for link in &query.links {
        sql.push_str(" AND id IN (SELECT id FROM ops_links WHERE kind = ? AND target = ?)");
        args.push(kind.as_str().to_string().into());
        args.push(link.clone().into());
    }
    if let Some(cursor) = &query.cursor {
        let (sort, id) = parse_cursor(cursor)?;
        sql.push_str(" AND (sort < ? OR (sort = ? AND id < ?))");
        args.extend([sort.into(), sort.into(), id.into()]);
    }
    sql.push_str(" ORDER BY sort DESC, id DESC LIMIT ?");
    let limit = query.limit.max(1);
    // One more than asked for tells whether another page follows.
    args.push((limit as i64 + 1).into());

    let mut rows: Vec<(String, i64, String)> = store.with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect()
    })?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = rows
        .last()
        .filter(|_| more)
        .map(|(id, sort, _)| format!("{sort}:{id}"));
    Ok(Page {
        heads: rows.into_iter().map(|(_, _, head)| head).collect(),
        next_cursor,
    })
}

/// Every head matching `query`, newest first, read page by page.
pub fn all(store: &Store, kind: Kind, mut query: Query) -> AppResult<Vec<String>> {
    query.limit = 500;
    let mut heads = Vec::new();
    loop {
        let page = page(store, kind, &query)?;
        heads.extend(page.heads);
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(cursor),
            None => return Ok(heads),
        }
    }
}

/// How many records each facet has.
pub fn facet_counts(store: &Store, kind: Kind) -> AppResult<Vec<(String, u64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT facet, COUNT(*) FROM ops_records WHERE kind = ?1 GROUP BY facet ORDER BY facet",
        )?;
        let rows = statement.query_map(params![kind.as_str()], |row| {
            Ok((row.get(0)?, row.get::<_, i64>(1)?.max(0) as u64))
        })?;
        rows.collect()
    })
}

/// Ids of the records that refer to `target`.
pub fn linked(store: &Store, kind: Kind, target: &str) -> AppResult<Vec<String>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT id FROM ops_links WHERE kind = ?1 AND target = ?2 ORDER BY id",
        )?;
        let rows = statement.query_map(params![kind.as_str(), target], |row| row.get(0))?;
        rows.collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn store() -> Store {
        let store = Store::in_memory().unwrap();
        ensure_schema(&store).unwrap();
        // Creating the tables twice is harmless.
        ensure_schema(&store).unwrap();
        store
    }

    fn record(id: &str, sort: i64, facet: &str, search: &str, links: &[&str]) -> Record {
        Record {
            id: id.into(),
            sort,
            facet: facet.into(),
            head: format!("head-{id}"),
            search: search.into(),
            body: format!("body-{id}"),
            links: links.iter().map(|link| link.to_string()).collect(),
        }
    }

    fn ids(page: &Page) -> Vec<&str> {
        page.heads
            .iter()
            .map(|head| head.trim_start_matches("head-"))
            .collect()
    }

    #[test]
    fn pages_newest_first_with_a_cursor_that_survives_equal_sort_keys() {
        let store = store();
        for (id, sort) in [("a", 10), ("b", 30), ("c", 20), ("d", 20), ("e", 5)] {
            save(&store, Kind::Incident, &record(id, sort, "open", "", &[])).unwrap();
        }
        // Another kind never leaks into a list.
        save(&store, Kind::Note, &record("z", 99, "", "", &[])).unwrap();

        let query = Query {
            limit: 2,
            ..Query::default()
        };
        let first = page(&store, Kind::Incident, &query).unwrap();
        assert_eq!(ids(&first), ["b", "d"]);
        let second = page(
            &store,
            Kind::Incident,
            &Query {
                cursor: first.next_cursor.clone(),
                ..query.clone()
            },
        )
        .unwrap();
        assert_eq!(ids(&second), ["c", "a"]);
        let third = page(
            &store,
            Kind::Incident,
            &Query {
                cursor: second.next_cursor.clone(),
                ..query.clone()
            },
        )
        .unwrap();
        assert_eq!(ids(&third), ["e"]);
        assert_eq!(third.next_cursor, None);

        assert_eq!(
            all(&store, Kind::Incident, Query::default()).unwrap().len(),
            5
        );
        let bad = page(
            &store,
            Kind::Incident,
            &Query {
                cursor: Some("nonsense".into()),
                ..query
            },
        )
        .unwrap_err();
        assert_eq!(bad.code, "invalid");
    }

    #[test]
    fn filters_by_facet_words_and_links() {
        let store = store();
        save(
            &store,
            Kind::Incident,
            &record("a", 1, "open", "core switch down in hq", &["device:d1", "alert:x"]),
        )
        .unwrap();
        save(
            &store,
            Kind::Incident,
            &record("b", 2, "resolved", "wan link flapping", &["device:d1"]),
        )
        .unwrap();
        save(
            &store,
            Kind::Incident,
            &record("c", 3, "mitigated", "core router 100% cpu", &["device:d2"]),
        )
        .unwrap();
        let find = |query: Query| ids(&page(&store, Kind::Incident, &Query { limit: 10, ..query }).unwrap())
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        assert_eq!(
            find(Query {
                facets: vec!["open".into(), "mitigated".into()],
                ..Query::default()
            }),
            ["c", "a"]
        );
        assert_eq!(
            find(Query {
                words: words("CORE  down"),
                ..Query::default()
            }),
            ["a"]
        );
        // `%` and `_` are ordinary characters, not wildcards.
        assert_eq!(
            find(Query {
                words: words("100%"),
                ..Query::default()
            }),
            ["c"]
        );
        assert_eq!(
            find(Query {
                words: words("c_re"),
                ..Query::default()
            }),
            Vec::<String>::new()
        );
        assert_eq!(
            find(Query {
                links: vec!["device:d1".into()],
                ..Query::default()
            }),
            ["b", "a"]
        );
        assert_eq!(
            find(Query {
                links: vec!["device:d1".into(), "alert:x".into()],
                ..Query::default()
            }),
            ["a"]
        );
        assert_eq!(
            facet_counts(&store, Kind::Incident).unwrap(),
            [
                ("mitigated".to_string(), 1),
                ("open".to_string(), 1),
                ("resolved".to_string(), 1)
            ]
        );
        assert_eq!(linked(&store, Kind::Incident, "device:d1").unwrap(), ["a", "b"]);
        assert!(linked(&store, Kind::Note, "device:d1").unwrap().is_empty());
    }

    #[test]
    fn update_is_read_modify_write_and_replaces_links() {
        let store = store();
        save(&store, Kind::Note, &record("n", 1, "", "old", &["device:d1"])).unwrap();

        let seen = update(&store, Kind::Note, "n", "note", |body| {
            let mut next = record("n", 2, "", "new", &["device:d2"]);
            next.body = format!("{body}+changed");
            Ok((Some(next), body.to_string()))
        })
        .unwrap();
        assert_eq!(seen, "body-n");
        assert_eq!(
            body(&store, Kind::Note, "n").unwrap().as_deref(),
            Some("body-n+changed")
        );
        assert!(linked(&store, Kind::Note, "device:d1").unwrap().is_empty());
        assert_eq!(linked(&store, Kind::Note, "device:d2").unwrap(), ["n"]);

        // A refused change stores nothing; `None` leaves the record alone.
        let refused = update(&store, Kind::Note, "n", "note", |_| {
            Err::<(Option<Record>, ()), _>(AppError::invalid("no"))
        })
        .unwrap_err();
        assert_eq!(refused.code, "invalid");
        update(&store, Kind::Note, "n", "note", |_| Ok((None, ()))).unwrap();
        assert_eq!(
            body(&store, Kind::Note, "n").unwrap().as_deref(),
            Some("body-n+changed")
        );

        let missing = update(&store, Kind::Note, "nope", "note", |_| Ok((None, ()))).unwrap_err();
        assert_eq!(missing.code, "not_found");
        assert!(missing.message.contains("note `nope`"), "{}", missing.message);

        assert!(delete(&store, Kind::Note, "n").unwrap());
        assert!(!delete(&store, Kind::Note, "n").unwrap());
        assert_eq!(body(&store, Kind::Note, "n").unwrap(), None);
        assert!(linked(&store, Kind::Note, "device:d2").unwrap().is_empty());
    }
}
