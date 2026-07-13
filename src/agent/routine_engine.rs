//! Routine execution engine.
//!
//! Handles loading routines, checking triggers, enforcing guardrails,
//! and executing both lightweight (single LLM call) and full-job routines.
//!
//! The engine runs two independent loops:
//! - A **cron ticker** that polls the DB every N seconds for due cron routines
//! - An **event matcher** called synchronously from the agent main loop
//!
//! Lightweight routines execute inline (single LLM call, no tools).
//! Full-job routines run their own bounded multi-turn tool-calling loop
//! (`execute_full_job`), independent of the user-facing `Scheduler`/`Worker`
//! job-state machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use crate::agent::routine::{
    NotifyConfig, Routine, RoutineAction, RoutineRun, RunStatus, Trigger, next_cron_fire,
};
use crate::channels::{IncomingMessage, OutgoingResponse};
use crate::config::RoutineConfig;
use crate::context::JobContext;
use crate::db::Database;
use crate::llm::{
    ChatMessage, CompletionRequest, FinishReason, LlmProvider, Reasoning, ReasoningContext,
    RespondResult,
};
use crate::safety::SafetyLayer;
use crate::tools::ToolRegistry;
use crate::workspace::Workspace;

/// The routine execution engine.
pub struct RoutineEngine {
    config: RoutineConfig,
    store: Arc<dyn Database>,
    llm: Arc<dyn LlmProvider>,
    workspace: Arc<Workspace>,
    tools: Arc<ToolRegistry>,
    safety: Arc<SafetyLayer>,
    /// Sender for notifications (routed to channel manager).
    notify_tx: mpsc::Sender<OutgoingResponse>,
    /// Currently running routine count (across all routines).
    running_count: Arc<AtomicUsize>,
    /// Compiled event regex cache: routine_id -> compiled regex.
    event_cache: Arc<RwLock<Vec<(Uuid, Routine, Regex)>>>,
}

