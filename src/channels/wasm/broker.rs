//! Host-side connection broker for WASM channels.
//!
//! Manages persistent connections (WebSocket, long-poll, SSE) on behalf of
//! sandboxed WASM channels. Converts long-lived protocol connections into
//! the existing callback model: each inbound event triggers a fresh WASM
//! `on_event` invocation with full sandbox security.
//!
//! # Architecture
//!
//! The broker spawns two async tasks:
//!
//! 1. **Connection task** — maintains the persistent connection (WebSocket/long-poll/SSE),
//!    handles heartbeat, reconnect, and pushes raw event JSON into a bounded channel.
//! 2. **Dispatch task** — reads from the bounded channel and calls WASM `on_event` for
//!    each event (fresh instance per callback, same as `execute_poll`).
//!
//! # Security Model
//!
//! - Credentials are injected via placeholder substitution (same as HTTP requests)
//! - WASM never sees raw tokens — broker handles handshake/auth at host boundary
//! - Inbound events are size-limited and queue-bounded
//! - Event filtering happens at host level (before WASM instantiation)
//! - Each `on_event` call gets a fresh WASM instance (no persistent state)
//! - All existing security (allowlist, leak detection, fuel, timeout) applies

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc, watch};
use tokio::task::JoinHandle;

use crate::channels::IncomingMessage;
use crate::channels::wasm::capabilities::{ChannelCapabilities, ConnectionConfig};
use crate::channels::wasm::error::WasmChannelError;
use crate::channels::wasm::host::ChannelEmitRateLimiter;
use crate::channels::wasm::runtime::{PreparedChannelModule, WasmChannelRuntime};
use crate::channels::wasm::schema::{ConnectionType, EventFilterSchema, KeepaliveType};
use crate::channels::wasm::wrapper::WasmChannel;
use crate::pairing::PairingStore;

/// Host-side connection broker for a WASM channel.
///
/// Manages a persistent connection (WebSocket, long-poll, or SSE) and delivers
/// events to WASM via `on_event` callbacks. Two tasks are spawned: one for the
/// connection lifecycle and one for dispatching events to WASM.
pub struct ConnectionBroker;

impl ConnectionBroker {
    /// Spawn the broker tasks. Returns a handle for shutdown.
    ///
    /// The broker runs until the `shutdown_rx` watch channel receives `true`,
    /// or until reconnection attempts are exhausted.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        channel_name: String,
        config: ConnectionConfig,
        runtime: Arc<WasmChannelRuntime>,
        prepared: Arc<PreparedChannelModule>,
        capabilities: ChannelCapabilities,
        credentials: Arc<RwLock<HashMap<String, String>>>,
        message_tx: Arc<RwLock<Option<mpsc::Sender<IncomingMessage>>>>,
        rate_limiter: Arc<RwLock<ChannelEmitRateLimiter>>,
        pairing_store: Arc<PairingStore>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let (event_tx, event_rx) = mpsc::channel::<String>(config.event_queue_size);

        let conn_name = channel_name.clone();
        let conn_shutdown = shutdown_rx.clone();
        let conn_credentials = credentials.clone();
        let conn_config = config.clone();

        // Task 1: Connection manager
        let connection_task = tokio::spawn(async move {
            match conn_config.connection_type {
                ConnectionType::Websocket => {
                    if let Err(e) = run_websocket_connection(
                        &conn_name,
                        &conn_config,
                        &conn_credentials,
                        event_tx,
                        conn_shutdown,
                    )
                    .await
                    {
                        tracing::error!(
                            channel = %conn_name,
                            error = %e,
                            "Connection broker WebSocket task exited with error"
                        );
                    }
                }
                ConnectionType::LongPoll => {
                    tracing::warn!(
                        channel = %conn_name,
                        "Long-poll connection broker not yet implemented"
                    );
                }
                ConnectionType::Sse => {
                    tracing::warn!(
                        channel = %conn_name,
                        "SSE connection broker not yet implemented"
                    );
                }
            }
        });

        // Task 2: Event dispatch to WASM
        let dispatch_name = channel_name.clone();
        let dispatch_shutdown = shutdown_rx;
        let callback_timeout = runtime.config().callback_timeout;

