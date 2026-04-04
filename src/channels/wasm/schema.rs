//! JSON schema for WASM channel capabilities files.
//!
//! External WASM channels declare their required capabilities via a sidecar JSON file
//! (e.g., `slack.capabilities.json`). This module defines the schema for those files
//! and provides conversion to runtime [`ChannelCapabilities`].
//!
//! # Example Capabilities File
//!
//! ```json
//! {
//!   "type": "channel",
//!   "name": "slack",
//!   "description": "Slack Events API channel",
//!   "capabilities": {
//!     "http": {
//!       "allowlist": [
//!         { "host": "slack.com", "path_prefix": "/api/" }
//!       ],
//!       "credentials": {
//!         "slack_bot": {
//!           "secret_name": "slack_bot_token",
//!           "location": { "type": "bearer" },
//!           "host_patterns": ["slack.com"]
//!         }
//!       }
//!     },
//!     "secrets": { "allowed_names": ["slack_*"] },
//!     "channel": {
//!       "allowed_paths": ["/webhook/slack"],
//!       "allow_polling": false,
//!       "workspace_prefix": "channels/slack/",
//!       "emit_rate_limit": { "messages_per_minute": 100 }
//!     }
//!   },
//!   "config": {
//!     "signing_secret_name": "slack_signing_secret"
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::channels::wasm::capabilities::{
    ChannelCapabilities, EmitRateLimitConfig, MIN_POLL_INTERVAL_MS,
};
use crate::tools::wasm::{CapabilitiesFile as ToolCapabilitiesFile, RateLimitSchema};

/// Root schema for a channel capabilities JSON file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelCapabilitiesFile {
    /// File type, must be "channel".
    #[serde(default = "default_type")]
    pub r#type: String,

    /// Channel name.
    pub name: String,

    /// Channel description.
    #[serde(default)]
    pub description: Option<String>,

    /// Setup configuration for the wizard.
    #[serde(default)]
    pub setup: SetupSchema,

    /// Capabilities (tool + channel specific).
    #[serde(default)]
    pub capabilities: ChannelCapabilitiesSchema,

    /// Channel-specific configuration passed to on_start.
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

fn default_type() -> String {
    "channel".to_string()
}

impl ChannelCapabilitiesFile {
    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Parse from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Convert to runtime ChannelCapabilities.
    pub fn to_capabilities(&self) -> ChannelCapabilities {
        self.capabilities.to_channel_capabilities(&self.name)
    }

    /// Get the channel config as JSON string.
    pub fn config_json(&self) -> String {
        serde_json::to_string(&self.config).unwrap_or_else(|_| "{}".to_string())
    }

    /// Get the webhook secret header name for this channel.
    ///
    /// Returns the configured header name from capabilities, or a sensible default.
    pub fn webhook_secret_header(&self) -> Option<&str> {
        self.capabilities
            .channel
            .as_ref()
            .and_then(|c| c.webhook.as_ref())
            .and_then(|w| w.secret_header.as_deref())
    }

    /// Get the webhook secret name for this channel.
    ///
    /// Returns the configured secret name or defaults to "{channel_name}_webhook_secret".
    pub fn webhook_secret_name(&self) -> String {
        self.capabilities
            .channel
            .as_ref()
            .and_then(|c| c.webhook.as_ref())
            .and_then(|w| w.secret_name.clone())
            .unwrap_or_else(|| format!("{}_webhook_secret", self.name))
    }
}

/// Schema for channel capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelCapabilitiesSchema {
    /// Tool capabilities (HTTP, secrets, workspace_read).
    /// Note: Using the struct directly (not Option) because #[serde(flatten)]
    /// with Option<T> doesn't work correctly when T has all-optional fields.
    #[serde(flatten)]
    pub tool: ToolCapabilitiesFile,

    /// Channel-specific capabilities.
    #[serde(default)]
    pub channel: Option<ChannelSpecificCapabilitiesSchema>,
}

