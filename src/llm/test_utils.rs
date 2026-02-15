//! Shared test utilities for LLM tests.
//!
//! Provides mock providers that can be used across all LLM-related test modules.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use rust_decimal::Decimal;

use crate::error::LlmError;
use crate::llm::provider::{
    CompletionRequest, CompletionResponse, FinishReason, LlmProvider, ToolCompletionRequest,
    ToolCompletionResponse,
};

/// A mock LLM provider that returns a single predetermined result.
///
/// Panics if called more than once (use `MultiCallMockProvider` for retry tests).
pub(crate) struct MockProvider {
    pub name: String,
    pub input_cost: Decimal,
    pub output_cost: Decimal,
    pub complete_result: Mutex<Option<Result<CompletionResponse, LlmError>>>,
    pub tool_complete_result: Mutex<Option<Result<ToolCompletionResponse, LlmError>>>,
}

impl MockProvider {
    pub fn succeeding(name: &str, content: &str) -> Self {
        Self {
            name: name.to_string(),
            input_cost: Decimal::ZERO,
            output_cost: Decimal::ZERO,
            complete_result: Mutex::new(Some(Ok(CompletionResponse {
                content: content.to_string(),
                input_tokens: 10,
                output_tokens: 5,
                finish_reason: FinishReason::Stop,
                response_id: None,
            }))),
            tool_complete_result: Mutex::new(Some(Ok(ToolCompletionResponse {
                content: Some(content.to_string()),
                tool_calls: vec![],
                input_tokens: 10,
                output_tokens: 5,
                finish_reason: FinishReason::Stop,
                response_id: None,
            }))),
        }
    }

    pub fn succeeding_with_cost(
        name: &str,
        content: &str,
        input_cost: Decimal,
        output_cost: Decimal,
    ) -> Self {
        Self {
            input_cost,
            output_cost,
            ..Self::succeeding(name, content)
        }
    }

    pub fn failing_retryable(name: &str) -> Self {
        Self {
            name: name.to_string(),
            input_cost: Decimal::ZERO,
            output_cost: Decimal::ZERO,
            complete_result: Mutex::new(Some(Err(LlmError::RequestFailed {
                provider: name.to_string(),
                reason: "server error".to_string(),
            }))),
            tool_complete_result: Mutex::new(Some(Err(LlmError::RequestFailed {
                provider: name.to_string(),
                reason: "server error".to_string(),
            }))),
        }
    }

    pub fn failing_non_retryable(name: &str) -> Self {
        Self {
            name: name.to_string(),
            input_cost: Decimal::ZERO,
            output_cost: Decimal::ZERO,
            complete_result: Mutex::new(Some(Err(LlmError::AuthFailed {
                provider: name.to_string(),
            }))),
            tool_complete_result: Mutex::new(Some(Err(LlmError::AuthFailed {
                provider: name.to_string(),
            }))),
        }
    }

    pub fn failing_rate_limited(name: &str) -> Self {
        Self {
            name: name.to_string(),
            input_cost: Decimal::ZERO,
            output_cost: Decimal::ZERO,
            complete_result: Mutex::new(Some(Err(LlmError::RateLimited {
                provider: name.to_string(),
                retry_after: Some(Duration::from_secs(30)),
            }))),
            tool_complete_result: Mutex::new(Some(Err(LlmError::RateLimited {
                provider: name.to_string(),
                retry_after: Some(Duration::from_secs(30)),
            }))),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn model_name(&self) -> &str {
        &self.name
    }

    fn cost_per_token(&self) -> (Decimal, Decimal) {
        (self.input_cost, self.output_cost)
    }

    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.complete_result
            .lock()
            .unwrap()
            .take()
            .expect("MockProvider::complete called more than once")
    }

    async fn complete_with_tools(
        &self,
        _request: ToolCompletionRequest,
    ) -> Result<ToolCompletionResponse, LlmError> {
        self.tool_complete_result
            .lock()
            .unwrap()
            .take()
            .expect("MockProvider::complete_with_tools called more than once")
    }

    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        Ok(vec![self.name.clone()])
    }
}

