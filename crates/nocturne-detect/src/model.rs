//! What a detection says, and what the catalogue shows of itself.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The alert list's own scale, so a hit and the alert raised for it agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Syslog-style severity for the `event` stream, as the alert list maps it.
    pub fn syslog(self) -> u8 {
        match self {
            Severity::Info => 6,
            Severity::Low => 5,
            Severity::Medium => 4,
            Severity::High => 3,
            Severity::Critical => 2,
        }
    }
}

/// One thing a detection told somebody about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub id: String,
    /// The catalogue id of the detection that fired.
    pub detection: String,
    /// When what it was decided from happened, not when it was judged.
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub severity: Severity,
    pub title: String,
    /// The records it was decided from, by id: log entry, change, audit
    /// entry, configuration version. Never the text of any of them.
    pub evidence: Value,
    /// Absent when `alerts.raise` is not in the build, or refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert_id: Option<String>,
    /// The device is a simulated one.
    #[serde(default)]
    pub lab: bool,
}

/// What a check concluded, before it is recorded and raised.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub detection: &'static str,
    pub at: i64,
    pub device_id: Option<String>,
    pub severity: Severity,
    pub title: String,
    pub evidence: Value,
    /// The last part of the alert's dedup key: what tells two findings of
    /// one detection on one device apart.
    pub key: String,
    /// The sentence under the alert's title: what it was decided from.
    pub detail: String,
}

/// A check that could not be made. Kept so that a detection which did not
/// judge an event says so, instead of looking as if it had found nothing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CheckError {
    pub at: i64,
    pub message: String,
}

/// One row of `detections.list`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub severity: Severity,
    pub decided_from: &'static str,
    pub cannot_see: &'static str,
    pub enabled: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_hit_at: Option<i64>,
    pub hits_24h: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<CheckError>,
}
