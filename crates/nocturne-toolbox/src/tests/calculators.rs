//! Every calculator through `app.rpc`: one answer that was worked out by hand,
//! and one refusal.
//!
//! These are tests of the façade — the wiring, the parameter names, the result
//! shapes and the error mapping — not of the arithmetic, which has 188 tests
//! of its own in `nocturne-tools`. The values are still hand-checked rather
//! than copied from the engine's output: a calculator that is wired to the
//! wrong function would otherwise pass.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support;

/// Call a method on an app with nothing else going on.
async fn call(method: &str, params: Value) -> Value {
    let lab = support::lab(&[]).await;
    lab.app
        .rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method} failed: {err}"))
}

async fn refuse(method: &str, params: Value) -> nocturne_app::AppError {
    let lab = support::lab(&[]).await;
    lab.app
        .rpc(method, params)
        .await
        .expect_err(&format!("{method} should have refused"))
}

#[tokio::test]
async fn the_subnet_calculator_answers_exactly_and_refuses_precisely() {
    let info = call("tools.subnet", json!({"input": "192.168.1.10/24"})).await;
    // Hand-checked: a /24 has 254 usable addresses, .0 and .255 at the ends.
    assert_eq!(info["network"], "192.168.1.0");
    assert_eq!(info["broadcast"], "192.168.1.255");
    assert_eq!(info["first_host"], "192.168.1.1");
    assert_eq!(info["last_host"], "192.168.1.254");
    assert_eq!(info["usable_hosts"], "254");
    assert_eq!(info["netmask"], "255.255.255.0");
    assert_eq!(info["wildcard"], "0.0.0.255");
    assert_eq!(info["class"], "C");
    assert_eq!(info["scope"], "private (RFC 1918)");
    assert_eq!(info["reverse_dns"], "10.1.168.192.in-addr.arpa");

    // A mask instead of a prefix, and a /31, which has two hosts and no
    // broadcast address.
    let masked = call("tools.subnet", json!({"input": "10.0.0.5 255.255.255.254"})).await;
    assert_eq!(masked["prefix"], 31);
    assert_eq!(masked["usable_hosts"], "2");
    assert_eq!(masked["broadcast"], "");

    let v6 = call("tools.subnet", json!({"input": "2001:db8::1/64"})).await;
    assert_eq!(v6["version"], 6);
    assert_eq!(v6["total_addresses"], "18446744073709551616");
    assert_eq!(v6["expanded"], "2001:0db8:0000:0000:0000:0000:0000:0001");

    let refused = refuse("tools.subnet", json!({"input": "10.0.300.1/24"})).await;
    assert_eq!(refused.code, "invalid");
    assert!(
        refused.message.contains("300"),
        "the engine's own words: {}",
        refused.message
    );
    // A missing parameter is a parameter error, not a default answer.
    assert_eq!(refuse("tools.subnet", json!({})).await.code, "invalid");
}

#[tokio::test]
async fn a_network_splits_by_prefix_length_or_by_how_many_are_wanted() {
    let by_length = call(
        "tools.subnet.split",
        json!({"cidr": "10.0.0.0/24", "new_len": 26}),
    )
    .await;
    assert_eq!(by_length["total"], "4");
    assert_eq!(by_length["subnets"][0]["cidr"], "10.0.0.0/26");
    assert_eq!(by_length["subnets"][3]["cidr"], "10.0.0.192/26");
    assert_eq!(by_length["subnets"][3]["last_host"], "10.0.0.254");

    let by_count = call(
        "tools.subnet.split",
        json!({"cidr": "10.0.0.0/24", "count": 2}),
    )
    .await;
    assert_eq!(by_count["total"], "2");
    assert_eq!(by_count["subnets"][1]["cidr"], "10.0.0.128/25");

    // Only the first `limit` are returned, and it says so.
    let limited = call(
        "tools.subnet.split",
        json!({"cidr": "10.0.0.0/16", "new_len": 24, "limit": 3}),
    )
    .await;
    assert_eq!(limited["total"], "256");
    assert_eq!(limited["subnets"].as_array().unwrap().len(), 3);
    assert_eq!(limited["truncated"], true);

    let silent = refuse("tools.subnet.split", json!({"cidr": "10.0.0.0/24"})).await;
    assert_eq!(silent.code, "invalid");
    assert!(silent.message.contains("new_len"), "{}", silent.message);
    assert_eq!(
        refuse(
            "tools.subnet.split",
            json!({"cidr": "10.0.0.0/24", "new_len": 20})
        )
        .await
        .code,
        "invalid",
        "a bigger prefix than the network is not a split"
    );
}

