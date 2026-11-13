//! Reading what is stored: search across the network, comparisons, export,
//! and the coverage table.

mod support;

use nocturne_app::App;
use serde_json::json;
use support::{backup_device, lab, load};

fn store(app: &App, device_id: &str, text: &str) -> i64 {
    app.store()
        .config_insert(device_id, "running", text, "manual", "")
        .expect("the configuration is stored")
        .0
        .id
}

const CONFIG: &str = "\
!
hostname SW1
!
vlan 10
 name users
vlan 99
 name quarantine
!
interface GigabitEthernet0/1
 description uplink to CORE1
 switchport mode trunk
 switchport trunk allowed vlan 10,99
!
interface GigabitEthernet0/2
 description desk 12
 switchport mode access
 switchport access vlan 10
!
ntp server 192.0.2.1
end
";

#[tokio::test]
async fn search_finds_lines_with_their_section_and_context() {
    let lab = lab(&["SW1", "SW2"]).await;
    store(&lab.app, &lab.id("SW1"), CONFIG);
    store(
        &lab.app,
        &lab.id("SW2"),
        &CONFIG.replace("hostname SW1", "hostname SW2"),
    );

    let found = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "switchport access vlan 10", "context": 1}),
        )
        .await
        .expect("the search");
    assert_eq!(found["devices_searched"], 2);
    assert_eq!(found["devices_matched"], 2);
    assert_eq!(found["truncated"], false);
    let first = &found["matches"][0];
    assert_eq!(first["name"], "SW1");
    assert_eq!(first["lab"], true, "a lab device says so");
    assert_eq!(first["line"], " switchport access vlan 10");
    assert_eq!(first["line_no"], 17);
    assert_eq!(first["section"], json!(["interface GigabitEthernet0/2"]));
    assert_eq!(first["before"], json!([" switchport mode access"]));
    assert_eq!(first["after"], json!(["!"]));

    // A regular expression, and the case-sensitive form.
    let found = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "^vlan \\d+$", "regex": true}),
        )
        .await
        .expect("the search");
    assert_eq!(found["matches"].as_array().unwrap().len(), 4);
    let found = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "HOSTNAME SW1", "case": true}),
        )
        .await
        .expect("the search");
    assert_eq!(found["matches"], json!([]));
    let found = lab
        .app
        .rpc("config.search", json!({"query": "HOSTNAME SW1"}))
        .await
        .expect("the search");
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);

    // A device with no backup is counted, not guessed at.
    let fresh = support::lab(&["SW1", "SW2"]).await;
    store(&fresh.app, &fresh.id("SW1"), CONFIG);
    let found = fresh
        .app
        .rpc("config.search", json!({"query": "hostname"}))
        .await
        .expect("the search");
    assert_eq!(found["devices_searched"], 1);
    assert_eq!(found["devices_without_backup"], 1);

    // An unusable pattern is refused with the reason.
    let err = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "interface (Gi", "regex": true}),
        )
        .await
        .expect_err("the pattern is broken");
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("not a valid regular expression"),
        "{err}"
    );
    let err = lab
        .app
        .rpc("config.search", json!({"query": "   "}))
        .await
        .expect_err("there is nothing to search for");
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn a_search_stops_at_its_limit_instead_of_reading_every_device() {
    let lab = lab(&["SW1"]).await;
    // Two hundred devices, each with a configuration of its own.
    let mut ids = Vec::new();
    for index in 0..200 {
        let device = lab
            .app
            .rpc(
                "devices.create",
                json!({"name": format!("R{index:03}"), "address": format!("10.1.0.{index}"), "profile": "cisco_ios"}),
            )
            .await
            .expect("the device is created");
        let id = device["id"].as_str().unwrap().to_string();
        let mut text = String::from("hostname R\n");
        for line in 0..200 {
            text.push_str(&format!("ip route 10.{line}.0.0 255.255.0.0 10.0.0.1\n"));
        }
        store(&lab.app, &id, &text);
        ids.push(id);
    }

    let found = lab
        .app
        .rpc("config.search", json!({"query": "ip route", "limit": 50}))
        .await
        .expect("the search");
    assert_eq!(found["matches"].as_array().unwrap().len(), 50);
    assert_eq!(found["truncated"], true);
    // It stopped at the first device that filled the page rather than reading
    // all two hundred configurations.
    assert_eq!(
        found["devices_searched"], 1,
        "{}",
        found["devices_searched"]
    );
    assert_eq!(found["devices_matched"], 1);

    // Without a limit in the way, every device is counted.
    let found = lab
        .app
        .rpc(
            "config.search",
            json!({"query": "hostname R", "limit": 500}),
        )
        .await
        .expect("the search");
    assert_eq!(found["devices_matched"], 200);
    assert_eq!(found["truncated"], false);
}

