//! Servers the service-check tests start for themselves, on loopback, on a
//! port the operating system chooses.
//!
//! Nothing here reaches the internet, nothing binds a fixed port and nothing
//! needs root. Each server is held by a [`Server`] handle; dropping it stops
//! the server, which is how a test makes a service go away.
//!
//! Two habits that matter on a busy machine, where another test's build may
//! be using every core:
//!
//! * **Wait for a condition, never for a duration.** A test that needs
//!   something to have happened polls for it against [`soon`], a deadline. A
//!   `sleep` long enough to be reliable here would be long enough to be slow
//!   everywhere.
//! * **Assert what was measured, not how long it took.** A response time is
//!   asserted to exist and to be finite, never to be under a number.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

/// How long a test waits for a condition before giving up. Generous on
/// purpose: this is a deadline, not a delay, so a quick machine does not pay
/// for it.
const DEADLINE: Duration = Duration::from_secs(20);

/// A server on loopback.
///
/// [`Server::stop`] takes a service away *and waits until it is gone*, which
/// is what a test needs before it can assert what a check makes of it:
/// aborting the task and hoping is a race, and a race in a test about
/// outages shows up as a mysterious `timeout` where a `refused` was meant.
/// Dropping the handle aborts the task instead, for the tests that simply
/// end.
pub struct Server {
    pub port: u16,
    task: Option<tokio::task::JoinHandle<()>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    /// A handle for a server started elsewhere in these tests, which cannot
    /// be asked to stop — only dropped.
    pub fn from_parts(port: u16, task: tokio::task::JoinHandle<()>) -> Server {
        Server {
            port,
            task: Some(task),
            stop: None,
        }
    }

    fn with_stop(
        port: u16,
        task: tokio::task::JoinHandle<()>,
        stop: tokio::sync::oneshot::Sender<()>,
    ) -> Server {
        Server {
            port,
            task: Some(task),
            stop: Some(stop),
        }
    }

    /// Stop serving, and wait until the socket is closed.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// A port nothing is listening on: bound to learn its number, then released.
pub async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    drop(listener);
    port
}

/// A TCP server that accepts connections and does nothing else.
pub async fn tcp_server() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    let (stop, mut stopping) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                // Taking the listener out of scope here is what closes the
                // socket, before `stop` returns to the test.
                _ = &mut stopping => return,
                accepted = listener.accept() => match accepted {
                    // An accepted connection is dropped straight away, which
                    // is what a service that answers and says nothing is.
                    Ok((stream, _)) => drop(stream),
                    Err(_) => return,
                },
            }
        }
    });
    Server::with_stop(port, task, stop)
}

/// What an HTTP server should answer with.
pub struct Reply {
    pub status: u16,
    pub reason: &'static str,
    pub body: String,
}

impl Reply {
    pub fn ok(body: &str) -> Reply {
        Reply {
            status: 200,
            reason: "OK",
            body: body.to_string(),
        }
    }

    pub fn status(status: u16, reason: &'static str) -> Reply {
        Reply {
            status,
            reason,
            body: String::new(),
        }
    }
}

/// An HTTP/1.1 server that answers every request the same way, and remembers
/// the request lines it was sent.
pub async fn http_server(reply: Reply) -> (Server, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.reason,
        reply.body.len(),
        reply.body
    );
    let (stop, mut stopping) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = &mut stopping => return,
                accepted = listener.accept() => accepted,
            };
            let Ok((mut stream, _)) = accepted else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 2048];
            // One read is enough for a request with no body.
            if let Ok(read) = stream.read(&mut chunk).await {
                buffer.extend_from_slice(&chunk[..read]);
            }
            let text = String::from_utf8_lossy(&buffer).to_string();
            if let Some(line) = text.lines().next() {
                recorded.lock().push(line.to_string());
            }
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });
    (Server::with_stop(port, task, stop), seen)
}

