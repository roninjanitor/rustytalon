//! Slack Events API channel for RustyTalon.
//!
//! This WASM component implements the channel interface for handling Slack
//! webhooks and sending messages back to Slack.
//!
//! # Features
//!
//! - URL verification for Slack Events API
//! - Message event parsing (@mentions, DMs)
//! - Thread support for conversations
//! - Response posting via Slack Web API
//!
//! # Security
//!
//! - Signature validation is handled by the host (webhook secrets)
//! - Bot token is injected by host during HTTP requests
//! - WASM never sees raw credentials

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "sandboxed-channel",
    path: "../../wit/channel.wit",
});

use serde::{Deserialize, Serialize};

// Re-export generated types
use exports::near::agent::channel::{
    AgentResponse, ChannelConfig, Guest, HttpEndpointConfig, IncomingHttpRequest,
    OutgoingHttpResponse, StatusType, StatusUpdate,
};
use near::agent::channel_host::{self, EmittedMessage};

/// Slack event wrapper.
#[derive(Debug, Deserialize)]
struct SlackEventWrapper {
    /// Event type (url_verification, event_callback, etc.)
    #[serde(rename = "type")]
    event_type: String,

    /// Challenge token for URL verification.
    challenge: Option<String>,

    /// The actual event payload (for event_callback).
    event: Option<SlackEvent>,

    /// Team ID that sent this event.
    team_id: Option<String>,

    /// Event ID for deduplication.
    event_id: Option<String>,
}

/// Slack event payload.
#[derive(Debug, Deserialize)]
struct SlackEvent {
    /// Event type (message, app_mention, etc.)
    #[serde(rename = "type")]
    event_type: String,

    /// User who triggered the event.
    user: Option<String>,

    /// Channel where the event occurred.
    channel: Option<String>,

    /// Message text.
    text: Option<String>,

    /// Thread timestamp (for threaded messages).
    thread_ts: Option<String>,

    /// Message timestamp.
    ts: Option<String>,

    /// Bot ID (if message is from a bot).
    bot_id: Option<String>,

    /// Subtype (bot_message, etc.)
    subtype: Option<String>,
}

/// Metadata stored with emitted messages for response routing.
#[derive(Debug, Serialize, Deserialize)]
struct SlackMessageMetadata {
    /// Slack channel ID.
    channel: String,

    /// Thread timestamp for threaded replies.
    thread_ts: Option<String>,

    /// Original message timestamp.
    message_ts: String,

    /// Team ID.
    team_id: Option<String>,
}

/// Slack API response for chat.postMessage.
#[derive(Debug, Deserialize)]
struct SlackPostMessageResponse {
    ok: bool,
    error: Option<String>,
    ts: Option<String>,
}

/// Slack interactive payload (sent when a Block Kit button is clicked).
/// https://api.slack.com/reference/interaction-payloads/block-actions
#[derive(Debug, Deserialize)]
struct SlackInteractivePayload {
    /// Payload type ("block_actions" for button clicks).
    #[serde(rename = "type")]
    payload_type: String,

    /// User who clicked the button.
    user: SlackActionUser,

    /// Channel where the interaction occurred.
    channel: Option<SlackActionChannel>,

    /// The actions that were triggered.
    #[serde(default)]
    actions: Vec<SlackAction>,

    /// Team info.
    team: Option<SlackActionTeam>,
}

#[derive(Debug, Deserialize)]
struct SlackActionUser {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SlackActionChannel {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SlackActionTeam {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SlackAction {
    /// The action_id set on the button ("approve_yes", "approve_always", "approve_deny").
    action_id: String,
}

/// Workspace path for persisting the proactive notification channel.
const NOTIFY_CHANNEL_PATH: &str = "state/notify_channel";

/// Channel configuration from capabilities file.
#[derive(Debug, Deserialize)]
struct SlackConfig {
    /// Name of secret containing signing secret (for verification by host).
    /// Parsed for forward compatibility; host handles signature verification.
    #[serde(default = "default_signing_secret_name")]
    #[allow(dead_code)]
    signing_secret_name: String,

