//! Secrets: a configuration is full of them, a change is made of
//! configuration lines, and none of it may leave the engine by itself.
//!
//! The device configurations in this file are synthetic: written by hand for
//! these tests, not recorded from a device. Every secret in them is a marker
//! string that occurs nowhere else, so finding one anywhere outside the engine
//! is a leak and nothing else.
//!
//! What is searched is everything that leaves: every `changes.*` answer made
//! with default parameters, every error, every published event (the change
//! pipeline's own and the job engine's), the job records with their logs, and
//! the audit trail with its details.

mod common;

use std::sync::Arc;

use common::*;
use nocturne_app::jobs::{Job, JobState};
use nocturne_app::{App, AppResult, Event};
use parking_lot::Mutex;
use serde_json::{Value, json};

// ------------------------------------------------- what must never be seen

/// Already on the device, beside the lines the changes below touch.
const ENABLE_HASH: &str = "$1$mERr$N0ctEnab1eS3cretHashXYZ";
const COMMUNITY: &str = "N0ctComm-R0";
const TYPE7: &str = "0822455D0A16544541";
const OSPF_KEY: &str = "N0ctOspfKey99";
const VTY_PASSWORD: &str = "N0ctVtyPlainPw";
const BGP_TYPE7: &str = "104D000A0618110F2F";

/// Put on the device by somebody else in the middle of a change.
const INTRUDER_COMMUNITY: &str = "N0ctIntruderRW";
const INTRUDER_HASH: &str = "$1$xYzQ$IntruderEnab1eHashABC";

/// Carried by a change's own lines.
const NEW_COMMUNITY: &str = "N0ctNewComm-RW";
const NEW_OSPF_KEY: &str = "N0ctNewOspfKey7";
const NEW_HASH: &str = "$1$nEwS$PlannedEnab1eHashDEF";
/// Lower-case letters and nothing else: only the word in front of it says
/// that it is a secret, and in a transcript a prompt stands in front of that.
const NEW_CONSOLE_PASSWORD: &str = "lowercaseconsolepw";

const SECRETS: &[&str] = &[
    ENABLE_HASH,
    COMMUNITY,
    TYPE7,
    OSPF_KEY,
    VTY_PASSWORD,
    BGP_TYPE7,
    INTRUDER_COMMUNITY,
    INTRUDER_HASH,
    NEW_COMMUNITY,
    NEW_OSPF_KEY,
    NEW_HASH,
    NEW_CONSOLE_PASSWORD,
];

/// Synthetic. `BASE_CONFIG` with an enable secret, a type 7 password and a
/// community next to the domain name, a routing key next to the description
/// of a desk port, and a line password under the VTYs.
fn secret_config() -> String {
    format!(
        "\
hostname SW1
ip domain-name lab.example
enable secret 5 {ENABLE_HASH}
username backup password 7 {TYPE7}
snmp-server community {COMMUNITY} RO
vlan 10
 name users
vlan 20
 name voice
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
interface GigabitEthernet0/2
 description desk 12
 ip ospf message-digest-key 1 md5 {OSPF_KEY}
 switchport mode access
 switchport access vlan 10
interface Vlan1
 ip address 10.0.0.2 255.255.255.0
ip default-gateway 10.0.0.1
line vty 0 4
 password {VTY_PASSWORD}
 transport input ssh
"
    )
}

/// A change that holds no secret itself: it renames a port and the domain,
/// and each of those lines has a secret within three lines of it.
fn beside_the_secrets() -> Value {
    json!({"type": "lines", "lines": [
        "ip domain-name corp.example",
        "interface GigabitEthernet0/2",
        "description desk 14",
        "exit",
    ]})
}

/// A change whose own lines are secrets.
fn made_of_secrets() -> Value {
    json!({"type": "lines", "lines": [
        format!("enable secret 5 {NEW_HASH}"),
        format!("snmp-server community {NEW_COMMUNITY} RW"),
        "interface GigabitEthernet0/2",
        format!("ip ospf message-digest-key 2 md5 {NEW_OSPF_KEY}"),
        "exit",
        "line con 0",
        format!("password {NEW_CONSOLE_PASSWORD}"),
        "exit",
    ]})
}

// ------------------------------------------------------ what left the engine

/// Collects what leaves the engine while a test runs.
struct Outside {
    events: Arc<Mutex<Vec<Event>>>,
    lagged: Arc<Mutex<u64>>,
    /// (where it was seen, what was seen).
    seen: Mutex<Vec<(String, String)>>,
}

