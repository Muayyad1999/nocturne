//! Reconciliation against devices that really are configured.
//!
//! Every "collected" row in this file came out of a simulated device's own
//! CLI through the real `cisco_ios` collectors — see `common/mod.rs`. Nothing
//! here hand-writes an interface table.

mod common;

use common::{Campus, LAB_ID};
use serde_json::{Value, json};

/// The namespace a lab's addresses live in, which is also the `site` its
/// prefixes carry.
fn lab_site() -> String {
    format!("lab:{LAB_ID}")
}

fn rows(value: &Value, key: &str) -> Vec<Value> {
    value[key]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("`{key}` is a list in {value:#}"))
}

fn find<'a>(rows: &'a [Value], key: &str, value: &str) -> Option<&'a Value> {
    rows.iter().find(|row| row[key] == value)
}

async fn documented_prefix(campus: &Campus, cidr: &str, name: &str) -> String {
    let saved = campus
        .ok(
            "ipam.prefix.save",
            json!({"cidr": cidr, "name": name, "site": lab_site()}),
        )
        .await;
    saved["prefix"]["id"].as_str().expect("an id").to_string()
}

#[tokio::test]
async fn a_sync_writes_evidence_and_not_one_record() {
    let campus = Campus::start(&["EDGE", "DS1", "AS1"]).await;
    let result = campus.ok("ipam.sync", json!({})).await;

    assert!(
        result["addresses_updated"].as_u64().unwrap_or(0) >= 10,
        "the campus has more than ten configured addresses: {result:#}"
    );
    assert!(
        result["prefixes_added"].as_u64().unwrap_or(0) >= 5,
        "a prefix per configured network: {result:#}"
    );

    let summary = campus.ok("ipam.summary", json!({})).await;
    assert_eq!(
        summary["addresses"], 0,
        "a sync never writes an address record: {summary:#}"
    );
    assert!(summary["observations"].as_u64().unwrap_or(0) >= 10);
    assert_eq!(
        summary["prefixes"], summary["prefixes_discovered"],
        "every prefix here exists because something was seen in it"
    );

    // The discovered prefixes are the ones the devices are really configured
    // with, and they say so.
    let prefixes = campus.ok("ipam.prefixes", json!({})).await;
    let prefixes = prefixes.as_array().expect("a list").clone();
    for cidr in [
        "10.0.0.0/30",    // EDGE Gi0/1 to DS1
        "10.10.10.0/24",  // the Staff SVI
        "10.10.99.0/24",  // management
        "10.255.0.1/32",  // EDGE's loopback
        "203.0.113.0/30", // the Internet uplink
    ] {
        let row = find(&prefixes, "cidr", cidr)
            .unwrap_or_else(|| panic!("a row for {cidr} in {prefixes:#?}"));
        assert_eq!(row["source"], "discovered", "{cidr}");
        assert_eq!(row["site"], lab_site().as_str(), "{cidr}");
        let from = rows(row, "discovered_from");
        assert!(
            !from.is_empty()
                && from[0]["interface"].is_string()
                && from[0]["collected_at"].is_i64(),
            "{cidr} says which interface it came from and when: {row:#}"
        );
    }

    // And the reconciliation report says they are not documented.
    let report = campus.ok("ipam.reconcile", json!({})).await;
    let discovered = rows(&report, "prefixes_not_documented");
    assert!(
        find(&discovered, "cidr", "10.10.10.0/24").is_some(),
        "{discovered:#?}"
    );
    assert!(
        report["refused"]
            .as_array()
            .expect("the refusals")
            .iter()
            .any(|line| line.as_str().unwrap_or_default().contains("decision")),
        "the report says what it will not decide: {report:#}"
    );
}