    /// Slack channel ID (e.g. "C01234ABC") or Slack member ID (e.g. "U01234ABC")
    /// to use for proactive notifications (routines, heartbeat alerts).
    ///
    /// When set to a member ID, the bot will open a DM with that user.
    /// Required for `on_broadcast` to deliver notifications.
    #[serde(default)]
    notify_channel: Option<String>,
}

fn default_signing_secret_name() -> String {
    "slack_signing_secret".to_string()
}

/// Response from `conversations.open`.
#[derive(Debug, Deserialize)]
struct ConversationsOpenResponse {
    ok: bool,
    error: Option<String>,
    channel: Option<ConversationsOpenChannel>,
}

#[derive(Debug, Deserialize)]
struct ConversationsOpenChannel {
    id: String,
}

struct SlackChannel;

impl Guest for SlackChannel {
    fn on_start(config_json: String) -> Result<ChannelConfig, String> {
        // Parse configuration
        let config: SlackConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse config: {}", e))?;

        channel_host::log(channel_host::LogLevel::Info, "Slack channel starting");

        // Persist notify_channel so on_broadcast can use it without re-reading config
        if let Some(ref ch) = config.notify_channel {
            let _ = channel_host::workspace_write(NOTIFY_CHANNEL_PATH, ch);
        }

        Ok(ChannelConfig {
            display_name: "Slack".to_string(),
            http_endpoints: vec![HttpEndpointConfig {
                path: "/webhook/slack".to_string(),
                methods: vec!["POST".to_string()],
                require_secret: true,
            }],
            poll: None, // Slack uses push via webhooks, no polling needed
        })
    }

    fn on_http_request(req: IncomingHttpRequest) -> OutgoingHttpResponse {
        // Parse the request body
        let body_str = match std::str::from_utf8(&req.body) {
            Ok(s) => s,
            Err(_) => {
                return json_response(400, serde_json::json!({"error": "Invalid UTF-8 body"}));
            }
        };

        // Slack interactive payloads (button clicks) arrive form-encoded as:
        //   payload=<url-encoded JSON>
        // Detect and handle these before trying to parse as an Events API payload.
        if let Some(encoded) = body_str.strip_prefix("payload=") {
            let decoded = percent_decode(encoded);
            return handle_interactive_payload(&decoded);
        }

        // Parse as Slack event
        let event_wrapper: SlackEventWrapper = match serde_json::from_str(body_str) {
            Ok(e) => e,
            Err(e) => {
                channel_host::log(
                    channel_host::LogLevel::Error,
                    &format!("Failed to parse Slack event: {}", e),
                );
                return json_response(400, serde_json::json!({"error": "Invalid event payload"}));
            }
        };

        match event_wrapper.event_type.as_str() {
            // URL verification challenge (Slack setup)
            "url_verification" => {
                if let Some(challenge) = event_wrapper.challenge {
                    channel_host::log(
                        channel_host::LogLevel::Info,
                        "Responding to Slack URL verification",
                    );
                    json_response(200, serde_json::json!({"challenge": challenge}))
                } else {
                    json_response(400, serde_json::json!({"error": "Missing challenge"}))
                }
            }

            // Actual event callback
            "event_callback" => {
                if let Some(event) = event_wrapper.event {
                    handle_slack_event(event, event_wrapper.team_id, event_wrapper.event_id);
                }
                // Always respond 200 quickly to Slack (they have a 3s timeout)
                json_response(200, serde_json::json!({"ok": true}))
            }

            // Unknown event type
            _ => {
                channel_host::log(
                    channel_host::LogLevel::Warn,
                    &format!("Unknown Slack event type: {}", event_wrapper.event_type),
                );
                json_response(200, serde_json::json!({"ok": true}))
            }
        }
    }

    fn on_poll() {
        // Slack uses webhooks, no polling needed
    }

