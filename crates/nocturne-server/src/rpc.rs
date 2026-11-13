//! `POST /api/rpc`, plus the two small status routes.

use axum::Json;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_LENGTH, EXPECT};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::VERSION;
use crate::envelope::{self, ApiError};
use crate::logging::RpcMethod;
use crate::state::Shared;
use crate::text::printable;

/// Largest request body the RPC endpoint accepts.
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// A client that is still uploading when the reply arrives usually reports a
/// broken connection instead of the status. So a body that is too large is
/// read to its end and thrown away before `413` is sent, up to this much.
const MAX_DISCARDED_BYTES: usize = 4 * MAX_BODY_BYTES;

const EXAMPLE: &str = r#"{"method": "devices.list", "params": {}}"#;

/// `GET /api/health`: is the process up. Deliberately says nothing else.
pub(crate) async fn health(method: Method) -> Response {
    match read_only(&method) {
        Ok(()) => Json(json!({"status": "ok", "version": VERSION})).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// `GET /api/info`: the version and the RPC methods this build offers.
pub(crate) async fn info(State(state): State<Shared>, method: Method) -> Response {
    match read_only(&method) {
        Ok(()) => Json(json!({"version": VERSION, "methods": state.app.methods()})).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

fn read_only(method: &Method) -> Result<(), ApiError> {
    if matches!(*method, Method::GET | Method::HEAD) {
        Ok(())
    } else {
        Err(ApiError::method_not_allowed(
            "GET, HEAD",
            format!("this endpoint is read with GET, not {method}"),
        ))
    }
}

/// `POST /api/rpc`: one call into the application.
pub(crate) async fn call(State(state): State<Shared>, request: Request) -> Response {
    if request.method() != Method::POST {
        return ApiError::method_not_allowed(
            "POST",
            format!("RPC calls are sent with POST, not {}", request.method()),
        )
        .into_response();
    }
    let (head, body) = request.into_parts();
    let call = match read_body(&head.headers, body).await {
        Ok(bytes) => parse(&bytes),
        Err(refusal) => Err(refusal),
    };
    let (method, params) = match call {
        Ok(call) => call,
        Err(refusal) => return refusal.into_response(),
    };

    let mut response = dispatch(&state, method.clone(), params).await;
    response.extensions_mut().insert(RpcMethod(method));
    response
}

/// Run the call and wrap its outcome in the envelope.
async fn dispatch(state: &Shared, method: String, params: Value) -> Response {
    let app = state.app.clone();
    let in_flight = state.in_flight.enter();
    let name = method.clone();
    // A task of its own: once a call has started it runs to the end even if
    // the browser tab is closed or the connection drops, exactly as it does
    // in the desktop app. Half-applied device changes are worse than an
    // unread reply.
    let call = tokio::spawn(async move {
        let _in_flight = in_flight;
        app.rpc(&method, params).await
    });
    match call.await {
        Ok(Ok(result)) => envelope::success(result),
        Ok(Err(error)) => envelope::app_error(&error),
        Err(failure) => {
            let name = printable(&name, 96);
            if failure.is_panic() {
                tracing::error!(rpc = %name, "an RPC method panicked");
                ApiError::internal(format!(
                    "`{name}` failed unexpectedly inside Nocturne; this is a bug, and the \
                     server log has the details"
                ))
            } else {
                ApiError::unavailable(format!(
                    "`{name}` was interrupted because the server is shutting down"
                ))
            }
            .into_response()
        }
    }
}

async fn read_body(headers: &HeaderMap, body: Body) -> Result<Vec<u8>, ApiError> {
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut too_large = declared.is_some_and(|length| length > MAX_BODY_BYTES as u64);
    if too_large && expects_continue(headers) {
        // The client is waiting for a go-ahead before it sends anything: say no right away.
        return Err(body_too_large());
    }

    let mut chunks = body.into_data_stream();
    let mut received = Vec::new();
    let mut discarded = 0usize;
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| {
            ApiError::invalid(
                "the request body could not be read: the connection was interrupted before \
                 the body was complete",
            )
        })?;
        if !too_large && received.len() + chunk.len() > MAX_BODY_BYTES {
            too_large = true;
            received = Vec::new();
        }
        if too_large {
            discarded += chunk.len();
            if discarded > MAX_DISCARDED_BYTES {
                break;
            }
        } else {
            received.extend_from_slice(&chunk);
        }
    }
    if too_large {
        Err(body_too_large())
    } else {
        Ok(received)
    }
}

fn expects_continue(headers: &HeaderMap) -> bool {
    headers
        .get(EXPECT)
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"100-continue"))
}

fn body_too_large() -> ApiError {
    ApiError::invalid_with(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "the request body is larger than {} MiB, the most this server accepts in one call",
            MAX_BODY_BYTES / (1024 * 1024)
        ),
    )
}

