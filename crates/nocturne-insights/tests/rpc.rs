//! The whole feature through `app.rpc`, in the shape the UI will see.

mod common;

use common::{With, all_open, collected, config, lab, open_subjects, scan};
use nocturne_app::util::now_ms;
use nocturne_drivers::model::Kind;
use serde_json::json;

/// A well-kept configuration, so that the only thing wrong with these two
/// switches is the one fault each test is about.
fn tidy(hostname: &str) -> String {
    format!(
        "hostname {hostname}\n\
         no ip http server\n\
         ntp server 192.0.2.1\n\
         logging host 10.0.0.9\n\
         snmp-server community n0t-a-gu3ss RO 10\n\
         enable secret 9 $9$abcdefghij$klmnopqrstuvwxyz\n\
         line vty 0 4\n transport input ssh\n access-class 10 in\n"
    )
}

/// Two switches with a duplex mismatch between them and nothing else wrong.
async fn mismatched(with: With) -> nocturne_app::testing::Lab {
    let lab = lab(&["SW1", "SW2"], with).await;
    let (app, one, two) = (&lab.app, lab.id("SW1"), lab.id("SW2"));
    config(app.store(), &one, "running", &tidy("SW1"));
    config(app.store(), &two, "running", &tidy("SW2"));
    collected(
        app,
        &one,
        Kind::Interfaces,
        vec![json!({
            "name": "GigabitEthernet0/1", "admin_up": true, "oper_up": true,
            "speed_bps": 100_000_000u64, "duplex": "half"
        })],
    );
    collected(
        app,
        &one,
        Kind::Neighbors,
        vec![json!({
            "local_interface": "GigabitEthernet0/1", "device": "SW2",
            "remote_interface": "GigabitEthernet0/2", "protocol": "cdp"
        })],
    );
    collected(
        app,
        &two,
        Kind::Interfaces,
        vec![json!({
            "name": "GigabitEthernet0/2", "admin_up": true, "oper_up": true,
            "speed_bps": 100_000_000u64, "duplex": "a-full"
        })],
    );
    collected(
        app,
        &two,
        Kind::Neighbors,
        vec![json!({
            "local_interface": "GigabitEthernet0/2", "device": "SW1",
            "remote_interface": "GigabitEthernet0/1", "protocol": "cdp"
        })],
    );
    // Where the topology feature is present it is authoritative, so the stub
    // has to know about the cable too.
    if app.methods().contains(&"topology.get") {
        *app.state::<common::Graph>().0.lock() = json!({
            "nodes": [
                {"id": "n1", "device_id": one, "name": "SW1"},
                {"id": "n2", "device_id": two, "name": "SW2"},
            ],
            "links": [{
                "a": {"node": "n1", "interface": "GigabitEthernet0/1"},
                "b": {"node": "n2", "interface": "GigabitEthernet0/2"},
                "kind": "lldp", "confidence": "confirmed"
            }]
        });
    }
    lab
}

