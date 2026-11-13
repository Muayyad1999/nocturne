//! The SSH client against `serve_ssh`.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;
use std::time::Duration;

use russh::keys::{Algorithm, HashAlg};
use russh::{MethodKind, cipher, kex, mac};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use super::support::{Lab, Tap, contains, pattern, soon, tap};
use crate::serve::{ServerHandle, SshServerConfig, SshServerOptions, serve_ssh, serve_ssh_with};
use crate::ssh::{CryptoPolicy, Role, preferred_algorithms};
use crate::{
    AcceptAnyHostKey, Connection, HostKeyVerifier, KeyKind, LegacyCrypto, MemoryKnownHosts,
    PresentedKey, SshAuth, SshHop, SshParams, SshSession, TransportError, TransportKind,
    connect_ssh, generate_keypair,
};

// ----------------------------------------------------------------- helpers

fn config(users: &[(&str, &str)]) -> SshServerConfig {
    SshServerConfig {
        users: users
            .iter()
            .map(|(user, password)| ((*user).to_owned(), (*password).to_owned()))
            .collect(),
        ..SshServerConfig::default()
    }
}

/// A server with default options; no users means anyone gets in.
async fn server(users: &[(&str, &str)]) -> (ServerHandle, Arc<Lab>) {
    server_with(config(users), SshServerOptions::default()).await
}

async fn server_with(
    config: SshServerConfig,
    options: SshServerOptions,
) -> (ServerHandle, Arc<Lab>) {
    let lab = Lab::new();
    let handle = serve_ssh_with(config, lab.clone(), options).await.unwrap();
    (handle, lab)
}

fn params_for(address: SocketAddr, username: &str, auth: Vec<SshAuth>) -> SshParams {
    SshParams {
        host: address.ip().to_string(),
        port: address.port(),
        username: username.to_owned(),
        auth,
        ..SshParams::default()
    }
}

fn params(server: &ServerHandle, username: &str, auth: Vec<SshAuth>) -> SshParams {
    params_for(server.local_addr(), username, auth)
}

fn password(password: &str) -> Vec<SshAuth> {
    vec![SshAuth::Password {
        password: password.to_owned(),
    }]
}

fn any_host_key() -> Arc<dyn HostKeyVerifier> {
    Arc::new(AcceptAnyHostKey)
}

/// Connects and opens a shell, trusting any host key.
async fn try_login(params: &SshParams) -> Result<Tap<Connection>, TransportError> {
    soon(connect_ssh(params, any_host_key())).await.map(tap)
}

async fn login(params: &SshParams) -> Tap<Connection> {
    try_login(params)
        .await
        .unwrap_or_else(|error| panic!("login failed: {error}"))
}

async fn login_error(params: &SshParams) -> TransportError {
    match try_login(params).await {
        Ok(_) => panic!("login unexpectedly succeeded"),
        Err(error) => error,
    }
}

/// Logs in to a server without users.
async fn guest(server: &ServerHandle) -> Tap<Connection> {
    login(&params(server, "op", vec![])).await
}

fn auth_failure(error: &TransportError) -> (&[String], &[String]) {
    match error {
        TransportError::AuthFailed {
            tried,
            server_methods,
        } => (tried, server_methods),
        other => panic!("expected an authentication failure, got: {other}"),
    }
}

/// RSA keys take a while to generate: one for the whole test run.
static RSA_KEY: LazyLock<(String, String)> =
    LazyLock::new(|| generate_keypair(KeyKind::Rsa3072, "rsa@test").unwrap());

// ------------------------------------------------------------------- login

#[tokio::test]
async fn password_login_opens_a_shell() {
    let (server, lab) = server(&[("admin", "secret")]).await;
    let mut conn = login(&params(&server, "admin", password("secret"))).await;

    let greeting = conn.until(b"lab> ").await;
    assert!(contains(&greeting, b"Welcome admin 120x40"));
    conn.send("hello\r").await.unwrap();
    let reply = conn.until(b"lab> ").await;
    assert!(contains(&reply, b"hello"));

    let info = conn.info();
    assert_eq!(info.kind, Some(TransportKind::Ssh));
    assert_eq!(info.peer, server.local_addr().to_string());
    assert!(
        info.server_banner
            .as_deref()
            .is_some_and(|banner| banner.starts_with("SSH-2.0-Nocturne_")),
        "{:?}",
        info.server_banner
    );
    assert_eq!(info.kex.as_deref(), Some("mlkem768x25519-sha256"));
    assert_eq!(
        info.cipher.as_deref(),
        Some("chacha20-poly1305@openssh.com")
    );
    assert_eq!(info.mac, None, "AEAD ciphers have no separate MAC");
    assert_eq!(info.host_key_algorithm.as_deref(), Some("ssh-ed25519"));
    assert!(
        info.host_key_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| fingerprint.starts_with("SHA256:"))
    );
    assert!(!info.legacy_crypto);
    assert!(info.via.is_empty());

    conn.close().await;
    lab.wait_idle().await;
    assert_eq!(lab.opened.load(Ordering::Acquire), 1);
    soon(server.shutdown()).await;
}

