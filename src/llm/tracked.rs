//! Tracked LLM provider wrapper.
//!
//! Wraps any `LlmProvider` to add:
//! - Per-request cost calculation and database recording
//! - Retry with exponential backoff on transient errors
//! - Latency measurement and logging

use std::sync::Arc;
use std::time::Instant;

use crate::db::Database;
use crate::error::LlmError;
use crate::llm::provider::{
    CompletionRequest, CompletionResponse, LlmProvider, ModelMetadata, ToolCompletionRequest,
    ToolCompletionResponse,
};
use crate::llm::retry::retry_backoff_delay;
use async_trait::async_trait;
use rust_decimal::Decimal;

/// Returns `true` if the LLM error is transient and worth retrying.
pub fn is_retryable_error(err: &LlmError) -> bool {
    matches!(
        err,
        LlmError::RequestFailed { .. }
            | LlmError::RateLimited { .. }
            | LlmError::InvalidResponse { .. }
            | LlmError::SessionRenewalFailed { .. }
            | LlmError::Http(_)
            | LlmError::Io(_)
    )
}

/// An LLM provider wrapper that records calls to the database and retries
/// transient failures with exponential backoff.
pub struct TrackedProvider {
    inner: Arc<dyn LlmProvider>,
    db: Arc<dyn Database>,
    max_retries: u32,
    /// Provider name for recording (e.g. "anthropic", "openai").
    provider_name: String,
}

impl TrackedProvider {
    /// Create a new tracked provider.
    ///
    /// - `inner`: The underlying LLM provider to delegate to.
    /// - `db`: Database for recording LLM call metrics.
    /// - `max_retries`: Maximum retry attempts on transient errors (0 = no retries).
    /// - `provider_name`: Human-readable provider name for cost tracking.
    pub fn new(
        inner: Arc<dyn LlmProvider>,
        db: Arc<dyn Database>,
        max_retries: u32,
        provider_name: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            db,
            max_retries,
            provider_name: provider_name.into(),
        }
    }

    /// Record an LLM call to the database.
    ///
    /// Best-effort: logs a warning on failure rather than propagating the error.
    async fn record_call(
        &self,
        input_tokens: u32,
        output_tokens: u32,
        cost: Decimal,
        purpose: &str,
        latency_ms: u64,
    ) {
        let record = crate::history::LlmCallRecord {
            job_id: None,
            conversation_id: None,
            provider: &self.provider_name,
            model: self.inner.model_name(),
            input_tokens,
            output_tokens,
            cost,
            purpose: Some(purpose),
        };

        if let Err(e) = self.db.record_llm_call(&record).await {
            tracing::warn!(
                provider = %self.provider_name,
                model = %self.inner.model_name(),
                latency_ms,
                error = %e,
                "Failed to record LLM call"
            );
        } else {
            tracing::debug!(
                provider = %self.provider_name,
                model = %self.inner.model_name(),
                input_tokens,
                output_tokens,
                cost = %cost,
                latency_ms,
                "Recorded LLM call"
            );
        }
    }
}

#[async_trait]
impl LlmProvider for TrackedProvider {
    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn cost_per_token(&self) -> (Decimal, Decimal) {
        self.inner.cost_per_token()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let start = Instant::now();
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = retry_backoff_delay(attempt - 1);
                tracing::info!(
                    provider = %self.provider_name,
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "Retrying LLM request"
                );
                tokio::time::sleep(delay).await;
            }

            match self.inner.complete(request.clone()).await {
                Ok(response) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    let cost = self
                        .inner
                        .calculate_cost(response.input_tokens, response.output_tokens);
                    self.record_call(
                        response.input_tokens,
                        response.output_tokens,
                        cost,
                        "complete",
                        latency_ms,
                    )
                    .await;
                    return Ok(response);
                }
                Err(e) => {
                    if attempt < self.max_retries && is_retryable_error(&e) {
                        tracing::warn!(
                            provider = %self.provider_name,
                            attempt,
                            error = %e,
                            "Transient LLM error, will retry"
                        );
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::RequestFailed {
            provider: self.provider_name.clone(),
            reason: "All retry attempts exhausted".to_string(),
        }))
    }

    async fn complete_with_tools(
        &self,
        request: ToolCompletionRequest,
    ) -> Result<ToolCompletionResponse, LlmError> {
        let start = Instant::now();
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = retry_backoff_delay(attempt - 1);
                tracing::info!(
                    provider = %self.provider_name,
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "Retrying LLM tool request"
                );
                tokio::time::sleep(delay).await;
            }

            match self.inner.complete_with_tools(request.clone()).await {
                Ok(response) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    let cost = self
                        .inner
                        .calculate_cost(response.input_tokens, response.output_tokens);
                    self.record_call(
                        response.input_tokens,
                        response.output_tokens,
                        cost,
                        "complete_with_tools",
                        latency_ms,
                    )
                    .await;
                    return Ok(response);
                }
                Err(e) => {
                    if attempt < self.max_retries && is_retryable_error(&e) {
                        tracing::warn!(
                            provider = %self.provider_name,
                            attempt,
                            error = %e,
                            "Transient LLM tool error, will retry"
                        );
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::RequestFailed {
            provider: self.provider_name.clone(),
            reason: "All retry attempts exhausted".to_string(),
        }))
    }

    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        self.inner.list_models().await
    }

    async fn model_metadata(&self) -> Result<ModelMetadata, LlmError> {
        self.inner.model_metadata().await
    }

    fn active_model_name(&self) -> String {
        self.inner.active_model_name()
    }

    fn set_model(&self, model: &str) -> Result<(), LlmError> {
        self.inner.set_model(model)
    }

    fn seed_response_chain(&self, thread_id: &str, response_id: String) {
        self.inner.seed_response_chain(thread_id, response_id);
    }

    fn get_response_chain_id(&self, thread_id: &str) -> Option<String> {
        self.inner.get_response_chain_id(thread_id)
    }
}

/// Summary of LLM call statistics aggregated from the database.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmCallStats {
    /// Provider name.
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Total number of calls.
    pub total_calls: i64,
    /// Total input tokens consumed.
    pub total_input_tokens: i64,
    /// Total output tokens generated.
    pub total_output_tokens: i64,
    /// Total cost in USD.
    pub total_cost: Decimal,
    /// Average cost per call.
    pub avg_cost_per_call: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_error() {
        // Retryable errors
        assert!(is_retryable_error(&LlmError::RequestFailed {
            provider: "p".into(),
            reason: "err".into(),
        }));
        assert!(is_retryable_error(&LlmError::RateLimited {
            provider: "p".into(),
            retry_after: None,
        }));
        assert!(is_retryable_error(&LlmError::InvalidResponse {
            provider: "p".into(),
            reason: "bad".into(),
        }));

        // Non-retryable errors
        assert!(!is_retryable_error(&LlmError::AuthFailed {
            provider: "p".into(),
        }));
        assert!(!is_retryable_error(&LlmError::ContextLengthExceeded {
            used: 100_000,
            limit: 50_000,
        }));
        assert!(!is_retryable_error(&LlmError::ModelNotAvailable {
            provider: "p".into(),
            model: "m".into(),
        }));
    }
}