    fn on_respond(response: AgentResponse) -> Result<(), String> {
        // Parse metadata to get channel info
        let metadata: SlackMessageMetadata = serde_json::from_str(&response.metadata_json)
            .map_err(|e| format!("Failed to parse metadata: {}", e))?;

        // Build Slack API request
        let mut payload = serde_json::json!({
            "channel": metadata.channel,
            "text": response.content,
        });

        // Add thread_ts for threaded replies
        if let Some(thread_ts) = response.thread_id.or(metadata.thread_ts) {
            payload["thread_ts"] = serde_json::Value::String(thread_ts);
        }

        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize payload: {}", e))?;

        // Make HTTP request to Slack API
        // The bot token is injected by the host based on credential configuration
        let headers = serde_json::json!({
            "Content-Type": "application/json"
        });

        let result = channel_host::http_request(
            "POST",
            "https://slack.com/api/chat.postMessage",
            &headers.to_string(),
            Some(&payload_bytes),
            None,
        );

        match result {
            Ok(http_response) => {
                if http_response.status != 200 {
                    return Err(format!(
                        "Slack API returned status {}",
                        http_response.status
                    ));
                }

                // Parse Slack response
                let slack_response: SlackPostMessageResponse =
                    serde_json::from_slice(&http_response.body)
                        .map_err(|e| format!("Failed to parse Slack response: {}", e))?;

                if !slack_response.ok {
                    return Err(format!(
                        "Slack API error: {}",
                        slack_response
                            .error
                            .unwrap_or_else(|| "unknown".to_string())
                    ));
                }

                channel_host::log(
                    channel_host::LogLevel::Debug,
                    &format!(
                        "Posted message to Slack channel {}: ts={}",
                        metadata.channel,
                        slack_response.ts.unwrap_or_default()
                    ),
                );

                Ok(())
            }
            Err(e) => Err(format!("HTTP request failed: {}", e)),
        }
    }

    fn on_status(update: StatusUpdate) {
        if !matches!(update.status, StatusType::ApprovalNeeded) {
            return;
        }

        let metadata: SlackMessageMetadata = match serde_json::from_str(&update.metadata_json) {
            Ok(m) => m,
            Err(_) => return,
        };

        let (tool_name, description, params_str) = update
            .extra_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .map(|v| {
                let tool = v["tool_name"].as_str().unwrap_or("unknown").to_string();
                let desc = v["description"].as_str().unwrap_or("").to_string();
                let params = format_params(&v["parameters"]);
                (tool, desc, params)
            })
            .unwrap_or_else(|| ("unknown".to_string(), String::new(), String::new()));

        // Build the section text (used as plain-text fallback too)
        let mut section_text = format!("⚠️ *Approval required* — tool: `{}`", tool_name);
        if !description.is_empty() {
            section_text.push('\n');
            section_text.push_str(&description);
        }
        if !params_str.is_empty() {
            section_text.push_str("\nParameters: `");
            section_text.push_str(&params_str);
            section_text.push('`');
        }

        // Use Block Kit: section with the message + actions row with buttons
        let blocks = serde_json::json!([
            {
                "type": "section",
                "text": { "type": "mrkdwn", "text": section_text }
            },
            {
                "type": "actions",
                "elements": [
                    {
                        "type": "button",
                        "text": { "type": "plain_text", "text": "✅ Approve" },
                        "action_id": "approve_yes",
                        "style": "primary"
                    },
                    {
                        "type": "button",
                        "text": { "type": "plain_text", "text": "🔁 Always" },
                        "action_id": "approve_always"
                    },
                    {
                        "type": "button",
                        "text": { "type": "plain_text", "text": "❌ Deny" },
                        "action_id": "approve_deny",
                        "style": "danger"
                    }
                ]
            }
        ]);

        let mut payload = serde_json::json!({
            "channel": metadata.channel,
            "text": section_text,  // plain-text fallback for notifications
            "blocks": blocks,
        });
        if let Some(thread_ts) = metadata.thread_ts {
            payload["thread_ts"] = serde_json::Value::String(thread_ts);
        }

        let payload_bytes = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(_) => return,
        };
        let headers = serde_json::json!({ "Content-Type": "application/json" });
        if let Err(e) = channel_host::http_request(
            "POST",
            "https://slack.com/api/chat.postMessage",
            &headers.to_string(),
            Some(&payload_bytes),
            None,
        ) {
            channel_host::log(
                channel_host::LogLevel::Warn,
                &format!("Failed to send approval prompt: {}", e),
            );
        }
    }

