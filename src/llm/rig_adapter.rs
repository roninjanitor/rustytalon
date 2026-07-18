//! Generic adapter that bridges rig-core's `CompletionModel` trait to RustyTalon's `LlmProvider`.
//!
//! This lets us use any rig-core provider (OpenAI, Anthropic, Ollama, etc.) as an
//! `Arc<dyn LlmProvider>` without changing any of the agent, reasoning, or tool code.

use async_trait::async_trait;
use rig::OneOrMany;
use rig::completion::message::DocumentSourceKind;
use rig::completion::{
    AssistantContent, CompletionModel, CompletionRequest as RigRequest,
    ToolDefinition as RigToolDefinition, Usage as RigUsage,
};
use rig::message::{
    Image as RigImage, ImageDetail as RigImageDetail, ImageMediaType as RigImageMediaType,
    Message as RigMessage, ToolChoice as RigToolChoice, ToolFunction, ToolResult as RigToolResult,
    ToolResultContent, UserContent,
};
use rust_decimal::Decimal;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::LlmError;
use crate::llm::costs;
use crate::llm::provider::{
    Attachment, ChatMessage, CompletionRequest, CompletionResponse, FinishReason, LlmProvider,
    ToolCall as IronToolCall, ToolCompletionRequest, ToolCompletionResponse,
    ToolDefinition as IronToolDefinition,
};

/// Parse a `Retry-After` value (integer seconds) from an error message string.
///
/// Handles both `Retry-After: 30` and `retry-after: 30` (case-insensitive).
/// Returns `None` if the header is absent or cannot be parsed.
fn parse_retry_after(msg: &str) -> Option<std::time::Duration> {
    let lower = msg.to_lowercase();
    let pos = lower.find("retry-after:")?;
    let after = msg[pos + "retry-after:".len()..].trim_start();
    let end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    after[..end]
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
}

/// Map a rig completion error to the appropriate `LlmError` variant.
///
/// HTTP 400 responses are mapped to `ModelNotAvailable` so that:
/// - `TrackedProvider` does **not** retry them (400s won't succeed on retry)
/// - `FailoverProvider` **does** fail over to the next provider immediately
///
/// HTTP 429 responses are mapped to `RateLimited` so that:
/// - `TrackedProvider` does **not** retry them on the same provider (already capped)
/// - `FailoverProvider` **does** fail over to the next provider after a backoff delay
///
/// All other errors map to `RequestFailed` and follow the normal retry path.
fn map_rig_error(model_name: &str, err: impl std::fmt::Display) -> LlmError {
    let msg = err.to_string();
    if msg.contains("status code 400") || msg.contains("400 Bad Request") {
        LlmError::ModelNotAvailable {
            provider: model_name.to_string(),
            model: model_name.to_string(),
        }
    } else if msg.contains("status code 429") || msg.contains("429 Too Many Requests") {
        LlmError::RateLimited {
            provider: model_name.to_string(),
            retry_after: parse_retry_after(&msg),
        }
    } else {
        LlmError::RequestFailed {
            provider: model_name.to_string(),
            reason: msg,
        }
    }
}

/// Adapter that wraps a rig-core `CompletionModel` and implements `LlmProvider`.
pub struct RigAdapter<M: CompletionModel> {
    model: M,
    model_name: String,
    input_cost: Decimal,
    output_cost: Decimal,
}

impl<M: CompletionModel> RigAdapter<M> {
    /// Create a new adapter wrapping the given rig-core model.
    pub fn new(model: M, model_name: impl Into<String>) -> Self {
        let name = model_name.into();
        let (input_cost, output_cost) =
            costs::model_cost(&name).unwrap_or_else(costs::default_cost);
        Self {
            model,
            model_name: name,
            input_cost,
            output_cost,
        }
    }
}

// -- Type conversion helpers --

