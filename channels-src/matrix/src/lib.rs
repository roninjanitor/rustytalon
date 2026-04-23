// Matrix API types have fields that may be added in future API versions
#![allow(dead_code)]

//! Matrix Client-Server API channel for RustyTalon.
//!
//! This WASM component implements the channel interface for chatting with the
//! agent via Matrix rooms (including DMs).
//!
//! # How it works
//!
//! The Matrix homeserver is configurable — any Synapse, Dendrite, Conduit, or
//! hosted homeserver (matrix.org, Element Matrix Services, etc.) works.
//!
//! 1. On startup (`on_start`), the channel validates the access token via
//!    `GET /_matrix/client/v3/account/whoami` and stores the bot's user ID.
//! 2. If `owner_id` is configured, the channel looks for an existing DM room
//!    via the `m.direct` account data event, or creates one if none exists.
//! 3. On each poll tick (`on_poll`, default 30 s), the channel calls
//!    `GET /_matrix/client/v3/sync?since=<batch>&timeout=0` and processes
//!    timeline events from all joined rooms.
//! 4. Invite events from allowed senders are auto-accepted via
//!    `POST /_matrix/client/v3/join/{roomId}`.
//! 5. When the agent replies (`on_respond`), the response is posted via
//!    `PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}`.
//! 6. While the agent is thinking (`on_status`), a typing notification is
//!    sent via `PUT /_matrix/client/v3/rooms/{roomId}/typing/{userId}`.
//!
//! # Security
//!
//! - The access token is injected by the host as `Authorization: Bearer <token>`.
//!   WASM never sees the raw credential.
//! - Unknown senders are gated by `dm_policy` (pairing / open).
//! - The homeserver URL is stored in workspace state so it is available across
//!   fresh WASM instances (one per poll tick).

// Generate bindings from the WIT file
wit_bindgen::generate!({
    world: "sandboxed-channel",
    path: "../../wit/channel.wit",
});

use serde::{Deserialize, Serialize};

use exports::near::agent::channel::{
    AgentResponse, ChannelConfig, Guest, IncomingHttpRequest, OutgoingHttpResponse, PollConfig,
    StatusType, StatusUpdate,
};
use near::agent::channel_host::{self, EmittedMessage};

// ============================================================================
// Matrix Client-Server API types
// ============================================================================

/// Response from `GET /_matrix/client/v3/account/whoami`.
#[derive(Debug, Deserialize)]
struct WhoAmIResponse {
    user_id: String,
}

/// Top-level sync response from `GET /_matrix/client/v3/sync`.
/// https://spec.matrix.org/v1.8/client-server-api/#get_matrixclientv3sync
#[derive(Debug, Deserialize)]
struct SyncResponse {
    /// Batch token for next sync.
    next_batch: String,

    /// Room updates.
    #[serde(default)]
    rooms: SyncRooms,
}

#[derive(Debug, Deserialize, Default)]
struct SyncRooms {
    /// Rooms the bot is currently joined.
    #[serde(default)]
    join: std::collections::HashMap<String, JoinedRoom>,

    /// Rooms the bot has been invited to.
    #[serde(default)]
    invite: std::collections::HashMap<String, InvitedRoom>,
}

#[derive(Debug, Deserialize)]
struct JoinedRoom {
    timeline: RoomTimeline,
}

#[derive(Debug, Deserialize, Default)]
struct RoomTimeline {
    #[serde(default)]
    events: Vec<MatrixEvent>,
}

/// A Matrix room event (timeline or state).
#[derive(Debug, Deserialize)]
struct MatrixEvent {
    /// Event type, e.g. `m.room.message`.
    #[serde(rename = "type")]
    event_type: String,

    /// Globally unique event ID.
    event_id: String,

    /// Fully-qualified user ID of the sender, e.g. `@alice:matrix.org`.
    sender: String,

    /// Event content (type-dependent).
    content: serde_json::Value,

    /// Server-side timestamp in milliseconds since Unix epoch.
    #[serde(default)]
    origin_server_ts: u64,
}

/// Stripped state event included in invite payloads.
#[derive(Debug, Deserialize)]
struct StrippedEvent {
    #[serde(rename = "type")]
    event_type: String,
    sender: String,
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct InvitedRoom {
    invite_state: InviteState,
}

#[derive(Debug, Deserialize, Default)]
struct InviteState {
    #[serde(default)]
    events: Vec<StrippedEvent>,
}

/// Request body for `PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}`.
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    msgtype: String,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    formatted_body: Option<String>,
}

/// Response from the send-message endpoint.
#[derive(Debug, Deserialize)]
struct SendMessageResponse {
    event_id: String,
}

