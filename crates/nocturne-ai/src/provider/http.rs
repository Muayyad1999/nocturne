//! Shared HTTP plumbing: client construction, retries, error-body parsing and
//! cancellable chunk reads.

use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::ProviderConfig;
use crate::error::AiError;

const MAX_ATTEMPTS: u32 = 3;

pub(crate) fn client() -> Result<reqwest::Client, AiError> {
    crate::ensure_tls_provider();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("nocturne/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| AiError::Config(format!("could not create the HTTP client: {err}")))
}

pub(crate) fn idle_timeout(config: &ProviderConfig) -> Duration {
    Duration::from_secs(config.idle_timeout_s.clamp(30, 3600))
}

/// `https://host/v1/` → `https://host/v1`; empty stays empty.
pub(crate) fn trim_base(base: &str) -> &str {
    base.trim().trim_end_matches('/')
}

/// True for loopback, link-local and RFC 1918 hosts: a model on the user's own network.
pub(crate) fn is_local_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") || host.ends_with(".lan")
    {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// Send a request, retrying transient failures with back-off. Returns only
/// successful (2xx) responses; anything else becomes an [`AiError::Api`].
pub(crate) async fn send(
    build: impl Fn() -> reqwest::RequestBuilder,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, AiError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let outcome = tokio::select! {
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            result = build().send() => result,
        };
        let error = match outcome {
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response) => {
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                let body = response.text().await.unwrap_or_default();
                api_error(status, &headers, &body)
            }
            Err(err) => AiError::from(err),
        };
        if attempt >= MAX_ATTEMPTS || !error.is_retryable() {
            return Err(error);
        }
        let wait = error
            .retry_after()
            .map(|delay| delay.min(Duration::from_secs(30)))
            .unwrap_or_else(|| {
                Duration::from_millis(800 * u64::from(attempt) * u64::from(attempt))
            });
        tracing::debug!(attempt, ?wait, %error, "retrying model request");
        tokio::select! {
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Build an [`AiError::Api`] from an error response, understanding the
/// Anthropic, OpenAI and Ollama body shapes.
pub(crate) fn api_error(status: u16, headers: &HeaderMap, body: &str) -> AiError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let (kind, message) = match parsed.as_ref().and_then(|value| value.get("error")) {
        Some(Value::String(message)) => (String::new(), message.clone()),
        Some(Value::Object(error)) => {
            let kind = error
                .get("type")
                .or_else(|| error.get("code"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (kind, message)
        }
        _ => (String::new(), String::new()),
    };
    let message = if message.is_empty() {
        truncate(body.trim(), 400)
    } else {
        message
    };
    let request_id = parsed
        .as_ref()
        .and_then(|value| value.get("request_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        });
    let retry_after = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(Duration::from_secs_f64);
    AiError::Api {
        status,
        kind,
        message,
        request_id,
        retry_after,
    }
}

/// Error reported inside a response stream after the HTTP status was already 200.
pub(crate) fn stream_error(error: &Value) -> AiError {
    let (kind, message) = match error {
        Value::String(message) => (String::new(), message.clone()),
        other => (
            other
                .get("type")
                .or_else(|| other.get("code"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            other
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("stream error")
                .to_string(),
        ),
    };
    let status = match kind.as_str() {
        "overloaded_error" => 529,
        "rate_limit_error" => 429,
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        _ => 500,
    };
    AiError::Api {
        status,
        kind,
        message,
        request_id: None,
        retry_after: None,
    }
}

/// Next body chunk, or `None` at the end of the stream.
pub(crate) async fn next_chunk(
    response: &mut reqwest::Response,
    idle: Duration,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>, AiError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(AiError::Cancelled),
        read = tokio::time::timeout(idle, response.chunk()) => match read {
            Err(_) => Err(AiError::Timeout),
            Ok(Ok(chunk)) => Ok(chunk.map(|bytes| bytes.to_vec())),
            Ok(Err(err)) => Err(AiError::from(err)),
        },
    }
}

pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_error_bodies_from_each_provider() {
        let headers = HeaderMap::new();
        let anthropic = api_error(
            429,
            &headers,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"},"request_id":"req_1"}"#,
        );
        match anthropic {
            AiError::Api {
                status,
                kind,
                message,
                request_id,
                ..
            } => {
                assert_eq!(
                    (status, kind.as_str(), message.as_str()),
                    (429, "rate_limit_error", "slow down")
                );
                assert_eq!(request_id.as_deref(), Some("req_1"));
            }
            other => panic!("unexpected {other:?}"),
        }
        let openai = api_error(
            401,
            &headers,
            r#"{"error":{"message":"bad key","type":"invalid_request_error","code":"invalid_api_key"}}"#,
        );
        assert!(openai.to_string().contains("bad key"));
        let ollama = api_error(
            400,
            &headers,
            r#"{"error":"registry.ollama.ai/library/gemma3 does not support tools"}"#,
        );
        assert!(ollama.is_tools_unsupported());
        let plain = api_error(502, &headers, "<html>Bad Gateway</html>");
        assert!(plain.is_retryable());
    }

    #[test]
    fn retry_after_header_is_honoured() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "7".parse().unwrap());
        let err = api_error(429, &headers, "{}");
        assert_eq!(err.retry_after(), Some(Duration::from_secs(7)));
    }

    #[test]
    fn recognises_local_urls() {
        for url in [
            "http://127.0.0.1:11434",
            "http://localhost:1234/v1",
            "http://192.168.1.20:8080/v1",
            "http://[::1]:8000",
            "http://gpu-box.lan:8000/v1",
        ] {
            assert!(is_local_url(url), "{url}");
        }
        for url in [
            "https://api.anthropic.com",
            "https://api.openai.com/v1",
            "http://8.8.8.8/v1",
        ] {
            assert!(!is_local_url(url), "{url}");
        }
    }
}
