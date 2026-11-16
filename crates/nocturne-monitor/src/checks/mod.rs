//! Per-host service checks: a named check on a device, with its own
//! schedule, its own up/down state, its own history and its own alert.
//!
//! The device poller answers one question per device — can we reach it? A
//! service check answers a different one, per service: *is the web server on
//! 10.0.0.5 answering, even though the box answers ping?* Nothing else in
//! Nocturne can say that sentence, and it is the sentence a NOC needs most
//! often.
//!
//! What this module is, in one paragraph: a check is a [`model::CheckSpec`]
//! attached to a device, run on the existing poller's scheduler at its own
//! interval ([`crate::scheduler`]), driving the existing probe engines
//! ([`run`]), judged into one of five states ([`model::CheckStatus`]),
//! recorded with its changes ([`store`]), charted from the existing time
//! series, and alerted on through the existing rules engine
//! ([`crate::model::Condition::CheckState`]) with its existing hysteresis,
//! damping and maintenance windows. There is no second scheduler, no second
//! alerting path and no new network code.
//!
//! Three decisions worth knowing before reading on:
//!
//! * **A check that could not run is not a check that is down.** It is a
//!   fifth state, it writes no `check.up` sample, and it raises a different
//!   alert — one that says monitoring has gone blind, not that a service has
//!   gone down. Conflating the two invents outages, which is the fastest way
//!   to make a NOC screen worthless.
//! * **A certificate that is about to expire is not a failure.** It is
//!   `warning`: the service is available, and will stop being available on a
//!   date that is known. It is still up in every availability figure.
//! * **A new device acquires no checks.** Nothing is created for anybody
//!   automatically; a check exists because a person asked for it.

pub mod model;
pub mod outcome;
pub mod rpc;
pub mod run;
pub mod store;

use nocturne_app::{App, AppResult};
use serde_json::json;

use self::model::{CheckStatus, ServiceCheck};
use self::store::Recorded;
use crate::state::MonitorState;

pub use rpc::register;

/// Run one check, record what it found, and let the rules see it.
///
/// This is the only path a check result takes, whether the run came from the
/// scheduler or from `checks.run_now`.
pub async fn run_and_record(app: &App, check: &ServiceCheck) -> AppResult<Recorded> {
    let device = nocturne_app::inventory::get(app, &check.device_id)
        .map(|device| device.name)
        .unwrap_or_default();
    let found = run::run_once(app, check).await;
    let at = app.state::<MonitorState>().now();
    let recorded = store::record(app, check, &device, &found, at)?;
    crate::poll::write_samples(app, &check.device_id, at, &run::samples(check, &found))?;
    app.publish(
        "checks.ran",
        &json!({
            "check_id": check.id,
            "device_id": check.device_id,
            "name": check.name,
            "kind": check.kind().as_str(),
            "at": at,
            "status": recorded.state.status.as_str(),
            "previous": recorded.previous.as_str(),
            "changed": recorded.changed,
            "code": recorded.state.code,
            "rtt_ms": recorded.state.rtt_ms,
        }),
    );
    // Through the rules engine, exactly as a device poll is: the rules decide
    // whether any of this is worth waking somebody for.
    crate::rules::evaluate_checks(app, &check.device_id).await;
    Ok(recorded)
}

/// Whether a check should be running at all right now.
///
/// A device that is not monitored does not have its services checked either:
/// switching monitoring off for a device has to switch *everything* off for
/// it, or it does not mean anything.
pub fn runnable(app: &App, check: &ServiceCheck) -> bool {
    check.enabled
        && nocturne_app::inventory::get(app, &check.device_id)
            .map(|device| device.monitor.enabled)
            .unwrap_or(false)
}

/// The checks of a device that are not up, for a device view or an alert's
/// detail.
pub fn problems(app: &App, device_id: &str) -> AppResult<Vec<(ServiceCheck, model::CheckState)>> {
    Ok(store::of_device(app, device_id)?
        .into_iter()
        .filter(|(check, state)| check.enabled && !state.status.is_up())
        .collect())
}

/// Checks that cannot run right now: monitoring that has gone blind.
pub fn blind(app: &App, device_id: &str) -> AppResult<Vec<(ServiceCheck, model::CheckState)>> {
    Ok(store::of_device(app, device_id)?
        .into_iter()
        .filter(|(check, state)| check.enabled && state.status == CheckStatus::CannotRun)
        .collect())
}
