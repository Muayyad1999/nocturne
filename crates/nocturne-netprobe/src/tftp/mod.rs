//! A TFTP server for moving configurations and firmware images to and from network devices.

mod packet;
mod path;
mod transfer;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};

use self::packet::{Malformed, Packet, Request, code};
use self::path::Denial;
use self::transfer::{DEFAULT_BLOCK_SIZE, Link, MAX_BLOCK_SIZE, Terms};
use crate::ProbeError;

/// Configuration of a [`TftpServer`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct TftpConfig {
    /// Where to listen. The standard port is 69, which needs elevated rights on most systems.
    pub bind: SocketAddr,
    /// The directory files are served from and stored in. Nothing outside it is reachable.
    pub root: PathBuf,
    /// Serve read requests (devices downloading).
    pub allow_read: bool,
    /// Accept write requests (devices uploading).
    pub allow_write: bool,
    /// Let an upload replace an existing file.
    pub allow_overwrite: bool,
    /// How long to wait for the peer before retransmitting (unless the client asks for
    /// another timeout).
    pub timeout_ms: u64,
    /// Retransmissions before a transfer is given up.
    pub retries: u32,
    /// The largest upload accepted.
    pub max_file_bytes: u64,
}

impl Default for TftpConfig {
    fn default() -> Self {
        TftpConfig {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 69),
            root: PathBuf::new(),
            allow_read: true,
            allow_write: false,
            allow_overwrite: false,
            timeout_ms: 3000,
            retries: 5,
            max_file_bytes: 1024 * 1024 * 1024,
        }
    }
}

/// What a [`TftpServer`] reports while it runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TftpEvent {
    /// A transfer began. `size` is the file size for a download, and for an upload the size
    /// the client announced, if it did.
    Started {
        id: u64,
        peer: String,
        file: String,
        write: bool,
        size: Option<u64>,
    },
    /// Bytes transferred so far (a few times per second while a transfer runs).
    Progress {
        id: u64,
        bytes: u64,
    },
    Finished {
        id: u64,
        bytes: u64,
        elapsed_ms: u64,
    },
    Failed {
        id: u64,
        error: String,
    },
    /// A request was refused before any transfer began.
    Denied {
        peer: String,
        file: String,
        reason: String,
    },
}

/// The most transfers served at the same time; further requests are told to retry.
const MAX_TRANSFERS: usize = 256;

