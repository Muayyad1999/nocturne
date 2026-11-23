//! QoS in monitoring: the cadence, the rates and their gaps, the cap, the
//! current table, `monitor.qos` and the built-in rule.
//!
//! Two kinds of device answer here, and neither is hardware:
//!
//! * **The simulator** (`nocturne-netsim`, an engine): the edge router of
//!   its reference campus, which shapes its uplink and sorts what its hosts
//!   send by DSCP. What it prints for `show policy-map interface` is copied
//!   to the device the poll reads before every poll, so the numbers judged
//!   at the end are the simulator's own, read by the real `cisco_ios`
//!   profile. That is the end-to-end proof.
//! * **A fake device with listings written by hand** in the layout the
//!   simulator prints (compare `qos-lab-campus-edge` among the driver's
//!   captures). Synthetic, and there so that a test can set one counter to
//!   exactly the value its question needs: a restart, a counter cleared, a
//!   read that fails. The parser is still the real one — the test profile
//!   inherits `collect.qos` from `cisco_ios` and replaces only the reads
//!   whose commands the fake device does not have.

use std::sync::Arc;
use std::time::Duration;

use nocturne_app::testing::Lab;
use serde_json::{Value, json};

use crate::clock::VirtualClock;
use crate::poll::{Poll, poll_device};
use crate::state::MonitorState;
use crate::testing::{FakeProbe, FakeSnmp, epoch};

/// `cisco_ios` with its QoS read as it stands, an interface table and an
/// uptime the fake device can print, and the uptime in seconds so that an
/// interval is whatever the test says it is.
const QOS_PROFILE: &str = r#"
id: test_ios_qos
name: Test IOS with QoS
vendor: Test
os: IOS
extends: cisco_ios
collect:
  interfaces:
    - command: show interfaces summary
      parser:
        regex: '(?m)^(?P<name>\S+)\s+(?P<admin>up|down)\s+(?P<oper>up|down)\s+(?P<speed>\d+)\s+(?P<in_octets>\d+)\s*$'
      map:
        name: name
        admin_up: "admin == 'up'"
        oper_up: "oper == 'up'"
        speed_bps: speed
        counters.in_octets: in_octets
  health:
    - command: show system uptime
      parser: {regex: 'uptime (?P<uptime>\d+) seconds'}
      map: {uptime_s: uptime}
"#;

const UPLINK: &str = "GigabitEthernet0/0";
const VOICE: &str = "GigabitEthernet0/0|out|WAN/VOICE";
const BULK: &str = "GigabitEthernet0/0|out|WAN/BULK";
const GUEST: &str = "GigabitEthernet0/1|in|LAN-IN/GUEST";

/// One class of a hand-written listing.
#[derive(Clone, Copy)]
struct Class {
    name: &'static str,
    packets: u64,
    bytes: u64,
    /// `(queue depth, total drops)` of a class that queues.
    queue: Option<(u64, u64)>,
    /// Packets a policer let through and packets it dropped.
    police: Option<(u64, u64)>,
}

impl Class {
    fn queued(name: &'static str, packets: u64, bytes: u64, depth: u64, drops: u64) -> Class {
        Class {
            name,
            packets,
            bytes,
            queue: Some((depth, drops)),
            police: None,
        }
    }

    fn policed(name: &'static str, packets: u64, bytes: u64, dropped: u64) -> Class {
        Class {
            name,
            packets,
            bytes,
            queue: None,
            police: Some((packets - dropped, dropped)),
        }
    }
}

/// One attached policy as `show policy-map interface` lists it. SYNTHETIC:
/// written here, line for line in the layout the simulator prints.
fn policy(interface: &str, direction: &str, name: &str, classes: &[Class]) -> String {
    let mut out = format!(" {interface}\n\n  Service-policy {direction}: {name}\n\n");
    for class in classes {
        out.push_str(&format!(
            "    Class-map: {} (match-any)\n      {} packets, {} bytes\n      \
             5 minute offered rate 0 bps, drop rate 0 bps\n      Match: any\n",
            class.name, class.packets, class.bytes
        ));
        if let Some((depth, drops)) = class.queue {
            out.push_str(&format!(
                "      Queueing\n      queue limit 64 packets\n      \
                 (queue depth/total drops/no-buffer drops) {depth}/{drops}/0\n      \
                 (pkts output/bytes output) {}/{}\n",
                class.packets - drops,
                class.bytes
            ));
        }
        if let Some((conformed, exceeded)) = class.police {
            out.push_str(&format!(
                "      police:\n          cir 3000000 bps, bc 93750 bytes\n        \
                 conformed {conformed} packets, {} bytes; actions:\n          transmit\n        \
                 exceeded {exceeded} packets, {} bytes; actions:\n          drop\n        \
                 conformed 0 bps, exceed 0 bps\n",
                conformed * 100,
                exceeded * 100
            ));
        }
        out.push('\n');
    }
    out
}

/// A lab device called EDGE, read with [`QOS_PROFILE`], on a clock that
/// follows the paused timer.
async fn edge() -> (Lab, String) {
    edge_on(Arc::new(VirtualClock::new(epoch()))).await
}

/// The same device, on a clock of the test's choosing.
async fn edge_on(clock: Arc<dyn crate::clock::Clock>) -> (Lab, String) {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        clock,
        Arc::new(FakeProbe::default()),
        Arc::new(FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["EDGE"]).await;
    nocturne_app::profiles::save(&lab.app, QOS_PROFILE).expect("the profile loads");
    let id = lab.id("EDGE");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_ios_qos",
                   "monitor": {"enabled": true, "icmp": true, "snmp": false, "interval_s": 10}}),
        )
        .await
        .unwrap();
    (lab, id)
}

/// What the device prints from now on: its uptime and its QoS listing.
fn report(lab: &Lab, uptime_s: u64, listing: &str) {
    lab.fake("EDGE").with(|state| {
        state.outputs.insert(
            "show interfaces summary".into(),
            "GigabitEthernet0/0     up    up    1000000000  1000\n".into(),
        );
        state.outputs.insert(
            "show system uptime".into(),
            format!("Device uptime {uptime_s} seconds\n"),
        );
        state
            .outputs
            .insert("show policy-map interface".into(), listing.to_string());
    });
}

/// The uplink's two queueing classes and the inside interface's policer, at
/// the counters given: `(packets, drops)` each, a packet being 100 bytes.
fn listing(voice: (u64, u64), bulk: (u64, u64), guest: (u64, u64)) -> String {
    policy(
        UPLINK,
        "output",
        "WAN",
        &[
            Class::queued("VOICE", voice.0, voice.0 * 100, 0, voice.1),
            Class::queued("BULK", bulk.0, bulk.0 * 100, 7, bulk.1),
        ],
    ) + &policy(
        "GigabitEthernet0/1",
        "input",
        "LAN-IN",
        &[Class::policed("GUEST", guest.0, guest.0 * 100, guest.1)],
    )
}

async fn poll(lab: &Lab, id: &str) -> Poll {
    poll_device(&lab.app, id).await.expect("the poll runs")
}

async fn after(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

async fn qos(lab: &Lab, params: Value) -> Value {
    lab.app
        .rpc("monitor.qos", params)
        .await
        .unwrap_or_else(|err| panic!("monitor.qos: {err}"))
}

/// The row of one class in a `monitor.qos` answer.
fn row<'a>(page: &'a Value, class: &str) -> &'a Value {
    page["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["class"] == class)
        .unwrap_or_else(|| panic!("no class {class} in {page}"))
}

/// Every sample of one series, oldest first.
///
/// The store answers a range in buckets, and averages what shares one. An
/// hour either side of now in 5,000 buckets keeps two reads thirty seconds
/// apart in two of them.
fn samples(lab: &Lab, id: &str, metric: &str, instance: &str) -> Vec<f64> {
    let store = lab.app.store();
    let series = store.series_id(id, metric, instance).unwrap();
    let now = nocturne_app::util::now_ms();
    store
        .samples_query(series, now - 3_600_000, now + 3_600_000, 5_000)
        .unwrap()
        .into_iter()
        .map(|point| point.avg)
        .collect()
}

// ------------------------------------------------------------------ the rates

#[tokio::test(start_paused = true)]
async fn the_first_read_has_every_class_and_its_queue_depth_but_no_rate() {
    let (lab, id) = edge().await;
    let mut events = lab.app.subscribe();
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    let first = poll(&lab, &id).await;
    assert_eq!(first.qos_read, Some(true));
    assert_eq!(first.qos.len(), 3);

    let page = qos(&lab, json!({"device_id": id})).await;
    let bulk = row(&page, "BULK");
    // What the device said is there…
    assert_eq!(bulk["packets"], 50_000);
    assert_eq!(bulk["dropped_packets"], 400);
    assert_eq!(bulk["queue_len"], 7);
    assert_eq!(bulk["instance"], BULK);
    // …and what needs a second reading is not, rather than being zero.
    for rate in ["bps", "drop_ps", "queue_drop_ps", "drop_pct"] {
        assert_eq!(bulk[rate], Value::Null, "{rate} after one reading");
        assert_eq!(first.measure(&format!("qos.{rate}"), BULK), None);
        assert!(samples(&lab, &id, &format!("qos.{rate}"), BULK).is_empty());
    }
    // A gauge needs no interval.
    assert_eq!(samples(&lab, &id, "qos.queue_len", BULK), [7.0]);
    assert!(
        qos(&lab, json!({"device_id": id, "problem_only": true})).await["rows"]
            .as_array()
            .unwrap()
            .is_empty(),
        "400 drops since the device started are not drops now"
    );

    let mut announced = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "monitor.qos.updated" {
            announced.push(event.payload);
        }
    }
    assert_eq!(announced, [json!({"device_id": id})]);
}

