//! Proactive heartbeat system for periodic execution.
//!
//! The heartbeat runner executes periodically (default: every 30 minutes) and:
//! 1. Reads the HEARTBEAT.md checklist
//! 2. Runs an agent turn to process the checklist
//! 3. Reports any findings to the configured channel
//!
//! If nothing needs attention, the agent replies "HEARTBEAT_OK" and no
//! message is sent to the user.
//!
//! # Usage
//!
//! Create a HEARTBEAT.md in the workspace with a checklist of things to monitor:
//!
//! ```markdown
//! # Heartbeat Checklist
//!
//! - [ ] Check for unread emails
//! - [ ] Review calendar for upcoming events
//! - [ ] Check project build status
//! ```
//!
//! The agent will process this checklist on each heartbeat and only notify
//! if action is needed.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::channels::OutgoingResponse;
use crate::db::Database;
use crate::llm::{ChatMessage, CompletionRequest, FinishReason, LlmProvider};
use crate::workspace::{Workspace, paths};

/// Default audit log retention in days.
const DEFAULT_AUDIT_RETENTION_DAYS: u64 = 90;

/// Default daily log retention in days (shared with audit log via `audit_retention_days` setting).
const DEFAULT_DAILY_LOG_RETENTION_DAYS: u64 = 90;

/// Configuration for the heartbeat runner.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Interval between heartbeat checks.
    pub interval: Duration,
    /// Whether heartbeat is enabled.
    pub enabled: bool,
    /// Maximum consecutive failures before disabling.
    pub max_failures: u32,
    /// User ID to notify on heartbeat findings.
    pub notify_user_id: Option<String>,
    /// Channel to notify on heartbeat findings.
    pub notify_channel: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30 * 60), // 30 minutes
            enabled: true,
            max_failures: 3,
            notify_user_id: None,
            notify_channel: None,
        }
    }
}

impl HeartbeatConfig {
    /// Create a config with a specific interval.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Disable heartbeat.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Set the notification target.
    pub fn with_notify(mut self, user_id: impl Into<String>, channel: impl Into<String>) -> Self {
        self.notify_user_id = Some(user_id.into());
        self.notify_channel = Some(channel.into());
        self
    }
}

/// Result of a heartbeat check.
#[derive(Debug)]
pub enum HeartbeatResult {
    /// Nothing needs attention.
    Ok,
    /// Something needs attention, with the message to send.
    NeedsAttention(String),
    /// Heartbeat was skipped (no checklist or disabled).
    Skipped,
    /// Heartbeat failed.
    Failed(String),
}

/// Heartbeat runner for proactive periodic execution.
pub struct HeartbeatRunner {
    config: HeartbeatConfig,
    workspace: Arc<Workspace>,
    llm: Arc<dyn LlmProvider>,
    db: Option<Arc<dyn Database>>,
    response_tx: Option<mpsc::Sender<OutgoingResponse>>,
    consecutive_failures: u32,
}

impl HeartbeatRunner {
    /// Create a new heartbeat runner.
    pub fn new(
        config: HeartbeatConfig,
        workspace: Arc<Workspace>,
        llm: Arc<dyn LlmProvider>,
    ) -> Self {
        Self {
            config,
            workspace,
            llm,
            db: None,
            response_tx: None,
            consecutive_failures: 0,
        }
    }