/// Convert an `Attachment` to a rig-core `UserContent` image part.
fn attachment_to_user_content(att: &Attachment) -> UserContent {
    match att {
        Attachment::ImageUrl { url } => {
            UserContent::image_url(url, None, Some(RigImageDetail::Auto))
        }
        Attachment::ImageBase64 { media_type, data } => {
            let rig_media_type = match media_type.as_str() {
                "image/jpeg" | "image/jpg" => Some(RigImageMediaType::JPEG),
                "image/png" => Some(RigImageMediaType::PNG),
                "image/gif" => Some(RigImageMediaType::GIF),
                "image/webp" => Some(RigImageMediaType::WEBP),
                _ => None,
            };
            if let Some(mt) = rig_media_type {
                UserContent::image_base64(data, Some(mt), Some(RigImageDetail::Auto))
            } else {
                // Unknown media type: wrap as URL using a data URI so the model
                // still sees the bytes rather than silently dropping the image.
                UserContent::Image(RigImage {
                    data: DocumentSourceKind::Base64(data.clone()),
                    media_type: None,
                    detail: None,
                    additional_params: None,
                })
            }
        }
    }
}

/// Convert RustyTalon messages to rig-core format.
///
/// Returns `(preamble, chat_history)` where preamble is extracted from
/// any System message and chat_history contains the rest.
fn convert_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<RigMessage>) {
    let mut preamble: Option<String> = None;
    let mut history = Vec::new();

    for msg in messages {
        match msg.role {
            crate::llm::Role::System => {
                // Concatenate system messages into preamble
                match preamble {
                    Some(ref mut p) => {
                        p.push('\n');
                        p.push_str(&msg.content);
                    }
                    None => preamble = Some(msg.content.clone()),
                }
            }
            crate::llm::Role::User => {
                if msg.attachments.is_empty() {
                    history.push(RigMessage::user(&msg.content));
                } else {
                    let mut parts: Vec<UserContent> = Vec::new();
                    if !msg.content.is_empty() {
                        parts.push(UserContent::text(&msg.content));
                    }
                    for att in &msg.attachments {
                        parts.push(attachment_to_user_content(att));
                    }
                    // Fall back to plain text if we somehow end up with nothing
                    if parts.is_empty() {
                        parts.push(UserContent::text(""));
                    }
                    match OneOrMany::many(parts) {
                        Ok(content) => history.push(RigMessage::User { content }),
                        Err(_) => history.push(RigMessage::user(&msg.content)),
                    }
                }
            }
            crate::llm::Role::Assistant => {
                if let Some(ref tool_calls) = msg.tool_calls {
                    // Assistant message with tool calls
                    let mut contents: Vec<AssistantContent> = Vec::new();
                    if !msg.content.is_empty() {
                        contents.push(AssistantContent::text(&msg.content));
                    }
                    for tc in tool_calls {
                        contents.push(AssistantContent::ToolCall(rig::message::ToolCall::new(
                            tc.id.clone(),
                            ToolFunction::new(tc.name.clone(), tc.arguments.clone()),
                        )));
                    }
                    if let Ok(many) = OneOrMany::many(contents) {
                        history.push(RigMessage::Assistant {
                            id: None,
                            content: many,
                        });
                    } else {
                        // Shouldn't happen but fall back to text
                        history.push(RigMessage::assistant(&msg.content));
                    }
                } else {
                    history.push(RigMessage::assistant(&msg.content));
                }
            }
            crate::llm::Role::Tool => {
                // Tool result message: wrap as User { ToolResult }
                let tool_id = msg.tool_call_id.clone().unwrap_or_default();
                history.push(RigMessage::User {
                    content: OneOrMany::one(UserContent::ToolResult(RigToolResult {
                        id: tool_id,
                        call_id: None,
                        content: OneOrMany::one(ToolResultContent::text(&msg.content)),
                    })),
                });
            }
        }
    }

    (preamble, history)
}

/// Convert RustyTalon tool definitions to rig-core format.
fn convert_tools(tools: &[IronToolDefinition]) -> Vec<RigToolDefinition> {
    tools
        .iter()
        .map(|t| RigToolDefinition {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        })
        .collect()
}

