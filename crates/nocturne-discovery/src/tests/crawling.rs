//! Discovery from what is already known: neighbour tables, ARP caches and
//! routing tables of devices already in the inventory.
//!
//! The lab is three fake switches whose CDP tables point at each other, and
//! whose management addresses Nocturne does not know yet. Reading a table is
//! a real CLI session against the fake device through the core's collector.

use nocturne_app::jobs::JobState;
use nocturne_app::model::{Device, collections::DEVICES};
use nocturne_app::testing::{FakeNeighbor, Lab};
use serde_json::{Value, json};

use super::support::{self, FakeHost, FakeProbes};
use std::sync::Arc;

/// SW1 knows SW2, SW2 knows SW1 and SW3, SW3 knows SW2 — a chain, with a
/// loop back along it, which is what a real campus looks like.
async fn chain() -> (Lab, Arc<FakeProbes>, String) {
    let (lab, probes) = support::lab(&["SW1", "SW2", "SW3"]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;

    neighbours(
        &lab,
        "SW1",
        &[(
            "GigabitEthernet0/1",
            "SW2.campus.example",
            "10.0.0.2",
            "Router Switch IGMP",
        )],
    );
    neighbours(
        &lab,
        "SW2",
        &[
            (
                "GigabitEthernet0/1",
                "SW1.campus.example",
                "10.0.0.1",
                "Switch IGMP",
            ),
            (
                "GigabitEthernet0/2",
                "SW3.campus.example",
                "10.0.0.3",
                "Switch IGMP",
            ),
        ],
    );
    neighbours(
        &lab,
        "SW3",
        &[(
            "GigabitEthernet0/1",
            "SW2.campus.example",
            "10.0.0.2",
            "Switch IGMP",
        )],
    );

    // Only SW1 is managed, and by an address none of the tables mention.
    let sw1 = lab.id("SW1");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": sw1, "address": "192.168.1.1", "site": "HQ"}),
        )
        .await
        .unwrap();
    // SW2 and SW3 are not in the inventory for this test: they are what the
    // crawl is supposed to find.
    let ids = [lab.id("SW2"), lab.id("SW3")].to_vec();
    assert_eq!(nocturne_app::inventory::delete(&lab.app, &ids).unwrap(), 2);

    probes
        .host(
            "10.0.0.2",
            FakeHost::alive()
                .port(22, Some("SSH-2.0-TestIOS"))
                .snmp("public", support::ios_sysinfo("SW2")),
        )
        .host(
            "10.0.0.3",
            FakeHost::alive()
                .port(22, Some("SSH-2.0-TestIOS"))
                .snmp("public", support::ios_sysinfo("SW3")),
        );
    (lab, probes, community)
}

fn neighbours(lab: &Lab, name: &str, entries: &[(&str, &str, &str, &str)]) {
    lab.fake(name).with(|state| {
        state.neighbors.clear();
        for (local, device, address, _capabilities) in entries {
            state.neighbors.push(FakeNeighbor {
                local_interface: (*local).to_string(),
                device: (*device).to_string(),
                remote_interface: "GigabitEthernet1/0/1".to_string(),
                address: (*address).to_string(),
                platform: "cisco WS-C3850-24T".to_string(),
            });
        }
        // The core's fake renders `Capabilities: Switch IGMP` itself; the
        // test profile parses whatever is between the commas, so a device
        // with a different capability line needs its own canned output.
        let rendered = state.cdp_neighbors_detail();
        let patched = entries.iter().fold(rendered, |text, (_, device, _, caps)| {
            replace_capabilities(&text, device, caps)
        });
        state
            .outputs
            .insert("show cdp neighbors detail".to_string(), patched);
    });
}

