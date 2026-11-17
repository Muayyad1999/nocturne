//! Sweeping a range: what answers, what is made of it, and what is refused.

use std::time::Duration;

use nocturne_app::jobs::JobState;
use serde_json::json;

use super::support::{self, FakeHost};
use crate::probe::SysInfo;

#[tokio::test]
async fn a_sweep_reports_what_each_host_answered_and_nothing_else() {
    let (lab, probes) = support::lab(&[]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;
    probes
        .host(
            "10.0.0.1",
            FakeHost::alive()
                .port(22, Some("SSH-2.0-TestIOS"))
                .port(23, None)
                .snmp("public", support::ios_sysinfo("CORE1"))
                .ptr("core1.campus.example"),
        )
        .host(
            "10.0.0.2",
            FakeHost::alive().port(80, Some("HTTP/1.0 200 OK")),
        )
        // 10.0.0.3 answers nothing at all.
        .host("10.0.0.4", FakeHost::default().port(22, None));

    let (scan_id, job) = support::scan(
        &lab.app,
        json!({
            "targets": "10.0.0.1-10.0.0.4",
            "ports": "22,23,80",
            "snmp_credential_ids": [community],
        }),
    )
    .await
    .unwrap();
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let addresses: Vec<&str> = hosts
        .iter()
        .map(|host| host["address"].as_str().unwrap())
        .collect();
    assert_eq!(
        addresses,
        ["10.0.0.1", "10.0.0.2", "10.0.0.4"],
        "an address that answered nothing is not a find"
    );

    let core = support::host_at(&hosts, "10.0.0.1");
    assert_eq!(core["via"], "icmp");
    assert_eq!(core["hostname"], "core1.campus.example");
    assert_eq!(core["snmp"]["sys_name"], "CORE1");
    assert_eq!(core["snmp"]["credential_id"], community);
    assert_eq!(core["ssh_banner"], "SSH-2.0-TestIOS");
    assert_eq!(core["profile"]["id"], "test_ios");
    assert_eq!(core["profile"]["method"], "snmp");
    assert_eq!(core["suggested"]["name"], "CORE1");
    assert_eq!(core["suggested"]["transport"], "ssh");
    assert_eq!(core["suggested"]["profile"], "test_ios");
    assert_eq!(core["device_id"], json!(null));
    let ports: Vec<u64> = core["open_ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|port| port["port"].as_u64().unwrap())
        .collect();
    assert_eq!(ports, [22, 23]);

    // A host that answers but matches no profile keeps its "unknown".
    let web = support::host_at(&hosts, "10.0.0.2");
    assert_eq!(web["profile"], json!(null));
    assert_eq!(web["snmp"], json!(null));
    assert_eq!(web["suggested"]["profile"], "");
    assert_eq!(
        web["suggested"]["transport"], "none",
        "no SSH and no Telnet means Nocturne would only monitor it"
    );
    assert_eq!(
        web["suggested"]["name"], "10.0.0.2",
        "nothing named it, so the address is the name"
    );

    // A host that ignores ICMP but accepts a connection is still a find.
    let quiet = support::host_at(&hosts, "10.0.0.4");
    assert_eq!(quiet["via"], "tcp/22");
    assert_eq!(quiet["ssh_banner"], json!(null), "it volunteered nothing");

    let scan = lab
        .app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    assert_eq!(scan["scan"]["planned"], 4);
    assert_eq!(scan["scan"]["probed"], 4);
    assert_eq!(scan["scan"]["found"], 3);
    assert_eq!(scan["scan"]["identified"], 1);
    assert_eq!(scan["scan"]["new"], 3);
    assert_eq!(scan["scan"]["state"], "finished");
}

#[tokio::test]
async fn every_field_of_a_find_carries_the_probe_that_produced_it() {
    let (lab, probes) = support::lab(&[]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;
    probes
        .host(
            "10.0.0.1",
            FakeHost::alive()
                .port(22, Some("SSH-2.0-TestIOS"))
                .snmp("public", support::ios_sysinfo("CORE1"))
                .ptr("core1.campus.example"),
        )
        .neighbour("10.0.0.1", "00:1b:21:3c:4d:5e", "reachable");

    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1", "ports": "22", "snmp_credential_ids": [community]}),
    )
    .await
    .unwrap();
    let hosts = support::hosts(&lab.app, &scan_id).await;
    let core = support::host_at(&hosts, "10.0.0.1");

    let alive = support::evidence_for(core, "alive");
    assert_eq!(alive.len(), 1);
    assert!(
        alive[0].starts_with("icmp: answered an echo request"),
        "{alive:?}"
    );

    assert!(
        support::evidence_for(core, "hostname")[0]
            .contains("reverse DNS says `core1.campus.example`")
    );
    assert!(
        support::evidence_for(core, "snmp")[0]
            .contains("the agent answered the credential `campus-ro`")
    );
    assert!(support::evidence_for(core, "ssh_banner")[0].contains("before any login"));
    assert!(
        support::evidence_for(core, "profile")[0].contains("sysDescr"),
        "the profile match must quote what matched: {:?}",
        support::evidence_for(core, "profile")
    );
    assert!(support::evidence_for(core, "mac")[0].contains("00:1b:21:3c:4d:5e"));
    assert!(
        support::evidence_for(core, "suggested.name")[0].contains("sysName"),
        "{:?}",
        support::evidence_for(core, "suggested.name")
    );

    // The MAC's vendor comes from the OUI registry and says so.
    assert_eq!(core["mac"], "00:1b:21:3c:4d:5e");
    let vendor = support::evidence_for(core, "vendor");
    assert!(
        vendor.iter().any(|line| line.starts_with("oui: ")),
        "{vendor:?}"
    );
}

#[tokio::test]
async fn a_host_that_answers_only_arp_is_found_and_labelled_as_such() {
    let (lab, probes) = support::lab(&[]).await;
    probes
        .host("10.0.0.1", FakeHost::alive())
        // 10.0.0.9 drops every probe but its MAC is in the neighbour table.
        .neighbour("10.0.0.9", "00:1b:21:aa:bb:cc", "reachable")
        .neighbour("10.0.0.1", "00:1b:21:11:22:33", "stale");

    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1-10.0.0.10", "ports": ""}),
    )
    .await
    .unwrap();
    let hosts = support::hosts(&lab.app, &scan_id).await;
    let silent = support::host_at(&hosts, "10.0.0.9");
    assert_eq!(silent["via"], "arp");
    assert_eq!(silent["alive"], true);
    assert_eq!(silent["rtt_ms"], json!(null), "nothing measured a time");
    assert!(
        support::evidence_for(silent, "alive")[0].contains("ignored every probe"),
        "{:?}",
        support::evidence_for(silent, "alive")
    );
}

#[tokio::test]
async fn a_find_that_is_already_in_the_inventory_is_matched_by_address_and_by_interface() {
    let (lab, probes) = support::lab(&["SW1"]).await;
    let sw1 = lab.id("SW1");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": sw1, "address": "10.0.0.2", "transport": "ssh"}),
        )
        .await
        .unwrap();
    // The same device also answers on a loopback address, which Nocturne only
    // knows because it collected the interface.
    nocturne_app::collect::store(
        &lab.app,
        &sw1,
        nocturne_drivers::model::Kind::Interfaces,
        vec![json!({"name": "Loopback0", "ipv4": ["10.255.0.2/32"]})],
        "cli",
    )
    .unwrap();

    probes
        .host("10.0.0.2", FakeHost::alive())
        .host("10.255.0.2", FakeHost::alive())
        .host("10.0.0.3", FakeHost::alive());

    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.2, 10.0.0.3, 10.255.0.2", "ports": ""}),
    )
    .await
    .unwrap();
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(support::host_at(&hosts, "10.0.0.2")["device_id"], sw1);
    assert_eq!(
        support::host_at(&hosts, "10.255.0.2")["device_id"],
        sw1,
        "an address on a collected interface is the same device, not a new find"
    );
    assert_eq!(support::host_at(&hosts, "10.255.0.2")["device_name"], "SW1");
    assert_eq!(
        support::host_at(&hosts, "10.0.0.3")["device_id"],
        json!(null)
    );

    let only_new = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": scan_id, "only_new": true}),
        )
        .await
        .unwrap();
    let addresses: Vec<&str> = only_new["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host["address"].as_str().unwrap())
        .collect();
    assert_eq!(addresses, ["10.0.0.3"]);
}

