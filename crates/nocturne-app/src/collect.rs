//! Structured device data: facts, interfaces, neighbours, VLANs, MAC and ARP
//! tables, routes, health, hardware inventory and per-class QoS statistics.
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
    /// No rows is an answer — the device has none — and not a missing one:
    /// a kind that could not be read has no document at all.
    pub rows: Vec<Value>,
    /// The rows were read by a profile that has not yet met a real device of
    /// its platform for this kind (`unverified: [<kind>]` in the profile), so
    /// they may be wrong in ways no test has had the chance to show. Said of
    /// the read that produced the rows, when it was made; absent when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unverified: bool,
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// One kind as it was just read, on its way into the store.
struct Reading {
    kind: Kind,
    rows: Vec<Value>,
    unverified: bool,
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

fn put(app: &App, device_id: &str, reading: Reading, source: &str, at: i64) -> AppResult<Stored> {
    let kind = reading.kind;
    let stored = Stored {
        device_id: device_id.to_string(),
        kind,
        collected_at: at,
        source: source.to_string(),
        rows: reading.rows,
        unverified: reading.unverified,
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
    // Whoever hands rows in here read them some other way than through the
    // device's profile, so what the profile says of its own reads does not
    // apply to them.
    let readings = data
        .into_iter()
        .map(|(kind, rows)| Reading {
            kind,
            rows,
            unverified: false,
        })
        .collect();
    store_readings(app, device_id, readings, source)
}

fn store_readings(
    app: &App,
    device_id: &str,
    readings: Vec<Reading>,
    source: &str,
) -> AppResult<()> {
    if readings.is_empty() {
        return Ok(());
    }
    // Refuse to record data for a device that does not exist (it would never be cleaned up).
    inventory::get(app, device_id)?;
    let at = now_ms();
    let mut kinds = Vec::with_capacity(readings.len());
    for reading in readings {
        kinds.push(reading.kind);
        put(app, device_id, reading, source, at)?;
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

/// Whether what the device's profile reads for `kind` has yet to be checked
/// against a real device of its platform. For saying so about a device that
/// has nothing stored yet; a stored document says it of itself
/// ([`Stored::unverified`]).
pub fn unverified(app: &App, device_id: &str, kind: Kind) -> AppResult<bool> {
    let device = inventory::get(app, device_id)?;
    Ok(profiles::platform(app, &device.profile)?
        .profile
        .is_unverified(kind))
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
    let readings: Vec<Reading> = Kind::ALL
        .into_iter()
        .filter_map(|kind| {
            snapshot.rows(kind).map(|rows| Reading {
                kind,
                rows,
                unverified: platform.profile.is_unverified(kind),
            })
        })
        .collect();
    let source = if cli.device.lab.is_some() {
        "lab"
    } else {
        "cli"
    };
    store_readings(app, &cli.device.id, readings, source)?;
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
    /// See [`Stored::unverified`]; absent when false.
    #[serde(skip_serializing_if = "is_false")]
    pub unverified: bool,
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
                unverified: stored.unverified,
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

    /// What Nocturne's simulator prints for an input policy with a policer
    /// (the sample `nocturne-drivers` keeps as
    /// `tests/profiles/cisco_ios/captures/qos-lab-policed-input`). Synthetic:
    /// no Cisco device printed it.
    const POLICED_INPUT: &str = " GigabitEthernet0/0\n\n  Service-policy input: LIMIT-USERS\n\n    Class-map: USERS (match-all)\n      7812 packets, 6250000 bytes\n      5 minute offered rate 164000 bps, drop rate 98000 bps\n      Match: access-group name USERS\n      police:\n          cir 2000000 bps, bc 62500 bytes\n        conformed 3125 packets, 2500000 bytes; actions:\n          transmit\n        exceeded 4687 packets, 3750000 bytes; actions:\n          drop\n        conformed 65000 bps, exceed 98000 bps\n\n    Class-map: class-default (match-any)\n      0 packets, 0 bytes\n      5 minute offered rate 0 bps, drop rate 0 bps\n      Match: any\n";

    /// Drive the fake device with the built-in IOS QoS read: the test profile
    /// plus `cisco_ios`'s own `collect.qos` steps, listed as unverified the
    /// way `cisco_ios` lists them.
    async fn read_qos_like_cisco_ios(app: &App, device_id: &str) {
        let ios = profiles::platform(app, "cisco_ios").unwrap();
        assert!(
            ios.profile.is_unverified(Kind::Qos),
            "this test is about a read that is listed as unverified"
        );
        let steps = serde_json::to_string(&ios.profile.collect[&Kind::Qos]).unwrap();
        // JSON is YAML, which spares the template a second round of quoting.
        let profile = format!(
            "id: test_ios_qos\nname: Test IOS with QoS\nextends: test_ios\nunverified: [qos]\ncollect: {{\"qos\": {steps}}}\n"
        );
        profiles::save(app, &profile).expect("the profile loads");
        app.rpc(
            "devices.update",
            json!({"id": device_id, "profile": "test_ios_qos"}),
        )
        .await
        .unwrap();
    }

    /// Run `collect.run` for one device and return its entry of the job's result.
    async fn collect_run(app: &App, device_id: &str, kinds: Value) -> Value {
        let started = app
            .rpc("collect.run", json!({"id": device_id, "kinds": kinds}))
            .await
            .unwrap();
        let job = jobs::wait(app, started["job_id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(job.state, jobs::JobState::Succeeded);
        job.result.unwrap()["devices"][0].clone()
    }

    #[tokio::test]
    async fn no_policy_no_answer_and_never_asked_are_three_different_qos_documents() {
        let lab = lab(builder(), &["R1"]).await;
        let (app, id) = (&lab.app, lab.id("R1"));
        let get = |previous: bool| {
            app.rpc(
                "collect.get",
                json!({"id": id, "kind": "qos", "previous": previous}),
            )
        };

        // A device type that has no QoS read: unsupported, and nothing stored.
        let outcome = collect_run(app, &id, json!(["qos"])).await;
        assert_eq!(outcome["collected"], json!([]));
        assert_eq!(outcome["problems"][0]["code"], "unsupported");
        assert_eq!(get(false).await.unwrap(), Value::Null);
        assert!(!unverified(app, &id, Kind::Qos).unwrap());

        read_qos_like_cisco_ios(app, &id).await;
        assert!(unverified(app, &id, Kind::Qos).unwrap());
        assert!(!unverified(app, &id, Kind::Facts).unwrap());

        // The device refuses the command. That is a failed read: it is in
        // the job's result, and there is still no document — not an empty one.
        let outcome = collect_run(app, &id, json!([])).await;
        assert!(
            outcome["collected"]
                .as_array()
                .unwrap()
                .iter()
                .all(|kind| kind != "qos"),
            "{outcome}"
        );
        let problem = outcome["problems"]
            .as_array()
            .unwrap()
            .iter()
            .find(|problem| problem["kind"] == "qos")
            .unwrap_or_else(|| panic!("a qos problem in {outcome}"));
        assert_eq!(problem["code"], "failed");
        assert!(
            problem["message"]
                .as_str()
                .unwrap()
                .contains("Invalid input"),
            "{problem}"
        );
        assert_eq!(get(false).await.unwrap(), Value::Null);

        // The device answers with nothing: no policy is attached. That is a
        // read, and it is stored — with no rows, and with the caveat.
        lab.fake("R1").with(|device| {
            device
                .outputs
                .insert("show policy-map interface".into(), String::new());
        });
        let outcome = collect_run(app, &id, json!([])).await;
        assert!(
            outcome["collected"]
                .as_array()
                .unwrap()
                .contains(&json!("qos")),
            "{outcome}"
        );
        let empty = get(false).await.unwrap();
        assert_eq!(empty["rows"], json!([]));
        assert_eq!(empty["unverified"], true);
        assert_eq!(empty["source"], "lab");
        assert_eq!(empty["kind"], "qos");

        // A policy is attached: one row per class.
        lab.fake("R1").with(|device| {
            device
                .outputs
                .insert("show policy-map interface".into(), POLICED_INPUT.into());
        });
        collect_run(app, &id, json!(["qos"])).await;
        let shown = get(false).await.unwrap();
        assert_eq!(shown["unverified"], true);
        assert_eq!(
            shown["rows"],
            json!([
                {
                    "interface": "GigabitEthernet0/0",
                    "direction": "in",
                    "policy": "LIMIT-USERS",
                    "class": "USERS",
                    "parent": null,
                    "rate_bps": 2_000_000,
                    "packets": 7812,
                    "bytes": 6_250_000,
                    "dropped_packets": 4687,
                    "dropped_bytes": 3_750_000,
                    "queue_dropped_packets": null,
                    "police_dropped_packets": 4687,
                    "queued_packets": null,
                    "offered_rate_bps": 164_000,
                    "drop_rate_bps": 98_000,
                },
                {
                    "interface": "GigabitEthernet0/0",
                    "direction": "in",
                    "policy": "LIMIT-USERS",
                    "class": "class-default",
                    "parent": null,
                    "rate_bps": null,
                    "packets": 0,
                    "bytes": 0,
                    "dropped_packets": null,
                    "dropped_bytes": null,
                    "queue_dropped_packets": null,
                    "police_dropped_packets": null,
                    "queued_packets": null,
                    "offered_rate_bps": 0,
                    "drop_rate_bps": 0,
                },
            ])
        );
        // The read before it — the empty one — is the previous generation.
        assert_eq!(get(true).await.unwrap()["rows"], json!([]));
        let status = app.rpc("collect.status", json!({"id": id})).await.unwrap();
        let qos = status
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["kind"] == "qos")
            .unwrap_or_else(|| panic!("qos in {status}"));
        assert_eq!(
            (&qos["rows"], &qos["unverified"]),
            (&json!(2), &json!(true))
        );

        // A kind the profile reads with a checked parser says nothing of the sort.
        let facts = app
            .rpc("collect.get", json!({"id": id, "kind": "facts"}))
            .await
            .unwrap();
        assert_eq!(facts["rows"][0]["hostname"], "R1");
        assert_eq!(facts.get("unverified"), None, "{facts}");
        // Neither do rows that were handed in rather than read through the profile.
        store(app, &id, Kind::Arp, vec![], "snmp").unwrap();
        let arp = app
            .rpc("collect.get", json!({"id": id, "kind": "arp"}))
            .await
            .unwrap();
        assert_eq!(arp.get("unverified"), None, "{arp}");

        // The device stops answering. The failure is reported, and what was
        // read before stays — it is not replaced by "no policy".
        lab.fake("R1").with(|device| {
            device.outputs.remove("show policy-map interface");
        });
        let outcome = collect_run(app, &id, json!(["qos"])).await;
        assert_eq!(outcome["collected"], json!([]));
        assert_eq!(outcome["problems"][0]["code"], "failed");
        assert_eq!(get(false).await.unwrap(), shown);

        // A document stored before the caveat existed still reads.
        let old: Stored = serde_json::from_value(json!({
            "device_id": id, "kind": "vlans", "collected_at": 1, "source": "cli", "rows": [],
        }))
        .unwrap();
        assert!(!old.unverified);
    }

    #[tokio::test]
    async fn the_list_of_device_types_says_which_of_their_reads_are_unverified() {
        let lab = lab(builder(), &["R1"]).await;
        let listed = lab.app.rpc("profiles.list", Value::Null).await.unwrap();
        let unverified = |id: &str| {
            listed["profiles"]
                .as_array()
                .expect("profiles")
                .iter()
                .find(|row| row["id"] == id)
                .unwrap_or_else(|| panic!("{id} in {listed}"))["unverified"]
                .clone()
        };
        // The two built-in profiles that read QoS say so about that read…
        assert_eq!(unverified("cisco_ios"), json!(["qos"]));
        assert_eq!(unverified("mikrotik_routeros"), json!(["qos"]));
        // …and a profile with nothing to say has an empty list, not a missing one.
        assert_eq!(unverified("test_ios"), json!([]));
        assert_eq!(unverified("cisco_nxos"), json!([]));
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