/// Put the test's capability string into the fake's rendered CDP output.
fn replace_capabilities(text: &str, device: &str, capabilities: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("Device ID: ") {
            inside = line.trim_end() == format!("Device ID: {device}");
        }
        if inside && line.contains("Capabilities:") {
            let platform = line.split(",  Capabilities:").next().unwrap_or_default();
            out.push_str(&format!("{platform},  Capabilities: {capabilities} \n"));
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

async fn crawl(lab: &Lab, params: Value) -> (String, nocturne_app::jobs::Job) {
    let started = lab.app.rpc("discovery.crawl", params).await.unwrap();
    let scan_id = started["scan_id"].as_str().unwrap().to_string();
    let job_id = started["job_id"].as_str().unwrap().to_string();
    let job = nocturne_app::jobs::wait(&lab.app, &job_id).await.unwrap();
    (scan_id, job)
}

#[tokio::test]
async fn a_crawl_finds_the_neighbours_of_a_seed_and_says_which_device_told_it() {
    let (lab, _probes, community) = chain().await;
    let sw1 = lab.id("SW1");

    let (scan_id, job) = crawl(
        &lab,
        json!({
            "seed_device_ids": [sw1],
            "depth": 1,
            "credential_ids": [community],
            "use_arp": false,
        }),
    )
    .await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.log
    );

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let addresses: Vec<&str> = hosts
        .iter()
        .map(|host| host["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        addresses,
        ["10.0.0.2"],
        "one level out from SW1 is SW2 and nothing else"
    );

    let sw2 = support::host_at(&hosts, "10.0.0.2");
    assert_eq!(sw2["told_by"]["device_name"], "SW1");
    assert_eq!(sw2["told_by"]["protocol"], "cdp");
    assert_eq!(sw2["told_by"]["local_interface"], "GigabitEthernet0/1");
    assert_eq!(sw2["told_by"]["reported_name"], "SW2.campus.example");
    assert_eq!(sw2["path"], json!(["SW1"]));
    assert_eq!(sw2["profile"]["id"], "test_ios");
    assert_eq!(sw2["suggested"]["name"], "SW2");
    assert_eq!(
        sw2["suggested"]["role"], "router",
        "SW1 reported it as a router, which is the only honest source of a role"
    );
    assert_eq!(
        sw2["suggested"]["site"], "HQ",
        "a crawled find inherits the seed's site"
    );
    assert!(
        support::evidence_for(sw2, "told_by")[0].contains("SW1` listed it in its cdp table"),
        "{:?}",
        support::evidence_for(sw2, "told_by")
    );

    // Nothing was added: a crawl proposes.
    let devices: Vec<Device> = lab.app.store().list(DEVICES).unwrap();
    assert_eq!(devices.len(), 1);
}

#[tokio::test]
async fn what_a_crawl_adds_carries_the_seeds_credential_site_and_profile() {
    let (lab, _probes, community) = chain().await;
    let sw1 = lab.id("SW1");
    let seed_credential = lab
        .app
        .store()
        .get::<Device>(DEVICES, &sw1)
        .unwrap()
        .unwrap()
        .credential_id;

    let (_scan_id, job) = crawl(
        &lab,
        json!({
            "seed_device_ids": [sw1],
            "depth": 1,
            "credential_ids": [community],
            "add": true,
            "use_arp": false,
        }),
    )
    .await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.log
    );

    let devices: Vec<Device> = lab.app.store().list(DEVICES).unwrap();
    let added = devices
        .iter()
        .find(|device| device.name == "SW2")
        .unwrap_or_else(|| panic!("SW2 was added: {:?}", job.log));
    assert_eq!(added.address, "10.0.0.2");
    assert_eq!(added.profile, "test_ios");
    assert_eq!(added.role, "router", "as SW1 reported it");
    assert_eq!(added.site, "HQ", "it inherits the seed's site");
    assert_eq!(
        added.credential_id, seed_credential,
        "and the seed's credential, which is the only login anyone said it may use"
    );
    assert!(added.monitor.snmp && added.monitor.enabled);
    assert!(added.notes.contains("neighbour crawl"), "{}", added.notes);
    assert!(
        job.log
            .iter()
            .filter(|line| line.message.contains("added `"))
            .count()
            == 1,
        "{:?}",
        job.log
    );
}

#[tokio::test]
async fn a_crawl_goes_through_the_devices_it_reaches_and_never_round_a_loop() {
    let (lab, _probes, community) = chain().await;
    let sw1 = lab.id("SW1");
    // SW2 is managed (by the address its neighbours report), so the crawl can
    // read it in turn — which is the only way to reach SW3.
    let sw2 = lab
        .app
        .rpc(
            "devices.create",
            json!({
                "name": "SW2", "address": "10.0.0.2", "profile": "test_ios",
                "credential_id": lab.credential_id, "site": "HQ",
                "lab": {"lab_id": "testlab", "node": "SW2"},
            }),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (scan_id, job) = crawl(
        &lab,
        json!({
            "seed_device_ids": [sw1],
            "depth": 3,
            "credential_ids": [community],
            "use_arp": false,
        }),
    )
    .await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?} {:?}",
        job.error,
        job.log
    );

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let addresses: Vec<&str> = hosts
        .iter()
        .map(|host| host["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        addresses,
        ["10.0.0.1", "10.0.0.2", "10.0.0.3"],
        "SW2 (already managed), SW3 beyond it, and — honestly — SW1 itself, which Nocturne manages at 192.168.1.1 and has never been asked what its other addresses are"
    );
    assert_eq!(support::host_at(&hosts, "10.0.0.2")["device_id"], sw2);
    assert_eq!(
        support::host_at(&hosts, "10.0.0.1")["device_id"],
        json!(null)
    );

    let sw3 = support::host_at(&hosts, "10.0.0.3");
    assert_eq!(sw3["told_by"]["device_name"], "SW2");
    assert_eq!(
        sw3["path"],
        json!(["SW1", "SW2"]),
        "the path records how the crawl got there"
    );

    // SW2's table names SW1 and SW3's table names SW2: neither is read twice,
    // and the crawl ends instead of going round for ever.
    let reads: Vec<&str> = job
        .log
        .iter()
        .filter(|line| line.message.contains("knows"))
        .map(|line| line.message.as_str())
        .collect();
    assert_eq!(reads.len(), 2, "{reads:?}");
    assert!(
        reads[0].contains("`SW1`") && reads[1].contains("`SW2`"),
        "{reads:?}"
    );
    assert_eq!(
        lab.app.store().list::<Device>(DEVICES).unwrap().len(),
        2,
        "nothing was added: `add` was not asked for"
    );
    assert!(!sw2.is_empty(), "SW2 has an id");

    // Once SW1's interfaces have been collected, the address its neighbour
    // reports is recognised as SW1 and stops being offered as a find.
    nocturne_app::collect::store(
        &lab.app,
        &sw1,
        nocturne_drivers::model::Kind::Interfaces,
        vec![json!({"name": "Vlan1", "ipv4": ["10.0.0.1/24"]})],
        "cli",
    )
    .unwrap();
    let (scan_id, _) = crawl(
        &lab,
        json!({
            "seed_device_ids": [sw1], "depth": 3,
            "credential_ids": [community], "use_arp": false,
        }),
    )
    .await;
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(
        support::host_at(&hosts, "10.0.0.1")["device_id"],
        sw1,
        "the same device, reached by another of its addresses"
    );
    let new = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": scan_id, "only_new": true}),
        )
        .await
        .unwrap();
    let new: Vec<&str> = new["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host["address"].as_str().unwrap())
        .collect();
    assert_eq!(new, ["10.0.0.3"]);
}

#[tokio::test]
async fn depth_bounds_how_far_a_crawl_goes() {
    let (lab, _probes, community) = chain().await;
    let sw1 = lab.id("SW1");

    // Depth 1 with adding reaches SW2 and stops: SW3 is one hop further.
    let (scan_id, _) = crawl(
        &lab,
        json!({
            "seed_device_ids": [sw1], "depth": 1, "credential_ids": [community],
            "add": true, "use_arp": false,
        }),
    )
    .await;
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["address"], "10.0.0.2");

    // The settings cap the depth, however deep the request asks.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "discovery", "value": {"max_crawl_depth": 1}}),
        )
        .await
        .unwrap();
    let (scan_id, _) = crawl(
        &lab,
        json!({
            "seed_device_ids": [sw1], "depth": 9, "credential_ids": [community],
            "add": true, "use_arp": false,
        }),
    )
    .await;
    let new = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": scan_id, "only_new": true}),
        )
        .await
        .unwrap();
    assert!(
        new["hosts"].as_array().unwrap().is_empty(),
        "SW2 is managed now, so one level from SW1 finds nothing new: {:#?}",
        new["hosts"]
    );
}

