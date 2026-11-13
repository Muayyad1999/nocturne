//! What the tests share: a server on a free port, clients, and fixtures.

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use nocturne_server::{ServerHandle, ServerOptions, Timing, serve_with_timing};
use nocturne_suite::{App, AppBuilder, AppConfig, AppError};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Long enough for a loaded machine, short enough to fail instead of hanging.
pub const PATIENCE: Duration = Duration::from_secs(20);

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// `future`, or a panic naming `what` if it takes unreasonably long.
pub async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[derive(Deserialize)]
struct SleepParams {
    ms: u64,
    /// Published when the sleep is over, if given.
    #[serde(default)]
    then_publish: Option<String>,
}

/// The real application, in memory, plus a few methods that make transport
/// behaviour observable.
pub fn test_app() -> App {
    let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
    nocturne_suite::init(&mut builder);
    builder
        .router
        .add("test.echo", |_app: App, params: Value| async move {
            Ok::<_, AppError>(params)
        });
    builder
        .router
        .add("test.sleep", |app: App, params: SleepParams| async move {
            tokio::time::sleep(Duration::from_millis(params.ms)).await;
            if let Some(topic) = params.then_publish {
                app.publish(&topic, &json!({"slept_ms": params.ms}));
            }
            Ok::<_, AppError>(json!({"slept_ms": params.ms}))
        });
    builder
        .router
        .add("test.panic", |_app: App, params: Value| async move {
            if params.get("really") != Some(&Value::Bool(false)) {
                panic!("test.panic was called");
            }
            Ok::<_, AppError>(Value::Null)
        });
    builder.build()
}

/// reqwest is built without a TLS provider; the tests use ring like the server.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// An HTTP client that talks to the server directly, whatever proxy the
/// environment configures, and does not follow redirects on its own.
pub fn http_client() -> reqwest::Client {
    install_crypto_provider();
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

pub struct TestServer {
    pub app: App,
    pub token: String,
    pub addr: SocketAddr,
    pub http: reqwest::Client,
    handle: Option<ServerHandle>,
}

impl TestServer {
    /// Loopback, a free port, plain HTTP, default timings.
    pub async fn start() -> TestServer {
        TestServer::start_with(|_| {}, Timing::default()).await
    }

    pub async fn start_with(
        configure: impl FnOnce(&mut ServerOptions),
        timing: Timing,
    ) -> TestServer {
        let app = test_app();
        let mut options = ServerOptions {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..ServerOptions::default()
        };
        configure(&mut options);
        let token = options.token.clone();
        let handle = serve_with_timing(app.clone(), options, timing)
            .await
            .unwrap_or_else(|err| panic!("the server did not start: {err:#}"));
        TestServer {
            app,
            token,
            addr: handle.local_addr(),
            http: http_client(),
            handle: Some(handle),
        }
    }

    pub fn handle(&self) -> &ServerHandle {
        self.handle.as_ref().expect("the server is running")
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// `ws://127.0.0.1:<port><path>`.
    pub fn ws_url(&self, path: &str) -> String {
        format!("ws://{}{path}", self.addr)
    }

    /// The `Host` value a well-behaved client sends.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    /// One authenticated RPC call; the HTTP status and the reply envelope.
    pub async fn rpc(&self, method: &str, params: Value) -> (u16, Value) {
        let response = self
            .http
            .post(self.url("/api/rpc"))
            .bearer_auth(&self.token)
            .json(&json!({"method": method, "params": params}))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap())
    }

    /// The `result` of a call that is expected to succeed.
    pub async fn call(&self, method: &str, params: Value) -> Value {
        let (status, reply) = self.rpc(method, params).await;
        assert_eq!(
            (status, &reply["ok"]),
            (200, &json!(true)),
            "{method}: {reply}"
        );
        reply["result"].clone()
    }

    /// An authenticated event stream.
    pub async fn events(&self, query: &str) -> Socket {
        let separator = if query.is_empty() { "" } else { "&" };
        let url = self.ws_url(&format!(
            "/api/events?token={}{separator}{query}",
            self.token
        ));
        ws_connect(&url, &[])
            .await
            .unwrap_or_else(|err| panic!("the event stream did not open: {err}"))
    }

    pub async fn shutdown(mut self) {
        let handle = self.handle.take().expect("the server is running");
        within("the server to shut down", handle.shutdown()).await;
    }

    pub fn take_handle(&mut self) -> ServerHandle {
        self.handle.take().expect("the server is running")
    }
}

/// Open a WebSocket, adding `headers` to the handshake.
pub async fn ws_connect(url: &str, headers: &[(&str, &str)]) -> Result<Socket, tungstenite::Error> {
    let mut request = url.into_client_request()?;
    for (name, value) in headers {
        request.headers_mut().insert(
            tungstenite::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            tungstenite::http::HeaderValue::from_str(value).unwrap(),
        );
    }
    let (socket, _response) = within(
        "the WebSocket handshake",
        tokio_tungstenite::connect_async(request),
    )
    .await?;
    Ok(socket)
}

/// The HTTP status a WebSocket handshake was refused with, and its body as JSON.
pub fn refusal(
    outcome: Result<Socket, tungstenite::Error>,
) -> (u16, tungstenite::http::HeaderMap, Value) {
    match outcome {
        Err(tungstenite::Error::Http(response)) => {
            let body = response.body().as_deref().unwrap_or_default();
            let json = serde_json::from_slice(body).unwrap_or(Value::Null);
            (response.status().as_u16(), response.headers().clone(), json)
        }
        Err(other) => panic!("expected an HTTP refusal, got {other}"),
        Ok(_) => panic!("expected an HTTP refusal, but the WebSocket opened"),
    }
}

/// The next event on `socket` as JSON. Pings are answered by the client
/// library and skipped here.
pub async fn next_event<S>(socket: &mut WebSocketStream<S>) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let frame = within("an event", socket.next())
            .await
            .expect("the event stream ended")
            .expect("the event stream failed");
        match frame {
            Message::Text(text) => return serde_json::from_str(&text).expect("events are JSON"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

/// The next event whose topic is `topic`.
pub async fn next_event_on<S>(socket: &mut WebSocketStream<S>, topic: &str) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let event = next_event(socket).await;
        if event["topic"] == topic {
            return event;
        }
    }
}

/// A response read off the wire, parsed just enough to assert on.
#[derive(Debug)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawResponse {
    /// The value of header `name` (lower case), if it was sent exactly once.
    pub fn header(&self, name: &str) -> Option<&str> {
        let mut values = self.headers.iter().filter(|(key, _)| key == name);
        match (values.next(), values.next()) {
            (Some((_, value)), None) => Some(value),
            _ => None,
        }
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn parse(bytes: &[u8]) -> RawResponse {
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap_or_else(|| {
                panic!("not an HTTP response: {:?}", String::from_utf8_lossy(bytes))
            });
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap();
        let status = status_line
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("bad status line {status_line:?}"));
        let headers = lines
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.trim().to_ascii_lowercase(), value.trim().to_string())
            })
            .collect();
        RawResponse {
            status,
            headers,
            body: bytes[split + 4..].to_vec(),
        }
    }
}

