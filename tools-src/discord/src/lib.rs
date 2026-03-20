//! Discord WASM Tool for RustyTalon.
//!
//! This is a standalone WASM component that provides Discord integration
//! via the Discord REST API v10. Operates as a bot user.
//!
//! # Capabilities Required
//!
//! - HTTP: `discord.com/api/v10/*` (GET, POST, PUT, PATCH, DELETE)
//! - Secrets: `discord_bot_token` (injected automatically as `Bot <token>`)
//!
//! # Supported Actions
//!
//! - `send_message`: Send a message to a channel (with optional reply)
//! - `list_channels`: List channels in a guild (server)
//! - `get_messages`: Get recent messages from a channel
//! - `add_reaction`: Add an emoji reaction to a message
//! - `get_user`: Get information about a Discord user
//! - `list_guilds`: List servers the bot is in
//! - `get_guild`: Get information about a specific server
//! - `create_thread`: Create a thread from a message
//!
//! # Example Usage
//!
//! ```json
//! {"action": "send_message", "channel_id": "1234567890123456789", "content": "Hello from the agent!"}
//! ```

mod api;
mod types;

use types::DiscordAction;

wit_bindgen::generate!({
    world: "sandboxed-tool",
    path: "../../wit/tool.wit",
});

struct DiscordTool;

impl exports::near::agent::tool::Guest for DiscordTool {
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
        "Discord bot integration for sending messages, listing channels and servers, \
         reading message history, adding reactions, getting user info, and creating threads. \
         Requires a Discord bot token with appropriate permissions (Send Messages, Read \
         Message History, Add Reactions, View Channels)."
            .to_string()
    }
}

fn execute_inner(params: &str) -> Result<String, String> {
    if !crate::near::agent::host::secret_exists("discord_bot_token") {
        return Err(
            "Discord bot token not configured. Please add the 'discord_bot_token' secret."
                .to_string(),
        );
    }

    let action: DiscordAction =
        serde_json::from_str(params).map_err(|e| format!("Invalid parameters: {}", e))?;

    crate::near::agent::host::log(
        crate::near::agent::host::LogLevel::Info,
        &format!("Executing Discord action: {:?}", action),
    );

    let result = match action {
        DiscordAction::SendMessage {
            channel_id,
            content,
            reply_to,
        } => {
            let result = api::send_message(&channel_id, &content, reply_to.as_deref())?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::ListChannels { guild_id } => {
            let result = api::list_channels(&guild_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::GetMessages {
            channel_id,
            limit,
            before,
        } => {
            let result = api::get_messages(&channel_id, limit, before.as_deref())?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::AddReaction {
            channel_id,
            message_id,
            emoji,
        } => {
            let result = api::add_reaction(&channel_id, &message_id, &emoji)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::GetUser { user_id } => {
            let result = api::get_user(&user_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::ListGuilds => {
            let result = api::list_guilds()?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::GetGuild { guild_id } => {
            let result = api::get_guild(&guild_id)?;
            serde_json::to_string(&result).map_err(|e| e.to_string())?
        }

        DiscordAction::CreateThread {
            channel_id,
            message_id,
            name,
        } => {
            let result = api::create_thread(&channel_id, &message_id, &name)?;
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
            "enum": ["send_message", "list_channels", "get_messages", "add_reaction", "get_user", "list_guilds", "get_guild", "create_thread"],
            "description": "The Discord operation to perform"
        },
        "channel_id": {
            "type": "string",
            "description": "Channel ID (snowflake). Required for: send_message, list_channels, get_messages, add_reaction, create_thread"
        },
        "guild_id": {
            "type": "string",
            "description": "Guild (server) ID (snowflake). Required for: list_channels, get_guild"
        },
        "content": {
            "type": "string",
            "description": "Message content (supports Discord markdown). Required for: send_message"
        },
        "reply_to": {
            "type": "string",
            "description": "Message ID to reply to. Used by: send_message"
        },
        "limit": {
            "type": "integer",
            "description": "Maximum number of messages to return (default: 20, max: 100). Used by: get_messages",
            "default": 20
        },
        "before": {
            "type": "string",
            "description": "Get messages before this message ID (for pagination). Used by: get_messages"
        },
        "message_id": {
            "type": "string",
            "description": "Message ID (snowflake). Required for: add_reaction, create_thread"
        },
        "emoji": {
            "type": "string",
            "description": "Emoji (Unicode character or custom format 'name:id'). Required for: add_reaction"
        },
        "user_id": {
            "type": "string",
            "description": "User ID (snowflake). Required for: get_user"
        },
        "name": {
            "type": "string",
            "description": "Thread name. Required for: create_thread"
        }
    }
}"#;

export!(DiscordTool);
