//! TCP connection establishment shared by the SSH and Telnet clients.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::error::TransportError;

/// Idle time before the OS starts probing a silent peer, and the probe period.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// No single address gets less than this, however many there are to try.
const MIN_ATTEMPT: Duration = Duration::from_secs(1);

/// `host:port`, with IPv6 literals bracketed.
pub(crate) fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Resolves `host` and connects to the first address that answers, within
/// `timeout` overall.
pub(crate) async fn connect(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<TcpStream, TransportError> {
    let deadline = Instant::now() + timeout;
    let addresses = resolve(host, port, deadline).await?;

    let mut failure: Option<TransportError> = None;
    for (index, address) in addresses.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Split what is left between the addresses still to try, so that one
        // black-holed address cannot eat the whole budget.
        let share = (remaining / (addresses.len() - index) as u32).max(MIN_ATTEMPT);
        let error =
            match tokio::time::timeout(share.min(remaining), TcpStream::connect(address)).await {
                Ok(Ok(stream)) => {
                    tune(&stream);
                    return Ok(stream);
                }
                Ok(Err(error)) => TransportError::from_connect(error),
                Err(_) => TransportError::ConnectTimeout,
            };
        tracing::debug!(%address, %error, "connection attempt failed");
        if failure
            .as_ref()
            .is_none_or(|kept| rank(&error) >= rank(kept))
        {
            failure = Some(error);
        }
    }
    Err(failure.unwrap_or(TransportError::ConnectTimeout))
}

async fn resolve(
    host: &str,
    port: u16,
    deadline: Instant,
) -> Result<Vec<SocketAddr>, TransportError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(TransportError::Resolve("no host given".to_owned()));
    }
    // `lookup_host` wants bare IPv6 literals.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let lookup = tokio::net::lookup_host((bare, port));
    let addresses: Vec<SocketAddr> = match tokio::time::timeout_at(deadline, lookup).await {
        Ok(Ok(addresses)) => addresses.collect(),
        Ok(Err(error)) => return Err(TransportError::Resolve(format!("{host}: {error}"))),
        Err(_) => {
            return Err(TransportError::Resolve(format!(
                "{host}: name lookup timed out"
            )));
        }
    };
    if addresses.is_empty() {
        return Err(TransportError::Resolve(format!("{host}: no address found")));
    }
    Ok(addresses)
}

/// When several addresses fail differently, report the most telling failure.
fn rank(error: &TransportError) -> u8 {
    match error {
        TransportError::Refused => 3,
        TransportError::ConnectTimeout => 2,
        TransportError::Unreachable(_) => 1,
        _ => 0,
    }
}

/// Interactive traffic: no Nagle delay, and let the OS notice dead peers.
fn tune(stream: &TcpStream) {
    if let Err(error) = stream.set_nodelay(true) {
        tracing::debug!(%error, "cannot disable Nagle's algorithm");
    }
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL);
    if let Err(error) = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive) {
        tracing::debug!(%error, "cannot enable TCP keep-alive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_brackets_ipv6() {
        assert_eq!(host_port("10.0.0.1", 22), "10.0.0.1:22");
        assert_eq!(host_port("router.lab", 2222), "router.lab:2222");
        assert_eq!(host_port("fe80::1", 22), "[fe80::1]:22");
        assert_eq!(host_port("[::1]", 22), "[::1]:22");
    }

    #[tokio::test]
    async fn connects_to_a_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = connect("127.0.0.1", port, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(stream.nodelay().unwrap());
    }

    #[tokio::test]
    async fn ipv6_literals_work_bare_and_bracketed() {
        let Ok(listener) = tokio::net::TcpListener::bind("[::1]:0").await else {
            return; // no IPv6 on this machine
        };
        let port = listener.local_addr().unwrap().port();
        for host in ["::1", "[::1]"] {
            let stream = connect(host, port, Duration::from_secs(5)).await.unwrap();
            assert!(stream.peer_addr().unwrap().is_ipv6(), "{host}");
        }
    }

    #[tokio::test]
    async fn refused_is_reported_as_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = connect("127.0.0.1", port, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "refused");
    }

    #[tokio::test]
    async fn bad_names_are_resolve_errors() {
        let error = connect("", 22, Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(error.code(), "resolve");
        let error = connect("no-such-host.invalid", 22, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "resolve");
    }
}