#[tokio::test]
async fn a_candidate_no_profile_recognises_is_neither_added_nor_crawled_into() {
    let (lab, probes) = support::lab(&["SW1"]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;
    neighbours(
        &lab,
        "SW1",
        &[("GigabitEthernet0/1", "mystery-box", "10.0.0.50", "Host")],
    );
    // It answers, but nothing about it matches a device type.
    probes.host(
        "10.0.0.50",
        FakeHost::alive().port(80, Some("HTTP/1.0 200 OK")),
    );

    let (scan_id, job) = crawl(
        &lab,
        json!({
            "seed_device_ids": [lab.id("SW1")], "depth": 2,
            "credential_ids": [community], "add": true, "use_arp": false,
        }),
    )
    .await;
    assert_eq!(job.state, JobState::Succeeded);

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let mystery = support::host_at(&hosts, "10.0.0.50");
    assert_eq!(mystery["alive"], true, "it is reported: it did answer");
    assert_eq!(mystery["profile"], json!(null));
    assert_eq!(mystery["device_id"], json!(null), "and it was not added");
    assert_eq!(
        lab.app.store().list::<Device>(DEVICES).unwrap().len(),
        1,
        "nothing was added"
    );
    assert!(
        job.log
            .iter()
            .any(|line| line.message.contains("no device type matched")),
        "the job log says why it stopped there: {:?}",
        job.log
    );
}

#[tokio::test]
async fn a_candidate_that_answers_nothing_is_still_reported_with_who_knows_it() {
    let (lab, _probes) = support::lab(&["SW1"]).await;
    neighbours(
        &lab,
        "SW1",
        &[(
            "GigabitEthernet0/1",
            "SW2.campus.example",
            "10.0.0.2",
            "Switch",
        )],
    );

    let (scan_id, job) = crawl(
        &lab,
        json!({"seed_device_ids": [lab.id("SW1")], "depth": 1, "use_arp": false}),
    )
    .await;
    assert_eq!(job.state, JobState::Succeeded);

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let silent = support::host_at(&hosts, "10.0.0.2");
    assert_eq!(
        silent["alive"], false,
        "it did not answer us, and that is what the record says"
    );
    assert_eq!(silent["via"], "neighbor");
    assert!(
        support::evidence_for(silent, "alive")[0].contains("did not answer any probe")
            && support::evidence_for(silent, "alive")[0].contains("cdp table"),
        "{:?}",
        support::evidence_for(silent, "alive")
    );
    assert_eq!(silent["told_by"]["device_name"], "SW1");
}

#[tokio::test]
async fn an_arp_cache_contributes_hosts_with_the_mac_the_device_knows() {
    let (lab, probes) = support::lab(&["SW1"]).await;
    lab.fake("SW1").with(|state| {
        state.neighbors.clear();
        state.outputs.insert(
            "show cdp neighbors detail".to_string(),
            "Total cdp entries displayed : 0\n".to_string(),
        );
        state.outputs.insert(
            "show ip arp".to_string(),
            "Protocol  Address          Age (min)  Hardware Addr   Type   Interface\n\
             Internet  10.0.0.1                 -   0011.2233.4455  ARPA   Vlan1\n\
             Internet  10.0.0.60               14   001b.21aa.bbcc  ARPA   Vlan10\n"
                .to_string(),
        );
    });
    probes.host("10.0.0.60", FakeHost::alive());

    let (scan_id, job) = crawl(
        &lab,
        json!({"seed_device_ids": [lab.id("SW1")], "depth": 1, "use_arp": true}),
    )
    .await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let host = support::host_at(&hosts, "10.0.0.60");
    assert_eq!(host["told_by"]["protocol"], "arp");
    assert_eq!(host["told_by"]["local_interface"], "Vlan10");
    assert_eq!(host["mac"], "00:1b:21:aa:bb:cc");
    assert!(
        support::evidence_for(host, "mac")[0].contains("SW1` has 10.0.0.60 at"),
        "{:?}",
        support::evidence_for(host, "mac")
    );
    assert!(
        host["vendor"]
            .as_str()
            .is_some_and(|vendor| !vendor.is_empty()),
        "the OUI registry knows that prefix: {:?}",
        host["vendor"]
    );
}

#[tokio::test]
async fn a_seed_that_cannot_be_read_is_reported_and_the_crawl_carries_on() {
    let (lab, probes) = support::lab(&["SW1"]).await;
    neighbours(
        &lab,
        "SW1",
        &[("GigabitEthernet0/1", "SW9", "10.0.0.9", "Switch")],
    );
    // A device whose device type is not loaded cannot be read at all.
    let ghost = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "GHOST", "address": "10.9.9.9", "profile": "no_such_type"}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    probes.host("10.0.0.9", FakeHost::alive());

    let (scan_id, job) = crawl(
        &lab,
        json!({"seed_device_ids": [ghost, lab.id("SW1")], "depth": 1, "use_arp": false}),
    )
    .await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "one unreadable seed does not fail the crawl: {:?}",
        job.error
    );
    assert!(
        job.log
            .iter()
            .any(|line| line.level == "warn" && line.message.contains("could not be read")),
        "{:?}",
        job.log
    );
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["address"], "10.0.0.9");
}

#[tokio::test]
async fn crawling_with_no_seeds_at_all_is_refused() {
    let (lab, _probes) = support::lab(&[]).await;
    let error = lab.app.rpc("discovery.crawl", json!({})).await.unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("nothing to crawl from"),
        "{}",
        error.message
    );

    let error = lab
        .app
        .rpc("discovery.crawl", json!({"seed_device_ids": ["nope"]}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
}
