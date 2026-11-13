//! Scaffolding for this crate's tests.
//!
//! Everything a test here probes, it starts itself: a TCP listener with a
//! banner, a small HTTP server, a DNS responder, an SNTP server, a WHOIS
//! server, and a TFTP client written by hand. Nothing binds a fixed port,
//! nothing needs the internet, and nothing needs root — the two probes that
//! cannot be guaranteed on an arbitrary machine (ICMP echo and Wake-on-LAN)
//! have tests that accept the honest refusal as well as the answer.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use nocturne_app::jobs::{self, Job};
use nocturne_app::testing::{self, Lab};
use nocturne_app::{App, AppBuilder, AppConfig, AppResult, Event};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::broadcast::{self, error::TryRecvError};

/// An app with the toolbox registered and `names` as fake devices.
pub async fn lab(names: &[&str]) -> Lab {
    let mut builder = testing::builder();
    crate::init(&mut builder);
    testing::lab(builder, names).await
}

/// An app that a browser talks to: callers are not at this machine.
pub fn server_mode() -> App {
    let mut builder =
        AppBuilder::new(AppConfig::in_memory().remote_users()).expect("an in-memory app");
    nocturne_app::init(&mut builder);
    crate::init(&mut builder);
    builder.build()
}

/// An app on a real data directory, so that a second process can be simulated.
pub fn on_disk(dir: &std::path::Path) -> App {
    let mut builder = AppBuilder::new(AppConfig::at(dir)).expect("an app on disk");
    nocturne_app::init(&mut builder);
    crate::init(&mut builder);
    builder.build()
}

/// Start a job-shaped probe and wait for the job to end.
pub async fn run_job(app: &App, method: &str, params: Value) -> AppResult<Job> {
    let started = app.rpc(method, params).await?;
    let job_id = started["job_id"]
        .as_str()
        .expect("a job-shaped method returns {job_id}")
        .to_string();
    let job = tokio::time::timeout(Duration::from_secs(20), jobs::wait(app, &job_id))
        .await
        .expect("the job finishes within 20 s")?;
    Ok(job)
}

/// A recorder of the events a test cares about.
pub struct Events {
    inner: broadcast::Receiver<Event>,
}

impl Events {
    pub fn new(app: &App) -> Events {
        Events {
            inner: app.subscribe(),
        }
    }

    /// Every event of `topic` that has arrived so far.
    pub fn drain(&mut self, topic: &str) -> Vec<Value> {
        let mut found = Vec::new();
        loop {
            match self.inner.try_recv() {
                Ok(event) if event.topic == topic => found.push(event.payload),
                Ok(_) => {}
                Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(_)) => continue,
            }
        }
        found
    }

    /// Wait for the next event of `topic`, or give up.
    pub async fn next(&mut self, topic: &str, within: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            match tokio::time::timeout(left, self.inner.recv()).await {
                Ok(Ok(event)) if event.topic == topic => return Some(event.payload),
                Ok(Ok(_)) => continue,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) | Err(_) => return None,
            }
        }
    }
}

/// A TCP listener that says `banner` and closes. Returns its port.
pub async fn banner_listener(banner: &'static str) -> u16 {
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

/// A TCP listener that accepts and hangs up without saying anything — which is
/// what a TLS handshake meets when it is pointed at something that is not TLS.
pub async fn silent_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    port
}

/// A port nothing is listening on (bound, read, released).
pub async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// The smallest HTTP/1.1 server that will do: one response, then close.
pub async fn http_server(response: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            // Read until the end of the headers, then answer.
            while let Ok(1) = stream.read(&mut byte).await {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });
    port
}

/// A WHOIS server: reads the query line, writes `answer`, closes.
pub async fn whois_server(answer: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut query = Vec::new();
            let mut byte = [0u8; 1];
            while let Ok(1) = stream.read(&mut byte).await {
                query.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });
    port
}

/// A DNS server that answers every `A` question with `address`.
///
/// Hand-built per RFC 1035: the question is echoed and one answer record
/// points back at it, which is all a resolver needs to accept the reply. The
/// question has to be measured rather than assumed, because the query carries
/// an EDNS record after it that must not be copied into the answer section.
pub async fn dns_responder(address: [u8; 4]) -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a socket");
    let port = socket.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buffer = [0u8; 1500];
        while let Ok((length, from)) = socket.recv_from(&mut buffer).await {
            let request = &buffer[..length];
            let Some(question_end) = question_end(request) else {
                continue;
            };
            let mut reply = Vec::with_capacity(question_end + 16);
            reply.extend_from_slice(&request[0..2]); // the query's id
            reply.extend_from_slice(&[0x81, 0x80]); // response, recursion available
            reply.extend_from_slice(&[0x00, 0x01]); // one question
            reply.extend_from_slice(&[0x00, 0x01]); // one answer
            reply.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // no authority, no additional
            reply.extend_from_slice(&request[12..question_end]); // the question, as asked
            reply.extend_from_slice(&[0xc0, 0x0c]); // the name: a pointer to it
            reply.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // A, IN
            reply.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]); // TTL 60
            reply.extend_from_slice(&[0x00, 0x04]); // four bytes of address
            reply.extend_from_slice(&address);
            let _ = socket.send_to(&reply, from).await;
        }
    });
    port
}

