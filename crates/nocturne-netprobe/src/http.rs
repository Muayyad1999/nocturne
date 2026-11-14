//! HTTP(S) check: one request, with every phase timed.
//!
//! A small HTTP/1.1 client on tokio (and tokio-rustls for HTTPS) rather than a general-purpose
//! one, so that name resolution, TCP connect, TLS handshake and time to first byte can be
//! measured separately — which is what tells a slow server from a slow network.

use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hickory_resolver::proto::rr::Name;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::tls::{self, Failure, Mode, TlsReport, Trust};
use crate::util::{self, millis};
use crate::{ProbeError, dns, tcp};

/// The outcome of an HTTP check.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HttpReport {
    /// The URL that was asked for (with `http://` added if it had no scheme).
    pub url: String,
    /// The URL that produced this response, after redirects.
    pub final_url: String,
    pub status: u16,
    /// The reason phrase of the status line (`OK`, `Not Found`); may be empty.
    pub reason: String,
    /// `HTTP/1.1` or `HTTP/1.0`.
    pub version: String,
    /// The response headers in the order received.
    pub headers: Vec<(String, String)>,
    /// The redirects that were followed: status and target, in order.
    pub redirects: Vec<(u16, String)>,
    /// Name resolution, TCP connect and TLS handshake of the final request.
    pub dns_ms: f64,
    pub connect_ms: f64,
    pub tls_ms: Option<f64>,
    /// From the request being sent to the first byte of the response.
    pub first_byte_ms: f64,
    /// The whole check, redirects included.
    pub total_ms: f64,
    /// Body bytes received (after chunked decoding); at most `max_body_bytes`.
    pub body_bytes: u64,
    /// The beginning of the body if it is text; empty for binary content.
    pub body_preview: String,
    /// The TLS session of the final request, for HTTPS.
    pub tls: Option<TlsReport>,
}

/// Options of [`check`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct HttpOptions {
    pub method: String,
    /// Extra request headers; one with the name of a default header replaces it.
    pub headers: Vec<(String, String)>,
    /// How many redirects are followed. With 0 a redirect is itself the result.
    pub follow_redirects: u8,
    /// Time allowed for the whole check.
    pub timeout_ms: u64,
    /// Require a trusted certificate for HTTPS. When off, the request is sent regardless and
    /// the report's `tls` section says whether the certificate would have been trusted.
    pub verify_tls: bool,
    /// Stop reading the body after this many bytes.
    pub max_body_bytes: u64,
}

impl Default for HttpOptions {
    fn default() -> Self {
        HttpOptions {
            method: "GET".to_owned(),
            headers: Vec::new(),
            follow_redirects: 5,
            timeout_ms: 10_000,
            verify_tls: true,
            max_body_bytes: 1024 * 1024,
        }
    }
}

/// Response headers larger than this are refused.
const MAX_HEAD: usize = 64 * 1024;
/// How much of a text body is kept as the preview.
const PREVIEW_BYTES: usize = 2048;

/// Requests `url` (plain HTTP or HTTPS) and reports the response and the timing of each phase.
///
/// An HTTP error status (404, 500, …) is a successful check and is reported in `status`;
/// errors are failures to get any HTTP response. `now_unix` is the time certificates are
/// judged against.
pub async fn check(
    url: &str,
    options: &HttpOptions,
    now_unix: i64,
) -> Result<HttpReport, ProbeError> {
    check_with(url, options, now_unix, Trust::Platform).await
}

pub(crate) async fn check_with(
    url: &str,
    options: &HttpOptions,
    now_unix: i64,
    trust: Trust,
) -> Result<HttpReport, ProbeError> {
    let request = RequestTemplate::new(options)?;
    let first = Url::parse(url)?;
    if options.timeout_ms == 0 {
        return Err(ProbeError::invalid(
            "the HTTP timeout must be at least 1 ms",
        ));
    }
    let started = Instant::now();
    let work = async {
        let mut url = first.clone();
        let mut method = request.method.clone();
        let mut redirects: Vec<(u16, String)> = Vec::new();
        loop {
            let exchange = exchange(&url, &method, &request, options, now_unix, &trust).await?;
            let location = exchange
                .head
                .header("location")
                .filter(|_| matches!(exchange.head.status, 301 | 302 | 303 | 307 | 308));
            if let Some(location) = location
                && redirects.len() < usize::from(options.follow_redirects)
            {
                let target = url.join(location)?;
                redirects.push((exchange.head.status, target.to_string()));
                // As browsers do: 303 always becomes GET, 301/302 turn POST into GET.
                if exchange.head.status == 303 && method != "HEAD"
                    || matches!(exchange.head.status, 301 | 302) && method == "POST"
                {
                    method = "GET".to_owned();
                }
                url = target;
                continue;
            }
            return Ok(HttpReport {
                url: first.to_string(),
                final_url: url.to_string(),
                status: exchange.head.status,
                reason: exchange.head.reason,
                version: exchange.head.version,
                headers: exchange.head.headers,
                redirects,
                dns_ms: exchange.dns_ms,
                connect_ms: exchange.connect_ms,
                tls_ms: exchange.tls.as_ref().map(|tls| tls.handshake_ms),
                first_byte_ms: exchange.first_byte_ms,
                total_ms: millis(started.elapsed()),
                body_bytes: exchange.body.bytes,
                body_preview: exchange.body.preview(),
                tls: exchange.tls,
            });
        }
    };
    tokio::time::timeout(Duration::from_millis(options.timeout_ms), work)
        .await
        .map_err(|_| ProbeError::Timeout)?
}

/// The parts of the request that are the same for every hop of a redirect chain.
struct RequestTemplate {
    method: String,
    headers: Vec<(String, String)>,
}