#[tokio::test]
async fn config_missing_names_the_devices_that_lack_a_line() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    store(&lab.app, &lab.id("SW1"), CONFIG);
    store(
        &lab.app,
        &lab.id("SW2"),
        &CONFIG.replace("ntp server 192.0.2.1", "ntp server 198.51.100.1"),
    );

    let missing = lab
        .app
        .rpc("config.missing", json!({"query": "ntp server 192.0.2.1"}))
        .await
        .expect("the report");
    assert_eq!(missing["devices_searched"], 2);
    let names: Vec<&str> = missing["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|device| device["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["SW2"]);
    assert_eq!(
        missing["without_backup"][0]["name"], "SW3",
        "a device with no backup is not said to be missing the line"
    );
    assert!(missing["devices"][0]["fetched_at"].as_i64().unwrap() > 0);

    // Spacing does not matter; a pattern is asked for explicitly.
    let missing = lab
        .app
        .rpc(
            "config.missing",
            json!({"query": "  ntp   server   192.0.2.1  "}),
        )
        .await
        .expect("the report");
    assert_eq!(missing["devices"].as_array().unwrap().len(), 1);
    let missing = lab
        .app
        .rpc(
            "config.missing",
            json!({"query": "^ntp server \\S+", "regex": true}),
        )
        .await
        .expect("the report");
    assert_eq!(missing["devices"], json!([]));

    let err = lab
        .app
        .rpc("config.missing", json!({"query": "a\nb"}))
        .await
        .expect_err("more than one line");
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn versions_are_paged_newest_first() {
    let lab = lab(&["SW1"]).await;
    let mut ids = Vec::new();
    for index in 0..5 {
        ids.push(store(
            &lab.app,
            &lab.id("SW1"),
            &format!("{CONFIG}ip route 10.{index}.0.0 255.255.0.0 10.0.0.1\n"),
        ));
    }

    let page = lab
        .app
        .rpc(
            "config.versions",
            json!({"device_id": lab.id("SW1"), "limit": 2}),
        )
        .await
        .expect("the first page");
    let first: Vec<i64> = page
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version["id"].as_i64().unwrap())
        .collect();
    assert_eq!(first, vec![ids[4], ids[3]]);

    let page = lab
        .app
        .rpc(
            "config.versions",
            json!({"device_id": lab.id("SW1"), "limit": 2, "before_id": ids[3]}),
        )
        .await
        .expect("the next page");
    let next: Vec<i64> = page
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version["id"].as_i64().unwrap())
        .collect();
    assert_eq!(next, vec![ids[2], ids[1]]);

    let err = lab
        .app
        .rpc("config.get", json!({"version_id": 9999}))
        .await
        .expect_err("there is no such version");
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn two_devices_are_compared_by_their_latest_configuration() {
    let lab = lab(&["SW1", "SW2"]).await;
    store(&lab.app, &lab.id("SW1"), CONFIG);
    store(
        &lab.app,
        &lab.id("SW2"),
        &CONFIG
            .replace("hostname SW1", "hostname SW2")
            .replace(" switchport access vlan 10", " switchport access vlan 99"),
    );

    let compare = lab
        .app
        .rpc(
            "config.compare",
            json!({"a": {"device_id": "SW1"}, "b": {"device_id": "SW2"}}),
        )
        .await
        .expect("the comparison");
    assert_eq!(compare["a"]["device_id"], lab.id("SW1").as_str());
    assert_eq!(compare["b"]["device_id"], lab.id("SW2").as_str());
    assert_eq!(compare["diff"]["added"], 2);
    assert_eq!(compare["diff"]["removed"], 2);
    let sections: Vec<&str> = compare["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| {
            section["path"]
                .as_array()
                .unwrap()
                .first()
                .and_then(|line| line.as_str())
                .unwrap_or("")
        })
        .collect();
    assert_eq!(sections, vec!["", "interface GigabitEthernet0/2"]);

    // One side may be a particular version instead of the latest.
    let older = store(&lab.app, &lab.id("SW1"), "hostname SW1\n");
    let compare = lab
        .app
        .rpc(
            "config.compare",
            json!({"a": {"version_id": older}, "b": {"device_id": "SW2"}}),
        )
        .await
        .expect("the comparison");
    assert_eq!(compare["a"]["id"], older);

    let err = lab
        .app
        .rpc(
            "config.compare",
            json!({"a": {"device_id": "SW1", "version_id": 1}, "b": {"device_id": "SW2"}}),
        )
        .await
        .expect_err("a side is either a device or a version");
    assert_eq!(err.code, "invalid");

    let err = lab
        .app
        .rpc("config.diff", json!({"device_id": lab.id("SW2")}))
        .await
        .expect_err("there is only one version");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("only one stored"), "{err}");
}

