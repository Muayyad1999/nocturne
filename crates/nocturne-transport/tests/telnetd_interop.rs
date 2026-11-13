//! Interoperability with the Telnet daemon installed on this machine (GNU
//! inetutils `telnetd`).
//!
//! `telnetd` expects to be started by inetd with the connection as its
//! standard input and output, so the test plays that part itself: it accepts
//! connections on a port of its own and runs the daemon for each. The login
//! is a real one, as a throwaway user. Skipped — not failed — where that is
//! not possible; the reason is printed.

#![cfg(unix)]

mod common;

use std::net::TcpListener;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use common::{Account, Tap, contains, skipped, soon, tap};
use nocturne_transport::{Connection, TelnetParams, TransportKind, connect_telnet};

const TELNETD: &str = "/usr/sbin/telnetd";

/// A one-service inetd: runs `telnetd` for every connection to its port.
struct Inetd {
    port: u16,
    account: &'static Account,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<Vec<Child>>>,
}

impl Inetd {
    /// `None` where these tests cannot run.
    fn start() -> Option<Inetd> {
        let account = common::account()?;
        if !Path::new(TELNETD).is_file() {
            skipped(&format!(
                "{TELNETD} is not installed; Telnet interoperability not tested"
            ));
            return None;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let acceptor = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut daemons = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((socket, _)) => daemons.push(Self::serve(socket, account)),
                        Err(_) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
                daemons
            }
        });
        Some(Inetd {
            port,
            account,
            stop,
            acceptor: Some(acceptor),
        })
    }

    /// Runs `telnetd` with the connection as its standard streams.
    fn serve(socket: std::net::TcpStream, account: &Account) -> Child {
        socket.set_nonblocking(false).unwrap();
        let stream = || Stdio::from(OwnedFd::from(socket.try_clone().unwrap()));
        let child = Command::new(TELNETD)
            .stdin(stream())
            .stdout(stream())
            .stderr(stream())
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {TELNETD}: {error}"));
        // For the watchdog, should the test die before it can clean up.
        let pid_file = account.unique_path("telnetd", "pid");
        std::fs::write(pid_file, child.id().to_string()).unwrap();
        child
    }

    fn params(&self) -> TelnetParams {
        TelnetParams {
            host: "127.0.0.1".to_owned(),
            port: self.port,
            ..TelnetParams::default()
        }
    }

    /// Connects and logs in as the test account, up to the shell.
    async fn login(&self, params: &TelnetParams) -> Tap<Connection> {
        let mut conn = tap(soon(connect_telnet(params)).await.unwrap());
        conn.until(b"login: ").await;
        conn.send(format!("{}\r", self.account.name)).await.unwrap();
        conn.until(b"Password: ").await;
        conn.send(format!("{}\r", self.account.password))
            .await
            .unwrap();
        // `login` discards what is typed ahead of the shell: wait for it.
        conn.until(b"$ ").await;
        // The shell computes the marker; the echo of the command cannot match.
        conn.send("stty -echo; echo \"IN$((1 + 1))IN\"\r")
            .await
            .unwrap();
        conn.until(b"IN2IN").await;
        conn
    }
}

impl Drop for Inetd {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(acceptor) = self.acceptor.take() {
            for mut daemon in acceptor.join().unwrap_or_default() {
                let _ = daemon.kill();
                let _ = daemon.wait();
            }
        }
    }
}

#[tokio::test]
async fn login_and_shell_over_telnet() {
    let Some(inetd) = Inetd::start() else { return };
    let params = TelnetParams {
        cols: 132,
        rows: 50,
        term: "vt220".to_owned(),
        ..inetd.params()
    };
    let mut conn = inetd.login(&params).await;
    assert_eq!(conn.info().kind, Some(TransportKind::Telnet));
    assert_eq!(conn.info().peer, format!("127.0.0.1:{}", inetd.port));

    // The daemon asked for, and applied, the terminal type and window size.
    conn.send("echo \"term=$TERM.\"\r").await.unwrap();
    conn.until(b"term=vt220.").await;
    conn.send("stty size\r").await.unwrap();
    conn.until(b"50 132").await;
    conn.resize(100, 30).await.unwrap();
    conn.send("stty size\r").await.unwrap();
    conn.until(b"30 100").await;

    conn.send("id -un | tr a-z A-Z\r").await.unwrap();
    conn.until(inetd.account.name.to_uppercase().as_bytes())
        .await;

    conn.send("exit\r").await.unwrap();
    let (_, reason) = conn.until_closed().await;
    assert_eq!(reason.as_deref(), Some("closed by remote host"));
}

#[tokio::test]
async fn data_passes_unharmed_through_the_daemon() {
    let Some(inetd) = Inetd::start() else { return };
    let mut conn = inetd.login(&inetd.params()).await;

    // Bytes that are special on the wire, from the server…
    conn.send("printf 'A\\377\\377B\\r\\000C\\n'; echo \"E$((2 + 2))E\"\r")
        .await
        .unwrap();
    let output = conn.until(b"E4E").await;
    assert!(
        contains(
            &output,
            &[b'A', 0xFF, 0xFF, b'B', b'\r', 0x00, b'C', b'\r', b'\n']
        ),
        "{output:?}"
    );

    // …a CR LF from the client is one line ending, not two…
    conn.send("echo \"one$((1 + 0))\"\r\necho \"two$((1 + 1))\"\r\n")
        .await
        .unwrap();
    let output = conn.until(b"two2").await;
    // The prompt left from before, then each answer after exactly one
    // prompt: an extra, empty command would show as a third `$ `.
    assert_eq!(String::from_utf8_lossy(&output), "\r\n$ one1\r\n$ two2");

    // …and a volume that needs flow control on the way.
    // (The empty line keeps the first number off the prompt's line.)
    conn.send("echo; seq 1 100000; echo \"S$((3 + 3))S\"\r")
        .await
        .unwrap();
    let output = conn.until(b"S6S").await;
    let numbers: Vec<&[u8]> = output
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty() && line.iter().all(u8::is_ascii_digit))
        .collect();
    assert_eq!(numbers.len(), 100_000);
    assert_eq!(numbers[0], b"1");
    assert_eq!(numbers[99_999], b"100000");

    conn.close().await;
}

#[tokio::test]
async fn a_break_reaches_the_remote_terminal() {
    let Some(inetd) = Inetd::start() else { return };
    let mut conn = inetd.login(&inetd.params()).await;
    // The daemon turns a Telnet break into the terminal's interrupt, which
    // ends the foreground command.
    conn.send("sleep 60; echo \"B$((4 + 4))B rc=$?\"\r")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(conn.supports_break());
    conn.send_break().await.unwrap();
    conn.until(b"B8B").await;
    conn.send("echo \"A$((5 + 5))A\"\r").await.unwrap();
    conn.until(b"A10A").await;
    conn.close().await;
}

#[tokio::test]
async fn a_wrong_password_is_refused_by_login() {
    let Some(inetd) = Inetd::start() else { return };
    let mut conn = tap(soon(connect_telnet(&inetd.params())).await.unwrap());
    conn.until(b"login: ").await;
    conn.send(format!("{}\r", inetd.account.name))
        .await
        .unwrap();
    conn.until(b"Password: ").await;
    conn.send("not the password\r").await.unwrap();
    conn.until(b"Login incorrect").await;
    conn.close().await;
}