impl ChannelCapabilitiesSchema {
    /// Convert to runtime ChannelCapabilities.
    pub fn to_channel_capabilities(&self, channel_name: &str) -> ChannelCapabilities {
        let tool_caps = self.tool.to_capabilities();

        let mut caps =
            ChannelCapabilities::for_channel(channel_name).with_tool_capabilities(tool_caps);

        if let Some(channel) = &self.channel {
            caps.allowed_paths = channel.allowed_paths.clone();
            caps.allow_polling = channel.allow_polling;
            caps.min_poll_interval_ms = channel
                .min_poll_interval_ms
                .unwrap_or(MIN_POLL_INTERVAL_MS)
                .max(MIN_POLL_INTERVAL_MS);

            if let Some(prefix) = &channel.workspace_prefix {
                caps.workspace_prefix = prefix.clone();
            }

            if let Some(rate) = &channel.emit_rate_limit {
                caps.emit_rate_limit = rate.to_emit_rate_limit();
            }

            if let Some(max_size) = channel.max_message_size {
                caps.max_message_size = max_size;
            }

            if let Some(timeout_secs) = channel.callback_timeout_secs {
                caps.callback_timeout = Duration::from_secs(timeout_secs);
            }

            if let Some(conn) = &channel.connection {
                caps.connection = Some(conn.clone().into());
            }
        }

        caps
    }
}

/// Channel-specific capabilities schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelSpecificCapabilitiesSchema {
    /// HTTP paths the channel can register for webhooks.
    #[serde(default)]
    pub allowed_paths: Vec<String>,

    /// Whether polling is allowed.
    #[serde(default)]
    pub allow_polling: bool,

    /// Minimum poll interval in milliseconds.
    #[serde(default)]
    pub min_poll_interval_ms: Option<u32>,

    /// Workspace prefix for storage (overrides default).
    #[serde(default)]
    pub workspace_prefix: Option<String>,

    /// Rate limiting for emit_message.
    #[serde(default)]
    pub emit_rate_limit: Option<EmitRateLimitSchema>,

    /// Maximum message content size in bytes.
    #[serde(default)]
    pub max_message_size: Option<usize>,

    /// Callback timeout in seconds.
    #[serde(default)]
    pub callback_timeout_secs: Option<u64>,

    /// Webhook configuration (secret header, etc.).
    #[serde(default)]
    pub webhook: Option<WebhookSchema>,

    /// Persistent connection broker configuration.
    /// When present, the host spawns a connection broker task that maintains
    /// a WebSocket, long-poll, or SSE connection and delivers events to WASM
    /// via `on_event` callbacks.
    #[serde(default)]
    pub connection: Option<ConnectionSchema>,
}

/// Webhook configuration schema.
///
/// Allows channels to specify their webhook validation requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSchema {
    /// HTTP header name for secret validation.
    ///
    /// Examples:
    /// - Telegram: "X-Telegram-Bot-Api-Secret-Token"
    /// - Slack: "X-Slack-Signature"
    /// - GitHub: "X-Hub-Signature-256"
    /// - Generic: "X-Webhook-Secret"
    #[serde(default)]
    pub secret_header: Option<String>,

    /// Secret name in secrets store for webhook validation.
    /// Default: "{channel_name}_webhook_secret"
    #[serde(default)]
    pub secret_name: Option<String>,
}

/// Setup configuration schema.
///
/// Allows channels to declare their setup requirements for the wizard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupSchema {
    /// Required secrets that must be configured during setup.
    #[serde(default)]
    pub required_secrets: Vec<SecretSetupSchema>,

    /// Optional validation endpoint to verify configuration.
    /// Placeholders like {secret_name} are replaced with actual values.
    #[serde(default)]
    pub validation_endpoint: Option<String>,
}

/// Configuration for a secret required during setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretSetupSchema {
    /// Secret name in the secrets store (e.g., "telegram_bot_token").
    pub name: String,

    /// Prompt to show the user during setup.
    pub prompt: String,

    /// Optional regex for validation.
    #[serde(default)]
    pub validation: Option<String>,

    /// Whether this secret is optional.
    #[serde(default)]
    pub optional: bool,

    /// Auto-generate configuration if the user doesn't provide a value.
    #[serde(default)]
    pub auto_generate: Option<AutoGenerateSchema>,
}

/// Configuration for auto-generating a secret value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoGenerateSchema {
    /// Length of the generated value in bytes (will be hex-encoded).
    #[serde(default = "default_auto_generate_length")]
    pub length: usize,
}