/// The method name and parameters of a request body.
fn parse(body: &[u8]) -> Result<(String, Value), ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(ApiError::invalid(format!(
            "the request body is empty; send JSON like {EXAMPLE}"
        )));
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| ApiError::invalid(format!("the request body is not valid JSON: {err}")))?;
    let Value::Object(mut fields) = value else {
        return Err(ApiError::invalid(format!(
            "the request body must be a JSON object like {EXAMPLE}"
        )));
    };
    let method = match fields.remove("method") {
        Some(Value::String(method)) if !method.trim().is_empty() => method,
        Some(Value::String(_)) | None => {
            return Err(ApiError::invalid(format!(
                "the request names no method; send JSON like {EXAMPLE}"
            )));
        }
        Some(_) => {
            return Err(ApiError::invalid(format!(
                "`method` must be a string; send JSON like {EXAMPLE}"
            )));
        }
    };
    let params = fields.remove("params").unwrap_or(Value::Null);
    Ok((method, params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn message(error: ApiError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        (
            status,
            body["error"]["message"].as_str().unwrap().to_string(),
        )
    }

    #[test]
    fn well_formed_requests_are_parsed() {
        assert_eq!(
            parse(br#"{"method": "devices.list"}"#).unwrap(),
            ("devices.list".to_string(), Value::Null)
        );
        assert_eq!(
            parse(br#" {"params": {"id": "x", "n": [1, 2]}, "method": "devices.get", "id": 7} "#)
                .unwrap(),
            ("devices.get".to_string(), json!({"id": "x", "n": [1, 2]}))
        );
        assert_eq!(
            parse(br#"{"method": "m", "params": null}"#).unwrap(),
            ("m".to_string(), Value::Null)
        );
        assert_eq!(
            parse(br#"{"method": "m", "params": [1, "two"]}"#).unwrap(),
            ("m".to_string(), json!([1, "two"]))
        );
    }

    #[tokio::test]
    async fn malformed_requests_say_what_is_wrong() {
        let cases: [(&[u8], &str); 10] = [
            (b"", "is empty"),
            (b"  \n", "is empty"),
            (b"{", "not valid JSON"),
            (b"{\"method\": \"m\"} trailing", "not valid JSON"),
            (b"not json", "not valid JSON"),
            (b"[\"devices.list\", {}]", "must be a JSON object"),
            (b"\"devices.list\"", "must be a JSON object"),
            (b"{}", "names no method"),
            (b"{\"method\": \"  \"}", "names no method"),
            (b"{\"method\": 5}", "must be a string"),
        ];
        for (body, expected) in cases {
            let (status, text) = message(parse(body).unwrap_err()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                text.contains(expected),
                "{:?} -> {text}",
                String::from_utf8_lossy(body)
            );
        }
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    axum::http::HeaderName::from_static(name),
                    axum::http::HeaderValue::from_str(value).unwrap(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn bodies_up_to_the_limit_are_read_and_larger_ones_refused() {
        let exact = vec![b'a'; MAX_BODY_BYTES];
        let read = read_body(&HeaderMap::new(), Body::from(exact.clone()))
            .await
            .unwrap();
        assert_eq!(read.len(), 16 * 1024 * 1024);
        assert!(read == exact);

        let over = vec![b'a'; MAX_BODY_BYTES + 1];
        let (status, text) = message(
            read_body(&HeaderMap::new(), Body::from(over))
                .await
                .unwrap_err(),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(text.contains("16 MiB"), "{text}");
    }

    #[tokio::test]
    async fn a_declared_oversize_is_refused_before_the_body_when_the_client_waits() {
        // The body stream would panic if it were polled.
        let poisoned = Body::from_stream(futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Vec<u8>, std::io::Error>>> {
                panic!("the body must not be read")
            },
        ));
        let declared = (MAX_BODY_BYTES + 1).to_string();
        let refusal = read_body(
            &headers(&[("content-length", &declared), ("expect", "100-continue")]),
            poisoned,
        )
        .await
        .unwrap_err();
        assert_eq!(message(refusal).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn an_oversized_stream_is_drained_only_so_far() {
        let chunk = vec![0u8; 1024 * 1024];
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = polled.clone();
        // An endless body: reading must stop on its own.
        let endless = Body::from_stream(futures::stream::repeat_with(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok::<_, std::io::Error>(chunk.clone())
        }));
        let refusal = read_body(&HeaderMap::new(), endless).await.unwrap_err();
        assert_eq!(message(refusal).await.0, StatusCode::PAYLOAD_TOO_LARGE);
        let megabytes = polled.load(std::sync::atomic::Ordering::Relaxed);
        // 16 MiB accepted, then at most 4 x 16 MiB thrown away (one chunk of slack each side).
        assert!(
            (17..=16 + 64 + 2).contains(&megabytes),
            "read {megabytes} MiB"
        );
    }

    #[tokio::test]
    async fn an_interrupted_body_is_reported() {
        let broken = Body::from_stream(futures::stream::iter([
            Ok(b"{\"method\"".to_vec()),
            Err(std::io::Error::other("connection reset")),
        ]));
        let (status, text) = message(read_body(&HeaderMap::new(), broken).await.unwrap_err()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(text.contains("interrupted"), "{text}");
    }
}
