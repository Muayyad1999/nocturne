//! `monitor.ifindex`: which ifIndex is which interface, for the features that
//! decode traps and flows. What the agent said, under the names the poller
//! uses, kept across a restart — and no map at all for a device SNMP never
//! read.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nocturne_app::{AppBuilder, AppConfig};
use nocturne_snmp::{Auth, Oid, VarBind};
use serde_json::{Value, json};

use crate::clock::{Clock, VirtualClock};
use crate::snmp::{SnmpSession, SnmpTransport, TableRow};
use crate::state::MonitorState;
use crate::testing::{AgentData, AgentIf, FakeProbe, FakeSnmp, Harness, epoch};

const MINUTE: Duration = Duration::from_secs(60);

/// An agent with these interfaces, in ifIndex order from 1.
fn agent(names: &[&str]) -> AgentData {
    AgentData {
        uptime_ticks: Some(360_000),
        counter64: true,
        interfaces: names.iter().map(|name| AgentIf::up(name, 1_000)).collect(),
        ..AgentData::default()
    }
}

async fn map_of(harness: &Harness, device_id: &str) -> Value {
    harness
        .rpc("monitor.ifindex", json!({"device_id": device_id}))
        .await
}

#[tokio::test(start_paused = true)]
async fn the_map_is_what_the_agent_said_under_the_names_the_poller_uses() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    // The third row is one the agent gives no name.
    let id = harness
        .snmp_device("SW1", "10.0.0.1", agent(&["Gi0/1", "Gi0/2", "", "Vlan10"]))
        .await;
    harness.poll(&id).await;

    let map = map_of(&harness, &id).await;
    assert_eq!(
        map,
        json!({"at": harness.now(), "interfaces": [
            {"index": 1, "name": "GigabitEthernet0/1"},
            {"index": 2, "name": "GigabitEthernet0/2"},
            {"index": 4, "name": "Vlan10"},
        ]})
    );

    // Every name is one the interface table and the metrics go by, so a
    // trap's interface can be looked up there. Should the poller ever spell
    // its interfaces differently, this is the assertion that says the map
    // has to follow.
    let table = harness
        .rpc("monitor.interfaces", json!({"device_id": id}))
        .await;
    let on_screen: Vec<&str> = table
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    for entry in map["interfaces"].as_array().unwrap() {
        assert!(
            on_screen.contains(&entry["name"].as_str().unwrap()),
            "{entry} is not an interface the poller knows: {on_screen:?}"
        );
    }
    assert!(
        on_screen.contains(&"ifIndex 3"),
        "the nameless row has a label on the screen, which is not a name to hand to anybody: {on_screen:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_never_read_over_snmp_has_no_map_rather_than_an_empty_one() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;

    // Polled, but over ICMP alone.
    let pinged = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.poll(&pinged).await;
    let never = map_of(&harness, &pinged).await;
    assert_eq!(never, json!({"interfaces": []}));
    assert!(never.get("at").is_none(), "no time, because it never was");

    // SNMP switched on, and an agent that does not answer.
    let silent = harness
        .snmp_device(
            "SW2",
            "10.0.0.2",
            AgentData {
                fail: Some("request timed out".into()),
                ..agent(&["Gi0/1"])
            },
        )
        .await;
    harness.poll(&silent).await;
    assert_eq!(map_of(&harness, &silent).await, json!({"interfaces": []}));

    // A lab device is read over its console.
    let lab = harness
        .device(
            "LAB-R1",
            "",
            json!({"lab": {"lab_id": "lab1", "node": "R1"}}),
        )
        .await;
    assert_eq!(map_of(&harness, &lab).await, json!({"interfaces": []}));

    // An agent that answers and has no interfaces is a different answer:
    // read, at this time, and empty.
    let bare = harness.snmp_device("SW3", "10.0.0.3", agent(&[])).await;
    harness.poll(&bare).await;
    assert_eq!(
        map_of(&harness, &bare).await,
        json!({"at": harness.now(), "interfaces": []})
    );

    let error = harness
        .app
        .rpc("monitor.ifindex", json!({"device_id": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
}

#[tokio::test(start_paused = true)]
async fn the_map_is_read_again_when_something_says_it_moved_and_otherwise_every_five_minutes() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device("SW1", "10.0.0.1", agent(&["Gi0/1", "Gi0/2"]))
        .await;
    harness.poll(&id).await;
    let first = map_of(&harness, &id).await;
    assert_eq!(
        first["interfaces"][0],
        json!({"index": 1, "name": "GigabitEthernet0/1"})
    );

    // The agent renumbers with nothing to show for it — which a real one
    // may only do when it restarts. The next poll reads the same names and
    // the same uptime, so the map stands, and `at` goes on saying how old
    // it is.
    harness
        .snmp
        .edit("10.0.0.1:161", |agent| agent.interfaces.reverse());
    tokio::time::sleep(MINUTE).await;
    harness.poll(&id).await;
    assert_eq!(map_of(&harness, &id).await, first);

    // It does not stand for ever.
    tokio::time::sleep(4 * MINUTE).await;
    harness.poll(&id).await;
    assert_eq!(
        map_of(&harness, &id).await,
        json!({"at": harness.now(), "interfaces": [
            {"index": 1, "name": "GigabitEthernet0/2"},
            {"index": 2, "name": "GigabitEthernet0/1"},
        ]})
    );

    // A restart is when indexes move, and the poll sees one: the uptime
    // went backwards. Asked at once.
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.interfaces.reverse();
        agent.uptime_ticks = Some(3_000);
    });
    tokio::time::sleep(MINUTE).await;
    assert!(harness.poll(&id).await.rebooted);
    assert_eq!(
        map_of(&harness, &id).await,
        json!({"at": harness.now(), "interfaces": first["interfaces"]})
    );

    // So is a port that was not there before.
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.interfaces.push(AgentIf::up("Te1/1", 10_000));
        agent.uptime_ticks = Some(9_000);
    });
    tokio::time::sleep(MINUTE).await;
    harness.poll(&id).await;
    let grown = map_of(&harness, &id).await;
    assert_eq!(grown["at"], json!(harness.now()));
    assert_eq!(
        grown["interfaces"][2],
        json!({"index": 3, "name": "TenGigabitEthernet1/1"})
    );
}