impl RoutineEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: RoutineConfig,
        store: Arc<dyn Database>,
        llm: Arc<dyn LlmProvider>,
        workspace: Arc<Workspace>,
        tools: Arc<ToolRegistry>,
        safety: Arc<SafetyLayer>,
        notify_tx: mpsc::Sender<OutgoingResponse>,
    ) -> Self {
        Self {
            config,
            store,
            llm,
            workspace,
            tools,
            safety,
            notify_tx,
            running_count: Arc::new(AtomicUsize::new(0)),
            event_cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Refresh the in-memory event trigger cache from DB.
    pub async fn refresh_event_cache(&self) {
        match self.store.list_event_routines().await {
            Ok(routines) => {
                let mut cache = Vec::new();
                for routine in routines {
                    if let Trigger::Event { ref pattern, .. } = routine.trigger {
                        match Regex::new(pattern) {
                            Ok(re) => cache.push((routine.id, routine.clone(), re)),
                            Err(e) => {
                                tracing::warn!(
                                    routine = %routine.name,
                                    "Invalid event regex '{}': {}",
                                    pattern, e
                                );
                            }
                        }
                    }
                }
                let count = cache.len();
                *self.event_cache.write().await = cache;
                tracing::debug!("Refreshed event cache: {} routines", count);
            }
            Err(e) => {
                tracing::error!("Failed to refresh event cache: {}", e);
            }
        }
    }

    /// Check incoming message against event triggers. Returns number of routines fired.
    ///
    /// Called synchronously from the main loop after handle_message(). The actual
    /// execution is spawned async so this returns quickly.
    pub async fn check_event_triggers(&self, message: &IncomingMessage) -> usize {
        let cache = self.event_cache.read().await;
        let mut fired = 0;

        for (_, routine, re) in cache.iter() {
            // Channel filter
            if let Trigger::Event {
                channel: Some(ch), ..
            } = &routine.trigger
                && ch != &message.channel
            {
                continue;
            }

            // Regex match
            if !re.is_match(&message.content) {
                continue;
            }

            // Cooldown check
            if !self.check_cooldown(routine) {
                tracing::debug!(routine = %routine.name, "Skipped: cooldown active");
                continue;
            }

            // Concurrent run check
            if !self.check_concurrent(routine).await {
                tracing::debug!(routine = %routine.name, "Skipped: max concurrent reached");
                continue;
            }

            // Global capacity check
            if self.running_count.load(Ordering::Relaxed) >= self.config.max_concurrent_routines {
                tracing::warn!(routine = %routine.name, "Skipped: global max concurrent reached");
                continue;
            }

            let detail = truncate(&message.content, 200);
            self.spawn_fire(routine.clone(), "event", Some(detail));
            fired += 1;
        }

        fired
    }

    /// Check all due cron routines and fire them. Called by the cron ticker.
    pub async fn check_cron_triggers(&self) {
        let routines = match self.store.list_due_cron_routines().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to load due cron routines: {}", e);
                return;
            }
        };

        for routine in routines {
            if self.running_count.load(Ordering::Relaxed) >= self.config.max_concurrent_routines {
                tracing::warn!("Global max concurrent routines reached, skipping remaining");
                break;
            }

            if !self.check_cooldown(&routine) {
                continue;
            }

            if !self.check_concurrent(&routine).await {
                continue;
            }

            let detail = if let Trigger::Cron { ref schedule } = routine.trigger {
                Some(schedule.clone())
            } else {
                None
            };

            self.spawn_fire(routine, "cron", detail);
        }
    }

    /// Fire a routine manually (from tool call or CLI).
    pub async fn fire_manual(&self, routine_id: Uuid) -> Result<Uuid, String> {
        let routine = self
            .store
            .get_routine(routine_id)
            .await
            .map_err(|e| format!("DB error: {e}"))?
            .ok_or_else(|| format!("routine {routine_id} not found"))?;

        if !routine.enabled {
            return Err(format!("routine '{}' is disabled", routine.name));
        }

        if !self.check_concurrent(&routine).await {
            return Err(format!(
                "routine '{}' already at max concurrent runs",
                routine.name
            ));
        }

        let run_id = Uuid::new_v4();
        let run = RoutineRun {
            id: run_id,
            routine_id: routine.id,
            trigger_type: "manual".to_string(),
            trigger_detail: None,
            started_at: Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            result_summary: None,
            tokens_used: None,
            job_id: None,
            created_at: Utc::now(),
        };

        if let Err(e) = self.store.create_routine_run(&run).await {
            return Err(format!("failed to create run record: {e}"));
        }

        // Execute inline for manual triggers (caller wants to wait)
        let engine = EngineContext {
            store: self.store.clone(),
            llm: self.llm.clone(),
            workspace: self.workspace.clone(),
            tools: self.tools.clone(),
            safety: self.safety.clone(),
            notify_tx: self.notify_tx.clone(),
            running_count: self.running_count.clone(),
        };

        tokio::spawn(async move {
            execute_routine(engine, routine, run).await;
        });

        Ok(run_id)
    }

    /// Spawn a fire in a background task.
    fn spawn_fire(&self, routine: Routine, trigger_type: &str, trigger_detail: Option<String>) {
        let run = RoutineRun {
            id: Uuid::new_v4(),
            routine_id: routine.id,
            trigger_type: trigger_type.to_string(),
            trigger_detail,
            started_at: Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            result_summary: None,
            tokens_used: None,
            job_id: None,
            created_at: Utc::now(),
        };

        let engine = EngineContext {
            store: self.store.clone(),
            llm: self.llm.clone(),
            workspace: self.workspace.clone(),
            tools: self.tools.clone(),
            safety: self.safety.clone(),
            notify_tx: self.notify_tx.clone(),
            running_count: self.running_count.clone(),
        };

        // Record the run in DB, then spawn execution
        let store = self.store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.create_routine_run(&run).await {
                tracing::error!(routine = %routine.name, "Failed to record run: {}", e);
                return;
            }
            execute_routine(engine, routine, run).await;
        });
    }

    fn check_cooldown(&self, routine: &Routine) -> bool {
        if let Some(last_run) = routine.last_run_at {
            let elapsed = Utc::now().signed_duration_since(last_run);
            let cooldown = chrono::Duration::from_std(routine.guardrails.cooldown)
                .unwrap_or(chrono::Duration::seconds(300));
            if elapsed < cooldown {
                return false;
            }
        }
        true
    }

    async fn check_concurrent(&self, routine: &Routine) -> bool {
        match self.store.count_running_routine_runs(routine.id).await {
            Ok(count) => count < routine.guardrails.max_concurrent as i64,
            Err(e) => {
                tracing::error!(
                    routine = %routine.name,
                    "Failed to check concurrent runs: {}", e
                );
                false
            }
        }
    }
}

