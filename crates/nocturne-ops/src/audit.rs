//! Reading the audit trail the core already keeps.
//!
//! `App::audit` writes one line per user-visible mutation into the core's log
//! store; the core's own `audit.list` hands those lines back raw. This adds
//! what an operations view needs: a filter by action, and the lines **joined
//! to the things they mention** — the device as the inventory has it now, and
//! the incidents, notes, handovers and reports whose ids appear in the entry's
//! own detail.
//!
//! Nothing is inferred. A reference exists only where the feature that wrote
//! the entry put an id in the detail, and a joined title only where that
//! record still exists.

use nocturne_app::store::{LogEntry, LogQuery};
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppBuilder, AppResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::records::{self, Kind};
use crate::refs::{Devices, Ref};

/// The fields features put ids in, and what kind of record each names.
const REFERENCE_FIELDS: [(&str, &str); 10] = [
    ("incident_id", "incident"),
    ("change_id", "change"),
    ("alert_id", "alert"),
    ("job_id", "job"),
    ("note_id", "note"),
    ("report_id", "report"),
    ("handover_id", "handover"),
    ("version_id", "config_version"),
    ("lab_id", "lab"),
    ("policy_id", "policy"),
];

/// Every record an audit entry's own detail names.
pub fn refs_of(entry: &LogEntry) -> Vec<Ref> {
    let Some(detail) = entry.body.as_ref().and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    for (field, kind) in REFERENCE_FIELDS {
        match detail.get(field) {
            Some(Value::String(id)) if !id.is_empty() => refs.push(Ref::new(kind, id.clone())),
            Some(Value::Number(id)) => refs.push(Ref::new(kind, id.to_string())),
            _ => {}
        }
    }
    refs
}

/// The title of one of this feature's own records, when it still exists.
fn title_of(app: &App, reference: &Ref) -> Option<String> {
    let kind = match reference.kind.as_str() {
        "incident" => Kind::Incident,
        "note" => Kind::Note,
        "handover" => Kind::Handover,
        "report" => Kind::Report,
        _ => return None,
    };
    let head = records::head(app.store(), kind, &reference.id).ok()??;
    let value: Value = serde_json::from_str(&head).ok()?;
    value
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            // A handover's head has a period rather than a title.
            let period = value.get("period")?;
            Some(format!(
                "handover {} → {}",
                period.get("since")?.as_i64()?,
                period.get("until")?.as_i64()?
            ))
        })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SearchParams {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub device_id: Option<String>,
    /// An exact action (`incidents.create`) or a prefix ending in `.`
    /// (`incidents.`), as written by whoever called `audit(...)`.
    pub action: Option<String>,
    /// Free text in the message or the action.
    pub text: Option<String>,
    /// Only entries whose detail names this record.
    pub reference: Option<Ref>,
    pub limit: Option<u32>,
    /// Page backwards: only entries older than this one.
    pub before_id: Option<i64>,
}

/// How many entries are read from the store per page while a filter that the
/// store cannot express is applied in this process.
const PAGE: u32 = 500;

