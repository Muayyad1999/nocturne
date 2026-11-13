//! Security headers that go on every response.

use axum::extract::Request;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

use crate::router::is_api_path;

const CROSS_ORIGIN_OPENER_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-opener-policy");

/// What a page served by this server may load and connect to.
const CONTENT_SECURITY_POLICY_VALUE: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; \
     connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// Middleware: add the headers to whatever the inner layers answered.
pub(crate) async fn secure(request: Request, next: Next) -> Response {
    let api = is_api_path(request.uri().path());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        CROSS_ORIGIN_OPENER_POLICY,
        HeaderValue::from_static("same-origin"),
    );

    let is_html = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("text/html")
        });
    if is_html {
        headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY_VALUE),
        );
    }
    // API replies describe the network and must not linger in any cache.
    if api && !headers.contains_key(CACHE_CONTROL) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}
