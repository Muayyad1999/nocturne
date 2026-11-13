//! The syslog listener over UDP and both TCP framings, fed by the test itself and by the real
//! `logger` program.

mod support;

use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::syslog::{SyslogMessage, SyslogServer};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc::UnboundedReceiver;

async fn start() -> (
    SyslogServer,
    SocketAddr,
    SocketAddr,
    UnboundedReceiver<SyslogMessage>,
) {
    let (server, messages) = SyslogServer::start(
        "127.0.0.1:0".parse().unwrap(),
        Some("127.0.0.1:0".parse().unwrap()),
    )
    .await
    .unwrap();
    let (udp, tcp) = (server.udp_addr(), server.tcp_addr().unwrap());
    (server, udp, tcp, messages)
}

async fn next(messages: &mut UnboundedReceiver<SyslogMessage>) -> SyslogMessage {
    tokio::time::timeout(Duration::from_secs(10), messages.recv())
        .await
        .expect("a message within ten seconds")
        .expect("the server is running")
}

#[tokio::test]
async fn udp_datagrams_become_messages() {
    let (_server, udp, _tcp, mut messages) = start().await;
    let device = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    device
        .send_to(b"<189>45: *Mar  1 00:02:11.003: %LINK-3-UPDOWN: Interface FastEthernet0/1, changed state to up", udp)
        .await
        .unwrap();
    let message = next(&mut messages).await;
    assert_eq!(message.source, "127.0.0.1");
    assert_eq!((message.facility, message.severity), (Some(23), Some(5)));
    assert_eq!(message.sequence, Some(45));
    let mnemonic = message.mnemonic.unwrap();
    assert_eq!(
        (
            mnemonic.facility.as_str(),
            mnemonic.severity,
            mnemonic.name.as_str()
        ),
        ("LINK", Some(3), "UPDOWN")
    );
    assert_eq!(
        message.message,
        "Interface FastEthernet0/1, changed state to up"
    );

    // Order is kept, and a large datagram arrives whole.
    let long_text = "x".repeat(8000);
    device
        .send_to(
            b"<134>system,info,account user admin logged in from 10.0.0.2 via ssh",
            udp,
        )
        .await
        .unwrap();
    device
        .send_to(
            format!("<14>Nov 13 10:15:01 host app: {long_text}").as_bytes(),
            udp,
        )
        .await
        .unwrap();
    device.send_to(b"", udp).await.unwrap();
    device
        .send_to(b"<13>after the empty one", udp)
        .await
        .unwrap();
    assert_eq!(
        next(&mut messages).await.message,
        "user admin logged in from 10.0.0.2 via ssh"
    );
    assert_eq!(next(&mut messages).await.message, long_text);
    let empty = next(&mut messages).await;
    assert_eq!((empty.raw.as_str(), empty.message.as_str()), ("", ""));
    assert_eq!(next(&mut messages).await.message, "after the empty one");
}

