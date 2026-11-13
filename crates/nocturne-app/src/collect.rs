//! Structured device data: facts, interfaces, neighbours, VLANs, MAC and ARP
//! tables, routes, health and hardware inventory.
//!
//! Whatever reads a device — the CLI collectors, SNMP polling, the lab
//! simulator — stores what it read here, one document per device and kind,
//! and everything that reasons about the network (topology, IPAM, insights,
//! the assistant) reads it back from here. The previous generation is kept so
//! that "what changed since last time" can be answered.

use nocturne_drivers::collect::Collector;
pub use nocturne_drivers::collect::{Problem, ProblemCode, Snapshot, StepReport};
use nocturne_drivers::model::{Facts, Kind};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, AppBuilder};
use crate::connect::{self, Cli, CliOptions};
use crate::error::{AppError, AppResult};
use crate::jobs::{self, JobSpec};
use crate::util::now_ms;
use crate::{inventory, profiles};

fn collection(kind: Kind) -> String {
    format!("collected.{}", kind.as_str())
}

fn previous_collection(kind: Kind) -> String {
    format!("collected_prev.{}", kind.as_str())
}

/// One kind of data as last read from one device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub device_id: String,
    pub kind: Kind,
    pub collected_at: i64,
    /// Where it came from: `cli`, `snmp`, `lab`, …
    pub source: String,
    /// Model rows as JSON (`nocturne_drivers::model::*`); single-row kinds hold one.
    pub rows: Vec<Value>,
}

impl Stored {
    /// The rows as model structs. A row that does not fit the model is skipped.
    pub fn typed<T: DeserializeOwned>(&self) -> Vec<T> {
        self.rows
            .iter()
            .filter_map(|row| serde_json::from_value(row.clone()).ok())
            .collect()
    }
}

/// See [`Stored::typed`].
pub fn typed<T: DeserializeOwned>(stored: &Stored) -> Vec<T> {
    stored.typed()
}

fn put(
    app: &App,
    device_id: &str,
    kind: Kind,
    rows: Vec<Value>,
    source: &str,
    at: i64,
) -> AppResult<Stored> {
    let stored = Stored {
        device_id: device_id.to_string(),
        kind,
        collected_at: at,
        source: source.to_string(),
        rows,
    };
    if let Some(current) = app.store().get::<Stored>(&collection(kind), device_id)? {
        app.store()
            .put(&previous_collection(kind), device_id, &current)?;
    }
    app.store().put(&collection(kind), device_id, &stored)?;
    if kind == Kind::Facts
        && let Some(facts) = stored.typed::<Facts>().into_iter().next()
    {
        inventory::update_state(app, device_id, |state| {
            state.facts = Some(facts);
            state.facts_at = Some(at);
        })?;
    }
    Ok(stored)
}

/// Store several kinds read in one go and announce them once.
pub fn store_many(
    app: &App,
    device_id: &str,
    data: Vec<(Kind, Vec<Value>)>,
    source: &str,
) -> AppResult<()> {
    if data.is_empty() {
        return Ok(());
    }
    // Refuse to record data for a device that does not exist (it would never be cleaned up).
    inventory::get(app, device_id)?;
    let at = now_ms();
    let mut kinds = Vec::with_capacity(data.len());
    for (kind, rows) in data {
        put(app, device_id, kind, rows, source, at)?;
        kinds.push(kind);
    }
    app.publish(
        "collect.updated",
        &json!({"device_id": device_id, "kinds": kinds, "at": at, "source": source}),
    );
    Ok(())
}

/// Store one kind of data read from a device.
pub fn store(
    app: &App,
    device_id: &str,
    kind: Kind,
    rows: Vec<Value>,
    source: &str,
) -> AppResult<()> {
    store_many(app, device_id, vec![(kind, rows)], source)
}

/// What was last read, or `None` if this kind was never collected from the device.
pub fn latest(app: &App, device_id: &str, kind: Kind) -> AppResult<Option<Stored>> {
    app.store().get(&collection(kind), device_id)
}

/// The generation before [`latest`], for "what changed" questions.
pub fn previous(app: &App, device_id: &str, kind: Kind) -> AppResult<Option<Stored>> {
    app.store().get(&previous_collection(kind), device_id)
}

/// One kind across every device that has it.
pub fn latest_all(app: &App, kind: Kind) -> AppResult<Vec<Stored>> {
    app.store().list(&collection(kind))
}

/// Drop everything collected from a device (when it is deleted).
pub fn forget(app: &App, device_id: &str) -> AppResult<()> {
    for kind in Kind::ALL {
        app.store().delete(&collection(kind), device_id)?;
        app.store().delete(&previous_collection(kind), device_id)?;
    }
    Ok(())
}

