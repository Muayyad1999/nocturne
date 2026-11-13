//! Reading a device's own log buffer, and telling a device where to send.

mod support;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

/// `show logging` as IOS and IOS-XE print it: the counters first, then the
/// buffer. The buffer lines carry `service timestamps log datetime msec`.
const SHOW_LOGGING: &str = "\
Syslog logging: enabled (0 messages dropped, 3 messages rate-limited, 0 flushes, 0 overruns, xml disabled, filtering disabled)
    Console logging: level debugging, 31 messages logged, xml disabled
    Buffer logging: level debugging, 31 messages logged, xml disabled
    Trap logging: level informational, 33 message lines logged

Log Buffer (8192 bytes):
*Nov 13 09:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)
*Nov 13 09:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down
*Nov 13 09:12:11.002: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to down
";

/// The same device with `service timestamps log uptime` instead.
const SHOW_LOGGING_UPTIME: &str = "\
Log Buffer (8192 bytes):
00:02:11: %SYS-5-RESTART: System restarted --
00:02:14: %SYS-6-LOGGINGHOST_STARTSTOP: Logging to host 10.0.0.1 port 514 started
";

async fn switch_with_buffer(output: &str) -> (nocturne_app::testing::Lab, String) {
    let lab = lab(&["SW1"]).await;
    let id = lab.id("SW1");
    set_address(&lab.app, &id, "127.0.0.1").await;
    let owned = output.to_string();
    lab.fake("SW1").with(|device| {
        device.outputs.insert("show logging".into(), owned);
    });
    (lab, id)
}

#[tokio::test]
async fn a_device_buffer_is_read_stored_and_dated_by_the_device() {
    let (lab, id) = switch_with_buffer(SHOW_LOGGING).await;
    let app = &lab.app;

    let pulled = app
        .rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(pulled["command"], json!("show logging"));
    assert_eq!(pulled["read"], json!(3), "{pulled:#}");
    assert_eq!(pulled["stored"], json!(3));
    assert_eq!(pulled["duplicates"], json!(0));
    assert_eq!(pulled["dated_by_device"], json!(3));

    let entries = query(app, json!({"device_id": id, "limit": 10})).await;
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[0]["body"]["mnemonic"],
        json!("LINEPROTO-5-UPDOWN"),
        "the newest line of the buffer is the newest entry: {entries:#?}"
    );
    assert_eq!(entries[0]["body"]["via"], json!("pull"));
    assert_eq!(entries[0]["body"]["ts_source"], json!("device"));
    assert_eq!(
        entries[0]["device_id"],
        json!(id),
        "a pulled line needs no attribution: the session says whose it is"
    );
    assert_eq!(entries[0]["severity"], json!(5));

    let now = nocturne_app::util::now_ms();
    let stamped = entries[0]["ts"].as_i64().unwrap();
    assert!(
        stamped < now && now - stamped < 400 * 86_400_000,
        "the device's own date was used, not the moment of the pull"
    );
    assert!(
        entries[2]["ts"].as_i64().unwrap() < entries[0]["ts"].as_i64().unwrap(),
        "and the buffer's order survived"
    );
}

#[tokio::test]
async fn an_uptime_stamped_buffer_is_dated_by_the_pull_and_says_so() {
    let (lab, id) = switch_with_buffer(SHOW_LOGGING_UPTIME).await;
    let app = &lab.app;

    let pulled = app
        .rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(pulled["read"], json!(2));
    assert_eq!(
        pulled["dated_by_device"],
        json!(0),
        "`00:02:11` is two minutes after this device booted, not two in the morning"
    );
    let entries = query(app, json!({"device_id": id, "limit": 10})).await;
    assert_eq!(entries[0]["body"]["ts_source"], json!("received"));
    let now = nocturne_app::util::now_ms();
    assert!((now - entries[0]["ts"].as_i64().unwrap()).abs() < 60_000);
}

#[tokio::test]
async fn pulling_the_same_buffer_twice_stores_nothing_the_second_time() {
    let (lab, id) = switch_with_buffer(SHOW_LOGGING).await;
    let app = &lab.app;

    app.rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    let again = app
        .rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(again["read"], json!(3));
    assert_eq!(again["stored"], json!(0));
    assert_eq!(again["duplicates"], json!(3));
    assert_eq!(
        query(app, json!({"device_id": id, "limit": 10}))
            .await
            .len(),
        3
    );

    // A new line in the buffer is stored; the old ones are not stored again.
    let grown = format!(
        "{SHOW_LOGGING}*Nov 13 09:13:00.000: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/4, changed state to down\n"
    );
    lab.fake("SW1").with(|device| {
        device.outputs.insert("show logging".into(), grown);
    });
    let third = app
        .rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(
        (third["read"].as_i64(), third["stored"].as_i64()),
        (Some(4), Some(1))
    );
}