/// What a test can make the agents do to the one request the index map
/// sends — the two name columns of the interface table — and to no other.
#[derive(Default)]
struct Meddling {
    /// The request is not answered.
    failing: AtomicBool,
    /// The answer has one row more than the poll's own walk had: a port
    /// that came up in between.
    late_row: AtomicBool,
    /// The application's events, for a test that wants to know what the
    /// poll had already published when the request arrived.
    watching: parking_lot::Mutex<Option<tokio::sync::broadcast::Receiver<nocturne_app::Event>>>,
    /// Whether `monitor.polled` was among them.
    polled_before_asked: AtomicBool,
}

/// An SNMP transport in front of the fake agents that meddles as told.
struct MeddlingSnmp {
    agents: Arc<FakeSnmp>,
    meddling: Arc<Meddling>,
}

struct MeddlingSession {
    session: Box<dyn SnmpSession>,
    meddling: Arc<Meddling>,
}

#[async_trait]
impl SnmpTransport for MeddlingSnmp {
    async fn open(
        &self,
        target: std::net::SocketAddr,
        auth: Auth,
        timeout: Duration,
    ) -> Result<Box<dyn SnmpSession>, String> {
        Ok(Box::new(MeddlingSession {
            session: self.agents.open(target, auth, timeout).await?,
            meddling: self.meddling.clone(),
        }))
    }
}

#[async_trait]
impl SnmpSession for MeddlingSession {
    async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, String> {
        self.session.get(oids).await
    }

