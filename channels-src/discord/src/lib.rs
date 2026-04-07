// Discord API types have fields that may be added in future API versions
#![allow(dead_code)]

//! Discord DM channel for RustyTalon.
//!
//! This WASM component implements the channel interface for chatting with the
//! agent via Discord Direct Messages.
//!
//! # How it works
//!
//! Since Discord's Gateway (WebSocket) is not available in WASM, this channel
//! uses REST polling:
//!
//! 1. On startup (`on_start`), if `owner_id` is configured the bot opens a DM
//!    channel with that user via `POST /users/@me/channels` and stores the
//!    channel ID in workspace state.
//! 2. On each poll tick (`on_poll`, default 30 s) the bot fetches new messages
//!    from every known DM channel using
//!    `GET /channels/{id}/messages?after={snowflake}&limit=50`.
//! 3. Messages from the bot itself are silently dropped; other messages are
//!    checked against the DM policy and emitted to the agent.
//! 4. When the agent replies (`on_respond`), the response is posted back to the
//!    same DM channel via `POST /channels/{id}/messages`.
//! 5. While the agent is thinking (`on_status`), a typing indicator is sent via
//!    `POST /channels/{id}/typing`.
//!
//! # Security
//!
//! - The bot token is injected by the host as `Authorization: Bot <token>`.
//!   WASM never sees the raw credential.
//! - Unknown senders are gated by `dm_policy` (pairing / open).

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "sandboxed-channel",
    path: "../../wit/channel.wit",
});

use serde::{Deserialize, Serialize};

use exports::near::agent::channel::{
    AgentResponse, ChannelConfig, Guest, IncomingHttpRequest,
    OutgoingHttpResponse, PollConfig, StatusType, StatusUpdate,
};
use near::agent::channel_host::{self, EmittedMessage};

// ============================================================================
// Discord REST API types
// ============================================================================

/// Partial Discord User object.
/// https://discord.com/developers/docs/resources/user#user-object
#[derive(Debug, Deserialize)]
struct DiscordUser {
    /// Snowflake user ID (as string).
    id: String,

    /// Username.
    username: String,

    /// Global display name (may differ from username).
    global_name: Option<String>,

    /// True if this user is a bot account.
    #[serde(default)]
    bot: bool,
}

/// Partial Discord Message object.
/// https://discord.com/developers/docs/resources/message#message-object
#[derive(Debug, Deserialize)]
struct DiscordMessage {
    /// Snowflake message ID.
    id: String,

    /// Channel the message was sent in.
    channel_id: String,

    /// Message author.
    author: DiscordUser,

    /// Plain-text message content.
    content: String,
}

/// Partial Discord DM Channel object (type 1 = DM).
/// https://discord.com/developers/docs/resources/channel#channel-object
#[derive(Debug, Deserialize)]
struct DiscordDmChannel {
    /// Snowflake channel ID.
    id: String,

    /// Channel type. 1 = DM.
    #[serde(rename = "type")]
    channel_type: u8,
}

/// Wrapper returned by `POST /users/@me/channels`.
#[derive(Debug, Deserialize)]
struct CreateDmResponse {
    id: String,
}

/// Bot's own user object returned by `GET /users/@me`.
#[derive(Debug, Deserialize)]
struct BotUser {
    id: String,
}

// ============================================================================
// Workspace state paths
// ============================================================================

/// Bot's own Discord user ID — used to filter out self-messages.
const BOT_ID_PATH: &str = "state/bot_id";

/// JSON map of `{ channel_id: last_snowflake_str }` — tracks poll position per channel.
const LAST_MESSAGE_IDS_PATH: &str = "state/last_message_ids";

/// JSON array of DM channel IDs the bot should poll.
const DM_CHANNELS_PATH: &str = "state/dm_channels";

/// DM policy persisted across callbacks: "owner_only" | "pairing" | "open".
const DM_POLICY_PATH: &str = "state/dm_policy";

/// Bot owner's Discord user ID — used by the "owner_only" policy.
const OWNER_ID_PATH: &str = "state/owner_id";

/// JSON array of allowed user IDs (from config `allow_from`).
const ALLOW_FROM_PATH: &str = "state/allow_from";

/// Channel name used by the pairing store host API.
const CHANNEL_NAME: &str = "discord";