/// A mock LLM provider that supports multiple calls via a queue of results.
///
/// Each call to `complete()` or `complete_with_tools()` pops the next result
/// from the queue. Panics if the queue is empty.
pub(crate) struct MultiCallMockProvider {
    pub name: String,
    pub input_cost: Decimal,
    pub output_cost: Decimal,
    pub complete_results: Mutex<VecDeque<Result<CompletionResponse, LlmError>>>,
    pub tool_complete_results: Mutex<VecDeque<Result<ToolCompletionResponse, LlmError>>>,
}

impl MultiCallMockProvider {
    /// Create a provider that fails `fail_count` times with a retryable error,
    /// then succeeds with the given content.
    pub fn failing_then_succeeding(name: &str, fail_count: usize, content: &str) -> Self {
        let mut complete_results = VecDeque::new();
        let mut tool_results = VecDeque::new();

        for _ in 0..fail_count {
            complete_results.push_back(Err(LlmError::RequestFailed {
                provider: name.to_string(),
                reason: "transient error".to_string(),
            }));
            tool_results.push_back(Err(LlmError::RequestFailed {
                provider: name.to_string(),
                reason: "transient error".to_string(),
            }));
        }

        complete_results.push_back(Ok(CompletionResponse {
            content: content.to_string(),
            input_tokens: 10,
            output_tokens: 5,
            finish_reason: FinishReason::Stop,
            response_id: None,
        }));
        tool_results.push_back(Ok(ToolCompletionResponse {
            content: Some(content.to_string()),
            tool_calls: vec![],
            input_tokens: 10,
            output_tokens: 5,
            finish_reason: FinishReason::Stop,
            response_id: None,
        }));

        Self {
            name: name.to_string(),
            input_cost: Decimal::ZERO,
            output_cost: Decimal::ZERO,
            complete_results: Mutex::new(complete_results),
            tool_complete_results: Mutex::new(tool_results),
        }
    }

    /// Create a provider that always fails with a retryable error.
    pub fn always_failing(name: &str, count: usize) -> Self {
        let mut complete_results = VecDeque::new();
        let mut tool_results = VecDeque::new();

        for _ in 0..count {
            complete_results.push_back(Err(LlmError::RequestFailed {
                provider: name.to_string(),
                reason: "persistent error".to_string(),
            }));
            tool_results.push_back(Err(LlmError::RequestFailed {
                provider: name.to_string(),
                reason: "persistent error".to_string(),
            }));
        }

        Self {
            name: name.to_string(),
            input_cost: Decimal::ZERO,
            output_cost: Decimal::ZERO,
            complete_results: Mutex::new(complete_results),
            tool_complete_results: Mutex::new(tool_results),
        }
    }
}

#[async_trait]
impl LlmProvider for MultiCallMockProvider {
    fn model_name(&self) -> &str {
        &self.name
    }

    fn cost_per_token(&self) -> (Decimal, Decimal) {
        (self.input_cost, self.output_cost)
    }

    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.complete_results
            .lock()
            .unwrap()
            .pop_front()
            .expect("MultiCallMockProvider::complete called more times than expected")
    }

    async fn complete_with_tools(
        &self,
        _request: ToolCompletionRequest,
    ) -> Result<ToolCompletionResponse, LlmError> {
        self.tool_complete_results
            .lock()
            .unwrap()
            .pop_front()
            .expect("MultiCallMockProvider::complete_with_tools called more times than expected")
    }

    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        Ok(vec![self.name.clone()])
    }
}

/// Create a simple `CompletionRequest` for tests.
pub(crate) fn make_request() -> CompletionRequest {
    CompletionRequest::new(vec![crate::llm::ChatMessage::user("hello")])
}

/// Create a simple `ToolCompletionRequest` for tests.
pub(crate) fn make_tool_request() -> ToolCompletionRequest {
    ToolCompletionRequest::new(vec![crate::llm::ChatMessage::user("hello")], vec![])
}