    /// Attach a database for audit log pruning.
    pub fn with_database(mut self, db: Arc<dyn Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// Set the response channel for notifications.
    pub fn with_response_channel(mut self, tx: mpsc::Sender<OutgoingResponse>) -> Self {
        self.response_tx = Some(tx);
        self
    }

    /// Run the heartbeat loop.
    ///
    /// This runs forever, checking periodically based on the configured interval.
    pub async fn run(&mut self) {
        if !self.config.enabled {
            tracing::info!("Heartbeat is disabled, not starting loop");
            return;
        }

        tracing::info!(
            "Starting heartbeat loop with interval {:?}",
            self.config.interval
        );

        let mut interval = tokio::time::interval(self.config.interval);
        // Don't run immediately on startup
        interval.tick().await;

        loop {
            interval.tick().await;

            match self.check_heartbeat().await {
                HeartbeatResult::Ok => {
                    tracing::debug!("Heartbeat OK");
                    self.consecutive_failures = 0;
                }
                HeartbeatResult::NeedsAttention(message) => {
                    tracing::info!("Heartbeat needs attention: {}", message);
                    self.consecutive_failures = 0;
                    self.send_notification(&message).await;
                }
                HeartbeatResult::Skipped => {
                    tracing::debug!("Heartbeat skipped");
                }
                HeartbeatResult::Failed(error) => {
                    tracing::error!("Heartbeat failed: {}", error);
                    self.consecutive_failures += 1;

                    if self.consecutive_failures >= self.config.max_failures {
                        tracing::error!(
                            "Heartbeat disabled after {} consecutive failures",
                            self.consecutive_failures
                        );
                        break;
                    }
                }
            }

            self.consolidate_daily_logs().await;
            self.prune_audit_log().await;
            self.prune_daily_logs().await;
        }
    }

    /// Run a single heartbeat check.
    pub async fn check_heartbeat(&self) -> HeartbeatResult {
        // Get the heartbeat checklist
        let checklist = match self.workspace.heartbeat_checklist().await {
            Ok(Some(content)) if !is_effectively_empty(&content) => content,
            Ok(_) => return HeartbeatResult::Skipped,
            Err(e) => return HeartbeatResult::Failed(format!("Failed to read checklist: {}", e)),
        };

        // Build the heartbeat prompt
        let prompt = format!(
            "Read the HEARTBEAT.md checklist below and follow it strictly. \
             Do not infer or repeat old tasks. Check each item and report findings.\n\
             \n\
             If nothing needs attention, reply EXACTLY with: HEARTBEAT_OK\n\
             \n\
             If something needs attention, provide a concise summary of what needs action.\n\
             \n\
             ## HEARTBEAT.md\n\
             \n\
             {}",
            checklist
        );

        // Get the system prompt for context
        let system_prompt = match self.workspace.system_prompt().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to get system prompt for heartbeat: {}", e);
                String::new()
            }
        };

        // Run the agent turn
        let messages = if system_prompt.is_empty() {
            vec![ChatMessage::user(&prompt)]
        } else {
            vec![
                ChatMessage::system(&system_prompt),
                ChatMessage::user(&prompt),
            ]
        };

        // Use the model's context_length to set max_tokens. The API returns
        // the total context window; we cap output at half of that (the rest is
        // the prompt) with a floor of 4096.
        let max_tokens = match self.llm.model_metadata().await {
            Ok(meta) => {
                let from_api = meta.context_length.map(|ctx| ctx / 2).unwrap_or(4096);
                from_api.max(4096)
            }
            Err(e) => {
                tracing::warn!(
                    "Could not fetch model metadata, using default max_tokens: {}",
                    e
                );
                4096
            }
        };

        let request = CompletionRequest::new(messages)
            .with_max_tokens(max_tokens)
            .with_temperature(0.3);

        let response = match self.llm.complete(request).await {
            Ok(r) => r,
            Err(e) => return HeartbeatResult::Failed(format!("LLM call failed: {}", e)),
        };

        let content = response.content.trim();

        // Guard against empty content. Reasoning models (e.g. GLM-4.7) may
        // burn all output tokens on chain-of-thought and return content: null.
        if content.is_empty() {
            return if response.finish_reason == FinishReason::Length {
                HeartbeatResult::Failed(
                    "LLM response was truncated (finish_reason=length) with no content. \
                     The model may have exhausted its token budget on reasoning."
                        .to_string(),
                )
            } else {
                HeartbeatResult::Failed("LLM returned empty content.".to_string())
            };
        }

        // Check if nothing needs attention
        if content == "HEARTBEAT_OK" || content.contains("HEARTBEAT_OK") {
            return HeartbeatResult::Ok;
        }

