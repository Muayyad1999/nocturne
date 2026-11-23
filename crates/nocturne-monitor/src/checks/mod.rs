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

use nocturne_app::{App, AppError, AppResult};
use serde_json::json;

use self::model::{CheckStatus, ServiceCheck};
use self::store::Recorded;
use crate::settings::MonitorSettings;
use crate::state::MonitorState;

pub use rpc::register;

/// Run one check, record what it found, and let the rules see it.
///
/// This is the only path a check result takes, whether the run came from the
/// scheduler or from `checks.run_now`.
///
/// `check` is the definition as the caller read it, which may be seconds old
/// by the time the probe comes back. If the check was edited or deleted in
/// the meantime the result is about something that no longer exists in that
/// form: nothing is recorded, no sample is written, the rules are not asked,
/// and the caller gets a `conflict` that says so.
pub async fn run_and_record(app: &App, check: &ServiceCheck) -> AppResult<Recorded> {
    let device = nocturne_app::inventory::get(app, &check.device_id)
        .map(|device| device.name)
        .unwrap_or_default();
    let state = app.state::<MonitorState>();
    let started_at = state.now();
    let found = run::run_once(app, check).await;
    let at = state.now();
    let Some(recorded) = store::record(app, check, &device, &found, started_at, at)? else {
        return Err(AppError::conflict(format!(
            "the `{}` check was changed or deleted while it was running, so what that run found was not recorded: it was about the check as it used to be",
            check.name
        )));
    };
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

/// Why nothing is running a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unwatched {
    /// The check itself is switched off.
    Disabled,
    /// Monitoring is switched off for the check's device.
    DeviceOff,
    /// Monitoring is switched off for the whole installation.
    MonitoringOff,
    /// Service checks are switched off for the whole installation.
    ChecksOff,
}

impl Unwatched {
    /// A stable word for it, for a caller that switches on the reason.
    pub fn code(self) -> &'static str {
        match self {
            Unwatched::Disabled => "disabled",
            Unwatched::DeviceOff | Unwatched::MonitoringOff => "monitoring_off",
            Unwatched::ChecksOff => "checks_off",
        }
    }

    /// The sentence a person is shown when they ask for a run and do not get
    /// one.
    pub fn sentence(self, check: &ServiceCheck, device: &str) -> String {
        match self {
            Unwatched::Disabled => format!(
                "the `{}` check is switched off, so it was not run: switch it on to have it checked",
                check.name
            ),
            Unwatched::DeviceOff => format!(
                "monitoring is switched off for `{device}`, and that switches off its service checks too, so `{}` was not run",
                check.name
            ),
            Unwatched::MonitoringOff => format!(
                "monitoring is switched off (the `monitoring` setting `enabled`), so `{}` was not run",
                check.name
            ),
            Unwatched::ChecksOff => format!(
                "service checks are switched off (the `monitoring` setting `checks_enabled`), so `{}` was not run",
                check.name
            ),
        }
    }
}

/// Whether anything is running a check: the one place that rule is written.
///
/// A check is watched when it is switched on, its device is monitored, and
/// monitoring and service checks are on for the installation. Switching
/// monitoring off for a device has to switch *everything* off for it, or it
/// does not mean anything — so everything that asks "is this check live?"
/// asks here:
///
/// * the scheduler, before it starts a run;
/// * `checks.run_now`, which leaves an unwatched check alone and says why —
///   a run by hand would record a verdict, and raise an alert, that no later
///   run would ever revisit;
/// * `monitor.status`, which counts an unwatched check as `disabled`
///   whatever it last found;
/// * `checks.history`, which stops crediting the last status with time once
///   nothing is running the check.
///
/// The alerts follow the same switch from the rules' side: see
/// [`crate::rules::stand_down`].
pub fn watched(
    settings: &MonitorSettings,
    device_monitored: bool,
    check_enabled: bool,
) -> Result<(), Unwatched> {
    if !check_enabled {
        Err(Unwatched::Disabled)
    } else if !device_monitored {
        Err(Unwatched::DeviceOff)
    } else if !settings.enabled {
        Err(Unwatched::MonitoringOff)
    } else if !settings.checks_enabled {
        Err(Unwatched::ChecksOff)
    } else {
        Ok(())
    }
}

/// [`watched`] for one check, reading the settings and the device itself.
/// A check whose device has left the inventory is not watched either.
pub fn watching(app: &App, check: &ServiceCheck) -> Result<(), Unwatched> {
    let monitored = nocturne_app::inventory::get(app, &check.device_id)
        .map(|device| device.monitor.enabled)
        .unwrap_or(false);
    watched(&MonitorSettings::load(app), monitored, check.enabled)
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
