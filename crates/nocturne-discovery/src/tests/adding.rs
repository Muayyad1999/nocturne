//! Turning finds into devices: the preview, the import, and the ignore list.

use nocturne_app::model::{Device, Transport, collections::DEVICES};
use serde_json::json;

use super::support::{self, FakeHost};
use crate::probe::SysInfo;

/// A scan whose finds cover every naming and transport case.
async fn scanned() -> (nocturne_app::testing::Lab, String, String) {
    let (lab, probes) = support::lab(&[]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;
    probes
        // Names itself over SNMP, and speaks SSH.
        .host(
            "10.0.0.1",
            FakeHost::alive()
                .port(22, Some("SSH-2.0-TestIOS"))
                .snmp("public", support::ios_sysinfo("CORE1"))
                .ptr("core1.campus.example"),
        )
        // No SNMP name, but reverse DNS knows it; Telnet only.
        .host(
            "10.0.0.2",
            FakeHost::alive()
                .port(23, None)
                .ptr("old-sw.campus.example"),
        )
        // Nothing names it and nothing lets us in.
        .host("10.0.0.3", FakeHost::alive().port(443, None))
        // Names itself the same as the first one.
        .host(
            "10.0.0.4",
            FakeHost::alive()
                .port(22, Some("SSH-2.0-TestIOS"))
                .snmp("public", support::ios_sysinfo("CORE1")),
        );

    let (scan_id, job) = support::scan(
        &lab.app,
        json!({
            "targets": "10.0.0.1-10.0.0.4",
            "ports": "22,23,443",
            "snmp_credential_ids": [community],
        }),
    )
    .await
    .unwrap();
    assert_eq!(job.state, nocturne_app::jobs::JobState::Succeeded);
    (lab, scan_id, community)
}

#[tokio::test]
async fn the_preview_says_what_would_be_created_and_why_before_anything_is() {
    let (lab, scan_id, community) = scanned().await;
    let preview = lab
        .app
        .rpc(
            "discovery.preview",
            json!({"scan_id": scan_id, "defaults": {"site": "HQ", "tags": ["found"]}}),
        )
        .await
        .unwrap();
    assert_eq!(preview["new"], 4);
    assert_eq!(preview["skipped"], 0);

    let rows = preview["rows"].as_array().unwrap();
    let row = |address: &str| {
        rows.iter()
            .find(|row| row["address"] == address)
            .unwrap_or_else(|| panic!("a row for {address}"))
    };
    assert_eq!(row("10.0.0.1")["name"], "CORE1");
    assert_eq!(row("10.0.0.1")["transport"], "ssh");
    assert_eq!(row("10.0.0.1")["profile"], "test_ios");
    assert_eq!(row("10.0.0.1")["site"], "HQ");
    assert_eq!(row("10.0.0.1")["tags"], json!(["found"]));
    assert_eq!(row("10.0.0.1")["monitor_snmp"], true);
    assert_eq!(row("10.0.0.1")["snmp_credential_id"], community);

    assert_eq!(
        row("10.0.0.2")["name"],
        "old-sw",
        "the short form of the reverse-DNS name"
    );
    assert_eq!(row("10.0.0.2")["transport"], "telnet");
    assert_eq!(row("10.0.0.2")["monitor_snmp"], false);
    assert_eq!(row("10.0.0.2")["profile"], "");

    assert_eq!(row("10.0.0.3")["name"], "10.0.0.3");
    assert_eq!(row("10.0.0.3")["transport"], "none");

    assert_eq!(
        row("10.0.0.4")["name"],
        "CORE1 (10.0.0.4)",
        "two devices calling themselves the same thing are told apart by address"
    );

    // Nothing was created by previewing.
    assert!(lab.app.store().list::<Device>(DEVICES).unwrap().is_empty());
}

#[tokio::test]
async fn adding_creates_the_devices_the_preview_promised_and_logs_in_nowhere() {
    let (lab, scan_id, community) = scanned().await;
    let mut events = support::Events::new(&lab.app);
    let added = lab
        .app
        .rpc(
            "discovery.add",
            json!({
                "scan_id": scan_id,
                "addresses": ["10.0.0.1", "10.0.0.2", "10.0.0.3"],
                "defaults": {"site": "HQ", "role": "access", "tags": ["found"]},
            }),
        )
        .await
        .unwrap();
    assert_eq!(added["created"].as_array().unwrap().len(), 3);
    assert!(added["skipped"].as_array().unwrap().is_empty());

    let devices: Vec<Device> = lab.app.store().list(DEVICES).unwrap();
    let device = |address: &str| {
        devices
            .iter()
            .find(|device| device.address == address)
            .unwrap_or_else(|| panic!("a device at {address}"))
    };
    let core = device("10.0.0.1");
    assert_eq!(core.name, "CORE1");
    assert_eq!(core.profile, "test_ios");
    assert_eq!(core.transport, Transport::Ssh);
    assert_eq!(core.site, "HQ");
    assert_eq!(core.role, "access");
    assert_eq!(core.tags, ["found"]);
    assert!(core.monitor.enabled && core.monitor.icmp && core.monitor.snmp);
    assert_eq!(
        core.monitor.snmp_credential_id.as_deref(),
        Some(community.as_str())
    );
    assert_eq!(
        core.credential_id, None,
        "no login credential was asked for, so none is attached"
    );
    assert!(
        core.notes.contains("Added by discovery") && core.notes.contains("sysName"),
        "the device keeps the evidence it was created from:\n{}",
        core.notes
    );

    assert_eq!(device("10.0.0.2").transport, Transport::Telnet);
    assert!(
        !device("10.0.0.2").monitor.snmp,
        "nothing answered SNMP there, so SNMP monitoring is not switched on"
    );
    assert_eq!(device("10.0.0.3").transport, Transport::None);

    // The whole scan is still readable and now knows the devices exist.
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(support::host_at(&hosts, "10.0.0.1")["device_name"], "CORE1");
    assert_eq!(
        support::host_at(&hosts, "10.0.0.4")["device_id"],
        json!(null)
    );

    assert!(
        support::audits(&lab.app, "discovery.add")
            .iter()
            .any(|message| message.contains("3 device(s)")),
        "{:?}",
        support::audits(&lab.app, "discovery.add")
    );
    assert_eq!(events.of("discovery.added").len(), 1);
    assert!(
        events
            .of("devices.changed")
            .iter()
            .any(|payload| payload["change"] == "created")
    );
}

#[tokio::test]
async fn adding_everything_new_skips_what_is_already_managed_and_what_is_ignored() {
    let (lab, scan_id, _community) = scanned().await;
    lab.app
        .rpc(
            "discovery.add",
            json!({"scan_id": scan_id, "addresses": ["10.0.0.1"], "defaults": {}}),
        )
        .await
        .unwrap();
    lab.app
        .rpc(
            "discovery.ignore",
            json!({"addresses": ["10.0.0.3"], "note": "a printer"}),
        )
        .await
        .unwrap();

    let preview = lab
        .app
        .rpc("discovery.preview", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    let addresses: Vec<&str> = preview["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        addresses,
        ["10.0.0.2", "10.0.0.4"],
        "`add everything new` leaves out the managed and the ignored"
    );

    // Asking for them by name says why each is skipped rather than silently
    // doing nothing.
    let added = lab
        .app
        .rpc(
            "discovery.add",
            json!({"scan_id": scan_id, "addresses": ["10.0.0.1", "10.0.0.3", "10.0.0.2"]}),
        )
        .await
        .unwrap();
    assert_eq!(added["created"].as_array().unwrap().len(), 1);
    let skipped = added["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert!(
        skipped.iter().any(|row| row["address"] == "10.0.0.1"
            && row["reason"]
                .as_str()
                .unwrap()
                .contains("already in the inventory")),
        "{skipped:?}"
    );
    assert!(
        skipped.iter().any(|row| row["address"] == "10.0.0.3"
            && row["reason"].as_str().unwrap().contains("ignore list")),
        "{skipped:?}"
    );
}

#[tokio::test]
async fn the_ignore_list_is_listed_reversible_and_only_takes_addresses() {
    let (lab, probes) = support::lab(&[]).await;
    probes.host("10.0.0.7", FakeHost::alive());

    lab.app
        .rpc(
            "discovery.ignore",
            json!({"addresses": ["10.0.0.7"], "note": "the lift controller"}),
        )
        .await
        .unwrap();
    let listed = lab.app.rpc("discovery.ignored", json!({})).await.unwrap();
    assert_eq!(listed[0]["address"], "10.0.0.7");
    assert_eq!(listed[0]["note"], "the lift controller");
    assert!(listed[0]["at"].as_i64().unwrap() > 0);

    // A later scan offers it, but marked, so a person can see why it is dim.
    let (scan_id, _) = support::scan(&lab.app, json!({"targets": "10.0.0.7", "ports": ""}))
        .await
        .unwrap();
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(support::host_at(&hosts, "10.0.0.7")["ignored"], true);

    assert_eq!(
        lab.app
            .rpc("discovery.unignore", json!({"addresses": ["10.0.0.7"]}))
            .await
            .unwrap()["unignored"],
        json!(["10.0.0.7"])
    );
    assert!(
        lab.app
            .rpc("discovery.ignored", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    // The find is offered again at once, without rescanning.
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(support::host_at(&hosts, "10.0.0.7")["ignored"], false);

    let error = lab
        .app
        .rpc("discovery.ignore", json!({"addresses": ["sw1.example"]}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("not an IP address"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn an_address_that_is_not_part_of_the_scan_is_an_error_not_an_invention() {
    let (lab, scan_id, _community) = scanned().await;
    let error = lab
        .app
        .rpc(
            "discovery.add",
            json!({"scan_id": scan_id, "addresses": ["10.9.9.9"]}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
    assert!(error.message.contains("10.9.9.9"), "{}", error.message);

    let error = lab
        .app
        .rpc("discovery.preview", json!({"scan_id": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
}

#[tokio::test]
async fn the_import_may_override_the_transport_and_attach_a_login_credential() {
    let (lab, probes) = support::lab(&[]).await;
    let login = lab
        .app
        .rpc(
            "credentials.create",
            json!({"name": "campus-cli", "username": "admin", "password": "pw"}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    probes.host(
        "10.0.0.8",
        FakeHost::alive().snmp(
            "public",
            SysInfo {
                sys_name: Some("sw-8".into()),
                ..SysInfo::default()
            },
        ),
    );

    let (scan_id, _) = support::scan(&lab.app, json!({"targets": "10.0.0.8", "ports": ""}))
        .await
        .unwrap();
    let added = lab
        .app
        .rpc(
            "discovery.add",
            json!({
                "scan_id": scan_id,
                "defaults": {"transport": "ssh", "credential_id": login},
            }),
        )
        .await
        .unwrap();
    assert_eq!(added["created"].as_array().unwrap().len(), 1);

    let devices: Vec<Device> = lab.app.store().list(DEVICES).unwrap();
    assert_eq!(devices[0].transport, Transport::Ssh);
    assert_eq!(devices[0].credential_id.as_deref(), Some(login.as_str()));
    assert_eq!(
        devices[0].name, "10.0.0.8",
        "no SNMP credential was offered, so nothing named it"
    );
    assert!(
        !devices[0].monitor.snmp,
        "a CLI credential is not an SNMP credential"
    );
}