#[tokio::test(start_paused = true)]
async fn rates_are_counter_deltas_over_the_devices_own_uptime() {
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;

    // The poller comes back after 30 s; the device counts 40 s between the
    // two readings (a slow login the first time), and the rates follow the
    // device. 2,000 voice packets and none dropped; 8,000 bulk packets of
    // which 600 were dropped from the queue; 4,000 guest packets of which a
    // policer dropped 1,000.
    after(30).await;
    report(
        &lab,
        100_040,
        &listing((3_000, 0), (58_000, 1_000), (13_000, 4_000)),
    );
    let second = poll(&lab, &id).await;
    let page = qos(&lab, json!({"device_id": id})).await;

    let voice = row(&page, "VOICE");
    assert_eq!(voice["bps"], 2_000.0 * 100.0 / 40.0 * 8.0);
    assert_eq!(voice["drop_ps"], 0.0, "a real zero, measured");
    assert_eq!(voice["queue_drop_ps"], 0.0);
    assert_eq!(voice["drop_pct"], 0.0);

    let bulk = row(&page, "BULK");
    assert_eq!(bulk["bps"], 8_000.0 * 100.0 / 40.0 * 8.0);
    assert_eq!(bulk["drop_ps"], 15.0);
    assert_eq!(bulk["queue_drop_ps"], 15.0);
    assert_eq!(bulk["drop_pct"], 7.5, "600 of the 8,000 offered");
    assert_eq!(
        (bulk["packets"].as_u64(), bulk["dropped_packets"].as_u64()),
        (Some(58_000), Some(1_000)),
        "the counters as last read"
    );

    // The policed class drops and has no queue: its drops are the policer's
    // and it has no congestion figure at all, not a zero one.
    let guest = row(&page, "GUEST");
    assert_eq!(guest["drop_ps"], 25.0);
    assert_eq!(guest["drop_pct"], 25.0);
    assert_eq!(guest["queue_drop_ps"], Value::Null);
    assert_eq!(guest["queue_len"], Value::Null);
    assert_eq!(guest["rate_bps"], 3_000_000);

    // The five series, under the instance the specification spells.
    assert_eq!(samples(&lab, &id, "qos.bps", BULK), [160_000.0]);
    assert_eq!(samples(&lab, &id, "qos.drop_ps", BULK), [15.0]);
    assert_eq!(samples(&lab, &id, "qos.queue_drop_ps", BULK), [15.0]);
    assert_eq!(samples(&lab, &id, "qos.drop_pct", BULK), [7.5]);
    assert_eq!(samples(&lab, &id, "qos.queue_len", BULK), [7.0, 7.0]);
    assert_eq!(samples(&lab, &id, "qos.drop_ps", GUEST), [25.0]);
    assert!(
        samples(&lab, &id, "qos.queue_drop_ps", GUEST).is_empty(),
        "only where the row separates them"
    );
    assert_eq!(second.measure("qos.queue_drop_ps", BULK), Some(15.0));
    assert_eq!(second.measure("qos.queue_drop_ps", VOICE), Some(0.0));

    // "Dropping" is congestion: the queue that overflows, not the policer
    // doing what it was configured to do.
    assert_eq!(
        (&voice["dropping"], &bulk["dropping"], &guest["dropping"]),
        (&json!(false), &json!(true), &json!(false))
    );
    let dropping = qos(&lab, json!({"device_id": id, "problem_only": true})).await;
    assert_eq!(
        dropping["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["class"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["BULK"]
    );
    // And the series are there to be charted by the instance the row gives.
    let charted = lab
        .app
        .rpc(
            "metrics.query",
            json!({"device_id": id, "metric": "qos.queue_drop_ps",
                   "instance": bulk["instance"], "since": epoch() - 3_600_000}),
        )
        .await
        .unwrap();
    assert_eq!(charted["points"].as_array().unwrap().len(), 1);
    assert_eq!(charted["points"][0][1], 15.0);
}

#[tokio::test(start_paused = true)]
async fn a_restart_throws_the_interval_away_rather_than_inventing_a_rate() {
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    after(30).await;
    report(
        &lab,
        100_030,
        &listing((2_500, 0), (53_000, 700), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    assert_eq!(samples(&lab, &id, "qos.drop_ps", BULK), [10.0]);

    // The router restarts: 20 s of uptime, and counters that have already
    // passed where they stood before (a busy class counts fast). Subtracted
    // from the old reading they would be a plausible, wrong rate.
    after(30).await;
    report(
        &lab,
        20,
        &listing((2_600, 0), (53_900, 760), (9_100, 3_050)),
    );
    let restarted = poll(&lab, &id).await;
    assert_eq!(restarted.qos_read, Some(true));
    let page = qos(&lab, json!({"device_id": id})).await;
    for class in ["VOICE", "BULK", "GUEST"] {
        let row = row(&page, class);
        for rate in ["bps", "drop_ps", "queue_drop_ps", "drop_pct"] {
            assert_eq!(row[rate], Value::Null, "{class} {rate} across a restart");
        }
    }
    assert_eq!(restarted.measure("qos.drop_ps", BULK), None);
    assert_eq!(
        samples(&lab, &id, "qos.drop_ps", BULK),
        [10.0],
        "nothing written for the interval that was not measured"
    );
    assert_eq!(page["coverage"]["devices"][0]["state"], "ok");

    // The read after the restart has a baseline again.
    after(30).await;
    report(
        &lab,
        50,
        &listing((2_900, 0), (54_500, 910), (9_100, 3_050)),
    );
    poll(&lab, &id).await;
    assert_eq!(samples(&lab, &id, "qos.drop_ps", BULK), [10.0, 5.0]);
}

#[tokio::test(start_paused = true)]
async fn a_counter_that_went_backwards_is_a_gap_for_what_it_measures() {
    // `clear counters` on a router that stays up. Policy counters read off
    // text have no width to wrap around: down is cleared.
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (5_000_000_000, 400_000), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    after(30).await;
    report(
        &lab,
        100_030,
        &listing((2_500, 0), (900, 12), (10_500, 3_300)),
    );
    let cleared = poll(&lab, &id).await;
    let page = qos(&lab, json!({"device_id": id})).await;
    let bulk = row(&page, "BULK");
    for rate in ["bps", "drop_ps", "queue_drop_ps", "drop_pct"] {
        assert_eq!(bulk[rate], Value::Null, "{rate}: a gap, not 10^17");
        assert_eq!(cleared.measure(&format!("qos.{rate}"), BULK), None);
    }
    // The classes whose counters kept counting keep their rates.
    assert_eq!(row(&page, "VOICE")["bps"], 1_500.0 * 100.0 / 30.0 * 8.0);
    assert_eq!(row(&page, "GUEST")["drop_ps"], 10.0);

    // And the cleared counters are the new baseline.
    after(30).await;
    report(
        &lab,
        100_060,
        &listing((2_500, 0), (3_900, 162), (10_500, 3_300)),
    );
    poll(&lab, &id).await;
    let page = qos(&lab, json!({"device_id": id})).await;
    assert_eq!(row(&page, "BULK")["drop_ps"], 5.0);
    assert_eq!(row(&page, "BULK")["drop_pct"], 5.0);
}

#[tokio::test(start_paused = true)]
async fn a_failed_read_is_a_gap_and_the_table_stops_claiming_its_rates() {
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    after(30).await;
    report(
        &lab,
        100_030,
        &listing((2_500, 0), (53_000, 700), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    let read_at = qos(&lab, json!({"device_id": id})).await["coverage"]["devices"][0]["at"]
        .as_i64()
        .unwrap();

    // The device refuses the command (a release without MQC, a view that
    // may not run it): the fake answers what it answers to anything unknown.
    let mut events = lab.app.subscribe();
    after(30).await;
    lab.fake("EDGE")
        .with(|state| state.outputs.remove("show policy-map interface"));
    let failed = poll(&lab, &id).await;
    assert_eq!(failed.qos_read, Some(false));
    assert!(failed.ok, "the device itself answered");
    assert!(
        !failed
            .measures
            .iter()
            .any(|measure| measure.metric.starts_with("qos.")),
        "no sample of any kind, and certainly no zero: {:?}",
        failed.measures
    );

    let page = qos(&lab, json!({"device_id": id})).await;
    let coverage = &page["coverage"]["devices"][0];
    assert_eq!(coverage["state"], "failed");
    assert!(coverage["at"].as_i64().unwrap() > read_at);
    assert!(
        coverage["error"]
            .as_str()
            .unwrap()
            .contains("show policy-map interface"),
        "{coverage}"
    );
    // The classes are still the last ones known, with the counters and the
    // time of the read that produced them; what they are doing now is not
    // known, and nothing is reported as dropping.
    let bulk = row(&page, "BULK");
    assert_eq!(bulk["packets"], 53_000);
    assert_eq!(bulk["at"], read_at);
    assert_eq!(bulk["drop_ps"], Value::Null);
    assert_eq!(bulk["bps"], Value::Null);
    assert_eq!(bulk["dropping"], false);
    assert!(
        qos(&lab, json!({"device_id": id, "problem_only": true})).await["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| event.topic == "monitor.qos.updated"),
        "what the table says changed, so it is announced"
    );

    // The read works again. The counters held before the failure were
    // dropped with it: this is a baseline, not a rate across the hole.
    after(30).await;
    report(
        &lab,
        100_090,
        &listing((4_000, 0), (59_000, 1_300), (9_000, 3_000)),
    );
    let back = poll(&lab, &id).await;
    assert_eq!(back.qos_read, Some(true));
    assert_eq!(back.measure("qos.drop_ps", BULK), None);
    assert_eq!(
        qos(&lab, json!({"device_id": id})).await["coverage"]["devices"][0]["state"],
        "ok"
    );
    after(30).await;
    report(
        &lab,
        100_120,
        &listing((4_000, 0), (62_000, 1_600), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    assert_eq!(
        samples(&lab, &id, "qos.drop_ps", BULK),
        [10.0, 10.0],
        "one sample either side of the hole and none in it"
    );
}

#[tokio::test(start_paused = true)]
async fn an_uptime_printed_to_the_minute_and_a_clock_set_back_still_make_an_interval() {
    // The device prints its uptime in whole minutes, so two reads thirty
    // seconds apart show the same figure: by the device's account no time
    // passed. The poller's clock measures the interval instead — the one
    // that only goes forwards, because in between NTP sets the time of day
    // back an hour, and thirty seconds must not become minus fifty-nine
    // and a half minutes.
    let clock = Arc::new(crate::testing::SteppedClock::new(epoch()));
    let (lab, id) = edge_on(clock.clone()).await;
    report(
        &lab,
        6_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;

    clock.step(-3_600_000);
    after(30).await;
    report(
        &lab,
        6_000,
        &listing((2_500, 0), (53_000, 700), (9_000, 3_000)),
    );
    let second = poll(&lab, &id).await;
    let measured = |metric: &str, instance: &str| {
        second
            .measure(metric, instance)
            .unwrap_or_else(|| panic!("no {metric} for {instance}: the interval was thrown away"))
    };
    // Thirty seconds, give or take the moment a login takes.
    let drops = measured("qos.queue_drop_ps", BULK);
    assert!((9.8..=10.0).contains(&drops), "{drops} drops a second");
    let traffic = measured("qos.bps", VOICE);
    assert!((39_200.0..=40_000.0).contains(&traffic), "{traffic} bit/s");
    // The interface rates are computed by the same rule from the same two
    // clocks: a counter that did not move is a measured zero, not a gap.
    assert_eq!(second.interfaces[0].in_bps, Some(0.0));

    // A minute later by both clocks: 60 s by the device's account is the
    // 30 s that passed, rounded up, and is not taken at its word either.
    after(30).await;
    report(
        &lab,
        6_060,
        &listing((4_000, 0), (56_000, 1_000), (9_000, 3_000)),
    );
    let third = poll(&lab, &id).await;
    let drops = third
        .measure("qos.queue_drop_ps", BULK)
        .expect("the interval is measured");
    assert!(
        (9.8..=10.0).contains(&drops),
        "{drops} drops a second, not 5"
    );
}

// --------------------------------------------------------- what absence means

#[tokio::test(start_paused = true)]
async fn never_read_no_policy_and_no_read_at_all_are_three_answers() {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        Arc::new(VirtualClock::new(epoch())),
        Arc::new(FakeProbe::default()),
        Arc::new(FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["EDGE", "SW1"]).await;
    nocturne_app::profiles::save(&lab.app, QOS_PROFILE).expect("the profile loads");
    let (edge, switch) = (lab.id("EDGE"), lab.id("SW1"));
    lab.app
        .rpc(
            "devices.update",
            json!({"id": edge, "profile": "test_ios_qos"}),
        )
        .await
        .unwrap();
    // A device with nothing but ICMP: the right profile and no command line.
    let pinged = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "CORE", "address": "10.0.0.9", "transport": "none",
                   "profile": "test_ios_qos",
                   "monitor": {"enabled": true, "icmp": true, "snmp": false, "interval_s": 60}}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // And one nobody monitors and nobody has read.
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "SPARE", "address": "10.0.0.10", "transport": "none",
                   "monitor": {"enabled": false}}),
        )
        .await
        .unwrap();

    let state_of = |page: &Value, device_id: &str| -> Value {
        page["coverage"]["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["device_id"] == device_id)
            .cloned()
            .unwrap_or(Value::Null)
    };

    // Before anything is polled.
    let before = qos(&lab, json!({})).await;
    assert_eq!(before["rows"], json!([]));
    assert_eq!(
        state_of(&before, &edge),
        json!({"device_id": edge, "state": "never", "at": null, "unverified": true,
               "series_capped": false, "error": null, "classes": 0, "series": 0}),
        "it can be read and has not been; that the read is unchecked is known already"
    );
    // SW1's profile (the core's test profile) has no QoS read; CORE has the
    // read and nothing to ask it on.
    assert_eq!(state_of(&before, &switch)["state"], "unsupported");
    assert_eq!(state_of(&before, &pinged)["state"], "unsupported");
    assert_eq!(
        before["coverage"]["devices"].as_array().unwrap().len(),
        3,
        "a device nobody watches is not in the coverage: {before}"
    );

    // EDGE has no policy attached: it answers the command with nothing.
    report(&lab, 100_000, "");
    lab.fake("SW1").with(|state| {
        state.outputs.insert(
            "show interfaces summary".into(),
            "GigabitEthernet0/1     up    up    1000000000  1000\n".into(),
        );
    });
    let read = poll(&lab, &edge).await;
    assert_eq!(read.qos_read, Some(true));
    let unasked = poll(&lab, &switch).await;
    assert_eq!(unasked.qos_read, None);
    assert!(
        !lab.fake("SW1")
            .seen()
            .iter()
            .any(|command| command.contains("policy-map")),
        "a device whose profile has no QoS read is not asked"
    );
    assert_eq!(poll(&lab, &pinged).await.qos_read, None);

    let after_poll = qos(&lab, json!({})).await;
    assert_eq!(after_poll["rows"], json!([]));
    let entry = state_of(&after_poll, &edge);
    assert_eq!(entry["state"], "ok", "read, and empty: {entry}");
    assert!(entry["at"].is_i64());
    assert_eq!(entry["classes"], 0);
    assert_eq!(state_of(&after_poll, &switch)["state"], "unsupported");
    assert_eq!(state_of(&after_poll, &pinged)["state"], "unsupported");

    // Asked about one device, the coverage is that device's.
    let one = qos(&lab, json!({"device_id": switch})).await;
    assert_eq!(
        one["coverage"],
        json!({"devices": [{"device_id": switch, "state": "unsupported", "at": null,
                            "unverified": false, "series_capped": false, "error": null,
                            "classes": 0, "series": 0}]})
    );
    let error = lab
        .app
        .rpc("monitor.qos", json!({"device_id": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
}

#[tokio::test(start_paused = true)]
async fn a_device_whose_read_goes_away_loses_its_rows() {
    // The profile is changed to one without the read (a device re-detected
    // as something else): what was read under the old one is not kept as if
    // it were current.
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    assert_eq!(
        qos(&lab, json!({"device_id": id})).await["rows"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    lab.app
        .rpc("devices.update", json!({"id": id, "profile": "test_ios"}))
        .await
        .unwrap();
    after(30).await;
    poll(&lab, &id).await;
    let page = qos(&lab, json!({"device_id": id})).await;
    assert_eq!(page["rows"], json!([]));
    assert_eq!(page["coverage"]["devices"][0]["state"], "unsupported");
}

// --------------------------------------------------------------------- the cap

#[tokio::test(start_paused = true)]
async fn series_stop_at_the_cap_in_a_stable_order_and_the_table_holds_every_class() {
    let (lab, id) = edge().await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"qos_max_series": 3}}),
        )
        .await
        .unwrap();
    // Five classes, listed by the device in an order that is not the
    // stable one.
    let classes = |step: u64| {
        policy(
            "GigabitEthernet0/2",
            "output",
            "SUBSCRIBERS",
            &[
                Class::queued(
                    "erin",
                    5_000 + step * 300,
                    500_000 + step * 30_000,
                    0,
                    step * 30,
                ),
                Class::queued("alice", 1_000 + step * 300, 100_000 + step * 30_000, 0, 0),
            ],
        ) + &policy(
            UPLINK,
            "output",
            "SUBSCRIBERS",
            &[
                Class::queued("dave", 4_000 + step * 300, 400_000 + step * 30_000, 0, 0),
                Class::queued("carol", 3_000 + step * 300, 300_000 + step * 30_000, 0, 0),
                Class::queued("bob", 2_000 + step * 300, 200_000 + step * 30_000, 0, 0),
            ],
        )
    };
    report(&lab, 100_000, &classes(0));
    poll(&lab, &id).await;
    after(30).await;
    report(&lab, 100_030, &classes(1));
    let second = poll(&lab, &id).await;

    let page = qos(&lab, json!({"device_id": id})).await;
    let held: Vec<(&str, &str, Value)> = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["interface"].as_str().unwrap(),
                row["class"].as_str().unwrap(),
                row["instance"].clone(),
            )
        })
        .collect();
    assert_eq!(
        held,
        [
            (
                UPLINK,
                "bob",
                json!("GigabitEthernet0/0|out|SUBSCRIBERS/bob")
            ),
            (
                UPLINK,
                "carol",
                json!("GigabitEthernet0/0|out|SUBSCRIBERS/carol")
            ),
            (
                UPLINK,
                "dave",
                json!("GigabitEthernet0/0|out|SUBSCRIBERS/dave")
            ),
            ("GigabitEthernet0/2", "alice", Value::Null),
            ("GigabitEthernet0/2", "erin", Value::Null),
        ],
        "by interface, direction, policy and class: not as the device listed them"
    );
    // Past the cap a class is still read, still has its rates and still
    // shows as dropping; it has no series, and so no alert rule sees it.
    let erin = row(&page, "erin");
    assert_eq!(erin["drop_ps"], 1.0);
    assert_eq!(erin["bps"], 8_000.0);
    assert_eq!(
        qos(&lab, json!({"device_id": id, "problem_only": true})).await["rows"][0]["class"],
        "erin"
    );
    assert!(
        samples(
            &lab,
            &id,
            "qos.bps",
            "GigabitEthernet0/2|out|SUBSCRIBERS/erin"
        )
        .is_empty()
    );
    assert_eq!(
        samples(
            &lab,
            &id,
            "qos.bps",
            "GigabitEthernet0/0|out|SUBSCRIBERS/bob"
        ),
        [8_000.0]
    );
    let instances: std::collections::BTreeSet<&str> = second
        .measures
        .iter()
        .filter(|measure| measure.metric.starts_with("qos."))
        .map(|measure| measure.instance.as_str())
        .collect();
    assert_eq!(instances.len(), 3);
    // And it is said, with the numbers.
    assert_eq!(
        page["coverage"]["devices"][0],
        json!({"device_id": id, "state": "ok", "at": page["rows"][0]["at"], "unverified": true,
               "series_capped": true, "error": null, "classes": 5, "series": 3})
    );

    // With room for all of them nothing is capped.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"qos_max_series": 5}}),
        )
        .await
        .unwrap();
    after(30).await;
    report(&lab, 100_060, &classes(2));
    poll(&lab, &id).await;
    let page = qos(&lab, json!({"device_id": id})).await;
    assert_eq!(page["coverage"]["devices"][0]["series_capped"], false);
    assert_eq!(page["coverage"]["devices"][0]["series"], 5);
    assert_eq!(
        samples(
            &lab,
            &id,
            "qos.bps",
            "GigabitEthernet0/2|out|SUBSCRIBERS/erin"
        ),
        [8_000.0]
    );
}

#[tokio::test(start_paused = true)]
async fn a_class_with_a_slash_in_its_name_keeps_one_series_under_one_instance() {
    let (lab, id) = edge().await;
    let named = |packets: u64| {
        policy(
            "Serial0/0/0",
            "output",
            "AutoQoS-Policy-Se0/0/0",
            &[
                Class::queued("VOICE/RTP", packets, packets * 100, 0, 0),
                Class::queued("class-default", packets, packets * 100, 0, 0),
            ],
        )
    };
    report(&lab, 100_000, &named(1_000));
    poll(&lab, &id).await;
    after(30).await;
    report(&lab, 100_030, &named(1_300));
    poll(&lab, &id).await;

    let page = qos(&lab, json!({"device_id": id, "interface": "Se0/0/0"})).await;
    let slashed = row(&page, "VOICE/RTP");
    assert_eq!(slashed["policy"], "AutoQoS-Policy-Se0/0/0");
    assert_eq!(
        slashed["instance"],
        "Serial0/0/0|out|AutoQoS-Policy-Se0/0/0/VOICE%2FRTP"
    );
    assert_eq!(
        row(&page, "class-default")["instance"],
        "Serial0/0/0|out|AutoQoS-Policy-Se0/0/0/class-default"
    );
    // The instance reads back to the four names, and is the series' name.
    assert_eq!(
        crate::qos::parse_instance(slashed["instance"].as_str().unwrap()),
        Some((
            "Serial0/0/0".to_string(),
            nocturne_drivers::model::Direction::Out,
            "AutoQoS-Policy-Se0/0/0".to_string(),
            "VOICE/RTP".to_string()
        ))
    );
    assert_eq!(
        samples(&lab, &id, "qos.bps", slashed["instance"].as_str().unwrap()),
        [8_000.0]
    );
}

// ------------------------------------------------------------------ monitor.qos

#[tokio::test(start_paused = true)]
async fn monitor_qos_pages_by_cursor_and_filters_by_interface() {
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    after(30).await;
    report(
        &lab,
        100_030,
        &listing((2_500, 0), (53_000, 700), (10_500, 3_300)),
    );
    poll(&lab, &id).await;

    // The whole answer for one row, as the interface will be typed from it.
    let page = qos(
        &lab,
        json!({"device_id": id, "interface": "GigabitEthernet0/1"}),
    )
    .await;
    let at = page["rows"][0]["at"].clone();
    assert!(at.is_i64());
    assert_eq!(
        page,
        json!({
            "rows": [{
                "device_id": id, "interface": "GigabitEthernet0/1", "direction": "in",
                "policy": "LAN-IN", "class": "GUEST", "parent": null,
                "instance": "GigabitEthernet0/1|in|LAN-IN/GUEST", "rate_bps": 3_000_000,
                "bps": 40_000.0, "drop_ps": 10.0, "queue_drop_ps": null, "drop_pct": 20.0,
                "queue_len": null, "packets": 10_500, "dropped_packets": 3_300,
                "dropping": false, "at": at, "unverified": true,
            }],
            "next_cursor": null,
            "coverage": {"devices": [{
                "device_id": id, "state": "ok", "at": at, "unverified": true,
                "series_capped": false, "error": null, "classes": 3, "series": 3,
            }]},
        })
    );

    // Two rows a page, three rows: a second page, then no third.
    let first = qos(&lab, json!({"limit": 2})).await;
    let classes = |page: &Value| -> Vec<String> {
        page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["class"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(classes(&first), ["BULK", "VOICE"]);
    let cursor = first["next_cursor"].as_str().expect("a cursor").to_string();
    let second = qos(&lab, json!({"limit": 2, "cursor": cursor})).await;
    assert_eq!(classes(&second), ["GUEST"]);
    assert_eq!(second["next_cursor"], Value::Null);
    // Every page says what its rows' absence would mean.
    assert_eq!(second["coverage"], first["coverage"]);
    // A page that ends exactly on the last row does not promise another.
    assert_eq!(
        qos(&lab, json!({"limit": 3})).await["next_cursor"],
        Value::Null
    );

    // The interface by any spelling of its name; a name nothing is attached
    // to is an empty page, not an error.
    assert_eq!(
        classes(&qos(&lab, json!({"interface": "Gi0/0"})).await),
        ["BULK", "VOICE"]
    );
    assert_eq!(
        classes(&qos(&lab, json!({"interface": "gigabitethernet0/0"})).await),
        ["BULK", "VOICE"]
    );
    assert!(classes(&qos(&lab, json!({"interface": "GigabitEthernet0/7"})).await).is_empty());

    let error = lab
        .app
        .rpc("monitor.qos", json!({"cursor": "later"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
}

// ------------------------------------------------------------------ the cadence

#[tokio::test(start_paused = true)]
async fn qos_is_read_on_its_own_cadence_in_the_poll_s_one_session() {
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    let asked = |lab: &Lab| {
        lab.fake("EDGE")
            .seen()
            .iter()
            .filter(|command| command.as_str() == "show policy-map interface")
            .count()
    };
    let first = poll(&lab, &id).await;
    assert_eq!((first.qos_read, asked(&lab)), (Some(true), 1));

    // The device is polled every ten seconds; a lab device's QoS is read
    // every thirty.
    for expected in [1, 1, 2, 2, 2, 3] {
        after(10).await;
        let poll = poll(&lab, &id).await;
        assert_eq!(asked(&lab), expected);
        assert_eq!(
            poll.qos_read.is_some(),
            poll.sources.cli.at == Some(poll.at)
        );
    }

    // The interfaces, the uptime and the classes come out of one login: a
    // poll that reads all three opens the session the reachability probe
    // uses and one more, not one per kind.
    lab.fake("EDGE").clear_seen();
    let sessions = lab.fake("EDGE").with(|state| state.sessions);
    after(30).await;
    let poll = poll(&lab, &id).await;
    assert_eq!(poll.qos_read, Some(true));
    assert!(poll.interfaces_read);
    assert_eq!(lab.fake("EDGE").with(|state| state.sessions), sessions + 2);
}

// --------------------------------------------------------------------- the rule

#[tokio::test(start_paused = true)]
async fn the_congestion_rule_waits_five_minutes_and_ignores_a_policer() {
    let (lab, id) = edge().await;
    // Lab devices alert only when a rule opts in.
    let rule = lab
        .app
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.qos_class_dropping", "scope": {"include_lab": true}}),
        )
        .await
        .unwrap();
    assert_eq!(
        (
            rule["name"].as_str(),
            rule["severity"].as_str(),
            rule["for_s"].as_u64()
        ),
        (
            Some("QoS class is dropping (congestion)"),
            Some("medium"),
            Some(300)
        )
    );
    assert_eq!(
        rule["condition"],
        json!({"type": "threshold", "metric": "qos.queue_drop_ps", "instance": null,
               "op": "gt", "value": 10.0, "clear_value": 1.0})
    );
    assert_eq!(
        (rule["builtin"].clone(), rule["enabled"].clone()),
        (json!(true), json!(true))
    );

    let open = |lab: &Lab| -> Vec<crate::model::Alert> {
        crate::alerts::list(
            &lab.app,
            &crate::alerts::AlertQuery {
                state: Some("open".into()),
                limit: Some(100),
                ..crate::alerts::AlertQuery::default()
            },
        )
        .unwrap()
        .into_iter()
        .filter(|alert| alert.rule_id.as_deref() == Some("builtin.qos_class_dropping"))
        .collect()
    };

    // Every thirty seconds: BULK loses 600 packets from its queue (20/s),
    // GUEST 3,000 to its policer (100/s), VOICE none. `congested` of the
    // 30-second steps were like that.
    let at_step = |lab: &Lab, step: u64, congested: u64| {
        report(
            lab,
            100_000 + step * 30,
            &listing(
                (1_000 + step * 1_000, 0),
                (50_000 + step * 6_000, 400 + congested * 600),
                (9_000 + step * 6_000, 3_000 + step * 3_000),
            ),
        );
    };
    at_step(&lab, 0, 0);
    let baseline = poll(&lab, &id).await;
    crate::rules::evaluate(&lab.app, &baseline).await;

    // The first measured interval is over the threshold at 30 s; 300 s of
    // it have to be seen before anybody is told.
    for step in 1..=10 {
        after(30).await;
        at_step(&lab, step, step);
        let poll = poll(&lab, &id).await;
        assert_eq!(poll.measure("qos.queue_drop_ps", BULK), Some(20.0));
        assert_eq!(poll.measure("qos.drop_ps", GUEST), Some(100.0));
        crate::rules::evaluate(&lab.app, &poll).await;
        assert!(open(&lab).is_empty(), "not yet: {:?}", open(&lab));
    }
    after(30).await;
    at_step(&lab, 11, 11);
    let poll = poll(&lab, &id).await;
    crate::rules::evaluate(&lab.app, &poll).await;

    let alerts = open(&lab);
    assert_eq!(alerts.len(), 1, "one class is congested: {alerts:?}");
    let alert = &alerts[0];
    assert_eq!(alert.instance.as_deref(), Some(BULK));
    assert_eq!(alert.severity, crate::model::Severity::Medium);
    assert_eq!(alert.value, Some(20.0));
    assert_eq!(alert.device_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        alert.title,
        format!("QoS class is dropping (congestion) — {BULK} on EDGE")
    );
    // The same path as every other alert: it is about a lab device, so it
    // is recorded and shown, and its notice is held back.
    assert!(alert.lab);
    assert_eq!(alert.notified_at, None);

    // The congestion ends: under the clearing value, the alert resolves.
    after(30).await;
    at_step(&lab, 12, 11);
    let quiet = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    assert_eq!(quiet.measure("qos.queue_drop_ps", BULK), Some(0.0));
    assert_eq!(quiet.measure("qos.drop_ps", GUEST), Some(100.0));
    crate::rules::evaluate(&lab.app, &quiet).await;
    assert!(open(&lab).is_empty());
    assert!(
        crate::rules::list(&lab.app)
            .unwrap()
            .iter()
            .all(|rule| !matches!(&rule.condition,
                crate::model::Condition::Threshold { metric, .. }
                    if rule.builtin && metric.starts_with("qos.") && metric != "qos.queue_drop_ps")),
        "no built-in rule on a class's policer drops, nor on all its drops"
    );
}

#[tokio::test(start_paused = true)]
async fn a_poll_between_two_qos_reads_neither_fires_nor_resolves_the_rule() {
    // QoS is read every thirty seconds here and the device polled every
    // ten: two polls in three carry no QoS measure, and say nothing.
    let (lab, id) = edge().await;
    lab.app
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.qos_class_dropping", "scope": {"include_lab": true},
                   "for_s": 0}),
        )
        .await
        .unwrap();
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    let first = poll(&lab, &id).await;
    crate::rules::evaluate(&lab.app, &first).await;
    after(30).await;
    report(
        &lab,
        100_030,
        &listing((2_000, 0), (56_000, 1_000), (9_000, 3_000)),
    );
    let dropping = poll(&lab, &id).await;
    crate::rules::evaluate(&lab.app, &dropping).await;
    let open = || {
        crate::alerts::list(
            &lab.app,
            &crate::alerts::AlertQuery {
                state: Some("open".into()),
                ..crate::alerts::AlertQuery::default()
            },
        )
        .unwrap()
        .len()
    };
    assert_eq!(open(), 1);

    after(10).await;
    let between = poll(&lab, &id).await;
    assert_eq!(between.qos_read, None);
    crate::rules::evaluate(&lab.app, &between).await;
    assert_eq!(
        open(),
        1,
        "no QoS measure is not a class that stopped dropping"
    );
}

/// The built-in congestion rule for lab devices, firing at the first
/// measurement over its threshold.
async fn congestion_rule_at_once(lab: &Lab) {
    lab.app
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.qos_class_dropping", "scope": {"include_lab": true},
                   "for_s": 0}),
        )
        .await
        .unwrap();
}

/// The built-in congestion rule's alerts in one state (`open`, `resolved`).
fn congestion_alerts(lab: &Lab, state: &str) -> Vec<crate::model::Alert> {
    crate::alerts::list(
        &lab.app,
        &crate::alerts::AlertQuery {
            state: Some(state.into()),
            limit: Some(100),
            ..crate::alerts::AlertQuery::default()
        },
    )
    .unwrap()
    .into_iter()
    .filter(|alert| alert.rule_id.as_deref() == Some("builtin.qos_class_dropping"))
    .collect()
}

/// Poll, and let the rules judge the poll, as the scheduler does.
async fn poll_and_judge(lab: &Lab, id: &str) -> Poll {
    let poll = poll(lab, id).await;
    crate::rules::evaluate(&lab.app, &poll).await;
    poll
}

#[tokio::test(start_paused = true)]
async fn a_counter_cleared_on_a_congested_class_leaves_its_alert_open() {
    let (lab, id) = edge().await;
    congestion_rule_at_once(&lab).await;
    let mut events = lab.app.subscribe();
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll_and_judge(&lab, &id).await;
    after(30).await;
    report(
        &lab,
        100_030,
        &listing((2_000, 0), (56_000, 1_000), (12_000, 4_000)),
    );
    poll_and_judge(&lab, &id).await;
    let open = congestion_alerts(&lab, "open");
    assert_eq!(
        open.iter()
            .map(|alert| alert.instance.as_deref())
            .collect::<Vec<_>>(),
        [Some(BULK)],
        "20 congestion drops a second"
    );

    // `clear counters` for BULK's queue while VOICE and GUEST count on:
    // what BULK dropped in this interval is not known, which is not the
    // same as its having stopped.
    after(30).await;
    report(
        &lab,
        100_060,
        &listing((3_000, 0), (900, 12), (15_000, 5_000)),
    );
    let cleared = poll_and_judge(&lab, &id).await;
    assert_eq!(cleared.measure("qos.queue_drop_ps", BULK), None);
    assert_eq!(cleared.measure("qos.queue_drop_ps", VOICE), Some(0.0));
    assert_eq!(
        congestion_alerts(&lab, "open")
            .iter()
            .map(|alert| alert.id.as_str())
            .collect::<Vec<_>>(),
        [open[0].id.as_str()],
        "a gap is not an all-clear"
    );

    // Measured again and still congested: the same alert, seen again, not
    // a resolve and a new alert five minutes later.
    after(30).await;
    report(
        &lab,
        100_090,
        &listing((4_000, 0), (6_900, 612), (18_000, 6_000)),
    );
    poll_and_judge(&lab, &id).await;
    let again = congestion_alerts(&lab, "open");
    assert_eq!(again.len(), 1);
    assert_eq!(
        (again[0].id.as_str(), again[0].count),
        (open[0].id.as_str(), 2)
    );
    assert!(congestion_alerts(&lab, "resolved").is_empty());

    // A lab's alert is recorded and told to nobody, at every step of it.
    assert!(again[0].lab);
    assert_eq!(again[0].notified_at, None);
    let changes: Vec<Value> = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.topic == "alerts.changed")
        .map(|event| event.payload["notification"].clone())
        .collect();
    assert!(!changes.is_empty());
    assert!(
        changes.iter().all(Value::is_null),
        "the desktop shell is not told either: {changes:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_policy_taken_off_the_device_takes_its_alerts_with_it() {
    let (lab, id) = edge().await;
    congestion_rule_at_once(&lab).await;
    let congested = |lab: &Lab, uptime_s: u64, step: u64| {
        report(
            lab,
            uptime_s,
            &listing(
                (1_000 + step * 1_000, 0),
                (50_000 + step * 6_000, 400 + step * 600),
                (9_000, 3_000),
            ),
        );
    };
    congested(&lab, 100_000, 0);
    poll_and_judge(&lab, &id).await;
    after(30).await;
    congested(&lab, 100_030, 1);
    poll_and_judge(&lab, &id).await;
    assert_eq!(congestion_alerts(&lab, "open").len(), 1);

    // The policy is detached: the device answers with no policy at all. The
    // class is not congested any more, because it is not there.
    after(30).await;
    report(&lab, 100_060, "");
    let bare = poll_and_judge(&lab, &id).await;
    assert_eq!(bare.qos_read, Some(true));
    assert!(congestion_alerts(&lab, "open").is_empty());
    assert_eq!(congestion_alerts(&lab, "resolved").len(), 1);

    // Attached again and congested again: a new alert…
    after(30).await;
    congested(&lab, 100_090, 0);
    poll_and_judge(&lab, &id).await;
    after(30).await;
    congested(&lab, 100_120, 1);
    poll_and_judge(&lab, &id).await;
    assert_eq!(congestion_alerts(&lab, "open").len(), 1);

    // …and a device whose type no longer has the read loses it the same
    // way: its classes are deleted, and nothing is left to be congested.
    lab.app
        .rpc("devices.update", json!({"id": id, "profile": "test_ios"}))
        .await
        .unwrap();
    after(30).await;
    poll_and_judge(&lab, &id).await;
    assert!(congestion_alerts(&lab, "open").is_empty());
    assert_eq!(congestion_alerts(&lab, "resolved").len(), 2);
}

// ------------------------------------------------------- one counter for drops

/// A platform that has queues instead of policies, names them freely, and
/// keeps one drop counter per queue: what RouterOS is like, without being
/// RouterOS. Its listing is one queue a line, fields separated by `;`.
const QUEUE_PROFILE: &str = r#"
id: test_queues
name: Test queues
vendor: Test
os: QueueOS
interface_names: verbatim
cli:
  prompt: '^(?P<host>[\w.\-]{1,63})(\([\w\-/. ]+\))?[>#]\s?$'
  user_prompt: '^[\w.\-]{1,63}>\s?$'
  enable: {}
  init: ['terminal length 0']
  errors: ['^% ']
policy:
  read_only: ['^show ', '^terminal (length|width) ']
collect:
  health:
    - command: show system uptime
      parser: {regex: 'uptime (?P<uptime>\d+) seconds'}
      map: {uptime_s: uptime}
  qos:
    - command: show queues
      parser:
        regex: '(?m)^(?P<interface>[^;\r\n]*);(?P<way>[^;\r\n]+);(?P<name>[^;\r\n]+);(?P<packets>\d+);(?P<dropped>\d+)\s*$'
      map:
        interface: interface
        direction: way
        policy: "'queue simple'"
        class: name
        packets: packets
        bytes: packets | int * 100
        dropped_packets: dropped
"#;

#[tokio::test(start_paused = true)]
async fn a_platform_with_one_drop_counter_has_no_congestion_series_and_awkward_names_stay_whole() {
    let (lab, id) = edge().await;
    nocturne_app::profiles::save(&lab.app, QUEUE_PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_queues"}),
        )
        .await
        .unwrap();
    // SYNTHETIC. A queue on an interface whose name holds a pipe, with a
    // slash in its own; a queue on addresses, which has no interface; and
    // one line that does not say which way it faces.
    let queues = |gold: (u64, u64), office: (u64, u64)| {
        format!(
            "ether1|wan;out;gold/silver;{};{}\n;in;office;{};{}\nether2;sideways;lost;5;0\n",
            gold.0, gold.1, office.0, office.1
        )
    };
    let report = |uptime_s: u64, listing: String| {
        lab.fake("EDGE").with(|state| {
            state.outputs.insert(
                "show system uptime".into(),
                format!("Device uptime {uptime_s} seconds\n"),
            );
            state.outputs.insert("show queues".into(), listing);
        });
    };
    report(100_000, queues((10_000, 100), (500, 0)));
    poll(&lab, &id).await;
    after(30).await;
    report(100_030, queues((13_000, 400), (800, 0)));
    let second = poll(&lab, &id).await;

    let page = qos(&lab, json!({"device_id": id})).await;
    // The line with no direction is not filed under a guess. It is left
    // out, and the device's coverage says so in the collector's words.
    assert_eq!(page["rows"].as_array().unwrap().len(), 2);
    let coverage = &page["coverage"]["devices"][0];
    assert_eq!(coverage["state"], "ok");
    assert_eq!(
        coverage["unverified"], false,
        "this profile lists no caveat"
    );
    assert_eq!(coverage["classes"], 2);
    let left_out = coverage["error"].as_str().expect("what was left out");
    assert!(
        left_out.contains("1 of 3 rows") && left_out.contains("sideways"),
        "{left_out}"
    );

    // One drop counter: every drop is in `drop_ps`, nothing says which were
    // congestion, and no figure is made up for it.
    let gold = row(&page, "gold/silver");
    assert_eq!(gold["drop_ps"], 10.0);
    assert_eq!(gold["drop_pct"], 10.0);
    assert_eq!(gold["queue_drop_ps"], Value::Null);
    assert!(
        !second
            .measures
            .iter()
            .any(|measure| measure.metric == "qos.queue_drop_ps"),
        "{:?}",
        second.measures
    );
    // With nothing to tell the two apart, a class that drops is shown as
    // dropping: it may be congested, and hiding it would be the guess.
    let dropping = qos(&lab, json!({"device_id": id, "problem_only": true})).await;
    assert_eq!(dropping["rows"].as_array().unwrap().len(), 1);
    assert_eq!(dropping["rows"][0]["class"], "gold/silver");

    // The pipe in the interface and the slash in the queue are written so
    // that the instance still splits where it should, and it names the
    // series.
    assert_eq!(gold["interface"], "ether1|wan");
    assert_eq!(
        gold["instance"],
        "ether1%7Cwan|out|queue simple/gold%2Fsilver"
    );
    assert_eq!(
        crate::qos::parse_instance(gold["instance"].as_str().unwrap()),
        Some((
            "ether1|wan".to_string(),
            nocturne_drivers::model::Direction::Out,
            "queue simple".to_string(),
            "gold/silver".to_string()
        ))
    );
    assert_eq!(
        samples(&lab, &id, "qos.drop_ps", gold["instance"].as_str().unwrap()),
        [10.0]
    );
    let office = row(&page, "office");
    assert_eq!(office["interface"], "");
    assert_eq!(office["instance"], "|in|queue simple/office");
    assert_eq!(office["bps"], 300.0 * 100.0 / 30.0 * 8.0);

    // Asked for by interface: the one with the pipe, and the queues that
    // are on no interface at all.
    let on = |interface: &'static str| {
        let lab = &lab;
        let id = id.clone();
        async move {
            qos(lab, json!({"device_id": id, "interface": interface})).await["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["class"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(on("ether1|wan").await, ["gold/silver"]);
    assert_eq!(on("").await, ["office"]);
    assert!(on("ether1").await.is_empty());
}

/// [`QUEUE_PROFILE`] with the policy read off the listing too, names that
/// may be empty, and its one drop counter taken for congestion, so that the
/// built-in rule judges what it reads.
const FREE_TEXT_PROFILE: &str = r#"
id: test_free_text_queues
name: Test free-text queues
vendor: Test
os: QueueOS
interface_names: verbatim
cli:
  prompt: '^(?P<host>[\w.\-]{1,63})(\([\w\-/. ]+\))?[>#]\s?$'
  user_prompt: '^[\w.\-]{1,63}>\s?$'
  enable: {}
  init: ['terminal length 0']
  errors: ['^% ']
policy:
  read_only: ['^show ', '^terminal (length|width) ']
collect:
  health:
    - command: show system uptime
      parser: {regex: 'uptime (?P<uptime>\d+) seconds'}
      map: {uptime_s: uptime}
  qos:
    - command: show queues
      parser:
        regex: '(?m)^(?P<interface>[^;\r\n]*);(?P<way>in|out);(?P<policy>[^;\r\n]*);(?P<name>[^;\r\n]*);(?P<packets>\d+);(?P<dropped>\d+)\s*$'
      map:
        interface: interface
        direction: way
        policy: policy
        class: name
        packets: packets
        bytes: packets | int * 100
        dropped_packets: dropped
        queue_dropped_packets: dropped
"#;

#[tokio::test(start_paused = true)]
async fn hostile_names_stay_whole_and_cost_the_rules_nothing() {
    // Names are what a device says they are: a RouterOS queue is named by
    // whoever configured it, and a dynamic one after a subscriber's login.
    let (lab, id) = edge().await;
    nocturne_app::profiles::save(&lab.app, FREE_TEXT_PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_free_text_queues"}),
        )
        .await
        .unwrap();
    congestion_rule_at_once(&lab).await;
    let long_policy = "p".repeat(10_000);
    let longer_queue = "x".repeat(100_000);
    // (interface, policy, class); every one of them drops to congestion.
    let named: [(&str, &str, &str); 6] = [
        ("a|b/c", "a|b/c", "a|b/c"),
        ("ether1", "P", ""),
        ("ether1", &long_policy, "VOICE"),
        ("ether1", "P", &longer_queue),
        ("ether1", "a/b", "c"),
        ("ether1", "a", "b/c"),
    ];
    // SYNTHETIC, in FREE_TEXT_PROFILE's layout.
    let report = |uptime_s: u64, step: u64| {
        let listing: String = named
            .iter()
            .map(|(interface, policy, class)| {
                format!(
                    "{interface};out;{policy};{class};{};{}\n",
                    1_000 + step * 3_000,
                    step * 600
                )
            })
            .collect();
        lab.fake("EDGE").with(|state| {
            state.outputs.insert(
                "show system uptime".into(),
                format!("Device uptime {uptime_s} seconds\n"),
            );
            state.outputs.insert("show queues".into(), listing);
        });
    };
    report(100_000, 0);
    poll_and_judge(&lab, &id).await;
    after(30).await;
    report(100_030, 1);
    poll_and_judge(&lab, &id).await;

    let page = qos(&lab, json!({"device_id": id})).await;
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(rows.len(), named.len(), "every class, none merged");
    let mut instances = std::collections::HashSet::new();
    for (interface, policy, class) in named {
        let row = rows
            .iter()
            .find(|row| {
                row["interface"] == interface && row["policy"] == policy && row["class"] == class
            })
            .unwrap_or_else(|| panic!("no row for ({interface:.20}, {policy:.20}, {class:.20})"));
        // The names come back whole, and the instance spells exactly them.
        let instance = row["instance"].as_str().expect("an instance").to_string();
        assert_eq!(
            crate::qos::parse_instance(&instance),
            Some((
                interface.to_string(),
                nocturne_drivers::model::Direction::Out,
                policy.to_string(),
                class.to_string()
            )),
            "{instance:.80}"
        );
        assert!(instances.insert(instance.clone()), "{instance:.80} twice");
        // Its series is under that instance, with the rate the counters make.
        assert_eq!(
            samples(&lab, &id, "qos.queue_drop_ps", &instance),
            [20.0],
            "{instance:.80}"
        );
    }
    // The rule judged every one of them, and its alerts name the instance
    // the table gives.
    let alerted: std::collections::HashSet<String> = congestion_alerts(&lab, "open")
        .into_iter()
        .filter_map(|alert| alert.instance)
        .collect();
    assert_eq!(alerted, instances);
    // Found by the interface they name, and paged through one at a time.
    assert_eq!(
        classes_of(&qos(&lab, json!({"device_id": id, "interface": "a|b/c"})).await),
        ["a|b/c"]
    );
    let mut paged = 0;
    let mut cursor: Option<String> = None;
    loop {
        let mut params = json!({"device_id": id, "limit": 1});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let page = qos(&lab, params).await;
        paged += page["rows"].as_array().unwrap().len();
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(paged, named.len());
}

#[tokio::test(start_paused = true)]
async fn a_router_with_a_queue_per_subscriber_keeps_them_all_and_charts_the_first_256() {
    // 3,000 queues on one device, as a RouterOS box at an ISP has: the
    // default cap, the whole table, and a way through it a page at a time.
    let (lab, id) = edge().await;
    nocturne_app::profiles::save(&lab.app, QUEUE_PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_queues"}),
        )
        .await
        .unwrap();
    // SYNTHETIC. Six subscribers lose packets, spread over the four ports.
    let queues = |step: u64| -> String {
        (0..3_000u64)
            .map(|n| {
                let dropped = if n % 501 == 7 { step * 60 } else { 0 };
                format!(
                    "ether{};out;sub-{n:04};{};{dropped}\n",
                    n % 4 + 1,
                    1_000 + step * 300
                )
            })
            .collect()
    };
    let report = |uptime_s: u64, listing: String| {
        lab.fake("EDGE").with(|state| {
            state.outputs.insert(
                "show system uptime".into(),
                format!("Device uptime {uptime_s} seconds\n"),
            );
            state.outputs.insert("show queues".into(), listing);
        });
    };
    report(100_000, queues(0));
    poll(&lab, &id).await;
    after(30).await;
    report(100_030, queues(1));
    let second = poll(&lab, &id).await;
    assert_eq!(second.qos.len(), 3_000);

    let page = qos(&lab, json!({"device_id": id, "limit": 1})).await;
    assert_eq!(
        page["coverage"]["devices"][0],
        json!({"device_id": id, "state": "ok", "at": page["rows"][0]["at"], "unverified": false,
               "series_capped": true, "error": null, "classes": 3_000, "series": 256})
    );
    // Five series a class at most, for 256 classes: what the rules and the
    // charts have to carry for this device, whatever it has configured.
    let written: std::collections::BTreeSet<&str> = second
        .measures
        .iter()
        .filter(|measure| measure.metric.starts_with("qos."))
        .map(|measure| measure.instance.as_str())
        .collect();
    assert_eq!(written.len(), 256);
    // The first 256 in the stable order are all on ether1 (750 queues).
    assert!(
        written
            .iter()
            .all(|instance| instance.starts_with("ether1|out|"))
    );

    // Every class is in the table, rates included, reachable page by page
    // with no row twice and none missed.
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut params = json!({"limit": 1_000});
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let page = qos(&lab, params).await;
        pages += 1;
        for row in page["rows"].as_array().unwrap() {
            assert_eq!(row["bps"], 8_000.0, "{row}");
            assert!(
                seen.insert(row["class"].as_str().unwrap().to_string()),
                "{} twice",
                row["class"]
            );
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!((pages, seen.len()), (3, 3_000));

    // And the six that drop are found without reading the other 2,994,
    // although only one of them is among the charted.
    let dropping = qos(&lab, json!({"problem_only": true})).await;
    let names: Vec<&str> = dropping["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["class"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "sub-0508", "sub-2512", "sub-1009", "sub-1510", "sub-0007", "sub-2011"
        ],
        "by interface, then by name"
    );
    assert_eq!(
        dropping["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| !row["instance"].is_null())
            .count(),
        1
    );
}

/// The classes of a `monitor.qos` page, in its order.
fn classes_of(page: &Value) -> Vec<String> {
    page["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["class"].as_str().expect("a class").to_string())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_read_between_two_pages_neither_repeats_nor_skips_a_class() {
    // 5,000 subscriber queues, read 1,000 at a time. While the reader is on
    // its first page the device is read again, and in the meantime
    // subscribers came and went on both sides of where it stopped: on a
    // router with a queue per subscriber that is every read, not a corner.
    let (lab, id) = edge().await;
    nocturne_app::profiles::save(&lab.app, QUEUE_PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_queues"}),
        )
        .await
        .unwrap();
    // SYNTHETIC, in QUEUE_PROFILE's layout.
    let report = |uptime_s: u64, names: &[String]| {
        let listing: String = names
            .iter()
            .map(|name| format!("ether1;out;{name};1000;0\n"))
            .collect();
        lab.fake("EDGE").with(|state| {
            state.outputs.insert(
                "show system uptime".into(),
                format!("Device uptime {uptime_s} seconds\n"),
            );
            state.outputs.insert("show queues".into(), listing);
        });
    };
    let before: Vec<String> = (0..5_000).map(|n| format!("sub-{n:04}")).collect();
    report(100_000, &before);
    poll(&lab, &id).await;

    let first = qos(&lab, json!({"device_id": id, "limit": 1_000})).await;
    let coverage = &first["coverage"]["devices"][0];
    assert_eq!(
        (
            &coverage["classes"],
            &coverage["series"],
            &coverage["series_capped"]
        ),
        (&json!(5_000), &json!(256), &json!(true)),
        "the table holds all of them, the series the first 256, and it says so"
    );
    let mut seen = classes_of(&first);
    assert_eq!(seen.last().map(String::as_str), Some("sub-0999"));
    let mut cursor = first["next_cursor"]
        .as_str()
        .expect("more pages")
        .to_string();

    // Ten leave and one arrives before the reader's place; one leaves and
    // one arrives after it.
    let mut now: Vec<String> = before
        .iter()
        .filter(|name| !(100..110).any(|n| **name == format!("sub-{n:04}")))
        .filter(|name| name.as_str() != "sub-3000")
        .cloned()
        .collect();
    now.push("sub-0050a".into());
    now.push("sub-2500a".into());
    after(30).await;
    report(100_030, &now);
    poll(&lab, &id).await;

    loop {
        let page = qos(
            &lab,
            json!({"device_id": id, "limit": 1_000, "cursor": cursor}),
        )
        .await;
        seen.extend(classes_of(&page));
        match page["next_cursor"].as_str() {
            Some(next) => cursor = next.to_string(),
            None => break,
        }
    }
    let mut counted: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for name in &seen {
        *counted.entry(name.as_str()).or_default() += 1;
    }
    let twice: Vec<&&str> = counted
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(name, _)| name)
        .collect();
    assert!(twice.is_empty(), "read twice: {twice:?}");
    let still_there: std::collections::HashSet<&str> = now.iter().map(String::as_str).collect();
    let skipped: Vec<&str> = before
        .iter()
        .map(String::as_str)
        .filter(|name| still_there.contains(name) && !counted.contains_key(name))
        .collect();
    assert!(skipped.is_empty(), "never read: {skipped:?}");
    assert!(
        counted.contains_key("sub-2500a"),
        "it arrived ahead of the reader, so the reader gets to it"
    );
    assert!(
        !counted.contains_key("sub-3000"),
        "it left before the reader got there"
    );
}

// ------------------------------------------------- a device polled over SNMP

/// EDGE with an SNMP agent answering for it: a device whose interfaces come
/// from SNMP and whose classes only the command line has. It is still a lab
/// device (the fake behind its command line is the simulator's hook), so its
/// CLI fallback runs on the lab cadence; [`qos_alone_due`] puts the two
/// cadences out of step, as a device on the real network has them at every
/// QoS read.
async fn edge_with_snmp() -> (Lab, String, Arc<FakeSnmp>) {
    let snmp = Arc::new(FakeSnmp::default());
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        Arc::new(VirtualClock::new(epoch())),
        Arc::new(FakeProbe::default()),
        snmp.clone(),
    ));
    let lab = nocturne_app::testing::lab(builder, &["EDGE"]).await;
    nocturne_app::profiles::save(&lab.app, QOS_PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "snmp_version": "v2c", "snmp_community": "public"}),
        )
        .await
        .unwrap();
    snmp.set(
        "10.0.0.1:161",
        crate::testing::AgentData {
            uptime_ticks: Some(10_000_000),
            interfaces: vec![crate::testing::AgentIf::up(UPLINK, 1_000)],
            ..crate::testing::AgentData::default()
        },
    );
    let id = lab.id("EDGE");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_ios_qos", "address": "10.0.0.1",
                   "monitor": {"enabled": true, "icmp": false, "snmp": true, "interval_s": 10}}),
        )
        .await
        .unwrap();
    (lab, id, snmp)
}

