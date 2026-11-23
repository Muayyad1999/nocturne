//! `logs.status`: what is listening, what has arrived, what was dropped,
//! what the rules are holding, what the forwarders have sent on and what is
//! stored.
//!
//! Each receiver reports its own numbers, in its own units: lines for
//! syslog, notifications for traps, datagrams and flow records for the flow
//! collector. A receiver that is off says so with `enabled: false`; one
//! that is on and could not bind says why in `error`.

use std::sync::atomic::Ordering;

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppResult};

use crate::db;
use crate::model::{LogsStatus, StoreStatus};
use crate::settings::LogsSettings;
use crate::state::LogsState;

pub fn snapshot(app: &App) -> AppResult<LogsStatus> {
    let state = app.state::<LogsState>();
    let settings = LogsSettings::load(app);
    let (rows, oldest_at) = db::extent(app.store())?;
    let unattributed = db::sources(app.store())?
        .into_iter()
        .filter(|row| row.device_id.is_none())
        .count();
    Ok(LogsStatus {
        syslog: state.syslog.status(now_ms()),
        traps: state.traps.status(now_ms()),
        flows: state.flows.status(now_ms()),
        rules: crate::rules::status(app),
        forwarders: crate::forward::status(app)?,
        store: StoreStatus {
            rows,
            oldest_at,
            retention_days: settings.retention_days,
            max_rows: settings.max_rows,
            last_prune_at: state.retention.last_prune_at(),
            pruned_by_age: state.retention.pruned_by_age.load(Ordering::Relaxed),
            pruned_by_rows: state.retention.pruned_by_rows.load(Ordering::Relaxed),
        },
        unattributed_sources: unattributed,
    })
}

/// Tell the UI the receiver's state changed. A status that cannot be read is
/// not worth an event.
pub fn publish(app: &App) {
    match snapshot(app) {
        Ok(status) => app.publish("logs.status_changed", &status),
        Err(err) => tracing::error!(%err, "the log status could not be read"),
    }
}