impl RequestTemplate {
    fn new(options: &HttpOptions) -> Result<Self, ProbeError> {
        let method = options.method.trim().to_ascii_uppercase();
        if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(ProbeError::invalid(format!(
                "'{}' is not an HTTP method; use GET, HEAD, POST, …",
                options.method
            )));
        }
        for (name, value) in &options.headers {
            let name_ok = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
            if !name_ok {
                return Err(ProbeError::invalid(format!(
                    "'{name}' is not a valid HTTP header name"
                )));
            }
            if value.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
                return Err(ProbeError::invalid(format!(
                    "the value of the '{name}' header contains a line break"
                )));
            }
        }
        Ok(RequestTemplate {
            method,
            headers: options.headers.clone(),
        })
    }

    /// The request head for `url`.
    fn render(&self, method: &str, url: &Url) -> String {
        let mut headers: Vec<(String, String)> = vec![
            ("Host".to_owned(), url.authority()),
            (
                "User-Agent".to_owned(),
                concat!("nocturne-netprobe/", env!("CARGO_PKG_VERSION")).to_owned(),
            ),
            ("Accept".to_owned(), "*/*".to_owned()),
            ("Connection".to_owned(), "close".to_owned()),
        ];
        if matches!(method, "POST" | "PUT" | "PATCH") {
            headers.push(("Content-Length".to_owned(), "0".to_owned()));
        }
        for (name, value) in &self.headers {
            headers.retain(|(default, _)| !default.eq_ignore_ascii_case(name));
            headers.push((name.clone(), value.trim().to_owned()));
        }
        let mut head = format!("{method} {} HTTP/1.1\r\n", url.target);
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        head
    }
}

/// One request / response on its own connection.
struct Exchange {
    head: Head,
    body: Body,
    dns_ms: f64,
    connect_ms: f64,
    first_byte_ms: f64,
    tls: Option<TlsReport>,
}

async fn exchange(
    url: &Url,
    method: &str,
    request: &RequestTemplate,
    options: &HttpOptions,
    now_unix: i64,
    trust: &Trust,
) -> Result<Exchange, ProbeError> {
    let resolving = Instant::now();
    let addresses = dns::resolve(&url.host).await?;
    let dns_ms = millis(resolving.elapsed());

    let mode = if options.verify_tls {
        Mode::Verify
    } else {
        Mode::Inspect
    };
    let (mut stream, connect_ms, tls) =
        connect(url, &addresses, &["http/1.1"], mode, now_unix, trust).await?;

    stream
        .write_all(request.render(method, url).as_bytes())
        .await?;
    stream.flush().await?;
    let sent = Instant::now();

    let mut reader = Reader::new(stream);
    let (head, first_byte_ms) = loop {
        let (bytes, first_byte) = reader.read_head().await?;
        let head = Head::parse(&bytes)?;
        // 100 Continue, 103 Early Hints: the real response follows.
        if !(100..200).contains(&head.status) || head.status == 101 {
            break (head, millis(first_byte.saturating_duration_since(sent)));
        }
    };

    let body = if method == "HEAD" || matches!(head.status, 100..=199 | 204 | 304) {
        Body::default()
    } else {
        reader.read_body(&head, options.max_body_bytes).await?
    };
    Ok(Exchange {
        head,
        body,
        dns_ms,
        connect_ms,
        first_byte_ms,
        tls,
    })
}

/// Connects to the first address that accepts, with TLS on top for HTTPS.
async fn connect(
    url: &Url,
    addresses: &[std::net::IpAddr],
    alpn: &[&str],
    mode: Mode,
    now_unix: i64,
    trust: &Trust,
) -> Result<(Stream, f64, Option<TlsReport>), ProbeError> {
    let mut last_error = ProbeError::Resolve;
    for address in addresses {
        let address = SocketAddr::new(*address, url.port);
        let connecting = Instant::now();
        let tcp = match tcp::connect(address).await {
            Ok(tcp) => tcp,
            Err(error) => {
                last_error = ProbeError::from_network_io(error);
                continue;
            }
        };
        let connect_ms = millis(connecting.elapsed());
        if !url.https {
            return Ok((Stream::Plain(tcp), connect_ms, None));
        }
        return match tls::handshake(tcp, &url.host, alpn, mode, now_unix, trust).await {
            Ok((stream, session)) => Ok((
                Stream::Tls(Box::new(stream)),
                connect_ms,
                Some(session.into_report(&url.host, address)),
            )),
            // The server knows no "http/1.1" protocol name; plain HTTP over TLS it is.
            Err(Failure::AlpnRejected) if !alpn.is_empty() => {
                Box::pin(connect(url, &[address.ip()], &[], mode, now_unix, trust)).await
            }
            Err(failure) => Err(failure.into_probe_error()),
        };
    }
    Err(last_error)
}

/// A plain or TLS connection.
enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Stream::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Stream::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Stream::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Stream::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// An absolute `http` / `https` URL, reduced to what a request needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Url {
    https: bool,
    /// Host name (ASCII) or address, without brackets.
    host: String,
    port: u16,
    /// Path and query as sent in the request line; always starts with `/`.
    target: String,
}

impl Url {
    fn parse(text: &str) -> Result<Url, ProbeError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ProbeError::invalid(
                "the URL is empty; enter something like https://device.example/",
            ));
        }
        if let Some(bad) = text.chars().find(|c| c.is_whitespace() || c.is_control()) {
            return Err(ProbeError::invalid(format!(
                "the URL '{text}' contains {bad:?}; encode spaces as %20"
            )));
        }
        let (https, rest) = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => (false, rest),
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => (true, rest),
            Some((scheme, _)) => {
                return Err(ProbeError::invalid(format!(
                    "'{scheme}://' URLs cannot be checked; only http:// and https:// are supported"
                )));
            }
            // "10.0.0.1/status" and the like: plain HTTP is what an unqualified device URL means.
            None => (false, text),
        };
        let rest = rest.split('#').next().unwrap_or_default();
        let split = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, target) = rest.split_at(split);
        if authority.contains('@') {
            return Err(ProbeError::invalid(
                "credentials inside the URL are not supported; send them in an Authorization header instead",
            ));
        }
        let default_port = if https { 443 } else { 80 };
        let (host, port) =
            util::split_host_port(authority, default_port).map_err(|error| match error {
                ProbeError::Invalid(reason) => {
                    ProbeError::invalid(format!("the URL '{text}' has no usable host: {reason}"))
                }
                other => other,
            })?;
        let host = if util::parse_ip_literal(&host).is_some() || host.is_ascii() {
            host.to_ascii_lowercase()
        } else {
            // Internationalised names travel as punycode.
            Name::from_utf8(&host)
                .map(|name| name.to_ascii().trim_end_matches('.').to_owned())
                .map_err(|_| ProbeError::invalid(format!("'{host}' is not a valid host name")))?
        };
        let target = match target {
            "" => "/".to_owned(),
            query if query.starts_with('?') => format!("/{}", percent_encode(query)),
            path => percent_encode(path),
        };
        Ok(Url {
            https,
            host,
            port,
            target,
        })
    }

    /// The `Host` header / URL authority: brackets around IPv6, port only when not the default.
    fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default_port = if self.https { 443 } else { 80 };
        if self.port == default_port {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// Resolves a `Location` header against this URL (RFC 3986 section 5).
    fn join(&self, location: &str) -> Result<Url, ProbeError> {
        let location = location.trim();
        let has_scheme = location.split_once("://").is_some_and(|(scheme, _)| {
            !scheme.is_empty()
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
        });
        if has_scheme {
            return Url::parse(location);
        }
        if let Some(rest) = location.strip_prefix("//") {
            let scheme = if self.https { "https" } else { "http" };
            return Url::parse(&format!("{scheme}://{rest}"));
        }
        let location = location.split('#').next().unwrap_or_default();
        let base_path = self.target.split('?').next().unwrap_or("/");
        let target = if location.is_empty() {
            self.target.clone()
        } else if location.starts_with('/') {
            location.to_owned()
        } else if location.starts_with('?') {
            format!("{base_path}{location}")
        } else {
            // Relative to the directory of the current path.
            let directory = base_path
                .rsplit_once('/')
                .map_or("", |(directory, _)| directory);
            format!("{directory}/{location}")
        };
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (target.as_str(), None),
        };
        let mut resolved = remove_dot_segments(path);
        if let Some(query) = query {
            resolved.push('?');
            resolved.push_str(query);
        }
        Ok(Url {
            target: percent_encode(&resolved),
            ..self.clone()
        })
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.https { "https" } else { "http" };
        write!(f, "{scheme}://{}{}", self.authority(), self.target)
    }
}