/// Shared context passed to the execution function.
struct EngineContext {
    store: Arc<dyn Database>,
    llm: Arc<dyn LlmProvider>,
    workspace: Arc<Workspace>,
    tools: Arc<ToolRegistry>,
    safety: Arc<SafetyLayer>,
    notify_tx: mpsc::Sender<OutgoingResponse>,
    running_count: Arc<AtomicUsize>,
}

/// Execute a routine run. Handles both lightweight and full_job modes.
async fn execute_routine(ctx: EngineContext, routine: Routine, run: RoutineRun) {
    // Increment running count (atomic: survives panics in the execution below)
    ctx.running_count.fetch_add(1, Ordering::Relaxed);

    let result = match &routine.action {
        RoutineAction::Lightweight {
            prompt,
            context_paths,
            max_tokens,
        } => execute_lightweight(&ctx, &routine, prompt, context_paths, *max_tokens).await,
        RoutineAction::FullJob {
            title,
            description,
            max_iterations,
        } => execute_full_job(&ctx, &routine, title, description, *max_iterations).await,
    };

    // Decrement running count
    ctx.running_count.fetch_sub(1, Ordering::Relaxed);

    // Process result
    let (status, summary, tokens) = match result {
        Ok(execution) => execution,
        Err(e) => {
            tracing::error!(routine = %routine.name, "Execution failed: {}", e);
            (RunStatus::Failed, Some(e), None)
        }
    };

    // Complete the run record
    if let Err(e) = ctx
        .store
        .complete_routine_run(run.id, status, summary.as_deref(), tokens)
        .await
    {
        tracing::error!(routine = %routine.name, "Failed to complete run record: {}", e);
    }

    // Update routine runtime state
    let now = Utc::now();
    let next_fire = if let Trigger::Cron { ref schedule } = routine.trigger {
        next_cron_fire(schedule).unwrap_or(None)
    } else {
        None
    };

    let new_failures = if status == RunStatus::Failed {
        routine.consecutive_failures + 1
    } else {
        0
    };

    if let Err(e) = ctx
        .store
        .update_routine_runtime(
            routine.id,
            now,
            next_fire,
            routine.run_count + 1,
            new_failures,
            &routine.state,
        )
        .await
    {
        tracing::error!(routine = %routine.name, "Failed to update runtime state: {}", e);
    }

    // Send notifications based on config
    send_notification(
        &ctx.notify_tx,
        &routine.notify,
        &routine.name,
        status,
        summary.as_deref(),
    )
    .await;
}

