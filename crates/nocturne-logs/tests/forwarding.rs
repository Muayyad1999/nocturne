//! Forwarders: what is stored is handed on to a log platform.
//!
//! Every collector here is a real listener on loopback — a UDP socket, a TCP
//! listener, a TLS listener and an HTTPS endpoint with a certificate made in
//! the test — and every line gets to it the way a device's line does: off the
//! syslog receiver's socket, through the store, onto the forwarder's queue.

mod support;

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nocturne_app::App;
use nocturne_app::testing::Lab;
use nocturne_logs::state::LogsState;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const LINK_DOWN: &str = "<187>81: *Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet0/1, changed state to down";
const CONFIGURED: &str = "<189>82: *Mar  1 00:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)";
/// A line that quotes a configuration command, community and all.
const WITH_SECRET: &str = "<189>83: *Mar  1 00:13:00.000: %PARSER-5-CFGLOG_LOGGEDCMD: User:admin  logged command:snmp-server community Str0ngR0 RO";
const INFORMATIONAL: &str =
    "<190>84: *Mar  1 00:14:00.000: %SYS-6-CLOCKUPDATE: System clock has been updated";
/// No priority and no mnemonic: nothing says how severe this is.
const NO_SEVERITY: &str = "the fan tray was reseated";

// ------------------------------------------------------------- collectors

/// A UDP collector: every datagram, as text.
async fn udp_collector() -> (u16, UnboundedReceiver<String>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a socket");
    let port = socket.local_addr().expect("an address").port();
    let (seen, received) = unbounded_channel();
    tokio::spawn(async move {
        let mut datagram = vec![0u8; 65_536];
        while let Ok((length, _)) = socket.recv_from(&mut datagram).await {
            if seen
                .send(String::from_utf8_lossy(&datagram[..length]).into_owned())
                .is_err()
            {
                break;
            }
        }
    });
    (port, received)
}

/// A collector that can be taken away again: aborting the task closes the
/// listener and every connection it accepted.
struct Collector {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Collector {
    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// A TCP collector on `port` (0 = any): every line of every connection.
async fn tcp_collector_on(port: u16) -> Option<(Collector, UnboundedReceiver<String>)> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.ok()?;
    let port = listener.local_addr().expect("an address").port();
    let (seen, received) = unbounded_channel();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        while let Ok((stream, _)) = listener.accept().await {
            let seen = seen.clone();
            connections.spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if seen.send(line).is_err() {
                        break;
                    }
                }
            });
        }
    });
    Some((Collector { port, task }, received))
}

async fn tcp_collector() -> (u16, UnboundedReceiver<String>) {
    let (collector, received) = tcp_collector_on(0).await.expect("a listener");
    (collector.port, received)
}

/// A certificate authority made for this test, and a server certificate it
/// signed for `localhost`.
struct Authority {
    ca_pem: String,
    server: Arc<rustls::ServerConfig>,
}

fn authority() -> Authority {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
        KeyPair, KeyUsagePurpose,
    };
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

    let ca_key = KeyPair::generate().expect("a key");
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("parameters");
    ca.distinguished_name
        .push(DnType::CommonName, "Nocturne forwarding test CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_certificate = ca.self_signed(&ca_key).expect("a CA certificate");

    let key = KeyPair::generate().expect("a key");
    let mut leaf = CertificateParams::new(vec!["localhost".to_string()]).expect("parameters");
    leaf.distinguished_name
        .push(DnType::CommonName, "localhost");
    leaf.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let certificate = leaf
        .signed_by(&key, &Issuer::from_params(&ca, &ca_key))
        .expect("a server certificate");

    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![certificate.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
    .expect("a server configuration");
    Authority {
        ca_pem: ca_certificate.pem(),
        server: Arc::new(server),
    }
}

/// A syslog-over-TLS collector: every octet-counted message (RFC 5425).
async fn tls_collector(authority: &Authority) -> (u16, UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    let acceptor = tokio_rustls::TlsAcceptor::from(authority.server.clone());
    let (seen, received) = unbounded_channel();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, seen) = (acceptor.clone(), seen.clone());
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buffer: Vec<u8> = Vec::new();
                let mut chunk = vec![0u8; 16 * 1024];
                while let Ok(count) = stream.read(&mut chunk).await {
                    if count == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..count]);
                    // `<length> <message>`, as many as are complete.
                    while let Some(space) = buffer.iter().position(|byte| *byte == b' ') {
                        let length: usize = std::str::from_utf8(&buffer[..space])
                            .ok()
                            .and_then(|digits| digits.parse().ok())
                            .expect("a frame begins with its length");
                        if buffer.len() < space + 1 + length {
                            break;
                        }
                        let message: Vec<u8> =
                            buffer.drain(..space + 1 + length).skip(space + 1).collect();
                        let _ = seen.send(String::from_utf8_lossy(&message).into_owned());
                    }
                }
            });
        }
    });
    (port, received)
}

/// One request an HTTPS collector received.
#[derive(Debug)]
struct Posted {
    /// The request target: path and query.
    target: String,
    authorization: Option<String>,
    content_type: Option<String>,
    body: String,
}

impl Posted {
    /// The envelopes of the request, one JSON object to a line.
    fn envelopes(&self) -> Vec<Value> {
        self.body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("each line is a JSON object"))
            .collect()
    }
}

/// Answer the requests of one connection, one after another, with whatever
/// `answer` holds at the time: a status and a body.
async fn answer_requests<S>(
    stream: S,
    seen: tokio::sync::mpsc::UnboundedSender<Posted>,
    answer: Arc<Mutex<(u16, String)>>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    loop {
        let mut request_line = String::new();
        match stream.read_line(&mut request_line).await {
            Ok(count) if count > 0 => {}
            _ => return,
        }
        let target = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        let mut headers = std::collections::HashMap::new();
        loop {
            let mut header = String::new();
            if stream.read_line(&mut header).await.unwrap_or(0) == 0 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let length: usize = headers
            .get("content-length")
            .and_then(|length| length.parse().ok())
            .unwrap_or_default();
        let mut body = vec![0u8; length];
        if stream.read_exact(&mut body).await.is_err() {
            return;
        }
        let _ = seen.send(Posted {
            target,
            authorization: headers.get("authorization").cloned(),
            content_type: headers.get("content-type").cloned(),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
        let (status, text) = answer.lock().unwrap().clone();
        let response = format!(
            "HTTP/1.1 {status} Answer\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{text}",
            text.len()
        );
        if stream
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .is_err()
            || stream.get_mut().flush().await.is_err()
        {
            return;
        }
    }
}

/// An HTTP event collector behind TLS. It answers every request with
/// whatever `answer` holds at the time: a status and a body.
async fn https_collector(
    authority: &Authority,
    answer: Arc<Mutex<(u16, String)>>,
) -> (Collector, UnboundedReceiver<Posted>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    let acceptor = tokio_rustls::TlsAcceptor::from(authority.server.clone());
    let (seen, received) = unbounded_channel();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, seen, answer) = (acceptor.clone(), seen.clone(), answer.clone());
            connections.spawn(async move {
                if let Ok(stream) = acceptor.accept(tcp).await {
                    answer_requests(stream, seen, answer).await;
                }
            });
        }
    });
    (Collector { port, task }, received)
}

/// The same collector without TLS, for an `http://` URL.
async fn http_collector(
    answer: Arc<Mutex<(u16, String)>>,
) -> (Collector, UnboundedReceiver<Posted>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    let (seen, received) = unbounded_channel();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        while let Ok((tcp, _)) = listener.accept().await {
            connections.spawn(answer_requests(tcp, seen.clone(), answer.clone()));
        }
    });
    (Collector { port, task }, received)
}

fn accepted() -> Arc<Mutex<(u16, String)>> {
    Arc::new(Mutex::new((
        200,
        r#"{"text":"Success","code":0}"#.to_string(),
    )))
}

// ---------------------------------------------------------------- helpers

/// An app with a simulated switch `SW1`, a real one `CORE1` at 127.0.0.1,
/// and the syslog receiver running. Returns CORE1's id and the UDP port.
async fn campus() -> (Lab, String, u16) {
    let lab = lab(&["SW1"]).await;
    let core = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "CORE1", "address": "127.0.0.1", "profile": "test_ios"}),
        )
        .await
        .expect("the device is created");
    let (udp, _tcp) = start_receiver(&lab.app).await;
    let id = core["id"].as_str().expect("an id").to_string();
    (lab, id, udp)
}

async fn save(app: &App, params: Value) -> Value {
    app.rpc("logs.forward.save", params)
        .await
        .expect("the forwarder is saved")
}

fn id_of(forwarder: &Value) -> String {
    forwarder["id"].as_str().expect("an id").to_string()
}