#[tokio::test]
async fn tcp_newline_framed_and_octet_counted() {
    let (_server, _udp, tcp, mut messages) = start().await;

    // Newline framing, with a message split across two writes.
    let mut lines = TcpStream::connect(tcp).await.unwrap();
    lines
        .write_all(b"<13>Nov 13 10:15:01 sw1 app: first\n<13>Nov 13 10:15:02 sw1 app: sec")
        .await
        .unwrap();
    lines.flush().await.unwrap();
    assert_eq!(next(&mut messages).await.message, "first");
    tokio::time::sleep(Duration::from_millis(50)).await;
    lines.write_all(b"ond\r\n").await.unwrap();
    let second = next(&mut messages).await;
    assert_eq!(
        (second.message.as_str(), second.hostname.as_deref()),
        ("second", Some("sw1"))
    );

    // Octet counting on another connection, several frames in one write and one in pieces.
    let mut counted = TcpStream::connect(tcp).await.unwrap();
    let one =
        "<165>1 2026-11-13T10:15:01Z mx1 mgd 3046 UI_COMMIT - User 'admin' requested 'commit'";
    let two = "<14>multi\nline message";
    counted
        .write_all(format!("{} {one}{} {two}", one.len(), two.len()).as_bytes())
        .await
        .unwrap();
    let first = next(&mut messages).await;
    assert_eq!(first.msgid.as_deref(), Some("UI_COMMIT"));
    assert_eq!(first.message, "User 'admin' requested 'commit'");
    assert_eq!(next(&mut messages).await.message, "multi\nline message");
    for piece in ["1", "0 <13>", "pie", "ces"] {
        counted.write_all(piece.as_bytes()).await.unwrap();
        counted.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(next(&mut messages).await.message, "pieces");

    // Both framings on one connection, and a last line without its newline.
    let mut mixed = TcpStream::connect(tcp).await.unwrap();
    mixed
        .write_all(b"<13>line\n9 <13>count<13>tail without newline")
        .await
        .unwrap();
    mixed.shutdown().await.unwrap();
    assert_eq!(next(&mut messages).await.message, "line");
    assert_eq!(next(&mut messages).await.message, "count");
    assert_eq!(next(&mut messages).await.message, "tail without newline");
}

#[tokio::test]
async fn many_senders_at_once() {
    let (_server, udp, tcp, mut messages) = start().await;
    let mut senders = tokio::task::JoinSet::new();
    for sender in 0..20u32 {
        senders.spawn(async move {
            if sender % 2 == 0 {
                let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                for index in 0..50 {
                    socket
                        .send_to(
                            format!("<13>udp sender {sender} message {index}").as_bytes(),
                            udp,
                        )
                        .await
                        .unwrap();
                    // Loopback UDP has no flow control; do not outrun the receiver.
                    tokio::task::yield_now().await;
                }
            } else {
                let mut stream = TcpStream::connect(tcp).await.unwrap();
                for index in 0..50 {
                    stream
                        .write_all(format!("<13>tcp sender {sender} message {index}\n").as_bytes())
                        .await
                        .unwrap();
                }
                stream.shutdown().await.unwrap();
            }
        });
    }
    while let Some(result) = senders.join_next().await {
        result.unwrap();
    }
    let mut seen = std::collections::HashSet::new();
    while seen.len() < 1000 {
        let message = next(&mut messages).await;
        assert!(message.message.contains(" sender "), "{message:?}");
        assert!(seen.insert(message.message), "every message arrives once");
    }
}

#[tokio::test]
async fn hostile_input_does_not_disturb_the_listener() {
    let (_server, udp, tcp, mut messages) = start().await;
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut state = 0x0123_4567_89ab_cdefu64;
    let mut random = move |limit: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % limit
    };
    for _ in 0..3000 {
        let junk: Vec<u8> = (0..random(1400)).map(|_| random(256) as u8).collect();
        socket.send_to(&junk, udp).await.unwrap();
        if random(50) == 0 {
            // Keep the channel from growing: this test is about the listener, not the queue.
            while messages.try_recv().is_ok() {}
            tokio::task::yield_now().await;
        }
    }
    // TCP: binary junk, an absurd length prefix, a line that never ends, an abrupt close.
    let mut junk = TcpStream::connect(tcp).await.unwrap();
    let noise: Vec<u8> = (0..50_000).map(|_| random(256) as u8).collect();
    let _ = junk.write_all(&noise).await;
    drop(junk);
    let mut absurd = TcpStream::connect(tcp).await.unwrap();
    let _ = absurd
        .write_all(b"4000000000 <13>this frame is four gigabytes long, honest")
        .await;
    let mut endless = TcpStream::connect(tcp).await.unwrap();
    for _ in 0..40 {
        if endless.write_all(&[b'a'; 8192]).await.is_err() {
            break;
        }
    }
    drop(endless);
    tokio::time::sleep(Duration::from_millis(200)).await;
    while messages.try_recv().is_ok() {}

    // Still listening on both, still parsing correctly.
    socket
        .send_to(b"<13>Nov 13 10:15:01 sw1 app: still here over UDP", udp)
        .await
        .unwrap();
    assert_eq!(next(&mut messages).await.message, "still here over UDP");
    let mut stream = TcpStream::connect(tcp).await.unwrap();
    stream
        .write_all(b"<13>Nov 13 10:15:01 sw1 app: still here over TCP\n")
        .await
        .unwrap();
    loop {
        // Leftovers of the junk connections may still trickle in.
        if next(&mut messages).await.message == "still here over TCP" {
            break;
        }
    }
}

#[tokio::test]
async fn stopping_and_dropping_release_the_ports() {
    let (server, udp, tcp, mut messages) = start().await;
    let mut open = TcpStream::connect(tcp).await.unwrap();
    open.write_all(b"<13>connected\n").await.unwrap();
    assert_eq!(next(&mut messages).await.message, "connected");
    server.stop().await;
    assert!(UdpSocket::bind(udp).await.is_ok());
    assert!(tokio::net::TcpListener::bind(tcp).await.is_ok());
    // The connection was closed by the server.
    let mut sink = [0u8; 16];
    let closed = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::io::AsyncReadExt::read(&mut open, &mut sink),
    )
    .await;
    assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))), "{closed:?}");

    let (server, udp, _tcp, _messages) = start().await;
    drop(server);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(UdpSocket::bind(udp).await.is_ok());

    // UDP only: no TCP address.
    let (server, _messages) = SyslogServer::start("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(server.tcp_addr(), None);
    server.stop().await;
}

#[tokio::test]
async fn bind_failures_are_explained() {
    let (_server, udp, tcp, _messages) = start().await;
    let error = SyslogServer::start(udp, None).await.unwrap_err();
    assert!(
        error.to_string().contains(&format!("UDP {udp}"))
            && error.to_string().contains("already using it"),
        "{error}"
    );
    let error = SyslogServer::start("127.0.0.1:0".parse().unwrap(), Some(tcp))
        .await
        .unwrap_err();
    assert!(error.to_string().contains(&format!("TCP {tcp}")), "{error}");

    // The standard port needs privileges; without them the error must say what to do.
    match SyslogServer::start("127.0.0.1:514".parse().unwrap(), None).await {
        Err(ProbeError::Permission(message)) => {
            assert!(
                message.contains("1024") && message.contains("CAP_NET_BIND_SERVICE"),
                "{message}"
            );
        }
        Ok((server, _messages)) => server.stop().await,
        Err(ProbeError::Io(_)) => {}
        Err(other) => panic!("unexpected error {other:?}"),
    }
}

