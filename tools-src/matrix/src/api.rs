//! Matrix Client-Server API implementation.
//!
//! Uses the v3 Client-Server API. All HTTP calls go through the host's
//! HTTP capability, which handles credential injection (Bearer token)
//! and rate limiting. The WASM tool never sees the actual access token.
//!
//! The homeserver URL is read from the workspace at `matrix/homeserver`.
//! If not set, defaults to `https://matrix-client.matrix.org`.

use crate::near::agent::host;
use crate::types::*;

const DEFAULT_HOMESERVER: &str = "https://matrix-client.matrix.org";

/// Get the homeserver base URL (from workspace or default).
fn homeserver() -> String {
    host::workspace_read("matrix/homeserver")
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_HOMESERVER.to_string())
}

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

/// Make a Matrix API call.
fn matrix_api_call(method: &str, endpoint: &str, body: Option<&str>) -> Result<String, String> {
    let url = format!("{}/_matrix/client/v3/{}", homeserver(), endpoint);

    let headers = if body.is_some() {
        r#"{"Content-Type": "application/json"}"#
    } else {
        "{}"
    };

    let body_bytes = body.map(|b| b.as_bytes().to_vec());

    host::log(
        host::LogLevel::Debug,
        &format!("Matrix API: {} {}", method, endpoint),
    );

    let response = host::http_request(method, &url, headers, body_bytes.as_deref(), None)?;

    if response.status < 200 || response.status >= 300 {
        let body_str = String::from_utf8_lossy(&response.body);
        // Matrix errors include errcode and error fields
        return Err(format!(
            "Matrix API returned status {}: {}",
            response.status, body_str
        ));
    }

    String::from_utf8(response.body).map_err(|e| format!("Invalid UTF-8 in response: {}", e))
}

/// Generate a transaction ID for idempotent PUT requests.
fn txn_id() -> String {
    // Use a combination of timestamp-like values from the host
    // Since we don't have full randomness in WASM, use the host's time
    format!("rt_{}", host::now_millis())
}

/// Send a message to a room.
pub fn send_message(
    room_id: &str,
    body: &str,
    format: Option<&str>,
    formatted_body: Option<&str>,
) -> Result<SendMessageResult, String> {
    let mut content = serde_json::json!({
        "msgtype": "m.text",
        "body": body,
    });

    if let (Some(fmt), Some(html)) = (format, formatted_body) {
        content["format"] = serde_json::Value::String(fmt.to_string());
        content["formatted_body"] = serde_json::Value::String(html.to_string());
    }

    let payload = serde_json::to_string(&content).map_err(|e| e.to_string())?;
    let endpoint = format!(
        "rooms/{}/send/m.room.message/{}",
        url_encode(room_id),
        txn_id()
    );

    let response = matrix_api_call("PUT", &endpoint, Some(&payload))?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(SendMessageResult {
        event_id: parsed["event_id"].as_str().unwrap_or("").to_string(),
    })
}

