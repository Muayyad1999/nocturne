use std::time::Duration;

#[derive(thiserror::Error, Debug, Clone)]
pub enum AiError {
    #[error("provider is not configured: {0}")]
    Config(String),
    #[error("could not reach the model endpoint: {0}")]
    Network(String),
    #[error("the model endpoint did not respond in time")]
    Timeout,
    #[error("{}", api_message(*.status, .kind, .message))]
    Api {
        status: u16,
        /// Provider error type, e.g. `rate_limit_error`.
        kind: String,
        message: String,
        request_id: Option<String>,
        retry_after: Option<Duration>,
    },
    #[error("the response stream was malformed: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

fn api_message(status: u16, kind: &str, message: &str) -> String {
    let hint = match status {
        401 => " — the API key was rejected; check it in Settings → AI",
        402 => " — the provider reports a billing problem on this account",
        403 => " — this key is not allowed to use that model or feature",
        404 => " — the model name is unknown to this provider (or not enabled for this key)",
        413 => " — the conversation is too large for this model",
        429 => " — rate limited; try again shortly",
        529 | 503 => " — the provider is overloaded; try again shortly",
        _ => "",
    };
    if kind.is_empty() {
        format!("model API error {status}: {message}{hint}")
    } else {
        format!("model API error {status} ({kind}): {message}{hint}")
    }
}

impl AiError {
    /// Whether sending the same request again can reasonably succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            AiError::Network(_) | AiError::Timeout => true,
            AiError::Api { status, .. } => {
                matches!(status, 408 | 409 | 429 | 500 | 502 | 503 | 504 | 529)
            }
            _ => false,
        }
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            AiError::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// True when the endpoint rejected the request because the model (or
    /// server) cannot do native tool calling.
    pub fn is_tools_unsupported(&self) -> bool {
        match self {
            AiError::Api {
                status: 400 | 404 | 422 | 501,
                message,
                ..
            } => {
                let m = message.to_ascii_lowercase();
                (m.contains("tool") || m.contains("function"))
                    && (m.contains("not support")
                        || m.contains("unsupported")
                        || m.contains("does not support"))
            }
            _ => false,
        }
    }
}

impl From<reqwest::Error> for AiError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            AiError::Timeout
        } else {
            // Walk the source chain: reqwest's top-level message is often just "error sending request".
            let mut text = err.to_string();
            let mut source = std::error::Error::source(&err);
            while let Some(inner) = source {
                let inner_text = inner.to_string();
                if !text.contains(&inner_text) {
                    text.push_str(": ");
                    text.push_str(&inner_text);
                }
                source = inner.source();
            }
            AiError::Network(text)
        }
    }
}