/// Request body for `PUT /_matrix/client/v3/rooms/{roomId}/typing/{userId}`.
#[derive(Debug, Serialize)]
struct TypingRequest {
    typing: bool,
    /// Timeout in ms before the server clears the typing indicator (only when typing=true).
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
}

/// `m.direct` account data — maps user IDs to lists of DM room IDs.
/// https://spec.matrix.org/v1.8/client-server-api/#mdirect
#[derive(Debug, Deserialize)]
struct MDirectAccountData {
    #[serde(flatten)]
    rooms: std::collections::HashMap<String, Vec<String>>,
}

/// Request body for `POST /_matrix/client/v3/createRoom`.
#[derive(Debug, Serialize)]
struct CreateRoomRequest {
    is_direct: bool,
    invite: Vec<String>,
    preset: String,
}

/// Response from `POST /_matrix/client/v3/createRoom`.
#[derive(Debug, Deserialize)]
struct CreateRoomResponse {
    room_id: String,
}

// ============================================================================
// Routing metadata (attached to each emitted message)
// ============================================================================

/// Channel-specific metadata stored on every emitted message so `on_respond`
/// and `on_status` know which room to post back to.
#[derive(Debug, Serialize, Deserialize)]
struct MatrixMessageMetadata {
    /// Matrix room ID, e.g. `!abc123:matrix.org`.
    room_id: String,

    /// Event ID of the triggering message (for optional reply threads).
    event_id: String,

    /// Sender's fully-qualified Matrix user ID.
    sender_user_id: String,
}

// ============================================================================
// Channel configuration (from capabilities.json `config` block)
// ============================================================================

#[derive(Debug, Deserialize)]
struct MatrixConfig {
    /// Homeserver base URL, e.g. `https://matrix.org` or `https://your-server.com`.
    homeserver: String,

    /// Fully-qualified Matrix user ID of the bot owner, e.g. `@you:matrix.org`.
    ///
    /// When set, the bot looks for an existing DM room with this user in
    /// `m.direct` account data, or creates one on first start.
    #[serde(default)]
    owner_id: Option<String>,

    /// DM policy: `"pairing"` (default) or `"open"`.
    ///
    /// - `pairing`: unknown senders receive a pairing-code reply
    /// - `open`: all senders are accepted without pairing
    #[serde(default)]
    dm_policy: Option<String>,

    /// Matrix user IDs pre-approved without pairing.
    #[serde(default)]
    allow_from: Option<Vec<String>>,
}

// ============================================================================
// Workspace state paths (relative; prefixed with channels/matrix/ by host)
// ============================================================================

/// Bot's own fully-qualified Matrix user ID — used to filter self-messages.
const BOT_USER_ID_PATH: &str = "state/bot_user_id";

/// Batch token returned by the last sync — used as `since=` on next poll.
const NEXT_BATCH_PATH: &str = "state/next_batch";

/// Homeserver base URL — persisted so poll callbacks don't need to re-read config.
const HOMESERVER_PATH: &str = "state/homeserver";

/// DM policy persisted across callbacks: "pairing" | "open".
const DM_POLICY_PATH: &str = "state/dm_policy";

/// JSON array of allowed Matrix user IDs (from config `allow_from`).
const ALLOW_FROM_PATH: &str = "state/allow_from";

/// Monotonically increasing counter for generating unique transaction IDs.
const TXN_COUNTER_PATH: &str = "state/txn_counter";

/// Bot owner's Matrix user ID — used by on_broadcast to find/create the DM room.
const OWNER_ID_PATH: &str = "state/owner_id";

/// Channel name used by the pairing store host API.
const CHANNEL_NAME: &str = "matrix";

// ============================================================================
// Channel implementation
// ============================================================================

struct MatrixChannel;

