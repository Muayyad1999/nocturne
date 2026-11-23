//! Undoing an applied change on demand: a change like any other.
//!
//! Weeks after a change the device has moved on, so the way back is worked
//! out from what the device holds now and shown, risk and protected
//! resources included, to a person who approves that exact plan. Asking for
//! the undo sends nothing.

mod common;

use common::*;
use nocturne_app::App;
use serde_json::{Value, json};

fn desk_change() -> Value {
    json!({"type": "lines", "lines": [
        "interface GigabitEthernet0/2",
        "description desk 14",
        "exit",
    ]})
}

/// Ask for the undo of a change and wait for the engine to plan it.
async fn undo_of(app: &App, id: &str, device_ids: Value) -> Value {
    let answer = app
        .rpc(
            "changes.rollback",
            json!({"id": id, "device_ids": device_ids}),
        )
        .await
        .unwrap_or_else(|err| panic!("changes.rollback: {err}"));
    let undo_id = answer["change"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer names the undo: {answer:#}"))
        .to_string();
    let planning = finish(app, &answer).await;
    assert_eq!(
        planning.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{:?}",
        planning.error
    );
    app.rpc("changes.get", json!({"id": undo_id}))
        .await
        .expect("the undo")
}

/// The commands in `seen` that do more than read the device, as its own device
/// type's policy has it. `enable` and `exit` are how a session logs in and
/// out; configuring anything would take `configure terminal`, which is not a
/// read.
fn not_reads(app: &App, seen: &[String]) -> Vec<String> {
    use nocturne_drivers::policy::CommandClass;
    let platform =
        nocturne_app::profiles::platform(app, VERIFIED_PROFILE).expect("the device type");
    seen.iter()
        .filter(|command| !matches!(command.as_str(), "enable" | "exit"))
        .filter(|command| platform.policy().classify(command).class != CommandClass::ReadOnly)
        .cloned()
        .collect()
}

#[tokio::test]
async fn asking_for_an_undo_sends_nothing_and_answers_a_change_to_approve() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let original = apply(app, &change).await;
    assert_eq!(original["state"], "applied");
    let id = original["id"].as_str().unwrap();
    let applied = lab.fake("SW1").running_config();

    lab.fake("SW1").clear_seen();
    let answer = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .expect("an undo");
    // The answer is the new change, being planned like one created with
    // `plan: true`.
    assert_eq!(answer["change"]["state"], "draft", "{answer:#}");
    assert_eq!(answer["change"]["undoes"], id);
    assert_eq!(answer["change"]["origin"], "restore");
    assert!(answer["job_id"].is_string(), "{answer:#}");
    finish(app, &answer).await;
    let undo_id = answer["change"]["id"].as_str().unwrap();
    let undo = app
        .rpc("changes.get", json!({"id": undo_id}))
        .await
        .unwrap();

    // Planned, read from the device — and nothing but reads reached it.
    assert_eq!(undo["state"], "planned", "{undo:#}");
    let seen = lab.fake("SW1").seen();
    assert!(
        seen.iter().any(|command| command == "show running-config"),
        "the undo is planned from what the device holds: {seen:?}"
    );
    assert_eq!(not_reads(app, &seen), Vec::<String>::new(), "{seen:?}");
    assert_eq!(lab.fake("SW1").running_config(), applied);
    assert_ne!(applied, before);

    // The plan is the way from now back to before, with the risk and the
    // hash a person approves.
    let commands = commands(&undo);
    assert!(
        commands.contains(&"description desk 12".to_string()),
        "{commands:?}"
    );
    assert!(plan(&undo)["plan_hash"].as_str().unwrap().len() == 64);
    assert!(plan(&undo)["risk"]["level"].is_string());
    assert!(undo["approval"].is_null());

    // The original is still applied until its undo is, and names it.
    let original = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original["state"], "applied");
    assert_eq!(target_state(&original), "applied");
    assert_eq!(original["undone_by"], undo_id);
}