/// Make the next poll one at which QoS is due and the CLI fallback is not.
fn qos_alone_due(lab: &Lab, id: &str) {
    let state = lab.app.state::<MonitorState>();
    let now = state.monotonic();
    state.device(id, |runtime| {
        runtime.last_cli_mono = Some(now);
        runtime.last_qos_mono = Some(now - 30_000);
    });
}

/// The `show` commands EDGE was sent since the last look.
fn shown(lab: &Lab) -> Vec<String> {
    let sent = lab.fake("EDGE").seen();
    lab.fake("EDGE").clear_seen();
    sent.into_iter()
        .filter(|command| command.starts_with("show "))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_snmp_polled_device_is_asked_for_its_classes_alone_and_timed_by_its_sysuptime() {
    let (lab, id, snmp) = edge_with_snmp().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    let first = poll(&lab, &id).await;
    assert_eq!(
        first.sources.snmp.ok,
        Some(true),
        "{:?}",
        first.sources.snmp.error
    );
    assert_eq!(first.qos_read, Some(true));
    shown(&lab);

    // QoS is due and the CLI fallback is not. SNMP has just said what the
    // device's uptime is, so the session asks for the classes and nothing
    // else: no `show version` beside it, every five minutes, on every router.
    after(10).await;
    qos_alone_due(&lab, &id);
    snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(10_001_000)
    });
    report(
        &lab,
        100_010,
        &listing((2_000, 0), (52_000, 500), (10_000, 3_200)),
    );
    let second = poll(&lab, &id).await;
    assert_eq!(second.qos_read, Some(true));
    assert_eq!(second.sources.cli.at, None, "the CLI fallback was not due");
    assert_eq!(shown(&lab), ["show policy-map interface"]);
    // The read before was timed by the session's own uptime and this one by
    // sysUpTime: two clocks, so no interval between them.
    assert_eq!(second.measure("qos.bps", VOICE), None);

    // Thirty seconds on by the poller's clock and forty by the agent's: the
    // rates follow the device.
    after(30).await;
    qos_alone_due(&lab, &id);
    snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(10_005_000)
    });
    report(
        &lab,
        100_040,
        &listing((4_000, 0), (60_000, 900), (14_000, 4_200)),
    );
    let third = poll(&lab, &id).await;
    assert_eq!(shown(&lab), ["show policy-map interface"]);
    assert_eq!(
        third.measure("qos.bps", VOICE),
        Some(2_000.0 * 100.0 / 40.0 * 8.0)
    );
    assert_eq!(third.measure("qos.queue_drop_ps", BULK), Some(400.0 / 40.0));
    assert_eq!(third.measure("qos.drop_ps", GUEST), Some(1_000.0 / 40.0));
}