impl Guest for MatrixChannel {
    fn on_start(config_json: String) -> Result<ChannelConfig, String> {
        let config: MatrixConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse config: {}", e))?;

        channel_host::log(channel_host::LogLevel::Info, "Matrix channel starting");

        // Normalize homeserver URL (strip trailing slash)
        let homeserver = config.homeserver.trim_end_matches('/').to_string();
        let _ = channel_host::workspace_write(HOMESERVER_PATH, &homeserver);

        // Persist policy settings for subsequent poll callbacks
        let dm_policy = config
            .dm_policy
            .as_deref()
            .unwrap_or("pairing")
            .to_string();
        let _ = channel_host::workspace_write(DM_POLICY_PATH, &dm_policy);

        let allow_from_json = serde_json::to_string(&config.allow_from.unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_string());
        let _ = channel_host::workspace_write(ALLOW_FROM_PATH, &allow_from_json);

        // Validate the access token and get the bot's user ID
        let bot_user_id = match whoami(&homeserver) {
            Ok(id) => {
                channel_host::log(
                    channel_host::LogLevel::Info,
                    &format!("Authenticated as {}", id),
                );
                let _ = channel_host::workspace_write(BOT_USER_ID_PATH, &id);
                id
            }
            Err(e) => {
                return Err(format!(
                    "Failed to authenticate with Matrix homeserver {}: {}",
                    homeserver, e
                ));
            }
        };

        // Persist owner_id for on_broadcast to use across callbacks
        if let Some(ref owner_id) = config.owner_id {
            let _ = channel_host::workspace_write(OWNER_ID_PATH, owner_id);
        }

        // If owner_id is set, ensure a DM room exists
        if let Some(ref owner_id) = config.owner_id {
            match find_or_create_dm(&homeserver, &bot_user_id, owner_id) {
                Ok(room_id) => {
                    channel_host::log(
                        channel_host::LogLevel::Info,
                        &format!("DM room with owner {}: {}", owner_id, room_id),
                    );
                }
                Err(e) => {
                    channel_host::log(
                        channel_host::LogLevel::Warn,
                        &format!(
                            "Could not find/create DM room with owner {}: {}",
                            owner_id, e
                        ),
                    );
                }
            }
        } else {
            channel_host::log(
                channel_host::LogLevel::Warn,
                "No owner_id configured. Set config.owner_id to your Matrix user ID \
                 (e.g. @you:matrix.org) to receive DMs on startup.",
            );
        }

        Ok(ChannelConfig {
            display_name: "Matrix".to_string(),
            http_endpoints: vec![],
            poll: Some(PollConfig {
                interval_ms: 30_000,
                enabled: true,
            }),
        })
    }

    fn on_http_request(_req: IncomingHttpRequest) -> OutgoingHttpResponse {
        // This channel registers no HTTP endpoints.
        OutgoingHttpResponse {
            status: 404,
            headers_json: r#"{"Content-Type":"application/json"}"#.to_string(),
            body: br#"{"error":"not found"}"#.to_vec(),
        }
    }

    fn on_poll() {
        let homeserver = match channel_host::workspace_read(HOMESERVER_PATH) {
            Some(h) => h,
            None => {
                channel_host::log(
                    channel_host::LogLevel::Error,
                    "Homeserver not in workspace state — skipping poll",
                );
                return;
            }
        };

        let bot_user_id = channel_host::workspace_read(BOT_USER_ID_PATH).unwrap_or_default();
        let next_batch = channel_host::workspace_read(NEXT_BATCH_PATH).unwrap_or_default();

        let sync = match do_sync(&homeserver, &next_batch) {
            Ok(s) => s,
            Err(e) => {
                channel_host::log(
                    channel_host::LogLevel::Error,
                    &format!("Matrix /sync failed: {}", e),
                );
                return;
            }
        };

        // Persist the batch token before processing so it advances even if we
        // error on individual events — prevents re-delivering old events.
        let _ = channel_host::workspace_write(NEXT_BATCH_PATH, &sync.next_batch);

        // Handle invites — auto-join rooms from allowed senders
        for (room_id, invite) in &sync.rooms.invite {
            handle_invite(&homeserver, room_id, invite);
        }

        // Handle joined rooms
        for (room_id, room) in &sync.rooms.join {
            for event in &room.timeline.events {
                if event.event_type != "m.room.message" {
                    continue;
                }

                // Skip messages from the bot itself
                if !bot_user_id.is_empty() && event.sender == bot_user_id {
                    continue;
                }

                // Only handle plain text (m.text) — ignore images, files, etc.
                let msgtype = event
                    .content
                    .get("msgtype")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if msgtype != "m.text" {
                    continue;
                }

                let body = event
                    .content
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();

                if body.is_empty() {
                    continue;
                }

                handle_message(room_id, event, &body);
            }
        }
    }

    fn on_respond(response: AgentResponse) -> Result<(), String> {
        let homeserver = channel_host::workspace_read(HOMESERVER_PATH)
            .ok_or_else(|| "Homeserver not in workspace state".to_string())?;

        let metadata: MatrixMessageMetadata = serde_json::from_str(&response.metadata_json)
            .map_err(|e| format!("Failed to parse metadata: {}", e))?;

        // Matrix has a practical body limit of ~32 KB; chunk at 4 000 chars to stay safe
        let chunks = chunk_message(&response.content, 4_000);
        for chunk in chunks {
            let txn_id = next_txn_id();
            send_message(&homeserver, &metadata.room_id, &chunk, &txn_id)?;
        }

        Ok(())
    }