fn default_auto_generate_length() -> usize {
    32
}

/// Schema for emit rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitRateLimitSchema {
    /// Maximum messages per minute.
    #[serde(default = "default_messages_per_minute")]
    pub messages_per_minute: u32,

    /// Maximum messages per hour.
    #[serde(default = "default_messages_per_hour")]
    pub messages_per_hour: u32,
}

fn default_messages_per_minute() -> u32 {
    100
}

fn default_messages_per_hour() -> u32 {
    5000
}

impl EmitRateLimitSchema {
    fn to_emit_rate_limit(&self) -> EmitRateLimitConfig {
        EmitRateLimitConfig {
            messages_per_minute: self.messages_per_minute,
            messages_per_hour: self.messages_per_hour,
        }
    }
}

impl From<RateLimitSchema> for EmitRateLimitSchema {
    fn from(schema: RateLimitSchema) -> Self {
        Self {
            messages_per_minute: schema.requests_per_minute,
            messages_per_hour: schema.requests_per_hour,
        }
    }
}

// ============================================================================
// Connection broker schema types
// ============================================================================

/// Connection broker configuration schema.
///
/// Declares a persistent connection the host should maintain on behalf of the WASM channel.
/// The broker handles connection lifecycle (connect, heartbeat, reconnect); WASM handles
/// message processing via `on_event` callbacks.
///
/// # Example (Discord Gateway WebSocket)
///
/// ```json
/// {
///   "type": "websocket",
///   "url": "wss://gateway.discord.gg/?v=10&encoding=json",
///   "keepalive": { "type": "json_opcode", "interval_field": "d.heartbeat_interval", ... },
///   "handshake": { "send": { "op": 2, "d": { "token": "{DISCORD_BOT_TOKEN}", ... } } },
///   "reconnect": { "max_retries": 5, "backoff_ms": 1000 },
///   "events": { "deliver_to_wasm": ["MESSAGE_CREATE"], "type_field": "t" }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionSchema {
    /// Connection protocol type.
    pub r#type: ConnectionType,

    /// Connection URL (may contain credential placeholders like `{DISCORD_BOT_TOKEN}`).
    /// Mutually exclusive with `url_from_api`.
    #[serde(default)]
    pub url: Option<String>,

    /// Obtain the connection URL by calling an API endpoint first.
    /// Used by Slack Socket Mode (POST to apps.connections.open to get WSS URL).
    #[serde(default)]
    pub url_from_api: Option<UrlFromApiSchema>,

    /// Keepalive/heartbeat configuration.
    #[serde(default)]
    pub keepalive: Option<KeepaliveSchema>,

    /// Handshake message to send after connecting (e.g., Discord Identify op 2).
    /// May contain credential placeholders.
    #[serde(default)]
    pub handshake: Option<HandshakeSchema>,

    /// Reconnection policy.
    #[serde(default)]
    pub reconnect: ReconnectSchema,

    /// Event filtering — which events to deliver to WASM vs drop silently.
    #[serde(default)]
    pub events: EventFilterSchema,

    /// Maximum size of a single inbound event in bytes (default: 64KB).
    #[serde(default = "default_max_event_size")]
    pub max_event_size: usize,

    /// Maximum number of events to buffer before dropping oldest (default: 100).
    #[serde(default = "default_event_queue_size")]
    pub event_queue_size: usize,

    /// Additional allowlist entries for the connection URL.
    /// Merged with the existing HTTP allowlist.
    #[serde(default)]
    pub additional_allowlist: Vec<AllowlistEntrySchema>,
}

/// Connection protocol type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    Websocket,
    LongPoll,
    Sse,
}

/// Obtain connection URL from an API call.
///
/// Some services (e.g., Slack Socket Mode) require a POST to get a temporary WSS URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlFromApiSchema {
    /// HTTP method (e.g., "POST").
    pub method: String,
    /// API endpoint URL (may contain credential placeholders).
    pub endpoint: String,
    /// JSON path to extract the URL from the response (dot-separated, e.g., "url").
    pub url_field: String,
}

