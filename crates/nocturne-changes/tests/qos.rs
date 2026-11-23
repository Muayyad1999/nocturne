//! QoS through the change pipeline.
//!
//! There is no QoS write path of its own: a QoS change is configuration lines
//! through `changes.create`. What has to be right is that those lines plan,
//! that the plan can be applied in the order it gives, that a device which did
//! what was asked matches the prediction, and that the way back works.
//!
//! The router here is the lab simulator (`nocturne-netsim`), served to the
//! pipeline as a lab device the way the lab feature serves one: what Nocturne
//! types reaches its console, it is driven with the real `cisco_ios` device
//! type, and it refuses what IOS refuses — a class map or policy map that is
//! in use cannot be removed, a second policy cannot be attached in one
//! direction. The fake switch of `nocturne_app::testing` holds one level of
//! configuration only, so it carries the changes that fit in one.
//!
//! The template lines below are what the built-in QoS templates render
//! (`templates.render`), copied from the review of those templates with the
//! values a test needs put in by hand: a feature crate does not link another.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use nocturne_app::connect::{ConnectState, SimConnector};
use nocturne_app::model::LabRef;
use nocturne_app::testing::{self, Lab};
use nocturne_app::{App, AppError, AppResult};
use nocturne_netsim::Lab as SimLab;
use nocturne_transport::{Connection, ConnectionInfo, Input, Output, TransportKind};
use parking_lot::Mutex;
use serde_json::{Value, json};

// ------------------------------------------------------------ the simulator

/// Simulated routers, served to the pipeline as lab devices.
struct Simulator {
    lab: Arc<Mutex<SimLab>>,
}

#[async_trait::async_trait]
impl SimConnector for Simulator {
    async fn open(&self, _app: &App, lab: &LabRef, cols: u16, rows: u16) -> AppResult<Connection> {
        let console = {
            let mut sim = self.lab.lock();
            let node = sim
                .node_by_name(&lab.node)
                .ok_or_else(|| AppError::not_found(format!("lab node `{}`", lab.node)))?;
            let console = sim
                .console_open(node)
                .map_err(|err| AppError::unavailable(err.to_string()))?;
            sim.console_resize(console, cols.max(1), rows.max(1));
            console
        };
        let info = ConnectionInfo {
            kind: Some(TransportKind::Sim),
            peer: format!("lab:{}", lab.node),
            ..ConnectionInfo::default()
        };
        let (connection, mut backend) = Connection::pair(info, Connection::DEFAULT_BUFFER);
        let sim = self.lab.clone();
        tokio::spawn(async move {
            loop {
                let output = sim.lock().console_output(console);
                if !output.is_empty()
                    && backend
                        .output
                        .send(Output::Data(output.into()))
                        .await
                        .is_err()
                {
                    break;
                }
                tokio::select! {
                    input = backend.input.recv() => match input {
                        Some(Input::Data(bytes)) => {
                            let mut sim = sim.lock();
                            sim.console_input(console, &bytes);
                            // As the lab feature does: a sliver of simulated
                            // time, so what the typing started has moved.
                            sim.advance(2_000);
                        }
                        Some(Input::Break) => sim.lock().console_input(console, &[0x03]),
                        Some(Input::Resize { cols, rows }) => {
                            sim.lock().console_resize(console, cols.max(1), rows.max(1));
                        }
                        Some(Input::Close) | None => break,
                    },
                    // A console also speaks by itself (a log message).
                    _ = tokio::time::sleep(Duration::from_millis(10)) => sim.lock().advance(10_000),
                }
            }
            sim.lock().console_close(console);
            let _ = backend.output.send(Output::Closed { reason: None }).await;
        });
        Ok(connection)
    }
}

/// An app with the change pipeline and simulated routers as lab devices,
/// driven with the device type the lab feature gives them.
struct Routers {
    lab: Lab,
    sim: Arc<Mutex<SimLab>>,
    ids: Vec<(String, String)>,
}

impl Routers {
    fn app(&self) -> &App {
        &self.lab.app
    }

    fn id(&self, name: &str) -> String {
        self.ids
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("no router `{name}`"))
    }

    /// `show running-config`, read off the simulator itself.
    fn running(&self, name: &str) -> String {
        let sim = self.sim.lock();
        let node = sim.node_by_name(name).expect("the router");
        sim.running_config(node).expect("its configuration")
    }

    fn exec(&self, name: &str, command: &str) -> String {
        let mut sim = self.sim.lock();
        let node = sim.node_by_name(name).expect("the router");
        sim.exec(node, command).expect("the command")
    }
}