/// A DNS server over UDP that answers every `A` question with `address`, or
/// with `rcode` when one is given.
///
/// Hand-built per RFC 1035 — the question is echoed and one answer record
/// points back at it, which is all a resolver needs. The question has to be
/// measured rather than assumed, because the query carries an EDNS record
/// after it that must not be copied into the answer section.
pub async fn dns_server(address: [u8; 4], rcode: u8) -> Server {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a socket");
    let port = socket.local_addr().expect("an address").port();
    let task = tokio::spawn(async move {
        let mut buffer = [0u8; 1500];
        while let Ok((length, from)) = socket.recv_from(&mut buffer).await {
            let request = &buffer[..length];
            let Some(question_end) = question_end(request) else {
                continue;
            };
            let answers: u16 = if rcode == 0 { 1 } else { 0 };
            let mut reply = Vec::with_capacity(question_end + 16);
            reply.extend_from_slice(&request[0..2]); // the query's id
            reply.push(0x81); // response, recursion desired
            reply.push(0x80 | (rcode & 0x0f)); // recursion available, rcode
            reply.extend_from_slice(&[0x00, 0x01]); // one question
            reply.extend_from_slice(&answers.to_be_bytes());
            reply.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // no authority, no additional
            reply.extend_from_slice(&request[12..question_end]); // the question, as asked
            if answers == 1 {
                reply.extend_from_slice(&[0xc0, 0x0c]); // the name: a pointer to it
                reply.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // A, IN
                reply.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]); // TTL 60
                reply.extend_from_slice(&[0x00, 0x04]); // four bytes of address
                reply.extend_from_slice(&address);
            }
            let _ = socket.send_to(&reply, from).await;
        }
    });
    Server::from_parts(port, task)
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

/// An SNTP server (RFC 4330) at `stratum`, whose clock is `offset_s` ahead of
/// the client's.
///
/// It echoes the client's transmit timestamp as the originate timestamp,
/// which is what the client checks, and shifts the receive and transmit
/// timestamps by `offset_s` — which is how a test produces a clock that is
/// measurably wrong without touching this machine's.
pub async fn ntp_server(stratum: u8, offset_s: f64) -> Server {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a socket");
    let port = socket.local_addr().expect("an address").port();
    let task = tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        while let Ok((length, from)) = socket.recv_from(&mut buffer).await {
            if length < 48 {
                continue;
            }
            let originate: [u8; 8] = buffer[40..48].try_into().expect("eight bytes");
            let shifted = shift(originate, offset_s);
            let mut reply = [0u8; 48];
            reply[0] = (4 << 3) | 4; // no leap warning, version 4, server
            reply[1] = stratum;
            reply[2] = 4; // poll
            reply[3] = 0xEC; // precision 2^-20
            reply[4..8].copy_from_slice(&[0x00, 0x00, 0x10, 0x00]); // root delay 1/16 s
            reply[8..12].copy_from_slice(&[0x00, 0x00, 0x08, 0x00]); // root dispersion 1/32 s
            reply[12..16].copy_from_slice(&[10, 0, 0, 1]); // its upstream
            reply[16..24].copy_from_slice(&shifted); // reference time
            reply[24..32].copy_from_slice(&originate); // originate: the client's own
            reply[32..40].copy_from_slice(&shifted); // receive
            reply[40..48].copy_from_slice(&shifted); // transmit
            let _ = socket.send_to(&reply, from).await;
        }
    });
    Server::from_parts(port, task)
}

/// Move an NTP timestamp (32 bits of seconds, 32 of fraction) by `seconds`.
fn shift(stamp: [u8; 8], seconds: f64) -> [u8; 8] {
    let whole = u32::from_be_bytes(stamp[0..4].try_into().expect("four bytes"));
    let fraction = u32::from_be_bytes(stamp[4..8].try_into().expect("four bytes"));
    let moved = (f64::from(whole) + f64::from(fraction) / 4_294_967_296.0 + seconds).max(0.0);
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&(moved as u32).to_be_bytes());
    out[4..8].copy_from_slice(&(((moved.fract()) * 4_294_967_296.0) as u32).to_be_bytes());
    out
}

/// How long a condition has to settle, as an `Instant` to compare against.
pub fn soon() -> Instant {
    Instant::now() + DEADLINE
}