/// Execute a lightweight routine (single LLM call).
async fn execute_lightweight(
    ctx: &EngineContext,
    routine: &Routine,
    prompt: &str,
    context_paths: &[String],
    max_tokens: u32,
) -> Result<(RunStatus, Option<String>, Option<i32>), String> {
    // Load context from workspace
    let mut context_parts = Vec::new();
    for path in context_paths {
        match ctx.workspace.read(path).await {
            Ok(doc) => {
                context_parts.push(format!("## {}\n\n{}", path, doc.content));
            }
            Err(e) => {
                tracing::debug!(
                    routine = %routine.name,
                    "Failed to read context path {}: {}", path, e
                );
            }
        }
    }

    // Load routine state from workspace
    let state_path = format!("routines/{}/state.md", routine.name);
    let state_content = match ctx.workspace.read(&state_path).await {
        Ok(doc) => Some(doc.content),
        Err(_) => None,
    };

    // Build the prompt
    let mut full_prompt = String::new();
    full_prompt.push_str(prompt);

    if !context_parts.is_empty() {
        full_prompt.push_str("\n\n---\n\n# Context\n\n");
        full_prompt.push_str(&context_parts.join("\n\n"));
    }

    if let Some(state) = &state_content {
        full_prompt.push_str("\n\n---\n\n# Previous State\n\n");
        full_prompt.push_str(state);
    }

    full_prompt.push_str(
        "\n\n---\n\nIf nothing needs attention, reply EXACTLY with: ROUTINE_OK\n\
         If something needs attention, provide a concise summary.",
    );

    // Get system prompt
    let system_prompt = match ctx.workspace.system_prompt().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(routine = %routine.name, "Failed to get system prompt: {}", e);
            String::new()
        }
    };

    let messages = if system_prompt.is_empty() {
        vec![ChatMessage::user(&full_prompt)]
    } else {
        vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(&full_prompt),
        ]
    };

    // Determine max_tokens from model metadata with fallback
    let effective_max_tokens = match ctx.llm.model_metadata().await {
        Ok(meta) => {
            let from_api = meta.context_length.map(|ctx| ctx / 2).unwrap_or(max_tokens);
            from_api.max(max_tokens)
        }
        Err(_) => max_tokens,
    };

    let request = CompletionRequest::new(messages)
        .with_max_tokens(effective_max_tokens)
        .with_temperature(0.3);

    let response = ctx
        .llm
        .complete(request)
        .await
        .map_err(|e| format!("LLM call failed: {e}"))?;

    let content = response.content.trim();
    let tokens_used = Some((response.input_tokens + response.output_tokens) as i32);

    // Empty content guard (same as heartbeat)
    if content.is_empty() {
        return if response.finish_reason == FinishReason::Length {
            Err(
                "LLM response truncated (finish_reason=length) with no content. \
                 Model may have exhausted token budget on reasoning."
                    .to_string(),
            )
        } else {
            Err("LLM returned empty content.".to_string())
        };
    }

    // Check for the "nothing to do" sentinel
    if content == "ROUTINE_OK" || content.contains("ROUTINE_OK") {
        return Ok((RunStatus::Ok, None, tokens_used));
    }

    Ok((RunStatus::Attention, Some(content.to_string()), tokens_used))
}

/// Execute a `RoutineAction::FullJob` — a bounded multi-turn tool-calling run.
///
/// This gives routines the same `Reasoning`/`ToolRegistry`/`SafetyLayer` primitives
/// `Worker` uses for user-facing jobs, but deliberately skips the parts of
/// `Worker` that only make sense for an interactive, ContextManager-tracked job:
/// no `JobState` transitions, no per-action DB persistence, no audit events. The
/// run terminates as soon as the model responds with plain text (no further tool
/// calls) rather than looping indefinitely waiting for a completion phrase —
/// routine runs are bounded background tasks, not open-ended agent sessions.
async fn execute_full_job(
    ctx: &EngineContext,
    routine: &Routine,
    title: &str,
    description: &str,
    max_iterations: u32,
) -> Result<(RunStatus, Option<String>, Option<i32>), String> {
    let job_ctx = JobContext::with_user(routine.user_id.clone(), title, description);

    let system_prompt = match ctx.workspace.system_prompt().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(routine = %routine.name, "Failed to get system prompt: {}", e);
            String::new()
        }
    };

    let reasoning =
        Reasoning::new(ctx.llm.clone(), ctx.safety.clone()).with_system_prompt(system_prompt);

    let initial_prompt = format!(
        "{description}\n\n---\n\nUse the available tools as needed to complete this task. \
         Once finished, if nothing needs the user's attention, reply EXACTLY with: ROUTINE_OK\n\
         If something needs attention, reply with a concise summary instead of calling more tools."
    );

    let mut reason_ctx = ReasoningContext::new().with_message(ChatMessage::user(initial_prompt));

    let mut total_tokens: i64 = 0;
    let mut iteration: u32 = 0;

    loop {
        iteration += 1;
        if iteration > max_iterations {
            return Ok((
                RunStatus::Failed,
                Some(format!(
                    "Exceeded max_iterations ({max_iterations}) without finishing"
                )),
                Some(total_tokens as i32),
            ));
        }

        reason_ctx.available_tools = ctx.tools.tool_definitions().await;

        let respond_output = reasoning
            .respond_with_tools(&reason_ctx)
            .await
            .map_err(|e| format!("LLM call failed: {e}"))?;
        total_tokens += respond_output.usage.total() as i64;

        match respond_output.result {
            RespondResult::Text(response) => {
                let content = response.trim();
                if content.is_empty() {
                    return Err("LLM returned empty content.".to_string());
                }
                if content == "ROUTINE_OK" || content.contains("ROUTINE_OK") {
                    return Ok((RunStatus::Ok, None, Some(total_tokens as i32)));
                }
                return Ok((
                    RunStatus::Attention,
                    Some(content.to_string()),
                    Some(total_tokens as i32),
                ));
            }
            RespondResult::ToolCalls {
                tool_calls,
                content,
            } => {
                reason_ctx
                    .messages
                    .push(ChatMessage::assistant_with_tool_calls(
                        content,
                        tool_calls.clone(),
                    ));

                for tc in tool_calls {
                    let message = match execute_tool_for_routine(
                        ctx,
                        &job_ctx,
                        &tc.name,
                        &tc.arguments,
                    )
                    .await
                    {
                        Ok(wrapped) => wrapped,
                        Err(e) => format!("Error: {e}"),
                    };
                    reason_ctx
                        .messages
                        .push(ChatMessage::tool_result(&tc.id, &tc.name, message));
                }
            }
        }
    }
}

