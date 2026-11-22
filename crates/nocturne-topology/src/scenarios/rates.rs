//! Link load: the rates monitoring measured, joined to the ports links end
//! on. The monitor feature is a separate crate, so these tests register a
//! `monitor.interfaces` that keeps the same contract the real one documents
//! (`docs/dev/features/monitor.md`): a bare array, filtered by `device_id`,
//! ordered by device and then by name, at most `limit` rows — 200 unless
//! asked, never more than 5 000 — starting `cursor` rows in.

use std::sync::{Arc, Mutex};

use nocturne_app::error::AppError;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppBuilder};
use nocturne_drivers::model::Kind;
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;

/// What a test has "polled": the rows the stand-in monitor answers with.
type Polled = Arc<Mutex<Vec<Value>>>;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct InterfaceQuery {
    device_id: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

fn with_monitor(builder: &mut AppBuilder) -> Polled {
    let polled: Polled = Arc::default();
    let rows = polled.clone();
    builder.router.add(
        "monitor.interfaces",
        move |_: App, query: InterfaceQuery| {
            let rows = rows.clone();
            async move {
                let text = |row: &Value, field: &str| {
                    row[field].as_str().unwrap_or_default().to_ascii_lowercase()
                };
                let mut matching: Vec<Value> = rows
                    .lock()
                    .expect("the rows")
                    .iter()
                    .filter(|row| {
                        query
                            .device_id
                            .as_deref()
                            .is_none_or(|id| row["device_id"] == json!(id))
                    })
                    .cloned()
                    .collect();
                matching.sort_by_key(|row| (text(row, "device"), text(row, "name")));
                let offset = match query.cursor.as_deref() {
                    Some(cursor) if !cursor.is_empty() => {
                        cursor.parse::<usize>().map_err(|_| {
                            AppError::invalid("the cursor is the number of rows already received")
                        })?
                    }
                    _ => 0,
                };
                let limit = query.limit.unwrap_or(200).clamp(1, 5_000) as usize;
                Ok(matching
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .collect::<Vec<Value>>())
            }
        },
    );
    polled
}

/// One monitored port: ten gigabits, with the rates given, measured at `at`.
fn polled_port(device_id: &str, device: &str, name: &str, in_bps: f64, out_bps: f64) -> Value {
    json!({
        "device_id": device_id,
        "device": device,
        "name": name,
        "speed_bps": 10_000_000_000u64,
        "in_bps": in_bps,
        "out_bps": out_bps,
        "util_pct": in_bps.max(out_bps) / 100_000_000.0,
        "at": 1_700_000_000_000i64,
    })
}

/// Two devices that both report the cable between `port` and `port`, and
/// both have that port in their collected interface table.
fn cable(app: &App, one: (&str, &str), other: (&str, &str), port: &str) {
    for ((near, _), (_, far_name)) in [(one, other), (other, one)] {
        store(
            app,
            near,
            vec![
                (
                    Kind::Neighbors,
                    vec![neighbor(port, far_name, port, "lldp")],
                ),
                (Kind::Interfaces, vec![trunk(port, 10_000_000_000)]),
            ],
        );
    }
}

async fn call(app: &App, method: &str, params: Value) -> Value {
    app.rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method} failed: {err}"))
}

/// The `{device_id, interface}` of both ends of the only link in the graph,
/// exactly as `topology.get` spells them — which is what the map asks with.
async fn link_ends(app: &App) -> Vec<Value> {
    let graph = call(app, "topology.get", Value::Null).await;
    let links = graph["links"].as_array().expect("links");
    assert_eq!(links.len(), 1, "{links:#?}");
    ["a", "b"]
        .into_iter()
        .map(|end| {
            json!({
                "device_id": links[0][end]["node"],
                "interface": links[0][end]["interface"],
            })
        })
        .collect()
}

fn port_of<'rates>(rates: &'rates Value, device_id: &str) -> Option<&'rates Value> {
    rates["ports"]
        .as_array()
        .expect("ports")
        .iter()
        .find(|port| port["device_id"] == json!(device_id))
}

