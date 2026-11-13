//! Tasks: a saved "what to run, and where", and the one name a schedule or a
//! trigger points at.

mod common;

use common::{Changes, lab, results, state_of, until};
use serde_json::{Value, json};

async fn task(lab: &nocturne_app::testing::Lab, action: Value, select: Value) -> Value {
    lab.app
        .rpc(
            "tasks.save",
            json!({"name": "Nightly versions", "description": "what everything is running",
                   "select": select, "action": action}),
        )
        .await
        .expect("tasks.save")
}

#[tokio::test]
async fn a_task_keeps_a_filter_and_resolves_it_every_time_it_runs() {
    let lab = lab(&["SW1"]).await;
    let saved = task(
        &lab,
        json!({"type": "commands", "commands": ["show version"]}),
        json!({"filter": {"site": "Lab"}}),
    )
    .await;
    let id = saved["id"].as_str().expect("an id").to_string();
    assert_eq!(saved["select"]["filter"]["site"], json!("Lab"));
    assert!(saved["last_run_at"].is_null());

    let started = lab
        .app
        .rpc("tasks.run", json!({"id": id}))
        .await
        .expect("tasks.run");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    common::finished(&lab.app, &started).await;
    let first = results(&lab.app, &run_id).await;
    assert_eq!(first.len(), 1, "one device matched when it ran");

    // A second device appears in the same site; the same task now covers it.
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "SW2", "address": "10.0.0.2", "site": "Lab"}),
        )
        .await
        .expect("devices.create");
    let started = lab
        .app
        .rpc("tasks.run", json!({"id": id}))
        .await
        .expect("tasks.run");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 2, "the list was never written down");
    assert_eq!(state_of(&results, "SW1"), "ok");
    assert_eq!(
        state_of(&results, "SW2"),
        "unreached",
        "and the new one, which is not really there, says so"
    );
}

#[tokio::test]
async fn a_task_records_how_its_last_run_went() {
    let lab = lab(&["SW1"]).await;
    let saved = task(
        &lab,
        json!({"type": "commands", "commands": ["show version"]}),
        json!({"device_ids": [lab.id("SW1")]}),
    )
    .await;
    let id = saved["id"].as_str().expect("an id").to_string();
    let started = lab
        .app
        .rpc("tasks.run", json!({"id": id}))
        .await
        .expect("tasks.run");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    common::finished(&lab.app, &started).await;

    // The outcome is recorded once the run is in.
    let id_for_wait = id.clone();
    let app = lab.app.clone();
    until(move || {
        app.store()
            .get::<Value>("automation.tasks", &id_for_wait)
            .ok()
            .flatten()
            .map(|task| !task["last_ok"].is_null())
            .unwrap_or(false)
    })
    .await;
    let stored = lab
        .app
        .rpc("tasks.get", json!({"id": id}))
        .await
        .expect("tasks.get");
    assert_eq!(stored["last_run_id"], json!(run_id));
    assert_eq!(stored["last_ok"], json!(true));
    assert!(stored["last_run_at"].as_i64().is_some());

    // A run with an unreachable device is not a run that went well.
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "GHOST", "profile": "test_ios", "credential_id": lab.credential_id,
                   "lab": {"lab_id": "testlab", "node": "NOPE"}}),
        )
        .await
        .expect("the ghost");
    lab.app
        .rpc(
            "tasks.save",
            json!({"id": id, "name": "Nightly versions",
                   "select": {"filter": {}},
                   "action": {"type": "commands", "commands": ["show version"]}}),
        )
        .await
        .expect("tasks.save");
    let started = lab
        .app
        .rpc("tasks.run", json!({"id": id}))
        .await
        .expect("tasks.run");
    common::finished(&lab.app, &started).await;
    let app = lab.app.clone();
    let id_for_wait = id.clone();
    until(move || {
        app.store()
            .get::<Value>("automation.tasks", &id_for_wait)
            .ok()
            .flatten()
            .map(|task| task["last_ok"] == json!(false))
            .unwrap_or(false)
    })
    .await;
}

#[tokio::test]
async fn a_task_can_be_a_template_and_then_it_produces_a_change() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "ntp", "name": "NTP", "kind": "config",
                   "body": "ntp server {{ server }}",
                   "variables": [{"name": "server", "kind": "ip", "required": true}]}),
        )
        .await
        .expect("templates.save");
    let saved = task(
        &lab,
        json!({"type": "template", "template_id": "ntp", "vars": {"server": "192.0.2.1"}}),
        json!({"filter": {}}),
    )
    .await;
    let result = lab
        .app
        .rpc("tasks.run", json!({"id": saved["id"]}))
        .await
        .expect("tasks.run");
    assert_eq!(result["change_id"], json!("chg-1"));
    let created = lab.app.state::<Changes>();
    assert_eq!(created.created.lock().expect("lock").len(), 1);
    assert!(!lab.fake("SW1").running_config().contains("ntp server"));
}

