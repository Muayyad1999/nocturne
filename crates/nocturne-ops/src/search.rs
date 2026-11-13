//! One box that searches everything.
//!
//! Devices, notes and incidents are this process's own records and are always
//! searched. Configuration text, addresses, logs and "which switch port is
//! this plugged into" belong to other features: each is asked in parallel
//! with a short budget, and a source that is slow, broken or absent is named
//! in `partial` rather than waited for or quietly dropped.

use std::time::Duration;

use nocturne_app::inventory::{self, DeviceFilter};
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppBuilder, AppError, AppResult};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::incidents;
use crate::model::IncidentHead;
use crate::notes;
use crate::records;
use crate::refs::{ChangeRef, DeviceRef};
use crate::settings;
use crate::sources::{self, Unavailable};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct QueryParams {
    pub q: String,
    /// How many results per kind. Eight is enough to recognise the one you
    /// meant.
    pub limit: Option<usize>,
}

/// Whether the query is an address, in which case the topology feature can
/// say which port it is on.
fn looks_like_an_address(query: &str) -> bool {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(
            r"(?xi)^(
                 \d{1,3}(\.\d{1,3}){3}                      # IPv4
               | [0-9a-f]{1,4}(:[0-9a-f]{0,4}){2,7}         # IPv6
               | ([0-9a-f]{2}[:-]){5}[0-9a-f]{2}            # MAC, colons or dashes
               | ([0-9a-f]{4}\.){2}[0-9a-f]{4}              # MAC, Cisco dotted
               | [0-9a-f]{12}                               # MAC, bare
             )$",
        )
        .expect("a valid pattern")
    });
    pattern.is_match(query.trim())
}

pub async fn query(app: &App, params: &QueryParams) -> AppResult<Value> {
    let text = params.q.trim();
    if text.is_empty() {
        return Err(AppError::invalid("say what to search for"));
    }
    let limit = params.limit.unwrap_or(8).clamp(1, 50);
    let config = settings::get(app)?;
    let budget = Duration::from_millis(config.search_budget_ms.clamp(50, 5_000));
    let words = records::words(text);
    let mut partial: Vec<Unavailable> = Vec::new();

    // ---- this process's own records, which cannot be slow or absent.
    let devices: Vec<DeviceRef> = inventory::list(
        app,
        &DeviceFilter {
            query: Some(text.to_string()),
            ..DeviceFilter::default()
        },
    )?
    .iter()
    .take(limit)
    .map(DeviceRef::from)
    .collect();

    let found_notes = notes::search(app, &words, limit)?;
    let incidents: Vec<IncidentHead> = incidents::list(
        app,
        &incidents::ListParams {
            query: Some(text.to_string()),
            limit: Some(limit),
            ..incidents::ListParams::default()
        },
    )?
    .incidents;

    // ---- the other features, all at once, each with its own budget.
    let locate = async {
        if looks_like_an_address(text) {
            Some(sources::call(app, "locate.host", json!({"query": text}), budget).await)
        } else {
            None
        }
    };
    let (changes, config_matches, addresses, logs, location) = tokio::join!(
        sources::call(app, "changes.list", json!({"limit": 200}), budget),
        sources::call(
            app,
            "config.search",
            json!({"query": text, "limit": limit, "context": 0}),
            budget
        ),
        sources::call(app, "ipam.search", json!({"query": text}), budget),
        sources::call(
            app,
            "logs.query",
            json!({"text": text, "limit": limit}),
            budget
        ),
        locate,
    );

    let mut result = json!({
        "query": text,
        "devices": devices,
        "notes": found_notes,
        "incidents": incidents,
        "generated_at": now_ms(),
    });

    // `changes.list` has no text filter in its contract, so the newest page
    // is filtered here. Anything older than that page is not searched, and
    // `changes_truncated` says so.
    match changes {
        Ok(value) => match sources::change_rows(value) {
            Ok(changes) => {
                let matched: Vec<ChangeRef> = changes
                    .iter()
                    .filter(|change| {
                        nocturne_app::util::matches_query(
                            &format!(
                                "{} {} {}",
                                change.title,
                                change.ticket.clone().unwrap_or_default(),
                                change.id
                            ),
                            text,
                        )
                    })
                    .take(limit)
                    .map(ChangeRef::from)
                    .collect();
                result["changes"] = serde_json::to_value(matched)?;
                result["changes_truncated"] = json!(changes.len() >= 200);
            }
            Err(gap) => {
                result["changes"] = Value::Null;
                partial.push(gap.for_section("changes"));
            }
        },
        Err(gap) => {
            result["changes"] = Value::Null;
            partial.push(gap.for_section("changes"));
        }
    }

    for (name, answer, pick) in [
        ("config", config_matches, "matches"),
        ("addresses", addresses, ""),
        ("logs", logs, "entries"),
    ] {
        match answer {
            Ok(value) => {
                result[name] = if pick.is_empty() {
                    value
                } else {
                    value.get(pick).cloned().unwrap_or(value)
                };
            }
            Err(gap) => {
                result[name] = Value::Null;
                partial.push(gap.for_section(name));
            }
        }
    }

    match location {
        None => {
            // Not an address, so nothing was asked and nothing is missing.
            result["location"] = Value::Null;
        }
        Some(Ok(value)) => result["location"] = value,
        Some(Err(gap)) if gap.is_not_found() => result["location"] = Value::Null,
        Some(Err(gap)) => {
            result["location"] = Value::Null;
            partial.push(gap.for_section("location"));
        }
    }

    result["partial"] = serde_json::to_value(&partial)?;
    Ok(result)
}

pub fn register(builder: &mut AppBuilder) {
    builder
        .router
        .add("search.query", |app: App, params: QueryParams| async move {
            query(&app, &params).await
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_recognised_and_hostnames_are_not() {
        for address in [
            "10.0.0.1",
            "192.168.254.254",
            "2001:db8::1",
            "fe80::",
            "aa:bb:cc:dd:ee:ff",
            "AA-BB-CC-DD-EE-FF",
            "aabb.ccdd.eeff",
            "aabbccddeeff",
            "  10.0.0.1  ",
        ] {
            assert!(looks_like_an_address(address), "{address}");
        }
        for not in [
            "core-sw1",
            "SW1.hq.example.com",
            "vlan 20",
            "how do we fail over the WAN",
            "10.0.0",
            "",
            "aabbccddeef",
        ] {
            assert!(!looks_like_an_address(not), "{not}");
        }
    }
}