impl Outside {
    /// Start listening to the event stream. Call before anything is done.
    fn watch(app: &App) -> Outside {
        let events = Arc::new(Mutex::new(Vec::new()));
        let lagged = Arc::new(Mutex::new(0));
        let mut stream = app.subscribe();
        let (into, missed) = (events.clone(), lagged.clone());
        tokio::spawn(async move {
            loop {
                match stream.recv().await {
                    Ok(event) => into.lock().push(event),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        *missed.lock() += count;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        Outside {
            events,
            lagged,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, place: impl Into<String>, what: &impl serde::Serialize) {
        self.seen.lock().push((
            place.into(),
            serde_json::to_string(what).expect("serializable"),
        ));
    }

    /// Call a method and keep whatever came back, answer or error.
    async fn call(&self, app: &App, method: &str, params: Value) -> AppResult<Value> {
        let outcome = app.rpc(method, params).await;
        match &outcome {
            Ok(value) => self.note(format!("the answer of {method}"), value),
            Err(err) => self.note(format!("the error of {method}"), err),
        }
        outcome
    }

    /// Start a job through `method`, wait for it, and keep its record.
    async fn job(&self, app: &App, method: &str, params: Value) -> Job {
        let started = self
            .call(app, method, params)
            .await
            .unwrap_or_else(|err| panic!("{method}: {err}"));
        let job = finish(app, &started).await;
        self.note(format!("the job record of {method}"), &job);
        // And as anybody else would read it.
        let read = app
            .rpc("jobs.get", json!({"id": job.id}))
            .await
            .expect("jobs.get");
        self.note(format!("jobs.get after {method}"), &read);
        job
    }

    /// Every place one of `secrets` was seen outside the engine.
    async fn leaks(&self, app: &App, secrets: &[&str]) -> Vec<String> {
        // Let the listener take what is still queued for it.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            *self.lagged.lock(),
            0,
            "events were dropped before they were looked at"
        );
        let mut places: Vec<(String, String)> = self.seen.lock().clone();
        for event in self.events.lock().iter() {
            places.push((
                format!("the event {}", event.topic),
                serde_json::to_string(event).expect("an event"),
            ));
        }
        let audit = app
            .store()
            .log_query(&nocturne_app::store::LogQuery {
                stream: Some("audit".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .expect("the audit trail");
        for entry in audit {
            places.push((
                format!("the audit entry {} ({})", entry.source, entry.message),
                serde_json::to_string(&entry).expect("an audit entry"),
            ));
        }
        let mut found = Vec::new();
        for (place, text) in &places {
            for secret in secrets {
                if text.contains(secret) {
                    found.push(format!("{secret} in {place}: {}", around(text, secret)));
                }
            }
        }
        found.sort();
        found.dedup();
        found
    }
}

/// A few characters either side of the first occurrence, for the failure message.
fn around(text: &str, secret: &str) -> String {
    let at = text.find(secret).unwrap_or(0);
    let start = text[..at]
        .char_indices()
        .rev()
        .nth(60)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let end = text[at..]
        .char_indices()
        .nth(secret.len() + 40)
        .map(|(index, _)| at + index)
        .unwrap_or(text.len());
    format!("…{}…", &text[start..end])
}

fn no_leaks(found: Vec<String>) {
    assert!(
        found.is_empty(),
        "{} secrets left the engine:\n{}",
        found.len(),
        found.join("\n")
    );
}

/// Create a change, plan it and read it back, all through `outside`.
async fn planned_through(outside: &Outside, app: &App, device_id: &str, intent: Value) -> Value {
    planned_with(outside, app, device_id, intent, json!({})).await
}

async fn planned_with(
    outside: &Outside,
    app: &App,
    device_id: &str,
    intent: Value,
    options: Value,
) -> Value {
    let created = outside
        .call(
            app,
            "changes.create",
            json!({
                "title": "secrets test",
                "targets": [{"device_id": device_id, "intent": intent}],
                "options": options,
            }),
        )
        .await
        .expect("create");
    let id = created["id"].as_str().expect("id").to_string();
    let job = outside.job(app, "changes.plan", json!({"id": id})).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .expect("get")
}

/// The change as it is kept, which is not what anybody is shown.
fn stored(app: &App, id: &str) -> nocturne_changes::Change {
    app.store()
        .get::<nocturne_changes::Change>("changes", id)
        .expect("the store")
        .expect("the change")
}

/// What an apply wrote down is kept without the values: it is not merely
/// hidden each time it is read. (The lines of the plan are another matter —
/// they are kept whole, because they are what is sent.)
fn nothing_recorded_whole(app: &App, id: &str) {
    nothing_of_these_recorded_whole(app, id, SECRETS);
}

fn nothing_of_these_recorded_whole(app: &App, id: &str, secrets: &[&str]) {
    for target in &stored(app, id).targets {
        let result = target.result.as_ref().expect("a result");
        for (what, recorded) in [
            ("the steps", serde_json::to_string(&result.steps).unwrap()),
            (
                "the sent lines",
                serde_json::to_string(&result.sent).unwrap(),
            ),
            ("the error", serde_json::to_string(&result.error).unwrap()),
            (
                "the checks",
                serde_json::to_string(&result.verification).unwrap(),
            ),
            ("the transcript", result.transcript.clone()),
        ] {
            for secret in secrets {
                assert!(
                    !recorded.contains(secret),
                    "{secret} is kept in {what}: {}",
                    around(&recorded, secret)
                );
            }
        }
        assert!(
            !result.transcript.is_empty(),
            "the session was kept, so there was something to hide"
        );
        assert!(
            result.redacted,
            "and the record says that it holds no value"
        );
    }
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn nothing_secret_leaves_when_verification_fails_and_the_change_is_rolled_back() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let before = lab.fake("SW1").running_config();
    let outside = Outside::watch(app);

    // What would this do? — the box a person types into before anything exists.
    outside
        .call(
            app,
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": beside_the_secrets()}),
        )
        .await
        .expect("preview");

    // A check whose detail quotes the device: a ping Nocturne cannot read the
    // answer of. This device answers with a line of its configuration.
    lab.fake("SW1").with(|state| {
        state.outputs.insert(
            "ping 192.0.2.50".into(),
            format!("Translating...\nusername backup password 7 {TYPE7}\n"),
        );
    });
    let change = planned_with(
        &outside,
        app,
        &lab.id("SW1"),
        beside_the_secrets(),
        json!({"verify": {"pings": ["192.0.2.50"]}}),
    )
    .await;
    let id = change["id"].as_str().unwrap().to_string();
    // The test is about the neighbourhood of the change: its own lines hold nothing.
    assert_eq!(
        commands(&change),
        vec![
            "ip domain-name corp.example",
            "interface GigabitEthernet0/2",
            "description desk 14",
            "exit"
        ]
    );
    // …and the neighbourhood is in the predicted difference, hidden.
    let context: Vec<&str> = plan(&change)["diff"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| line["text"].as_str())
        .collect();
    assert!(
        context
            .iter()
            .any(|line| line.starts_with("enable secret 5 <secret:")),
        "the diff still shows that there is an enable secret beside the change: {context:#?}"
    );
    assert!(
        context
            .iter()
            .any(|line| line.contains("message-digest-key 1 md5 <secret:")),
        "{context:#?}"
    );
    outside.call(app, "changes.list", json!({})).await.unwrap();

    let approved = outside
        .call(
            app,
            "changes.approve",
            json!({"id": id, "plan_hashes": [plan(&change)["plan_hash"]]}),
        )
        .await
        .expect("approve");
    assert_eq!(approved["state"], "approved");

    // Somebody else changes the enable secret and adds a community after the
    // drift check and before the change is sent: the device is then not what
    // the plan predicted, and the lines that differ are secrets.
    let fake = lab.fake("SW1").clone();
    when_step(app, "backup", move || {
        fake.with(|state| {
            state
                .apply(&[
                    &format!("enable secret 5 {INTRUDER_HASH}"),
                    &format!("snmp-server community {INTRUDER_COMMUNITY} RW"),
                ])
                .expect("the intruder's lines")
        });
    });
    let job = outside.job(app, "changes.apply", json!({"id": id})).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let after = outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .unwrap();
    outside.call(app, "changes.list", json!({})).await.unwrap();

    // It failed for the reason this test is about, and it was undone.
    assert_eq!(target_state(&after), "rolled_back", "{after:#}");
    let result = &after["targets"][0]["result"];
    let check = result["verification"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "configuration")
        .expect("the configuration check");
    assert_eq!(check["ok"], false);
    let detail = check["detail"].as_str().unwrap();
    // The detail still says which lines differ — it only does not say the values.
    assert!(
        detail.contains("enable secret 5 <hidden>")
            && detail.contains("snmp-server community <hidden> RW"),
        "{detail}"
    );
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("snmp-server community <hidden> RW"),
        "{result:#}"
    );
    let ping = result["verification"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "ping 192.0.2.50")
        .expect("the ping check");
    assert!(
        ping["detail"]
            .as_str()
            .unwrap()
            .ends_with("username backup password 7 <hidden>"),
        "what the device answered is quoted without the value: {ping}"
    );
    // The lines derived on the spot to finish the rollback were sent whole
    // (the device is back as it was) and are recorded with the values hidden.
    assert_eq!(
        canonical(&lab.fake("SW1").running_config()),
        canonical(&before),
        "the rollback put the intruder's lines back too"
    );
    let sent: Vec<&str> = result["sent"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| line["command"].as_str())
        .collect();
    assert!(
        sent.contains(&"enable secret 5 <hidden>"),
        "the rollback's own commands are in the record, hidden: {sent:#?}"
    );
    assert!(
        lab.fake("SW1")
            .seen()
            .contains(&format!("enable secret 5 {ENABLE_HASH}")),
        "and the device was sent the real line"
    );

    nothing_recorded_whole(app, &id);
    no_leaks(outside.leaks(app, SECRETS).await);
}

#[tokio::test]
async fn nothing_secret_leaves_when_a_change_made_of_secrets_is_applied_and_later_undone() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let outside = Outside::watch(app);

    outside
        .call(
            app,
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": made_of_secrets()}),
        )
        .await
        .expect("preview");
    let change = planned_through(&outside, app, &lab.id("SW1"), made_of_secrets()).await;
    let id = change["id"].as_str().unwrap().to_string();
    assert!(
        hit_kinds(&change).contains(&"access_enable".to_string()),
        "the enable secret is a protected resource: {:?}",
        hit_kinds(&change)
    );

    // Editing the title returns the change again.
    outside
        .call(
            app,
            "changes.update",
            json!({"id": id, "title": "secrets test, renamed"}),
        )
        .await
        .expect("update");

    // An approval without the typed name is refused, and the refusal lists
    // the protected resources — with the commands that touch them.
    let hash = plan(&change)["plan_hash"].clone();
    let refused = outside
        .call(
            app,
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash]}),
        )
        .await
        .expect_err("no typed confirmation");
    assert_eq!(refused.code, "forbidden");
    let approved = outside
        .call(
            app,
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash], "confirm": {"SW1": "SW1"}}),
        )
        .await
        .expect("approve");
    assert_eq!(approved["state"], "approved");
    outside
        .call(app, "changes.unapprove", json!({"id": id}))
        .await
        .expect("unapprove");
    outside
        .call(
            app,
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash], "confirm": {"SW1": "SW1"}}),
        )
        .await
        .expect("approve again");

    let job = outside.job(app, "changes.apply", json!({"id": id})).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let applied = outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(target_state(&applied), "applied", "{applied:#}");

    // The device got the real lines…
    let running = lab.fake("SW1").running_config();
    for secret in [NEW_HASH, NEW_COMMUNITY, NEW_OSPF_KEY, NEW_CONSOLE_PASSWORD] {
        assert!(running.contains(secret), "{secret} is not on the device");
    }
    // …and the record of what was sent names them without their values.
    let sent: Vec<&str> = applied["targets"][0]["result"]["sent"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| line["command"].as_str())
        .collect();
    assert_eq!(
        sent,
        vec![
            "enable secret 5 <hidden>",
            "snmp-server community <hidden> RW",
            "interface GigabitEthernet0/2",
            "ip ospf message-digest-key 2 md5 <hidden>",
            "exit",
            "line con 0",
            "password <hidden>",
            "exit",
        ]
    );
    // The session was kept too: the device echoed every one of those lines
    // behind a prompt, and printed its configuration twice.
    let transcript = applied["targets"][0]["result"]["transcript"]
        .as_str()
        .unwrap();
    assert!(
        transcript.contains("(config-line)#password <hidden>"),
        "{transcript}"
    );
    assert!(
        !transcript.contains("<secret:"),
        "a transcript carries no number: its table is not kept"
    );
    nothing_recorded_whole(app, &id);

    // Undoing it on demand. The undo is a change of its own, made of the
    // whole configuration the device had before — every secret it held — and
    // planned from what it holds now: its answers, its planning job and its
    // audit entries are swept like the rest.
    let job = outside
        .job(app, "changes.rollback", json!({"id": id}))
        .await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let original = outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .unwrap();
    let undo_id = original["undone_by"]
        .as_str()
        .expect("the original names its undo")
        .to_string();
    let undo = outside
        .call(app, "changes.get", json!({"id": undo_id}))
        .await
        .unwrap();
    assert_eq!(undo["state"], "planned", "{undo:#}");
    assert!(
        undo["targets"][0]["intent"]["config"]
            .as_str()
            .unwrap()
            .contains("enable secret 5 <secret:"),
        "the configuration it puts back is shown with its values hidden"
    );
    assert!(undo["hidden"].as_u64().unwrap() >= 5, "{}", undo["hidden"]);
    outside.call(app, "changes.list", json!({})).await.unwrap();

    // On a device that refuses the line that would put the old enable secret
    // back: the undo's own report quotes that line.
    lab.fake("SW1")
        .with(|state| state.reject.push("enable secret".into()));
    outside
        .call(
            app,
            "changes.approve",
            json!({"id": undo_id, "plan_hashes": [plan(&undo)["plan_hash"]], "confirm": {"SW1": "SW1"}}),
        )
        .await
        .expect("approve the undo");
    let job = outside
        .job(app, "changes.apply", json!({"id": undo_id}))
        .await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let undone = outside
        .call(app, "changes.get", json!({"id": undo_id}))
        .await
        .unwrap();
    assert!(
        matches!(
            target_state(&undone).as_str(),
            "rolled_back" | "rollback_failed"
        ),
        "{undone:#}"
    );
    let steps: Vec<&str> = undone["targets"][0]["result"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["detail"].as_str())
        .collect();
    assert!(
        steps
            .iter()
            .any(|detail| detail.contains("the device rejected `enable secret 5 <hidden>`")),
        "the report still says which line was refused: {steps:#?}"
    );
    nothing_recorded_whole(app, &undo_id);
    // The undo did not go through, so the change it was for still stands.
    let original = outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(target_state(&original), "applied", "{original:#}");
    nothing_recorded_whole(app, &id);

    for deleted in [&undo_id, &id] {
        outside
            .call(app, "changes.delete", json!({"id": deleted}))
            .await
            .expect("delete");
    }

    no_leaks(outside.leaks(app, SECRETS).await);
}