/// Escapes what may not appear raw in a request target (non-ASCII, quotes, angle brackets…).
fn percent_encode(target: &str) -> String {
    let mut encoded = String::with_capacity(target.len());
    for byte in target.bytes() {
        let keep = byte.is_ascii_graphic()
            && !matches!(
                byte,
                b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}'
            );
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `/a/b/../c/./d` -> `/a/c/d`
fn remove_dot_segments(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/').skip(1) {
        match segment {
            "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let mut out = String::from("/");
    out.push_str(&segments.join("/"));
    if (path.ends_with("/.") || path.ends_with("/..")) && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// A response's status line and headers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Head {
    version: String,
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
}

impl Head {
    /// Parses the bytes up to and including the blank line.
    fn parse(bytes: &[u8]) -> Result<Head, ProbeError> {
        let text = String::from_utf8_lossy(bytes);
        let mut lines = text.split('\n').map(|line| line.trim_end_matches('\r'));
        let status_line = lines.next().unwrap_or_default();
        let not_http = || {
            ProbeError::protocol(format!(
                "the server did not answer with HTTP; it sent: {}",
                util::printable(status_line, 60)
            ))
        };
        let mut parts = status_line.splitn(3, ' ');
        let version = parts
            .next()
            .filter(|version| version.starts_with("HTTP/"))
            .ok_or_else(not_http)?;
        let status = parts
            .next()
            .and_then(|status| status.parse::<u16>().ok())
            .filter(|status| (100..=999).contains(status))
            .ok_or_else(not_http)?;
        let reason = parts.next().unwrap_or_default().trim().to_owned();

        let mut headers: Vec<(String, String)> = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if line.starts_with([' ', '\t']) {
                // An obsolete folded continuation of the previous header.
                if let Some((_, value)) = headers.last_mut() {
                    value.push(' ');
                    value.push_str(line.trim());
                }
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_owned(), value.trim().to_owned()));
            }
        }
        Ok(Head {
            version: version.to_owned(),
            status,
            reason,
            headers,
        })
    }

    /// The first header called `name` (case-insensitive).
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn is_chunked(&self) -> bool {
        self.headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
            .flat_map(|(_, value)| value.split(','))
            .next_back()
            .is_some_and(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
    }

    /// The declared body length; an error when the server contradicts itself.
    fn content_length(&self) -> Result<Option<u64>, ProbeError> {
        let mut lengths = self
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .flat_map(|(_, value)| value.split(','))
            .map(|value| value.trim().parse::<u64>());
        let Some(first) = lengths.next() else {
            return Ok(None);
        };
        let invalid = || ProbeError::protocol("the response has an invalid Content-Length header");
        let first = first.map_err(|_| invalid())?;
        for other in lengths {
            if other.map_err(|_| invalid())? != first {
                return Err(ProbeError::protocol(
                    "the response has conflicting Content-Length headers",
                ));
            }
        }
        Ok(Some(first))
    }
}

/// What was read of a response body.
#[derive(Debug, Default)]
struct Body {
    bytes: u64,
    /// The first bytes, kept for the preview.
    start: Vec<u8>,
}

impl Body {
    fn push(&mut self, data: &[u8]) {
        self.bytes += data.len() as u64;
        let room = PREVIEW_BYTES.saturating_sub(self.start.len());
        self.start.extend_from_slice(&data[..data.len().min(room)]);
    }

    /// The start of the body as text, or nothing if it does not look like text.
    fn preview(&self) -> String {
        if self.start.contains(&0) {
            return String::new();
        }
        let text = String::from_utf8_lossy(&self.start);
        let total = text.chars().count();
        let odd = text
            .chars()
            .filter(|c| *c == char::REPLACEMENT_CHARACTER || (c.is_control() && !c.is_whitespace()))
            .count();
        // A character cut in half at the end of the window is not a sign of binary data.
        if odd * 20 > total + 20 {
            return String::new();
        }
        text.trim_end_matches(char::REPLACEMENT_CHARACTER)
            .to_owned()
    }
}

/// Reads a response off a stream, keeping what was read ahead.
struct Reader {
    stream: Stream,
    buffer: Vec<u8>,
}

impl Reader {
    fn new(stream: Stream) -> Self {
        Reader {
            stream,
            buffer: Vec::with_capacity(8192),
        }
    }

    /// Reads more into the buffer; `false` at the end of the stream.
    async fn fill(&mut self) -> Result<bool, ProbeError> {
        let mut chunk = [0u8; 8192];
        match self.stream.read(&mut chunk).await {
            Ok(0) => Ok(false),
            Ok(count) => {
                self.buffer.extend_from_slice(&chunk[..count]);
                Ok(true)
            }
            // TLS servers often close without the closing alert; that is the end of the data.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
            Err(error) => Err(ProbeError::from_network_io(error)),
        }
    }