#[tokio::test(start_paused = true)]
async fn a_device_busy_with_another_session_does_not_hold_up_its_snmp_poll() {
    let (lab, id, _snmp) = edge_with_snmp().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;

    // A change, a backup or somebody's collection holds the device's one
    // automated session, and the QoS read falls due in the middle of it.
    let held =
        nocturne_app::connect::cli(&lab.app, &id, nocturne_app::connect::CliOptions::default())
            .await
            .expect("a session of its own");
    after(10).await;
    qos_alone_due(&lab, &id);
    let started = tokio::time::Instant::now();
    let busy = poll(&lab, &id).await;
    let waited = started.elapsed();
    assert!(
        waited <= Duration::from_secs(10),
        "the SNMP poll waited {waited:?} for a session it only wanted for QoS"
    );
    assert!(busy.ok);
    assert_eq!(busy.sources.snmp.ok, Some(true));
    assert_eq!(busy.qos_read, Some(false));
    let coverage = qos(&lab, json!({"device_id": id})).await["coverage"]["devices"][0].clone();
    assert_eq!(coverage["state"], "failed");
    assert!(
        coverage["error"].as_str().unwrap().contains("busy"),
        "{coverage}"
    );
    held.close().await;

    // Free again: the next read that falls due is made, and is a baseline.
    after(30).await;
    qos_alone_due(&lab, &id);
    let back = poll(&lab, &id).await;
    assert_eq!(back.qos_read, Some(true));
    assert_eq!(back.measure("qos.bps", VOICE), None);
}

