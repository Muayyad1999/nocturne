//! Targets that are not in the inventory: SSH and Telnet to a host and port,
//! a local shell, serial ports — and what a backend whose users are remote
//! refuses to do.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use common::{Events, PATIENCE, SECRET, Script, ScriptedDevice, session_id};
use nocturne_app::{App, AppResult};
use nocturne_transport::Connection;
use nocturne_transport::serve::{
    ServerHandle, SessionHandler, SshServerConfig, serve_ssh, serve_telnet,
};
use serde_json::{Value, json};

/// Serves every session from one scripted console.
struct Serving(Arc<ScriptedDevice>);

#[async_trait::async_trait]
impl SessionHandler for Serving {
    async fn open(
        &self,
        peer: SocketAddr,
        _user: Option<&str>,
        _cols: u16,
        _rows: u16,
    ) -> Option<Connection> {
        Some(self.0.connection(&peer.to_string()))
    }
}

/// A shell-like console that echoes what it is told, with no login.
fn shell_script() -> Script {
    Script {
        banner: "\r\nsrv>".into(),
        listen_only: true,
        ..Script::default()
    }
}

async fn ssh_server(
    script: Script,
    host_key: Option<String>,
) -> (ServerHandle, Arc<ScriptedDevice>) {
    let device = Arc::new(ScriptedDevice::new(script));
    let handle = serve_ssh(
        SshServerConfig {
            users: vec![("admin".into(), SECRET.into())],
            host_key,
            ..SshServerConfig::default()
        },
        Arc::new(Serving(device.clone())),
    )
    .await
    .expect("an SSH server on a free port");
    (handle, device)
}

async fn open_adhoc(app: &App, adhoc: Value) -> AppResult<Value> {
    app.rpc(
        "terminal.open",
        json!({"adhoc": adhoc, "cols": 100, "rows": 30}),
    )
    .await
}

#[tokio::test]
async fn an_adhoc_ssh_session_opens_types_and_keeps_the_password_to_itself() {
    let (server, device) = ssh_server(shell_script(), None).await;
    let port = server.local_addr().port();
    let app = common::builder().build();
    let mut events = Events::new(&app);

    let info = open_adhoc(
        &app,
        json!({
            "transport": "ssh",
            "host": "127.0.0.1",
            "port": port,
            "username": "admin",
            "password": SECRET,
        }),
    )
    .await
    .expect("open");
    let id = session_id(&info);
    assert_eq!(info["kind"], "ssh");
    assert_eq!(info["title"], format!("admin@127.0.0.1:{port}"));
    assert_eq!(info["device_id"], json!(null));
    assert_eq!(info["lab"], false);
    assert_eq!(info["info"]["peer"], format!("127.0.0.1:{port}"));
    assert!(info["info"]["host_key_fingerprint"].as_str().is_some());

    events.wait_for(&id, "srv>").await;
    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "show version\r"}),
    )
    .await
    .expect("input");
    let deadline = std::time::Instant::now() + PATIENCE;
    while device.heard.lines().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the command never arrived"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(device.heard.lines(), ["show version"]);

    // The password was used and then forgotten: it is in no event, no audit
    // entry and nothing the UI can read back.
    events.drain();
    assert!(
        !events.any_payload_contains(SECRET),
        "the password leaked into an event"
    );
    assert!(
        !common::audit_text(&app).contains(SECRET),
        "the password leaked into the audit trail"
    );
    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert!(!listed.to_string().contains(SECRET));

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    server.shutdown().await;
}