#[tokio::test]
async fn a_port_is_found_however_far_down_monitorings_table_its_device_sorts() {
    // The finding's network: 150 access switches with about sixty monitored
    // ports each, nine thousand rows, of which one call returns the first
    // five thousand — sw-001 to sw-083. The link is between sw-140 and sw-141.
    let mut builder = builder();
    let polled = with_monitor(&mut builder);
    let app = builder.build();
    let one = device(&app, "sw-140", "10.0.0.140", "access", "HQ");
    let two = device(&app, "sw-141", "10.0.0.141", "access", "HQ");
    cable(
        &app,
        (&one, "sw-140"),
        (&two, "sw-141"),
        "TenGigabitEthernet1/1/1",
    );
    {
        let mut rows = polled.lock().expect("the rows");
        for index in 1..=150 {
            let name = format!("sw-{index:03}");
            let id = match index {
                140 => one.clone(),
                141 => two.clone(),
                _ => format!("other-{index:03}"),
            };
            for port in 1..=59 {
                rows.push(polled_port(
                    &id,
                    &name,
                    &format!("GigabitEthernet1/0/{port}"),
                    0.0,
                    0.0,
                ));
            }
            rows.push(polled_port(&id, &name, "TenGigabitEthernet1/1/1", 2e9, 3e9));
        }
        assert_eq!(rows.len(), 9_000);
    }

    // The table decorates the graph itself, and that read was one call capped
    // at five thousand rows: the link's utilisation was simply missing.
    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["links"][0]["util_pct"], 30.0);

    // And it is what the map's link load is drawn from.
    let rates = call(
        &app,
        "topology.rates",
        json!({"ports": link_ends(&app).await}),
    )
    .await;
    assert_eq!(rates["monitoring"], true);
    for id in [&one, &two] {
        let port = port_of(&rates, id).expect("the port has a row in monitoring's table");
        assert_eq!(port["in_bps"], 2e9);
        assert_eq!(port["out_bps"], 3e9);
        assert_eq!(port["speed_bps"], 10_000_000_000u64);
        assert_eq!(port["at"], 1_700_000_000_000i64);
    }
}

#[tokio::test]
async fn a_device_with_more_ports_than_one_page_is_read_to_the_end() {
    // One chassis whose own rows outnumber a page: the port asked about is
    // the last of them by name.
    let mut builder = builder();
    let polled = with_monitor(&mut builder);
    let app = builder.build();
    let chassis = device(&app, "CHASSIS", "10.0.0.1", "core", "HQ");
    {
        let mut rows = polled.lock().expect("the rows");
        for port in 0..5_200 {
            rows.push(polled_port(
                &chassis,
                "CHASSIS",
                &format!("GigabitEthernet{}/{}", port / 48, port % 48),
                0.0,
                0.0,
            ));
        }
        rows.push(polled_port(&chassis, "CHASSIS", "Vlan4000", 7e6, 9e6));
    }

    let rates = call(
        &app,
        "topology.rates",
        json!({"ports": [{"device_id": chassis, "interface": "Vlan4000"}]}),
    )
    .await;
    let port = port_of(&rates, &chassis).expect("the last row of the device is reached");
    assert_eq!(port["name"], "Vlan4000");
    assert_eq!(port["out_bps"], 9e6);
}

#[tokio::test]
async fn a_port_monitoring_spells_its_own_way_is_the_same_port() {
    // Two IOS XR routers. Their profile keeps interface names verbatim, so
    // monitoring's rows are called `TenGigE0/0/0/1`; the graph always writes
    // a link end the canonical way, `TenGigabitEthernet0/0/0/1`. Compared as
    // text the two never met, and a backbone link with current rates on both
    // ports was drawn as "no rate" in both directions.
    let mut builder = builder();
    let polled = with_monitor(&mut builder);
    let app = builder.build();
    let one = device(&app, "XR1", "10.0.0.1", "router", "HQ");
    let two = device(&app, "XR2", "10.0.0.2", "router", "HQ");
    cable(&app, (&one, "XR1"), (&two, "XR2"), "TenGigE0/0/0/1");
    polled.lock().expect("the rows").extend([
        polled_port(&one, "XR1", "TenGigE0/0/0/1", 1e9, 4e9),
        polled_port(&two, "XR2", "TenGigE0/0/0/1", 4e9, 1e9),
    ]);

    let ends = link_ends(&app).await;
    assert_eq!(
        ends[0]["interface"], "TenGigabitEthernet0/0/0/1",
        "the graph spells a link end the canonical way"
    );
    let rates = call(&app, "topology.rates", json!({"ports": ends})).await;
    for (id, out_bps) in [(&one, 4e9), (&two, 1e9)] {
        let port = port_of(&rates, id).expect("both ends are found");
        // Keyed by the spelling that was asked with, so the caller needs no
        // idea of interface names of its own; `name` is monitoring's.
        assert_eq!(port["interface"], "TenGigabitEthernet0/0/0/1");
        assert_eq!(port["name"], "TenGigE0/0/0/1");
        assert_eq!(port["out_bps"], out_bps);
    }
    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["links"][0]["util_pct"], 40.0);
}