#[tokio::test]
async fn the_credentials_offered_are_the_ones_the_request_named_and_no_others() {
    let (lab, probes) = support::lab(&[]).await;
    let offered = support::snmp_credential(&lab.app, "offered", "public").await;
    let _withheld = support::snmp_credential(&lab.app, "withheld", "s3cret").await;
    probes.host(
        "10.0.0.1",
        FakeHost::alive().snmp("s3cret", support::ios_sysinfo("CORE1")),
    );

    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1", "ports": "", "snmp_credential_ids": [offered]}),
    )
    .await
    .unwrap();

    let tried: Vec<String> = probes
        .sent_of("snmp")
        .into_iter()
        .map(|sent| sent.credential)
        .collect();
    assert_eq!(
        tried,
        ["public"],
        "only the credential the request named may be sent anywhere"
    );
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(
        support::host_at(&hosts, "10.0.0.1")["snmp"],
        json!(null),
        "the agent only answers the credential that was not offered, so there is no SNMP"
    );
    assert!(
        probes.sent_of("tcp").is_empty() && probes.sent_of("echo").len() == 1,
        "nothing beyond the asked-for probes was sent"
    );
}

#[tokio::test]
async fn a_credential_without_an_snmp_community_is_refused_before_anything_is_sent() {
    let (lab, probes) = support::lab(&[]).await;
    let login = lab
        .app
        .rpc(
            "credentials.create",
            json!({"name": "cli-only", "username": "admin", "password": "pw"}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    probes.host("10.0.0.1", FakeHost::alive());

    let error = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.1", "snmp_credential_ids": [login]}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("no SNMP access"),
        "{}",
        error.message
    );
    assert!(
        probes.sent().is_empty(),
        "a refused scan sends nothing at all"
    );
}

#[tokio::test]
async fn a_scan_streams_its_finds_and_announces_when_it_is_done() {
    let (lab, probes) = support::lab(&[]).await;
    probes.host("10.0.0.1", FakeHost::alive());
    let mut events = support::Events::new(&lab.app);

    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1-10.0.0.3", "ports": ""}),
    )
    .await
    .unwrap();

    let found = events.of("discovery.found");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["scan_id"], scan_id);
    assert_eq!(found[0]["host"]["address"], "10.0.0.1");

    let finished = events.of("discovery.finished");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["summary"]["found"], 1);
    assert_eq!(finished[0]["summary"]["probed"], 3);
    assert_eq!(finished[0]["summary"]["state"], "finished");
}