#[tokio::test]
async fn an_empty_buffer_is_reported_as_empty_rather_than_as_nothing_happening() {
    let (lab, id) = switch_with_buffer("Log Buffer (8192 bytes):\n\n").await;
    let app = &lab.app;

    let pulled = app
        .rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(pulled["read"], json!(0));
    assert_eq!(pulled["stored"], json!(0));
    assert!(
        pulled["note"].as_str().unwrap().contains("no log lines"),
        "{pulled:#}"
    );
    assert_eq!(pulled["oldest_at"], Value::Null);
}

#[tokio::test]
async fn a_device_that_refuses_the_command_is_an_error_not_an_empty_buffer() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    // No canned output: the fake answers with its error pattern.
    lab.fake("SW1").with(|device| {
        device.outputs.insert(
            "show logging".into(),
            "% Invalid input detected at '^' marker.".into(),
        );
    });
    let err = app
        .rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "device_error");
    assert!(err.message.contains("show logging"), "{err}");
    assert!(query(app, json!({"device_id": id})).await.is_empty());
}

#[tokio::test]
async fn pulled_lines_do_not_raise_signals_because_they_are_history() {
    let (lab, id) = switch_with_buffer(SHOW_LOGGING).await;
    let app = &lab.app;
    let mut events = app.subscribe();

    app.rpc("logs.pull", json!({"device_id": id}))
        .await
        .unwrap();
    let published = drain_all(&mut events);
    assert!(
        topic(&published, "logs.config_changed").is_empty(),
        "backing a switch up because its buffer still remembers last week's \
         change is not what the configuration feature is being told"
    );
    assert!(topic(&published, "logs.signal").is_empty());
    // But the line is there to be found.
    let found = query(app, json!({"device_id": id, "text": "CONFIG_I"})).await;
    assert_eq!(found.len(), 1);
}

#[tokio::test]
async fn device_setup_writes_the_lines_for_the_platform_without_applying_them() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let (udp, _) = start_receiver(app).await;

    let setup = app
        .rpc("logs.device_setup", json!({"device_id": id}))
        .await
        .unwrap();
    let lines: Vec<&str> = setup["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "service timestamps log datetime msec show-timezone",
            "logging origin-id hostname",
            "logging trap informational",
            &format!("logging host 127.0.0.1 transport udp port {udp}"),
        ],
        "the port it is really listening on, not the one in the settings"
    );
    assert_eq!(setup["udp_port"], json!(udp));
    assert_eq!(setup["address"], json!("127.0.0.1"));
    assert!(
        setup["note"].as_str().unwrap().contains("changes.create"),
        "{setup:#}"
    );
    assert!(
        setup["note"].as_str().unwrap().contains("below 1024"),
        "an unprivileged port needs explaining: {setup:#}"
    );
    // Nothing was sent to the device.
    assert!(
        lab.fake("SW1")
            .with(|device| device.seen.clone())
            .is_empty(),
        "device_setup does not touch the device"
    );
}

#[tokio::test]
async fn device_setup_knows_routeros_and_refuses_what_it_does_not_know() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    let router = app
        .rpc(
            "devices.create",
            json!({"name": "RB1", "address": "127.0.0.1", "profile": "mikrotik_routeros"}),
        )
        .await
        .unwrap();
    let setup = app
        .rpc(
            "logs.device_setup",
            json!({"device_id": router["id"].as_str().unwrap()}),
        )
        .await
        .unwrap();
    let lines = setup["lines"].as_array().unwrap();
    assert!(
        lines[0]
            .as_str()
            .unwrap()
            .starts_with("/system logging action set [find name=remote] remote=127.0.0.1"),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line
            .as_str()
            .unwrap()
            .contains("topics=error action=remote")),
        "{lines:#?}"
    );
    assert!(
        setup["note"].as_str().unwrap().contains("switched off"),
        "the receiver is not running, and the lines would send into nothing: {setup:#}"
    );

    // A platform whose syslog configuration this crate does not know.
    nocturne_app::profiles::save(
        app,
        "id: junos_like\nname: Something Else\nvendor: Other\nos: Other\nconfig:\n  syntax: set\n",
    )
    .expect("the profile is stored");
    let other = app
        .rpc(
            "devices.create",
            json!({"name": "OTHER", "address": "10.0.0.9", "profile": "junos_like"}),
        )
        .await
        .unwrap();
    let err = app
        .rpc(
            "logs.device_setup",
            json!({"device_id": other["id"].as_str().unwrap()}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "unsupported");
    assert!(err.message.contains("Something Else"), "{err}");
}

#[tokio::test]
async fn a_device_without_a_profile_is_asked_about_rather_than_guessed_at() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let device = app
        .rpc(
            "devices.create",
            json!({"name": "NEW", "address": "10.0.0.8", "profile": ""}),
        )
        .await
        .unwrap();
    let err = app
        .rpc(
            "logs.device_setup",
            json!({"device_id": device["id"].as_str().unwrap()}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("no device profile yet"), "{err}");
}
