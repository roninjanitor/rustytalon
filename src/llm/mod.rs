//! LLM integration for the agent.
//!
//! Supports multiple backends:
//! - **Anthropic**: Direct API access with your own key (default)
//! - **OpenAI**: Direct API access with your own key
//! - **Ollama**: Local model inference
//! - **OpenAI-compatible**: Any endpoint that speaks the OpenAI API
//!
//! # Smart Routing
//!
//! The `routing` module provides intelligent query routing between providers
//! based on complexity analysis, cost optimization, and provider health.

mod costs;
pub mod failover;
mod provider;
mod reasoning;
mod retry;
mod rig_adapter;
pub mod routing;
pub mod tracked;

#[cfg(test)]
pub(crate) mod test_utils;

pub use costs::{default_cost, model_cost};
pub use failover::FailoverProvider;
pub use provider::{
    ChatMessage, CompletionRequest, CompletionResponse, FinishReason, LlmProvider, ModelMetadata,
    Role, ToolCall, ToolCompletionRequest, ToolCompletionResponse, ToolDefinition, ToolResult,
};
pub use reasoning::{
    ActionPlan, Reasoning, ReasoningContext, RespondOutput, RespondResult, TokenUsage,
    ToolSelection,
};
pub use rig_adapter::RigAdapter;
pub use tracked::{LlmCallStats, TrackedProvider};

use std::sync::Arc;

use rig::client::CompletionClient;
use secrecy::ExposeSecret;

use crate::config::{LlmBackend, LlmConfig};
use crate::error::LlmError;

/// Create an LLM provider for the primary backend.
///
/// Supports Anthropic, OpenAI, Ollama, and OpenAI-compatible backends.
pub fn create_llm_provider(config: &LlmConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
    match config.backend {
        LlmBackend::Anthropic => create_anthropic_provider(config),
        LlmBackend::OpenAi => create_openai_provider(config),
        LlmBackend::Ollama => create_ollama_provider(config),
        LlmBackend::OpenAiCompatible => create_openai_compatible_provider(config),
    }
}

/// A backend paired with its provider instance.
pub type BackendProvider = (LlmBackend, Arc<dyn LlmProvider>);

/// Create providers for all backends that have credentials configured.
///
/// Returns a list of `(backend, provider)` pairs. The primary backend is
/// always first. Additional backends with env vars present are included
/// as potential fallback/routing targets.
pub fn create_all_providers(config: &LlmConfig) -> Result<Vec<BackendProvider>, LlmError> {
    let mut providers = Vec::new();

    // Primary backend first (must succeed)
    let primary = create_llm_provider(config)?;
    providers.push((config.backend, primary));

    // Try to create providers for other backends that have configs populated
    for &backend in &config.available_backends() {
        if backend == config.backend {
            continue; // already added as primary
        }

        let result = match backend {
            LlmBackend::Anthropic => create_anthropic_provider(config),
            LlmBackend::OpenAi => create_openai_provider(config),
            LlmBackend::Ollama => create_ollama_provider(config),
            LlmBackend::OpenAiCompatible => create_openai_compatible_provider(config),
        };

        match result {
            Ok(provider) => {
                tracing::info!(
                    "Additional LLM provider available for routing/failover: {} ({})",
                    backend,
                    provider.model_name()
                );
                providers.push((backend, provider));
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create {} provider (skipping as fallback): {}",
                    backend,
                    e
                );
            }
        }
    }

    Ok(providers)
}

fn create_openai_provider(config: &LlmConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let oai = config.openai.as_ref().ok_or_else(|| LlmError::AuthFailed {
        provider: "openai".to_string(),
    })?;

    use rig::providers::openai;

    let client: openai::Client =
        openai::Client::new(oai.api_key.expose_secret()).map_err(|e| LlmError::RequestFailed {
            provider: "openai".to_string(),
            reason: format!("Failed to create OpenAI client: {}", e),
        })?;

    let model = client.completion_model(&oai.model);
    tracing::info!("Using OpenAI direct API (model: {})", oai.model);
    Ok(Arc::new(RigAdapter::new(model, &oai.model)))
}

