//! Receiving: real datagrams and real connections into a real receiver.

mod support;

use nocturne_app::collect;
use nocturne_drivers::model::Kind;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

/// What a Catalyst sends with `logging host` configured: facility local7
/// (the IOS default) and the severity of the message itself, 187 = 23 * 8 + 3.
const IOS_LINK_DOWN: &str = "<187>45: *Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down";
const RFC5424: &str = "<34>1 2026-11-13T09:13:01.123Z SW1 sshd 1234 ID47 [exampleSDID@32473 iut=\"3\"] Accepted publickey for admin from 10.0.0.5 port 54321";
const ROUTEROS: &str =
    "<30>Nov 13 09:13:01 MikroTik system,info,account user admin logged in from 10.0.0.5 via ssh";
const NO_PRIORITY: &str =
    "*Mar  1 00:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)";

#[tokio::test]
async fn udp_lines_in_every_form_are_parsed_stored_and_attributed() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let (udp, _) = start_receiver(app).await;

    send_udp(
        "127.0.0.1",
        udp,
        &[IOS_LINK_DOWN, RFC5424, ROUTEROS, NO_PRIORITY],
    )
    .await;
    wait_until_settled(app, 4).await;

    let entries = query(app, json!({"device_id": id, "limit": 10})).await;
    assert_eq!(entries.len(), 4, "{entries:#?}");
    // Newest first; they were sent in order over one socket.
    let messages: Vec<&str> = entries
        .iter()
        .map(|entry| entry["message"].as_str().unwrap_or_default())
        .collect();
    assert!(
        messages[3].contains("changed state to down"),
        "the mnemonic is peeled off the message: {messages:#?}"
    );

    let link_down = &entries[3];
    assert_eq!(
        link_down["severity"],
        json!(3),
        "the priority says local7.error, and so does the mnemonic"
    );
    assert_eq!(link_down["body"]["facility"], json!(23));
    assert_eq!(link_down["body"]["facility_name"], json!("local7"));
    assert_eq!(link_down["body"]["mnemonic"], json!("LINK-3-UPDOWN"));
    assert_eq!(link_down["body"]["sequence"], json!(45));
    assert_eq!(link_down["body"]["via"], json!("syslog"));
    assert_eq!(link_down["source"], json!("127.0.0.1"));
    assert_eq!(link_down["device_id"], json!(id));

    let rfc5424 = &entries[2];
    assert_eq!(rfc5424["body"]["app"], json!("sshd"));
    assert_eq!(rfc5424["body"]["host"], json!("SW1"));
    assert_eq!(rfc5424["severity"], json!(2));
    assert_eq!(
        rfc5424["body"]["timestamp"],
        json!("2026-11-13T09:13:01.123Z")
    );

    let routeros = &entries[1];
    assert_eq!(
        routeros["body"]["mnemonic"],
        json!("system-6-account"),
        "RouterOS topics become the same facility-severity-name key a Cisco \
         mnemonic does, so one grouping works for both"
    );
    assert!(
        routeros["message"]
            .as_str()
            .unwrap()
            .contains("logged in from"),
        "{routeros:#}"
    );

    let no_priority = &entries[0];
    assert_eq!(
        no_priority["severity"],
        json!(5),
        "a line without a <PRI> keeps the severity the vendor wrote into its mnemonic"
    );
    assert_eq!(
        no_priority["body"].get("facility"),
        None,
        "and claims no facility"
    );
}

#[tokio::test]
async fn tcp_lines_arrive_with_either_framing_and_a_huge_line_is_bounded() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let (_, tcp) = start_receiver(app).await;

    // RFC 6587: newline framing, then octet counting, on one connection.
    let counted = format!("{} {IOS_LINK_DOWN}", IOS_LINK_DOWN.len());
    let huge = format!("<189>{}", "x".repeat(70_000));
    let payload = format!("{RFC5424}\n{counted}{huge}\n");
    send_tcp("127.0.0.1", tcp, payload.as_bytes()).await;
    wait_until_settled(app, 3).await;

    let entries = query(app, json!({"device_id": id, "limit": 10})).await;
    assert_eq!(entries.len(), 3, "{entries:#?}");
    assert!(
        entries[2]["message"]
            .as_str()
            .unwrap()
            .contains("Accepted publickey")
    );
    assert_eq!(entries[1]["body"]["mnemonic"], json!("LINK-3-UPDOWN"));
    let cut = entries[0]["message"].as_str().unwrap();
    assert!(
        (60_000..=65_536).contains(&cut.len()),
        "an endless line is cut, not stored whole or lost: {} bytes",
        cut.len()
    );
}