#[tokio::test]
async fn cancelling_a_scan_stops_it_sending_and_keeps_what_it_already_found() {
    let (lab, probes) = support::lab(&[]).await;
    for last in 1..=40u8 {
        probes.host(&format!("10.0.0.{last}"), FakeHost::alive());
    }
    // Each probe takes long enough that the scan is still going when it is cancelled.
    probes.delay(Duration::from_millis(40));

    let started = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.1-10.0.0.40", "ports": "", "concurrency": 2}),
        )
        .await
        .unwrap();
    let (scan_id, job_id) = (
        started["scan_id"].as_str().unwrap().to_string(),
        started["job_id"].as_str().unwrap().to_string(),
    );
    // Cancel in the middle, and make "the middle" a condition rather than a
    // guess at the clock: wait until the scan has found something and is
    // still going. A fixed sleep here decided both of the assertions below by
    // how busy the machine was — too slow and nothing had been found yet, too
    // fast and the scan was already over, and both read as a broken cancel.
    // The fake probe sleeps 40 ms and the scan runs two at a time, so 40
    // addresses cannot be done inside 800 ms however fast the machine is.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let scan = lab
            .app
            .rpc("discovery.scan_get", json!({"scan_id": &scan_id}))
            .await
            .unwrap();
        let found = scan["hosts"].as_array().unwrap().len();
        let sent = probes.sent_of("echo").len();
        // Half way at the outside, so that the probes still in flight when
        // the cancel lands cannot take the total to 40 either.
        if found > 0 && sent <= 20 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "waited for the scan to be underway and to have found something to keep, \
             and it never was: {found} found, {sent} of 40 probes sent, state {}",
            scan["scan"]["state"]
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        lab.app
            .rpc("jobs.cancel", json!({"id": job_id}))
            .await
            .unwrap()["cancelled"],
        true
    );
    let job = nocturne_app::jobs::wait(&lab.app, &job_id).await.unwrap();
    assert_eq!(job.state, JobState::Cancelled);

    let sent_at_cancel = probes.sent_of("echo").len();
    // The job has already reported Cancelled, so nothing more is coming; this
    // window is there to catch a cancel that only stopped the reporting and
    // left the queue draining. Widening it can only make the check stricter.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        probes.sent_of("echo").len(),
        sent_at_cancel,
        "a cancelled scan stops sending rather than draining its queue"
    );
    assert!(sent_at_cancel < 40, "it was cancelled before the end");

    let scan = lab
        .app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    assert_eq!(scan["scan"]["state"], "cancelled");
    let found = scan["hosts"].as_array().unwrap().len();
    assert!(
        found > 0 && found < 40,
        "what was found before the stop is kept: {found}"
    );
    assert_eq!(scan["scan"]["found"], found as u64);
}