// ------------------------------------------------------ a device that is gone

#[tokio::test(start_paused = true)]
async fn a_device_that_stops_answering_is_not_asked_and_stops_claiming_its_rates() {
    // A router reached over SSH. Nothing listens at its address, so what it
    // "was last read as" is written down the way a read writes it; the poll
    // that follows is the real one.
    let harness = crate::testing::Harness::new().await;
    nocturne_app::profiles::save(&harness.app, QOS_PROFILE).expect("the profile loads");
    let id = harness
        .device(
            "WAN1",
            "10.7.0.1",
            json!({"transport": "ssh", "profile": "test_ios_qos"}),
        )
        .await;
    let classes: Vec<nocturne_drivers::model::QosClass> = vec![
        serde_json::from_value(json!({
            "interface": UPLINK, "direction": "out", "policy": "WAN", "class": "BULK",
            "packets": 50_000, "bytes": 5_000_000, "dropped_packets": 400,
            "queue_dropped_packets": 400,
        }))
        .unwrap(),
    ];
    let read = |uptime_s: u64, classes: &[nocturne_drivers::model::QosClass]| {
        let state = harness.app.state::<MonitorState>();
        let applied = state.device(&id, |runtime| {
            crate::qos::apply(
                &mut runtime.qos,
                &crate::qos::QosReading {
                    device_id: &id,
                    classes,
                    base: crate::rates::TimeBase::Seconds(uptime_s),
                    tick: uptime_s as i64 * 1_000,
                    at: harness.now(),
                    unverified: true,
                },
                256,
            )
        });
        crate::qos::Outcome::Read {
            applied,
            left_out: None,
            unverified: true,
        }
    };
    let mut record = crate::model::DeviceRecord {
        device_id: id.clone(),
        ..crate::model::DeviceRecord::default()
    };
    let settle = |record: &mut crate::model::DeviceRecord, outcome: &crate::qos::Outcome| {
        let changed =
            crate::qos::settle(&harness.app, &mut record.qos, &id, harness.now(), outcome).unwrap();
        harness
            .app
            .store()
            .put(crate::db::collections::DEVICES, &id, record)
            .unwrap();
        changed
    };
    settle(&mut record, &read(1_000, &classes));
    let mut later = classes.clone();
    later[0].packets = Some(56_000);
    later[0].dropped_packets = Some(1_000);
    later[0].queue_dropped_packets = Some(1_000);
    settle(&mut record, &read(1_031, &later));
    let page = harness.rpc("monitor.qos", json!({"device_id": id})).await;
    assert_eq!(page["rows"][0]["drop_ps"], 600.0 / 31.0);
    assert_eq!(page["coverage"]["devices"][0]["state"], "ok");

    // The router goes off the network. Its QoS read is due, and it is not
    // logged in to: it does not answer a ping.
    harness.probe.down("10.7.0.1");
    let mut events = harness.app.subscribe();
    let lost = harness.poll(&id).await;
    assert!(!lost.ok);
    assert_eq!(lost.qos_read, None, "not asked");
    let page = harness.rpc("monitor.qos", json!({"device_id": id})).await;
    let coverage = &page["coverage"]["devices"][0];
    assert_eq!(coverage["state"], "failed");
    assert!(
        coverage["error"]
            .as_str()
            .unwrap()
            .contains("was not answering"),
        "{coverage}"
    );
    assert_eq!(
        coverage["classes"], 1,
        "the class is still the last one known"
    );
    assert_eq!(page["rows"][0]["packets"], 56_000);
    assert_eq!(page["rows"][0]["drop_ps"], Value::Null);
    assert!(
        harness
            .rpc("monitor.qos", json!({"problem_only": true}))
            .await["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let announced = |events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>| {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| event.topic == "monitor.qos.updated")
            .count()
    };
    assert_eq!(announced(&mut events), 1);
    // And the counters it held are gone: whenever it is read again, that
    // read is a baseline.
    assert!(
        harness
            .app
            .state::<MonitorState>()
            .device(&id, |runtime| runtime.qos.classes.is_empty())
    );

    // It stays down. That is one change, said once, not one a poll.
    let at = coverage["at"].clone();
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.poll(&id).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.poll(&id).await;
    assert_eq!(announced(&mut events), 0);
    assert_eq!(
        harness.rpc("monitor.qos", json!({"device_id": id})).await["coverage"]["devices"][0]["at"],
        at
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_never_read_that_does_not_answer_stays_never_read() {
    let harness = crate::testing::Harness::new().await;
    nocturne_app::profiles::save(&harness.app, QOS_PROFILE).expect("the profile loads");
    let id = harness
        .device(
            "WAN2",
            "10.7.0.2",
            json!({"transport": "ssh", "profile": "test_ios_qos"}),
        )
        .await;
    harness.probe.down("10.7.0.2");
    let mut events = harness.app.subscribe();
    harness.poll(&id).await;
    let page = harness.rpc("monitor.qos", json!({"device_id": id})).await;
    assert_eq!(
        page["coverage"]["devices"][0]["state"], "never",
        "a read that was never made has not failed"
    );
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| event.topic == "monitor.qos.updated")
    );
}

#[tokio::test(start_paused = true)]
async fn a_deleted_device_s_classes_go_with_it() {
    let (lab, id) = edge().await;
    report(
        &lab,
        100_000,
        &listing((1_000, 0), (50_000, 400), (9_000, 3_000)),
    );
    poll(&lab, &id).await;
    assert_eq!(
        qos(&lab, json!({})).await["rows"].as_array().unwrap().len(),
        3
    );

    crate::scheduler::spawn(&lab.app);
    lab.app
        .rpc("devices.delete", json!({"ids": [id]}))
        .await
        .unwrap();
    // Forgotten when the feature hears of the deletion on the event bus.
    after(2).await;
    let page = qos(&lab, json!({})).await;
    assert_eq!(page["rows"], json!([]));
    assert_eq!(page["coverage"], json!({"devices": []}));
}

// --------------------------------------------------------------- the simulator

/// The commands the `cisco_ios` profile sends for interfaces, health and QoS.
const IOS_COMMANDS: [&str; 9] = [
    "show interfaces",
    "show interfaces switchport",
    "show ip interface",
    "show ipv6 interface brief",
    "show processes cpu",
    "show processes memory sorted",
    "show version",
    "show environment temperature",
    "show policy-map interface",
];

/// Copy what the simulated router prints now to the device the poll reads.
/// A command the simulator refuses is left out, and the fake refuses it too.
fn mirror(sim: &mut nocturne_netsim::Lab, node: nocturne_netsim::NodeId, lab: &Lab) {
    lab.fake("EDGE").with(|state| {
        for command in IOS_COMMANDS {
            match sim.exec(node, command) {
                Ok(output) => state.outputs.insert(command.to_string(), output),
                Err(_) => state.outputs.remove(command),
            };
        }
    });
}

/// `(packets, bytes)` classified into the `nth` class called `class` in the
/// simulator's listing: the line under its `Class-map:` line.
fn classified(listing: &str, class: &str, nth: usize) -> (u64, u64) {
    let header = format!("Class-map: {class} (");
    let line = listing
        .lines()
        .skip_while({
            let mut seen = 0;
            move |line| {
                let hit = line.trim_start().starts_with(&header);
                seen += usize::from(hit);
                !(hit && seen > nth)
            }
        })
        .nth(1)
        .unwrap_or_else(|| panic!("no class {class} #{nth} in:\n{listing}"));
    let numbers: Vec<u64> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap())
        .collect();
    (numbers[0], numbers[1])
}

