//! The reply envelope shared with the UI: `{"ok": true, "result": …}` or
//! `{"ok": false, "error": {"code", "message", "details"?}}`.
//!
//! Application errors travel in a `200` reply. Failures of the transport
//! itself (bad request, missing token, wrong origin, …) use the same error
//! shape together with an HTTP status, which is what [`ApiError`] is.

use axum::Json;
use axum::http::header::{ALLOW, WWW_AUTHENTICATE};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use nocturne_suite::AppError;
use serde_json::{Value, json};

/// The successful reply to an RPC call.
pub(crate) fn success(result: Value) -> Response {
    Json(json!({"ok": true, "result": result})).into_response()
}

/// The reply to an RPC call the application refused. Still `200`: the request was delivered.
pub(crate) fn app_error(error: &AppError) -> Response {
    Json(json!({"ok": false, "error": error})).into_response()
}

/// A request that failed before (or instead of) reaching the application.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    header: Option<(HeaderName, HeaderValue)>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code,
            message: message.into(),
            header: None,
        }
    }

    /// `400`: the request itself is wrong.
    pub(crate) fn invalid(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid", message)
    }

    /// Like [`invalid`](Self::invalid), with a more specific status (`413`, `426`, …).
    pub(crate) fn invalid_with(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError::new(status, "invalid", message)
    }

    /// `401`, with the challenge that tells clients which scheme to use.
    pub(crate) fn unauthorized(message: impl Into<String>) -> ApiError {
        ApiError {
            header: Some((WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))),
            ..ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
        }
    }

    /// `403`: the Host or Origin check failed.
    pub(crate) fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    /// `404`.
    pub(crate) fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    /// `405`, naming the methods that would have worked.
    pub(crate) fn method_not_allowed(
        allowed: &'static str,
        message: impl Into<String>,
    ) -> ApiError {
        ApiError {
            header: Some((ALLOW, HeaderValue::from_static(allowed))),
            ..ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "invalid", message)
        }
    }

    /// `503`: the server cannot take this on right now.
    pub(crate) fn unavailable(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }

    /// `500`: a bug or an unexpected failure inside the server.
    pub(crate) fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"ok": false, "error": {"code": self.code, "message": self.message}});
        let mut response = (self.status, Json(body)).into_response();
        if let Some((name, value)) = self.header {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn parts(response: Response) -> (StatusCode, axum::http::HeaderMap, Value) {
        let (head, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX).await.unwrap();
        (
            head.status,
            head.headers,
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    #[tokio::test]
    async fn success_and_application_errors_are_200() {
        let (status, headers, body) = parts(success(json!([1, 2]))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(body, json!({"ok": true, "result": [1, 2]}));

        let plain = AppError::not_found("device `x`");
        let (status, _, body) = parts(app_error(&plain)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({"ok": false, "error": {"code": "not_found", "message": "device `x` was not found"}})
        );

        let detailed = AppError::conflict("taken").with_details(json!({"name": "sw1"}));
        let (_, _, body) = parts(app_error(&detailed)).await;
        assert_eq!(body["error"]["details"], json!({"name": "sw1"}));
    }

    #[tokio::test]
    async fn transport_errors_carry_status_and_envelope() {
        let cases = [
            (ApiError::invalid("bad"), 400, "invalid"),
            (
                ApiError::invalid_with(StatusCode::PAYLOAD_TOO_LARGE, "big"),
                413,
                "invalid",
            ),
            (ApiError::unauthorized("who"), 401, "unauthorized"),
            (ApiError::forbidden("no"), 403, "forbidden"),
            (ApiError::not_found("where"), 404, "not_found"),
            (ApiError::method_not_allowed("POST", "how"), 405, "invalid"),
            (ApiError::unavailable("later"), 503, "unavailable"),
            (ApiError::internal("oops"), 500, "internal"),
        ];
        for (error, status, code) in cases {
            let (actual, headers, body) = parts(error.into_response()).await;
            assert_eq!(actual.as_u16(), status);
            assert_eq!(body["ok"], false);
            assert_eq!(body["error"]["code"], code);
            assert!(body["error"]["message"].is_string());
            assert_eq!(headers.contains_key("www-authenticate"), status == 401);
            assert_eq!(headers.contains_key("allow"), status == 405);
        }
        let (_, headers, _) = parts(ApiError::unauthorized("who").into_response()).await;
        assert_eq!(headers["www-authenticate"], "Bearer");
        let (_, headers, _) =
            parts(ApiError::method_not_allowed("POST", "how").into_response()).await;
        assert_eq!(headers["allow"], "POST");
    }
}
