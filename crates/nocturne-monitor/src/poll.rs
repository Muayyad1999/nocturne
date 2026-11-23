//! Polling one device: reach it, read it, work out the rates, write the
//! samples, decide whether it is up, and hand the result to the rules.
//!
//! The two things this module is careful about:
//!
//! * **A failed read is a gap.** Nothing writes a zero for a device that did
//!   not answer. The only samples a failed poll produces are `up` = 0 and,
//!   when echoes were actually sent, the loss they measured.
//! * **Damping.** One lost poll is not an outage. A device is called down
//!   only after `down_after` consecutive failures, and up again on the first
//!   success; each crossing is recorded once, in the `event` log and on the
//!   device's state.

use std::time::Duration;

use nocturne_app::model::{Device, Transport};
use nocturne_app::store::LogEntry;
use nocturne_app::{App, AppError, AppResult, collect, inventory, jobs};
use nocturne_drivers::model::Kind;
use serde::Serialize;
use serde_json::json;

use crate::db::collections;
use crate::model::{DeviceHealth, DeviceRecord, IfStatus, Sources};
use crate::rates::{self, Interval};
use crate::read::{self, Reading};
use crate::settings::{LAB_CLI_INTERVAL_S, MonitorSettings, PING_COUNT};
use crate::state::{CounterSource, DeviceRuntime, MonitorState};

/// One number to store, in the series (device, metric, instance).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Measure {
    pub metric: String,
    pub instance: String,
    pub value: f64,
}

impl Measure {
    pub fn new(metric: &str, instance: &str, value: f64) -> Measure {
        Measure {
            metric: metric.to_string(),
            instance: instance.to_string(),
            value,
        }
    }
}

/// Everything one poll of one device produced.
#[derive(Clone, Debug, Default)]
pub struct Poll {
    pub device_id: String,
    pub device: String,
    pub lab: bool,
    pub at: i64,
    /// Whether anything answered at all.
    pub ok: bool,
    /// Which probe answered: `icmp`, `tcp/22`, `snmp`, `cli`, `sim`.
    pub via: Option<String>,
    pub error: Option<String>,
    /// The damped verdict after this poll.
    pub down: bool,
    /// When the device last came back from being down
    /// ([`DeviceRecord::up_since`]), this poll included.
    pub up_since: Option<i64>,
    /// `Some(true)` when the device came up with this poll, `Some(false)`
    /// when it went down, `None` when nothing changed.
    pub transition: Option<bool>,
    pub measures: Vec<Measure>,
    pub interfaces: Vec<IfStatus>,
    /// Whether the interface table was refreshed by this poll.
    pub interfaces_read: bool,
    pub health: DeviceHealth,
    pub sources: Sources,
    /// The device's uptime went backwards: counters were discarded.
    pub rebooted: bool,
}

impl Poll {
    pub fn measure(&self, metric: &str, instance: &str) -> Option<f64> {
        self.measures
            .iter()
            .find(|measure| measure.metric == metric && measure.instance == instance)
            .map(|measure| measure.value)
    }
}

/// Which sources are switched on for a device.
pub struct Plan {
    pub icmp: bool,
    pub snmp: bool,
    /// Whether the device has a command line at all.
    pub cli: bool,
    pub lab: bool,
}

pub fn plan(device: &Device) -> Plan {
    let lab = device.lab.is_some();
    Plan {
        // Lab addresses are never pinged: a simulated device's address is
        // either empty or belongs to somebody else on the real network.
        icmp: device.monitor.icmp && !lab,
        snmp: device.monitor.snmp,
        cli: device.transport != Transport::None,
        lab,
    }
}

impl Plan {
    /// Whether this device can be polled at all.
    pub fn any(&self) -> bool {
        self.icmp || self.snmp || self.cli || self.lab
    }
}

/// The TCP port a device is managed on, for the reachability fallback.
fn management_port(device: &Device) -> u16 {
    let default = match device.transport {
        Transport::Telnet => 23,
        _ => 22,
    };
    device
        .port
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .unwrap_or(default)
}

