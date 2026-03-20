//! Discord REST API v10 implementation.
//!
//! All API calls go through the host's HTTP capability, which handles
//! credential injection (Bot token) and rate limiting. The WASM tool
//! never sees the actual bot token.

use crate::near::agent::host;
use crate::types::*;

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Percent-encode a string for use in URL path segments.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0xf) as usize]));
            }
        }
    }
    out
}

/// Make a Discord API call.
fn discord_api_call(method: &str, endpoint: &str, body: Option<&str>) -> Result<String, String> {
    let url = format!("{}/{}", DISCORD_API_BASE, endpoint);

    let headers = if body.is_some() {
        r#"{"Content-Type": "application/json"}"#
    } else {
        "{}"
    };

    let body_bytes = body.map(|b| b.as_bytes().to_vec());

    host::log(
        host::LogLevel::Debug,
        &format!("Discord API: {} {}", method, endpoint),
    );

    let response = host::http_request(method, &url, headers, body_bytes.as_deref(), None)?;

    if response.status < 200 || response.status >= 300 {
        return Err(format!(
            "Discord API returned status {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
    }

    // DELETE reactions return 204 No Content
    if response.status == 204 {
        return Ok(String::new());
    }

    String::from_utf8(response.body).map_err(|e| format!("Invalid UTF-8 in response: {}", e))
}

fn parse_user(v: &serde_json::Value) -> UserInfo {
    UserInfo {
        id: v["id"].as_str().unwrap_or("").to_string(),
        username: v["username"].as_str().unwrap_or("").to_string(),
        global_name: v["global_name"].as_str().map(|s| s.to_string()),
        bot: v["bot"].as_bool().unwrap_or(false),
    }
}

/// Send a message to a channel.
pub fn send_message(
    channel_id: &str,
    content: &str,
    reply_to: Option<&str>,
) -> Result<SendMessageResult, String> {
    let mut payload = serde_json::json!({
        "content": content,
    });

    if let Some(msg_id) = reply_to {
        payload["message_reference"] = serde_json::json!({
            "message_id": msg_id,
        });
    }

    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let response = discord_api_call(
        "POST",
        &format!("channels/{}/messages", channel_id),
        Some(&body),
    )?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(SendMessageResult {
        id: parsed["id"].as_str().unwrap_or("").to_string(),
        channel_id: parsed["channel_id"]
            .as_str()
            .unwrap_or(channel_id)
            .to_string(),
        content: parsed["content"].as_str().unwrap_or("").to_string(),
        author: parsed.get("author").map(parse_user),
    })
}

/// List channels in a guild.
pub fn list_channels(guild_id: &str) -> Result<ListChannelsResult, String> {
    let response = discord_api_call("GET", &format!("guilds/{}/channels", guild_id), None)?;

    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    let channels = parsed
        .iter()
        .map(|c| ChannelInfo {
            id: c["id"].as_str().unwrap_or("").to_string(),
            name: c["name"].as_str().unwrap_or("").to_string(),
            channel_type: c["type"].as_u64().unwrap_or(0) as u32,
            topic: c["topic"].as_str().map(|s| s.to_string()),
            parent_id: c["parent_id"].as_str().map(|s| s.to_string()),
        })
        .collect();

    Ok(ListChannelsResult { channels })
}

/// Get message history from a channel.
pub fn get_messages(
    channel_id: &str,
    limit: u32,
    before: Option<&str>,
) -> Result<GetMessagesResult, String> {
    let mut endpoint = format!("channels/{}/messages?limit={}", channel_id, limit.min(100));

    if let Some(before_id) = before {
        endpoint.push_str(&format!("&before={}", url_encode(before_id)));
    }

    let response = discord_api_call("GET", &endpoint, None)?;

    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    let messages = parsed
        .iter()
        .map(|m| MessageInfo {
            id: m["id"].as_str().unwrap_or("").to_string(),
            content: m["content"].as_str().unwrap_or("").to_string(),
            author: m.get("author").map(parse_user),
            timestamp: m["timestamp"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(GetMessagesResult { messages })
}

/// Add a reaction to a message.
pub fn add_reaction(
    channel_id: &str,
    message_id: &str,
    emoji: &str,
) -> Result<AddReactionResult, String> {
    let encoded_emoji = url_encode(emoji);
    let endpoint = format!(
        "channels/{}/messages/{}/reactions/{}/@me",
        channel_id, message_id, encoded_emoji
    );

    discord_api_call("PUT", &endpoint, None)?;

    Ok(AddReactionResult { ok: true })
}

/// Get information about a user.
pub fn get_user(user_id: &str) -> Result<GetUserResult, String> {
    let response = discord_api_call("GET", &format!("users/{}", user_id), None)?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(GetUserResult {
        user: parse_user(&parsed),
    })
}

/// List guilds the bot is a member of.
pub fn list_guilds() -> Result<ListGuildsResult, String> {
    let response = discord_api_call("GET", "users/@me/guilds", None)?;

    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    let guilds = parsed
        .iter()
        .map(|g| GuildInfo {
            id: g["id"].as_str().unwrap_or("").to_string(),
            name: g["name"].as_str().unwrap_or("").to_string(),
            icon: g["icon"].as_str().map(|s| s.to_string()),
            owner_id: None,
        })
        .collect();

    Ok(ListGuildsResult { guilds })
}

/// Get information about a specific guild.
pub fn get_guild(guild_id: &str) -> Result<GetGuildResult, String> {
    let response = discord_api_call("GET", &format!("guilds/{}", guild_id), None)?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(GetGuildResult {
        guild: GuildInfo {
            id: parsed["id"].as_str().unwrap_or("").to_string(),
            name: parsed["name"].as_str().unwrap_or("").to_string(),
            icon: parsed["icon"].as_str().map(|s| s.to_string()),
            owner_id: parsed["owner_id"].as_str().map(|s| s.to_string()),
        },
    })
}

/// Create a thread from a message.
pub fn create_thread(
    channel_id: &str,
    message_id: &str,
    name: &str,
) -> Result<CreateThreadResult, String> {
    let payload = serde_json::json!({
        "name": name,
    });

    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let endpoint = format!("channels/{}/messages/{}/threads", channel_id, message_id);
    let response = discord_api_call("POST", &endpoint, Some(&body))?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(CreateThreadResult {
        id: parsed["id"].as_str().unwrap_or("").to_string(),
        name: parsed["name"].as_str().unwrap_or("").to_string(),
        channel_id: parsed["parent_id"]
            .as_str()
            .unwrap_or(channel_id)
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_unreserved_chars_unchanged() {
        assert_eq!(url_encode("abc123-_.~"), "abc123-_.~");
    }

    #[test]
    fn url_encode_spaces_and_special() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a/b"), "a%2Fb");
        assert_eq!(url_encode("name:id"), "name%3Aid");
    }

    #[test]
    fn url_encode_emoji() {
        // 👍 is U+1F44D, encoded as %F0%9F%91%8D
        assert_eq!(url_encode("👍"), "%F0%9F%91%8D");
    }

    #[test]
    fn url_encode_empty() {
        assert_eq!(url_encode(""), "");
    }
}
