//! The detections, in the one place where what they watch is real.
//!
//! `nocturne-detect` may not link to another feature (`CLAUDE.md`), so its
//! own tests register stand-ins for the log collector, the configuration
//! store, the change engine and monitoring, each answering in the shape its
//! author read out of the real crate. That is the right test for the
//! detections' judgement and the wrong one for the question that matters
//! most about a feature that only listens: is anybody speaking? A detection
//! that waits for an event nobody publishes, or calls a method under a name
//! nobody registered, is silent for ever and looks exactly like a quiet
//! network.
//!
//! The suite is where the real crates meet, so that is asked here.

use std::time::Duration;

use nocturne_suite::{App, AppConfig};
use serde_json::{Value, json};

/// How long a hit may take to appear. The detections judge on their own
/// task, after the audit entry is written, so the answer is polled for.
const PATIENCE: Duration = Duration::from_secs(20);

async fn eventually<F, Fut>(what: &str, mut look: F) -> Value
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<Value>>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Some(found) = look().await {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn list(app: &App, method: &str, params: Value) -> Vec<Value> {
    app.rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method}: {err:?}"))
        .as_array()
        .unwrap_or_else(|| panic!("{method} answers with a list"))
        .clone()
}

#[tokio::test]
async fn in_the_assembled_application_no_detection_is_without_its_source() {
    let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
    let catalogue = list(&app, "detections.list", Value::Null).await;
    assert!(
        !catalogue.is_empty(),
        "the catalogue is fixed and not empty"
    );
    let blind: Vec<String> = catalogue
        .iter()
        .filter(|entry| entry["available"] != true)
        .map(|entry| format!("{}: {}", entry["id"], entry["unavailable_reason"]))
        .collect();
    assert!(
        blind.is_empty(),
        "every feature a detection watches is in this build, so each must find \
         its source under the name it looks for; these did not: {blind:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_vault_passwords_become_one_hit_and_one_alert_in_the_real_monitor() {
    // On disk: only there does the vault have a master password to get wrong.
    let dir = tempfile::tempdir().unwrap();
    let app = nocturne_suite::start(AppConfig::at(dir.path())).unwrap();
    app.rpc("vault.set_password", json!({"new": "correct horse"}))
        .await
        .unwrap();
    app.rpc("vault.lock", Value::Null).await.unwrap();

    // The threshold is a setting; read it rather than assume it.
    let settings = app
        .rpc("settings.get", json!({"section": "detect"}))
        .await
        .unwrap();
    let needed = settings["failed_unlocks"]["count"]
        .as_u64()
        .expect("the detect section declares failed_unlocks.count");
    let guess = |n: u64| format!("not-the-password-{n}");
    for n in 0..needed {
        app.rpc("vault.unlock", json!({"password": guess(n)}))
            .await
            .expect_err("a wrong password is refused");
    }

    let hit = eventually("the detection", || async {
        list(&app, "detections.recent", json!({"id": "failed_unlocks"}))
            .await
            .into_iter()
            .next()
    })
    .await;
    assert_eq!(hit["evidence"]["what"], "vault", "{hit:#}");
    assert_eq!(hit["evidence"]["count"], needed, "{hit:#}");

    // Raised through the real monitor, under the key the specification gives,
    // once: the monitor's own de-duplication is part of what is being joined.
    let alert = eventually("the alert", || async {
        list(&app, "alerts.list", json!({"state": "open"}))
            .await
            .into_iter()
            .find(|alert| alert["dedup_key"] == "detect:failed_unlocks:-:vault")
    })
    .await;
    assert_eq!(alert["state"], "firing", "{alert:#}");
    assert_eq!(
        hit["alert_id"], alert["id"],
        "the hit names the alert it raised"
    );
    let same_key = list(&app, "alerts.list", Value::Null)
        .await
        .into_iter()
        .filter(|alert| alert["dedup_key"] == "detect:failed_unlocks:-:vault")
        .count();
    assert_eq!(same_key, 1);

    // What was tried is in none of it.
    let everything = format!(
        "{hit} {alert} {}",
        app.rpc("audit.search", json!({})).await.unwrap()
    );
    for n in 0..needed {
        assert!(
            !everything.contains(&guess(n)),
            "a password that was tried is in a hit, an alert or the audit trail"
        );
    }
}
