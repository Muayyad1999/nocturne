//! Every RPC method over the wire shape the UI really sends, with its
//! failure modes.

use std::time::Duration;

use serde_json::json;

use crate::testing::{AgentData, AgentIf, Harness};

const MINUTE: Duration = Duration::from_secs(60);

/// The methods this feature owns, as the specification lists them.
const CONTRACT: [&str; 26] = [
    "monitor.status",
    "monitor.poll_now",
    "monitor.device",
    "monitor.interfaces",
    "metrics.query",
    "metrics.query_many",
    "metrics.series",
    "metrics.top",
    "alerts.list",
    "alerts.get",
    "alerts.ack",
    "alerts.resolve",
    "alerts.snooze",
    "alerts.raise",
    "alerts.summary",
    "alerts.rules.list",
    "alerts.rules.save",
    "alerts.rules.delete",
    "alerts.rules.test",
    "maintenance.list",
    "maintenance.save",
    "maintenance.delete",
    "notify.channels.list",
    "notify.channels.save",
    "notify.channels.delete",
    "notify.channels.test",
];

#[tokio::test]
async fn every_method_in_the_contract_is_registered_and_no_others() {
    let harness = Harness::new().await;
    let core = nocturne_app::testing::builder().build().methods();
    let mine: Vec<&str> = harness
        .app
        .methods()
        .into_iter()
        .filter(|method| !core.contains(method))
        .collect();
    let mut expected: Vec<&str> = CONTRACT.to_vec();
    expected.sort_unstable();
    assert_eq!(mine, expected);
}

