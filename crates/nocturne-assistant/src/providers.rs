//! Model providers: Claude, anything that speaks OpenAI's API, and local
//! runtimes (Ollama, LM Studio).
//!
//! A provider is a document plus one secret: the API key goes into the vault
//! under `ai:provider:<id>:key` and never comes back out through RPC, an
//! event or a log line. Having no provider at all is a normal state — the
//! rest of Nocturne does not depend on the assistant — so everything here
//! answers with where to add one rather than with an error nobody can act on.

use std::sync::Arc;

use nocturne_ai::provider::{Effort, LocalEndpoint, ToolMode};
use nocturne_ai::{
    Message, Provider, ProviderConfig, ProviderKind, ToolSpec, build_provider, detect_local,
    presets,
};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, NoParams};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub const COLLECTION: &str = "ai_providers";

/// Where to point someone who has not set a provider up yet.
pub const WHERE_TO_ADD: &str = "Add one under Settings → AI: Claude, any OpenAI-compatible endpoint, or a local model with \
     Ollama or LM Studio (nothing leaves your network with those).";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub effort: Option<Effort>,
    pub tool_mode: ToolMode,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub is_default: bool,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for ProviderRecord {
    fn default() -> Self {
        ProviderRecord {
            id: String::new(),
            name: String::new(),
            kind: ProviderKind::Anthropic,
            base_url: String::new(),
            model: String::new(),
            effort: None,
            tool_mode: ToolMode::Auto,
            context_window: None,
            max_output_tokens: None,
            is_default: false,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// A provider as the UI sees it: the record, plus whether a key is stored.
#[derive(Clone, Debug, Serialize)]
pub struct ProviderEntry {
    #[serde(flatten)]
    pub record: ProviderRecord,
    pub has_key: bool,
}

fn key_id(provider_id: &str) -> String {
    format!("ai:provider:{provider_id}:key")
}

fn entry(app: &App, record: ProviderRecord) -> ProviderEntry {
    let has_key = app.vault().exists(&key_id(&record.id)).unwrap_or(false);
    ProviderEntry { record, has_key }
}

pub fn list(app: &App) -> AppResult<Vec<ProviderRecord>> {
    let mut records: Vec<ProviderRecord> = app.store().list(COLLECTION)?;
    records.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(records)
}

pub fn get(app: &App, id: &str) -> AppResult<ProviderRecord> {
    app.store().require(COLLECTION, id, "model provider")
}

/// The provider a new chat uses: the default, or the only enabled one.
pub fn default_provider(app: &App) -> AppResult<ProviderRecord> {
    let enabled: Vec<ProviderRecord> = list(app)?
        .into_iter()
        .filter(|record| record.enabled && !record.model.trim().is_empty())
        .collect();
    enabled
        .iter()
        .find(|record| record.is_default)
        .or_else(|| enabled.first())
        .cloned()
        .ok_or_else(|| {
            AppError::unavailable(format!(
                "No AI model is configured, so the assistant cannot answer. {WHERE_TO_ADD}"
            ))
            .with_details(json!({"needs": "provider", "where": "settings.ai"}))
        })
}

pub fn configured(app: &App) -> bool {
    default_provider(app).is_ok()
}

/// The base URL to use when the user did not type one.
fn default_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com",
        ProviderKind::Openai => "https://api.openai.com/v1",
        ProviderKind::Ollama => "http://127.0.0.1:11434",
    }
}

pub fn config_of(record: &ProviderRecord) -> ProviderConfig {
    ProviderConfig {
        id: record.id.clone(),
        name: record.name.clone(),
        kind: record.kind,
        base_url: if record.base_url.trim().is_empty() {
            default_base_url(record.kind).to_string()
        } else {
            record.base_url.clone()
        },
        model: record.model.clone(),
        effort: record.effort,
        max_tokens: record.max_output_tokens,
        context_window: record.context_window,
        tool_mode: record.tool_mode,
        ..ProviderConfig::default()
    }
}

/// A provider put in place of the real one.
///
/// The desktop application never sets this; it is the seam the tests use to
/// drive the assistant with `nocturne_ai`'s scripted provider instead of a
/// model endpoint, so no test ever needs a key or a network.
#[derive(Default)]
pub struct Override {
    provider: parking_lot::Mutex<Option<Arc<dyn Provider>>>,
}

/// Use `provider` for every chat from now on. Tests only.
#[doc(hidden)]
pub fn use_provider(app: &App, provider: Arc<dyn Provider>) {
    *app.state::<Override>().provider.lock() = Some(provider);
}

/// Go back to building providers from their records. Tests only.
#[doc(hidden)]
pub fn use_real_providers(app: &App) {
    *app.state::<Override>().provider.lock() = None;
}

/// The window a turn with this provider is planned for: the one configured,
/// or else the one the provider assumes — 16,000 tokens for a model on the
/// local network nobody configured one for. `ai.tools` and `ai.status` ask
/// this too, so a screen never lists tools the model is not offered. `0`
/// (unknown) when the provider cannot be built, which a turn cannot use
/// either.
pub fn context_window(app: &App, record: &ProviderRecord) -> u32 {
    record.context_window.unwrap_or_else(|| {
        build(app, record)
            .map(|provider| provider.context_window())
            .unwrap_or(0)
    })
}

/// Whether the default provider's model gets the small tool set.
fn small_default(app: &App, default: Option<&ProviderRecord>) -> bool {
    default.is_some_and(|record| crate::settings::small_context(context_window(app, record)))
}

/// Build a live provider from a record, taking its key out of the vault.
pub fn build(app: &App, record: &ProviderRecord) -> AppResult<Arc<dyn Provider>> {
    if let Some(provider) = app.state::<Override>().provider.lock().clone() {
        return Ok(provider);
    }
    let key = app
        .vault()
        .get(&key_id(&record.id))
        .map_err(|err| {
            AppError::unauthorized(format!(
                "the API key of `{}` could not be read ({err}). Unlock the vault, or add the key again under Settings → AI.",
                record.name
            ))
        })?
        .map(|key| key.to_string());
    build_provider(&config_of(record), key).map_err(AppError::from)
}

// ------------------------------------------------------------------- save

#[derive(Deserialize)]
struct SaveParams {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<ProviderKind>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// Stored in the vault; an empty string removes the stored key.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    effort: Option<Effort>,
    #[serde(default)]
    tool_mode: Option<ToolMode>,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    max_output_tokens: Option<u32>,
    #[serde(default)]
    is_default: Option<bool>,
    #[serde(default)]
    enabled: Option<bool>,
}

fn save(app: &App, params: SaveParams) -> AppResult<ProviderEntry> {
    let existing = match &params.id {
        Some(id) if !id.is_empty() => Some(get(app, id)?),
        _ => None,
    };
    let updated = existing.is_some();
    let mut record = existing.unwrap_or_else(|| ProviderRecord {
        id: new_id(),
        created_at: now_ms(),
        ..ProviderRecord::default()
    });
    if let Some(kind) = params.kind {
        record.kind = kind;
    }
    if let Some(name) = params.name {
        record.name = name.trim().to_string();
    }
    if let Some(base_url) = params.base_url {
        record.base_url = base_url.trim().trim_end_matches('/').to_string();
    }
    if let Some(model) = params.model {
        record.model = model.trim().to_string();
    }
    if let Some(effort) = params.effort {
        record.effort = Some(effort);
    }
    if let Some(tool_mode) = params.tool_mode {
        record.tool_mode = tool_mode;
    }
    if params.context_window.is_some() {
        record.context_window = params.context_window;
    }
    if params.max_output_tokens.is_some() {
        record.max_output_tokens = params.max_output_tokens;
    }
    if let Some(enabled) = params.enabled {
        record.enabled = enabled;
    }
    if record.name.is_empty() {
        record.name = format!("{:?}", record.kind);
    }
    if record.model.trim().is_empty() {
        return Err(AppError::invalid(
            "choose a model — `ai.providers.models` lists what this endpoint offers",
        ));
    }
    if record.kind != ProviderKind::Anthropic && record.base_url.is_empty() {
        record.base_url = default_base_url(record.kind).to_string();
    }
    record.updated_at = now_ms();

    // The key never goes into the document.
    if let Some(api_key) = params.api_key {
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            app.vault().delete(&key_id(&record.id))?;
        } else {
            app.vault().put(&key_id(&record.id), &api_key)?;
        }
    }

    if params.is_default == Some(true) || list(app)?.is_empty() {
        for mut other in list(app)? {
            if other.is_default && other.id != record.id {
                other.is_default = false;
                app.store().put(COLLECTION, &other.id, &other)?;
            }
        }
        record.is_default = true;
    } else if params.is_default == Some(false) {
        record.is_default = false;
    }

    app.store().put(COLLECTION, &record.id, &record)?;
    app.audit(
        "ai.provider.save",
        None,
        format!(
            "{} the AI provider `{}` ({})",
            if updated { "Updated" } else { "Added" },
            record.name,
            record.model
        ),
        json!({"provider_id": record.id, "kind": record.kind, "model": record.model}),
    );
    app.publish("ai.providers.changed", &json!({"id": record.id}));
    Ok(entry(app, record))
}