/// Convert RustyTalon tool_choice string to rig-core ToolChoice.
fn convert_tool_choice(choice: Option<&str>) -> Option<RigToolChoice> {
    match choice.map(|s| s.to_lowercase()).as_deref() {
        Some("auto") => Some(RigToolChoice::Auto),
        Some("required") => Some(RigToolChoice::Required),
        Some("none") => Some(RigToolChoice::None),
        _ => None,
    }
}

/// Extract text and tool calls from a rig-core completion response.
fn extract_response(
    choice: &OneOrMany<AssistantContent>,
    _usage: &RigUsage,
) -> (Option<String>, Vec<IronToolCall>, FinishReason) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<IronToolCall> = Vec::new();

    for content in choice.iter() {
        match content {
            AssistantContent::Text(t) if !t.text.is_empty() => {
                text_parts.push(t.text.clone());
            }
            AssistantContent::ToolCall(tc) => {
                tool_calls.push(IronToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                });
            }
            // Reasoning and Image variants are not mapped to RustyTalon types
            _ => {}
        }
    }

    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };

    let finish = if !tool_calls.is_empty() {
        FinishReason::ToolUse
    } else {
        FinishReason::Stop
    };

    (text, tool_calls, finish)
}

/// Saturate u64 to u32 for token counts.
fn saturate_u32(val: u64) -> u32 {
    val.min(u32::MAX as u64) as u32
}

