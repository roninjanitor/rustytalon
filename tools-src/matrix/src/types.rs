//! Types for Matrix Client-Server API requests and responses.

use serde::{Deserialize, Serialize};

/// Input parameters for the Matrix tool.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MatrixAction {
    /// Send a text message to a room.
    SendMessage {
        /// Room ID (e.g., "!abcdef:matrix.org") or alias (e.g., "#general:matrix.org").
        room_id: String,
        /// Message body (plain text or HTML via `format`).
        body: String,
        /// Optional: set to "org.matrix.custom.html" for HTML messages.
        #[serde(default)]
        format: Option<String>,
        /// Optional: HTML-formatted body (used when format is set).
        #[serde(default)]
        formatted_body: Option<String>,
    },

    /// List rooms the user has joined.
    ListRooms,

    /// Get messages from a room.
    GetMessages {
        /// Room ID.
        room_id: String,
        /// Maximum number of messages to return (default: 20).
        #[serde(default = "default_limit")]
        limit: u32,
        /// Pagination token from a previous response.
        #[serde(default)]
        from: Option<String>,
    },

    /// Join a room by ID or alias.
    JoinRoom {
        /// Room ID or alias.
        room_id: String,
    },

    /// Leave a room.
    LeaveRoom {
        /// Room ID.
        room_id: String,
    },

    /// Get the user's own profile.
    GetProfile,

    /// Get another user's profile.
    GetUserProfile {
        /// User ID (e.g., "@alice:matrix.org").
        user_id: String,
    },

    /// Get room state (name, topic, members count).
    GetRoomInfo {
        /// Room ID.
        room_id: String,
    },

    /// Send a read receipt for a message.
    SendReadReceipt {
        /// Room ID.
        room_id: String,
        /// Event ID to mark as read.
        event_id: String,
    },

    /// Add a reaction to a message.
    AddReaction {
        /// Room ID.
        room_id: String,
        /// Event ID of the message to react to.
        event_id: String,
        /// Reaction key (emoji, e.g., "👍").
        key: String,
    },
}

fn default_limit() -> u32 {
    20
}

/// Result from send_message.
#[derive(Debug, Serialize)]
pub struct SendMessageResult {
    pub event_id: String,
}

/// A joined room.
#[derive(Debug, Serialize)]
pub struct RoomInfo {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_count: Option<u32>,
}

/// Result from list_rooms.
#[derive(Debug, Serialize)]
pub struct ListRoomsResult {
    pub rooms: Vec<RoomInfo>,
}

/// A message event from room history.
#[derive(Debug, Serialize)]
pub struct MessageEvent {
    pub event_id: String,
    pub sender: String,
    pub body: String,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_type: Option<String>,
}

/// Result from get_messages.
#[derive(Debug, Serialize)]
pub struct GetMessagesResult {
    pub messages: Vec<MessageEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
}

/// Result from join_room / leave_room.
#[derive(Debug, Serialize)]
pub struct RoomActionResult {
    pub room_id: String,
}

/// User profile.
#[derive(Debug, Serialize)]
pub struct ProfileInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub displayname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// Result from add_reaction / send_read_receipt.
#[derive(Debug, Serialize)]
pub struct OkResult {
    pub ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_message_required_fields() {
        let json = r#"{"action":"send_message","room_id":"!abc:matrix.org","body":"hello"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, MatrixAction::SendMessage { ref body, .. } if body == "hello"));
    }

    #[test]
    fn send_message_with_html() {
        let json = r#"{"action":"send_message","room_id":"!abc:matrix.org","body":"hello",
            "format":"org.matrix.custom.html","formatted_body":"<b>hello</b>"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            MatrixAction::SendMessage { formatted_body: Some(ref html), .. } if html == "<b>hello</b>"
        ));
    }

    #[test]
    fn send_message_html_defaults_to_none() {
        let json = r#"{"action":"send_message","room_id":"!abc:matrix.org","body":"hi"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            MatrixAction::SendMessage {
                format: None,
                formatted_body: None,
                ..
            }
        ));
    }

    #[test]
    fn get_messages_limit_defaults_to_20() {
        let json = r#"{"action":"get_messages","room_id":"!abc:matrix.org"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            MatrixAction::GetMessages { limit: 20, .. }
        ));
    }

    #[test]
    fn get_messages_with_pagination_token() {
        let json = r#"{"action":"get_messages","room_id":"!abc:matrix.org","from":"s123_456"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            MatrixAction::GetMessages { from: Some(ref t), .. } if t == "s123_456"
        ));
    }

    #[test]
    fn list_rooms_no_extra_fields() {
        let json = r#"{"action":"list_rooms"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, MatrixAction::ListRooms));
    }

    #[test]
    fn get_profile_no_extra_fields() {
        let json = r#"{"action":"get_profile"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, MatrixAction::GetProfile));
    }

    #[test]
    fn add_reaction_required_fields() {
        let json =
            r#"{"action":"add_reaction","room_id":"!abc:matrix.org","event_id":"$ev1","key":"👍"}"#;
        let action: MatrixAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            MatrixAction::AddReaction { ref key, .. } if key == "👍"
        ));
    }

    #[test]
    fn unknown_action_is_error() {
        let json = r#"{"action":"nonexistent"}"#;
        assert!(serde_json::from_str::<MatrixAction>(json).is_err());
    }

    #[test]
    fn send_message_missing_body_is_error() {
        let json = r#"{"action":"send_message","room_id":"!abc:matrix.org"}"#;
        assert!(serde_json::from_str::<MatrixAction>(json).is_err());
    }
}
