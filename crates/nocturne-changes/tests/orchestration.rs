//! Several devices, several changes, and the things that interrupt them.

mod common;

use common::*;
use nocturne_app::jobs::JobState;
use serde_json::json;

fn desk_change() -> serde_json::Value {
    json!({"type": "lines", "lines": [
        "interface GigabitEthernet0/2",
        "description desk 14",
        "exit",
    ]})
}

/// Create one change over several devices with the same intent.
async fn over(
    lab: &nocturne_app::testing::Lab,
    names: &[&str],
    options: serde_json::Value,
) -> serde_json::Value {
    let targets: Vec<serde_json::Value> = names
        .iter()
        .map(|name| json!({"device_id": lab.id(name), "intent": desk_change()}))
        .collect();
    let created = lab
        .app
        .rpc(
            "changes.create",
            json!({"title": "desks", "targets": targets, "options": options}),
        )
        .await
        .expect("create");
    let id = created["id"].as_str().expect("id").to_string();
    run_job(&lab.app, "changes.plan", json!({"id": id})).await;
    lab.app
        .rpc("changes.get", json!({"id": id}))
        .await
        .expect("get")
}

fn states(change: &serde_json::Value) -> Vec<String> {
    change["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| {
            target["result"]["state"]
                .as_str()
                .unwrap_or("(none)")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_failure_stops_the_devices_that_come_after_it() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    let app = &lab.app;
    lab.fake("SW2")
        .with(|state| state.reject.push("desk 14".into()));

    let change = over(&lab, &["SW1", "SW2", "SW3"], json!({})).await;
    // Planning connected to all three; from here on SW3 must be left alone.
    lab.fake("SW3").clear_seen();
    let applied = apply(app, &change).await;

    assert_eq!(
        states(&applied),
        vec!["applied", "rolled_back", "skipped"],
        "{applied:#}"
    );
    assert_eq!(applied["state"], "partially_applied");
    let skipped = applied["targets"][2]["result"]["error"].as_str().unwrap();
    assert!(skipped.contains("stops on failure"), "{skipped}");
    assert!(
        lab.fake("SW3").seen().is_empty(),
        "the third switch was never even connected to"
    );
    assert!(
        lab.fake("SW1")
            .running_config()
            .contains("description desk 14")
    );
}

#[tokio::test]
async fn devices_can_be_done_in_parallel_and_carry_on_past_a_failure() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    let app = &lab.app;
    lab.fake("SW2")
        .with(|state| state.reject.push("desk 14".into()));

    let change = over(
        &lab,
        &["SW1", "SW2", "SW3"],
        json!({"parallel": 3, "stop_on_failure": false}),
    )
    .await;
    let applied = apply(app, &change).await;

    assert_eq!(states(&applied), vec!["applied", "rolled_back", "applied"]);
    assert_eq!(applied["state"], "partially_applied");
    for name in ["SW1", "SW3"] {
        assert!(
            lab.fake(name)
                .running_config()
                .contains("description desk 14"),
            "{name}"
        );
    }
    assert!(
        !lab.fake("SW2")
            .running_config()
            .contains("description desk 14")
    );
}

#[tokio::test]
async fn cancelling_an_apply_finishes_the_device_in_flight_and_skips_the_rest() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    let app = &lab.app;
    let change = over(&lab, &["SW1", "SW2", "SW3"], json!({})).await;
    approve(app, &change).await.unwrap();
    let id = change["id"].as_str().unwrap().to_string();

    // Cancel as soon as the first device has been saved.
    let cancelling = app.clone();
    let cancel_id = id.clone();
    when_step(app, "save", move || {
        tokio::spawn(async move {
            let _ = cancelling
                .rpc("changes.cancel", json!({"id": cancel_id}))
                .await;
        });
    });

    let job = try_job(app, "changes.apply", json!({"id": id.clone()}))
        .await
        .unwrap();
    assert_eq!(job.state, JobState::Cancelled);
    let after = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    // The device that was already under way finishes properly; the ones that
    // had not started are skipped. Nothing is left half-configured.
    let states = states(&after);
    assert_eq!(states[0], "applied", "{after:#}");
    assert_eq!(states.last().unwrap(), "skipped", "{states:?}");
    assert!(
        states
            .iter()
            .all(|state| state == "applied" || state == "skipped"),
        "{states:?}"
    );
    let skipped = states.iter().position(|state| state == "skipped").unwrap();
    assert!(
        after["targets"][skipped]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
    assert_eq!(after["state"], "partially_applied");
    // The first device is finished properly: changed, verified and saved.
    assert!(
        lab.fake("SW1")
            .running_config()
            .contains("description desk 14")
    );
    assert!(lab.fake("SW1").with(|state| state.saved()));
}

