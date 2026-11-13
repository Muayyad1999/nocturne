//! Each physical check as a table: the problem, the healthy case, the
//! look-alike that must stay quiet, and the case where the data does not
//! allow a verdict.

use nocturne_app::store::LogEntry;
use nocturne_drivers::model::{Counters, Interface};

use crate::settings::Settings;
use crate::testdata::*;
use crate::world::{DeviceData, World};

/// Two switches cabled together, each reporting the other.
fn pair(a: Interface, b: Interface) -> World {
    let (a_name, b_name) = (a.name.clone(), b.name.clone());
    world(vec![
        device("a", "SW1")
            .interfaces(vec![a])
            .neighbors(vec![neighbor(&a_name, "SW2", &b_name)]),
        device("b", "SW2")
            .interfaces(vec![b])
            .neighbors(vec![neighbor(&b_name, "SW1", &a_name)]),
    ])
}

// ----------------------------------------------------------------- duplex

#[test]
fn duplex_mismatch_table() {
    // Positive: resolved half against resolved full.
    let found = run(
        "duplex_mismatch",
        &pair(
            port("GigabitEthernet0/1", 100_000_000, "half"),
            port("GigabitEthernet0/2", 100_000_000, "a-full"),
        ),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    let finding = &found.findings[0];
    assert_eq!(finding.related[0].device_id, "b");
    assert!(finding.title.contains("half duplex"));
    assert!(
        finding
            .evidence
            .iter()
            .any(|item| item.value.contains("half")),
        "the evidence carries what each end reported"
    );

    // Negative: both ends agree, however they spell it.
    assert!(
        run(
            "duplex_mismatch",
            &pair(
                port("GigabitEthernet0/1", 1_000_000_000, "Full-duplex"),
                port("GigabitEthernet0/2", 1_000_000_000, "a-full"),
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: 10 Mb/s half duplex at both ends. Legacy equipment
    // agreeing is not a mismatch — and this is why the check compares the two
    // ends rather than looking for the word "half".
    assert!(
        run(
            "duplex_mismatch",
            &pair(
                port("Ethernet0/1", 10_000_000, "half"),
                port("Ethernet0/2", 10_000_000, "half"),
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: `auto` is not a verdict, so nothing is compared.
    let negotiating = run(
        "duplex_mismatch",
        &pair(
            port("GigabitEthernet0/1", 100_000_000, "auto"),
            port("GigabitEthernet0/2", 100_000_000, "full"),
        ),
    );
    assert!(negotiating.findings.is_empty());
    assert_eq!(
        negotiating.skips().count(),
        2,
        "both ends are not evaluated"
    );

    // Not enough data: no link at all.
    let alone = run(
        "duplex_mismatch",
        &world(vec![device("a", "SW1").interfaces(vec![port(
            "GigabitEthernet0/1",
            100_000_000,
            "half",
        )])]),
    );
    assert!(alone.findings.is_empty());
    assert_eq!(
        alone.skips().map(|(_, reason)| reason).collect::<Vec<_>>(),
        [crate::checks::NO_LINK]
    );
}

#[test]
fn duplex_mismatch_only_offers_a_fix_to_the_end_that_forces_it() {
    let mismatch = |a_config: &str, b_config: &str| -> crate::report::CheckReport {
        let a = port("GigabitEthernet0/1", 100_000_000, "half");
        let b = port("GigabitEthernet0/2", 100_000_000, "full");
        let world = world(vec![
            device("a", "SW1")
                .interfaces(vec![a])
                .neighbors(vec![neighbor(
                    "GigabitEthernet0/1",
                    "SW2",
                    "GigabitEthernet0/2",
                )])
                .config(a_config),
            device("b", "SW2")
                .interfaces(vec![b])
                .neighbors(vec![neighbor(
                    "GigabitEthernet0/2",
                    "SW1",
                    "GigabitEthernet0/1",
                )])
                .config(b_config),
        ]);
        run("duplex_mismatch", &world)
    };

    // The half end forces it: the fix belongs here.
    let ours = mismatch(
        "interface GigabitEthernet0/1\n duplex half\n",
        "interface GigabitEthernet0/2\n",
    );
    let suggestion = ours.findings[0].suggestion.as_ref().unwrap();
    assert_eq!(
        suggestion.config_lines.as_deref(),
        Some(
            [
                "interface GigabitEthernet0/1".to_string(),
                "duplex auto".to_string()
            ]
            .as_slice()
        )
    );

    // The far end forces it: no configuration lines, because applying them
    // here would not fix anything.
    let theirs = mismatch(
        "interface GigabitEthernet0/1\n",
        "interface GigabitEthernet0/2\n duplex full\n",
    );
    let suggestion = theirs.findings[0].suggestion.as_ref().unwrap();
    assert_eq!(suggestion.config_lines, None);
    assert!(suggestion.text.contains("SW2"));

    // Neither configuration explains it: still no lines.
    let neither = mismatch(
        "interface GigabitEthernet0/1\n",
        "interface GigabitEthernet0/2\n",
    );
    assert_eq!(
        neither.findings[0]
            .suggestion
            .as_ref()
            .unwrap()
            .config_lines,
        None
    );
}

#[test]
fn half_duplex_uplink_table() {
    let with_neighbour = |speed: u64, duplex: &str| {
        world(vec![
            device("a", "SW1")
                .interfaces(vec![port("GigabitEthernet0/1", speed, duplex)])
                .neighbors(vec![neighbor("GigabitEthernet0/1", "ap-3", "eth0")]),
        ])
    };

    // Positive: 100 Mb/s half duplex with something plugged in.
    let found = run("half_duplex_uplink", &with_neighbour(100_000_000, "a-half"));
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);

    // Negative: full duplex.
    assert!(
        run("half_duplex_uplink", &with_neighbour(1_000_000_000, "full"))
            .findings
            .is_empty()
    );

    // Look-alike 1: 10 Mb/s half duplex is what 10BASE-T does.
    assert!(
        run("half_duplex_uplink", &with_neighbour(10_000_000, "half"))
            .findings
            .is_empty()
    );

    // Look-alike 2: a port with no neighbour. A half-duplex printer is not a
    // network fault, and this check is about links.
    let no_neighbour = world(vec![
        device("a", "SW1")
            .interfaces(vec![port("GigabitEthernet0/1", 100_000_000, "half")])
            .neighbors(vec![]),
    ]);
    assert!(run("half_duplex_uplink", &no_neighbour).findings.is_empty());

    // Look-alike 3: the far end is managed and full duplex — that is a duplex
    // mismatch, reported once, by `duplex_mismatch`.
    let managed = pair(
        port("GigabitEthernet0/1", 100_000_000, "half"),
        port("GigabitEthernet0/2", 100_000_000, "full"),
    );
    assert!(run("half_duplex_uplink", &managed).findings.is_empty());
    assert_eq!(run("duplex_mismatch", &managed).findings.len(), 1);

    // Not enough data: no neighbours collected at all.
    let unknown = run(
        "half_duplex_uplink",
        &world(vec![device("a", "SW1").interfaces(vec![port(
            "GigabitEthernet0/1",
            100_000_000,
            "half",
        )])]),
    );
    assert!(unknown.findings.is_empty());
    assert_eq!(unknown.skips().count(), 1);
}

// ------------------------------------------------------------ speed and MTU

#[test]
fn speed_mismatch_table() {
    // Positive.
    let found = run(
        "speed_mismatch",
        &pair(
            port("GigabitEthernet0/1", 1_000_000_000, "full"),
            port("GigabitEthernet0/2", 100_000_000, "full"),
        ),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(found.findings[0].title.contains("1 Gb/s"));

    // Negative.
    assert!(
        run(
            "speed_mismatch",
            &pair(
                port("GigabitEthernet0/1", 1_000_000_000, "full"),
                port("GigabitEthernet0/2", 1_000_000_000, "full"),
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike: a down link. What a down port calls its speed is the
    // configured maximum, so the ends "disagreeing" means nothing.
    let mut down_a = port("GigabitEthernet0/1", 1_000_000_000, "full");
    down_a.oper_up = Some(false);
    let down = pair(down_a, port("GigabitEthernet0/2", 100_000_000, "full"));
    let report = run("speed_mismatch", &down);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);

    // Not enough data: one end reports no speed.
    let mut unknown = port("GigabitEthernet0/2", 0, "full");
    unknown.speed_bps = None;
    let report = run(
        "speed_mismatch",
        &pair(port("GigabitEthernet0/1", 1_000_000_000, "full"), unknown),
    );
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);
}

#[test]
fn mtu_mismatch_table() {
    let with_mtus = |a: Option<u64>, b: Option<u64>| {
        let mut one = port("GigabitEthernet0/1", 1_000_000_000, "full");
        let mut other = port("GigabitEthernet0/2", 1_000_000_000, "full");
        one.mtu = a;
        other.mtu = b;
        pair(one, other)
    };

    // Positive: a jumbo-frame end against a standard one.
    let found = run("mtu_mismatch", &with_mtus(Some(9216), Some(1500)));
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);

    // Negative: the same MTU.
    assert!(
        run("mtu_mismatch", &with_mtus(Some(1500), Some(1500)))
            .findings
            .is_empty()
    );

    // Look-alike: 1500 against 1514. The same MTU counted with and without
    // the Ethernet header and FCS — reporting it would be noise on every
    // mixed-vendor link in the estate.
    assert!(
        run("mtu_mismatch", &with_mtus(Some(1500), Some(1514)))
            .findings
            .is_empty()
    );
    assert!(
        run("mtu_mismatch", &with_mtus(Some(9216), Some(9198)))
            .findings
            .is_empty()
    );

    // Not enough data.
    let report = run("mtu_mismatch", &with_mtus(Some(1500), None));
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);
}

// --------------------------------------------------------- interface errors

fn with_counters(before: Counters, now: Counters, minutes: i64) -> World {
    let mut then = port("GigabitEthernet0/1", 1_000_000_000, "full");
    then.counters = before;
    let mut latest = port("GigabitEthernet0/1", 1_000_000_000, "full");
    latest.counters = now;
    world(vec![
        device("a", "SW1")
            .interfaces_at(vec![latest], NOW)
            .previous_interfaces(vec![then], NOW - minutes * 60_000),
    ])
}

#[test]
fn interface_errors_table() {
    // Positive: errors growing fast against the traffic carried.
    let found = run(
        "interface_errors",
        &with_counters(
            counters(100, 100, 1_000_000),
            counters(900, 900, 2_000_000),
            10,
        ),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(found.findings[0].title.contains("per million packets"));

    // Negative: counters that are not moving.
    assert!(
        run(
            "interface_errors",
            &with_counters(
                counters(5_000, 5_000, 1_000_000),
                counters(5_000, 5_000, 9_000_000),
                10
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: a large counter that is not growing. Every port that was
    // ever unplugged has errors since boot; only the difference counts.
    let since_boot = run(
        "interface_errors",
        &with_counters(
            counters(1_000_000, 900_000, 10),
            counters(1_000_000, 900_000, 5_000_000),
            10,
        ),
    );
    assert!(since_boot.findings.is_empty());
    assert_eq!(
        since_boot.evaluated().len(),
        1,
        "it was decided, not skipped"
    );

    // Look-alike 2: a handful of new errors on a very busy port.
    assert!(
        run(
            "interface_errors",
            &with_counters(counters(0, 0, 0), counters(20, 20, 100_000_000), 10)
        )
        .findings
        .is_empty()
    );

    // Look-alike 3: counters cleared — the delta is negative, so there is no
    // rate to read and the port is not evaluated.
    let cleared = run(
        "interface_errors",
        &with_counters(counters(5_000, 5_000, 9_000_000), counters(3, 3, 1_000), 10),
    );
    assert!(cleared.findings.is_empty());
    assert_eq!(cleared.skips().count(), 1);

    // Not enough data: only one collection.
    let once = run(
        "interface_errors",
        &world(vec![device("a", "SW1").interfaces(vec![port(
            "GigabitEthernet0/1",
            1_000_000_000,
            "full",
        )])]),
    );
    assert!(once.findings.is_empty());
    assert_eq!(once.skips().count(), 1);

    // Not enough data: collections seconds apart are not a rate.
    let mut latest = port("GigabitEthernet0/1", 1_000_000_000, "full");
    latest.counters = counters(900, 900, 2_000);
    let mut then = port("GigabitEthernet0/1", 1_000_000_000, "full");
    then.counters = counters(100, 100, 1_000);
    let too_close = run(
        "interface_errors",
        &world(vec![
            device("a", "SW1")
                .interfaces_at(vec![latest], NOW)
                .previous_interfaces(vec![then], NOW - 10_000),
        ]),
    );
    assert!(too_close.findings.is_empty());
    assert_eq!(too_close.skips().count(), 1);

    // With no packet counts the check falls back on errors a minute.
    let no_packets = run(
        "interface_errors",
        &with_counters(
            Counters {
                in_errors: Some(0),
                ..Counters::default()
            },
            Counters {
                in_errors: Some(600),
                ..Counters::default()
            },
            10,
        ),
    );
    assert_eq!(no_packets.findings.len(), 1);
    assert!(no_packets.findings[0].title.contains("a minute"));
}

// -------------------------------------------------------------- flapping

fn sampled(world: &mut World, device_id: &str, instance: &str, values: &[(i64, f64)]) {
    let series = world
        .store()
        .series_id(device_id, "if.oper", instance)
        .expect("series");
    let samples: Vec<(i64, i64, f64)> = values
        .iter()
        .map(|(at, value)| (series, *at, *value))
        .collect();
    world.store().samples_insert(&samples).expect("samples");
    world.reindex_series();
}

#[test]
fn interface_flapping_table() {
    let minute = 60_000;
    let mut flapping = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    sampled(
        &mut flapping,
        "a",
        "GigabitEthernet0/1",
        &[
            (NOW - 50 * minute, 1.0),
            (NOW - 40 * minute, 0.0),
            (NOW - 30 * minute, 1.0),
            (NOW - 20 * minute, 0.0),
            (NOW - 10 * minute, 1.0),
        ],
    );
    let found = run("interface_flapping", &flapping);
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(found.findings[0].title.contains("4 times"));

    // Negative: a port that stayed up.
    let mut steady = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    sampled(
        &mut steady,
        "a",
        "GigabitEthernet0/1",
        &[(NOW - 50 * minute, 1.0), (NOW - 10 * minute, 1.0)],
    );
    assert!(run("interface_flapping", &steady).findings.is_empty());

    // Look-alike: a port that flapped and has since been shut. That is
    // history, and somebody dealt with it.
    let mut shut = port("GigabitEthernet0/1", 1_000_000_000, "full");
    shut.admin_up = Some(false);
    shut.oper_up = Some(false);
    let mut dealt_with = world(vec![device("a", "SW1").interfaces(vec![shut])]);
    sampled(
        &mut dealt_with,
        "a",
        "GigabitEthernet0/1",
        &[
            (NOW - 50 * minute, 1.0),
            (NOW - 40 * minute, 0.0),
            (NOW - 30 * minute, 1.0),
            (NOW - 20 * minute, 0.0),
            (NOW - 10 * minute, 1.0),
        ],
    );
    assert!(run("interface_flapping", &dealt_with).findings.is_empty());

    // Not enough data: nothing watched the port, so it is not called stable.
    let unwatched = run(
        "interface_flapping",
        &world(vec![device("a", "SW1").interfaces(vec![port(
            "GigabitEthernet0/1",
            1_000_000_000,
            "full",
        )])]),
    );
    assert!(unwatched.findings.is_empty());
    assert_eq!(unwatched.skips().count(), 1);
}

#[test]
fn interface_flapping_falls_back_to_the_devices_own_log() {
    let logging = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/5",
        1_000_000_000,
        "full",
    )])]);
    // IOS reports one flap twice — `%LINK-3-UPDOWN` and
    // `%LINEPROTO-5-UPDOWN` — so what is counted is state *changes*, not log
    // lines. These ten entries are five changes of state.
    let mut at = NOW - 50 * 60_000;
    for state in ["down", "up", "down", "up", "down"] {
        for facility in [
            "%LINK-3-UPDOWN: Interface",
            "%LINEPROTO-5-UPDOWN: Line protocol on Interface",
        ] {
            logging
                .store()
                .log_append(&LogEntry {
                    id: 0,
                    ts: at,
                    stream: "syslog".into(),
                    device_id: Some("a".into()),
                    severity: Some(3),
                    source: "10.0.0.2".into(),
                    message: format!("{facility} GigabitEthernet0/5, changed state to {state}"),
                    body: None,
                })
                .expect("append");
            at += 1_000;
        }
        at += 60_000;
    }
    let found = run("interface_flapping", &logging);
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/5"]);
    assert!(
        found.findings[0].title.contains("4 times"),
        "ten log lines for five states are four changes, not ten: {}",
        found.findings[0].title
    );
    assert!(
        found.findings[0]
            .evidence
            .iter()
            .any(|item| item.value.contains("log entries")),
        "the evidence says where the count came from"
    );

    // Two log lines for one flap are one change of state, which is under the
    // threshold: a single flap is not a flapping port.
    let one_flap = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/5",
        1_000_000_000,
        "full",
    )])]);
    for (index, (facility, state)) in [
        ("%LINK-3-UPDOWN: Interface", "down"),
        ("%LINEPROTO-5-UPDOWN: Line protocol on Interface", "down"),
        ("%LINK-3-UPDOWN: Interface", "up"),
        ("%LINEPROTO-5-UPDOWN: Line protocol on Interface", "up"),
    ]
    .into_iter()
    .enumerate()
    {
        one_flap
            .store()
            .log_append(&LogEntry {
                id: 0,
                ts: NOW - 600_000 + index as i64 * 1_000,
                stream: "syslog".into(),
                device_id: Some("a".into()),
                severity: Some(3),
                source: "10.0.0.2".into(),
                message: format!("{facility} GigabitEthernet0/5, changed state to {state}"),
                body: None,
            })
            .expect("append");
    }
    let report = run("interface_flapping", &one_flap);
    assert!(report.findings.is_empty());
    assert_eq!(report.evaluated().len(), 1, "it was decided, not skipped");
}

