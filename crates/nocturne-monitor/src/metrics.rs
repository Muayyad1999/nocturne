//! Reading the time series back: `metrics.query`, `query_many`, `series` and
//! `top`.
//!
//! Gaps stay gaps. The store groups samples into buckets and returns only the
//! buckets that have something in them, so a period when a device was not
//! answering comes back as missing points rather than as zeros, and a graph
//! drawn from them shows a hole.

use std::collections::BTreeMap;

use nocturne_app::store::Point;
use nocturne_app::{App, AppError, AppResult, Router, inventory, util};

use crate::state::MonitorState;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// Mirrors the store's own choice of resolution (`store.rs`): raw samples are
/// kept for two days, five-minute roll-ups for thirty, hourly ones beyond.
const RAW_RETENTION_MS: i64 = 48 * 3_600_000;
const FINE_RETENTION_MS: i64 = 30 * 86_400_000;
const FINE_RES_S: i64 = 300;
const COARSE_RES_S: i64 = 3_600;
const DEFAULT_POINTS: i64 = 500;

/// Does `text` match a glob of `*` and `?`?
///
/// The text is usually a device's: an interface, a sensor, a QoS class named
/// by whoever configured it (on RouterOS a dynamic queue is named after a
/// subscriber's login), and every threshold rule matches its instance glob
/// against every instance at every poll. So this is a loop with one place to
/// go back to, the last `*`: no recursion for a long name to run the thread
/// out of stack with (one call per byte aborted the process on a 30,000-byte
/// name), and at worst the text's length times the pattern's, where trying
/// every split of every star took minutes for a few stars over a few
/// thousand bytes.
pub fn glob(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    // The pattern just past the last `*` seen, and how much of the text that
    // star has taken so far.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p).copied() {
            Some(b'*') => {
                star = Some((p + 1, t));
                p += 1;
            }
            Some(b'?') => {
                p += 1;
                t += 1;
            }
            Some(byte) if byte.eq_ignore_ascii_case(&text[t]) => {
                p += 1;
                t += 1;
            }
            // A mismatch: the last star takes one more byte and the rest of
            // the pattern is tried again from there. Giving an earlier star
            // more instead can never match where this does not.
            _ => match star {
                Some((after, taken)) => {
                    star = Some((after, taken + 1));
                    p = after;
                    t = taken + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|byte| *byte == b'*')
}

/// The latest value of each of a device's series, newest sample only.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Latest {
    pub metric: String,
    pub instance: String,
    pub at: i64,
    pub value: f64,
}

pub fn latest_of_device(app: &App, device_id: &str) -> AppResult<Vec<Latest>> {
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT s.metric, s.instance, m.ts, m.value
             FROM series s JOIN samples m ON m.series_id = s.id
             WHERE s.device_id = ?1
               AND m.ts = (SELECT MAX(ts) FROM samples x WHERE x.series_id = s.id)
             ORDER BY s.metric, s.instance",
        )?;
        statement
            .query_map(params![device_id], |row| {
                Ok(Latest {
                    metric: row.get(0)?,
                    instance: row.get(1)?,
                    at: row.get(2)?,
                    value: row.get(3)?,
                })
            })?
            .collect()
    })
}

/// The latest values of one device's series whose metric matches a glob.
pub fn latest_matching(app: &App, device_id: &str, metric: &str) -> AppResult<Vec<Latest>> {
    Ok(latest_of_device(app, device_id)?
        .into_iter()
        .filter(|latest| glob(metric, &latest.metric))
        .collect())
}

/// One point, as the UI wants it: `[timestamp, value]`.
pub type Pair = (i64, f64);

#[derive(Debug, Serialize)]
pub struct Series {
    pub device_id: String,
    pub metric: String,
    pub instance: String,
    pub points: Vec<Pair>,
    pub step_s: i64,
    /// `raw`, `5m` or `1h`: which resolution the points came from.
    pub rollup: &'static str,
}

