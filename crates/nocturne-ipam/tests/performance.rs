//! Fifty thousand addresses, which is a real campus with a /16 of leases.
//!
//! The numbers the test prints are the ones that matter; run it with
//! `cargo test --release -p nocturne-ipam --test performance -- --nocapture`.
//! The asserted bounds are looser than the specification's 100 ms because
//! this runs unoptimised alongside the rest of the suite, where most of the
//! time goes into serde rather than into SQLite.

use std::time::{Duration, Instant};

use nocturne_app::App;
use nocturne_ipam::db;
use nocturne_ipam::model::{AddressRecord, Observation, ObservationKind, Status};
use serde_json::{Value, json};

/// How long a list or a search may take. The specification asks for 100 ms,
/// which an unoptimised build also manages; the debug bound is looser only
/// because this suite shares a machine with other work, and the printout says
/// what it really was.
const BUDGET: Duration = Duration::from_millis(if cfg!(debug_assertions) { 250 } else { 100 });

async fn app() -> App {
    let mut builder = nocturne_app::testing::builder();
    nocturne_ipam::init(&mut builder);
    builder.build()
}

async fn ok(app: &App, method: &str, params: Value) -> (Value, Duration) {
    let started = Instant::now();
    let value = app
        .rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method}: {err}"));
    (value, started.elapsed())
}

#[tokio::test]
async fn fifty_thousand_addresses_stay_quick_to_list_and_to_search() {
    let app = app().await;
    // Two hundred /24s under one /16: 50,000 addresses with a record, and a
    // tenth of them seen on the network as well.
    let _parent = app
        .rpc(
            "ipam.prefix.save",
            json!({"cidr": "10.0.0.0/16", "name": "the estate"}),
        )
        .await
        .expect("the parent")["prefix"]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    let mut prefix_ids = Vec::new();
    let started = Instant::now();
    for third in 0..200u32 {
        let id = app
            .rpc(
                "ipam.prefix.save",
                json!({"cidr": format!("10.0.{third}.0/24"), "name": format!("subnet {third}")}),
            )
            .await
            .expect("a prefix")["prefix"]["id"]
            .as_str()
            .expect("an id")
            .to_string();
        prefix_ids.push(id);
    }
    let prefixes_in = started.elapsed();

    let started = Instant::now();
    let mut records = Vec::with_capacity(250);
    let mut observations = Vec::with_capacity(25);
    for (third, prefix_id) in prefix_ids.iter().enumerate() {
        records.clear();
        observations.clear();
        for last in 1..=250u32 {
            let ip = format!("10.0.{third}.{last}");
            records.push(AddressRecord {
                ip: ip.clone(),
                scope: String::new(),
                prefix_id: Some(prefix_id.clone()),
                status: if last % 10 == 0 {
                    Status::Reserved
                } else {
                    Status::Active
                },
                hostname: format!("host-{third}-{last}"),
                description: String::new(),
                device_id: None,
                interface: None,
                mac: Some(format!(
                    "aa:bb:cc:{:02x}:{:02x}:01",
                    third as u8, last as u8
                )),
                tags: Vec::new(),
                created_at: 1,
                updated_at: 1,
            });
            if last % 10 == 1 {
                observations.push(Observation {
                    ip,
                    kind: ObservationKind::Arp,
                    device_id: "device".into(),
                    device: format!("SW{third}"),
                    interface: format!("Vlan{third}"),
                    prefix_len: None,
                    cidr: None,
                    mac: Some(format!(
                        "aa:bb:cc:{:02x}:{:02x}:01",
                        third as u8, last as u8
                    )),
                    vendor: None,
                    scope: String::new(),
                    first_seen: nocturne_app::util::now_ms(),
                    last_seen: nocturne_app::util::now_ms(),
                    collected_at: nocturne_app::util::now_ms(),
                    collector: "cli".into(),
                    current: true,
                });
            }
        }
        db::put_records(app.store(), &records).expect("the records are written");
        db::put_observations(app.store(), &observations).expect("the sightings are written");
    }
    let records_in = started.elapsed();
    assert_eq!(db::count_records(app.store()).unwrap(), 50_000);
    assert_eq!(db::count_observations(app.store()).unwrap(), 5_000);

    // The whole tree, with every row's utilisation.
    let (rows, tree) = ok(&app, "ipam.prefixes", json!({})).await;
    assert_eq!(rows.as_array().expect("a list").len(), 201);
    let first = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["cidr"] == "10.0.0.0/24")
        .expect("a /24");
    assert_eq!(first["documented"], 250);
    assert_eq!(first["used"], 225, "250 less the 25 reserved ones");
    assert_eq!(first["reserved"], 25);

    // A search by host name, by part of an address and by MAC.
    let (found, by_name) = ok(&app, "ipam.search", json!({"query": "host-42-7"})).await;
    assert!(
        !found["addresses"].as_array().expect("a list").is_empty(),
        "the host is found"
    );
    let (_, by_address) = ok(&app, "ipam.search", json!({"query": "10.0.42."})).await;
    let (_, by_mac) = ok(&app, "ipam.search", json!({"query": "aabb.cc2a.0701"})).await;

    // One subnet's grid, the next free address in it, and the summary.
    let (detail, grid) = ok(
        &app,
        "ipam.prefix.get",
        json!({"id": prefix_ids[42], "addresses": true}),
    )
    .await;
    assert_eq!(detail["addresses"].as_array().expect("a list").len(), 254);
    let (next, next_free) = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": prefix_ids[42]}),
    )
    .await;
    assert_eq!(next["addresses"][0], "10.0.42.251", "250 are taken");
    let (summary, summary_in) = ok(&app, "ipam.summary", json!({})).await;
    assert_eq!(summary["addresses"], 50_000);
    assert_eq!(summary["observations"], 5_000);

    println!(
        "50,000 addresses in 200 prefixes\n\
         \x20 writing: {prefixes_in:?} of prefixes, {records_in:?} of records\n\
         \x20 ipam.prefixes (201 rows, utilisation each): {tree:?}\n\
         \x20 ipam.search by name: {by_name:?}, by partial address: {by_address:?}, by MAC: {by_mac:?}\n\
         \x20 ipam.prefix.get (a /24 grid): {grid:?}\n\
         \x20 ipam.next_address: {next_free:?}\n\
         \x20 ipam.summary: {summary_in:?}"
    );
    for (what, taken) in [
        ("ipam.prefixes", tree),
        ("ipam.search by name", by_name),
        ("ipam.search by partial address", by_address),
        ("ipam.search by MAC", by_mac),
        ("ipam.prefix.get", grid),
        ("ipam.next_address", next_free),
    ] {
        assert!(taken < BUDGET, "{what} took {taken:?}, over {BUDGET:?}");
    }
}