    async fn table(&mut self, columns: &[Oid], limit: usize) -> Result<Vec<TableRow>, String> {
        // The poll's own walk asks for thirteen columns or more and a
        // health OID for one; only the index map asks for exactly two.
        if columns.len() != 2 {
            return self.session.table(columns, limit).await;
        }
        if let Some(events) = self.meddling.watching.lock().as_mut() {
            let mut polled = false;
            while let Ok(event) = events.try_recv() {
                polled |= event.topic == "monitor.polled";
            }
            self.meddling
                .polled_before_asked
                .store(polled, Ordering::SeqCst);
        }
        if self.meddling.failing.load(Ordering::SeqCst) {
            return Err("request timed out".into());
        }
        let mut rows = self.session.table(columns, limit).await?;
        if self.meddling.late_row.load(Ordering::SeqCst) {
            rows.push(TableRow {
                index: vec![9],
                values: vec![(columns[0].clone(), nocturne_snmp::Value::from("Gi0/9"))],
            });
        }
        Ok(rows)
    }

    fn supports_counter64(&self) -> bool {
        self.session.supports_counter64()
    }
}

/// A harness whose SNMP goes through [`MeddlingSnmp`].
async fn meddled_harness() -> (Harness, Arc<Meddling>) {
    let probe = Arc::new(FakeProbe::default());
    let agents = Arc::new(FakeSnmp::default());
    let meddling = Arc::new(Meddling::default());
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock::new(epoch()));
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        clock.clone(),
        probe.clone(),
        Arc::new(MeddlingSnmp {
            agents: agents.clone(),
            meddling: meddling.clone(),
        }),
    ));
    let lab = nocturne_app::testing::lab(builder, &[]).await;
    let harness = Harness {
        app: lab.app,
        probe,
        snmp: agents,
        clock,
        credential_id: lab.credential_id,
    };
    harness.with_snmp_credential("public").await;
    (harness, meddling)
}

#[tokio::test(start_paused = true)]
async fn a_map_that_cannot_be_read_stays_as_it_was_and_costs_the_poll_nothing() {
    let (harness, meddling) = meddled_harness().await;
    let id = harness
        .snmp_device("SW1", "10.0.0.1", agent(&["Gi0/1", "Gi0/2"]))
        .await;
    harness.poll(&id).await;
    let first = map_of(&harness, &id).await;
    assert_eq!(first["interfaces"].as_array().unwrap().len(), 2);

    // A third port appears, which is a reason to ask again — and the agent
    // does not answer the question.
    meddling.failing.store(true, Ordering::SeqCst);
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.interfaces.push(AgentIf::up("Gi0/3", 1_000))
    });
    tokio::time::sleep(MINUTE).await;
    let poll = harness.poll(&id).await;
    assert!(poll.ok && poll.sources.snmp.ok == Some(true), "{poll:?}");
    assert_eq!(poll.interfaces.len(), 3, "the poll itself read all three");
    assert_eq!(
        map_of(&harness, &id).await,
        first,
        "the last thing the agent said, with the time it said it; nothing made up for the third"
    );

    // Nor is an agent that will not answer asked again on every poll: a
    // request that fails throws the session away, so each one would show
    // as a session opened.
    for _ in 0..3 {
        tokio::time::sleep(MINUTE).await;
        harness.poll(&id).await;
    }
    assert_eq!(
        harness.snmp.opens("10.0.0.1:161"),
        2,
        "the first, and one after the failed read"
    );

    meddling.failing.store(false, Ordering::SeqCst);
    tokio::time::sleep(2 * MINUTE).await;
    harness.poll(&id).await;
    let healed = map_of(&harness, &id).await;
    assert_eq!(healed["at"], json!(harness.now()));
    assert_eq!(
        healed["interfaces"][2],
        json!({"index": 3, "name": "GigabitEthernet0/3"})
    );
}

#[tokio::test(start_paused = true)]
async fn the_map_is_asked_for_after_the_poll_has_said_what_it_found() {
    // One more request to an agent is one more wait if the agent has just
    // stopped answering, and it is for other features. So it comes last:
    // the device's state, its samples and `monitor.polled` are out before
    // it is sent, and none of them waits for it.
    let (harness, meddling) = meddled_harness().await;
    let id = harness
        .snmp_device("SW1", "10.0.0.1", agent(&["Gi0/1"]))
        .await;
    *meddling.watching.lock() = Some(harness.app.subscribe());
    harness.poll(&id).await;
    assert_eq!(
        map_of(&harness, &id).await["interfaces"],
        json!([{"index": 1, "name": "GigabitEthernet0/1"}]),
        "it was asked for, and answered"
    );
    assert!(
        meddling.polled_before_asked.load(Ordering::SeqCst),
        "the poll had not yet said it was done when the map was asked for"
    );
}