/// List joined rooms with their names.
pub fn list_rooms() -> Result<ListRoomsResult, String> {
    let response = matrix_api_call("GET", "joined_rooms", None)?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    let room_ids: Vec<String> = parsed["joined_rooms"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Fetch name/topic for each room via state events
    let mut rooms = Vec::with_capacity(room_ids.len());
    for rid in &room_ids {
        let info = get_room_info_inner(rid).unwrap_or(RoomInfo {
            room_id: rid.clone(),
            name: None,
            topic: None,
            member_count: None,
        });
        rooms.push(info);
    }

    Ok(ListRoomsResult { rooms })
}

/// Get messages from a room.
pub fn get_messages(
    room_id: &str,
    limit: u32,
    from: Option<&str>,
) -> Result<GetMessagesResult, String> {
    let mut endpoint = format!(
        "rooms/{}/messages?dir=b&limit={}",
        url_encode(room_id),
        limit.min(100)
    );

    if let Some(token) = from {
        endpoint.push_str(&format!("&from={}", url_encode(token)));
    } else {
        // Need a sync token; use the "end" from an initial sync or just do a messages call
        // The Matrix spec requires a `from` param; get it from /sync
        let sync_resp = matrix_api_call(
            "GET",
            "sync?timeout=0&filter={\"room\":{\"timeline\":{\"limit\":0}}}",
            None,
        )?;
        let sync: serde_json::Value =
            serde_json::from_str(&sync_resp).map_err(|e| format!("Failed to parse sync: {}", e))?;
        if let Some(token) = sync["next_batch"].as_str() {
            endpoint.push_str(&format!("&from={}", url_encode(token)));
        }
    }

    // Filter to only message events
    endpoint.push_str(&format!(
        "&filter={}",
        url_encode(r#"{"types":["m.room.message"]}"#)
    ));

    let response = matrix_api_call("GET", &endpoint, None)?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    let messages = parsed["chunk"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|ev| {
                    let content = &ev["content"];
                    Some(MessageEvent {
                        event_id: ev["event_id"].as_str()?.to_string(),
                        sender: ev["sender"].as_str().unwrap_or("").to_string(),
                        body: content["body"].as_str().unwrap_or("").to_string(),
                        timestamp: ev["origin_server_ts"].as_u64().unwrap_or(0),
                        msg_type: content["msgtype"].as_str().map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let end = parsed["end"].as_str().map(|s| s.to_string());

    Ok(GetMessagesResult { messages, end })
}

/// Join a room.
pub fn join_room(room_id: &str) -> Result<RoomActionResult, String> {
    let endpoint = format!("join/{}", url_encode(room_id));
    let response = matrix_api_call("POST", &endpoint, Some("{}"))?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(RoomActionResult {
        room_id: parsed["room_id"].as_str().unwrap_or(room_id).to_string(),
    })
}

/// Leave a room.
pub fn leave_room(room_id: &str) -> Result<RoomActionResult, String> {
    let endpoint = format!("rooms/{}/leave", url_encode(room_id));
    matrix_api_call("POST", &endpoint, Some("{}"))?;

    Ok(RoomActionResult {
        room_id: room_id.to_string(),
    })
}

/// Get the authenticated user's profile.
pub fn get_profile() -> Result<ProfileInfo, String> {
    // First get our user ID from /account/whoami
    let whoami_resp = matrix_api_call("GET", "account/whoami", None)?;
    let whoami: serde_json::Value =
        serde_json::from_str(&whoami_resp).map_err(|e| format!("Failed to parse whoami: {}", e))?;

    let user_id = whoami["user_id"]
        .as_str()
        .ok_or("Could not determine user ID")?;

    let mut profile = get_user_profile_inner(user_id)?;
    profile.user_id = Some(user_id.to_string());
    Ok(profile)
}

/// Get another user's profile.
pub fn get_user_profile(user_id: &str) -> Result<ProfileInfo, String> {
    let mut profile = get_user_profile_inner(user_id)?;
    profile.user_id = Some(user_id.to_string());
    Ok(profile)
}

fn get_user_profile_inner(user_id: &str) -> Result<ProfileInfo, String> {
    let endpoint = format!("profile/{}", url_encode(user_id));
    let response = matrix_api_call("GET", &endpoint, None)?;

    let parsed: serde_json::Value =
        serde_json::from_str(&response).map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(ProfileInfo {
        displayname: parsed["displayname"].as_str().map(|s| s.to_string()),
        avatar_url: parsed["avatar_url"].as_str().map(|s| s.to_string()),
        user_id: None,
    })
}

/// Get room state info (name, topic, member count).
pub fn get_room_info(room_id: &str) -> Result<RoomInfo, String> {
    get_room_info_inner(room_id)
}

fn get_room_info_inner(room_id: &str) -> Result<RoomInfo, String> {
    let encoded = url_encode(room_id);

    // Fetch name
    let name = matrix_api_call("GET", &format!("rooms/{}/state/m.room.name", encoded), None)
        .ok()
        .and_then(|r| {
            serde_json::from_str::<serde_json::Value>(&r).ok()?["name"]
                .as_str()
                .map(|s| s.to_string())
        });

    // Fetch topic
    let topic = matrix_api_call(
        "GET",
        &format!("rooms/{}/state/m.room.topic", encoded),
        None,
    )
    .ok()
    .and_then(|r| {
        serde_json::from_str::<serde_json::Value>(&r).ok()?["topic"]
            .as_str()
            .map(|s| s.to_string())
    });

    // Fetch member count
    let member_count = matrix_api_call("GET", &format!("rooms/{}/joined_members", encoded), None)
        .ok()
        .and_then(|r| {
            let v: serde_json::Value = serde_json::from_str(&r).ok()?;
            v["joined"].as_object().map(|o| o.len() as u32)
        });

    Ok(RoomInfo {
        room_id: room_id.to_string(),
        name,
        topic,
        member_count,
    })
}

/// Send a read receipt.
pub fn send_read_receipt(room_id: &str, event_id: &str) -> Result<OkResult, String> {
    let endpoint = format!(
        "rooms/{}/receipt/m.read/{}",
        url_encode(room_id),
        url_encode(event_id)
    );

    matrix_api_call("POST", &endpoint, Some("{}"))?;

    Ok(OkResult { ok: true })
}

/// Add a reaction to a message.
pub fn add_reaction(room_id: &str, event_id: &str, key: &str) -> Result<OkResult, String> {
    let content = serde_json::json!({
        "m.relates_to": {
            "rel_type": "m.annotation",
            "event_id": event_id,
            "key": key,
        }
    });

    let payload = serde_json::to_string(&content).map_err(|e| e.to_string())?;
    let endpoint = format!("rooms/{}/send/m.reaction/{}", url_encode(room_id), txn_id());

    matrix_api_call("PUT", &endpoint, Some(&payload))?;

    Ok(OkResult { ok: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_unreserved_chars_unchanged() {
        assert_eq!(url_encode("abc123-_.~"), "abc123-_.~");
    }

    #[test]
    fn url_encode_room_id() {
        // Room IDs contain '!' and ':' which must be encoded in path segments
        assert_eq!(url_encode("!abc123:matrix.org"), "%21abc123%3Amatrix.org");
    }

    #[test]
    fn url_encode_user_id() {
        // User IDs contain '@' and ':'
        assert_eq!(url_encode("@alice:matrix.org"), "%40alice%3Amatrix.org");
    }

    #[test]
    fn url_encode_event_id() {
        // Event IDs contain '$'
        assert_eq!(url_encode("$eventid"), "%24eventid");
    }

    #[test]
    fn url_encode_empty() {
        assert_eq!(url_encode(""), "");
    }
}
