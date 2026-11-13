//! Model providers: one trait, three wire formats.
//!
//! * [`anthropic`] — the Claude Messages API (native tool use, adaptive thinking).
//! * [`openai`] — Chat Completions, which nearly every hosted and local
//!   server speaks (OpenAI, OpenRouter, Groq, Gemini, Mistral, LM Studio,
//!   vLLM, llama.cpp, LiteLLM, …).
//! * [`ollama`] — Ollama's native API, used instead of its OpenAI shim so the
//!   context window (`num_ctx`) can be set per request.

pub mod anthropic;
mod http;
pub mod ollama;
pub mod openai;
pub mod prompted;
pub mod scripted;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::error::AiError;
use crate::types::{ChatRequest, ChatResponse, ModelInfo, StreamEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    /// Any server that implements OpenAI's Chat Completions API.
    Openai,
    Ollama,
}

/// How hard the model should think; maps to each provider's own control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// How tools are offered to the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    /// Native tool calling, falling back to prompted tools if the model
    /// or server rejects it.
    #[default]
    Auto,
    Native,
    /// Tools are described in the system prompt and calls are parsed from
    /// the reply text. Works with any model that can follow instructions.
    Prompted,
}

/// A configured model endpoint. The API key is supplied separately, from the vault.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub effort: Option<Effort>,
    /// Output token ceiling per response; a sensible per-provider default when unset.
    pub max_tokens: Option<u32>,
    /// Context window in tokens; discovered or assumed when unset.
    pub context_window: Option<u32>,
    pub extra_headers: IndexMap<String, String>,
    pub tool_mode: ToolMode,
    /// Anthropic only: let the API retry on a permitted fallback model when a
    /// safety classifier declines a request.
    pub refusal_fallback: bool,
    /// Seconds without any data before a streaming response is abandoned.
    pub idle_timeout_s: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            id: String::new(),
            name: String::new(),
            kind: ProviderKind::Anthropic,
            base_url: String::new(),
            model: String::new(),
            effort: None,
            max_tokens: None,
            context_window: None,
            extra_headers: IndexMap::new(),
            tool_mode: ToolMode::Auto,
            refusal_fallback: true,
            idle_timeout_s: 600,
        }
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn model(&self) -> &str;
    /// Tokens of context the configured model can hold.
    fn context_window(&self) -> u32;

    /// Generate one assistant message, streaming increments to `events`.
    async fn chat(
        &self,
        request: ChatRequest<'_>,
        events: &UnboundedSender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError>;

    async fn list_models(&self) -> Result<Vec<ModelInfo>, AiError>;
}

/// A tool-call id for providers that do not issue their own. Unique across
/// restarts (time-based prefix) and within a process (counter).
pub(crate) fn next_call_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("call_{millis:x}{count:x}")
}

pub fn build_provider(
    config: &ProviderConfig,
    api_key: Option<String>,
) -> Result<Arc<dyn Provider>, AiError> {
    crate::ensure_tls_provider();
    if config.model.trim().is_empty() {
        return Err(AiError::Config("no model selected".into()));
    }
    let provider: Arc<dyn Provider> = match config.kind {
        ProviderKind::Anthropic => Arc::new(anthropic::Anthropic::new(config.clone(), api_key)?),
        ProviderKind::Openai => Arc::new(openai::OpenAi::new(config.clone(), api_key)?),
        ProviderKind::Ollama => Arc::new(ollama::Ollama::new(config.clone())?),
    };
    Ok(provider)
}

/// A ready-made provider entry for the settings screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: ProviderKind,
    pub base_url: &'static str,
    pub default_model: &'static str,
    pub needs_key: bool,
    /// Runs on the user's own machine or network: nothing leaves the site.
    pub local: bool,
    pub key_url: &'static str,
    pub note: &'static str,
}

