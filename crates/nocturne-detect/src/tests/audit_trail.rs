//! The detections decided from the audit trail: `unredacted_config_views`,
//! `secrets_changed`, `failed_unlocks`.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use nocturne_app::App;
use nocturne_app::store::{LogEntry, LogQuery};

use super::support::*;
use crate::catalogue::{FAILED_UNLOCKS, SECRETS_CHANGED, UNREDACTED_CONFIG_VIEWS};

/// What `nocturne-config/src/versions.rs` audits when a configuration is
/// shown with its secrets.
fn viewed(app: &App, device_id: &str) {
    app.audit(
        "config.view_unredacted",
        Some(device_id),
        "Viewed unredacted configuration of `SW1`",
        json!({"version_id": 7, "kind": "running"}),
    );
}

/// The ids of the audit entries of one action, oldest first.
fn audit_ids(app: &App, action: &str) -> Vec<i64> {
    let mut ids: Vec<i64> = app
        .store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            ..LogQuery::default()
        })
        .unwrap()
        .into_iter()
        .filter(|entry| entry.source == action)
        .map(|entry| entry.id)
        .collect();
    ids.sort_unstable();
    ids
}

// ------------------------------------------------- unredacted_config_views

#[tokio::test]
async fn the_fifth_unredacted_view_in_ten_minutes_is_a_hit_naming_the_five_audit_entries() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;

    for _ in 0..4 {
        viewed(app, &sw1);
    }
    settle(app).await;
    assert_eq!(
        hits_of(app, UNREDACTED_CONFIG_VIEWS).await,
        Vec::<Value>::new()
    );

    viewed(app, &sw2);
    settle(app).await;
    let hits = hits_of(app, UNREDACTED_CONFIG_VIEWS).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(
        hit["title"],
        "5 unredacted configuration views in 10 minutes"
    );
    assert_eq!(hit["severity"], "medium");
    // It is about whoever is reading, not about a device.
    assert_eq!(hit.get("device_id"), None);
    let ids = audit_ids(app, "config.view_unredacted");
    assert_eq!(ids.len(), 5);
    let mut named: Vec<i64> = hit["evidence"]["audit_entry_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_i64().unwrap())
        .collect();
    named.sort_unstable();
    assert_eq!(named, ids);
    assert_eq!(hit["evidence"]["count"], 5);
    assert_eq!(hit["evidence"]["window_min"], 10);
    let mut devices = vec![json!(sw1), json!(sw2)];
    devices.sort_by_key(|id| id.as_str().unwrap().to_string());
    assert_eq!(hit["evidence"]["device_ids"], json!(devices));
    // There are no users, and the hit says so instead of leaving it out.
    assert_eq!(
        hit["evidence"]["who"],
        "Nocturne has one operator identity, not users, so this cannot say who."
    );
    assert_eq!(
        world
            .fakes
            .raised_with("detect:unredacted_config_views:-:views")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_reported_run_of_views_is_not_reported_again_view_by_view() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    for _ in 0..5 {
        viewed(app, &sw1);
    }
    settle(app).await;
    assert_eq!(hits_of(app, UNREDACTED_CONFIG_VIEWS).await.len(), 1);

    // The sixth to the ninth are the first four of the next five.
    for _ in 0..4 {
        viewed(app, &sw1);
    }
    settle(app).await;
    assert_eq!(hits_of(app, UNREDACTED_CONFIG_VIEWS).await.len(), 1);

    viewed(app, &sw1);
    settle(app).await;
    let hits = hits_of(app, UNREDACTED_CONFIG_VIEWS).await;
    assert_eq!(hits.len(), 2, "{hits:#?}");
    let ids = audit_ids(app, "config.view_unredacted");
    let mut second: Vec<i64> = hits[0]["evidence"]["audit_entry_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_i64().unwrap())
        .collect();
    second.sort_unstable();
    assert_eq!(second, ids[5..], "the second hit names the second five");
}

#[tokio::test]
async fn views_from_before_the_window_do_not_count_towards_it() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    // Three views eleven minutes ago, as the audit trail stores them.
    for _ in 0..3 {
        app.store()
            .log_append(&LogEntry {
                id: 0,
                ts: now() - 11 * MINUTE,
                stream: "audit".into(),
                device_id: Some(sw1.clone()),
                severity: Some(6),
                source: "config.view_unredacted".into(),
                message: "Viewed unredacted configuration of `SW1`".into(),
                body: None,
            })
            .unwrap();
    }

    for _ in 0..4 {
        viewed(app, &sw1);
    }
    settle(app).await;

    assert_eq!(
        hits_of(app, UNREDACTED_CONFIG_VIEWS).await,
        Vec::<Value>::new()
    );
}

#[tokio::test]
async fn the_threshold_is_the_setting() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    set(
        app,
        json!({"unredacted_views": {"count": 2, "window_min": 3}}),
    )
    .await;

    viewed(app, &sw1);
    viewed(app, &sw1);
    settle(app).await;

    let hits = hits_of(app, UNREDACTED_CONFIG_VIEWS).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(
        hits[0]["title"],
        "2 unredacted configuration views in 3 minutes"
    );
}