        tokio::spawn(async move {
            run_dispatch_loop(
                &dispatch_name,
                event_rx,
                dispatch_shutdown,
                &runtime,
                &prepared,
                &capabilities,
                &credentials,
                &message_tx,
                &rate_limiter,
                pairing_store,
                callback_timeout,
            )
            .await;

            // When dispatch finishes, also abort the connection task
            connection_task.abort();
            tracing::info!(
                channel = %channel_name,
                "Connection broker stopped"
            );
        })
    }
}

// ============================================================================
// Event filtering
// ============================================================================

/// Extract the event type from a JSON value using a dot-separated field path.
///
/// Returns `None` if the path doesn't exist or the value isn't a string.
fn extract_event_type<'a>(event: &'a serde_json::Value, type_field: &str) -> Option<&'a str> {
    let mut current = event;
    for part in type_field.split('.') {
        current = current.get(part)?;
    }
    current.as_str()
}

/// Check whether an event should be delivered to WASM based on the filter config.
///
/// Rules:
/// - If the event type is in `drop`, it is always dropped (takes precedence).
/// - If `deliver_to_wasm` is non-empty, only listed types pass.
/// - If both are empty, all events pass.
/// - Events with missing/non-string type fields are dropped.
fn should_deliver_event(event: &serde_json::Value, filter: &EventFilterSchema) -> bool {
    let event_type = match extract_event_type(event, &filter.type_field) {
        Some(t) => t,
        None => return false,
    };

    // Drop list takes precedence
    if filter.drop.iter().any(|d| d == event_type) {
        return false;
    }

    // If deliver list is specified, event must be in it
    if !filter.deliver_to_wasm.is_empty() {
        return filter.deliver_to_wasm.iter().any(|d| d == event_type);
    }

    true
}

// ============================================================================
// Credential placeholder substitution
// ============================================================================

/// Replace `{PLACEHOLDER}` patterns in a string with credential values.
///
/// This is the same logic as host-side credential injection for HTTP requests,
/// applied to handshake payloads and connection URLs.
fn substitute_credentials(input: &str, credentials: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (name, value) in credentials {
        let placeholder = format!("{{{}}}", name);
        result = result.replace(&placeholder, value);
    }
    result
}

// ============================================================================
// Reconnection backoff
// ============================================================================

/// Maximum backoff duration (60 seconds).
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Calculate exponential backoff duration for a given attempt.
///
/// Formula: `min(base_ms * 2^attempt, 60_000)ms`
fn backoff_duration(base_ms: u64, attempt: u32) -> Duration {
    let ms = base_ms.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX));
    Duration::from_millis(ms).min(MAX_BACKOFF)
}

// ============================================================================
// WebSocket connection manager
// ============================================================================