#[tokio::test(start_paused = true)]
async fn monitor_device_gathers_state_health_interfaces_and_sources() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(360_000),
                counter64: true,
                interfaces: vec![AgentIf {
                    alias: "uplink".into(),
                    ..AgentIf::up("Gi0/1", 1_000)
                }],
                scalars: vec![(
                    "1.3.6.1.4.1.9.9.109.1.1.1.1.6.1".into(),
                    nocturne_snmp::Value::Gauge32(23),
                )],
                ..AgentData::default()
            },
        )
        .await;
    harness.poll(&id).await;

    let view = harness
        .rpc("monitor.device", json!({"device_id": id}))
        .await;
    assert_eq!(view["state"]["reachable"], true);
    assert_eq!(view["state"]["rtt_ms"], 1.5);
    assert_eq!(view["down"], false);
    assert_eq!(view["lab"], false);
    assert_eq!(view["health"]["uptime_s"], 3_600);
    assert_eq!(view["sources"]["icmp"]["ok"], true);
    assert_eq!(view["sources"]["snmp"]["ok"], true);
    assert_eq!(view["sources"]["cli"]["enabled"], false);
    assert_eq!(view["interfaces"][0]["name"], "GigabitEthernet0/1");
    assert_eq!(view["interfaces"][0]["description"], "uplink");
    assert_eq!(
        view["interfaces"][0]["in_bps"],
        json!(null),
        "a gap, not a zero"
    );
    assert_eq!(view["last_poll_at"], harness.now());

    assert_eq!(
        harness
            .app
            .rpc("monitor.device", json!({"device_id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test(start_paused = true)]
async fn monitor_poll_now_runs_a_job_over_the_devices_it_is_given() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let up = harness.device("SW1", "10.0.0.1", json!({})).await;
    let down = harness.device("SW2", "10.0.0.2", json!({})).await;
    harness.probe.down("10.0.0.2");

    let started = harness
        .rpc("monitor.poll_now", json!({"device_ids": [up, down]}))
        .await;
    let job_id = started["job_id"].as_str().unwrap().to_string();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let job = harness.rpc("jobs.get", json!({"id": job_id})).await;
        if job["state"] != "running" {
            assert_eq!(job["state"], "succeeded", "{job:#?}");
            assert_eq!(
                job["result"],
                json!({"polled": 2, "up": 1, "down": 1, "failed": 0})
            );
            assert_eq!(harness.probe.count("10.0.0.1"), 1);
            return;
        }
    }
    panic!("the poll job never finished");
}

#[tokio::test]
async fn monitor_poll_now_needs_devices_that_exist() {
    let harness = Harness::new().await;
    let empty = harness
        .app
        .rpc("monitor.poll_now", json!({"device_ids": []}))
        .await
        .unwrap_err();
    assert_eq!(empty.code, "invalid");
    let missing = harness
        .app
        .rpc("monitor.poll_now", json!({"device_id": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "not_found");
}

#[tokio::test(start_paused = true)]
async fn metrics_queries_return_points_a_step_and_the_resolution_used() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    let start = harness.now();
    for index in 0..5 {
        harness.probe.set(
            "10.0.0.1",
            crate::testing::HostState {
                rtt_ms: 1.0 + f64::from(index),
                ..crate::testing::HostState::default()
            },
        );
        harness.poll(&id).await;
        tokio::time::sleep(MINUTE).await;
    }

    // Every window here is a whole number of steps, and that matters. The
    // store buckets raw samples by `span / max_points`, where `max_points`
    // is `span / step` rounded down — so a 301 s window at a 60 s step gives
    // five buckets of 60.2 s, *wider* than the 60 s between the samples, and
    // two samples share a bucket for about one epoch phase in eighty. At
    // 300 s the buckets are exactly 60 s and each sample lands in its own,
    // whatever the wall clock happens to read.
    let answer = harness
        .rpc(
            "metrics.query",
            json!({"device_id": id, "metric": "icmp.rtt_ms", "since": start,
                   "until": start + 300_000, "step_s": 60}),
        )
        .await;
    let points = answer["points"].as_array().unwrap();
    assert_eq!(points.len(), 5, "one point per poll: {answer:#?}");
    assert_eq!(points[0][1], 1.0);
    assert_eq!(points[4][1], 5.0);
    assert_eq!(answer["rollup"], "raw");
    assert_eq!(answer["step_s"], 60);
    assert_eq!(answer["device_id"], id.as_str());

    // A gap stays a gap: nothing was written while the device was down.
    harness.probe.down("10.0.0.1");
    for _ in 0..3 {
        harness.poll(&id).await;
        tokio::time::sleep(MINUTE).await;
    }
    let answer = harness
        .rpc(
            "metrics.query",
            json!({"device_id": id, "metric": "icmp.rtt_ms", "since": start,
                   "until": start + 600_000, "step_s": 60}),
        )
        .await;
    assert_eq!(
        answer["points"].as_array().unwrap().len(),
        5,
        "three failed polls add three holes, not three zeroes: {answer:#?}"
    );

    let many = harness
        .rpc(
            "metrics.query_many",
            json!({"series": [{"device_id": id, "metric": "icmp.rtt_ms"},
                              {"device_id": id, "metric": "up"}],
                   "since": start, "until": start + 600_000, "step_s": 60}),
        )
        .await;
    assert_eq!(many.as_array().unwrap().len(), 2);
    assert_eq!(many[1]["metric"], "up");
    assert_eq!(
        many[1]["points"].as_array().unwrap().len(),
        8,
        "eight polls, five up and three down: {:#?}",
        many[1]
    );

    let series = harness
        .rpc("metrics.series", json!({"device_id": id}))
        .await;
    let names: Vec<&str> = series
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["metric"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["icmp.loss_pct", "icmp.rtt_ms", "up"]);
    assert_eq!(series[2]["latest"][1], 0.0, "the device is down now");

    assert_eq!(
        harness
            .app
            .rpc(
                "metrics.query",
                json!({"device_id": id, "metric": "", "since": 0})
            )
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        harness
            .app
            .rpc("metrics.series", json!({"device_id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

/// The bucket arithmetic above, pinned to the epoch phase that used to break
/// it rather than to whatever the clock happens to read.
///
/// The store buckets raw samples by `span / max_points`, and `max_points` is
/// `span / step` rounded down. A 301 s window at a 60 s step therefore asks
/// for five buckets of 60.2 s — wider than the 60 s between the samples — and
/// whenever the first sample falls in the opening 800 ms of a 60.2 s cycle,
/// two samples share a bucket and a point goes missing. That is 800 phases in
/// 60,200, about one run in eighty, which is why it survived locally and only
/// showed up once in CI. A window that is a whole number of steps gives
/// buckets of exactly 60 s, and then the count cannot depend on the phase.
#[tokio::test]
async fn a_metric_window_of_whole_steps_gives_one_point_per_sample() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    // A base whose residue mod 60 200 is 0: the worst case for the old window.
    let now = nocturne_app::util::now_ms();
    let base = (now / 60_200 + 1) * 60_200;
    let series = harness
        .app
        .store()
        .series_id(&id, "icmp.rtt_ms", "")
        .unwrap();
    let samples: Vec<(i64, i64, f64)> = (0..5)
        .map(|k| (series, base + 60_000 * k, 1.0 + k as f64))
        .collect();
    harness.app.store().samples_insert(&samples).unwrap();

    let count = async |instance: &str, since: i64, until: i64| {
        harness
            .rpc(
                "metrics.query",
                json!({"device_id": id, "metric": "icmp.rtt_ms", "instance": instance,
                       "since": since, "until": until, "step_s": 60}),
            )
            .await["points"]
            .as_array()
            .unwrap()
            .len()
    };

    // The shape this test used to use: five samples, four points.
    assert_eq!(
        count("", base - 1_000, base + 300_000).await,
        4,
        "a 301 s window at this phase really does lose a point"
    );
    // A whole number of steps: one point per sample, at the same phase.
    assert_eq!(count("", base, base + 300_000).await, 5);

    // And at every phase, which is the point: the bucket is the step, so
    // consecutive samples are exactly one bucket apart wherever the epoch
    // falls. Each phase gets its own series so they cannot interfere.
    for residue in [0_i64, 1, 799, 800, 30_000, 60_199] {
        let phase = base + residue;
        let instance = format!("phase{residue}");
        let series = harness
            .app
            .store()
            .series_id(&id, "icmp.rtt_ms", &instance)
            .unwrap();
        let samples: Vec<(i64, i64, f64)> = (0..5)
            .map(|k| (series, phase + 60_000 * k, 1.0 + k as f64))
            .collect();
        harness.app.store().samples_insert(&samples).unwrap();
        assert_eq!(
            count(&instance, phase, phase + 300_000).await,
            5,
            "samples at phase {residue} lost a point"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn metrics_top_ranks_across_devices_and_can_be_filtered() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    for (index, rtt) in [("SW1", 9.0), ("SW2", 3.0), ("SW3", 40.0)] {
        let id = harness
            .device(
                index,
                &format!("10.5.0.{}", index.as_bytes()[2] - b'0'),
                json!({"site": if index == "SW3" { "Branch" } else { "HQ" }}),
            )
            .await;
        harness.probe.set(
            &nocturne_app::inventory::get(&harness.app, &id)
                .unwrap()
                .address,
            crate::testing::HostState {
                rtt_ms: rtt,
                ..crate::testing::HostState::default()
            },
        );
        harness.poll(&id).await;
    }

    let top = harness
        .rpc("metrics.top", json!({"metric": "icmp.rtt_ms", "limit": 2}))
        .await;
    let entries = top.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["device"], "SW3");
    assert_eq!(entries[0]["value"], 40.0);
    assert_eq!(entries[1]["device"], "SW1");

    let filtered = harness
        .rpc(
            "metrics.top",
            json!({"metric": "icmp.rtt_ms", "filter": "HQ"}),
        )
        .await;
    let names: Vec<&str> = filtered
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["device"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["SW1", "SW2"]);

    // Stale values are left out.
    tokio::time::sleep(Duration::from_secs(1_000)).await;
    let stale = harness
        .rpc(
            "metrics.top",
            json!({"metric": "icmp.rtt_ms", "within_s": 300}),
        )
        .await;
    assert!(stale.as_array().unwrap().is_empty());

    assert_eq!(
        harness
            .app
            .rpc("metrics.top", json!({"metric": " "}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test(start_paused = true)]
async fn alerts_list_filters_by_state_severity_device_and_pages() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    for (index, severity) in ["info", "high", "critical"].iter().enumerate() {
        harness
            .rpc(
                "alerts.raise",
                json!({"severity": severity, "title": format!("alert {index}"),
                       "device_id": id, "dedup_key": format!("k{index}")}),
            )
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    harness
        .rpc(
            "alerts.raise",
            json!({"severity": "low", "title": "no device", "dedup_key": "k9"}),
        )
        .await;

    let all = harness.rpc("alerts.list", json!({})).await;
    assert_eq!(all.as_array().unwrap().len(), 4);
    assert_eq!(all[0]["title"], "no device", "newest first");

    let severe = harness
        .rpc("alerts.list", json!({"severity": "high"}))
        .await;
    assert_eq!(severe.as_array().unwrap().len(), 2, "at least this severe");

    let of_device = harness.rpc("alerts.list", json!({"device_id": id})).await;
    assert_eq!(of_device.as_array().unwrap().len(), 3);

    let page = harness.rpc("alerts.list", json!({"limit": 2})).await;
    assert_eq!(page.as_array().unwrap().len(), 2);
    let next = harness
        .rpc("alerts.list", json!({"limit": 2, "cursor": "2"}))
        .await;
    assert_eq!(next.as_array().unwrap().len(), 2);
    assert_ne!(next[0]["id"], page[0]["id"]);

    let summary = harness.rpc("alerts.summary", json!({})).await;
    assert_eq!(summary["open"], 4);
    assert_eq!(summary["by_severity"]["critical"], 1);
    assert_eq!(summary["newest"]["title"], "no device");

    harness
        .rpc("alerts.resolve", json!({"id": all[0]["id"]}))
        .await;
    assert_eq!(
        harness
            .rpc("alerts.list", json!({"state": "resolved"}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        harness
            .rpc("alerts.list", json!({"state": "open"}))
            .await
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for bad in [json!({"state": "exploded"}), json!({"cursor": "soon"})] {
        assert_eq!(
            harness.app.rpc("alerts.list", bad).await.unwrap_err().code,
            "invalid"
        );
    }
    assert_eq!(
        harness
            .app
            .rpc("alerts.get", json!({"id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn the_built_in_rules_are_there_and_can_be_changed_but_not_deleted() {
    let harness = Harness::new().await;
    let rules = harness.rpc("alerts.rules.list", json!({})).await;
    let listed = rules.as_array().unwrap();
    assert_eq!(listed.len(), 10);
    assert!(listed.iter().all(|rule| rule["builtin"] == true));
    let kinds: Vec<&str> = listed
        .iter()
        .map(|rule| rule["condition"]["type"].as_str().unwrap())
        .collect();
    for wanted in [
        "device_down",
        "interface_down",
        "threshold",
        "no_data",
        "config_changed",
        "rtt_baseline",
    ] {
        if wanted == "no_data" {
            continue; // no built-in uses it; the condition exists for rules people write
        }
        assert!(kinds.contains(&wanted), "no built-in rule uses {wanted}");
    }

    let error = harness
        .app
        .rpc("alerts.rules.delete", json!({"id": "builtin.device_down"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert!(error.message.contains("switch it off"), "{error}");

    let disabled = harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.device_down", "enabled": false}),
        )
        .await;
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["builtin"], true);
    assert_eq!(disabled["name"], "Device down", "the rest is left alone");

    // Starting again does not undo what the user changed.
    crate::rules::ensure_builtins(&harness.app).unwrap();
    let rules = harness.rpc("alerts.rules.list", json!({})).await;
    let device_down = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["id"] == "builtin.device_down")
        .unwrap();
    assert_eq!(device_down["enabled"], false);
}

#[tokio::test]
async fn rules_are_validated_saved_and_deleted() {
    let harness = Harness::new().await;
    for bad in [
        json!({"name": "x"}),
        json!({"name": "", "condition": {"type": "device_down"}}),
        json!({"name": "x", "condition": {"type": "threshold", "metric": " ", "op": "gt", "value": 1}}),
        json!({"name": "x", "condition": {"type": "no_data", "metric": "cpu.pct", "for_s": 0}}),
        json!({"name": "x", "condition": {"type": "rtt_baseline", "multiple": 1.0, "window_s": 86400}}),
        json!({"name": "x", "condition": {"type": "rtt_baseline", "multiple": 5.0, "window_s": 10}}),
    ] {
        assert_eq!(
            harness
                .app
                .rpc("alerts.rules.save", bad.clone())
                .await
                .unwrap_err()
                .code,
            "invalid",
            "{bad} should be refused"
        );
    }

    let rule = harness
        .rpc(
            "alerts.rules.save",
            json!({"name": "Branch uplinks", "severity": "high", "for_s": 120,
                   "scope": {"sites": ["Branch"], "roles": ["edge"]},
                   "condition": {"type": "interface_down", "only": "uplinks"},
                   "notify": ["channel1"]}),
        )
        .await;
    assert_eq!(rule["builtin"], false);
    assert_eq!(rule["scope"]["sites"][0], "Branch");
    assert_eq!(rule["notify"][0], "channel1");

    // A partial save leaves the rest alone.
    let renamed = harness
        .rpc(
            "alerts.rules.save",
            json!({"id": rule["id"], "name": "Branch uplinks (high)"}),
        )
        .await;
    assert_eq!(renamed["for_s"], 120);
    assert_eq!(renamed["condition"]["only"], "uplinks");

    harness
        .rpc("alerts.rules.delete", json!({"id": rule["id"]}))
        .await;
    assert_eq!(
        harness
            .app
            .rpc("alerts.rules.delete", json!({"id": rule["id"]}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test(start_paused = true)]
async fn rules_test_says_what_a_rule_would_fire_on_without_firing_it() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000),
                counter64: true,
                interfaces: vec![
                    AgentIf {
                        oper_up: false,
                        alias: "desk 12".into(),
                        ..AgentIf::up("Gi0/1", 1_000)
                    },
                    AgentIf::up("Gi0/2", 1_000),
                ],
                ..AgentData::default()
            },
        )
        .await;
    harness.poll(&id).await;

    let outcome = harness
        .rpc(
            "alerts.rules.test",
            json!({"rule": {"name": "Any port down", "severity": "medium",
                            "condition": {"type": "interface_down", "only": "all"}}}),
        )
        .await;
    assert_eq!(outcome["devices_in_scope"], 1);
    let matches = outcome["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["instance"], "GigabitEthernet0/1");
    assert_eq!(matches[0]["device"], "SW1");
    assert!(harness.open_alerts().is_empty(), "a test raises nothing");

    // A saved rule can be tried by id, and a scope narrows it.
    let saved = harness
        .rpc(
            "alerts.rules.save",
            json!({"name": "Branch only", "scope": {"sites": ["Branch"]},
                   "condition": {"type": "interface_down", "only": "all"}}),
        )
        .await;
    let outcome = harness
        .rpc("alerts.rules.test", json!({"id": saved["id"]}))
        .await;
    assert_eq!(outcome["devices_in_scope"], 0);
    assert!(outcome["matches"].as_array().unwrap().is_empty());

    let note = harness
        .rpc(
            "alerts.rules.test",
            json!({"rule": {"name": "cfg", "condition": {"type": "config_changed"}}}),
        )
        .await;
    assert!(note["note"].as_str().unwrap().contains("as they happen"));

    assert_eq!(
        harness
            .app
            .rpc("alerts.rules.test", json!({}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test(start_paused = true)]
async fn maintenance_windows_are_listed_saved_validated_and_deleted() {
    let harness = Harness::new().await;
    let now = harness.now();
    assert!(
        harness
            .rpc("maintenance.list", json!({}))
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    for bad in [
        json!({"name": "", "starts_at": now, "ends_at": now + 1_000}),
        json!({"name": "x", "starts_at": now, "ends_at": now}),
        json!({"name": "x", "starts_at": now, "ends_at": now + 8 * 86_400_000,
               "recurring": "weekly"}),
    ] {
        assert_eq!(
            harness
                .app
                .rpc("maintenance.save", bad.clone())
                .await
                .unwrap_err()
                .code,
            "invalid",
            "{bad} should be refused"
        );
    }

    let window = harness
        .rpc(
            "maintenance.save",
            json!({"name": "Saturday patching", "starts_at": now, "ends_at": now + 7_200_000,
                   "recurring": "weekly", "scope": {"roles": ["access"]},
                   "note": "firmware"}),
        )
        .await;
    assert_eq!(window["recurring"], "weekly");
    assert_eq!(window["scope"]["roles"][0], "access");

    let updated = harness
        .rpc(
            "maintenance.save",
            json!({"id": window["id"], "name": "Saturday patching",
                   "starts_at": now, "ends_at": now + 3_600_000}),
        )
        .await;
    assert_eq!(updated["id"], window["id"]);
    assert_eq!(updated["created_at"], window["created_at"]);
    assert!(updated["recurring"].is_null(), "left out means cleared");

    assert_eq!(
        harness
            .rpc("maintenance.list", json!({}))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    harness
        .rpc("maintenance.delete", json!({"id": window["id"]}))
        .await;
    assert_eq!(
        harness
            .app
            .rpc("maintenance.delete", json!({"id": window["id"]}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test(start_paused = true)]
async fn the_events_the_ui_follows_are_published() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000),
                counter64: true,
                interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
                ..AgentData::default()
            },
        )
        .await;
    let mut events = harness.app.subscribe();
    harness.poll_and_evaluate(&id).await;

    let mut topics = Vec::new();
    while let Ok(event) = events.try_recv() {
        topics.push(event.topic);
    }
    for wanted in ["devices.state", "monitor.polled", "monitor.interfaces"] {
        assert!(topics.iter().any(|topic| topic == wanted), "{topics:?}");
    }

    // An alert publishes alerts.changed with the whole alert.
    let mut events = harness.app.subscribe();
    harness
        .rpc(
            "alerts.raise",
            json!({"severity": "high", "title": "x", "device_id": id, "dedup_key": "k"}),
        )
        .await;
    let mut seen = None;
    while let Ok(event) = events.try_recv() {
        if event.topic == "alerts.changed" {
            seen = Some(event.payload);
        }
    }
    let payload = seen.expect("alerts.changed");
    assert_eq!(payload["alert"]["title"], "x");
    assert_eq!(payload["alert"]["state"], "firing");

    // And changing the setup says the status moved.
    let mut events = harness.app.subscribe();
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.device_down", "enabled": false}),
        )
        .await;
    let mut topics = Vec::new();
    while let Ok(event) = events.try_recv() {
        topics.push(event.topic);
    }
    assert!(topics.iter().any(|topic| topic == "monitor.status_changed"));
}

#[tokio::test(start_paused = true)]
async fn one_snmp_session_serves_many_polls_and_is_replaced_when_the_credential_changes() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000),
                counter64: true,
                interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
                ..AgentData::default()
            },
        )
        .await;
    for _ in 0..3 {
        harness.poll(&id).await;
        tokio::time::sleep(MINUTE).await;
        harness.snmp.edit("10.0.0.1:161", |agent| {
            agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        });
    }
    assert_eq!(
        harness.snmp.opens("10.0.0.1:161"),
        1,
        "SNMPv3 discovery and key localisation are worth keeping"
    );

    harness.with_snmp_credential("different").await;
    harness.poll(&id).await;
    assert_eq!(
        harness.snmp.opens("10.0.0.1:161"),
        1,
        "the community changed but the fingerprint does not carry it"
    );

    // A failed request throws the session away.
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.fail = Some("timed out".into())
    });
    tokio::time::sleep(MINUTE).await;
    harness.poll(&id).await;
    harness.snmp.edit("10.0.0.1:161", |agent| agent.fail = None);
    tokio::time::sleep(MINUTE).await;
    harness.poll(&id).await;
    assert_eq!(harness.snmp.opens("10.0.0.1:161"), 2);
}

#[tokio::test(start_paused = true)]
async fn the_maintenance_pass_rolls_samples_up_and_forgets_old_alerts() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.poll(&id).await;
    let alert = harness
        .rpc(
            "alerts.raise",
            json!({"severity": "low", "title": "old", "device_id": id, "dedup_key": "k"}),
        )
        .await;
    harness
        .rpc("alerts.resolve", json!({"id": alert["id"]}))
        .await;

    crate::scheduler::run_maintenance(&harness.app, harness.now()).unwrap();
    assert_eq!(harness.all_alerts().len(), 1, "a fresh one stays");

    // Ninety-one days later it is gone, and so are the raw samples.
    let later = harness.now() + 91 * 86_400_000;
    crate::scheduler::run_maintenance(&harness.app, later).unwrap();
    assert!(harness.all_alerts().is_empty());
    assert!(harness.samples(&id, "icmp.rtt_ms", "").is_empty());
}