/// A running TFTP server. It stops when [`stop`](TftpServer::stop) is called or it is dropped.
#[derive(Debug)]
pub struct TftpServer {
    local_addr: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl TftpServer {
    /// Starts serving.
    ///
    /// RFC 1350 read and write requests in octet mode (netascii is accepted and treated as
    /// octet), with option negotiation (RFC 2347–2349: `blksize` up to 65464, `tsize`,
    /// `timeout`) and block-number wraparound for files of any size. Every transfer runs on
    /// its own port, and many can run at once.
    ///
    /// Files are confined to `root`: absolute paths, `..`, symbolic links that lead out of the
    /// root, and device names are refused. Uploads are written to a temporary file and appear
    /// under their name only when complete.
    pub async fn start(
        config: TftpConfig,
    ) -> Result<(TftpServer, UnboundedReceiver<TftpEvent>), ProbeError> {
        if config.timeout_ms == 0 {
            return Err(ProbeError::invalid(
                "the TFTP timeout must be at least 1 ms",
            ));
        }
        let root = match tokio::fs::canonicalize(&config.root).await {
            Ok(root) if root.is_dir() => root,
            Ok(_) => {
                return Err(ProbeError::invalid(format!(
                    "the TFTP root '{}' is not a directory",
                    config.root.display()
                )));
            }
            Err(error) => {
                return Err(ProbeError::invalid(format!(
                    "the TFTP root '{}' cannot be used: {error}",
                    config.root.display()
                )));
            }
        };
        let socket = UdpSocket::bind(config.bind)
            .await
            .map_err(|error| ProbeError::from_bind(error, "UDP", config.bind))?;
        let local_addr = socket.local_addr()?;
        let (events, receiver) = unbounded_channel();
        let settings = Arc::new(Settings {
            config,
            root,
            events,
        });
        let task = tokio::spawn(serve(socket, settings));
        Ok((
            TftpServer {
                local_addr,
                task: Some(task),
            },
            receiver,
        ))
    }

    /// The address the server listens on (useful after binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops the server and every transfer in progress; returns when its sockets are closed.
    /// Partial uploads are removed.
    pub async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for TftpServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct Settings {
    config: TftpConfig,
    /// The canonical form of `config.root`.
    root: PathBuf,
    events: UnboundedSender<TftpEvent>,
}

/// Receives requests and starts a transfer task for each. Aborting this task drops the set of
/// transfers, which aborts them.
async fn serve(socket: UdpSocket, settings: Arc<Settings>) {
    let mut transfers: JoinSet<()> = JoinSet::new();
    let mut next_id: u64 = 1;
    // A request is small (RFC 2347 limits it to 512 bytes); anything larger is cut and refused.
    let mut buffer = [0u8; 2048];
    // Consecutive receive errors, reset by every datagram that arrives. The
    // only reason to count them is to tell one bad datagram from a socket
    // that has stopped working.
    let mut failures: u32 = 0;
    loop {
        tokio::select! {
            received = socket.recv_from(&mut buffer) => {
                let (length, peer) = match received {
                    Ok(received) => {
                        failures = 0;
                        received
                    }
                    Err(error) => {
                        // An ICMP error about an earlier reply, or a full
                        // buffer. Not fatal, and — this is the part that
                        // matters — not rare either: on Windows an ICMP
                        // port-unreachable for a reply we sent is delivered
                        // as a receive error on this socket, one per reply.
                        // A flood of junk requests is answered with a refusal
                        // each, so a flood of refusals comes back as a flood
                        // of these. Sleeping on each one let a sender stall
                        // the whole server: 13,000 junk datagrams bought ten
                        // seconds of sleep and genuine requests waited behind
                        // it. So carry straight on, and only slow down if the
                        // errors keep coming with nothing arriving in
                        // between, which is the one case a busy loop is a
                        // real risk.
                        failures = failures.saturating_add(1);
                        tracing::debug!(%error, failures, "TFTP receive failed");
                        if failures >= 64 {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        } else {
                            tokio::task::yield_now().await;
                        }
                        continue;
                    }
                };
                let request = match packet::parse(&buffer[..length]) {
                    Ok(Packet::Request(request)) => request,
                    // A transfer packet sent to the request port belongs to no transfer.
                    Ok(Packet::Data { .. } | Packet::Ack { .. }) => {
                        let reply = packet::error(code::UNKNOWN_TRANSFER_ID, "Unknown transfer ID");
                        let _ = socket.send_to(&reply, peer).await;
                        continue;
                    }
                    Err(Malformed::BadRequest(reason)) => {
                        let reply = packet::error(code::ILLEGAL_OPERATION, reason);
                        let _ = socket.send_to(&reply, peer).await;
                        continue;
                    }
                    // Not TFTP at all: answering would only feed whoever sent it.
                    Ok(_) | Err(Malformed::NotTftp) => continue,
                };
                if transfers.len() >= MAX_TRANSFERS {
                    refuse(&socket, peer, &request.filename, &settings, Denial {
                        code: code::NOT_DEFINED,
                        reason: format!("the server is busy with {MAX_TRANSFERS} transfers; try again later"),
                    }).await;
                    continue;
                }
                match prepare(&request, peer, &settings).await {
                    Ok(job) => {
                        let id = next_id;
                        next_id += 1;
                        transfers.spawn(run(job, id, peer, request, settings.clone()));
                    }
                    Err(denial) => refuse(&socket, peer, &request.filename, &settings, denial).await,
                }
            }
            // Reap finished transfers so the set does not grow.
            Some(_) = transfers.join_next() => {}
        }
    }
}

async fn refuse(
    socket: &UdpSocket,
    peer: SocketAddr,
    file: &str,
    settings: &Settings,
    denial: Denial,
) {
    let _ = socket
        .send_to(&packet::error(denial.code, &denial.reason), peer)
        .await;
    let _ = settings.events.send(TftpEvent::Denied {
        peer: peer.to_string(),
        file: crate::util::printable(file, 200),
        reason: denial.reason,
    });
}

/// A request that passed every check and is ready to run.
struct Job {
    socket: UdpSocket,
    terms: Terms,
    kind: JobKind,
}

enum JobKind {
    Send {
        file: tokio::fs::File,
        size: u64,
    },
    Receive {
        target: PathBuf,
        announced: Option<u64>,
    },
}

/// Checks a request against the configuration and the file system and opens what it needs.
async fn prepare(request: &Request, peer: SocketAddr, settings: &Settings) -> Result<Job, Denial> {
    let config = &settings.config;
    let deny = |code, reason: &str| Denial {
        code,
        reason: reason.to_owned(),
    };
    if !matches!(request.mode.as_str(), "octet" | "netascii") {
        return Err(deny(
            code::ILLEGAL_OPERATION,
            "only octet (binary) transfers are supported",
        ));
    }
    let option = |name: &str| {
        request
            .options
            .iter()
            .find(|(option, _)| option == name)
            .map(|(_, value)| value.as_str())
    };

    let mut acknowledged: Vec<(String, String)> = Vec::new();
    let block_size = match option("blksize").and_then(|value| value.parse::<usize>().ok()) {
        // The server may answer with a smaller size than asked, never a larger one.
        Some(wanted) if wanted >= 8 => {
            let granted = wanted.min(MAX_BLOCK_SIZE);
            acknowledged.push(("blksize".to_owned(), granted.to_string()));
            granted
        }
        _ => DEFAULT_BLOCK_SIZE,
    };
    let timeout = match option("timeout").and_then(|value| value.parse::<u64>().ok()) {
        Some(seconds @ 1..=255) => {
            acknowledged.push(("timeout".to_owned(), seconds.to_string()));
            Duration::from_secs(seconds)
        }
        _ => Duration::from_millis(config.timeout_ms),
    };
    let announced = option("tsize").and_then(|value| value.parse::<u64>().ok());

    let kind = if request.write {
        if !config.allow_write {
            return Err(deny(
                code::ACCESS_VIOLATION,
                "uploads are disabled on this server",
            ));
        }
        if let Some(size) = announced {
            if size > config.max_file_bytes {
                return Err(Denial {
                    code: code::DISK_FULL,
                    reason: format!(
                        "the file is {size} bytes; this server accepts at most {} bytes",
                        config.max_file_bytes
                    ),
                });
            }
            acknowledged.push(("tsize".to_owned(), size.to_string()));
        }
        let target =
            path::resolve_write(&settings.root, &request.filename, config.allow_overwrite).await?;
        JobKind::Receive { target, announced }
    } else {
        if !config.allow_read {
            return Err(deny(
                code::ACCESS_VIOLATION,
                "downloads are disabled on this server",
            ));
        }
        let source = path::resolve_read(&settings.root, &request.filename).await?;
        let file = tokio::fs::File::open(&source)
            .await
            .map_err(|error| open_denial(&error))?;
        let size = file
            .metadata()
            .await
            .map_err(|error| open_denial(&error))?
            .len();
        if announced.is_some() {
            acknowledged.push(("tsize".to_owned(), size.to_string()));
        }
        JobKind::Send { file, size }
    };

    // Each transfer talks from its own port (its transfer ID), on the address family of the client.
    let local = match (config.bind.ip(), peer) {
        (ip, _) if !ip.is_unspecified() => SocketAddr::new(ip, 0),
        (_, SocketAddr::V4(_)) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        (_, SocketAddr::V6(_)) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    };
    let unavailable = |error: std::io::Error| Denial {
        code: code::NOT_DEFINED,
        reason: format!("the server could not open a socket for the transfer ({error})"),
    };
    let socket = UdpSocket::bind(local).await.map_err(unavailable)?;
    socket.connect(peer).await.map_err(unavailable)?;
    Ok(Job {
        socket,
        terms: Terms {
            block_size,
            timeout,
            retries: config.retries,
            acknowledged,
        },
        kind,
    })
}

fn open_denial(error: &std::io::Error) -> Denial {
    match error.kind() {
        std::io::ErrorKind::NotFound => Denial {
            code: code::FILE_NOT_FOUND,
            reason: "file not found".to_owned(),
        },
        _ => Denial {
            code: code::ACCESS_VIOLATION,
            reason: format!("the server cannot read the file ({error})"),
        },
    }
}

/// Runs one transfer and reports how it went.
async fn run(job: Job, id: u64, peer: SocketAddr, request: Request, settings: Arc<Settings>) {
    let events = settings.events.clone();
    let started = Instant::now();
    let size = match &job.kind {
        JobKind::Send { size, .. } => Some(*size),
        JobKind::Receive { announced, .. } => *announced,
    };
    let _ = events.send(TftpEvent::Started {
        id,
        peer: peer.to_string(),
        file: relative_name(&request.filename),
        write: request.write,
        size,
    });
    let link = Link {
        socket: job.socket,
        terms: job.terms,
        id,
        events: events.clone(),
    };
    let finished = |bytes| TftpEvent::Finished {
        id,
        bytes,
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    let failed = |failure: transfer::Failure| TftpEvent::Failed {
        id,
        error: failure.0,
    };
    match job.kind {
        JobKind::Send { file, .. } => {
            let outcome = transfer::send_file(&link, file).await;
            let _ = events.send(outcome.map_or_else(failed, finished));
        }
        JobKind::Receive { target, .. } => {
            match transfer::receive_file(&link, &target, settings.config.max_file_bytes).await {
                Ok(received) => {
                    // The file is in place: say so now, then see the client off.
                    let _ = events.send(finished(received.bytes));
                    received.linger(&link).await;
                }
                Err(failure) => {
                    let _ = events.send(failed(failure));
                }
            }
        }
    }
}

/// The requested name with forward slashes and without redundant parts, for display.
fn relative_name(requested: &str) -> String {
    let parts: Vec<&str> = requested
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    Path::new(&parts.join("/")).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn configuration_defaults_and_json() {
        let config = TftpConfig::default();
        assert_eq!(config.bind.port(), 69);
        assert_eq!((config.timeout_ms, config.retries), (3000, 5));
        assert!(config.allow_read && !config.allow_write && !config.allow_overwrite);
        let partial: TftpConfig = serde_json::from_str(
            r#"{"bind": "127.0.0.1:6969", "root": "/srv/tftp", "allow_write": true}"#,
        )
        .unwrap();
        assert_eq!(partial.bind, "127.0.0.1:6969".parse().unwrap());
        assert_eq!(partial.root, PathBuf::from("/srv/tftp"));
        assert!(partial.allow_write);
        assert_eq!(partial.retries, 5);

        let event = TftpEvent::Started {
            id: 1,
            peer: "10.0.0.2:50000".into(),
            file: "sw1.cfg".into(),
            write: true,
            size: None,
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"started":{"id":1,"peer":"10.0.0.2:50000","file":"sw1.cfg","write":true,"size":null}}"#
        );
    }

    #[tokio::test]
    async fn a_missing_or_invalid_root_is_explained() {
        let missing = TftpConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            root: PathBuf::from("/nonexistent/tftp-root"),
            ..TftpConfig::default()
        };
        let error = TftpServer::start(missing).await.unwrap_err();
        assert!(
            matches!(&error, ProbeError::Invalid(text) if text.contains("/nonexistent/tftp-root")),
            "{error:?}"
        );

        let file = tempfile::NamedTempFile::new().unwrap();
        let not_a_directory = TftpConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            root: file.path().to_path_buf(),
            ..TftpConfig::default()
        };
        let error = TftpServer::start(not_a_directory).await.unwrap_err();
        assert!(error.to_string().contains("is not a directory"), "{error}");

        // The default configuration has no root: it must be chosen deliberately.
        let unset = TftpConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..TftpConfig::default()
        };
        assert!(matches!(
            TftpServer::start(unset).await,
            Err(ProbeError::Invalid(_))
        ));
    }

    #[test]
    fn names_are_shown_in_one_form() {
        assert_eq!(
            relative_name("configs\\core//./r1.cfg"),
            "configs/core/r1.cfg"
        );
        assert_eq!(relative_name("sw1.cfg"), "sw1.cfg");
    }
}
