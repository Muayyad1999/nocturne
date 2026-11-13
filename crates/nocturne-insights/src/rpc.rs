//! The RPC surface: everything a person can do with a finding.

use nocturne_app::{App, AppError, Router, inventory, util};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::catalogue;
use crate::db;
use crate::model::{Category, Insight, Severity, State};
use crate::scan;
use crate::settings;

#[derive(Default, Deserialize)]
#[serde(default)]
struct ScanParams {
    device_ids: Vec<String>,
    /// Convenience for a single device.
    id: Option<String>,
    /// Empty = every enabled check.
    checks: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ListParams {
    /// `open` (the default), `dismissed`, `resolved`, or `any`.
    state: Option<String>,
    /// Only findings at least this severe.
    severity: Option<Severity>,
    category: Option<Category>,
    check: Option<String>,
    device_id: Option<String>,
    query: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
    /// Overrides the `include_lab` setting for one list.
    include_lab: Option<bool>,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct DismissParams {
    id: String,
    /// `None` = for good.
    days: Option<u32>,
    note: Option<String>,
}

#[derive(Serialize)]
struct ListResult {
    insights: Vec<Insight>,
    counts: Counts,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct Counts {
    by_severity: std::collections::BTreeMap<String, u64>,
    by_category: std::collections::BTreeMap<String, u64>,
}

pub fn register(router: &mut Router) {
    router.add("insights.scan", |app: App, params: ScanParams| async move {
        let mut device_ids = Vec::new();
        for reference in params.device_ids.iter().chain(params.id.iter()) {
            let device = inventory::find(&app, reference)?;
            if !device_ids.contains(&device.id) {
                device_ids.push(device.id);
            }
        }
        for check in &params.checks {
            if catalogue::get(check).is_none() {
                return Err(AppError::invalid(format!(
                    "there is no check called `{check}`; `insights.checks` lists them"
                )));
            }
        }
        scan::start_job(&app, device_ids, params.checks)
    });

    router.add("insights.list", |app: App, params: ListParams| async move {
        let settings = settings::load(&app)?;
        let state = match params.state.as_deref() {
            None => Some("open".to_string()),
            Some("any") => None,
            Some(state) => Some(
                State::parse(state)
                    .ok_or_else(|| {
                        AppError::invalid(format!(
                            "`{state}` is not a state (open, dismissed, resolved, any)"
                        ))
                    })?
                    .as_str()
                    .to_string(),
            ),
        };
        let device_id = match params.device_id.as_deref() {
            Some(reference) => Some(inventory::find(&app, reference)?.id),
            None => None,
        };
        let filter = db::Filter {
            state,
            severity: params.severity,
            category: params.category,
            check: params.check,
            device_id,
            query: params.query,
            include_lab: params.include_lab.unwrap_or(settings.include_lab),
        };
        let limit = params.limit.unwrap_or(50).clamp(1, 500) as usize;
        let (insights, next_cursor) =
            db::page(app.store(), &filter, params.cursor.as_deref(), limit)?;
        let (by_severity, by_category) = db::counts(app.store(), &filter)?;
        Ok(ListResult {
            insights,
            counts: Counts {
                by_severity,
                by_category,
            },
            next_cursor,
        })
    });

    router.add("insights.get", |app: App, params: IdParams| async move {
        db::get(app.store(), &params.id)
    });

    router.add(
        "insights.checks",
        |app: App, _: nocturne_app::NoParams| async move {
            let settings = settings::load(&app)?;
            Ok(catalogue::info(&settings))
        },
    );

    router.add(
        "insights.coverage",
        |app: App, _: nocturne_app::NoParams| async move { scan::coverage(&app) },
    );

    router.add(
        "insights.dismiss",
        |app: App, params: DismissParams| async move {
            let mut insight = db::get(app.store(), &params.id)?;
            let now = util::now_ms();
            insight.state = State::Dismissed;
            insight.dismissed_at = Some(now);
            insight.dismissed_until = params.days.map(|days| now + i64::from(days) * 86_400_000);
            insight.dismiss_note = params.note.clone().filter(|note| !note.trim().is_empty());
            insight.resolved_at = None;
            db::save(app.store(), &insight)?;
            app.audit(
                "insights.dismiss",
                Some(&insight.device_id),
                format!(
                    "Dismissed `{}` on {} {}",
                    insight.check, insight.device, insight.subject
                ),
                json!({
                    "insight_id": insight.id,
                    "until": insight.dismissed_until,
                    "note": insight.dismiss_note,
                }),
            );
            app.publish(
                "insights.changed",
                &json!({"device_id": insight.device_id, "opened": 0, "resolved": 0}),
            );
            Ok(insight)
        },
    );

    router.add("insights.reopen", |app: App, params: IdParams| async move {
        let mut insight = db::get(app.store(), &params.id)?;
        insight.state = State::Open;
        insight.dismissed_until = None;
        insight.dismissed_at = None;
        insight.dismiss_note = None;
        insight.resolved_at = None;
        db::save(app.store(), &insight)?;
        app.audit(
            "insights.reopen",
            Some(&insight.device_id),
            format!(
                "Reopened `{}` on {} {}",
                insight.check, insight.device, insight.subject
            ),
            json!({"insight_id": insight.id}),
        );
        app.publish(
            "insights.changed",
            &json!({"device_id": insight.device_id, "opened": 1, "resolved": 0}),
        );
        Ok(insight)
    });

    router.add("insights.fix", |app: App, params: IdParams| async move {
        let insight = db::get(app.store(), &params.id)?;
        let lines = insight
            .suggestion
            .as_ref()
            .and_then(|suggestion| suggestion.config_lines.clone())
            .filter(|lines| !lines.is_empty())
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "`{}` has no configuration fix: what to do about it is in the suggestion and \
                     the evidence",
                    insight.check
                ))
                .with_details(json!({"insight_id": insight.id, "check": insight.check}))
            })?;
        if !app.methods().contains(&"changes.create") {
            return Err(AppError::unavailable(
                "the change pipeline is not installed, so a fix cannot be prepared",
            ));
        }
        // The device must still exist; the change pipeline would say so too,
        // but the error is clearer from here.
        let device = inventory::get(&app, &insight.device_id)?;
        let change = app
            .rpc(
                "changes.create",
                json!({
                    "title": format!("{} on {}", insight.title, device.name),
                    "description": format!(
                        "Prepared from the insight `{}` ({}). {}",
                        insight.check, insight.subject, insight.detail
                    ),
                    "origin": "user",
                    "targets": [{
                        "device_id": insight.device_id,
                        "intent": {"type": "lines", "lines": lines},
                    }],
                }),
            )
            .await?;
        let change_id = change["id"]
            .as_str()
            .ok_or_else(|| AppError::internal("the change pipeline returned no change id"))?
            .to_string();
        app.audit(
            "insights.fix",
            Some(&insight.device_id),
            format!(
                "Prepared a change from the insight `{}` on {} {}",
                insight.check, insight.device, insight.subject
            ),
            json!({"insight_id": insight.id, "change_id": change_id}),
        );
        Ok(json!({"change_id": change_id}))
    });
}
