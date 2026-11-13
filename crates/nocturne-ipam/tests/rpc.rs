//! The RPC surface, through `App::rpc` — the real wire shape, including the
//! failure modes.

use nocturne_app::App;
use serde_json::{Value, json};

/// The next `ipam.changed`, skipping the audit entries that travel with it.
async fn next_change(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> nocturne_app::Event {
    loop {
        let event = events.recv().await.expect("an event");
        if event.topic == "ipam.changed" {
            return event;
        }
    }
}

async fn app() -> App {
    let mut builder = nocturne_app::testing::builder();
    nocturne_ipam::init(&mut builder);
    builder.build()
}

async fn ok(app: &App, method: &str, params: Value) -> Value {
    app.rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method}: {err}"))
}

async fn err(app: &App, method: &str, params: Value) -> nocturne_app::AppError {
    app.rpc(method, params)
        .await
        .expect_err(&format!("{method} should have been refused"))
}

async fn prefix(app: &App, cidr: &str, name: &str) -> String {
    ok(app, "ipam.prefix.save", json!({"cidr": cidr, "name": name})).await["prefix"]["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

fn list(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a list, got {value:#}"))
}

#[tokio::test]
async fn every_method_in_the_specification_is_registered() {
    let app = app().await;
    let methods = app.methods();
    for method in [
        "ipam.prefixes",
        "ipam.prefix.get",
        "ipam.prefix.save",
        "ipam.prefix.delete",
        "ipam.prefix.split",
        "ipam.next_prefix",
        "ipam.next_address",
        "ipam.address.save",
        "ipam.address.delete",
        "ipam.address.reserve",
        "ipam.address.get",
        "ipam.search",
        "ipam.sync",
        "ipam.reconcile",
        "ipam.learn",
        "ipam.conflicts",
        "ipam.vlans",
        "ipam.vlan.save",
        "ipam.vlan.delete",
        "ipam.import",
        "ipam.export",
        "ipam.summary",
    ] {
        assert!(methods.contains(&method), "{method} is not registered");
    }
    assert_eq!(
        app.rpc("ipam.nope", Value::Null).await.unwrap_err().code,
        "unknown_method"
    );
    // The settings section is declared with its defaults.
    let settings = ok(&app, "settings.get", json!({"section": "ipam"})).await;
    assert_eq!(settings["arp_stale_days"], 30);
    assert_eq!(settings["sync_on_collect"], true);
}

#[tokio::test]
async fn prefixes_come_back_as_a_tree_in_address_order() {
    let app = app().await;
    // Saved out of order, on purpose.
    let v6 = prefix(&app, "2001:db8::/32", "documentation").await;
    let twenty_four = prefix(&app, "10.1.10.0/24", "users").await;
    let eight = prefix(&app, "10.0.0.0/8", "the estate").await;
    let sixteen = prefix(&app, "10.1.0.0/16", "site one").await;
    let v6_child = prefix(&app, "2001:db8:1::/48", "site one v6").await;
    let other = prefix(&app, "10.2.0.0/16", "site two").await;

    let rows = list(&ok(&app, "ipam.prefixes", json!({})).await);
    let shape: Vec<(&str, u64, u64)> = rows
        .iter()
        .map(|row| {
            (
                row["cidr"].as_str().unwrap_or_default(),
                row["depth"].as_u64().unwrap_or_default(),
                row["children"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("10.0.0.0/8", 0, 2),
            ("10.1.0.0/16", 1, 1),
            ("10.1.10.0/24", 2, 0),
            ("10.2.0.0/16", 1, 0),
            ("2001:db8::/32", 0, 1),
            ("2001:db8:1::/48", 1, 0),
        ],
        "IPv4 before IPv6, each block followed by what is inside it"
    );
    let parents: Vec<&str> = rows
        .iter()
        .map(|row| row["parent_id"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(
        parents,
        [
            "-",
            eight.as_str(),
            sixteen.as_str(),
            eight.as_str(),
            "-",
            v6.as_str()
        ]
    );

    // The subtree of one prefix.
    let under = list(&ok(&app, "ipam.prefixes", json!({"parent_id": eight})).await);
    assert_eq!(under.len(), 3, "both /16s and the /24 under one of them");
    assert!(under.iter().all(|row| row["cidr"] != "10.0.0.0/8"));

    // A query keeps the ancestors of a match, so the tree is not broken...
    let matching = list(&ok(&app, "ipam.prefixes", json!({"query": "users"})).await);
    assert_eq!(
        matching
            .iter()
            .map(|row| row["cidr"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["10.0.0.0/8", "10.1.0.0/16", "10.1.10.0/24"]
    );
    // ...unless the caller asks for a flat list.
    let flat = list(
        &ok(
            &app,
            "ipam.prefixes",
            json!({"query": "users", "flat": true}),
        )
        .await,
    );
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0]["id"], twenty_four.as_str());
    assert_eq!(
        flat[0]["depth"], 2,
        "its depth in the tree is still the truth"
    );

    // A site is a filter of its own. Containment is not a site boundary: a
    // branch's block still sits inside the estate-wide one, and the tree view
    // keeps that parent so the branch is not shown floating.
    ok(
        &app,
        "ipam.prefix.save",
        json!({"id": other, "site": "Branch"}),
    )
    .await;
    let branch = list(&ok(&app, "ipam.prefixes", json!({"site": "Branch"})).await);
    assert_eq!(
        branch
            .iter()
            .map(|row| row["cidr"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["10.0.0.0/8", "10.2.0.0/16"]
    );
    assert_eq!(branch[1]["site"], "Branch");
    assert_eq!(branch[1]["parent_id"], eight.as_str());
    let only = list(
        &ok(
            &app,
            "ipam.prefixes",
            json!({"site": "Branch", "flat": true}),
        )
        .await,
    );
    assert_eq!(
        only.len(),
        1,
        "the flat list is the matches and nothing else"
    );
    assert_eq!(only[0]["cidr"], "10.2.0.0/16");
    assert_eq!(v6_child.len(), 16);
}

#[tokio::test]
async fn utilisation_is_counted_from_what_is_inside_the_block() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    for (ip, status) in [
        ("10.1.10.5", "active"),
        ("10.1.10.6", "active"),
        ("10.1.10.7", "reserved"),
    ] {
        ok(
            &app,
            "ipam.address.save",
            json!({"ip": ip, "prefix_id": id, "status": status, "hostname": "host"}),
        )
        .await;
    }
    let row = ok(&app, "ipam.prefix.get", json!({"id": id})).await["prefix"].clone();
    assert_eq!(row["usable"], 254, "a /24 minus network and broadcast");
    assert_eq!(row["usable_text"], "254");
    assert_eq!(row["used"], 2);
    assert_eq!(row["reserved"], 1);
    assert_eq!(row["free"], 251, "254 - 2 - 1, by hand");
    assert_eq!(row["documented"], 3);
    assert_eq!(row["observed"], 0, "no device has been read");
    assert_eq!(row["util_known"], true);
    let percent = row["util_pct"].as_f64().expect("a percentage");
    assert!(
        (percent - 3.0 / 254.0 * 100.0).abs() < 0.01,
        "three of 254: {percent}"
    );

    // A child prefix covers a quarter of the parent.
    prefix(&app, "10.1.10.0/26", "the first quarter").await;
    let row = ok(&app, "ipam.prefix.get", json!({"id": id})).await["prefix"].clone();
    assert_eq!(row["children"], 1);
    assert_eq!(row["child_coverage_pct"], 25.0);
}

#[tokio::test]
async fn a_small_block_is_listed_address_by_address_and_a_large_one_is_not() {
    let app = app().await;
    let small = prefix(&app, "10.1.10.0/29", "a rack").await;
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.3", "prefix_id": small, "hostname": "switch"}),
    )
    .await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": small})).await;
    assert_eq!(detail["grid"], true);
    let addresses = list(&detail["addresses"]);
    assert_eq!(
        addresses
            .iter()
            .map(|row| row["ip"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        [
            "10.1.10.1",
            "10.1.10.2",
            "10.1.10.3",
            "10.1.10.4",
            "10.1.10.5",
            "10.1.10.6"
        ],
        "every host address of the block, network and broadcast left out"
    );
    let documented = addresses
        .iter()
        .find(|row| row["ip"] == "10.1.10.3")
        .unwrap();
    assert_eq!(documented["status"], "active");
    assert_eq!(documented["documented"], true);
    let empty = addresses
        .iter()
        .find(|row| row["ip"] == "10.1.10.4")
        .unwrap();
    assert_eq!(empty["status"], "free");
    assert_eq!(
        (empty["documented"].as_bool(), empty["observed"].as_bool()),
        (Some(false), Some(false)),
        "`free` here means nobody wrote it down and nothing has answered"
    );
    assert!(
        detail["note"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing documented it and nothing has answered"),
        "{:#}",
        detail["note"]
    );
    let free = list(&detail["free_ranges"]);
    assert_eq!(
        free.iter()
            .map(|range| (
                range["start"].as_str().unwrap_or_default(),
                range["end"].as_str().unwrap_or_default(),
                range["count"].as_u64().unwrap_or_default()
            ))
            .collect::<Vec<_>>(),
        [("10.1.10.1", "10.1.10.2", 2), ("10.1.10.4", "10.1.10.6", 3)]
    );

    // A /16 is not listed address by address.
    let big = prefix(&app, "10.2.0.0/16", "a site").await;
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.2.3.4", "prefix_id": big, "hostname": "one host"}),
    )
    .await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": big})).await;
    assert_eq!(detail["grid"], false);
    assert_eq!(list(&detail["addresses"]).len(), 1, "only what is known");
    assert!(
        detail["note"]
            .as_str()
            .unwrap_or_default()
            .contains("too large to list"),
    );
    // Asking for no addresses at all is cheaper and still gives the numbers.
    let detail = ok(
        &app,
        "ipam.prefix.get",
        json!({"id": big, "addresses": false}),
    )
    .await;
    assert!(list(&detail["addresses"]).is_empty());
    assert_eq!(detail["prefix"]["used"], 1);
}

#[tokio::test]
async fn a_prefix_is_validated_with_a_message_that_says_what_to_write() {
    let app = app().await;
    let detail = err(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.10.7/24", "name": "typo"}),
    )
    .await;
    assert_eq!(detail.code, "invalid");
    assert!(
        detail.message.contains("10.1.10.0/24"),
        "{}",
        detail.message
    );
    assert_eq!(
        detail.details.expect("details")["corrected"],
        "10.1.10.0/24"
    );

    prefix(&app, "10.1.10.0/24", "users").await;
    let duplicate = err(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.10.0/24", "name": "users again"}),
    )
    .await;
    assert_eq!(duplicate.code, "conflict");
    assert!(
        duplicate.message.contains("already exists"),
        "{}",
        duplicate.message
    );
    // The same block in another site is a different prefix.
    ok(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.10.0/24", "name": "users", "site": "Branch"}),
    )
    .await;

    // An overlap that is not containment is a warning, not a refusal.
    let saved = ok(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.10.128/25", "name": "the top half"}),
    )
    .await;
    assert!(
        list(&saved["warnings"]).is_empty(),
        "containment is not an overlap"
    );
    let saved = ok(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.10.192/26", "name": "crossing"}),
    )
    .await;
    assert!(list(&saved["warnings"]).is_empty());
    let crossing = ok(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.0.0/20", "name": "overlaps the /24 and not the /8"}),
    )
    .await;
    assert!(list(&crossing["warnings"]).is_empty(), "containment again");

    // Gateways and DHCP ranges must be inside the block.
    assert_eq!(
        err(
            &app,
            "ipam.prefix.save",
            json!({"cidr": "10.1.10.0/24", "gateway": "10.9.9.1"})
        )
        .await
        .code,
        "invalid"
    );
    let bad_range = err(
        &app,
        "ipam.prefix.save",
        json!({"cidr": "10.1.10.0/24", "dhcp": {"start": "10.1.10.200", "end": "10.1.10.100"}}),
    )
    .await;
    assert!(
        bad_range.message.contains("backwards"),
        "{}",
        bad_range.message
    );
    assert_eq!(
        err(
            &app,
            "ipam.prefix.save",
            json!({"cidr": "10.1.10.0/24", "vlan_id": 5000})
        )
        .await
        .code,
        "invalid"
    );
    assert_eq!(
        err(&app, "ipam.prefix.get", json!({"id": "nope"}))
            .await
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn the_next_free_address_says_what_it_skipped_and_why() {
    let app = app().await;
    let id = ok(
        &app,
        "ipam.prefix.save",
        json!({
            "cidr": "10.1.10.0/24",
            "name": "users",
            "gateway": "10.1.10.1",
            "dhcp": {"start": "10.1.10.100", "end": "10.1.10.199"},
        }),
    )
    .await["prefix"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    for ip in ["10.1.10.2", "10.1.10.3"] {
        ok(
            &app,
            "ipam.address.save",
            json!({"ip": ip, "prefix_id": id, "hostname": "taken"}),
        )
        .await;
    }
    let next = ok(&app, "ipam.next_address", json!({"prefix_id": id})).await;
    assert_eq!(list(&next["addresses"]), [json!("10.1.10.4")]);
    assert_eq!(next["skipped"]["gateway"], 1);
    assert_eq!(next["skipped"]["documented"], 2);
    assert_eq!(next["skipped"]["network"], 1);
    assert_eq!(next["skipped"]["broadcast"], 1);
    assert!(
        next["basis"]
            .as_str()
            .unwrap_or_default()
            .contains("not known to be free")
            || next["basis"]
                .as_str()
                .unwrap_or_default()
                .contains("nothing has ever been seen"),
        "{:#}",
        next["basis"]
    );

    // Several at once, and the DHCP range is stepped over rather than into.
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": id, "count": 3, "skip_first": 94}),
    )
    .await;
    assert_eq!(
        list(&next["addresses"]),
        [
            json!("10.1.10.95"),
            json!("10.1.10.96"),
            json!("10.1.10.97")
        ]
    );
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": id, "count": 2, "skip_first": 97}),
    )
    .await;
    assert_eq!(
        list(&next["addresses"]),
        [json!("10.1.10.98"), json!("10.1.10.99")],
        "the hundred DHCP addresses are not offered"
    );
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": id, "count": 2, "skip_first": 98}),
    )
    .await;
    assert_eq!(
        list(&next["addresses"]),
        [json!("10.1.10.99"), json!("10.1.10.200")]
    );
    assert_eq!(next["skipped"]["dhcp_range"], 100);
}

#[tokio::test]
async fn a_reservation_refuses_an_address_that_is_already_recorded() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    let reserved = ok(
        &app,
        "ipam.address.reserve",
        json!({"prefix_id": id, "hostname": "new printer", "description": "to be installed"}),
    )
    .await;
    assert_eq!(
        reserved["address"]["ip"], "10.1.10.1",
        "the first free address"
    );
    assert_eq!(reserved["address"]["status"], "reserved");
    assert_eq!(reserved["address"]["hostname"], "new printer");

    let again = err(
        &app,
        "ipam.address.reserve",
        json!({"prefix_id": id, "ip": "10.1.10.1", "hostname": "another printer"}),
    )
    .await;
    assert_eq!(again.code, "conflict");
    assert!(
        again.message.contains("new printer") && again.message.contains("reserved"),
        "a reservation never writes over a record: {}",
        again.message
    );

    // The next reservation moves on.
    let next = ok(
        &app,
        "ipam.address.reserve",
        json!({"prefix_id": id, "hostname": "another printer"}),
    )
    .await;
    assert_eq!(next["address"]["ip"], "10.1.10.2");
    assert_eq!(
        err(
            &app,
            "ipam.address.reserve",
            json!({"prefix_id": id, "ip": "10.9.9.9", "hostname": "elsewhere"})
        )
        .await
        .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_record_cannot_claim_a_prefix_it_is_not_inside() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    let outside = err(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.11.5", "prefix_id": id}),
    )
    .await;
    assert_eq!(outside.code, "invalid");
    assert!(
        outside.message.contains("not inside 10.1.10.0/24"),
        "{}",
        outside.message
    );

    // An address with no prefix on record is kept, unfiled, and said to be.
    let saved = ok(
        &app,
        "ipam.address.save",
        json!({"ip": "192.0.2.5", "hostname": "somewhere else"}),
    )
    .await;
    assert_eq!(saved["address"]["prefix_id"], Value::Null);
    assert!(
        list(&saved["warnings"])[0]
            .as_str()
            .unwrap_or_default()
            .contains("no prefix on record"),
    );
    let conflicts = list(&ok(&app, "ipam.conflicts", json!({})).await);
    let row = conflicts
        .iter()
        .find(|row| row["subject"] == "192.0.2.5")
        .unwrap_or_else(|| panic!("{conflicts:#?}"));
    assert_eq!(row["kind"], "outside_any_prefix");

    // A record cannot be `free` (nothing to record) or `conflict` (a finding).
    for status in ["free", "conflict"] {
        let refused = err(
            &app,
            "ipam.address.save",
            json!({"ip": "10.1.10.9", "prefix_id": id, "status": status}),
        )
        .await;
        assert_eq!(refused.code, "invalid", "{status}");
    }
}

#[tokio::test]
async fn deleting_a_prefix_keeps_the_addresses_and_reparents_the_children() {
    let app = app().await;
    let parent = prefix(&app, "10.1.0.0/16", "site one").await;
    let middle = prefix(&app, "10.1.10.0/24", "users").await;
    prefix(&app, "10.1.10.0/26", "a quarter").await;
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.5", "prefix_id": middle, "hostname": "printer"}),
    )
    .await;

    let deleted = ok(&app, "ipam.prefix.delete", json!({"id": middle})).await;
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["cidr"], "10.1.10.0/24");
    assert_eq!(deleted["children_reparented"], 1);
    assert_eq!(deleted["addresses_repointed"], 1);

    let address = ok(&app, "ipam.address.get", json!({"ip": "10.1.10.5"})).await;
    assert_eq!(
        address["address"]["hostname"], "printer",
        "the record survives"
    );
    assert_eq!(address["address"]["prefix_id"], parent.as_str());
    let rows = list(&ok(&app, "ipam.prefixes", json!({})).await);
    let quarter = rows
        .iter()
        .find(|row| row["cidr"] == "10.1.10.0/26")
        .unwrap();
    assert_eq!(quarter["parent_id"], parent.as_str());
    assert_eq!(quarter["depth"], 1);

    // Deleting the record leaves nothing behind but says so.
    let gone = ok(&app, "ipam.address.delete", json!({"ip": "10.1.10.5"})).await;
    assert_eq!(
        (
            gone["deleted"].as_bool(),
            gone["observations_left"].as_u64()
        ),
        (Some(true), Some(0))
    );
    assert_eq!(
        err(&app, "ipam.address.get", json!({"ip": "10.9.9.9"}))
            .await
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_search_finds_the_address_and_the_prefix_that_contains_it() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.25", "prefix_id": id, "hostname": "printer-3",
               "mac": "aa:bb:cc:dd:ee:ff"}),
    )
    .await;
    ok(
        &app,
        "ipam.vlan.save",
        json!({"vid": 10, "name": "users", "prefix_ids": [id]}),
    )
    .await;

    // By address: the record and the block it is in.
    let found = ok(&app, "ipam.search", json!({"query": "10.1.10.25"})).await;
    assert_eq!(list(&found["addresses"])[0]["hostname"], "printer-3");
    assert_eq!(list(&found["prefixes"])[0]["cidr"], "10.1.10.0/24");

    // By part of an address.
    let found = ok(&app, "ipam.search", json!({"query": "10.1.10."})).await;
    assert_eq!(list(&found["addresses"]).len(), 1);

    // By MAC, in a notation nobody typed.
    let found = ok(&app, "ipam.search", json!({"query": "aabb.ccdd.eeff"})).await;
    assert_eq!(list(&found["addresses"])[0]["ip"], "10.1.10.25");

    // By name, which also finds the VLAN and the prefix.
    let found = ok(&app, "ipam.search", json!({"query": "users"})).await;
    assert_eq!(list(&found["prefixes"]).len(), 1);
    assert_eq!(list(&found["vlans"])[0]["vid"], 10);

    // An address nothing knows about: the prefix, and a row that says so.
    let found = ok(&app, "ipam.search", json!({"query": "10.1.10.77"})).await;
    assert_eq!(list(&found["prefixes"])[0]["cidr"], "10.1.10.0/24");
    let row = &list(&found["addresses"])[0];
    assert_eq!(row["ip"], "10.1.10.77");
    assert_eq!(row["status"], "free");
    assert_eq!(
        (row["documented"].as_bool(), row["observed"].as_bool()),
        (Some(false), Some(false))
    );
    assert_eq!(
        err(&app, "ipam.search", json!({"query": "  "})).await.code,
        "invalid"
    );
}

#[tokio::test]
async fn an_import_previews_before_it_commits_and_an_export_round_trips() {
    let app = app().await;
    let csv = "cidr,name,site,vlan_id,role,gateway,tags\n\
               10.1.10.0/24,Users,HQ,10,lan,10.1.10.1,users;wired\n\
               10.1.20.0/24,Voice,HQ,20,voice,10.1.20.1,\n\
               10.1.30.7/24,Broken,HQ,,,,\n\
               10.1.40.0/24,Bad VLAN,HQ,9000,,,\n";
    let preview = ok(
        &app,
        "ipam.import",
        json!({"kind": "prefixes", "text": csv}),
    )
    .await;
    assert_eq!(preview["dry_run"], true);
    assert_eq!(
        (preview["new"].as_u64(), preview["invalid"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(preview["created"], 0, "a preview creates nothing");
    let rows = list(&preview["rows"]);
    assert!(
        rows[2]["problem"]
            .as_str()
            .unwrap_or_default()
            .contains("10.1.30.0/24"),
        "the correction is in the preview: {:#}",
        rows[2]
    );
    assert!(
        rows[3]["problem"]
            .as_str()
            .unwrap_or_default()
            .contains("VLAN id")
    );
    assert_eq!(list(&ok(&app, "ipam.prefixes", json!({})).await).len(), 0);

    let done = ok(
        &app,
        "ipam.import",
        json!({"kind": "prefixes", "text": csv, "dry_run": false}),
    )
    .await;
    assert_eq!(
        (done["created"].as_u64(), done["updated"].as_u64()),
        (Some(2), Some(0))
    );
    let rows = list(&ok(&app, "ipam.prefixes", json!({})).await);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["role"], "lan");
    assert_eq!(rows[0]["vlan_id"], 10);
    assert_eq!(rows[0]["gateway"], "10.1.10.1");
    assert_eq!(list(&rows[0]["tags"]), [json!("users"), json!("wired")]);

    // Export, then import the export: the second time everything is an update.
    let exported = ok(&app, "ipam.export", json!({"kind": "prefixes"})).await;
    let text = exported["text"].as_str().expect("csv").to_string();
    assert!(text.starts_with("cidr,name,description,site,"), "{text}");
    let again = ok(
        &app,
        "ipam.import",
        json!({"kind": "prefixes", "text": text, "dry_run": false}),
    )
    .await;
    assert_eq!(
        (again["created"].as_u64(), again["updated"].as_u64()),
        (Some(0), Some(2))
    );
    assert_eq!(list(&ok(&app, "ipam.prefixes", json!({})).await).len(), 2);

    // Addresses, including one in no prefix at all.
    let addresses = "ip,prefix,status,hostname,description,device,interface,mac,tags\n\
                     10.1.10.5,10.1.10.0/24,active,printer-3,by the door,,,,\n\
                     10.1.10.6,,reserved,spare,,,,,\n\
                     192.0.2.9,,active,elsewhere,,,,,\n";
    let done = ok(
        &app,
        "ipam.import",
        json!({"kind": "addresses", "text": addresses, "dry_run": false}),
    )
    .await;
    assert_eq!(done["created"], 3);
    let detail = ok(&app, "ipam.prefix.get", json!({"id": rows[0]["id"]})).await;
    assert_eq!(detail["prefix"]["used"], 1);
    assert_eq!(detail["prefix"]["reserved"], 1);
    let text = ok(&app, "ipam.export", json!({"kind": "addresses"})).await["text"]
        .as_str()
        .expect("csv")
        .to_string();
    assert_eq!(text.lines().count(), 4, "a header and three records");
    assert!(text.contains("printer-3,by the door"), "{text}");

    // VLANs.
    let vlans = "vid,name,site,description,prefixes\n10,Users,HQ,user VLAN,10.1.10.0/24\n";
    let done = ok(
        &app,
        "ipam.import",
        json!({"kind": "vlans", "text": vlans, "dry_run": false}),
    )
    .await;
    assert_eq!(done["created"], 1);
    let listed = list(&ok(&app, "ipam.vlans", json!({})).await);
    assert_eq!(listed[0]["vid"], 10);
    assert_eq!(listed[0]["documented"], true);
    assert_eq!(list(&listed[0]["prefixes"])[0]["cidr"], "10.1.10.0/24");

    assert_eq!(
        err(&app, "ipam.import", json!({"kind": "nonsense", "text": ""}))
            .await
            .code,
        "invalid"
    );
    assert_eq!(
        err(
            &app,
            "ipam.export",
            json!({"kind": "prefixes", "format": "xlsx"})
        )
        .await
        .code,
        "invalid"
    );
}

#[tokio::test]
async fn vlan_records_are_unique_per_site_and_keep_their_prefixes() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    let saved = ok(
        &app,
        "ipam.vlan.save",
        json!({"vid": 10, "name": "Users", "site": "HQ", "prefix_ids": [id]}),
    )
    .await;
    assert_eq!(saved["documented"], true);
    assert_eq!(saved["observed"], false, "no device has been read");
    assert_eq!(list(&saved["devices"]).len(), 0);

    let duplicate = err(
        &app,
        "ipam.vlan.save",
        json!({"vid": 10, "name": "Users again", "site": "HQ"}),
    )
    .await;
    assert_eq!(duplicate.code, "conflict");
    // The same VLAN id in another site is another VLAN.
    ok(
        &app,
        "ipam.vlan.save",
        json!({"vid": 10, "name": "Users", "site": "Branch"}),
    )
    .await;
    assert_eq!(
        err(&app, "ipam.vlan.save", json!({"vid": 4095})).await.code,
        "invalid"
    );
    assert_eq!(
        err(
            &app,
            "ipam.vlan.save",
            json!({"vid": 10, "prefix_ids": ["nope"]})
        )
        .await
        .code,
        "not_found"
    );
    assert_eq!(
        list(&ok(&app, "ipam.vlans", json!({"site": "HQ"})).await).len(),
        1
    );

    let deleted = ok(
        &app,
        "ipam.vlan.delete",
        json!({"id": saved["id"].as_str().expect("an id")}),
    )
    .await;
    assert_eq!(
        (deleted["deleted"].as_bool(), deleted["vid"].as_u64()),
        (Some(true), Some(10))
    );
    assert_eq!(
        err(&app, "ipam.vlan.delete", json!({"id": "nope"}))
            .await
            .code,
        "not_found"
    );
    // The prefix is untouched by its VLAN record going away.
    assert_eq!(list(&ok(&app, "ipam.prefixes", json!({})).await).len(), 1);
}

#[tokio::test]
async fn the_summary_counts_records_and_sightings_as_different_things() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.5", "prefix_id": id, "hostname": "printer"}),
    )
    .await;
    ok(&app, "ipam.vlan.save", json!({"vid": 10, "name": "users"})).await;

    let summary = ok(&app, "ipam.summary", json!({})).await;
    assert_eq!(summary["prefixes"], 1);
    assert_eq!(summary["prefixes_discovered"], 0);
    assert_eq!(summary["addresses"], 1, "records");
    assert_eq!(
        summary["observations"], 0,
        "sightings — nothing was collected"
    );
    assert_eq!(summary["vlans"], 1);
    assert_eq!(summary["conflicts"], 0);
    assert_eq!(summary["devices_total"], 0);
    let utilisation = list(&summary["utilisation"]);
    assert_eq!(utilisation[0]["prefix"], "10.1.10.0/24");
    assert_eq!(utilisation[0]["used"], 1);
    assert_eq!(utilisation[0]["util_known"], true);
}

#[tokio::test]
async fn every_change_announces_itself() {
    let app = app().await;
    let mut events = app.subscribe();
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    let event = next_change(&mut events).await;
    assert_eq!(event.payload["kinds"], json!(["prefixes"]));

    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.5", "prefix_id": id, "hostname": "printer"}),
    )
    .await;
    let event = next_change(&mut events).await;
    assert_eq!(event.payload["kinds"], json!(["addresses"]));

    // And it is in the audit trail, in the past tense.
    let audit = ok(&app, "audit.list", json!({"limit": 50})).await;
    let entries = list(&audit["entries"]);
    assert!(
        entries
            .iter()
            .any(|entry| entry["source"] == "ipam.prefix.save"
                && entry["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("Added prefix 10.1.10.0/24")),
        "{entries:#?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["source"] == "ipam.address.save"),
        "{entries:#?}"
    );
}

#[tokio::test]
async fn without_a_single_device_the_reports_say_so_rather_than_claiming_a_clean_bill() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/24", "users").await;
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.5", "prefix_id": id, "hostname": "printer"}),
    )
    .await;
    let synced = ok(&app, "ipam.sync", json!({})).await;
    assert_eq!(synced["addresses_updated"], 0);
    assert_eq!(list(&synced["devices"]).len(), 0);

    let report = ok(&app, "ipam.reconcile", json!({})).await;
    assert_eq!(report["coverage"]["devices_total"], 0);
    assert_eq!(report["coverage"]["newest_collection"], Value::Null);
    let unobserved = list(&report["unobserved"]);
    assert_eq!(unobserved.len(), 1);
    assert_eq!(unobserved[0]["had_coverage"], false);
    assert!(
        unobserved[0]["summary"]
            .as_str()
            .unwrap_or_default()
            .contains("no evidence either way"),
        "{:#}",
        unobserved[0]
    );
    assert!(list(&report["undocumented"]).is_empty());
    let learn = ok(&app, "ipam.learn", json!({"dry_run": false})).await;
    assert_eq!(
        learn["created_addresses"], 0,
        "there is nothing to learn from"
    );
}
