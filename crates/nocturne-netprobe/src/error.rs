//! The crate's single error type.

use std::io;

/// Why a probe or a server could not do what was asked.
///
/// The messages are written for the person running the tool: they say what went wrong and, where
/// it is known, what to do about it.
#[derive(thiserror::Error, Debug)]
pub enum ProbeError {
    /// The host name does not resolve to any address.
    #[error("the host name could not be resolved (check the spelling and the DNS settings)")]
    Resolve,
    /// Nothing answered within the timeout.
    #[error("timed out waiting for a reply")]
    Timeout,
    /// The network reported that the destination cannot be reached; the text is the reason
    /// (for example the ICMP message and the router that sent it).
    #[error("destination unreachable: {0}")]
    Unreachable(String),
    /// The destination actively refused the connection (TCP reset / ICMP port unreachable).
    #[error("connection refused by the destination")]
    Refused,
    /// The operating system did not allow the operation.
    #[error("permission denied: {0}")]
    Permission(String),
    /// The operation is not available on this machine or for this input.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// The caller's input is wrong; the text says which part and what is expected.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A local socket, file or process operation failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The peer answered, but not with what the protocol requires.
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl ProbeError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        ProbeError::Invalid(message.into())
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        ProbeError::Protocol(message.into())
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        ProbeError::Unsupported(message.into())
    }

    /// Classifies an error from `connect`, `send` or `recv` towards a remote host.
    pub(crate) fn from_network_io(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::TimedOut => ProbeError::Timeout,
            io::ErrorKind::ConnectionRefused => ProbeError::Refused,
            io::ErrorKind::HostUnreachable => {
                ProbeError::Unreachable("no route to the host".to_owned())
            }
            io::ErrorKind::NetworkUnreachable => {
                ProbeError::Unreachable("the network is unreachable from this machine".to_owned())
            }
            io::ErrorKind::PermissionDenied => ProbeError::Permission(format!(
                "the operating system refused to send to this destination ({error}); \
                 a local firewall rule or a broadcast address is the usual cause"
            )),
            _ => ProbeError::Io(error),
        }
    }

    /// Explains why a server socket could not be bound.
    pub(crate) fn from_bind(error: io::Error, what: &str, addr: std::net::SocketAddr) -> Self {
        match error.kind() {
            io::ErrorKind::PermissionDenied => ProbeError::Permission(format!(
                "cannot listen on {what} {addr}: ports below 1024 need elevated rights. \
                 Choose a port of 1024 or above, or on Linux grant the program \
                 CAP_NET_BIND_SERVICE (sudo setcap cap_net_bind_service=+ep <program>)"
            )),
            io::ErrorKind::AddrInUse => ProbeError::Io(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("cannot listen on {what} {addr}: another program is already using it"),
            )),
            io::ErrorKind::AddrNotAvailable => ProbeError::Invalid(format!(
                "cannot listen on {what} {addr}: that address is not configured on this machine"
            )),
            _ => ProbeError::Io(io::Error::new(
                error.kind(),
                format!("cannot listen on {what} {addr}: {error}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_errors_are_classified() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert!(matches!(
            ProbeError::from_network_io(refused),
            ProbeError::Refused
        ));
        let timed_out = io::Error::from(io::ErrorKind::TimedOut);
        assert!(matches!(
            ProbeError::from_network_io(timed_out),
            ProbeError::Timeout
        ));
        let unreachable = io::Error::from(io::ErrorKind::NetworkUnreachable);
        assert!(matches!(
            ProbeError::from_network_io(unreachable),
            ProbeError::Unreachable(_)
        ));
    }

    #[test]
    fn privileged_bind_failure_tells_the_user_what_to_do() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let error = ProbeError::from_bind(denied, "UDP", "0.0.0.0:69".parse().unwrap());
        let ProbeError::Permission(message) = error else {
            panic!("expected Permission, got {error:?}");
        };
        assert!(message.contains("0.0.0.0:69"), "{message}");
        assert!(message.contains("1024"), "{message}");
        assert!(message.contains("CAP_NET_BIND_SERVICE"), "{message}");
    }

    #[test]
    fn busy_port_names_the_address() {
        let busy = io::Error::from(io::ErrorKind::AddrInUse);
        let error = ProbeError::from_bind(busy, "UDP", "127.0.0.1:5514".parse().unwrap());
        assert!(error.to_string().contains("127.0.0.1:5514"));
        assert!(error.to_string().contains("already using it"));
    }
}