    /// Reads up to and including the blank line that ends a header block. Also returns when
    /// the first byte of it arrived.
    async fn read_head(&mut self) -> Result<(Vec<u8>, Instant), ProbeError> {
        let mut first_byte = (!self.buffer.is_empty()).then(Instant::now);
        loop {
            if let Some(end) = find(&self.buffer, b"\r\n\r\n")
                .map(|at| at + 4)
                .or_else(|| find(&self.buffer, b"\n\n").map(|at| at + 2))
            {
                let head: Vec<u8> = self.buffer.drain(..end).collect();
                return Ok((head, first_byte.unwrap_or_else(Instant::now)));
            }
            if self.buffer.len() > MAX_HEAD {
                return Err(ProbeError::protocol(
                    "the response headers are larger than 64 KiB",
                ));
            }
            if !self.fill().await? {
                return Err(match self.buffer.is_empty() {
                    true => ProbeError::protocol(
                        "the server closed the connection without sending a response",
                    ),
                    false => ProbeError::protocol(format!(
                        "the server did not answer with HTTP; it sent: {}",
                        util::printable(&String::from_utf8_lossy(&self.buffer), 60)
                    )),
                });
            }
            first_byte.get_or_insert_with(Instant::now);
        }
    }

    async fn read_body(&mut self, head: &Head, limit: u64) -> Result<Body, ProbeError> {
        let mut body = Body::default();
        if head.is_chunked() {
            self.read_chunked(&mut body, limit).await?;
        } else if let Some(length) = head.content_length()? {
            self.read_exactly(&mut body, length, limit).await?;
        } else {
            // Delimited by the end of the connection.
            loop {
                let take = self.take(u64::MAX, limit - body.bytes);
                body.push(&take);
                if body.bytes >= limit || !self.fill().await? {
                    break;
                }
            }
        }
        Ok(body)
    }

    /// Removes up to `wanted` buffered bytes, but no more than `room`.
    fn take(&mut self, wanted: u64, room: u64) -> Vec<u8> {
        let count = (self.buffer.len() as u64).min(wanted).min(room) as usize;
        self.buffer.drain(..count).collect()
    }

    /// Reads `length` body bytes, stopping early at the limit.
    async fn read_exactly(
        &mut self,
        body: &mut Body,
        length: u64,
        limit: u64,
    ) -> Result<(), ProbeError> {
        let mut remaining = length;
        while remaining > 0 && body.bytes < limit {
            if self.buffer.is_empty() && !self.fill().await? {
                return Err(ProbeError::protocol(format!(
                    "the server closed the connection after {} of {length} body bytes",
                    length - remaining
                )));
            }
            let data = self.take(remaining, limit - body.bytes);
            remaining -= data.len() as u64;
            body.push(&data);
        }
        Ok(())
    }

    async fn read_chunked(&mut self, body: &mut Body, limit: u64) -> Result<(), ProbeError> {
        loop {
            let line = self.read_line().await?;
            let size = line.split(';').next().unwrap_or_default().trim();
            let size = u64::from_str_radix(size, 16).map_err(|_| {
                ProbeError::protocol(format!(
                    "the chunked response body is malformed (chunk size {:?})",
                    util::printable(&line, 20)
                ))
            })?;
            if size == 0 {
                // Trailers, if any, and the final blank line carry nothing we report.
                return Ok(());
            }
            self.read_exactly(body, size, limit).await?;
            if body.bytes >= limit {
                return Ok(());
            }
            // The CRLF after the chunk data.
            self.read_line().await?;
        }
    }

