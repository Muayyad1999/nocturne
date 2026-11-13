//! A small campus with known faults planted in it, and look-alikes planted
//! beside them, scanned once — then precision and recall per check.
//!
//! This is the measurement that decides whether the feature is worth having.
//! Every finding the scan produces is either one of the faults listed in
//! `expected` or a false positive; every fault that is missing is a false
//! negative. The look-alikes are the point: each one is a real thing a real
//! network does that an over-eager check would report.

mod common;

use std::collections::BTreeSet;

use common::{With, collected, config, lab, scan};
use nocturne_app::App;
use nocturne_app::testing::Lab;
use nocturne_app::util::now_ms;
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

/// A device's configuration: the lines every well-kept device here has, plus
/// whatever this one adds.
fn configuration(hostname: &str, vty: &str, extra: &str) -> String {
    format!(
        "hostname {hostname}\n\
         no ip http server\n\
         ntp server 192.0.2.1\n\
         logging host 10.0.0.9\n\
         snmp-server community n0t-a-gu3ss RO 10\n\
         enable secret 9 $9$abcdefghij$klmnopqrstuvwxyz\n\
         {extra}\
         line vty 0 4\n transport input {vty}\n access-class 10 in\n"
    )
}

fn interface(name: &str, speed: u64, duplex: &str) -> Value {
    json!({
        "name": name, "admin_up": true, "oper_up": true,
        "speed_bps": speed, "duplex": duplex, "mtu": 1500
    })
}

fn trunk(name: &str, speed: u64, duplex: &str, native: u64) -> Value {
    let mut port = interface(name, speed, duplex);
    port["mode"] = json!("trunk");
    port["vlan"] = json!(native);
    port
}

fn access(name: &str, speed: u64, duplex: &str, vlan: u64) -> Value {
    let mut port = interface(name, speed, duplex);
    port["mode"] = json!("access");
    port["vlan"] = json!(vlan);
    port
}

fn addressed(name: &str, cidr: &str) -> Value {
    json!({"name": name, "admin_up": true, "oper_up": true, "ipv4": [cidr]})
}

fn neighbour(local: &str, device: &str, remote: &str) -> Value {
    json!({
        "local_interface": local, "device": device,
        "remote_interface": remote, "protocol": "cdp"
    })
}

fn vlans() -> Vec<Value> {
    vec![
        json!({"id": 10, "name": "users", "status": "active"}),
        json!({"id": 20, "name": "voice", "status": "active"}),
    ]
}

fn facts(model: &str, version: &str, uptime_s: u64) -> Vec<Value> {
    vec![json!({
        "hostname": "", "vendor": "Cisco", "os": "IOS",
        "model": model, "os_version": version, "uptime_s": uptime_s
    })]
}

const ACCESS_MODEL: &str = "WS-C2960X-48FPD-L";
const DIST_MODEL: &str = "WS-C3850-24T";
const CURRENT: &str = "15.2(7)E7";

