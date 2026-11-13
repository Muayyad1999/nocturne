//! The one error type that crosses the RPC boundary.

use serde::Serialize;
use serde_json::Value;

/// An error as the UI sees it: a stable machine `code`, a message written for
/// the person at the keyboard, and optional structured details.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> AppError {
        AppError {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> AppError {
        self.details = Some(details);
        self
    }

    /// The named thing does not exist.
    pub fn not_found(what: impl std::fmt::Display) -> AppError {
        AppError::new("not_found", format!("{what} was not found"))
    }

    /// The request itself is wrong; `message` says how to fix it.
    pub fn invalid(message: impl Into<String>) -> AppError {
        AppError::new("invalid", message)
    }

    /// The request clashes with existing state (duplicate name, stale version).
    pub fn conflict(message: impl Into<String>) -> AppError {
        AppError::new("conflict", message)
    }

    /// Something outside Nocturne is not answering (device, model endpoint).
    pub fn unavailable(message: impl Into<String>) -> AppError {
        AppError::new("unavailable", message)
    }

    pub fn timeout(message: impl Into<String>) -> AppError {
        AppError::new("timeout", message)
    }

    /// The vault is locked or a credential is missing.
    pub fn unauthorized(message: impl Into<String>) -> AppError {
        AppError::new("unauthorized", message)
    }

    /// The action was refused by policy (read-only mode, protected device).
    pub fn forbidden(message: impl Into<String>) -> AppError {
        AppError::new("forbidden", message)
    }

    /// A bug or an unexpected failure inside Nocturne.
    pub fn internal(message: impl Into<String>) -> AppError {
        AppError::new("internal", message)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        AppError::internal(format!("database error: {err}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::internal(format!("could not encode or decode data: {err}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::internal(format!("file error: {err}"))
    }
}

impl From<nocturne_drivers::session::DriverError> for AppError {
    fn from(err: nocturne_drivers::session::DriverError) -> Self {
        use nocturne_drivers::session::DriverError as E;
        let code = match &err {
            E::Timeout { .. } => "timeout",
            E::AuthFailed(_) | E::EnableFailed(_) => "unauthorized",
            E::Channel(_) | E::Closed { .. } => "unavailable",
            E::Unsupported(_) => "invalid",
            E::ConfigModeFailed(_) | E::Parse { .. } | E::Rejected { .. } => "device_error",
        };
        AppError::new(code, err.to_string())
    }
}

impl From<nocturne_ai::AiError> for AppError {
    fn from(err: nocturne_ai::AiError) -> Self {
        use nocturne_ai::AiError as E;
        let code = match &err {
            E::Config(_) => "invalid",
            E::Timeout => "timeout",
            E::Cancelled => "cancelled",
            E::Api {
                status: 401 | 403, ..
            } => "unauthorized",
            _ => "unavailable",
        };
        AppError::new(code, err.to_string())
    }
}
