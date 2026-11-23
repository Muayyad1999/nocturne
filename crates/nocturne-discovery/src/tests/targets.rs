//! What a sweep may and may not be pointed at.

use serde_json::json;

use super::support;
use crate::targets::plan;

const NO_REFUSALS: &[String] = &[];

#[tokio::test]
async fn a_cidr_block_expands_without_its_network_and_broadcast_addresses() {
    let planned = plan("10.0.0.0/29", 4096, false, NO_REFUSALS).await.unwrap();
    let addresses: Vec<String> = planned.addresses.iter().map(ToString::to_string).collect();
    assert_eq!(
        addresses,
        [
            "10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5", "10.0.0.6"
        ]
    );
    assert!(!planned.public);
}

#[tokio::test]
async fn ranges_lists_and_single_addresses_mix_and_deduplicate() {
    let planned = plan(
        "10.0.1.5-10.0.1.8, 10.0.1.7 192.168.4.1;fd00::1",
        4096,
        false,
        NO_REFUSALS,
    )
    .await
    .unwrap();
    let addresses: Vec<String> = planned.addresses.iter().map(ToString::to_string).collect();
    assert_eq!(
        addresses,
        [
            "10.0.1.5",
            "10.0.1.6",
            "10.0.1.7",
            "10.0.1.8",
            "192.168.4.1",
            "fd00::1"
        ],
        "an address given twice is probed once"
    );
}