#[tokio::test]
async fn a_scan_is_a_job_and_its_findings_are_listed_filtered_and_counted() {
    let lab = mismatched(With::all()).await;
    let app = &lab.app;

    let result = scan(app, json!({})).await;
    assert_eq!(result["devices"], 2);
    assert!(result["findings"].as_u64().unwrap() >= 1);
    assert!(result["took_ms"].as_u64().is_some());

    let listed = app
        .rpc("insights.list", json!({"check": "duplex_mismatch"}))
        .await
        .unwrap();
    let insights = listed["insights"].as_array().unwrap();
    assert_eq!(insights.len(), 1);
    let insight = &insights[0];
    assert_eq!(insight["device"], "SW1");
    assert_eq!(insight["subject"], "GigabitEthernet0/1");
    assert_eq!(insight["severity"], "high");
    assert_eq!(insight["category"], "physical");
    assert_eq!(insight["state"], "open");
    assert_eq!(insight["lab"], true, "lab devices are labelled as such");
    assert_eq!(insight["occurrences"], 1);
    assert!(!insight["evidence"].as_array().unwrap().is_empty());
    assert_eq!(insight["related"][0]["device"], "SW2");
    assert_eq!(listed["counts"]["by_severity"]["high"], 1);
    assert_eq!(listed["counts"]["by_category"]["physical"], 1);

    // The same insight is reachable by id, and from either end of the link.
    let id = insight["id"].as_str().unwrap().to_string();
    let fetched = app.rpc("insights.get", json!({"id": id})).await.unwrap();
    assert_eq!(fetched["title"], insight["title"]);
    let far_end = app
        .rpc("insights.list", json!({"device_id": "SW2"}))
        .await
        .unwrap();
    assert_eq!(far_end["insights"].as_array().unwrap().len(), 1);

    // Filters.
    for (params, expected) in [
        (json!({"severity": "critical"}), 0),
        (json!({"severity": "high"}), 1),
        (json!({"category": "l2"}), 0),
        (json!({"query": "gi0/1"}), 0),
        (json!({"query": "GigabitEthernet0/1"}), 1),
        (json!({"state": "resolved"}), 0),
        (json!({"check": "nothing_like_this"}), 0),
    ] {
        let mut params = params;
        params["limit"] = json!(500);
        let listed = app.rpc("insights.list", params.clone()).await.unwrap();
        assert_eq!(
            listed["insights"].as_array().unwrap().len(),
            expected,
            "{params}"
        );
    }

    assert_eq!(
        app.rpc("insights.list", json!({"state": "nonsense"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        app.rpc("insights.get", json!({"id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        app.rpc("insights.scan", json!({"checks": ["nope"]}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_finding_keeps_its_id_and_resolves_after_two_clean_scans() {
    let lab = mismatched(With::all()).await;
    let (app, one) = (&lab.app, lab.id("SW1"));

    scan(app, json!({})).await;
    let first = app
        .rpc("insights.list", json!({"check": "duplex_mismatch"}))
        .await
        .unwrap()["insights"][0]
        .clone();
    let id = first["id"].as_str().unwrap().to_string();

    // Scanning again changes nothing about its identity.
    scan(app, json!({})).await;
    let again = app.rpc("insights.get", json!({"id": id})).await.unwrap();
    assert_eq!(again["first_seen"], first["first_seen"]);
    assert!(again["last_seen"].as_i64().unwrap() >= first["last_seen"].as_i64().unwrap());

    // Somebody fixes the port. One clean scan is not enough.
    collected(
        app,
        &one,
        Kind::Interfaces,
        vec![json!({
            "name": "GigabitEthernet0/1", "admin_up": true, "oper_up": true,
            "speed_bps": 100_000_000u64, "duplex": "a-full"
        })],
    );
    scan(app, json!({})).await;
    assert_eq!(
        app.rpc("insights.get", json!({"id": id})).await.unwrap()["state"],
        "open"
    );
    let second = scan(app, json!({})).await;
    assert_eq!(second["resolved"], 1);
    let resolved = app.rpc("insights.get", json!({"id": id})).await.unwrap();
    assert_eq!(resolved["state"], "resolved");
    assert!(resolved["resolved_at"].as_i64().is_some());
    assert!(
        open_subjects(app, "duplex_mismatch").await.is_empty(),
        "a resolved finding is not in the open list"
    );

    // It comes back: the same id, counted as a second occurrence.
    collected(
        app,
        &one,
        Kind::Interfaces,
        vec![json!({
            "name": "GigabitEthernet0/1", "admin_up": true, "oper_up": true,
            "speed_bps": 100_000_000u64, "duplex": "half"
        })],
    );
    scan(app, json!({})).await;
    let back = app.rpc("insights.get", json!({"id": id})).await.unwrap();
    assert_eq!(back["state"], "open");
    assert_eq!(back["occurrences"], 2);
    assert_eq!(back["first_seen"], first["first_seen"]);
}

#[tokio::test]
async fn dismissal_expires_and_can_be_undone() {
    let lab = mismatched(With::all()).await;
    let app = &lab.app;
    scan(app, json!({})).await;
    let id = app
        .rpc("insights.list", json!({"check": "duplex_mismatch"}))
        .await
        .unwrap()["insights"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Dismissed for a week, with a note.
    let dismissed = app
        .rpc(
            "insights.dismiss",
            json!({"id": id, "days": 7, "note": "cabling work on Friday"}),
        )
        .await
        .unwrap();
    assert_eq!(dismissed["state"], "dismissed");
    assert_eq!(dismissed["dismiss_note"], "cabling work on Friday");
    let until = dismissed["dismissed_until"].as_i64().unwrap();
    assert!(until > now_ms() + 6 * 86_400_000);
    // It stays dismissed over a scan that still finds it.
    scan(app, json!({})).await;
    assert_eq!(
        app.rpc("insights.get", json!({"id": id})).await.unwrap()["state"],
        "dismissed"
    );
    assert!(open_subjects(app, "duplex_mismatch").await.is_empty());

    // Reopening brings it back.
    let reopened = app.rpc("insights.reopen", json!({"id": id})).await.unwrap();
    assert_eq!(reopened["state"], "open");
    assert_eq!(reopened["dismissed_until"], json!(null));
    assert_eq!(open_subjects(app, "duplex_mismatch").await.len(), 1);

    // Dismissed for good: no expiry at all.
    let forever = app
        .rpc("insights.dismiss", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(forever["dismissed_until"], json!(null));
    assert_eq!(forever["state"], "dismissed");

    // A dismissal that has run out opens again on the next sighting.
    let mut expired = serde_json::from_value::<serde_json::Value>(forever).unwrap();
    expired["dismissed_until"] = json!(now_ms() - 1_000);
    // (Written back through the only door there is: dismiss with 0 days.)
    app.rpc("insights.dismiss", json!({"id": id, "days": 0}))
        .await
        .unwrap();
    scan(app, json!({})).await;
    assert_eq!(
        app.rpc("insights.get", json!({"id": id})).await.unwrap()["state"],
        "open"
    );
}

#[tokio::test]
async fn the_catalogue_and_the_coverage_report_say_what_was_not_checked() {
    let lab = mismatched(With::all()).await;
    let app = &lab.app;

    let checks = app
        .rpc("insights.checks", json!({}))
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    assert!(checks.len() > 20);
    let duplex = checks
        .iter()
        .find(|check| check["id"] == "duplex_mismatch")
        .expect("the catalogue lists duplex_mismatch");
    assert_eq!(duplex["category"], "physical");
    assert_eq!(duplex["default_severity"], "high");
    assert_eq!(duplex["enabled"], true);
    assert!(!duplex["needs"].as_array().unwrap().is_empty());
    assert!(duplex["description"].as_str().unwrap().contains("duplex"));
    assert!(
        duplex["not_evaluated"].as_str().unwrap().len() > 20,
        "every check says when it declines to answer"
    );

    // Before a scan there is no coverage at all, and that is said plainly.
    assert_eq!(
        app.rpc("insights.coverage", json!({})).await.unwrap(),
        json!(null)
    );

    scan(app, json!({})).await;
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    assert_eq!(coverage["devices"], 2);
    assert_eq!(coverage["link_source"], "topology");
    let per_check = coverage["checks"].as_array().unwrap();
    assert_eq!(per_check.len(), checks.len());

    let duplex = per_check
        .iter()
        .find(|check| check["check"] == "duplex_mismatch")
        .unwrap();
    assert_eq!(duplex["findings"], 1);
    assert_eq!(duplex["evaluated"], 2);
    assert_eq!(duplex["skipped"], 0);

    // A check with no data to work with reports every device as skipped, with
    // the reason — never as "nothing found".
    let errors = per_check
        .iter()
        .find(|check| check["check"] == "interface_errors")
        .unwrap();
    assert_eq!(errors["findings"], 0);
    assert_eq!(errors["evaluated"], 0);
    assert_eq!(errors["skipped"], 2);
    let reason = errors["reasons"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("one collection"), "{reason}");
    assert_eq!(errors["reasons"][0]["devices"], 2);
}

#[tokio::test]
async fn links_come_from_the_topology_feature_when_it_is_there() {
    // The graph is authoritative: with it saying the ports are cabled the
    // other way round, the mismatch is not between these two ports.
    let lab = mismatched(With::all()).await;
    let (app, one, two) = (&lab.app, lab.id("SW1"), lab.id("SW2"));
    *app.state::<common::Graph>().0.lock() = json!({
        "nodes": [
            {"id": "n1", "device_id": one, "name": "SW1"},
            {"id": "n2", "device_id": two, "name": "SW2"},
        ],
        "links": [{
            "a": {"node": "n1", "interface": "GigabitEthernet0/9"},
            "b": {"node": "n2", "interface": "GigabitEthernet0/9"},
            "kind": "lldp", "confidence": "confirmed"
        }]
    });
    scan(app, json!({})).await;
    assert!(open_subjects(app, "duplex_mismatch").await.is_empty());
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    assert_eq!(coverage["link_source"], "topology");
    assert_eq!(coverage["links"], 1);

    // Without the feature, links are derived from the collected neighbours.
    let lab = mismatched(With::default()).await;
    let app = &lab.app;
    scan(app, json!({})).await;
    assert_eq!(open_subjects(app, "duplex_mismatch").await.len(), 1);
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    assert_eq!(coverage["link_source"], "neighbors");
}

#[tokio::test]
async fn a_fix_goes_to_the_change_pipeline_and_is_never_applied_here() {
    let lab = lab(&["SW1"], With::all()).await;
    let (app, id) = (&lab.app, lab.id("SW1"));
    // A telnet-enabled VTY line: a finding whose fix is exact.
    config(
        app.store(),
        &id,
        "running",
        "hostname SW1\nline vty 0 4\n transport input telnet ssh\n access-class 10 in\n",
    );
    scan(app, json!({})).await;
    let insight = app
        .rpc("insights.list", json!({"check": "telnet_enabled"}))
        .await
        .unwrap()["insights"][0]
        .clone();
    let insight_id = insight["id"].as_str().unwrap().to_string();
    assert_eq!(
        insight["suggestion"]["config_lines"],
        json!(["line vty 0 4", "transport input ssh"])
    );

    let fixed = app
        .rpc("insights.fix", json!({"id": insight_id}))
        .await
        .unwrap();
    assert_eq!(fixed["change_id"], "chg-1");
    let changes = app.state::<common::Recorder>().changes.lock().clone();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["origin"], "user");
    assert_eq!(changes[0]["targets"][0]["device_id"], id);
    assert_eq!(
        changes[0]["targets"][0]["intent"],
        json!({"type": "lines", "lines": ["line vty 0 4", "transport input ssh"]})
    );
    assert!(
        changes[0]["title"].as_str().unwrap().contains("SW1"),
        "{}",
        changes[0]["title"]
    );
    // The device itself is untouched: nothing was sent to it.
    assert!(
        lab.fake("SW1")
            .seen()
            .iter()
            .all(|command| !command.contains("transport input")),
        "{:?}",
        lab.fake("SW1").seen()
    );

    // A finding with no configuration fix says so rather than inventing one.
    let no_fix = app
        .rpc("insights.list", json!({"check": "no_syslog_host"}))
        .await
        .unwrap()["insights"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        app.rpc("insights.fix", json!({"id": no_fix}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn without_the_change_pipeline_a_fix_is_unavailable_not_applied() {
    let lab = lab(&["SW1"], With::default()).await;
    let (app, id) = (&lab.app, lab.id("SW1"));
    config(
        app.store(),
        &id,
        "running",
        "hostname SW1\nline vty 0 4\n transport input telnet\n",
    );
    scan(app, json!({})).await;
    let insight_id = app
        .rpc("insights.list", json!({"check": "telnet_enabled"}))
        .await
        .unwrap()["insights"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        app.rpc("insights.fix", json!({"id": insight_id}))
            .await
            .unwrap_err()
            .code,
        "unavailable"
    );
}

#[tokio::test]
async fn serious_findings_are_raised_as_alerts_once() {
    let lab = mismatched(With::all()).await;
    let app = &lab.app;
    scan(app, json!({})).await;
    let raised = app.state::<common::Recorder>().alerts.lock().clone();
    assert_eq!(raised.len(), 1);
    let alert = &raised[0];
    assert_eq!(alert["severity"], "high");
    assert_eq!(alert["instance"], "GigabitEthernet0/1");
    assert_eq!(
        alert["dedup_key"],
        app.rpc("insights.list", json!({"check": "duplex_mismatch"}))
            .await
            .unwrap()["insights"][0]["id"],
        "the insight id is the dedup key, so one problem is one alert"
    );

    // Seeing it again does not raise it again.
    scan(app, json!({})).await;
    assert_eq!(app.state::<common::Recorder>().alerts.lock().len(), 1);
}

#[tokio::test]
async fn settings_switch_checks_off_and_move_the_thresholds() {
    let lab = mismatched(With::all()).await;
    let app = &lab.app;
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"enabled_checks": {"duplex_mismatch": false}}}),
    )
    .await
    .unwrap();
    scan(app, json!({})).await;
    assert!(open_subjects(app, "duplex_mismatch").await.is_empty());
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    let duplex = coverage["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["check"] == "duplex_mismatch")
        .unwrap();
    assert_eq!(duplex["enabled"], false);
    assert_eq!(duplex["reasons"][0]["reason"], "the check is switched off");
    assert_eq!(
        app.rpc("insights.checks", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == "duplex_mismatch")
            .unwrap()["enabled"],
        false
    );

    // Asking for a check by name runs it even so: the operator is explicit.
    scan(app, json!({"checks": ["duplex_mismatch"]})).await;
    assert_eq!(open_subjects(app, "duplex_mismatch").await.len(), 1);

    // Lab devices can be left out of the lists altogether.
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"include_lab": false}}),
    )
    .await
    .unwrap();
    assert!(all_open(app).await.is_empty());
    let with_lab = app
        .rpc("insights.list", json!({"include_lab": true}))
        .await
        .unwrap();
    assert!(!with_lab["insights"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_scan_can_be_narrowed_to_one_device() {
    let lab = mismatched(With::default()).await;
    let (app, two) = (&lab.app, lab.id("SW2"));
    let result = scan(app, json!({"device_ids": [two]})).await;
    assert_eq!(result["devices"], 1);
    // The link is still judged — the far end's data is context — and the
    // finding is filed against the half-duplex end.
    assert_eq!(
        open_subjects(app, "duplex_mismatch").await,
        ["SW1:GigabitEthernet0/1"]
    );

    assert_eq!(
        app.rpc("insights.scan", json!({"id": "no-such-device"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn deleting_a_device_forgets_its_findings() {
    let lab = mismatched(With::default()).await;
    let (app, one) = (&lab.app, lab.id("SW1"));
    scan(app, json!({})).await;
    assert_eq!(all_open(app).await.len(), 1);

    app.rpc("devices.delete", json!({"ids": [one]}))
        .await
        .unwrap();
    // The watcher does the forgetting; give it the turn it needs. The budget
    // is a ceiling on a watcher that never ran, not an estimate of how long
    // it takes: the loop leaves the moment the findings are gone, so making
    // it generous costs a passing run nothing and spares a busy one.
    for _ in 0..250 {
        if all_open(app).await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        all_open(app).await.is_empty(),
        "the findings of a deleted device were never forgotten"
    );
}

#[tokio::test]
async fn lists_page_with_a_cursor() {
    let lab = lab(&["SW1"], With::default()).await;
    let (app, id) = (&lab.app, lab.id("SW1"));
    // A configuration with several things wrong gives several findings.
    config(
        app.store(),
        &id,
        "running",
        "hostname SW1\nip http server\nsnmp-server community public RO\nenable password 7 104B\n\
         line vty 0 4\n transport input telnet\n",
    );
    scan(app, json!({})).await;
    let total = all_open(app).await.len();
    assert!(total >= 5, "{total}");

    let first = app.rpc("insights.list", json!({"limit": 2})).await.unwrap();
    assert_eq!(first["insights"].as_array().unwrap().len(), 2);
    let cursor = first["next_cursor"].as_str().expect("a cursor").to_string();
    // The counts are of everything that matches, not of the page.
    let counted: u64 = first["counts"]["by_severity"]
        .as_object()
        .unwrap()
        .values()
        .map(|count| count.as_u64().unwrap_or(0))
        .sum();
    assert_eq!(counted as usize, total);

    let second = app
        .rpc("insights.list", json!({"limit": 500, "cursor": cursor}))
        .await
        .unwrap();
    assert_eq!(second["insights"].as_array().unwrap().len(), total - 2);
    assert_eq!(second["next_cursor"], json!(null));
    assert_eq!(
        app.rpc("insights.list", json!({"cursor": "nonsense"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collections_and_configuration_changes_start_a_debounced_scan() {
    let lab = mismatched(With::default()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    // Shorten the debounce so the test does not have to wait out the default.
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"auto_scan_delay_ms": 50}}),
    )
    .await
    .unwrap();

    // Nothing has been scanned yet: the collections that built the fixture
    // happened before the debounce was shortened, but even so, a scan is not
    // synchronous with a collection.
    assert!(all_open(app).await.is_empty());

    // Collecting from a device publishes `collect.updated`; the watcher waits
    // for the debounce to pass and then scans by itself.
    collected(
        app,
        &one,
        Kind::Facts,
        vec![json!({"hostname": "SW1", "model": "WS-C2960-24TT-L", "os_version": "15.2(4)E10"})],
    );
    let mut found = Vec::new();
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        found = all_open(app).await;
        if !found.is_empty() {
            break;
        }
    }
    assert_eq!(
        found.len(),
        1,
        "the automatic scan found the mismatch without anybody asking: {found:?}"
    );
    assert!(
        app.rpc("insights.coverage", json!({})).await.unwrap()["scanned_at"]
            .as_i64()
            .is_some()
    );
}