#[tokio::test]
async fn without_the_config_feature_views_are_unavailable_with_the_reason() {
    let world = world(Build {
        config: false,
        ..Build::FULL
    })
    .await;
    let entry = listed(&world.app, UNREDACTED_CONFIG_VIEWS).await;
    assert_eq!(entry["available"], false);
    assert_eq!(
        entry["unavailable_reason"],
        "the config feature is not in this build, so nothing records an unredacted view"
    );
}

// --------------------------------------------------------- secrets_changed

#[tokio::test]
async fn saving_and_deleting_a_credential_are_each_a_hit_naming_the_audit_entry() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    // The lab's own credential was saved while the world was assembled.
    let before = hits_of(app, SECRETS_CHANGED).await.len();
    assert_eq!(before, 1);

    let credential = app
        .rpc(
            "credentials.create",
            json!({"name": "Campus", "username": "netops", "password": "TopSecret!"}),
        )
        .await
        .unwrap();
    app.rpc("credentials.delete", json!({"id": credential["id"]}))
        .await
        .unwrap();
    settle(app).await;

    let hits = hits_of(app, SECRETS_CHANGED).await;
    assert_eq!(hits.len(), before + 2, "{hits:#?}");
    let (deleted, saved) = (&hits[0], &hits[1]);
    assert_eq!(
        saved["title"],
        "A credential was saved (created or replaced)"
    );
    assert_eq!(deleted["title"], "A credential was deleted");
    for hit in [saved, deleted] {
        assert_eq!(hit["severity"], "info");
        assert_eq!(hit.get("device_id"), None);
    }
    assert_eq!(
        saved["evidence"]["audit_entry_id"],
        json!(audit_ids(app, "credential.save")[1])
    );
    assert_eq!(
        deleted["evidence"]["audit_entry_id"],
        json!(audit_ids(app, "credential.delete")[0])
    );
    assert_eq!(saved["evidence"]["action"], "credential.save");
    // Nothing of the credential itself is in a hit, an alert or the stream.
    let everything = format!(
        "{hits:?} {:?} {:?}",
        world.fakes.raised.lock(),
        stored_events(app, SECRETS_CHANGED)
    );
    assert!(!everything.contains("TopSecret!"), "{everything}");
    assert_eq!(
        world
            .fakes
            .raised_with("detect:secrets_changed:-:credential_deleted")
            .len(),
        1
    );
}

