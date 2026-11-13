//! What a caller decides about a server: where it listens, who may talk to
//! it, and how it is secured.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::token;

/// The port Nocturne listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7878;

/// How [`serve`](crate::serve) should run.
#[derive(Clone)]
pub struct ServerOptions {
    /// Address to listen on. Port `0` picks a free port; read the real one
    /// from [`ServerHandle::local_addr`](crate::ServerHandle::local_addr).
    pub bind: SocketAddr,
    /// The bearer token every API request must present.
    pub token: String,
    /// Serve HTTPS with this certificate instead of plain HTTP.
    pub tls: Option<Tls>,
    /// Accept plain HTTP on an address other machines can reach.
    pub allow_insecure_http: bool,
    /// Serve the web UI from this directory instead of the embedded one.
    pub ui_dir: Option<PathBuf>,
}

impl Default for ServerOptions {
    /// Loopback on port 7878, plain HTTP, the embedded UI and a freshly
    /// generated random token (never an empty or a well-known one).
    fn default() -> Self {
        ServerOptions {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT),
            token: token::generate(),
            tls: None,
            allow_insecure_http: false,
            ui_dir: None,
        }
    }
}

impl ServerOptions {
    /// Check everything that can be checked without touching the network or
    /// the disk. [`serve`](crate::serve) calls this first; front ends call it
    /// earlier to fail before they start the application.
    pub fn validate(&self) -> anyhow::Result<()> {
        token::validate(&self.token).context("the access token cannot be used")?;
        if self.tls.is_none() && !is_loopback(self.bind.ip()) && !self.allow_insecure_http {
            bail!(
                "refusing to serve plain HTTP on {}: the access token and every device password \
                 typed into the UI would cross the network in clear text. Serve HTTPS \
                 (--tls-cert with --tls-key, or --tls-self-signed), bind to 127.0.0.1 and reach \
                 it through an SSH tunnel, or pass --allow-insecure-http to accept the risk",
                self.bind
            );
        }
        Ok(())
    }
}

// The token is a secret: keep it out of anything that formats the options.
impl fmt::Debug for ServerOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerOptions")
            .field("bind", &self.bind)
            .field("token", &"<redacted>")
            .field("tls", &self.tls)
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field("ui_dir", &self.ui_dir)
            .finish()
    }
}

/// A TLS certificate chain and its private key, both PEM.
#[derive(Clone)]
pub enum Tls {
    /// Read from files when the server starts.
    Files { cert: PathBuf, key: PathBuf },
    /// Already in memory.
    Pem { cert: String, key: String },
}

impl fmt::Debug for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tls::Files { cert, key } => f
                .debug_struct("Files")
                .field("cert", cert)
                .field("key", key)
                .finish(),
            Tls::Pem { .. } => f
                .debug_struct("Pem")
                .field("cert", &"<pem>")
                .field("key", &"<redacted>")
                .finish(),
        }
    }
}

/// Protocol timings. The defaults are the documented behaviour; other values
/// exist for tests that cannot wait a minute for an idle client to be dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// How often the server pings every WebSocket client.
    pub ws_ping_interval: Duration,
    /// A WebSocket client that sends nothing (not even a pong) for this long is dropped.
    pub ws_idle_timeout: Duration,
    /// How long a client may take to complete the TLS handshake and to send a
    /// complete request head.
    pub header_read_timeout: Duration,
    /// How long [`ServerHandle::shutdown`](crate::ServerHandle::shutdown) waits
    /// for requests that are still running.
    pub shutdown_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            ws_ping_interval: Duration::from_secs(20),
            ws_idle_timeout: Duration::from_secs(60),
            header_read_timeout: Duration::from_secs(15),
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

/// Whether `ip` can only be reached from this machine. `::ffff:127.0.0.1` counts.
pub(crate) fn is_loopback(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(bind: &str) -> ServerOptions {
        ServerOptions {
            bind: bind.parse().unwrap(),
            ..ServerOptions::default()
        }
    }

    #[test]
    fn defaults_match_the_spec() {
        let options = ServerOptions::default();
        assert_eq!(options.bind.to_string(), "127.0.0.1:7878");
        assert!(options.tls.is_none());
        assert!(!options.allow_insecure_http);
        assert!(options.ui_dir.is_none());
        // 32 random bytes in hex, different every time.
        assert_eq!(options.token.len(), 64);
        assert_ne!(options.token, ServerOptions::default().token);

        let timing = Timing::default();
        assert_eq!(timing.ws_ping_interval, Duration::from_secs(20));
        assert_eq!(timing.ws_idle_timeout, Duration::from_secs(60));
        assert_eq!(timing.header_read_timeout, Duration::from_secs(15));
        assert_eq!(timing.shutdown_grace, Duration::from_secs(5));
    }

    #[test]
    fn plain_http_is_only_accepted_on_loopback() {
        for loopback in [
            "127.0.0.1:0",
            "127.8.9.10:0",
            "[::1]:0",
            "[::ffff:127.0.0.1]:0",
        ] {
            options(loopback).validate().unwrap();
        }
        for exposed in [
            "0.0.0.0:0",
            "[::]:0",
            "192.0.2.10:7878",
            "[2001:db8::1]:7878",
        ] {
            let err = options(exposed).validate().unwrap_err().to_string();
            assert!(err.contains("refusing to serve plain HTTP"), "{err}");
            assert!(err.contains("--allow-insecure-http"), "{err}");

            let mut allowed = options(exposed);
            allowed.allow_insecure_http = true;
            allowed.validate().unwrap();

            let mut tls = options(exposed);
            tls.tls = Some(Tls::Pem {
                cert: String::new(),
                key: String::new(),
            });
            tls.validate().unwrap();
        }
    }

    #[test]
    fn an_unusable_token_is_refused() {
        let mut empty = options("127.0.0.1:0");
        empty.token = String::new();
        assert!(empty.validate().is_err());
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let mut options = options("127.0.0.1:0");
        options.token = "s3cr3t-token-value".into();
        options.tls = Some(Tls::Pem {
            cert: "CERT-PEM".into(),
            key: "PRIVATE-KEY-PEM".into(),
        });
        let printed = format!("{options:?}");
        assert!(!printed.contains("s3cr3t-token-value"), "{printed}");
        assert!(!printed.contains("PRIVATE-KEY-PEM"), "{printed}");
        assert!(printed.contains("127.0.0.1:0"), "{printed}");
    }
}