#[tokio::test]
async fn a_refused_line_is_named_without_the_secret_in_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let outside = Outside::watch(app);

    // A top-level line typed inside an open section is refused at planning,
    // and the refusal quotes the line.
    let misplaced = json!({"type": "lines", "lines": [
        "interface GigabitEthernet0/2",
        "description desk 14",
        format!("enable secret 5 {NEW_HASH}"),
    ]});
    let refused = outside
        .call(
            app,
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": misplaced}),
        )
        .await
        .expect_err("a global line in an open section");
    assert_eq!(refused.code, "invalid");
    assert!(
        refused.message.contains("`enable secret 5 <hidden>`"),
        "the refusal still names the line: {}",
        refused.message
    );
    // The same through a planning job, whose error is kept in the job record,
    // its log and the audit trail.
    let created = outside
        .call(
            app,
            "changes.create",
            json!({
                "title": "misplaced",
                "targets": [{"device_id": lab.id("SW1"), "intent": misplaced}],
            }),
        )
        .await
        .expect("create");
    let job = outside
        .job(app, "changes.plan", json!({"id": created["id"]}))
        .await;
    assert_eq!(job.state, JobState::Failed);

    // A line the device itself rejects while the change is being applied.
    lab.fake("SW1")
        .with(|state| state.reject.push("message-digest-key 2".into()));
    let change = planned_through(&outside, app, &lab.id("SW1"), made_of_secrets()).await;
    let id = change["id"].as_str().unwrap().to_string();
    outside
        .call(
            app,
            "changes.approve",
            json!({"id": id, "plan_hashes": [plan(&change)["plan_hash"]], "confirm": {"SW1": "SW1"}}),
        )
        .await
        .expect("approve");
    let job = outside.job(app, "changes.apply", json!({"id": id})).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let after = outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(target_state(&after), "rolled_back", "{after:#}");
    let error = after["targets"][0]["result"]["error"].as_str().unwrap();
    assert!(
        error.contains("the device rejected `ip ospf message-digest-key 2 md5 <hidden>`"),
        "{error}"
    );

    nothing_recorded_whole(app, &id);
    no_leaks(outside.leaks(app, SECRETS).await);
}