#[tokio::test]
async fn a_dry_run_of_a_task_changes_nothing_and_is_not_recorded_as_a_run() {
    let lab = lab(&["SW1"]).await;
    let saved = task(
        &lab,
        json!({"type": "commands", "commands": ["show version"]}),
        json!({"filter": {}}),
    )
    .await;
    let started = lab
        .app
        .rpc("tasks.run", json!({"id": saved["id"], "dry_run": true}))
        .await
        .expect("tasks.run");
    common::finished(&lab.app, &started).await;
    assert!(lab.fake("SW1").seen().is_empty());
    let stored = lab
        .app
        .rpc("tasks.get", json!({"id": saved["id"]}))
        .await
        .expect("tasks.get");
    assert!(
        stored["last_run_at"].is_null(),
        "a dry run is not the task's last run: {stored:?}"
    );
}

#[tokio::test]
async fn one_run_can_be_narrowed_to_one_device_without_changing_the_task() {
    let lab = lab(&["SW1", "SW2"]).await;
    let saved = task(
        &lab,
        json!({"type": "commands", "commands": ["show version"]}),
        json!({"filter": {"site": "Lab"}}),
    )
    .await;
    let started = lab
        .app
        .rpc(
            "tasks.run",
            json!({"id": saved["id"], "device_ids": [lab.id("SW2")]}),
        )
        .await
        .expect("tasks.run");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["name"], json!("SW2"));
    assert!(lab.fake("SW1").seen().is_empty());

    // The task itself still says what it said.
    let stored = lab
        .app
        .rpc("tasks.get", json!({"id": saved["id"]}))
        .await
        .expect("tasks.get");
    assert_eq!(stored["select"]["filter"]["site"], json!("Lab"));
}

#[tokio::test]
async fn a_task_that_could_never_run_is_not_saved() {
    let lab = lab(&["SW1"]).await;
    for (name, select, action, wanted) in [
        (
            "no devices",
            json!({}),
            json!({"type": "commands", "commands": ["show version"]}),
            "a task needs devices",
        ),
        (
            "no commands",
            json!({"filter": {}}),
            json!({"type": "commands", "commands": []}),
            "needs commands",
        ),
        (
            "no such playbook",
            json!({"filter": {}}),
            json!({"type": "playbook", "playbook_id": "nope"}),
            "playbook `nope`",
        ),
        (
            "no such template",
            json!({"filter": {}}),
            json!({"type": "template", "template_id": "nope"}),
            "template `nope`",
        ),
        (
            "no playbook path",
            json!({"filter": {}}),
            json!({"type": "ansible", "playbook_path": "  "}),
            "needs a playbook path",
        ),
        (
            "a playbook that does not validate",
            json!({"filter": {}}),
            json!({"type": "playbook", "yaml": "steps:\n  - commands: [reload]\n"}),
            "does not validate",
        ),
    ] {
        let err = lab
            .app
            .rpc(
                "tasks.save",
                json!({"name": "Bad", "select": select, "action": action}),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("`{name}` should not be saved"));
        assert!(
            err.message.contains(wanted),
            "`{name}`: {} does not mention {wanted}",
            err.message
        );
    }
    assert!(
        lab.app
            .rpc("tasks.list", json!({}))
            .await
            .expect("tasks.list")
            .as_array()
            .expect("array")
            .is_empty()
    );

    let err = lab
        .app
        .rpc(
            "tasks.save",
            json!({"name": "  ", "action": {"type": "commands"}}),
        )
        .await
        .expect_err("a task needs a name");
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn a_task_is_listed_edited_and_deleted() {
    let lab = lab(&["SW1"]).await;
    let saved = task(
        &lab,
        json!({"type": "commands", "commands": ["show version"]}),
        json!({"filter": {}}),
    )
    .await;
    let id = saved["id"].as_str().expect("an id").to_string();
    assert_eq!(
        lab.app
            .rpc("tasks.list", json!({}))
            .await
            .expect("tasks.list")
            .as_array()
            .expect("array")
            .len(),
        1
    );
    let edited = lab
        .app
        .rpc(
            "tasks.save",
            json!({"id": id, "name": "Renamed", "select": {"filter": {}},
                   "action": {"type": "commands", "commands": ["show clock"]}}),
        )
        .await
        .expect("tasks.save");
    assert_eq!(edited["name"], json!("Renamed"));
    assert_eq!(edited["created_at"], saved["created_at"], "same task");
    assert_eq!(
        lab.app
            .rpc("tasks.delete", json!({"id": id}))
            .await
            .expect("tasks.delete")["deleted"],
        json!(true)
    );
    assert_eq!(
        lab.app
            .rpc("tasks.get", json!({"id": id}))
            .await
            .expect_err("gone")
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_task_that_runs_a_playbook_runs_it() {
    let lab = lab(&["SW1"]).await;
    let saved = task(
        &lab,
        json!({"type": "playbook",
               "yaml": "name: Read\nsteps:\n  - name: Read\n    commands: [show version]\n"}),
        json!({"filter": {}}),
    )
    .await;
    let started = lab
        .app
        .rpc("tasks.run", json!({"id": saved["id"]}))
        .await
        .expect("tasks.run");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    common::finished(&lab.app, &started).await;
    let run = lab
        .app
        .rpc("runner.get", json!({"run_id": run_id}))
        .await
        .expect("runner.get")["run"]
        .clone();
    assert_eq!(run["kind"], json!("playbook"));
    assert_eq!(run["task_id"], saved["id"]);
    assert_eq!(
        lab.app
            .rpc("runner.runs", json!({"task_id": saved["id"]}))
            .await
            .expect("runner.runs")
            .as_array()
            .expect("array")
            .len(),
        1,
        "history can be read back by task"
    );
}