// ============================================================================
// Channel metadata (attached to each emitted message for routing responses)
// ============================================================================

/// Routing information needed to post a response back to the correct DM.
#[derive(Debug, Serialize, Deserialize)]
struct DiscordMessageMetadata {
    /// Channel ID to post the reply in.
    channel_id: String,

    /// Snowflake of the user's message (used for optional reply references).
    message_id: String,

    /// Discord user ID of the sender.
    user_id: String,
}

// ============================================================================
// Channel configuration (from capabilities.json `config` block)
// ============================================================================

/// Per-channel configuration injected by the host from the capabilities file.
#[derive(Debug, Deserialize)]
struct DiscordConfig {
    /// Discord user ID of the bot owner.
    ///
    /// When set, the bot opens a DM channel with this user on startup and polls it.
    /// Right-click a username in Discord → "Copy User ID" (Developer Mode must be on).
    #[serde(default)]
    owner_id: Option<String>,

    /// DM policy: `"owner_only"` (default), `"pairing"`, or `"open"`.
    ///
    /// - `owner_only`: only the configured `owner_id` can message the bot (most secure).
    /// - `pairing`: unknown senders receive a pairing-code reply; messages are
    ///   not forwarded until approved via `rustytalon pairing approve discord <code>`.
    /// - `open`: all senders are accepted without pairing.
    #[serde(default)]
    dm_policy: Option<String>,

    /// Extra user IDs (Discord snowflakes or usernames) to allow without pairing.
    #[serde(default)]
    allow_from: Option<Vec<String>>,
}

// ============================================================================
// Channel implementation
// ============================================================================

struct DiscordChannel;

impl Guest for DiscordChannel {
    fn on_start(config_json: String) -> Result<ChannelConfig, String> {
        channel_host::log(
            channel_host::LogLevel::Debug,
            &format!("Discord channel config: {}", config_json),
        );

        let config: DiscordConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse config: {}", e))?;

        channel_host::log(channel_host::LogLevel::Info, "Discord channel starting");

        // Persist dm_policy and allow_from for subsequent poll callbacks
        let dm_policy = config
            .dm_policy
            .as_deref()
            .unwrap_or("owner_only")
            .to_string();
        let _ = channel_host::workspace_write(DM_POLICY_PATH, &dm_policy);

        // Persist owner_id so handle_message can access it for owner_only policy
        if let Some(ref owner_id) = config.owner_id {
            let _ = channel_host::workspace_write(OWNER_ID_PATH, owner_id);
        }

        let allow_from_json = serde_json::to_string(&config.allow_from.unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_string());
        let _ = channel_host::workspace_write(ALLOW_FROM_PATH, &allow_from_json);

        // Fetch and store the bot's own user ID so we can filter self-messages
        if let Err(e) = fetch_and_store_bot_id() {
            channel_host::log(
                channel_host::LogLevel::Warn,
                &format!(
                    "Could not fetch bot user ID (self-message filtering disabled): {}",
                    e
                ),
            );
        }

        // If owner_id is set, open (or retrieve) the DM channel with that user
        if let Some(ref owner_id) = config.owner_id {
            channel_host::log(
                channel_host::LogLevel::Info,
                &format!("Opening DM channel with owner user {}", owner_id),
            );
            match open_dm_channel(owner_id) {
                Ok(channel_id) => {
                    channel_host::log(
                        channel_host::LogLevel::Info,
                        &format!("DM channel with owner: {}", channel_id),
                    );
                    add_dm_channel(&channel_id);

                    // Seed last_message_ids with the current latest snowflake so
                    // that on_poll only delivers messages that arrive AFTER startup.
                    // Without this, every restart would replay the last 50 messages.
                    seed_last_message_id(&channel_id);
                }
                Err(e) => {
                    channel_host::log(
                        channel_host::LogLevel::Error,
                        &format!(
                            "Failed to open DM channel with owner {}: {}",
                            owner_id, e
                        ),
                    );
                }
            }
        } else {
            channel_host::log(
                channel_host::LogLevel::Warn,
                "No owner_id configured. Set config.owner_id to your Discord user ID \
                 to receive DMs. Get it by right-clicking your name in Discord \
                 (Developer Mode must be enabled).",
            );
        }

        Ok(ChannelConfig {
            display_name: "Discord".to_string(),
            http_endpoints: vec![],
            poll: Some(PollConfig {
                interval_ms: 30_000,
                enabled: true,
            }),
        })
    }