/// Poll one device once.
pub async fn poll_device(app: &App, device_id: &str) -> AppResult<Poll> {
    let device = inventory::get(app, device_id)?;
    let state = app.state::<MonitorState>();
    let settings = MonitorSettings::load(app);
    let plan = plan(&device);
    if !plan.any() {
        return Err(AppError::invalid(format!(
            "`{}` is monitored but has no way to be reached: switch on ICMP or SNMP for it, or give it a transport",
            device.name
        )));
    }
    let at = state.now();
    // `at` dates what this poll finds. `tick` decides what is due: the CLI
    // read, the hand-over to the collected store and the scheduler's next
    // turn are all "so long after the last one", which a time of day that
    // can be set back cannot measure.
    let tick = state.monotonic();
    let mut poll = Poll {
        device_id: device.id.clone(),
        device: device.name.clone(),
        lab: plan.lab,
        at,
        ..Poll::default()
    };

    // ---------------------------------------------------------- reachability
    let timeout = Duration::from_millis(settings.ping_timeout_ms);
    let mut rtt_ms: Option<f64> = None;
    poll.sources.icmp.enabled = plan.icmp || plan.lab;
    if plan.lab {
        poll.sources.icmp.at = Some(at);
        match lab_probe(app, &device).await {
            Ok(()) => {
                poll.ok = true;
                poll.via = Some("sim".into());
                poll.sources.icmp.ok = Some(true);
                poll.sources.icmp.via = Some("sim".into());
            }
            Err(error) => {
                poll.sources.icmp.ok = Some(false);
                poll.sources.icmp.error = Some(error.clone());
                poll.error = Some(error);
            }
        }
    } else if plan.icmp {
        poll.sources.icmp.at = Some(at);
        match state.probe.ping(&device.address, PING_COUNT, timeout).await {
            Ok(echoes) => {
                poll.measures
                    .push(Measure::new("icmp.loss_pct", "", echoes.loss_pct()));
                if echoes.answered() {
                    poll.ok = true;
                    poll.via = Some("icmp".into());
                    poll.sources.icmp.ok = Some(true);
                    poll.sources.icmp.via = Some("icmp".into());
                    rtt_ms = echoes.rtt_ms;
                    if let Some(rtt) = echoes.rtt_ms {
                        poll.measures.push(Measure::new("icmp.rtt_ms", "", rtt));
                    }
                } else {
                    poll.sources.icmp.ok = Some(false);
                    poll.sources.icmp.error = Some("no reply to ICMP echo".into());
                }
            }
            Err(error) => {
                // The probe could not even be sent: not a measurement, so no
                // loss sample is written.
                poll.sources.icmp.ok = Some(false);
                poll.sources.icmp.error = Some(error);
            }
        }
        if !poll.ok {
            // The device may simply drop ICMP while its management port is open.
            let port = management_port(&device);
            match state.probe.tcp(&device.address, port, timeout).await {
                Ok(rtt) => {
                    poll.ok = true;
                    let via = format!("tcp/{port}");
                    poll.via = Some(via.clone());
                    poll.sources.icmp.ok = Some(true);
                    poll.sources.icmp.via = Some(via);
                    poll.sources.icmp.error = None;
                    rtt_ms = Some(rtt);
                }
                Err(error) => {
                    poll.error = Some(match &poll.sources.icmp.error {
                        Some(icmp) => format!("{icmp}; {error}"),
                        None => error,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------- the read
    let mut reading: Option<(Reading, CounterSource)> = None;
    poll.sources.snmp.enabled = plan.snmp;
    if plan.snmp {
        poll.sources.snmp.at = Some(at);
        match read::over_snmp(app, &device, &state.snmp, &state.sessions).await {
            Ok(read) => {
                poll.sources.snmp.ok = Some(true);
                if !poll.ok {
                    // SNMP answering is proof the device is there.
                    poll.ok = true;
                    poll.via = Some("snmp".into());
                    poll.error = None;
                }
                reading = Some((read, CounterSource::Snmp));
            }
            Err(error) => {
                poll.sources.snmp.ok = Some(false);
                poll.sources.snmp.error = Some(error.clone());
                if poll.error.is_none() && !poll.ok {
                    poll.error = Some(error);
                }
            }
        }
    }

    let cli_due = cli_due(&state, &device, &plan, &settings, reading.is_some(), tick);
    poll.sources.cli.enabled = plan.cli && (plan.lab || !plan.snmp);
    if cli_due && (poll.ok || !plan.icmp) {
        poll.sources.cli.at = Some(at);
        state.device(&device.id, |runtime| runtime.last_cli_mono = Some(tick));
        match read::over_cli(app, &device.id).await {
            Ok(read) => {
                poll.sources.cli.ok = Some(true);
                if !poll.ok {
                    poll.ok = true;
                    poll.via = Some("cli".into());
                    poll.error = None;
                }
                reading = Some((read, CounterSource::Cli));
            }
            Err(error) => {
                poll.sources.cli.ok = Some(false);
                poll.sources.cli.error = Some(error.message.clone());
                if !poll.ok && poll.error.is_none() {
                    poll.error = Some(error.message);
                }
            }
        }
    }

    // ------------------------------------------------------- rates and gaps
    if let Some((reading, source)) = &reading {
        let outcome = state.device(&device.id, |runtime| apply(runtime, reading, *source, at));
        poll.rebooted = outcome.rebooted;
        poll.interfaces_read = reading.interfaces_read;
        poll.health = DeviceHealth {
            cpu_pct: reading.cpu_pct,
            mem_pct: reading.mem_pct,
            temp_c: reading
                .temperatures
                .iter()
                .map(|(_, celsius)| *celsius)
                .fold(None, |hottest: Option<f64>, celsius| {
                    Some(hottest.map_or(celsius, |best| best.max(celsius)))
                }),
            uptime_s: reading.uptime_s,
            at: Some(at),
        };
        if let Some(uptime) = reading.uptime_s {
            poll.measures
                .push(Measure::new("uptime_s", "", uptime as f64));
        }
        if let Some(cpu) = reading.cpu_pct {
            poll.measures.push(Measure::new("cpu.pct", "", cpu));
        }
        if let Some(memory) = reading.mem_pct {
            poll.measures.push(Measure::new("mem.pct", "", memory));
        }
        for (sensor, celsius) in &reading.temperatures {
            poll.measures.push(Measure::new("temp.c", sensor, *celsius));
        }
        let uplinks = uplink_names(app, &device.id);
        for rates in outcome.interfaces {
            push_interface_measures(&mut poll.measures, &rates);
            poll.interfaces.push(IfStatus {
                device_id: device.id.clone(),
                device: device.name.clone(),
                uplink: uplinks
                    .iter()
                    .any(|name| nocturne_drivers::ifname::same(name, &rates.name)),
                lab: plan.lab,
                at,
                ..rates.status
            });
        }
    } else {
        // No reading this time. If the source that left the counters we hold
        // was asked again and did not answer, they are now next to an unknown
        // amount of time, and are dropped rather than used. A poll in which
        // the *other* source failed, or in which the CLI was simply not due,
        // is not that: with SNMP switched on but answering nobody, every
        // poll between two CLI reads is one, and dropping the counters there
        // would leave the CLI fallback with nothing to subtract from, ever.
        state.device(&device.id, |runtime| {
            let asked_and_lost = match runtime.counters_from {
                Some(CounterSource::Snmp) => plan.snmp,
                Some(CounterSource::Cli) => cli_due,
                None => false,
            };
            if asked_and_lost {
                runtime.reset_counters();
            }
        });
    }

    poll.measures
        .push(Measure::new("up", "", if poll.ok { 1.0 } else { 0.0 }));

    // ------------------------------------------------------------- damping
    let mut record: DeviceRecord = app
        .store()
        .get(collections::DEVICES, &device.id)?
        .unwrap_or_else(|| DeviceRecord {
            device_id: device.id.clone(),
            ..DeviceRecord::default()
        });
    let was_down = record.down;
    if poll.ok {
        record.failures = 0;
        record.down = false;
        record.last_ok_at = Some(at);
    } else {
        record.failures = record.failures.saturating_add(1);
        if record.failures >= settings.down_after {
            record.down = true;
        }
    }
    record.last_poll_at = Some(at);
    record.sources = poll.sources.clone();
    if reading.is_some() {
        record.health = poll.health.clone();
    }
    poll.down = record.down;
    poll.transition = (record.down != was_down).then_some(!record.down);
    if poll.transition == Some(true) {
        // The moment it is declared up, not the moment this poll set out: a
        // check that ran while the echoes were still in the air ran against
        // a device nobody had yet seen answer.
        record.up_since = Some(state.now());
    }
    poll.up_since = record.up_since;
    app.store().put(collections::DEVICES, &device.id, &record)?;

    // ------------------------------------------------------------- storing
    write_samples(app, &device.id, at, &poll.measures)?;
    if poll.interfaces_read {
        crate::interfaces::replace(app, &device.id, &poll.interfaces)?;
        app.publish("monitor.interfaces", &json!({"device_id": device.id}));
    }
    store_collected(app, &state, &device, &reading, &settings, tick)?;

    inventory::update_state(app, &device.id, |device_state| {
        device_state.reachable = Some(!record.down);
        device_state.rtt_ms = rtt_ms;
        device_state.last_checked = Some(at);
        if poll.ok {
            device_state.last_seen = Some(at);
        }
        device_state.last_error = poll.error.clone();
    })?;

    if let Some(up) = poll.transition {
        record_transition(app, &device, &poll, up);
    }
    if poll.rebooted {
        let _ = app.store().log_append(&LogEntry {
            id: 0,
            ts: at,
            stream: "event".into(),
            device_id: Some(device.id.clone()),
            severity: Some(4),
            source: "monitor".into(),
            message: format!(
                "`{}` restarted: its uptime went backwards, so the counters since the last poll were discarded",
                device.name
            ),
            body: Some(json!({"event": "rebooted", "uptime_s": poll.health.uptime_s})),
        });
    }

    state.device(&device.id, |runtime| runtime.last_poll_mono = Some(tick));
    {
        let mut runtime = state.runtime.lock();
        runtime.polls.record(tick);
    }
    app.publish(
        "monitor.polled",
        &json!({"device_id": device.id, "at": at, "ok": poll.ok}),
    );
    Ok(poll)
}

/// Reachability for a simulated device: the simulator is asked for a console.
async fn lab_probe(app: &App, device: &Device) -> Result<(), String> {
    let options = nocturne_app::connect::OpenOptions {
        cols: 80,
        rows: 24,
        term: "dumb".into(),
    };
    let connection = nocturne_app::connect::open(app, device, &options)
        .await
        .map_err(|err| err.message)?;
    connection.close().await;
    Ok(())
}

fn cli_due(
    state: &MonitorState,
    device: &Device,
    plan: &Plan,
    settings: &MonitorSettings,
    snmp_read: bool,
    tick: i64,
) -> bool {
    if !plan.cli {
        return false;
    }
    // SNMP is cheaper and finer-grained; the CLI is the fallback for devices
    // that have none, and the only way into a simulated one.
    if plan.snmp && snmp_read && !plan.lab {
        return false;
    }
    let cadence = if plan.lab {
        LAB_CLI_INTERVAL_S
    } else {
        settings.cli_interval_s
    };
    let last = state.device(&device.id, |runtime| runtime.last_cli_mono);
    last.is_none_or(|last| tick - last >= i64::from(cadence) * 1_000)
}

/// The rates of one interface, next to the status row they belong to.
pub struct IfRates {
    pub name: String,
    pub status: IfStatus,
}

pub struct ReadingOutcome {
    pub interfaces: Vec<IfRates>,
    pub rebooted: bool,
}

/// Turn a reading plus what the previous poll left into rates, and leave the
/// new counters behind for the next one.
pub fn apply(
    runtime: &mut DeviceRuntime,
    reading: &Reading,
    source: CounterSource,
    at: i64,
) -> ReadingOutcome {
    let elapsed = match (runtime.time_base, reading.time_base) {
        (Some(previous), Some(current)) => {
            rates::interval(previous, current, at - runtime.counters_at)
        }
        _ => Interval::Unusable,
    };
    let rebooted = elapsed == Interval::Rebooted;
    let seconds = match elapsed {
        Interval::Elapsed(seconds) => Some(seconds),
        _ => None,
    };

    let mut interfaces = Vec::with_capacity(reading.interfaces.len());
    for raw in &reading.interfaces {
        let previous = seconds
            .and_then(|_| runtime.interfaces.get(&raw.name))
            .copied();
        let rate = |take: fn(&crate::state::IfCounters) -> Option<crate::rates::Counter>| {
            let (before, now) = (take(&previous?)?, take(&raw.counters)?);
            rates::rate(before, now, seconds?)
        };
        let bits = |take: fn(&crate::state::IfCounters) -> Option<crate::rates::Counter>| {
            let (before, now) = (take(&previous?)?, take(&raw.counters)?);
            rates::bits_rate(before, now, seconds?)
        };
        let in_bps = bits(|counters| counters.in_octets);
        let out_bps = bits(|counters| counters.out_octets);
        interfaces.push(IfRates {
            name: raw.name.clone(),
            status: IfStatus {
                name: raw.name.clone(),
                description: raw.description.clone(),
                admin_up: raw.admin_up,
                oper_up: raw.oper_up,
                speed_bps: raw.speed_bps,
                in_bps,
                out_bps,
                util_pct: rates::utilisation(in_bps, out_bps, raw.speed_bps),
                in_err_ps: rate(|counters| counters.in_errors),
                out_err_ps: rate(|counters| counters.out_errors),
                in_disc_ps: rate(|counters| counters.in_discards),
                out_disc_ps: rate(|counters| counters.out_discards),
                ..IfStatus::default()
            },
        });
    }

    runtime.interfaces = reading
        .interfaces
        .iter()
        .map(|raw| (raw.name.clone(), raw.counters))
        .collect();
    runtime.time_base = reading.time_base;
    runtime.counters_at = at;
    runtime.counters_from = Some(source);
    ReadingOutcome {
        interfaces,
        rebooted,
    }
}

fn push_interface_measures(measures: &mut Vec<Measure>, rates: &IfRates) {
    let status = &rates.status;
    let name = &rates.name;
    if let Some(oper) = status.oper_up {
        measures.push(Measure::new("if.oper", name, if oper { 1.0 } else { 0.0 }));
    }
    for (metric, value) in [
        ("if.in_bps", status.in_bps),
        ("if.out_bps", status.out_bps),
        ("if.util_pct", status.util_pct),
        ("if.in_err_ps", status.in_err_ps),
        ("if.out_err_ps", status.out_err_ps),
        ("if.in_disc_ps", status.in_disc_ps),
        ("if.out_disc_ps", status.out_disc_ps),
    ] {
        // `None` is a gap, not a zero: nothing is written.
        if let Some(value) = value {
            measures.push(Measure::new(metric, name, value));
        }
    }
}

/// Interfaces of a device that have a collected LLDP/CDP neighbour.
fn uplink_names(app: &App, device_id: &str) -> Vec<String> {
    collect::latest(app, device_id, Kind::Neighbors)
        .ok()
        .flatten()
        .map(|stored| {
            stored
                .typed::<nocturne_drivers::model::Neighbor>()
                .into_iter()
                .map(|neighbor| neighbor.local_interface)
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn write_samples(app: &App, device_id: &str, at: i64, measures: &[Measure]) -> AppResult<()> {
    if measures.is_empty() {
        return Ok(());
    }
    let store = app.store();
    let mut samples = Vec::with_capacity(measures.len());
    for measure in measures {
        let series = store.series_id(device_id, &measure.metric, &measure.instance)?;
        samples.push((series, at, measure.value));
    }
    store.samples_insert(&samples)
}

/// Hand SNMP interfaces to the collected-data store, so topology and IPAM see
/// the ports of devices that have no command line.
///
/// Only at the CLI cadence: the store keeps one previous generation for
/// "what changed since last time", and overwriting it every minute would
/// make that useless.
fn store_collected(
    app: &App,
    state: &MonitorState,
    device: &Device,
    reading: &Option<(Reading, CounterSource)>,
    settings: &MonitorSettings,
    tick: i64,
) -> AppResult<()> {
    // Only what SNMP itself read. A CLI read has been stored by the
    // collector already, in full and under its own name — including the one
    // that stands in while SNMP is switched on and not answering.
    let Some((reading, CounterSource::Snmp)) = reading else {
        return Ok(());
    };
    if !reading.interfaces_read {
        return Ok(());
    }
    let due = state.device(&device.id, |runtime| {
        runtime
            .last_collect_mono
            .is_none_or(|last| tick - last >= i64::from(settings.cli_interval_s) * 1_000)
    });
    if !due {
        return Ok(());
    }
    state.device(&device.id, |runtime| runtime.last_collect_mono = Some(tick));
    collect::store(
        app,
        &device.id,
        Kind::Interfaces,
        read::as_collected(&reading.interfaces),
        "snmp",
    )
}

fn record_transition(app: &App, device: &Device, poll: &Poll, up: bool) {
    let message = if up {
        match &poll.via {
            Some(via) => format!("`{}` is reachable again (over {via})", device.name),
            None => format!("`{}` is reachable again", device.name),
        }
    } else {
        match &poll.error {
            Some(error) => format!("`{}` is not answering: {error}", device.name),
            None => format!("`{}` is not answering", device.name),
        }
    };
    let _ = app.store().log_append(&LogEntry {
        id: 0,
        ts: poll.at,
        stream: "event".into(),
        device_id: Some(device.id.clone()),
        severity: Some(if up { 5 } else { 3 }),
        source: "monitor".into(),
        message,
        body: Some(json!({
            "event": if up { "device_up" } else { "device_down" },
            "via": poll.via,
            "error": poll.error,
            "lab": poll.lab,
        })),
    });
}

// ----------------------------------------------------------- monitor.poll_now

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct PollNowParams {
    pub device_ids: Vec<String>,
    /// Convenience for a single device.
    pub device_id: Option<String>,
}

pub fn poll_now(app: &App, params: PollNowParams) -> AppResult<serde_json::Value> {
    let mut ids = params.device_ids;
    ids.extend(params.device_id);
    if ids.is_empty() {
        return Err(AppError::invalid("name at least one device to poll"));
    }
    for id in &ids {
        inventory::get(app, id)?;
    }
    let settings = MonitorSettings::load(app);
    let title = if ids.len() == 1 {
        format!("Polling {}", inventory::get(app, &ids[0])?.name)
    } else {
        format!("Polling {} devices", ids.len())
    };
    let job = jobs::start(
        app,
        jobs::JobSpec::new("monitor.poll", title)
            .devices(&ids)
            .origin("user"),
        move |ctx| async move {
            let concurrency = settings.concurrency as usize;
            let results = ctx
                .for_each(ids.clone(), concurrency, |ctx, id| async move {
                    let poll = poll_device(ctx.app(), &id).await?;
                    crate::rules::evaluate(ctx.app(), &poll).await;
                    Ok(poll)
                })
                .await;
            let mut up = 0;
            let mut down = 0;
            let mut failed = 0;
            for (id, result) in ids.iter().zip(&results) {
                match result {
                    Ok(poll) if poll.ok => {
                        up += 1;
                        ctx.device(id, "info", format!("`{}` answered", poll.device));
                    }
                    Ok(poll) => {
                        down += 1;
                        ctx.device(
                            id,
                            "warn",
                            format!(
                                "`{}` did not answer: {}",
                                poll.device,
                                poll.error.as_deref().unwrap_or("no reply")
                            ),
                        );
                    }
                    Err(error) => {
                        failed += 1;
                        ctx.device(id, "error", error.message.clone());
                    }
                }
            }
            Ok(json!({"polled": results.len(), "up": up, "down": down, "failed": failed}))
        },
    );
    Ok(jobs::started(&job))
}