/// Build a rig-core CompletionRequest from our internal types.
fn build_rig_request(
    preamble: Option<String>,
    mut history: Vec<RigMessage>,
    tools: Vec<RigToolDefinition>,
    tool_choice: Option<RigToolChoice>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Result<RigRequest, LlmError> {
    // rig-core requires at least one message in chat_history
    if history.is_empty() {
        history.push(RigMessage::user("Hello"));
    }

    let chat_history = OneOrMany::many(history).map_err(|e| LlmError::RequestFailed {
        provider: "rig".to_string(),
        reason: format!("Failed to build chat history: {}", e),
    })?;

    Ok(RigRequest {
        preamble,
        chat_history,
        documents: Vec::new(),
        tools,
        temperature: temperature.map(|t| t as f64),
        max_tokens: max_tokens.map(|t| t as u64),
        tool_choice,
        additional_params: None,
        model: None,
        output_schema: None,
    })
}

/// Drain a rig-core streaming response, forwarding text chunks over `chunk_tx`
/// as they arrive and returning the same `(text, tool_calls, finish_reason)`
/// shape as `extract_response`, plus token usage collected at stream end.
///
/// Tool-call deltas are accumulated by rig-core internally (into
/// `stream.choice`) but not forwarded chunk-by-chunk -- only text content
/// streams live to the UI.
async fn drain_stream<R>(
    stream: &mut rig::streaming::StreamingCompletionResponse<R>,
    chunk_tx: &tokio::sync::mpsc::UnboundedSender<String>,
    model_name: &str,
) -> Result<(Option<String>, Vec<IronToolCall>, FinishReason, RigUsage), LlmError>
where
    R: Clone + Unpin + rig::completion::GetTokenUsage,
{
    use futures::StreamExt;
    use rig::streaming::StreamedAssistantContent;

    while let Some(item) = stream.next().await {
        if let StreamedAssistantContent::Text(text) =
            item.map_err(|e| map_rig_error(model_name, e))?
        {
            let _ = chunk_tx.send(text.text);
        }
    }

    let (text, tool_calls, finish) = extract_response(&stream.choice, &RigUsage::new());
    let usage = stream
        .response
        .as_ref()
        .and_then(|r| r.token_usage())
        .unwrap_or_default();

    Ok((text, tool_calls, finish, usage))
}

#[async_trait]
impl<M> LlmProvider for RigAdapter<M>
where
    M: CompletionModel + Send + Sync + 'static,
    M::Response: Send + Sync + Serialize + DeserializeOwned,
{
    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn cost_per_token(&self) -> (Decimal, Decimal) {
        (self.input_cost, self.output_cost)
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let (preamble, history) = convert_messages(&request.messages);

        let rig_req = build_rig_request(
            preamble,
            history,
            Vec::new(),
            None,
            request.temperature,
            request.max_tokens,
        )?;

        let response = self
            .model
            .completion(rig_req)
            .await
            .map_err(|e| map_rig_error(&self.model_name, e))?;

        let (text, _tool_calls, finish) = extract_response(&response.choice, &response.usage);

        Ok(CompletionResponse {
            content: text.unwrap_or_default(),
            input_tokens: saturate_u32(response.usage.input_tokens),
            output_tokens: saturate_u32(response.usage.output_tokens),
            finish_reason: finish,
            response_id: None,
        })
    }

    async fn complete_with_tools(
        &self,
        request: ToolCompletionRequest,
    ) -> Result<ToolCompletionResponse, LlmError> {
        let (preamble, history) = convert_messages(&request.messages);
        let tools = convert_tools(&request.tools);
        let tool_choice = convert_tool_choice(request.tool_choice.as_deref());

        let rig_req = build_rig_request(
            preamble,
            history,
            tools,
            tool_choice,
            request.temperature,
            request.max_tokens,
        )?;

        let response = self
            .model
            .completion(rig_req)
            .await
            .map_err(|e| map_rig_error(&self.model_name, e))?;

        let (text, tool_calls, finish) = extract_response(&response.choice, &response.usage);

        Ok(ToolCompletionResponse {
            content: text,
            tool_calls,
            input_tokens: saturate_u32(response.usage.input_tokens),
            output_tokens: saturate_u32(response.usage.output_tokens),
            finish_reason: finish,
            response_id: None,
        })
    }

    async fn complete_streaming(
        &self,
        request: CompletionRequest,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<CompletionResponse, LlmError> {
        let (preamble, history) = convert_messages(&request.messages);

        let rig_req = build_rig_request(
            preamble,
            history,
            Vec::new(),
            None,
            request.temperature,
            request.max_tokens,
        )?;

        let mut stream = self
            .model
            .stream(rig_req)
            .await
            .map_err(|e| map_rig_error(&self.model_name, e))?;

        let (text, _tool_calls, finish, usage) =
            drain_stream(&mut stream, &chunk_tx, &self.model_name).await?;

        Ok(CompletionResponse {
            content: text.unwrap_or_default(),
            input_tokens: saturate_u32(usage.input_tokens),
            output_tokens: saturate_u32(usage.output_tokens),
            finish_reason: finish,
            response_id: None,
        })
    }

    async fn complete_with_tools_streaming(
        &self,
        request: ToolCompletionRequest,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<ToolCompletionResponse, LlmError> {
        let (preamble, history) = convert_messages(&request.messages);
        let tools = convert_tools(&request.tools);
        let tool_choice = convert_tool_choice(request.tool_choice.as_deref());

        let rig_req = build_rig_request(
            preamble,
            history,
            tools,
            tool_choice,
            request.temperature,
            request.max_tokens,
        )?;

        let mut stream = self
            .model
            .stream(rig_req)
            .await
            .map_err(|e| map_rig_error(&self.model_name, e))?;

        let (text, tool_calls, finish, usage) =
            drain_stream(&mut stream, &chunk_tx, &self.model_name).await?;

        Ok(ToolCompletionResponse {
            content: text,
            tool_calls,
            input_tokens: saturate_u32(usage.input_tokens),
            output_tokens: saturate_u32(usage.output_tokens),
            finish_reason: finish,
            response_id: None,
        })
    }

    fn active_model_name(&self) -> String {
        self.model_name.clone()
    }

    fn set_model(&self, _model: &str) -> Result<(), LlmError> {
        // rig-core models are baked at construction time.
        // Switching requires creating a new adapter.
        Err(LlmError::RequestFailed {
            provider: self.model_name.clone(),
            reason: "Runtime model switching not supported for rig-core providers. \
                     Restart with a different model configured."
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_rig_error_400_is_model_not_available() {
        let err = map_rig_error("model", "HttpError: Invalid status code 400 Bad Request");
        assert!(matches!(err, LlmError::ModelNotAvailable { .. }));
    }

    #[test]
    fn test_map_rig_error_429_is_rate_limited() {
        let err = map_rig_error(
            "model",
            "HttpError: Invalid status code 429 Too Many Requests with message: ...",
        );
        assert!(matches!(err, LlmError::RateLimited { .. }));
    }

    #[test]
    fn test_map_rig_error_429_no_retry_after_is_none() {
        let err = map_rig_error("model", "status code 429 Too Many Requests");
        match err {
            LlmError::RateLimited { retry_after, .. } => assert!(retry_after.is_none()),
            other => panic!("expected RateLimited, got: {other:?}"),
        }
    }

    #[test]
    fn test_map_rig_error_429_with_retry_after_header() {
        let msg = "HttpError: Invalid status code 429 Too Many Requests\nRetry-After: 60";
        match map_rig_error("model", msg) {
            LlmError::RateLimited { retry_after, .. } => {
                assert_eq!(retry_after, Some(std::time::Duration::from_secs(60)));
            }
            other => panic!("expected RateLimited, got: {other:?}"),
        }
    }

    #[test]
    fn test_map_rig_error_500_is_request_failed() {
        let err = map_rig_error(
            "model",
            "HttpError: Invalid status code 500 Internal Server Error",
        );
        assert!(matches!(err, LlmError::RequestFailed { .. }));
    }

    #[test]
    fn test_parse_retry_after_present() {
        let msg = "429 Too Many Requests\nRetry-After: 30\nbody here";
        assert_eq!(
            parse_retry_after(msg),
            Some(std::time::Duration::from_secs(30))
        );
    }

    #[test]
    fn test_parse_retry_after_case_insensitive() {
        assert_eq!(
            parse_retry_after("retry-after: 5"),
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(
            parse_retry_after("RETRY-AFTER: 10"),
            Some(std::time::Duration::from_secs(10))
        );
    }

    #[test]
    fn test_parse_retry_after_absent() {
        assert!(parse_retry_after("429 Too Many Requests, no header").is_none());
    }

    #[test]
    fn test_parse_retry_after_non_numeric() {
        assert!(parse_retry_after("retry-after: tomorrow").is_none());
    }

    #[test]
    fn test_convert_messages_system_to_preamble() {
        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("Hello"),
        ];
        let (preamble, history) = convert_messages(&messages);
        assert_eq!(preamble, Some("You are a helpful assistant.".to_string()));
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_convert_messages_multiple_systems_concatenated() {
        let messages = vec![
            ChatMessage::system("System 1"),
            ChatMessage::system("System 2"),
            ChatMessage::user("Hi"),
        ];
        let (preamble, history) = convert_messages(&messages);
        assert_eq!(preamble, Some("System 1\nSystem 2".to_string()));
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let messages = vec![ChatMessage::tool_result(
            "call_123",
            "search",
            "result text",
        )];
        let (preamble, history) = convert_messages(&messages);
        assert!(preamble.is_none());
        assert_eq!(history.len(), 1);
        // Tool results become User messages in rig-core
        match &history[0] {
            RigMessage::User { .. } => {}
            other => panic!("Expected User message, got: {:?}", other),
        }
    }

    #[test]
    fn test_convert_messages_assistant_with_tool_calls() {
        let tc = IronToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"query": "test"}),
        };
        let msg = ChatMessage::assistant_with_tool_calls(Some("thinking".to_string()), vec![tc]);
        let messages = vec![msg];
        let (_preamble, history) = convert_messages(&messages);
        assert_eq!(history.len(), 1);
        match &history[0] {
            RigMessage::Assistant { content, .. } => {
                // Should have both text and tool call
                assert!(content.iter().count() >= 2);
            }
            other => panic!("Expected Assistant message, got: {:?}", other),
        }
    }

    #[test]
    fn test_convert_tools() {
        let tools = vec![IronToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                }
            }),
        }];
        let rig_tools = convert_tools(&tools);
        assert_eq!(rig_tools.len(), 1);
        assert_eq!(rig_tools[0].name, "search");
        assert_eq!(rig_tools[0].description, "Search the web");
    }

    #[test]
    fn test_convert_tool_choice() {
        assert!(matches!(
            convert_tool_choice(Some("auto")),
            Some(RigToolChoice::Auto)
        ));
        assert!(matches!(
            convert_tool_choice(Some("required")),
            Some(RigToolChoice::Required)
        ));
        assert!(matches!(
            convert_tool_choice(Some("none")),
            Some(RigToolChoice::None)
        ));
        assert!(matches!(
            convert_tool_choice(Some("AUTO")),
            Some(RigToolChoice::Auto)
        ));
        assert!(convert_tool_choice(None).is_none());
        assert!(convert_tool_choice(Some("unknown")).is_none());
    }

    #[test]
    fn test_extract_response_text_only() {
        let content = OneOrMany::one(AssistantContent::text("Hello world"));
        let usage = RigUsage::new();
        let (text, calls, finish) = extract_response(&content, &usage);
        assert_eq!(text, Some("Hello world".to_string()));
        assert!(calls.is_empty());
        assert_eq!(finish, FinishReason::Stop);
    }

    #[test]
    fn test_extract_response_tool_call() {
        let tc = AssistantContent::tool_call("call_1", "search", serde_json::json!({"q": "test"}));
        let content = OneOrMany::one(tc);
        let usage = RigUsage::new();
        let (text, calls, finish) = extract_response(&content, &usage);
        assert!(text.is_none());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(finish, FinishReason::ToolUse);
    }

    #[test]
    fn test_saturate_u32() {
        assert_eq!(saturate_u32(100), 100);
        assert_eq!(saturate_u32(u64::MAX), u32::MAX);
        assert_eq!(saturate_u32(u32::MAX as u64), u32::MAX);
    }

    #[test]
    fn test_convert_messages_with_image_url_attachment() {
        let msg = ChatMessage::user_with_attachments(
            "What is in this image?",
            vec![Attachment::ImageUrl {
                url: "https://example.com/photo.jpg".into(),
            }],
        );
        let (preamble, history) = convert_messages(&[msg]);
        assert!(preamble.is_none());
        assert_eq!(history.len(), 1);
        // Should be a multi-part User message (text + image)
        match &history[0] {
            RigMessage::User { content } => {
                assert!(content.iter().count() >= 2);
            }
            other => panic!("Expected User message, got: {:?}", other),
        }
    }

    #[test]
    fn test_convert_messages_with_base64_attachment() {
        let msg = ChatMessage::user_with_attachments(
            "Describe this",
            vec![Attachment::ImageBase64 {
                media_type: "image/png".into(),
                data: "abc123".into(),
            }],
        );
        let (preamble, history) = convert_messages(&[msg]);
        assert!(preamble.is_none());
        assert_eq!(history.len(), 1);
        match &history[0] {
            RigMessage::User { content } => {
                assert!(content.iter().count() >= 2);
            }
            other => panic!("Expected User message, got: {:?}", other),
        }
    }

    #[test]
    fn test_convert_messages_no_attachments_stays_simple() {
        let msg = ChatMessage::user("Hello");
        let (_, history) = convert_messages(&[msg]);
        assert_eq!(history.len(), 1);
        // Plain message with no attachments should be a simple User message
        match &history[0] {
            RigMessage::User { content } => {
                assert_eq!(content.iter().count(), 1);
            }
            other => panic!("Expected User message, got: {:?}", other),
        }
    }
}