        HeartbeatResult::NeedsAttention(content.to_string())
    }

    /// Extract atomic facts from past daily logs into USER.md and MEMORY.md.
    ///
    /// For each daily log older than today:
    /// 1. Makes a lightweight LLM call to extract new facts not already in USER.md / MEMORY.md.
    /// 2. Appends any new facts to the appropriate file.
    /// 3. Deletes the daily log (facts are now in permanent storage).
    ///
    /// If the LLM call fails the daily log is preserved and retried next tick.
    /// Runs silently — errors are logged but never surface to callers.
    async fn consolidate_daily_logs(&self) {
        let today = Utc::now().date_naive();

        let entries = match self.workspace.list("daily/").await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Failed to list daily logs for consolidation: {e}");
                return;
            }
        };

        for entry in entries {
            if entry.is_directory {
                continue;
            }

            let name = entry.name().to_string();
            let date_str = name.trim_end_matches(".md");
            let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
                continue;
            };
            if date >= today {
                continue;
            }

            let path = format!("daily/{}.md", date_str);
            let log_content = match self.workspace.read(&path).await {
                Ok(d) if !d.content.trim().is_empty() => d.content,
                Ok(_) => {
                    // Empty log — nothing to extract, just clean it up.
                    let _ = self.workspace.delete(&path).await;
                    continue;
                }
                Err(_) => continue,
            };

            // Read current USER.md and MEMORY.md so the LLM can skip duplicates.
            let user_md = self
                .workspace
                .read(paths::USER)
                .await
                .map(|d| d.content)
                .unwrap_or_default();
            let memory_md = self
                .workspace
                .read(paths::MEMORY)
                .await
                .map(|d| d.content)
                .unwrap_or_default();

            let prompt = format!(
                "Review this daily activity log from {} and extract facts worth keeping.\n\
                 \n\
                 ## Daily Log\n\
                 {}\n\
                 \n\
                 ## Current USER.md\n\
                 {}\n\
                 \n\
                 ## Current MEMORY.md\n\
                 {}\n\
                 \n\
                 Output ONLY new facts not already captured above, one per line:\n\
                 USER: <fact about the user — preference, context, skill, identity>\n\
                 MEMORY: <important decision, outcome, or lesson worth remembering>\n\
                 \n\
                 Skip trivial exchanges. If nothing new is worth keeping, output exactly: NONE",
                date, log_content, user_md, memory_md
            );

            let request = CompletionRequest::new(vec![ChatMessage::user(&prompt)])
                .with_max_tokens(512)
                .with_temperature(0.1);

            let response = match self.llm.complete(request).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Fact extraction failed for {path}: {e}");
                    continue; // preserve the log, retry next tick
                }
            };

            let output = response.content.trim();

            if !output.is_empty() && output != "NONE" {
                let mut user_facts: Vec<&str> = Vec::new();
                let mut memory_facts: Vec<&str> = Vec::new();

                for line in output.lines() {
                    let line = line.trim();
                    if let Some(fact) = line.strip_prefix("USER: ") {
                        user_facts.push(fact);
                    } else if let Some(fact) = line.strip_prefix("MEMORY: ") {
                        memory_facts.push(fact);
                    }
                }

                if !user_facts.is_empty() {
                    let text = user_facts
                        .iter()
                        .map(|f| format!("- {f}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Err(e) = self.workspace.append(paths::USER, &text).await {
                        tracing::warn!("Failed to write user facts from {path}: {e}");
                    } else {
                        tracing::info!("Extracted {} user fact(s) from {}", user_facts.len(), path);
                    }
                }

                if !memory_facts.is_empty() {
                    let text = memory_facts
                        .iter()
                        .map(|f| format!("- {f}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Err(e) = self.workspace.append(paths::MEMORY, &text).await {
                        tracing::warn!("Failed to write memory facts from {path}: {e}");
                    } else {
                        tracing::info!(
                            "Extracted {} memory fact(s) from {}",
                            memory_facts.len(),
                            path
                        );
                    }
                }
            } else {
                tracing::debug!("No new facts to extract from {path}");
            }

            // Daily log is now consolidated — delete it.
            // prune_daily_logs() handles any that survive past the retention window.
            if let Err(e) = self.workspace.delete(&path).await {
                tracing::warn!("Failed to delete consolidated daily log {path}: {e}");
            }
        }
    }

    /// Prune audit log rows older than the configured retention window.
    ///
    /// Reads `audit_retention_days` from the user's settings (default 90).
    /// Runs silently — any errors are logged but never surface to callers.
    async fn prune_audit_log(&self) {
        let Some(ref db) = self.db else { return };

        // Read user-configured retention, fall back to 90 days.
        let retention_days = if let Some(ref user_id) = self.config.notify_user_id {
            match db.get_setting(user_id, "audit_retention_days").await {
                Ok(Some(v)) => v.as_u64().unwrap_or(DEFAULT_AUDIT_RETENTION_DAYS),
                Ok(None) => DEFAULT_AUDIT_RETENTION_DAYS,
                Err(e) => {
                    tracing::warn!("Failed to read audit_retention_days setting: {e}");
                    DEFAULT_AUDIT_RETENTION_DAYS
                }
            }
        } else {
            DEFAULT_AUDIT_RETENTION_DAYS
        };

        let older_than = Utc::now() - chrono::Duration::days(retention_days as i64);
        match db.prune_audit_log(older_than).await {
            Ok(0) => tracing::debug!("Audit log pruner: nothing to delete"),
            Ok(n) => tracing::info!(
                "Audit log pruner: deleted {n} rows older than {retention_days} days"
            ),
            Err(e) => tracing::warn!("Audit log pruner failed: {e}"),
        }
    }

    /// Prune daily log documents older than the configured retention window.
    ///
    /// Reads `audit_retention_days` from settings (same knob as audit log pruning,
    /// default 90). Runs silently — errors are logged but never surface to callers.
    async fn prune_daily_logs(&self) {
        let retention_days = if let Some(ref db) = self.db {
            if let Some(ref user_id) = self.config.notify_user_id {
                match db.get_setting(user_id, "audit_retention_days").await {
                    Ok(Some(v)) => v.as_u64().unwrap_or(DEFAULT_DAILY_LOG_RETENTION_DAYS),
                    Ok(None) => DEFAULT_DAILY_LOG_RETENTION_DAYS,
                    Err(e) => {
                        tracing::warn!("Failed to read audit_retention_days setting: {e}");
                        DEFAULT_DAILY_LOG_RETENTION_DAYS
                    }
                }
            } else {
                DEFAULT_DAILY_LOG_RETENTION_DAYS
            }
        } else {
            DEFAULT_DAILY_LOG_RETENTION_DAYS
        };

        match self.workspace.prune_old_daily_logs(retention_days).await {
            Ok(0) => tracing::debug!("Daily log pruner: nothing to delete"),
            Ok(n) => tracing::info!(
                "Daily log pruner: deleted {n} entries older than {retention_days} days"
            ),
            Err(e) => tracing::warn!("Daily log pruner failed: {e}"),
        }
    }

    /// Send a notification about heartbeat findings.
    async fn send_notification(&self, message: &str) {
        let Some(ref tx) = self.response_tx else {
            tracing::debug!("No response channel configured for heartbeat notifications");
            return;
        };

        let response = OutgoingResponse {
            content: format!("🔔 *Heartbeat Alert*\n\n{}", message),
            thread_id: None,
            metadata: serde_json::json!({
                "source": "heartbeat",
            }),
        };

        if let Err(e) = tx.send(response).await {
            tracing::error!("Failed to send heartbeat notification: {}", e);
        }
    }
}

