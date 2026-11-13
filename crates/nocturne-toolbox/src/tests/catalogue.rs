//! The catalogue and the settings: the two things a UI reads before anything
//! else, and the two most likely to drift.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support;
use crate::catalogue::CATALOGUE;

#[tokio::test]
async fn the_catalogue_and_the_methods_are_the_same_list() {
    let lab = support::lab(&[]).await;
    let methods = lab.app.methods();

    for entry in CATALOGUE {
        assert!(
            methods.contains(&entry.id),
            "the catalogue offers `{}`, which is not a method",
            entry.id
        );
        assert!(!entry.title.is_empty() && !entry.description.is_empty());
        assert!(!entry.group.is_empty());
    }
    // And the other way round: nothing this crate registers is missing from
    // the catalogue, or the UI and the assistant cannot find it.
    let listed: Vec<&str> = CATALOGUE.iter().map(|entry| entry.id).collect();
    for method in methods {
        let ours = method.starts_with("tools.")
            || method.starts_with("probe.")
            || method.starts_with("tftp.");
        if ours && method != "tools.list" {
            assert!(
                listed.contains(&method),
                "`{method}` is registered but not in the catalogue"
            );
        }
    }

    let served = lab.app.rpc("tools.list", Value::Null).await.unwrap();
    assert_eq!(served.as_array().unwrap().len(), CATALOGUE.len());
    assert_eq!(served[0]["id"], "tools.subnet");
    // Offline is the distinction a caller needs: arithmetic or packets.
    let offline = served
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["offline"] == true)
        .count();
    assert!(offline > 20, "{offline} offline tools");
    assert!(
        served
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "probe.ping" && entry["job"] == true),
    );
}

#[tokio::test]
async fn the_settings_section_is_declared_and_brought_into_range_on_read() {
    let lab = support::lab(&[]).await;
    let section = lab
        .app
        .rpc("settings.get", json!({"section": "toolbox"}))
        .await
        .unwrap();
    assert_eq!(section["probe"]["ping_count"], 4);
    assert_eq!(section["sweep"]["allow_public"], false);
    assert_eq!(section["tftp"]["port"], 6969);
    assert_eq!(section["syslog"]["port"], 5515);
    assert_eq!(section["keep_runs"], 200);

    // A number that would do harm is clamped when it is read, not when it is
    // stored: the settings screen is not the place to enforce this.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "toolbox", "value": {
                "keep_runs": 999_999,
                "probe": {"max_run_s": 86_400, "ping_count": 0},
                "sweep": {"max_hosts": 10_000_000}
            }}),
        )
        .await
        .unwrap();
    let settings = crate::settings::ToolboxSettings::load(&lab.app);
    assert_eq!(settings.keep_runs, 2000);
    assert_eq!(settings.probe.max_run_s, crate::settings::MAX_RUN_S);
    assert_eq!(settings.probe.ping_count, 1);
    assert_eq!(settings.sweep.max_hosts, crate::settings::MAX_SWEEP_HOSTS);
}