// ------------------------------------------------------------------- test

#[derive(Serialize)]
struct TestResult {
    ok: bool,
    model: String,
    latency_ms: u64,
    /// `native`, `prompted` or `none`.
    tool_calling: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn test_tool() -> ToolSpec {
    ToolSpec {
        name: "report_ok".into(),
        description: "Report that the connection works. Call this and nothing else.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"],
            "additionalProperties": false
        }),
        parallel_safe: true,
    }
}

const TEST_SYSTEM: &str = "You are being checked by a network tool. Call the tool `report_ok` with ok set to true. \
     Do not write anything else.";

/// One tiny request, including one tool call, so the user learns whether this
/// model can actually use tools — which is what decides if it is any use here.
async fn probe(provider: &Arc<dyn Provider>) -> Result<bool, AppError> {
    let tools = [test_tool()];
    let messages = [Message::user("Run the check.")];
    let request = nocturne_ai::ChatRequest {
        system: TEST_SYSTEM,
        messages: &messages,
        tools: &tools,
        allow_tools: true,
    };
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        provider.chat(request, &events, &cancel),
    )
    .await
    .map_err(|_| AppError::timeout("the model did not answer within a minute"))?
    .map_err(AppError::from)?;
    Ok(response.message.tool_uses().next().is_some())
}

