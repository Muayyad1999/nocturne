//! `host_key_changed`.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use nocturne_app::App;

use super::support::*;
use crate::catalogue::HOST_KEY_CHANGED as ID;

const KNOWN: &str = "SHA256:kAp1b0mL8sQw3nRZ6vT2yXc9dE4fG7hJ5uI0oPqLmNs";
const PRESENTED: &str = "SHA256:Zz9yXw8vUt7sRq6pOn5mLk4jIh3gFe2dCb1aZ0yXwVu";
const ANOTHER: &str = "SHA256:Qq1wWe2eRr3tTy4yUu5iIo6oPp7aAs8sDd9fFg0gHhJ";

/// `connect.host_key_changed` as the core's verifier publishes it
/// (`nocturne-app/src/connect.rs`, `StoreVerifier::verify`).
fn mismatch(app: &App, host: &str, port: u16, presented: &str) {
    app.publish(
        "connect.host_key_changed",
        &json!({
            "host": host, "port": port,
            "known": {"algorithm": "ssh-ed25519", "fingerprint": KNOWN, "first_seen": 1_700_000_000_000_i64},
            "presented": {
                "algorithm": "ssh-ed25519", "fingerprint": presented,
                "openssh": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPublicKeyBlobThatIsNotEvidence",
            },
        }),
    );
}

#[tokio::test]
async fn a_different_host_key_is_a_critical_hit_with_both_fingerprints() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    mismatch(app, "10.0.0.1", 22, PRESENTED);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(hit["title"], "SW1 presented a different SSH host key");
    assert_eq!(hit["severity"], "critical");
    assert_eq!(hit["device_id"], json!(sw1));
    assert_eq!(
        hit["evidence"],
        json!({
            "decided_from": "connect.host_key_changed",
            "host": "10.0.0.1",
            "port": 22,
            "known": {"algorithm": "ssh-ed25519", "fingerprint": KNOWN, "first_seen": 1_700_000_000_000_i64},
            "presented": {"algorithm": "ssh-ed25519", "fingerprint": PRESENTED},
            "device_ids": [sw1],
            "host_key_policy": "tofu",
            "connection": "refused",
        })
    );
    // Fingerprints, not the key: the blob is not evidence anybody reads.
    assert!(!hit.to_string().contains("AAAAC3Nza"), "{hit}");
    assert_eq!(stored_events(app, ID).len(), 1);

    let key = format!("detect:host_key_changed:{sw1}:10.0.0.1:22:{PRESENTED}");
    let raised = world.fakes.raised_with(&key);
    assert_eq!(raised.len(), 1, "{:#?}", world.fakes.raised.lock());
    assert_eq!(raised[0]["severity"], "critical");
}

#[tokio::test]
async fn the_same_key_on_every_reconnect_is_one_hit_and_another_key_is_another() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    world.device("SW1", "10.0.0.1").await;

    // The monitor and the backups reconnect all day, and the core announces
    // the mismatch every time.
    for _ in 0..5 {
        mismatch(app, "10.0.0.1", 22, PRESENTED);
    }
    settle(app).await;
    assert_eq!(hits_of(app, ID).await.len(), 1);

    mismatch(app, "10.0.0.1", 22, ANOTHER);
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["presented"]["fingerprint"], ANOTHER);
}

#[tokio::test]
async fn a_host_that_is_no_inventory_device_or_is_several_is_filed_under_none() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    // Two devices behind one address: picking one would be a guess.
    let a = world.device("CONSOLE-A", "10.0.0.9").await;
    let b = world.device("CONSOLE-B", "10.0.0.9").await;

    mismatch(app, "192.0.2.77", 2222, PRESENTED);
    mismatch(app, "10.0.0.9", 22, PRESENTED);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 2, "{hits:#?}");
    let (shared, stranger) = (&hits[0], &hits[1]);
    assert_eq!(
        stranger["title"],
        "192.0.2.77:2222 presented a different SSH host key"
    );
    assert_eq!(stranger.get("device_id"), None);
    assert_eq!(stranger["evidence"]["device_ids"], json!([]));
    assert_eq!(
        world
            .fakes
            .raised_with(&format!(
                "detect:host_key_changed:-:192.0.2.77:2222:{PRESENTED}"
            ))
            .len(),
        1
    );

    assert_eq!(shared.get("device_id"), None);
    let mut named: Vec<Value> = shared["evidence"]["device_ids"].as_array().unwrap().clone();
    named.sort_by_key(|id| id.as_str().unwrap().to_string());
    let mut expected = vec![json!(a), json!(b)];
    expected.sort_by_key(|id| id.as_str().unwrap().to_string());
    assert_eq!(named, expected);
}

#[tokio::test]
async fn a_port_that_is_not_the_devices_is_not_that_device() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    world.device("SW1", "10.0.0.1").await;

    mismatch(app, "10.0.0.1", 2222, PRESENTED);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].get("device_id"), None);
}

#[tokio::test]
async fn with_host_key_checking_off_the_hit_says_the_connection_went_ahead() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    world.device("SW1", "10.0.0.1").await;
    app.rpc(
        "settings.set",
        json!({"section": "connections", "value": {"host_key_policy": "accept"}}),
    )
    .await
    .unwrap();

    mismatch(app, "10.0.0.1", 22, PRESENTED);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(
        hits[0]["title"],
        "SW1 presented a different SSH host key, and the connection went ahead because host key checking is off"
    );
    assert_eq!(hits[0]["evidence"]["connection"], "went_ahead");
}

#[tokio::test]
async fn it_needs_nothing_but_the_core() {
    let world = world(Build::CORE).await;
    let app = &world.app;
    world.device("SW1", "10.0.0.1").await;

    mismatch(app, "10.0.0.1", 22, PRESENTED);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].get("alert_id"), None);
    let entry = listed(app, ID).await;
    assert_eq!(entry["available"], true);
    assert_eq!(entry.get("unavailable_reason"), None);
}
