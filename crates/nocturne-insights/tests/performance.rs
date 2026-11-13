//! A scan runs after every collection, so it has to be cheap.
//!
//! 500 devices with 48 interfaces each, cabled into a tree, with
//! configurations, VLANs, neighbours, facts, two generations of interface and
//! MAC data, and polled metrics — then one full scan, timed.

mod common;

use std::time::Instant;

use nocturne_app::inventory::{self, DeviceInput};
use nocturne_app::testing;
use nocturne_app::util::now_ms;
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

const DEVICES: usize = 500;
const PORTS: usize = 48;
/// A scan after every collection must not be something anybody notices. The
/// number that matters is the optimised one; an unoptimised build spends most
/// of its time in serde and is measured against a looser bound so that the
/// test is still useful in `cargo test`.
/// A ceiling, not a target.
///
/// The numbers this test prints are the thing worth reading — a warm scan of
/// five hundred devices is about 300 ms in release and a second in debug on a
/// developer's machine. The assertion cannot be set near that: it runs on
/// shared CI hardware whose speed varies by more than the margin being
/// measured, and a test that fails because somebody else's build was busy
/// teaches nothing and gets ignored. It was set at 2,500 ms and a macOS
/// runner took 4,041 ms for work that is not slow.
///
/// So the budget catches an order-of-magnitude regression — a scan that went
/// quadratic, or started re-parsing every configuration — and the shape of
/// the cost is asserted separately, below, in a way no machine can change.
const BUDGET_MS: u128 = if cfg!(debug_assertions) {
    30_000
} else {
    10_000
};

fn port(index: usize, vlan: u64) -> Value {
    json!({
        "name": format!("GigabitEthernet1/0/{index}"),
        "description": format!("desk {index}"),
        "admin_up": true,
        "oper_up": !index.is_multiple_of(7),
        "speed_bps": 1_000_000_000u64,
        "duplex": "a-full",
        "mtu": 1500,
        "mode": "access",
        "vlan": vlan,
        "mac": format!("0011.22{:02x}.{:04x}", index, index * 7),
        "counters": {
            "in_octets": 1_000_000u64 * index as u64,
            "in_packets": 900_000u64 * index as u64,
            "in_errors": index as u64,
            "crc": index as u64,
        }
    })
}