/// Execute a single tool call on behalf of a `FullJob` routine.
///
/// Mirrors the safety-relevant checks in `Worker::execute_tool_inner` (approval
/// gating, parameter validation, per-tool timeout, output sanitization) but
/// omits the job-state-machine and DB action/audit bookkeeping that only apply
/// to `ContextManager`-tracked user jobs — routine runs aren't jobs, and their
/// outcome is already recorded via `RoutineRun`.
async fn execute_tool_for_routine(
    ctx: &EngineContext,
    job_ctx: &JobContext,
    tool_name: &str,
    params: &serde_json::Value,
) -> Result<String, String> {
    let tool = ctx
        .tools
        .get(tool_name)
        .await
        .ok_or_else(|| format!("Tool '{tool_name}' not found"))?;

    // Tools requiring approval are blocked: there's no human in the loop to
    // approve them during an unattended routine run, same rule Worker applies
    // to autonomous jobs.
    if tool.requires_approval() {
        return Err(format!(
            "Tool '{tool_name}' requires approval and cannot be used in an unattended routine"
        ));
    }

    let validation = ctx.safety.validator().validate_tool_params(params);
    if !validation.is_valid {
        let details = validation
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.field, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "Invalid parameters for tool '{tool_name}': {details}"
        ));
    }

    let tool_timeout = tool.execution_timeout();
    let result = tokio::time::timeout(tool_timeout, tool.execute(params.clone(), job_ctx)).await;

    match result {
        Ok(Ok(output)) => {
            let result_str = serde_json::to_string_pretty(&output.result)
                .unwrap_or_else(|_| "<serialize error>".to_string());
            let sanitized = ctx.safety.sanitize_tool_output(tool_name, &result_str);
            Ok(ctx
                .safety
                .wrap_for_llm(tool_name, &sanitized.content, sanitized.was_modified))
        }
        Ok(Err(e)) => Err(format!("Tool '{tool_name}' failed: {e}")),
        Err(_) => Err(format!(
            "Tool '{tool_name}' timed out after {tool_timeout:?}"
        )),
    }
}

/// Send a notification based on the routine's notify config and run status.
async fn send_notification(
    tx: &mpsc::Sender<OutgoingResponse>,
    notify: &NotifyConfig,
    routine_name: &str,
    status: RunStatus,
    summary: Option<&str>,
) {
    let should_notify = match status {
        RunStatus::Ok => notify.on_success,
        RunStatus::Attention => notify.on_attention,
        RunStatus::Failed => notify.on_failure,
        RunStatus::Running => false,
    };

    if !should_notify {
        return;
    }

    let icon = match status {
        RunStatus::Ok => "✅",
        RunStatus::Attention => "🔔",
        RunStatus::Failed => "❌",
        RunStatus::Running => "⏳",
    };

    let message = match summary {
        Some(s) => format!("{} *Routine '{}'*: {}\n\n{}", icon, routine_name, status, s),
        None => format!("{} *Routine '{}'*: {}", icon, routine_name, status),
    };

    let response = OutgoingResponse {
        content: message,
        thread_id: None,
        metadata: serde_json::json!({
            "source": "routine",
            "routine_name": routine_name,
            "status": status.to_string(),
            "notify_user": notify.user,
            "notify_channel": notify.channel,
        }),
    };

    if let Err(e) = tx.send(response).await {
        tracing::error!(routine = %routine_name, "Failed to send notification: {}", e);
    }
}