#[tokio::test]
async fn every_spelling_the_canonical_form_rewrites_still_finds_its_row() {
    // (what monitoring kept, what a link end is called): each pair is one
    // port, and each of them used to be two.
    let spellings = [
        ("TenGigE0/0/0/1", "TenGigabitEthernet0/0/0/1"),
        ("FortyGigE0/0/0/2", "FortyGigabitEthernet0/0/0/2"),
        ("HundredGigE0/0/1/0", "HundredGigE0/0/1/0"),
        ("Bundle-Ether12", "Bundle-Ether12"),
        ("eth0", "Ethernet0"),
        ("Eth1/1", "Ethernet1/1"),
        ("Tw1/0/1", "TwoGigabitEthernet1/0/1"),
        ("Fi1/0/2", "FiveGigabitEthernet1/0/2"),
        ("Gi0/3", "GigabitEthernet0/3"),
        ("xe-0/0/1", "xe-0/0/1"),
        ("ether1", "ether1"),
        ("sfp-sfpplus1", "sfp-sfpplus1"),
    ];
    let mut builder = builder();
    let polled = with_monitor(&mut builder);
    let app = builder.build();
    let router = device(&app, "R1", "10.0.0.1", "router", "HQ");
    polled.lock().expect("the rows").extend(
        spellings
            .iter()
            .enumerate()
            .map(|(index, (kept, _))| polled_port(&router, "R1", kept, 0.0, index as f64)),
    );

    let asked: Vec<Value> = spellings
        .iter()
        .map(|(_, end)| json!({"device_id": router, "interface": end}))
        .collect();
    let rates = call(&app, "topology.rates", json!({"ports": asked})).await;
    let ports = rates["ports"].as_array().expect("ports");
    for (index, (kept, end)) in spellings.iter().enumerate() {
        let port = ports
            .iter()
            .find(|port| port["interface"] == json!(end))
            .unwrap_or_else(|| panic!("{end} did not find the row monitoring calls {kept}"));
        assert_eq!(port["name"], json!(kept));
        assert_eq!(
            port["out_bps"], index as f64,
            "{end} found another port's row"
        );
    }
    assert_eq!(ports.len(), spellings.len());

    // A different port is a different port: nothing is matched on a prefix.
    let rates = call(
        &app,
        "topology.rates",
        json!({"ports": [
            {"device_id": router, "interface": "TenGigabitEthernet0/0/0/10"},
            {"device_id": router, "interface": "Ethernet1"},
            {"device_id": "nobody", "interface": "TenGigabitEthernet0/0/0/1"},
        ]}),
    )
    .await;
    assert_eq!(rates["ports"], json!([]));
}

/// A SONiC leaf: a management port `eth0` and a front-panel port `Ethernet0`,
/// whose canonical form is `Ethernet0` for both, each with its own row in
/// monitoring's table. The uplink to the spine is on Ethernet0, and only the
/// leaf is monitored.
async fn sonic_leaf() -> (App, String) {
    let mut builder = builder();
    let polled = with_monitor(&mut builder);
    let app = builder.build();
    let leaf = device(&app, "LEAF1", "10.0.0.1", "access", "HQ");
    let spine = device(&app, "SPINE1", "10.0.0.2", "core", "HQ");
    cable(&app, (&leaf, "LEAF1"), (&spine, "SPINE1"), "Ethernet0");
    polled.lock().expect("the rows").extend([
        polled_port(&leaf, "LEAF1", "eth0", 1e6, 2e6),
        polled_port(&leaf, "LEAF1", "Ethernet0", 5e9, 6e9),
    ]);
    (app, leaf)
}

