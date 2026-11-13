//! Metric history, summarised in the database rather than loaded.
//!
//! "Sustained over 80 %" and "four transitions in an hour" are questions
//! about a series, not about a value, and a campus switch has a hundred
//! series. Answering them by reading samples into memory would mean millions
//! of rows per scan, and answering them through `Store::samples_query` would
//! mean one query per interface — tens of thousands per scan. So the
//! aggregation happens in SQL: one statement per device and metric.
//!
//! **This is the one place in the crate that reads tables belonging to the
//! core** (`series`, `samples`), read-only. It wants a core API —
//! `samples_window(device_id, metric, since)` or an aggregate equivalent —
//! and should be a handful of lines once there is one. It is kept in a module
//! of its own so that the day that API arrives, nothing else changes.

use nocturne_app::AppResult;
use nocturne_app::store::Store;
use rusqlite::params;

/// How one series behaved over a window.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// The interface or sensor the series is about (empty for device-wide metrics).
    pub instance: String,
    pub points: u32,
    pub first_at: i64,
    pub last_at: i64,
    pub avg: f64,
    pub max: f64,
    /// Points at or above the threshold the query was given.
    pub over: u32,
}

impl Series {
    /// The stretch the samples cover, in minutes.
    pub fn span_minutes(&self) -> i64 {
        (self.last_at - self.first_at) / 60_000
    }

    /// The share of the window spent over the threshold.
    pub fn over_fraction(&self) -> f64 {
        if self.points == 0 {
            0.0
        } else {
            f64::from(self.over) / f64::from(self.points)
        }
    }
}

/// Every instance of one metric on one device, summarised against `threshold`.
pub fn summarise(
    store: &Store,
    device_id: &str,
    metric: &str,
    since: i64,
    threshold: f64,
) -> AppResult<Vec<Series>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT s.instance, COUNT(*), MIN(m.ts), MAX(m.ts), AVG(m.value), MAX(m.value),
                    SUM(CASE WHEN m.value >= ?4 THEN 1 ELSE 0 END)
             FROM series s JOIN samples m ON m.series_id = s.id
             WHERE s.device_id = ?1 AND s.metric = ?2 AND m.ts >= ?3
             GROUP BY s.instance",
        )?;
        let rows = statement.query_map(params![device_id, metric, since, threshold], |row| {
            Ok(Series {
                instance: row.get(0)?,
                points: row.get::<_, i64>(1)?.max(0) as u32,
                first_at: row.get(2)?,
                last_at: row.get(3)?,
                avg: row.get(4)?,
                max: row.get(5)?,
                over: row.get::<_, i64>(6)?.max(0) as u32,
            })
        })?;
        rows.collect()
    })
}

/// How often each instance of a metric changed value in the window — for
/// `if.oper`, which is 1 or 0, that is the number of up/down transitions.
pub fn transitions(
    store: &Store,
    device_id: &str,
    metric: &str,
    since: i64,
) -> AppResult<Vec<(String, u32, i64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT instance, SUM(changed), MAX(ts) FROM (
                 SELECT s.instance AS instance, m.ts AS ts,
                        CASE WHEN m.value <> LAG(m.value) OVER (PARTITION BY s.id ORDER BY m.ts)
                             THEN 1 ELSE 0 END AS changed
                 FROM series s JOIN samples m ON m.series_id = s.id
                 WHERE s.device_id = ?1 AND s.metric = ?2 AND m.ts >= ?3
             ) GROUP BY instance",
        )?;
        let rows = statement.query_map(params![device_id, metric, since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as u32,
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })?;
        rows.collect()
    })
}

/// The hour of the day (UTC) in which one series is busiest, and its average
/// there — asked only for instances that already qualify, so the cost of the
/// extra grouping is paid a handful of times per scan.
pub fn busiest_hour(
    store: &Store,
    device_id: &str,
    metric: &str,
    instance: &str,
    since: i64,
) -> AppResult<Option<(u32, f64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT (m.ts / 3600000) % 24 AS hour, AVG(m.value)
             FROM series s JOIN samples m ON m.series_id = s.id
             WHERE s.device_id = ?1 AND s.metric = ?2 AND s.instance = ?3 AND m.ts >= ?4
             GROUP BY hour ORDER BY AVG(m.value) DESC LIMIT 1",
        )?;
        let mut rows = statement.query_map(params![device_id, metric, instance, since], |row| {
            Ok((row.get::<_, i64>(0)?.max(0) as u32, row.get::<_, f64>(1)?))
        })?;
        rows.next().transpose()
    })
}