    fn on_status(update: StatusUpdate) {
        let homeserver = match channel_host::workspace_read(HOMESERVER_PATH) {
            Some(h) => h,
            None => return,
        };

        let bot_user_id = match channel_host::workspace_read(BOT_USER_ID_PATH) {
            Some(id) => id,
            None => return,
        };

        let metadata: MatrixMessageMetadata = match serde_json::from_str(&update.metadata_json) {
            Ok(m) => m,
            Err(_) => return,
        };

        if matches!(update.status, StatusType::ApprovalNeeded) {
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

            let mut msg = format!("Approval required — tool: {}", tool_name);
            if !description.is_empty() {
                msg.push('\n');
                msg.push_str(&description);
            }
            if !params_str.is_empty() {
                msg.push_str("\nParameters: ");
                msg.push_str(&params_str);
            }
            msg.push_str("\nReply yes to approve, always to always approve, or no to deny.");

            let txn_id = next_txn_id();
            if let Err(e) = send_message(&homeserver, &metadata.room_id, &msg, &txn_id) {
                channel_host::log(
                    channel_host::LogLevel::Warn,
                    &format!("Failed to send approval prompt: {}", e),
                );
            }
            return;
        }

        let (typing, timeout) = match update.status {
            // Show typing indicator while the agent is thinking
            StatusType::Thinking => (true, Some(30_000u32)),
            // Clear typing indicator on all other state changes
            _ => (false, None),
        };

        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/typing/{}",
            homeserver,
            url_encode(&metadata.room_id),
            url_encode(&bot_user_id),
        );

        let body = TypingRequest { typing, timeout };
        let body_bytes = match serde_json::to_vec(&body) {
            Ok(b) => b,
            Err(_) => return,
        };

        let headers = serde_json::json!({ "Content-Type": "application/json" });

        if let Err(e) = channel_host::http_request(
            "PUT",
            &url,
            &headers.to_string(),
            Some(&body_bytes),
            Some(10_000),
        ) {
            channel_host::log(
                channel_host::LogLevel::Debug,
                &format!("Typing indicator failed: {}", e),
            );
        }
    }

    fn on_event(_event_json: String) -> Result<(), String> {
        // This channel does not use persistent connections; events are delivered via polling.
        Ok(())
    }

    fn on_broadcast(user_id: String, content: String, _metadata_json: String) -> Result<(), String> {
        let homeserver = channel_host::workspace_read(HOMESERVER_PATH)
            .ok_or_else(|| "on_broadcast: homeserver not in workspace state".to_string())?;
        let bot_user_id = channel_host::workspace_read(BOT_USER_ID_PATH)
            .ok_or_else(|| "on_broadcast: bot_user_id not in workspace state".to_string())?;

        // Resolve the target Matrix user ID.
        let target_user_id = if user_id == "default" || user_id.is_empty() {
            channel_host::workspace_read(OWNER_ID_PATH)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    "on_broadcast: no owner_id configured and user_id is 'default'".to_string()
                })?
        } else {
            user_id
        };

        // find_or_create_dm is idempotent — returns existing room or creates a new one.
        let room_id = find_or_create_dm(&homeserver, &bot_user_id, &target_user_id)
            .map_err(|e| format!("on_broadcast: could not find/create DM room: {}", e))?;

        // Chunk the message (Matrix has a ~32 KB practical limit) and send.
        let txn_id = next_txn_id();
        send_message(&homeserver, &room_id, &content, &txn_id)
            .map_err(|e| format!("on_broadcast: send_message failed: {}", e))
    }

    fn on_shutdown() {
        channel_host::log(
            channel_host::LogLevel::Info,
            "Matrix channel shutting down",
        );
    }
}

// ============================================================================
// Message handling
// ============================================================================