/// One forwarder's row of `logs.status`.
async fn status_of(app: &App, id: &str) -> Value {
    let status = app.rpc("logs.status", Value::Null).await.expect("status");
    status["forwarders"]
        .as_array()
        .expect("forwarders")
        .iter()
        .find(|forwarder| forwarder["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("no forwarder {id} in {status:#}"))
}

/// Wait, for as long as `seconds`, until the forwarder's status row says so.
async fn until(
    app: &App,
    id: &str,
    seconds: u64,
    what: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let row = status_of(app, id).await;
        if ready(&row) {
            return row;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for {what}; the forwarder was {row:#}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The next thing a collector received. Ten seconds is a collector that
/// got nothing, not a slow machine.
async fn next<T>(received: &mut UnboundedReceiver<T>, what: &str) -> T {
    tokio::time::timeout(Duration::from_secs(10), received.recv())
        .await
        .unwrap_or_else(|_| panic!("the collector never received {what}"))
        .expect("the collector is running")
}

/// What a collector received within `wait`, when the point is what it did not.
async fn within<T>(received: &mut UnboundedReceiver<T>, wait: Duration) -> Vec<T> {
    tokio::time::sleep(wait).await;
    let mut all = Vec::new();
    while let Ok(item) = received.try_recv() {
        all.push(item);
    }
    all
}

/// Longer than the half second the `event` and `audit` streams are read at.
const A_READ_OF_THE_RECORDS: Duration = Duration::from_millis(1_500);

fn document(line: &str) -> Value {
    serde_json::from_str(line).unwrap_or_else(|err| panic!("not a JSON document ({err}): {line}"))
}

// ------------------------------------------------------------------- UDP

#[tokio::test]
async fn a_udp_collector_gets_each_stored_line_as_rfc_5424_with_its_secret_already_gone() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = udp_collector().await;
    let forwarder = save(
        app,
        json!({"name": "SIEM", "kind": "syslog_udp", "host": "127.0.0.1", "port": collector, "streams": ["syslog"]}),
    )
    .await;
    assert_eq!(forwarder["format"], json!("rfc5424"));
    assert_eq!(
        status_of(app, &id_of(&forwarder)).await,
        json!({"id": forwarder["id"], "name": "SIEM", "state": "idle", "sent": 0, "dropped": 0, "queued": 0, "queued_bytes": 0, "max_queued_bytes": 16_777_216, "last_error": null, "last_at": null})
    );

    send_udp("127.0.0.1", port, &[LINK_DOWN, WITH_SECRET]).await;
    wait_until_settled(app, 2).await;
    let first = next(&mut received, "the first line").await;
    let second = next(&mut received, "the second line").await;

    // <local7.error>, version 1, the time it was stored, the device by its
    // inventory name, the mnemonic as the message id, where it came from.
    assert!(first.starts_with("<187>1 20"), "{first}");
    assert!(
        first.ends_with(
            "Z CORE1 - - LINK-3-UPDOWN [origin ip=\"127.0.0.1\"] Interface GigabitEthernet0/1, changed state to down"
        ),
        "{first}"
    );
    // What left is what is stored, and what is stored has no community in it.
    let stored = messages(app, json!({"stream": "syslog", "text": "CFGLOG"})).await;
    assert_eq!(stored.len(), 1);
    assert!(stored[0].contains("<secret:"), "{stored:?}");
    assert!(second.ends_with(&stored[0]), "{second}\n{stored:?}");
    assert!(!second.contains("Str0ngR0") && !first.contains("Str0ngR0"));

    let row = until(app, &id_of(&forwarder), 5, "two lines sent", |row| {
        row["sent"] == json!(2)
    })
    .await;
    assert_eq!(row["state"], json!("ok"));
    assert_eq!(row["dropped"], json!(0));
    assert_eq!(row["last_error"], Value::Null);
    assert!(row["last_at"].as_i64().is_some_and(|at| at > 0), "{row}");
}

// ------------------------------------------------------------------- TCP

#[tokio::test]
async fn a_tcp_collector_gets_one_json_document_to_a_line_and_it_is_the_stored_row() {
    let (lab, core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = tcp_collector().await;
    // Nothing said about streams: syslog, traps and events, not the audit trail.
    let forwarder = save(
        app,
        json!({"name": "Lake", "kind": "syslog_tcp", "host": "localhost", "port": collector, "format": "json"}),
    )
    .await;
    assert_eq!(forwarder["streams"], json!(["syslog", "trap", "event"]));

    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    wait_until_settled(app, 1).await;

    // The line itself, and the signal the built-in rule recorded for it in
    // the event stream.
    let mut line = None;
    let mut signal = None;
    while line.is_none() || signal.is_none() {
        let received = document(&next(&mut received, "the line and its signal").await);
        match received["stream"].as_str() {
            Some("syslog") => line = Some(received),
            Some("event") => signal = Some(received),
            other => panic!("a stream nobody asked for: {other:?}"),
        }
    }
    let (line, signal) = (line.unwrap(), signal.unwrap());

    let stored = &query(app, json!({"stream": "syslog"})).await[0];
    for field in [
        "id",
        "ts",
        "stream",
        "device_id",
        "severity",
        "source",
        "message",
        "body",
    ] {
        assert_eq!(line[field], stored[field], "{field}");
    }
    assert_eq!(line["device_id"], json!(core));
    assert_eq!(line["device"], json!("CORE1"));
    assert_eq!(line["severity"], json!(3));
    assert!(
        line["time"]
            .as_str()
            .is_some_and(|time| time.len() == 24 && time.ends_with('Z')),
        "{line}"
    );
    assert_eq!(signal["device"], json!("CORE1"));
    assert!(
        signal["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("logs.")),
        "{signal}"
    );

    // The audit trail was not asked for and did not leave, although this
    // test has written to it (the device, the settings, the forwarder).
    let rest = within(&mut received, A_READ_OF_THE_RECORDS).await;
    assert!(
        rest.iter()
            .all(|line| document(line)["stream"] != json!("audit")),
        "{rest:?}"
    );
    assert!(!query(app, json!({"stream": "audit"})).await.is_empty());
}

#[tokio::test]
async fn a_line_read_back_from_a_devices_own_buffer_is_handed_on_too() {
    let (lab, core, _port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = tcp_collector().await;
    save(
        app,
        json!({"name": "Lake", "kind": "syslog_tcp", "host": "127.0.0.1", "port": collector, "format": "json", "streams": ["syslog"]}),
    )
    .await;
    let pulled = nocturne_logs::pull::store(
        app,
        &core,
        "show logging",
        "Log Buffer (4096 bytes):\n\n*Mar  1 00:20:00.000: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)\n",
    )
    .expect("the buffer is stored");
    assert_eq!(pulled.stored, 1);
    let line = document(&next(&mut received, "the pulled line").await);
    assert_eq!(line["body"]["via"], json!("pull"));
    assert_eq!(line["device"], json!("CORE1"));
}

#[tokio::test]
async fn a_line_break_a_device_sent_starts_no_second_message_at_the_collector() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = tcp_collector().await;
    save(
        app,
        json!({"name": "SIEM", "kind": "syslog_tcp", "host": "127.0.0.1", "port": collector, "streams": ["syslog"]}),
    )
    .await;
    // One datagram, written to look like two messages to anything that
    // splits a stream on line ends — the second an emergency from the core.
    send_udp(
        "127.0.0.1",
        port,
        &["<190>90: *Mar  1 00:15:00.000: %SYS-6-CLOCKUPDATE: System clock has been updated\n<0>1 2026-01-01T00:00:00Z core1 forged - - - the core is on fire"],
    )
    .await;
    wait_until_settled(app, 1).await;
    let stored = query(app, json!({"stream": "syslog"})).await;
    assert_eq!(stored.len(), 1, "one datagram is one stored line");

    let first = next(&mut received, "the line").await;
    let rest = within(&mut received, Duration::from_millis(300)).await;
    assert!(
        rest.is_empty(),
        "one stored line is one message at the collector, not {first:?} and then {rest:?}"
    );
    assert!(first.starts_with("<190>1 20"), "{first}");
    // What the store kept of the second half travels inside the first.
    if stored[0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("on fire"))
    {
        assert!(first.ends_with("the core is on fire"), "{first}");
    }
}

// ------------------------------------------------------------------- TLS

#[tokio::test]
async fn over_tls_a_collector_is_sent_to_only_when_its_certificate_checks_out() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let authority = authority();
    let (vouched, mut received) = tls_collector(&authority).await;
    let (unknown, mut never) = tls_collector(&authority).await;
    let trusting = save(
        app,
        json!({"name": "Vouched for", "kind": "syslog_tls", "host": "localhost", "port": vouched, "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    // The same certificate, and nobody this machine trusts signed it.
    let wary = save(
        app,
        json!({"name": "Unknown", "kind": "syslog_tls", "host": "localhost", "port": unknown, "streams": ["syslog"]}),
    )
    .await;
    assert_eq!(trusting["ca_pem"], json!(authority.ca_pem.trim()));
    assert_eq!(wary["ca_pem"], Value::Null);

    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    wait_until_settled(app, 1).await;

    let message = next(&mut received, "the line over TLS").await;
    assert!(message.starts_with("<187>1 20"), "{message}");
    assert!(
        message.ends_with("LINK-3-UPDOWN [origin ip=\"127.0.0.1\"] Interface GigabitEthernet0/1, changed state to down"),
        "{message}"
    );
    until(app, &id_of(&trusting), 5, "the line sent", |row| {
        row["sent"] == json!(1) && row["state"] == json!("ok")
    })
    .await;

    let row = until(app, &id_of(&wary), 10, "the refusal", |row| {
        row["state"] == json!("failing")
    })
    .await;
    assert!(
        row["last_error"]
            .as_str()
            .is_some_and(|error| error.contains("TLS handshake")),
        "{row}"
    );
    assert_eq!(row["sent"], json!(0));
    assert!(
        within(&mut never, Duration::from_millis(300))
            .await
            .is_empty(),
        "nothing is sent to a collector whose certificate was refused"
    );
}

// ----------------------------------------------------------------- HTTPS

#[tokio::test]
async fn an_https_collector_gets_the_stored_rows_and_its_url_and_token_are_kept_nowhere() {
    const PATH: &str = "/services/collector/event/hidden-PATH-7731";
    const QUERY: &str = "channel=hidden-QUERY-18";
    const TOKEN: &str = "tok-SECRET-99aa";

    let logged = LogLines::capture();
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let mut events = app.subscribe();
    let authority = authority();
    let answer = accepted();
    let (collector, mut posted) = https_collector(&authority, answer.clone()).await;
    let url = format!("https://localhost:{}{PATH}?{QUERY}", collector.port);
    let mut said: Vec<String> = Vec::new();

    let forwarder = save(
        app,
        json!({"name": "Splunk", "kind": "https_json", "url": url, "token": TOKEN, "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    let id = id_of(&forwarder);
    assert_eq!(forwarder["has_secret"], json!(true));
    assert_eq!(forwarder["host"], json!(""));
    assert_eq!(forwarder["port"], json!(0));
    assert_eq!(forwarder["format"], json!("json"));
    assert!(forwarder.get("url").is_none() && forwarder.get("token").is_none());
    said.push(forwarder.to_string());

    send_udp("127.0.0.1", port, &[LINK_DOWN, WITH_SECRET]).await;
    wait_until_settled(app, 2).await;
    let mut envelopes = Vec::new();
    while envelopes.len() < 2 {
        let request = next(&mut posted, "a request").await;
        assert_eq!(request.target, format!("{PATH}?{QUERY}"));
        assert_eq!(
            request.authorization.as_deref(),
            Some("Splunk tok-SECRET-99aa")
        );
        assert_eq!(request.content_type.as_deref(), Some("application/json"));
        envelopes.extend(request.envelopes());
    }
    let stored = query(app, json!({"stream": "syslog"})).await;
    // Newest first in the query; oldest first on the wire.
    for (envelope, stored) in envelopes.iter().zip(stored.iter().rev()) {
        assert_eq!(envelope["host"], json!("CORE1"));
        assert_eq!(envelope["source"], json!("nocturne"));
        assert_eq!(envelope["sourcetype"], json!("nocturne:syslog"));
        assert_eq!(
            envelope["time"]
                .as_f64()
                .map(|time| (time * 1000.0).round() as i64),
            stored["ts"].as_i64()
        );
        assert_eq!(envelope["event"]["message"], stored["message"]);
        assert_eq!(envelope["event"]["body"], stored["body"]);
    }
    assert!(
        !envelopes
            .iter()
            .any(|envelope| envelope.to_string().contains("Str0ngR0"))
    );
    until(app, &id, 5, "both sent", |row| {
        row["sent"] == json!(2) && row["state"] == json!("ok")
    })
    .await;

    // A collector that refuses, and repeats the token and the URL back in
    // its answer the way an error page does.
    *answer.lock().unwrap() = (
        403,
        json!({"text": format!("Invalid token {TOKEN} for {url}"), "code": 4}).to_string(),
    );
    send_udp("127.0.0.1", port, &[CONFIGURED]).await;
    let row = until(app, &id, 10, "the refusal", |row| {
        row["state"] == json!("failing")
    })
    .await;
    assert_eq!(
        row["last_error"],
        json!("the collector answered HTTP 403 (its own error code: 4)")
    );
    let refused = app
        .rpc("logs.forward.test", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "unavailable");
    assert_eq!(
        refused.message,
        "the collector answered HTTP 403 (its own error code: 4)"
    );
    said.push(refused.to_string());

    // And one that is not there any more.
    let gone = collector.port;
    collector.stop().await;
    let unreachable = app
        .rpc("logs.forward.test", json!({"id": id}))
        .await
        .unwrap_err();
    assert_eq!(unreachable.code, "unavailable");
    assert!(
        unreachable.message.contains("refused the connection")
            || unreachable.message.contains("could not be reached"),
        "{unreachable}"
    );
    said.push(unreachable.to_string());
    // Another line, so that the forwarder has something to try whether or
    // not it has given up on the one the collector refused.
    send_udp("127.0.0.1", port, &[INFORMATIONAL]).await;
    let row = until(app, &id, 20, "the forwarder to notice", |row| {
        row["last_error"]
            .as_str()
            .is_some_and(|error| !error.contains("HTTP 403"))
    })
    .await;
    said.push(row.to_string());

    // A URL that is refused when it is saved is not repeated either.
    let invalid = app
        .rpc(
            "logs.forward.save",
            json!({"id": id, "name": "Splunk", "kind": "https_json", "url": format!("ftp://localhost:{gone}{PATH}"), "token": TOKEN}),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code, "invalid");
    said.push(invalid.to_string());
    let invalid = app
        .rpc(
            "logs.forward.save",
            json!({"id": id, "name": "Splunk", "kind": "https_json", "token": 7_731_993}),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code, "invalid");
    assert!(!invalid.message.contains("7731993"), "{invalid}");
    said.push(invalid.to_string());

    // Everywhere a value can get to.
    tokio::time::sleep(A_READ_OF_THE_RECORDS).await;
    let rpc = |method: &'static str, params: Value| async move {
        app.rpc(method, params)
            .await
            .unwrap_or_else(|err| panic!("{method}: {err}"))
            .to_string()
    };
    let places: Vec<(&str, String)> = vec![
        ("the database, every table", everything_stored(app)),
        (
            "the events on the bus",
            drain_all(&mut events)
                .iter()
                .map(|(topic, payload)| format!("{topic} {payload}\n"))
                .collect(),
        ),
        (
            "logs.forward.list",
            rpc("logs.forward.list", Value::Null).await,
        ),
        ("logs.status", rpc("logs.status", Value::Null).await),
        (
            "the audit trail",
            rpc("logs.query", json!({"stream": "audit"})).await,
        ),
        (
            "the event stream",
            rpc("logs.query", json!({"stream": "event"})).await,
        ),
        (
            "the settings",
            rpc("settings.get", json!({"section": "logs"})).await,
        ),
        ("results and errors", said.join("\n")),
        ("the log lines", logged.text()),
    ];
    let secrets = [
        "hidden-PATH-7731".to_string(),
        "hidden-QUERY-18".to_string(),
        TOKEN.to_string(),
        format!("localhost:{gone}"),
    ];
    for (place, text) in &places {
        assert!(
            !text.is_empty() || *place == "the log lines",
            "{place} is empty"
        );
        for secret in &secrets {
            assert!(
                !text.contains(secret.as_str()),
                "`{secret}` is in {place}:\n{text}"
            );
        }
    }
    // The search is of something: the forwarder itself is in those places.
    assert!(places[0].1.contains("logs_forwarders") && places[2].1.contains("Splunk"));
    assert!(places[4].1.contains("logs.forward.save") && places[4].1.contains("secret_changed"));
}

#[tokio::test]
async fn an_https_collector_whose_certificate_does_not_check_out_is_sent_nothing_and_told_apart_from_one_that_is_gone()
 {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let authority = authority();
    let somebody_else = self::authority();
    let (collector, mut posted) = https_collector(&authority, accepted()).await;
    let url = format!(
        "https://localhost:{}/services/collector/event",
        collector.port
    );
    // Nobody this machine trusts signed the collector's certificate.
    let wary = save(
        app,
        json!({"name": "Unknown", "kind": "https_json", "url": url, "token": "tok-1", "streams": ["syslog"]}),
    )
    .await;
    // And an authority was given that did not sign it either.
    let misled = save(
        app,
        json!({"name": "Wrong authority", "kind": "https_json", "url": url, "token": "tok-1", "ca_pem": somebody_else.ca_pem, "streams": ["syslog"]}),
    )
    .await;

    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    wait_until_settled(app, 1).await;
    for forwarder in [&wary, &misled] {
        let id = id_of(forwarder);
        let row = until(app, &id, 20, "the refusal", |row| {
            row["state"] == json!("failing")
        })
        .await;
        let said = row["last_error"].as_str().expect("an error");
        // The collector is there and answered: what is wrong is its
        // certificate, and "could not be reached" would send a person to
        // look at the network.
        assert!(
            said.starts_with("the TLS handshake with the collector failed: its certificate"),
            "{row}"
        );
        assert!(
            !said.contains("localhost") && !said.contains("collector/event"),
            "{row}"
        );
        let tested = app
            .rpc("logs.forward.test", json!({"id": id}))
            .await
            .unwrap_err();
        assert_eq!(tested.code, "unavailable");
        assert!(
            tested
                .message
                .starts_with("the TLS handshake with the collector failed: its certificate"),
            "{tested}"
        );
        assert_eq!(row["sent"], json!(0));
    }
    assert!(
        within(&mut posted, Duration::from_millis(300))
            .await
            .is_empty(),
        "no request — no log text and no token — goes to a collector whose certificate was refused"
    );

    // One that is not there is said to be that.
    collector.stop().await;
    let gone = app
        .rpc("logs.forward.test", json!({"id": id_of(&wary)}))
        .await
        .unwrap_err();
    assert!(
        gone.message.contains("refused the connection")
            || gone.message.contains("could not be reached"),
        "{gone}"
    );
}

#[tokio::test]
async fn a_token_given_for_one_url_is_not_sent_to_another() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let authority = authority();
    let (first, mut at_first) = https_collector(&authority, accepted()).await;
    let (second, mut at_second) = https_collector(&authority, accepted()).await;
    let first_url = format!("https://localhost:{}/services/collector/event", first.port);
    let second_url = format!("https://localhost:{}/elsewhere", second.port);
    let forwarder = save(
        app,
        json!({"name": "Splunk", "kind": "https_json", "url": first_url, "token": "Bearer first-token", "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    let id = id_of(&forwarder);
    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    // A token written with its own scheme is sent as written.
    assert_eq!(
        next(&mut at_first, "the first request")
            .await
            .authorization
            .as_deref(),
        Some("Bearer first-token")
    );

    // Renamed, nothing said about the destination: it stays, token and all.
    save(
        app,
        json!({"id": id, "name": "Splunk (prod)", "kind": "https_json", "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    send_udp("127.0.0.1", port, &[CONFIGURED]).await;
    let request = next(&mut at_first, "the request after the rename").await;
    assert_eq!(request.authorization.as_deref(), Some("Bearer first-token"));

    // Pointed somewhere else without a token: the old one does not follow.
    let moved = save(
        app,
        json!({"id": id, "name": "Splunk (prod)", "kind": "https_json", "url": second_url, "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    assert_eq!(moved["has_secret"], json!(true));
    send_udp("127.0.0.1", port, &[INFORMATIONAL]).await;
    let request = next(&mut at_second, "the request at the new URL").await;
    assert_eq!(request.target, "/elsewhere");
    assert_eq!(request.authorization, None, "{request:?}");
    assert!(
        within(&mut at_first, Duration::from_millis(200))
            .await
            .is_empty(),
        "the old destination gets nothing more"
    );

    // The same URL again, with nothing else: what is there stays as it is.
    save(
        app,
        json!({"id": id, "name": "Splunk (prod)", "kind": "https_json", "url": second_url, "token": "second-token", "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    save(
        app,
        json!({"id": id, "name": "Splunk (prod)", "kind": "https_json", "url": second_url, "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    let request = next(&mut at_second, "the request with the second token").await;
    assert_eq!(
        request.authorization.as_deref(),
        Some("Splunk second-token")
    );
}

// ------------------------------------------------------- the audit trail

#[tokio::test]
async fn a_batch_a_forwarder_is_trying_again_still_counts_against_its_queue() {
    let (lab, _core, _port) = campus().await;
    let app = &lab.app;
    // Nothing listens there: every attempt is refused at once, and the
    // forwarder waits a second, then two, before the next — holding the batch.
    let gone = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let forwarder = save(
        app,
        json!({"name": "Gone", "kind": "syslog_tcp", "host": "127.0.0.1", "port": gone, "streams": ["syslog"]}),
    )
    .await;
    let id = id_of(&forwarder);
    let index = nocturne_logs::attribute::refresh(app, nocturne_app::util::now_ms(), 0).unwrap();
    let rows: Vec<nocturne_app::store::LogEntry> = (0..20)
        .map(|n| nocturne_app::store::LogEntry {
            id: 1_000 + n,
            ts: nocturne_app::util::now_ms(),
            stream: "syslog".into(),
            device_id: None,
            severity: Some(5),
            source: "127.0.0.1".into(),
            message: "x".repeat(1_000),
            body: None,
        })
        .collect();
    // All twenty before the worker runs (this runtime has one thread), so it
    // takes them as one batch.
    nocturne_logs::forward::offer(app, &index, rows.iter());
    let row = until(app, &id, 10, "the first attempt to fail", |row| {
        row["state"] == json!("failing")
    })
    .await;
    assert_eq!(row["queued"], json!(0), "the queue is empty: {row:#}");
    assert!(
        row["queued_bytes"].as_u64().unwrap_or_default() >= 20 * 1_000,
        "the batch it is trying again is memory as much as the queue is, and counts with it \
         until it is sent or dropped: {row:#}"
    );
    let row = until(
        app,
        &id,
        30,
        "the batch to be dropped after its last attempt",
        |row| row["dropped"] == json!(20),
    )
    .await;
    assert_eq!(
        row["queued_bytes"],
        json!(0),
        "and then given back: {row:#}"
    );
}

#[tokio::test]
async fn a_token_is_not_sent_where_anyone_on_the_way_can_read_it() {
    const TOKEN: &str = "tok-CLEAR-4411";
    const PASSWORD: &str = "pw-CLEAR-5522";
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut posted) = http_collector(accepted()).await;
    let plain = format!("http://127.0.0.1:{}/services/collector", collector.port);
    let with_password = format!(
        "http://admin:{PASSWORD}@127.0.0.1:{}/services/collector",
        collector.port
    );

    // Over http:// a token, or a password in the URL, crosses the network
    // as text anybody on the way can read.
    for params in [
        json!({"name": "Clear", "kind": "https_json", "url": plain, "token": TOKEN}),
        json!({"name": "Clear", "kind": "https_json", "url": with_password}),
    ] {
        let refused = app
            .rpc("logs.forward.save", params)
            .await
            .expect_err("refused");
        assert_eq!(refused.code, "invalid", "{refused}");
        assert!(
            refused.message.contains("https://"),
            "it says what would do: {refused}"
        );
        assert!(
            !refused.message.contains(TOKEN) && !refused.message.contains(PASSWORD),
            "{refused}"
        );
    }
    let stored = everything_stored(app);
    assert!(
        !stored.contains(TOKEN) && !stored.contains(PASSWORD),
        "nothing of either is kept"
    );
    assert_eq!(
        app.rpc("logs.forward.list", Value::Null).await.unwrap(),
        json!([])
    );

    // Without one, http:// is what it was: a collector on one's own network.
    let forwarder = save(
        app,
        json!({"name": "Clear", "kind": "https_json", "url": plain, "streams": ["syslog"]}),
    )
    .await;
    let id = id_of(&forwarder);
    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    let request = next(&mut posted, "the line over http://").await;
    assert_eq!(request.authorization, None);

    // A token added afterwards, to the URL as stored, is refused as well.
    let refused = app
        .rpc(
            "logs.forward.save",
            json!({"id": id, "name": "Clear", "kind": "https_json", "token": TOKEN, "streams": ["syslog"]}),
        )
        .await
        .expect_err("refused");
    assert_eq!(refused.code, "invalid", "{refused}");
    assert!(!everything_stored(app).contains(TOKEN));
}

#[tokio::test]
async fn a_token_an_earlier_version_kept_for_an_http_url_is_not_sent_either() {
    use nocturne_logs::model::{ForwardFormat, ForwardKind, Forwarder};
    const TOKEN: &str = "tok-OLDER-7733";
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut posted) = http_collector(accepted()).await;
    // Stored the way a version that took a token for an http:// URL stored
    // it, which `logs.forward.save` now refuses to.
    let id = "older1";
    let forwarder = Forwarder {
        id: id.into(),
        name: "Older".into(),
        enabled: true,
        kind: ForwardKind::HttpsJson,
        format: ForwardFormat::Json,
        streams: vec!["syslog".into()],
        has_secret: true,
        ..Forwarder::default()
    };
    let secret = json!({"url": format!("http://127.0.0.1:{}/services/collector", collector.port), "token": TOKEN});
    app.vault()
        .put(&nocturne_logs::forward::secret_id(id), &secret.to_string())
        .unwrap();
    app.store()
        .put(nocturne_logs::forward::COLLECTION, id, &forwarder)
        .unwrap();
    nocturne_logs::forward::apply(app).unwrap();

    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    let row = until(
        app,
        id,
        10,
        "the forwarder to say why it sent nothing",
        |row| row["state"] == json!("failing"),
    )
    .await;
    let error = row["last_error"].as_str().unwrap_or_default();
    assert!(error.contains("https://"), "{row:#}");
    assert!(!row.to_string().contains(TOKEN), "{row:#}");
    assert!(
        within(&mut posted, Duration::from_millis(500))
            .await
            .is_empty(),
        "the collector was sent nothing, and so the token went nowhere"
    );
}

/// Set in the environment of the process
/// `a_forwarder_goes_straight_to_its_collector_whatever_proxy_the_environment_names`
/// starts this binary in.
const BEHIND_A_PROXY: &str = "NOCTURNE_TEST_BEHIND_A_PROXY";

// The environment is the process's, so the proxy variables are set for a
// process of their own: this binary, started again to run `behind_a_proxy`
// alone. This one plays the proxy, and notes who comes to it.
#[test]
fn a_forwarder_goes_straight_to_its_collector_whatever_proxy_the_environment_names() {
    use std::io::Read;
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("a listener");
    proxy
        .set_nonblocking(true)
        .expect("a listener that does not wait");
    let address = format!("http://{}", proxy.local_addr().expect("an address"));
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let printed = scratch.path().join("output");
    let output = std::fs::File::create(&printed).expect("a file for the output");
    let mut child = std::process::Command::new(std::env::current_exe().expect("this binary"))
        .args([
            "--exact",
            "behind_a_proxy",
            "--include-ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(BEHIND_A_PROXY, "1")
        .env("HTTP_PROXY", &address)
        .env("http_proxy", &address)
        .env("HTTPS_PROXY", &address)
        .env("https_proxy", &address)
        .env("ALL_PROXY", &address)
        .env("all_proxy", &address)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdout(output.try_clone().expect("the file, twice"))
        .stderr(output)
        .spawn()
        .expect("this binary starts again");
    let mut asked = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        while let Ok((mut stream, _)) = proxy.accept() {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let mut first = [0u8; 512];
            let read = stream.read(&mut first).unwrap_or(0);
            asked.push(
                String::from_utf8_lossy(&first[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        if let Some(status) = child.try_wait().expect("the child can be waited for") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the forwarders behind the proxy never finished");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        asked.is_empty(),
        "a forwarder went to a proxy nobody configured for it, and told it where it was going: {asked:?}"
    );
    assert!(
        status.success(),
        "the forwarders behind the proxy did not deliver ({status}):\n{}",
        std::fs::read_to_string(&printed).unwrap_or_default()
    );
}

#[tokio::test]
#[ignore = "started by a_forwarder_goes_straight_to_its_collector_whatever_proxy_the_environment_names, in a process whose environment names a proxy"]
async fn behind_a_proxy() {
    if std::env::var_os(BEHIND_A_PROXY).is_none() {
        return;
    }
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let authority = authority();
    let (secure, mut over_tls) = https_collector(&authority, accepted()).await;
    let (plain, mut over_http) = http_collector(accepted()).await;
    save(
        app,
        json!({"name": "Secure", "kind": "https_json", "url": format!("https://localhost:{}/services/collector/hidden-PATH-1", secure.port), "token": "tok-PROXY-61", "ca_pem": authority.ca_pem, "streams": ["syslog"]}),
    )
    .await;
    save(
        app,
        json!({"name": "Plain", "kind": "https_json", "url": format!("http://127.0.0.1:{}/services/collector/hidden-PATH-2", plain.port), "streams": ["syslog"]}),
    )
    .await;
    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    let request = next(&mut over_tls, "the line, over HTTPS").await;
    assert_eq!(request.target, "/services/collector/hidden-PATH-1");
    let request = next(&mut over_http, "the line, over HTTP").await;
    assert_eq!(request.target, "/services/collector/hidden-PATH-2");
}

#[tokio::test]
async fn the_audit_trail_leaves_only_through_a_forwarder_that_names_it() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (lines_port, mut lines) = tcp_collector().await;
    let (audit_port, mut audit) = tcp_collector().await;
    let plain = save(
        app,
        json!({"name": "Lines", "kind": "syslog_tcp", "host": "127.0.0.1", "port": lines_port, "format": "json"}),
    )
    .await;
    let auditing = save(
        app,
        json!({"name": "Compliance", "kind": "syslog_tcp", "host": "127.0.0.1", "port": audit_port, "format": "json", "streams": ["audit"]}),
    )
    .await;

    // Something a person did, and something a device said.
    app.rpc(
        "devices.create",
        json!({"name": "EDGE9", "address": "192.0.2.99", "profile": "test_ios"}),
    )
    .await
    .unwrap();
    send_udp("127.0.0.1", port, &[CONFIGURED]).await;
    wait_until_settled(app, 1).await;

    // The forwarder that names the audit trail gets it — starting with the
    // entry for its own creation, which says what it does — and nothing else.
    let mut actions = Vec::new();
    while !actions.iter().any(|action| action == "device.create") {
        let entry = document(&next(&mut audit, "the audit entries").await);
        assert_eq!(entry["stream"], json!("audit"), "{entry}");
        actions.push(entry["source"].as_str().unwrap_or_default().to_string());
        if entry["source"] == json!("logs.forward.save") {
            assert_eq!(entry["body"]["id"], auditing["id"]);
            assert_eq!(entry["body"]["audit"], json!(true));
            assert!(
                entry["message"].as_str().is_some_and(
                    |message| message.contains("sends the audit trail off this machine")
                ),
                "{entry}"
            );
        }
    }
    assert_eq!(actions[0], "logs.forward.save", "{actions:?}");

    // The other one gets the line, and never an audit entry.
    let mut seen = vec![document(&next(&mut lines, "the line").await)];
    seen.extend(
        within(&mut lines, A_READ_OF_THE_RECORDS)
            .await
            .iter()
            .map(|line| document(line)),
    );
    assert!(seen.iter().any(|entry| entry["stream"] == json!("syslog")));
    assert!(
        seen.iter().all(|entry| entry["stream"] != json!("audit")),
        "{seen:?}"
    );
    let rest = within(&mut audit, Duration::from_millis(100)).await;
    assert!(
        rest.iter()
            .all(|line| document(line)["stream"] == json!("audit")),
        "{rest:?}"
    );

    // What the audit trail says about the first forwarder: no audit.
    let trail = query(app, json!({"stream": "audit", "text": "Lines"})).await;
    assert_eq!(trail.len(), 1, "{trail:?}");
    assert_eq!(trail[0]["source"], json!("logs.forward.save"));
    assert_eq!(
        trail[0]["message"],
        json!("Saved the log forwarder `Lines`")
    );
    assert_eq!(
        trail[0]["body"],
        json!({"id": plain["id"], "kind": "syslog_tcp", "enabled": true, "format": "json",
               "streams": ["syslog", "trap", "event"], "audit": false, "lab": false,
               "host": "127.0.0.1", "port": lines_port})
    );

    // Taking `audit` off again stops it.
    save(
        app,
        json!({"id": auditing["id"], "name": "Compliance", "kind": "syslog_tcp", "host": "127.0.0.1", "port": audit_port, "format": "json", "streams": ["syslog"]}),
    )
    .await;
    within(&mut audit, Duration::from_millis(100)).await;
    app.rpc("devices.delete", json!({"ids": [lab.id("SW1")]}))
        .await
        .unwrap();
    let after = within(&mut audit, A_READ_OF_THE_RECORDS).await;
    assert!(after.is_empty(), "{after:?}");
}

// ------------------------------------------------------------------ labs

#[tokio::test]
async fn what_a_lab_writes_stays_here_unless_the_forwarder_asks_for_it() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (real_port, mut real) = tcp_collector().await;
    let (lab_port, mut with_lab) = tcp_collector().await;
    save(
        app,
        json!({"name": "SIEM", "kind": "syslog_tcp", "host": "127.0.0.1", "port": real_port, "format": "json"}),
    )
    .await;
    save(
        app,
        json!({"name": "Test collector", "kind": "syslog_tcp", "host": "127.0.0.1", "port": lab_port, "format": "json", "lab": true}),
    )
    .await;

    // A port going down on the simulated switch, then on the real one.
    let stored = app
        .rpc(
            "logs.ingest",
            json!({"device_id": lab.id("SW1"), "lines": [LINK_DOWN.trim_start_matches("<187>81: ")]}),
        )
        .await
        .unwrap();
    assert_eq!(stored, json!({"stored": 1}));
    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    wait_until_settled(app, 1).await;

    // The forwarder that asked gets both devices' lines and their signals.
    let mut devices = std::collections::BTreeSet::new();
    while devices.len() < 4 {
        let entry = document(&next(&mut with_lab, "the lab's and the real lines").await);
        devices.insert(format!(
            "{} {}",
            entry["device"].as_str().unwrap_or_default(),
            entry["stream"].as_str().unwrap_or_default()
        ));
    }
    assert_eq!(
        devices.into_iter().collect::<Vec<_>>(),
        vec!["CORE1 event", "CORE1 syslog", "SW1 event", "SW1 syslog"]
    );

    // The one that did not ask never hears of SW1: not its line, and not
    // the signal recorded about it.
    let mut seen = vec![document(&next(&mut real, "the real line").await)];
    seen.extend(
        within(&mut real, A_READ_OF_THE_RECORDS)
            .await
            .iter()
            .map(|line| document(line)),
    );
    assert!(seen.iter().any(|entry| entry["device"] == json!("CORE1")));
    assert!(
        seen.iter()
            .all(|entry| entry["device"] != json!("SW1")
                && !entry.to_string().contains(&lab.id("SW1"))),
        "{seen:?}"
    );
}

// -------------------------------------------------------------- severity

#[tokio::test]
async fn a_severity_floor_holds_back_what_is_less_severe_and_nothing_that_has_no_severity() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = tcp_collector().await;
    save(
        app,
        json!({"name": "Warnings", "kind": "syslog_tcp", "host": "127.0.0.1", "port": collector, "format": "json", "streams": ["syslog", "audit"], "min_severity": 4}),
    )
    .await;
    send_udp(
        "127.0.0.1",
        port,
        &[INFORMATIONAL, CONFIGURED, LINK_DOWN, NO_SEVERITY],
    )
    .await;
    wait_until_settled(app, 4).await;

    let mut lines = Vec::new();
    let mut audit = Vec::new();
    while lines.len() < 2 || audit.is_empty() {
        let entry = document(
            &next(
                &mut received,
                "the error, the line without a severity and the audit entry",
            )
            .await,
        );
        match entry["stream"].as_str() {
            Some("syslog") => lines.push(entry),
            Some("audit") => audit.push(entry),
            other => panic!("a stream nobody asked for: {other:?}"),
        }
    }
    for entry in within(&mut received, A_READ_OF_THE_RECORDS).await {
        let entry = document(&entry);
        if entry["stream"] == json!("syslog") {
            lines.push(entry);
        }
    }
    // The informational line and the notice stayed; the error and the
    // line that says nothing about its severity went.
    let severities: Vec<&Value> = lines.iter().map(|entry| &entry["severity"]).collect();
    assert_eq!(severities, vec![&json!(3), &Value::Null], "{lines:?}");
    assert_eq!(lines[1]["message"], json!(NO_SEVERITY));
    // The audit trail is not thinned by a floor: every entry of it is
    // written as informational, and it was asked for by name.
    assert_eq!(audit[0]["source"], json!("logs.forward.save"));
    assert_eq!(audit[0]["severity"], json!(6));
}

// ------------------------------------------------- slow and dead collectors

/// Lines of about a kilobyte, parsed as the receiver parses them: sixteen
/// thousand of them are several times what a socket and a forwarder's queue
/// hold between them.
fn bulk(count: usize, round: usize) -> Vec<nocturne_netprobe::syslog::SyslogMessage> {
    let source = "127.0.0.1:514".parse().expect("an address");
    (0..count)
        .map(|n| {
            // A message no rule looks at: what is timed is storing a line,
            // not raising a signal for it.
            let line = format!(
                "<190>{n}: *Mar  1 00:00:00.000: %SYS-6-CLOCKUPDATE: System clock has been updated, round {round} line {n} {}",
                "padding ".repeat(120)
            );
            nocturne_netprobe::syslog::parse(line.as_bytes(), source)
        })
        .collect()
}

// On threads of its own, as in the application: the forwarder's task is
// stuck in a write while this thread stores lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collector_that_stops_reading_costs_ingest_nothing() {
    const ROUNDS: usize = 32;
    const LINES: usize = 500;

    // Two applications alike in everything but the forwarder.
    let quiet = lab(&[]).await;
    let forwarding = lab(&[]).await;
    for app in [&quiet.app, &forwarding.app] {
        app.rpc(
            "devices.create",
            json!({"name": "CORE1", "address": "127.0.0.1", "profile": "test_ios"}),
        )
        .await
        .unwrap();
    }
    // A collector that takes one connection, never reads a byte from it and
    // takes no other. Its receive buffer is made small, or the operating
    // system would read tens of megabytes on its behalf; and it stops
    // listening, because every new connection would be a few more megabytes
    // of buffer for the forwarder to fill, as fast as this machine is slow.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(8 * 1024).unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(1).unwrap();
    let port = listener.local_addr().unwrap().port();
    let held = tokio::spawn(async move {
        let connection = listener.accept().await;
        drop(listener);
        std::future::pending::<()>().await;
        drop(connection);
    });
    let forwarder = save(
        &forwarding.app,
        json!({"name": "Stalled", "kind": "syslog_tcp", "host": "127.0.0.1", "port": port, "format": "json", "streams": ["syslog"]}),
    )
    .await;
    let id = id_of(&forwarder);
    let state = forwarding.app.state::<LogsState>();
    let running = state.forward.running()[0].clone();

    // Everything the forwarder says is wrong, while it happens: the stall
    // is what it says first, and a refused reconnection soon replaces it.
    let said = Arc::new(Mutex::new(std::collections::BTreeSet::new()));
    let watcher = {
        let (running, said) = (running.clone(), said.clone());
        tokio::spawn(async move {
            loop {
                if let Some(error) = running.counters.last_error.lock().clone() {
                    said.lock().unwrap().insert(error);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };

    let mut without = Duration::ZERO;
    let mut with = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    let mut slowest_without = Duration::ZERO;
    let mut most_waiting = 0;
    for round in 0..ROUNDS {
        let lines = bulk(LINES, round);
        // Alternating, so that whatever else this machine is doing falls on
        // both alike.
        let started = Instant::now();
        nocturne_logs::ingest::write_batch(&quiet.app, &lines, "syslog").unwrap();
        without += started.elapsed();
        slowest_without = slowest_without.max(started.elapsed());
        let started = Instant::now();
        nocturne_logs::ingest::write_batch(&forwarding.app, &lines, "syslog").unwrap();
        let took = started.elapsed();
        with += took;
        slowest = slowest.max(took);
        most_waiting = most_waiting.max(running.queued());
    }
    let offered = (ROUNDS * LINES) as u64;
    println!(
        "ingest of {offered} lines: {without:?} with no forwarder (slowest batch of {LINES}: {slowest_without:?}), {with:?} with a collector that has stopped reading (slowest batch: {slowest:?})"
    );

    // Every line is stored in both, whatever became of the forwarding.
    for app in [&quiet.app, &forwarding.app] {
        let status = app.rpc("logs.status", Value::Null).await.unwrap();
        assert_eq!(status["syslog"]["stored"], json!(offered));
    }
    // The queue never grew past its limit, and what did not fit was counted.
    assert!(
        most_waiting <= nocturne_logs::forward::QUEUE_LIMIT,
        "{most_waiting}"
    );
    use std::sync::atomic::Ordering;
    let accounted = || {
        // Waiting first: an entry the task takes in between is then counted
        // as in its hand rather than twice.
        let waiting = running.queued() as u64;
        let sent = running.counters.sent.load(Ordering::Relaxed);
        let dropped = running.counters.dropped.load(Ordering::Relaxed);
        (sent, dropped, waiting)
    };
    let (sent, dropped, waiting) = accounted();
    assert!(dropped > 0, "{sent} sent, {waiting} waiting, none dropped");
    // Sent, dropped, waiting, or in the one batch the task is holding.
    let in_hand = offered.saturating_sub(sent + dropped + waiting);
    assert!(
        sent + dropped + waiting <= offered && in_hand <= 200,
        "{in_hand} unaccounted for: {sent} sent, {dropped} dropped, {waiting} waiting"
    );
    println!(
        "of {offered} offered to the stalled forwarder: {sent} written before it stalled, {dropped} dropped and counted, {waiting} waiting, {in_hand} in hand"
    );

    // Not slower in any way that matters. A forwarder that made ingest wait
    // for its collector even once would hold a batch for the five seconds
    // it gives a write. Measured against the other application rather than
    // against a number, because how long a batch takes is this machine's
    // business.
    assert!(
        with < without * 3 / 2 + Duration::from_millis(500),
        "ingest took {with:?} with a stalled collector and {without:?} without one"
    );
    assert!(
        slowest < slowest_without * 2 + Duration::from_secs(2),
        "one batch took {slowest:?}; the slowest without a forwarder took {slowest_without:?}"
    );

    // And the forwarder says what is wrong with its collector.
    let row = until(&forwarding.app, &id, 30, "the stall to be noticed", |row| {
        row["state"] == json!("failing")
    })
    .await;
    assert!(row["last_error"].is_string(), "{row}");
    let said = said.lock().unwrap().clone();
    assert!(
        said.iter().any(|error| error.contains("stopped reading")),
        "{said:?}"
    );
    watcher.abort();
    held.abort();
}

#[tokio::test]
async fn a_collector_that_is_down_is_tried_again_and_what_cannot_be_sent_is_counted() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    // A port with nothing behind it.
    let (placeholder, _) = tcp_collector_on(0).await.unwrap();
    let collector_port = placeholder.port;
    placeholder.stop().await;
    let forwarder = save(
        app,
        json!({"name": "SIEM", "kind": "syslog_tcp", "host": "127.0.0.1", "port": collector_port, "streams": ["syslog"]}),
    )
    .await;
    let id = id_of(&forwarder);

    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    let row = until(app, &id, 5, "the refusal", |row| {
        row["state"] == json!("failing")
    })
    .await;
    assert_eq!(
        row["last_error"],
        json!("connection refused by the destination")
    );
    assert_eq!(
        (row["sent"].clone(), row["dropped"].clone()),
        (json!(0), json!(0))
    );

    // The collector comes up before the attempts run out: the line that
    // was waiting arrives, it was not thrown away at the first failure.
    let Some((collector, mut received)) = tcp_collector_on(collector_port).await else {
        // Something else took the port in between; nothing to prove here.
        return;
    };
    let line = next(&mut received, "the line that was waiting").await;
    assert!(line.contains("LINK-3-UPDOWN"), "{line}");
    let row = until(app, &id, 5, "the recovery", |row| {
        row["state"] == json!("ok")
    })
    .await;
    assert_eq!(row["last_error"], Value::Null);
    assert_eq!(
        (row["sent"].clone(), row["dropped"].clone()),
        (json!(1), json!(0))
    );

    // Down again, and it stays down: after its attempts the line is counted
    // as dropped rather than held for ever.
    collector.stop().await;
    send_udp("127.0.0.1", port, &[CONFIGURED]).await;
    let row = until(app, &id, 20, "the line to be given up on", |row| {
        row["dropped"] == json!(1)
    })
    .await;
    assert_eq!(row["state"], json!("failing"));
    assert_eq!(row["sent"], json!(1));
    assert!(row["last_error"].is_string(), "{row}");
    // It is in the store all the same.
    assert_eq!(messages(app, json!({"stream": "syslog"})).await.len(), 2);
}

// ---------------------------------------------------------------- the test

#[tokio::test]
async fn a_test_sends_one_event_that_says_it_is_a_test_and_stores_nothing() {
    let (lab, _core, _port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = tcp_collector().await;
    // Switched off: a destination is tested before it is trusted with logs.
    let forwarder = save(
        app,
        json!({"name": "SIEM", "kind": "syslog_tcp", "host": "127.0.0.1", "port": collector, "enabled": false}),
    )
    .await;
    let id = id_of(&forwarder);
    assert_eq!(status_of(app, &id).await["state"], json!("disabled"));

    let result = app
        .rpc("logs.forward.test", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(result, json!({"sent": true}));
    let event = next(&mut received, "the test event").await;
    assert!(event.starts_with("<30>1 20"), "{event}");
    assert!(
        event.contains(" - nocturne - logs.forward.test [origin software=\"Nocturne\" ")
            && event.ends_with(
                "Nocturne test event for the log forwarder `SIEM`. This is a test: no device sent it and nothing is wrong."
            ),
        "{event}"
    );
    assert!(
        within(&mut received, Duration::from_millis(200))
            .await
            .is_empty(),
        "one event"
    );

    // Not a row: it is not something that happened on the network.
    assert!(query(app, json!({"text": "test event"})).await.is_empty());
    // It is in the audit trail, as a thing somebody did.
    let trail = query(app, json!({"stream": "audit", "text": "test event"})).await;
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0]["source"], json!("logs.forward.test"));
    assert_eq!(trail[0]["body"], json!({"id": id, "ok": true}));
    // It is not counted as forwarding either.
    let row = status_of(app, &id).await;
    assert_eq!(
        (row["sent"].clone(), row["state"].clone()),
        (json!(0), json!("disabled"))
    );

    assert_eq!(
        app.rpc("logs.forward.test", json!({"id": "no-such-forwarder"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

// ------------------------------------------------------- saving, deleting

#[tokio::test]
async fn a_forwarder_that_cannot_work_is_refused_with_the_reason() {
    let lab = lab(&[]).await;
    let app = &lab.app;
    let cases = [
        (json!({"name": "x", "host": "10.0.0.1"}), "kind"),
        (
            json!({"name": " ", "kind": "syslog_udp", "host": "10.0.0.1"}),
            "name",
        ),
        (json!({"name": "x", "kind": "syslog_udp"}), "host"),
        (
            json!({"name": "x", "kind": "syslog_tcp", "host": "two words"}),
            "host name",
        ),
        (
            json!({"name": "x", "kind": "syslog_tcp", "host": "tcp://10.0.0.1"}),
            "host name",
        ),
        (
            json!({"name": "x", "kind": "syslog_tcp", "host": "https://hec.example.net/services/collector?token=pasted-in-the-wrong-field"}),
            "host name",
        ),
        (
            json!({"name": "x", "kind": "syslog_tls", "host": "10.0.0.1", "port": 0, "streams": ["s".repeat(5_000)]}),
            "no stream called",
        ),
        (
            json!({"name": "x", "kind": "syslog_udp", "host": "10.0.0.1", "streams": ["netflow"]}),
            "netflow",
        ),
        (
            json!({"name": "x", "kind": "syslog_udp", "host": "10.0.0.1", "streams": []}),
            "at least one stream",
        ),
        (
            json!({"name": "x", "kind": "syslog_udp", "host": "10.0.0.1", "min_severity": 8}),
            "severity",
        ),
        (
            json!({"name": "x", "kind": "syslog_udp", "host": "10.0.0.1", "ca_pem": "-----BEGIN CERTIFICATE-----"}),
            "certificate",
        ),
        (
            json!({"name": "x", "kind": "syslog_tls", "host": "10.0.0.1", "ca_pem": "not a certificate"}),
            "certificate",
        ),
        (
            json!({"name": "x", "kind": "syslog_udp", "host": "10.0.0.1", "url": "https://10.0.0.1/x"}),
            "not a URL",
        ),
        (json!({"name": "x", "kind": "https_json"}), "needs the URL"),
        (
            json!({"name": "x", "kind": "https_json", "token": "t"}),
            "needs the URL",
        ),
        (
            json!({"name": "x", "kind": "https_json", "url": "collector.example.net"}),
            "https://",
        ),
        (
            json!({"name": "x", "kind": "https_json", "url": "https://c.example.net/x", "format": "rfc5424"}),
            "JSON",
        ),
        (
            json!({"name": "x", "kind": "https_json", "url": "https://c.example.net/x", "host": "c.example.net"}),
            "URL",
        ),
        (
            json!({"id": "no-such-forwarder", "name": "x", "kind": "syslog_udp", "host": "10.0.0.1"}),
            "no-such-forwarder",
        ),
    ];
    for (params, reason) in cases {
        let refused = app
            .rpc("logs.forward.save", params.clone())
            .await
            .expect_err(&format!("{params} should be refused"));
        assert!(
            ["invalid", "not_found"].contains(&refused.code),
            "{params}: {refused}"
        );
        assert!(refused.message.contains(reason), "{params}: {refused}");
        // A refusal says what is wrong; it does not hand back what was sent.
        assert!(
            !refused.message.contains("pasted-in-the-wrong-field") && refused.message.len() < 300,
            "{refused}"
        );
    }
    // Nothing was written by any of them.
    assert_eq!(
        app.rpc("logs.forward.list", Value::Null).await.unwrap(),
        json!([])
    );
    assert!(
        query(app, json!({"stream": "audit", "text": "forwarder"}))
            .await
            .is_empty()
    );
    assert_eq!(
        app.rpc("logs.status", Value::Null).await.unwrap()["forwarders"],
        json!([])
    );
}

#[tokio::test]
async fn an_authority_pasted_together_with_a_private_key_is_refused_and_the_key_is_kept_nowhere() {
    let (lab, _core, _port) = campus().await;
    let app = &lab.app;
    let authority = authority();
    // A key made here, in the PEM form a "certificate and key" file has.
    let key = rcgen::KeyPair::generate().expect("a key").serialize_pem();
    let inside = key
        .lines()
        .find(|line| !line.starts_with("-----"))
        .expect("a line of the key")
        .to_string();
    for kind in ["syslog_tls", "https_json"] {
        let mut params = json!({"name": "Bundle", "kind": kind, "ca_pem": format!("{}\n{key}", authority.ca_pem)});
        if kind == "syslog_tls" {
            params["host"] = json!("localhost");
        } else {
            params["url"] = json!("https://localhost:8088/services/collector/event");
        }
        let refused = app.rpc("logs.forward.save", params).await.unwrap_err();
        assert_eq!(refused.code, "invalid", "{refused}");
        assert!(refused.message.contains("not a certificate"), "{refused}");
        assert!(!refused.message.contains(&inside), "{refused}");
    }
    assert_eq!(
        app.rpc("logs.forward.list", Value::Null).await.unwrap(),
        json!([]),
        "nothing was saved"
    );
    assert!(
        !everything_stored(app).contains(&inside),
        "the key is in no table"
    );

    // The certificate alone is what is asked for, and is kept as given.
    let saved = save(
        app,
        json!({"name": "Bundle", "kind": "syslog_tls", "host": "localhost", "ca_pem": authority.ca_pem}),
    )
    .await;
    assert_eq!(saved["ca_pem"], json!(authority.ca_pem.trim()));
}

#[tokio::test]
async fn a_forwarder_pointed_at_this_applications_own_receiver_is_refused() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    // Its own lines would come back as new lines, to be stored and sent again.
    for host in ["127.0.0.1", "localhost", "0.0.0.0", "::1"] {
        let refused = app
            .rpc(
                "logs.forward.save",
                json!({"name": "Loop", "kind": "syslog_udp", "host": host, "port": port}),
            )
            .await
            .expect_err(host);
        assert_eq!(refused.code, "invalid", "{host}: {refused}");
        assert!(refused.message.contains("own syslog receiver"), "{refused}");
    }
    assert_eq!(
        app.rpc("logs.forward.list", Value::Null).await.unwrap(),
        json!([])
    );
    // Another port on this machine is somebody else's collector.
    let (collector, _received) = udp_collector().await;
    save(
        app,
        json!({"name": "Local collector", "kind": "syslog_udp", "host": "127.0.0.1", "port": collector}),
    )
    .await;
}

#[tokio::test]
async fn a_forwarder_is_saved_with_careful_defaults_listed_by_name_and_deleted_with_its_secret() {
    let lab = lab(&[]).await;
    let app = &lab.app;
    let tls = save(
        app,
        json!({"name": "b-tls", "kind": "syslog_tls", "host": "collector.example.net"}),
    )
    .await;
    let id = id_of(&tls);
    let updated_at = tls["updated_at"].as_i64().expect("a time");
    assert_eq!(
        tls,
        json!({"id": id, "name": "b-tls", "enabled": true, "kind": "syslog_tls",
               "host": "collector.example.net", "port": 6514, "format": "rfc5424",
               "streams": ["syslog", "trap", "event"], "min_severity": null, "lab": false,
               "ca_pem": null, "has_secret": false, "updated_at": updated_at})
    );
    let udp = save(
        app,
        json!({"name": "A-udp", "kind": "syslog_udp", "host": "10.0.0.9", "enabled": false}),
    )
    .await;
    assert_eq!(
        (udp["port"].clone(), udp["enabled"].clone()),
        (json!(514), json!(false))
    );
    let https = save(
        app,
        json!({"name": "c-hec", "kind": "https_json", "url": "https://hec.example.net:8088/services/collector/event", "token": "abc"}),
    )
    .await;
    let hec = id_of(&https);
    let secret_id = nocturne_logs::forward::secret_id(&hec);
    assert_eq!(secret_id, format!("logs:forward:{hec}:secret"));
    assert!(app.vault().exists(&secret_id).unwrap());

    let listed = app.rpc("logs.forward.list", Value::Null).await.unwrap();
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|forwarder| forwarder["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["A-udp", "b-tls", "c-hec"]);
    assert_eq!(listed[1], tls);
    let states: Vec<Value> = app.rpc("logs.status", Value::Null).await.unwrap()["forwarders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|forwarder| forwarder["state"].clone())
        .collect();
    assert_eq!(
        states,
        vec![json!("disabled"), json!("idle"), json!("idle")]
    );

    // Saved again without `enabled`: a forwarder that is off stays off.
    let renamed = save(
        app,
        json!({"id": udp["id"], "name": "a-udp", "kind": "syslog_udp", "host": "10.0.0.9"}),
    )
    .await;
    assert_eq!(renamed["enabled"], json!(false));

    // Turned into a syslog forwarder, an HTTPS one leaves no URL behind.
    let turned = save(
        app,
        json!({"id": hec, "name": "c-hec", "kind": "syslog_tcp", "host": "10.0.0.7"}),
    )
    .await;
    assert_eq!(turned["has_secret"], json!(false));
    assert!(!app.vault().exists(&secret_id).unwrap());
    save(
        app,
        json!({"id": hec, "name": "c-hec", "kind": "https_json", "url": "https://hec.example.net:8088/services/collector/event"}),
    )
    .await;
    assert!(app.vault().exists(&secret_id).unwrap());

    let mut events = app.subscribe();
    let deleted = app
        .rpc("logs.forward.delete", json!({"id": hec}))
        .await
        .unwrap();
    assert_eq!(deleted, json!({"deleted": hec}));
    assert!(
        !app.vault().exists(&secret_id).unwrap(),
        "the URL goes with the forwarder"
    );
    assert_eq!(
        app.rpc("logs.forward.list", Value::Null)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let changed = drained(&mut events, "logs.status_changed");
    assert_eq!(
        changed.last().expect("the status is announced")["forwarders"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let trail = query(
        app,
        json!({"stream": "audit", "text": "Deleted the log forwarder"}),
    )
    .await;
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0]["source"], json!("logs.forward.delete"));
    assert_eq!(trail[0]["body"], json!({"id": hec, "kind": "https_json"}));
    assert_eq!(
        app.rpc("logs.forward.delete", json!({"id": hec}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );

    // There is a limit, because each one is a task and a queue.
    for n in 0..14 {
        save(app, json!({"name": format!("extra {n}"), "kind": "syslog_udp", "host": "10.0.0.9", "enabled": false})).await;
    }
    let refused = app
        .rpc(
            "logs.forward.save",
            json!({"name": "one too many", "kind": "syslog_udp", "host": "10.0.0.9"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("16"), "{refused}");
}

#[tokio::test]
async fn switching_a_forwarder_off_stops_it_and_renaming_it_keeps_its_count() {
    let (lab, _core, port) = campus().await;
    let app = &lab.app;
    let (collector, mut received) = tcp_collector().await;
    let params = json!({"name": "SIEM", "kind": "syslog_tcp", "host": "127.0.0.1", "port": collector, "streams": ["syslog"]});
    let forwarder = save(app, params.clone()).await;
    let id = id_of(&forwarder);
    send_udp("127.0.0.1", port, &[LINK_DOWN]).await;
    next(&mut received, "the first line").await;
    until(app, &id, 5, "one sent", |row| row["sent"] == json!(1)).await;

    let mut renamed = params.clone();
    renamed["id"] = json!(id);
    renamed["name"] = json!("SIEM (primary)");
    save(app, renamed.clone()).await;
    send_udp("127.0.0.1", port, &[CONFIGURED]).await;
    next(&mut received, "the second line").await;
    let row = until(app, &id, 5, "two sent", |row| row["sent"] == json!(2)).await;
    assert_eq!(row["name"], json!("SIEM (primary)"));

    renamed["enabled"] = json!(false);
    save(app, renamed).await;
    send_udp("127.0.0.1", port, &[INFORMATIONAL]).await;
    wait_until_settled(app, 3).await;
    assert!(
        within(&mut received, Duration::from_millis(300))
            .await
            .is_empty()
    );
    let row = status_of(app, &id).await;
    assert_eq!(
        (
            row["state"].clone(),
            row["sent"].clone(),
            row["dropped"].clone()
        ),
        (json!("disabled"), json!(2), json!(0))
    );
}
