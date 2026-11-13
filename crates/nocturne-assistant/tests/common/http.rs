//! A local HTTP server that answers prepared replies, in the style of
//! `crates/nocturne-ai/tests/providers_http.rs`.
//!
//! It is how the provider methods and the prompted-tools path are tested
//! without a key, without the network and without a model.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Reply {
    /// An OpenAI-style streamed completion.
    pub fn sse_data(chunks: &[Value]) -> Reply {
        let mut body: String = chunks
            .iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .collect();
        body.push_str("data: [DONE]\n\n");
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body,
        }
    }

    /// An Anthropic-style streamed message.
    pub fn sse(events: &[Value]) -> Reply {
        let body = events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap_or("message")
                )
            })
            .collect();
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body,
        }
    }

    pub fn json(status: u16, body: Value) -> Reply {
        Reply {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub struct Server {
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    pub async fn start(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let queue = queue.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().unwrap().push(request);
                    let reply = queue.lock().unwrap().pop_front();
                    let Some(reply) = reply else {
                        let _ = socket
                            .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                            .await;
                        return;
                    };
                    let head = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        reply.status,
                        reply.content_type,
                        reply.body.len(),
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    // Two writes, split mid-body, to exercise reassembly.
                    let (first, second) = reply.body.split_at(reply.body.len() / 2);
                    let _ = socket.write_all(first.as_bytes()).await;
                    let _ = socket.flush().await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let _ = socket.write_all(second.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        Server { url, seen }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&buffer[header_end..]).unwrap_or(Value::Null);
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}