#[tokio::test]
async fn the_exact_plan_is_shown_on_request_and_the_request_is_audited() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let change = planned(app, &lab.id("SW1"), made_of_secrets(), json!({})).await;
    let id = change["id"].as_str().unwrap().to_string();

    // By default: placeholders, and the answer says so.
    assert_eq!(change["redacted"], true);
    assert_eq!(
        commands(&change),
        vec![
            "enable secret 5 <secret:1>",
            "snmp-server community <secret:2> RW",
            "interface GigabitEthernet0/2",
            "ip ospf message-digest-key 2 md5 <secret:3>",
            "exit",
            "line con 0",
            "password <secret:4>",
            "exit",
        ]
    );
    // The same value has the same number wherever it stands in the answer:
    // the community is added by the plan and removed by its rollback.
    assert!(
        rollback(&change)
            .iter()
            .any(|line| line.starts_with("no snmp-server community <secret:2>")),
        "{:?}",
        rollback(&change)
    );
    assert!(
        change["hidden"].as_u64().unwrap() >= 5,
        "the four values of the plan and at least the enable secret it replaces: {}",
        change["hidden"]
    );
    assert!(
        audit(app)
            .iter()
            .all(|(source, _)| source != "changes.view_unredacted"),
        "a redacted read is not an unredacted one"
    );

    // On request: the lines exactly as they will be sent.
    let whole = app
        .rpc("changes.get", json!({"id": id, "redact": false}))
        .await
        .expect("the unredacted read");
    assert_eq!(whole["redacted"], false);
    assert_eq!(
        commands(&whole),
        vec![
            format!("enable secret 5 {NEW_HASH}"),
            format!("snmp-server community {NEW_COMMUNITY} RW"),
            "interface GigabitEthernet0/2".to_string(),
            format!("ip ospf message-digest-key 2 md5 {NEW_OSPF_KEY}"),
            "exit".to_string(),
            "line con 0".to_string(),
            format!("password {NEW_CONSOLE_PASSWORD}"),
            "exit".to_string(),
        ]
    );
    assert_eq!(whole["hidden"], 0);
    let text = whole.to_string();
    for secret in [NEW_HASH, NEW_COMMUNITY, NEW_OSPF_KEY, ENABLE_HASH] {
        assert!(
            text.contains(secret),
            "{secret} is part of the plan (its commands, its rollback or its predicted difference)"
        );
    }
    assert!(
        !text.contains("<secret:"),
        "nothing is hidden in an unredacted read"
    );

    // …and that somebody asked is on the record, without the secrets.
    let entries = app
        .store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            limit: Some(500),
            ..Default::default()
        })
        .unwrap();
    let viewed: Vec<_> = entries
        .iter()
        .filter(|entry| entry.source == "changes.view_unredacted")
        .collect();
    assert_eq!(viewed.len(), 1, "one unredacted read, one entry");
    assert!(
        viewed[0].message.contains("secrets test") || viewed[0].message.contains("test change"),
        "{}",
        viewed[0].message
    );
    assert_eq!(viewed[0].body.as_ref().unwrap()["change_id"], id.as_str());
    let recorded = serde_json::to_string(&viewed[0]).unwrap();
    for secret in SECRETS {
        assert!(!recorded.contains(secret), "{recorded}");
    }

    // The preview has the same switch, and the same record.
    let preview = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": made_of_secrets()}),
        )
        .await
        .unwrap();
    assert_eq!(preview["redacted"], true);
    assert!(!preview.to_string().contains(NEW_COMMUNITY));
    let preview = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": made_of_secrets(), "redact": false}),
        )
        .await
        .unwrap();
    assert_eq!(preview["redacted"], false);
    assert_eq!(
        preview["commands"][1],
        format!("snmp-server community {NEW_COMMUNITY} RW")
    );
    assert_eq!(
        audit(app)
            .iter()
            .filter(|(source, _)| source == "changes.view_unredacted")
            .count(),
        2
    );
}

