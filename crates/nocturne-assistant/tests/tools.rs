//! Every tool this build offers, called once, shaped and checked.
//!
//! The point is breadth: a tool whose handler misreads the shape its feature
//! answers with would otherwise only be found by a user. Each call must come
//! back `ok`, with something in it, and without a JSON dump.

mod common;

use common::{Harness, Options};
use nocturne_ai::provider::scripted::Step;
use serde_json::{Value, json};

/// Arguments that work against the stub features, one call per tool.
fn calls() -> Vec<(String, Value)> {
    vec![
        ("list_devices".into(), json!({"site": "Lab"})),
        ("get_device".into(), json!({"device": "SW1"})),
        (
            "run_commands".into(),
            json!({"devices": ["SW1"], "commands": ["show version"]}),
        ),
        (
            "device_ping".into(),
            json!({"device": "SW1", "target": "10.0.0.1"}),
        ),
        ("explain_terminal".into(), json!({"session_id": "term1"})),
        (
            "collect_data".into(),
            json!({"devices": ["SW1"], "kinds": ["facts", "vlans"]}),
        ),
        (
            "get_collected".into(),
            json!({"device": "SW1", "kind": "vlans"}),
        ),
        (
            "get_config".into(),
            json!({"device": "SW1", "section": "interface"}),
        ),
        ("search_configs".into(), json!({"query": "snmp-server"})),
        ("diff_config".into(), json!({"device": "SW1"})),
        ("backup_config".into(), json!({"devices": ["SW1"]})),
        ("compliance_results".into(), json!({"severity": "high"})),
        ("list_alerts".into(), json!({"state": "firing"})),
        (
            "get_metrics".into(),
            json!({"device": "SW1", "metric": "cpu.pct", "hours": 2}),
        ),
        (
            "interface_status".into(),
            json!({"device": "SW1", "problem_only": true}),
        ),
        ("query_logs".into(), json!({"stream": "syslog", "hours": 6})),
        ("list_insights".into(), json!({"severity": "high"})),
        ("topology_neighbors".into(), json!({"device": "SW1"})),
        ("find_path".into(), json!({"from": "SW1", "to": "10.0.0.1"})),
        ("locate_host".into(), json!({"query": "10.0.10.37"})),
        ("ipam_search".into(), json!({"query": "10.0.10.0/24"})),
        (
            "next_free_address".into(),
            json!({"prefix_id": "p1", "count": 2}),
        ),
        (
            "subnet_calc".into(),
            json!({"input": "10.0.4.7 255.255.252.0"}),
        ),
        (
            "vlsm_plan".into(),
            json!({"base": "10.20.0.0/16",
                   "sizes": [{"name": "users", "hosts": 500}, {"name": "voice", "hosts": 200}]}),
        ),
        ("ping".into(), json!({"host": "10.0.0.1", "count": 4})),
        ("traceroute".into(), json!({"host": "10.0.0.1"})),
        (
            "port_check".into(),
            json!({"host": "10.0.0.1", "ports": "22,443"}),
        ),
        (
            "dns_lookup".into(),
            json!({"name": "sw1.lab.example", "kind": "A"}),
        ),
        (
            "tls_inspect".into(),
            json!({"host": "10.0.0.5", "port": 443}),
        ),
        ("list_templates".into(), json!({"query": "access"})),
        (
            "render_template".into(),
            json!({"template_id": "access-port", "devices": ["SW1"],
                   "vars": {"interface": "GigabitEthernet0/5", "vlan": 40}}),
        ),
        (
            "validate_profile".into(),
            json!({"yaml": "id: drafted\nname: Drafted\nvendor: Test\nos: Test\n"}),
        ),
        ("search_notes".into(), json!({"query": "wan"})),
        (
            "design_network".into(),
            json!({"requirements": {"sites": 2, "users": 200, "redundancy": "single"}}),
        ),
        (
            "create_lab".into(),
            json!({"name": "proof", "requirements": {"sites": 1, "users": 20}}),
        ),
        ("list_labs".into(), json!({})),
        ("lab_connectivity".into(), json!({"lab_id": "lab1"})),
        (
            "lab_probe".into(),
            json!({"lab_id": "lab1", "from": "HQ-ACC1", "to": "HQ-CORE1", "kind": "ping"}),
        ),
        (
            "preview_change".into(),
            json!({"device": "SW1", "lines": ["vlan 40", " name guests"]}),
        ),
        (
            "propose_change".into(),
            json!({"title": "Add VLAN 40",
                   "targets": [{"device": "SW1", "lines": ["vlan 40", " name guests"]}]}),
        ),
        ("list_changes".into(), json!({})),
        (
            "log_bursts".into(),
            json!({"device": "SW1", "hours": 6, "min_count": 5}),
        ),
        ("log_context".into(), json!({"entry_id": 42, "minutes": 5})),
        ("list_detections".into(), json!({"hours": 24})),
        ("service_checks".into(), json!({"device": "SW1"})),
        ("list_incidents".into(), json!({"state": "unresolved"})),
        ("get_incident".into(), json!({"id": "inc-1"})),
        ("handover_summary".into(), json!({"hours": 8})),
        ("list_evidence".into(), json!({"kind": "terminal"})),
        ("get_evidence".into(), json!({"id": "ev-term"})),
        ("discovery_results".into(), json!({})),
        ("automation_runs".into(), json!({})),
        ("qos_status".into(), json!({"device": "SW1"})),
        (
            "top_talkers".into(),
            json!({"by": "conversation", "minutes": 30}),
        ),
        ("list_snippets".into(), json!({"device": "SW1"})),
    ]
}