#[tokio::test]
async fn a_vlsm_plan_allocates_largest_first_and_says_what_did_not_fit() {
    let plan = call(
        "tools.vlsm",
        json!({"base": "10.0.0.0/24", "sizes": [
            {"name": "desks", "hosts": 100},
            {"name": "phones", "hosts": 50},
            {"name": "link", "hosts": 2}
        ]}),
    )
    .await;
    let allocations = plan["allocations"].as_array().unwrap();
    // Hand-checked: 100 hosts needs a /25, 50 a /26, 2 a /30, in that order.
    assert_eq!(allocations[0]["name"], "desks");
    assert_eq!(allocations[0]["cidr"], "10.0.0.0/25");
    assert_eq!(allocations[0]["usable_hosts"], 126);
    assert_eq!(allocations[1]["cidr"], "10.0.0.128/26");
    assert_eq!(allocations[2]["cidr"], "10.0.0.192/30");
    assert_eq!(allocations[2]["spare"], 0);

    // A /31 for a point-to-point link when it is asked for.
    let p2p = call(
        "tools.vlsm",
        json!({"base": "10.0.0.0/24", "sizes": [{"name": "link", "hosts": 2}], "p2p31": true}),
    )
    .await;
    assert_eq!(p2p["allocations"][0]["cidr"], "10.0.0.0/31");

    let too_big = refuse(
        "tools.vlsm",
        json!({"base": "10.0.0.0/28", "sizes": [{"name": "campus", "hosts": 500}]}),
    )
    .await;
    assert_eq!(too_big.code, "invalid");
    assert!(too_big.message.contains("campus"), "{}", too_big.message);
    assert_eq!(
        refuse("tools.vlsm", json!({"base": "10.0.0.0/24"}))
            .await
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn networks_are_summarised_and_compared() {
    let summary = call(
        "tools.supernet",
        json!({"cidrs": ["10.0.1.0/24", "10.0.0.0/24"]}),
    )
    .await;
    // Two adjacent /24s are one /23 — and that is also their supernet.
    assert_eq!(summary["summarized"], json!(["10.0.0.0/23"]));
    assert_eq!(summary["supernet"], "10.0.0.0/23");

    let apart = call(
        "tools.supernet",
        json!({"cidrs": ["10.0.0.0/24", "10.0.8.0/24"]}),
    )
    .await;
    assert_eq!(apart["summarized"], json!(["10.0.0.0/24", "10.0.8.0/24"]));
    assert_eq!(apart["supernet"], "10.0.0.0/20");

    let inside = call(
        "tools.contains",
        json!({"a": "10.0.0.0/22", "b": "10.0.2.7"}),
    )
    .await;
    assert_eq!(
        (&inside["contains"], &inside["overlaps"]),
        (&json!(true), &json!(true))
    );
    let outside = call(
        "tools.contains",
        json!({"a": "10.0.0.0/22", "b": "10.0.4.7"}),
    )
    .await;
    assert_eq!(outside["contains"], false);

    let overlaps = call(
        "tools.overlaps",
        json!({"cidrs": ["10.0.0.0/24", "10.0.0.128/25", "192.168.0.0/24"]}),
    )
    .await;
    assert_eq!(overlaps["any"], true);
    assert_eq!(overlaps["pairs"].as_array().unwrap().len(), 1);
    assert_eq!(overlaps["pairs"][0]["b"], "10.0.0.128/25");

    let range = call(
        "tools.ip.range",
        json!({"first": "10.0.0.5", "last": "10.0.0.10"}),
    )
    .await;
    // Hand-checked: .5 alone, then .6-.7, then .8-.9, then .10 alone.
    assert_eq!(
        range["cidrs"],
        json!(["10.0.0.5/32", "10.0.0.6/31", "10.0.0.8/31", "10.0.0.10/32"])
    );

    assert_eq!(
        refuse("tools.supernet", json!({"cidrs": []})).await.code,
        "invalid"
    );
    assert_eq!(
        refuse(
            "tools.contains",
            json!({"a": "not a network", "b": "10.0.0.1"})
        )
        .await
        .code,
        "invalid"
    );
}

#[tokio::test]
async fn an_address_is_shown_in_every_form_it_can_be_written_in() {
    let forms = call("tools.ip.convert", json!({"input": "192.168.1.1"})).await;
    // 192.168.1.1 is 3232235777, 0xC0A80101.
    assert_eq!(forms["number"]["decimal"], "3232235777");
    assert_eq!(forms["number"]["hex"], "0xC0A80101");
    assert_eq!(forms["address"]["reverse_dns"], "1.1.168.192.in-addr.arpa");
    assert_eq!(forms["address"]["version"], 4);

    // The same address written as a number comes back as the address.
    let from_number = call("tools.ip.convert", json!({"input": "3232235777"})).await;
    assert_eq!(from_number["address"]["address"], "192.168.1.1");
    let from_hex = call("tools.ip.convert", json!({"input": "0xC0A80101"})).await;
    assert_eq!(from_hex["address"]["address"], "192.168.1.1");

    let v6 = call("tools.ip.convert", json!({"input": "2001:0db8:0000::0001"})).await;
    assert_eq!(v6["address"]["compressed"], "2001:db8::1");
    assert_eq!(
        v6["address"]["expanded"],
        "2001:0db8:0000:0000:0000:0000:0000:0001"
    );

    assert_eq!(
        refuse("tools.ip.convert", json!({"input": "what"}))
            .await
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn masks_wildcards_and_acl_pairs_are_explained() {
    let from_prefix = call("tools.wildcard", json!({"input": "/26"})).await;
    assert_eq!(from_prefix["netmask"], "255.255.255.192");
    assert_eq!(from_prefix["wildcard"], "0.0.0.63");
    assert_eq!(from_prefix["addresses"], "64");
    assert_eq!(from_prefix["contiguous"], true);

    let from_wildcard = call("tools.wildcard", json!({"input": "0.0.0.255"})).await;
    assert_eq!(from_wildcard["prefix"], 24);

    // A non-contiguous wildcard is a real thing in an ACL, and says so.
    let odd = call("tools.wildcard", json!({"input": "0.0.0.254"})).await;
    assert_eq!(odd["contiguous"], false);
    assert_eq!(odd["prefix"], Value::Null);

    let acl = call(
        "tools.wildcard.acl",
        json!({"address": "10.0.0.0", "wildcard": "0.0.0.255", "candidate": "10.0.0.42"}),
    )
    .await;
    assert_eq!(acl["matches"], true);
    assert!(
        acl["explanation"].as_str().unwrap().contains("10.0.0."),
        "{}",
        acl["explanation"]
    );
    let outside = call(
        "tools.wildcard.acl",
        json!({"address": "10.0.0.0", "wildcard": "0.0.0.255", "candidate": "10.0.1.42"}),
    )
    .await;
    assert_eq!(outside["matches"], false);

    assert_eq!(
        refuse("tools.wildcard", json!({"input": "nonsense"}))
            .await
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_mac_address_is_normalised_and_its_vendor_looked_up_both_ways() {
    let info = call("tools.mac", json!({"input": "0000.0c12.3456"})).await;
    assert_eq!(info["colon"], "00:00:0c:12:34:56");
    assert_eq!(info["hyphen"], "00-00-0C-12-34-56");
    assert_eq!(info["dotted"], "0000.0c12.3456");
    assert_eq!(info["bare"], "00000c123456");
    assert_eq!(info["oui"], "00:00:0C");
    assert_eq!(info["vendor"], "Cisco Systems");
    assert_eq!(info["is_multicast"], false);
    assert_eq!(info["is_locally_administered"], false);
    // The SLAAC address of 00:00:0c:12:34:56 flips the local bit: 02:00:0c…
    assert_eq!(info["link_local_ipv6"], "fe80::200:cff:fe12:3456");

    let broadcast = call("tools.mac", json!({"input": "ff:ff:ff:ff:ff:ff"})).await;
    assert_eq!(
        (&broadcast["is_broadcast"], &broadcast["is_multicast"]),
        (&json!(true), &json!(true))
    );

    // By prefix: one answer, with what the bundled registry is.
    let by_prefix = call("tools.oui", json!({"query": "00:00:0c"})).await;
    assert_eq!(by_prefix["read_as"], "prefix");
    assert_eq!(by_prefix["assignments"][0]["vendor"], "Cisco Systems");
    assert!(by_prefix["registry"]["assignments"].as_u64().unwrap() > 1000);
    assert!(by_prefix["registry"]["date"].as_str().unwrap().len() >= 10);

    // A prefix nobody has registered is an empty list, not a guess.
    let unregistered = call("tools.oui", json!({"query": "02:00:00"})).await;
    assert_eq!(unregistered["total"], 0);

    assert_eq!(
        refuse("tools.mac", json!({"input": "00:00:0c:12:34"}))
            .await
            .code,
        "invalid"
    );
    assert_eq!(
        refuse("tools.oui", json!({"query": " "})).await.code,
        "invalid"
    );
}

#[tokio::test]
async fn a_vendor_name_finds_the_prefixes_registered_to_it() {
    // The other direction, which the engine cannot do: see `calc::identity`.
    let found = call(
        "tools.oui",
        json!({"query": "cisco systems", "limit": 1000}),
    )
    .await;
    assert_eq!(found["read_as"], "vendor");
    // Cisco holds hundreds of blocks, 00:00:0C among them — the one every
    // engineer recognises, and a check that the index really is the registry.
    assert!(found["total"].as_u64().unwrap() > 100, "{}", found["total"]);
    assert!(
        found["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["prefix"] == "00:00:0C"),
        "00:00:0C is missing from a search for Cisco"
    );
    assert!(found["assignments"].as_array().unwrap().len() <= 1000);
    // And `limit` really limits.
    let few = call("tools.oui", json!({"query": "cisco systems", "limit": 5})).await;
    assert_eq!(few["assignments"].as_array().unwrap().len(), 5);
    assert_eq!(few["total"], found["total"]);
    let vendor = found["assignments"][0]["vendor"].as_str().unwrap();
    assert!(vendor.to_lowercase().contains("cisco"), "{vendor}");
    // The prefix it found really is that vendor's, according to the engine.
    let prefix = found["assignments"][0]["prefix"].as_str().unwrap();
    let back = call("tools.oui", json!({"query": prefix})).await;
    assert_eq!(back["assignments"][0]["vendor"], vendor);
    // And the answer says what a name search does not cover.
    assert!(found["note"].as_str().unwrap().contains("MA-M"), "{found}");

    let nobody = call("tools.oui", json!({"query": "no such company anywhere"})).await;
    assert_eq!(nobody["total"], 0);
    assert!(nobody["assignments"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn snmp_objects_and_interface_names_resolve_both_ways() {
    let by_name = call("tools.oid", json!({"input": "ifInOctets"})).await;
    assert_eq!(by_name["oid"], "1.3.6.1.2.1.2.2.1.10");
    assert_eq!(by_name["known"], true);

    let by_oid = call("tools.oid", json!({"input": "1.3.6.1.2.1.1.5.0"})).await;
    assert_eq!(by_oid["name"], "sysName.0");

    let trap = call("tools.oid", json!({"input": "1.3.6.1.6.3.1.1.5.3"})).await;
    assert_eq!(trap["notification"], "linkDown");

    let unknown = call("tools.oid", json!({"input": "1.3.6.1.4.1.99999.7"})).await;
    assert_eq!(unknown["known"], false);
    assert_eq!(unknown["name"], Value::Null);

    let short = call("tools.interface", json!({"input": "Gi1/0/1"})).await;
    assert_eq!(short["long"], "GigabitEthernet1/0/1");
    assert_eq!(short["short"], "Gi1/0/1");
    let long = call(
        "tools.interface",
        json!({"input": "TenGigabitEthernet1/1/1"}),
    )
    .await;
    assert_eq!(long["short"], "Te1/1/1");

    assert_eq!(
        refuse("tools.oid", json!({"input": ""})).await.code,
        "invalid"
    );
}

#[tokio::test]
async fn the_reversible_encodings_decode_and_the_hashes_are_named_not_attempted() {
    // `0822455D0A16` is `cisco`: the canonical type-7 example.
    let seven = call(
        "tools.secret.decode",
        json!({"text": "username admin password 7 0822455D0A16"}),
    )
    .await;
    assert_eq!(seven["kind"], "cisco7");
    assert_eq!(seven["secret"], "cisco");
    assert!(seven["note"].as_str().unwrap().contains("not encryption"));

    // Naming the encoding explicitly works too.
    let named = call(
        "tools.secret.decode",
        json!({"kind": "cisco7", "text": "0822455D0A16"}),
    )
    .await;
    assert_eq!(named["secret"], "cisco");

    let identified = call(
        "tools.secret.identify",
        json!({"text": "enable secret 5 $1$mERr$3GCGQMcGNJZPAJLtgvmgO0"}),
    )
    .await;
    assert_eq!(identified["kind"], "cisco_type5");
    assert_eq!(identified["reversible"], false);
    assert_eq!(identified["decodable"], false);
    assert_eq!(identified["decoder"], Value::Null);
    assert!(identified["advice"].as_str().unwrap().contains("type 8"));

    let refused = refuse(
        "tools.secret.decode",
        json!({"text": "enable secret 5 $1$mERr$3GCGQMcGNJZPAJLtgvmgO0"}),
    )
    .await;
    assert_eq!(refused.code, "invalid");
    assert!(
        refused.message.contains("cannot be decoded"),
        "{}",
        refused.message
    );

    // Encoding is for comparing with a configuration, and round-trips.
    let encoded = call(
        "tools.secret.encode",
        json!({"kind": "cisco7", "text": "lab-only", "salt": 7}),
    )
    .await;
    let round_trip = call(
        "tools.secret.decode",
        json!({"text": encoded["encoded"].as_str().unwrap()}),
    )
    .await;
    assert_eq!(round_trip["secret"], "lab-only");

    assert_eq!(
        refuse(
            "tools.secret.encode",
            json!({"kind": "sha512", "text": "x"})
        )
        .await
        .code,
        "invalid"
    );
}

/// The difference between "no key can undo this" and "a key can, but the
/// device has it" is the one thing `decodable` exists to say, so a refusal
/// must not flatten it into "that is a hash" — type 6 is AES, not a hash.
#[tokio::test]
async fn a_secret_only_the_device_can_undo_is_refused_for_that_reason_not_as_a_hash() {
    let line = "crypto isakmp key 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB address 192.0.2.1";

    let identified = call("tools.secret.identify", json!({"text": line})).await;
    assert_eq!(identified["kind"], "cisco_type6");
    assert_eq!(
        identified["reversible"], true,
        "type 6 is encryption, and it does come back"
    );
    assert_eq!(
        identified["decodable"], false,
        "but not here: the key is the device's master key"
    );

    for params in [
        json!({"text": line}),
        // Naming the encoding must not take a shortcut past the real reason.
        json!({"kind": "cisco6", "text": line}),
    ] {
        let refused = refuse("tools.secret.decode", params.clone()).await;
        assert_eq!(refused.code, "invalid");
        assert!(
            refused.message.contains("key the device holds"),
            "the refusal should name the reason: {}",
            refused.message
        );
        assert!(
            !refused.message.contains("a hash"),
            "type 6 is not a hash: {}",
            refused.message
        );
        assert_eq!(
            refused
                .details
                .as_ref()
                .and_then(|d| d["identified"]["kind"].as_str()),
            Some("cisco_type6"),
            "and carry what it actually is"
        );
    }
}

#[tokio::test]
async fn passwords_and_keys_are_generated_from_the_systems_randomness() {
    let generated = call(
        "tools.password",
        json!({"length": 18, "symbols": false, "count": 2}),
    )
    .await;
    let passwords = generated["passwords"].as_array().unwrap();
    assert_eq!(passwords.len(), 2);
    assert!(
        passwords
            .iter()
            .all(|word| word.as_str().unwrap().chars().count() == 18)
    );
    assert_ne!(passwords[0], passwords[1]);
    assert_eq!(generated["symbols"], "%+,-.=^_");

    let key = call("tools.keys.generate", json!({"comment": "ops@nocturne"})).await;
    assert!(
        key["private_key"]
            .as_str()
            .unwrap()
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"),
    );
    assert!(
        key["public_key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 "),
    );
    assert!(
        key["public_key"]
            .as_str()
            .unwrap()
            .ends_with("ops@nocturne")
    );
    assert_eq!(key["kind"], "ed25519");

    assert_eq!(
        refuse("tools.keys.generate", json!({"kind": "dsa"}))
            .await
            .code,
        "invalid"
    );
    assert_eq!(
        refuse("tools.keys.generate", json!({"comment": "two\nlines"}))
            .await
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_key_pair_comes_from_whoever_already_generates_them() {
    // Features talk to each other by name: with `keys.generate` present (the
    // terminal feature registers it), `tools.keys.generate` delegates, which
    // is how the fingerprint gets computed in one place only.
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.router.add(
        "keys.generate",
        |_app: nocturne_app::App, params: Value| async move {
            Ok(json!({
                "private_key": "from the terminal feature",
                "public_key": "ssh-ed25519 AAAA",
                "kind": params["kind"],
                "fingerprint": "SHA256:delegated",
            }))
        },
    );
    let app = builder.build();
    let delegated = app
        .rpc("tools.keys.generate", json!({"kind": "rsa3072"}))
        .await
        .unwrap();
    assert_eq!(delegated["fingerprint"], "SHA256:delegated");
    assert_eq!(delegated["kind"], "rsa3072");

    // Without it, the engine is called here and the fingerprint is `null`
    // rather than a made-up value.
    let alone = support::lab(&[]).await;
    let direct = alone
        .app
        .rpc("tools.keys.generate", json!({}))
        .await
        .unwrap();
    assert_eq!(direct["fingerprint"], Value::Null);
    assert!(
        direct["public_key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 ")
    );
}

#[tokio::test]
async fn units_sizes_rates_and_power_convert() {
    let size = call(
        "tools.convert",
        json!({"value": "4.7", "from": "GB", "to": "GiB"}),
    )
    .await;
    // 4.7 GB is 4_700_000_000 bytes, which is 4.377… GiB.
    assert_eq!(size["canonical"], 4_700_000_000.0);
    assert!((size["value"].as_f64().unwrap() - 4.3771).abs() < 0.001);
    assert_eq!(size["human"], "4.38 GiB");

    let rate = call("tools.convert", json!({"value": "100 Mbps", "to": "MB/s"})).await;
    assert_eq!(rate["value"], 12.5);

    let power = call(
        "tools.convert",
        json!({"value": "-3", "from": "dBm", "to": "mW"}),
    )
    .await;
    assert!((power["value"].as_f64().unwrap() - 0.5012).abs() < 0.001);

    let transfer = call(
        "tools.transfer_time",
        json!({"size": "700 MB", "rate": "100 Mbps"}),
    )
    .await;
    // 700 MB over 100 Mbps with no overhead is 56 seconds exactly.
    assert_eq!(transfer["seconds"], 56.0);
    assert!(transfer["human"].as_str().unwrap().contains("56"));

    let with_overhead = call(
        "tools.transfer_time",
        json!({"size": "700 MB", "rate": "100 Mbps", "overhead_pct": 5.0}),
    )
    .await;
    assert!(with_overhead["seconds"].as_f64().unwrap() > 58.0);

    let mtu = call(
        "tools.mtu",
        json!({"link_mtu": 1500, "encapsulations": ["gre", "dot1q"]}),
    )
    .await;
    // GRE is 24 bytes and a 802.1Q tag is 4: 1500 - 28 = 1472, MSS 1432.
    assert_eq!(mtu["payload_mtu"], 1472);
    assert_eq!(mtu["overhead"], 28);
    assert_eq!(mtu["tcp_mss_v4"], 1432);

    assert_eq!(
        refuse("tools.convert", json!({"value": "5", "to": "furlongs"}))
            .await
            .code,
        "invalid"
    );
    assert_eq!(
        refuse(
            "tools.transfer_time",
            json!({"size": "700 MB", "rate": "100 Mbps", "overhead_pct": 150.0})
        )
        .await
        .code,
        "invalid"
    );
    assert_eq!(
        refuse(
            "tools.mtu",
            json!({"link_mtu": 1500, "encapsulations": ["teleport"]})
        )
        .await
        .code,
        "invalid"
    );
}

#[tokio::test]
async fn the_reference_tables_are_served_and_searched() {
    let tables = call("tools.reference.tables", Value::Null).await;
    let ids: Vec<&str> = tables
        .as_array()
        .unwrap()
        .iter()
        .map(|table| table["table"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"ports") && ids.contains(&"dscp") && ids.contains(&"admin_distance"));
    assert!(tables[0]["rows"].as_u64().unwrap() > 0);

    let ef = call("tools.reference", json!({"table": "dscp", "query": "ef"})).await;
    assert_eq!(ef["rows"][0]["dscp"], 46);
    assert_eq!(ef["rows"][0]["name"], "EF");

    let icmp = call(
        "tools.reference",
        json!({"table": "icmp_types", "query": "8"}),
    )
    .await;
    assert!(
        icmp["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["number"] == 8 && row["name"].as_str().unwrap().contains("Echo")),
        "{}",
        icmp["rows"]
    );

    let distances = call("tools.reference", json!({"table": "admin_distance"})).await;
    assert!(distances["rows"].as_array().unwrap().iter().any(|row| {
        row["source"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("ospf")
    }),);

    let encapsulations = call("tools.reference", json!({"table": "encapsulations"})).await;
    assert!(
        encapsulations["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "vxlan"),
    );

    let refused = refuse("tools.reference", json!({"table": "cable_pinouts"})).await;
    assert_eq!(refused.code, "not_found");
}

#[tokio::test]
async fn the_text_tools_test_patterns_diff_and_encode() {
    let matched = call(
        "tools.regex",
        json!({
            "pattern": r"(?<name>\w+)Ethernet(?<port>[\d/]+)",
            "text": "GigabitEthernet1/0/1 is up",
        }),
    )
    .await;
    assert_eq!(matched["matches"][0]["text"], "GigabitEthernet1/0/1");
    assert_eq!(matched["matches"][0]["named"][0][0], "name");
    assert_eq!(matched["matches"][0]["named"][0][1], "Gigabit");
    assert_eq!(matched["error"], Value::Null);

    // A half-typed pattern is a result with an error in it, not a failed call:
    // this method is called on every keystroke.
    let broken = call("tools.regex", json!({"pattern": "(unclosed", "text": "x"})).await;
    assert!(
        broken["error"].as_str().unwrap().contains("unclosed"),
        "{broken}"
    );
    assert!(broken["matches"].as_array().unwrap().is_empty());

    let flags = call(
        "tools.regex",
        json!({"pattern": "^up$", "text": "down\nUP", "flags": "im"}),
    )
    .await;
    assert_eq!(flags["matches"].as_array().unwrap().len(), 1);

    let diff = call(
        "tools.diff",
        json!({"a": "hostname SW1\nntp server 10.0.0.1\n", "b": "hostname SW1\nntp server 10.0.0.2\n"}),
    )
    .await;
    assert_eq!((&diff["added"], &diff["removed"]), (&json!(1), &json!(1)));
    assert!(
        diff["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["tag"] == "+" && line["text"] == "ntp server 10.0.0.2"),
    );

    // A diff of two very large texts is refused rather than attempted.
    let huge = "x\n".repeat(3 * 1024 * 1024);
    let refused = refuse("tools.diff", json!({"a": huge, "b": "x\n"})).await;
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("MiB"), "{}", refused.message);

    // Hand-checked: the MD5 of "abc" is 900150983cd24fb0d6963f7d28e17f72.
    let hashes = call("tools.hash", json!({"text": "abc"})).await;
    assert_eq!(hashes["md5"], "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(hashes["sha1"], "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(hashes["bytes"], 3);
    assert_eq!(hashes["base64"], "YWJj");

    let encoded = call("tools.base64", json!({"mode": "encode", "text": "abc"})).await;
    assert_eq!(encoded["base64"], "YWJj");
    let decoded = call("tools.base64", json!({"mode": "decode", "text": "YWJj"})).await;
    assert_eq!(decoded["text"], "abc");
    // Bytes that are not text say so instead of coming back empty.
    let binary = call("tools.base64", json!({"mode": "decode", "text": "/w=="})).await;
    assert_eq!(binary["text"], Value::Null);
    assert!(binary["note"].as_str().unwrap().contains("not valid UTF-8"));

    assert_eq!(
        refuse("tools.base64", json!({"mode": "decode", "text": "!!!"}))
            .await
            .code,
        "invalid"
    );
    assert_eq!(
        refuse("tools.base64", json!({"mode": "sideways", "text": "x"}))
            .await
            .code,
        "invalid"
    );
    assert_eq!(refuse("tools.hash", json!({})).await.code, "invalid");
}