#[tokio::test]
async fn two_ports_whose_names_have_one_canonical_form_are_still_two_ports() {
    // Joined on the canonical form alone, the uplink on Ethernet0 was
    // answered with the management port's rates.
    let (app, leaf) = sonic_leaf().await;

    let rates = call(
        &app,
        "topology.rates",
        json!({"ports": [
            {"device_id": leaf, "interface": "Ethernet0"},
            {"device_id": leaf, "interface": "eth0"},
            // Neither row's own spelling: which of the two it means cannot be
            // told, so it is answered with neither.
            {"device_id": leaf, "interface": "Eth0"},
        ]}),
    )
    .await;
    let ports = rates["ports"].as_array().expect("ports");
    let answer = |asked: &str| ports.iter().find(|port| port["interface"] == json!(asked));
    let front = answer("Ethernet0").expect("the front-panel port has a row");
    assert_eq!(front["name"], "Ethernet0");
    assert_eq!(front["out_bps"], 6e9);
    let management = answer("eth0").expect("the management port has a row");
    assert_eq!(management["name"], "eth0");
    assert_eq!(management["out_bps"], 2e6);
    assert!(answer("Eth0").is_none(), "{ports:#?}");
    assert_eq!(ports.len(), 2);
}

#[tokio::test]
async fn a_link_is_as_busy_as_its_own_port_not_as_one_with_the_same_canonical_name() {
    // The same on the drawing itself: the uplink is sixty per cent used, and
    // the management port's fraction of one per cent is not its utilisation.
    let (app, _) = sonic_leaf().await;
    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["links"][0]["util_pct"], 60.0);
}

#[tokio::test]
async fn a_port_nobody_measured_is_left_out_and_an_unmeasured_rate_stays_unmeasured() {
    let mut builder = builder();
    let polled = with_monitor(&mut builder);
    let app = builder.build();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    polled.lock().expect("the rows").push(json!({
        "device_id": one,
        "device": "SW1",
        "name": "GigabitEthernet0/1",
        "speed_bps": null,
        "in_bps": null,
        "out_bps": 0.0,
        "at": 1_700_000_000_000i64,
    }));

    let rates = call(
        &app,
        "topology.rates",
        json!({"ports": [
            {"device_id": one, "interface": "GigabitEthernet0/1"},
            {"device_id": one, "interface": "GigabitEthernet0/2"},
            // Asked twice, answered once.
            {"device_id": one, "interface": "GigabitEthernet0/1"},
        ]}),
    )
    .await;
    let ports = rates["ports"].as_array().expect("ports");
    assert_eq!(ports.len(), 1, "{ports:#?}");
    let port = &ports[0];
    assert_eq!(port["interface"], "GigabitEthernet0/1");
    // Zero was measured and is zero; the rest was not measured and is absent.
    assert_eq!(port["out_bps"], 0.0);
    assert!(port.get("in_bps").is_none(), "{port}");
    assert!(port.get("speed_bps").is_none(), "{port}");
}

#[tokio::test]
async fn the_answer_carries_the_servers_own_clock() {
    // An age is a reading's time taken from *this* clock. The browser's may
    // be minutes out, so the answer says what time it is here.
    let mut builder = builder();
    with_monitor(&mut builder);
    let app = builder.build();

    let before = now_ms();
    let rates = call(&app, "topology.rates", json!({"ports": []})).await;
    let after = now_ms();
    let at = rates["at"].as_i64().expect("at");
    assert!(
        (before..=after).contains(&at),
        "{before} <= {at} <= {after}"
    );
    assert_eq!(rates["monitoring"], true);
    assert_eq!(rates["ports"], json!([]));
}

#[tokio::test]
async fn a_build_without_monitoring_says_so_instead_of_answering_nothing_measured() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");

    for ports in [
        json!([]),
        json!([{"device_id": one, "interface": "GigabitEthernet0/1"}]),
    ] {
        let rates = call(&app, "topology.rates", json!({"ports": ports})).await;
        assert_eq!(rates["monitoring"], false);
        assert_eq!(rates["ports"], json!([]));
        assert!(rates["at"].as_i64().is_some());
    }
}

#[tokio::test]
async fn a_monitor_that_fails_is_a_failure_and_not_an_empty_answer() {
    let mut builder = builder();
    builder.router.add(
        "monitor.interfaces",
        |_: App, _: nocturne_app::NoParams| async move {
            Err::<Value, AppError>(AppError::unavailable("the store is busy"))
        },
    );
    let app = builder.build();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");

    let refused = app
        .rpc(
            "topology.rates",
            json!({"ports": [{"device_id": one, "interface": "GigabitEthernet0/1"}]}),
        )
        .await
        .expect_err("no rows were read, so there is no answer to give");
    assert_eq!(refused.code, "unavailable");

    // The graph itself is still drawn: utilisation is a decoration on it.
    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["nodes"].as_array().expect("nodes").len(), 1);
}

