//! Recording a hit, and reading hits back.
//!
//! A hit lives in the `event` stream (`source: detect.<id>`, the [`Hit`] as
//! the body) — the same place the log rules keep their signals and the alert
//! list its history, so `logs.query` and `logs.around` show it with no table
//! of its own. It is raised through `alerts.raise` and nowhere else: there is
//! no second inbox.

use std::collections::HashMap;

use serde_json::{Value, json};

use nocturne_app::store::{LogEntry, LogQuery};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppResult, inventory};

use crate::catalogue;
use crate::model::{Finding, Hit};
use crate::sources::{self, Gap};

pub const STREAM: &str = "event";
/// When each detection last fired, kept apart from the stream so the list
/// does not have to search a month of events for something that may never
/// have happened — and still knows after the stream has aged the hit out.
const LAST_HIT: &str = "detect.last_hit";
/// How many entries are read from the store at a time while a filter the
/// store cannot express (an exact source) is applied here.
const PAGE: u32 = 500;
/// The most hits `hits_24h` counts. A day with more than this is a flood,
/// and the number is then a floor.
const COUNT_CAP: usize = 100_000;

pub fn source(detection: &str) -> String {
    format!("detect.{detection}")
}

/// `detect:<id>:<device or "-">:<key>`.
pub fn dedup_key(finding: &Finding) -> String {
    format!(
        "detect:{}:{}:{}",
        finding.detection,
        finding.device_id.as_deref().unwrap_or("-"),
        finding.key
    )
}

/// Raise, record and announce a finding.
pub async fn record(app: &App, finding: Finding) -> Hit {
    // A device that has left the inventory since is still named by the hit
    // (it is what the evidence says), but the alert list refuses an alert
    // for a device it cannot find, so the alert goes out without one.
    let device = finding
        .device_id
        .as_deref()
        .and_then(|id| inventory::get(app, id).ok());
    let alert_id = raise(app, &finding, device.is_some()).await;
    let hit = Hit {
        id: new_id(),
        detection: finding.detection.to_string(),
        at: finding.at,
        device_id: finding.device_id.clone(),
        severity: finding.severity,
        title: finding.title.clone(),
        evidence: finding.evidence.clone(),
        alert_id,
        lab: device.is_some_and(|device| device.lab.is_some()),
    };
    let entry = LogEntry {
        id: 0,
        ts: hit.at,
        stream: STREAM.into(),
        device_id: hit.device_id.clone(),
        severity: Some(hit.severity.syslog()),
        source: source(&hit.detection),
        message: hit.title.clone(),
        body: serde_json::to_value(&hit).ok(),
    };
    if let Err(err) = app.store().log_append(&entry) {
        tracing::error!(%err, detection = hit.detection, "a detection hit could not be stored");
    }
    remember_last(app, &hit);
    app.publish("detections.hit", &json!({ "hit": hit }));
    hit
}

/// Hand the finding to the alert list, if this build has one.
///
/// Which channels hear of it is the alert list's decision, including for a
/// lab device: no `notify` is passed, so its own rule ("an alert for a lab
/// device reaches no channel unless `notify_for_lab` is on") applies
/// instead of being overridden from here.
async fn raise(app: &App, finding: &Finding, device_known: bool) -> Option<String> {
    let name = catalogue::find(finding.detection).map_or(finding.detection, |entry| entry.name);
    let params = json!({
        "rule": format!("Detection: {name}"),
        "device_id": finding.device_id.as_deref().filter(|_| device_known),
        "severity": finding.severity,
        "title": finding.title,
        "detail": finding.detail,
        "dedup_key": dedup_key(finding),
    });
    match sources::call(app, sources::ALERTS_RAISE, params).await {
        Ok(alert) => match alert.get("id").and_then(Value::as_str) {
            Some(id) => Some(id.to_string()),
            None => {
                tracing::warn!(
                    detection = finding.detection,
                    "`alerts.raise` answered without an alert id"
                );
                None
            }
        },
        // No alert list in this build: the hit is kept all the same.
        Err(Gap::Absent(_)) => None,
        Err(gap) => {
            tracing::warn!(
                detection = finding.detection,
                reason = gap.sentence(),
                "a detection hit could not be raised as an alert"
            );
            None
        }
    }
}