/// Build the campus. Returns the lab and the faults that were planted.
async fn campus() -> (Lab, BTreeSet<(String, String, String)>) {
    let lab = lab(
        &["CORE1", "DIST1", "DIST2", "ACC1", "ACC2", "ACC3", "ACC4"],
        With {
            changes: true,
            alerts: true,
            topology: false,
        },
    )
    .await;
    let app = &lab.app;
    let id = |name: &str| lab.id(name);

    // ---------------------------------------------------------------- CORE1
    collected(
        app,
        &id("CORE1"),
        Kind::Interfaces,
        vec![
            interface("GigabitEthernet1/0/1", 1_000_000_000, "full"),
            // Look-alike: an anycast loopback is *meant* to be in two places.
            addressed("Loopback0", "10.255.0.1/32"),
        ],
    );
    collected(
        app,
        &id("CORE1"),
        Kind::Neighbors,
        vec![neighbour(
            "GigabitEthernet1/0/1",
            "DIST1",
            "GigabitEthernet1/0/9",
        )],
    );
    collected(
        app,
        &id("CORE1"),
        Kind::Routes,
        vec![
            json!({"prefix": "0.0.0.0/0", "protocol": "static", "next_hop": "198.51.100.1"}),
            json!({"prefix": "10.0.0.0/8", "protocol": "ospf"}),
        ],
    );
    collected(
        app,
        &id("CORE1"),
        Kind::Facts,
        facts("C9500-24Y4C", "17.9.4a", 5_000_000),
    );
    config(
        app.store(),
        &id("CORE1"),
        "running",
        &configuration("CORE1", "ssh", ""),
    );

    // ---------------------------------------------------------------- DIST1
    collected(
        app,
        &id("DIST1"),
        Kind::Interfaces,
        vec![
            trunk("GigabitEthernet1/0/1", 100_000_000, "full", 1),
            trunk("GigabitEthernet1/0/2", 1_000_000_000, "full", 1),
            interface("GigabitEthernet1/0/9", 1_000_000_000, "full"),
            // Look-alike: the HSRP virtual address, which both distribution
            // switches answer for on purpose.
            addressed("Vlan10", "10.0.10.1/24"),
            addressed("Loopback0", "10.255.0.1/32"),
        ],
    );
    collected(
        app,
        &id("DIST1"),
        Kind::Neighbors,
        vec![
            neighbour("GigabitEthernet1/0/1", "ACC1", "GigabitEthernet0/1"),
            neighbour("GigabitEthernet1/0/2", "ACC2", "GigabitEthernet0/1"),
            neighbour("GigabitEthernet1/0/9", "CORE1", "GigabitEthernet1/0/1"),
        ],
    );
    collected(app, &id("DIST1"), Kind::Vlans, vlans());
    // Planted: a distribution switch with no default route.
    collected(
        app,
        &id("DIST1"),
        Kind::Routes,
        vec![
            json!({"prefix": "10.0.10.0/24", "protocol": "connected"}),
            json!({"prefix": "10.0.0.0/8", "protocol": "ospf"}),
        ],
    );
    collected(
        app,
        &id("DIST1"),
        Kind::Facts,
        facts(DIST_MODEL, "16.12.9", 3_000_000),
    );
    config(
        app.store(),
        &id("DIST1"),
        "running",
        &configuration(
            "DIST1",
            "ssh",
            "ip routing\n\
             interface GigabitEthernet1/0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n\
             interface GigabitEthernet1/0/2\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n\
             interface Vlan10\n ip address 10.0.10.2 255.255.255.0\n standby 10 ip 10.0.10.1\n",
        ),
    );

    // ---------------------------------------------------------------- DIST2
    collected(
        app,
        &id("DIST2"),
        Kind::Interfaces,
        vec![
            trunk("GigabitEthernet1/0/1", 1_000_000_000, "full", 1),
            trunk("GigabitEthernet1/0/2", 1_000_000_000, "full", 1),
            addressed("Vlan10", "10.0.10.1/24"),
        ],
    );
    collected(
        app,
        &id("DIST2"),
        Kind::Neighbors,
        vec![
            neighbour("GigabitEthernet1/0/1", "ACC3", "GigabitEthernet0/1"),
            neighbour("GigabitEthernet1/0/2", "ACC4", "GigabitEthernet0/1"),
        ],
    );
    collected(app, &id("DIST2"), Kind::Vlans, vlans());
    collected(
        app,
        &id("DIST2"),
        Kind::Facts,
        facts(DIST_MODEL, "16.12.9", 3_000_000),
    );
    config(
        app.store(),
        &id("DIST2"),
        "running",
        &configuration(
            "DIST2",
            "ssh",
            "ip routing\n\
             interface GigabitEthernet1/0/1\n switchport mode trunk\n\
             interface GigabitEthernet1/0/2\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n switchport trunk native vlan 1\n\
             interface Vlan10\n ip address 10.0.10.3 255.255.255.0\n standby 10 ip 10.0.10.1\n",
        ),
    );

    // ----------------------------------------------------------------- ACC1
    // Planted: half duplex against DIST1's full duplex at 100 Mb/s.
    collected(
        app,
        &id("ACC1"),
        Kind::Interfaces,
        vec![
            trunk("GigabitEthernet0/1", 100_000_000, "half", 1),
            access("GigabitEthernet0/3", 1_000_000_000, "full", 10),
            access("GigabitEthernet0/4", 1_000_000_000, "full", 10),
        ],
    );
    collected(
        app,
        &id("ACC1"),
        Kind::Neighbors,
        vec![neighbour(
            "GigabitEthernet0/1",
            "DIST1",
            "GigabitEthernet1/0/1",
        )],
    );
    collected(app, &id("ACC1"), Kind::Vlans, vlans());
    collected(
        app,
        &id("ACC1"),
        Kind::Facts,
        facts(ACCESS_MODEL, CURRENT, 900_000),
    );
    // Look-alike: one address moving between two ports is a laptop at
    // another desk, not a loop.
    collected(
        app,
        &id("ACC1"),
        Kind::MacTable,
        vec![
            json!({"vlan": 10, "mac": "0011.2233.4401", "interface": "GigabitEthernet0/3", "kind": "dynamic"}),
        ],
    );
    collected(
        app,
        &id("ACC1"),
        Kind::MacTable,
        vec![
            json!({"vlan": 10, "mac": "0011.2233.4401", "interface": "GigabitEthernet0/4", "kind": "dynamic"}),
        ],
    );
    let acc1_trunk = "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n";
    // Planted: the running configuration was never saved.
    config(
        app.store(),
        &id("ACC1"),
        "startup",
        &configuration("ACC1", "ssh", acc1_trunk),
    );
    config(
        app.store(),
        &id("ACC1"),
        "running",
        &configuration(
            "ACC1",
            "ssh",
            &format!("{acc1_trunk}ntp server 192.0.2.250\n"),
        ),
    );

    // ----------------------------------------------------------------- ACC2
    // Planted: a native VLAN of 10 against DIST1's (unconfigured) 1.
    collected(
        app,
        &id("ACC2"),
        Kind::Interfaces,
        vec![
            trunk("GigabitEthernet0/1", 1_000_000_000, "full", 10),
            access("GigabitEthernet0/5", 1_000_000_000, "full", 10),
        ],
    );
    collected(
        app,
        &id("ACC2"),
        Kind::Neighbors,
        vec![neighbour(
            "GigabitEthernet0/1",
            "DIST1",
            "GigabitEthernet1/0/2",
        )],
    );
    collected(app, &id("ACC2"), Kind::Vlans, vlans());
    collected(
        app,
        &id("ACC2"),
        Kind::Facts,
        facts(ACCESS_MODEL, CURRENT, 900_000),
    );
    // Planted: Gi0/5 flapped five times in the last hour.
    let series = app
        .store()
        .series_id(&id("ACC2"), "if.oper", "GigabitEthernet0/5")
        .expect("series");
    let now = now_ms();
    let samples: Vec<(i64, i64, f64)> = (0..6)
        .map(|index| (series, now - (55 - index * 9) * 60_000, (index % 2) as f64))
        .collect();
    app.store().samples_insert(&samples).expect("samples");
    let acc2_trunk = "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n switchport trunk native vlan 10\n";
    // Look-alike: running and startup differ only by a volatile line.
    config(
        app.store(),
        &id("ACC2"),
        "startup",
        &configuration("ACC2", "telnet ssh", acc2_trunk),
    );
    config(
        app.store(),
        &id("ACC2"),
        "running",
        &configuration(
            "ACC2",
            "telnet ssh",
            &format!("{acc2_trunk}ntp clock-period 17179860\n"),
        ),
    );

    // ----------------------------------------------------------------- ACC3
    collected(
        app,
        &id("ACC3"),
        Kind::Interfaces,
        vec![
            trunk("GigabitEthernet0/1", 1_000_000_000, "full", 1),
            // Look-alike: input errors since boot that are not growing.
            {
                let mut port = access("GigabitEthernet0/2", 1_000_000_000, "full", 10);
                port["counters"] =
                    json!({"in_errors": 51_234, "crc": 51_234, "in_packets": 900_000_000u64});
                port
            },
            // Look-alike: a 10 Mb/s half-duplex link where both ends agree.
            access("FastEthernet0/1", 10_000_000, "half", 10),
            // Planted: one address on two switches.
            addressed("Vlan1", "10.0.0.5/24"),
        ],
    );
    // A second generation with the same counters: nothing is growing.
    collected(
        app,
        &id("ACC3"),
        Kind::Interfaces,
        vec![
            trunk("GigabitEthernet0/1", 1_000_000_000, "full", 1),
            {
                let mut port = access("GigabitEthernet0/2", 1_000_000_000, "full", 10);
                port["counters"] =
                    json!({"in_errors": 51_234, "crc": 51_234, "in_packets": 1_900_000_000u64});
                port
            },
            access("FastEthernet0/1", 10_000_000, "half", 10),
            addressed("Vlan1", "10.0.0.5/24"),
        ],
    );
    collected(
        app,
        &id("ACC3"),
        Kind::Neighbors,
        vec![
            neighbour("GigabitEthernet0/1", "DIST2", "GigabitEthernet1/0/1"),
            neighbour("FastEthernet0/1", "ACC4", "FastEthernet0/1"),
        ],
    );
    collected(app, &id("ACC3"), Kind::Vlans, vlans());
    collected(
        app,
        &id("ACC3"),
        Kind::Facts,
        facts(ACCESS_MODEL, CURRENT, 900_000),
    );
    config(
        app.store(),
        &id("ACC3"),
        "running",
        &configuration(
            "ACC3",
            "ssh",
            "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n",
        ),
    );

    // ----------------------------------------------------------------- ACC4
    collected(
        app,
        &id("ACC4"),
        Kind::Interfaces,
        vec![
            // Look-alike: a trunk with no native VLAN line against DIST2's
            // explicit `native vlan 1` — the same VLAN, written twice over.
            trunk("GigabitEthernet0/1", 1_000_000_000, "full", 1),
            access("FastEthernet0/1", 10_000_000, "half", 10),
            addressed("Vlan1", "10.0.0.5/24"),
        ],
    );
    collected(
        app,
        &id("ACC4"),
        Kind::Neighbors,
        vec![
            neighbour("GigabitEthernet0/1", "DIST2", "GigabitEthernet1/0/2"),
            neighbour("FastEthernet0/1", "ACC3", "FastEthernet0/1"),
        ],
    );
    collected(app, &id("ACC4"), Kind::Vlans, vlans());
    // Look-alike: the uptime counter wrapped at 497 days.
    collected(
        app,
        &id("ACC4"),
        Kind::Facts,
        facts(ACCESS_MODEL, "15.0(2)SE11", 497 * 86_400),
    );
    // Planted: the odd one out of four identical switches.
    collected(
        app,
        &id("ACC4"),
        Kind::Facts,
        facts(ACCESS_MODEL, "15.0(2)SE11", 300),
    );
    config(
        app.store(),
        &id("ACC4"),
        "running",
        &configuration(
            "ACC4",
            "ssh",
            "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n",
        ),
    );

    // The faults that were planted. `duplicate_ip` and `native_vlan_mismatch`
    // are about two devices, and the crate decides which end owns the finding
    // — deterministically, so the expectation can say which.
    let duplicate_owner = if id("ACC3") <= id("ACC4") {
        "ACC3"
    } else {
        "ACC4"
    };
    let expected: BTreeSet<(String, String, String)> = [
        ("duplex_mismatch", "ACC1", "GigabitEthernet0/1"),
        // DIST1's end is the one nobody configured, so the fix belongs there.
        ("native_vlan_mismatch", "DIST1", "GigabitEthernet1/0/2"),
        ("duplicate_ip", duplicate_owner, "10.0.0.5"),
        ("unsaved_config", "ACC1", "running-config"),
        ("interface_flapping", "ACC2", "GigabitEthernet0/5"),
        ("no_default_route", "DIST1", "0.0.0.0/0"),
        ("telnet_enabled", "ACC2", "line vty"),
        ("software_inconsistent", "ACC4", "15.0(2)SE11"),
    ]
    .into_iter()
    .map(|(check, device, subject)| (check.to_string(), device.to_string(), subject.to_string()))
    .collect();
    (lab, expected)
}

