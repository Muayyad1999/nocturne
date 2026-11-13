//! `logs.status`: what is listening, what has arrived, what was dropped and
//! what is stored.
//!
//! Three of the four receivers the specification describes are not in this
//! build. They are reported as `implemented: false` rather than left out or
//! shown as "disabled": a screen that offers a switch for a trap receiver
//! that does not exist would be the application lying about itself.

use std::sync::atomic::Ordering;

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppResult};

use crate::db;
use crate::model::{LogsStatus, NotBuilt, StoreStatus};
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
        syslog: state.status(now_ms()),
        traps: NotBuilt::new("the SNMP trap receiver"),
        flows: NotBuilt::new("the flow collector"),
        store: StoreStatus {
            rows,
            oldest_at,
            retention_days: settings.retention_days,
            max_rows: settings.max_rows,
            last_prune_at: state.counters.last_prune_at(),
            pruned_by_age: state.counters.pruned_by_age.load(Ordering::Relaxed),
            pruned_by_rows: state.counters.pruned_by_rows.load(Ordering::Relaxed),
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
