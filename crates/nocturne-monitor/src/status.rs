//! What the NOC screen asks for: `monitor.status`, `monitor.device` and
//! `monitor.interfaces`.

use nocturne_app::model::DeviceState;
use nocturne_app::{App, AppResult, Router, inventory};
use serde::Serialize;

use crate::db::collections;
use crate::interfaces::{self, InterfaceQuery};
use crate::model::{Channel, DeviceHealth, DeviceRecord, IfStatus, Sources};
use crate::poll::{self, PollNowParams};
use crate::settings::MonitorSettings;
use crate::state::MonitorState;
use crate::{maintenance, snmp};

/// Something a person should look at, in the installation rather than on a
/// device: a credential that is not set, a channel that cannot be reached.
#[derive(Debug, Serialize)]
pub struct Problem {
    /// `unreachable`, `no_probe`, `snmp_credential`, `channel`, `ping_method`,
    /// `maintenance`, `interfaces`.
    pub kind: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub devices_monitored: u64,
    /// Polls started in the last minute.
    pub polls_per_min: u64,
    pub last_cycle_ms: u64,
    /// How echo requests are sent on this machine.
    pub ping_method: String,
    pub problems: Vec<Problem>,
}

/// How many devices this installation is actually watching, and what is
/// stopping it watching the rest.
pub fn status(app: &App) -> AppResult<Status> {
    let state = app.state::<MonitorState>();
    let settings = MonitorSettings::load(app);
    let now = state.now();
    let devices = inventory::list(app, &inventory::DeviceFilter::default())?;

    let mut monitored = 0u64;
    let mut problems = Vec::new();
    let mut down: Vec<String> = Vec::new();
    for row in &devices {
        let device = &row.device;
        if !device.monitor.enabled {
            continue;
        }
        let plan = poll::plan(device);
        if !plan.any() {
            problems.push(Problem {
                kind: "no_probe",
                message: format!(
                    "`{}` is monitored but has nothing to probe it with: switch on ICMP or SNMP, or give it a transport",
                    device.name
                ),
                device_id: Some(device.id.clone()),
                count: None,
            });
            continue;
        }
        monitored += 1;
        if plan.snmp
            && let Err(err) = snmp::target(app, device)
        {
            problems.push(Problem {
                kind: "snmp_credential",
                message: err.message,
                device_id: Some(device.id.clone()),
                count: None,
            });
        }
        if let Ok(Some(record)) = app
            .store()
            .get::<DeviceRecord>(collections::DEVICES, &device.id)
            && record.down
        {
            down.push(device.name.clone());
        }
    }
    if !down.is_empty() {
        down.sort();
        let shown: Vec<&str> = down.iter().take(5).map(String::as_str).collect();
        problems.push(Problem {
            kind: "unreachable",
            message: format!(
                "{} not answering: {}{}",
                if down.len() == 1 {
                    "1 device is".to_string()
                } else {
                    format!("{} devices are", down.len())
                },
                shown.join(", "),
                if down.len() > shown.len() {
                    ", …"
                } else {
                    ""
                }
            ),
            device_id: None,
            count: Some(down.len() as u64),
        });
    }

    let interface_problems = interfaces::problem_count(app)?;
    if interface_problems > 0 {
        problems.push(Problem {
            kind: "interfaces",
            message: format!(
                "{interface_problems} interfaces are down, taking errors or close to full"
            ),
            device_id: None,
            count: Some(interface_problems),
        });
    }

    for channel in app.store().list::<Channel>(collections::CHANNELS)? {
        if let Some(error) = channel.last_error.filter(|_| channel.enabled) {
            problems.push(Problem {
                kind: "channel",
                message: format!(
                    "The notification channel `{}` failed: {error}",
                    channel.name
                ),
                device_id: None,
                count: None,
            });
        }
    }

    let open = maintenance::open_now(app, now)?;
    if !open.is_empty() {
        problems.push(Problem {
            kind: "maintenance",
            message: format!(
                "{} maintenance window{} open: {}",
                open.len(),
                if open.len() == 1 { " is" } else { "s are" },
                open.iter()
                    .map(|window| window.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            device_id: None,
            count: Some(open.len() as u64),
        });
    }

    let ping_method = state.probe.method();
    if ping_method == "system_command" {
        problems.push(Problem {
            kind: "ping_method",
            message: "ICMP sockets are not available to this process, so echoes go through the system `ping` program: slower, and round-trip times are less precise. On Linux, `sysctl -w net.ipv4.ping_group_range='0 2147483647'` fixes it."
                .into(),
            device_id: None,
            count: None,
        });
    }

    let (polls_per_min, last_cycle_ms) = {
        let mut runtime = state.runtime.lock();
        let last = runtime.last_cycle_ms;
        (runtime.polls.per_minute(now), last)
    };
    Ok(Status {
        enabled: settings.enabled,
        devices_monitored: monitored,
        polls_per_min,
        last_cycle_ms,
        ping_method,
        problems,
    })
}

#[derive(Debug, Serialize)]
pub struct DeviceView {
    pub state: DeviceState,
    pub health: DeviceHealth,
    pub interfaces: Vec<IfStatus>,
    pub sources: Sources,
    /// The damped verdict, which can differ from `state.reachable` only
    /// between a failed poll and the one that confirms it.
    pub down: bool,
    pub last_poll_at: Option<i64>,
    pub lab: bool,
}

pub fn device_view(app: &App, device_id: &str) -> AppResult<DeviceView> {
    let device = inventory::get(app, device_id)?;
    let record: DeviceRecord = app
        .store()
        .get(collections::DEVICES, device_id)?
        .unwrap_or_else(|| DeviceRecord {
            device_id: device_id.to_string(),
            ..DeviceRecord::default()
        });
    Ok(DeviceView {
        state: inventory::state_of(app, device_id)?,
        health: record.health,
        interfaces: interfaces::of_device(app, device_id)?,
        sources: record.sources,
        down: record.down,
        last_poll_at: record.last_poll_at,
        lab: device.lab.is_some(),
    })
}

#[derive(Debug, serde::Deserialize)]
pub struct DeviceParams {
    pub device_id: String,
}

pub fn register(router: &mut Router) {
    router.add(
        "monitor.status",
        |app: App, _: nocturne_app::NoParams| async move { status(&app) },
    );
    router.add(
        "monitor.device",
        |app: App, params: DeviceParams| async move { device_view(&app, &params.device_id) },
    );
    router.add(
        "monitor.interfaces",
        |app: App, params: InterfaceQuery| async move {
            if let Some(device_id) = &params.device_id {
                inventory::get(&app, device_id)?;
            }
            interfaces::query(&app, &params)
        },
    );
    router.add(
        "monitor.poll_now",
        |app: App, params: PollNowParams| async move { poll::poll_now(&app, params) },
    );
}
