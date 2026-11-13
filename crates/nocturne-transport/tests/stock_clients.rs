//! This crate's servers against the clients everybody has: OpenSSH `ssh`,
//! `telnet` and `nc`, as installed on this machine. Skipped — not failed —
//! where a program is missing; the reason is printed.
//!
//! Where a client wants a terminal it gets one from this crate's own local
//! transport, which runs it in a pseudo-terminal.

#![cfg(unix)]

mod common;

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use common::{PATIENCE, Tap, contains, skipped, soon, tap, which};
use nocturne_transport::serve::{
    ServerHandle, SessionHandler, SshServerConfig, serve_ssh, serve_telnet,
};
use nocturne_transport::{
    Backend, Connection, ConnectionInfo, Input, KeyKind, LocalParams, Output, TransportKind,
    connect_local, generate_keypair,
};
use tokio::io::AsyncWriteExt;

// ------------------------------------------------------------ the device

/// Ends the "sink" mode of the device (Ctrl-D).
const END_OF_TRANSMISSION: u8 = 0x04;

/// A lab whose sessions are fake device consoles (see [`console`]).
#[derive(Default)]
struct Lab {
    /// Sessions currently running.
    active: Arc<AtomicUsize>,
}

impl Lab {
    /// Waits until no device session is left running.
    async fn wait_idle(&self) {
        let idle = async {
            while self.active.load(Ordering::Acquire) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(PATIENCE, idle)
            .await
            .expect("device sessions should have ended");
    }
}

#[async_trait::async_trait]
impl SessionHandler for Lab {
    async fn open(
        &self,
        _peer: SocketAddr,
        user: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Option<Connection> {
        let info = ConnectionInfo {
            kind: Some(TransportKind::Sim),
            peer: "lab:device".to_owned(),
            ..ConnectionInfo::default()
        };
        let (connection, backend) = Connection::pair(info, Connection::DEFAULT_BUFFER);
        let active = Arc::clone(&self.active);
        let user = user.unwrap_or("nobody").to_owned();
        active.fetch_add(1, Ordering::AcqRel);
        tokio::spawn(async move {
            console(backend, &user, cols, rows).await;
            active.fetch_sub(1, Ordering::AcqRel);
        });
        Some(connection)
    }
}

/// The checksum the device reports for what it was fed in "sink" mode.
fn checksum(sum: u32, data: &[u8]) -> u32 {
    data.iter().fold(sum, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(u32::from(*byte))
    })
}

/// A fake device console.
///
/// It greets with `Welcome <user> <cols>x<rows>` and a `lab> ` prompt, echoes
/// what it receives line by line, and understands (lines end in CR or LF):
///
/// * `exit` — say `bye` and close the session;
/// * `size` — print the window size;
/// * `count <n>` — print the lines `line 1` … `line <n>`;
/// * `sink` — stop echoing and, slowly, swallow everything up to a Ctrl-D,
///   then report how many bytes that was and their checksum.
///
/// Resizes are reported as `[resize <cols>x<rows>]`.
async fn console(mut backend: Backend, user: &str, mut cols: u16, mut rows: u16) {
    let output = backend.output.clone();
    let say = async |text: String| output.send(Output::Data(Bytes::from(text))).await.is_ok();
    if !say(format!("Welcome {user} {cols}x{rows}\r\nlab> ")).await {
        return;
    }
    let mut line = Vec::new();
    // In "sink" mode: the bytes swallowed so far and their checksum.
    let mut sink: Option<(usize, u32)> = None;
    while let Some(input) = backend.input.recv().await {
        let mut data = match input {
            Input::Data(data) => data,
            Input::Resize { cols: c, rows: r } => {
                (cols, rows) = (c, r);
                if !say(format!("\r\n[resize {cols}x{rows}]\r\n")).await {
                    return;
                }
                continue;
            }
            Input::Break => continue,
            Input::Close => break,
        };
        while !data.is_empty() {
            if let Some((count, sum)) = &mut sink {
                // A device that cannot keep up with what it is sent.
                tokio::time::sleep(Duration::from_millis(2)).await;
                let end = data.iter().position(|byte| *byte == END_OF_TRANSMISSION);
                let swallowed = data.split_to(end.unwrap_or(data.len()));
                *count += swallowed.len();
                *sum = checksum(*sum, &swallowed);
                if end.is_some() {
                    let report = format!("\r\nswallowed {count} bytes, checksum {sum}\r\nlab> ");
                    sink = None;
                    data = data.slice(1..);
                    if !say(report).await {
                        return;
                    }
                }
                continue;
            }

            let end = data.iter().position(|byte| matches!(byte, b'\r' | b'\n'));
            let typed = data.split_to(end.map_or(data.len(), |end| end + 1));
            if output.send(Output::Data(typed.clone())).await.is_err() {
                return;
            }
            if end.is_none() {
                line.extend_from_slice(&typed);
                continue;
            }
            line.extend_from_slice(&typed[..typed.len() - 1]);
            let command = String::from_utf8_lossy(&std::mem::take(&mut line)).into_owned();
            let words: Vec<&str> = command.split_whitespace().collect();
            let reply = match words[..] {
                [] => continue,
                ["exit"] => {
                    let _ = say("\r\nbye\r\n".to_owned()).await;
                    let _ = output.send(Output::Closed { reason: None }).await;
                    return;
                }
                ["size"] => format!("\r\n{cols}x{rows}\r\nlab> "),
                ["count", total] => {
                    let total: usize = total.parse().unwrap_or(0);
                    let mut lines = String::from("\r\n");
                    for number in 1..=total {
                        lines.push_str(&format!("line {number}\r\n"));
                        if lines.len() > 8000 && !say(std::mem::take(&mut lines)).await {
                            return;
                        }
                    }
                    lines + "lab> "
                }
                ["sink"] => {
                    sink = Some((0, 0));
                    continue;
                }
                _ => "\r\nlab> ".to_owned(),
            };
            if !say(reply).await {
                return;
            }
        }
    }
    let _ = output.send(Output::Closed { reason: None }).await;
}

// --------------------------------------------------------------- helpers

/// `None` (after saying so) when `program` is not installed.
fn installed(program: &str) -> Option<String> {
    match which(program) {
        Some(path) => Some(path.to_string_lossy().into_owned()),
        None => {
            skipped(&format!("`{program}` is not installed"));
            None
        }
    }
}

async fn ssh_server(config: SshServerConfig) -> (ServerHandle, Arc<Lab>) {
    let lab = Arc::new(Lab::default());
    let server = serve_ssh(config, lab.clone()).await.unwrap();
    (server, lab)
}

async fn telnet_server() -> (ServerHandle, Arc<Lab>) {
    let lab = Arc::new(Lab::default());
    let server = serve_telnet("127.0.0.1:0".parse().unwrap(), lab.clone())
        .await
        .unwrap();
    (server, lab)
}

/// The arguments for `ssh` to reach `server` as `user`, touching none of
/// the configuration, keys or known hosts of whoever runs the tests.
fn ssh_args(server: &ServerHandle, user: &str, options: &[&str]) -> Vec<String> {
    let port = server.local_addr().port().to_string();
    let mut args: Vec<String> = [
        "-F",
        "/dev/null",
        "-p",
        port.as_str(),
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "GlobalKnownHostsFile=/dev/null",
        "-o",
        "IdentityAgent=none",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "ConnectTimeout=20",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    args.extend(options.iter().map(|option| (*option).to_owned()));
    args.push(format!("{user}@127.0.0.1"));
    args
}

/// What a program that was fed its input through a pipe did.
struct Piped {
    /// The exit code; `None` when a signal ended it.
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

/// Runs `program` with `input` on its standard input, which is closed after
/// it, and collects what it prints.
async fn pipe(program: &str, args: &[String], envs: &[(&str, &str)], input: Vec<u8>) -> Piped {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
    let mut stdin = child.stdin.take().unwrap();
    let feeding = tokio::spawn(async move {
        // The program may be gone before it took everything.
        let _ = stdin.write_all(&input).await;
        let _ = stdin.shutdown().await;
    });
    let output = soon(child.wait_with_output()).await.unwrap();
    feeding.abort();
    Piped {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Runs `program` in a pseudo-terminal of the given size.
async fn in_terminal(program: &str, args: Vec<String>, cols: u16, rows: u16) -> Tap<Connection> {
    let params = LocalParams {
        program: Some(program.to_owned()),
        args,
        cols,
        rows,
        ..LocalParams::default()
    };
    tap(soon(connect_local(&params)).await.unwrap())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

// ------------------------------------------------------------------- ssh

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_runs_a_script_without_a_terminal() {
    let Some(ssh) = installed("ssh") else { return };
    let config = SshServerConfig {
        banner: Some("Lab access only.\n".to_owned()),
        ..SshServerConfig::default()
    };
    let (server, lab) = ssh_server(config).await;
    let args = ssh_args(&server, "op", &["-T", "-o", "BatchMode=yes"]);

    // The device ends the session.
    let run = pipe(&ssh, &args, &[], b"size\rcount 3\rexit\r".to_vec()).await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        text(&run.stdout),
        "Welcome op 80x24\r\nlab> size\r\r\n80x24\r\nlab> count 3\r\r\nline 1\r\nline 2\r\n\
         line 3\r\nlab> exit\r\r\nbye\r\n"
    );
    assert!(run.stderr.contains("Lab access only."), "{}", run.stderr);

    // The script just ends: the device gets to finish it first.
    let run = pipe(&ssh, &args, &[], b"count 20000\rsize\r".to_vec()).await;
    let seen = text(&run.stdout);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let lines: String = (1..=20000).map(|n| format!("line {n}\r\n")).collect();
    assert!(seen.contains(&lines), "the output was cut short");
    assert!(seen.ends_with("line 20000\r\nlab> size\r\r\n80x24\r\nlab> "));

    // A terminal is asked for by a client that has none itself: the size it
    // then reports (0 x 0) says nothing and must not reach the device.
    let args = ssh_args(&server, "op", &["-tt", "-o", "BatchMode=yes"]);
    let run = pipe(&ssh, &args, &[], b"size\rexit\r".to_vec()).await;
    let seen = text(&run.stdout);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(seen.starts_with("Welcome op 80x24\r\nlab> "), "{seen}");
    assert!(seen.contains("\r\n80x24\r\nlab> "), "{seen}");
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_in_a_terminal_reports_its_size_and_resizes() {
    let Some(ssh) = installed("ssh") else { return };
    let (server, lab) = ssh_server(SshServerConfig::default()).await;
    let args = ssh_args(
        &server,
        "op",
        &["-o", "BatchMode=yes", "-o", "LogLevel=ERROR"],
    );
    let mut terminal = in_terminal(&ssh, args, 132, 43).await;

    terminal.until(b"Welcome op 132x43\r\nlab> ").await;
    terminal.send("size\r").await.unwrap();
    let answer = terminal.until(b"\r\n132x43\r\nlab> ").await;
    // The device echoes; the terminal of the client must not.
    assert_eq!(occurrences(&answer, b"size"), 1, "{}", text(&answer));

    terminal.resize(100, 30).await.unwrap();
    terminal.until(b"[resize 100x30]").await;
    terminal.send("size\r").await.unwrap();
    terminal.until(b"\r\n100x30\r\nlab> ").await;

    terminal.send("exit\r").await.unwrap();
    let (rest, reason) = terminal.until_closed().await;
    assert!(contains(&rest, b"bye"), "{}", text(&rest));
    assert_eq!(reason.as_deref(), Some("exit status 0"));
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_logs_in_with_a_password_either_way() {
    let Some(ssh) = installed("ssh") else { return };
    let config = SshServerConfig {
        users: vec![("admin".to_owned(), "s3cret pass".to_owned())],
        ..SshServerConfig::default()
    };
    let (server, lab) = ssh_server(config).await;

    // `ssh` asks this program for the password instead of a terminal.
    let scratch = tempfile::tempdir().unwrap();
    let askpass = scratch.path().join("askpass");
    std::fs::write(
        &askpass,
        "#!/bin/sh\nprintf '%s\\n' \"$NOCTURNE_TEST_PASSWORD\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o700)).unwrap();
    let askpass = askpass.to_string_lossy().into_owned();

    for method in ["password", "keyboard-interactive"] {
        let preferred = format!("PreferredAuthentications={method}");
        let options = [
            "-T",
            "-o",
            preferred.as_str(),
            "-o",
            "NumberOfPasswordPrompts=1",
        ];
        let login = async |user: &str, password: &str| {
            let envs = [
                ("SSH_ASKPASS", askpass.as_str()),
                ("SSH_ASKPASS_REQUIRE", "force"),
                ("NOCTURNE_TEST_PASSWORD", password),
            ];
            let args = ssh_args(&server, user, &options);
            pipe(&ssh, &args, &envs, b"exit\r".to_vec()).await
        };

        let run = login("admin", "s3cret pass").await;
        let seen = text(&run.stdout);
        assert_eq!(run.code, Some(0), "{method}: {seen}\n{}", run.stderr);
        assert!(seen.starts_with("Welcome admin 80x24"), "{method}: {seen}");
        assert!(seen.ends_with("bye\r\n"), "{method}: {seen}");

        for (user, password) in [("admin", "wrong"), ("nobody", "s3cret pass")] {
            let run = login(user, password).await;
            assert_eq!(run.code, Some(255), "{method}: {}", run.stderr);
            assert!(run.stdout.is_empty(), "{method}: {}", text(&run.stdout));
            assert!(
                run.stderr.contains("Permission denied"),
                "{method}: {}",
                run.stderr
            );
        }
    }
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_negotiates_every_modern_algorithm_and_host_key() {
    let Some(ssh) = installed("ssh") else { return };
    let offered = |kind: &str| -> Vec<String> {
        common::run(&ssh, &["-Q", kind])
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    };
    let (rsa, _) = generate_keypair(KeyKind::Rsa3072, "host").unwrap();
    let (ed25519, _) = generate_keypair(KeyKind::Ed25519, "host").unwrap();

    let mut checked = Vec::new();
    for (host_key, host_key_algorithms) in [
        (ed25519, vec!["ssh-ed25519"]),
        (rsa, vec!["rsa-sha2-512", "rsa-sha2-256"]),
    ] {
        let config = SshServerConfig {
            host_key: Some(host_key),
            ..SshServerConfig::default()
        };
        let (server, lab) = ssh_server(config).await;
        let mut choices: Vec<(&str, &str)> = host_key_algorithms
            .iter()
            .map(|algorithm| ("HostKeyAlgorithms", *algorithm))
            .collect();
        if checked.is_empty() {
            // Once is enough for what does not depend on the host key.
            choices.extend([
                ("KexAlgorithms", "mlkem768x25519-sha256"),
                ("KexAlgorithms", "curve25519-sha256"),
                ("KexAlgorithms", "curve25519-sha256@libssh.org"),
                ("KexAlgorithms", "ecdh-sha2-nistp256"),
                ("KexAlgorithms", "ecdh-sha2-nistp384"),
                ("KexAlgorithms", "ecdh-sha2-nistp521"),
                ("KexAlgorithms", "diffie-hellman-group-exchange-sha256"),
                ("KexAlgorithms", "diffie-hellman-group14-sha256"),
                ("KexAlgorithms", "diffie-hellman-group16-sha512"),
                ("KexAlgorithms", "diffie-hellman-group18-sha512"),
                ("Ciphers", "chacha20-poly1305@openssh.com"),
                ("Ciphers", "aes256-gcm@openssh.com"),
                ("Ciphers", "aes128-gcm@openssh.com"),
                ("Ciphers", "aes256-ctr"),
                ("Ciphers", "aes192-ctr"),
                ("Ciphers", "aes128-ctr"),
                ("MACs", "hmac-sha2-256-etm@openssh.com"),
                ("MACs", "hmac-sha2-512-etm@openssh.com"),
                ("MACs", "hmac-sha2-256"),
                ("MACs", "hmac-sha2-512"),
                ("Compression", "yes"),
            ]);
        }
        for (option, value) in choices {
            let kind = match option {
                "KexAlgorithms" => Some("kex"),
                "Ciphers" => Some("cipher"),
                "MACs" => Some("mac"),
                "HostKeyAlgorithms" => Some("key-sig"),
                _ => None,
            };
            if kind.is_some_and(|kind| !offered(kind).iter().any(|name| name == value)) {
                skipped(&format!("this `ssh` has no {value}"));
                continue;
            }
            let choice = format!("{option}={value}");
            let mut options = vec!["-T", "-o", "BatchMode=yes", "-o", choice.as_str()];
            if option == "MACs" {
                // A MAC is only negotiated with a cipher that needs one.
                options.extend(["-o", "Ciphers=aes128-ctr"]);
            }
            let args = ssh_args(&server, "op", &options);
            let run = pipe(&ssh, &args, &[], b"count 300\rexit\r".to_vec()).await;
            let seen = text(&run.stdout);
            assert_eq!(run.code, Some(0), "{choice}: {}", run.stderr);
            assert!(seen.contains("line 300\r\nlab> "), "{choice}: {seen}");
            assert!(seen.ends_with("bye\r\n"), "{choice}: {seen}");
            checked.push(choice);
        }
        lab.wait_idle().await;
        soon(server.shutdown()).await;
    }
    assert!(checked.len() >= 12, "too little was checked: {checked:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_bulk_input_reaches_a_slow_device_complete_and_in_order() {
    let Some(ssh) = installed("ssh") else { return };
    let (server, lab) = ssh_server(SshServerConfig::default()).await;
    let args = ssh_args(&server, "op", &["-T", "-o", "BatchMode=yes"]);

    // Far more than the server lets a client send ahead of a device, so the
    // client is slowed down — and let go again — over and over.
    const TOTAL: usize = 12 * 1024 * 1024;
    let payload: Vec<u8> = (0..TOTAL).map(|index| b'a' + (index % 23) as u8).collect();
    let expected = format!(
        "swallowed {TOTAL} bytes, checksum {}",
        checksum(0, &payload)
    );
    let mut script = b"sink\r".to_vec();
    script.extend_from_slice(&payload);
    script.push(END_OF_TRANSMISSION);
    script.extend_from_slice(b"size\rexit\r");

    let run = pipe(&ssh, &args, &[], script).await;
    let seen = text(&run.stdout);
    assert_eq!(run.code, Some(0), "{seen}\n{}", run.stderr);
    assert_eq!(
        seen,
        format!(
            "Welcome op 80x24\r\nlab> sink\r\r\n{expected}\r\nlab> size\r\r\n80x24\r\nlab> exit\r\r\nbye\r\n"
        )
    );
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_killed_ssh_client_ends_the_device_session() {
    let Some(ssh) = installed("ssh") else { return };
    let (server, lab) = ssh_server(SshServerConfig::default()).await;
    let args = ssh_args(
        &server,
        "op",
        &["-o", "BatchMode=yes", "-o", "LogLevel=ERROR"],
    );
    let mut terminal = in_terminal(&ssh, args, 80, 24).await;
    terminal.until(b"lab> ").await;
    assert_eq!(lab.active.load(Ordering::Acquire), 1);

    // Closing the local terminal hangs up on `ssh`.
    terminal.close().await;
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

// ---------------------------------------------------------------- telnet

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telnet_in_a_terminal_negotiates_echo_and_window_size() {
    let Some(telnet) = installed("telnet") else {
        return;
    };
    let (server, lab) = telnet_server().await;
    let port = server.local_addr().port().to_string();
    let mut terminal = in_terminal(&telnet, vec!["127.0.0.1".to_owned(), port], 132, 43).await;

    // The window size was known when the session opened.
    terminal.until(b"Welcome nobody 132x43\r\nlab> ").await;
    terminal.send("size\r").await.unwrap();
    let answer = terminal.until(b"\r\n132x43\r\nlab> ").await;
    // Character at a time, echoed by the device and by nobody else.
    assert_eq!(occurrences(&answer, b"size"), 1, "{}", text(&answer));

    terminal.resize(90, 28).await.unwrap();
    terminal.until(b"[resize 90x28]").await;
    terminal.send("count 3\r").await.unwrap();
    terminal
        .until(b"\r\nline 1\r\nline 2\r\nline 3\r\nlab> ")
        .await;

    terminal.send("exit\r").await.unwrap();
    let (rest, reason) = terminal.until_closed().await;
    let rest = text(&rest);
    assert!(rest.contains("bye"), "{rest}");
    assert!(rest.contains("Connection closed by foreign host"), "{rest}");
    assert!(reason.is_some_and(|reason| reason.starts_with("exit status")));
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nc_pipelines_get_the_whole_answer() {
    let Some(nc) = installed("nc") else { return };
    let (server, lab) = telnet_server().await;
    let port = server.local_addr().port().to_string();
    // `-N`: close the sending side once the script is out.
    let args = ["-N".to_owned(), "127.0.0.1".to_owned(), port];

    let run = pipe(&nc, &args, &[], b"size\r\ncount 20000\r\n".to_vec()).await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let seen = text(&run.stdout);
    assert!(seen.contains("Welcome nobody 80x24\r\nlab> "), "{seen}");
    let lines: String = (1..=20000).map(|n| format!("line {n}\r\n")).collect();
    assert!(seen.contains(&lines), "the output was cut short");
    assert!(seen.ends_with("line 20000\r\nlab> "));
    lab.wait_idle().await;

    let run = pipe(&nc, &args, &[], b"exit\r\nnever read\r\n".to_vec()).await;
    let seen = text(&run.stdout);
    assert!(seen.ends_with("\r\nbye\r\n"), "{seen}");
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}