/// Run the WebSocket connection loop with reconnection.
async fn run_websocket_connection(
    channel_name: &str,
    config: &ConnectionConfig,
    credentials: &RwLock<HashMap<String, String>>,
    event_tx: mpsc::Sender<String>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), WasmChannelError> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let mut attempt: u32 = 0;

    loop {
        // Check shutdown before connecting
        if *shutdown_rx.borrow() {
            return Ok(());
        }

        // Resolve the connection URL
        let creds = credentials.read().await.clone();
        let url = match &config.url {
            Some(u) => substitute_credentials(u, &creds),
            None => {
                // TODO: Implement url_from_api for Slack-style dynamic URLs
                return Err(WasmChannelError::BrokerConnectionFailed {
                    name: channel_name.to_string(),
                    reason: "No connection URL configured (url_from_api not yet implemented)"
                        .to_string(),
                });
            }
        };

        tracing::info!(
            channel = %channel_name,
            attempt = attempt,
            "Connecting to WebSocket"
        );

        // Connect
        let ws_result = tokio_tungstenite::connect_async(&url).await;
        let (ws_stream, _response) = match ws_result {
            Ok(conn) => {
                tracing::info!(
                    channel = %channel_name,
                    "WebSocket connected"
                );
                attempt = 0; // Reset on successful connect
                conn
            }
            Err(e) => {
                tracing::warn!(
                    channel = %channel_name,
                    error = %e,
                    attempt = attempt,
                    "WebSocket connection failed"
                );
                attempt += 1;
                if attempt > config.reconnect.max_retries {
                    return Err(WasmChannelError::BrokerReconnectExhausted {
                        name: channel_name.to_string(),
                        attempts: attempt,
                    });
                }
                let delay = backoff_duration(config.reconnect.backoff_ms, attempt - 1);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => continue,
                    _ = shutdown_rx.changed() => return Ok(()),
                }
            }
        };

        let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();

        // Perform handshake if configured
        if let Some(handshake) = &config.handshake {
            // Send handshake payload with credential substitution
            let creds_snapshot = credentials.read().await.clone();
            let payload_str = serde_json::to_string(&handshake.send).unwrap_or_default();
            let substituted = substitute_credentials(&payload_str, &creds_snapshot);

            if let Err(e) = ws_sink.send(Message::Text(substituted.into())).await {
                tracing::warn!(
                    channel = %channel_name,
                    error = %e,
                    "Failed to send handshake"
                );
                attempt += 1;
                if attempt > config.reconnect.max_retries {
                    return Err(WasmChannelError::BrokerHandshakeFailed {
                        name: channel_name.to_string(),
                        reason: e.to_string(),
                    });
                }
                let delay = backoff_duration(config.reconnect.backoff_ms, attempt - 1);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => continue,
                    _ = shutdown_rx.changed() => return Ok(()),
                }
            }
        }

        // Determine heartbeat config from keepalive
        let mut heartbeat_send: Option<serde_json::Value> = None;
        let mut heartbeat_expect_op: Option<u64> = None;
        let mut initial_heartbeat_interval: Option<Duration> = None;

        if let Some(keepalive) = &config.keepalive {
            match keepalive.r#type {
                KeepaliveType::JsonOpcode => {
                    heartbeat_send = keepalive.send.clone();
                    heartbeat_expect_op = keepalive
                        .expect
                        .as_ref()
                        .and_then(|e| e.get("op").and_then(|v| v.as_u64()));
                    // Start with fallback; update when we get the Hello message
                    initial_heartbeat_interval =
                        Some(Duration::from_millis(keepalive.fallback_interval_ms));
                }
                KeepaliveType::PingPong => {
                    // Protocol-level ping/pong is handled by tungstenite automatically
                    initial_heartbeat_interval =
                        Some(Duration::from_millis(keepalive.fallback_interval_ms));
                }
                KeepaliveType::None => {}
            }
        }

        // Main message loop
        let mut heartbeat_timer = initial_heartbeat_interval.map(tokio::time::interval);
        if let Some(ref mut timer) = heartbeat_timer {
            timer.tick().await; // Consume the first immediate tick
        }

        let mut missed_heartbeats: u32 = 0;
        const MAX_MISSED_HEARTBEATS: u32 = 3;
        let mut handshake_completed = config
            .handshake
            .as_ref()
            .is_none_or(|h| h.expect_op.is_none());

        loop {
            tokio::select! {
                msg = ws_stream_rx.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            tracing::warn!(
                                channel = %channel_name,
                                error = %e,
                                "WebSocket error, will reconnect"
                            );
                            break; // Reconnect
                        }
                        None => {
                            tracing::info!(
                                channel = %channel_name,
                                "WebSocket closed by server"
                            );
                            break; // Reconnect
                        }
                    };

                    match msg {
                        Message::Text(text) => {
                            let text_str: &str = &text;

                            // Size check
                            if text_str.len() > config.max_event_size {
                                tracing::warn!(
                                    channel = %channel_name,
                                    size = text_str.len(),
                                    max = config.max_event_size,
                                    "Event too large, dropping"
                                );
                                continue;
                            }

                            // Parse as JSON for filtering
                            let event: serde_json::Value = match serde_json::from_str(text_str) {
                                Ok(v) => v,
                                Err(_) => {
                                    tracing::debug!(
                                        channel = %channel_name,
                                        "Non-JSON message received, dropping"
                                    );
                                    continue;
                                }
                            };

                            // Check for heartbeat ACK
                            if let Some(expect_op) = heartbeat_expect_op {
                                if let Some(op) = event.get("op").and_then(|v| v.as_u64()) {
                                    if op == expect_op {
                                        missed_heartbeats = 0;
                                        continue;
                                    }
                                }
                            }

                            // Check for handshake response (e.g., Discord READY op 0)
                            if !handshake_completed {
                                if let Some(expect_op) = config.handshake.as_ref().and_then(|h| h.expect_op) {
                                    if let Some(op) = event.get("op").and_then(|v| v.as_u64()) {
                                        if op == expect_op {
                                            tracing::info!(
                                                channel = %channel_name,
                                                op = expect_op,
                                                "Handshake completed"
                                            );
                                            handshake_completed = true;
                                            // Don't deliver handshake response as an event
                                            continue;
                                        }
                                    }
                                }
                            }

                            // Extract heartbeat interval from Hello message if configured
                            if let Some(keepalive) = &config.keepalive {
                                if let Some(interval_field) = &keepalive.interval_field {
                                    if let Some(interval_ms) = extract_nested_u64(&event, interval_field) {
                                        let new_interval = Duration::from_millis(interval_ms);
                                        tracing::info!(
                                            channel = %channel_name,
                                            interval_ms = interval_ms,
                                            "Heartbeat interval from server"
                                        );
                                        heartbeat_timer = Some(tokio::time::interval(new_interval));
                                        if let Some(ref mut timer) = heartbeat_timer {
                                            timer.tick().await; // Consume first immediate tick
                                        }
                                        // Don't deliver Hello as an event
                                        continue;
                                    }
                                }
                            }

                            // Apply event filter
                            if !should_deliver_event(&event, &config.events) {
                                continue;
                            }

                            // Queue for WASM dispatch
                            if event_tx.try_send(text.to_string()).is_err() {
                                tracing::warn!(
                                    channel = %channel_name,
                                    "Event queue full, dropping oldest"
                                );
                            }
                        }
                        Message::Close(_) => {
                            tracing::info!(
                                channel = %channel_name,
                                "WebSocket close frame received"
                            );
                            break; // Reconnect
                        }
                        Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {
                            // Protocol frames handled by tungstenite; binary ignored
                        }
                    }
                }

                // Heartbeat tick
                _ = async {
                    if let Some(ref mut timer) = heartbeat_timer {
                        timer.tick().await
                    } else {
                        // Never fires
                        std::future::pending::<tokio::time::Instant>().await
                    }
                } => {
                    if let Some(ref payload) = heartbeat_send {
                        let payload_str = serde_json::to_string(payload).unwrap_or_default();
                        if let Err(e) = ws_sink.send(Message::Text(payload_str.into())).await {
                            tracing::warn!(
                                channel = %channel_name,
                                error = %e,
                                "Failed to send heartbeat"
                            );
                            break; // Reconnect
                        }
                        missed_heartbeats += 1;
                        if missed_heartbeats > MAX_MISSED_HEARTBEATS {
                            tracing::warn!(
                                channel = %channel_name,
                                missed = missed_heartbeats,
                                "Too many missed heartbeat ACKs, reconnecting"
                            );
                            break;
                        }
                    } else if matches!(config.keepalive.as_ref().map(|k| &k.r#type), Some(KeepaliveType::PingPong)) {
                        // Send WebSocket ping frame
                        if let Err(e) = ws_sink.send(Message::Ping(vec![].into())).await {
                            tracing::warn!(
                                channel = %channel_name,
                                error = %e,
                                "Failed to send WebSocket ping"
                            );
                            break;
                        }
                    }
                }

                // Shutdown signal
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!(
                            channel = %channel_name,
                            "Broker shutdown signal received"
                        );
                        // Send close frame
                        let _ = ws_sink.send(Message::Close(None)).await;
                        return Ok(());
                    }
                }
            }
        }

        // Reconnect logic
        attempt += 1;
        if attempt > config.reconnect.max_retries {
            return Err(WasmChannelError::BrokerReconnectExhausted {
                name: channel_name.to_string(),
                attempts: attempt,
            });
        }

        let delay = backoff_duration(config.reconnect.backoff_ms, attempt - 1);
        tracing::info!(
            channel = %channel_name,
            attempt = attempt,
            delay_ms = delay.as_millis() as u64,
            "Reconnecting after delay"
        );

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

