//! The edges, through the RPC surface: a /31, a /32, the first and last
//! address of a block, IPv6 beside IPv4, /0, and the splits.
//!
//! The arithmetic itself belongs to `nocturne-tools`; what is checked here is
//! that this feature uses it with the right conventions, with numbers worked
//! out by hand.

use nocturne_app::App;
use serde_json::{Value, json};

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

async fn prefix(app: &App, cidr: &str) -> String {
    ok(app, "ipam.prefix.save", json!({"cidr": cidr, "name": cidr})).await["prefix"]["id"]
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

fn ips(value: &Value) -> Vec<String> {
    list(value)
        .iter()
        .map(|row| {
            row["ip"]
                .as_str()
                .or_else(|| row.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_point_to_point_link_has_two_addresses_and_a_host_route_has_one() {
    let app = app().await;
    let link = prefix(&app, "192.168.1.0/31").await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": link})).await;
    assert_eq!(detail["prefix"]["usable"], 2, "RFC 3021");
    assert_eq!(detail["prefix"]["usable_text"], "2");
    assert_eq!(detail["prefix"]["free"], 2);
    assert_eq!(
        ips(&detail["addresses"]),
        ["192.168.1.0", "192.168.1.1"],
        "both addresses of a /31 are hosts"
    );
    let free = list(&detail["free_ranges"]);
    assert_eq!(
        (
            free[0]["start"].as_str(),
            free[0]["end"].as_str(),
            free[0]["count"].as_u64()
        ),
        (Some("192.168.1.0"), Some("192.168.1.1"), Some(2))
    );
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": link, "count": 3}),
    )
    .await;
    assert_eq!(ips(&next["addresses"]), ["192.168.1.0", "192.168.1.1"]);
    assert_eq!(next["exhausted"], true, "there is no third address");
    assert_eq!(
        next["skipped"]["network"], 0,
        "a /31 has no network address"
    );
    assert_eq!(next["skipped"]["broadcast"], 0);

    let host = prefix(&app, "10.255.0.1/32").await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": host})).await;
    assert_eq!(detail["prefix"]["usable"], 1);
    assert_eq!(ips(&detail["addresses"]), ["10.255.0.1"]);
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": host, "count": 2}),
    )
    .await;
    assert_eq!(ips(&next["addresses"]), ["10.255.0.1"]);
    // Once it is taken there is nothing left, and that is not an error.
    ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.255.0.1", "prefix_id": host, "hostname": "a loopback"}),
    )
    .await;
    let next = ok(&app, "ipam.next_address", json!({"prefix_id": host})).await;
    assert!(list(&next["addresses"]).is_empty());
    assert_eq!(next["exhausted"], true);
    assert_eq!(next["skipped"]["documented"], 1);
    let detail = ok(&app, "ipam.prefix.get", json!({"id": host})).await;
    assert_eq!(detail["prefix"]["free"], 0);
    assert_eq!(detail["prefix"]["util_pct"], 100.0);
    assert!(list(&detail["free_ranges"]).is_empty());
}

#[tokio::test]
async fn the_network_and_broadcast_addresses_of_a_block_are_not_hosts() {
    let app = app().await;
    let id = prefix(&app, "10.1.10.0/30").await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": id})).await;
    assert_eq!(detail["prefix"]["usable"], 2);
    assert_eq!(
        ips(&detail["addresses"]),
        ["10.1.10.1", "10.1.10.2"],
        "the grid of a /30 is the two hosts"
    );
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": id, "count": 4}),
    )
    .await;
    assert_eq!(ips(&next["addresses"]), ["10.1.10.1", "10.1.10.2"]);
    assert_eq!(next["skipped"]["network"], 1);
    assert_eq!(next["skipped"]["broadcast"], 1);

    // A record on the network address is allowed — real networks have them,
    // on purpose, and refusing would lose the information — but it is said.
    let saved = ok(
        &app,
        "ipam.address.save",
        json!({"ip": "10.1.10.0", "prefix_id": id, "hostname": "the network address"}),
    )
    .await;
    assert!(
        list(&saved["warnings"])[0]
            .as_str()
            .unwrap_or_default()
            .contains("network, broadcast or anycast address"),
        "{saved:#}"
    );
    // It is then listed outside the host range rather than hidden.
    let detail = ok(&app, "ipam.prefix.get", json!({"id": id})).await;
    assert_eq!(
        ips(&detail["addresses"]),
        ["10.1.10.0", "10.1.10.1", "10.1.10.2"]
    );
    // And the next free address still never offers it.
    let next = ok(&app, "ipam.next_address", json!({"prefix_id": id})).await;
    assert_eq!(ips(&next["addresses"]), ["10.1.10.1"]);
}

#[tokio::test]
async fn ipv6_sits_beside_ipv4_with_its_own_conventions() {
    let app = app().await;
    let sixty_four = prefix(&app, "2001:db8:a::/64").await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": sixty_four})).await;
    assert_eq!(
        detail["prefix"]["usable_text"], "18446744073709551615",
        "every address but the Subnet-Router anycast one, exactly"
    );
    assert_eq!(
        detail["prefix"]["util_known"], false,
        "a percentage of 2^64 means nothing, and saying 0% would be a lie"
    );
    assert_eq!(detail["prefix"]["util_pct"], 0.0);
    assert_eq!(detail["grid"], false, "nobody lists a /64");
    let next = ok(
        &app,
        "ipam.next_address",
        json!({"prefix_id": sixty_four, "count": 2}),
    )
    .await;
    assert_eq!(
        ips(&next["addresses"]),
        ["2001:db8:a::1", "2001:db8:a::2"],
        "the anycast address at the bottom of the subnet is skipped"
    );
    assert_eq!(next["skipped"]["anycast"], 1);
    assert_eq!(
        next["skipped"]["broadcast"], 0,
        "IPv6 has no broadcast address"
    );

    // A /126 is small enough to list, and a /127 is a point-to-point link.
    let quad = prefix(&app, "2001:db8:b::/126").await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": quad})).await;
    assert_eq!(detail["prefix"]["usable"], 3);
    assert_eq!(
        ips(&detail["addresses"]),
        ["2001:db8:b::1", "2001:db8:b::2", "2001:db8:b::3"]
    );
    let p2p = prefix(&app, "2001:db8:c::/127").await;
    let detail = ok(&app, "ipam.prefix.get", json!({"id": p2p})).await;
    assert_eq!(detail["prefix"]["usable"], 2, "RFC 6164");
    assert_eq!(ips(&detail["addresses"]), ["2001:db8:c::", "2001:db8:c::1"]);

    // Addresses are stored in their canonical form, however they are typed.
    let saved = ok(
        &app,
        "ipam.address.save",
        json!({"ip": "2001:0DB8:000A:0000:0000:0000:0000:0001", "hostname": "a server"}),
    )
    .await;
    assert_eq!(saved["address"]["ip"], "2001:db8:a::1");
    assert_eq!(saved["address"]["prefix_id"], sixty_four.as_str());
    let found = ok(&app, "ipam.search", json!({"query": "2001:db8:a::1"})).await;
    assert_eq!(ips(&found["addresses"]), ["2001:db8:a::1"]);
    assert_eq!(
        list(&found["prefixes"])[0]["cidr"],
        "2001:db8:a::/64",
        "an address finds the block that contains it"
    );
    // The two families never contain one another.
    let v4 = prefix(&app, "0.0.0.0/0").await;
    let rows = list(&ok(&app, "ipam.prefixes", json!({})).await);
    let v6_rows: Vec<&Value> = rows
        .iter()
        .filter(|row| row["cidr"].as_str().unwrap_or_default().contains(':'))
        .collect();
    assert!(
        v6_rows.iter().all(|row| row["parent_id"] != v4.as_str()),
        "0.0.0.0/0 is not the parent of the IPv6 world"
    );
    let detail = ok(&app, "ipam.prefix.get", json!({"id": v4})).await;
    assert_eq!(detail["prefix"]["usable"], 4_294_967_294u64);
    assert_eq!(detail["prefix"]["util_known"], true);
}

#[tokio::test]
async fn a_split_creates_what_it_said_it_would() {
    let app = app().await;
    let id = prefix(&app, "10.1.0.0/22").await;

    // Equal blocks, previewed first.
    let preview = ok(
        &app,
        "ipam.prefix.split",
        json!({"id": id, "new_len": 24, "dry_run": true}),
    )
    .await;
    assert_eq!(preview["created"], 0);
    assert_eq!(
        list(&preview["children"])
            .iter()
            .map(|child| child["cidr"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        ["10.1.0.0/24", "10.1.1.0/24", "10.1.2.0/24", "10.1.3.0/24"]
    );
    assert_eq!(list(&ok(&app, "ipam.prefixes", json!({})).await).len(), 1);

    let done = ok(&app, "ipam.prefix.split", json!({"id": id, "new_len": 24})).await;
    assert_eq!(done["created"], 4);
    let rows = list(&ok(&app, "ipam.prefixes", json!({"parent_id": id})).await);
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|row| row["depth"] == 1 && row["parent_id"] == id.as_str())
    );
    let parent = ok(&app, "ipam.prefix.get", json!({"id": id})).await;
    assert_eq!(parent["prefix"]["children"], 4);
    assert_eq!(parent["prefix"]["child_coverage_pct"], 100.0);
    assert!(
        list(&parent["free_child_prefixes"]).is_empty(),
        "there is no room left for another child"
    );

    // Splitting again refuses to make the same blocks twice.
    let again = ok(&app, "ipam.prefix.split", json!({"id": id, "new_len": 24})).await;
    assert_eq!(again["created"], 0);
    assert!(
        list(&again["children"]).iter().all(|child| child["skipped"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists")),
        "{again:#}"
    );

    // And by host counts (VLSM), largest first, aligned.
    let vlsm = prefix(&app, "10.2.0.0/22").await;
    let plan = ok(
        &app,
        "ipam.prefix.split",
        json!({
            "id": vlsm,
            "sizes": [{"name": "users", "hosts": 500}, {"name": "voice", "hosts": 200}, {"name": "link", "hosts": 2}],
        }),
    )
    .await;
    assert_eq!(
        list(&plan["children"])
            .iter()
            .map(|child| (
                child["name"].as_str().unwrap_or_default().to_string(),
                child["cidr"].as_str().unwrap_or_default().to_string(),
                child["usable"].as_u64().unwrap_or_default(),
                child["spare"].as_u64().unwrap_or_default()
            ))
            .collect::<Vec<_>>(),
        [
            ("users".to_string(), "10.2.0.0/23".to_string(), 510, 10),
            ("voice".to_string(), "10.2.2.0/24".to_string(), 254, 54),
            ("link".to_string(), "10.2.3.0/30".to_string(), 2, 0),
        ]
    );
    // A bare list of host counts works too, and /31 links if asked.
    let p2p = prefix(&app, "10.3.0.0/29").await;
    let plan = ok(
        &app,
        "ipam.prefix.split",
        json!({"id": p2p, "sizes": [2, 2], "p2p31": true, "dry_run": true}),
    )
    .await;
    assert_eq!(
        list(&plan["children"])
            .iter()
            .map(|child| child["cidr"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        ["10.3.0.0/31", "10.3.0.2/31"]
    );

    // A split that cannot fit says which request did not.
    let err = app
        .rpc(
            "ipam.prefix.split",
            json!({"id": p2p, "sizes": [{"name": "far too many", "hosts": 5000}]}),
        )
        .await
        .expect_err("it does not fit");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("far too many"), "{}", err.message);
}

#[tokio::test]
async fn the_next_free_prefix_finds_the_hole_and_says_when_there_is_none() {
    let app = app().await;
    let id = prefix(&app, "10.1.0.0/22").await;
    prefix(&app, "10.1.0.0/24").await;
    prefix(&app, "10.1.1.0/24").await;

    let next = ok(
        &app,
        "ipam.next_prefix",
        json!({"parent_id": id, "len": 24}),
    )
    .await;
    assert_eq!(next["cidr"], "10.1.2.0/24");
    assert_eq!(
        list(&next["free_space"])
            .iter()
            .map(|cidr| cidr.as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        ["10.1.2.0/23"]
    );
    assert_eq!(
        list(&ok(&app, "ipam.prefixes", json!({})).await).len(),
        3,
        "nothing was created"
    );
    // A smaller block fits in the same hole.
    assert_eq!(
        ok(
            &app,
            "ipam.next_prefix",
            json!({"parent_id": id, "len": 28})
        )
        .await["cidr"],
        "10.1.2.0/28"
    );
    // Fill it up: there is then no answer, which is not an error.
    prefix(&app, "10.1.2.0/23").await;
    let next = ok(
        &app,
        "ipam.next_prefix",
        json!({"parent_id": id, "len": 24}),
    )
    .await;
    assert_eq!(next["cidr"], Value::Null);
    assert!(list(&next["free_space"]).is_empty());

    // A length that is not inside the parent is refused with a reason.
    let err = app
        .rpc("ipam.next_prefix", json!({"parent_id": id, "len": 21}))
        .await
        .expect_err("a /21 does not come out of a /22");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("larger than"), "{}", err.message);
    assert_eq!(
        app.rpc("ipam.next_prefix", json!({"parent_id": id, "len": 40}))
            .await
            .expect_err("not an IPv4 length")
            .code,
        "invalid"
    );
}