    fn on_http_request(_req: IncomingHttpRequest) -> OutgoingHttpResponse {
        // This channel registers no HTTP endpoints, so this should never be called.
        OutgoingHttpResponse {
            status: 404,
            headers_json: r#"{"Content-Type":"application/json"}"#.to_string(),
            body: br#"{"error":"not found"}"#.to_vec(),
        }
    }

    fn on_poll() {
        let dm_channels = load_dm_channels();

        if dm_channels.is_empty() {
            channel_host::log(
                channel_host::LogLevel::Debug,
                "No DM channels configured yet — skipping poll",
            );
            return;
        }

        // Load last-seen snowflake per channel
        let mut last_ids: std::collections::HashMap<String, String> =
            channel_host::workspace_read(LAST_MESSAGE_IDS_PATH)
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

        let bot_id = channel_host::workspace_read(BOT_ID_PATH).unwrap_or_default();

        for channel_id in &dm_channels {
            let after = last_ids.get(channel_id).cloned().unwrap_or_default();

            match fetch_messages(channel_id, &after) {
                Ok(messages) => {
                    let mut newest = after.clone();

                    for msg in messages {
                        // Skip messages from the bot itself
                        if !bot_id.is_empty() && msg.author.id == bot_id {
                            continue;
                        }
                        // Skip other bots
                        if msg.author.bot {
                            continue;
                        }
                        // Skip empty messages
                        if msg.content.trim().is_empty() {
                            continue;
                        }

                        // Track the newest snowflake seen
                        if snowflake_gt(&msg.id, &newest) {
                            newest = msg.id.clone();
                        }

                        handle_message(&msg, channel_id);
                    }

                    if newest != after {
                        last_ids.insert(channel_id.clone(), newest);
                    }
                }
                Err(e) => {
                    channel_host::log(
                        channel_host::LogLevel::Error,
                        &format!("Failed to fetch messages for channel {}: {}", channel_id, e),
                    );
                }
            }
        }

        // Persist updated last_ids
        if let Ok(json) = serde_json::to_string(&last_ids) {
            let _ = channel_host::workspace_write(LAST_MESSAGE_IDS_PATH, &json);
        }
    }

    fn on_respond(response: AgentResponse) -> Result<(), String> {
        let metadata: DiscordMessageMetadata = serde_json::from_str(&response.metadata_json)
            .map_err(|e| format!("Failed to parse metadata: {}", e))?;

        send_message(&metadata.channel_id, &response.content)
    }

    fn on_status(update: StatusUpdate) {
        // Only send typing indicator while the agent is thinking
        if !matches!(update.status, StatusType::Thinking) {
            return;
        }

        let metadata: DiscordMessageMetadata =
            match serde_json::from_str(&update.metadata_json) {
                Ok(m) => m,
                Err(_) => {
                    channel_host::log(
                        channel_host::LogLevel::Debug,
                        "on_status: no valid Discord metadata, skipping typing indicator",
                    );
                    return;
                }
            };

        let url = format!(
            "https://discord.com/api/v10/channels/{}/typing",
            metadata.channel_id
        );

        let headers = serde_json::json!({ "Content-Type": "application/json" });

        // POST with empty body — triggers the "Bot is typing…" indicator
        if let Err(e) = channel_host::http_request(
            "POST",
            &url,
            &headers.to_string(),
            Some(&[]),
            None,
        ) {
            channel_host::log(
                channel_host::LogLevel::Debug,
                &format!("Typing indicator failed: {}", e),
            );
        }
    }

    fn on_event(event_json: String) -> Result<(), String> {
        // Parse the raw gateway event (already filtered to MESSAGE_CREATE by the broker)
        let event: serde_json::Value =
            serde_json::from_str(&event_json).map_err(|e| format!("Invalid event JSON: {}", e))?;

        // Extract the message payload from "d"
        let d = event.get("d").ok_or("Missing 'd' field in gateway event")?;
        let msg: DiscordMessage =
            serde_json::from_value(d.clone()).map_err(|e| format!("Failed to parse message: {}", e))?;

        let bot_id = channel_host::workspace_read(BOT_ID_PATH).unwrap_or_default();

        // Skip messages from the bot itself
        if !bot_id.is_empty() && msg.author.id == bot_id {
            return Ok(());
        }
        // Skip other bots
        if msg.author.bot {
            return Ok(());
        }
        // Skip empty messages
        if msg.content.trim().is_empty() {
            return Ok(());
        }

        // Ensure this DM channel is tracked for future polling/responses
        add_dm_channel(&msg.channel_id);

        let channel_id = msg.channel_id.clone();
        handle_message(&msg, &channel_id);

        Ok(())
    }

