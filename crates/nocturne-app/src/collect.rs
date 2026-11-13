//! Structured device data: facts, interfaces, neighbours, VLANs, MAC and ARP
//! tables, routes, health and hardware inventory.
//!
//! Whatever reads a device — the CLI collectors, SNMP polling, the lab
//! simulator — stores what it read here, one document per device and kind,
//! and everything that reasons about the network (topology, IPAM, insights,
//! the assistant) reads it back from here. The previous generation is kept so
//! that "what changed since last time" can be answered.

use nocturne_drivers::model::{Facts, Kind};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, AppBuilder};
use crate::error::{AppError, AppResult};
use crate::inventory;
use crate::util::now_ms;

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
}