/// Where the question section of a DNS query ends: the labels, their
/// terminating zero, and the two-byte type and class.
fn question_end(request: &[u8]) -> Option<usize> {
    let mut at = 12;
    loop {
        let length = *request.get(at)? as usize;
        at += 1;
        if length == 0 {
            break;
        }
        at += length;
    }
    let end = at + 4;
    (end <= request.len()).then_some(end)
}

/// An SNTP server (RFC 4330): stratum 2, echoing the client's transmit
/// timestamp as the originate timestamp, which is what the client checks.
pub async fn ntp_responder() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a socket");
    let port = socket.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        while let Ok((length, from)) = socket.recv_from(&mut buffer).await {
            if length < 48 {
                continue;
            }
            let originate = &buffer[40..48];
            let mut reply = [0u8; 48];
            reply[0] = (4 << 3) | 4; // no leap warning, version 4, server
            reply[1] = 2; // stratum 2
            reply[2] = 4; // poll
            reply[3] = 0xEC; // precision 2^-20
            reply[4..8].copy_from_slice(&[0x00, 0x00, 0x10, 0x00]); // root delay 1/16 s
            reply[8..12].copy_from_slice(&[0x00, 0x00, 0x08, 0x00]); // root dispersion 1/32 s
            reply[12..16].copy_from_slice(&[10, 0, 0, 1]); // its upstream
            reply[16..24].copy_from_slice(originate); // reference time
            reply[24..32].copy_from_slice(originate); // originate: the client's own
            reply[32..40].copy_from_slice(originate); // receive
            reply[40..48].copy_from_slice(originate); // transmit
            let _ = socket.send_to(&reply, from).await;
        }
    });
    port
}

/// Send one syslog datagram to `address`.
pub async fn send_syslog(address: &str, message: &str) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a socket");
    socket
        .send_to(message.as_bytes(), address)
        .await
        .expect("the datagram goes out");
}

/// Fetch a file from a TFTP server: RRQ, then acknowledge each block.
///
/// A deliberately minimal client (RFC 1350, octet mode, 512-byte blocks) so
/// that the server is tested against something other than itself.
pub async fn tftp_get(server: SocketAddr, file: &str) -> Result<Vec<u8>, String> {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let mut request = vec![0x00, 0x01];
    request.extend_from_slice(file.as_bytes());
    request.push(0);
    request.extend_from_slice(b"octet");
    request.push(0);
    socket
        .send_to(&request, server)
        .await
        .map_err(|e| e.to_string())?;

    let mut contents = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let (length, from) =
            tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
                .await
                .map_err(|_| "the server did not answer".to_string())?
                .map_err(|e| e.to_string())?;
        let packet = &buffer[..length];
        match packet.first().copied().zip(packet.get(1).copied()) {
            Some((0, 3)) => {
                let block = u16::from_be_bytes([packet[2], packet[3]]);
                contents.extend_from_slice(&packet[4..]);
                let ack = [0x00, 0x04, packet[2], packet[3]];
                socket
                    .send_to(&ack, from)
                    .await
                    .map_err(|e| e.to_string())?;
                if length - 4 < 512 {
                    return Ok(contents);
                }
                let _ = block;
            }
            Some((0, 5)) => {
                let message = String::from_utf8_lossy(&packet[4..])
                    .trim_end_matches('\0')
                    .to_string();
                return Err(format!("error {}: {message}", packet[3]));
            }
            _ => return Err(format!("unexpected packet {packet:02x?}")),
        }
    }
}

/// Upload a file to a TFTP server: WRQ, then one data block.
pub async fn tftp_put(server: SocketAddr, file: &str, contents: &[u8]) -> Result<(), String> {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let mut request = vec![0x00, 0x02];
    request.extend_from_slice(file.as_bytes());
    request.push(0);
    request.extend_from_slice(b"octet");
    request.push(0);
    socket
        .send_to(&request, server)
        .await
        .map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 1024];
    let (length, from) =
        tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
            .await
            .map_err(|_| "the server did not answer".to_string())?
            .map_err(|e| e.to_string())?;
    let packet = &buffer[..length];
    if packet.get(1) == Some(&5) {
        let message = String::from_utf8_lossy(&packet[4..])
            .trim_end_matches('\0')
            .to_string();
        return Err(format!("error {}: {message}", packet[3]));
    }
    if packet.get(1) != Some(&4) {
        return Err(format!("unexpected packet {packet:02x?}"));
    }
    let mut data = vec![0x00, 0x03, 0x00, 0x01];
    data.extend_from_slice(contents);
    socket
        .send_to(&data, from)
        .await
        .map_err(|e| e.to_string())?;
    let (length, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
        .await
        .map_err(|_| "no final acknowledgement".to_string())?
        .map_err(|e| e.to_string())?;
    let packet = &buffer[..length];
    if packet.get(1) == Some(&5) {
        return Err(format!("error {}", packet[3]));
    }
    Ok(())
}

/// Add a device with an address, so that the inventory has something for a
/// probe to recognise.
pub async fn device_at(app: &App, name: &str, address: &str) -> String {
    let created = app
        .rpc(
            "devices.create",
            json!({"name": name, "address": address, "transport": "none", "role": "access", "site": "Lab"}),
        )
        .await
        .expect("the device is created");
    created["id"].as_str().expect("an id").to_string()
}