fn create_anthropic_provider(config: &LlmConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let anth = config
        .anthropic
        .as_ref()
        .ok_or_else(|| LlmError::AuthFailed {
            provider: "anthropic".to_string(),
        })?;

    use rig::providers::anthropic;

    let mut builder =
        anthropic::Client::builder().api_key(anth.api_key.expose_secret().to_string());

    if let Some(ref base_url) = anth.base_url {
        builder = builder.base_url(base_url);
    }

    if !anth.extra_headers.is_empty() {
        let mut headers = http::HeaderMap::new();
        for (key, value) in &anth.extra_headers {
            tracing::debug!("Adding extra header: {} = {}", key, value);
            let name = http::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                LlmError::RequestFailed {
                    provider: "anthropic".to_string(),
                    reason: format!("Invalid header name '{key}': {e}"),
                }
            })?;
            let val = http::header::HeaderValue::from_str(value).map_err(|e| {
                LlmError::RequestFailed {
                    provider: "anthropic".to_string(),
                    reason: format!("Invalid header value for '{key}': {e}"),
                }
            })?;
            headers.insert(name, val);
        }
        tracing::debug!(
            "Configured {} extra headers (http_headers should merge with api_key)",
            headers.len()
        );
        builder = builder.http_headers(headers);
    }

    let client: anthropic::Client = builder.build().map_err(|e| LlmError::RequestFailed {
        provider: "anthropic".to_string(),
        reason: format!("Failed to create Anthropic client: {}", e),
    })?;

    let model = client.completion_model(&anth.model);
    let display_url = anth
        .base_url
        .as_deref()
        .unwrap_or("https://api.anthropic.com");
    tracing::info!(
        "Using Anthropic API (base_url: {}, model: {})",
        display_url,
        anth.model
    );
    Ok(Arc::new(RigAdapter::new(model, &anth.model)))
}

fn create_ollama_provider(config: &LlmConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let oll = config.ollama.as_ref().ok_or_else(|| LlmError::AuthFailed {
        provider: "ollama".to_string(),
    })?;

    use rig::client::Nothing;
    use rig::providers::ollama;

    let client: ollama::Client = ollama::Client::builder()
        .base_url(&oll.base_url)
        .api_key(Nothing)
        .build()
        .map_err(|e| LlmError::RequestFailed {
            provider: "ollama".to_string(),
            reason: format!("Failed to create Ollama client: {}", e),
        })?;

    let model = client.completion_model(&oll.model);
    tracing::info!(
        "Using Ollama (base_url: {}, model: {})",
        oll.base_url,
        oll.model
    );
    Ok(Arc::new(RigAdapter::new(model, &oll.model)))
}

fn create_openai_compatible_provider(config: &LlmConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let compat = config
        .openai_compatible
        .as_ref()
        .ok_or_else(|| LlmError::AuthFailed {
            provider: "openai_compatible".to_string(),
        })?;

    use rig::providers::openai;

    let api_key = compat
        .api_key
        .as_ref()
        .map(|k| k.expose_secret().to_string())
        .unwrap_or_else(|| "no-key".to_string());

    let mut builder = openai::Client::builder()
        .base_url(&compat.base_url)
        .api_key(api_key);

    if !compat.extra_headers.is_empty() {
        let mut headers = http::HeaderMap::new();
        for (key, value) in &compat.extra_headers {
            let name = http::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                LlmError::RequestFailed {
                    provider: "openai_compatible".to_string(),
                    reason: format!("Invalid header name '{key}': {e}"),
                }
            })?;
            let val = http::header::HeaderValue::from_str(value).map_err(|e| {
                LlmError::RequestFailed {
                    provider: "openai_compatible".to_string(),
                    reason: format!("Invalid header value for '{key}': {e}"),
                }
            })?;
            headers.insert(name, val);
        }
        builder = builder.http_headers(headers);
    }

    let client: openai::Client = builder.build().map_err(|e| LlmError::RequestFailed {
        provider: "openai_compatible".to_string(),
        reason: format!("Failed to create OpenAI-compatible client: {e}"),
    })?;

    let model = client.completion_model(&compat.model);
    tracing::info!(
        "Using OpenAI-compatible endpoint (base_url: {}, model: {})",
        compat.base_url,
        compat.model
    );
    Ok(Arc::new(RigAdapter::new(model, &compat.model)))
}