#[tokio::test]
async fn one_device_is_never_touched_by_two_changes_at_once() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let first = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let second = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": ["ntp server 192.0.2.1"]}),
        json!({}),
    )
    .await;
    approve(app, &first).await.unwrap();
    approve(app, &second).await.unwrap();

    let started = app
        .rpc("changes.apply", json!({"id": first["id"]}))
        .await
        .unwrap();
    // The second change wants the same switch while the first one has it.
    let err = app
        .rpc("changes.apply", json!({"id": second["id"]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("SW1"), "{}", err.message);
    assert!(
        err.message.contains("only one change may touch a device"),
        "{}",
        err.message
    );
    // And a change cannot be applied twice either.
    let again = app
        .rpc("changes.apply", json!({"id": first["id"]}))
        .await
        .unwrap_err();
    assert_eq!(again.code, "conflict");

    finish(app, &started).await;

    // Once it is over the lock is gone — but the second change was planned
    // against the configuration the first one has just changed, so it has to
    // be planned and approved again before it may go anywhere near the device.
    let drifted = try_job(app, "changes.apply", json!({"id": second["id"]}))
        .await
        .unwrap();
    assert_eq!(drifted.state, JobState::Succeeded);
    let refused = app
        .rpc("changes.get", json!({"id": second["id"]}))
        .await
        .unwrap();
    assert_eq!(target_state(&refused), "failed");
    assert!(
        refused["targets"][0]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("changed since this was planned")
    );

    run_job(app, "changes.plan", json!({"id": second["id"]})).await;
    let replanned = app
        .rpc("changes.get", json!({"id": second["id"]}))
        .await
        .unwrap();
    let applied = apply(app, &replanned).await;
    assert_eq!(applied["state"], "applied");
    assert!(
        lab.fake("SW1")
            .running_config()
            .contains("ntp server 192.0.2.1")
    );
}

#[tokio::test]
async fn an_applied_change_can_be_rolled_back_on_demand() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let applied = apply(app, &change).await;
    assert_eq!(applied["state"], "applied");
    assert!(
        lab.fake("SW1")
            .running_config()
            .contains("description desk 14")
    );

    let id = change["id"].as_str().unwrap();
    run_job(app, "changes.rollback", json!({"id": id})).await;
    let after = app.rpc("changes.get", json!({"id": id})).await.unwrap();

    assert_eq!(after["state"], "rolled_back");
    assert_eq!(target_state(&after), "rolled_back");
    assert_eq!(
        lab.fake("SW1").running_config(),
        before,
        "the device is back where it started"
    );
    assert!(lab.fake("SW1").with(|state| state.saved()));
    assert!(
        audit(app)
            .iter()
            .any(|(source, _)| source == "changes.rollback")
    );

    // There is nothing left to roll back.
    let err = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn changes_found_applying_after_a_restart_are_failed_with_the_devices_to_check() {
    let dir = tempfile::tempdir().unwrap();
    let change_id;
    {
        let mut builder =
            nocturne_app::AppBuilder::new(nocturne_app::AppConfig::at(dir.path())).unwrap();
        nocturne_app::init(&mut builder);
        nocturne_changes::init(&mut builder);
        nocturne_app::profiles::register(&mut builder, &verified_profile());
        let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
        let app = &lab.app;
        app.rpc(
            "devices.update",
            json!({"id": lab.id("SW1"), "profile": VERIFIED_PROFILE}),
        )
        .await
        .unwrap();
        let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
        change_id = change["id"].as_str().unwrap().to_string();
        approve(app, &change).await.unwrap();
        // Pretend the process stopped in the middle of the apply.
        let mut stored: serde_json::Value = app
            .store()
            .get("changes", &change_id)
            .unwrap()
            .expect("the change");
        stored["state"] = json!("applying");
        stored["targets"][0]["result"] = json!({"state": "applying", "started_at": 1});
        app.store().put("changes", &change_id, &stored).unwrap();
        app.shutdown();
    }

    // Start again on the same data directory.
    let mut builder =
        nocturne_app::AppBuilder::new(nocturne_app::AppConfig::at(dir.path())).unwrap();
    nocturne_app::init(&mut builder);
    nocturne_changes::init(&mut builder);
    let app = builder.build();
    let change = app
        .rpc("changes.get", json!({"id": change_id}))
        .await
        .unwrap();
    assert_eq!(change["state"], "failed");
    let error = change["targets"][0]["result"]["error"].as_str().unwrap();
    assert!(error.contains("Nocturne was closed"), "{error}");
    assert!(
        error.contains("SW1"),
        "it names the devices to check: {error}"
    );
    assert!(
        audit(&app)
            .iter()
            .any(|(source, _)| source == "changes.interrupted")
    );
}

#[tokio::test]
async fn the_list_shows_what_a_change_is_without_its_plans() {
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    let change = over(&lab, &["SW1", "SW2"], json!({})).await;

    let list = app.rpc("changes.list", json!({})).await.unwrap();
    let row = &list[0];
    assert_eq!(row["id"], change["id"]);
    assert_eq!(row["state"], "planned");
    assert_eq!(row["risk"], "low");
    assert_eq!(row["devices"].as_array().unwrap().len(), 2);
    assert_eq!(row["devices"][0]["name"], "SW1");
    assert_eq!(row["devices"][0]["commands"], 3);
    assert!(row["devices"][0]["planned"].as_bool().unwrap());
    assert!(row.get("targets").is_none(), "no plans in a summary: {row}");

    // Filters and the empty page.
    assert_eq!(
        app.rpc("changes.list", json!({"state": "applied"}))
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        app.rpc("changes.list", json!({"device_id": lab.id("SW1")}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        app.rpc("changes.list", json!({"origin": "assistant"}))
            .await
            .unwrap(),
        json!([])
    );

    // Paging: one at a time, then the next page after the cursor.
    let page = app.rpc("changes.list", json!({"limit": 1})).await.unwrap();
    assert_eq!(page.as_array().unwrap().len(), 1);
    let next = app
        .rpc("changes.list", json!({"limit": 1, "cursor": page[0]["id"]}))
        .await
        .unwrap();
    assert_eq!(next, json!([]), "there is only one change so far");
    let err = app
        .rpc("changes.list", json!({"cursor": "nonsense"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");

    // Cancelling and deleting.
    let id = change["id"].as_str().unwrap();
    assert_eq!(
        app.rpc("changes.cancel", json!({"id": id})).await.unwrap()["state"],
        "cancelled"
    );
    assert_eq!(
        app.rpc("changes.delete", json!({"id": id})).await.unwrap()["deleted"],
        true
    );
    assert_eq!(
        app.rpc("changes.get", json!({"id": id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    // The audit trail keeps the record that it existed, with its commands.
    assert!(
        audit(app)
            .iter()
            .any(|(source, message)| source == "changes.delete" && message.contains("desks"))
    );
}

#[tokio::test]
async fn create_plans_in_one_call_when_asked() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let created = app
        .rpc(
            "changes.create",
            json!({
                "title": "one call",
                "ticket": "CHG-42",
                "targets": [{"device_id": "SW1", "intent": desk_change()}],
                "plan": true,
            }),
        )
        .await
        .unwrap();
    assert!(created["job_id"].is_string(), "{created}");
    assert_eq!(created["change"]["ticket"], "CHG-42");
    // Devices can be named instead of given by id.
    assert_eq!(created["change"]["targets"][0]["device_id"], lab.id("SW1"));
    finish(app, &created).await;
    let change = app
        .rpc("changes.get", json!({"id": created["change"]["id"]}))
        .await
        .unwrap();
    assert_eq!(change["state"], "planned");
    assert_eq!(commands(&change).len(), 3);

    // The same device twice in one change is a mistake worth saying so.
    let err = app
        .rpc(
            "changes.create",
            json!({
                "title": "twice",
                "targets": [
                    {"device_id": "SW1", "intent": desk_change()},
                    {"device_id": "SW1", "intent": desk_change()},
                ],
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("twice"), "{}", err.message);
}

#[tokio::test]
async fn preview_answers_what_would_this_do_without_creating_anything() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let plan = app
        .rpc(
            "changes.preview",
            json!({"device_id": "SW1", "intent": desk_change()}),
        )
        .await
        .unwrap();
    assert_eq!(plan["commands"].as_array().unwrap().len(), 3);
    assert_eq!(plan["risk"]["level"], "low");
    assert!(plan["plan_hash"].as_str().unwrap().len() == 64);
    assert_eq!(
        app.rpc("changes.list", json!({})).await.unwrap(),
        json!([]),
        "a preview creates no change"
    );
}
