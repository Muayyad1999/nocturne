//! The routes, and the layers every request passes through.

use std::any::Any;

use axum::Router;
use axum::extract::{Request, State};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use tower_http::catch_panic::CatchPanicLayer;

use crate::envelope::ApiError;
use crate::state::Shared;
use crate::text::printable;
use crate::{auth, events, guard, headers, logging, rpc, ui};

/// Whether `path` belongs to the API rather than to the web UI.
pub(crate) fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The complete application as one service.
pub(crate) fn build(state: Shared) -> Router {
    protect(routes(&state), state)
}

/// Handlers check the HTTP method themselves (`any`), so that a request with
/// the wrong method still has to pass authentication first.
fn routes(state: &Shared) -> Router<Shared> {
    let token_in_header = middleware::from_fn_with_state(state.clone(), auth::require_header);
    let token_in_header_or_query =
        middleware::from_fn_with_state(state.clone(), auth::require_header_or_query);

    Router::new()
        .route("/api/health", any(rpc::health))
        .route("/api/info", any(rpc::info).layer(token_in_header.clone()))
        .route("/api/rpc", any(rpc::call).layer(token_in_header))
        .route(
            "/api/events",
            any(events::connect).layer(token_in_header_or_query),
        )
        .fallback(fallback)
}

/// Wrap `routes` in the layers that apply to every request. A request passes
/// them from the last one listed to the first.
fn protect(routes: Router<Shared>, state: Shared) -> Router {
    routes
        .layer(middleware::from_fn_with_state(
            state.clone(),
            guard::enforce,
        ))
        .layer(CatchPanicLayer::custom(panic_response))
        .layer(middleware::from_fn(logging::log_request))
        .layer(middleware::from_fn(headers::secure))
        .with_state(state)
}

/// Everything that is not an API route: an unknown API path, or the web UI.
async fn fallback(State(state): State<Shared>, request: Request) -> Response {
    let path = request.uri().path();
    if is_api_path(path) {
        return ApiError::not_found(format!(
            "there is no API endpoint at `{}`",
            printable(path, 100)
        ))
        .into_response();
    }
    ui::serve(&state.ui, request).await
}