#[tokio::test(start_paused = true)]
async fn a_lab_router_shaping_its_uplink_is_measured_class_by_class() {
    // The reference campus: EDGE shapes its Internet uplink to 6 Mbit/s,
    // serves voice (EF) first, and lets guests (policed and re-marked CS1
    // where they come in) give way. Its hosts send real flows through it.
    let mut sim = nocturne_netsim::Lab::from_spec(&nocturne_netsim::demo_campus())
        .expect("the reference campus builds");
    let node = sim.node_by_name("EDGE").expect("the edge router");
    // More than the uplink carries, so that a queue overflows: a second
    // staff download on top of what the campus already sends.
    sim.add_flow(nocturne_netsim::FlowSpec {
        name: "Staff backup to the cloud".into(),
        src: "Staff-PC1".into(),
        dst_ip: "8.8.8.8".into(),
        bps: 6_000_000,
        ..nocturne_netsim::FlowSpec::default()
    })
    .expect("the flow is accepted");
    sim.settle(60_000_000);

    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        Arc::new(VirtualClock::new(epoch())),
        Arc::new(FakeProbe::default()),
        Arc::new(FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["EDGE"]).await;
    let id = lab.id("EDGE");
    // The real profile, as a lab device has it.
    lab.app
        .rpc("devices.update", json!({"id": id, "profile": "cisco_ios"}))
        .await
        .unwrap();
    lab.app
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.qos_class_dropping", "scope": {"include_lab": true}}),
        )
        .await
        .unwrap();

    // Thirty seconds of simulated traffic between two polls, thirty seconds
    // on the poller's clock: a lab device's cadence.
    let mut listings = Vec::new();
    let mut polls = Vec::new();
    for round in 0..13 {
        if round > 0 {
            sim.advance(30_000_000);
            after(30).await;
        }
        mirror(&mut sim, node, &lab);
        listings.push(sim.exec(node, "show policy-map interface").unwrap());
        let poll = poll(&lab, &id).await;
        assert_eq!(poll.qos_read, Some(true), "round {round}: {:?}", poll.error);
        crate::rules::evaluate(&lab.app, &poll).await;
        polls.push(poll);
    }

    let page = qos(&lab, json!({"device_id": id})).await;
    let coverage = &page["coverage"]["devices"][0];
    assert_eq!(coverage["state"], "ok", "{coverage}");
    assert_eq!(
        coverage["unverified"], true,
        "no real IOS device has met this read"
    );
    assert_eq!(coverage["error"], Value::Null);
    let rows = page["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 8, "{page}");

    // The policy as a tree: the shaper's class, and the three classes of
    // the policy inside it.
    let of = |policy: &str, class: &str, interface: &str| -> &Value {
        rows.iter()
            .find(|row| {
                row["policy"] == policy && row["class"] == class && row["interface"] == interface
            })
            .unwrap_or_else(|| panic!("no {policy}/{class} on {interface} in {page}"))
    };
    let shaper = of("WAN-OUT", "class-default", UPLINK);
    assert_eq!(shaper["parent"], Value::Null);
    assert_eq!(shaper["rate_bps"], 6_000_000);
    assert_eq!(
        shaper["instance"],
        "GigabitEthernet0/0|out|WAN-OUT/class-default"
    );
    let voice = of("WAN-QUEUE", "VOICE", UPLINK);
    assert_eq!(voice["parent"], "class-default");
    assert_eq!(voice["instance"], "GigabitEthernet0/0|out|WAN-QUEUE/VOICE");

    // The rates are the simulator's own counters, subtracted and divided
    // by the thirty seconds between the last two polls. The simulated
    // router prints its uptime to the minute, as IOS does: two readings
    // thirty seconds apart are 0 or 60 s apart by that account, and the
    // rate is still the right one. Right to within two per cent, not to
    // the bit: the poller's clock also counts the fraction of a second a
    // login takes, which the simulated traffic here does not advance by.
    let (before, now) = (&listings[listings.len() - 2], &listings[listings.len() - 1]);
    let expected_bps = |class: &str, nth: usize| {
        let (_, earlier) = classified(before, class, nth);
        let (_, later) = classified(now, class, nth);
        (later - earlier) as f64 / 30.0 * 8.0
    };
    let close = |measured: &Value, expected: f64, what: &str| {
        let measured = measured
            .as_f64()
            .unwrap_or_else(|| panic!("{what} was not measured: {page}"));
        assert!(
            (measured - expected).abs() <= expected.abs() * 0.02 + 1.0,
            "{what}: measured {measured}, the simulator's counters say {expected}"
        );
    };
    // Voice is 256 kbit/s of EF and is served first: all of it, none lost.
    close(&voice["bps"], expected_bps("VOICE", 0), "voice traffic");
    close(&voice["bps"], 256_000.0, "voice traffic against the flow");
    assert_eq!(voice["drop_ps"], 0.0);
    assert_eq!(voice["queue_drop_ps"], 0.0);
    // The shaper's class is offered more than 6 Mbit/s and drops the rest
    // from its queues: congestion, counted as such.
    close(
        &shaper["bps"],
        expected_bps("class-default", 0),
        "uplink traffic",
    );
    assert!(shaper["bps"].as_f64().unwrap() > 6_000_000.0, "{shaper}");
    assert!(shaper["queue_drop_ps"].as_f64().unwrap() > 10.0, "{shaper}");
    assert!(shaper["drop_pct"].as_f64().unwrap() > 0.0);
    assert!(shaper["drop_pct"].as_f64().unwrap() < 100.0);
    // The guests' policer on the inside drops, and that is not congestion.
    let guest = of("LAN-IN", "GUEST", "GigabitEthernet0/2");
    assert_eq!(guest["direction"], "in");
    assert!(guest["drop_ps"].as_f64().unwrap() > 10.0, "{guest}");
    assert_eq!(guest["queue_drop_ps"], Value::Null);

    // Every poll after the first measured its interval: no gap every other
    // read, which is what an uptime printed to the minute used to make of
    // a thirty-second cadence.
    for (round, poll) in polls.iter().enumerate().skip(1) {
        assert!(
            poll.measure("qos.bps", "GigabitEthernet0/0|out|WAN-QUEUE/VOICE")
                .is_some(),
            "round {round} has no rate"
        );
    }
    assert_eq!(
        samples(
            &lab,
            &id,
            "qos.bps",
            "GigabitEthernet0/0|out|WAN-QUEUE/VOICE"
        )
        .len(),
        12
    );
    // The interface rates of the same polls come off the same session and
    // the same uptime, so they are whole too: the uplink carries what the
    // shaper lets through, at every poll and not at every other one.
    for (round, poll) in polls.iter().enumerate().skip(1) {
        let carried = poll
            .measure("if.out_bps", UPLINK)
            .unwrap_or_else(|| panic!("round {round} has no interface rate"));
        assert!(
            (5_500_000.0..=6_500_000.0).contains(&carried),
            "round {round}: {carried} bit/s through a 6 Mbit/s shaper"
        );
    }

    // "Dropping" lists the congested classes and leaves the policer out.
    let dropping = qos(&lab, json!({"device_id": id, "problem_only": true})).await;
    let names: Vec<String> = dropping["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            format!(
                "{}/{}",
                row["policy"].as_str().unwrap(),
                row["class"].as_str().unwrap()
            )
        })
        .collect();
    assert!(
        names.contains(&"WAN-OUT/class-default".to_string()),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.starts_with("LAN-IN/")),
        "{names:?}"
    );

    // And after five minutes of it the built-in rule has said so, about the
    // classes that are congested and not about the policer.
    let alerts: Vec<crate::model::Alert> = crate::alerts::list(
        &lab.app,
        &crate::alerts::AlertQuery {
            state: Some("open".into()),
            limit: Some(100),
            ..crate::alerts::AlertQuery::default()
        },
    )
    .unwrap();
    assert!(
        !alerts.is_empty(),
        "the uplink is congested and nobody was told"
    );
    for alert in &alerts {
        assert_eq!(alert.rule_id.as_deref(), Some("builtin.qos_class_dropping"));
        let instance = alert.instance.as_deref().unwrap();
        assert!(
            instance.starts_with("GigabitEthernet0/0|out|"),
            "{instance}"
        );
        assert!(alert.lab);
    }
    assert!(
        alerts.iter().any(|alert| alert.instance.as_deref()
            == Some("GigabitEthernet0/0|out|WAN-OUT/class-default")),
        "{alerts:?}"
    );
}