    fn on_event(_event_json: String) -> Result<(), String> {
        // This channel does not use persistent connections; events are delivered via polling.
        Ok(())
    }

    fn on_broadcast(user_id: String, content: String, _metadata_json: String) -> Result<(), String> {
        // Resolve the Slack channel ID to post to.
        //
        // Resolution order:
        // 1. If user_id is a Slack member ID ("U…" or "W…"), open a DM via conversations.open.
        // 2. If user_id is a channel ID ("C…" or "D…"), post directly.
        // 3. If user_id is "default" or empty, use the configured notify_channel.
        let channel_id = if user_id.starts_with('U') || user_id.starts_with('W') {
            // Open (or retrieve) a DM channel with this member
            open_dm_with_user(&user_id)?
        } else if user_id.starts_with('C') || user_id.starts_with('D') {
            // Already a channel or DM ID
            user_id.clone()
        } else {
            // "default" or empty — use the persisted notify_channel
            channel_host::workspace_read(NOTIFY_CHANNEL_PATH)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    "on_broadcast: no notify_channel configured. \
                     Set config.notify_channel to a Slack channel or member ID."
                        .to_string()
                })?
        };

        post_message(&channel_id, &content)
    }

    fn on_shutdown() {
        channel_host::log(channel_host::LogLevel::Info, "Slack channel shutting down");
    }
}

/// Minimal percent-decode for Slack's `payload=` form field.
/// Handles `%XX` sequences and `+` as space.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push((hi * 16 + lo) as u8 as char);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(' ');
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

/// Handle a Slack interactive payload (block_actions — button click).
fn handle_interactive_payload(json: &str) -> OutgoingHttpResponse {
    let payload: SlackInteractivePayload = match serde_json::from_str(json) {
        Ok(p) => p,
        Err(e) => {
            channel_host::log(
                channel_host::LogLevel::Error,
                &format!("Failed to parse Slack interactive payload: {}", e),
            );
            return json_response(400, serde_json::json!({"error": "Invalid interactive payload"}));
        }
    };

    if payload.payload_type != "block_actions" {
        return json_response(200, serde_json::json!({"ok": true}));
    }

    let action = match payload.actions.first() {
        Some(a) => a,
        None => return json_response(200, serde_json::json!({"ok": true})),
    };

    // Map action_id to approval text
    let content = match action.action_id.as_str() {
        "approve_yes" => "yes",
        "approve_always" => "always",
        "approve_deny" => "no",
        _ => return json_response(200, serde_json::json!({"ok": true})),
    };

    let channel_id = match &payload.channel {
        Some(c) => c.id.clone(),
        None => return json_response(200, serde_json::json!({"ok": true})),
    };

    let team_id = payload.team.as_ref().map(|t| t.id.clone());

    // Emit as a regular message — the approval flow handles it identically to typed input
    emit_message(
        payload.user.id,
        content.to_string(),
        channel_id,
        None,
        team_id,
    );

    json_response(200, serde_json::json!({"ok": true}))
}

/// Format tool parameters as a compact, truncated string for display.
fn format_params(params: &serde_json::Value) -> String {
    if params.is_null() {
        return String::new();
    }
    let s = params.to_string();
    if s == "{}" || s == "null" {
        return String::new();
    }
    if s.len() > 300 {
        format!("{}…", &s[..300])
    } else {
        s
    }
}