/// Every `(device, metric)` monitoring records, in one query. A scan asks
/// "is this measured here" once per device and metric, which is thousands of
/// round trips on a campus for a question one `DISTINCT` answers.
pub fn present(store: &Store) -> AppResult<std::collections::HashSet<(String, String)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached("SELECT DISTINCT device_id, metric FROM series")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(samples: &[(&str, &str, i64, f64)]) -> Store {
        let store = Store::in_memory().expect("store");
        for (metric, instance, ts, value) in samples {
            let series = store.series_id("dev1", metric, instance).expect("series");
            store
                .samples_insert(&[(series, *ts, *value)])
                .expect("insert");
        }
        store
    }

    #[test]
    fn summarises_each_instance_against_a_threshold() {
        let store = store_with(&[
            ("if.util_pct", "GigabitEthernet0/1", 1_000, 90.0),
            ("if.util_pct", "GigabitEthernet0/1", 61_000, 95.0),
            ("if.util_pct", "GigabitEthernet0/1", 121_000, 10.0),
            ("if.util_pct", "GigabitEthernet0/2", 1_000, 5.0),
        ]);
        let mut summary = summarise(&store, "dev1", "if.util_pct", 0, 80.0).unwrap();
        summary.sort_by(|a, b| a.instance.cmp(&b.instance));
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].points, 3);
        assert_eq!(summary[0].over, 2);
        assert_eq!(summary[0].max, 95.0);
        assert_eq!(summary[0].span_minutes(), 2);
        assert_eq!(summary[1].over, 0);
        assert!(
            summarise(&store, "dev1", "cpu.pct", 0, 80.0)
                .unwrap()
                .is_empty()
        );
        let present = present(&store).unwrap();
        assert!(present.contains(&("dev1".to_string(), "if.util_pct".to_string())));
        assert!(!present.contains(&("dev1".to_string(), "cpu.pct".to_string())));
    }

    #[test]
    fn counts_transitions_not_samples() {
        let store = store_with(&[
            ("if.oper", "Gi0/1", 1_000, 1.0),
            ("if.oper", "Gi0/1", 2_000, 0.0),
            ("if.oper", "Gi0/1", 3_000, 1.0),
            ("if.oper", "Gi0/1", 4_000, 1.0),
            ("if.oper", "Gi0/2", 1_000, 1.0),
            ("if.oper", "Gi0/2", 2_000, 1.0),
        ]);
        let mut counted = transitions(&store, "dev1", "if.oper", 0).unwrap();
        counted.sort();
        assert_eq!(counted[0].0, "Gi0/1");
        assert_eq!(counted[0].1, 2, "down then up is two transitions");
        assert_eq!(counted[1].1, 0);
        // The window is honoured.
        let later = transitions(&store, "dev1", "if.oper", 3_000).unwrap();
        assert!(later.iter().all(|(_, changes, _)| *changes == 0));
    }

    #[test]
    fn finds_the_busiest_hour() {
        let hour = 3_600_000;
        let store = store_with(&[
            ("if.util_pct", "Gi0/1", 9 * hour, 90.0),
            ("if.util_pct", "Gi0/1", 9 * hour + 1000, 92.0),
            ("if.util_pct", "Gi0/1", 2 * hour, 10.0),
        ]);
        let busiest = busiest_hour(&store, "dev1", "if.util_pct", "Gi0/1", 0).unwrap();
        assert_eq!(busiest.map(|(hour, _)| hour), Some(9));
        assert_eq!(
            busiest_hour(&store, "dev1", "if.util_pct", "Gi0/9", 0).unwrap(),
            None
        );
    }
}