async fn test(app: &App, id: &str) -> AppResult<TestResult> {
    let record = get(app, id)?;
    let started = std::time::Instant::now();
    let fail = |error: AppError, started: std::time::Instant| TestResult {
        ok: false,
        model: String::new(),
        latency_ms: started.elapsed().as_millis() as u64,
        tool_calling: "none",
        error: Some(error.message),
    };

    // Native first (unless the user chose prompted), then prompted: that
    // order is also the order of preference.
    let native_first = record.tool_mode != ToolMode::Prompted;
    let first_mode = if native_first {
        ToolMode::Native
    } else {
        ToolMode::Prompted
    };
    let first = ProviderRecord {
        tool_mode: first_mode,
        ..record.clone()
    };
    let provider = match build(app, &first) {
        Ok(provider) => provider,
        Err(err) => return Ok(fail(err, started)),
    };
    match probe(&provider).await {
        Ok(true) => {
            return Ok(TestResult {
                ok: true,
                model: record.model,
                latency_ms: started.elapsed().as_millis() as u64,
                tool_calling: if native_first { "native" } else { "prompted" },
                error: None,
            });
        }
        // Prompted mode was the first and only attempt: the model answered
        // but did not call the tool.
        outcome if !native_first => {
            return match outcome {
                Err(err) => Ok(fail(err, started)),
                Ok(_) => Ok(TestResult {
                    ok: true,
                    model: record.model,
                    latency_ms: started.elapsed().as_millis() as u64,
                    tool_calling: "none",
                    error: Some(
                        "the model answered but did not call the tool it was asked to call".into(),
                    ),
                }),
            };
        }
        // No key, no such model, nothing listening: another tool mode will
        // not help.
        Err(err) if matches!(err.code, "unauthorized" | "timeout") => {
            return Ok(fail(err, started));
        }
        _ => {}
    }

    // Second chance: tools described in the prompt, which works with models
    // that have no tool API at all.
    let fallback = ProviderRecord {
        tool_mode: ToolMode::Prompted,
        ..record.clone()
    };
    let provider = match build(app, &fallback) {
        Ok(provider) => provider,
        Err(err) => return Ok(fail(err, started)),
    };
    match probe(&provider).await {
        Ok(true) => Ok(TestResult {
            ok: true,
            model: record.model,
            latency_ms: started.elapsed().as_millis() as u64,
            tool_calling: "prompted",
            error: Some(
                "this model has no native tool calling; Nocturne will describe the tools in the prompt instead".into(),
            ),
        }),
        Ok(false) => Ok(TestResult {
            ok: true,
            model: record.model,
            latency_ms: started.elapsed().as_millis() as u64,
            tool_calling: "none",
            error: Some(
                "the model answers but cannot use tools, so it can talk about the network but not look at it".into(),
            ),
        }),
        Err(err) => Ok(fail(err, started)),
    }
}