/// The kinds a platform's profile knows how to collect.
pub fn supported_kinds(platform: &nocturne_drivers::profile::Platform) -> Vec<Kind> {
    Kind::ALL
        .into_iter()
        .filter(|kind| {
            platform
                .profile
                .collect
                .get(kind)
                .is_some_and(|steps| !steps.is_empty())
        })
        .collect()
}

/// Collect over a session the caller already holds, store what was read and
/// announce it. `kinds` empty = everything the platform's profile defines.
///
/// The snapshot says per kind whether it was read (`Some`), and why not when
/// it was not (`problems`): a kind that could not be read is never stored as
/// empty, and what was stored before stays.
pub async fn run_on(app: &App, cli: &mut Cli, kinds: &[Kind]) -> AppResult<Snapshot> {
    let platform = cli.platform.clone();
    let kinds: Vec<Kind> = if kinds.is_empty() {
        supported_kinds(&platform)
    } else {
        kinds.to_vec()
    };
    let collector = Collector::new(profiles::expressions(app));
    let snapshot = collector.collect(cli.session(), &platform, &kinds).await;
    let data: Vec<(Kind, Vec<Value>)> = Kind::ALL
        .into_iter()
        .filter_map(|kind| snapshot.rows(kind).map(|rows| (kind, rows)))
        .collect();
    let source = if cli.device.lab.is_some() {
        "lab"
    } else {
        "cli"
    };
    store_many(app, &cli.device.id, data, source)?;
    Ok(snapshot)
}

/// Open a CLI session to the device, collect, and close it.
pub async fn run(app: &App, device_id: &str, kinds: &[Kind]) -> AppResult<Snapshot> {
    let mut cli = connect::cli(app, device_id, CliOptions::default()).await?;
    let snapshot = run_on(app, &mut cli, kinds).await;
    cli.close().await;
    snapshot
}

/// The outcome of collecting from one device, for job results.
#[derive(Debug, Serialize)]
pub struct DeviceOutcome {
    pub device_id: String,
    pub name: String,
    pub collected: Vec<Kind>,
    pub problems: Vec<Problem>,
    /// Set when the device could not be reached or logged in to at all.
    pub error: Option<String>,
}

/// What has been collected from a device, without the rows.
#[derive(Debug, Serialize)]
pub struct KindStatus {
    pub kind: Kind,
    pub collected_at: i64,
    pub source: String,
    pub rows: usize,
}

pub fn status(app: &App, device_id: &str) -> AppResult<Vec<KindStatus>> {
    let mut status = Vec::new();
    for kind in Kind::ALL {
        if let Some(stored) = latest(app, device_id, kind)? {
            status.push(KindStatus {
                kind,
                collected_at: stored.collected_at,
                source: stored.source,
                rows: stored.rows.len(),
            });
        }
    }
    Ok(status)
}