/// Check if heartbeat content is effectively empty.
///
/// Returns true if the content contains only:
/// - Whitespace
/// - Markdown headers (lines starting with #)
/// - HTML comments (`<!-- ... -->`)
/// - Empty list items (`- [ ]`, `- [x]`, `-`, `*`)
///
/// This skips the LLM call when the user hasn't added real tasks yet,
/// saving API costs.
fn is_effectively_empty(content: &str) -> bool {
    let without_comments = strip_html_comments(content);

    without_comments.lines().all(|line| {
        let trimmed = line.trim();
        trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed == "- [ ]"
            || trimmed == "- [x]"
            || trimmed == "-"
            || trimmed == "*"
    })
}

/// Remove HTML comments from content.
fn strip_html_comments(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find("<!--") {
        result.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return result, // unclosed comment, treat rest as comment
        }
    }
    result.push_str(rest);
    result
}

/// Spawn the heartbeat runner as a background task.
///
/// Returns a handle that can be used to stop the runner.
pub fn spawn_heartbeat(
    config: HeartbeatConfig,
    workspace: Arc<Workspace>,
    llm: Arc<dyn LlmProvider>,
    db: Option<Arc<dyn Database>>,
    response_tx: Option<mpsc::Sender<OutgoingResponse>>,
) -> tokio::task::JoinHandle<()> {
    let mut runner = HeartbeatRunner::new(config, workspace, llm);
    if let Some(db) = db {
        runner = runner.with_database(db);
    }
    if let Some(tx) = response_tx {
        runner = runner.with_response_channel(tx);
    }

    tokio::spawn(async move {
        runner.run().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_config_defaults() {
        let config = HeartbeatConfig::default();
        assert!(config.enabled);
        assert_eq!(config.interval, Duration::from_secs(30 * 60));
        assert_eq!(config.max_failures, 3);
    }

    #[test]
    fn test_heartbeat_config_builders() {
        let config = HeartbeatConfig::default()
            .with_interval(Duration::from_secs(60))
            .with_notify("user1", "telegram");

        assert_eq!(config.interval, Duration::from_secs(60));
        assert_eq!(config.notify_user_id, Some("user1".to_string()));
        assert_eq!(config.notify_channel, Some("telegram".to_string()));

        let disabled = HeartbeatConfig::default().disabled();
        assert!(!disabled.enabled);
    }

    // ==================== strip_html_comments ====================

    #[test]
    fn test_strip_html_comments_no_comments() {
        assert_eq!(strip_html_comments("hello world"), "hello world");
    }

    #[test]
    fn test_strip_html_comments_single() {
        assert_eq!(
            strip_html_comments("before<!-- gone -->after"),
            "beforeafter"
        );
    }

    #[test]
    fn test_strip_html_comments_multiple() {
        let input = "a<!-- 1 -->b<!-- 2 -->c";
        assert_eq!(strip_html_comments(input), "abc");
    }

    #[test]
    fn test_strip_html_comments_multiline() {
        let input = "# Title\n<!-- multi\nline\ncomment -->\nreal content";
        assert_eq!(strip_html_comments(input), "# Title\n\nreal content");
    }

    #[test]
    fn test_strip_html_comments_unclosed() {
        let input = "before<!-- never closed";
        assert_eq!(strip_html_comments(input), "before");
    }

    // ==================== is_effectively_empty ====================

    #[test]
    fn test_effectively_empty_empty_string() {
        assert!(is_effectively_empty(""));
    }

    #[test]
    fn test_effectively_empty_whitespace() {
        assert!(is_effectively_empty("   \n\n  \n  "));
    }

    #[test]
    fn test_effectively_empty_headers_only() {
        assert!(is_effectively_empty("# Title\n## Subtitle\n### Section"));
    }

    #[test]
    fn test_effectively_empty_html_comments_only() {
        assert!(is_effectively_empty("<!-- this is a comment -->"));
    }

    #[test]
    fn test_effectively_empty_empty_checkboxes() {
        assert!(is_effectively_empty("# Checklist\n- [ ]\n- [x]"));
    }

    #[test]
    fn test_effectively_empty_bare_list_markers() {
        assert!(is_effectively_empty("-\n*\n-"));
    }

    #[test]
    fn test_effectively_empty_seeded_template() {
        let template = "\
# Heartbeat Checklist

<!-- Keep this file empty to skip heartbeat API calls.
     Add tasks below when you want the agent to check something periodically.

     Example:
     - [ ] Check for unread emails needing a reply
     - [ ] Review today's calendar for upcoming meetings
     - [ ] Check CI build status for main branch
-->";
        assert!(is_effectively_empty(template));
    }

    #[test]
    fn test_effectively_empty_real_checklist() {
        let content = "\
# Heartbeat Checklist

- [ ] Check for unread emails needing a reply
- [ ] Review today's calendar for upcoming meetings";
        assert!(!is_effectively_empty(content));
    }

    #[test]
    fn test_effectively_empty_mixed_real_and_headers() {
        let content = "# Title\n\nDo something important";
        assert!(!is_effectively_empty(content));
    }

    #[test]
    fn test_effectively_empty_comment_plus_real_content() {
        let content = "<!-- comment -->\nActual task here";
        assert!(!is_effectively_empty(content));
    }
}