/// A handler panicked: answer `500` instead of dropping the connection.
fn panic_response(panic: Box<dyn Any + Send + 'static>) -> Response {
    let detail = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("(no message)");
    tracing::error!(panic = detail, "a request handler panicked");
    // The panic text stays in the log: it may quote data the client should not see.
    ApiError::internal(
        "the server hit a bug while handling this request; the server log has the details",
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{OriginPolicy, Scheme};
    use crate::options::Timing;
    use crate::state::ServerState;
    use crate::ui::UiSource;
    use axum::body::{Body, Bytes, to_bytes};
    use axum::http::{HeaderMap, Method, StatusCode};
    use axum::routing::get;
    use nocturne_suite::AppConfig;
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    const TOKEN: &str = "unit-test-token";
    const HOST: &str = "127.0.0.1:7878";

    fn state() -> Shared {
        ServerState::new(
            nocturne_suite::start(AppConfig::in_memory()).unwrap(),
            TOKEN,
            OriginPolicy::new(Scheme::Http, HOST.parse().unwrap()),
            UiSource::fallback(),
            Timing::default(),
            CancellationToken::new(),
        )
    }

    fn request(method: Method, path: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", HOST)
    }

    async fn send(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
        let response = router.clone().oneshot(request).await.unwrap();
        let (head, body) = response.into_parts();
        (
            head.status,
            head.headers,
            to_bytes(body, usize::MAX).await.unwrap(),
        )
    }

    async fn get_path(router: &Router, path: &str) -> (StatusCode, HeaderMap, Bytes) {
        send(
            router,
            request(Method::GET, path).body(Body::empty()).unwrap(),
        )
        .await
    }

    fn json_of(body: &Bytes) -> Value {
        serde_json::from_slice(body).unwrap_or_else(|_| panic!("not JSON: {body:?}"))
    }

    fn assert_security_headers(headers: &HeaderMap) {
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(headers["cross-origin-opener-policy"], "same-origin");
        assert!(
            !headers
                .keys()
                .any(|name| name.as_str().starts_with("access-control-")),
            "no CORS headers at all: {headers:?}"
        );
    }

    #[test]
    fn api_paths() {
        for api in ["/api", "/api/", "/api/rpc", "/api/anything/else"] {
            assert!(is_api_path(api), "{api}");
        }
        for ui in [
            "/",
            "/apiary",
            "/api.html",
            "/assets/api/x.js",
            "/devices/api",
        ] {
            assert!(!is_api_path(ui), "{ui}");
        }
    }

    #[tokio::test]
    async fn without_an_embedded_ui_every_page_is_the_fallback_page() {
        let router = build(state());
        for path in ["/", "/index.html", "/devices"] {
            let (status, headers, body) = get_path(&router, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(headers["content-type"], "text/html; charset=utf-8");
            assert_eq!(headers["cache-control"], "no-cache");
            assert_eq!(
                headers["content-security-policy"],
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                 img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' ws: wss:; \
                 frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
            );
            assert_security_headers(&headers);
            let page = String::from_utf8(body.to_vec()).unwrap();
            assert!(
                page.contains("was not built into this binary")
                    && page.contains("pnpm --dir ui build"),
                "{page}"
            );
        }
        let (status, headers, _) = get_path(&router, "/assets/app.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_security_headers(&headers);
        let (status, headers, _) = get_path(&router, "/devices/42").await;
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(headers["location"], "/");
    }

    #[tokio::test]
    async fn unknown_api_paths_are_404_with_the_envelope_and_never_the_page() {
        let router = build(state());
        for path in [
            "/api",
            "/api/",
            "/api/nope",
            "/api/rpc/extra",
            "/api/health/",
        ] {
            let (status, headers, body) = get_path(&router, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(headers["content-type"], "application/json");
            assert_eq!(headers["cache-control"], "no-store");
            assert!(!headers.contains_key("content-security-policy"));
            assert_security_headers(&headers);
            let body = json_of(&body);
            assert_eq!(body["ok"], false);
            assert_eq!(body["error"]["code"], "not_found");
        }
    }

    #[tokio::test]
    async fn a_panicking_handler_becomes_a_500_and_the_service_keeps_working() {
        async fn boom() -> &'static str {
            panic!("secret detail that must stay in the log")
        }
        let state = state();
        let router = protect(routes(&state).route("/api/boom", get(boom)), state);

        let (status, headers, body) = get_path(&router, "/api/boom").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_security_headers(&headers);
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains("secret detail"), "{text}");
        let body = json_of(&body);
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "internal");

        let (status, _, body) = get_path(&router, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json_of(&body)["status"], "ok");
    }

    #[tokio::test]
    async fn authentication_comes_before_the_method_check() {
        let router = build(state());
        let (status, headers, _) = get_path(&router, "/api/rpc").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers["www-authenticate"], "Bearer");

        let authorized = request(Method::GET, "/api/rpc")
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let (status, headers, body) = send(&router, authorized).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(headers["allow"], "POST");
        assert_eq!(json_of(&body)["error"]["code"], "invalid");

        for (method, path, allow) in [
            (Method::POST, "/api/health", "GET, HEAD"),
            (Method::DELETE, "/", "GET, HEAD"),
            (Method::POST, "/devices", "GET, HEAD"),
        ] {
            let (status, headers, _) =
                send(&router, request(method, path).body(Body::empty()).unwrap()).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{path}");
            assert_eq!(headers["allow"], allow, "{path}");
        }
    }

    #[tokio::test]
    async fn host_and_origin_are_checked_before_the_token() {
        let router = build(state());
        // Wrong host and no token: the host decides.
        let foreign_host = Request::builder()
            .uri("/api/rpc")
            .method(Method::POST)
            .header("host", "evil.example:7878")
            .body(Body::empty())
            .unwrap();
        let (status, headers, body) = send(&router, foreign_host).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json_of(&body)["error"]["code"], "forbidden");
        assert_security_headers(&headers);

        // Right host, valid token, foreign origin: still refused.
        let foreign_origin = request(Method::POST, "/api/rpc")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("origin", "http://evil.example")
            .body(Body::from(r#"{"method": "devices.list"}"#))
            .unwrap();
        let (status, _, body) = send(&router, foreign_origin).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json_of(&body)["error"]["code"], "forbidden");

        // The same call from the server's own page goes through.
        let same_origin = request(Method::POST, "/api/rpc")
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("origin", format!("http://{HOST}"))
            .body(Body::from(r#"{"method": "devices.list"}"#))
            .unwrap();
        let (status, headers, body) = send(&router, same_origin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(json_of(&body), json!({"ok": true, "result": []}));
    }

    #[tokio::test]
    async fn head_requests_get_headers_only() {
        let router = build(state());
        for path in ["/api/health", "/"] {
            let (status, headers, body) = send(
                &router,
                request(Method::HEAD, path).body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.is_empty(), "{path}");
            assert_security_headers(&headers);
        }
    }
}