// ------------------------------------------------------------ err-disabled

fn logged(world: &World, device_id: &str, at: i64, message: &str) {
    world
        .store()
        .log_append(&LogEntry {
            id: 0,
            ts: at,
            stream: "syslog".into(),
            device_id: Some(device_id.into()),
            severity: Some(4),
            source: "10.0.0.2".into(),
            message: message.into(),
            body: None,
        })
        .expect("append");
}

#[test]
fn err_disabled_table() {
    let down_port = |admin: bool, oper: bool| {
        let mut port = port("GigabitEthernet0/5", 100_000_000, "full");
        port.admin_up = Some(admin);
        port.oper_up = Some(oper);
        port
    };

    // Positive: the switch disabled it and it is still down.
    let disabled = world(vec![
        device("a", "SW1").interfaces(vec![down_port(true, false)]),
    ]);
    logged(
        &disabled,
        "a",
        NOW - 600_000,
        "%PM-4-ERR_DISABLE: bpduguard error detected on Gi0/5, putting Gi0/5 in err-disable state",
    );
    let found = run("err_disabled", &disabled);
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/5"]);
    assert!(
        found.findings[0]
            .evidence
            .iter()
            .any(|item| item.value.contains("bpduguard")),
        "the cause the platform printed is the evidence"
    );

    // Negative: the device logs, and nothing was disabled.
    let quiet = world(vec![
        device("a", "SW1").interfaces(vec![down_port(true, true)]),
    ]);
    logged(
        &quiet,
        "a",
        NOW - 600_000,
        "%SYS-5-CONFIG_I: Configured from console by admin",
    );
    let report = run("err_disabled", &quiet);
    assert!(report.findings.is_empty());
    assert_eq!(report.evaluated().len(), 1);

    // Look-alike 1: the port recovered.
    let recovered = world(vec![
        device("a", "SW1").interfaces(vec![down_port(true, true)]),
    ]);
    logged(
        &recovered,
        "a",
        NOW - 600_000,
        "%PM-4-ERR_DISABLE: psecure-violation error detected on Gi0/5, putting Gi0/5 in err-disable state",
    );
    assert!(run("err_disabled", &recovered).findings.is_empty());

    // Look-alike 2: somebody has since shut the port deliberately.
    let shut = world(vec![
        device("a", "SW1").interfaces(vec![down_port(false, false)]),
    ]);
    logged(
        &shut,
        "a",
        NOW - 600_000,
        "%PM-4-ERR_DISABLE: bpduguard error detected on Gi0/5, putting Gi0/5 in err-disable state",
    );
    assert!(run("err_disabled", &shut).findings.is_empty());

    // Not enough data: the device sends its log nowhere.
    let silent = run(
        "err_disabled",
        &world(vec![
            device("a", "SW1").interfaces(vec![down_port(true, false)]),
        ]),
    );
    assert!(silent.findings.is_empty());
    assert_eq!(silent.skips().count(), 1);
}

