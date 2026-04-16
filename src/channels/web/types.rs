//! Request and response DTOs for the web gateway API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// --- Chat ---

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub thread_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub message_id: Uuid,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ThreadInfo {
    pub id: Uuid,
    pub state: String,
    pub turn_count: usize,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ThreadListResponse {
    /// The pinned assistant thread (always present after first load).
    pub assistant_thread: Option<ThreadInfo>,
    /// Regular conversation threads.
    pub threads: Vec<ThreadInfo>,
    pub active_thread: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct TurnInfo {
    pub turn_number: usize,
    pub user_input: String,
    pub response: Option<String>,
    pub state: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub tool_calls: Vec<ToolCallInfo>,
}

#[derive(Debug, Serialize)]
pub struct ToolCallInfo {
    pub name: String,
    pub has_result: bool,
    pub has_error: bool,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub thread_id: Uuid,
    pub turns: Vec<TurnInfo>,
    /// Whether there are older messages available.
    #[serde(default)]
    pub has_more: bool,
    /// Cursor for the next page (ISO8601 timestamp of the oldest message returned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_timestamp: Option<String>,
}

// --- Approval ---

#[derive(Debug, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    /// "approve", "always", or "deny"
    pub action: String,
    /// Thread that owns the pending approval (so the agent loop finds the right session).
    pub thread_id: Option<String>,
}

// --- SSE Event Types ---

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SseEvent {
    #[serde(rename = "response")]
    Response { content: String, thread_id: String },
    #[serde(rename = "thinking")]
    Thinking {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "tool_started")]
    ToolStarted {
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "tokens_used")]
    TokensUsed {
        input_tokens: u32,
        output_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "tool_completed")]
    ToolCompleted {
        name: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        name: String,
        preview: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "stream_chunk")]
    StreamChunk {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "status")]
    Status {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "job_started")]
    JobStarted {
        job_id: String,
        title: String,
        browse_url: String,
    },
    #[serde(rename = "approval_needed")]
    ApprovalNeeded {
        request_id: String,
        tool_name: String,
        description: String,
        parameters: String,
    },
    #[serde(rename = "auth_required")]
    AuthRequired {
        extension_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        auth_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        setup_url: Option<String>,
    },
    #[serde(rename = "auth_completed")]
    AuthCompleted {
        extension_name: String,
        success: bool,
        message: String,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    #[serde(rename = "heartbeat")]
    Heartbeat,

    // Sandbox job streaming events (worker + Claude Code bridge)
    #[serde(rename = "job_message")]
    JobMessage {
        job_id: String,
        role: String,
        content: String,
    },
    #[serde(rename = "job_tool_use")]
    JobToolUse {
        job_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "job_tool_result")]
    JobToolResult {
        job_id: String,
        tool_name: String,
        output: String,
    },
    #[serde(rename = "job_status")]
    JobStatus { job_id: String, message: String },
    #[serde(rename = "job_result")]
    JobResult {
        job_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
}

// --- Memory ---

#[derive(Debug, Serialize)]
pub struct MemoryTreeResponse {
    pub entries: Vec<TreeEntry>,
}

#[derive(Debug, Serialize)]
pub struct TreeEntry {
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize)]
pub struct MemoryListResponse {
    pub path: String,
    pub entries: Vec<ListEntry>,
}

