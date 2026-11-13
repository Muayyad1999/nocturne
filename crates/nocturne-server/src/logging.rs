//! One log line per request.
//!
//! A line shows the HTTP method, the path without its query (the event
//! stream's query carries the token), the status, the duration and, for RPC
//! calls, the method name. Headers, parameters and results are never logged.

use std::net::SocketAddr;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request};
use axum::middleware::Next;
use axum::response::Response;

use crate::router::is_api_path;
use crate::text::printable;

/// Set on the reply to an RPC call so the log line can name the method.
#[derive(Clone)]
pub(crate) struct RpcMethod(pub(crate) String);

/// Middleware: log the request once its response is known.
pub(crate) async fn log_request(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let client = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip().to_string());

    let response = next.run(request).await;

    let status = response.status().as_u16();
    let ms = started.elapsed().as_millis() as u64;
    let rpc = response
        .extensions()
        .get::<RpcMethod>()
        .map(|RpcMethod(name)| printable(name, 96));

    // The level is the only thing that differs, and tracing wants it as a literal.
    // `rpc` and `client` appear in the line only when they are known.
    macro_rules! line {
        ($level:ident) => {
            tracing::$level!(
                %method,
                path,
                status,
                ms,
                rpc = rpc.as_deref(),
                client = client.as_deref(),
                "request"
            )
        };
    }
    match status {
        500.. => line!(error),
        // Somebody is knocking with the wrong token, host or origin.
        401 | 403 => line!(warn),
        _ if is_api_path(&path) && path != "/api/health" => line!(info),
        _ => line!(debug),
    }
    response
}