pub fn search(app: &App, params: &SearchParams) -> AppResult<Value> {
    let limit = params.limit.unwrap_or(100).clamp(1, 1_000);
    let devices = Devices::load(app)?;
    let wanted_action = params.action.as_deref().filter(|action| !action.is_empty());
    let post_filtered = wanted_action.is_some() || params.reference.is_some();

    let keep = |entry: &LogEntry| {
        let action_ok = wanted_action.is_none_or(|action| match action.strip_suffix('.') {
            Some(prefix) => entry.source.starts_with(prefix),
            None => entry.source == action,
        });
        let reference_ok = params
            .reference
            .as_ref()
            .is_none_or(|wanted| refs_of(entry).contains(wanted));
        action_ok && reference_ok
    };
    // The filters the store understands are a single query. An action or a
    // reference filter is applied here instead, a page at a time, so that a
    // narrow filter over a long trail still fills a page.
    let read_page = |before_id: Option<i64>, want: u32| {
        app.store().log_query(&LogQuery {
            stream: Some("audit".into()),
            device_id: params.device_id.clone(),
            since: params.since,
            until: params.until,
            text: params.text.clone(),
            before_id,
            limit: Some(want),
            ..LogQuery::default()
        })
    };

    let mut found: Vec<LogEntry> = Vec::new();
    let mut has_more = false;
    if post_filtered {
        let mut before_id = params.before_id;
        for _ in 0..20 {
            let page = read_page(before_id, PAGE)?;
            let exhausted = (page.len() as u32) < PAGE;
            before_id = page.last().map(|entry| entry.id);
            for entry in page.into_iter().filter(|entry| keep(entry)) {
                if found.len() as u32 == limit {
                    has_more = true;
                    break;
                }
                found.push(entry);
            }
            if has_more || exhausted {
                break;
            }
        }
    } else {
        let mut page = read_page(params.before_id, limit + 1)?;
        has_more = page.len() as u32 > limit;
        page.truncate(limit as usize);
        found = page;
    }

    let entries: Vec<Value> = found
        .iter()
        .map(|entry| {
            let refs: Vec<Value> = refs_of(entry)
                .into_iter()
                .map(|reference| {
                    let title = title_of(app, &reference);
                    json!({
                        "type": reference.kind,
                        "id": reference.id,
                        "title": title,
                        // `false` means the record is gone, or belongs to a
                        // feature whose records this view cannot read.
                        "resolved": title.is_some(),
                    })
                })
                .collect();
            json!({
                "id": entry.id,
                "ts": entry.ts,
                "action": entry.source,
                "message": entry.message,
                "device_id": entry.device_id,
                "device": entry.device_id.as_deref().map(|id| devices.reference(id)),
                "refs": refs,
                "detail": entry.body,
            })
        })
        .collect();
    let next_before_id = found.last().map(|entry| entry.id).filter(|_| has_more);
    Ok(json!({
        "entries": entries,
        "has_more": has_more,
        "next_before_id": next_before_id,
        "read_at": now_ms(),
    }))
}

/// Which actions appear in the trail and how often, for a filter menu.
pub fn actions(app: &App, since: Option<i64>) -> AppResult<Value> {
    let entries = app.store().log_query(&LogQuery {
        stream: Some("audit".into()),
        since,
        limit: Some(5_000),
        ..LogQuery::default()
    })?;
    let mut counts: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for entry in &entries {
        *counts.entry(entry.source.clone()).or_default() += 1;
    }
    Ok(json!({
        "actions": counts
            .into_iter()
            .map(|(action, count)| json!({"action": action, "count": count}))
            .collect::<Vec<_>>(),
        // The trail may be longer than what was counted.
        "truncated": entries.len() >= 5_000,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ActionsParams {
    pub since: Option<i64>,
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add(
        "audit.search",
        |app: App, params: SearchParams| async move { search(&app, &params) },
    );
    router.add(
        "audit.actions",
        |app: App, params: ActionsParams| async move { actions(&app, params.since) },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn entry(source: &str, detail: Value) -> LogEntry {
        LogEntry {
            id: 1,
            ts: 1_000,
            stream: "audit".into(),
            device_id: Some("d1".into()),
            severity: Some(6),
            source: source.into(),
            message: "did a thing".into(),
            body: if detail.is_null() { None } else { Some(detail) },
        }
    }

    #[test]
    fn references_come_only_from_ids_the_writer_recorded() {
        assert_eq!(
            refs_of(&entry(
                "incidents.link",
                json!({"incident_id": "i1", "change_id": "c2", "version_id": 7, "linked": ["x"]})
            )),
            [
                Ref::new("incident", "i1"),
                Ref::new("change", "c2"),
                Ref::new("config_version", "7"),
            ]
        );
        assert!(refs_of(&entry("vault.lock", Value::Null)).is_empty());
        // Something that is not an id is not turned into one.
        assert!(refs_of(&entry("x", json!({"incident_id": ""}))).is_empty());
        assert!(refs_of(&entry("x", json!({"incident_id": ["i1"]}))).is_empty());
        assert!(refs_of(&entry("x", json!(["not an object"]))).is_empty());
    }
}