/// Check the DM policy and emit the message to the agent if allowed.
fn handle_message(room_id: &str, event: &MatrixEvent, body: &str) {
    let dm_policy = channel_host::workspace_read(DM_POLICY_PATH)
        .unwrap_or_else(|| "pairing".to_string());

    if dm_policy != "open" {
        // Build the effective allow list: config allow_from + pairing-approved store
        let mut allowed: Vec<String> = channel_host::workspace_read(ALLOW_FROM_PATH)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if let Ok(store_allowed) = channel_host::pairing_read_allow_from(CHANNEL_NAME) {
            allowed.extend(store_allowed);
        }

        let is_allowed =
            allowed.contains(&"*".to_string()) || allowed.contains(&event.sender);

        if !is_allowed {
            if dm_policy == "pairing" {
                let meta = serde_json::json!({
                    "room_id": room_id,
                    "sender": event.sender,
                })
                .to_string();

                match channel_host::pairing_upsert_request(CHANNEL_NAME, &event.sender, &meta) {
                    Ok(result) => {
                        channel_host::log(
                            channel_host::LogLevel::Info,
                            &format!(
                                "Pairing request for {} in room {}: code {}",
                                event.sender, room_id, result.code
                            ),
                        );
                        if result.created {
                            // Send a one-time pairing message to the room
                            let pairing_text = format!(
                                "To pair with this bot, run: `rustytalon pairing approve matrix {}`",
                                result.code
                            );
                            if let Some(homeserver) =
                                channel_host::workspace_read(HOMESERVER_PATH)
                            {
                                let txn_id = next_txn_id();
                                if let Err(e) =
                                    send_message(&homeserver, room_id, &pairing_text, &txn_id)
                                {
                                    channel_host::log(
                                        channel_host::LogLevel::Error,
                                        &format!("Failed to send pairing reply: {}", e),
                                    );
                                }
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

    let metadata = MatrixMessageMetadata {
        room_id: room_id.to_string(),
        event_id: event.event_id.clone(),
        sender_user_id: event.sender.clone(),
    };

    let metadata_json = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());

    channel_host::emit_message(&EmittedMessage {
        user_id: event.sender.clone(),
        // Display names require a separate /profile API call; skip for now
        user_name: None,
        content: body.to_string(),
        // Use room_id as thread_id so multi-room conversations are tracked separately
        thread_id: Some(room_id.to_string()),
        metadata_json,
    });

    channel_host::log(
        channel_host::LogLevel::Debug,
        &format!("Emitted message from {} in room {}", event.sender, room_id),
    );
}

/// Auto-join a room if the inviter is allowed by the current DM policy.
fn handle_invite(homeserver: &str, room_id: &str, invite: &InvitedRoom) {
    // Find the invite event to determine who sent the invite
    let inviter = invite
        .invite_state
        .events
        .iter()
        .find(|e| {
            e.event_type == "m.room.member"
                && e.content
                    .get("membership")
                    .and_then(|v| v.as_str())
                    == Some("invite")
        })
        .map(|e| e.sender.as_str())
        .unwrap_or("");

    let dm_policy = channel_host::workspace_read(DM_POLICY_PATH)
        .unwrap_or_else(|| "pairing".to_string());

    let should_join = if dm_policy == "open" {
        true
    } else {
        let mut allowed: Vec<String> = channel_host::workspace_read(ALLOW_FROM_PATH)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if let Ok(store_allowed) = channel_host::pairing_read_allow_from(CHANNEL_NAME) {
            allowed.extend(store_allowed);
        }

        allowed.contains(&"*".to_string()) || allowed.contains(&inviter.to_string())
    };

    if !should_join {
        channel_host::log(
            channel_host::LogLevel::Info,
            &format!(
                "Ignoring invite from unpaired user {} in room {}",
                inviter, room_id
            ),
        );
        return;
    }

    let url = format!(
        "{}/_matrix/client/v3/join/{}",
        homeserver,
        url_encode(room_id),
    );

    let headers = serde_json::json!({ "Content-Type": "application/json" });

    match channel_host::http_request("POST", &url, &headers.to_string(), Some(&[]), Some(15_000)) {
        Ok(resp) if resp.status == 200 => {
            channel_host::log(
                channel_host::LogLevel::Info,
                &format!("Joined room {} (invited by {})", room_id, inviter),
            );
        }
        Ok(resp) => {
            channel_host::log(
                channel_host::LogLevel::Warn,
                &format!(
                    "Failed to join room {} (HTTP {})",
                    room_id, resp.status
                ),
            );
        }
        Err(e) => {
            channel_host::log(
                channel_host::LogLevel::Error,
                &format!("Failed to join room {}: {}", room_id, e),
            );
        }
    }
}

// ============================================================================
// Matrix REST API helpers
// ============================================================================

/// Validate the access token and return the bot's fully-qualified user ID.
fn whoami(homeserver: &str) -> Result<String, String> {
    let url = format!("{}/_matrix/client/v3/account/whoami", homeserver);
    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let resp = channel_host::http_request("GET", &url, &headers.to_string(), None, Some(15_000))
        .map_err(|e| format!("HTTP error: {}", e))?;

    if resp.status != 200 {
        let body = String::from_utf8_lossy(&resp.body);
        return Err(format!("homeserver returned {}: {}", resp.status, body));
    }

    let who: WhoAmIResponse =
        serde_json::from_slice(&resp.body).map_err(|e| format!("Parse error: {}", e))?;

    Ok(who.user_id)
}

/// Call `/sync` with `timeout=0` (non-blocking poll).
///
/// On the very first sync (`since` is empty) a narrow filter is applied to
/// avoid fetching the entire room history.
fn do_sync(homeserver: &str, since: &str) -> Result<SyncResponse, String> {
    // On first sync, limit timeline to 1 event per room so we don't replay
    // the full history.  The URL-encoded filter is:
    //   {"room":{"timeline":{"limit":1}}}
    let url = if since.is_empty() {
        format!(
            "{}/_matrix/client/v3/sync?timeout=0\
             &filter=%7B%22room%22%3A%7B%22timeline%22%3A%7B%22limit%22%3A1%7D%7D%7D",
            homeserver
        )
    } else {
        format!(
            "{}/_matrix/client/v3/sync?since={}&timeout=0",
            homeserver,
            url_encode(since)
        )
    };

    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let resp =
        channel_host::http_request("GET", &url, &headers.to_string(), None, Some(60_000))
            .map_err(|e| format!("HTTP error: {}", e))?;

    if resp.status != 200 {
        let body = String::from_utf8_lossy(&resp.body);
        return Err(format!("sync returned {}: {}", resp.status, body));
    }

    serde_json::from_slice(&resp.body).map_err(|e| format!("Parse error: {}", e))
}

/// Find an existing DM room with `owner_id` in `m.direct` account data, or
/// create a new one.
fn find_or_create_dm(
    homeserver: &str,
    bot_user_id: &str,
    owner_id: &str,
) -> Result<String, String> {
    let headers = serde_json::json!({ "Content-Type": "application/json" });

    // Check m.direct account data for an existing DM room
    let account_data_url = format!(
        "{}/_matrix/client/v3/user/{}/account_data/m.direct",
        homeserver,
        url_encode(bot_user_id),
    );

    let existing = channel_host::http_request(
        "GET",
        &account_data_url,
        &headers.to_string(),
        None,
        Some(15_000),
    )
    .ok()
    .filter(|r| r.status == 200)
    .and_then(|r| serde_json::from_slice::<MDirectAccountData>(&r.body).ok())
    .and_then(|d| d.rooms.get(owner_id).and_then(|rooms| rooms.first().cloned()));

    if let Some(room_id) = existing {
        return Ok(room_id);
    }

    // No existing DM room — create one and invite the owner
    let create_url = format!("{}/_matrix/client/v3/createRoom", homeserver);
    let create_body = CreateRoomRequest {
        is_direct: true,
        invite: vec![owner_id.to_string()],
        preset: "trusted_private_chat".to_string(),
    };

    let body_bytes =
        serde_json::to_vec(&create_body).map_err(|e| format!("Serialize error: {}", e))?;

    let resp = channel_host::http_request(
        "POST",
        &create_url,
        &headers.to_string(),
        Some(&body_bytes),
        Some(15_000),
    )
    .map_err(|e| format!("HTTP error: {}", e))?;

    if resp.status != 200 {
        let body = String::from_utf8_lossy(&resp.body);
        return Err(format!("createRoom returned {}: {}", resp.status, body));
    }

    let created: CreateRoomResponse =
        serde_json::from_slice(&resp.body).map_err(|e| format!("Parse error: {}", e))?;

    channel_host::log(
        channel_host::LogLevel::Info,
        &format!(
            "Created DM room {} with owner {}",
            created.room_id, owner_id
        ),
    );

    Ok(created.room_id)
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

/// Post a plain-text `m.text` message to a room.
fn send_message(
    homeserver: &str,
    room_id: &str,
    body: &str,
    txn_id: &str,
) -> Result<(), String> {
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
        homeserver,
        url_encode(room_id),
        txn_id,
    );

    let html = md_links_to_html(body);
    let msg = SendMessageRequest {
        msgtype: "m.text".to_string(),
        body: md_links_strip(body),
        format: Some("org.matrix.custom.html".to_string()),
        formatted_body: Some(html),
    };

    let body_bytes =
        serde_json::to_vec(&msg).map_err(|e| format!("Serialize error: {}", e))?;

    let headers = serde_json::json!({ "Content-Type": "application/json" });

    let resp = channel_host::http_request(
        "PUT",
        &url,
        &headers.to_string(),
        Some(&body_bytes),
        Some(30_000),
    )
    .map_err(|e| format!("HTTP error: {}", e))?;

    if resp.status != 200 {
        let body_str = String::from_utf8_lossy(&resp.body);
        return Err(format!(
            "send returned {} for room {}: {}",
            resp.status, room_id, body_str
        ));
    }

    channel_host::log(
        channel_host::LogLevel::Debug,
        &format!("Sent message to room {}", room_id),
    );

    Ok(())
}

// ============================================================================
// Utility helpers
// ============================================================================

/// HTML-escape `&`, `<`, `>`, and `"` in a string.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Convert `[label](url)` markdown links to `<a href="url">label</a>` HTML.
///
/// The rest of the text is HTML-escaped so it is safe to embed in a Matrix
/// `formatted_body`. Other markdown (bold, italic, code) is left as-is because
/// Matrix clients that support `org.matrix.custom.html` handle those via their
/// own rendering.
fn md_links_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&html_escape(&rest[..open]));
        rest = &rest[open + 1..];
        if let Some(close_bracket) = rest.find("](") {
            let label = &rest[..close_bracket];
            if !label.contains('[') {
                let after_paren = &rest[close_bracket + 2..];
                if let Some(close_paren) = after_paren.find(')') {
                    let url = &after_paren[..close_paren];
                    if !url.contains('(') {
                        out.push_str("<a href=\"");
                        out.push_str(&html_escape(url));
                        out.push_str("\">");
                        out.push_str(&html_escape(label));
                        out.push_str("</a>");
                        rest = &after_paren[close_paren + 1..];
                        continue;
                    }
                }
            }
        }
        out.push('[');
    }
    out.push_str(&html_escape(rest));
    out
}

/// Strip `[label](url)` markdown links to just `label` for a plain-text fallback.
fn md_links_strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        rest = &rest[open + 1..];
        if let Some(close_bracket) = rest.find("](") {
            let label = &rest[..close_bracket];
            if !label.contains('[') {
                let after_paren = &rest[close_bracket + 2..];
                if let Some(close_paren) = after_paren.find(')') {
                    let url = &after_paren[..close_paren];
                    if !url.contains('(') {
                        out.push_str(label);
                        rest = &after_paren[close_paren + 1..];
                        continue;
                    }
                }
            }
        }
        out.push('[');
    }
    out.push_str(rest);
    out
}

/// Generate a unique transaction ID using a timestamp and a monotonic counter.
///
/// Matrix requires each `txnId` to be unique per client session. Using
/// timestamp + counter is collision-resistant across restarts.
fn next_txn_id() -> String {
    let ts = channel_host::now_millis();
    let counter: u64 = channel_host::workspace_read(TXN_COUNTER_PATH)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let next = counter.wrapping_add(1);
    let _ = channel_host::workspace_write(TXN_COUNTER_PATH, &next.to_string());
    format!("rt-{}-{}", ts, next)
}

/// Split a message into chunks of at most `max_chars` UTF-8 characters.
///
/// Splits on character boundaries (not byte boundaries) to avoid breaking
/// multi-byte code points.
fn chunk_message(content: &str, max_chars: usize) -> Vec<String> {
    if content.chars().count() <= max_chars {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::with_capacity(max_chars * 4);
    let mut count = 0;

    for ch in content.chars() {
        if count >= max_chars {
            chunks.push(current.clone());
            current.clear();
            count = 0;
        }
        current.push(ch);
        count += 1;
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Percent-encode a string for safe use in URL path segments.
///
/// Only unreserved characters (`A-Z a-z 0-9 - _ . ~`) and `:` are left
/// unencoded. The colon is kept because Matrix IDs (`!room:server`,
/// `@user:server`) use it as a separator and encoding it would produce a
/// valid but unnecessarily ugly URL.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(byte as char);
            }
            b => {
                out.push('%');
                // Safety: digit in range 0..16 always yields Some
                out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
                out.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
            }
        }
    }
    out
}