    fn on_shutdown() {
        channel_host::log(
            channel_host::LogLevel::Info,
            "Discord channel shutting down",
        );
    }
}

// ============================================================================
// Message handling
// ============================================================================

/// Check message access policy and emit to the agent if allowed.
fn handle_message(msg: &DiscordMessage, channel_id: &str) {
    let dm_policy = channel_host::workspace_read(DM_POLICY_PATH)
        .unwrap_or_else(|| "owner_only".to_string());

    if dm_policy == "owner_only" {
        // Only the configured owner may send messages. Drop everything else silently.
        let owner_id = channel_host::workspace_read(OWNER_ID_PATH).unwrap_or_default();
        if owner_id.is_empty() || msg.author.id != owner_id {
            channel_host::log(
                channel_host::LogLevel::Debug,
                &format!(
                    "owner_only: dropping message from non-owner user {}",
                    msg.author.id
                ),
            );
            return;
        }
    } else if dm_policy != "open" {
        // Build effective allow list: config allow_from + pairing-approved store
        let mut allowed: Vec<String> = channel_host::workspace_read(ALLOW_FROM_PATH)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if let Ok(store_allowed) = channel_host::pairing_read_allow_from(CHANNEL_NAME) {
            allowed.extend(store_allowed);
        }

        let is_allowed = allowed.contains(&"*".to_string())
            || allowed.contains(&msg.author.id);

        if !is_allowed {
            if dm_policy == "pairing" {
                let meta = serde_json::json!({
                    "channel_id": channel_id,
                    "user_id": msg.author.id,
                    "username": msg.author.username,
                })
                .to_string();

                match channel_host::pairing_upsert_request(
                    CHANNEL_NAME,
                    &msg.author.id,
                    &meta,
                ) {
                    Ok(result) => {
                        channel_host::log(
                            channel_host::LogLevel::Info,
                            &format!(
                                "Pairing request for user {} (channel {}): code {}",
                                msg.author.id, channel_id, result.code
                            ),
                        );
                        if result.created {
                            let pairing_text = format!(
                                "To pair with this bot, run: `rustytalon pairing approve discord {}`",
                                result.code
                            );
                            if let Err(e) = send_message(channel_id, &pairing_text) {
                                channel_host::log(
                                    channel_host::LogLevel::Error,
                                    &format!("Failed to send pairing reply: {}", e),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        channel_host::log(
                            channel_host::LogLevel::Error,
                            &format!("Pairing upsert failed: {}", e),
                        );
                    }
                }
            }
            return;
        }
    }

    // Build user display name
    let user_name = msg
        .author
        .global_name
        .as_deref()
        .unwrap_or(&msg.author.username)
        .to_string();

    let metadata = DiscordMessageMetadata {
        channel_id: channel_id.to_string(),
        message_id: msg.id.clone(),
        user_id: msg.author.id.clone(),
    };

    let metadata_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());

    channel_host::emit_message(&EmittedMessage {
        user_id: msg.author.id.clone(),
        user_name: Some(user_name),
        content: msg.content.clone(),
        thread_id: None,
        metadata_json,
    });

    channel_host::log(
        channel_host::LogLevel::Debug,
        &format!(
            "Emitted message from user {} in channel {}",
            msg.author.id, channel_id
        ),
    );
}

// ============================================================================
// Discord REST API helpers
// ============================================================================

/// Fetch the bot's own user ID and persist it for self-message filtering.
fn fetch_and_store_bot_id() -> Result<(), String> {
    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let response = channel_host::http_request(
        "GET",
        "https://discord.com/api/v10/users/@me",
        &headers.to_string(),
        None,
        None,
    )
    .map_err(|e| format!("HTTP request failed: {}", e))?;

    if response.status != 200 {
        let body = String::from_utf8_lossy(&response.body);
        return Err(format!("Discord returned {}: {}", response.status, body));
    }

    let user: BotUser = serde_json::from_slice(&response.body)
        .map_err(|e| format!("Failed to parse bot user: {}", e))?;

    let _ = channel_host::workspace_write(BOT_ID_PATH, &user.id);

    channel_host::log(
        channel_host::LogLevel::Info,
        &format!("Bot user ID: {}", user.id),
    );

    Ok(())
}

/// Open (or retrieve) a DM channel with a Discord user.
///
/// Discord is idempotent here — calling this with the same recipient always
/// returns the same DM channel.
fn open_dm_channel(recipient_id: &str) -> Result<String, String> {
    let body = serde_json::json!({ "recipient_id": recipient_id });
    let body_bytes =
        serde_json::to_vec(&body).map_err(|e| format!("Serialization failed: {}", e))?;

    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let response = channel_host::http_request(
        "POST",
        "https://discord.com/api/v10/users/@me/channels",
        &headers.to_string(),
        Some(&body_bytes),
        None,
    )
    .map_err(|e| format!("HTTP request failed: {}", e))?;

    if response.status != 200 {
        let body_str = String::from_utf8_lossy(&response.body);
        return Err(format!(
            "Discord returned {} while opening DM: {}",
            response.status, body_str
        ));
    }

    let channel: CreateDmResponse = serde_json::from_slice(&response.body)
        .map_err(|e| format!("Failed to parse DM channel response: {}", e))?;

    Ok(channel.id)
}

/// Fetch up to 50 messages from a channel after a given snowflake.
///
/// If `after` is empty, returns the most recent 50 messages.
fn fetch_messages(channel_id: &str, after: &str) -> Result<Vec<DiscordMessage>, String> {
    let url = if after.is_empty() {
        format!(
            "https://discord.com/api/v10/channels/{}/messages?limit=50",
            channel_id
        )
    } else {
        format!(
            "https://discord.com/api/v10/channels/{}/messages?after={}&limit=50",
            channel_id, after
        )
    };

    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let response = channel_host::http_request("GET", &url, &headers.to_string(), None, None)
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if response.status == 403 {
        return Err(format!(
            "Bot lacks access to channel {} (403 Forbidden)",
            channel_id
        ));
    }

    if response.status != 200 {
        let body_str = String::from_utf8_lossy(&response.body);
        return Err(format!(
            "Discord returned {} for channel {}: {}",
            response.status, channel_id, body_str
        ));
    }

    let messages: Vec<DiscordMessage> = serde_json::from_slice(&response.body)
        .map_err(|e| format!("Failed to parse messages: {}", e))?;

    // Discord returns messages newest-first; reverse to process oldest-first
    let mut messages = messages;
    messages.reverse();
    Ok(messages)
}

/// Seed `last_message_ids` for a channel with the current latest snowflake.
///
/// Called once on `on_start` after opening the DM channel. This ensures that
/// `on_poll` only processes messages that arrive after the bot starts — without
/// this, every restart would replay the last 50 messages and reply to all of them.
///
/// If the channel is empty or the fetch fails, we skip seeding (the first poll
/// will start from the beginning, which is acceptable for a fresh channel).
fn seed_last_message_id(channel_id: &str) {
    // Load existing map so we don't overwrite a position we already have
    // (e.g. on_event may have updated it before on_start finishes).
    let mut last_ids: std::collections::HashMap<String, String> =
        channel_host::workspace_read(LAST_MESSAGE_IDS_PATH)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

    // If we already have a position for this channel, leave it alone.
    if last_ids.contains_key(channel_id) {
        return;
    }

    // Fetch the single most recent message to get its snowflake.
    let url = format!(
        "https://discord.com/api/v10/channels/{}/messages?limit=1",
        channel_id
    );
    let headers = serde_json::json!({ "Content-Type": "application/json" });

    match channel_host::http_request("GET", &url, &headers.to_string(), None, None) {
        Ok(resp) if resp.status == 200 => {
            if let Ok(msgs) = serde_json::from_slice::<Vec<serde_json::Value>>(&resp.body) {
                if let Some(latest_id) = msgs.first().and_then(|m| m["id"].as_str()) {
                    channel_host::log(
                        channel_host::LogLevel::Info,
                        &format!(
                            "Seeding last_message_id for channel {} to {}",
                            channel_id, latest_id
                        ),
                    );
                    last_ids.insert(channel_id.to_string(), latest_id.to_string());
                    if let Ok(json) = serde_json::to_string(&last_ids) {
                        let _ = channel_host::workspace_write(LAST_MESSAGE_IDS_PATH, &json);
                    }
                }
                // Empty channel — no messages yet, leave unset (on_poll will fetch from beginning)
            }
        }
        Ok(resp) => {
            channel_host::log(
                channel_host::LogLevel::Warn,
                &format!(
                    "Could not seed last_message_id for channel {} (status {}), will fetch from beginning on first poll",
                    channel_id, resp.status
                ),
            );
        }
        Err(e) => {
            channel_host::log(
                channel_host::LogLevel::Warn,
                &format!(
                    "Could not seed last_message_id for channel {} ({}), will fetch from beginning on first poll",
                    channel_id, e
                ),
            );
        }
    }
}

/// Post a text message to a Discord channel.
fn send_message(channel_id: &str, content: &str) -> Result<(), String> {
    // Discord limits messages to 2000 characters
    let content = if content.len() > 2000 {
        &content[..2000]
    } else {
        content
    };

    let body = serde_json::json!({ "content": content });
    let body_bytes =
        serde_json::to_vec(&body).map_err(|e| format!("Serialization failed: {}", e))?;

    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let url = format!(
        "https://discord.com/api/v10/channels/{}/messages",
        channel_id
    );

    let response = channel_host::http_request(
        "POST",
        &url,
        &headers.to_string(),
        Some(&body_bytes),
        None,
    )
    .map_err(|e| format!("HTTP request failed: {}", e))?;

    if response.status != 200 && response.status != 201 {
        let body_str = String::from_utf8_lossy(&response.body);
        return Err(format!(
            "Discord returned {} while sending message: {}",
            response.status, body_str
        ));
    }

    channel_host::log(
        channel_host::LogLevel::Debug,
        &format!("Sent message to Discord channel {}", channel_id),
    );

    Ok(())
}

// ============================================================================
// Workspace state helpers
// ============================================================================

/// Load the list of DM channel IDs from workspace state.
fn load_dm_channels() -> Vec<String> {
    channel_host::workspace_read(DM_CHANNELS_PATH)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Add a DM channel ID to the persisted list (deduplicated).
fn add_dm_channel(channel_id: &str) {
    let mut channels = load_dm_channels();
    if !channels.contains(&channel_id.to_string()) {
        channels.push(channel_id.to_string());
        if let Ok(json) = serde_json::to_string(&channels) {
            let _ = channel_host::workspace_write(DM_CHANNELS_PATH, &json);
        }
    }
}

// ============================================================================
// Snowflake comparison
// ============================================================================

/// Returns true if snowflake `a` is greater than snowflake `b`.
///
/// Discord snowflakes are 64-bit integers encoded as decimal strings.
/// Comparing by decimal string length first, then lexicographically, is
/// correct for fixed-width integers of the same magnitude.
fn snowflake_gt(a: &str, b: &str) -> bool {
    if b.is_empty() {
        return !a.is_empty();
    }
    if a.len() != b.len() {
        return a.len() > b.len();
    }
    a > b
}

// Export the component
export!(DiscordChannel);

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snowflake_gt_empty() {
        assert!(snowflake_gt("1234567890", ""));
        assert!(!snowflake_gt("", ""));
        assert!(!snowflake_gt("", "1234567890"));
    }

    #[test]
    fn test_snowflake_gt_same_length() {
        assert!(snowflake_gt("1234567891", "1234567890"));
        assert!(!snowflake_gt("1234567890", "1234567891"));
        assert!(!snowflake_gt("1234567890", "1234567890"));
    }

    #[test]
    fn test_snowflake_gt_different_length() {
        // Longer decimal string = larger number
        assert!(snowflake_gt("12345678901", "1234567890"));
        assert!(!snowflake_gt("1234567890", "12345678901"));
    }

    #[test]
    fn test_snowflake_gt_realistic() {
        // Real Discord snowflakes are 18-19 digits
        let older = "1180000000000000000";
        let newer = "1180000000000000001";
        assert!(snowflake_gt(newer, older));
        assert!(!snowflake_gt(older, newer));
    }
}