/// Offered, and not called here: these need an id from another call, or wait
/// for a person. `autonomy.rs` and `reach.rs` cover them end to end.
const ELSEWHERE: &[&str] = &[
    "get_change",
    "apply_change",
    "acknowledge_alert",
    "reserve_address",
    "start_discovery",
];

#[tokio::test]
async fn every_tool_answers_with_something_shaped_for_a_model() {
    let harness = Harness::with(Options {
        devices: vec!["SW1", "SW2"],
        settings: json!({"max_steps": 80, "autonomy": "lab"}),
        ..Options::default()
    })
    .await;
    let calls = calls();

    // Every tool the build offers must be in the list above.
    let offered: Vec<String> = nocturne_assistant::tools::available(harness.app(), false)
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    let covered: Vec<String> = calls.iter().map(|(name, _)| name.clone()).collect();
    for name in &offered {
        if ELSEWHERE.contains(&name.as_str()) {
            continue;
        }
        assert!(covered.contains(name), "`{name}` is not exercised here");
    }
    assert_eq!(
        offered.len(),
        covered.len() + ELSEWHERE.len(),
        "{offered:?}"
    );
    // The free edition's build, with the ledger closed: no ISP tool at all.
    assert!(
        !offered.iter().any(|name| name.starts_with("isp_")),
        "{offered:?}"
    );

    // In batches, so the scripted model looks like a model.
    let mut steps: Vec<Step> = calls
        .chunks(6)
        .map(|chunk| Step::Tools(None, chunk.to_vec()))
        .collect();
    steps.push(Step::text("I looked at everything."));
    harness.script(steps);

    let turn = harness.send("show me everything you can do").await;
    assert_eq!(turn.stop(), "end", "{:#?}", turn.kinds());

    let results = turn.tool_results();
    assert_eq!(results.len(), calls.len());
    for ((name, arguments), (_, status, summary)) in calls.iter().zip(&results) {
        assert_eq!(status, "ok", "`{name}` {arguments} → {summary}");
        assert!(!summary.is_empty(), "`{name}` came back with no summary");
    }

    // What the model actually received: shaped text, not JSON dumps.
    let sent = harness.sent_to_model();
    for (fragment, from) in [
        ("GigabitEthernet0/1", "the interface table"),
        ("10.0.4.0", "the subnet calculator"),
        ("100 percent (5/5)", "the device's own ping output"),
        ("loss %: 0", "the ping report"),
        ("1 hop(s)", "the traceroute"),
        ("port  state", "the port check table"),
        ("NOERROR", "the DNS answer"),
        ("days left: 12", "the certificate"),
        ("CORE1.lab.example", "the neighbour table"),
        ("no route data collected", "the honest stop in find_path"),
        ("10.20.0.0/23", "the address plan"),
        ("voice", "the VLAN table collected from the device"),
        ("risk: low", "the planned change"),
        ("Two-site campus", "the design"),
        ("VLAN 20 is not allowed on the trunk", "the lab matrix"),
        ("WAN failover runbook", "the team's notes"),
        ("users HQ", "the address plan's own name for the prefix"),
        ("LINK-3-UPDOWN", "the folded log messages"),
        (
            "Changes: none in the window",
            "an empty list that is not unknown",
        ),
        ("Unplanned configuration change", "the detection's name"),
        ("tls_expiring", "what a service check last found"),
        ("SW1 uplink flapping", "the incident list"),
        ("Checked the patch panel", "the incident's timeline"),
        (
            "`changes.list` is not part of this build",
            "a handover section that is unknown",
        ),
        ("10.0.10.77", "what a discovery scan found"),
        ("Nightly version check", "the saved automation tasks"),
        ("WAN-OUT/class-default", "a QoS class"),
        (
            "10.0.0.5 <-> 192.0.2.80",
            "a conversation the flow records hold",
        ),
        (
            "Port bounce — for test_ios",
            "the snippet variant for the device",
        ),
    ] {
        assert!(
            sent.contains(fragment),
            "`{fragment}` ({from}) never reached the model"
        );
    }
    // Device text is always delimited, and never a raw JSON object.
    assert!(sent.contains("<nocturne:data source="));
    assert!(
        !sent.contains("\\\"rows\\\":["),
        "a tool returned a JSON dump"
    );
}

#[tokio::test]
async fn a_tool_whose_feature_answers_with_nothing_says_so_rather_than_inventing() {
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        ..Options::default()
    })
    .await;
    harness.script(vec![
        // Nothing has been collected for this kind, and no metric exists.
        Step::tool(
            "get_collected",
            json!({"device": "SW1", "kind": "mac_table"}),
        ),
        Step::tool(
            "get_metrics".to_string(),
            json!({"device": "SW1", "metric": "temp.c", "hours": 1}),
        ),
        Step::text("Nothing is stored for either."),
    ]);
    // The metric stub answers with points, so take it out of this build.
    let turn = harness.send("what is in the mac table of SW1?").await;
    assert_eq!(turn.stop(), "end");
    let results = turn.tool_results();
    assert_eq!(results[0].1, "ok");
    assert_eq!(results[0].2, "nothing stored");
    let sent = harness.sent_to_model();
    assert!(
        sent.contains("Nothing of kind `mac_table` has been collected")
            && sent.contains("Use collect_data"),
        "{sent}"
    );
    assert!(
        !sent.contains("(no rows)") || sent.contains("Nothing of kind"),
        "an empty table and no data at all are different answers"
    );
}