// ------------------------------------------------------------- down uplink

#[test]
fn down_uplink_table() {
    let link_down = |admin: bool| {
        let mut port = port("GigabitEthernet0/1", 1_000_000_000, "full");
        port.admin_up = Some(admin);
        port.oper_up = Some(false);
        port
    };

    // Positive: the neighbour was there at the previous collection and the
    // link is down now, without anybody shutting it.
    let found = run(
        "down_uplink",
        &world(vec![
            device("a", "SW1")
                .interfaces(vec![link_down(true)])
                .neighbors(vec![])
                .previous_neighbors(vec![neighbor("GigabitEthernet0/1", "CORE1", "Gi1/0/1")]),
        ]),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(found.findings[0].title.contains("CORE1"));

    // Negative: the link is up.
    assert!(
        run(
            "down_uplink",
            &world(vec![
                device("a", "SW1")
                    .interfaces(vec![port("GigabitEthernet0/1", 1_000_000_000, "full")])
                    .neighbors(vec![neighbor("GigabitEthernet0/1", "CORE1", "Gi1/0/1")]),
            ])
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: the port was shut. Somebody decided that.
    assert!(
        run(
            "down_uplink",
            &world(vec![
                device("a", "SW1")
                    .interfaces(vec![link_down(false)])
                    .neighbors(vec![])
                    .previous_neighbors(vec![neighbor("GigabitEthernet0/1", "CORE1", "Gi1/0/1")]),
            ])
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: a desk port with no link and no neighbour, ever. Half the
    // ports on an access switch look like this.
    let desk = run(
        "down_uplink",
        &world(vec![
            device("a", "SW1")
                .interfaces(vec![link_down(true)])
                .neighbors(vec![]),
        ]),
    );
    assert!(desk.findings.is_empty());
    assert_eq!(desk.evaluated().len(), 1);

    // Not enough data: neighbours were never collected.
    let unknown = run(
        "down_uplink",
        &world(vec![device("a", "SW1").interfaces(vec![link_down(true)])]),
    );
    assert!(unknown.findings.is_empty());
    assert_eq!(unknown.skips().count(), 1);
}

// -------------------------------------------------------- high utilisation

fn utilised(world: &mut World, device_id: &str, instance: &str, values: &[(i64, f64)]) {
    let series = world
        .store()
        .series_id(device_id, "if.util_pct", instance)
        .expect("series");
    let samples: Vec<(i64, i64, f64)> = values
        .iter()
        .map(|(at, value)| (series, *at, *value))
        .collect();
    world.store().samples_insert(&samples).expect("samples");
    world.reindex_series();
}

#[test]
fn high_utilisation_table() {
    let minute = 60_000;
    let busy: Vec<(i64, f64)> = (0..12)
        .map(|index| (NOW - (50 - index * 4) * minute, 92.0))
        .collect();

    // Positive: over the threshold for most of a long window.
    let mut congested = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    utilised(&mut congested, "a", "GigabitEthernet0/1", &busy);
    let found = run("high_utilisation", &congested);
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(
        found.findings[0]
            .evidence
            .iter()
            .any(|item| item.label == "Busiest hour")
    );

    // Negative: a quiet port.
    let quiet: Vec<(i64, f64)> = (0..12)
        .map(|index| (NOW - (50 - index * 4) * minute, 5.0))
        .collect();
    let mut idle = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    utilised(&mut idle, "a", "GigabitEthernet0/1", &quiet);
    assert!(run("high_utilisation", &idle).findings.is_empty());

    // Look-alike: one busy poll in an otherwise quiet window. A backup job
    // finishing is not congestion.
    let mut spike = quiet.clone();
    spike[6] = (spike[6].0, 99.0);
    let mut spiky = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    utilised(&mut spiky, "a", "GigabitEthernet0/1", &spike);
    assert!(run("high_utilisation", &spiky).findings.is_empty());

    // Not enough data: a long-busy port with only three samples.
    let mut thin = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    utilised(
        &mut thin,
        "a",
        "GigabitEthernet0/1",
        &[
            (NOW - 40 * minute, 95.0),
            (NOW - 20 * minute, 95.0),
            (NOW - minute, 95.0),
        ],
    );
    assert!(run("high_utilisation", &thin).findings.is_empty());

    // Not enough data: nothing is polled at all.
    let unpolled = run(
        "high_utilisation",
        &world(vec![device("a", "SW1").interfaces(vec![port(
            "GigabitEthernet0/1",
            1_000_000_000,
            "full",
        )])]),
    );
    assert_eq!(unpolled.skips().count(), 1);
}

#[test]
fn thresholds_come_from_the_settings() {
    let strict = Settings {
        thresholds: crate::settings::Thresholds {
            flap_transitions_per_hour: 2,
            ..Default::default()
        },
        ..Settings::default()
    };
    let minute = 60_000;
    let mut twice = world(vec![device("a", "SW1").interfaces(vec![port(
        "GigabitEthernet0/1",
        1_000_000_000,
        "full",
    )])]);
    sampled(
        &mut twice,
        "a",
        "GigabitEthernet0/1",
        &[
            (NOW - 30 * minute, 1.0),
            (NOW - 20 * minute, 0.0),
            (NOW - 10 * minute, 1.0),
        ],
    );
    assert!(run("interface_flapping", &twice).findings.is_empty());
    assert_eq!(
        run_with("interface_flapping", &twice, &strict)
            .findings
            .len(),
        1
    );
}

/// Devices that are not a scan's target are context, not subjects: a finding
/// is never reported against one.
#[test]
fn only_targets_are_reported_on() {
    let devices: Vec<DeviceData> = vec![
        device("a", "SW1")
            .interfaces(vec![port("GigabitEthernet0/1", 100_000_000, "half")])
            .neighbors(vec![neighbor(
                "GigabitEthernet0/1",
                "SW2",
                "GigabitEthernet0/2",
            )]),
        device("b", "SW2")
            .interfaces(vec![port("GigabitEthernet0/2", 100_000_000, "full")])
            .neighbors(vec![neighbor(
                "GigabitEthernet0/2",
                "SW1",
                "GigabitEthernet0/1",
            )]),
    ];
    let mut world = world(devices);
    // Only SW2 is a target; the finding belongs to SW1 (the half-duplex end),
    // and the link is still evaluated because SW2 is one of its ends.
    world.targets = vec![1];
    world.target_flags = vec![false, true];
    let found = run("duplex_mismatch", &world);
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(found.evaluated().contains("b"));
}