    /// Reads one CRLF- (or LF-) terminated line of the chunked framing.
    async fn read_line(&mut self) -> Result<String, ProbeError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                return Ok(String::from_utf8_lossy(&line).trim_end().to_owned());
            }
            if self.buffer.len() > 4096 {
                return Err(ProbeError::protocol(
                    "the chunked response body is malformed (endless chunk header)",
                ));
            }
            if !self.fill().await? {
                return Err(ProbeError::protocol(
                    "the server closed the connection in the middle of a chunked body",
                ));
            }
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tls::test_support::{
        EXPIRED_FROM, EXPIRED_UNTIL, TestCa, days_left, days_until, now, says_untrusted,
        self_signed, server_config, tls_server, valid_from, valid_until,
    };
    use pretty_assertions::assert_eq;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn urls_are_parsed_and_normalised() {
        let cases = [
            ("http://example.com", "http://example.com/"),
            (
                "HTTP://Example.COM:80/Path?q=1#frag",
                "http://example.com/Path?q=1",
            ),
            (
                "https://example.com:8443/a/b",
                "https://example.com:8443/a/b",
            ),
            ("https://example.com:443", "https://example.com/"),
            ("http://10.0.0.1:8080?x=1", "http://10.0.0.1:8080/?x=1"),
            (
                "http://[2001:db8::1]:8080/status",
                "http://[2001:db8::1]:8080/status",
            ),
            ("https://[2001:db8::1]/", "https://[2001:db8::1]/"),
            // No scheme: a device address typed as is.
            ("10.0.0.1/cgi-bin/status", "http://10.0.0.1/cgi-bin/status"),
            ("switch.lab:8080", "http://switch.lab:8080/"),
            (
                "http://bücher.example/straße",
                "http://xn--bcher-kva.example/stra%C3%9Fe",
            ),
            (
                "http://example.com/a%20b/\"q\"",
                "http://example.com/a%20b/%22q%22",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(url(input).to_string(), expected, "{input}");
        }
        let parsed = url("https://example.com:8443/a?b");
        assert_eq!(
            (
                parsed.https,
                parsed.host.as_str(),
                parsed.port,
                parsed.target.as_str()
            ),
            (true, "example.com", 8443, "/a?b")
        );
        assert_eq!(parsed.authority(), "example.com:8443");
    }

    #[test]
    fn bad_urls_explain_themselves() {
        let cases = [
            ("", "empty"),
            ("ftp://example.com/file", "only http:// and https://"),
            ("http://user:secret@example.com/", "Authorization header"),
            ("http://exa mple.com/", "encode spaces"),
            ("http://example.com:99999/", "port"),
            ("http:///path", "no usable host"),
            ("https://[::1/", "no usable host"),
        ];
        for (input, expected) in cases {
            let error = Url::parse(input).unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_)),
                "{input:?}: {error:?}"
            );
            assert!(error.to_string().contains(expected), "{input:?}: {error}");
        }
    }

    #[test]
    fn redirect_targets_are_resolved() {
        let base = url("http://example.com:8080/a/b/c?x=1");
        let cases = [
            ("https://other.example/login", "https://other.example/login"),
            ("//cdn.example/x", "http://cdn.example/x"),
            ("/root", "http://example.com:8080/root"),
            ("sibling", "http://example.com:8080/a/b/sibling"),
            ("../up", "http://example.com:8080/a/up"),
            ("../../../../too-far", "http://example.com:8080/too-far"),
            ("./here?y=2", "http://example.com:8080/a/b/here?y=2"),
            ("?y=2", "http://example.com:8080/a/b/c?y=2"),
            ("", "http://example.com:8080/a/b/c?x=1"),
            (
                "/login?next=http://example.com/a",
                "http://example.com:8080/login?next=http://example.com/a",
            ),
            ("/with#fragment", "http://example.com:8080/with"),
        ];
        for (location, expected) in cases {
            assert_eq!(
                base.join(location).unwrap().to_string(),
                expected,
                "{location:?}"
            );
        }
        assert!(base.join("ftp://example.com/").is_err());
        assert_eq!(remove_dot_segments("/a/b/../c/./d"), "/a/c/d");
        assert_eq!(remove_dot_segments("/a/b/.."), "/a/");
        assert_eq!(remove_dot_segments("/"), "/");
    }

    #[test]
    fn response_heads_are_parsed() {
        let head = Head::parse(
            b"HTTP/1.1 200 OK\r\nServer: nginx\r\nContent-Type: text/html;\r\n charset=utf-8\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nContent-Length: 12\r\n\r\n",
        )
        .unwrap();
        assert_eq!(
            (head.version.as_str(), head.status, head.reason.as_str()),
            ("HTTP/1.1", 200, "OK")
        );
        assert_eq!(
            head.headers,
            [
                ("Server", "nginx"),
                ("Content-Type", "text/html; charset=utf-8"),
                ("Set-Cookie", "a=1"),
                ("Set-Cookie", "b=2"),
                ("Content-Length", "12"),
            ]
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
        );
        assert_eq!(
            head.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(head.content_length().unwrap(), Some(12));
        assert!(!head.is_chunked());

        // No reason phrase, bare line feeds, a long reason with spaces.
        let bare = Head::parse(b"HTTP/1.0 204\n\n").unwrap();
        assert_eq!(
            (bare.version.as_str(), bare.status, bare.reason.as_str()),
            ("HTTP/1.0", 204, "")
        );
        let wordy = Head::parse(b"HTTP/1.1 404 Not Found Here\r\n\r\n").unwrap();
        assert_eq!(wordy.reason, "Not Found Here");

        let chunked =
            Head::parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n").unwrap();
        assert!(chunked.is_chunked());
        let not_last =
            Head::parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\n").unwrap();
        assert!(!not_last.is_chunked());

        let conflicting =
            Head::parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n")
                .unwrap();
        assert!(conflicting.content_length().is_err());
        let repeated = Head::parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5, 5\r\n\r\n").unwrap();
        assert_eq!(repeated.content_length().unwrap(), Some(5));
        let negative = Head::parse(b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n").unwrap();
        assert!(negative.content_length().is_err());

        for junk in [
            &b"SSH-2.0-OpenSSH_9.6\r\n\r\n"[..],
            b"HTTP/1.1 abc OK\r\n\r\n",
            b"HTTP/1.1\r\n\r\n",
            b"\r\n\r\n",
            b"HTTP/1.1 99 Low\r\n\r\n",
        ] {
            let error = Head::parse(junk).unwrap_err();
            assert!(matches!(error, ProbeError::Protocol(_)), "{error:?}");
        }
    }

    #[test]
    fn body_previews() {
        let mut text = Body::default();
        text.push(b"<html>\n<title>Switch</title>\n</html>\n");
        assert_eq!(text.preview(), "<html>\n<title>Switch</title>\n</html>\n");
        assert_eq!(text.bytes, 37);

        let mut binary = Body::default();
        binary.push(&[
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d,
        ]);
        assert_eq!(binary.preview(), "");

        let mut gzip = Body::default();
        gzip.push(&[
            0x1f, 0x8b, 0x08, 0x08, 0xd3, 0x9e, 0xfa, 0x65, 0x02, 0xff, 0xad, 0x92, 0xc1, 0x8e,
            0xda, 0x30,
        ]);
        assert_eq!(gzip.preview(), "");

        // Only the beginning of a long body is kept, and a UTF-8 character cut by the window
        // does not make the text binary.
        let mut long = Body::default();
        for _ in 0..1000 {
            long.push("größe ".as_bytes());
        }
        assert_eq!(long.bytes, 8000);
        let preview = long.preview();
        assert!(preview.starts_with("größe größe"));
        assert!(preview.len() <= PREVIEW_BYTES && preview.len() > PREVIEW_BYTES - 8);
        assert!(!preview.contains(char::REPLACEMENT_CHARACTER));
        assert_eq!(Body::default().preview(), "");
    }

    /// How a test server answers one request.
    struct Reply {
        /// The raw bytes of the response.
        raw: Vec<u8>,
        /// Leave the connection open afterwards, so the client must rely on the framing.
        keep_open: bool,
    }

    fn reply(raw: impl AsRef<[u8]>) -> Reply {
        Reply {
            raw: raw.as_ref().to_vec(),
            keep_open: true,
        }
    }

    async fn serve_connection<S, F>(
        mut stream: S,
        respond: Arc<F>,
        requests: tokio::sync::mpsc::UnboundedSender<String>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin,
        F: Fn(&str) -> Reply,
    {
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        while find(&request, b"\r\n\r\n").is_none() {
            match stream.read(&mut chunk).await {
                Ok(count) if count > 0 => request.extend_from_slice(&chunk[..count]),
                _ => return,
            }
        }
        let request = String::from_utf8_lossy(&request).into_owned();
        let answer = respond(&request);
        let _ = requests.send(request);
        let _ = stream.write_all(&answer.raw).await;
        let _ = stream.flush().await;
        if answer.keep_open {
            // Wait for the client to hang up.
            while matches!(stream.read(&mut chunk).await, Ok(count) if count > 0) {}
        } else {
            let _ = stream.shutdown().await;
        }
    }

    /// A plain HTTP server on loopback. Returns its base URL and the requests it receives.
    async fn http_server<F>(respond: F) -> (String, UnboundedReceiver<String>)
    where
        F: Fn(&str) -> Reply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (requests, received) = unbounded_channel();
        let respond = Arc::new(respond);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_connection(stream, respond.clone(), requests.clone()));
            }
        });
        (format!("http://127.0.0.1:{port}"), received)
    }

    fn quick() -> HttpOptions {
        HttpOptions {
            timeout_ms: 5000,
            ..HttpOptions::default()
        }
    }

    #[tokio::test]
    async fn content_length_response() {
        let (base, mut requests) = http_server(|_| {
            reply("HTTP/1.1 200 OK\r\nServer: TestServer/1.0\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, world!")
        })
        .await;
        let report = check(&format!("{base}/status?verbose=1"), &quick(), now())
            .await
            .unwrap();
        assert_eq!(report.url, format!("{base}/status?verbose=1"));
        assert_eq!(report.final_url, report.url);
        assert_eq!(
            (
                report.status,
                report.reason.as_str(),
                report.version.as_str()
            ),
            (200, "OK", "HTTP/1.1")
        );
        assert_eq!(
            report.headers[0],
            ("Server".to_owned(), "TestServer/1.0".to_owned())
        );
        assert_eq!(report.headers.len(), 3);
        assert_eq!(report.redirects, vec![]);
        assert_eq!(report.body_bytes, 13);
        assert_eq!(report.body_preview, "Hello, world!");
        assert_eq!(report.tls, None);
        assert_eq!(report.tls_ms, None);
        assert!(
            report.dns_ms < 5.0,
            "an address needs no resolution: {}",
            report.dns_ms
        );
        assert!(report.connect_ms > 0.0 && report.connect_ms < 1000.0);
        assert!(report.first_byte_ms >= 0.0 && report.first_byte_ms < 1000.0);
        assert!(report.total_ms >= report.connect_ms && report.total_ms < 5000.0);

        let request = requests.recv().await.unwrap();
        let port = base.rsplit(':').next().unwrap();
        assert_eq!(
            request,
            format!(
                "GET /status?verbose=1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: nocturne-netprobe/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
                env!("CARGO_PKG_VERSION")
            )
        );

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["status"], 200);
        assert_eq!(json["headers"][0][0], "Server");
        assert_eq!(json["tls"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn chunked_response_with_extensions_and_trailers() {
        let (base, _requests) = http_server(|_| {
            reply(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-Checksum\r\n\r\n\
                 7\r\nMozilla\r\n\
                 11;ext=1\r\nDeveloper Network\r\n\
                 0\r\nX-Checksum: abc\r\n\r\n",
            )
        })
        .await;
        let report = check(&base, &quick(), now()).await.unwrap();
        assert_eq!(report.status, 200);
        assert_eq!(report.body_bytes, 7 + 17);
        assert_eq!(report.body_preview, "MozillaDeveloper Network");
    }

    #[tokio::test]
    async fn body_delimited_by_the_end_of_the_connection() {
        let (base, _requests) = http_server(|_| Reply {
            raw: b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nline one\nline two\n"
                .to_vec(),
            keep_open: false,
        })
        .await;
        let report = check(&base, &quick(), now()).await.unwrap();
        assert_eq!(report.version, "HTTP/1.0");
        assert_eq!(report.body_bytes, 18);
        assert_eq!(report.body_preview, "line one\nline two\n");
    }

    #[tokio::test]
    async fn redirect_chain_is_followed_and_recorded() {
        let (base, mut requests) = http_server(|request| {
            let path = request.split(' ').nth(1).unwrap_or_default();
            match path {
                "/" => reply("HTTP/1.1 301 Moved Permanently\r\nLocation: /step2\r\nContent-Length: 0\r\n\r\n"),
                "/step2" => reply("HTTP/1.1 302 Found\r\nLocation: sub/step3?x=1\r\nContent-Length: 5\r\n\r\nmoved"),
                "/sub/step3?x=1" => reply("HTTP/1.1 307 Temporary Redirect\r\nLocation: ../final\r\nContent-Length: 0\r\n\r\n"),
                "/final" => reply("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndone"),
                _ => reply("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
            }
        })
        .await;
        let report = check(&base, &quick(), now()).await.unwrap();
        assert_eq!(report.status, 200);
        assert_eq!(report.url, format!("{base}/"));
        assert_eq!(report.final_url, format!("{base}/final"));
        assert_eq!(
            report.redirects,
            vec![
                (301, format!("{base}/step2")),
                (302, format!("{base}/sub/step3?x=1")),
                (307, format!("{base}/final")),
            ]
        );
        assert_eq!(report.body_preview, "done");
        let mut paths = Vec::new();
        while let Ok(request) = requests.try_recv() {
            paths.push(request.split(' ').nth(1).unwrap().to_owned());
        }
        assert_eq!(paths, ["/", "/step2", "/sub/step3?x=1", "/final"]);
    }

    #[tokio::test]
    async fn redirects_beyond_the_budget_are_the_result() {
        // A server that redirects forever.
        let (base, mut requests) = http_server(|request| {
            let hop: u32 = request
                .split(' ')
                .nth(1)
                .unwrap()
                .trim_start_matches("/hop")
                .parse()
                .unwrap_or(0);
            reply(format!(
                "HTTP/1.1 302 Found\r\nLocation: /hop{}\r\nContent-Length: 0\r\n\r\n",
                hop + 1
            ))
        })
        .await;
        let report = check(
            &format!("{base}/hop0"),
            &HttpOptions {
                follow_redirects: 3,
                ..quick()
            },
            now(),
        )
        .await
        .unwrap();
        assert_eq!(report.status, 302);
        assert_eq!(report.redirects.len(), 3);
        assert_eq!(report.final_url, format!("{base}/hop3"));
        assert_eq!(report.header_value("location"), Some("/hop4"));
        let mut count = 0;
        while requests.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 4);

        // Not following at all shows the redirect itself.
        let report = check(
            &format!("{base}/hop0"),
            &HttpOptions {
                follow_redirects: 0,
                ..quick()
            },
            now(),
        )
        .await
        .unwrap();
        assert_eq!((report.status, report.redirects.len()), (302, 0));
        assert_eq!(report.final_url, format!("{base}/hop0"));
    }

    impl HttpReport {
        fn header_value(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(header, _)| header.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }
    }

    #[tokio::test]
    async fn method_and_headers_are_sent_and_post_becomes_get_on_303() {
        let (base, mut requests) = http_server(|request| {
            if request.starts_with("POST /submit ") {
                reply("HTTP/1.1 303 See Other\r\nLocation: /result\r\nContent-Length: 0\r\n\r\n")
            } else {
                reply("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            }
        })
        .await;
        let options = HttpOptions {
            method: "post".to_owned(),
            headers: vec![
                ("Authorization".to_owned(), "Bearer abc123".to_owned()),
                ("user-agent".to_owned(), "probe-test".to_owned()),
                ("X-Trace".to_owned(), "  1  ".to_owned()),
            ],
            ..quick()
        };
        let report = check(&format!("{base}/submit"), &options, now())
            .await
            .unwrap();
        assert_eq!(report.status, 200);
        assert_eq!(report.redirects, vec![(303, format!("{base}/result"))]);

        let first = requests.recv().await.unwrap();
        assert!(first.starts_with("POST /submit HTTP/1.1\r\n"), "{first}");
        assert!(first.contains("\r\nContent-Length: 0\r\n"), "{first}");
        assert!(
            first.contains("\r\nAuthorization: Bearer abc123\r\n"),
            "{first}"
        );
        assert!(first.contains("\r\nuser-agent: probe-test\r\n"), "{first}");
        assert!(
            !first.contains("nocturne-netprobe"),
            "the default User-Agent is replaced: {first}"
        );
        assert!(first.contains("\r\nX-Trace: 1\r\n"), "{first}");
        let second = requests.recv().await.unwrap();
        assert!(second.starts_with("GET /result HTTP/1.1\r\n"), "{second}");
    }

    #[tokio::test]
    async fn head_requests_and_bodiless_statuses_read_no_body() {
        let (base, mut requests) = http_server(|request| {
            if request.starts_with("HEAD ") {
                // A HEAD response advertises the length of the body it does not send.
                reply("HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n")
            } else if request.contains("/empty") {
                reply("HTTP/1.1 204 No Content\r\n\r\n")
            } else {
                reply("HTTP/1.1 304 Not Modified\r\nContent-Length: 77\r\n\r\n")
            }
        })
        .await;
        let head = check(
            &base,
            &HttpOptions {
                method: "HEAD".to_owned(),
                ..quick()
            },
            now(),
        )
        .await
        .unwrap();
        assert_eq!((head.status, head.body_bytes), (200, 0));
        assert!(
            requests
                .recv()
                .await
                .unwrap()
                .starts_with("HEAD / HTTP/1.1\r\n")
        );
        let empty = check(&format!("{base}/empty"), &quick(), now())
            .await
            .unwrap();
        assert_eq!(
            (empty.status, empty.body_bytes, empty.reason.as_str()),
            (204, 0, "No Content")
        );
        let cached = check(&format!("{base}/cached"), &quick(), now())
            .await
            .unwrap();
        assert_eq!((cached.status, cached.body_bytes), (304, 0));
    }

    #[tokio::test]
    async fn interim_responses_are_skipped() {
        let (base, _requests) = http_server(|_| {
            reply("HTTP/1.1 103 Early Hints\r\nLink: </style.css>; rel=preload\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
        })
        .await;
        let report = check(&base, &quick(), now()).await.unwrap();
        assert_eq!((report.status, report.body_preview.as_str()), (200, "hi"));
        assert_eq!(report.headers.len(), 1);
    }

    #[tokio::test]
    async fn large_bodies_stop_at_the_limit() {
        let (base, _requests) = http_server(|request| {
            let body = vec![b'x'; 300_000];
            if request.contains("/chunked") {
                let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
                for piece in body.chunks(50_000) {
                    raw.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
                    raw.extend_from_slice(piece);
                    raw.extend_from_slice(b"\r\n");
                }
                raw.extend_from_slice(b"0\r\n\r\n");
                Reply {
                    raw,
                    keep_open: true,
                }
            } else if request.contains("/close") {
                let mut raw = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
                raw.extend_from_slice(&body);
                Reply {
                    raw,
                    keep_open: false,
                }
            } else {
                let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: 300000\r\n\r\n".to_vec();
                raw.extend_from_slice(&body);
                Reply {
                    raw,
                    keep_open: true,
                }
            }
        })
        .await;
        for path in ["/length", "/chunked", "/close"] {
            let all = check(&format!("{base}{path}"), &quick(), now())
                .await
                .unwrap();
            assert_eq!(all.body_bytes, 300_000, "{path}");
            assert_eq!(all.body_preview.len(), PREVIEW_BYTES);

            let limited = HttpOptions {
                max_body_bytes: 70_000,
                ..quick()
            };
            let some = check(&format!("{base}{path}"), &limited, now())
                .await
                .unwrap();
            assert_eq!(some.body_bytes, 70_000, "{path}");

            let none = HttpOptions {
                max_body_bytes: 0,
                ..quick()
            };
            let nothing = check(&format!("{base}{path}"), &none, now()).await.unwrap();
            assert_eq!(
                (nothing.body_bytes, nothing.body_preview.as_str()),
                (0, ""),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn error_statuses_are_results_not_errors() {
        let (base, _requests) = http_server(|_| {
            reply("HTTP/1.1 503 Service Unavailable\r\nRetry-After: 30\r\nContent-Length: 11\r\n\r\nmaintenance")
        })
        .await;
        let report = check(&base, &quick(), now()).await.unwrap();
        assert_eq!(
            (report.status, report.reason.as_str()),
            (503, "Service Unavailable")
        );
        assert_eq!(report.body_preview, "maintenance");
    }

    #[tokio::test]
    async fn broken_servers_are_protocol_errors() {
        let cases: [(&[u8], bool, &str); 5] = [
            (
                b"SSH-2.0-OpenSSH_9.6\r\n",
                false,
                "did not answer with HTTP",
            ),
            (b"", false, "without sending a response"),
            (
                b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort",
                false,
                "after 5 of 100 body bytes",
            ),
            (
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nabc\r\n",
                false,
                "chunk size",
            ),
            (
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab",
                false,
                "after 2 of 5 body bytes",
            ),
        ];
        for (raw, keep_open, expected) in cases {
            let (base, _requests) = http_server(move |_| Reply {
                raw: raw.to_vec(),
                keep_open,
            })
            .await;
            let error = check(&base, &quick(), now()).await.unwrap_err();
            assert!(
                matches!(error, ProbeError::Protocol(_)),
                "{expected}: {error:?}"
            );
            assert!(error.to_string().contains(expected), "{expected}: {error}");
        }
        // Headers that never end.
        let (base, _requests) = http_server(|_| {
            let mut raw = b"HTTP/1.1 200 OK\r\n".to_vec();
            raw.extend_from_slice(&b"X-Padding: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n".repeat(3000));
            Reply {
                raw,
                keep_open: true,
            }
        })
        .await;
        let error = check(&base, &quick(), now()).await.unwrap_err();
        assert!(error.to_string().contains("larger than 64 KiB"), "{error}");
    }

    #[tokio::test]
    async fn timeouts_refusals_and_bad_options() {
        // Accepts, reads the request, never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let started = Instant::now();
        let error = check(
            &format!("http://127.0.0.1:{port}/"),
            &HttpOptions {
                timeout_ms: 300,
                ..quick()
            },
            now(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProbeError::Timeout), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(listener);

        let error = check(&format!("http://127.0.0.1:{port}/"), &quick(), now())
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Refused), "{error:?}");
        assert!(matches!(
            check("http://no-such-host.invalid/", &quick(), now()).await,
            Err(ProbeError::Resolve)
        ));

        let bad = [
            HttpOptions {
                method: "GET /evil HTTP/1.1".to_owned(),
                ..quick()
            },
            HttpOptions {
                method: String::new(),
                ..quick()
            },
            HttpOptions {
                headers: vec![("X-Evil".to_owned(), "a\r\nHost: other".to_owned())],
                ..quick()
            },
            HttpOptions {
                headers: vec![("Bad Name".to_owned(), "x".to_owned())],
                ..quick()
            },
            HttpOptions {
                headers: vec![(String::new(), "x".to_owned())],
                ..quick()
            },
            HttpOptions {
                timeout_ms: 0,
                ..quick()
            },
        ];
        for options in bad {
            let error = check("http://127.0.0.1:9/", &options, now())
                .await
                .unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_)),
                "{options:?}: {error:?}"
            );
        }
    }

    #[test]
    fn options_default_and_deserialize_partially() {
        let options = HttpOptions::default();
        assert_eq!(options.method, "GET");
        assert_eq!(
            (
                options.follow_redirects,
                options.timeout_ms,
                options.verify_tls,
                options.max_body_bytes
            ),
            (5, 10_000, true, 1_048_576)
        );
        let partial: HttpOptions =
            serde_json::from_str(r#"{"verify_tls": false, "method": "HEAD"}"#).unwrap();
        assert_eq!(
            partial,
            HttpOptions {
                verify_tls: false,
                method: "HEAD".to_owned(),
                ..HttpOptions::default()
            }
        );
    }

    /// An HTTPS server on loopback answering every request with a small page.
    async fn https_server(config: rustls::ServerConfig) -> String {
        let (requests, _received) = unbounded_channel();
        let port = tls_server(config, move |stream| {
            let respond = Arc::new(|_: &str| reply("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\nsecure"));
            serve_connection(stream, respond, requests.clone())
        })
        .await;
        format!("https://localhost:{port}/")
    }

    #[tokio::test]
    async fn https_with_a_trusted_certificate() {
        let ca = TestCa::new();
        let url = https_server(server_config(ca.issue(
            &["localhost"],
            valid_from(),
            valid_until(),
        )))
        .await;
        let report = check_with(&url, &quick(), now(), ca.trust()).await.unwrap();
        assert_eq!(
            (report.status, report.body_preview.as_str()),
            (200, "secure")
        );
        let tls = report
            .tls
            .as_ref()
            .expect("an HTTPS check reports its TLS session");
        assert!(tls.trusted && tls.hostname_matches);
        assert_eq!(tls.host, "localhost");
        assert_eq!(tls.protocol, "TLSv1.3");
        assert_eq!(tls.chain[0].common_name, "localhost");
        assert_eq!(tls.expires_in_days, days_left());
        assert_eq!(report.tls_ms, Some(tls.handshake_ms));
        assert!(report.total_ms >= tls.handshake_ms);
    }

    #[tokio::test]
    async fn https_verification_rejects_untrusted_expired_and_misnamed_certificates() {
        let ca = TestCa::new();
        let self_signed_url = https_server(server_config(self_signed(
            &["localhost"],
            valid_from(),
            valid_until(),
        )))
        .await;
        let error = check_with(&self_signed_url, &quick(), now(), ca.trust())
            .await
            .unwrap_err();
        assert!(says_untrusted(&error.to_string(), "localhost"), "{error}");

        let expired_url = https_server(server_config(ca.issue(
            &["localhost"],
            EXPIRED_FROM,
            EXPIRED_UNTIL,
        )))
        .await;
        let error = check_with(&expired_url, &quick(), now(), ca.trust())
            .await
            .unwrap_err();
        assert!(
            matches!(&error, ProbeError::Protocol(text) if text.contains("has expired")),
            "{error:?}"
        );

        let misnamed_url = https_server(server_config(ca.issue(
            &["other.example"],
            valid_from(),
            valid_until(),
        )))
        .await;
        let error = check_with(&misnamed_url, &quick(), now(), ca.trust())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("not valid for this host name"),
            "{error}"
        );

        // With verification off the same servers answer, and the report says what is wrong.
        let lenient = HttpOptions {
            verify_tls: false,
            ..quick()
        };
        let report = check_with(&self_signed_url, &lenient, now(), ca.trust())
            .await
            .unwrap();
        let tls = report.tls.unwrap();
        assert_eq!(report.status, 200);
        assert!(!tls.trusted && tls.hostname_matches);
        // Not our wording specifically: with verification off the platform
        // verifier is still the one asked about the chain, and on macOS a
        // self-signed server certificate comes back in its words. See
        // `says_untrusted`.
        assert!(says_untrusted(&tls.trust_error.unwrap(), "localhost"));

        let report = check_with(&expired_url, &lenient, now(), ca.trust())
            .await
            .unwrap();
        let tls = report.tls.unwrap();
        assert_eq!(
            tls.trust_error.as_deref(),
            Some("the certificate has expired")
        );
        assert_eq!(tls.expires_in_days, days_until(EXPIRED_UNTIL));

        let report = check_with(&misnamed_url, &lenient, now(), ca.trust())
            .await
            .unwrap();
        let tls = report.tls.unwrap();
        assert!(tls.trusted && !tls.hostname_matches);
    }

    #[tokio::test]
    async fn redirect_from_http_to_https() {
        let ca = TestCa::new();
        let secure = https_server(server_config(ca.issue(
            &["localhost"],
            valid_from(),
            valid_until(),
        )))
        .await;
        let target = secure.clone();
        let (base, _requests) = http_server(move |_| {
            reply(format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n"
            ))
        })
        .await;
        let report = check_with(&base, &quick(), now(), ca.trust())
            .await
            .unwrap();
        assert_eq!(report.status, 200);
        assert_eq!(report.final_url, secure);
        assert_eq!(report.redirects, vec![(301, secure)]);
        assert!(report.tls.is_some() && report.tls_ms.is_some());
    }
}