#[tokio::test]
async fn a_target_larger_than_the_cap_is_refused_rather_than_truncated() {
    let error = plan("10.0.0.0/16", 4096, false, NO_REFUSALS)
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("covers 65534 addresses")
            && error.message.contains("at most 4096")
            && error.message.contains("max_hosts"),
        "{}",
        error.message
    );
    // Several targets that are each small but together too large: also refused.
    let error = plan("10.0.0.0/24, 10.0.1.0/24", 300, false, NO_REFUSALS)
        .await
        .unwrap_err();
    assert!(
        error
            .message
            .contains("more than 300 addresses between them"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn a_range_larger_than_the_cap_is_refused_as_a_block_is_not_looked_up_as_a_name() {
    // Before, a range the engine would not expand was handed to the DNS
    // resolver as a host name, and came back with the engine's own limit.
    for (range, covers) in [
        ("10.0.0.1-10.0.255.255", "covers 65535 addresses"),
        ("fd00::1-fd00::1:0", "covers 65536 addresses"),
    ] {
        let error = plan(range, 4096, false, NO_REFUSALS).await.unwrap_err();
        assert_eq!(error.code, "invalid");
        assert!(
            error.message.contains(covers)
                && error.message.contains("at most 4096")
                && error.message.contains("max_hosts"),
            "{range}: {}",
            error.message
        );
    }
    // A mistyped address is refused in the engine's words, not resolved.
    let error = plan("10.0.0.256", 4096, false, NO_REFUSALS)
        .await
        .unwrap_err();
    assert!(
        error.message.contains("four numbers from 0 to 255"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn public_address_space_needs_asking_for_and_is_announced() {
    let error = plan("198.51.100.0/30", 4096, false, NO_REFUSALS)
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("public address"),
        "{}",
        error.message
    );
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details["needs"].as_str()),
        Some("allow_public")
    );

    let planned = plan("198.51.100.0/30", 4096, true, NO_REFUSALS)
        .await
        .unwrap();
    assert!(planned.public);
    assert!(
        planned
            .notes
            .iter()
            .any(|note| note.contains("public address space")),
        "the job log must say it swept public space: {:?}",
        planned.notes
    );
}

#[tokio::test]
async fn link_local_unique_local_and_carrier_space_count_as_internal() {
    for target in [
        "169.254.1.1",
        "fd00::5",
        "fe80::1",
        "100.64.3.3",
        "127.0.0.1",
    ] {
        let planned = plan(target, 16, false, NO_REFUSALS)
            .await
            .unwrap_or_else(|error| panic!("{target} should be allowed: {}", error.message));
        assert_eq!(planned.addresses.len(), 1);
        assert!(!planned.public, "{target}");
    }
    // A public IPv6 address is treated exactly like a public IPv4 one.
    let error = plan("2001:db8::1", 16, false, NO_REFUSALS)
        .await
        .unwrap_err();
    assert!(
        error.message.contains("public address"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn multicast_broadcast_and_the_unspecified_address_are_never_probed() {
    for (target, reason) in [
        ("224.0.0.1", "multicast"),
        ("255.255.255.255", "broadcast"),
        ("0.0.0.0", "unspecified"),
        ("ff02::1", "multicast"),
        ("::", "unspecified"),
    ] {
        let error = plan(target, 16, true, NO_REFUSALS).await.unwrap_err();
        assert!(
            error.message.contains(reason),
            "{target}: {}",
            error.message
        );
    }
}

#[tokio::test]
async fn the_never_scan_list_wins_over_the_request() {
    let refused = vec!["10.0.0.4/30".to_string(), "10.0.0.1".to_string()];
    let planned = plan("10.0.0.0/29", 4096, false, &refused).await.unwrap();
    let addresses: Vec<String> = planned.addresses.iter().map(ToString::to_string).collect();
    assert_eq!(addresses, ["10.0.0.2", "10.0.0.3"]);
    assert_eq!(planned.excluded, 4);
    assert!(
        planned.notes.iter().any(|note| note.contains("never-scan")),
        "{:?}",
        planned.notes
    );

    // A scan of nothing but refused space is an error, not an empty success.
    let error = plan("10.0.0.4/30", 4096, false, &refused)
        .await
        .unwrap_err();
    assert!(error.message.contains("never-scan"), "{}", error.message);

    let error = plan("10.0.0.0/29", 4096, false, &["nonsense".to_string()])
        .await
        .unwrap_err();
    assert!(
        error.message.contains("not an address or a CIDR block"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn nonsense_and_emptiness_are_refused_with_something_to_do_about_it() {
    let error = plan("   ", 16, false, NO_REFUSALS).await.unwrap_err();
    assert!(error.message.contains("CIDR"), "{}", error.message);

    let error = plan("10.0.0.0/24 what-is-this", 4096, false, NO_REFUSALS)
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(error.message.contains("what-is-this"), "{}", error.message);
}

#[tokio::test]
async fn a_host_name_is_resolved_and_the_log_says_what_it_resolved_to() {
    // `localhost` is in every hosts file; if this machine cannot resolve even
    // that, the test says so rather than passing quietly.
    let planned = match plan("localhost", 16, false, NO_REFUSALS).await {
        Ok(planned) => planned,
        Err(error) => {
            eprintln!("skipping: this machine cannot resolve `localhost`: {error}");
            return;
        }
    };
    assert!(
        planned
            .addresses
            .iter()
            .all(|address| address.is_loopback()),
        "{:?}",
        planned.addresses
    );
    assert!(
        planned
            .notes
            .iter()
            .any(|note| note.contains("`localhost` resolves to")),
        "{:?}",
        planned.notes
    );
}

#[tokio::test]
async fn the_scan_method_refuses_the_same_things_the_planner_does() {
    let (lab, _probes) = support::lab(&[]).await;
    let error = lab
        .app
        .rpc("discovery.scan", json!({"targets": "198.51.100.0/30"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");

    // Asking for public space without the installation allowing it is refused
    // before anything is planned at all.
    let error = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "198.51.100.0/30", "allow_public": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert!(
        error.message.contains("Settings → Discovery"),
        "{}",
        error.message
    );

    lab.app
        .rpc(
            "settings.set",
            json!({"section": "discovery", "value": {"allow_public": true}}),
        )
        .await
        .unwrap();
    // The setting allows it; the request still has to ask for it.
    let (scan_id, job) = support::scan(
        &lab.app,
        json!({"targets": "198.51.100.1", "icmp": true, "ports": "", "allow_public": true}),
    )
    .await
    .expect("the scan starts once public scanning is allowed");
    assert_eq!(job.state, nocturne_app::jobs::JobState::Succeeded);
    let scan = lab
        .app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    assert_eq!(scan["scan"]["allow_public"], true);
    assert!(
        job.log
            .iter()
            .any(|line| line.message.contains("public address space")),
        "the job log must record it: {:?}",
        job.log
    );
}