#[tokio::test]
async fn a_line_nobody_claims_is_kept_with_the_reason_and_the_address_is_listed() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "10.99.99.99").await;
    let (udp, _) = start_receiver(app).await;

    send_udp("127.0.0.1", udp, &[IOS_LINK_DOWN]).await;
    wait_until_settled(app, 1).await;

    let entries = query(app, json!({"limit": 10})).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["device_id"],
        Value::Null,
        "it is stored, with no device, rather than filed against a guess"
    );

    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    let source = &sources.as_array().expect("an array")[0];
    assert_eq!(source["address"], json!("127.0.0.1"));
    assert_eq!(source["count"], json!(1));
    assert_eq!(source["device_id"], Value::Null);
    assert!(
        source["reason"]
            .as_str()
            .unwrap()
            .contains("no device in the inventory has the address"),
        "{source:#}"
    );

    // Giving a device that address fixes the list without waiting for a line.
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    let source = &sources.as_array().expect("an array")[0];
    assert_eq!(source["device_id"], json!(lab.id("SW1")));
    assert_eq!(source["device"], json!("SW1"));
    assert_eq!(source["reason"], json!(""));
}

#[tokio::test]
async fn unknown_sources_can_be_dropped_and_the_drop_is_still_counted() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let (udp, _) = start_receiver(app).await;
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"unknown_sources": "drop"}}),
    )
    .await
    .unwrap();
    // The settings change restarts the receiver, so ask where it is now.
    let status = wait_for(app, "the receiver to come back", |status| {
        status["syslog"]["listening"]
            .as_array()
            .is_some_and(|l| l.len() == 2)
    })
    .await;
    let udp_now = status["syslog"]["listening"][0]
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_eq!(udp_now, udp, "nothing about the sockets changed");

    send_udp("127.0.0.1", udp, &[IOS_LINK_DOWN]).await;
    let status = wait_until_settled(app, 1).await;
    assert_eq!(status["syslog"]["stored"], json!(0));
    assert_eq!(status["syslog"]["dropped"], json!(1));
    assert!(query(app, json!({"limit": 10})).await.is_empty());

    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    assert_eq!(
        sources[0]["dropped"],
        json!(1),
        "an address whose lines are all thrown away is still visible: {sources:#}"
    );
}

#[tokio::test]
async fn the_receiver_stays_off_until_asked_and_says_why_it_cannot_bind() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["syslog"]["enabled"], json!(false));
    assert_eq!(status["syslog"]["listening"], json!([]));
    assert_eq!(status["syslog"]["error"], Value::Null);
    assert_eq!(
        status["traps"],
        json!({"enabled": false, "implemented": false,
               "error": "the SNMP trap receiver is not implemented in this build"}),
        "a receiver this build does not have says so instead of looking switched off"
    );

    // Hold a port, then ask the receiver for it.
    let taken = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"syslog": {"enabled": true, "udp_port": port, "tcp_port": 0, "bind": "127.0.0.1"}}}),
    )
    .await
    .unwrap();
    let status = wait_for(app, "the receiver to report the clash", |status| {
        !status["syslog"]["error"].is_null()
    })
    .await;
    let error = status["syslog"]["error"].as_str().unwrap();
    assert!(
        error.contains(&port.to_string()) && error.contains("already using it"),
        "the error names the port and what is wrong: {error}"
    );
    assert_eq!(status["syslog"]["enabled"], json!(true));
    assert_eq!(status["syslog"]["listening"], json!([]));

    // An address this machine does not have is refused with advice.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"syslog": {"bind": "not-an-address"}}}),
    )
    .await
    .unwrap();
    let status = wait_for(app, "the receiver to refuse the address", |status| {
        status["syslog"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("not-an-address"))
    })
    .await;
    assert!(
        status["syslog"]["error"]
            .as_str()
            .unwrap()
            .contains("0.0.0.0"),
        "{status:#}"
    );
}

