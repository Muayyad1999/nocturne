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

    // A human can still approve and apply it.
    let applied = apply(app, &change).await;
    assert_eq!(applied["state"], "applied");
}

#[tokio::test]
async fn a_change_that_is_already_approved_may_be_started_by_the_assistant() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    approve(app, &change).await.unwrap();
    let id = change["id"].as_str().unwrap();

    // Autonomy is `draft`, but a human has signed this one off already.
    let started = app
        .rpc("changes.assistant_apply", json!({"id": id}))
        .await
        .unwrap();
    finish(app, &started).await;
    let applied = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(applied["state"], "applied");
    assert_eq!(applied["approval"]["by"], "user");
}