/// Which resolution a range will be answered from, and how wide its buckets
/// are. Kept in step with the store's own rule.
fn resolution(since: i64, until: i64, max_points: i64, now: i64) -> (&'static str, i64) {
    let span = (until - since).max(1);
    let wanted_ms = (span / max_points).max(1);
    if since >= now - RAW_RETENTION_MS && wanted_ms < FINE_RES_S * 1000 {
        return ("raw", (wanted_ms / 1000).max(1));
    }
    let res = if since >= now - FINE_RETENTION_MS && wanted_ms < COARSE_RES_S * 1000 {
        FINE_RES_S
    } else {
        COARSE_RES_S
    };
    let width = (wanted_ms / (res * 1000)).max(1) * res;
    (if res == FINE_RES_S { "5m" } else { "1h" }, width)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QueryParams {
    pub device_id: String,
    pub metric: String,
    pub instance: Option<String>,
    pub since: i64,
    pub until: Option<i64>,
    /// Wanted seconds between points; the answer says what was used.
    pub step_s: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SeriesRef {
    pub device_id: String,
    pub metric: String,
    #[serde(default)]
    pub instance: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct QueryManyParams {
    pub series: Vec<SeriesRef>,
    pub since: i64,
    pub until: Option<i64>,
    pub step_s: Option<i64>,
}

fn query_one(
    app: &App,
    device_id: &str,
    metric: &str,
    instance: &str,
    since: i64,
    until: i64,
    step_s: Option<i64>,
) -> AppResult<Series> {
    if metric.trim().is_empty() {
        return Err(AppError::invalid("name the metric to query"));
    }
    let span = (until - since).max(1);
    let max_points = match step_s {
        Some(step) if step > 0 => (span / (step * 1000)).clamp(2, 5_000),
        _ => DEFAULT_POINTS,
    };
    let (rollup, step) = resolution(since, until, max_points, app.state::<MonitorState>().now());
    let store = app.store();
    let series_id = store.series_id(device_id, metric, instance)?;
    let points: Vec<Point> = store.samples_query(series_id, since, until, max_points as usize)?;
    Ok(Series {
        device_id: device_id.to_string(),
        metric: metric.to_string(),
        instance: instance.to_string(),
        points: points
            .into_iter()
            .map(|point| (point.ts, point.avg))
            .collect(),
        step_s: step,
        rollup,
    })
}

fn window(app: &App, since: i64, until: Option<i64>) -> (i64, i64) {
    let until = until.unwrap_or_else(|| app.state::<MonitorState>().now().max(since + 1));
    (since, until.max(since + 1))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TopParams {
    pub metric: String,
    pub limit: Option<u32>,
    /// Free text matched against the device's name, site, role and tags.
    pub filter: Option<String>,
    /// Ignore values older than this many seconds (default 15 minutes).
    pub within_s: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct TopEntry {
    pub device_id: String,
    pub device: String,
    pub instance: String,
    pub value: f64,
    pub at: i64,
    pub lab: bool,
}

pub fn top(app: &App, params: &TopParams) -> AppResult<Vec<TopEntry>> {
    if params.metric.trim().is_empty() {
        return Err(AppError::invalid("name the metric to rank by"));
    }
    let limit = params.limit.unwrap_or(10).clamp(1, 500) as i64;
    let within = params.within_s.unwrap_or(900).clamp(1, 86_400);
    let since = app.state::<MonitorState>().now() - within * 1000;
    // Over-fetch so that the device filter still leaves a full page.
    let fetch = if params.filter.is_some() {
        (limit * 20).min(5_000)
    } else {
        limit
    };
    let rows: Vec<(String, String, i64, f64)> = app.store().with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT s.device_id, s.instance, m.ts, m.value
             FROM series s JOIN samples m ON m.series_id = s.id
             WHERE s.metric = ?1 AND m.ts >= ?2
               AND m.ts = (SELECT MAX(ts) FROM samples x WHERE x.series_id = s.id)
             ORDER BY m.value DESC LIMIT ?3",
        )?;
        statement
            .query_map(params![params.metric.trim(), since, fetch], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect()
    })?;

    let devices: BTreeMap<String, nocturne_app::model::Device> =
        inventory::list(app, &inventory::DeviceFilter::default())?
            .into_iter()
            .map(|row| (row.device.id.clone(), row.device))
            .collect();
    let mut entries = Vec::new();
    for (device_id, instance, at, value) in rows {
        let Some(device) = devices.get(&device_id) else {
            continue;
        };
        if let Some(filter) = &params.filter {
            let haystack = format!(
                "{} {} {} {} {}",
                device.name,
                device.address,
                device.site,
                device.role,
                device.tags.join(" ")
            );
            if !util::matches_query(&haystack, filter) {
                continue;
            }
        }
        entries.push(TopEntry {
            device_id,
            device: device.name.clone(),
            instance,
            value,
            at,
            lab: device.lab.is_some(),
        });
        if entries.len() as i64 >= limit {
            break;
        }
    }
    Ok(entries)
}

#[derive(Debug, Deserialize)]
pub struct DeviceParams {
    pub device_id: String,
}

pub fn register(router: &mut Router) {
    router.add(
        "metrics.query",
        |app: App, params: QueryParams| async move {
            inventory::get(&app, &params.device_id)?;
            let (since, until) = window(&app, params.since, params.until);
            query_one(
                &app,
                &params.device_id,
                &params.metric,
                params.instance.as_deref().unwrap_or(""),
                since,
                until,
                params.step_s,
            )
        },
    );
    router.add(
        "metrics.query_many",
        |app: App, params: QueryManyParams| async move {
            if params.series.len() > 200 {
                return Err(AppError::invalid("ask for at most 200 series at a time"));
            }
            let (since, until) = window(&app, params.since, params.until);
            let mut answers = Vec::with_capacity(params.series.len());
            for wanted in &params.series {
                answers.push(query_one(
                    &app,
                    &wanted.device_id,
                    &wanted.metric,
                    wanted.instance.as_deref().unwrap_or(""),
                    since,
                    until,
                    params.step_s,
                )?);
            }
            Ok(answers)
        },
    );
    router.add(
        "metrics.series",
        |app: App, params: DeviceParams| async move {
            inventory::get(&app, &params.device_id)?;
            let latest = latest_of_device(&app, &params.device_id)?;
            Ok(latest
                .into_iter()
                .map(|entry| {
                    serde_json::json!({
                        "metric": entry.metric,
                        "instance": entry.instance,
                        "latest": [entry.at, entry.value],
                    })
                })
                .collect::<Vec<_>>())
        },
    );
    router.add("metrics.top", |app: App, params: TopParams| async move {
        top(&app, &params)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_stars_question_marks_and_nothing_else() {
        assert!(glob("if.*_err_ps", "if.in_err_ps"));
        assert!(glob("if.*_err_ps", "if.out_err_ps"));
        assert!(!glob("if.*_err_ps", "if.in_disc_ps"));
        assert!(glob("cpu.pct", "cpu.pct"));
        assert!(!glob("cpu.pct", "cpu.pctx"));
        assert!(glob("*", "anything"));
        assert!(glob("Gi0/?", "Gi0/4"));
        assert!(!glob("Gi0/?", "Gi0/44"));
        assert!(glob("GI0/*", "Gi0/1"), "globs ignore case");
        assert!(glob("", "anything"), "an empty pattern is no filter");
    }

    #[test]
    fn a_pathological_glob_still_terminates() {
        assert!(!glob("*a*a*a*a*b", "aaaaaaaaaaaaaaaaaaaac"));
    }

    #[test]
    fn a_long_name_costs_a_glob_neither_its_stack_nor_its_time() {
        // A queue named by a device, a million bytes of it, judged on a
        // thread with a small stack: a glob that recursed once a byte runs
        // out of it, and one that tried every split of every star never
        // finishes the second pattern.
        let long = format!("ether1|out|queue simple/{}", "x".repeat(1_000_000));
        let matched = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                (
                    glob("*", &long),
                    glob("*x*x*x*x*x*x*Q", &long),
                    glob("ether1|*|queue simple/x*x", &long),
                    glob("*?", &long),
                    glob("*xQ*", &long),
                )
            })
            .expect("a thread")
            .join()
            .expect("no overflow");
        assert_eq!(matched, (true, false, true, true, false));
        // And the edges of an empty text.
        assert!(glob("*", ""));
        assert!(glob("**", ""));
        assert!(!glob("?", ""));
        assert!(!glob("a*", ""));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(!glob("a*b*c", "aXbYcZ"));
        assert!(glob("*/VOICE", "Gi0/1|out|WAN/VOICE"));
    }

    #[test]
    fn the_resolution_follows_how_far_back_and_how_fine() {
        let now = 1_700_000_000_000;
        // An hour at 500 points: raw samples, seven seconds apart.
        assert_eq!(resolution(now - 3_600_000, now, 500, now), ("raw", 7));
        // A week at 500 points: five-minute roll-ups.
        let week = now - 7 * 86_400_000;
        assert_eq!(resolution(week, now, 500, now), ("5m", 1_200));
        // A year: hourly ones.
        assert_eq!(resolution(now - 365 * 86_400_000, now, 500, now).0, "1h");
    }
}
