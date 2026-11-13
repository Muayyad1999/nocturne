//! The same pipeline against real packets.
//!
//! These tests use the probe layer that ships: real ICMP through
//! `nocturne-netprobe`, a real TCP listener whose banner is grabbed over a
//! real connection, and a real SNMP agent (`nocturne_snmp::Agent`) answering
//! real v2c GETs on an ephemeral port. Everything is on loopback and nothing
//! binds a fixed port, so they run anywhere; the ICMP ones say why they are
//! skipped on a machine that cannot send an echo request at all.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use nocturne_snmp::{Agent, Auth, Oid, Value as SnmpValue, oids};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::support;
use crate::probe::{Probes, RealProbes};

const TIMEOUT: Duration = Duration::from_secs(2);

/// An SNMP agent that answers as a device of the test profile's platform.
async fn agent(community: &str, sys_name: &str) -> (Agent, SocketAddr) {
    let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), community)
        .await
        .expect("an SNMP agent on loopback");
    let address = agent.local_addr();
    let mut view: BTreeMap<Oid, SnmpValue> = BTreeMap::new();
    view.insert(
        oids::SYS_DESCR.parse().unwrap(),
        SnmpValue::from(
            "Test IOS Software, C2960 Software (C2960-LANBASEK9-M), Version 15.0(2)SE11",
        ),
    );
    view.insert(
        oids::SYS_OBJECT_ID.parse().unwrap(),
        SnmpValue::Oid("1.3.6.1.4.1.99999.1.516".parse().unwrap()),
    );
    view.insert(oids::SYS_NAME.parse().unwrap(), SnmpValue::from(sys_name));
    view.insert(
        oids::SYS_UP_TIME.parse().unwrap(),
        SnmpValue::TimeTicks(1_234_500),
    );
    agent.set_view(view);
    tokio::spawn(agent.clone().run());
    (agent, address)
}

/// A listener that says what an SSH server says and then closes.
async fn banner_listener(banner: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(banner.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });
    port
}