#[tokio::test]
async fn hiding_the_values_changes_neither_the_hash_nor_what_is_kept_and_sent() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let shown = planned(app, &lab.id("SW1"), made_of_secrets(), json!({})).await;
    let id = shown["id"].as_str().unwrap().to_string();
    let whole = app
        .rpc("changes.get", json!({"id": id, "redact": false}))
        .await
        .unwrap();

    // One plan, one hash, however it is looked at — and it is the hash of the
    // lines that will be sent, not of what is shown.
    let kept = stored(app, &id);
    let kept_plan = kept.targets[0].plan.as_ref().expect("the stored plan");
    assert_eq!(plan(&shown)["plan_hash"], kept_plan.plan_hash.as_str());
    assert_eq!(plan(&whole)["plan_hash"], kept_plan.plan_hash.as_str());
    assert_eq!(
        kept_plan.commands,
        commands(&whole),
        "what is kept is what will be sent"
    );
    assert_ne!(kept_plan.commands, commands(&shown));
    assert_eq!(kept_plan.rollback, rollback(&whole));

    // Reading the change — either way — writes nothing back.
    let again = stored(app, &id);
    assert_eq!(again, kept);

    // The approval is given on the hash a person saw beside the hidden lines,
    // and it is the real lines that reach the device.
    let applied = apply(app, &shown).await;
    assert_eq!(target_state(&applied), "applied", "{applied:#}");
    let seen = lab.fake("SW1").seen();
    for line in &kept_plan.commands {
        assert!(seen.contains(line), "`{line}` was not sent as planned");
    }
    assert!(
        seen.iter()
            .all(|line| !line.contains("<secret:") && !line.contains("<hidden>")),
        "a placeholder was sent to the device: {seen:#?}"
    );
    // And the plan is still kept whole after the apply recorded its result.
    let kept_after = stored(app, &id);
    assert_eq!(kept_after.targets[0].plan.as_ref(), Some(kept_plan));
    assert_eq!(
        kept_after.approval.as_ref().unwrap().plan_hashes,
        vec![kept_plan.plan_hash.clone()]
    );
}

#[tokio::test]
async fn what_a_plan_says_about_its_lines_is_kept_without_their_values() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(
        &lab,
        "SW1",
        &format!(
            "{}router bgp 65000\n neighbor 192.0.2.1 remote-as 65001\n neighbor 192.0.2.1 password 7 {BGP_TYPE7}\n",
            secret_config()
        ),
    );
    let outside = Outside::watch(app);
    // Taking a neighbour's password away is a routing removal: a protected
    // resource, and the reason it is one quotes the line.
    let change = planned_through(
        &outside,
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "router bgp 65000",
            format!("no neighbor 192.0.2.1 password 7 {BGP_TYPE7}"),
            "exit",
        ]}),
    )
    .await;
    let id = change["id"].as_str().unwrap().to_string();
    assert_eq!(hit_kinds(&change), vec!["routing_removal"]);

    // As it is kept: the line whole, because it is what will be sent, and
    // the sentence about it without the value, because it is only ever read.
    let kept = stored(app, &id);
    let kept_plan = kept.targets[0].plan.as_ref().unwrap();
    assert_eq!(
        kept_plan.commands[1],
        format!("no neighbor 192.0.2.1 password 7 {BGP_TYPE7}")
    );
    let hit = &kept_plan.protected_hits[0];
    assert_eq!(hit.command, kept_plan.commands[1]);
    assert_eq!(
        hit.why,
        "This removes `neighbor 192.0.2.1 password 7 <hidden>` from `router bgp 65000`: the routes it carried disappear."
    );
    let said =
        serde_json::to_string(&(&kept_plan.warnings, &kept_plan.risk, &hit.why, &hit.subject))
            .unwrap();
    for secret in SECRETS {
        assert!(!said.contains(secret), "{secret} in {said}");
    }

    // As it is shown: the line with a placeholder, the sentence as it is kept.
    let shown_hit = &plan(&change)["protected_hits"][0];
    assert_eq!(
        shown_hit["command"],
        "no neighbor 192.0.2.1 password 7 <secret:1>"
    );
    assert_eq!(shown_hit["why"], hit.why.as_str());

    // The assistant's own apply is refused for it, in the plan's words.
    app.rpc(
        "settings.set",
        json!({"section": "assistant", "value": {"autonomy": "low_risk"}}),
    )
    .await
    .expect("settings");
    outside
        .call(app, "changes.assistant_apply", json!({"id": id}))
        .await
        .expect_err("a protected resource always needs a person");

    no_leaks(outside.leaks(app, SECRETS).await);
}

#[tokio::test]
async fn a_line_copied_from_a_redacted_view_is_refused_rather_than_sent() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let before = lab.fake("SW1").running_config();

    for placeholder in ["<secret:2>", "<secret:2:weak>", "<hidden>"] {
        let pasted = json!({"type": "lines", "lines": [
            format!("snmp-server community {placeholder} RW"),
        ]});
        let err = app
            .rpc(
                "changes.create",
                json!({"title": "pasted", "targets": [{"device_id": lab.id("SW1"), "intent": pasted}]}),
            )
            .await
            .expect_err("a placeholder is not a value");
        assert_eq!(err.code, "invalid", "{err}");
        assert!(err.message.contains("redact"), "{}", err.message);
        let err = app
            .rpc(
                "changes.preview",
                json!({"device_id": lab.id("SW1"), "intent": pasted}),
            )
            .await
            .expect_err("a placeholder is not a value");
        assert_eq!(err.code, "invalid", "{err}");
    }

    // The way a screen would do it by accident: read a change, change one
    // line, send the lines back.
    let change = planned(app, &lab.id("SW1"), made_of_secrets(), json!({})).await;
    let mut lines = change["targets"][0]["intent"]["lines"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(lines[1], "snmp-server community <secret:2> RW");
    lines.push(json!("ntp server 192.0.2.1"));
    let err = app
        .rpc(
            "changes.update",
            json!({"id": change["id"], "targets": [{"device_id": lab.id("SW1"), "intent": {"type": "lines", "lines": lines}}]}),
        )
        .await
        .expect_err("the lines came from a redacted read");
    assert_eq!(err.code, "invalid", "{err}");
    // The change is as it was, and nothing reached the device.
    let kept = stored(app, change["id"].as_str().unwrap());
    assert!(kept.targets[0].plan.is_some(), "the plan was not discarded");
    assert_eq!(lab.fake("SW1").running_config(), before);

    // An intended configuration is held to the same rule: a numbered
    // placeholder is something only Nocturne writes…
    let err = app
        .rpc(
            "changes.create",
            json!({"title": "pasted", "targets": [{"device_id": lab.id("SW1"), "intent":
                {"type": "intended", "config": "snmp-server community <secret:1> RO\n", "exact": false}}]}),
        )
        .await
        .expect_err("a placeholder is not a value");
    assert_eq!(err.code, "invalid", "{err}");
    // …and a `<hidden>` is refused where it would be sent…
    let err = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent":
                {"type": "intended", "config": "snmp-server community <hidden> RO\n", "exact": false}}),
        )
        .await
        .expect_err("the line would be sent as it stands");
    assert_eq!(err.code, "invalid", "{err}");
    assert!(err.message.contains("placeholder"), "{}", err.message);
    // …but not where it stands on a line that is not going to change: a
    // configuration read from a device that masks its own secrets can still
    // be the intended one.
    load(
        &lab,
        "SW1",
        &secret_config().replace(" description uplink\n", " description <hidden>\n"),
    );
    let preview = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": {"type": "intended", "exact": false, "config":
                "interface GigabitEthernet0/1\n description <hidden>\ninterface GigabitEthernet0/2\n description desk 14\n"}}),
        )
        .await
        .expect("nothing that would be sent holds a placeholder");
    let sent: Vec<&str> = preview["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(sent.contains(&"description desk 14"), "{sent:?}");
    assert!(
        sent.iter().all(|line| !line.contains("<hidden>")),
        "{sent:?}"
    );
}

