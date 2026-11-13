//! The error type shared by every transport.

use std::io;

/// Why a transport operation failed.
///
/// [`TransportError::code`] gives a stable machine-readable identifier for the
/// UI; the `Display` text is meant for humans.
#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    /// The host name could not be resolved.
    #[error("cannot resolve host: {0}")]
    Resolve(String),
    /// The TCP connection (or the protocol handshake) did not complete in time.
    #[error("connection attempt timed out")]
    ConnectTimeout,
    /// The peer actively refused the connection.
    #[error("connection refused")]
    Refused,
    /// No route to the peer.
    #[error("host unreachable: {0}")]
    Unreachable(String),
    /// The [`HostKeyVerifier`](crate::HostKeyVerifier) rejected the server key.
    #[error("host key rejected: {0}")]
    HostKeyRejected(String),
    /// Every configured authentication method was refused.
    #[error("authentication failed{}", auth_detail(.tried, .server_methods))]
    AuthFailed {
        /// What was attempted, in order (skipped methods carry a note).
        tried: Vec<String>,
        /// The methods the server said it would accept.
        server_methods: Vec<String>,
    },
    /// Client and server share no algorithm; the message lists what the
    /// server offered.
    #[error("no common algorithm: {0}")]
    NoCommonAlgorithm(String),
    /// The peer violated the protocol or refused a request.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// An operating-system I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The connection is closed.
    #[error("connection closed")]
    Closed,
    /// An operation on an established connection timed out.
    #[error("operation timed out")]
    Timeout,
    /// The request cannot be served by this build or platform.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl TransportError {
    /// Stable machine code for the UI: `"resolve"`, `"connect_timeout"`,
    /// `"refused"`, `"unreachable"`, `"host_key"`, `"auth"`, `"algorithms"`,
    /// `"protocol"`, `"io"`, `"closed"`, `"timeout"`, `"unsupported"`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Resolve(_) => "resolve",
            Self::ConnectTimeout => "connect_timeout",
            Self::Refused => "refused",
            Self::Unreachable(_) => "unreachable",
            Self::HostKeyRejected(_) => "host_key",
            Self::AuthFailed { .. } => "auth",
            Self::NoCommonAlgorithm(_) => "algorithms",
            Self::Protocol(_) => "protocol",
            Self::Io(_) => "io",
            Self::Closed => "closed",
            Self::Timeout => "timeout",
            Self::Unsupported(_) => "unsupported",
        }
    }

    /// Classifies an I/O error raised while establishing a TCP connection.
    pub(crate) fn from_connect(error: io::Error) -> Self {
        use io::ErrorKind::{
            AddrNotAvailable, ConnectionRefused, HostUnreachable, NetworkUnreachable, TimedOut,
        };
        match error.kind() {
            ConnectionRefused => Self::Refused,
            TimedOut => Self::ConnectTimeout,
            HostUnreachable | NetworkUnreachable | AddrNotAvailable => {
                Self::Unreachable(error.to_string())
            }
            _ => Self::Io(error),
        }
    }

    /// Prefixes the human-readable part of the error with `context`, keeping
    /// the variant (and therefore [`code`](Self::code)) intact.
    pub(crate) fn with_context(self, context: &str) -> Self {
        let prefix = |message: String| format!("{context}: {message}");
        match self {
            Self::Resolve(m) => Self::Resolve(prefix(m)),
            Self::Unreachable(m) => Self::Unreachable(prefix(m)),
            Self::HostKeyRejected(m) => Self::HostKeyRejected(prefix(m)),
            Self::NoCommonAlgorithm(m) => Self::NoCommonAlgorithm(prefix(m)),
            Self::Protocol(m) => Self::Protocol(prefix(m)),
            Self::Unsupported(m) => Self::Unsupported(prefix(m)),
            Self::AuthFailed {
                mut tried,
                server_methods,
            } => {
                tried.insert(0, format!("[{context}]"));
                Self::AuthFailed {
                    tried,
                    server_methods,
                }
            }
            Self::Io(e) => Self::Io(io::Error::new(e.kind(), prefix(e.to_string()))),
            unit @ (Self::ConnectTimeout | Self::Refused | Self::Closed | Self::Timeout) => unit,
        }
    }
}

fn auth_detail(tried: &[String], server_methods: &[String]) -> String {
    let list = |items: &[String]| {
        if items.is_empty() {
            "nothing".to_owned()
        } else {
            items.join(", ")
        }
    };
    format!(
        " (tried: {}; server accepts: {})",
        list(tried),
        list(server_methods)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        let cases: Vec<(TransportError, &str)> = vec![
            (TransportError::Resolve("x".into()), "resolve"),
            (TransportError::ConnectTimeout, "connect_timeout"),
            (TransportError::Refused, "refused"),
            (TransportError::Unreachable("x".into()), "unreachable"),
            (TransportError::HostKeyRejected("x".into()), "host_key"),
            (
                TransportError::AuthFailed {
                    tried: vec![],
                    server_methods: vec![],
                },
                "auth",
            ),
            (TransportError::NoCommonAlgorithm("x".into()), "algorithms"),
            (TransportError::Protocol("x".into()), "protocol"),
            (TransportError::Io(io::Error::other("x")), "io"),
            (TransportError::Closed, "closed"),
            (TransportError::Timeout, "timeout"),
            (TransportError::Unsupported("x".into()), "unsupported"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code, "{error}");
        }
    }

    #[test]
    fn auth_failure_lists_what_was_tried_and_offered() {
        let error = TransportError::AuthFailed {
            tried: vec!["none".into(), "password".into()],
            server_methods: vec!["publickey".into()],
        };
        assert_eq!(
            error.to_string(),
            "authentication failed (tried: none, password; server accepts: publickey)"
        );
    }

    #[test]
    fn connect_errors_are_classified() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(TransportError::from_connect(refused).code(), "refused");
        let timeout = io::Error::from(io::ErrorKind::TimedOut);
        assert_eq!(
            TransportError::from_connect(timeout).code(),
            "connect_timeout"
        );
        let unreachable = io::Error::from(io::ErrorKind::HostUnreachable);
        assert_eq!(
            TransportError::from_connect(unreachable).code(),
            "unreachable"
        );
        let other = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(TransportError::from_connect(other).code(), "io");
    }

    #[test]
    fn context_keeps_the_variant() {
        let error = TransportError::Refused.with_context("jump host a:22");
        assert_eq!(error.code(), "refused");
        let error = TransportError::Protocol("boom".into()).with_context("jump host a:22");
        assert_eq!(error.to_string(), "protocol error: jump host a:22: boom");
        let error = TransportError::AuthFailed {
            tried: vec!["password".into()],
            server_methods: vec![],
        }
        .with_context("jump host a:22");
        assert!(error.to_string().contains("[jump host a:22], password"));
    }
}