/// Keepalive configuration.
///
/// Configures how the broker keeps the connection alive. Different services use
/// different strategies: WebSocket ping/pong, JSON opcodes (Discord), or nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeepaliveSchema {
    /// Keepalive strategy.
    pub r#type: KeepaliveType,

    /// For `json_opcode` type: which field in the server's Hello message contains the interval.
    /// Dot-separated path (e.g., "d.heartbeat_interval" for Discord).
    #[serde(default)]
    pub interval_field: Option<String>,

    /// For `json_opcode` type: the JSON payload to send as heartbeat.
    #[serde(default)]
    pub send: Option<serde_json::Value>,

    /// For `json_opcode` type: the expected ACK shape (broker checks `op` field match).
    #[serde(default)]
    pub expect: Option<serde_json::Value>,

    /// Fallback fixed interval in milliseconds (used if server doesn't provide one).
    #[serde(default = "default_keepalive_interval_ms")]
    pub fallback_interval_ms: u64,
}

/// Keepalive strategy type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepaliveType {
    /// WebSocket ping/pong frames (protocol-level).
    PingPong,
    /// JSON opcode-based heartbeat (e.g., Discord op 1/11).
    JsonOpcode,
    /// No keepalive needed (server handles it).
    None,
}

/// Handshake message sent after connection established.
///
/// For example, Discord requires an Identify payload (op 2) with the bot token
/// after connecting. The `send` field may contain credential placeholders like
/// `{DISCORD_BOT_TOKEN}` which are substituted at the host boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeSchema {
    /// JSON payload to send. May contain credential placeholders.
    pub send: serde_json::Value,

    /// Optional: wait for a specific response before considering handshake complete.
    /// If set, the broker waits for a message matching this `op` field value.
    #[serde(default)]
    pub expect_op: Option<u64>,
}

/// Reconnection policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectSchema {
    /// Maximum reconnection attempts before giving up (0 = never reconnect).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Initial backoff in milliseconds (doubles each retry, capped at 60s).
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,

    /// Whether to attempt resume (e.g., Discord RESUME with session_id + sequence).
    /// Note: resume logic is parsed but not yet implemented.
    #[serde(default)]
    pub resumable: bool,
}

impl Default for ReconnectSchema {
    fn default() -> Self {
        Self {
            max_retries: 5,
            backoff_ms: 1000,
            resumable: false,
        }
    }
}

/// Event filtering configuration.
///
/// Controls which events from the persistent connection are delivered to WASM.
/// Events are identified by a type field in the JSON (configurable via `type_field`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFilterSchema {
    /// Event type names to deliver to WASM (empty = deliver all).
    #[serde(default)]
    pub deliver_to_wasm: Vec<String>,

    /// Event type names to silently drop (takes precedence over deliver_to_wasm).
    #[serde(default)]
    pub drop: Vec<String>,

    /// JSON path to the event type field (e.g., "t" for Discord, "type" for Slack).
    #[serde(default = "default_event_type_field")]
    pub type_field: String,
}

impl Default for EventFilterSchema {
    fn default() -> Self {
        Self {
            deliver_to_wasm: Vec::new(),
            drop: Vec::new(),
            type_field: "t".to_string(),
        }
    }
}

/// Allowlist entry for connection URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntrySchema {
    pub host: String,
    #[serde(default)]
    pub path_prefix: Option<String>,
}

fn default_max_event_size() -> usize {
    65_536
} // 64KB
fn default_event_queue_size() -> usize {
    100
}
fn default_keepalive_interval_ms() -> u64 {
    30_000
} // 30s
fn default_max_retries() -> u32 {
    5
}
fn default_backoff_ms() -> u64 {
    1_000
}
fn default_event_type_field() -> String {
    "t".to_string()
}

/// Channel configuration returned by on_start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Display name for the channel.
    pub display_name: String,

    /// HTTP endpoints to register.
    #[serde(default)]
    pub http_endpoints: Vec<HttpEndpointConfigSchema>,

    /// Polling configuration.
    #[serde(default)]
    pub poll: Option<PollConfigSchema>,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            display_name: "WASM Channel".to_string(),
            http_endpoints: Vec::new(),
            poll: None,
        }
    }
}

/// HTTP endpoint configuration schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpEndpointConfigSchema {
    /// Path to register.
    pub path: String,

    /// HTTP methods to accept.
    #[serde(default)]
    pub methods: Vec<String>,

    /// Whether secret validation is required.
    #[serde(default)]
    pub require_secret: bool,
}