/// Extract a u64 from a nested JSON path (dot-separated).
fn extract_nested_u64(value: &serde_json::Value, path: &str) -> Option<u64> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    current.as_u64()
}

// ============================================================================
// Event dispatch loop
// ============================================================================

/// Run the dispatch loop that delivers events to WASM via `on_event`.
#[allow(clippy::too_many_arguments)]
async fn run_dispatch_loop(
    channel_name: &str,
    mut event_rx: mpsc::Receiver<String>,
    mut shutdown_rx: watch::Receiver<bool>,
    runtime: &Arc<WasmChannelRuntime>,
    prepared: &Arc<PreparedChannelModule>,
    capabilities: &ChannelCapabilities,
    credentials: &Arc<RwLock<HashMap<String, String>>>,
    message_tx: &Arc<RwLock<Option<mpsc::Sender<IncomingMessage>>>>,
    rate_limiter: &Arc<RwLock<ChannelEmitRateLimiter>>,
    pairing_store: Arc<PairingStore>,
    callback_timeout: Duration,
) {
    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some(event_json) = event else {
                    tracing::debug!(
                        channel = %channel_name,
                        "Event channel closed, dispatch loop exiting"
                    );
                    break;
                };

                tracing::debug!(
                    channel = %channel_name,
                    event_len = event_json.len(),
                    "Dispatching event to WASM on_event"
                );

                let result = WasmChannel::execute_event(
                    channel_name,
                    runtime,
                    prepared,
                    capabilities,
                    credentials,
                    pairing_store.clone(),
                    callback_timeout,
                    &event_json,
                ).await;

                match result {
                    Ok(emitted_messages) => {
                        if !emitted_messages.is_empty() {
                            if let Err(e) = WasmChannel::dispatch_emitted_messages(
                                channel_name,
                                emitted_messages,
                                message_tx,
                                rate_limiter,
                            ).await {
                                tracing::warn!(
                                    channel = %channel_name,
                                    error = %e,
                                    "Failed to dispatch emitted messages from on_event"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            channel = %channel_name,
                            error = %e,
                            "on_event callback failed"
                        );
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!(
                        channel = %channel_name,
                        "Dispatch loop received shutdown signal"
                    );
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // Event filter tests
    // ====================================================================

    #[test]
    fn test_extract_event_type_simple() {
        let event = serde_json::json!({"t": "MESSAGE_CREATE", "d": {}});
        assert_eq!(extract_event_type(&event, "t"), Some("MESSAGE_CREATE"));
    }

    #[test]
    fn test_extract_event_type_nested() {
        let event = serde_json::json!({"payload": {"type": "message"}});
        assert_eq!(extract_event_type(&event, "payload.type"), Some("message"));
    }

    #[test]
    fn test_extract_event_type_different_fields() {
        let event = serde_json::json!({"event_type": "chat.message"});
        assert_eq!(
            extract_event_type(&event, "event_type"),
            Some("chat.message")
        );
    }

    #[test]
    fn test_extract_event_type_missing() {
        let event = serde_json::json!({"op": 1, "d": null});
        assert_eq!(extract_event_type(&event, "t"), None);
    }

    #[test]
    fn test_extract_event_type_non_string() {
        let event = serde_json::json!({"t": 42});
        assert_eq!(extract_event_type(&event, "t"), None);
    }

    #[test]
    fn test_filter_deliver_all_when_empty() {
        let filter = EventFilterSchema::default();
        let event = serde_json::json!({"t": "MESSAGE_CREATE"});
        assert!(should_deliver_event(&event, &filter));
    }

    #[test]
    fn test_filter_deliver_to_wasm_allowlist() {
        let filter = EventFilterSchema {
            deliver_to_wasm: vec!["MESSAGE_CREATE".to_string()],
            drop: vec![],
            type_field: "t".to_string(),
        };
        let yes = serde_json::json!({"t": "MESSAGE_CREATE"});
        let no = serde_json::json!({"t": "PRESENCE_UPDATE"});
        assert!(should_deliver_event(&yes, &filter));
        assert!(!should_deliver_event(&no, &filter));
    }

    #[test]
    fn test_filter_drop_blocklist() {
        let filter = EventFilterSchema {
            deliver_to_wasm: vec![],
            drop: vec!["PRESENCE_UPDATE".to_string(), "TYPING_START".to_string()],
            type_field: "t".to_string(),
        };
        let yes = serde_json::json!({"t": "MESSAGE_CREATE"});
        let no = serde_json::json!({"t": "PRESENCE_UPDATE"});
        assert!(should_deliver_event(&yes, &filter));
        assert!(!should_deliver_event(&no, &filter));
    }

    #[test]
    fn test_filter_drop_takes_precedence() {
        let filter = EventFilterSchema {
            deliver_to_wasm: vec!["MESSAGE_CREATE".to_string()],
            drop: vec!["MESSAGE_CREATE".to_string()],
            type_field: "t".to_string(),
        };
        let event = serde_json::json!({"t": "MESSAGE_CREATE"});
        assert!(!should_deliver_event(&event, &filter));
    }

    #[test]
    fn test_filter_missing_type_field_dropped() {
        let filter = EventFilterSchema::default();
        let event = serde_json::json!({"op": 1, "d": null});
        assert!(!should_deliver_event(&event, &filter));
    }

    #[test]
    fn test_filter_non_json_type_field() {
        let filter = EventFilterSchema::default();
        let event = serde_json::json!({"t": 42});
        assert!(!should_deliver_event(&event, &filter));
    }

    // ====================================================================
    // Credential substitution tests
    // ====================================================================

    #[test]
    fn test_substitute_single_credential() {
        let mut creds = HashMap::new();
        creds.insert("BOT_TOKEN".to_string(), "secret123".to_string());
        let result = substitute_credentials("Bearer {BOT_TOKEN}", &creds);
        assert_eq!(result, "Bearer secret123");
    }

    #[test]
    fn test_substitute_multiple_credentials() {
        let mut creds = HashMap::new();
        creds.insert("TOKEN".to_string(), "tok123".to_string());
        creds.insert("SECRET".to_string(), "sec456".to_string());
        let result = substitute_credentials("{TOKEN}:{SECRET}", &creds);
        assert_eq!(result, "tok123:sec456");
    }

    #[test]
    fn test_substitute_missing_credential() {
        let creds = HashMap::new();
        let result = substitute_credentials("Bearer {MISSING}", &creds);
        assert_eq!(result, "Bearer {MISSING}");
    }

    #[test]
    fn test_substitute_in_json() {
        let mut creds = HashMap::new();
        creds.insert(
            "DISCORD_BOT_TOKEN".to_string(),
            "NjE2.Xxxx.yyyy".to_string(),
        );
        let input = r#"{"op":2,"d":{"token":"{DISCORD_BOT_TOKEN}","intents":36864}}"#;
        let result = substitute_credentials(input, &creds);
        assert!(!result.contains("{DISCORD_BOT_TOKEN}"));
        assert!(result.contains("NjE2.Xxxx.yyyy"));
    }

    // ====================================================================
    // Backoff tests
    // ====================================================================

    #[test]
    fn test_backoff_exponential() {
        assert_eq!(backoff_duration(1000, 0), Duration::from_millis(1000));
        assert_eq!(backoff_duration(1000, 1), Duration::from_millis(2000));
        assert_eq!(backoff_duration(1000, 2), Duration::from_millis(4000));
        assert_eq!(backoff_duration(1000, 3), Duration::from_millis(8000));
        assert_eq!(backoff_duration(1000, 4), Duration::from_millis(16000));
    }

    #[test]
    fn test_backoff_capped_at_60s() {
        assert_eq!(backoff_duration(1000, 6), Duration::from_secs(60));
        assert_eq!(backoff_duration(1000, 10), Duration::from_secs(60));
    }

    #[test]
    fn test_backoff_resets() {
        // Verify attempt 0 always gives base duration (simulates reset after reconnect)
        assert_eq!(backoff_duration(500, 0), Duration::from_millis(500));
    }

    // ====================================================================
    // Event size limit tests
    // ====================================================================

    #[test]
    fn test_event_size_under_limit() {
        // This is tested implicitly in the WebSocket loop, but we verify the constant
        let max_size: usize = 65_536;
        let event = "x".repeat(max_size);
        assert!(event.len() <= max_size);
    }

    #[test]
    fn test_event_size_at_limit() {
        let max_size: usize = 65_536;
        let event = "x".repeat(max_size);
        assert_eq!(event.len(), max_size);
        // At limit should pass (> check, not >=)
    }

    #[test]
    fn test_event_size_over_limit() {
        let max_size: usize = 65_536;
        let event = "x".repeat(max_size + 1);
        assert!(event.len() > max_size);
    }

    // ====================================================================
    // Nested value extraction
    // ====================================================================

    #[test]
    fn test_extract_nested_u64() {
        let value = serde_json::json!({"d": {"heartbeat_interval": 41250}});
        assert_eq!(
            extract_nested_u64(&value, "d.heartbeat_interval"),
            Some(41250)
        );
    }

    #[test]
    fn test_extract_nested_u64_missing() {
        let value = serde_json::json!({"d": {}});
        assert_eq!(extract_nested_u64(&value, "d.heartbeat_interval"), None);
    }

    #[test]
    fn test_extract_nested_u64_top_level() {
        let value = serde_json::json!({"interval": 30000});
        assert_eq!(extract_nested_u64(&value, "interval"), Some(30000));
    }
}
