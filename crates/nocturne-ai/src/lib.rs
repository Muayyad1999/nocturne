//! LLM providers and the tool-using agent loop behind Nocturne's AI features.
//!
//! The crate is deliberately ignorant of networking gear: it knows how to talk
//! to model APIs ([`provider`]), how to drive a model through a set of tools
//! until a task is done ([`agent`]), and how to keep secrets out of what the
//! model sees ([`redact`]). The application crate supplies the tools.

pub mod agent;
pub mod error;
pub mod provider;
pub mod redact;
pub mod schema;
mod sse;
pub mod types;

pub use agent::{Agent, AgentConfig, AgentEvent, ToolCall, ToolHost, ToolOutput, TurnEnd};
pub use error::AiError;
pub use provider::{
    Effort, Provider, ProviderConfig, ProviderKind, ProviderPreset, ToolMode, build_provider,
    detect_local, presets,
};
pub use redact::Redactor;
pub use tokio_util::sync::CancellationToken;
pub use types::{
    Block, ChatRequest, ChatResponse, Message, ModelInfo, RawContent, Role, StopReason,
    StreamEvent, ToolSpec, Usage,
};

/// Install the process-wide TLS crypto provider (ring) if none is set yet.
///
/// The workspace builds reqwest without a bundled provider so that no C
/// toolchain is needed; every binary must call this once before making an
/// HTTPS request. Calling it repeatedly is harmless.
pub fn ensure_tls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // A concurrent installer may win the race; either outcome is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}
