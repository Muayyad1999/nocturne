//! The servers: the whole lifecycle, the root they are confined to, and what
//! they say when a port cannot be had.
//!
//! The TFTP server is tested with a client written in `support` rather than
//! with itself, so the thing under test is the protocol on the wire. Every
//! port is ephemeral (`port: 0`), so these run anywhere and never clash with
//! another test or another agent on the machine.

use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::net::UdpSocket;

use super::support;

#[tokio::test]
async fn a_tftp_server_serves_its_root_and_nothing_else() {
    let lab = support::lab(&[]).await;
    let mut events = support::Events::new(&lab.app);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("firmware.bin"), b"a firmware image").unwrap();
    // A file that is not in the root, to try to reach from outside it.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"not yours").unwrap();

    let started = lab
        .app
        .rpc(
            "tftp.start",
            json!({"root": root.path().to_str().unwrap(), "bind": "127.0.0.1", "port": 0}),
        )
        .await
        .unwrap();
    let address: std::net::SocketAddr = started["address"].as_str().unwrap().parse().unwrap();
    assert_ne!(address.port(), 0, "the port it actually got");
    assert_eq!(started["allow_write"], false);

    // It is running, it is a job, and the status says where and what.
    let status = lab.app.rpc("tftp.status", Value::Null).await.unwrap();
    assert_eq!(status["running"], true);
    assert_eq!(status["address"], started["address"]);
    assert_eq!(status["root"], root.path().to_str().unwrap());
    let job = nocturne_app::jobs::get(&lab.app, started["job_id"].as_str().unwrap()).unwrap();
    assert_eq!(job.kind, "tftp.serve");
    assert!(!job.state.is_finished());

    // A real transfer, with a client that is not this server.
    let fetched = support::tftp_get(address, "firmware.bin")
        .await
        .expect("the file comes back");
    assert_eq!(fetched, b"a firmware image");

    // Confinement: outside the root, and above it, are refused.
    let escape = support::tftp_get(address, "../secret.txt").await;
    assert!(escape.is_err(), "{escape:?}");
    let absolute =
        support::tftp_get(address, outside.path().join("secret.txt").to_str().unwrap()).await;
    assert!(absolute.is_err(), "{absolute:?}");
    // Writing is off unless it is asked for.
    let upload = support::tftp_put(address, "new.bin", b"nope").await;
    assert!(upload.is_err(), "{upload:?}");

    // The transfers were reported as they happened.
    let transfers = events.drain("tftp.transfer");
    assert!(
        transfers
            .iter()
            .any(|event| event["started"]["file"] == "firmware.bin"),
        "{transfers:?}"
    );
    assert!(
        transfers
            .iter()
            .any(|event| event["finished"]["bytes"] == 16),
        "{transfers:?}"
    );
    assert!(
        transfers.iter().any(|event| event["denied"].is_object()),
        "the refusals are reported too: {transfers:?}"
    );
    assert_eq!(transfers[0]["job_id"], started["job_id"]);

    // Status carries the same transfers for a screen that was not watching.
    let status = lab.app.rpc("tftp.status", Value::Null).await.unwrap();
    assert!(
        status["transfers"].as_array().unwrap().len() >= 3,
        "{status}"
    );

    // Stopping closes the socket and ends the job.
    assert_eq!(
        lab.app.rpc("tftp.stop", Value::Null).await.unwrap()["stopped"],
        true
    );
    let job = tokio::time::timeout(
        Duration::from_secs(5),
        nocturne_app::jobs::wait(&lab.app, started["job_id"].as_str().unwrap()),
    )
    .await
    .expect("the job ends when the server is stopped")
    .unwrap();
    assert!(job.state.is_finished());
    assert_eq!(
        lab.app.rpc("tftp.status", Value::Null).await.unwrap()["running"],
        false
    );
    // Nothing answers any more.
    assert!(support::tftp_get(address, "firmware.bin").await.is_err());
    // Stopping again is not an error, it just says nothing was running.
    assert_eq!(
        lab.app.rpc("tftp.stop", Value::Null).await.unwrap()["stopped"],
        false
    );

    let runs = lab
        .app
        .rpc("probe.history", json!({"kind": "tftp"}))
        .await
        .unwrap();
    assert_eq!(runs["total"], 1);
    assert_eq!(runs["runs"][0]["ok"], true);
}