pub fn presets() -> Vec<ProviderPreset> {
    use ProviderKind::{Anthropic, Ollama, Openai};
    let preset =
        |id, name, kind, base_url, default_model, needs_key, local, key_url, note| ProviderPreset {
            id,
            name,
            kind,
            base_url,
            default_model,
            needs_key,
            local,
            key_url,
            note,
        };
    vec![
        preset(
            "anthropic",
            "Anthropic Claude",
            Anthropic,
            "https://api.anthropic.com",
            "claude-opus-5-5",
            true,
            false,
            "https://console.anthropic.com/settings/keys",
            "Best tool use and reasoning for network work.",
        ),
        preset(
            "ollama",
            "Ollama (local)",
            Ollama,
            "http://127.0.0.1:11434",
            "",
            false,
            true,
            "",
            "Runs models on this machine. Nothing leaves your network.",
        ),
        preset(
            "lmstudio",
            "LM Studio (local)",
            Openai,
            "http://127.0.0.1:1234/v1",
            "",
            false,
            true,
            "",
            "Local server built into LM Studio.",
        ),
        preset(
            "openai",
            "OpenAI",
            Openai,
            "https://api.openai.com/v1",
            "",
            true,
            false,
            "https://platform.openai.com/api-keys",
            "",
        ),
        preset(
            "gemini",
            "Google Gemini",
            Openai,
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "",
            true,
            false,
            "https://aistudio.google.com/apikey",
            "Uses Gemini's OpenAI-compatible endpoint.",
        ),
        preset(
            "openrouter",
            "OpenRouter",
            Openai,
            "https://openrouter.ai/api/v1",
            "",
            true,
            false,
            "https://openrouter.ai/keys",
            "One key for many hosted models.",
        ),
        preset(
            "groq",
            "Groq",
            Openai,
            "https://api.groq.com/openai/v1",
            "",
            true,
            false,
            "https://console.groq.com/keys",
            "",
        ),
        preset(
            "mistral",
            "Mistral",
            Openai,
            "https://api.mistral.ai/v1",
            "",
            true,
            false,
            "https://console.mistral.ai/api-keys",
            "",
        ),
        preset(
            "deepseek",
            "DeepSeek",
            Openai,
            "https://api.deepseek.com/v1",
            "",
            true,
            false,
            "https://platform.deepseek.com/api_keys",
            "",
        ),
        preset(
            "xai",
            "xAI",
            Openai,
            "https://api.x.ai/v1",
            "",
            true,
            false,
            "https://console.x.ai",
            "",
        ),
        preset(
            "custom",
            "Custom OpenAI-compatible",
            Openai,
            "",
            "",
            false,
            false,
            "",
            "vLLM, llama.cpp, LiteLLM, Azure OpenAI v1 or any compatible gateway.",
        ),
    ]
}

/// A local model server found by [`detect_local`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalEndpoint {
    pub preset: String,
    pub base_url: String,
    pub models: Vec<ModelInfo>,
}

/// Probe the usual localhost ports for Ollama and LM Studio.
pub async fn detect_local() -> Vec<LocalEndpoint> {
    crate::ensure_tls_provider();
    let mut found = Vec::new();
    let probe = Duration::from_millis(900);

    let ollama = ProviderConfig {
        kind: ProviderKind::Ollama,
        base_url: "http://127.0.0.1:11434".into(),
        model: "probe".into(),
        ..ProviderConfig::default()
    };
    if let Ok(provider) = ollama::Ollama::new(ollama.clone())
        && let Ok(Ok(models)) = tokio::time::timeout(probe, provider.list_models()).await
    {
        found.push(LocalEndpoint {
            preset: "ollama".into(),
            base_url: ollama.base_url,
            models,
        });
    }

    let lmstudio = ProviderConfig {
        kind: ProviderKind::Openai,
        base_url: "http://127.0.0.1:1234/v1".into(),
        model: "probe".into(),
        ..ProviderConfig::default()
    };
    if let Ok(provider) = openai::OpenAi::new(lmstudio.clone(), None)
        && let Ok(Ok(models)) = tokio::time::timeout(probe, provider.list_models()).await
    {
        found.push(LocalEndpoint {
            preset: "lmstudio".into(),
            base_url: lmstudio.base_url,
            models,
        });
    }
    found
}