#[derive(Deserialize)]
struct GetParams {
    id: String,
    kind: Kind,
    /// Return the previous generation instead of the latest.
    #[serde(default)]
    previous: bool,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RunParams {
    device_ids: Vec<String>,
    /// Convenience for a single device.
    id: Option<String>,
    /// Empty = everything each device's profile defines.
    kinds: Vec<Kind>,
    /// Who asked: `user` (default), `schedule`, `assistant`.
    origin: Option<String>,
}

pub fn init(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add("collect.get", |app: App, params: GetParams| async move {
        let device = inventory::find(&app, &params.id)?;
        if params.previous {
            previous(&app, &device.id, params.kind)
        } else {
            latest(&app, &device.id, params.kind)
        }
    });
    router.add("collect.run", |app: App, params: RunParams| async move {
        let mut ids = Vec::new();
        for reference in params.device_ids.iter().chain(params.id.iter()) {
            let device = inventory::find(&app, reference)?;
            if !ids.contains(&device.id) {
                ids.push(device.id);
            }
        }
        if ids.is_empty() {
            return Err(AppError::invalid("no devices given"));
        }
        let title = match ids.len() {
            1 => format!("Collect from {}", inventory::get(&app, &ids[0])?.name),
            count => format!("Collect from {count} devices"),
        };
        let kinds = params.kinds;
        let spec = JobSpec::new("collect.run", title)
            .devices(&ids)
            .origin(params.origin.as_deref().unwrap_or("user"));
        let job = jobs::start(&app, spec, move |ctx| async move {
            let outcomes = ctx
                .for_each(ids.clone(), 4, move |ctx, id| {
                    let kinds = kinds.clone();
                    async move {
                        let app = ctx.app().clone();
                        let name = inventory::get(&app, &id)
                            .map(|device| device.name)
                            .unwrap_or_default();
                        let outcome = match run(&app, &id, &kinds).await {
                            Ok(snapshot) => {
                                for problem in &snapshot.problems {
                                    let level = if problem.code == ProblemCode::Unsupported {
                                        "info"
                                    } else {
                                        "warn"
                                    };
                                    ctx.device(
                                        &id,
                                        level,
                                        format!("{}: {}", problem.kind, problem.message),
                                    );
                                }
                                DeviceOutcome {
                                    collected: Kind::ALL
                                        .into_iter()
                                        .filter(|kind| snapshot.has(*kind))
                                        .collect(),
                                    problems: snapshot.problems,
                                    device_id: id,
                                    name,
                                    error: None,
                                }
                            }
                            Err(err) => {
                                ctx.device(&id, "error", err.message.clone());
                                DeviceOutcome {
                                    device_id: id,
                                    name,
                                    collected: Vec::new(),
                                    problems: Vec::new(),
                                    error: Some(err.message),
                                }
                            }
                        };
                        Ok(outcome)
                    }
                })
                .await;
            let devices: Vec<DeviceOutcome> = outcomes.into_iter().filter_map(Result::ok).collect();
            let failed = devices
                .iter()
                .filter(|device| device.error.is_some())
                .count();
            Ok(json!({"devices": devices, "failed": failed}))
        });
        Ok(jobs::started(&job))
    });
    router.add("collect.status", |app: App, params: IdParams| async move {
        let device = inventory::find(&app, &params.id)?;
        status(&app, &device.id)
    });
}

/// A kind name as the RPC spells it, for error messages.
pub fn kind_from_str(name: &str) -> AppResult<Kind> {
    Kind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == name)
        .ok_or_else(|| {
            AppError::invalid(format!(
                "unknown kind `{name}` (one of: {})",
                Kind::ALL
                    .iter()
                    .map(|kind| kind.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{builder, lab};
    use nocturne_drivers::model::Interface;
    use serde_json::json;

    #[tokio::test]
    async fn stores_generations_updates_facts_and_announces_once() {
        let lab = lab(builder(), &["SW1"]).await;
        let (app, id) = (&lab.app, lab.id("SW1"));
        let mut events = app.subscribe();

        assert_eq!(latest(app, &id, Kind::Interfaces).unwrap(), None);
        store_many(
            app,
            &id,
            vec![
                (
                    Kind::Facts,
                    vec![json!({"hostname": "SW1", "model": "C2960", "serial": "FOC1"})],
                ),
                (
                    Kind::Interfaces,
                    vec![
                        json!({"name": "GigabitEthernet0/1", "oper_up": true}),
                        json!("not a row"),
                    ],
                ),
            ],
            "cli",
        )
        .unwrap();
        let updates: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| event.topic == "collect.updated")
            .collect();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].payload["kinds"], json!(["facts", "interfaces"]));

        // Facts land in the device state the inventory shows.
        let device = app.rpc("devices.get", json!({"id": id})).await.unwrap();
        assert_eq!(device["state"]["facts"]["model"], "C2960");

        // Typed access skips rows that do not fit the model.
        let interfaces = latest(app, &id, Kind::Interfaces).unwrap().unwrap();
        assert_eq!(interfaces.rows.len(), 2);
        let typed: Vec<Interface> = interfaces.typed();
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].name, "GigabitEthernet0/1");

        // A second collection keeps the first as the previous generation.
        assert_eq!(previous(app, &id, Kind::Interfaces).unwrap(), None);
        store(
            app,
            &id,
            Kind::Interfaces,
            vec![json!({"name": "GigabitEthernet0/1", "oper_up": false})],
            "snmp",
        )
        .unwrap();
        assert_eq!(
            previous(app, &id, Kind::Interfaces)
                .unwrap()
                .unwrap()
                .rows
                .len(),
            2
        );
        assert_eq!(
            latest(app, &id, Kind::Interfaces).unwrap().unwrap().source,
            "snmp"
        );
        assert_eq!(latest_all(app, Kind::Interfaces).unwrap().len(), 1);

        let status = app
            .rpc("collect.status", json!({"id": "sw1"}))
            .await
            .unwrap();
        assert_eq!(status.as_array().unwrap().len(), 2);
        let got = app
            .rpc(
                "collect.get",
                json!({"id": id, "kind": "interfaces", "previous": true}),
            )
            .await
            .unwrap();
        assert_eq!(got["rows"].as_array().unwrap().len(), 2);
        assert_eq!(
            app.rpc("collect.get", json!({"id": id, "kind": "routes"}))
                .await
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            app.rpc("collect.get", json!({"id": id, "kind": "nonsense"}))
                .await
                .unwrap_err()
                .code,
            "invalid"
        );

        // Unknown devices are refused; deleting a device removes what was collected from it.
        assert_eq!(
            store(app, "nope", Kind::Arp, vec![], "cli")
                .unwrap_err()
                .code,
            "not_found"
        );
        app.rpc("devices.delete", json!({"ids": [id]}))
            .await
            .unwrap();
        assert!(latest_all(app, Kind::Interfaces).unwrap().is_empty());
        assert!(latest_all(app, Kind::Facts).unwrap().is_empty());
        assert_eq!(kind_from_str("mac_table").unwrap(), Kind::MacTable);
    }