// ----------------------------------------------------------------- models

#[derive(Deserialize)]
struct ModelsParams {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    kind: Option<ProviderKind>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

async fn models(app: &App, params: ModelsParams) -> AppResult<Vec<nocturne_ai::ModelInfo>> {
    let (config, key) = match &params.id {
        Some(id) if !id.is_empty() => {
            let record = get(app, id)?;
            let key = app
                .vault()
                .get(&key_id(&record.id))?
                .map(|key| key.to_string());
            (config_of(&record), key)
        }
        _ => {
            let kind = params.kind.ok_or_else(|| {
                AppError::invalid("give either a saved provider id, or a kind and a base URL")
            })?;
            let base_url = params
                .base_url
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| default_base_url(kind).to_string());
            (
                ProviderConfig {
                    kind,
                    base_url: base_url.trim().trim_end_matches('/').to_string(),
                    // Listing models does not need one, but the builder does.
                    model: "list".into(),
                    ..ProviderConfig::default()
                },
                params.api_key,
            )
        }
    };
    let provider = build_provider(&config, key).map_err(AppError::from)?;
    provider.list_models().await.map_err(AppError::from)
}

// ------------------------------------------------------------------ status

#[derive(Serialize)]
struct Status {
    configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<ProviderEntry>,
    autonomy: &'static str,
    /// Always true: there is no way to send a model unredacted device output.
    redaction: bool,
    /// How many tools this build can offer the model.
    tools: usize,
}

// ---------------------------------------------------------------- register

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("ai.providers.list", |app: App, _: NoParams| async move {
        Ok(list(&app)?
            .into_iter()
            .map(|record| entry(&app, record))
            .collect::<Vec<_>>())
    });

    router.add(
        "ai.providers.presets",
        |_app: App, _: NoParams| async move { Ok(presets()) },
    );

    router.add(
        "ai.providers.save",
        |app: App, params: SaveParams| async move { save(&app, params) },
    );

    router.add(
        "ai.providers.delete",
        |app: App, params: IdParams| async move {
            let record = get(&app, &params.id)?;
            app.store().delete(COLLECTION, &params.id)?;
            app.vault().delete(&key_id(&params.id))?;
            // Something has to be the default if anything is left.
            if record.is_default
                && let Some(mut next) = list(&app)?.into_iter().next()
            {
                next.is_default = true;
                app.store().put(COLLECTION, &next.id, &next)?;
            }
            app.audit(
                "ai.provider.delete",
                None,
                format!("Removed the AI provider `{}`", record.name),
                json!({"provider_id": params.id}),
            );
            app.publish("ai.providers.changed", &json!({"id": params.id}));
            Ok(json!({"deleted": true}))
        },
    );

    router.add(
        "ai.providers.test",
        |app: App, params: IdParams| async move { test(&app, &params.id).await },
    );

    router.add(
        "ai.providers.models",
        |app: App, params: ModelsParams| async move { models(&app, params).await },
    );

    router.add(
        "ai.providers.detect_local",
        |_app: App, _: NoParams| async move { Ok::<Vec<LocalEndpoint>, AppError>(detect_local().await) },
    );

    router.add("ai.status", |app: App, _: NoParams| async move {
        let default = default_provider(&app).ok();
        let small = small_default(&app, default.as_ref());
        Ok(Status {
            configured: default.is_some(),
            default: default.map(|record| entry(&app, record)),
            autonomy: crate::settings::Settings::read(&app).autonomy.as_str(),
            redaction: true,
            tools: crate::tools::available(&app, small).len(),
        })
    });

    router.add("ai.tools", |app: App, _: NoParams| async move {
        let small = small_default(&app, default_provider(&app).ok().as_ref());
        Ok(crate::tools::available(&app, small)
            .into_iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "title": tool.title,
                    "touches_device": tool.touches_device,
                    "methods": tool.reachable().map(|method| method.name()).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>())
    });
}