#[derive(Debug, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MemoryReadResponse {
    pub path: String,
    pub content: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryWriteRequest {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct MemoryWriteResponse {
    pub path: String,
    pub status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct MemorySearchResponse {
    pub results: Vec<SearchHit>,
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub content: String,
    pub score: f64,
}

// --- Jobs ---

#[derive(Debug, Serialize)]
pub struct JobInfo {
    pub id: Uuid,
    pub title: String,
    pub state: String,
    pub user_id: String,
    pub created_at: String,
    pub started_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JobListResponse {
    pub jobs: Vec<JobInfo>,
}

#[derive(Debug, Serialize)]
pub struct JobSummaryResponse {
    pub total: usize,
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub failed: usize,
    pub stuck: usize,
}

#[derive(Debug, Serialize)]
pub struct JobDetailResponse {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub state: String,
    pub user_id: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub elapsed_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browse_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_mode: Option<String>,
    pub transitions: Vec<TransitionInfo>,
}

// --- Project Files ---

#[derive(Debug, Serialize)]
pub struct ProjectFileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectFilesResponse {
    pub entries: Vec<ProjectFileEntry>,
}

#[derive(Debug, Serialize)]
pub struct ProjectFileReadResponse {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct TransitionInfo {
    pub from: String,
    pub to: String,
    pub timestamp: String,
    pub reason: Option<String>,
}

// --- Extensions ---

#[derive(Debug, Serialize)]
pub struct ExtensionInfo {
    pub name: String,
    pub kind: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub authenticated: bool,
    pub active: bool,
    /// Always `true` — every entry returned by the installed-list endpoint is installed.
    /// Included so the setup wizard (shared with the catalog flow) can tell the
    /// difference and skip the install step.
    pub installed: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tools: Vec<String>,
}

/// One entry in the extension catalog (registry entry + installed status).
#[derive(Debug, Serialize)]
pub struct CatalogEntry {
    pub name: String,
    pub display_name: String,
    pub kind: String,
    pub description: String,
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// "dcr" | "oauth" | "manual" | "none"
    pub auth_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_url: Option<String>,
    pub installed: bool,
    pub authenticated: bool,
    pub active: bool,
    pub status: String,
    /// Whether this extension can be installed via one-click (has a downloadable binary or URL).
    /// False for WasmBuildable entries that require building from source.
    pub installable: bool,
    /// For buildable extensions: the source subdirectory (e.g. "channels-src/discord").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_dir: Option<String>,
    /// Name of the setup guide doc served at /api/docs/{docs_file} (e.g. "DISCORD_SETUP").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_file: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CatalogResponse {
    pub entries: Vec<CatalogEntry>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct CatalogFilterParams {
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CatalogSearchRequest {
    pub query: Option<String>,
    pub kind: Option<String>,
    pub discover: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ExtensionAuthInfoResponse {
    pub info: crate::extensions::ExtensionAuthInfo,
}

// --- Extension config ---

/// Response for `GET /api/extensions/{name}/config`.
/// Returns the JSON Schema for non-secret config fields plus the current saved values.
#[derive(Debug, Serialize)]
pub struct ExtensionConfigResponse {
    pub name: String,
    /// JSON Schema (`type: object` with `properties`) describing configurable fields.
    /// `None` if the extension has no `config_schema`.
    pub schema: Option<serde_json::Value>,
    /// Current saved values keyed by field name.
    /// Missing keys mean the field has no saved value; callers should fall back to the
    /// schema `default`.
    pub values: std::collections::HashMap<String, serde_json::Value>,
}

/// Request body for `PUT /api/extensions/{name}/config`.
#[derive(Debug, Deserialize)]
pub struct ExtensionConfigWriteRequest {
    /// Field name → new value. Only present keys are written.
    /// Sending `null` for a key deletes that setting.
    pub values: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct KindFilterParams {
    pub kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExtensionListResponse {
    pub extensions: Vec<ExtensionInfo>,
}

#[derive(Debug, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct ToolListResponse {
    pub tools: Vec<ToolInfo>,
}

#[derive(Debug, Deserialize)]
pub struct InstallExtensionRequest {
    pub name: String,
    pub url: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActionResponse {
    pub success: bool,
    pub message: String,
    /// Auth URL to open (when activation requires OAuth).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// Whether the extension is waiting for a manual token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub awaiting_token: Option<bool>,
    /// Instructions for manual token entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

impl ActionResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            auth_url: None,
            awaiting_token: None,
            instructions: None,
        }
    }

    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            auth_url: None,
            awaiting_token: None,
            instructions: None,
        }
    }
}

// --- Auth Token ---

/// Request to submit an auth token for an extension (dedicated endpoint).
#[derive(Debug, Deserialize)]
pub struct AuthTokenRequest {
    pub extension_name: String,
    pub token: String,
}

/// Request to cancel an in-progress auth flow.
#[derive(Debug, Deserialize)]
pub struct AuthCancelRequest {
    pub extension_name: String,
}

// --- WebSocket ---

/// Message sent by a WebSocket client to the server.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum WsClientMessage {
    /// Send a chat message to the agent.
    #[serde(rename = "message")]
    Message {
        content: String,
        thread_id: Option<String>,
    },
    /// Approve or deny a pending tool execution.
    #[serde(rename = "approval")]
    Approval {
        request_id: String,
        /// "approve", "always", or "deny"
        action: String,
        /// Thread that owns the pending approval.
        thread_id: Option<String>,
    },
    /// Submit an auth token for an extension (bypasses message pipeline).
    #[serde(rename = "auth_token")]
    AuthToken {
        extension_name: String,
        token: String,
    },
    /// Cancel an in-progress auth flow.
    #[serde(rename = "auth_cancel")]
    AuthCancel { extension_name: String },
    /// Client heartbeat ping.
    #[serde(rename = "ping")]
    Ping,
}

/// Message sent by the server to a WebSocket client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WsServerMessage {
    /// An SSE-style event forwarded over WebSocket.
    #[serde(rename = "event")]
    Event {
        /// The event sub-type (response, thinking, tool_started, etc.)
        event_type: String,
        /// The event payload as a JSON value.
        data: serde_json::Value,
    },
    /// Server heartbeat pong.
    #[serde(rename = "pong")]
    Pong,
    /// Error message.
    #[serde(rename = "error")]
    Error { message: String },
}

impl WsServerMessage {
    /// Create a WsServerMessage from an SseEvent.
    pub fn from_sse_event(event: &SseEvent) -> Self {
        let event_type = match event {
            SseEvent::Response { .. } => "response",
            SseEvent::Thinking { .. } => "thinking",
            SseEvent::ToolStarted { .. } => "tool_started",
            SseEvent::TokensUsed { .. } => "tokens_used",
            SseEvent::ToolCompleted { .. } => "tool_completed",
            SseEvent::ToolResult { .. } => "tool_result",
            SseEvent::StreamChunk { .. } => "stream_chunk",
            SseEvent::Status { .. } => "status",
            SseEvent::JobStarted { .. } => "job_started",
            SseEvent::ApprovalNeeded { .. } => "approval_needed",
            SseEvent::AuthRequired { .. } => "auth_required",
            SseEvent::AuthCompleted { .. } => "auth_completed",
            SseEvent::Error { .. } => "error",
            SseEvent::Heartbeat => "heartbeat",
            SseEvent::JobMessage { .. } => "job_message",
            SseEvent::JobToolUse { .. } => "job_tool_use",
            SseEvent::JobToolResult { .. } => "job_tool_result",
            SseEvent::JobStatus { .. } => "job_status",
            SseEvent::JobResult { .. } => "job_result",
        };
        let data = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
        WsServerMessage::Event {
            event_type: event_type.to_string(),
            data,
        }
    }
}

// --- Routines ---

#[derive(Debug, Serialize)]
pub struct RoutineInfo {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub trigger_type: String,
    pub trigger_summary: String,
    pub action_type: String,
    pub last_run_at: Option<String>,
    pub next_fire_at: Option<String>,
    pub run_count: u64,
    pub consecutive_failures: u32,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct RoutineListResponse {
    pub routines: Vec<RoutineInfo>,
}

#[derive(Debug, Serialize)]
pub struct RoutineSummaryResponse {
    pub total: u64,
    pub enabled: u64,
    pub disabled: u64,
    pub failing: u64,
    pub runs_today: u64,
}

#[derive(Debug, Serialize)]
pub struct RoutineDetailResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub trigger: serde_json::Value,
    pub action: serde_json::Value,
    pub guardrails: serde_json::Value,
    pub notify: serde_json::Value,
    pub last_run_at: Option<String>,
    pub next_fire_at: Option<String>,
    pub run_count: u64,
    pub consecutive_failures: u32,
    pub created_at: String,
    pub recent_runs: Vec<RoutineRunInfo>,
}

#[derive(Debug, Serialize)]
pub struct RoutineRunInfo {
    pub id: Uuid,
    pub trigger_type: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub result_summary: Option<String>,
    pub tokens_used: Option<i32>,
}

// --- Settings ---

#[derive(Debug, Serialize)]
pub struct SettingResponse {
    pub key: String,
    pub value: serde_json::Value,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct SettingsListResponse {
    pub settings: Vec<SettingResponse>,
}

#[derive(Debug, Deserialize)]
pub struct SettingWriteRequest {
    pub value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct SettingsImportRequest {
    pub settings: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct SettingsExportResponse {
    pub settings: std::collections::HashMap<String, serde_json::Value>,
}

// --- Health ---

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub channel: &'static str,
}

// --- Version ---

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    /// Current running version (from Cargo.toml at compile time).
    pub version: &'static str,
    /// Latest published version tag from GitHub releases, if the update check
    /// was requested and succeeded.  `None` if not checked or the check failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// Whether a newer version is available (only set when `latest` is present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
    /// Direct URL to the GitHub release page for `latest`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    /// Docker pull command for the latest image, if an update is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_pull: Option<String>,
    /// Human-readable error if the update check failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- WsClientMessage deserialization tests ----

    #[test]
    fn test_ws_client_message_parse() {
        let json = r#"{"type":"message","content":"hello","thread_id":"t1"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Message { content, thread_id } => {
                assert_eq!(content, "hello");
                assert_eq!(thread_id.as_deref(), Some("t1"));
            }
            _ => panic!("Expected Message variant"),
        }
    }

    #[test]
    fn test_ws_client_message_no_thread() {
        let json = r#"{"type":"message","content":"hi"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Message { content, thread_id } => {
                assert_eq!(content, "hi");
                assert!(thread_id.is_none());
            }
            _ => panic!("Expected Message variant"),
        }
    }

    #[test]
    fn test_ws_client_approval_parse() {
        let json =
            r#"{"type":"approval","request_id":"abc-123","action":"approve","thread_id":"t1"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Approval {
                request_id,
                action,
                thread_id,
            } => {
                assert_eq!(request_id, "abc-123");
                assert_eq!(action, "approve");
                assert_eq!(thread_id.as_deref(), Some("t1"));
            }
            _ => panic!("Expected Approval variant"),
        }
    }

    #[test]
    fn test_ws_client_approval_parse_no_thread() {
        let json = r#"{"type":"approval","request_id":"abc-123","action":"deny"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Approval {
                request_id,
                action,
                thread_id,
            } => {
                assert_eq!(request_id, "abc-123");
                assert_eq!(action, "deny");
                assert!(thread_id.is_none());
            }
            _ => panic!("Expected Approval variant"),
        }
    }

    #[test]
    fn test_ws_client_ping_parse() {
        let json = r#"{"type":"ping"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMessage::Ping));
    }

    #[test]
    fn test_ws_client_unknown_type_fails() {
        let json = r#"{"type":"unknown"}"#;
        let result: Result<WsClientMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // ---- WsServerMessage serialization tests ----

    #[test]
    fn test_ws_server_pong_serialize() {
        let msg = WsServerMessage::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);
    }

    #[test]
    fn test_ws_server_error_serialize() {
        let msg = WsServerMessage::Error {
            message: "bad request".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["message"], "bad request");
    }

    #[test]
    fn test_ws_server_from_sse_response() {
        let sse = SseEvent::Response {
            content: "hello".to_string(),
            thread_id: "t1".to_string(),
        };
        let ws = WsServerMessage::from_sse_event(&sse);
        match ws {
            WsServerMessage::Event { event_type, data } => {
                assert_eq!(event_type, "response");
                assert_eq!(data["content"], "hello");
                assert_eq!(data["thread_id"], "t1");
            }
            _ => panic!("Expected Event variant"),
        }
    }

    #[test]
    fn test_ws_server_from_sse_thinking() {
        let sse = SseEvent::Thinking {
            message: "reasoning...".to_string(),
            thread_id: None,
        };
        let ws = WsServerMessage::from_sse_event(&sse);
        match ws {
            WsServerMessage::Event { event_type, data } => {
                assert_eq!(event_type, "thinking");
                assert_eq!(data["message"], "reasoning...");
            }
            _ => panic!("Expected Event variant"),
        }
    }

    #[test]
    fn test_ws_server_from_sse_approval_needed() {
        let sse = SseEvent::ApprovalNeeded {
            request_id: "r1".to_string(),
            tool_name: "shell".to_string(),
            description: "Run ls".to_string(),
            parameters: "{}".to_string(),
        };
        let ws = WsServerMessage::from_sse_event(&sse);
        match ws {
            WsServerMessage::Event { event_type, data } => {
                assert_eq!(event_type, "approval_needed");
                assert_eq!(data["tool_name"], "shell");
            }
            _ => panic!("Expected Event variant"),
        }
    }

    #[test]
    fn test_ws_server_from_sse_heartbeat() {
        let sse = SseEvent::Heartbeat;
        let ws = WsServerMessage::from_sse_event(&sse);
        match ws {
            WsServerMessage::Event { event_type, .. } => {
                assert_eq!(event_type, "heartbeat");
            }
            _ => panic!("Expected Event variant"),
        }
    }

    // ---- Auth type tests ----

    #[test]
    fn test_ws_client_auth_token_parse() {
        let json = r#"{"type":"auth_token","extension_name":"notion","token":"sk-123"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::AuthToken {
                extension_name,
                token,
            } => {
                assert_eq!(extension_name, "notion");
                assert_eq!(token, "sk-123");
            }
            _ => panic!("Expected AuthToken variant"),
        }
    }

    #[test]
    fn test_ws_client_auth_cancel_parse() {
        let json = r#"{"type":"auth_cancel","extension_name":"notion"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::AuthCancel { extension_name } => {
                assert_eq!(extension_name, "notion");
            }
            _ => panic!("Expected AuthCancel variant"),
        }
    }

    #[test]
    fn test_sse_auth_required_serialize() {
        let event = SseEvent::AuthRequired {
            extension_name: "notion".to_string(),
            instructions: Some("Get your token from...".to_string()),
            auth_url: None,
            setup_url: Some("https://notion.so/integrations".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "auth_required");
        assert_eq!(parsed["extension_name"], "notion");
        assert_eq!(parsed["instructions"], "Get your token from...");
        assert!(parsed.get("auth_url").is_none());
        assert_eq!(parsed["setup_url"], "https://notion.so/integrations");
    }

    #[test]
    fn test_sse_auth_completed_serialize() {
        let event = SseEvent::AuthCompleted {
            extension_name: "notion".to_string(),
            success: true,
            message: "notion authenticated (3 tools loaded)".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "auth_completed");
        assert_eq!(parsed["extension_name"], "notion");
        assert_eq!(parsed["success"], true);
    }

    #[test]
    fn test_ws_server_from_sse_auth_required() {
        let sse = SseEvent::AuthRequired {
            extension_name: "openai".to_string(),
            instructions: Some("Enter API key".to_string()),
            auth_url: None,
            setup_url: None,
        };
        let ws = WsServerMessage::from_sse_event(&sse);
        match ws {
            WsServerMessage::Event { event_type, data } => {
                assert_eq!(event_type, "auth_required");
                assert_eq!(data["extension_name"], "openai");
            }
            _ => panic!("Expected Event variant"),
        }
    }

    #[test]
    fn test_ws_server_from_sse_auth_completed() {
        let sse = SseEvent::AuthCompleted {
            extension_name: "slack".to_string(),
            success: false,
            message: "Invalid token".to_string(),
        };
        let ws = WsServerMessage::from_sse_event(&sse);
        match ws {
            WsServerMessage::Event { event_type, data } => {
                assert_eq!(event_type, "auth_completed");
                assert_eq!(data["success"], false);
            }
            _ => panic!("Expected Event variant"),
        }
    }

    #[test]
    fn test_auth_token_request_deserialize() {
        let json = r#"{"extension_name":"telegram","token":"bot12345"}"#;
        let req: AuthTokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.extension_name, "telegram");
        assert_eq!(req.token, "bot12345");
    }

    #[test]
    fn test_auth_cancel_request_deserialize() {
        let json = r#"{"extension_name":"telegram"}"#;
        let req: AuthCancelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.extension_name, "telegram");
    }

    // ---- ExtensionConfigResponse / ExtensionConfigWriteRequest ----

    #[test]
    fn test_extension_config_response_serialize_with_schema() {
        let mut values = std::collections::HashMap::new();
        values.insert("owner_id".to_string(), serde_json::json!("12345"));
        values.insert("dm_policy".to_string(), serde_json::json!("owner_only"));
        let resp = ExtensionConfigResponse {
            name: "discord".to_string(),
            schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "owner_id": { "type": "string", "nullable": true },
                    "dm_policy": { "type": "string", "enum": ["pairing", "owner_only", "anyone"] }
                }
            })),
            values,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["name"], "discord");
        assert!(json["schema"]["properties"].get("owner_id").is_some());
        assert_eq!(json["values"]["owner_id"], "12345");
        assert_eq!(json["values"]["dm_policy"], "owner_only");
    }

    #[test]
    fn test_extension_config_response_serialize_no_schema() {
        let resp = ExtensionConfigResponse {
            name: "unknown_ext".to_string(),
            schema: None,
            values: std::collections::HashMap::new(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["name"], "unknown_ext");
        assert!(json["schema"].is_null());
        assert!(json["values"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_extension_config_write_request_deserialize() {
        let json = r#"{"values":{"owner_id":"99","dm_policy":"owner_only"}}"#;
        let req: ExtensionConfigWriteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.values["owner_id"], "99");
        assert_eq!(req.values["dm_policy"], "owner_only");
    }

    #[test]
    fn test_extension_config_write_request_null_means_delete() {
        let json = r#"{"values":{"owner_id":null}}"#;
        let req: ExtensionConfigWriteRequest = serde_json::from_str(json).unwrap();
        assert!(req.values["owner_id"].is_null());
    }

    #[test]
    fn test_extension_config_write_request_empty_values() {
        let json = r#"{"values":{}}"#;
        let req: ExtensionConfigWriteRequest = serde_json::from_str(json).unwrap();
        assert!(req.values.is_empty());
    }

    // --- ConversationTokenStatsResponse ---

    #[test]
    fn test_conversation_token_stats_response_serializes() {
        use rust_decimal::Decimal;
        let resp = ConversationTokenStatsResponse {
            thread_id: uuid::Uuid::nil(),
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_tokens: 1500,
            total_cost: Decimal::new(125, 4), // 0.0125
            call_count: 3,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["total_input_tokens"], 1000);
        assert_eq!(v["total_output_tokens"], 500);
        assert_eq!(v["total_tokens"], 1500);
        assert_eq!(v["call_count"], 3);
        // thread_id should be a UUID string
        assert_eq!(
            v["thread_id"].as_str().unwrap(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn test_conversation_token_stats_total_tokens_is_sum() {
        use rust_decimal::Decimal;
        let input = 800_i64;
        let output = 200_i64;
        let resp = ConversationTokenStatsResponse {
            thread_id: uuid::Uuid::new_v4(),
            total_input_tokens: input,
            total_output_tokens: output,
            total_tokens: input + output,
            total_cost: Decimal::ZERO,
            call_count: 1,
        };
        assert_eq!(
            resp.total_tokens,
            resp.total_input_tokens + resp.total_output_tokens
        );
    }

    // ── Analytics types ───────────────────────────────────────────────────────

    #[test]
    fn test_model_stats_serializes_with_latency() {
        let stats = ModelStats {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            total_calls: 42,
            total_input_tokens: 10_000,
            total_output_tokens: 5_000,
            total_cost_usd: "0.001234".to_string(),
            avg_cost_per_call_usd: "0.000029".to_string(),
            avg_latency_ms: Some(823.5),
            p95_latency_ms: Some(1200.0),
        };
        let v = serde_json::to_value(&stats).unwrap();
        assert_eq!(v["provider"], "anthropic");
        assert_eq!(v["total_calls"], 42);
        assert_eq!(v["avg_latency_ms"], 823.5);
    }

    #[test]
    fn test_model_stats_omits_latency_when_none() {
        // avg_latency_ms is skip_serializing_if = Option::is_none — must be absent,
        // not null, when there's no latency data (pre-V9 calls).
        let stats = ModelStats {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            total_calls: 1,
            total_input_tokens: 100,
            total_output_tokens: 50,
            total_cost_usd: "0.000010".to_string(),
            avg_cost_per_call_usd: "0.000010".to_string(),
            avg_latency_ms: None,
            p95_latency_ms: None,
        };
        let v = serde_json::to_value(&stats).unwrap();
        assert!(
            !v.as_object().unwrap().contains_key("avg_latency_ms"),
            "avg_latency_ms should be absent when None, not serialized as null"
        );
    }

    #[test]
    fn test_model_analytics_response_totals() {
        let resp = ModelAnalyticsResponse {
            models: vec![],
            total_input_tokens: 1_000_000,
            total_output_tokens: 500_000,
            total_cost_usd: "1.5000".to_string(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["total_input_tokens"], 1_000_000);
        assert_eq!(v["total_output_tokens"], 500_000);
        assert_eq!(v["total_cost_usd"], "1.5000");
        assert!(v["models"].as_array().unwrap().is_empty());
    }
}

// --- Channels ---

#[derive(Debug, Serialize)]
pub struct ChannelInfo {
    pub name: String,
    pub description: Option<String>,
    pub running: bool,
    /// Whether the channel is enabled (will load on next restart). Defaults to true.
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ChannelListResponse {
    pub channels: Vec<ChannelInfo>,
}

#[derive(Debug, Serialize)]
pub struct ChannelToggleResponse {
    pub name: String,
    pub enabled: bool,
    pub message: String,
}

// --- Conversation Token Stats ---

#[derive(Debug, Serialize)]
pub struct ConversationTokenStatsResponse {
    pub thread_id: Uuid,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cost: rust_decimal::Decimal,
    pub call_count: i64,
}

// --- Provider Health & Costs ---

/// Response for the provider health summary endpoint.
#[derive(Debug, Serialize)]
pub struct ProviderHealthResponse {
    pub providers: Vec<crate::llm::routing::ProviderHealthReport>,
}

/// Response for the LLM cost stats endpoint.
#[derive(Debug, Serialize)]
pub struct LlmCostStatsResponse {
    pub stats: Vec<crate::llm::tracked::LlmCallStats>,
}

// --- Analytics ---

/// Response for GET /api/analytics/models — per-model usage breakdown.
#[derive(Debug, Serialize)]
pub struct ModelAnalyticsResponse {
    pub models: Vec<ModelStats>,
    /// Grand total input tokens across all models.
    pub total_input_tokens: i64,
    /// Grand total output tokens across all models.
    pub total_output_tokens: i64,
    /// Grand total cost across all models (USD string).
    pub total_cost_usd: String,
}

/// Per-model stats row returned by the analytics endpoint.
#[derive(Debug, Serialize)]
pub struct ModelStats {
    pub provider: String,
    pub model: String,
    pub total_calls: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    /// Total cost formatted as a USD decimal string (e.g. "0.002341").
    pub total_cost_usd: String,
    /// Average cost per call formatted as a USD decimal string.
    pub avg_cost_per_call_usd: String,
    /// Average end-to-end latency in milliseconds, absent for calls recorded before V9.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
    /// 95th-percentile latency in milliseconds (PostgreSQL only; null on libSQL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95_latency_ms: Option<f64>,
}

/// Response for GET /api/analytics/jobs — job health summary.
#[derive(Debug, Serialize)]
pub struct JobAnalyticsResponse {
    pub total_jobs: i64,
    pub completed_jobs: i64,
    pub failed_jobs: i64,
    pub in_progress_jobs: i64,
    pub success_rate: f64,
    pub avg_duration_secs: f64,
    pub total_cost_usd: String,
}

/// Response for GET /api/analytics/tools — per-tool usage breakdown.
#[derive(Debug, Serialize)]
pub struct ToolAnalyticsResponse {
    pub tools: Vec<ToolStats>,
}

/// Per-tool stats row.
#[derive(Debug, Serialize)]
pub struct ToolStats {
    pub tool_name: String,
    pub total_calls: i64,
    pub successful_calls: i64,
    pub failed_calls: i64,
    pub success_rate: f64,
    pub avg_duration_ms: f64,
    pub total_cost_usd: String,
}

/// Response for GET /api/analytics/cost-over-time.
#[derive(Debug, Serialize)]
pub struct CostOverTimeResponse {
    pub data: Vec<CostPoint>,
}

/// One day's worth of cost data.
#[derive(Debug, Serialize)]
pub struct CostPoint {
    pub day: String,
    pub cost_usd: String,
    pub call_count: i64,
}

// --- Skills ---

#[derive(Debug, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillListResponse {
    pub skills: Vec<SkillInfo>,
}

#[derive(Debug, Deserialize)]
pub struct SaveSkillRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
}

// --- Audit Log ---

/// Query parameters for GET /api/audit/log.
#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub since: Option<String>,
    pub until: Option<String>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub event_type: Option<String>,
    pub outcome: Option<String>,
    pub limit: Option<i64>,
}

/// A single row in the audit log API response.
#[derive(Debug, Serialize)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub created_at: String,
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Response for GET /api/audit/log.
#[derive(Debug, Serialize)]
pub struct AuditLogResponse {
    pub entries: Vec<AuditLogEntry>,
    pub count: usize,
}

/// One row of event-type counts for the summary card.
#[derive(Debug, Serialize)]
pub struct AuditEventCountEntry {
    pub event_type: String,
    pub count: i64,
}

/// Response for GET /api/audit/summary.
#[derive(Debug, Serialize)]
pub struct AuditSummaryResponse {
    pub counts: Vec<AuditEventCountEntry>,
    pub total: i64,
}