#[tokio::test]
async fn a_credential_in_a_line_is_replaced_before_the_line_is_stored() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let (udp, _) = start_receiver(app).await;

    // IOS logs the command that was typed when `archive log config` is on.
    let line = "<189>46: *Mar  1 00:13:00.000: %PARSER-5-CFGLOG_LOGGEDCMD: User:admin  logged command:snmp-server community Str0ngR0 RO";
    send_udp("127.0.0.1", udp, &[line]).await;
    wait_until_settled(app, 1).await;

    let entries = query(app, json!({"limit": 5})).await;
    let stored = serde_json::to_string(&entries).unwrap();
    assert!(
        !stored.contains("Str0ngR0"),
        "the community string reached the database: {stored}"
    );
    assert!(stored.contains("<secret:"), "{stored}");
    assert!(
        entries[0]["body"]["raw"]
            .as_str()
            .unwrap()
            .contains("<secret:"),
        "the raw line is redacted too, not only the message"
    );
}

#[tokio::test]
async fn attribution_falls_back_to_a_collected_interface_address() {
    if !loopback_alias("127.0.0.2").await {
        eprintln!("skipped: this machine does not allow a second loopback address");
        return;
    }
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    // Managed through one address, logging from another (a loopback the
    // device happens to source syslog from — the usual Cisco behaviour).
    set_address(app, &id, "10.99.99.99").await;
    collect::store(
        app,
        &id,
        Kind::Interfaces,
        vec![json!({"name": "Loopback0", "ipv4": ["127.0.0.2/32"]})],
        "test",
    )
    .unwrap();
    let (udp, _) = start_receiver(app).await;

    send_udp("127.0.0.2", udp, &[IOS_LINK_DOWN]).await;
    wait_until_settled(app, 1).await;

    let entries = query(app, json!({"limit": 5})).await;
    assert_eq!(entries[0]["device_id"], json!(id));
    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    assert_eq!(sources[0]["address"], json!("127.0.0.2"));
    assert_eq!(sources[0]["device_id"], json!(id));
}

#[tokio::test]
async fn two_devices_with_one_address_are_refused_rather_than_guessed() {
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    set_address(app, &lab.id("SW2"), "127.0.0.1").await;
    let (udp, _) = start_receiver(app).await;

    send_udp("127.0.0.1", udp, &[IOS_LINK_DOWN]).await;
    wait_until_settled(app, 1).await;

    let entries = query(app, json!({"limit": 5})).await;
    assert_eq!(entries[0]["device_id"], Value::Null);
    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    let reason = sources[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("2 devices") && reason.contains("SW1") && reason.contains("SW2"),
        "the reason names both: {reason}"
    );
}

#[tokio::test]
async fn a_line_that_names_an_interface_is_tied_to_that_interface() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    collect::store(
        app,
        &id,
        Kind::Interfaces,
        vec![
            json!({"name": "GigabitEthernet1/0/3"}),
            json!({"name": "GigabitEthernet1/0/4"}),
        ],
        "test",
    )
    .unwrap();
    let (udp, _) = start_receiver(app).await;

    // The second line names a port this device does not have.
    let elsewhere = IOS_LINK_DOWN.replace("GigabitEthernet1/0/3", "GigabitEthernet9/9/9");
    send_udp("127.0.0.1", udp, &[IOS_LINK_DOWN, &elsewhere]).await;
    wait_until_settled(app, 2).await;

    let tied = query(app, json!({"interface": "Gi1/0/3", "limit": 5})).await;
    assert_eq!(tied.len(), 1, "asked for in the short spelling: {tied:#?}");
    assert_eq!(tied[0]["body"]["interface"], json!("GigabitEthernet1/0/3"));

    let all = query(app, json!({"limit": 5})).await;
    assert_eq!(
        all[0]["body"].get("interface"),
        None,
        "a port the device does not have resolves to nothing rather than to a name"
    );

    let top = app
        .rpc("logs.top", json!({"by": "interface", "limit": 5}))
        .await
        .unwrap();
    assert_eq!(top[0]["key"], json!("GigabitEthernet1/0/3"));
    assert_eq!(top[0]["count"], json!(1));
}