/// Handle a Slack event and emit message if applicable.
fn handle_slack_event(event: SlackEvent, team_id: Option<String>, _event_id: Option<String>) {
    match event.event_type.as_str() {
        // Direct mention of the bot
        "app_mention" => {
            if let (Some(user), Some(channel), Some(text), Some(ts)) = (
                event.user,
                event.channel.clone(),
                event.text,
                event.ts.clone(),
            ) {
                emit_message(user, text, channel, event.thread_ts.or(Some(ts)), team_id);
            }
        }

        // Direct message to the bot
        "message" => {
            // Skip messages from bots (including ourselves)
            if event.bot_id.is_some() || event.subtype.is_some() {
                return;
            }

            if let (Some(user), Some(channel), Some(text), Some(ts)) = (
                event.user,
                event.channel.clone(),
                event.text,
                event.ts.clone(),
            ) {
                // Only process DMs (channel IDs starting with D)
                if channel.starts_with('D') {
                    emit_message(user, text, channel, event.thread_ts.or(Some(ts)), team_id);
                }
            }
        }

        _ => {
            channel_host::log(
                channel_host::LogLevel::Debug,
                &format!("Ignoring Slack event type: {}", event.event_type),
            );
        }
    }
}

/// Emit a message to the agent.
fn emit_message(
    user_id: String,
    text: String,
    channel: String,
    thread_ts: Option<String>,
    team_id: Option<String>,
) {
    let message_ts = thread_ts.clone().unwrap_or_default();

    let metadata = SlackMessageMetadata {
        channel: channel.clone(),
        thread_ts: thread_ts.clone(),
        message_ts: message_ts.clone(),
        team_id,
    };

    let metadata_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());

    // Strip @ mentions of the bot from the text for cleaner messages
    let cleaned_text = strip_bot_mention(&text);

    channel_host::emit_message(&EmittedMessage {
        user_id,
        user_name: None, // Could fetch from Slack API if needed
        content: cleaned_text,
        thread_id: thread_ts,
        metadata_json,
    });
}

