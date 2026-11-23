//! Approval: the signature on a specific plan — and the autonomy policy, the
//! only thing that may stand in for it.

mod common;

use common::*;
use serde_json::json;

fn desk_change() -> serde_json::Value {
    json!({"type": "lines", "lines": [
        "interface GigabitEthernet0/2",
        "description desk 14",
        "exit",
    ]})
}

/// A change that touches the management SVI: a protected hit on every device.
fn management_change() -> serde_json::Value {
    json!({"type": "lines", "lines": [
        "interface Vlan1",
        "ip address 10.0.0.3 255.255.255.0",
        "exit",
    ]})
}

async fn with_address(names: &[&str]) -> nocturne_app::testing::Lab {
    let lab = lab(names).await;
    for name in names {
        lab.app
            .rpc(
                "devices.update",
                json!({"id": lab.id(name), "address": "10.0.0.2"}),
            )
            .await
            .unwrap();
    }
    lab
}

#[tokio::test]
async fn an_approval_names_the_exact_plan_it_approves() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let id = change["id"].as_str().unwrap();
    let hash = plan(&change)["plan_hash"].as_str().unwrap().to_string();

    // No hash at all.
    let err = app
        .rpc("changes.approve", json!({"id": id, "plan_hashes": []}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("planned again"), "{}", err.message);

    // Somebody else's hash.
    let err = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": ["0".repeat(64)]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");

    // An extra hash that belongs to nothing is refused too.
    let err = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash, "1".repeat(64)]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("not part of this change"));

    // The right one works, and is recorded.
    let approved = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash], "note": "window 22:00"}),
        )
        .await
        .unwrap();
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["approval"]["by"], "user");
    assert_eq!(approved["approval"]["note"], "window 22:00");
    assert!(
        audit(app)
            .iter()
            .any(|(source, _)| source == "changes.approve")
    );

    // And it can be taken back.
    let withdrawn = app
        .rpc("changes.unapprove", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(withdrawn["state"], "planned");
    assert!(withdrawn["approval"].is_null());
    let err = app
        .rpc("changes.apply", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("approved"), "{}", err.message);
}

#[tokio::test]
async fn a_protected_resource_needs_the_device_name_typed_out() {
    let lab = with_address(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), management_change(), json!({})).await;
    let id = change["id"].as_str().unwrap();
    let hash = plan(&change)["plan_hash"].as_str().unwrap().to_string();
    assert!(!hit_kinds(&change).is_empty());

    // The hash alone is not enough.
    let err = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash.clone()]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(
        err.message.contains("type the device's name"),
        "{}",
        err.message
    );
    assert_eq!(err.details.as_ref().unwrap()["needs"], "typed_confirmation");

    // Nor is something that is not the name.
    let err = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash.clone()], "confirm": {"SW1": "yes"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");

    let approved = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": [hash], "confirm": {"SW1": "SW1"}}),
        )
        .await
        .unwrap();
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["approval"]["confirmed_devices"], json!(["SW1"]));
}

#[tokio::test]
async fn planning_again_or_editing_invalidates_the_approval() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let id = change["id"].as_str().unwrap().to_string();
    approve(app, &change).await.unwrap();

    // Planning again drops the approval and the old hash with it.
    run_job(app, "changes.plan", json!({"id": id})).await;
    let replanned = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(replanned["state"], "planned");
    assert!(replanned["approval"].is_null());
    let err = app
        .rpc("changes.apply", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");

    // Editing the targets throws the plans away as well.
    approve(app, &replanned).await.unwrap();
    let edited = app
        .rpc(
            "changes.update",
            json!({"id": id, "targets": [{"device_id": lab.id("SW1"), "intent": {"type": "lines", "lines": ["ntp server 192.0.2.1"]}}]}),
        )
        .await;
    // An approved change is not editable at all.
    assert_eq!(edited.unwrap_err().code, "conflict");

    app.rpc("changes.unapprove", json!({"id": id}))
        .await
        .unwrap();
    let edited = app
        .rpc(
            "changes.update",
            json!({"id": id, "options": {"save": false}}),
        )
        .await
        .unwrap();
    assert_eq!(edited["state"], "draft");
    assert!(edited["targets"][0]["plan"].is_null());
    assert!(edited["approval"].is_null());
}