#[tokio::test]
async fn wrong_password_reports_what_was_tried_and_offered() {
    let (server, lab) = server(&[("admin", "secret")]).await;
    let error = login_error(&params(&server, "admin", password("guess"))).await;
    assert_eq!(error.code(), "auth");
    let (tried, offered) = auth_failure(&error);
    assert_eq!(tried, ["none", "password"]);
    assert_eq!(offered, ["password", "keyboard-interactive"]);
    assert_eq!(lab.opened.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn unknown_user_and_missing_credentials_fail_cleanly() {
    let (server, _lab) = server(&[("admin", "secret")]).await;
    let error = login_error(&params(&server, "root", password("secret"))).await;
    assert_eq!(error.code(), "auth");

    let error = login_error(&params(&server, "admin", vec![])).await;
    let (tried, offered) = auth_failure(&error);
    assert_eq!(tried, ["none"]);
    assert!(!offered.is_empty());
}

#[tokio::test]
async fn methods_are_tried_in_order_until_one_works() {
    let (server, _lab) = server(&[("admin", "secret")]).await;
    let auth = vec![
        SshAuth::KeyFile {
            path: "/nonexistent/id_ed25519".to_owned(),
            passphrase: None,
        },
        SshAuth::Agent,
        SshAuth::None,
        SshAuth::Password {
            password: "secret".to_owned(),
        },
    ];
    let mut conn = login(&params(&server, "admin", auth)).await;
    conn.until(b"Welcome admin").await;
    conn.close().await;
}

#[tokio::test]
async fn a_server_without_users_lets_anyone_in() {
    let (server, _lab) = server(&[]).await;
    let mut conn = login(&params(&server, "guest", vec![])).await;
    conn.until(b"Welcome guest").await;
    conn.close().await;
}

fn keys_only() -> SshServerOptions {
    SshServerOptions {
        methods: Some(vec![MethodKind::PublicKey]),
        ..SshServerOptions::default()
    }
}

#[tokio::test]
async fn public_key_login_with_ed25519_and_rsa() {
    let (server, _lab) = server_with(config(&[]), keys_only()).await;

    let (ed25519, _) = generate_keypair(KeyKind::Ed25519, "me@test").unwrap();
    let auth = vec![SshAuth::PrivateKey {
        key: ed25519,
        passphrase: None,
    }];
    let mut conn = login(&params(&server, "keyuser", auth)).await;
    conn.until(b"Welcome keyuser").await;
    assert!(!conn.info().legacy_crypto);
    conn.close().await;

    // An RSA key from a file: signed with SHA-2, so nothing legacy about it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("id_rsa");
    std::fs::write(&path, &RSA_KEY.0).unwrap();
    let auth = vec![SshAuth::KeyFile {
        path: path.to_string_lossy().into_owned(),
        passphrase: None,
    }];
    let mut conn = login(&params(&server, "rsauser", auth)).await;
    conn.until(b"Welcome rsauser").await;
    assert!(!conn.info().legacy_crypto);
    conn.close().await;

    // No key, no entry.
    let error = login_error(&params(&server, "nokey", password("anything"))).await;
    let (tried, offered) = auth_failure(&error);
    assert_eq!(tried.len(), 2, "{tried:?}");
    assert!(tried[1].contains("skipped"), "{tried:?}");
    assert_eq!(offered, ["publickey"]);
}

#[tokio::test]
async fn encrypted_keys_need_their_passphrase() {
    use russh::keys::ssh_key::LineEnding;

    let (server, _lab) = server_with(config(&[]), keys_only()).await;
    let (plain, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
    let encrypted = crate::ssh::decode_private_key(&plain, None)
        .unwrap()
        .encrypt(&mut russh::keys::key::safe_rng(), "open sesame")
        .unwrap()
        .to_openssh(LineEnding::LF)
        .unwrap()
        .to_string();

    let key = |passphrase: Option<&str>| SshAuth::PrivateKey {
        key: encrypted.clone(),
        passphrase: passphrase.map(str::to_owned),
    };
    let error = login_error(&params(&server, "u", vec![key(None), key(Some("wrong"))])).await;
    let (tried, _) = auth_failure(&error);
    assert_eq!(tried.len(), 3, "{tried:?}");
    assert!(tried[1].contains("encrypted"), "{tried:?}");
    assert!(tried[2].contains("wrong passphrase"), "{tried:?}");

    let auth = vec![key(Some("wrong")), key(Some("open sesame"))];
    let mut conn = login(&params(&server, "u", auth)).await;
    conn.until(b"Welcome u").await;
    conn.close().await;
}

fn keyboard_interactive_only() -> SshServerOptions {
    SshServerOptions {
        methods: Some(vec![MethodKind::KeyboardInteractive]),
        ..SshServerOptions::default()
    }
}

#[tokio::test]
async fn password_answers_keyboard_interactive_when_that_is_all_there_is() {
    let (server, _lab) =
        server_with(config(&[("admin", "secret")]), keyboard_interactive_only()).await;

    let mut conn = login(&params(&server, "admin", password("secret"))).await;
    conn.until(b"Welcome admin").await;
    conn.close().await;

    let auth = vec![SshAuth::KeyboardInteractive {
        response: "secret".to_owned(),
    }];
    let mut conn = login(&params(&server, "admin", auth)).await;
    conn.until(b"Welcome admin").await;
    conn.close().await;

    let error = login_error(&params(&server, "admin", password("guess"))).await;
    let (tried, offered) = auth_failure(&error);
    assert_eq!(tried, ["none", "password (via keyboard-interactive)"]);
    assert_eq!(offered, ["keyboard-interactive"]);
}

#[tokio::test]
async fn methods_the_server_does_not_offer_are_skipped() {
    let (server, _lab) =
        server_with(config(&[("admin", "secret")]), keyboard_interactive_only()).await;
    let (key, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
    let auth = vec![
        SshAuth::PrivateKey {
            key,
            passphrase: None,
        },
        SshAuth::KeyboardInteractive {
            response: "guess".to_owned(),
        },
    ];
    let error = login_error(&params(&server, "admin", auth)).await;
    let (tried, _) = auth_failure(&error);
    assert_eq!(tried.len(), 3, "{tried:?}");
    assert!(tried[1].contains("skipped"), "{tried:?}");
    assert_eq!(tried[2], "keyboard-interactive");
}

// ----------------------------------------------------------------- session

#[tokio::test]
async fn pre_auth_banner_is_kept_with_the_identification() {
    let config = SshServerConfig {
        banner: Some("Authorized users only.\r\n".to_owned()),
        ..config(&[("admin", "secret")])
    };
    let (server, _lab) = server_with(config, SshServerOptions::default()).await;
    let params = params(&server, "admin", password("secret"));
    let session = soon(SshSession::connect(&params, any_host_key()))
        .await
        .unwrap();
    let banner = session.info().server_banner.clone().unwrap();
    let (identification, text) = banner.split_once('\n').unwrap();
    assert!(identification.starts_with("SSH-2.0-Nocturne_"), "{banner}");
    assert_eq!(text, "Authorized users only.");
    soon(session.close()).await;
}

#[tokio::test]
async fn window_size_is_requested_and_resizes_propagate() {
    let (server, _lab) = server(&[]).await;
    let params = SshParams {
        cols: 132,
        rows: 50,
        ..params(&server, "op", vec![])
    };
    let mut conn = login(&params).await;
    conn.until(b"Welcome op 132x50").await;

    conn.resize(200, 60).await.unwrap();
    conn.until(b"[resize 200x60]").await;
    conn.send("size\r").await.unwrap();
    conn.until(b"200x60\r\nlab> ").await;
    conn.close().await;
}

/// Asks the device for `total` bytes in chunks of `chunk` and checks every
/// one of them.
async fn flood_and_verify(conn: &mut Tap<Connection>, total: usize, chunk: usize) {
    let command = format!("flood {total} {chunk}\r");
    conn.send(command.clone()).await.unwrap();
    conn.until(command.as_bytes()).await; // the echo
    let payload = conn.exactly(total).await;
    for (index, byte) in payload.iter().enumerate() {
        assert_eq!(*byte, pattern(index), "byte {index} of {total} is wrong");
    }
    conn.until(b"\r\nlab> ").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_output_arrives_complete_and_in_order() {
    let (server, _lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    flood_and_verify(&mut conn, 6 * 1024 * 1024, 7000).await;
    flood_and_verify(&mut conn, 300_000, 1).await;
    conn.close().await;
}

#[tokio::test]
async fn remote_exit_is_reported_with_its_status() {
    let (server, lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    conn.send("exit\r").await.unwrap();
    let (rest, reason) = conn.until_closed().await;
    assert!(contains(&rest, b"bye"));
    assert_eq!(reason.as_deref(), Some("exit status 0"));
    assert!(conn.is_closed());
    assert_eq!(conn.send("x").await.unwrap_err().code(), "closed");
    lab.wait_idle().await;
}

#[tokio::test]
async fn output_right_before_exit_is_not_lost() {
    let (server, _lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    let commands = "flood 200000 500\rexit\r";
    conn.send(commands).await.unwrap();
    conn.until(commands.as_bytes()).await;
    let (seen, reason) = conn.until_closed().await;
    assert_eq!(reason.as_deref(), Some("exit status 0"));
    assert_eq!(seen.len(), 200_000 + b"\r\nlab> \r\nbye\r\n".len());
    assert!(seen.ends_with(b"bye\r\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_words_of_a_device_survive_input_it_never_reads() {
    let (server, lab) = server(&[]).await;
    for _ in 0..20 {
        let mut conn = guest(&server).await;
        conn.until(b"lab> ").await;
        // The device leaves on the first line; the rest has nowhere to go,
        // which must not cut the goodbye short.
        conn.send("exit\r").await.unwrap();
        for _ in 0..50 {
            if conn.send("typed ahead\r").await.is_err() {
                break;
            }
        }
        let (rest, reason) = conn.until_closed().await;
        assert!(
            rest.ends_with(b"\r\nbye\r\n"),
            "{:?}",
            String::from_utf8_lossy(&rest)
        );
        assert_eq!(reason.as_deref(), Some("exit status 0"));
    }
    lab.wait_idle().await;
}

#[tokio::test]
async fn a_device_that_stopped_listening_is_still_heard() {
    let (server, lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    conn.send("monologue\r").await.unwrap();
    for _ in 0..50 {
        conn.send("anyone there?\r").await.unwrap();
    }
    let (rest, reason) = conn.until_closed().await;
    assert!(
        rest.ends_with(b"\r\none\r\ntwo\r\nthree"),
        "{:?}",
        String::from_utf8_lossy(&rest)
    );
    assert_eq!(reason.as_deref(), Some("exit status 0"));
    lab.wait_idle().await;
}

#[tokio::test]
async fn local_close_has_no_reason_and_ends_the_remote_session() {
    let (server, lab) = server(&[]).await;
    let (writer, reader) = guest(&server).await.inner.split();
    let mut reader = tap(reader);
    reader.until(b"lab> ").await;
    writer.close().await;
    let (_, reason) = reader.until_closed().await;
    assert_eq!(reason, None);
    lab.wait_idle().await;
}

#[tokio::test]
async fn dropping_the_connection_ends_the_remote_session() {
    let (server, lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    drop(conn);
    lab.wait_idle().await;
}

#[tokio::test]
async fn close_returns_promptly_while_output_is_pouring_in() {
    let (server, lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    conn.send("flood 50000000 1000\r").await.unwrap();
    let started = std::time::Instant::now();
    conn.close().await;
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    lab.wait_idle().await;
}

#[tokio::test]
async fn server_shutdown_is_reported_as_a_disconnect() {
    let (server, _lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    soon(server.shutdown()).await;
    let (_, reason) = conn.until_closed().await;
    let reason = reason.unwrap();
    assert!(reason.contains("disconnected by the server"), "{reason}");
    assert!(reason.contains("shutting down"), "{reason}");
}

/// A TCP relay in front of a server that can be cut or frozen.
struct Wire {
    address: SocketAddr,
    fault: tokio::sync::watch::Sender<Fault>,
}

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    None,
    /// Close both sockets.
    Cut,
    /// Keep the sockets open but stop relaying.
    Freeze,
}

impl Wire {
    async fn to(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (fault, faults) = tokio::sync::watch::channel(Fault::None);
        tokio::spawn(async move {
            while let Ok((mut near, _)) = listener.accept().await {
                let mut faults = faults.clone();
                tokio::spawn(async move {
                    let mut far = TcpStream::connect(target).await.unwrap();
                    let fault = tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut near, &mut far) => Fault::Cut,
                        fault = faults.wait_for(|fault| *fault != Fault::None) => {
                            fault.map_or(Fault::Cut, |fault| *fault)
                        }
                    };
                    if fault == Fault::Freeze {
                        // Hold both sockets until told otherwise.
                        let _ = faults.wait_for(|fault| *fault == Fault::Cut).await;
                    }
                });
            }
        });
        Self { address, fault }
    }
}

#[tokio::test]
async fn a_lost_connection_is_reported() {
    let (server, lab) = server(&[]).await;
    let wire = Wire::to(server.local_addr()).await;
    let mut conn = login(&params_for(wire.address, "op", vec![])).await;
    conn.until(b"lab> ").await;
    wire.fault.send_replace(Fault::Cut);
    let (_, reason) = conn.until_closed().await;
    assert_eq!(reason.as_deref(), Some("connection closed by remote host"));
    lab.wait_idle().await;
}

#[tokio::test]
async fn a_silent_peer_is_detected_by_keepalives() {
    let (server, _lab) = server(&[]).await;
    let wire = Wire::to(server.local_addr()).await;
    let params = SshParams {
        keepalive_s: 1,
        ..params_for(wire.address, "op", vec![])
    };
    let mut conn = login(&params).await;
    conn.until(b"lab> ").await;
    wire.fault.send_replace(Fault::Freeze);
    let (_, reason) = conn.until_closed().await;
    let reason = reason.unwrap();
    assert!(reason.contains("keep-alives"), "{reason}");
    wire.fault.send_replace(Fault::Cut);
}

#[tokio::test]
async fn one_session_carries_several_shells() {
    let (server, lab) = server(&[]).await;
    let session = soon(SshSession::connect(
        &params(&server, "op", vec![]),
        any_host_key(),
    ))
    .await
    .unwrap();
    let mut first = tap(soon(session.shell()).await.unwrap());
    let mut second = tap(soon(session.shell()).await.unwrap());
    first.until(b"lab> ").await;
    second.until(b"lab> ").await;
    assert_eq!(lab.active.load(Ordering::Acquire), 2);

    first.send("exit\r").await.unwrap();
    let (_, reason) = first.until_closed().await;
    assert_eq!(reason.as_deref(), Some("exit status 0"));

    // The other shell is unaffected…
    second.send("size\r").await.unwrap();
    second.until(b"120x40").await;
    // …until the session is closed under it.
    soon(session.close()).await;
    let (_, reason) = second.until_closed().await;
    assert_eq!(reason.as_deref(), Some("the SSH session was closed"));
    lab.wait_idle().await;
}

#[tokio::test]
async fn a_shell_keeps_its_session_alive() {
    let (server, lab) = server(&[]).await;
    let session = soon(SshSession::connect(
        &params(&server, "op", vec![]),
        any_host_key(),
    ))
    .await
    .unwrap();
    let mut shell = tap(soon(session.shell()).await.unwrap());
    drop(session);
    shell.until(b"lab> ").await;
    shell.send("size\r").await.unwrap();
    shell.until(b"120x40").await;
    shell.close().await;
    lab.wait_idle().await;
}

#[tokio::test]
async fn refused_requests_are_errors_not_hangs() {
    // This server has no exec or SFTP, and this lab refuses shells.
    let lab = Arc::new(Lab {
        closed_for_business: true,
        ..Lab::default()
    });
    let server = serve_ssh(config(&[]), lab).await.unwrap();
    let params = params(&server, "op", vec![]);
    let session = soon(SshSession::connect(&params, any_host_key()))
        .await
        .unwrap();

    let error = soon(session.exec("show version", Duration::from_secs(5)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "protocol");
    assert!(error.to_string().contains("refused to run"), "{error}");

    let error = soon(session.sftp()).await.unwrap_err();
    assert_eq!(error.code(), "unsupported");

    let error = soon(session.shell()).await.unwrap_err();
    assert_eq!(error.code(), "protocol");
    assert!(
        error.to_string().contains("refused to start a shell"),
        "{error}"
    );
    soon(session.close()).await;

    assert_eq!(login_error(&params).await.code(), "protocol");
}

#[tokio::test]
async fn break_is_reported_as_unsupported() {
    let (server, _lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    assert!(!conn.supports_break());
    let error = conn.send_break().await.unwrap_err();
    assert_eq!(error.code(), "unsupported");
    assert!(error.to_string().contains("RFC 4335"), "{error}");
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_sessions_run_concurrently() {
    const SESSIONS: usize = 24;
    let (server, lab) = server(&[("admin", "secret")]).await;
    let mut clients = tokio::task::JoinSet::new();
    for index in 0..SESSIONS {
        let params = params(&server, "admin", password("secret"));
        clients.spawn(async move {
            let mut conn = login(&params).await;
            conn.until(b"lab> ").await;
            flood_and_verify(&mut conn, 20_000 + index, 100 + index).await;
            conn.close().await;
        });
    }
    while let Some(joined) = soon(clients.join_next()).await {
        joined.unwrap();
    }
    assert_eq!(lab.opened.load(Ordering::Acquire), SESSIONS);
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

// ---------------------------------------------------------------- host key

struct Reject;

#[async_trait::async_trait]
impl HostKeyVerifier for Reject {
    async fn verify(&self, host: &str, port: u16, key: &PresentedKey) -> Result<(), String> {
        Err(format!(
            "{host}:{port} presented {} which is not on file",
            key.algorithm
        ))
    }
}

#[tokio::test]
async fn a_rejected_host_key_aborts_the_handshake() {
    let (server, lab) = server(&[]).await;
    let params = params(&server, "op", vec![]);
    let error = soon(connect_ssh(&params, Arc::new(Reject)))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "host_key");
    let TransportError::HostKeyRejected(reason) = &error else {
        panic!("{error}");
    };
    let expected = format!(
        "127.0.0.1:{} presented ssh-ed25519 which is not on file",
        params.port
    );
    assert_eq!(reason, &expected);
    assert_eq!(lab.opened.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn trust_on_first_use_rejects_a_changed_key() {
    let known: Arc<MemoryKnownHosts> = Arc::new(MemoryKnownHosts::new());
    let (first_key, first_public) = generate_keypair(KeyKind::Ed25519, "").unwrap();
    let keyed = |key: &str| SshServerConfig {
        host_key: Some(key.to_owned()),
        ..config(&[])
    };

    let (server, _lab) = server_with(keyed(&first_key), SshServerOptions::default()).await;
    let address = server.local_addr();
    let params = params_for(address, "op", vec![]);
    for _ in 0..2 {
        let conn = soon(connect_ssh(&params, known.clone())).await.unwrap();
        soon(conn.close()).await;
    }
    let stored = known.get("127.0.0.1", address.port());
    assert_eq!(stored.len(), 1);
    assert!(
        first_public.starts_with(&stored[0]),
        "{stored:?} vs {first_public}"
    );
    soon(server.shutdown()).await;

    // Same address, different key: the classic man-in-the-middle picture.
    let (second_key, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
    let config = SshServerConfig {
        bind: address,
        ..keyed(&second_key)
    };
    let (server, lab) = server_with(config, SshServerOptions::default()).await;
    let error = soon(connect_ssh(&params, known.clone())).await.unwrap_err();
    assert_eq!(error.code(), "host_key");
    assert!(error.to_string().contains("has changed"), "{error}");
    assert_eq!(lab.opened.load(Ordering::Acquire), 0);

    // Once the user decides to trust the new key, it works again.
    assert!(known.remove("127.0.0.1", address.port()));
    let conn = soon(connect_ssh(&params, known.clone())).await.unwrap();
    soon(conn.close()).await;
    soon(server.shutdown()).await;
}

/// A verifier that takes its time, like a dialog waiting for the user.
struct Slow(Duration);

#[async_trait::async_trait]
impl HostKeyVerifier for Slow {
    async fn verify(&self, _host: &str, _port: u16, _key: &PresentedKey) -> Result<(), String> {
        tokio::time::sleep(self.0).await;
        Ok(())
    }
}

#[tokio::test]
async fn the_handshake_timeout_waits_for_the_host_key_decision() {
    let (server, _lab) = server(&[]).await;
    let params = SshParams {
        connect_timeout_ms: 300,
        ..params(&server, "op", vec![])
    };
    let verifier = Arc::new(Slow(Duration::from_millis(1200)));
    let conn = soon(connect_ssh(&params, verifier)).await.unwrap();
    soon(conn.close()).await;
}

// ---------------------------------------------------------------- failures

#[tokio::test]
async fn connection_errors_are_classified() {
    // Nothing listens there.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = login_error(&params_for(address, "op", vec![])).await;
    assert_eq!(error.code(), "refused");

    let params = SshParams {
        host: "no-such-host.invalid".to_owned(),
        ..SshParams::default()
    };
    assert_eq!(login_error(&params).await.code(), "resolve");
}

#[tokio::test]
async fn a_peer_that_never_speaks_ssh_times_out() {
    // Accepts and stays silent.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let params = SshParams {
        connect_timeout_ms: 400,
        ..params_for(address, "op", vec![])
    };
    let started = std::time::Instant::now();
    let error = login_error(&params).await;
    assert_eq!(error.code(), "connect_timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
    silent.abort();
}

#[tokio::test]
async fn a_peer_that_is_not_ssh_is_a_protocol_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"220 mail.example.com ESMTP\r\n").await;
            let _ = stream.shutdown().await;
        }
    });
    let error = login_error(&params_for(address, "op", vec![])).await;
    assert_eq!(error.code(), "protocol", "{error}");
}

// -------------------------------------------------------------- jump hosts

fn jump_host() -> SshServerOptions {
    SshServerOptions {
        allow_tcp_forwarding: true,
        ..SshServerOptions::default()
    }
}

fn hop(server: &ServerHandle, username: &str, auth: Vec<SshAuth>) -> SshHop {
    let address = server.local_addr();
    SshHop {
        host: address.ip().to_string(),
        port: address.port(),
        username: username.to_owned(),
        auth,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn target_is_reached_through_a_jump_host() {
    let (bastion, bastion_lab) = server_with(config(&[("jumper", "hop")]), jump_host()).await;
    let (target, target_lab) = server(&[("admin", "secret")]).await;

    let params = SshParams {
        jump: vec![hop(&bastion, "jumper", password("hop"))],
        ..params(&target, "admin", password("secret"))
    };
    let mut conn = login(&params).await;
    conn.until(b"Welcome admin").await;
    assert_eq!(conn.info().via, [bastion.local_addr().to_string()]);
    assert_eq!(conn.info().peer, target.local_addr().to_string());
    conn.until(b"lab> ").await;
    flood_and_verify(&mut conn, 3_000_000, 1000).await;

    // The shell ran on the target, not on the bastion.
    assert_eq!(target_lab.opened.load(Ordering::Acquire), 1);
    assert_eq!(bastion_lab.opened.load(Ordering::Acquire), 0);
    conn.close().await;
    target_lab.wait_idle().await;
}

#[tokio::test]
async fn two_jump_hosts_can_be_chained() {
    let (first, _) = server_with(config(&[("a", "one")]), jump_host()).await;
    let (second, _) = server_with(config(&[("b", "two")]), jump_host()).await;
    let (target, lab) = server(&[("admin", "secret")]).await;

    // Each hop verifies the key of the host it is reaching.
    let known = Arc::new(MemoryKnownHosts::new());
    let params = SshParams {
        jump: vec![
            hop(&first, "a", password("one")),
            hop(&second, "b", password("two")),
        ],
        ..params(&target, "admin", password("secret"))
    };
    let mut conn = tap(soon(connect_ssh(&params, known.clone())).await.unwrap());
    conn.until(b"Welcome admin").await;
    assert_eq!(
        conn.info().via,
        [
            first.local_addr().to_string(),
            second.local_addr().to_string()
        ]
    );
    assert_eq!(known.entries().len(), 3);
    conn.close().await;
    lab.wait_idle().await;
}

#[tokio::test]
async fn jump_host_failures_say_where_they_happened() {
    let (bastion, _) = server_with(config(&[("jumper", "hop")]), jump_host()).await;
    let (target, _) = server(&[("admin", "secret")]).await;
    let through = |jump: Vec<SshHop>, target: SocketAddr| SshParams {
        jump,
        ..params_for(target, "admin", password("secret"))
    };

    // Wrong credentials on the hop.
    let wrong_hop = hop(&bastion, "jumper", password("wrong"));
    let error = login_error(&through(vec![wrong_hop], target.local_addr())).await;
    let (tried, _) = auth_failure(&error);
    assert_eq!(
        tried[0],
        format!("[jump host {}]", bastion.local_addr()),
        "{tried:?}"
    );

    // The hop cannot reach the target.
    let good_hop = hop(&bastion, "jumper", password("hop"));
    let unreachable = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let nowhere = unreachable.local_addr().unwrap();
    drop(unreachable);
    let error = login_error(&through(vec![good_hop.clone()], nowhere)).await;
    assert_eq!(error.code(), "unreachable", "{error}");
    assert!(error.to_string().contains(&nowhere.to_string()), "{error}");

    // A hop that does not forward (the public `serve_ssh` never does).
    let (strict, _) = server(&[("jumper", "hop")]).await;
    let strict_hop = hop(&strict, "jumper", password("hop"));
    let error = login_error(&through(vec![strict_hop], target.local_addr())).await;
    assert_eq!(error.code(), "protocol", "{error}");
    assert!(
        error.to_string().contains("does not allow forwarding"),
        "{error}"
    );

    // Too long a chain.
    let error = login_error(&through(vec![good_hop; 6], target.local_addr())).await;
    assert_eq!(error.code(), "unsupported", "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_traffic_both_ways_on_several_shells_through_a_jump_host() {
    let (bastion, _) = server_with(config(&[]), jump_host()).await;
    let (target, lab) = server(&[]).await;
    let params = SshParams {
        jump: vec![hop(&bastion, "jumper", vec![])],
        ..params(&target, "op", vec![])
    };
    echo_in_bulk_on_several_shells(&params).await;
    lab.wait_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_traffic_both_ways_on_several_shells() {
    let (target, lab) = server(&[]).await;
    echo_in_bulk_on_several_shells(&params(&target, "op", vec![])).await;
    lab.wait_idle().await;
}

/// Megabytes of input, echoed, on four shells of one session at once.
async fn echo_in_bulk_on_several_shells(params: &SshParams) {
    const SHELLS: usize = 4;
    const CHUNK: usize = 32_000;
    const CHUNKS: usize = 150;
    let session = soon(SshSession::connect(params, any_host_key()))
        .await
        .unwrap();

    let mut shells = Vec::new();
    for _ in 0..SHELLS {
        let (writer, reader) = soon(session.shell()).await.unwrap().split();
        shells.push(tokio::spawn(async move {
            let mut reader = tap(reader);
            reader.until(b"lab> ").await;
            let sender = tokio::spawn(async move {
                let chunk = bytes::Bytes::from((0..CHUNK).map(pattern).collect::<Vec<u8>>());
                for _ in 0..CHUNKS {
                    writer.send(chunk.clone()).await.unwrap();
                }
                writer
            });
            let echoed = reader.exactly(CHUNK * CHUNKS).await;
            for (index, byte) in echoed.iter().enumerate() {
                assert_eq!(*byte, pattern(index % CHUNK), "byte {index} is wrong");
            }
            let writer = soon(sender).await.unwrap();
            writer.close().await;
            let (_, reason) = reader.until_closed().await;
            assert_eq!(reason, None);
        }));
    }
    for shell in shells {
        soon(shell).await.unwrap();
    }
    soon(session.close()).await;
}

// ------------------------------------------------------------ legacy crypto

/// The one algorithm a restricted server offers in some family.
enum Only {
    Kex(kex::Name),
    Cipher(cipher::Name),
    Mac(mac::Name),
    HostKey(Algorithm),
}

fn restricted(only: &Only) -> SshServerOptions {
    let mut preferred = preferred_algorithms(CryptoPolicy::Legacy, Role::Server);
    match only {
        Only::Kex(name) => {
            preferred.kex = Cow::Owned(vec![
                *name,
                kex::EXTENSION_SUPPORT_AS_SERVER,
                kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
            ]);
        }
        Only::Cipher(name) => preferred.cipher = Cow::Owned(vec![*name]),
        Only::Mac(name) => {
            // With an AEAD cipher the MAC would not matter.
            preferred.cipher = Cow::Owned(vec![cipher::AES_256_CTR]);
            preferred.mac = Cow::Owned(vec![*name]);
        }
        Only::HostKey(algorithm) => preferred.key = Cow::Owned(vec![algorithm.clone()]),
    }
    SshServerOptions {
        preferred: Some(preferred),
        ..SshServerOptions::default()
    }
}

/// `Auto` and `Always` connect and flag the session; `Never` explains what
/// the server offers.
async fn assert_legacy_only(only: Only, host_key: Option<String>, offered: &str) {
    let config = SshServerConfig {
        host_key,
        ..config(&[("admin", "secret")])
    };
    let (server, _lab) = server_with(config, restricted(&only)).await;
    let params = |legacy| SshParams {
        legacy,
        ..params(&server, "admin", password("secret"))
    };

    for legacy in [LegacyCrypto::Auto, LegacyCrypto::Always] {
        let mut conn = try_login(&params(legacy))
            .await
            .unwrap_or_else(|error| panic!("{offered} with {legacy:?}: {error}"));
        conn.until(b"Welcome admin").await;
        let info = conn.info().clone();
        assert!(
            info.legacy_crypto,
            "{offered} with {legacy:?} must be flagged"
        );
        let negotiated = match &only {
            Only::Kex(_) => info.kex,
            Only::Cipher(_) => info.cipher,
            Only::Mac(_) => info.mac,
            Only::HostKey(_) => info.host_key_algorithm,
        };
        assert_eq!(negotiated.as_deref(), Some(offered));
        conn.close().await;
    }

    let error = login_error(&params(LegacyCrypto::Never)).await;
    assert_eq!(error.code(), "algorithms", "{offered}: {error}");
    let message = error.to_string();
    assert!(message.contains(offered), "{message}");
    assert!(message.contains("legacy"), "{message}");
    soon(server.shutdown()).await;
}

#[tokio::test]
async fn legacy_key_exchanges() {
    assert_legacy_only(
        Only::Kex(kex::DH_G14_SHA1),
        None,
        "diffie-hellman-group14-sha1",
    )
    .await;
    assert_legacy_only(
        Only::Kex(kex::DH_G1_SHA1),
        None,
        "diffie-hellman-group1-sha1",
    )
    .await;
    assert_legacy_only(
        Only::Kex(kex::DH_GEX_SHA1),
        None,
        "diffie-hellman-group-exchange-sha1",
    )
    .await;
}

#[tokio::test]
async fn legacy_ciphers() {
    assert_legacy_only(Only::Cipher(cipher::AES_128_CBC), None, "aes128-cbc").await;
    assert_legacy_only(Only::Cipher(cipher::AES_192_CBC), None, "aes192-cbc").await;
    assert_legacy_only(Only::Cipher(cipher::AES_256_CBC), None, "aes256-cbc").await;
    assert_legacy_only(Only::Cipher(cipher::TRIPLE_DES_CBC), None, "3des-cbc").await;
}

#[tokio::test]
async fn legacy_macs() {
    assert_legacy_only(Only::Mac(mac::HMAC_SHA1), None, "hmac-sha1").await;
    assert_legacy_only(
        Only::Mac(mac::HMAC_SHA1_ETM),
        None,
        "hmac-sha1-etm@openssh.com",
    )
    .await;
}

#[tokio::test]
async fn legacy_rsa_sha1_host_key() {
    assert_legacy_only(
        Only::HostKey(Algorithm::Rsa { hash: None }),
        Some(RSA_KEY.0.clone()),
        "ssh-rsa",
    )
    .await;
}

#[tokio::test]
async fn modern_rsa_host_key_is_not_legacy() {
    let config = SshServerConfig {
        host_key: Some(RSA_KEY.0.clone()),
        ..config(&[])
    };
    let options = restricted(&Only::HostKey(Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    }));
    let (server, _lab) = server_with(config, options).await;
    let params = SshParams {
        legacy: LegacyCrypto::Never,
        ..params(&server, "op", vec![])
    };
    let conn = login(&params).await;
    assert_eq!(
        conn.info().host_key_algorithm.as_deref(),
        Some("rsa-sha2-256")
    );
    assert!(!conn.info().legacy_crypto);
    conn.close().await;
}

#[tokio::test]
async fn always_does_not_flag_a_modern_server() {
    let (server, _lab) = server(&[]).await;
    let params = SshParams {
        legacy: LegacyCrypto::Always,
        ..params(&server, "op", vec![])
    };
    let conn = login(&params).await;
    assert!(!conn.info().legacy_crypto);
    assert_eq!(conn.info().kex.as_deref(), Some("mlkem768x25519-sha256"));
    conn.close().await;
}

#[tokio::test]
async fn algorithms_no_set_contains_are_reported_as_unsupported() {
    // A server whose only MAC is one the client has in neither set (`none`,
    // standing in for hmac-md5 and friends, which russh cannot offer).
    let mut options = restricted(&Only::Cipher(cipher::AES_256_CTR));
    if let Some(preferred) = options.preferred.as_mut() {
        preferred.mac = Cow::Owned(vec![mac::NONE]);
    }
    let (server, _lab) = server_with(config(&[]), options).await;
    for legacy in [
        LegacyCrypto::Auto,
        LegacyCrypto::Always,
        LegacyCrypto::Never,
    ] {
        let params = SshParams {
            legacy,
            ..params(&server, "op", vec![])
        };
        let error = login_error(&params).await;
        assert_eq!(error.code(), "algorithms", "{legacy:?}: {error}");
        let message = error.to_string();
        assert!(message.contains("no common MAC algorithm"), "{message}");
        assert!(message.contains("none of them is supported"), "{message}");
    }
}

// ------------------------------------------------------------ back-pressure

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_reader_does_not_stall_keepalives() {
    // The server drops clients that miss two keep-alives 150 ms apart.
    let options = SshServerOptions {
        keepalive: Some((Duration::from_millis(150), 2)),
        ..SshServerOptions::default()
    };
    let (server, lab) = server_with(config(&[]), options).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;

    // Lots of small chunks — the shape of real device output — and a reader
    // that looks away for many keep-alive periods.
    const TOTAL: usize = 1_500_000;
    let command = format!("flood {TOTAL} 60\r");
    conn.send(command.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;

    conn.until(command.as_bytes()).await;
    let payload = conn.exactly(TOTAL).await;
    for (index, byte) in payload.iter().enumerate() {
        assert_eq!(*byte, pattern(index), "byte {index} is wrong");
    }
    // Still alive and responsive afterwards.
    conn.send("size\r").await.unwrap();
    conn.until(b"120x40").await;
    conn.close().await;
    lab.wait_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_input_with_echo_does_not_deadlock() {
    let (server, _lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;

    // Send a lot without reading the echo in between: with pumps that block
    // one direction on the other this never finishes.
    const CHUNK: usize = 1000;
    const CHUNKS: usize = 2000;
    let chunk: Vec<u8> = (0..CHUNK).map(pattern).collect();
    for _ in 0..CHUNKS {
        soon(conn.send(chunk.clone())).await.unwrap();
    }
    let echoed = conn.exactly(CHUNK * CHUNKS).await;
    for (index, byte) in echoed.iter().enumerate() {
        assert_eq!(*byte, pattern(index % CHUNK), "byte {index} is wrong");
    }
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_reader_loses_nothing() {
    // Far more output than the transport is willing to stage: the flow has
    // to stop without dropping a byte, and resume when the reader returns.
    const TOTAL: usize = 24 * 1024 * 1024;
    let (server, _lab) = server(&[]).await;
    let mut conn = guest(&server).await;
    conn.until(b"lab> ").await;
    let command = format!("flood {TOTAL} 32000\r");
    conn.send(command.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;

    conn.until(command.as_bytes()).await;
    let payload = conn.exactly(TOTAL).await;
    for (index, byte) in payload.iter().enumerate() {
        assert_eq!(*byte, pattern(index), "byte {index} is wrong");
    }
    conn.until(b"\r\nlab> ").await;
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn input_far_beyond_what_the_server_stages_is_echoed_in_full() {
    // The device cannot keep up, so the server has to slow the client down
    // again and again; that must neither lose a byte nor lock anything up.
    const CHUNK: usize = 32_000;
    const CHUNKS: usize = 750;
    let (server, lab) = server(&[]).await;
    let (writer, reader) = guest(&server).await.inner.split();
    let mut reader = tap(reader);
    reader.until(b"lab> ").await;

    let sender = tokio::spawn(async move {
        let chunk = bytes::Bytes::from((0..CHUNK).map(pattern).collect::<Vec<u8>>());
        for _ in 0..CHUNKS {
            writer.send(chunk.clone()).await.unwrap();
        }
        writer
    });
    let echoed = reader.exactly(CHUNK * CHUNKS).await;
    for (index, byte) in echoed.iter().enumerate() {
        assert_eq!(*byte, pattern(index % CHUNK), "byte {index} is wrong");
    }
    let writer = soon(sender).await.unwrap();
    writer.close().await;
    let (_, reason) = reader.until_closed().await;
    assert_eq!(reason, None);
    lab.wait_idle().await;
}

/// One connection with two shells, the first of which talks to a device
/// that stopped reading and is sent input without end. Returns the session,
/// the stuck shell, the other shell, and the task that floods (which reports
/// how much the transport took off its hands).
async fn flood_a_stuck_device(
    server: &ServerHandle,
) -> (
    SshSession,
    Tap<Connection>,
    Tap<Connection>,
    tokio::task::JoinHandle<usize>,
) {
    let params = SshParams {
        // A connection that stops answering is noticed within seconds.
        keepalive_s: 1,
        ..params(server, "op", vec![])
    };
    let session = soon(SshSession::connect(&params, any_host_key()))
        .await
        .unwrap();
    let mut stuck = tap(soon(session.shell()).await.unwrap());
    let mut healthy = tap(soon(session.shell()).await.unwrap());
    stuck.until(b"lab> ").await;
    healthy.until(b"lab> ").await;
    stuck.send("hang\r").await.unwrap();
    stuck.until(b"hang\r").await;

    let writer = stuck.writer();
    let flood = tokio::spawn(async move {
        let chunk = bytes::Bytes::from(vec![b'x'; 32 * 1024]);
        let mut taken = 0;
        while writer.send(chunk.clone()).await.is_ok() {
            taken += chunk.len();
        }
        taken
    });
    (session, stuck, healthy, flood)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stuck_device_holds_its_client_back_and_nothing_else() {
    let (server, _lab) = server(&[]).await;
    let (session, stuck, mut healthy, flood) = flood_a_stuck_device(&server).await;

    // Long enough for a connection that froze to miss its keep-alives.
    for _ in 0..9 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        healthy.send("size\r").await.unwrap();
        healthy.until(b"120x40\r\nlab> ").await;
    }
    assert!(!flood.is_finished(), "the flood should be waiting");
    assert!(!stuck.is_closed());

    // Giving up on the stuck shell works, and tells how much got through:
    // megabytes sit in queues along the way, but not the endless stream.
    stuck.close().await;
    let taken = soon(flood).await.unwrap();
    assert!(taken < 32 * 1024 * 1024, "{taken} bytes were taken");

    healthy.send("exit\r").await.unwrap();
    let (_, reason) = healthy.until_closed().await;
    assert_eq!(reason.as_deref(), Some("exit status 0"));
    soon(session.close()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_overruns_the_server_is_cut_off_alone() {
    // Our own client respects its window, so the limit is lowered to where
    // the window does not protect it.
    let options = SshServerOptions {
        input_limit: Some(64 * 1024),
        ..SshServerOptions::default()
    };
    let (server, _lab) = server_with(config(&[]), options).await;
    let (session, mut stuck, mut healthy, flood) = flood_a_stuck_device(&server).await;

    let (rest, reason) = stuck.until_closed().await;
    assert!(
        contains(&rest, b"too much input that the device does not read"),
        "{:?}",
        String::from_utf8_lossy(&rest)
    );
    assert_eq!(reason.as_deref(), Some("exit status 1"));
    soon(flood).await.unwrap();

    // The connection, and the other shell on it, are none the worse for it.
    healthy.send("size\r").await.unwrap();
    healthy.until(b"120x40").await;
    healthy.send("exit\r").await.unwrap();
    let (_, reason) = healthy.until_closed().await;
    assert_eq!(reason.as_deref(), Some("exit status 0"));
    soon(session.close()).await;
}