#[tokio::test]
async fn an_undo_goes_back_from_what_the_device_holds_now_and_applying_it_restores_the_device() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let original = apply(app, &change).await;
    let id = original["id"].as_str().unwrap().to_string();

    // Somebody at the console adds a line after the change was applied.
    lab.fake("SW1").with(|state| {
        state
            .apply(&["ntp server 192.0.2.99"])
            .expect("a line typed by somebody else")
    });

    let undo = undo_of(app, &id, json!([])).await;
    let undo_id = undo["id"].as_str().unwrap().to_string();
    let commands = commands(&undo);
    for line in ["no ntp server 192.0.2.99", "description desk 12"] {
        assert!(
            commands.contains(&line.to_string()),
            "`{line}` is part of the way back: {commands:?}"
        );
    }
    let warnings = plan(&undo)["warnings"].to_string();
    assert!(
        warnings.contains("changed since"),
        "the plan says the device moved on since the change: {warnings}"
    );
    assert!(
        warnings.contains("ntp server 192.0.2.99"),
        "and names what moved: {warnings}"
    );

    // Approved and applied like any change: backup, send, verify, save.
    let done = apply(app, &undo).await;
    assert_eq!(done["state"], "applied", "{done:#}");
    assert_eq!(target_state(&done), "applied");
    for step in ["drift check", "backup", "apply", "verify", "save"] {
        assert!(
            steps(&done).contains(&step.to_string()),
            "{:?}",
            steps(&done)
        );
    }
    assert_eq!(
        lab.fake("SW1").running_config(),
        before,
        "the device is back where it was before the change"
    );
    assert!(lab.fake("SW1").with(|state| state.saved()));

    // Now the original reads as rolled back, and the two name each other.
    let original = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original["state"], "rolled_back", "{original:#}");
    assert_eq!(target_state(&original), "rolled_back");
    assert_eq!(original["undone_by"], undo_id.as_str());
    assert_eq!(done["undoes"], id.as_str());
    assert!(
        original["targets"][0]["result"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["name"] == "undone"
                && step["detail"].as_str().unwrap().contains(&undo_id)),
        "{:#}",
        original["targets"][0]["result"]["steps"]
    );
    assert!(
        audit(app).iter().any(
            |(source, message)| source == "changes.rollback" && message.contains("Rolled back")
        ),
        "{:#?}",
        audit(app)
    );

    // There is nothing left to undo.
    let err = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid", "{err:?}");
}

#[tokio::test]
async fn an_undo_that_fails_its_checks_is_itself_rolled_back() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // The undo is applied with the original change's options, and so has to
    // pass the same checks: here, that the device can still ping its server.
    let change = planned(
        app,
        &lab.id("SW1"),
        desk_change(),
        json!({"verify": {"pings": ["192.0.2.7"]}}),
    )
    .await;
    let original = apply(app, &change).await;
    assert_eq!(original["state"], "applied", "{original:#}");
    let id = original["id"].as_str().unwrap().to_string();
    let applied = lab.fake("SW1").running_config();

    let undo = undo_of(app, &id, json!([])).await;
    lab.fake("SW1")
        .with(|state| state.unreachable.push("192.0.2.7".into()));
    let done = apply(app, &undo).await;

    assert_eq!(target_state(&done), "rolled_back", "{done:#}");
    assert_eq!(done["state"], "rolled_back");
    assert!(
        done["targets"][0]["result"]["error"]
            .as_str()
            .unwrap()
            .contains("ping 192.0.2.7"),
        "{done:#}"
    );
    assert_eq!(
        canonical(&lab.fake("SW1").running_config()),
        canonical(&applied),
        "the device is as the original change left it"
    );
    let original = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original["state"], "applied", "{original:#}");
    assert_eq!(target_state(&original), "applied");
}