#[tokio::test(start_paused = true)]
async fn a_row_the_poll_did_not_read_is_not_given_a_name_by_the_map_alone() {
    // The map is asked for after the poll's own walk of the table, so the
    // two can differ by a port that came up in between. A name the poll
    // does not know is a name `monitor.interfaces` and the metrics do not
    // have: whoever looked it up there would find nothing. It waits for the
    // poll that reads the port.
    let (harness, meddling) = meddled_harness().await;
    meddling.late_row.store(true, Ordering::SeqCst);
    let id = harness
        .snmp_device("SW1", "10.0.0.1", agent(&["Gi0/1", "Gi0/2"]))
        .await;
    harness.poll(&id).await;
    assert_eq!(
        map_of(&harness, &id).await,
        json!({"at": harness.now(), "interfaces": [
            {"index": 1, "name": "GigabitEthernet0/1"},
            {"index": 2, "name": "GigabitEthernet0/2"},
        ]})
    );
}

#[tokio::test]
async fn the_map_goes_when_its_device_does() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device("SW1", "10.0.0.1", agent(&["Gi0/1", "Gi0/2"]))
        .await;
    harness.poll(&id).await;
    assert!(
        crate::ifindex::stored(&harness.app, &id)
            .unwrap()
            .at
            .is_some()
    );

    crate::scheduler::spawn(&harness.app);
    harness.rpc("devices.delete", json!({"ids": [id]})).await;
    // Forgotten when the feature hears of the deletion on the event bus:
    // waited for with a deadline, not slept through.
    let deadline = super::listeners::soon();
    while crate::ifindex::stored(&harness.app, &id)
        .unwrap()
        .at
        .is_some()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the map of a deleted device was never forgotten"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_map_is_still_there_after_a_restart_before_the_device_is_polled_again() {
    let directory = tempfile::tempdir().unwrap();
    let start = |agents: Arc<FakeSnmp>| {
        let mut builder = AppBuilder::new(AppConfig::at(directory.path())).expect("an app on disk");
        nocturne_app::init(&mut builder);
        crate::init(&mut builder);
        builder.state(MonitorState::fake(
            Arc::new(VirtualClock::new(epoch())),
            Arc::new(FakeProbe::default()),
            agents,
        ));
        builder.build()
    };

    let (id, before) = {
        let agents = Arc::new(FakeSnmp::default());
        agents.set("10.0.0.1:161", agent(&["Gi0/1", "Gi0/2"]));
        let app = start(agents);
        let credential = app
            .rpc(
                "credentials.create",
                json!({"name": "snmp", "snmp_version": "v2c", "snmp_community": "public"}),
            )
            .await
            .unwrap();
        let device = app
            .rpc(
                "devices.create",
                json!({"name": "SW1", "address": "10.0.0.1", "transport": "none",
                       "credential_id": credential["id"],
                       "monitor": {"enabled": true, "icmp": true, "snmp": true}}),
            )
            .await
            .unwrap();
        let id = device["id"].as_str().unwrap().to_string();
        crate::poll::poll_device(&app, &id).await.unwrap();
        let before = app
            .rpc("monitor.ifindex", json!({"device_id": id}))
            .await
            .unwrap();
        assert_eq!(before["interfaces"].as_array().unwrap().len(), 2);
        app.shutdown();
        (id, before)
    };
    // Shutting down is asynchronous: the data directory is released once
    // the background tasks have had their turn.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The application again, with nothing polled yet and no agent to ask:
    // a trap that arrives now can still be given its interface.
    let app = start(Arc::new(FakeSnmp::default()));
    let after = app
        .rpc("monitor.ifindex", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(after, before);
    assert!(after["at"].is_i64());
}