#[tokio::test]
async fn a_monitor_whose_pages_never_end_is_refused_rather_than_read_for_ever() {
    // A page as long as was asked for means "there is more". An answer that
    // ignores the cursor says that for ever.
    let mut builder = builder();
    builder.router.add(
        "monitor.interfaces",
        |_: App, query: InterfaceQuery| async move {
            let id = query.device_id.unwrap_or_default();
            let rows: Vec<Value> = (0..query.limit.unwrap_or(200))
                .map(|port| polled_port(&id, "SW1", &format!("GigabitEthernet0/{port}"), 0.0, 0.0))
                .collect();
            Ok(rows)
        },
    );
    let app = builder.build();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");

    let refused = app
        .rpc(
            "topology.rates",
            json!({"ports": [{"device_id": one, "interface": "GigabitEthernet0/1"}]}),
        )
        .await
        .expect_err("a table with no end has no last page");
    assert_eq!(refused.code, "internal");
}

#[tokio::test]
async fn open_alerts_are_counted_past_the_first_page_of_them() {
    // `alerts.list` pages the same way and gives at most two thousand rows a
    // call. On a bad day there are more open alerts than that, and a device
    // whose alerts were in the part that was not read had a clean card.
    let mut builder = builder();
    let alerts: Polled = Arc::default();
    let rows = alerts.clone();
    builder
        .router
        .add("alerts.list", move |_: App, query: InterfaceQuery| {
            let rows = rows.clone();
            async move {
                let offset = query
                    .cursor
                    .as_deref()
                    .map_or(Ok(0), str::parse::<usize>)
                    .map_err(|_| AppError::invalid("the cursor is the number of rows received"))?;
                let limit = query.limit.unwrap_or(100).clamp(1, 2_000) as usize;
                Ok(rows
                    .lock()
                    .expect("the alerts")
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .cloned()
                    .collect::<Vec<Value>>())
            }
        });
    let app = builder.build();
    let quiet = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    {
        let mut rows = alerts.lock().expect("the alerts");
        for index in 0..2_400 {
            rows.push(json!({"device_id": format!("other-{}", index % 40), "state": "firing"}));
        }
        for _ in 0..3 {
            rows.push(json!({"device_id": quiet, "state": "firing"}));
        }
    }

    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["nodes"][0]["name"], "SW1");
    assert_eq!(graph["nodes"][0]["alerts"], 3);
}

#[tokio::test]
async fn an_alert_that_fires_between_two_pages_does_not_count_another_one_twice() {
    // The pages are cut by offset, newest alert first. One that fires while
    // they are being read moves every later row down a place, and the last
    // row of one page comes back as the first of the next. It is still one
    // alert.
    let mut builder = builder();
    let alerts: Polled = Arc::default();
    let rows = alerts.clone();
    builder
        .router
        .add("alerts.list", move |_: App, query: InterfaceQuery| {
            let rows = rows.clone();
            async move {
                let offset = query
                    .cursor
                    .as_deref()
                    .map_or(Ok(0), str::parse::<usize>)
                    .map_err(|_| AppError::invalid("the cursor is the number of rows received"))?;
                let limit = query.limit.unwrap_or(100).clamp(1, 2_000) as usize;
                let mut rows = rows.lock().expect("the alerts");
                let page: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
                // Something fires on another device before the next page is
                // asked for.
                let fired = rows.len();
                rows.insert(
                    0,
                    json!({"id": format!("fired-{fired}"), "device_id": "elsewhere", "state": "firing"}),
                );
                Ok(page)
            }
        });
    let app = builder.build();
    let quiet = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    {
        let mut rows = alerts.lock().expect("the alerts");
        for index in 0..1_500 {
            // The one alert on SW1 is the last row of the first page.
            let device_id = if index == 999 {
                quiet.clone()
            } else {
                format!("other-{}", index % 40)
            };
            rows.push(
                json!({"id": format!("alert-{index}"), "device_id": device_id, "state": "firing"}),
            );
        }
    }

    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["nodes"][0]["name"], "SW1");
    assert_eq!(graph["nodes"][0]["alerts"], 1);
}