#[tokio::test(start_paused = true)]
async fn the_probe_rate_is_what_the_scan_was_given() {
    let (lab, probes) = support::lab(&[]).await;
    for last in 1..=20u8 {
        probes.host(&format!("10.0.0.{last}"), FakeHost::alive());
    }

    // 20 addresses, echo only, at 10 probes a second.
    let started = tokio::time::Instant::now();
    let (scan_id, job) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1-10.0.0.20", "ports": "", "rate": 10, "concurrency": 20}),
    )
    .await
    .unwrap();
    assert_eq!(job.state, JobState::Succeeded);
    let elapsed = started.elapsed();

    let sent = probes.sent_of("echo");
    assert_eq!(sent.len(), 20);
    assert!(
        elapsed >= Duration::from_millis(1900),
        "20 probes at 10/s cannot take less than ~2 s, took {elapsed:?}"
    );
    // No second of the scan may hold more than the rate allows.
    for window in sent.windows(11) {
        let span = window[10].at.duration_since(window[0].at);
        assert!(
            span >= Duration::from_millis(990),
            "11 probes inside {span:?} is faster than 10 per second"
        );
    }

    let scan = lab
        .app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    assert_eq!(
        scan["scan"]["probes_sent"], 20,
        "the record says how many packets the scan sent"
    );
}

#[tokio::test(start_paused = true)]
async fn the_settings_bound_what_a_request_may_ask_for() {
    let (lab, probes) = support::lab(&[]).await;
    for last in 1..=6u8 {
        probes.host(&format!("10.0.0.{last}"), FakeHost::alive());
    }
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "discovery", "value": {"rate": 4, "max_hosts": 6}}),
        )
        .await
        .unwrap();

    // Asking for a faster rate than the settings allow gets the settings'.
    let started = tokio::time::Instant::now();
    support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1-10.0.0.6", "ports": "", "rate": 10_000}),
    )
    .await
    .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(1200),
        "6 probes at the settings' 4/s take at least 1.25 s, took {:?}",
        started.elapsed()
    );

    // Asking for more addresses than the settings allow is refused.
    let error = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.0/24", "max_hosts": 60_000}),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("at most 6"), "{}", error.message);
}