    #[tokio::test]
    async fn collects_from_a_device_through_its_profile() {
        let lab = lab(builder(), &["SW1", "SW2"]).await;
        let (app, id) = (&lab.app, lab.id("SW1"));

        let snapshot = run(app, &id, &[]).await.unwrap();
        let facts = snapshot.facts.as_ref().expect("facts");
        assert_eq!(
            (
                facts.hostname.as_str(),
                facts.model.as_str(),
                facts.serial.as_str()
            ),
            ("SW1", "WS-C2960-24TT-L", "FOC1234X5YZ")
        );
        assert_eq!(facts.os_version, "15.2(4)E10");
        assert_eq!(
            facts.uptime_s,
            Some(7 * 86_400 + 2 * 86_400 + 3 * 3600 + 4 * 60)
        );
        let interfaces = snapshot.interfaces.as_ref().expect("interfaces");
        assert_eq!(interfaces.len(), 3);
        assert!(
            interfaces
                .iter()
                .all(|interface| interface.oper_up == Some(true))
        );
        let neighbors = snapshot.neighbors.as_ref().expect("neighbors");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(
            (
                neighbors[0].device.as_str(),
                neighbors[0].local_interface.as_str(),
                neighbors[0].address.as_str()
            ),
            ("CORE1.lab.example", "GigabitEthernet0/1", "10.0.0.1")
        );
        assert_eq!(snapshot.vlans.as_ref().map(Vec::len), Some(2));
        // Kinds the profile does not define are reported, not invented.
        assert!(snapshot.routes.is_none());

        // Stored, announced, and visible in the inventory.
        assert_eq!(
            latest(app, &id, Kind::Neighbors).unwrap().unwrap().source,
            "lab"
        );
        assert_eq!(
            app.rpc("devices.get", json!({"id": id})).await.unwrap()["state"]["facts"]["serial"],
            "FOC1234X5YZ"
        );

        // A link goes down and a neighbour disappears: the next collection shows it and keeps the previous one.
        lab.fake("SW1").with(|device| {
            device.link_down.push("GigabitEthernet0/1".into());
            device.neighbors.clear();
        });
        let after = run(app, &id, &[Kind::Interfaces, Kind::Neighbors])
            .await
            .unwrap();
        let uplink = after
            .interfaces
            .unwrap()
            .into_iter()
            .find(|interface| interface.name == "GigabitEthernet0/1")
            .unwrap();
        assert_eq!((uplink.admin_up, uplink.oper_up), (Some(true), Some(false)));
        assert_eq!(after.neighbors.map(|neighbors| neighbors.len()), Some(0));
        assert_eq!(
            previous(app, &id, Kind::Neighbors)
                .unwrap()
                .unwrap()
                .rows
                .len(),
            1
        );

        // Asking for a kind the profile lacks is a recorded problem and leaves stored data alone.
        let unsupported = run(app, &id, &[Kind::Routes]).await.unwrap();
        assert!(unsupported.problems.iter().any(
            |problem| problem.kind == Kind::Routes && problem.code == ProblemCode::Unsupported
        ));
        assert_eq!(latest(app, &id, Kind::Routes).unwrap(), None);
    }

    #[tokio::test]
    async fn collect_run_is_a_job_over_many_devices() {
        let lab = lab(builder(), &["SW1", "SW2"]).await;
        let app = &lab.app;
        let unreachable = app
            .rpc(
                "devices.create",
                json!({"name": "ghost", "address": "192.0.2.9", "transport": "none"}),
            )
            .await
            .unwrap();
        let started = app
            .rpc(
                "collect.run",
                json!({"device_ids": ["SW1", lab.id("SW2"), unreachable["id"]], "kinds": ["facts", "vlans"]}),
            )
            .await
            .unwrap();
        let job = jobs::wait(app, started["job_id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(job.state, jobs::JobState::Succeeded);
        let result = job.result.unwrap();
        assert_eq!(result["failed"], 1);
        let devices = result["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0]["collected"], json!(["facts", "vlans"]));
        assert!(
            devices[2]["error"]
                .as_str()
                .unwrap()
                .contains("monitor-only")
        );
        assert_eq!(latest_all(app, Kind::Vlans).unwrap().len(), 2);
        assert_eq!(
            app.rpc("collect.run", json!({})).await.unwrap_err().code,
            "invalid"
        );
    }
}