#[tokio::test]
async fn an_address_configured_on_a_switch_but_missing_from_the_plan_is_reported() {
    let campus = Campus::start(&["DS1", "AS1"]).await;
    let prefix = documented_prefix(&campus, "10.10.99.0/24", "Management").await;
    // The plan knows about the distribution switch, not the access switch.
    campus
        .ok(
            "ipam.address.save",
            json!({"ip": "10.10.99.2", "prefix_id": prefix, "hostname": "ds1-mgmt"}),
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;

    let report = campus.ok("ipam.reconcile", json!({})).await;
    let undocumented = rows(&report, "undocumented");
    let row = find(&undocumented, "ip", "10.10.99.11")
        .unwrap_or_else(|| panic!("AS1's management address: {undocumented:#?}"));
    assert_eq!(row["prefix"], "10.10.99.0/24");
    assert_eq!(row["prefix_documented"], true);
    let summary = row["summary"].as_str().unwrap_or_default();
    assert!(
        summary.contains("AS1")
            && summary.contains("Vlan99")
            && summary.contains("not in the plan"),
        "{summary}"
    );
    let evidence = rows(row, "observations");
    let configured = evidence
        .iter()
        .find(|observation| observation["kind"] == "interface")
        .unwrap_or_else(|| panic!("the interface it is configured on: {evidence:#?}"));
    assert_eq!(configured["device"], "AS1");
    assert_eq!(configured["interface"], "Vlan99");
    assert_eq!(configured["prefix_len"], 24, "with the mask it carries");
    assert!(
        configured["collected_at"].as_i64().unwrap_or(0) > 0,
        "an observation says when it was collected"
    );

    // The documented one is not in the list, and the prefix's own numbers
    // count both halves.
    assert!(find(&undocumented, "ip", "10.10.99.2").is_none());
    let detail = campus.ok("ipam.prefix.get", json!({"id": prefix})).await;
    assert!(
        detail["prefix"]["used"].as_u64().unwrap_or(0) >= 3,
        "{:#}",
        detail["prefix"]
    );
    assert_eq!(detail["prefix"]["documented"], 1, "one record");
    assert!(
        detail["prefix"]["observed"].as_u64().unwrap_or(0) >= 3,
        "several sightings"
    );
}

#[tokio::test]
async fn an_address_in_the_plan_that_nothing_answers_on_says_whether_anybody_looked() {
    let campus = Campus::start(&["DS1", "AS1"]).await;
    let seen = documented_prefix(&campus, "10.10.99.0/24", "Management").await;
    // A block in the plan that no device in this campus is configured in.
    let unseen = documented_prefix(&campus, "192.168.50.0/24", "Branch office").await;
    campus
        .ok(
            "ipam.address.save",
            json!({"ip": "10.10.99.50", "prefix_id": seen, "hostname": "spare-switch"}),
        )
        .await;
    campus
        .ok(
            "ipam.address.save",
            json!({"ip": "192.168.50.10", "prefix_id": unseen, "hostname": "branch-router"}),
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;

    let report = campus.ok("ipam.reconcile", json!({})).await;
    let unobserved = rows(&report, "unobserved");

    let spare = find(&unobserved, "ip", "10.10.99.50").expect("the spare address");
    assert_eq!(
        spare["had_coverage"], true,
        "devices in that prefix were read"
    );
    assert_eq!(spare["last_seen"], Value::Null, "nothing has ever answered");
    assert!(
        spare["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing has ever answered on it"),
        "{spare:#}"
    );

    let branch = find(&unobserved, "ip", "192.168.50.10").expect("the branch address");
    assert_eq!(branch["had_coverage"], false);
    assert!(
        branch["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing that could see it has been collected"),
        "an address nobody looked for is not an address that is not there: {branch:#}"
    );

    // The same distinction on the prefix: one was looked in, one was not.
    let without = rows(&report, "prefixes_without_evidence");
    assert!(
        find(&without, "cidr", "192.168.50.0/24").is_some(),
        "{without:#?}"
    );
    assert!(
        find(&without, "cidr", "10.10.99.0/24").is_none(),
        "{without:#?}"
    );
}

#[tokio::test]
async fn a_record_that_names_the_wrong_device_is_reported_and_left_alone() {
    let campus = Campus::start(&["DS1", "DS2"]).await;
    let prefix = documented_prefix(&campus, "10.10.10.0/24", "Staff").await;
    // The plan says DS2 holds .2; the network says DS1 does.
    campus
        .ok(
            "ipam.address.save",
            json!({
                "ip": "10.10.10.2",
                "prefix_id": prefix,
                "hostname": "staff-gateway",
                "device_id": campus.id("DS2"),
                "interface": "Vlan10",
            }),
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;

    let report = campus.ok("ipam.reconcile", json!({})).await;
    let mismatches = rows(&report, "mismatches");
    let row = find(&mismatches, "ip", "10.10.10.2").expect("the disagreement");
    assert_eq!(row["field"], "device");
    assert_eq!(row["documented"], campus.id("DS2").as_str());
    assert_eq!(row["observed"], campus.id("DS1").as_str());
    assert_eq!(row["interface"], "Vlan10");

    // The record still says exactly what the person wrote.
    let address = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.10.2", "scope": lab_site()}),
        )
        .await;
    assert_eq!(address["address"]["device_id"], campus.id("DS2").as_str());
    assert_eq!(address["address"]["hostname"], "staff-gateway");
    assert_eq!(address["address"]["documented"], true);
    assert_eq!(address["address"]["observed"], true);
    assert_eq!(
        rows(&address["address"], "disagreements")[0]["field"],
        "device"
    );
}

#[tokio::test]
async fn one_address_configured_on_two_switches_is_a_conflict() {
    let mut campus = Campus::start(&["AS1", "AS2"]).await;
    campus.ok("ipam.sync", json!({})).await;
    assert!(
        campus
            .ok("ipam.conflicts", json!({}))
            .await
            .as_array()
            .expect("a list")
            .is_empty(),
        "the campus as shipped has no duplicate address"
    );

    // Somebody types AS1's management address into AS2. The simulated switch
    // really takes it.
    campus
        .configure(
            "AS2",
            &["interface Vlan99", "ip address 10.10.99.11 255.255.255.0"],
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;

    let conflicts = campus.ok("ipam.conflicts", json!({})).await;
    let conflicts = conflicts.as_array().expect("a list").clone();
    let row = find(&conflicts, "subject", "10.10.99.11")
        .unwrap_or_else(|| panic!("the duplicate: {conflicts:#?}"));
    assert_eq!(row["kind"], "duplicate_ip");
    let involved = rows(row, "involved");
    assert_eq!(involved.len(), 2);
    assert!(
        involved
            .iter()
            .all(|side| side["kind"] == "observation" && side["collected_at"].is_i64()),
        "both sides are sightings, with their collection times: {involved:#?}"
    );
    let devices: Vec<&str> = involved
        .iter()
        .filter_map(|side| side["device"].as_str())
        .collect();
    assert!(
        devices.contains(&"AS1") && devices.contains(&"AS2"),
        "{devices:?}"
    );

    // And the address itself is shown as a conflict rather than as active.
    let address = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.99.11", "scope": lab_site()}),
        )
        .await;
    assert_eq!(address["address"]["status"], "conflict");
    let evidence = rows(&address["address"], "observations");
    assert_eq!(
        evidence
            .iter()
            .filter(|observation| observation["kind"] == "interface")
            .count(),
        2,
        "two switches have it configured: {evidence:#?}"
    );
}

#[tokio::test]
async fn an_address_the_device_no_longer_has_stops_being_evidence() {
    let mut campus = Campus::start(&["AS1"]).await;
    campus
        .configure(
            "AS1",
            &[
                "interface Loopback0",
                "ip address 10.255.0.11 255.255.255.255",
            ],
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;
    let address = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.255.0.11", "scope": lab_site()}),
        )
        .await;
    assert_eq!(address["address"]["status"], "active");
    assert_eq!(address["address"]["source"], "interface");

    // The loopback goes away on the device.
    campus.configure("AS1", &["no interface Loopback0"]).await;
    let result = campus.ok("ipam.sync", json!({})).await;
    assert_eq!(
        result["addresses_withdrawn"], 1,
        "an interface address that is gone is gone, not stale: {result:#}"
    );
    let address = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.255.0.11", "scope": lab_site()}),
        )
        .await;
    assert_eq!(address["address"]["status"], "free");
    assert_eq!(address["address"]["observed"], false);
    assert_eq!(address["address"]["last_seen"], Value::Null);
}

#[tokio::test]
async fn a_labs_addresses_never_count_towards_a_production_prefix() {
    let campus = Campus::start(&["DS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    // The same block, documented in production.
    let production = campus
        .ok(
            "ipam.prefix.save",
            json!({"cidr": "10.10.10.0/24", "name": "Staff", "site": "Campus"}),
        )
        .await;
    assert_eq!(
        production["prefix"]["used"], 0,
        "the simulated campus is a lab: its addresses are not production's"
    );
    assert_eq!(production["prefix"]["observed"], 0);
    assert!(
        production["prefix"]["devices"]
            .as_array()
            .expect("a list")
            .is_empty()
    );

    let prefixes = campus
        .ok("ipam.prefixes", json!({"site": lab_site()}))
        .await;
    let lab_row = find(
        prefixes.as_array().expect("a list"),
        "cidr",
        "10.10.10.0/24",
    )
    .expect("the lab's own prefix");
    assert!(lab_row["observed"].as_u64().unwrap_or(0) >= 1);
    assert!(
        rows(lab_row, "devices")
            .iter()
            .any(|device| device["device"] == "DS1" && device["lab"] == true)
    );
}

#[tokio::test]
async fn learning_shows_what_it_would_create_before_it_creates_anything() {
    let campus = Campus::start(&["AS1", "AS2"]).await;
    campus.ok("ipam.sync", json!({})).await;

    let preview = campus.ok("ipam.learn", json!({})).await;
    assert_eq!(preview["dry_run"], true, "learning previews by default");
    assert_eq!(preview["created_addresses"], 0);
    assert_eq!(preview["created_prefixes"], 0);
    let planned = rows(&preview, "addresses");
    assert!(
        planned
            .iter()
            .all(|row| row["action"] == "create" && row["kind"] == "interface"),
        "{planned:#?}"
    );
    assert!(
        find(&planned, "ip", "10.10.99.11").is_some()
            && find(&planned, "ip", "10.10.99.12").is_some(),
        "both access switches: {planned:#?}"
    );
    assert_eq!(
        campus.ok("ipam.summary", json!({})).await["addresses"],
        0,
        "the preview wrote nothing"
    );

    let done = campus.ok("ipam.learn", json!({"dry_run": false})).await;
    assert!(
        done["created_addresses"].as_u64().unwrap_or(0) >= 2,
        "{done:#}"
    );
    let summary = campus.ok("ipam.summary", json!({})).await;
    assert_eq!(
        summary["addresses"], done["created_addresses"],
        "the records it said it would create are the records there are"
    );

    // Again: everything is already there, and nothing is overwritten.
    let again = campus.ok("ipam.learn", json!({"dry_run": false})).await;
    assert_eq!(again["created_addresses"], 0);
    assert!(
        rows(&again, "addresses")
            .iter()
            .all(|row| row["action"] == "exists")
    );
    let address = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.99.11", "scope": lab_site()}),
        )
        .await;
    assert_eq!(address["address"]["documented"], true);
    assert_eq!(
        address["address"]["interface"], "Vlan99",
        "the record says where it was learned from"
    );
    assert!(
        address["address"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("learned from AS1"),
        "{:#}",
        address["address"]
    );
}

#[tokio::test]
async fn arp_sightings_are_learned_only_when_they_are_asked_for() {
    let campus = Campus::start(&["DS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    let without = campus.ok("ipam.learn", json!({})).await;
    let rows_without = rows(&without, "addresses");
    assert!(
        rows_without.iter().all(|row| row["kind"] == "interface"),
        "ARP is left out unless asked for"
    );
    assert!(
        without["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note.as_str().unwrap_or_default().contains("ARP")),
        "and the plan says so: {without:#}"
    );

    let with = campus.ok("ipam.learn", json!({"include_arp": true})).await;
    let rows_with = rows(&with, "addresses");
    let leased = find(&rows_with, "ip", "10.10.10.100")
        .unwrap_or_else(|| panic!("a host that took a DHCP lease: {rows_with:#?}"));
    assert_eq!(leased["kind"], "arp");
    assert!(leased["mac"].is_string(), "with the MAC that answered");
}

#[tokio::test]
async fn a_device_that_was_never_collected_is_named_rather_than_counted_as_empty() {
    let mut campus = Campus::start(&["DS1"]).await;
    let waiting = campus.add_uncollected("AS9").await;
    campus.ok("ipam.sync", json!({})).await;

    let report = campus.ok("ipam.reconcile", json!({})).await;
    assert_eq!(report["coverage"]["devices_total"], 2);
    assert_eq!(report["coverage"]["devices_never_collected"], 1);
    let devices = rows(&report["coverage"], "devices");
    let row = find(&devices, "device_id", &waiting).expect("the uncollected device");
    assert_eq!(row["interfaces_at"], Value::Null);
    assert_eq!(row["addresses_configured"], 0);
    assert!(
        row["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("never been collected"),
        "{row:#}"
    );
    let collected = find(&devices, "device", "DS1").expect("DS1");
    assert!(collected["interfaces_at"].as_i64().unwrap_or(0) > 0);
    assert!(collected["addresses_configured"].as_u64().unwrap_or(0) >= 6);

    let summary = campus.ok("ipam.summary", json!({})).await;
    assert_eq!(summary["devices_never_collected"], 1);
    assert_eq!(summary["devices_total"], 2);
}

#[tokio::test]
async fn the_vlans_the_devices_report_sit_beside_the_prefixes() {
    let campus = Campus::start(&["DS1", "AS1"]).await;
    campus.ok("ipam.sync", json!({})).await;

    let vlans = campus.ok("ipam.vlans", json!({})).await;
    let vlans = vlans.as_array().expect("a list").clone();
    let staff = vlans
        .iter()
        .find(|row| row["vid"] == 10)
        .unwrap_or_else(|| panic!("VLAN 10: {vlans:#?}"));
    assert_eq!(staff["documented"], false, "nobody wrote it down");
    assert_eq!(staff["observed"], true);
    assert_eq!(staff["name"], "Staff", "the name the devices give it");
    let devices = rows(staff, "devices");
    assert!(
        devices
            .iter()
            .any(|device| device["device"] == "DS1" && device["name"] == "Staff"),
        "{devices:#?}"
    );
    assert!(devices.iter().all(|device| device["collected_at"].is_i64()));

    // Documenting it keeps the sightings and ties it to its prefix.
    let prefix = campus
        .ok(
            "ipam.prefix.save",
            json!({"cidr": "10.10.10.0/24", "name": "Staff", "site": lab_site(), "vlan_id": 10}),
        )
        .await;
    assert_eq!(
        prefix["adopted"], true,
        "documenting a block a sync discovered adopts that record"
    );
    assert_eq!(prefix["prefix"]["source"], "manual");
    let saved = campus
        .ok(
            "ipam.vlan.save",
            json!({"vid": 10, "name": "Staff", "site": lab_site(), "description": "user VLAN"}),
        )
        .await;
    assert_eq!(saved["documented"], true);
    assert_eq!(saved["source"], "manual");
    assert!(
        !rows(&saved, "devices").is_empty(),
        "the evidence is still there"
    );
    assert_eq!(
        rows(&saved, "prefixes")[0]["cidr"],
        prefix["prefix"]["cidr"],
        "a VLAN and a subnet are one decision"
    );
}

#[tokio::test]
async fn an_interface_mask_that_disagrees_with_the_prefix_is_reported() {
    let mut campus = Campus::start(&["AS1"]).await;
    documented_prefix(&campus, "10.10.99.0/24", "Management").await;
    campus
        .configure(
            "AS1",
            &["interface Vlan99", "ip address 10.10.99.11 255.255.255.128"],
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;

    let report = campus.ok("ipam.reconcile", json!({})).await;
    let masks = rows(&report, "mask_mismatches");
    let row = find(&masks, "ip", "10.10.99.11")
        .unwrap_or_else(|| panic!("the mask disagreement: {masks:#?}"));
    assert_eq!(row["prefix"], "10.10.99.0/24");
    assert_eq!(row["prefix_len"], 24);
    assert_eq!(row["configured_len"], 25);
    assert_eq!(row["device"], "AS1");
    assert_eq!(row["interface"], "Vlan99");
}

#[tokio::test]
async fn the_gateway_a_prefix_documents_is_checked_against_what_is_configured() {
    let campus = Campus::start(&["DS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    // The real gateway of the Staff VLAN is the HSRP address .1; a plan that
    // says .9 is wrong, and the devices say so.
    campus
        .ok(
            "ipam.prefix.save",
            json!({"cidr": "10.10.10.0/24", "name": "Staff", "site": lab_site(), "gateway": "10.10.10.9"}),
        )
        .await;
    let conflicts = campus.ok("ipam.conflicts", json!({})).await;
    let conflicts = conflicts.as_array().expect("a list").clone();
    let row = conflicts
        .iter()
        .find(|row| row["kind"] == "gateway_mismatch")
        .unwrap_or_else(|| panic!("{conflicts:#?}"));
    assert_eq!(row["subject"], "10.10.10.0/24");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("10.10.10.9") && detail.contains("10.10.10.2"),
        "it names what was documented and what is configured: {detail}"
    );

    // A prefix nothing is configured in produces no such claim.
    campus
        .ok(
            "ipam.prefix.save",
            json!({"cidr": "192.168.77.0/24", "name": "Branch", "site": lab_site(), "gateway": "192.168.77.1"}),
        )
        .await;
    let conflicts = campus.ok("ipam.conflicts", json!({})).await;
    assert!(
        conflicts
            .as_array()
            .expect("a list")
            .iter()
            .all(|row| row["subject"] != "192.168.77.0/24"),
        "nothing was seen in it, so there is nothing to disagree with: {conflicts:#?}"
    );
}

#[tokio::test]
async fn reserving_an_address_a_device_already_has_is_refused_naming_who_has_it() {
    let campus = Campus::start(&["AS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    let prefix = documented_prefix(&campus, "10.10.99.0/24", "Management").await;

    let refused = campus
        .rpc(
            "ipam.address.reserve",
            json!({"prefix_id": prefix, "ip": "10.10.99.11", "hostname": "a new switch"}),
        )
        .await
        .expect_err("AS1 has that address");
    assert_eq!(refused.code, "conflict");
    assert!(
        refused.message.contains("AS1") && refused.message.contains("Vlan99"),
        "it names who has it: {}",
        refused.message
    );
    // A sentence for a person has no machine timestamp in it. The time is in
    // the evidence below, where a caller can render it the way it renders
    // every other time — this message used to end "collected at
    // 1794614077077" and the screen had to find those digits and rewrite them.
    assert!(
        !has_an_epoch(&refused.message),
        "no raw timestamp in a sentence: {}",
        refused.message
    );
    let details = refused.details.expect("the evidence");
    assert_eq!(details["kind"], "duplicate_ip");
    assert_eq!(details["involved"][0]["device"], "AS1");
    assert!(
        details["involved"][0]["collected_at"].as_i64().is_some(),
        "but the time is there, as a number, where it belongs"
    );

    // An address in the same prefix that nothing has answered on is reserved,
    // and the reservation says what that rests on.
    let reserved = campus
        .ok(
            "ipam.address.reserve",
            json!({"prefix_id": prefix, "ip": "10.10.99.200", "hostname": "a new switch"}),
        )
        .await;
    assert_eq!(reserved["address"]["status"], "reserved");
    assert_eq!(reserved["address"]["observed"], false);
}

#[tokio::test]
async fn what_a_person_wrote_survives_every_sync() {
    let mut campus = Campus::start(&["DS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    // Adopt a discovered prefix and fill it in.
    let saved = campus
        .ok(
            "ipam.prefix.save",
            json!({
                "cidr": "10.10.10.0/24",
                "name": "Staff",
                "description": "wired user ports",
                "role": "lan",
                "gateway": "10.10.10.1",
                "vlan_id": 10,
                "site": lab_site(),
                "tags": ["pci"],
            }),
        )
        .await;
    let id = saved["prefix"]["id"].as_str().expect("an id").to_string();
    assert_eq!(saved["prefix"]["source"], "manual");
    // Adoption keeps the note of where the block was first seen, as it was.
    let provenance = rows(&saved["prefix"], "discovered_from");
    assert_eq!(provenance[0]["device"], "DS1");

    // Collect and sync again, twice, with a configuration change in between.
    campus.ok("ipam.sync", json!({})).await;
    campus
        .configure(
            "DS1",
            &["interface Vlan10", "ip address 10.10.10.4 255.255.255.0"],
        )
        .await;
    campus.ok("ipam.sync", json!({})).await;

    let detail = campus.ok("ipam.prefix.get", json!({"id": id})).await;
    let prefix = &detail["prefix"];
    assert_eq!(prefix["source"], "manual", "a sync never adopts it back");
    assert_eq!(prefix["name"], "Staff");
    assert_eq!(prefix["description"], "wired user ports");
    assert_eq!(prefix["role"], "lan");
    assert_eq!(prefix["gateway"], "10.10.10.1");
    assert_eq!(prefix["vlan_id"], 10);
    assert_eq!(rows(prefix, "tags"), [json!("pci")]);
    assert_eq!(
        rows(prefix, "discovered_from"),
        provenance,
        "the note of where the block came from is kept exactly as it was: a sync refreshes the evidence of discovered prefixes, never of a record somebody owns"
    );
    // The new address is evidence against the plan, not a change to it.
    let report = campus.ok("ipam.reconcile", json!({})).await;
    assert!(
        find(&rows(&report, "undocumented"), "ip", "10.10.10.4").is_some(),
        "{report:#}"
    );
}

#[tokio::test]
async fn an_arp_sighting_nobody_refreshes_is_forgotten_and_a_configured_one_is_not() {
    let campus = Campus::start(&["DS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    // A host that took a DHCP lease, seen in DS1's ARP table.
    let leased = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.10.100", "scope": lab_site()}),
        )
        .await;
    assert_eq!(leased["address"]["source"], "arp");
    assert_eq!(leased["address"]["status"], "active");
    assert!(leased["address"]["mac"].is_string());

    // Shorten the window to nothing: every sighting is then out of date the
    // moment the collection that made it is in the past.
    campus
        .ok(
            "settings.set",
            json!({"section": "ipam", "value": {"arp_stale_days": 0}}),
        )
        .await;
    let result = campus.ok("ipam.sync", json!({})).await;
    assert!(
        result["addresses_aged_out"].as_u64().unwrap_or(0) >= 1,
        "the ARP sightings are forgotten: {result:#}"
    );

    let gone = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.10.100", "scope": lab_site()}),
        )
        .await;
    assert_eq!(
        gone["address"]["status"], "free",
        "an address that has not answered inside the window is not seen, which is not the same as free for the taking"
    );
    assert_eq!(gone["address"]["observed"], false);
    // What is configured on an interface is not aged out: it is configured.
    let configured = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.10.2", "scope": lab_site()}),
        )
        .await;
    assert_eq!(configured["address"]["source"], "interface");
    assert_eq!(configured["address"]["status"], "active");
}

/// The watcher: the only test that lets the background sync run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collection_syncs_by_itself_once_the_dust_settles() {
    let mut campus = Campus::start(&["AS1"]).await;
    // Nothing has been read into IPAM yet: a collection and a sync are not
    // the same event.
    assert_eq!(
        campus.ok("ipam.summary", json!({})).await["observations"],
        0
    );
    campus
        .ok(
            "settings.set",
            // Shortened so the test does not wait out the default.
            json!({"section": "ipam", "value": {"sync_on_collect": true, "sync_delay_ms": 50}}),
        )
        .await;

    // Collecting from the device publishes `collect.updated`; the watcher
    // waits for the dust to settle and then reads it in by itself.
    campus.collect("AS1").await;
    let mut observations = 0;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        observations = campus.ok("ipam.summary", json!({})).await["observations"]
            .as_u64()
            .unwrap_or(0);
        if observations > 0 {
            break;
        }
    }
    assert!(
        observations > 0,
        "the sync ran without anybody asking for it"
    );
    let address = campus
        .ok(
            "ipam.address.get",
            json!({"ip": "10.10.99.11", "scope": lab_site()}),
        )
        .await;
    assert_eq!(address["address"]["observed"], true);
    assert_eq!(address["address"]["documented"], false);
}

/// Does this text carry a raw millisecond timestamp — thirteen digits in a
/// row — where a person is meant to read it?
fn has_an_epoch(text: &str) -> bool {
    let digits: Vec<char> = text.chars().collect();
    digits
        .windows(13)
        .any(|run| run.iter().all(char::is_ascii_digit))
}

/// No sentence this engine writes for a person contains a machine timestamp,
/// and the ones that have a time to report carry it as a field instead.
#[tokio::test]
async fn the_sentences_this_engine_writes_carry_no_raw_timestamps() {
    let campus = Campus::start(&["AS1"]).await;
    campus.ok("ipam.sync", json!({})).await;
    let prefix = documented_prefix(&campus, "10.10.99.0/24", "Management").await;

    // `next_address` in a block that has been seen: the case with a time.
    let next = campus
        .ok("ipam.next_address", json!({"prefix_id": prefix}))
        .await;
    let basis = next["basis"].as_str().expect("a basis");
    assert!(!has_an_epoch(basis), "{basis}");
    assert!(
        basis.contains("sighting"),
        "it still says what the answer rests on: {basis}"
    );
    assert!(
        next["seen_until"].as_i64().is_some(),
        "and when, as a field: {next}"
    );

    // Anything `ipam.learn` writes is *stored*, so a timestamp in it would
    // outlive the sentence that carried it.
    let learned = campus.ok("ipam.learn", json!({"dry_run": false})).await;
    let text = serde_json::to_string(&learned).expect("serialisable");
    assert!(!has_an_epoch_in_prose(&text), "{text}");

    let grid = campus.ok("ipam.prefix.get", json!({"id": prefix})).await;
    let note = grid["note"].as_str().unwrap_or_default();
    assert!(!has_an_epoch(note), "{note}");
}

/// The same question over a JSON document, ignoring the numbers that are
/// *meant* to be timestamps: a raw epoch is only wrong inside prose.
fn has_an_epoch_in_prose(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .is_some_and(|value| prose(&value).iter().any(|text| has_an_epoch(text)))
}

fn prose(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(text) => vec![text.clone()],
        serde_json::Value::Array(items) => items.iter().flat_map(prose).collect(),
        serde_json::Value::Object(fields) => fields.values().flat_map(prose).collect(),
        _ => Vec::new(),
    }
}

/// Two syncs at once must not discover the same block twice.
///
/// Discovery is a read then an insert, so without serialising, both see that
/// nothing covers a block and both write it — and the next attempt to
/// *document* that block is refused as a duplicate of itself. The watcher
/// syncs by itself shortly after a collection, so this is what happens to
/// anyone who presses Sync at the wrong moment.
///
/// It needs a real two-thread runtime and two spawned tasks. `tokio::join!`
/// on the default single-threaded test runtime proves nothing here: `sync` is
/// synchronous from end to end, so the first future runs to completion before
/// the second is ever polled, and the test passes whether or not anything
/// serialises. This version fails without the lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_syncs_at_once_discover_each_block_once() {
    let campus = Campus::start(&["AS1", "AS2"]).await;

    let one = campus.app.clone();
    let two = campus.app.clone();
    let first = tokio::spawn(async move { one.rpc("ipam.sync", json!({})).await });
    let second = tokio::spawn(async move { two.rpc("ipam.sync", json!({})).await });
    first
        .await
        .expect("the first task")
        .expect("the first sync");
    second
        .await
        .expect("the second task")
        .expect("the second sync");

    let prefixes = campus.ok("ipam.prefixes", json!({})).await;
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    collect_cidrs(&prefixes, &mut seen);
    let twice: Vec<(&String, &usize)> = seen.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        twice.is_empty(),
        "a block was discovered more than once: {twice:?}"
    );
    assert!(!seen.is_empty(), "the campus has addresses to discover");

    // And the proof that it matters: a discovered block can still be
    // documented, which is exactly what a duplicate row prevents.
    let cidr = seen.keys().next().expect("a discovered block").clone();
    campus
        .rpc(
            "ipam.prefix.save",
            json!({"cidr": cidr, "name": "documented"}),
        )
        .await
        .unwrap_or_else(|err| {
            panic!("documenting {cidr} should adopt the discovered row: {err:?}")
        });
}

/// Every `cidr` anywhere in a prefix tree, counted.
fn collect_cidrs(value: &serde_json::Value, into: &mut std::collections::BTreeMap<String, usize>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_cidrs(item, into);
            }
        }
        serde_json::Value::Object(fields) => {
            if let Some(cidr) = fields.get("cidr").and_then(serde_json::Value::as_str) {
                *into.entry(cidr.to_string()).or_default() += 1;
            }
            for nested in fields.values() {
                collect_cidrs(nested, into);
            }
        }
        _ => {}
    }
}