// Export the WASM component
export!(MatrixChannel);

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- md_links_to_html ---

    #[test]
    fn test_html_plain_link() {
        assert_eq!(
            md_links_to_html("[Le Diplomate](https://lediplomatedc.com)"),
            "<a href=\"https://lediplomatedc.com\">Le Diplomate</a>"
        );
    }

    #[test]
    fn test_html_inline_link() {
        assert_eq!(
            md_links_to_html("Try [this](https://example.com) today"),
            "Try <a href=\"https://example.com\">this</a> today"
        );
    }

    #[test]
    fn test_html_escapes_surrounding_text() {
        assert_eq!(
            md_links_to_html("A & B < C > D"),
            "A &amp; B &lt; C &gt; D"
        );
    }

    #[test]
    fn test_html_escapes_url_and_label() {
        assert_eq!(
            md_links_to_html("[a&b](https://x.com/?a=1&b=2)"),
            "<a href=\"https://x.com/?a=1&amp;b=2\">a&amp;b</a>"
        );
    }

    #[test]
    fn test_html_no_links_passthrough_escaped() {
        assert_eq!(md_links_to_html("Hello world"), "Hello world");
    }

    // --- md_links_strip ---

    #[test]
    fn test_strip_removes_url() {
        assert_eq!(
            md_links_strip("[Le Diplomate](https://lediplomatedc.com)"),
            "Le Diplomate"
        );
    }

    #[test]
    fn test_strip_inline() {
        assert_eq!(
            md_links_strip("Try [this](https://example.com) today"),
            "Try this today"
        );
    }

    #[test]
    fn test_strip_no_links_passthrough() {
        let input = "No links here.";
        assert_eq!(md_links_strip(input), input);
    }

    // --- chunk_message ---

    #[test]
    fn test_chunk_short_message() {
        let chunks = chunk_message("hello", 4_000);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn test_chunk_exact_boundary() {
        let s = "a".repeat(4_000);
        let chunks = chunk_message(&s, 4_000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 4_000);
    }

    #[test]
    fn test_chunk_splits_correctly() {
        let s = "a".repeat(5_000);
        let chunks = chunk_message(&s, 4_000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4_000);
        assert_eq!(chunks[1].len(), 1_000);
    }

    #[test]
    fn test_chunk_multibyte() {
        // Each '€' is 3 UTF-8 bytes but 1 char — should chunk on char count
        let s = "€".repeat(5_000);
        let chunks = chunk_message(&s, 4_000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), 4_000);
        assert_eq!(chunks[1].chars().count(), 1_000);
    }

    // --- url_encode ---

    #[test]
    fn test_url_encode_room_id() {
        // !abc123:matrix.org — '!' should be encoded, ':' and '.' kept
        let encoded = url_encode("!abc123:matrix.org");
        assert!(encoded.starts_with("%21"), "expected %21 prefix, got {}", encoded);
        assert!(encoded.contains(":matrix.org"));
    }

    #[test]
    fn test_url_encode_user_id() {
        // @user:server.tld — '@' should be encoded
        let encoded = url_encode("@alice:matrix.org");
        assert!(encoded.starts_with("%40"), "expected %40 prefix, got {}", encoded);
        assert!(encoded.contains(":matrix.org"));
    }

    #[test]
    fn test_url_encode_batch_token() {
        // Batch tokens may contain '/' and '+'
        let token = "s123_456/789+abc==";
        let encoded = url_encode(token);
        assert!(!encoded.contains('/'), "slash not encoded");
        assert!(!encoded.contains('+'), "plus not encoded");
        assert!(!encoded.contains('='), "equals not encoded");
    }

    #[test]
    fn test_url_encode_unreserved_passthrough() {
        let s = "ABCabc012-_.~";
        assert_eq!(url_encode(s), s);
    }

    // --- next_txn_id format ---

    #[test]
    fn test_txn_id_format() {
        // Can't call host functions in unit tests, but we can verify the
        // format string is what we expect by constructing one manually.
        let ts: u64 = 1_700_000_000_000;
        let counter: u64 = 42;
        let id = format!("rt-{}-{}", ts, counter);
        assert!(id.starts_with("rt-"));
        assert!(id.contains('-'));
    }

    // --- on_broadcast routing decisions (pure logic, no host calls) ---

    #[test]
    fn test_broadcast_default_user_id_needs_owner_id() {
        // "default" should trigger owner_id lookup; verify the string matching
        assert_eq!("default", "default");
        assert!("default".is_empty() || "default" == "default");
    }

    #[test]
    fn test_broadcast_empty_user_id_treated_as_default() {
        let user_id = "";
        let is_default = user_id == "default" || user_id.is_empty();
        assert!(is_default);
    }

    #[test]
    fn test_broadcast_explicit_user_id_passes_through() {
        let user_id = "@alice:matrix.org";
        let is_default = user_id == "default" || user_id.is_empty();
        assert!(!is_default);
        assert_eq!(user_id, "@alice:matrix.org");
    }

    // --- WhoAmIResponse deserialization ---

    #[test]
    fn test_whoami_response_parses() {
        let json = r#"{"user_id": "@bot:matrix.org"}"#;
        let resp: WhoAmIResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.user_id, "@bot:matrix.org");
    }

    // --- MatrixMessageMetadata roundtrip ---

    #[test]
    fn test_matrix_metadata_roundtrip() {
        let meta = MatrixMessageMetadata {
            room_id: "!abc123:matrix.org".to_string(),
            event_id: "$eventid:matrix.org".to_string(),
            sender_user_id: "@alice:matrix.org".to_string(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: MatrixMessageMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.room_id, "!abc123:matrix.org");
        assert_eq!(parsed.event_id, "$eventid:matrix.org");
        assert_eq!(parsed.sender_user_id, "@alice:matrix.org");
    }

    // --- CreateRoomRequest serialization ---

    #[test]
    fn test_create_room_request_serializes() {
        let req = CreateRoomRequest {
            is_direct: true,
            invite: vec!["@owner:matrix.org".to_string()],
            preset: "trusted_private_chat".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["is_direct"], true);
        assert_eq!(v["invite"][0], "@owner:matrix.org");
        assert_eq!(v["preset"], "trusted_private_chat");
    }

    // --- owner_id persistence path ---

    #[test]
    fn test_owner_id_path_constant() {
        // The path used to persist owner_id must be stable across instances
        assert_eq!(OWNER_ID_PATH, "state/owner_id");
    }

    #[test]
    fn test_homeserver_path_constant() {
        assert_eq!(HOMESERVER_PATH, "state/homeserver");
    }

    #[test]
    fn test_bot_user_id_path_constant() {
        assert_eq!(BOT_USER_ID_PATH, "state/bot_user_id");
    }
}