#[tokio::test]
async fn a_real_agent_is_read_with_real_snmp() {
    let (_agent, address) = agent("public", "LIVE-SW1").await;
    let probes = RealProbes::default();

    let info = probes
        .snmp(
            address,
            Auth::V2c {
                community: "public".into(),
            },
            TIMEOUT,
        )
        .await
        .expect("the agent answers");
    assert_eq!(info.sys_name.as_deref(), Some("LIVE-SW1"));
    assert!(
        info.sys_descr
            .as_deref()
            .is_some_and(|descr| descr.contains("C2960")),
        "{info:?}"
    );
    assert_eq!(
        info.sys_object_id.as_deref(),
        Some("1.3.6.1.4.1.99999.1.516")
    );
    assert_eq!(
        info.uptime_s,
        Some(12_345),
        "TimeTicks are hundredths of a second"
    );

    // The wrong community gets no answer, and no answer is not an identity.
    let refused = probes
        .snmp(
            address,
            Auth::V2c {
                community: "wrong".into(),
            },
            Duration::from_millis(400),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
}

#[tokio::test]
async fn a_real_banner_is_read_without_sending_anything_but_the_connection() {
    let port = banner_listener("SSH-2.0-TestIOS\r\n").await;
    let probes = RealProbes::default();

    let answers = probes
        .ports(
            "127.0.0.1".parse().unwrap(),
            &[port, port + 1],
            TIMEOUT,
            true,
        )
        .await;
    let open: Vec<_> = answers.iter().filter(|answer| answer.open).collect();
    assert_eq!(open.len(), 1, "{answers:?}");
    assert_eq!(open[0].port, port);
    assert_eq!(open[0].banner.as_deref(), Some("SSH-2.0-TestIOS"));

    // Asked not to grab banners, it connects and says nothing about content.
    let quiet = probes
        .ports("127.0.0.1".parse().unwrap(), &[port], TIMEOUT, false)
        .await;
    assert!(quiet[0].open && quiet[0].banner.is_none());
}

#[tokio::test]
async fn real_echo_requests_reach_loopback() {
    let probes = RealProbes::default();
    let address = "127.0.0.1".parse().unwrap();
    match probes.echo(address, TIMEOUT).await {
        Some(rtt) => assert!((0.0..2000.0).contains(&rtt), "{rtt}"),
        None => eprintln!(
            "skipping the assertion: this machine cannot send an echo request at all \
             (no unprivileged ICMP socket, no raw socket, no `ping`)"
        ),
    }
    // An address nothing is listening on: silence, never a made-up reply.
    assert!(
        !probes
            .ports(address, &[1], Duration::from_millis(500), true)
            .await[0]
            .open
    );
}

#[tokio::test]
async fn a_scan_over_real_packets_identifies_a_real_agent() {
    let (lab, _fake) = support::lab(&[]).await;
    // The probe layer that ships, not the test one.
    crate::db::state(&lab.app)
        .probes
        .set(std::sync::Arc::new(RealProbes::default()));

    let (_agent, snmp) = agent("public", "LIVE-CORE").await;
    let port = banner_listener("SSH-2.0-TestIOS\r\n").await;
    let community = support::snmp_credential(&lab.app, "live-ro", "public").await;

    let (scan_id, job) = support::scan(
        &lab.app,
        json!({
            "targets": "127.0.0.1",
            "ports": format!("{port}"),
            "snmp_credential_ids": [community],
            "snmp_port": snmp.port(),
            "resolve_names": true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        job.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{:?}",
        job.error
    );

    let hosts = support::hosts(&lab.app, &scan_id).await;
    let found = support::host_at(&hosts, "127.0.0.1");
    assert_eq!(found["alive"], true);
    assert_eq!(found["snmp"]["sys_name"], "LIVE-CORE");
    assert_eq!(found["snmp"]["credential_id"], community);
    assert_eq!(
        found["profile"]["id"], "test_ios",
        "a real sysDescr, matched by the profile registry's own detect rules"
    );
    assert_eq!(found["suggested"]["name"], "LIVE-CORE");
    let banners: Vec<&str> = found["open_ports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["banner"].as_str())
        .collect();
    assert_eq!(banners, ["SSH-2.0-TestIOS"]);
    // Nothing on loopback suggests SSH or Telnet, so Nocturne would not
    // pretend it can log in.
    assert_eq!(found["suggested"]["transport"], "none");
}

#[tokio::test]
async fn suggest_offers_this_machines_own_networks() {
    let (lab, _fake) = support::lab(&[]).await;
    crate::db::state(&lab.app)
        .probes
        .set(std::sync::Arc::new(RealProbes::default()));

    let suggestions = lab.app.rpc("discovery.suggest", json!({})).await.unwrap();
    assert_eq!(suggestions["local"], true);
    assert_eq!(suggestions["max_hosts"], 4096);

    let networks = suggestions["networks"].as_array().unwrap();
    if networks.is_empty() {
        eprintln!("skipping the assertions: this machine has no private address of its own");
        return;
    }
    for network in networks {
        let cidr = network["cidr"].as_str().unwrap();
        let block: ipnet::IpNet = cidr.parse().expect("a CIDR block");
        assert_eq!(
            block.to_string(),
            cidr,
            "a network is offered as its block, not as this machine's address in it"
        );
        assert!(
            network["hosts"].as_u64().unwrap() > 0,
            "a network with no host addresses is no place to start: {network}"
        );
        assert!(!network["interface"].as_str().unwrap().is_empty() && network["interface"] != "lo");
        // Whether it needs permission is stated, and matches what a scan of
        // it would actually do.
        let public = network["needs_allow_public"].as_bool().unwrap();
        assert_eq!(
            public,
            !crate::targets::is_internal(block.addr()),
            "{network}"
        );
        let planned = crate::targets::plan(cidr, 65_536, public, &[]).await;
        assert!(
            planned.is_ok(),
            "{cidr} is offered as a starting point, so it must be scannable: {:?}",
            planned.err()
        );
        if public {
            assert!(
                crate::targets::plan(cidr, 65_536, false, &[])
                    .await
                    .is_err(),
                "{cidr} is public, so a scan of it must ask for that"
            );
        }
    }
}