#[tokio::test]
async fn the_coverage_table_shows_what_is_backed_up() {
    let lab = lab(&["SW1", "SW2"]).await;
    backup_device(&lab, "SW1").await;

    let devices = lab
        .app
        .rpc("config.devices", json!({}))
        .await
        .expect("the table");
    let rows = devices.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let sw1 = &rows[0];
    assert_eq!(sw1["name"], "SW1");
    assert_eq!(sw1["profile"], "test_ios");
    assert_eq!(sw1["versions"], 2, "running and startup");
    assert!(sw1["running"]["sha256"].as_str().unwrap().len() == 64);
    assert!(sw1["running"]["fetched_at"].as_i64().unwrap() > 0);
    assert!(
        sw1["last_change_at"].is_null(),
        "a first backup is not a change"
    );
    assert!(rows[1]["running"].is_null());

    // After a change there is a time to show for it.
    support::configure(&lab, "SW1", &["ntp server 192.0.2.1"], false).await;
    backup_device(&lab, "SW1").await;
    let devices = lab
        .app
        .rpc("config.devices", json!({"device_ids": ["SW1"]}))
        .await
        .expect("the table");
    assert!(devices[0]["last_change_at"].as_i64().unwrap() > 0);
    assert_eq!(devices[0]["versions"], 3);
}

#[tokio::test]
async fn an_export_writes_one_file_per_device_and_kind() {
    let lab = lab(&["SW1", "SW2"]).await;
    load(&lab, "SW1", "hostname SW1\nntp server 192.0.2.1\n");
    backup_device(&lab, "SW1").await;

    let dir = tempfile::tempdir().expect("a temporary folder");
    let into = dir.path().join("export");
    let result = lab
        .app
        .rpc("config.export", json!({"dir": into.to_str().unwrap()}))
        .await
        .expect("the export runs");
    assert_eq!(result["files"], 2);
    assert_eq!(result["devices"], 1);
    assert_eq!(result["skipped"][0]["name"], "SW2");
    assert_eq!(result["skipped"][0]["reason"], "no backup yet");
    let text = std::fs::read_to_string(into.join("SW1.running.cfg")).expect("the file");
    assert!(text.contains("ntp server 192.0.2.1"), "{text}");
    assert!(into.join("SW1.startup.cfg").is_file());

    // Exporting again into the same folder is allowed and tidies up after itself.
    lab.app
        .rpc(
            "devices.update",
            json!({"id": lab.id("SW1"), "name": "SW1-RENAMED"}),
        )
        .await
        .expect("the device is renamed");
    let result = lab
        .app
        .rpc("config.export", json!({"dir": into.to_str().unwrap()}))
        .await
        .expect("the export runs");
    assert_eq!(result["removed"], 2, "the files of the old name are gone");
    assert!(into.join("SW1-RENAMED.running.cfg").is_file());
    assert!(!into.join("SW1.running.cfg").exists());

    // A folder with someone else's files in it is refused.
    let other = dir.path().join("mine");
    std::fs::create_dir_all(&other).expect("the folder");
    std::fs::write(other.join("notes.txt"), "do not delete me").expect("a file");
    let err = lab
        .app
        .rpc("config.export", json!({"dir": other.to_str().unwrap()}))
        .await
        .expect_err("the folder is in use");
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("not empty"), "{err}");
    assert!(other.join("notes.txt").is_file(), "nothing was touched");

    let err = lab
        .app
        .rpc("config.export", json!({"dir": "relative/path"}))
        .await
        .expect_err("a relative path is refused");
    assert_eq!(err.code, "invalid");
}