#[tokio::test]
async fn a_tftp_server_that_may_write_does_and_still_refuses_to_overwrite() {
    let lab = support::lab(&[]).await;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("already.bin"), b"original").unwrap();
    let started = lab
        .app
        .rpc(
            "tftp.start",
            json!({"root": root.path().to_str().unwrap(), "bind": "127.0.0.1", "port": 0,
                   "allow_write": true}),
        )
        .await
        .unwrap();
    let address: std::net::SocketAddr = started["address"].as_str().unwrap().parse().unwrap();

    support::tftp_put(address, "config.txt", b"hostname SW9\n")
        .await
        .expect("the upload is accepted");
    // The file appears under its name only when it is complete.
    let written = std::fs::read(root.path().join("config.txt")).unwrap();
    assert_eq!(written, b"hostname SW9\n");

    // Overwriting needs its own permission.
    let refused = support::tftp_put(address, "already.bin", b"replaced").await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(
        std::fs::read(root.path().join("already.bin")).unwrap(),
        b"original"
    );

    lab.app.rpc("tftp.stop", Value::Null).await.unwrap();
}

#[tokio::test]
async fn a_port_that_cannot_be_bound_is_said_plainly() {
    let lab = support::lab(&[]).await;
    let root = tempfile::tempdir().unwrap();
    // Hold the port ourselves, then ask for it.
    let held = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = held.local_addr().unwrap().port();

    let refused = lab
        .app
        .rpc(
            "tftp.start",
            json!({"root": root.path().to_str().unwrap(), "bind": "127.0.0.1", "port": port}),
        )
        .await
        .unwrap_err();
    assert!(
        refused.message.contains("already using it") && refused.message.contains(&port.to_string()),
        "{}",
        refused.message
    );
    // Nothing is left behind: no job, no "running" state.
    assert_eq!(
        lab.app.rpc("tftp.status", Value::Null).await.unwrap()["running"],
        false
    );

    // A root that is not there, and a bind address that is not this machine's.
    let missing = lab
        .app
        .rpc(
            "tftp.start",
            json!({"root": "/no/such/directory", "port": 0}),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code, "invalid");
    assert!(
        missing.message.contains("not a directory"),
        "{}",
        missing.message
    );
    assert_eq!(
        lab.app
            .rpc(
                "tftp.start",
                json!({"root": root.path().to_str().unwrap(), "bind": "10.254.254.254", "port": 0})
            )
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    // And a root is required, because the default is empty.
    assert_eq!(
        lab.app
            .rpc("tftp.start", json!({"port": 0}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn one_server_at_a_time_and_only_from_this_machine() {
    let lab = support::lab(&[]).await;
    let root = tempfile::tempdir().unwrap();
    let params = json!({"root": root.path().to_str().unwrap(), "bind": "127.0.0.1", "port": 0});
    lab.app.rpc("tftp.start", params.clone()).await.unwrap();
    let twice = lab.app.rpc("tftp.start", params).await.unwrap_err();
    assert_eq!(twice.code, "conflict");
    assert!(
        twice.message.contains("already running"),
        "{}",
        twice.message
    );
    lab.app.rpc("tftp.stop", Value::Null).await.unwrap();

    // In server mode the machine with the files is not the user's machine.
    let server = support::server_mode();
    let refused = server
        .rpc("tftp.start", json!({"root": "/tmp", "port": 0}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
    let syslog = server
        .rpc("probe.syslog.start", json!({"port": 0}))
        .await
        .unwrap_err();
    assert_eq!(syslog.code, "forbidden");
}

#[tokio::test]
async fn the_staging_syslog_listener_parses_what_a_device_shouts() {
    let lab = support::lab(&[]).await;
    let mut events = support::Events::new(&lab.app);
    support::device_at(&lab.app, "SW9", "127.0.0.1").await;

    let started = lab
        .app
        .rpc(
            "probe.syslog.start",
            json!({"bind": "127.0.0.1", "port": 0}),
        )
        .await
        .unwrap();
    let address = started["udp_address"].as_str().unwrap().to_string();
    assert_eq!(started["tcp_address"], Value::Null, "TCP is off by default");

    // A Cisco line as a switch sends it.
    support::send_syslog(
        &address,
        "<189>45: *Mar  1 00:02:11.003: %LINK-3-UPDOWN: Interface GigabitEthernet0/2, changed state to down",
    )
    .await;
    let event = events
        .next("probe.syslog.message", Duration::from_secs(5))
        .await
        .expect("the message arrives as an event");
    assert_eq!(event["mnemonic"]["facility"], "LINK");
    assert_eq!(event["mnemonic"]["name"], "UPDOWN");
    assert_eq!(event["severity"], 5);
    assert!(
        event["message"]
            .as_str()
            .unwrap()
            .contains("changed state to down"),
        "{event}"
    );
    assert_eq!(event["job_id"], started["job_id"]);

    // And it is readable afterwards by a screen that was not listening.
    let messages = lab
        .app
        .rpc("probe.syslog.messages", json!({}))
        .await
        .unwrap();
    assert_eq!(messages["listening"], true);
    assert_eq!(messages["total"], 1);
    assert!(
        messages["messages"][0]["raw"]
            .as_str()
            .unwrap()
            .contains("%LINK-3-UPDOWN"),
    );

    let status = lab
        .app
        .rpc("probe.syslog.status", Value::Null)
        .await
        .unwrap();
    assert_eq!(status["listening"], true);
    assert_eq!(status["received"], 1);
    assert_eq!(status["senders"][0]["device"], "SW9");
    assert!(
        status["note"].as_str().unwrap().contains("logs feature"),
        "it says what it is not: {}",
        status["note"]
    );

    assert_eq!(
        lab.app.rpc("probe.syslog.stop", Value::Null).await.unwrap()["stopped"],
        true
    );
    let job = tokio::time::timeout(
        Duration::from_secs(5),
        nocturne_app::jobs::wait(&lab.app, started["job_id"].as_str().unwrap()),
    )
    .await
    .expect("the listener's job ends")
    .unwrap();
    assert!(job.state.is_finished());
    let after = lab
        .app
        .rpc("probe.syslog.messages", json!({}))
        .await
        .unwrap();
    assert_eq!(after["listening"], false);
    assert_eq!(after["total"], 0, "nothing is kept once it has stopped");
}

#[tokio::test]
async fn the_servers_stop_when_the_application_does() {
    let lab = support::lab(&[]).await;
    let root = tempfile::tempdir().unwrap();
    let started = lab
        .app
        .rpc(
            "tftp.start",
            json!({"root": root.path().to_str().unwrap(), "bind": "127.0.0.1", "port": 0}),
        )
        .await
        .unwrap();
    let address: std::net::SocketAddr = started["address"].as_str().unwrap().parse().unwrap();
    assert!(support::tftp_get(address, "nothing").await.is_err());

    // Shutting down cancels the jobs, which is what closes the sockets.
    lab.app.shutdown();
    let job = tokio::time::timeout(
        Duration::from_secs(5),
        nocturne_app::jobs::wait(&lab.app, started["job_id"].as_str().unwrap()),
    )
    .await
    .expect("the server's job ends at shutdown")
    .unwrap();
    assert!(job.state.is_finished());
    // The socket is gone, so a client gets nothing at all rather than an error
    // packet from a server that is still half there.
    assert!(
        support::tftp_get(address, "nothing").await.is_err(),
        "the server is still answering after shutdown"
    );
}