/// Send `request` verbatim over a new connection and read the reply until
/// the server closes. Requests must ask for `Connection: close`.
pub async fn raw(addr: SocketAddr, request: impl AsRef<[u8]>) -> RawResponse {
    let mut stream = within("the connection", TcpStream::connect(addr))
        .await
        .unwrap();
    stream.write_all(request.as_ref()).await.unwrap();
    let mut reply = Vec::new();
    within("the reply", stream.read_to_end(&mut reply))
        .await
        .unwrap();
    RawResponse::parse(&reply)
}

/// A minimal `GET` with full control over the headers.
pub async fn raw_get(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> RawResponse {
    raw(addr, raw_request("GET", path, headers, "")).await
}

pub fn raw_request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut request = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    request
}

/// The headers the spec requires on every response, written out here rather
/// than taken from the server's code.
pub fn assert_security_headers(header: impl Fn(&str) -> Option<String>, context: &str) {
    for (name, expected) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        ("cross-origin-opener-policy", "same-origin"),
    ] {
        assert_eq!(
            header(name).as_deref(),
            Some(expected),
            "{name} on {context}"
        );
    }
}

/// The content security policy for HTML, verbatim from the spec.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

pub const INDEX_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>fixture ui</title><script type=\"module\" src=\"./assets/app-3f9a1c.js\"></script></head><body><div id=\"root\"></div></body></html>";
pub const APP_JS: &str = "export const answer = 42;\n";
pub const APP_CSS: &str = "body { margin: 0 }\n";
pub const FAVICON_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

/// A small UI build in `root/ui`, and a file next to it that must never be served.
pub fn write_ui_fixture(root: &Path) -> std::path::PathBuf {
    let ui = root.join("ui");
    std::fs::create_dir_all(ui.join("assets")).unwrap();
    std::fs::write(ui.join("index.html"), INDEX_HTML).unwrap();
    std::fs::write(ui.join("assets/app-3f9a1c.js"), APP_JS).unwrap();
    std::fs::write(ui.join("assets/app-77aa01.css"), APP_CSS).unwrap();
    std::fs::write(
        ui.join("assets/font-51c2.woff2"),
        (0..=255u8).collect::<Vec<_>>(),
    )
    .unwrap();
    std::fs::write(ui.join("favicon.svg"), FAVICON_SVG).unwrap();
    std::fs::write(ui.join("manifest.webmanifest"), "{\"name\": \"fixture\"}").unwrap();
    std::fs::write(ui.join("release notes.txt"), "spaces work").unwrap();
    std::fs::write(root.join("secret.txt"), "outside the ui directory").unwrap();
    ui
}
