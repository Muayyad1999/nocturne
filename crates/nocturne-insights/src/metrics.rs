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

/// As [`summarise`], but each instance is summarised over its latest
/// *unbroken* run only: the samples from the last hole longer than `hole_ms`
/// onwards.
///
/// A failed read writes no sample, so a series with a hole in it says nothing
/// about the time the hole covers. Counting the samples on both sides as one
/// stretch would claim "over the threshold for fifty minutes" on the strength
/// of ten measured ones. The sample that ends a hole is kept: it is a rate
/// over the interval that ended when it was taken, and the run starts there.
pub fn summarise_unbroken(
    store: &Store,
    device_id: &str,
    metric: &str,
    since: i64,
    threshold: f64,
    hole_ms: i64,
) -> AppResult<Vec<Series>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "WITH recent AS (
                 SELECT s.instance AS instance, m.ts AS ts, m.value AS value,
                        m.ts - LAG(m.ts) OVER (PARTITION BY s.id ORDER BY m.ts) AS gap
                 FROM series s JOIN samples m ON m.series_id = s.id
                 WHERE s.device_id = ?1 AND s.metric = ?2 AND m.ts >= ?3
             ),
             resumed AS (
                 SELECT instance, MAX(ts) AS at FROM recent WHERE gap > ?5 GROUP BY instance
             )
             SELECT r.instance, COUNT(*), MIN(r.ts), MAX(r.ts), AVG(r.value), MAX(r.value),
                    SUM(CASE WHEN r.value >= ?4 THEN 1 ELSE 0 END)
             FROM recent r LEFT JOIN resumed h ON h.instance = r.instance
             WHERE h.at IS NULL OR r.ts >= h.at
             GROUP BY r.instance",
        )?;
        let rows = statement.query_map(
            params![device_id, metric, since, threshold, hole_ms],
            |row| {
                Ok(Series {
                    instance: row.get(0)?,
                    points: row.get::<_, i64>(1)?.max(0) as u32,
                    first_at: row.get(2)?,
                    last_at: row.get(3)?,
                    avg: row.get(4)?,
                    max: row.get(5)?,
                    over: row.get::<_, i64>(6)?.max(0) as u32,
                })
            },
        )?;
        rows.collect()
    })
}