fn remember_last(app: &App, hit: &Hit) {
    let mut last = last_hits(app);
    let newest = last.entry(hit.detection.clone()).or_insert(hit.at);
    *newest = (*newest).max(hit.at);
    if let Err(err) = app.store().kv_set(LAST_HIT, &last) {
        tracing::error!(%err, "the time of the last detection hit could not be stored");
    }
}

/// Detection id → when it last fired.
pub fn last_hits(app: &App) -> HashMap<String, i64> {
    app.store()
        .kv_get::<HashMap<String, i64>>(LAST_HIT)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn parse(entry: LogEntry) -> Option<Hit> {
    let body = entry.body?;
    match serde_json::from_value::<Hit>(body) {
        Ok(hit) => Some(hit),
        Err(err) => {
            tracing::debug!(%err, entry = entry.id, "an event filed under a detection is not a hit");
            None
        }
    }
}

/// What to read back.
#[derive(Clone, Debug, Default)]
pub struct Wanted<'a> {
    pub detection: Option<&'a str>,
    pub device_id: Option<&'a str>,
    pub since: Option<i64>,
    pub limit: usize,
}

/// Hits, newest first.
pub fn recent(app: &App, wanted: &Wanted<'_>) -> AppResult<Vec<Hit>> {
    let exact = wanted.detection.map(source);
    // The store narrows by substring; the exact source is checked here, a
    // page at a time, so a narrow filter over a long stream still fills up.
    let needle = exact.clone().unwrap_or_else(|| source(""));
    let mut hits: Vec<Hit> = Vec::new();
    let mut before_id: Option<i64> = None;
    while hits.len() < wanted.limit {
        let page = app.store().log_query(&LogQuery {
            stream: Some(STREAM.into()),
            device_id: wanted.device_id.map(str::to_string),
            since: wanted.since,
            text: Some(needle.clone()),
            before_id,
            limit: Some(PAGE),
            ..LogQuery::default()
        })?;
        let exhausted = (page.len() as u32) < PAGE;
        before_id = page.last().map(|entry| entry.id);
        for entry in page {
            let ours = match &exact {
                Some(exact) => &entry.source == exact,
                None => entry.source.starts_with(&needle),
            };
            if !ours {
                continue;
            }
            if let Some(hit) = parse(entry) {
                hits.push(hit);
                if hits.len() == wanted.limit {
                    break;
                }
            }
        }
        if exhausted {
            break;
        }
    }
    Ok(hits)
}

/// Detection id → how many hits since `since`.
pub fn counts_since(app: &App, since: i64) -> AppResult<HashMap<String, u64>> {
    let prefix = source("");
    let mut counts: HashMap<String, u64> = HashMap::new();
    let mut before_id: Option<i64> = None;
    let mut read = 0;
    while read < COUNT_CAP {
        let page = app.store().log_query(&LogQuery {
            stream: Some(STREAM.into()),
            since: Some(since),
            text: Some(prefix.clone()),
            before_id,
            limit: Some(PAGE),
            ..LogQuery::default()
        })?;
        let exhausted = (page.len() as u32) < PAGE;
        before_id = page.last().map(|entry| entry.id);
        read += page.len();
        for entry in &page {
            if let Some(detection) = entry.source.strip_prefix(&prefix) {
                *counts.entry(detection.to_string()).or_default() += 1;
            }
        }
        if exhausted {
            break;
        }
    }
    Ok(counts)
}

/// Where the `hits_24h` of the list starts counting.
pub fn day_ago() -> i64 {
    now_ms() - 24 * 60 * 60_000
}
