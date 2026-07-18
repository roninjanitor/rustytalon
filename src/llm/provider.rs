//! LLM provider trait and types.

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;

/// A media attachment that can accompany a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Attachment {
    /// An image referenced by URL (fetched by the model at inference time).
    ImageUrl { url: String },
    /// An image supplied as raw base64 data.
    ImageBase64 {
        /// MIME media type: "image/jpeg", "image/png", "image/gif", or "image/webp".
        media_type: String,
        /// Base64-encoded image bytes.
        data: String,
    },
}

impl Attachment {
    /// Create an `ImageUrl` attachment.
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::ImageUrl { url: url.into() }
    }
}

/// Role in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Media attachments (images) that accompany this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Tool call ID if this is a tool result message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Name of the tool for tool results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls made by the assistant (OpenAI protocol requires these
    /// to appear on the assistant message preceding tool result messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl ChatMessage {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    /// Create a user message with media attachments.
    pub fn user_with_attachments(content: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            attachments,
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    /// Create an assistant message that includes tool calls.
    ///
    /// Per the OpenAI protocol, an assistant message with tool_calls must
    /// precede the corresponding tool result messages in the conversation.
    pub fn assistant_with_tool_calls(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.unwrap_or_default(),
            attachments: Vec::new(),
            tool_call_id: None,
            name: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        }
    }

    /// Create a tool result message.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            attachments: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
            tool_calls: None,
        }
    }
}

/// Request for a chat completion.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stop_sequences: Option<Vec<String>>,
    /// Opaque metadata passed through to the provider (e.g. thread_id for chaining).
    pub metadata: std::collections::HashMap<String, String>,
}

impl CompletionRequest {
    /// Create a new completion request.
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            max_tokens: None,
            temperature: None,
            stop_sequences: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Set max tokens.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// Response from a chat completion.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub finish_reason: FinishReason,
    /// Provider-specific response ID (e.g. for response chaining).
    pub response_id: Option<String>,
}

/// Why the completion finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ToolUse,
    ContentFilter,
    Unknown,
}

/// Definition of a tool for the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result of a tool execution to send back to the LLM.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// Request for a completion with tool use.
#[derive(Debug, Clone)]
pub struct ToolCompletionRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// How to handle tool use: "auto", "required", or "none".
    pub tool_choice: Option<String>,
    /// Opaque metadata passed through to the provider (e.g. thread_id for chaining).
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolCompletionRequest {
    /// Create a new tool completion request.
    pub fn new(messages: Vec<ChatMessage>, tools: Vec<ToolDefinition>) -> Self {
        Self {
            messages,
            tools,
            max_tokens: None,
            temperature: None,
            tool_choice: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Set max tokens.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set tool choice mode.
    pub fn with_tool_choice(mut self, choice: impl Into<String>) -> Self {
        self.tool_choice = Some(choice.into());
        self
    }
}

/// Response from a completion with potential tool calls.
#[derive(Debug, Clone)]
pub struct ToolCompletionResponse {
    /// Text content (may be empty if tool calls are present).
    pub content: Option<String>,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub finish_reason: FinishReason,
    /// Provider-specific response ID (e.g. for response chaining).
    pub response_id: Option<String>,
}

/// Metadata about a model returned by the provider's API.
#[derive(Debug, Clone)]
pub struct ModelMetadata {
    pub id: String,
    /// Total context window size in tokens.
    pub context_length: Option<u32>,
}

/// Trait for LLM providers.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get the model name.
    fn model_name(&self) -> &str;

    /// Get cost per token (input, output).
    fn cost_per_token(&self) -> (Decimal, Decimal);

    /// Complete a chat conversation.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Complete with tool use support.
    async fn complete_with_tools(
        &self,
        request: ToolCompletionRequest,
    ) -> Result<ToolCompletionResponse, LlmError>;

    /// Complete a chat conversation, forwarding incremental text chunks over
    /// `chunk_tx` as they arrive so callers can show live output.
    ///
    /// Default implementation buffers the full response from `complete()`
    /// and sends it as a single chunk at the end -- providers/wrappers that
    /// don't override this still work correctly, just without incremental
    /// output. Only `RigAdapter` currently overrides this with real
    /// provider-level streaming.
    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<CompletionResponse, LlmError> {
        let response = self.complete(request).await?;
        let _ = chunk_tx.send(response.content.clone());
        Ok(response)
    }

    /// Complete with tool use support, forwarding incremental text chunks
    /// (not tool-call argument deltas) over `chunk_tx` as they arrive.
    ///
    /// Same fallback behavior as `complete_streaming`: default buffers and
    /// sends one final chunk.
    async fn complete_with_tools_streaming(
        &self,
        request: ToolCompletionRequest,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<ToolCompletionResponse, LlmError> {
        let response = self.complete_with_tools(request).await?;
        if let Some(content) = &response.content {
            let _ = chunk_tx.send(content.clone());
        }
        Ok(response)
    }

    /// List available models from the provider.
    /// Default implementation returns empty list.
    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        Ok(Vec::new())
    }

    /// Fetch metadata for the current model (context length, etc.).
    /// Default returns the model name with no size info.
    async fn model_metadata(&self) -> Result<ModelMetadata, LlmError> {
        Ok(ModelMetadata {
            id: self.model_name().to_string(),
            context_length: None,
        })
    }

    /// Get the currently active model name.
    ///
    /// May differ from `model_name()` if the model was switched at runtime
    /// via `set_model()`. Default returns `model_name()`.
    fn active_model_name(&self) -> String {
        self.model_name().to_string()
    }

    /// Switch the active model at runtime. Not all providers support this.
    fn set_model(&self, _model: &str) -> Result<(), LlmError> {
        Err(LlmError::RequestFailed {
            provider: "unknown".to_string(),
            reason: "Runtime model switching not supported by this provider".to_string(),
        })
    }

    /// Seed a response chain for a thread (e.g. restoring from DB).
    ///
    /// Providers that support response chaining store this so subsequent calls
    /// send only delta messages.
    fn seed_response_chain(&self, _thread_id: &str, _response_id: String) {}

    /// Get the last response chain ID for a thread.
    ///
    /// Returns `None` if the provider doesn't support chaining or has no
    /// stored state for this thread.
    fn get_response_chain_id(&self, _thread_id: &str) -> Option<String> {
        None
    }

    /// Calculate cost for a completion.
    fn calculate_cost(&self, input_tokens: u32, output_tokens: u32) -> Decimal {
        let (input_cost, output_cost) = self.cost_per_token();
        input_cost * Decimal::from(input_tokens) + output_cost * Decimal::from(output_tokens)
    }
}