#[tokio::test]
async fn settings_are_declared_with_the_documented_defaults_and_clamped() {
    let (lab, _probes) = support::lab(&[]).await;
    let stored = lab
        .app
        .rpc("settings.get", json!({"section": "discovery"}))
        .await
        .unwrap();
    assert_eq!(
        stored,
        json!({
            "max_hosts": 4096,
            "rate": 500,
            "concurrency": 64,
            "timeout_ms": 800,
            "ports": "22,23,80,443,161,8291",
            "allow_public": false,
            "never_scan": [],
            "keep_scans": 20,
            "max_crawl_depth": 3,
        })
    );

    lab.app
        .rpc(
            "settings.set",
            json!({"section": "discovery", "value": {
                "rate": 0, "concurrency": 0, "max_hosts": 10_000_000, "timeout_ms": 1, "keep_scans": 0,
            }}),
        )
        .await
        .unwrap();
    let settings = crate::settings::DiscoverySettings::load(&lab.app);
    assert_eq!(settings.rate, 1);
    assert_eq!(settings.concurrency, 1);
    assert_eq!(settings.max_hosts, 65_536);
    assert_eq!(settings.timeout_ms, 50);
    assert_eq!(settings.keep_scans, 1);
}

#[tokio::test]
async fn scans_are_listed_searchable_deletable_and_pruned() {
    let (lab, probes) = support::lab(&[]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;
    probes
        .host(
            "10.0.0.1",
            FakeHost::alive()
                .snmp("public", support::ios_sysinfo("CORE1"))
                .ptr("core1.campus.example"),
        )
        .host("10.0.0.2", FakeHost::alive().ptr("printer.campus.example"));
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "discovery", "value": {"keep_scans": 2}}),
        )
        .await
        .unwrap();

    let mut ids = Vec::new();
    for _ in 0..3 {
        let (scan_id, _) = support::scan(
            &lab.app,
            json!({"targets": "10.0.0.1-10.0.0.2", "ports": "", "snmp_credential_ids": [community]}),
        )
        .await
        .unwrap();
        ids.push(scan_id);
    }

    let listed = lab.app.rpc("discovery.scans", json!({})).await.unwrap();
    let kept: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|scan| scan["id"].as_str().unwrap())
        .collect();
    assert_eq!(kept.len(), 2, "keep_scans = 2 keeps the newest two");
    assert!(kept.contains(&ids[2].as_str()) && !kept.contains(&ids[0].as_str()));

    // The oldest scan's finds went with it.
    assert_eq!(
        lab.app
            .rpc("discovery.scan_get", json!({"scan_id": ids[0]}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );

    let searched = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": ids[2], "query": "printer"}),
        )
        .await
        .unwrap();
    let addresses: Vec<&str> = searched["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host["address"].as_str().unwrap())
        .collect();
    assert_eq!(addresses, ["10.0.0.2"]);

    assert_eq!(
        lab.app
            .rpc("discovery.scan_delete", json!({"scan_ids": [ids[2]]}))
            .await
            .unwrap()["deleted"],
        1
    );
    assert_eq!(
        lab.app
            .rpc("discovery.scans", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_scan_with_no_way_to_find_anything_is_refused() {
    let (lab, _probes) = support::lab(&[]).await;
    let error = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.1", "icmp": false, "ports": ""}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("no way to find"),
        "{}",
        error.message
    );

    let error = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.1", "ports": "not-a-port"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
}

#[tokio::test]
async fn an_ssh_banner_identifies_a_device_that_does_not_answer_snmp() {
    let (lab, probes) = support::lab(&[]).await;
    probes.host(
        "10.0.0.5",
        FakeHost::alive().port(22, Some("SSH-2.0-TestIOS")),
    );

    let (scan_id, _) = support::scan(&lab.app, json!({"targets": "10.0.0.5", "ports": "22"}))
        .await
        .unwrap();
    let hosts = support::hosts(&lab.app, &scan_id).await;
    let found = support::host_at(&hosts, "10.0.0.5");
    assert_eq!(found["profile"]["id"], "test_ios");
    assert_eq!(found["profile"]["method"], "banner");
    assert!(
        found["profile"]["evidence"]
            .as_str()
            .unwrap()
            .contains("SSH-2.0-TestIOS")
    );
}

#[tokio::test]
async fn an_unknown_banner_and_a_known_port_are_not_turned_into_a_device_type() {
    let (lab, probes) = support::lab(&[]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", "public").await;
    probes.host(
        "10.0.0.6",
        FakeHost::alive()
            // The ports a well-known vendor uses, and an SNMP agent that
            // describes a platform no profile knows.
            .port(22, Some("SSH-2.0-Unlisted_1.2"))
            .port(8291, None)
            .snmp(
                "public",
                SysInfo {
                    sys_descr: Some("Some Unlisted OS 1.2".into()),
                    sys_object_id: Some("1.3.6.1.4.1.6527001.1".into()),
                    sys_name: Some("edge-1".into()),
                    uptime_s: Some(42),
                },
            ),
    );

    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.6", "ports": "22,8291", "snmp_credential_ids": [community]}),
    )
    .await
    .unwrap();
    let hosts = support::hosts(&lab.app, &scan_id).await;
    let found = support::host_at(&hosts, "10.0.0.6");
    assert_eq!(
        found["profile"],
        json!(null),
        "no profile's detect rules matched, so there is no device type"
    );
    assert_eq!(found["suggested"]["role"], "", "a role is never guessed");
    assert_eq!(
        found["suggested"]["name"], "edge-1",
        "the device's own sysName is still its name"
    );
    assert_eq!(found["snmp"]["sys_descr"], "Some Unlisted OS 1.2");
}

#[tokio::test]
async fn the_community_that_answered_appears_in_nothing_a_user_can_read() {
    const SECRET: &str = "s3cr3t-c0mmun1ty";
    let (lab, probes) = support::lab(&[]).await;
    let community = support::snmp_credential(&lab.app, "campus-ro", SECRET).await;
    probes.host(
        "10.0.0.1",
        FakeHost::alive()
            .port(22, Some("SSH-2.0-TestIOS"))
            .snmp(SECRET, support::ios_sysinfo("CORE1")),
    );
    let mut events = support::Events::new(&lab.app);

    let (scan_id, job) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1", "ports": "22", "snmp_credential_ids": [community]}),
    )
    .await
    .unwrap();
    lab.app
        .rpc("discovery.add", json!({"scan_id": scan_id}))
        .await
        .unwrap();

    // Everything a person, a log reader or a model could get at.
    let scan = lab
        .app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    let devices = lab.app.rpc("devices.list", json!({})).await.unwrap();
    let audit = lab.app.rpc("audit.list", json!({})).await.unwrap();
    let mut readable = vec![
        serde_json::to_string(&scan).unwrap(),
        serde_json::to_string(&devices).unwrap(),
        serde_json::to_string(&audit).unwrap(),
        serde_json::to_string(&job).unwrap(),
    ];
    readable.extend(
        events
            .drain()
            .iter()
            .map(|event| serde_json::to_string(event).unwrap()),
    );
    for text in &readable {
        assert!(
            !text.contains(SECRET),
            "the community leaked into something readable:\n{text}"
        );
    }
    // What is kept is which credential answered, by id and by name.
    assert_eq!(scan["hosts"][0]["snmp"]["credential_id"], community);
    assert_eq!(scan["hosts"][0]["snmp"]["credential_name"], "campus-ro");
}