/// Runs `logger` with the given arguments.
async fn logger(args: Vec<String>) {
    let output =
        tokio::task::spawn_blocking(move || Command::new("logger").args(&args).output().unwrap())
            .await
            .unwrap();
    assert!(
        output.status.success(),
        "logger failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn logger_args(port: u16, extra: &[&str], text: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-n",
        "127.0.0.1",
        "-P",
        &port.to_string(),
        "-t",
        "nocturne-test",
        "-p",
        "local7.notice",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend(extra.iter().map(|arg| (*arg).to_owned()));
    args.push(text.to_owned());
    args
}

#[tokio::test]
async fn interoperates_with_logger_over_udp() {
    if support::require_program("logger").is_none() {
        return;
    }
    let (_server, udp, _tcp, mut messages) = start().await;

    logger(logger_args(
        udp.port(),
        &["-d", "--rfc3164"],
        "BSD format over UDP",
    ))
    .await;
    let bsd = next(&mut messages).await;
    assert_eq!((bsd.facility, bsd.severity), (Some(23), Some(5)), "{bsd:?}");
    assert_eq!(bsd.app.as_deref(), Some("nocturne-test"), "{bsd:?}");
    assert_eq!(bsd.message, "BSD format over UDP");
    assert!(bsd.timestamp.is_some() && bsd.hostname.is_some(), "{bsd:?}");
    assert_eq!(bsd.msgid, None);

    logger(logger_args(
        udp.port(),
        &["-d", "--rfc5424", "--msgid", "NETPROBE", "--id=4242"],
        "IETF format over UDP",
    ))
    .await;
    let ietf = next(&mut messages).await;
    assert_eq!(
        (ietf.facility, ietf.severity),
        (Some(23), Some(5)),
        "{ietf:?}"
    );
    assert_eq!(ietf.app.as_deref(), Some("nocturne-test"));
    assert_eq!(ietf.procid.as_deref(), Some("4242"));
    assert_eq!(ietf.msgid.as_deref(), Some("NETPROBE"));
    assert_eq!(ietf.message, "IETF format over UDP");
    let timestamp = ietf.timestamp.as_deref().unwrap();
    assert!(
        timestamp.len() >= 20 && timestamp.as_bytes()[10] == b'T',
        "{timestamp}"
    );
    assert_eq!(
        ietf.hostname, bsd.hostname,
        "both formats name the same host"
    );

    // A message that itself looks like a Cisco line is still taken apart.
    logger(logger_args(
        udp.port(),
        &["-d", "--rfc5424"],
        "%LINEPROTO-5-UPDOWN: Line protocol on Interface Ethernet1, changed state to up",
    ))
    .await;
    let cisco = next(&mut messages).await;
    let mnemonic = cisco.mnemonic.expect("the mnemonic is recognised");
    assert_eq!(
        (
            mnemonic.facility.as_str(),
            mnemonic.severity,
            mnemonic.name.as_str()
        ),
        ("LINEPROTO", Some(5), "UPDOWN")
    );
    assert_eq!(
        cisco.message,
        "Line protocol on Interface Ethernet1, changed state to up"
    );
}

#[tokio::test]
async fn interoperates_with_logger_over_tcp() {
    if support::require_program("logger").is_none() {
        return;
    }
    let (_server, _udp, tcp, mut messages) = start().await;

    // logger's default TCP framing is newline-terminated.
    logger(logger_args(
        tcp.port(),
        &["-T", "--rfc3164"],
        "BSD format over TCP",
    ))
    .await;
    let bsd = next(&mut messages).await;
    assert_eq!(bsd.app.as_deref(), Some("nocturne-test"), "{bsd:?}");
    assert_eq!(bsd.message, "BSD format over TCP");
    assert_eq!(bsd.severity, Some(5));

    logger(logger_args(
        tcp.port(),
        &["-T", "--rfc5424"],
        "IETF format over TCP",
    ))
    .await;
    let ietf = next(&mut messages).await;
    assert_eq!(ietf.message, "IETF format over TCP");
    assert_eq!(ietf.app.as_deref(), Some("nocturne-test"));

    // RFC 6587 octet counting.
    logger(logger_args(
        tcp.port(),
        &["-T", "--octet-count", "--rfc5424", "--msgid", "COUNTED"],
        "octet-counted over TCP",
    ))
    .await;
    let counted = next(&mut messages).await;
    assert_eq!(counted.message, "octet-counted over TCP", "{counted:?}");
    assert_eq!(counted.msgid.as_deref(), Some("COUNTED"));
    logger(logger_args(
        tcp.port(),
        &["-T", "--octet-count", "--rfc3164"],
        "octet-counted BSD",
    ))
    .await;
    let counted = next(&mut messages).await;
    assert_eq!(counted.message, "octet-counted BSD", "{counted:?}");
    assert_eq!(counted.app.as_deref(), Some("nocturne-test"));
}