#[tokio::test]
async fn one_value_has_one_number_on_every_device_and_each_device_shown_whole_is_on_record() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    // SW2 and SW3 keep `BASE_CONFIG`, which holds no secret at all.
    let community = json!({"type": "lines", "lines": [
        format!("snmp-server community {NEW_COMMUNITY} RW"),
    ]});
    let desk = json!({"type": "lines", "lines": [
        "interface GigabitEthernet0/2",
        "description desk 14",
        "exit",
    ]});
    let created = app
        .rpc(
            "changes.create",
            json!({"title": "one key, two switches", "targets": [
                {"device_id": lab.id("SW1"), "intent": community},
                {"device_id": lab.id("SW2"), "intent": community},
                {"device_id": lab.id("SW3"), "intent": desk},
            ]}),
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    run_job(app, "changes.plan", json!({"id": id})).await;

    // The same community on two switches reads as the same value.
    let shown = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    let first = |index: usize| shown["targets"][index]["plan"]["commands"][0].clone();
    assert_eq!(first(0), "snmp-server community <secret:1> RW");
    assert_eq!(first(1), first(0));
    assert_eq!(first(2), "interface GigabitEthernet0/2");

    // Shown whole: one entry for each device whose lines hold a value, in
    // that device's own trail, and none for the one that holds nothing.
    let whole = app
        .rpc("changes.get", json!({"id": id, "redact": false}))
        .await
        .unwrap();
    assert_eq!(
        whole["targets"][1]["plan"]["commands"][0],
        format!("snmp-server community {NEW_COMMUNITY} RW")
    );
    let viewed = |app: &nocturne_app::App| -> Vec<(Option<String>, Value)> {
        app.store()
            .log_query(&nocturne_app::store::LogQuery {
                stream: Some("audit".into()),
                limit: Some(500),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .filter(|entry| entry.source == "changes.view_unredacted")
            .map(|entry| (entry.device_id, entry.body.unwrap_or_default()))
            .collect()
    };
    let mut devices: Vec<Option<String>> =
        viewed(app).into_iter().map(|(device, _)| device).collect();
    devices.sort();
    let mut expected = vec![Some(lab.id("SW1")), Some(lab.id("SW2"))];
    expected.sort();
    assert_eq!(devices, expected);

    // A change in which nothing is hidden, asked for whole: still on record.
    let clean = planned(app, &lab.id("SW3"), desk, json!({})).await;
    assert_eq!(clean["redacted"], true);
    assert_eq!(clean["hidden"], 0, "what is shown is what will be sent");
    app.rpc("changes.get", json!({"id": clean["id"], "redact": false}))
        .await
        .unwrap();
    let entries = viewed(app);
    assert_eq!(entries.len(), 3, "{entries:?}");
    assert!(
        entries.iter().any(|(device, body)| device.is_none()
            && body["change_id"] == clean["id"]
            && body["values"] == 0),
        "{entries:?}"
    );
}

#[tokio::test]
async fn a_record_written_before_values_were_hidden_is_read_without_them() {
    use nocturne_changes::model::{
        Change, ChangeState, Check, Intent, Plan, ProtectedHit, Risk, Step, Target, TargetResult,
        TargetState,
    };
    use nocturne_drivers::hier::{DiffLine, TextDiff};
    use nocturne_drivers::session::CommandOutput;

    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    // What an apply used to write down: the lines that differed, the line the
    // device refused and the session, as they came.
    let differed = format!(
        "the running configuration is not what the plan predicted — missing from the device: `enable secret 5 {ENABLE_HASH}`; on the device but not predicted: `line vty 0 4 / password {VTY_PASSWORD}`"
    );
    let command = format!("snmp-server community {NEW_COMMUNITY} RW");
    let old = Change {
        id: "written-before".into(),
        title: "written before".into(),
        state: ChangeState::RolledBack,
        targets: vec![Target {
            device_id: lab.id("SW1"),
            intent: Intent::Lines {
                lines: vec![command.clone()],
            },
            plan: Some(Plan {
                commands: vec![command.clone()],
                rollback: vec![format!("no {command}")],
                diff: TextDiff {
                    lines: vec![
                        DiffLine {
                            tag: " ".into(),
                            text: format!("username backup password 7 {TYPE7}"),
                            old_line: Some(4),
                            new_line: Some(4),
                        },
                        DiffLine {
                            tag: "+".into(),
                            text: command.clone(),
                            old_line: None,
                            new_line: Some(5),
                        },
                    ],
                    added: 1,
                    removed: 0,
                },
                risk: Risk {
                    reasons: vec![format!(
                        "it removes whole sections (`no username backup password 7 {TYPE7}`)"
                    )],
                    ..Risk::default()
                },
                protected_hits: vec![ProtectedHit {
                    kind: "routing_removal".into(),
                    subject: "router bgp 65000".into(),
                    command: format!("no neighbor 192.0.2.1 password {OSPF_KEY}"),
                    why: format!(
                        "This removes `neighbor 192.0.2.1 password {OSPF_KEY}` from `router bgp 65000`: the routes it carried disappear."
                    ),
                }],
                warnings: vec![format!(
                    "Nocturne cannot predict what `{command}` changes, so the configuration after this change cannot be fully checked."
                )],
                plan_hash: "the-old-hash".into(),
                profile: VERIFIED_PROFILE.into(),
                maturity: "verified".into(),
                ..Plan::default()
            }),
            result: Some(TargetResult {
                state: TargetState::RolledBack,
                steps: vec![
                    // What a device logged while the change was made: the
                    // value stands where no rule can tell it is one, and only
                    // the change's own lines say what it is.
                    Step {
                        name: "apply".into(),
                        ok: true,
                        detail: format!(
                            "%SNMP-3-AUTHFAIL: bad community {NEW_COMMUNITY} from 192.0.2.9"
                        ),
                        at: 1,
                    },
                    Step {
                        name: "verify".into(),
                        ok: false,
                        detail: format!("configuration: {differed}"),
                        at: 2,
                    },
                ],
                sent: vec![CommandOutput {
                    command: command.clone(),
                    output: String::new(),
                    error: Some("% Invalid input detected at '^' marker.".into()),
                    elapsed_ms: 1,
                }],
                failed_line: Some(0),
                error: Some(format!(
                    "the device rejected `{command}`: % Invalid input detected at '^' marker."
                )),
                verification: vec![Check {
                    name: "configuration".into(),
                    ok: false,
                    detail: differed.clone(),
                }],
                transcript: format!(
                    "SW1(config)#{command}\r\nSW1(config)#line con 0\r\nSW1(config-line)#password {NEW_CONSOLE_PASSWORD}\r\n"
                ),
                ..TargetResult::default()
            }),
        }],
        ..Change::default()
    };
    app.store().put("changes", &old.id, &old).unwrap();

    // By default nothing of it is shown.
    let shown = app.rpc("changes.get", json!({"id": old.id})).await.unwrap();
    let text = shown.to_string();
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} in {shown:#}");
    }
    assert_eq!(
        shown["targets"][0]["result"]["verification"][0]["detail"],
        "the running configuration is not what the plan predicted — missing from the device: `enable secret 5 <hidden>`; on the device but not predicted: `line vty 0 4 / password <hidden>`"
    );
    assert_eq!(
        shown["targets"][0]["result"]["steps"][0]["detail"],
        "%SNMP-3-AUTHFAIL: bad community <hidden> from 192.0.2.9"
    );
    assert_eq!(
        shown["targets"][0]["plan"]["protected_hits"][0]["why"],
        "This removes `neighbor 192.0.2.1 password <hidden>` from `router bgp 65000`: the routes it carried disappear."
    );
    assert!(!old.targets[0].result.as_ref().unwrap().redacted);
    assert_eq!(
        shown["targets"][0]["result"]["redacted"], true,
        "what is answered holds no value, whatever the record says of itself"
    );

    // Asked for whole, the lines are whole — and what was said about them and
    // what was recorded while applying them still holds no value.
    let whole = app
        .rpc("changes.get", json!({"id": old.id, "redact": false}))
        .await
        .unwrap();
    let target = &whole["targets"][0];
    assert_eq!(target["plan"]["commands"][0], command.as_str());
    assert_eq!(
        target["plan"]["protected_hits"][0]["command"],
        format!("no neighbor 192.0.2.1 password {OSPF_KEY}")
    );
    let said = json!([
        target["result"],
        target["plan"]["warnings"],
        target["plan"]["risk"],
        target["plan"]["protected_hits"][0]["why"],
    ])
    .to_string();
    for secret in SECRETS {
        assert!(!said.contains(secret), "{secret} in {said}");
    }

    // Reading it did not rewrite it.
    assert_eq!(stored(app, &old.id), old);
}

// ------------------------------------------- the same, in other grammars

/// A configuration in a grammar other than IOS, hidden with the patterns of
/// the device type that ships for it (see `styled_profile`). Synthetic, like
/// every configuration in this file.
///
/// None of these grammars is modelled, so a change to them cannot be
/// predicted: the configuration check fails by design, the engine derives a
/// rollback from the device, and on the fake (which keeps a `remove` or a
/// `delete` as a line of its own) that rollback cannot finish. So every one
/// of them ends with a report of the lines that are still different — and a
/// value of a flat configuration may have ` / ` in it, which is also what
/// separates a line from its section in such a report.
struct Style {
    profile: &'static str,
    syntax: &'static str,
    /// The built-in device type whose `config.secrets` apply.
    secrets_of: &'static str,
    config: &'static str,
    lines: &'static [&'static str],
    /// A line the rollback's report must still name, its value hidden.
    named: &'static str,
    /// Every value in the configuration and in the change, and each half of
    /// the ones with a ` / ` in them.
    secrets: &'static [&'static str],
}