#[tokio::test]
async fn a_wrong_password_is_refused_with_the_transports_own_code() {
    let (server, _device) = ssh_server(shell_script(), None).await;
    let port = server.local_addr().port();
    let app = common::builder().build();
    let mut events = Events::new(&app);

    let refused = open_adhoc(
        &app,
        json!({
            "transport": "ssh",
            "host": "127.0.0.1",
            "port": port,
            "username": "admin",
            "password": "not-the-password",
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code, "auth");
    assert!(!refused.message.contains("not-the-password"), "{refused}");

    // A failed attempt leaves nothing behind.
    assert!(
        app.rpc("terminal.list", json!(null))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .is_empty()
    );
    events.drain();
    assert!(events.of("terminal.opened").is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn an_unknown_host_key_is_learned_and_a_changed_one_is_refused() {
    let (server, _device) = ssh_server(shell_script(), None).await;
    let port = server.local_addr().port();
    let app = common::builder().build();
    let target = json!({
        "transport": "ssh",
        "host": "127.0.0.1",
        "port": port,
        "username": "admin",
        "password": SECRET,
    });

    let info = open_adhoc(&app, target.clone()).await.expect("open");
    let fingerprint = info["info"]["host_key_fingerprint"]
        .as_str()
        .expect("a fingerprint")
        .to_string();
    app.rpc("terminal.close", json!({"session_id": session_id(&info)}))
        .await
        .expect("close");

    // Trust on first use remembered it, in the store the whole application
    // shares.
    let id = format!("127.0.0.1:{port}");
    let known: nocturne_app::connect::KnownHost = app
        .store()
        .get("known_hosts", &id)
        .expect("store")
        .expect("the key was learned");
    assert_eq!(known.fingerprint, fingerprint);
    assert_eq!(known.port, port);

    // The device is re-keyed (or someone is in the way).
    app.store()
        .put(
            "known_hosts",
            &id,
            &nocturne_app::connect::KnownHost {
                fingerprint: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
                algorithm: "ssh-ed25519".into(),
                ..known
            },
        )
        .expect("store");

    let refused = open_adhoc(&app, target).await.unwrap_err();
    assert_eq!(refused.code, "host_key");
    let details = refused.details.clone().expect("both fingerprints");
    assert_eq!(details["host"], "127.0.0.1");
    assert_eq!(details["port"], port);
    assert_eq!(
        details["known"]["fingerprint"],
        "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    );
    assert_eq!(details["presented"]["fingerprint"], fingerprint.as_str());
    assert!(
        refused.message.contains("Known hosts"),
        "the refusal should say what to do: {refused}"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn an_adhoc_telnet_session_is_logged_in_from_the_password_typed_for_it() {
    let device = Arc::new(ScriptedDevice::new(Script::default()));
    let server = serve_telnet(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        Arc::new(Serving(device.clone())),
    )
    .await
    .expect("a Telnet server on a free port");
    let port = server.local_addr().port();
    let app = common::builder().build();
    let mut events = Events::new(&app);

    let info = open_adhoc(
        &app,
        json!({
            "transport": "telnet",
            "host": "127.0.0.1",
            "port": port,
            "username": "operator",
            "password": SECRET,
        }),
    )
    .await
    .expect("open");
    let id = session_id(&info);
    assert_eq!(info["kind"], "telnet");
    assert_eq!(info["title"], format!("127.0.0.1:{port}"));

    events.wait_for(&id, "SW1>").await;
    assert_eq!(device.heard.lines(), ["operator", SECRET]);

    // Typed once, kept nowhere.
    events.drain();
    assert!(!events.any_payload_contains(SECRET), "the password leaked");
    assert!(!common::audit_text(&app).contains(SECRET));

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    server.shutdown().await;
}

#[tokio::test]
async fn the_local_shell_runs_and_ends() {
    let app = common::builder().build();
    let mut events = Events::new(&app);
    let info = open_adhoc(
        &app,
        json!({
            "transport": "local",
            "local": common::local_shell(
                "echo nocturne-is-here; exit 7",
                "echo nocturne-is-here & exit 7",
            ),
        }),
    )
    .await
    .expect("open");
    let id = session_id(&info);
    assert_eq!(info["kind"], "local");
    assert_eq!(info["title"], "Local shell");

    common::answer_console_probe(&app, &id, &mut events).await;
    events.wait_for(&id, "nocturne-is-here").await;
    let closed = events.wait_topic("terminal.closed").await;
    assert_eq!(closed.payload["session_id"], id.as_str());
    assert!(
        closed.payload["reason"]
            .as_str()
            .expect("a reason")
            .contains('7'),
        "the exit status should be reported: {}",
        closed.payload
    );
}

#[tokio::test]
async fn a_local_shell_cannot_carry_a_break() {
    let app = common::builder().build();
    let info = open_adhoc(
        &app,
        json!({
            "transport": "local",
            "local": common::local_shell("sleep 30", "ping -n 31 127.0.0.1 > nul"),
            "title": "Local shell",
        }),
    )
    .await
    .expect("open");
    let id = session_id(&info);
    assert_eq!(info["title"], "Local shell");

    let refused = app
        .rpc("terminal.send_break", json!({"session_id": id}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "unsupported");
    assert!(refused.message.contains("serial console"), "{refused}");

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
}

#[tokio::test]
async fn the_local_machine_is_out_of_reach_when_the_caller_is_not_at_it() {
    let app = common::builder_remote().build();
    for adhoc in [
        json!({"transport": "local"}),
        json!({"transport": "serial", "host": "/dev/ttyUSB0"}),
    ] {
        let refused = open_adhoc(&app, adhoc.clone()).await.unwrap_err();
        assert_eq!(refused.code, "forbidden", "{adhoc}");
        assert!(refused.message.contains("desktop app"), "{refused}");
    }
    let refused = app
        .rpc("terminal.serial_ports", json!(null))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");

    // A serial device in the inventory is refused for the same reason.
    let device = app
        .rpc(
            "devices.create",
            json!({"name": "CON1", "transport": "serial", "address": "/dev/ttyUSB0"}),
        )
        .await
        .expect("device");
    let refused = app
        .rpc(
            "terminal.open",
            json!({"device_id": device["id"], "cols": 80, "rows": 24}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
}

#[tokio::test]
async fn serial_ports_are_listed_where_there_are_any() {
    let app = common::builder().build();
    let ports = app
        .rpc("terminal.serial_ports", json!(null))
        .await
        .expect("serial ports");
    // There are rarely any on a build machine; what matters is that the list
    // is a list and not an invention.
    for port in ports.as_array().expect("a list") {
        assert!(port["name"].as_str().is_some_and(|name| !name.is_empty()));
    }
}

#[tokio::test]
async fn an_adhoc_target_has_to_be_complete() {
    let app = common::builder().build();
    for (adhoc, wanted) in [
        (json!({"transport": "ssh", "host": ""}), "host"),
        (json!({"transport": "ssh", "host": "10.0.0.1"}), "user name"),
        (json!({"transport": "serial"}), "serial port"),
        (
            json!({"transport": "telnet", "host": "10.0.0.1", "jump_device_id": "x"}),
            "jump host",
        ),
    ] {
        let refused = open_adhoc(&app, adhoc.clone()).await.unwrap_err();
        assert_eq!(refused.code, "invalid", "{adhoc}");
        assert!(refused.message.contains(wanted), "{adhoc}: {refused}");
    }
}

#[tokio::test]
async fn a_session_closed_while_it_is_connecting_gives_up() {
    // A listener that accepts and then says nothing: the SSH handshake hangs.
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("a free port");
    let port = listener.local_addr().expect("address").port();
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });

    let app = common::builder().build();
    app.rpc(
        "settings.set",
        json!({"section": "connections", "value": {"connect_timeout_s": 120}}),
    )
    .await
    .expect("settings");
    let opening = {
        let app = app.clone();
        tokio::spawn(async move {
            open_adhoc(
                &app,
                json!({
                    "transport": "ssh", "host": "127.0.0.1", "port": port,
                    "username": "admin", "password": SECRET,
                }),
            )
            .await
        })
    };
    let id = {
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
            if let Some(session) = listed.as_array().expect("a list").first() {
                assert_eq!(session["state"], "connecting");
                break session_id(session);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "nothing was registered"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };
    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    let outcome = tokio::time::timeout(PATIENCE, opening)
        .await
        .expect("the attempt should have given up")
        .expect("the task");
    assert_eq!(outcome.unwrap_err().code, "closed");
    silent.abort();
}