/// One series from `since` on, or `None` when it has no sample there — for
/// the figures quoted beside a finding, which have to cover the stretch the
/// finding was decided over and not a different one.
pub fn summarise_one(
    store: &Store,
    device_id: &str,
    metric: &str,
    instance: &str,
    since: i64,
) -> AppResult<Option<Series>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT COUNT(*), MIN(m.ts), MAX(m.ts), AVG(m.value), MAX(m.value)
             FROM series s JOIN samples m ON m.series_id = s.id
             WHERE s.device_id = ?1 AND s.metric = ?2 AND s.instance = ?3 AND m.ts >= ?4",
        )?;
        // An aggregate over no rows is one row of NULLs, not no row.
        statement.query_row(params![device_id, metric, instance, since], |row| {
            let points = row.get::<_, i64>(0)?.max(0) as u32;
            if points == 0 {
                return Ok(None);
            }
            Ok(Some(Series {
                instance: instance.to_string(),
                points,
                first_at: row.get(1)?,
                last_at: row.get(2)?,
                avg: row.get(3)?,
                max: row.get(4)?,
                over: 0,
            }))
        })
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
///
/// Recorded means a series that holds a sample. A series row alone is not
/// one: the core makes the row the first time anybody names the series, and
/// a read through `metrics.query` names it — so asking once for a series
/// nobody records leaves a row behind for as long as the device exists. Taken
/// as a measurement, one such row of `qos.queue_drop_ps` made a device whose
/// classes were dropping into one that counts congestion apart and had none.
pub fn present(store: &Store) -> AppResult<std::collections::HashSet<(String, String)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT DISTINCT s.device_id, s.metric FROM series s
             WHERE EXISTS (SELECT 1 FROM samples m WHERE m.series_id = s.id)",
        )?;
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
    fn a_hole_in_a_series_starts_the_summary_again_after_it() {
        let minute = 60_000;
        let store = store_with(&[
            // Three samples, forty minutes of nothing, three more.
            ("qos.queue_drop_ps", "holed", 0, 90.0),
            ("qos.queue_drop_ps", "holed", 5 * minute, 90.0),
            ("qos.queue_drop_ps", "holed", 10 * minute, 90.0),
            ("qos.queue_drop_ps", "holed", 50 * minute, 20.0),
            ("qos.queue_drop_ps", "holed", 55 * minute, 30.0),
            ("qos.queue_drop_ps", "holed", 60 * minute, 2.0),
            // The same six, evenly spread: no hole.
            ("qos.queue_drop_ps", "whole", 0, 90.0),
            ("qos.queue_drop_ps", "whole", 12 * minute, 90.0),
            ("qos.queue_drop_ps", "whole", 24 * minute, 90.0),
            ("qos.queue_drop_ps", "whole", 36 * minute, 20.0),
            ("qos.queue_drop_ps", "whole", 48 * minute, 30.0),
            ("qos.queue_drop_ps", "whole", 60 * minute, 2.0),
        ]);
        let mut runs =
            summarise_unbroken(&store, "dev1", "qos.queue_drop_ps", 0, 10.0, 15 * minute).unwrap();
        runs.sort_by(|a, b| a.instance.cmp(&b.instance));
        assert_eq!(runs.len(), 2);

        let holed = &runs[0];
        assert_eq!(
            (holed.points, holed.first_at, holed.last_at),
            (3, 50 * minute, 60 * minute),
            "only what follows the hole, starting with the sample that ended it"
        );
        assert_eq!((holed.over, holed.max), (2, 30.0));
        assert_eq!(holed.span_minutes(), 10);

        let whole = &runs[1];
        assert_eq!((whole.points, whole.span_minutes(), whole.over), (6, 60, 5));

        // A gap of exactly the allowed length is not a hole.
        let exact =
            summarise_unbroken(&store, "dev1", "qos.queue_drop_ps", 0, 10.0, 40 * minute).unwrap();
        assert!(exact.iter().all(|series| series.points == 6));
        // The window is honoured before holes are looked for.
        let late = summarise_unbroken(
            &store,
            "dev1",
            "qos.queue_drop_ps",
            55 * minute,
            10.0,
            15 * minute,
        )
        .unwrap();
        assert!(late.iter().all(|series| series.points <= 2));
        assert!(
            summarise_unbroken(&store, "dev1", "cpu.pct", 0, 10.0, minute)
                .unwrap()
                .is_empty()
        );

        // One series from a moment on: the figures quoted beside a run.
        let after = summarise_one(&store, "dev1", "qos.queue_drop_ps", "holed", 50 * minute)
            .unwrap()
            .expect("three samples");
        assert_eq!((after.points, after.max), (3, 30.0));
        assert!((after.avg - 52.0 / 3.0).abs() < 1e-9);
        for (instance, since) in [("holed", 61 * minute), ("absent", 0)] {
            assert_eq!(
                summarise_one(&store, "dev1", "qos.queue_drop_ps", instance, since).unwrap(),
                None,
                "{instance}: nothing measured is not a measurement of nothing"
            );
        }
    }

    #[test]
    fn a_series_that_holds_no_sample_is_not_a_metric_that_is_recorded() {
        // `Store::series_id` makes the row on first use, and the monitor's
        // `metrics.query` uses it to read: asking for a series nobody records
        // leaves its row behind with nothing in it.
        let store = store_with(&[("qos.drop_ps", "ether1|out|subscribers/alice", 1_000, 25.0)]);
        store
            .series_id("dev1", "qos.queue_drop_ps", "ether1|out|subscribers/alice")
            .expect("an empty series");
        let present = present(&store).unwrap();
        assert!(present.contains(&("dev1".to_string(), "qos.drop_ps".to_string())));
        assert!(
            !present.contains(&("dev1".to_string(), "qos.queue_drop_ps".to_string())),
            "a row with no sample in it was read, not recorded"
        );
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
