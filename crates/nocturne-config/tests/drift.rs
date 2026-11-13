//! Intended configuration, drift against it, and restoring an old version —
//! all of which end at `changes.create` and never at a device.

mod support;

use serde_json::json;
use support::{audits, backup_device, lab, lab_with_changes, load};

const RUNNING: &str = "\
hostname SW1
ip domain-name lab.example
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
line vty 0 4
 transport input ssh
";

#[tokio::test]
async fn an_intended_fragment_is_what_must_be_present_nothing_more() {
    let (lab, changes) = lab_with_changes(&["SW1"]).await;
    load(&lab, "SW1", RUNNING);
    backup_device(&lab, "SW1").await;

    let err = lab
        .app
        .rpc("config.drift", json!({"device_id": lab.id("SW1")}))
        .await
        .expect_err("there is nothing to compare with yet");
    assert_eq!(err.code, "not_found");
    assert!(err.message.contains("no intended configuration"), "{err}");

    let set = lab
        .app
        .rpc(
            "config.intended.set",
            json!({
                "device_id": lab.id("SW1"),
                "text": "ntp server 192.0.2.1\ninterface GigabitEthernet0/1\n description uplink\n storm-control broadcast level 5.00\n",
            }),
        )
        .await
        .expect("the intended configuration is stored");
    assert_eq!(set["exact"], false, "a new text is a fragment");
    assert_eq!(set["lines"], 4);
    assert_eq!(
        audits(&lab.app, "config.intended.set")[0].message,
        "Set the intended configuration of `SW1` (4 lines, a fragment)"
    );

    let drift = lab
        .app
        .rpc("config.drift", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the drift");
    assert_eq!(drift["supported"], true);
    assert_eq!(drift["in_sync"], false);
    assert_eq!(drift["exact"], false);
    assert_eq!(
        drift["commands"],
        json!([
            "interface GigabitEthernet0/1",
            "storm-control broadcast level 5.00",
            "exit",
            "ntp server 192.0.2.1"
        ])
    );
    // Nothing the device has that the fragment does not mention is removed.
    let removals: Vec<&str> = drift["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|command| command.as_str())
        .filter(|command| command.starts_with("no "))
        .collect();
    assert_eq!(removals, Vec::<&str>::new());
    assert!(drift["version_id"].as_i64().is_some());

    let handed = lab
        .app
        .rpc("config.drift.fix", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the fix is handed over");
    assert_eq!(handed["change_id"], "change-1");
    let call = changes.last();
    assert_eq!(call["origin"], "drift");
    assert_eq!(call["targets"][0]["device_id"], lab.id("SW1").as_str());
    assert_eq!(call["targets"][0]["intent"]["type"], "intended");
    assert_eq!(call["targets"][0]["intent"]["exact"], false);
    assert!(
        call["targets"][0]["intent"]["config"]
            .as_str()
            .unwrap()
            .contains("storm-control"),
        "{call}"
    );
    assert!(call["title"].as_str().unwrap().contains("drift on `SW1`"));
    assert_eq!(audits(&lab.app, "config.drift.fix").len(), 1);
}

#[tokio::test]
async fn an_exact_intended_configuration_also_removes_what_is_not_in_it() {
    let (lab, changes) = lab_with_changes(&["SW1"]).await;
    load(&lab, "SW1", RUNNING);
    backup_device(&lab, "SW1").await;
    lab.app
        .rpc(
            "config.intended.set",
            json!({
                "device_id": lab.id("SW1"),
                "text": "hostname SW1\nline vty 0 4\n transport input ssh\n",
                "exact": true,
            }),
        )
        .await
        .expect("the intended configuration is stored");

    let drift = lab
        .app
        .rpc("config.drift", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the drift");
    assert_eq!(drift["exact"], true);
    let commands: Vec<&str> = drift["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command.as_str().unwrap())
        .collect();
    assert!(
        commands.contains(&"no ip domain-name lab.example"),
        "{commands:?}"
    );
    // A physical interface is reset, not removed: `no interface` is refused
    // on a port that exists.
    assert!(
        commands.contains(&"default interface GigabitEthernet0/1"),
        "{commands:?}"
    );
    assert!(drift["diff"]["removed"].as_u64().unwrap() >= 3);

    lab.app
        .rpc("config.drift.fix", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the fix is handed over");
    assert_eq!(changes.last()["targets"][0]["intent"]["exact"], true);

    // A device that matches has nothing to fix, and says so without a change.
    lab.app
        .rpc(
            "config.intended.set",
            json!({"device_id": lab.id("SW1"), "text": RUNNING, "exact": true}),
        )
        .await
        .expect("the intended configuration is stored");
    let drift = lab
        .app
        .rpc("config.drift", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the drift");
    assert_eq!(drift["in_sync"], true);
    assert_eq!(drift["commands"], json!([]));
    let err = lab
        .app
        .rpc("config.drift.fix", json!({"device_id": lab.id("SW1")}))
        .await
        .expect_err("there is nothing to fix");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("matches its intended"), "{err}");
    assert_eq!(changes.calls().len(), 1, "no second change was created");
}

#[tokio::test]
async fn the_intended_configuration_can_be_cleared() {
    let lab = lab(&["SW1"]).await;
    let none = lab
        .app
        .rpc("config.intended.get", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the call succeeds");
    assert!(none.is_null(), "a device without one says so plainly");

    lab.app
        .rpc(
            "config.intended.set",
            json!({"device_id": lab.id("SW1"), "text": "ntp server 192.0.2.1\n"}),
        )
        .await
        .expect("it is stored");
    let cleared = lab
        .app
        .rpc("config.intended.clear", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("it is removed");
    assert_eq!(cleared["cleared"], true);
    let cleared = lab
        .app
        .rpc("config.intended.clear", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("clearing twice is not an error");
    assert_eq!(cleared["cleared"], false);

    let err = lab
        .app
        .rpc(
            "config.intended.set",
            json!({"device_id": lab.id("SW1"), "text": "   \n"}),
        )
        .await
        .expect_err("an empty text is not an intended configuration");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("config.intended.clear"), "{err}");
}

#[tokio::test]
async fn a_restore_is_a_change_computed_by_diff_not_a_blind_overwrite() {
    let (lab, changes) = lab_with_changes(&["SW1"]).await;
    load(&lab, "SW1", RUNNING);
    backup_device(&lab, "SW1").await;
    let old = lab
        .app
        .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("versions")[0]["id"]
        .as_i64()
        .unwrap();

    support::configure(&lab, "SW1", &["ntp server 192.0.2.9"], false).await;
    backup_device(&lab, "SW1").await;

    let handed = lab
        .app
        .rpc("config.restore", json!({"version_id": old}))
        .await
        .expect("the restore is handed over");
    assert_eq!(handed["change_id"], "change-1");
    let call = changes.last();
    assert_eq!(call["origin"], "restore");
    assert_eq!(call["targets"].as_array().unwrap().len(), 1);
    assert_eq!(call["targets"][0]["intent"]["type"], "intended");
    assert_eq!(
        call["targets"][0]["intent"]["exact"], true,
        "a restore is the whole configuration"
    );
    let wanted = call["targets"][0]["intent"]["config"].as_str().unwrap();
    assert!(wanted.contains("hostname SW1"));
    assert!(!wanted.contains("ntp server 192.0.2.9"));
    assert!(
        call["description"]
            .as_str()
            .unwrap()
            .contains("computed by the change pipeline"),
        "{call}"
    );
    assert!(call["title"].as_str().unwrap().contains("Restore `SW1`"));
    assert_eq!(audits(&lab.app, "config.restore").len(), 1);

    // The device was not touched by any of it.
    assert!(
        lab.fake("SW1")
            .running_config()
            .contains("ntp server 192.0.2.9")
    );

    let err = lab
        .app
        .rpc("config.restore", json!({"version_id": 9999}))
        .await
        .expect_err("there is no such version");
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn a_hand_off_without_the_change_pipeline_fails_cleanly() {
    let lab = lab(&["SW1"]).await;
    load(&lab, "SW1", RUNNING);
    backup_device(&lab, "SW1").await;
    lab.app
        .rpc(
            "config.intended.set",
            json!({"device_id": lab.id("SW1"), "text": "ntp server 192.0.2.1\n"}),
        )
        .await
        .expect("the intended configuration is stored");

    for (method, params) in [
        ("config.drift.fix", json!({"device_id": lab.id("SW1")})),
        ("config.restore", json!({"version_id": 1})),
    ] {
        let err = lab
            .app
            .rpc(method, params)
            .await
            .expect_err("there is no change pipeline in this build");
        assert_eq!(err.code, "unavailable", "{method}");
        assert!(err.message.contains("`changes.create`"), "{method}: {err}");
        assert!(
            err.message.contains("Nothing was changed"),
            "{method}: {err}"
        );
    }
    assert!(audits(&lab.app, "config.drift.fix").is_empty());
    assert!(audits(&lab.app, "config.restore").is_empty());
}

#[tokio::test]
async fn drift_on_a_device_type_whose_grammar_is_unknown_is_shown_but_not_derived() {
    let (lab, changes) = lab_with_changes(&["SW1"]).await;
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "FW1", "address": "10.0.0.7", "profile": "generic"}),
        )
        .await
        .expect("the device is created");
    let fw = lab
        .app
        .rpc("devices.get", json!({"id": "FW1"}))
        .await
        .expect("the device")["id"]
        .as_str()
        .unwrap()
        .to_string();
    lab.app
        .store()
        .config_insert(&fw, "running", "set system host-name FW1\n", "import", "")
        .expect("a stored configuration");
    lab.app
        .rpc(
            "config.intended.set",
            json!({"device_id": &fw, "text": "set system host-name FW1\nset system ntp server 192.0.2.1\n"}),
        )
        .await
        .expect("the intended configuration is stored");

    let drift = lab
        .app
        .rpc("config.drift", json!({"device_id": &fw}))
        .await
        .expect("the drift");
    assert_eq!(drift["supported"], false);
    assert_eq!(drift["commands"], json!([]));
    assert!(!drift["warnings"].as_array().unwrap().is_empty());
    assert_eq!(drift["diff"]["added"], 1, "the difference is still shown");

    let err = lab
        .app
        .rpc("config.drift.fix", json!({"device_id": &fw}))
        .await
        .expect_err("the commands cannot be derived");
    assert_eq!(err.code, "unsupported");
    assert!(err.message.contains("not modelled"), "{err}");
    assert!(changes.calls().is_empty());
}