/// Strip leading bot mention from text.
fn strip_bot_mention(text: &str) -> String {
    // Slack mentions look like <@U12345678>
    let trimmed = text.trim();
    if trimmed.starts_with("<@") {
        if let Some(end) = trimmed.find('>') {
            return trimmed[end + 1..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

/// Create a JSON HTTP response.
fn json_response(status: u16, value: serde_json::Value) -> OutgoingHttpResponse {
    let body = serde_json::to_vec(&value).unwrap_or_default();
    let headers = serde_json::json!({"Content-Type": "application/json"});

    OutgoingHttpResponse {
        status,
        headers_json: headers.to_string(),
        body,
    }
}

/// Post a plain-text message to a Slack channel or DM.
fn post_message(channel_id: &str, text: &str) -> Result<(), String> {
    let payload = serde_json::json!({ "channel": channel_id, "text": text });
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|e| format!("Serialize error: {}", e))?;
    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let http_response = channel_host::http_request(
        "POST",
        "https://slack.com/api/chat.postMessage",
        &headers.to_string(),
        Some(&payload_bytes),
        None,
    )
    .map_err(|e| format!("HTTP request failed: {}", e))?;

    if http_response.status != 200 {
        return Err(format!("Slack API returned status {}", http_response.status));
    }

    let slack_response: SlackPostMessageResponse = serde_json::from_slice(&http_response.body)
        .map_err(|e| format!("Failed to parse Slack response: {}", e))?;

    if !slack_response.ok {
        return Err(format!(
            "Slack API error: {}",
            slack_response.error.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    channel_host::log(
        channel_host::LogLevel::Debug,
        &format!("Broadcast message posted to Slack channel {}", channel_id),
    );

    Ok(())
}

/// Open (or retrieve) a DM channel with a Slack member and return its channel ID.
fn open_dm_with_user(user_id: &str) -> Result<String, String> {
    let payload = serde_json::json!({ "users": user_id });
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|e| format!("Serialize error: {}", e))?;
    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let http_response = channel_host::http_request(
        "POST",
        "https://slack.com/api/conversations.open",
        &headers.to_string(),
        Some(&payload_bytes),
        None,
    )
    .map_err(|e| format!("HTTP request failed: {}", e))?;

    if http_response.status != 200 {
        return Err(format!(
            "conversations.open returned status {}",
            http_response.status
        ));
    }

    let resp: ConversationsOpenResponse = serde_json::from_slice(&http_response.body)
        .map_err(|e| format!("Failed to parse conversations.open response: {}", e))?;

    if !resp.ok {
        return Err(format!(
            "conversations.open error: {}",
            resp.error.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    resp.channel
        .map(|c| c.id)
        .ok_or_else(|| "conversations.open returned no channel".to_string())
}

// Export the component
export!(SlackChannel);

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- strip_bot_mention ---

    #[test]
    fn test_strip_mention_present() {
        assert_eq!(strip_bot_mention("<@U12345678> hello world"), "hello world");
    }

    #[test]
    fn test_strip_mention_no_mention() {
        assert_eq!(strip_bot_mention("hello world"), "hello world");
    }

    #[test]
    fn test_strip_mention_trims_whitespace() {
        assert_eq!(strip_bot_mention("  hello  "), "hello");
    }

    #[test]
    fn test_strip_mention_only_mention() {
        assert_eq!(strip_bot_mention("<@UABC123>"), "");
    }

    // --- SlackConfig deserialization ---

    #[test]
    fn test_config_minimal_defaults() {
        let json = r#"{}"#;
        let config: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.signing_secret_name, "slack_signing_secret");
        assert!(config.notify_channel.is_none());
    }

    #[test]
    fn test_config_with_notify_channel() {
        let json = r#"{"notify_channel": "C01234ABC"}"#;
        let config: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.notify_channel, Some("C01234ABC".to_string()));
    }

    #[test]
    fn test_config_with_member_id_notify_channel() {
        let json = r#"{"notify_channel": "U01MEMBER"}"#;
        let config: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.notify_channel, Some("U01MEMBER".to_string()));
    }

    #[test]
    fn test_config_custom_signing_secret() {
        let json = r#"{"signing_secret_name": "my_slack_secret"}"#;
        let config: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.signing_secret_name, "my_slack_secret");
    }

    // --- ConversationsOpenResponse parsing ---

    #[test]
    fn test_conversations_open_response_ok() {
        let json = r#"{"ok": true, "channel": {"id": "D01234XYZ"}}"#;
        let resp: ConversationsOpenResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.channel.unwrap().id, "D01234XYZ");
    }

    #[test]
    fn test_conversations_open_response_error() {
        let json = r#"{"ok": false, "error": "user_not_found"}"#;
        let resp: ConversationsOpenResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error, Some("user_not_found".to_string()));
        assert!(resp.channel.is_none());
    }

    // --- on_broadcast routing decisions (pure logic, no host calls) ---

    #[test]
    fn test_broadcast_user_id_prefix_member() {
        // U and W prefixes indicate Slack member IDs (DM routing)
        assert!("U01MEMBER".starts_with('U'));
        assert!("W01MEMBER".starts_with('W'));
        // C and D prefixes are already channel/DM IDs (direct routing)
        assert!("C01CHANNEL".starts_with('C'));
        assert!("D01DMCHAN".starts_with('D'));
    }

    #[test]
    fn test_broadcast_default_user_id_falls_through() {
        // "default" doesn't start with U, W, C, or D
        let user_id = "default";
        assert!(!user_id.starts_with('U'));
        assert!(!user_id.starts_with('W'));
        assert!(!user_id.starts_with('C'));
        assert!(!user_id.starts_with('D'));
    }

    #[test]
    fn test_broadcast_empty_user_id_falls_through() {
        let user_id = "";
        assert!(!user_id.starts_with('U'));
        assert!(!user_id.starts_with('W'));
        assert!(!user_id.starts_with('C'));
        assert!(!user_id.starts_with('D'));
    }

    // --- SlackPostMessageResponse parsing ---

    #[test]
    fn test_post_message_response_ok() {
        let json = r#"{"ok": true, "ts": "1234567890.123456"}"#;
        let resp: SlackPostMessageResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.ts, Some("1234567890.123456".to_string()));
    }

    #[test]
    fn test_post_message_response_error() {
        let json = r#"{"ok": false, "error": "channel_not_found"}"#;
        let resp: SlackPostMessageResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error, Some("channel_not_found".to_string()));
    }

    // --- SlackMessageMetadata roundtrip ---

    #[test]
    fn test_metadata_roundtrip_with_thread() {
        let meta = SlackMessageMetadata {
            channel: "C01CHANNEL".to_string(),
            thread_ts: Some("1234.5678".to_string()),
            message_ts: "1234.5678".to_string(),
            team_id: Some("T01TEAM".to_string()),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: SlackMessageMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.channel, "C01CHANNEL");
        assert_eq!(parsed.thread_ts, Some("1234.5678".to_string()));
        assert_eq!(parsed.team_id, Some("T01TEAM".to_string()));
    }

    #[test]
    fn test_metadata_roundtrip_no_thread() {
        let meta = SlackMessageMetadata {
            channel: "D01DMCHAN".to_string(),
            thread_ts: None,
            message_ts: "9876.5432".to_string(),
            team_id: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: SlackMessageMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.channel, "D01DMCHAN");
        assert!(parsed.thread_ts.is_none());
        assert!(parsed.team_id.is_none());
    }

    // --- percent_decode ---

    #[test]
    fn test_percent_decode_plain_string() {
        assert_eq!(percent_decode("hello"), "hello");
    }

    #[test]
    fn test_percent_decode_encoded_chars() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("%7B%22key%22%3A%22value%22%7D"), r#"{"key":"value"}"#);
    }

    #[test]
    fn test_percent_decode_plus_as_space() {
        assert_eq!(percent_decode("hello+world"), "hello world");
    }

    #[test]
    fn test_percent_decode_empty_string() {
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn test_percent_decode_incomplete_sequence_passthrough() {
        // A lone '%' with no two following hex digits is passed through literally.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
    }

    // --- format_params ---

    #[test]
    fn test_format_params_null() {
        assert_eq!(format_params(&serde_json::Value::Null), "");
    }

    #[test]
    fn test_format_params_empty_object() {
        assert_eq!(format_params(&serde_json::json!({})), "");
    }

    #[test]
    fn test_format_params_normal_object() {
        let v = serde_json::json!({"key": "value"});
        let result = format_params(&v);
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn test_format_params_truncates_long_input() {
        let long_str = "x".repeat(400);
        let v = serde_json::json!({"data": long_str});
        let result = format_params(&v);
        assert!(result.ends_with('…'));
        // The truncated portion is 300 bytes of the serialised JSON.
        let without_ellipsis: String = result.chars().filter(|&c| c != '…').collect();
        assert_eq!(without_ellipsis.len(), 300);
    }

    // --- SlackInteractivePayload parsing ---

    #[test]
    fn test_interactive_payload_block_actions_parses() {
        let json = serde_json::json!({
            "type": "block_actions",
            "user": {"id": "U123", "name": "alice"},
            "channel": {"id": "C456"},
            "team": {"id": "T789"},
            "actions": [{"action_id": "approve_yes", "block_id": "b1", "value": "yes"}]
        })
        .to_string();
        let payload: SlackInteractivePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.payload_type, "block_actions");
        assert_eq!(payload.user.id, "U123");
        assert_eq!(payload.channel.as_ref().unwrap().id, "C456");
        assert_eq!(payload.team.as_ref().unwrap().id, "T789");
        assert_eq!(payload.actions[0].action_id, "approve_yes");
    }

    #[test]
    fn test_interactive_payload_missing_channel_is_none() {
        let json = serde_json::json!({
            "type": "block_actions",
            "user": {"id": "U123", "name": "alice"},
            "actions": [{"action_id": "approve_deny", "block_id": "b1", "value": "no"}]
        })
        .to_string();
        let payload: SlackInteractivePayload = serde_json::from_str(&json).unwrap();
        assert!(payload.channel.is_none());
        assert!(payload.team.is_none());
    }

    #[test]
    fn test_interactive_payload_action_ids() {
        for action_id in &["approve_yes", "approve_always", "approve_deny"] {
            let json = serde_json::json!({
                "type": "block_actions",
                "user": {"id": "U1", "name": "u"},
                "channel": {"id": "C1"},
                "actions": [{"action_id": action_id, "block_id": "b", "value": "v"}]
            })
            .to_string();
            let payload: SlackInteractivePayload = serde_json::from_str(&json).unwrap();
            assert_eq!(&payload.actions[0].action_id, action_id);
        }
    }
}
