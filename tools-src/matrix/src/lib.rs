//! Matrix WASM Tool for RustyTalon.
//!
//! This is a standalone WASM component that provides Matrix integration
//! via the Client-Server API v3. Supports any Matrix homeserver
//! (matrix.org, Element, Synapse, Dendrite, self-hosted).
//!
//! # Capabilities Required
//!
//! - HTTP: `<homeserver>/_matrix/client/*` (GET, POST, PUT)
//! - Secrets: `matrix_access_token` (injected automatically as Bearer token)
//! - Workspace: `matrix/` prefix (reads homeserver URL from `matrix/homeserver`)
//!
//! # Homeserver Configuration
//!
//! The tool reads the homeserver URL from workspace at `matrix/homeserver`.
//! If not set, defaults to `https://matrix-client.matrix.org`.
//!
//! To use a custom homeserver, write the URL via memory_write:
//! ```json
//! {"path": "matrix/homeserver", "content": "https://matrix.example.com"}
//! ```
//!
//! # Supported Actions
//!
//! - `send_message`: Send a text message to a room (plain text or HTML)
//! - `list_rooms`: List rooms the user has joined
//! - `get_messages`: Get recent messages from a room
//! - `join_room`: Join a room by ID or alias
//! - `leave_room`: Leave a room
//! - `get_profile`: Get the authenticated user's profile
//! - `get_user_profile`: Get another user's profile
//! - `get_room_info`: Get room name, topic, and member count
//! - `send_read_receipt`: Mark a message as read
//! - `add_reaction`: React to a message with an emoji
//!
//! # Example Usage
//!
//! ```json
//! {"action": "send_message", "room_id": "!abc123:matrix.org", "body": "Hello from the agent!"}
//! ```

mod api;
mod types;

use types::MatrixAction;

wit_bindgen::generate!({
    world: "sandboxed-tool",
    path: "../../wit/tool.wit",
});

struct MatrixTool;

impl exports::near::agent::tool::Guest for MatrixTool {
    fn execute(req: exports::near::agent::tool::Request) -> exports::near::agent::tool::Response {
        match execute_inner(&req.params) {
            Ok(result) => exports::near::agent::tool::Response {
                output: Some(result),
                error: None,
            },
            Err(e) => exports::near::agent::tool::Response {
                output: None,
                error: Some(e),
            },
        }
    }

    fn schema() -> String {
        SCHEMA.to_string()
    }

    fn description() -> String {
        "Matrix messaging integration for sending messages, listing rooms, reading history, \
         joining/leaving rooms, viewing profiles, and reacting to messages. Works with any \
         Matrix homeserver (matrix.org, Element, self-hosted). Configure homeserver URL in \
         workspace at matrix/homeserver. Requires a Matrix access token."
            .to_string()
    }
}

fn execute_inner(params: &str) -> Result<String, String> {
    if !crate::near::agent::host::secret_exists("matrix_access_token") {
        return Err(
            "Matrix access token not configured. Please add the 'matrix_access_token' secret."
                .to_string(),
        );
    }

    let action: MatrixAction =
        serde_json::from_str(params).map_err(|e| format!("Invalid parameters: {}", e))?;

    crate::near::agent::host::log(
        crate::near::agent::host::LogLevel::Info,
        &format!("Executing Matrix action: {:?}", action),
    );

    let result = match action {
        MatrixAction::SendMessage {
            room_id,
            body,
            format,
            formatted_body,
        } => {
            let result = api::send_message(
                &room_id,
                &body,
                format.as_deref(),
                formatted_body.as_deref(),
            )?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::ListRooms => {
            let result = api::list_rooms()?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::GetMessages {
            room_id,
            limit,
            from,
        } => {
            let result = api::get_messages(&room_id, limit, from.as_deref())?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::JoinRoom { room_id } => {
            let result = api::join_room(&room_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::LeaveRoom { room_id } => {
            let result = api::leave_room(&room_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::GetProfile => {
            let result = api::get_profile()?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::GetUserProfile { user_id } => {
            let result = api::get_user_profile(&user_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::GetRoomInfo { room_id } => {
            let result = api::get_room_info(&room_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::SendReadReceipt { room_id, event_id } => {
            let result = api::send_read_receipt(&room_id, &event_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        MatrixAction::AddReaction {
            room_id,
            event_id,
            key,
        } => {
            let result = api::add_reaction(&room_id, &event_id, &key)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }
    };

    Ok(result)
}

const SCHEMA: &str = r#"{
    "type": "object",
    "required": ["action"],
    "properties": {
        "action": {
            "type": "string",
            "enum": ["send_message", "list_rooms", "get_messages", "join_room", "leave_room", "get_profile", "get_user_profile", "get_room_info", "send_read_receipt", "add_reaction"],
            "description": "The Matrix operation to perform"
        },
        "room_id": {
            "type": "string",
            "description": "Room ID (e.g., '!abc123:matrix.org') or alias (e.g., '#general:matrix.org'). Required for: send_message, get_messages, join_room, leave_room, get_room_info, send_read_receipt, add_reaction"
        },
        "body": {
            "type": "string",
            "description": "Message body (plain text). Required for: send_message"
        },
        "format": {
            "type": "string",
            "description": "Message format (set to 'org.matrix.custom.html' for HTML). Used by: send_message"
        },
        "formatted_body": {
            "type": "string",
            "description": "HTML-formatted message body (used with format). Used by: send_message"
        },
        "limit": {
            "type": "integer",
            "description": "Maximum number of messages to return (default: 20, max: 100). Used by: get_messages",
            "default": 20
        },
        "from": {
            "type": "string",
            "description": "Pagination token from a previous get_messages response. Used by: get_messages"
        },
        "user_id": {
            "type": "string",
            "description": "User ID (e.g., '@alice:matrix.org'). Required for: get_user_profile"
        },
        "event_id": {
            "type": "string",
            "description": "Event ID (e.g., '$abc123'). Required for: send_read_receipt, add_reaction"
        },
        "key": {
            "type": "string",
            "description": "Reaction key (emoji, e.g., '👍'). Required for: add_reaction"
        }
    }
}"#;

export!(MatrixTool);