async fn found(app: &App) -> BTreeSet<(String, String, String)> {
    let listed = app
        .rpc("insights.list", json!({"limit": 500, "include_lab": true}))
        .await
        .expect("list");
    listed["insights"]
        .as_array()
        .expect("insights")
        .iter()
        .map(|insight| {
            (
                insight["check"].as_str().unwrap_or_default().to_string(),
                insight["device"].as_str().unwrap_or_default().to_string(),
                insight["subject"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn precision_and_recall_over_a_planted_network() {
    let (lab, expected) = campus().await;
    let app = &lab.app;
    let result = scan(app, json!({})).await;
    let found = found(app).await;

    let checks: BTreeSet<String> = expected
        .iter()
        .map(|(check, ..)| check.clone())
        .chain(found.iter().map(|(check, ..)| check.clone()))
        .collect();

    println!("\n  check                              TP  FP  FN  precision  recall");
    println!("  ------------------------------------------------------------------");
    let (mut total_tp, mut total_fp, mut total_fn) = (0, 0, 0);
    for check in &checks {
        let of = |set: &BTreeSet<(String, String, String)>| -> BTreeSet<(String, String)> {
            set.iter()
                .filter(|(name, ..)| name == check)
                .map(|(_, device, subject)| (device.clone(), subject.clone()))
                .collect()
        };
        let (wanted, got) = (of(&expected), of(&found));
        let tp = wanted.intersection(&got).count();
        let false_positives: Vec<_> = got.difference(&wanted).collect();
        let false_negatives: Vec<_> = wanted.difference(&got).collect();
        total_tp += tp;
        total_fp += false_positives.len();
        total_fn += false_negatives.len();
        let ratio = |hit: usize, total: usize| -> String {
            if total == 0 {
                "     n/a".to_string()
            } else {
                format!("{:8.2}", hit as f64 / total as f64)
            }
        };
        println!(
            "  {:32} {:3} {:3} {:3} {}  {}",
            check,
            tp,
            false_positives.len(),
            false_negatives.len(),
            ratio(tp, tp + false_positives.len()),
            ratio(tp, tp + false_negatives.len()),
        );
        for extra in &false_positives {
            println!("      false positive: {} {}", extra.0, extra.1);
        }
        for missing in &false_negatives {
            println!("      missed:         {} {}", missing.0, missing.1);
        }
    }
    println!(
        "  ------------------------------------------------------------------\n  \
         total                            {total_tp:3} {total_fp:3} {total_fn:3} {:8.2}  {:8.2}\n",
        total_tp as f64 / (total_tp + total_fp).max(1) as f64,
        total_tp as f64 / (total_tp + total_fn).max(1) as f64,
    );
    println!(
        "  7 devices, {} links, {} findings, scan {} ms\n",
        app.rpc("insights.coverage", json!({})).await.unwrap()["links"],
        result["findings"],
        result["took_ms"]
    );

    assert_eq!(total_fp, 0, "a false positive is worse than a missed fault");
    assert_eq!(total_fn, 0, "every planted fault must be found");
}

/// The look-alikes, named one by one, so that a future change that starts
/// reporting one of them fails here and says which.
#[tokio::test]
async fn the_look_alikes_stay_quiet() {
    let (lab, _) = campus().await;
    let app = &lab.app;
    scan(app, json!({})).await;
    let found = found(app).await;
    let open = |check: &str| -> Vec<String> {
        found
            .iter()
            .filter(|(name, ..)| name == check)
            .map(|(_, device, subject)| format!("{device} {subject}"))
            .collect()
    };

    for (check, what) in [
        (
            "trunk_vlan_mismatch",
            "DIST2 allows every VLAN on its trunk to ACC3, which prunes: normal",
        ),
        (
            // Here the two collections are milliseconds apart, so the check
            // declines to read a rate from them at all; the growing and
            // not-growing cases are measured in the unit tests, which can
            // place the two collections an hour apart.
            "interface_errors",
            "ACC3 Gi0/2 has 51,234 errors since boot that are not growing",
        ),
        (
            "uptime_reset",
            "ACC4's uptime counter wrapped at 497 days; it did not reboot",
        ),
        (
            "mac_flapping",
            "one address moved between two of ACC1's ports: a laptop, not a loop",
        ),
        (
            "half_duplex_uplink",
            "ACC3 and ACC4 are joined by a 10 Mb/s half-duplex link, as 10BASE-T is",
        ),
        (
            "speed_mismatch",
            "every link has matching speeds at both ends",
        ),
        ("down_uplink", "no port is up with the link down"),
        ("subnet_mask_mismatch", "no link has addresses on both ends"),
        (
            "overlapping_subnets",
            "the anycast loopback does not overlap anything",
        ),
        (
            "vlan_not_on_trunk",
            "every access VLAN is on its switch's uplink",
        ),
        ("unused_vlan", "both VLANs are in use or carried"),
        (
            "vlan_name_inconsistent",
            "every switch names VLAN 10 the same",
        ),
        ("no_recent_backup", "every device was backed up just now"),
        (
            "http_server_enabled",
            "every device has `no ip http server`",
        ),
        (
            "snmp_default_community",
            "no device uses `public` or `private`",
        ),
        ("weak_enable_password", "every device has a type 9 secret"),
        ("no_vty_acl", "every device restricts its VTY lines"),
        ("ntp_not_configured", "every device has an NTP server"),
        ("no_syslog_host", "every device logs to the collector"),
        (
            "config_changed_outside_nocturne",
            "one stored version each: nothing can be seen to have changed",
        ),
    ] {
        assert!(
            open(check).is_empty(),
            "{check} fired: {what} — {:?}",
            open(check)
        );
    }

    // And the two cross-device look-alikes, which are the ones that would
    // make this feature unusable on a real network.
    let duplicates = open("duplicate_ip");
    assert_eq!(
        duplicates.len(),
        1,
        "only the planted duplicate: HSRP's 10.0.10.1 and the anycast loopback \
         10.255.0.1 are not duplicates — {duplicates:?}"
    );
    assert!(duplicates[0].contains("10.0.0.5"));

    let natives = open("native_vlan_mismatch");
    assert_eq!(
        natives.len(),
        1,
        "only the planted mismatch: ACC4's unconfigured native VLAN against \
         DIST2's explicit `native vlan 1` is the same VLAN — {natives:?}"
    );
}

/// What the coverage report says about this network: which checks reached a
/// verdict, and which could not and why.
#[tokio::test]
async fn coverage_accounts_for_every_device_of_every_check() {
    let (lab, _) = campus().await;
    let app = &lab.app;
    scan(app, json!({})).await;
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    assert_eq!(coverage["devices"], 7);

    println!("\n  check                            evaluated skipped findings  reason");
    for check in coverage["checks"].as_array().unwrap() {
        let evaluated = check["evaluated"].as_u64().unwrap();
        let skipped = check["skipped"].as_u64().unwrap();
        assert_eq!(
            evaluated + skipped,
            7,
            "{} accounts for {} of 7 devices",
            check["check"],
            evaluated + skipped
        );
        println!(
            "  {:32} {:9} {:7} {:8}  {}",
            check["check"].as_str().unwrap(),
            evaluated,
            skipped,
            check["findings"],
            check["reasons"][0]["reason"].as_str().unwrap_or("—"),
        );
    }
    println!();
}