#[tokio::test]
async fn an_undo_that_touches_a_protected_resource_needs_the_device_named() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let change = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": ["line vty 0 4", "exec-timeout 5 0", "exit"]}),
        json!({}),
    )
    .await;
    assert!(hit_kinds(&change).contains(&"access_vty".to_string()));
    let original = apply(app, &change).await;
    assert_eq!(original["state"], "applied", "{original:#}");

    let undo = undo_of(app, original["id"].as_str().unwrap(), json!([])).await;
    let undo_id = undo["id"].as_str().unwrap();
    assert!(
        hit_kinds(&undo).contains(&"access_vty".to_string()),
        "the way back touches the login lines too: {:?}",
        hit_kinds(&undo)
    );
    let err = app
        .rpc(
            "changes.approve",
            json!({"id": undo_id, "plan_hashes": [plan(&undo)["plan_hash"]]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert_eq!(err.details.as_ref().unwrap()["needs"], "typed_confirmation");

    let done = apply(app, &undo).await;
    assert_eq!(done["state"], "applied", "{done:#}");
    assert_eq!(done["approval"]["confirmed_devices"], json!(["SW1"]));
    assert_eq!(lab.fake("SW1").running_config(), before);
}

#[tokio::test]
async fn the_assistant_cannot_start_an_undo_past_the_autonomy_gate() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let original = apply(app, &change).await;
    let applied = lab.fake("SW1").running_config();
    let undo = undo_of(app, original["id"].as_str().unwrap(), json!([])).await;
    let undo_id = undo["id"].as_str().unwrap();

    // Autonomy is `draft`: planned or approved, the undo is a person's to start.
    for approved in [false, true] {
        if approved {
            approve(app, &undo).await.unwrap();
        }
        let err = app
            .rpc("changes.assistant_apply", json!({"id": undo_id}))
            .await
            .expect_err("the assistant may not start the undo");
        assert_eq!(err.code, "forbidden", "{err:?}");
        assert_eq!(err.details.as_ref().unwrap()["needs"], "human_approval");
    }
    assert_eq!(lab.fake("SW1").running_config(), applied);
}

#[tokio::test]
async fn one_undo_at_a_time_and_only_for_the_devices_asked_for() {
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    let created = app
        .rpc(
            "changes.create",
            json!({"title": "desks", "targets": [
                {"device_id": lab.id("SW1"), "intent": desk_change()},
                {"device_id": lab.id("SW2"), "intent": desk_change()},
            ]}),
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    run_job(app, "changes.plan", json!({"id": id})).await;
    let change = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    let original = apply(app, &change).await;
    assert_eq!(original["state"], "applied", "{original:#}");

    // `device_ids` narrows it, by name as well as by id.
    let first = undo_of(app, &id, json!(["SW1"])).await;
    let devices: Vec<&str> = first["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["device_id"].as_str().unwrap())
        .collect();
    assert_eq!(devices, vec![lab.id("SW1").as_str()]);

    // A second one while the first is open is refused, and names it.
    let err = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict", "{err:?}");
    assert!(
        err.message.contains(first["title"].as_str().unwrap()),
        "{}",
        err.message
    );
    assert_eq!(err.details.as_ref().unwrap()["change_id"], first["id"]);

    // Once it is cancelled, another may be asked for.
    app.rpc("changes.cancel", json!({"id": first["id"]}))
        .await
        .unwrap();
    let second = undo_of(app, &id, json!([])).await;
    assert_eq!(second["targets"].as_array().unwrap().len(), 2);
    let original = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original["undone_by"], second["id"]);

    // The lines of an undo are the engine's: they are not edited by hand.
    let err = app
        .rpc(
            "changes.update",
            json!({"id": second["id"], "targets": [
                {"device_id": lab.id("SW1"), "intent": {"type": "lines", "lines": ["ntp server 192.0.2.1"]}},
            ]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid", "{err:?}");
}

#[tokio::test]
async fn a_device_the_change_did_not_leave_changed_or_whose_backup_is_gone_is_not_offered() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    let app = &lab.app;
    let sw1 = lab.fake("SW1").running_config();
    // SW2 refuses the line, so the change is rolled back there.
    lab.fake("SW2")
        .with(|state| state.reject.push("desk 14".into()));
    let created = app
        .rpc(
            "changes.create",
            json!({"title": "desks", "options": {"stop_on_failure": false}, "targets": [
                {"device_id": lab.id("SW1"), "intent": desk_change()},
                {"device_id": lab.id("SW2"), "intent": desk_change()},
                {"device_id": lab.id("SW3"), "intent": desk_change()},
            ]}),
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    run_job(app, "changes.plan", json!({"id": id})).await;
    let change = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    let original = apply(app, &change).await;
    assert_eq!(original["state"], "partially_applied", "{original:#}");

    // The backup of what SW3 had before is gone: retention removed it, which
    // it does once the pin the apply put on it has been taken off. This is
    // the row retention deletes.
    let backup = original["targets"][2]["result"]["before_version_id"]
        .as_i64()
        .expect("SW3 was backed up before the change");
    let removed = app
        .store()
        .with(|conn| conn.execute("DELETE FROM configs WHERE id = ?1", [backup]))
        .unwrap();
    assert_eq!(removed, 1);

    // Asked for by name, each of the two is refused with its reason…
    let err = app
        .rpc("changes.rollback", json!({"id": id, "device_ids": ["SW2"]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid", "{err:?}");
    assert!(
        err.message.contains("SW2") && err.message.contains("rolled back"),
        "{}",
        err.message
    );
    let err = app
        .rpc("changes.rollback", json!({"id": id, "device_ids": ["SW3"]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid", "{err:?}");
    assert!(
        err.message.contains("SW3") && err.message.contains("can no longer be read"),
        "{}",
        err.message
    );

    // …and left out when nobody named them, with the reason in the undo.
    let undo = undo_of(app, &id, json!([])).await;
    let devices: Vec<&str> = undo["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["device_id"].as_str().unwrap())
        .collect();
    assert_eq!(devices, vec![lab.id("SW1").as_str()]);
    let description = undo["description"].as_str().unwrap();
    assert!(
        description.contains("SW3") && description.contains("can no longer be read"),
        "{description}"
    );
    assert!(!description.contains("SW2"), "{description}");

    // Applied, it puts back the one device it names. The change still stands
    // on SW3, so it reads partially applied, device by device as it is.
    let sw3 = lab.fake("SW3").running_config();
    assert_eq!(apply(app, &undo).await["state"], "applied");
    assert_eq!(
        lab.fake("SW1").running_config(),
        sw1,
        "SW1 is back as it was"
    );
    assert_eq!(lab.fake("SW3").running_config(), sw3, "SW3 was not touched");
    let after = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(after["state"], "partially_applied", "{after:#}");
    let states: Vec<&str> = after["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["result"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["rolled_back", "rolled_back", "applied"]);

    // What is left of it cannot be undone without its backup.
    let err = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid", "{err:?}");
    assert!(
        err.message.contains("nothing to roll back") && err.message.contains("SW3"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn until_a_person_applies_the_undo_the_device_is_only_read_and_then_it_is_typed_the_approved_plan()
 {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let original = apply(app, &change).await;
    let id = original["id"].as_str().unwrap().to_string();
    lab.fake("SW1").with(|state| {
        state
            .apply(&["ntp server 192.0.2.99"])
            .expect("a line typed by somebody else")
    });

    // Every byte typed to the device from here on is kept.
    let typed = tap(&lab, &["SW1"]);
    lab.fake("SW1").clear_seen();

    // Asked for, planned, read and approved: the device is only read.
    let undo = undo_of(app, &id, json!([])).await;
    let undo_id = undo["id"].as_str().unwrap().to_string();
    approve(app, &undo)
        .await
        .expect("a person approves the undo");
    let seen = lab.fake("SW1").seen();
    assert!(
        seen.iter().any(|command| command == "show running-config"),
        "{seen:?}"
    );
    assert_eq!(
        not_reads(app, &seen),
        Vec::<String>::new(),
        "sent before a person applied the undo: {seen:?}"
    );
    let so_far = String::from_utf8(typed.lock().clone()).expect("text");
    assert_eq!(
        configuration_sessions(&so_far),
        Vec::<String>::new(),
        "{so_far}"
    );

    // Applied by the person: what is typed in configuration mode is the plan
    // they approved, byte for byte, in one session.
    let kept: nocturne_changes::model::Change =
        app.store().get("changes", &undo_id).unwrap().unwrap();
    let approved = kept.targets[0].plan.as_ref().expect("the undo's plan");
    assert_eq!(
        kept.approval.as_ref().expect("approved").plan_hashes,
        vec![approved.plan_hash.clone()]
    );
    let mark = typed.lock().len();
    run_job(app, "changes.apply", json!({"id": undo_id})).await;
    let done = app
        .rpc("changes.get", json!({"id": undo_id}))
        .await
        .unwrap();
    assert_eq!(done["state"], "applied", "{done:#}");
    let during = String::from_utf8(typed.lock()[mark..].to_vec()).expect("text");
    let expected: String = approved
        .commands
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    assert!(
        expected.contains("no ntp server 192.0.2.99\n"),
        "{expected}"
    );
    assert_eq!(
        configuration_sessions(&during),
        vec![expected],
        "what was typed in configuration mode"
    );
}

#[tokio::test]
async fn an_undo_is_not_undone_in_turn_and_the_change_it_undid_is_made_again_by_planning_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let original = apply(app, &change).await;
    let id = original["id"].as_str().unwrap().to_string();
    let applied = lab.fake("SW1").running_config();
    let undo = undo_of(app, &id, json!([])).await;
    let undo_id = undo["id"].as_str().unwrap().to_string();
    assert_eq!(apply(app, &undo).await["state"], "applied");
    assert_eq!(lab.fake("SW1").running_config(), before);

    // Undoing the undo is refused, sends nothing, and says how to have the
    // change back.
    lab.fake("SW1").clear_seen();
    let err = app
        .rpc("changes.rollback", json!({"id": undo_id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid", "{err:?}");
    assert!(
        err.message.contains("not undone in turn")
            && err.message.contains("`test change`")
            && err.message.contains("plan it again"),
        "{}",
        err.message
    );
    assert!(
        lab.fake("SW1").seen().is_empty(),
        "{:?}",
        lab.fake("SW1").seen()
    );
    let unchanged = app
        .rpc("changes.get", json!({"id": undo_id}))
        .await
        .unwrap();
    assert!(unchanged["undone_by"].is_null(), "{unchanged:#}");

    // The change it undid is planned again from what the device holds now
    // and approved and applied like any change. Planned again it starts
    // over: it no longer says it was undone.
    let started = app.rpc("changes.plan", json!({"id": id})).await.unwrap();
    let replanning = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert!(
        matches!(replanning["state"].as_str(), Some("draft" | "planned")),
        "{replanning:#}"
    );
    assert!(replanning["undone_by"].is_null(), "{replanning:#}");
    assert!(replanning["targets"][0]["result"].is_null());
    finish(app, &started).await;
    let replanned = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(replanned["state"], "planned", "{replanned:#}");
    let again = apply(app, &replanned).await;
    assert_eq!(again["state"], "applied", "{again:#}");
    assert_eq!(lab.fake("SW1").running_config(), applied);
    let undo = app
        .rpc("changes.get", json!({"id": undo_id}))
        .await
        .unwrap();
    assert_eq!(undo["undoes"], id.as_str(), "the undo still names it");

    // Made again, it can be undone again, by an undo of its own.
    let second = undo_of(app, &id, json!([])).await;
    assert_ne!(second["id"], undo_id.as_str());
    let original = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original["undone_by"], second["id"]);
}

#[tokio::test]
async fn while_an_undo_waits_neither_the_change_nor_another_undo_of_it_is_planned_again() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    // The change fails its ping and the device refuses the line its rollback
    // would put back: the change is failed, which may be planned again, and
    // leaves the device changed, which may be undone.
    let change = planned(
        app,
        &lab.id("SW1"),
        desk_change(),
        json!({"verify": {"pings": ["192.0.2.7"]}}),
    )
    .await;
    lab.fake("SW1").with(|state| {
        state.unreachable.push("192.0.2.7".into());
        state.reject.push("desk 12".into());
    });
    let original = apply(app, &change).await;
    assert_eq!(original["state"], "failed", "{original:#}");
    assert_eq!(target_state(&original), "rollback_failed");
    let id = original["id"].as_str().unwrap().to_string();
    lab.fake("SW1").with(|state| state.reject.clear());

    // While its undo waits, the change is not planned again: its results are
    // what the undo goes back from.
    let first = undo_of(app, &id, json!([])).await;
    let err = app
        .rpc("changes.plan", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict", "{err:?}");
    assert_eq!(err.details.as_ref().unwrap()["change_id"], first["id"]);

    // The undo fails the same ping and is rolled back; a second is asked for.
    let undone = apply(app, &first).await;
    assert_eq!(undone["state"], "rolled_back", "{undone:#}");
    lab.fake("SW1").with(|state| state.unreachable.clear());
    let second = undo_of(app, &id, json!([])).await;

    // The first is not planned again while the second waits…
    let err = app
        .rpc("changes.plan", json!({"id": first["id"]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict", "{err:?}");
    assert_eq!(err.details.as_ref().unwrap()["change_id"], second["id"]);

    // …but once that one is cancelled it may be, and is the change's undo,
    // waiting from the moment it is planned again.
    app.rpc("changes.cancel", json!({"id": second["id"]}))
        .await
        .unwrap();
    let replanning = app
        .rpc("changes.plan", json!({"id": first["id"]}))
        .await
        .unwrap();
    let err = app
        .rpc("changes.rollback", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict", "{err:?}");
    assert_eq!(err.details.as_ref().unwrap()["change_id"], first["id"]);
    let original_now = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original_now["undone_by"], first["id"]);
    let planning = finish(app, &replanning).await;
    assert_eq!(
        planning.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{:?}",
        planning.error
    );

    // Applied, it takes the device back and the change reads rolled back.
    let first = app
        .rpc("changes.get", json!({"id": first["id"]}))
        .await
        .unwrap();
    assert_eq!(apply(app, &first).await["state"], "applied");
    assert_eq!(lab.fake("SW1").running_config(), before);
    let original_now = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(original_now["state"], "rolled_back", "{original_now:#}");
}

#[tokio::test]
async fn a_person_undoes_a_change_on_a_device_type_nobody_has_verified_but_the_assistant_does_not()
{
    let lab = lab(&["LAB1"]).await;
    let app = &lab.app;
    let before = lab.fake("LAB1").running_config();
    let unverified = |allowed: bool| json!({"section": "changes", "value": {"allow_unverified_writes": allowed}});
    app.rpc(
        "devices.update",
        json!({"id": lab.id("LAB1"), "profile": "test_ios"}),
    )
    .await
    .unwrap();
    app.rpc("settings.set", unverified(true)).await.unwrap();
    let change = planned(app, &lab.id("LAB1"), desk_change(), json!({})).await;
    assert_eq!(apply(app, &change).await["state"], "applied");

    // Writes to unverified device types are turned off again: a change is
    // refused there, the way back from one is not.
    app.rpc("settings.set", unverified(false)).await.unwrap();
    let undo = undo_of(app, change["id"].as_str().unwrap(), json!([])).await;
    assert_eq!(plan(&undo)["maturity"], "experimental");

    // The assistant is held to the device type whatever the mode.
    app.rpc(
        "settings.set",
        json!({"section": "assistant", "value": {"autonomy": "lab"}}),
    )
    .await
    .unwrap();
    let err = app
        .rpc("changes.assistant_apply", json!({"id": undo["id"]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden", "{err:?}");
    assert!(err.message.contains("verified"), "{}", err.message);

    let done = apply(app, &undo).await;
    assert_eq!(done["state"], "applied", "{done:#}");
    assert_eq!(lab.fake("LAB1").running_config(), before);
}