#[tokio::test]
async fn the_assistant_may_only_apply_what_the_autonomy_setting_allows() {
    // One lab switch and one reached over the network like a real one.
    let lab = lab(&["LAB1", "PROD1"]).await;
    let app = &lab.app;
    let (prod_id, server) = production(&lab, "PROD1", "PROD-SSH").await;

    let set_mode = |mode: &str| {
        let app = app.clone();
        let mode = mode.to_string();
        async move {
            app.rpc(
                "settings.set",
                json!({"section": "assistant", "value": {"autonomy": mode}}),
            )
            .await
            .unwrap();
        }
    };

    let assistant_change = |device_id: String, intent: serde_json::Value| {
        let app = app.clone();
        async move {
            let created = app
                .rpc(
                    "changes.create",
                    json!({
                        "title": "assistant change",
                        "origin": "assistant",
                        "targets": [{"device_id": device_id, "intent": intent}],
                    }),
                )
                .await
                .unwrap();
            let change_id = created["id"].as_str().unwrap().to_string();
            run_job(&app, "changes.plan", json!({"id": change_id})).await;
            change_id
        }
    };

    // draft: never, not even in the lab.
    set_mode("draft").await;
    let id = assistant_change(lab.id("LAB1"), desk_change()).await;
    let err = app
        .rpc("changes.assistant_apply", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("draft"), "{}", err.message);
    assert!(
        err.message.contains("Changes screen"),
        "it tells the model what to ask for: {}",
        err.message
    );

    // lab: lab devices only.
    set_mode("lab").await;
    let started = app
        .rpc("changes.assistant_apply", json!({"id": id}))
        .await
        .unwrap();
    finish(app, &started).await;
    let applied = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(applied["state"], "applied", "{applied}");
    assert_eq!(applied["approval"]["by"], "policy:lab");
    assert!(
        applied["approval"]["confirmed_devices"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let real = assistant_change(prod_id.clone(), desk_change()).await;
    let err = app
        .rpc("changes.assistant_apply", json!({"id": real}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("lab"), "{}", err.message);

    // low_risk: a real device is allowed when the plan is low risk.
    set_mode("low_risk").await;
    let started = app
        .rpc("changes.assistant_apply", json!({"id": real}))
        .await
        .unwrap();
    finish(app, &started).await;
    let applied = app.rpc("changes.get", json!({"id": real})).await.unwrap();
    assert_eq!(applied["state"], "applied", "{applied}");
    assert_eq!(applied["approval"]["by"], "policy:low_risk");

    // …but never when it is more than low risk, whatever the mode.
    let risky = assistant_change(
        prod_id.clone(),
        json!({"type": "lines", "lines": ["no vlan 20"]}),
    )
    .await;
    let err = app
        .rpc("changes.assistant_apply", json!({"id": risky}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("risk"), "{}", err.message);

    // …nor when it touches a protected resource, even in the lab.
    app.rpc(
        "devices.update",
        json!({"id": lab.id("LAB1"), "address": "10.0.0.2"}),
    )
    .await
    .unwrap();
    set_mode("lab").await;
    let protected = assistant_change(lab.id("LAB1"), management_change()).await;
    let err = app
        .rpc("changes.assistant_apply", json!({"id": protected}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(
        err.message.contains("protected resources"),
        "{}",
        err.message
    );

    // …nor to a protected device.
    app.rpc(
        "devices.update",
        json!({"id": lab.id("LAB1"), "address": "", "protected": true}),
    )
    .await
    .unwrap();
    let guarded = assistant_change(lab.id("LAB1"), desk_change()).await;
    let err = app
        .rpc("changes.assistant_apply", json!({"id": guarded}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("protected device"), "{}", err.message);
    server.shutdown().await;
}

#[tokio::test]
async fn an_unverified_device_type_always_needs_a_human() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // Back to the device type nobody has checked against hardware.
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "profile": "test_ios"}),
    )
    .await
    .unwrap();
    app.rpc(
        "settings.set",
        json!({"section": "assistant", "value": {"autonomy": "lab"}}),
    )
    .await
    .unwrap();

    let created = app
        .rpc(
            "changes.create",
            json!({"title": "t", "origin": "assistant", "targets": [{"device_id": lab.id("SW1"), "intent": desk_change()}]}),
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    run_job(app, "changes.plan", json!({"id": id})).await;
    let change = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(plan(&change)["maturity"], "experimental");
    assert_eq!(
        plan(&change)["risk"]["level"],
        "medium",
        "an unverified device type is a level worse"
    );

    let err = app
        .rpc("changes.assistant_apply", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("verified"), "{}", err.message);

    // A human is *necessary* and no longer sufficient. This test used to end
    // "A human can still approve and apply it", asserting the applied state —
    // which is precisely the gap: `docs/profiles.md` and `docs/STATUS.md` both
    // said a device type nobody had checked against hardware gets no write
    // access by default, and nothing enforced it, so this test blessed the
    // behaviour the documentation forbade. The gate is in `start_apply`, which
    // both the human and the assistant paths pass through.
    approve(app, &change)
        .await
        .expect("a human signs it off in full");
    let refused = app
        .rpc("changes.apply", json!({"id": id}))
        .await
        .expect_err("approval alone is not permission for an unchecked platform");
    assert_eq!(refused.code, "forbidden", "{refused:?}");
    assert!(
        refused.message.contains("allow_unverified_writes"),
        "and it says how to proceed deliberately: {}",
        refused.message
    );
}

async fn set_autonomy(app: &nocturne_app::App, mode: &str) {
    app.rpc(
        "settings.set",
        json!({"section": "assistant", "value": {"autonomy": mode}}),
    )
    .await
    .unwrap();
}

/// What the assistant is answered when it asks to start a change it may not
/// start, and proof that nothing went to the device.
async fn refused_to_the_assistant(
    lab: &nocturne_app::testing::Lab,
    name: &str,
    id: &str,
) -> String {
    let app = &lab.app;
    lab.fake(name).clear_seen();
    let err = app
        .rpc("changes.assistant_apply", json!({"id": id}))
        .await
        .expect_err("the assistant may not start this change");
    assert_eq!(err.code, "forbidden", "{err:?}");
    assert_eq!(
        err.details.as_ref().unwrap()["needs"],
        "human_approval",
        "{err:?}"
    );
    assert!(
        err.message.contains("Apply"),
        "it says that a person has to press Apply: {}",
        err.message
    );
    assert!(
        lab.fake(name).seen().is_empty(),
        "the device was not even connected to: {:?}",
        lab.fake(name).seen()
    );
    let after = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(after["state"], "approved", "{after:#}");
    assert_eq!(
        after["approval"]["by"], "user",
        "the approval is the person's"
    );
    err.message
}

#[tokio::test]
async fn an_approval_is_not_permission_for_the_assistant_to_start_the_change() {
    // A person approves in the afternoon what they mean to apply tonight; text
    // a stranger wrote into the assistant's context must not start it now.
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    approve(app, &change).await.unwrap();
    let id = change["id"].as_str().unwrap();

    // Autonomy is `draft` (the default): the assistant applies nothing,
    // approved or not.
    let message = refused_to_the_assistant(&lab, "SW1", id).await;
    assert!(message.contains("draft"), "{message}");
    assert_eq!(lab.fake("SW1").running_config(), before);

    // The person who approved it can still apply it.
    run_job(app, "changes.apply", json!({"id": id})).await;
    let applied = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(applied["state"], "applied", "{applied:#}");
    assert_eq!(applied["approval"]["by"], "user");
}

#[tokio::test]
async fn whatever_the_autonomy_an_approved_change_that_needs_a_person_is_not_started_by_the_assistant()
 {
    let lab = with_address(&["SW1"]).await;
    let app = &lab.app;
    let device = lab.id("SW1");
    set_autonomy(app, "low_risk").await;
    // So that a person may apply to the unverified device type below: what
    // holds the assistant back there has to be the autonomy gate itself.
    app.rpc(
        "settings.set",
        json!({"section": "changes", "value": {"allow_unverified_writes": true}}),
    )
    .await
    .unwrap();

    // Protected resources: the management SVI.
    let hits = planned(app, &device, management_change(), json!({})).await;
    assert!(!hit_kinds(&hits).is_empty());
    approve(app, &hits).await.unwrap();
    let message = refused_to_the_assistant(&lab, "SW1", hits["id"].as_str().unwrap()).await;
    assert!(message.contains("protected resources"), "{message}");
    app.rpc("changes.cancel", json!({"id": hits["id"]}))
        .await
        .unwrap();

    // High risk, with nothing protected in it: a line Nocturne can derive no
    // undo for (the port is a trunk already, so there is nothing to put back).
    let risky = planned(
        app,
        &device,
        json!({"type": "lines", "lines": [
            "interface GigabitEthernet0/1",
            "switchport mode trunk",
            "exit",
        ]}),
        json!({}),
    )
    .await;
    assert!(hit_kinds(&risky).is_empty(), "{:?}", hit_kinds(&risky));
    assert!(
        matches!(
            plan(&risky)["risk"]["level"].as_str(),
            Some("high" | "critical")
        ),
        "{:#}",
        plan(&risky)["risk"]
    );
    approve(app, &risky).await.unwrap();
    let message = refused_to_the_assistant(&lab, "SW1", risky["id"].as_str().unwrap()).await;
    assert!(message.contains("risk"), "{message}");
    app.rpc("changes.cancel", json!({"id": risky["id"]}))
        .await
        .unwrap();

    // A device type nobody has checked against real hardware.
    app.rpc(
        "devices.update",
        json!({"id": device, "profile": "test_ios"}),
    )
    .await
    .unwrap();
    let unverified = planned(app, &device, desk_change(), json!({})).await;
    assert_eq!(plan(&unverified)["maturity"], "experimental");
    approve(app, &unverified).await.unwrap();
    let message = refused_to_the_assistant(&lab, "SW1", unverified["id"].as_str().unwrap()).await;
    assert!(message.contains("verified"), "{message}");
    app.rpc("changes.cancel", json!({"id": unverified["id"]}))
        .await
        .unwrap();

    // A protected device.
    app.rpc(
        "devices.update",
        json!({"id": device, "profile": VERIFIED_PROFILE, "protected": true}),
    )
    .await
    .unwrap();
    let guarded = planned(app, &device, desk_change(), json!({})).await;
    approve(app, &guarded).await.unwrap();
    let message = refused_to_the_assistant(&lab, "SW1", guarded["id"].as_str().unwrap()).await;
    assert!(message.contains("protected device"), "{message}");
}

#[tokio::test]
async fn an_approved_change_the_autonomy_allows_is_started_by_the_assistant_and_stays_the_persons()
{
    let lab = lab(&["LAB1"]).await;
    let app = &lab.app;
    set_autonomy(app, "lab").await;
    let change = planned(app, &lab.id("LAB1"), desk_change(), json!({})).await;
    let id = change["id"].as_str().unwrap();
    let approved = app
        .rpc(
            "changes.approve",
            json!({"id": id, "plan_hashes": [plan(&change)["plan_hash"]], "note": "tonight"}),
        )
        .await
        .unwrap();
    let signed = approved["approval"].clone();

    let started = app
        .rpc("changes.assistant_apply", json!({"id": id}))
        .await
        .expect("a lab-only, low-risk change in `lab` mode");
    finish(app, &started).await;
    let applied = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(applied["state"], "applied", "{applied:#}");
    assert_eq!(
        applied["approval"], signed,
        "the person's approval is kept as they gave it"
    );
    assert_eq!(applied["approval"]["by"], "user");

    // The trail says who started it, and on whose approval.
    let started_by = audit(app)
        .into_iter()
        .filter(|(source, _)| source == "changes.apply")
        .map(|(_, message)| message)
        .find(|message| message.contains("started"))
        .expect("an entry for the start of the apply");
    assert!(
        started_by.contains("assistant") && started_by.contains("user"),
        "{started_by}"
    );
}

#[tokio::test]
async fn an_approval_that_does_not_bind_the_stored_plan_starts_nothing_for_the_assistant() {
    // `lab` mode would let the assistant start this lab-only, low-risk change
    // a person approved — but only the plan they approved.
    let lab = lab(&["LAB1"]).await;
    let app = &lab.app;
    set_autonomy(app, "lab").await;
    let before = lab.fake("LAB1").running_config();
    let change = planned(app, &lab.id("LAB1"), desk_change(), json!({})).await;
    let id = change["id"].as_str().unwrap();
    approve(app, &change).await.unwrap();

    // As the record reads while it is planned again (still `approved`, its
    // approval taken away), and approved for a plan that is not the one kept.
    let mut stored: serde_json::Value = app.store().get("changes", id).unwrap().unwrap();
    let mut stale = stored["approval"].clone();
    stale["plan_hashes"] = json!(["0".repeat(64)]);
    for approval in [json!(null), stale] {
        stored["approval"] = approval;
        app.store().put("changes", id, &stored).unwrap();
        lab.fake("LAB1").clear_seen();
        let err = app
            .rpc("changes.assistant_apply", json!({"id": id}))
            .await
            .expect_err("an approval that does not name the stored plan starts nothing");
        assert_eq!(err.code, "conflict", "{err:?}");
        let err = app
            .rpc("changes.apply", json!({"id": id}))
            .await
            .expect_err("nor does it for a person");
        assert_eq!(err.code, "conflict", "{err:?}");
        assert!(
            lab.fake("LAB1").seen().is_empty(),
            "the device was connected to: {:?}",
            lab.fake("LAB1").seen()
        );
    }
    assert_eq!(lab.fake("LAB1").running_config(), before);
}

/// A device type nobody has checked against real hardware gets no write
/// access, from anybody, until somebody says otherwise in as many words.
///
/// `docs/profiles.md` and `docs/STATUS.md` have both promised this since the
/// profile pack grew past Cisco and MikroTik. Nothing enforced it: the risk
/// level was raised and said why, and `autonomy::decide` held the *assistant*
/// back — but a human could approve and apply, and seven of the nine profiles
/// that ship are `community`.
#[tokio::test]
async fn an_unverified_device_type_is_not_written_to_unless_that_is_asked_for() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "profile": "test_ios"}),
    )
    .await
    .unwrap();
    let before = lab.fake("SW1").running_config();

    let created = app
        .rpc(
            "changes.create",
            json!({"title": "ntp", "targets": [{"device_id": lab.id("SW1"), "intent": desk_change()}]}),
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    run_job(app, "changes.plan", json!({"id": id})).await;

    // A human approves it in full, with the typed confirmation if it needs
    // one. The approval is not the question; the device type is.
    let change = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    approve(app, &change).await.expect("a human approves it");

    let refused = app
        .rpc("changes.apply", json!({"id": id}))
        .await
        .expect_err("an unverified device type is not written to by default");
    assert_eq!(refused.code, "forbidden");
    for expected in ["test_ios", "experimental", "SW1", "allow_unverified_writes"] {
        assert!(
            refused.message.contains(expected),
            "the refusal should name {expected}: {}",
            refused.message
        );
    }
    assert_eq!(
        lab.fake("SW1").running_config(),
        before,
        "and the device is untouched"
    );

    // Said in as many words, it goes ahead: this is a decision, not a wall.
    app.rpc(
        "settings.set",
        json!({"section": "changes", "value": {"allow_unverified_writes": true}}),
    )
    .await
    .unwrap();
    let started = app.rpc("changes.apply", json!({"id": id})).await;
    assert!(started.is_ok(), "{started:?}");
    let job = started.unwrap()["job_id"].as_str().unwrap().to_string();
    nocturne_app::jobs::wait(app, &job).await.unwrap();
    assert_ne!(
        lab.fake("SW1").running_config(),
        before,
        "the change really was applied once it was allowed"
    );
}
