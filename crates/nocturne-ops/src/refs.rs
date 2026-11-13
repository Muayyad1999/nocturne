//! How results point at devices and at records of other features.
//!
//! Every reference carries the record's id and enough to show it without a
//! second request. Saved documents (handovers) keep these as they were when
//! the document was made, so they stay readable after a device is deleted.

use std::collections::HashMap;

use nocturne_app::inventory::{self, DeviceFilter};
use nocturne_app::jobs::{Job, JobState};
use nocturne_app::model::DeviceRow;
use nocturne_app::{App, AppResult};
use serde::{Deserialize, Serialize};

use crate::sources::{Alert, ChangeSummary, Insight, Severity};

pub(crate) fn is_false(flag: &bool) -> bool {
    !*flag
}

/// A span of time, `since` inclusive and `until` exclusive, in ms since the epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period {
    pub since: i64,
    pub until: i64,
}

impl Period {
    pub fn contains(&self, at: i64) -> bool {
        self.since <= at && at < self.until
    }
}

/// A pointer to a record: `{type: "alert", id: "…"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

impl Ref {
    pub fn new(kind: &str, id: impl Into<String>) -> Ref {
        Ref {
            kind: kind.to_string(),
            id: id.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRef {
    pub id: String,
    /// The device's name; its id when the device no longer exists.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub site: String,
    /// A simulated device in a lab.
    #[serde(default)]
    pub lab: bool,
    /// The device has been deleted from the inventory since.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deleted: bool,
}

impl From<&DeviceRow> for DeviceRef {
    fn from(row: &DeviceRow) -> DeviceRef {
        DeviceRef {
            id: row.device.id.clone(),
            name: row.device.name.clone(),
            site: row.device.site.clone(),
            lab: row.device.lab.is_some(),
            deleted: false,
        }
    }
}

/// The inventory as one request sees it.
pub struct Devices {
    rows: Vec<DeviceRow>,
    by_id: HashMap<String, usize>,
}

impl Devices {
    pub fn load(app: &App) -> AppResult<Devices> {
        Devices::matching(app, &DeviceFilter::default())
    }

    pub fn matching(app: &App, filter: &DeviceFilter) -> AppResult<Devices> {
        let rows = inventory::list(app, filter)?;
        let by_id = rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.device.id.clone(), index))
            .collect();
        Ok(Devices { rows, by_id })
    }

    /// In the inventory's own (natural name) order.
    pub fn rows(&self) -> &[DeviceRow] {
        &self.rows
    }

    pub fn get(&self, id: &str) -> Option<&DeviceRow> {
        self.by_id.get(id).map(|index| &self.rows[*index])
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn reference(&self, id: &str) -> DeviceRef {
        match self.get(id) {
            Some(row) => DeviceRef::from(row),
            None => DeviceRef {
                id: id.to_string(),
                name: id.to_string(),
                site: String::new(),
                lab: false,
                deleted: true,
            },
        }
    }

    /// The device's name, or its id when it is gone.
    pub fn name(&self, id: &str) -> String {
        self.get(id)
            .map_or_else(|| id.to_string(), |row| row.device.name.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRef {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    /// `firing`, `acknowledged` or `resolved`.
    pub state: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// The device's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The name of the rule that raised it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default)]
    pub lab: bool,
}

impl AlertRef {
    pub fn new(alert: &Alert, devices: &Devices) -> AlertRef {
        let row = alert.device_id.as_deref().and_then(|id| devices.get(id));
        let state = if !alert.is_open() {
            "resolved"
        } else if alert.acked_at.is_some() || alert.state.as_deref() == Some("acknowledged") {
            "acknowledged"
        } else {
            "firing"
        };
        AlertRef {
            id: alert.id.clone(),
            severity: alert.severity,
            title: alert.title.clone(),
            state: state.to_string(),
            started_at: alert.started_at,
            acked_at: alert.acked_at,
            resolved_at: alert.resolved_at,
            device_id: alert.device_id.clone(),
            device: row
                .map(|row| row.device.name.clone())
                .or_else(|| alert.device.clone()),
            rule: alert.rule_name.clone(),
            lab: alert
                .lab
                .unwrap_or_else(|| row.is_some_and(|row| row.device.lab.is_some())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeRef {
    pub id: String,
    pub title: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Absent when the change list does not say which devices a change targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_ids: Option<Vec<String>>,
}

impl From<&ChangeSummary> for ChangeRef {
    fn from(change: &ChangeSummary) -> ChangeRef {
        ChangeRef {
            id: change.id.clone(),
            title: change.title.clone(),
            state: change.state.clone(),
            origin: change.origin.clone(),
            ticket: change.ticket.clone().filter(|ticket| !ticket.is_empty()),
            created_by: change.created_by.clone(),
            created_at: change.created_at,
            updated_at: change.updated_at,
            device_ids: change.devices(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InsightRef {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// What on the device it is about (`Gi0/3`, `VLAN 20`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<i64>,
    #[serde(default)]
    pub lab: bool,
}

impl InsightRef {
    pub fn new(insight: &Insight, devices: &Devices) -> InsightRef {
        let row = insight.device_id.as_deref().and_then(|id| devices.get(id));
        InsightRef {
            id: insight.id.clone(),
            title: insight.title.clone(),
            severity: insight.severity,
            device_id: insight.device_id.clone(),
            device: row
                .map(|row| row.device.name.clone())
                .or_else(|| insight.device.clone()),
            subject: insight.subject.clone(),
            check: insight.check.clone(),
            first_seen: insight.first_seen,
            lab: insight
                .lab
                .unwrap_or_else(|| row.is_some_and(|row| row.device.lab.is_some())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobRef {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub state: JobState,
    /// `user`, `schedule`, `assistant` or `system`.
    pub origin: String,
    pub done: u64,
    /// `0` while the amount of work is unknown.
    pub total: u64,
    pub message: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub device_ids: Vec<String>,
}

impl From<&Job> for JobRef {
    fn from(job: &Job) -> JobRef {
        JobRef {
            id: job.id.clone(),
            kind: job.kind.clone(),
            title: job.title.clone(),
            state: job.state,
            origin: job.origin.clone(),
            done: job.done,
            total: job.total,
            message: job.message.clone(),
            started_at: job.started_at,
            finished_at: job.finished_at,
            error: job.error.clone(),
            device_ids: job.device_ids.clone(),
        }
    }
}
