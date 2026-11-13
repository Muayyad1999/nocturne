//! Reaching the target: directly, or hop by hop through jump hosts
//! (`ProxyJump`), each hop authenticated on its own and the next one reached
//! through a `direct-tcpip` channel of the previous.

use std::net::Shutdown;
use std::sync::Arc;

use tokio::net::TcpStream;

use super::algorithms::CryptoPolicy;
use super::client::{ConnectOptions, Endpoint, Established, SessionInner, establish, release};
use super::{LegacyCrypto, MAX_JUMP_HOSTS, SshParams};
use crate::error::TransportError;
use crate::net::{self, host_port};

/// Connects to the target of `params` through its jump hosts.
///
/// Returns the target session and the hops traversed (`host:port`).
pub(crate) async fn open_chain(
    params: &SshParams,
    options: &ConnectOptions,
) -> Result<(Established, Vec<String>), TransportError> {
    if params.jump.len() > MAX_JUMP_HOSTS {
        return Err(TransportError::Unsupported(format!(
            "{} jump hosts were given; at most {MAX_JUMP_HOSTS} are supported",
            params.jump.len()
        )));
    }

    let mut previous: Option<Arc<SessionInner>> = None;
    let mut via = Vec::with_capacity(params.jump.len());
    for hop in &params.jump {
        let endpoint = Endpoint {
            host: &hop.host,
            port: hop.port,
            username: &hop.username,
            auth: &hop.auth,
        };
        let label = host_port(&hop.host, hop.port);
        let established = connect(previous.take(), &endpoint, options)
            .await
            .map_err(|error| error.with_context(&format!("jump host {label}")))?;
        via.push(label);
        previous = Some(established.session);
    }

    let target = Endpoint {
        host: &params.host,
        port: params.port,
        username: &params.username,
        auth: &params.auth,
    };
    let established = connect(previous, &target, options).await?;
    Ok((established, via))
}

/// Connects to one endpoint, applying the legacy-crypto policy.
async fn connect(
    previous: Option<Arc<SessionInner>>,
    endpoint: &Endpoint<'_>,
    options: &ConnectOptions,
) -> Result<Established, TransportError> {
    let hop = || previous.clone();
    let result = match options.legacy {
        LegacyCrypto::Never => attempt(hop(), endpoint, CryptoPolicy::Modern, options).await,
        LegacyCrypto::Always => attempt(hop(), endpoint, CryptoPolicy::Legacy, options).await,
        LegacyCrypto::Auto => {
            match attempt(hop(), endpoint, CryptoPolicy::Modern, options).await {
                Err(TransportError::NoCommonAlgorithm(reason)) => {
                    tracing::info!(
                        host = endpoint.host,
                        port = endpoint.port,
                        %reason,
                        "retrying with legacy algorithms"
                    );
                    attempt(hop(), endpoint, CryptoPolicy::Legacy, options).await
                }
                other => other,
            }
        }
    };
    if result.is_err()
        && let Some(hop) = previous
    {
        // Nothing will use the hops we got through: hang up politely.
        release(hop).await;
    }
    result
}

/// One connection attempt with a fixed algorithm set.
async fn attempt(
    previous: Option<Arc<SessionInner>>,
    endpoint: &Endpoint<'_>,
    policy: CryptoPolicy,
    options: &ConnectOptions,
) -> Result<Established, TransportError> {
    match previous {
        Some(hop) => {
            let tunnel = hop
                .open_tunnel(endpoint.host, endpoint.port, options.timeout)
                .await?;
            establish(tunnel, endpoint, policy, options, Some(hop)).await
        }
        None => {
            let stream = net::connect(endpoint.host, endpoint.port, options.timeout).await?;
            let (stream, kill_switch) = with_kill_switch(stream)?;
            let result = establish(stream, endpoint, policy, options, None).await;
            if result.is_err() {
                // A handshake that was given up on leaves russh's session task
                // waiting on the socket; closing it lets the task end.
                let _ = kill_switch.shutdown(Shutdown::Both);
            }
            result
        }
    }
}

/// Splits off a second handle to the socket with which it can be shut down
/// after it has been handed to russh.
fn with_kill_switch(stream: TcpStream) -> Result<(TcpStream, std::net::TcpStream), TransportError> {
    let stream = stream.into_std()?;
    let kill_switch = stream.try_clone()?;
    Ok((TcpStream::from_std(stream)?, kill_switch))
}