async fn routers(names: &[&str]) -> Routers {
    let mut builder = testing::builder();
    nocturne_changes::init(&mut builder);
    let lab = testing::lab(builder, &[]).await;
    let mut sim = SimLab::new("qos");
    for name in names {
        sim.add_node("router", Some(name), 0.0, 0.0)
            .expect("a router");
    }
    sim.settle(30_000_000);
    let sim = Arc::new(Mutex::new(sim));
    lab.app
        .state::<ConnectState>()
        .set_sim_connector(Arc::new(Simulator { lab: sim.clone() }));
    let mut ids = Vec::new();
    for name in names {
        let device = lab
            .app
            .rpc(
                "devices.create",
                json!({"name": name, "profile": "cisco_ios", "site": "Lab",
                       "lab": {"lab_id": "qos", "node": name}}),
            )
            .await
            .expect("a lab router");
        ids.push((name.to_string(), device["id"].as_str().expect("id").to_string()));
    }
    Routers { lab, sim, ids }
}

// ---------------------------------------------------------------- helpers

fn lines(lines: &[String]) -> Value {
    json!({"type": "lines", "lines": lines})
}

fn texts(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

/// Every step of the first target with its detail, for a failure message.
fn story(change: &Value) -> String {
    let result = &change["targets"][0]["result"];
    let mut out = String::new();
    for step in result["steps"].as_array().cloned().unwrap_or_default() {
        out.push_str(&format!(
            "\n  {} {} — {}",
            step["name"].as_str().unwrap_or_default(),
            if step["ok"] == true { "ok" } else { "FAILED" },
            step["detail"].as_str().unwrap_or_default()
        ));
    }
    out.push_str(&format!("\n  error: {}", result["error"]));
    out
}

/// Is the line at `path` (section headers first) in `config`?
fn has(config: &str, path: &[&str]) -> bool {
    let line = path.join(" / ");
    canonical(config).contains(&line)
}

/// Ask for the undo of a change and wait for it to be planned.
async fn undo_of(app: &App, id: &str) -> Value {
    let answer = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .unwrap_or_else(|err| panic!("changes.rollback: {err}"));
    let undo_id = answer["change"]["id"].as_str().expect("the undo").to_string();
    let planning = finish(app, &answer).await;
    assert_eq!(
        planning.state,
        nocturne_app::jobs::JobState::Succeeded,
        "the undo did not plan: {:?}",
        planning.error
    );
    app.rpc("changes.get", json!({"id": undo_id}))
        .await
        .expect("the undo")
}

/// The lines `ios_qos_wan_edge` renders for `interface: Gi0/1` with its
/// default markings, the given voice share and — when not zero — the circuit
/// rate the policy is shaped to.
fn wan_edge(voice_percent: u32, shape_kbps: u64) -> Vec<String> {
    let mut out = texts(&[
        "class-map match-any WAN-EDGE-VOICE",
        " match dscp ef",
        "class-map match-any WAN-EDGE-VIDEO",
        " match dscp af41 af42 af43",
        "class-map match-any WAN-EDGE-CRITICAL",
        " match dscp af21 af22 af23 cs3",
        "policy-map WAN-EDGE",
        " class WAN-EDGE-VOICE",
    ]);
    out.push(format!("  priority percent {voice_percent}"));
    out.extend(texts(&[
        "  exit",
        " class WAN-EDGE-VIDEO",
        "  bandwidth percent 20",
        "  exit",
        " class WAN-EDGE-CRITICAL",
        "  bandwidth percent 25",
    ]));
    if shape_kbps > 0 {
        out.extend([
            "policy-map WAN-EDGE-SHAPE".to_string(),
            " class class-default".to_string(),
            format!("  shape average {}", shape_kbps * 1000),
            "  service-policy WAN-EDGE".to_string(),
            "interface GigabitEthernet0/1".to_string(),
            " service-policy output WAN-EDGE-SHAPE".to_string(),
        ]);
    } else {
        out.extend(texts(&[
            "interface GigabitEthernet0/1",
            " service-policy output WAN-EDGE",
        ]));
    }
    out
}

/// `ios_qos_shaped_uplink` for `interface: Gi0/1`.
fn shaped_uplink(rate_kbps: u64) -> Vec<String> {
    vec![
        format!("policy-map SHAPE-{rate_kbps}K"),
        " class class-default".to_string(),
        format!("  shape average {}", rate_kbps * 1000),
        "interface GigabitEthernet0/1".to_string(),
        format!(" service-policy output SHAPE-{rate_kbps}K"),
    ]
}

// --------------------------------------------------- the router, for real

/// h2-qos-templates-and-sim-0: re-running a template with a new voice share
/// and a new circuit rate used to be rolled back every time, because the
/// prediction kept the old `priority` and `shape` line beside the new one.
#[tokio::test]
async fn a_qos_value_typed_again_replaces_the_old_one_and_the_change_is_applied() {
    let r = routers(&["R1"]).await;
    let app = r.app();
    let pristine = r.running("R1");

    let first = planned(app, &r.id("R1"), lines(&wan_edge(10, 50_000)), json!({})).await;
    let first = apply(app, &first).await;
    assert_eq!(target_state(&first), "applied", "{}", story(&first));
    let shaped = r.running("R1");

    let second = planned(app, &r.id("R1"), lines(&wan_edge(15, 40_000)), json!({})).await;
    let second = apply(app, &second).await;
    assert_eq!(target_state(&second), "applied", "{}", story(&second));
    let upgraded = r.running("R1");
    for (path, there) in [
        (
            vec!["policy-map WAN-EDGE", "class WAN-EDGE-VOICE", "priority percent 15"],
            true,
        ),
        (
            vec!["policy-map WAN-EDGE", "class WAN-EDGE-VOICE", "priority percent 10"],
            false,
        ),
        (
            vec!["policy-map WAN-EDGE-SHAPE", "class class-default", "shape average 40000000"],
            true,
        ),
        (
            vec!["policy-map WAN-EDGE-SHAPE", "class class-default", "shape average 50000000"],
            false,
        ),
    ] {
        assert_eq!(has(&upgraded, &path), there, "{path:?} in:\n{upgraded}");
    }

    // Undone on demand, each change takes the router back exactly.
    let undo = undo_of(app, second["id"].as_str().unwrap()).await;
    let undo = apply(app, &undo).await;
    assert_eq!(target_state(&undo), "applied", "{}", story(&undo));
    assert_eq!(canonical(&r.running("R1")), canonical(&shaped));
    let undo = undo_of(app, first["id"].as_str().unwrap()).await;
    let undo = apply(app, &undo).await;
    assert_eq!(target_state(&undo), "applied", "{}", story(&undo));
    assert_eq!(
        canonical(&r.running("R1")),
        canonical(&pristine),
        "nothing of the QoS change is left on the router"
    );
}

/// h2-qos-templates-and-sim-1: the way back removed the class maps before
/// the policy maps that use them, and a child policy before its parent; the
/// router refused those lines and the rollback failed with orphans left.
#[tokio::test]
async fn undoing_the_wan_policy_takes_it_off_first_and_removes_what_uses_before_what_is_used() {
    let r = routers(&["R1"]).await;
    let app = r.app();
    let pristine = r.running("R1");
    let change = planned(app, &r.id("R1"), lines(&wan_edge(10, 50_000)), json!({})).await;
    let change = apply(app, &change).await;
    assert_eq!(target_state(&change), "applied", "{}", story(&change));

    let undo = undo_of(app, change["id"].as_str().unwrap()).await;
    let undo_lines = commands(&undo);
    let at = |line: &str| {
        undo_lines
            .iter()
            .position(|sent| sent == line)
            .unwrap_or_else(|| panic!("`{line}` is not in {undo_lines:#?}"))
    };
    assert!(at("no service-policy output WAN-EDGE-SHAPE") < at("no policy-map WAN-EDGE-SHAPE"));
    assert!(at("no policy-map WAN-EDGE-SHAPE") < at("no policy-map WAN-EDGE"));
    for map in ["VOICE", "VIDEO", "CRITICAL"] {
        assert!(at("no policy-map WAN-EDGE") < at(&format!("no class-map match-any WAN-EDGE-{map}")));
    }
    let undo = apply(app, &undo).await;
    assert_eq!(target_state(&undo), "applied", "{}", story(&undo));
    let after = r.running("R1");
    assert_eq!(canonical(&after), canonical(&pristine), "{after}");
    assert!(!after.contains("class-map") && !after.contains("policy-map"), "{after}");
    assert_eq!(
        r.exec("R1", "show policy-map interface").trim(),
        "",
        "nothing is attached any more"
    );
}

/// The same, for the rollback inside an apply: a WAN policy whose checks
/// fail is taken off again, every class map and policy map with it.
#[tokio::test]
async fn a_wan_policy_that_fails_its_checks_is_rolled_back_without_leaving_anything_behind() {
    let r = routers(&["R1"]).await;
    let app = r.app();
    let pristine = r.running("R1");
    let change = planned(
        app,
        &r.id("R1"),
        lines(&wan_edge(10, 50_000)),
        // Nothing answers there: the check fails after the policy is on.
        json!({"verify": {"pings": ["192.0.2.250"]}}),
    )
    .await;
    let change = apply(app, &change).await;
    assert_eq!(target_state(&change), "rolled_back", "{}", story(&change));
    let after = r.running("R1");
    assert_eq!(canonical(&after), canonical(&pristine), "{after}");
}

/// h2-qos-templates-and-sim-7, the predictor's half: `no match dscp V` takes
/// one value out of a statement that lists several, as IOS does; the
/// prediction used to keep the statement whole, so the change was rolled
/// back from a router that had done what was asked.
#[tokio::test]
async fn taking_one_marking_out_of_a_match_statement_is_predicted_as_the_router_does_it() {
    let r = routers(&["R1"]).await;
    let app = r.app();
    let change = planned(app, &r.id("R1"), lines(&wan_edge(10, 0)), json!({})).await;
    let change = apply(app, &change).await;
    assert_eq!(target_state(&change), "applied", "{}", story(&change));
    let before = r.running("R1");

    let narrow = planned(
        app,
        &r.id("R1"),
        lines(&texts(&["class-map match-any WAN-EDGE-VIDEO", " no match dscp af42"])),
        json!({}),
    )
    .await;
    let narrow = apply(app, &narrow).await;
    assert_eq!(target_state(&narrow), "applied", "{}", story(&narrow));
    let after = r.running("R1");
    assert!(
        has(&after, &["class-map match-any WAN-EDGE-VIDEO", "match dscp af41 af43"]),
        "{after}"
    );

    let undo = undo_of(app, narrow["id"].as_str().unwrap()).await;
    let undo = apply(app, &undo).await;
    assert_eq!(target_state(&undo), "applied", "{}", story(&undo));
    assert_eq!(canonical(&r.running("R1")), canonical(&before));
}

/// Hand-typed lines are sent as typed; the prediction is made the way the
/// router prints them back — a marking by name, a statement's values in
/// ascending order, a class named from inside another one moving to it —
/// so a router that did what was asked is not rolled back.
#[tokio::test]
async fn markings_in_any_spelling_and_classes_without_exit_verify_on_the_router() {
    let r = routers(&["R1"]).await;
    let app = r.app();
    let typed = texts(&[
        "class-map match-any PHONES",
        " match dscp ef 44",
        "class-map match-any SIGNALLING",
        " match dscp 24",
        "policy-map MARK",
        " class PHONES",
        "  set dscp 46",
        " class SIGNALLING",
        "  set dscp 24",
        " class class-default",
        "  set dscp 0",
        "interface GigabitEthernet0/0",
        " service-policy input MARK",
    ]);
    let change = planned(app, &r.id("R1"), lines(&typed), json!({})).await;
    assert_eq!(
        commands(&change),
        typed.iter().map(|line| line.trim().to_string()).collect::<Vec<_>>(),
        "what is sent is what was typed"
    );
    let change = apply(app, &change).await;
    assert_eq!(target_state(&change), "applied", "{}", story(&change));
    let after = r.running("R1");
    for path in [
        vec!["class-map match-any PHONES", "match dscp 44 ef"],
        vec!["class-map match-any SIGNALLING", "match dscp cs3"],
        vec!["policy-map MARK", "class PHONES", "set dscp ef"],
        vec!["policy-map MARK", "class SIGNALLING", "set dscp cs3"],
        vec!["policy-map MARK", "class class-default", "set dscp default"],
    ] {
        assert!(has(&after, &path), "{path:?} in:\n{after}");
    }
}

// ------------------------------------------------------------- vocabulary

/// A class or policy name is a name, whatever case it is in and whatever
/// keyword it happens to look like.
#[tokio::test]
async fn qos_names_in_lower_case_are_names_not_keywords() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let preview = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": lines(&texts(&[
                "class-map match-any voice",
                " match dscp ef",
                "class-map match-all int",
                " match dscp cs3",
                "policy-map voice",
                " class voice",
                "  priority percent 10",
                " class int",
                "  bandwidth percent 5",
                "interface GigabitEthernet0/2",
                " service-policy output voice",
                " service-policy input int",
            ]))}),
        )
        .await
        .unwrap_or_else(|err| panic!("the lines are refused: {err}"));
    let reasons = preview["risk"]["reasons"].to_string();
    for name in ["voice", "int"] {
        assert!(
            !reasons.contains(&format!("`{name}`")),
            "`{name}` is read as a word: {reasons}"
        );
    }
}