/// Polling configuration schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollConfigSchema {
    /// Polling interval in milliseconds.
    pub interval_ms: u32,

    /// Whether polling is enabled.
    #[serde(default)]
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use crate::channels::wasm::schema::ChannelCapabilitiesFile;

    #[test]
    fn test_parse_minimal() {
        let json = r#"{
            "name": "test"
        }"#;
        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        assert_eq!(file.name, "test");
        assert_eq!(file.r#type, "channel");
    }

    #[test]
    fn test_parse_full_slack_example() {
        let json = r#"{
            "type": "channel",
            "name": "slack",
            "description": "Slack Events API channel",
            "capabilities": {
                "http": {
                    "allowlist": [
                        { "host": "slack.com", "path_prefix": "/api/" }
                    ],
                    "credentials": {
                        "slack_bot": {
                            "secret_name": "slack_bot_token",
                            "location": { "type": "bearer" },
                            "host_patterns": ["slack.com"]
                        }
                    },
                    "rate_limit": { "requests_per_minute": 50, "requests_per_hour": 1000 }
                },
                "secrets": { "allowed_names": ["slack_*"] },
                "channel": {
                    "allowed_paths": ["/webhook/slack"],
                    "allow_polling": false,
                    "emit_rate_limit": { "messages_per_minute": 100, "messages_per_hour": 5000 }
                }
            },
            "config": {
                "signing_secret_name": "slack_signing_secret"
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        assert_eq!(file.name, "slack");
        assert_eq!(
            file.description,
            Some("Slack Events API channel".to_string())
        );

        let caps = file.to_capabilities();
        assert!(caps.is_path_allowed("/webhook/slack"));
        assert!(!caps.allow_polling);
        assert_eq!(caps.workspace_prefix, "channels/slack/");

        // Check tool capabilities were parsed
        assert!(caps.tool_capabilities.http.is_some());
        assert!(caps.tool_capabilities.secrets.is_some());

        // Check config
        let config_json = file.config_json();
        assert!(config_json.contains("signing_secret_name"));
    }

    #[test]
    fn test_parse_with_polling() {
        let json = r#"{
            "name": "telegram",
            "capabilities": {
                "channel": {
                    "allowed_paths": [],
                    "allow_polling": true,
                    "min_poll_interval_ms": 60000
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();

        assert!(caps.allow_polling);
        assert_eq!(caps.min_poll_interval_ms, 60000);
    }

    #[test]
    fn test_min_poll_interval_enforced() {
        let json = r#"{
            "name": "test",
            "capabilities": {
                "channel": {
                    "allow_polling": true,
                    "min_poll_interval_ms": 1000
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();

        // Should be clamped to minimum
        assert_eq!(caps.min_poll_interval_ms, 30000);
    }

    #[test]
    fn test_workspace_prefix_override() {
        let json = r#"{
            "name": "custom",
            "capabilities": {
                "channel": {
                    "workspace_prefix": "integrations/custom/"
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();

        assert_eq!(caps.workspace_prefix, "integrations/custom/");
    }

    #[test]
    fn test_emit_rate_limit() {
        let json = r#"{
            "name": "test",
            "capabilities": {
                "channel": {
                    "emit_rate_limit": {
                        "messages_per_minute": 50,
                        "messages_per_hour": 1000
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();

        assert_eq!(caps.emit_rate_limit.messages_per_minute, 50);
        assert_eq!(caps.emit_rate_limit.messages_per_hour, 1000);
    }

    #[test]
    fn test_webhook_schema() {
        let json = r#"{
            "name": "telegram",
            "capabilities": {
                "channel": {
                    "allowed_paths": ["/webhook/telegram"],
                    "webhook": {
                        "secret_header": "X-Telegram-Bot-Api-Secret-Token",
                        "secret_name": "telegram_webhook_secret"
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        assert_eq!(
            file.webhook_secret_header(),
            Some("X-Telegram-Bot-Api-Secret-Token")
        );
        assert_eq!(file.webhook_secret_name(), "telegram_webhook_secret");
    }

    #[test]
    fn test_webhook_secret_name_default() {
        let json = r#"{
            "name": "mybot",
            "capabilities": {}
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        assert_eq!(file.webhook_secret_header(), None);
        assert_eq!(file.webhook_secret_name(), "mybot_webhook_secret");
    }

    #[test]
    fn test_parse_connection_websocket_full() {
        let json = r#"{
            "name": "discord",
            "capabilities": {
                "channel": {
                    "allow_polling": true,
                    "connection": {
                        "type": "websocket",
                        "url": "wss://gateway.discord.gg/?v=10&encoding=json",
                        "keepalive": {
                            "type": "json_opcode",
                            "interval_field": "d.heartbeat_interval",
                            "send": { "op": 1, "d": null },
                            "expect": { "op": 11 },
                            "fallback_interval_ms": 41250
                        },
                        "handshake": {
                            "send": {
                                "op": 2,
                                "d": { "token": "{DISCORD_BOT_TOKEN}", "intents": 36864 }
                            },
                            "expect_op": 0
                        },
                        "reconnect": {
                            "max_retries": 5,
                            "backoff_ms": 1000,
                            "resumable": true
                        },
                        "events": {
                            "deliver_to_wasm": ["MESSAGE_CREATE"],
                            "drop": ["PRESENCE_UPDATE", "TYPING_START"],
                            "type_field": "t"
                        },
                        "max_event_size": 65536,
                        "event_queue_size": 100,
                        "additional_allowlist": [
                            { "host": "gateway.discord.gg" }
                        ]
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();

        let conn = caps
            .connection
            .expect("connection config should be present");
        assert!(matches!(
            conn.connection_type,
            crate::channels::wasm::schema::ConnectionType::Websocket
        ));
        assert_eq!(
            conn.url.as_deref(),
            Some("wss://gateway.discord.gg/?v=10&encoding=json")
        );

        // Keepalive
        let ka = conn.keepalive.expect("keepalive should be present");
        assert!(matches!(
            ka.r#type,
            crate::channels::wasm::schema::KeepaliveType::JsonOpcode
        ));
        assert_eq!(ka.interval_field.as_deref(), Some("d.heartbeat_interval"));
        assert_eq!(ka.fallback_interval_ms, 41250);

        // Handshake
        let hs = conn.handshake.expect("handshake should be present");
        assert_eq!(hs.expect_op, Some(0));
        assert!(
            hs.send["d"]["token"]
                .as_str()
                .unwrap()
                .contains("DISCORD_BOT_TOKEN")
        );

        // Reconnect
        assert_eq!(conn.reconnect.max_retries, 5);
        assert_eq!(conn.reconnect.backoff_ms, 1000);
        assert!(conn.reconnect.resumable);

        // Events
        assert_eq!(conn.events.deliver_to_wasm, vec!["MESSAGE_CREATE"]);
        assert_eq!(conn.events.drop, vec!["PRESENCE_UPDATE", "TYPING_START"]);
        assert_eq!(conn.events.type_field, "t");

        // Limits
        assert_eq!(conn.max_event_size, 65536);
        assert_eq!(conn.event_queue_size, 100);

        // Allowlist
        assert_eq!(conn.additional_allowlist.len(), 1);
        assert_eq!(conn.additional_allowlist[0].host, "gateway.discord.gg");
    }

    #[test]
    fn test_parse_connection_minimal() {
        let json = r#"{
            "name": "test",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "websocket",
                        "url": "wss://example.com/ws"
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();

        let conn = caps
            .connection
            .expect("connection config should be present");
        assert!(matches!(
            conn.connection_type,
            crate::channels::wasm::schema::ConnectionType::Websocket
        ));
        assert_eq!(conn.url.as_deref(), Some("wss://example.com/ws"));

        // Verify defaults
        assert!(conn.keepalive.is_none());
        assert!(conn.handshake.is_none());
        assert_eq!(conn.reconnect.max_retries, 5);
        assert_eq!(conn.reconnect.backoff_ms, 1000);
        assert!(!conn.reconnect.resumable);
        assert!(conn.events.deliver_to_wasm.is_empty());
        assert!(conn.events.drop.is_empty());
        assert_eq!(conn.events.type_field, "t");
        assert_eq!(conn.max_event_size, 65536);
        assert_eq!(conn.event_queue_size, 100);
        assert!(conn.additional_allowlist.is_empty());
    }

    #[test]
    fn test_parse_connection_long_poll() {
        let json = r#"{
            "name": "matrix",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "long_poll",
                        "url": "https://matrix.org/_matrix/client/v3/sync"
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        let conn = caps.connection.unwrap();
        assert!(matches!(
            conn.connection_type,
            crate::channels::wasm::schema::ConnectionType::LongPoll
        ));
    }

    #[test]
    fn test_parse_connection_sse() {
        let json = r#"{
            "name": "stream",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "sse",
                        "url": "https://api.example.com/events"
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        let conn = caps.connection.unwrap();
        assert!(matches!(
            conn.connection_type,
            crate::channels::wasm::schema::ConnectionType::Sse
        ));
    }

    #[test]
    fn test_parse_connection_url_from_api() {
        let json = r#"{
            "name": "slack",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "websocket",
                        "url_from_api": {
                            "method": "POST",
                            "endpoint": "https://slack.com/api/apps.connections.open",
                            "url_field": "url"
                        }
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        let conn = caps.connection.unwrap();
        assert!(conn.url.is_none());
        let api = conn.url_from_api.unwrap();
        assert_eq!(api.method, "POST");
        assert_eq!(api.endpoint, "https://slack.com/api/apps.connections.open");
        assert_eq!(api.url_field, "url");
    }

    #[test]
    fn test_parse_keepalive_ping_pong() {
        let json = r#"{
            "name": "test",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "websocket",
                        "url": "wss://example.com/ws",
                        "keepalive": { "type": "ping_pong" }
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        let ka = caps.connection.unwrap().keepalive.unwrap();
        assert!(matches!(
            ka.r#type,
            crate::channels::wasm::schema::KeepaliveType::PingPong
        ));
        assert_eq!(ka.fallback_interval_ms, 30000);
    }

    #[test]
    fn test_parse_keepalive_none() {
        let json = r#"{
            "name": "test",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "websocket",
                        "url": "wss://example.com/ws",
                        "keepalive": { "type": "none" }
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        let ka = caps.connection.unwrap().keepalive.unwrap();
        assert!(matches!(
            ka.r#type,
            crate::channels::wasm::schema::KeepaliveType::None
        ));
    }

    #[test]
    fn test_parse_handshake_without_expect_op() {
        let json = r#"{
            "name": "test",
            "capabilities": {
                "channel": {
                    "connection": {
                        "type": "websocket",
                        "url": "wss://example.com/ws",
                        "handshake": {
                            "send": { "type": "hello", "token": "{TOKEN}" }
                        }
                    }
                }
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        let hs = caps.connection.unwrap().handshake.unwrap();
        assert!(hs.expect_op.is_none());
        assert_eq!(hs.send["type"].as_str(), Some("hello"));
    }

    #[test]
    fn test_no_connection_by_default() {
        let json = r#"{ "name": "test" }"#;
        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        let caps = file.to_capabilities();
        assert!(caps.connection.is_none());
    }

    #[test]
    fn test_setup_schema() {
        let json = r#"{
            "name": "telegram",
            "setup": {
                "required_secrets": [
                    {
                        "name": "telegram_bot_token",
                        "prompt": "Enter your Telegram Bot Token",
                        "validation": "^[0-9]+:[A-Za-z0-9_-]+$"
                    },
                    {
                        "name": "telegram_webhook_secret",
                        "prompt": "Webhook secret (leave empty to auto-generate)",
                        "optional": true,
                        "auto_generate": { "length": 64 }
                    }
                ],
                "validation_endpoint": "https://api.telegram.org/bot{telegram_bot_token}/getMe"
            }
        }"#;

        let file = ChannelCapabilitiesFile::from_json(json).unwrap();
        assert_eq!(file.setup.required_secrets.len(), 2);
        assert_eq!(file.setup.required_secrets[0].name, "telegram_bot_token");
        assert!(!file.setup.required_secrets[0].optional);
        assert!(file.setup.required_secrets[1].optional);
        assert_eq!(
            file.setup.required_secrets[1]
                .auto_generate
                .as_ref()
                .unwrap()
                .length,
            64
        );
    }
}