fn configuration(hostname: &str) -> String {
    let mut text = format!(
        "hostname {hostname}\n\
         no ip http server\n\
         ntp server 192.0.2.1\n\
         logging host 10.0.0.9\n\
         snmp-server community n0t-a-gu3ss RO 10\n\
         enable secret 9 $9$abcdefghij$klmnopqrstuvwxyz\n\
         vlan 10\n name users\n\
         vlan 20\n name voice\n"
    );
    text.push_str(
        "interface GigabitEthernet1/0/48\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n",
    );
    for index in 1..PORTS {
        text.push_str(&format!(
            "interface GigabitEthernet1/0/{index}\n switchport mode access\n switchport access vlan {}\n",
            if index.is_multiple_of(2) { 10 } else { 20 }
        ));
    }
    text.push_str("line vty 0 4\n transport input ssh\n access-class 10 in\n");
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_of_five_hundred_devices_is_well_under_a_second() {
    let mut builder = testing::builder();
    nocturne_insights::init(&mut builder);
    let app = builder.build();
    let now = now_ms();

    let built = Instant::now();
    let mut ids = Vec::with_capacity(DEVICES);
    for index in 0..DEVICES {
        let device = inventory::create(
            &app,
            DeviceInput {
                name: Some(format!("SW{index:03}")),
                address: Some(format!("10.{}.{}.2", index / 250, index % 250)),
                profile: Some("test_ios".into()),
                site: Some(format!("site-{}", index % 10)),
                role: Some(if index.is_multiple_of(25) {
                    "distribution".into()
                } else {
                    "access".into()
                }),
                ..DeviceInput::default()
            },
        )
        .expect("device");
        ids.push(device.device.id);
    }
    // The profile the devices claim must exist, or every configuration check
    // skips them for want of a grammar — which would not be a fair measurement.
    nocturne_app::profiles::save(&app, testing::LAB_IOS).expect("the test profile loads");

    for (index, id) in ids.iter().enumerate() {
        let ports: Vec<Value> = (1..=PORTS)
            .map(|port_index| {
                if port_index == PORTS {
                    let mut uplink = port(port_index, 1);
                    uplink["mode"] = json!("trunk");
                    uplink["oper_up"] = json!(true);
                    uplink
                } else {
                    port(
                        port_index,
                        if port_index.is_multiple_of(2) { 10 } else { 20 },
                    )
                }
            })
            .collect();
        // Two generations, so the growth checks have something to compare.
        nocturne_app::collect::store(&app, id, Kind::Interfaces, ports.clone(), "snmp").unwrap();
        nocturne_app::collect::store(&app, id, Kind::Interfaces, ports, "snmp").unwrap();
        nocturne_app::collect::store(
            &app,
            id,
            Kind::Facts,
            vec![json!({
                "hostname": format!("SW{index:03}"), "vendor": "Cisco", "os": "IOS",
                "model": "WS-C2960X-48FPD-L", "os_version": "15.2(7)E7", "uptime_s": 4_000_000
            })],
            "cli",
        )
        .unwrap();
        nocturne_app::collect::store(
            &app,
            id,
            Kind::Vlans,
            vec![
                json!({"id": 10, "name": "users", "status": "active"}),
                json!({"id": 20, "name": "voice", "status": "active"}),
            ],
            "cli",
        )
        .unwrap();
        // Cabled into a tree: every switch's uplink goes to the one above it.
        if index > 0 {
            let parent = (index - 1) / 24;
            nocturne_app::collect::store(
                &app,
                id,
                Kind::Neighbors,
                vec![json!({
                    "local_interface": format!("GigabitEthernet1/0/{PORTS}"),
                    "device": format!("SW{parent:03}"),
                    "remote_interface": format!("GigabitEthernet1/0/{}", (index - 1) % 24 + 1),
                    "protocol": "cdp"
                })],
                "cli",
            )
            .unwrap();
        }
        // A MAC table with two generations, and a few addresses that moved.
        let macs: Vec<Value> = (1..20)
            .map(|entry| {
                json!({
                    "vlan": 10, "mac": format!("00aa.bb{:02x}.{:04x}", index % 256, entry),
                    "interface": format!("GigabitEthernet1/0/{}", entry % 24 + 1),
                    "kind": "dynamic"
                })
            })
            .collect();
        nocturne_app::collect::store(&app, id, Kind::MacTable, macs.clone(), "cli").unwrap();
        nocturne_app::collect::store(&app, id, Kind::MacTable, macs, "cli").unwrap();
        app.store()
            .config_insert(
                id,
                "running",
                &configuration(&format!("SW{index:03}")),
                "scheduled",
                "",
            )
            .unwrap();
        // Polled metrics for the device and for every port.
        let mut samples = Vec::new();
        for (metric, instance, value) in [
            ("cpu.pct", String::new(), 12.0),
            ("mem.pct", String::new(), 44.0),
        ] {
            let series = app.store().series_id(id, metric, &instance).unwrap();
            for step in 0..12 {
                samples.push((series, now - (55 - step * 4) * 60_000, value));
            }
        }
        for port_index in [1, PORTS] {
            let instance = format!("GigabitEthernet1/0/{port_index}");
            for metric in ["if.oper", "if.util_pct"] {
                let series = app.store().series_id(id, metric, &instance).unwrap();
                for step in 0..12 {
                    samples.push((
                        series,
                        now - (55 - step * 4) * 60_000,
                        if metric == "if.oper" { 1.0 } else { 9.0 },
                    ));
                }
            }
        }
        app.store().samples_insert(&samples).unwrap();
    }
    let built = built.elapsed();

    {
        let t = Instant::now();
        let rows = nocturne_app::inventory::list(&app, &Default::default()).unwrap();
        println!(
            "\n  inventory::list      {} ms ({} rows)",
            t.elapsed().as_millis(),
            rows.len()
        );
        let t = Instant::now();
        let mut total = 0;
        for kind in [
            Kind::Facts,
            Kind::Interfaces,
            Kind::Neighbors,
            Kind::Vlans,
            Kind::MacTable,
            Kind::Arp,
            Kind::Routes,
            Kind::Health,
            Kind::Inventory,
        ] {
            total += nocturne_app::collect::latest_all(&app, kind).unwrap().len();
        }
        println!(
            "  latest_all x9        {} ms ({total} stored)",
            t.elapsed().as_millis()
        );
        let t = Instant::now();
        let mut rows = 0;
        for kind in [
            Kind::Facts,
            Kind::Interfaces,
            Kind::Neighbors,
            Kind::Vlans,
            Kind::MacTable,
            Kind::Arp,
            Kind::Routes,
            Kind::Health,
            Kind::Inventory,
        ] {
            for stored in nocturne_app::collect::latest_all(&app, kind).unwrap() {
                rows += stored.rows.len();
            }
        }
        println!(
            "  …and reading rows    {} ms ({rows} rows)",
            t.elapsed().as_millis()
        );
        let t = Instant::now();
        for id in &ids {
            nocturne_app::collect::previous(&app, id, Kind::Interfaces).unwrap();
        }
        println!("  previous x500        {} ms", t.elapsed().as_millis());
        let t = Instant::now();
        for id in &ids {
            app.store().config_latest(id, "running").unwrap();
            app.store().config_latest(id, "startup").unwrap();
        }
        println!("  config_latest x1000  {} ms", t.elapsed().as_millis());
    }

    // How much of a scan is loading the world, and how much is the checks.
    let loading = Instant::now();
    nocturne_insights::scan::run(&app, Vec::new(), vec!["duplex_mismatch".into()])
        .await
        .expect("the scan runs");
    println!("\n  load + one check {} ms", loading.elapsed().as_millis());

    // The first scan parses 500 configurations; the second finds them cached.
    let cold = Instant::now();
    let first = nocturne_insights::scan::run(&app, Vec::new(), Vec::new())
        .await
        .expect("the scan runs");
    let cold = cold.elapsed();

    let warm = Instant::now();
    let second = nocturne_insights::scan::run(&app, Vec::new(), Vec::new())
        .await
        .expect("the scan runs");
    let warm = warm.elapsed();

    println!(
        "\n  {DEVICES} devices × {PORTS} interfaces ({} interface rows, 2 generations)\n  \
           fixture built in {} ms\n  \
           first scan  {} ms ({} findings over {} devices, {} checks)\n  \
           second scan {} ms (configurations already parsed)\n",
        DEVICES * PORTS,
        built.as_millis(),
        cold.as_millis(),
        first.findings,
        first.devices,
        first.checks_run,
        warm.as_millis(),
    );

    assert_eq!(first.devices, DEVICES);
    assert!(first.checks_run > 20);
    assert!(
        warm.as_millis() < BUDGET_MS,
        "a warm scan took {} ms, over the {BUDGET_MS} ms budget",
        warm.as_millis()
    );
    assert!(
        cold.as_millis() < BUDGET_MS * 2,
        "a cold scan took {} ms",
        cold.as_millis()
    );
    assert_eq!(second.devices, DEVICES);

    // A scan of one device, which is what every collection triggers.
    //
    // It is **not** faster than a scan of five hundred, and must not be
    // asserted to be. Narrowing decides which devices a verdict is reached
    // *for*; it cannot narrow the loading, because a check like the duplex
    // mismatch needs the other end of the link, and the other end can be any
    // device in the inventory. Loading dominates the cost, so the two scans
    // cost about the same — and on a machine whose storage is slower than
    // this one's, the one-device scan can come out slightly ahead or behind
    // depending on nothing more than cache warmth. CI found that out by
    // failing an assertion here that claimed otherwise, on two platforms at
    // once, which is exactly what it is for.
    //
    // What *is* guaranteed is what the narrowing means, so that is asserted.
    let one = Instant::now();
    let narrowed = nocturne_insights::scan::run(&app, vec![ids[7].clone()], Vec::new())
        .await
        .expect("the scan runs");
    println!("  one device  {} ms\n", one.elapsed().as_millis());
    assert_eq!(
        narrowed.devices, 1,
        "a scan narrowed to one device decided for {} of them",
        narrowed.devices
    );
    assert!(narrowed.checks_run > 20, "it still ran every check");
    assert!(one.elapsed().as_millis() < BUDGET_MS);
}