const ROUTEROS_STYLE: Style = Style {
    profile: "test_routeros_style",
    syntax: "routeros",
    secrets_of: "mikrotik_routeros",
    config: r#"/interface bridge add name=bridge
/ip address add address=10.0.0.2/24 interface=bridge
/snmp community add addresses=10.0.0.0/8 comment="noc / monitoring" name=R0sBaseComm7
/ppp secret add comment="branch a / b" name=branch password="r0sbase / pw44" service=pppoe
/ip ipsec identity add peer=hq secret=R0sBaseIpsec1
"#,
    lines: &[
        r#"/ppp secret add comment="vpn / roaming" name=u1 password="qqvv / zzkk" service=pppoe"#,
        r#"/interface wifi security add name=sec1 passphrase="qqhome / qqoffice""#,
        "/snmp community add addresses=10.1.0.0/16 name=R0sNewComm42",
    ],
    named: r#"/ppp secret add comment="vpn / roaming" name=u1 password="<hidden>" service=pppoe"#,
    secrets: &[
        "R0sBaseComm7",
        "r0sbase",
        "pw44",
        "R0sBaseIpsec1",
        "qqvv",
        "zzkk",
        "qqhome",
        "qqoffice",
        "R0sNewComm42",
    ],
};

const JUNOS_STYLE: Style = Style {
    profile: "test_junos_style",
    syntax: "set",
    secrets_of: "juniper_junos",
    config: r#"set system host-name SW1
set system root-authentication encrypted-password "$6$JnBase01$BaseRootHashAbCdEf0123"
set snmp community "j0sbase / c0mmx" authorization read-only
set system radius-server 10.0.0.5 secret "$9$J0sRadiusBase9"
set interfaces ge-0/0/0 unit 0 family inet address 10.0.0.2/24
"#,
    lines: &[
        r#"set system login user ops authentication encrypted-password "$6$JnNew001$NewOpsHashGhIjKl4567""#,
        r#"set snmp community "jjqq / kkww" authorization read-only"#,
        r#"set protocols bgp group ext authentication-key "$9$J0sNewBgpKey3""#,
    ],
    named: r#"set snmp community "<hidden>" authorization read-only"#,
    secrets: &[
        "JnBase01",
        "BaseRootHashAbCdEf0123",
        "j0sbase",
        "c0mmx",
        "J0sRadiusBase9",
        "JnNew001",
        "NewOpsHashGhIjKl4567",
        "jjqq",
        "kkww",
        "J0sNewBgpKey3",
    ],
};

/// A file of `key=value` lines, read with the patterns of the Linux device
/// type (which itself has no running configuration to plan against).
const KEY_VALUE_STYLE: Style = Style {
    profile: "test_key_value_style",
    syntax: "flat",
    secrets_of: "linux",
    config: r#"hostname=SW1
password=L1nBasePw01
PSK="l1nbase / psk9"
token=L1nBaseTok9
"#,
    lines: &[
        r#"psk="llqq / mmzz""#,
        "api_key = LnNewApiKey77",
        "secret: lnnewsecret",
    ],
    // The Linux type's rule for `secret: …` reads the engine's own marker
    // as one, and takes the closing quote with it; nothing of the value is
    // left either way.
    named: r#"psk="<hidden>"#,
    secrets: &[
        "L1nBasePw01",
        "l1nbase",
        "psk9",
        "L1nBaseTok9",
        "llqq",
        "mmzz",
        "LnNewApiKey77",
        "lnnewsecret",
    ],
};