/// Spawn the cron ticker background task.
pub fn spawn_cron_ticker(
    engine: Arc<RoutineEngine>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip immediate first tick
        ticker.tick().await;

        loop {
            ticker.tick().await;
            engine.check_cron_triggers().await;
        }
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = crate::util::floor_char_boundary(s, max);
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::{EngineContext, execute_tool_for_routine, send_notification};
    use crate::agent::routine::{NotifyConfig, RunStatus};
    use crate::channels::OutgoingResponse;
    use crate::config::SafetyConfig;
    use crate::context::JobContext;
    use crate::db::Database;
    use crate::safety::SafetyLayer;
    use crate::tools::ToolRegistry;
    use crate::tools::{Tool, ToolError, ToolOutput};

    #[test]
    fn test_notification_gating() {
        let config = NotifyConfig {
            on_success: false,
            on_failure: true,
            on_attention: true,
            ..Default::default()
        };

        // on_success = false means Ok status should not notify
        assert!(!config.on_success);
        assert!(config.on_failure);
        assert!(config.on_attention);
    }

    #[test]
    fn test_run_status_icons() {
        // Just verify the mapping doesn't panic
        for status in [
            RunStatus::Ok,
            RunStatus::Attention,
            RunStatus::Failed,
            RunStatus::Running,
        ] {
            let _ = status.to_string();
        }
    }

    #[tokio::test]
    async fn test_send_notification_embeds_notify_user_and_channel() {
        let (tx, mut rx) = mpsc::channel::<OutgoingResponse>(4);

        let notify = NotifyConfig {
            channel: Some("discord".to_string()),
            user: "owner123".to_string(),
            on_attention: true,
            on_failure: true,
            on_success: false,
        };

        send_notification(
            &tx,
            &notify,
            "morning_alert",
            RunStatus::Attention,
            Some("Time to wake up!"),
        )
        .await;

        let response = rx.recv().await.expect("should receive notification");
        assert!(response.content.contains("morning_alert"));
        assert!(response.content.contains("Time to wake up!"));

        // Verify metadata contains routing fields
        assert_eq!(
            response
                .metadata
                .get("notify_user")
                .and_then(|v| v.as_str()),
            Some("owner123")
        );
        assert_eq!(
            response
                .metadata
                .get("notify_channel")
                .and_then(|v| v.as_str()),
            Some("discord")
        );
        assert_eq!(
            response.metadata.get("source").and_then(|v| v.as_str()),
            Some("routine")
        );
    }

    #[tokio::test]
    async fn test_send_notification_null_channel_when_none() {
        let (tx, mut rx) = mpsc::channel::<OutgoingResponse>(4);

        let notify = NotifyConfig {
            channel: None,
            user: "default".to_string(),
            on_failure: true,
            ..Default::default()
        };

        send_notification(
            &tx,
            &notify,
            "test_routine",
            RunStatus::Failed,
            Some("error"),
        )
        .await;

        let response = rx.recv().await.expect("should receive notification");
        assert_eq!(
            response
                .metadata
                .get("notify_user")
                .and_then(|v| v.as_str()),
            Some("default")
        );
        // When channel is None, it should be serialized as JSON null
        assert!(
            response
                .metadata
                .get("notify_channel")
                .expect("notify_channel key should exist")
                .is_null()
        );
    }

    #[tokio::test]
    async fn test_send_notification_skipped_when_status_not_configured() {
        let (tx, mut rx) = mpsc::channel::<OutgoingResponse>(4);

        let notify = NotifyConfig {
            on_success: false, // Ok status should NOT notify
            on_failure: true,
            on_attention: true,
            ..Default::default()
        };

        // Status is Ok, on_success is false → should not send
        send_notification(&tx, &notify, "quiet_routine", RunStatus::Ok, None).await;

        // Channel should be empty
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_send_notification_failure_includes_error_icon() {
        let (tx, mut rx) = mpsc::channel::<OutgoingResponse>(4);

        let notify = NotifyConfig {
            on_failure: true,
            ..Default::default()
        };

        send_notification(
            &tx,
            &notify,
            "broken_routine",
            RunStatus::Failed,
            Some("connection refused"),
        )
        .await;

        let response = rx.recv().await.expect("should receive notification");
        assert!(response.content.contains("❌"));
        assert!(response.content.contains("connection refused"));
    }

    // --- execute_tool_for_routine ---

    struct AlwaysApproveTool;

    #[async_trait::async_trait]
    impl Tool for AlwaysApproveTool {
        fn name(&self) -> &str {
            "echo_ok"
        }
        fn description(&self) -> &str {
            "test tool that echoes its input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            params: serde_json::Value,
            _ctx: &JobContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text(
                format!("got: {params}"),
                Duration::from_millis(1),
            ))
        }
    }

    struct RequiresApprovalTool;

    #[async_trait::async_trait]
    impl Tool for RequiresApprovalTool {
        fn name(&self) -> &str {
            "dangerous_tool"
        }
        fn description(&self) -> &str {
            "test tool that requires human approval"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn requires_approval(&self) -> bool {
            true
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: &JobContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text(
                "should never run",
                Duration::from_millis(1),
            ))
        }
    }

    fn test_engine_ctx(tools: Arc<ToolRegistry>) -> EngineContext {
        let (notify_tx, _rx) = mpsc::channel::<OutgoingResponse>(4);
        let store: Arc<dyn Database> = crate::db::test_utils::MockDatabase::new();
        let workspace = Arc::new(crate::workspace::Workspace::new_with_db(
            "routine-test-user",
            store.clone(),
        ));
        EngineContext {
            store,
            llm: Arc::new(crate::llm::test_utils::MockProvider::succeeding(
                "mock", "unused",
            )),
            workspace,
            tools,
            safety: Arc::new(SafetyLayer::new(&SafetyConfig {
                max_output_length: 100_000,
                injection_check_enabled: true,
            })),
            notify_tx,
            running_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[tokio::test]
    async fn test_execute_tool_for_routine_runs_normal_tool() {
        let tools = Arc::new(ToolRegistry::new());
        tools.register(Arc::new(AlwaysApproveTool)).await;
        let ctx = test_engine_ctx(tools);
        let job_ctx = JobContext::with_user("routine-user", "test", "test");

        let result =
            execute_tool_for_routine(&ctx, &job_ctx, "echo_ok", &serde_json::json!({"a": 1}))
                .await
                .expect("tool call should succeed");

        assert!(result.contains("got:"));
        assert!(result.contains("name=\"echo_ok\""));
    }

    #[tokio::test]
    async fn test_execute_tool_for_routine_blocks_approval_gated_tool() {
        let tools = Arc::new(ToolRegistry::new());
        tools.register(Arc::new(RequiresApprovalTool)).await;
        let ctx = test_engine_ctx(tools);
        let job_ctx = JobContext::with_user("routine-user", "test", "test");

        let err =
            execute_tool_for_routine(&ctx, &job_ctx, "dangerous_tool", &serde_json::json!({}))
                .await
                .expect_err("approval-gated tool must be rejected in an unattended routine");

        assert!(err.contains("requires approval"));
    }

    #[tokio::test]
    async fn test_execute_tool_for_routine_unknown_tool() {
        let tools = Arc::new(ToolRegistry::new());
        let ctx = test_engine_ctx(tools);
        let job_ctx = JobContext::with_user("routine-user", "test", "test");

        let err =
            execute_tool_for_routine(&ctx, &job_ctx, "does_not_exist", &serde_json::json!({}))
                .await
                .expect_err("unknown tool must error");

        assert!(err.contains("not found"));
    }
}
