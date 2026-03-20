//! Types for Discord API requests and responses.

use serde::{Deserialize, Serialize};

/// Input parameters for the Discord tool.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DiscordAction {
    /// Send a message to a channel.
    SendMessage {
        /// Channel ID (snowflake, e.g., "1234567890123456789").
        channel_id: String,
        /// Message content (supports Discord markdown).
        content: String,
        /// Optional message ID to reply to.
        #[serde(default)]
        reply_to: Option<String>,
    },

    /// List channels in a guild (server).
    ListChannels {
        /// Guild ID (snowflake).
        guild_id: String,
    },

    /// Get message history from a channel.
    GetMessages {
        /// Channel ID (snowflake).
        channel_id: String,
        /// Maximum number of messages to return (default: 20, max: 100).
        #[serde(default = "default_limit")]
        limit: u32,
        /// Get messages before this message ID (for pagination).
        #[serde(default)]
        before: Option<String>,
    },

    /// Add a reaction to a message.
    AddReaction {
        /// Channel ID containing the message.
        channel_id: String,
        /// Message ID to react to.
        message_id: String,
        /// Emoji (Unicode emoji or custom format "name:id").
        emoji: String,
    },

    /// Get information about a user.
    GetUser {
        /// User ID (snowflake).
        user_id: String,
    },

    /// List guilds (servers) the bot is in.
    ListGuilds,

    /// Get information about a specific guild.
    GetGuild {
        /// Guild ID (snowflake).
        guild_id: String,
    },

    /// Create a new thread from a message.
    CreateThread {
        /// Channel ID containing the message.
        channel_id: String,
        /// Message ID to start the thread from.
        message_id: String,
        /// Thread name.
        name: String,
    },
}

fn default_limit() -> u32 {
    20
}

/// Result from send_message.
#[derive(Debug, Serialize)]
pub struct SendMessageResult {
    pub id: String,
    pub channel_id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<UserInfo>,
}

/// A Discord channel.
#[derive(Debug, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

/// Result from list_channels.
#[derive(Debug, Serialize)]
pub struct ListChannelsResult {
    pub channels: Vec<ChannelInfo>,
}

/// A message from channel history.
#[derive(Debug, Serialize)]
pub struct MessageInfo {
    pub id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<UserInfo>,
    pub timestamp: String,
}

/// Result from get_messages.
#[derive(Debug, Serialize)]
pub struct GetMessagesResult {
    pub messages: Vec<MessageInfo>,
}

/// Result from add_reaction.
#[derive(Debug, Serialize)]
pub struct AddReactionResult {
    pub ok: bool,
}

/// User information.
#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
    pub bot: bool,
}

/// Result from get_user.
#[derive(Debug, Serialize)]
pub struct GetUserResult {
    pub user: UserInfo,
}

/// Basic guild (server) information.
#[derive(Debug, Serialize)]
pub struct GuildInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
}

/// Result from list_guilds.
#[derive(Debug, Serialize)]
pub struct ListGuildsResult {
    pub guilds: Vec<GuildInfo>,
}

/// Result from get_guild.
#[derive(Debug, Serialize)]
pub struct GetGuildResult {
    pub guild: GuildInfo,
}

/// Result from create_thread.
#[derive(Debug, Serialize)]
pub struct CreateThreadResult {
    pub id: String,
    pub name: String,
    pub channel_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_message_required_fields() {
        let json = r#"{"action":"send_message","channel_id":"123","content":"hello"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(
            matches!(action, DiscordAction::SendMessage { ref content, .. } if content == "hello")
        );
    }

    #[test]
    fn send_message_with_reply() {
        let json =
            r#"{"action":"send_message","channel_id":"123","content":"hi","reply_to":"456"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            DiscordAction::SendMessage { reply_to: Some(ref id), .. } if id == "456"
        ));
    }

    #[test]
    fn send_message_reply_defaults_to_none() {
        let json = r#"{"action":"send_message","channel_id":"123","content":"hi"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            DiscordAction::SendMessage { reply_to: None, .. }
        ));
    }

    #[test]
    fn get_messages_limit_defaults_to_20() {
        let json = r#"{"action":"get_messages","channel_id":"123"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            DiscordAction::GetMessages { limit: 20, .. }
        ));
    }

    #[test]
    fn get_messages_custom_limit_and_before() {
        let json = r#"{"action":"get_messages","channel_id":"123","limit":50,"before":"999"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            DiscordAction::GetMessages { limit: 50, before: Some(ref b), .. } if b == "999"
        ));
    }

    #[test]
    fn list_guilds_no_extra_fields() {
        let json = r#"{"action":"list_guilds"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, DiscordAction::ListGuilds));
    }

    #[test]
    fn add_reaction_required_fields() {
        let json = r#"{"action":"add_reaction","channel_id":"1","message_id":"2","emoji":"👍"}"#;
        let action: DiscordAction = serde_json::from_str(json).unwrap();
        assert!(matches!(
            action,
            DiscordAction::AddReaction { ref emoji, .. } if emoji == "👍"
        ));
    }

    #[test]
    fn unknown_action_is_error() {
        let json = r#"{"action":"nonexistent"}"#;
        assert!(serde_json::from_str::<DiscordAction>(json).is_err());
    }

    #[test]
    fn send_message_missing_content_is_error() {
        let json = r#"{"action":"send_message","channel_id":"123"}"#;
        assert!(serde_json::from_str::<DiscordAction>(json).is_err());
    }
}