async fn nothing_secret_leaves_a_change_written_in(style: &Style) {
    let lab = lab_with(
        &["SW1"],
        &[styled_profile(
            style.profile,
            style.syntax,
            style.secrets_of,
        )],
    )
    .await;
    let app = &lab.app;
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "profile": style.profile}),
    )
    .await
    .expect("the device type");
    load(&lab, "SW1", style.config);
    let outside = Outside::watch(app);
    let intent = json!({"type": "lines", "lines": style.lines});

    outside
        .call(
            app,
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": intent}),
        )
        .await
        .expect("preview");
    let change = planned_with(
        &outside,
        app,
        &lab.id("SW1"),
        intent,
        json!({"guard_minutes": 0, "verify": {"neighbors": false, "interfaces": false}}),
    )
    .await;
    let id = change["id"].as_str().unwrap().to_string();
    // What the person approving is shown: each line, a placeholder where its
    // value is.
    let shown = commands(&change);
    assert_eq!(shown.len(), style.lines.len(), "{shown:#?}");
    for line in &shown {
        assert!(line.contains("<secret:"), "nothing hidden in `{line}`");
    }
    assert!(
        change["hidden"].as_u64().unwrap() >= 3,
        "{}",
        change["hidden"]
    );

    outside
        .call(
            app,
            "changes.approve",
            json!({"id": id, "plan_hashes": [plan(&change)["plan_hash"]], "confirm": {"SW1": "SW1"}}),
        )
        .await
        .expect("approve");
    let job = outside.job(app, "changes.apply", json!({"id": id})).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let after = outside
        .call(app, "changes.get", json!({"id": id}))
        .await
        .unwrap();

    // The device was sent the real lines…
    let seen = lab.fake("SW1").seen();
    for line in style.lines {
        assert!(seen.contains(&line.to_string()), "`{line}` was not sent");
    }
    // …and the report of what is left still names them, without the value.
    assert_eq!(target_state(&after), "rollback_failed", "{after:#}");
    let steps: Vec<&str> = after["targets"][0]["result"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|step| step["detail"].as_str())
        .collect();
    assert!(
        steps
            .iter()
            .any(|detail| detail.contains("still not what it was")
                || detail.contains("no further commands")),
        "{steps:#?}"
    );
    assert!(
        steps.iter().any(|detail| detail.contains(style.named)),
        "the rollback's report no longer names `{}`: {steps:#?}",
        style.named
    );

    nothing_of_these_recorded_whole(app, &id, style.secrets);
    no_leaks(outside.leaks(app, style.secrets).await);
}

#[tokio::test]
async fn a_routeros_value_with_a_slash_in_it_is_hidden_whole_in_every_report() {
    nothing_secret_leaves_a_change_written_in(&ROUTEROS_STYLE).await;
}

#[tokio::test]
async fn a_junos_value_with_a_slash_in_it_is_hidden_whole_in_every_report() {
    nothing_secret_leaves_a_change_written_in(&JUNOS_STYLE).await;
}

#[tokio::test]
async fn a_key_value_file_with_a_slash_in_a_value_is_hidden_whole_in_every_report() {
    nothing_secret_leaves_a_change_written_in(&KEY_VALUE_STYLE).await;
}

// ------------------------------------------------- what reaches the device

#[tokio::test]
async fn what_is_typed_on_the_device_is_the_planned_lines_byte_for_byte() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let typed = tap(&lab, &["SW1"]);

    // Read, approved and applied the way a screen does it: from the default
    // view, which shows placeholders.
    let shown = planned(app, &lab.id("SW1"), made_of_secrets(), json!({})).await;
    assert!(
        commands(&shown)
            .iter()
            .any(|line| line.contains("<secret:")),
        "the view hides the values"
    );
    let applied = apply(app, &shown).await;
    assert_eq!(target_state(&applied), "applied", "{applied:#}");

    let kept = stored(app, shown["id"].as_str().unwrap());
    let planned_lines = &kept.targets[0].plan.as_ref().unwrap().commands;
    let typed = String::from_utf8(typed.lock().clone()).expect("what was typed is text");
    // One configuration session in a clean apply: the planned lines, each
    // with the device type's line ending, between entering and leaving
    // configuration mode — and nothing else in it.
    let entered = typed
        .find("configure terminal\n")
        .expect("configuration mode was entered")
        + "configure terminal\n".len();
    let left = entered
        + typed[entered..]
            .find("\nend\n")
            .expect("configuration mode was left")
        + 1;
    let expected: String = planned_lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(
        &typed[entered..left],
        expected,
        "what was typed in configuration mode"
    );
    assert_eq!(
        typed.matches("configure terminal\n").count(),
        1,
        "a clean apply configures once"
    );
    for marker in ["<secret:", "<hidden>"] {
        assert!(!typed.contains(marker), "a placeholder was typed: {typed}");
    }
}

// --------------------------------------------- an approval from before this

#[tokio::test]
async fn an_approval_given_before_values_were_hidden_still_applies_the_same_plan() {
    use nocturne_changes::model::{Approval, ChangeState};

    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    load(&lab, "SW1", &secret_config());
    let change = planned(app, &lab.id("SW1"), made_of_secrets(), json!({})).await;
    let id = change["id"].as_str().unwrap().to_string();

    // The record as the engine kept it before it hid anything: the same
    // lines and hash (what is sent and hashed did not change), its sentences
    // quoting the values, and signed off on that hash.
    let mut old = stored(app, &id);
    let hash = {
        let plan = old.targets[0].plan.as_mut().unwrap();
        plan.warnings.push(format!(
            "Nocturne cannot predict what `enable secret 5 {NEW_HASH}` changes."
        ));
        plan.plan_hash.clone()
    };
    old.approval = Some(Approval {
        by: "user".into(),
        at: 1,
        plan_hashes: vec![hash.clone()],
        note: String::new(),
        confirmed_devices: vec!["SW1".into()],
    });
    old.state = ChangeState::Approved;
    app.store().put("changes", &id, &old).unwrap();

    let shown = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(plan(&shown)["plan_hash"], hash.as_str());
    assert!(!shown.to_string().contains(NEW_HASH), "{shown:#}");

    run_job(app, "changes.apply", json!({"id": id})).await;
    let applied = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(target_state(&applied), "applied", "{applied:#}");
    assert!(lab.fake("SW1").running_config().contains(NEW_HASH));
}