#[tokio::test]
async fn the_master_password_being_set_or_changed_is_information_and_removed_is_high() {
    let world = world_on_disk(Build::FULL).await;
    let app = &world.app;

    app.rpc("vault.set_password", json!({"new": "correct horse"}))
        .await
        .unwrap();
    app.rpc(
        "vault.set_password",
        json!({"current": "correct horse", "new": "battery staple"}),
    )
    .await
    .unwrap();
    app.rpc("vault.set_password", json!({"current": "battery staple"}))
        .await
        .unwrap();
    // There is none left: asking again removes nothing and reports nothing.
    app.rpc("vault.set_password", json!({})).await.unwrap();
    settle(app).await;

    let said: Vec<(String, String)> = hits_of(app, SECRETS_CHANGED)
        .await
        .iter()
        .rev()
        .filter(|hit| hit["evidence"]["action"] == "vault.set_password")
        .map(|hit| {
            (
                hit["title"].as_str().unwrap().to_string(),
                hit["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            (
                "The vault's master password was set".to_string(),
                "info".to_string()
            ),
            (
                "The vault's master password was changed".to_string(),
                "info".to_string()
            ),
            (
                "The vault's master password was removed".to_string(),
                "high".to_string()
            ),
        ]
    );
    let raised = world
        .fakes
        .raised_with("detect:secrets_changed:-:master_password");
    assert_eq!(raised.len(), 3);
    assert_eq!(raised[2]["severity"], "high");
}

// ---------------------------------------------------------- failed_unlocks

#[tokio::test]
async fn the_fifth_wrong_vault_password_in_five_minutes_is_a_hit_and_none_of_them_is_in_it() {
    let world = world_on_disk(Build::FULL).await;
    let app = &world.app;
    app.rpc("vault.set_password", json!({"new": "correct horse"}))
        .await
        .unwrap();
    app.rpc("vault.lock", Value::Null).await.unwrap();

    let guess = |n: usize| format!("guess-number-{n}-xyzzy");
    for n in 0..4 {
        app.rpc("vault.unlock", json!({"password": guess(n)}))
            .await
            .unwrap_err();
    }
    settle(app).await;
    assert_eq!(hits_of(app, FAILED_UNLOCKS).await, Vec::<Value>::new());

    app.rpc("vault.unlock", json!({"password": guess(4)}))
        .await
        .unwrap_err();
    settle(app).await;
    let hits = hits_of(app, FAILED_UNLOCKS).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(hit["title"], "5 wrong vault passwords in 5 minutes");
    assert_eq!(hit["severity"], "high");
    assert_eq!(hit["evidence"]["what"], "vault");
    assert_eq!(hit["evidence"]["count"], 5);
    let mut named: Vec<i64> = hit["evidence"]["audit_entry_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_i64().unwrap())
        .collect();
    named.sort_unstable();
    assert_eq!(named, audit_ids(app, "vault.wrong_password"));
    assert_eq!(
        world
            .fakes
            .raised_with("detect:failed_unlocks:-:vault")
            .len(),
        1
    );

    // The sixth is the first of the next five.
    app.rpc("vault.unlock", json!({"password": guess(5)}))
        .await
        .unwrap_err();
    settle(app).await;
    assert_eq!(hits_of(app, FAILED_UNLOCKS).await.len(), 1);

    // What was tried is nowhere: not in the hit, the alert or the stream.
    let everything = format!(
        "{:?} {:?} {:?}",
        hits_of(app, FAILED_UNLOCKS).await,
        world.fakes.raised.lock(),
        stored_events(app, FAILED_UNLOCKS)
    );
    assert!(!everything.contains("xyzzy"), "{everything}");
}

/// What `nocturne-server/src/auth.rs` audits for a minute of refused
/// requests.
fn refused(app: &App, bad_token: u64, missing_token: u64, from: &[&str]) {
    let count = bad_token + missing_token;
    app.audit(
        "server.token_refused",
        None,
        format!(
            "Refused {count} requests to the server within a minute: {bad_token} with a wrong access token, {missing_token} with none"
        ),
        json!({
            "count": count, "bad_token": bad_token, "missing_token": missing_token,
            "from": from, "more_sources": false, "window_s": 60,
        }),
    );
}

#[tokio::test]
async fn requests_with_a_wrong_token_add_up_across_minutes_and_ones_with_none_do_not_count() {
    let world = world(Build::FULL).await;
    let app = &world.app;

    // A scanner without a token, however busy, is not a guess.
    refused(app, 0, 400, &["198.51.100.4"]);
    refused(app, 2, 7, &["203.0.113.9"]);
    settle(app).await;
    assert_eq!(hits_of(app, FAILED_UNLOCKS).await, Vec::<Value>::new());

    refused(app, 3, 0, &["203.0.113.9", "203.0.113.10"]);
    settle(app).await;
    let hits = hits_of(app, FAILED_UNLOCKS).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(
        hit["title"],
        "5 requests to the server with a wrong access token in 5 minutes"
    );
    assert_eq!(hit["severity"], "high");
    assert_eq!(hit["evidence"]["what"], "server");
    assert_eq!(hit["evidence"]["count"], 5);
    assert_eq!(
        hit["evidence"]["from"],
        json!(["203.0.113.10", "203.0.113.9"])
    );
    // The two entries that carried guesses are named; the scanner's is not.
    let ids = audit_ids(app, "server.token_refused");
    let mut named: Vec<i64> = hit["evidence"]["audit_entry_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_i64().unwrap())
        .collect();
    named.sort_unstable();
    assert_eq!(named, ids[1..]);
    assert_eq!(
        world
            .fakes
            .raised_with("detect:failed_unlocks:-:server")
            .len(),
        1
    );
}

#[tokio::test]
async fn wrong_passwords_and_wrong_tokens_are_counted_apart() {
    let world = world(Build::FULL).await;
    let app = &world.app;

    // Four of each: nine failures, and neither kind has reached five.
    refused(app, 4, 0, &["203.0.113.9"]);
    for _ in 0..4 {
        // What the core audits for a wrong master password
        // (`nocturne-app/src/system.rs`).
        app.audit(
            "vault.wrong_password",
            None,
            "Refused a wrong password for the credential vault",
            json!({"method": "vault.unlock"}),
        );
    }
    settle(app).await;
    assert_eq!(hits_of(app, FAILED_UNLOCKS).await, Vec::<Value>::new());

    // One more token, and only the tokens are reported.
    refused(app, 1, 0, &["203.0.113.9"]);
    settle(app).await;
    let hits = hits_of(app, FAILED_UNLOCKS).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["what"], "server");

    // A hit for the tokens has not used up the passwords.
    app.audit(
        "vault.wrong_password",
        None,
        "Refused a wrong password for the credential vault",
        json!({"method": "vault.unlock"}),
    );
    settle(app).await;
    let hits = hits_of(app, FAILED_UNLOCKS).await;
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["what"], "vault");
    assert_eq!(hits[0]["evidence"]["count"], 5);
}

#[tokio::test]
async fn the_audit_detections_need_nothing_but_the_core() {
    let world = world(Build::CORE).await;
    let app = &world.app;
    for id in [SECRETS_CHANGED, FAILED_UNLOCKS] {
        let entry = listed(app, id).await;
        assert_eq!(entry["available"], true, "{id}");
    }
    // The lab's credential was reported even here; it just raised no alert.
    let hits = hits_of(app, SECRETS_CHANGED).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].get("alert_id"), None);
}
