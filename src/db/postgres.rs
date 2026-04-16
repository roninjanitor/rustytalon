//! PostgreSQL backend for the Database trait.
//!
//! Delegates to the existing `Store` (history) and `Repository` (workspace)
//! implementations, avoiding SQL duplication.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::agent::BrokenTool;
use crate::agent::routine::{Routine, RoutineRun, RunStatus};
use crate::config::DatabaseConfig;
use crate::context::{ActionRecord, JobContext, JobState};
use crate::db::{
    AuditEvent, AuditEventCount, AuditLogRow, AuditQuery, CostDataPoint, Database, JobAnalytics,
    ToolAnalytics,
};
use crate::error::{DatabaseError, WorkspaceError};
use crate::history::{
    ConversationMessage, ConversationSummary, JobEventRecord, LlmCallRecord, SandboxJobRecord,
    SandboxJobSummary, SettingRow, Store,
};
use crate::llm::tracked::LlmCallStats;
use crate::workspace::{
    MemoryChunk, MemoryDocument, Repository, SearchConfig, SearchResult, WorkspaceEntry,
};

/// PostgreSQL database backend.
///
/// Wraps the existing `Store` (for history/conversations/jobs/routines/settings)
/// and `Repository` (for workspace documents/chunks/search) to implement the
/// unified `Database` trait.
pub struct PgBackend {
    store: Store,
    repo: Repository,
}

impl PgBackend {
    /// Create a new PostgreSQL backend from configuration.
    pub async fn new(config: &DatabaseConfig) -> Result<Self, DatabaseError> {
        let store = Store::new(config).await?;
        let repo = Repository::new(store.pool());
        Ok(Self { store, repo })
    }

    /// Get a clone of the connection pool.
    ///
    /// Useful for sharing with components that still need raw pool access.
    pub fn pool(&self) -> Pool {
        self.store.pool()
    }
}

#[async_trait]
impl Database for PgBackend {
    async fn run_migrations(&self) -> Result<(), DatabaseError> {
        self.store.run_migrations().await
    }

    // ==================== Conversations ====================

    async fn create_conversation(
        &self,
        channel: &str,
        user_id: &str,
        thread_id: Option<&str>,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .create_conversation(channel, user_id, thread_id)
            .await
    }

    async fn touch_conversation(&self, id: Uuid) -> Result<(), DatabaseError> {
        self.store.touch_conversation(id).await
    }

    async fn add_conversation_message(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .add_conversation_message(conversation_id, role, content)
            .await
    }

    async fn ensure_conversation(
        &self,
        id: Uuid,
        channel: &str,
        user_id: &str,
        thread_id: Option<&str>,
    ) -> Result<(), DatabaseError> {
        self.store
            .ensure_conversation(id, channel, user_id, thread_id)
            .await
    }

    async fn list_conversations_with_preview(
        &self,
        user_id: &str,
        channel: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ConversationSummary>, DatabaseError> {
        self.store
            .list_conversations_with_preview(user_id, channel, limit)
            .await
    }

    async fn get_or_create_assistant_conversation(
        &self,
        user_id: &str,
        channel: &str,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .get_or_create_assistant_conversation(user_id, channel)
            .await
    }

    async fn create_conversation_with_metadata(
        &self,
        channel: &str,
        user_id: &str,
        metadata: &serde_json::Value,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .create_conversation_with_metadata(channel, user_id, metadata)
            .await
    }

    async fn list_conversation_messages_paginated(
        &self,
        conversation_id: Uuid,
        before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<(Vec<ConversationMessage>, bool), DatabaseError> {
        self.store
            .list_conversation_messages_paginated(conversation_id, before, limit)
            .await
    }

    async fn update_conversation_metadata_field(
        &self,
        id: Uuid,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_conversation_metadata_field(id, key, value)
            .await
    }

    async fn get_conversation_metadata(
        &self,
        id: Uuid,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        self.store.get_conversation_metadata(id).await
    }

    async fn list_conversation_messages(
        &self,
        conversation_id: Uuid,
    ) -> Result<Vec<ConversationMessage>, DatabaseError> {
        self.store.list_conversation_messages(conversation_id).await
    }

    async fn conversation_belongs_to_user(
        &self,
        conversation_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        self.store
            .conversation_belongs_to_user(conversation_id, user_id)
            .await
    }

    // ==================== Jobs ====================

    async fn save_job(&self, ctx: &JobContext) -> Result<(), DatabaseError> {
        self.store.save_job(ctx).await
    }

    async fn get_job(&self, id: Uuid) -> Result<Option<JobContext>, DatabaseError> {
        self.store.get_job(id).await
    }

    async fn update_job_status(
        &self,
        id: Uuid,
        status: JobState,
        failure_reason: Option<&str>,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_job_status(id, status, failure_reason)
            .await
    }

    async fn mark_job_stuck(&self, id: Uuid) -> Result<(), DatabaseError> {
        self.store.mark_job_stuck(id).await
    }

    async fn get_stuck_jobs(&self) -> Result<Vec<Uuid>, DatabaseError> {
        self.store.get_stuck_jobs().await
    }

    // ==================== Actions ====================

    async fn save_action(&self, job_id: Uuid, action: &ActionRecord) -> Result<(), DatabaseError> {
        self.store.save_action(job_id, action).await
    }

    async fn get_job_actions(&self, job_id: Uuid) -> Result<Vec<ActionRecord>, DatabaseError> {
        self.store.get_job_actions(job_id).await
    }

    // ==================== LLM Calls ====================

    async fn record_llm_call(&self, record: &LlmCallRecord<'_>) -> Result<Uuid, DatabaseError> {
        self.store.record_llm_call(record).await
    }

    async fn get_llm_call_stats(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<LlmCallStats>, DatabaseError> {
        let conn = self.store.conn().await?;
        let (where_clause, params): (&str, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) =
            if let Some(ref s) = since {
                (
                    "WHERE created_at >= $1",
                    vec![s as &(dyn tokio_postgres::types::ToSql + Sync)],
                )
            } else {
                ("", vec![])
            };

        let sql = format!(
            r#"
            SELECT
                provider,
                model,
                COUNT(*)::bigint AS total_calls,
                COALESCE(SUM(input_tokens), 0)::bigint AS total_input_tokens,
                COALESCE(SUM(output_tokens), 0)::bigint AS total_output_tokens,
                COALESCE(SUM(cost), 0) AS total_cost,
                CASE WHEN COUNT(*) > 0
                    THEN COALESCE(SUM(cost), 0) / COUNT(*)
                    ELSE 0
                END AS avg_cost_per_call,
                AVG(latency_ms)::float8 AS avg_latency_ms,
                percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)
                    FILTER (WHERE latency_ms IS NOT NULL) AS p95_latency_ms,
                COALESCE(SUM(CASE WHEN cost_unknown THEN 1 ELSE 0 END), 0)::bigint
                    AS unconfigured_calls
            FROM llm_calls
            {where_clause}
            GROUP BY provider, model
            ORDER BY total_cost DESC
            "#
        );

        let rows = conn.query(&sql, &params).await?;

        let stats = rows
            .iter()
            .map(|row| LlmCallStats {
                provider: row.get("provider"),
                model: row.get("model"),
                total_calls: row.get("total_calls"),
                total_input_tokens: row.get("total_input_tokens"),
                total_output_tokens: row.get("total_output_tokens"),
                total_cost: row.get("total_cost"),
                avg_cost_per_call: row.get("avg_cost_per_call"),
                avg_latency_ms: row.get("avg_latency_ms"),
                p95_latency_ms: row.get("p95_latency_ms"),
                unconfigured_calls: row.get("unconfigured_calls"),
            })
            .collect();

        Ok(stats)
    }

    async fn get_job_analytics(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<JobAnalytics, DatabaseError> {
        let conn = self.store.conn().await?;
        let (where_clause, params): (&str, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) =
            if let Some(ref s) = since {
                (
                    "WHERE created_at >= $1",
                    vec![s as &(dyn tokio_postgres::types::ToSql + Sync)],
                )
            } else {
                ("", vec![])
            };

        let sql = format!(
            r#"
            SELECT
                COUNT(*)::bigint AS total_jobs,
                COUNT(*) FILTER (WHERE status = 'accepted')::bigint AS completed_jobs,
                COUNT(*) FILTER (WHERE status = 'failed')::bigint AS failed_jobs,
                COUNT(*) FILTER (WHERE status = 'in_progress')::bigint AS in_progress_jobs,
                COALESCE(
                    AVG(EXTRACT(EPOCH FROM (completed_at - started_at)))
                        FILTER (WHERE completed_at IS NOT NULL),
                    0.0
                ) AS avg_duration_secs,
                COALESCE(SUM(actual_cost), 0) AS total_cost
            FROM agent_jobs
            {where_clause}
            "#
        );

        let row = conn.query_one(&sql, &params).await?;
        let total: i64 = row.get("total_jobs");
        let completed: i64 = row.get("completed_jobs");
        let failed: i64 = row.get("failed_jobs");
        let in_progress: i64 = row.get("in_progress_jobs");
        let avg_duration: f64 = row.get("avg_duration_secs");
        let total_cost: Decimal = row
            .get::<_, Option<Decimal>>("total_cost")
            .unwrap_or_default();

        Ok(JobAnalytics {
            total_jobs: total,
            completed_jobs: completed,
            failed_jobs: failed,
            in_progress_jobs: in_progress,
            success_rate: if total > 0 {
                completed as f64 / total as f64
            } else {
                0.0
            },
            avg_duration_secs: avg_duration,
            total_cost_usd: total_cost.to_string(),
        })
    }

    async fn get_tool_analytics(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ToolAnalytics>, DatabaseError> {
        let conn = self.store.conn().await?;
        let (where_clause, params): (&str, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) =
            if let Some(ref s) = since {
                (
                    "WHERE created_at >= $1",
                    vec![s as &(dyn tokio_postgres::types::ToSql + Sync)],
                )
            } else {
                ("", vec![])
            };

        let sql = format!(
            r#"
            SELECT
                tool_name,
                COUNT(*)::bigint AS total_calls,
                COUNT(*) FILTER (WHERE success = true)::bigint AS successful_calls,
                COUNT(*) FILTER (WHERE success = false)::bigint AS failed_calls,
                COALESCE(AVG(duration_ms)::float8, 0.0) AS avg_duration_ms,
                COALESCE(SUM(cost), 0) AS total_cost
            FROM job_actions
            {where_clause}
            GROUP BY tool_name
            ORDER BY total_calls DESC
            "#
        );

        let rows = conn.query(&sql, &params).await?;

        let stats = rows
            .iter()
            .map(|row| {
                let total: i64 = row.get("total_calls");
                let successful: i64 = row.get("successful_calls");
                let total_cost: Decimal = row
                    .get::<_, Option<Decimal>>("total_cost")
                    .unwrap_or_default();
                ToolAnalytics {
                    tool_name: row.get("tool_name"),
                    total_calls: total,
                    successful_calls: successful,
                    failed_calls: row.get("failed_calls"),
                    success_rate: if total > 0 {
                        successful as f64 / total as f64
                    } else {
                        0.0
                    },
                    avg_duration_ms: row.get("avg_duration_ms"),
                    total_cost_usd: total_cost.to_string(),
                }
            })
            .collect();

        Ok(stats)
    }

    async fn get_cost_over_time(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<CostDataPoint>, DatabaseError> {
        let conn = self.store.conn().await?;
        let rows = conn
            .query(
                r#"
                SELECT
                    DATE(created_at AT TIME ZONE 'UTC')::text AS day,
                    COALESCE(SUM(cost), 0) AS daily_cost,
                    COUNT(*)::bigint AS call_count
                FROM llm_calls
                WHERE created_at >= $1
                GROUP BY DATE(created_at AT TIME ZONE 'UTC')
                ORDER BY day ASC
                "#,
                &[&since],
            )
            .await?;

        let points = rows
            .iter()
            .map(|row| {
                let cost: Decimal = row
                    .get::<_, Option<Decimal>>("daily_cost")
                    .unwrap_or_default();
                CostDataPoint {
                    day: row.get("day"),
                    cost_usd: cost.to_string(),
                    call_count: row.get("call_count"),
                }
            })
            .collect();

        Ok(points)
    }

    async fn get_conversation_token_stats(
        &self,
        conversation_id: Uuid,
    ) -> Result<crate::db::ConversationTokenStats, DatabaseError> {
        let conn = self.store.conn().await?;
        let row = conn
            .query_one(
                r#"
                SELECT
                    COALESCE(SUM(input_tokens), 0)::bigint AS total_input_tokens,
                    COALESCE(SUM(output_tokens), 0)::bigint AS total_output_tokens,
                    COALESCE(SUM(cost), 0) AS total_cost,
                    COUNT(*)::bigint AS call_count
                FROM llm_calls
                WHERE conversation_id = $1
                "#,
                &[&conversation_id],
            )
            .await?;
        Ok(crate::db::ConversationTokenStats {
            conversation_id,
            total_input_tokens: row.get("total_input_tokens"),
            total_output_tokens: row.get("total_output_tokens"),
            total_cost: row.get("total_cost"),
            call_count: row.get("call_count"),
        })
    }

    // ==================== Estimation Snapshots ====================

    async fn save_estimation_snapshot(
        &self,
        job_id: Uuid,
        category: &str,
        tool_names: &[String],
        estimated_cost: Decimal,
        estimated_time_secs: i32,
        estimated_value: Decimal,
    ) -> Result<Uuid, DatabaseError> {
        self.store
            .save_estimation_snapshot(
                job_id,
                category,
                tool_names,
                estimated_cost,
                estimated_time_secs,
                estimated_value,
            )
            .await
    }

    async fn update_estimation_actuals(
        &self,
        id: Uuid,
        actual_cost: Decimal,
        actual_time_secs: i32,
        actual_value: Option<Decimal>,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_estimation_actuals(id, actual_cost, actual_time_secs, actual_value)
            .await
    }

    // ==================== Sandbox Jobs ====================

    async fn save_sandbox_job(&self, job: &SandboxJobRecord) -> Result<(), DatabaseError> {
        self.store.save_sandbox_job(job).await
    }

    async fn get_sandbox_job(&self, id: Uuid) -> Result<Option<SandboxJobRecord>, DatabaseError> {
        self.store.get_sandbox_job(id).await
    }

    async fn list_sandbox_jobs(&self) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        self.store.list_sandbox_jobs().await
    }

    async fn update_sandbox_job_status(
        &self,
        id: Uuid,
        status: &str,
        success: Option<bool>,
        message: Option<&str>,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_sandbox_job_status(id, status, success, message, started_at, completed_at)
            .await
    }

    async fn cleanup_stale_sandbox_jobs(&self) -> Result<u64, DatabaseError> {
        self.store.cleanup_stale_sandbox_jobs().await
    }

    async fn sandbox_job_summary(&self) -> Result<SandboxJobSummary, DatabaseError> {
        self.store.sandbox_job_summary().await
    }

    async fn list_sandbox_jobs_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        self.store.list_sandbox_jobs_for_user(user_id).await
    }

    async fn sandbox_job_summary_for_user(
        &self,
        user_id: &str,
    ) -> Result<SandboxJobSummary, DatabaseError> {
        self.store.sandbox_job_summary_for_user(user_id).await
    }

    async fn sandbox_job_belongs_to_user(
        &self,
        job_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        self.store
            .sandbox_job_belongs_to_user(job_id, user_id)
            .await
    }

    async fn update_sandbox_job_mode(&self, id: Uuid, mode: &str) -> Result<(), DatabaseError> {
        self.store.update_sandbox_job_mode(id, mode).await
    }

    async fn get_sandbox_job_mode(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        self.store.get_sandbox_job_mode(id).await
    }

    // ==================== Job Events ====================

    async fn save_job_event(
        &self,
        job_id: Uuid,
        event_type: &str,
        data: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store.save_job_event(job_id, event_type, data).await
    }

    async fn list_job_events(&self, job_id: Uuid) -> Result<Vec<JobEventRecord>, DatabaseError> {
        self.store.list_job_events(job_id).await
    }

    // ==================== Routines ====================

    async fn create_routine(&self, routine: &Routine) -> Result<(), DatabaseError> {
        self.store.create_routine(routine).await
    }

    async fn get_routine(&self, id: Uuid) -> Result<Option<Routine>, DatabaseError> {
        self.store.get_routine(id).await
    }

    async fn get_routine_by_name(
        &self,
        user_id: &str,
        name: &str,
    ) -> Result<Option<Routine>, DatabaseError> {
        self.store.get_routine_by_name(user_id, name).await
    }

    async fn list_routines(&self, user_id: &str) -> Result<Vec<Routine>, DatabaseError> {
        self.store.list_routines(user_id).await
    }

    async fn list_event_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        self.store.list_event_routines().await
    }

    async fn list_due_cron_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        self.store.list_due_cron_routines().await
    }

    async fn update_routine(&self, routine: &Routine) -> Result<(), DatabaseError> {
        self.store.update_routine(routine).await
    }

    async fn update_routine_runtime(
        &self,
        id: Uuid,
        last_run_at: DateTime<Utc>,
        next_fire_at: Option<DateTime<Utc>>,
        run_count: u64,
        consecutive_failures: u32,
        state: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store
            .update_routine_runtime(
                id,
                last_run_at,
                next_fire_at,
                run_count,
                consecutive_failures,
                state,
            )
            .await
    }

    async fn delete_routine(&self, id: Uuid) -> Result<bool, DatabaseError> {
        self.store.delete_routine(id).await
    }

    // ==================== Routine Runs ====================

    async fn create_routine_run(&self, run: &RoutineRun) -> Result<(), DatabaseError> {
        self.store.create_routine_run(run).await
    }

    async fn complete_routine_run(
        &self,
        id: Uuid,
        status: RunStatus,
        result_summary: Option<&str>,
        tokens_used: Option<i32>,
    ) -> Result<(), DatabaseError> {
        self.store
            .complete_routine_run(id, status, result_summary, tokens_used)
            .await
    }

    async fn list_routine_runs(
        &self,
        routine_id: Uuid,
        limit: i64,
    ) -> Result<Vec<RoutineRun>, DatabaseError> {
        self.store.list_routine_runs(routine_id, limit).await
    }

    async fn count_running_routine_runs(&self, routine_id: Uuid) -> Result<i64, DatabaseError> {
        self.store.count_running_routine_runs(routine_id).await
    }

    // ==================== Tool Failures ====================

    async fn record_tool_failure(
        &self,
        tool_name: &str,
        error_message: &str,
    ) -> Result<(), DatabaseError> {
        self.store
            .record_tool_failure(tool_name, error_message)
            .await
    }

    async fn get_broken_tools(&self, threshold: i32) -> Result<Vec<BrokenTool>, DatabaseError> {
        self.store.get_broken_tools(threshold).await
    }

    async fn mark_tool_repaired(&self, tool_name: &str) -> Result<(), DatabaseError> {
        self.store.mark_tool_repaired(tool_name).await
    }

    async fn increment_repair_attempts(&self, tool_name: &str) -> Result<(), DatabaseError> {
        self.store.increment_repair_attempts(tool_name).await
    }

    // ==================== Settings ====================

    async fn get_setting(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        self.store.get_setting(user_id, key).await
    }

    async fn get_setting_full(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<SettingRow>, DatabaseError> {
        self.store.get_setting_full(user_id, key).await
    }

    async fn set_setting(
        &self,
        user_id: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        self.store.set_setting(user_id, key, value).await
    }

    async fn delete_setting(&self, user_id: &str, key: &str) -> Result<bool, DatabaseError> {
        self.store.delete_setting(user_id, key).await
    }

    async fn list_settings(&self, user_id: &str) -> Result<Vec<SettingRow>, DatabaseError> {
        self.store.list_settings(user_id).await
    }

    async fn get_all_settings(
        &self,
        user_id: &str,
    ) -> Result<HashMap<String, serde_json::Value>, DatabaseError> {
        self.store.get_all_settings(user_id).await
    }

    async fn set_all_settings(
        &self,
        user_id: &str,
        settings: &HashMap<String, serde_json::Value>,
    ) -> Result<(), DatabaseError> {
        self.store.set_all_settings(user_id, settings).await
    }

    async fn has_settings(&self, user_id: &str) -> Result<bool, DatabaseError> {
        self.store.has_settings(user_id).await
    }

    // ==================== Workspace: Documents ====================

    async fn get_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        self.repo
            .get_document_by_path(user_id, agent_id, path)
            .await
    }

    async fn get_document_by_id(&self, id: Uuid) -> Result<MemoryDocument, WorkspaceError> {
        self.repo.get_document_by_id(id).await
    }

    async fn get_or_create_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        self.repo
            .get_or_create_document_by_path(user_id, agent_id, path)
            .await
    }

    async fn update_document(&self, id: Uuid, content: &str) -> Result<(), WorkspaceError> {
        self.repo.update_document(id, content).await
    }

    async fn delete_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<(), WorkspaceError> {
        self.repo
            .delete_document_by_path(user_id, agent_id, path)
            .await
    }

    async fn list_directory(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        directory: &str,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        self.repo.list_directory(user_id, agent_id, directory).await
    }

    async fn list_all_paths(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
    ) -> Result<Vec<String>, WorkspaceError> {
        self.repo.list_all_paths(user_id, agent_id).await
    }

    async fn list_documents(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
    ) -> Result<Vec<MemoryDocument>, WorkspaceError> {
        self.repo.list_documents(user_id, agent_id).await
    }

    // ==================== Workspace: Chunks ====================

    async fn delete_chunks(&self, document_id: Uuid) -> Result<(), WorkspaceError> {
        self.repo.delete_chunks(document_id).await
    }

    async fn insert_chunk(
        &self,
        document_id: Uuid,
        chunk_index: i32,
        content: &str,
        embedding: Option<&[f32]>,
    ) -> Result<Uuid, WorkspaceError> {
        self.repo
            .insert_chunk(document_id, chunk_index, content, embedding)
            .await
    }

    async fn update_chunk_embedding(
        &self,
        chunk_id: Uuid,
        embedding: &[f32],
    ) -> Result<(), WorkspaceError> {
        self.repo.update_chunk_embedding(chunk_id, embedding).await
    }

    async fn get_chunks_without_embeddings(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<MemoryChunk>, WorkspaceError> {
        self.repo
            .get_chunks_without_embeddings(user_id, agent_id, limit)
            .await
    }

    // ==================== Workspace: Search ====================

    async fn hybrid_search(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        query: &str,
        embedding: Option<&[f32]>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>, WorkspaceError> {
        self.repo
            .hybrid_search(user_id, agent_id, query, embedding, config)
            .await
    }

    // ==================== Audit Log ====================

    async fn append_audit_event(&self, event: &AuditEvent<'_>) -> Result<(), DatabaseError> {
        let conn = self.store.conn().await?;
        let meta_str = event.metadata.and_then(|m| serde_json::to_value(m).ok());
        conn.execute(
            r#"INSERT INTO audit_log (
                event_type, user_id, session_id, job_id, actor,
                tool_name, input_hash, input_summary, outcome,
                error_msg, duration_ms, cost_usd, metadata
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)"#,
            &[
                &event.event_type,
                &event.user_id,
                &event.session_id,
                &event.job_id,
                &event.actor,
                &event.tool_name,
                &event.input_hash,
                &event.input_summary,
                &event.outcome,
                &event.error_msg,
                &event.duration_ms,
                &event.cost_usd,
                &meta_str,
            ],
        )
        .await?;
        Ok(())
    }

    async fn query_audit_log(&self, query: AuditQuery) -> Result<Vec<AuditLogRow>, DatabaseError> {
        let conn = self.store.conn().await?;
        let limit = query.limit.unwrap_or(100).min(500);

        // Build WHERE clauses dynamically. Owned values must outlive `params`.
        let since = query.since;
        let until = query.until;
        let user_id = query.user_id;
        let session_id = query.session_id;
        let event_type = query.event_type;
        let outcome = query.outcome;

        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
        let mut idx = 1usize;

        if let Some(ref v) = since {
            clauses.push(format!("created_at >= ${idx}"));
            params.push(v);
            idx += 1;
        }
        if let Some(ref v) = until {
            clauses.push(format!("created_at <= ${idx}"));
            params.push(v);
            idx += 1;
        }
        if let Some(ref v) = user_id {
            clauses.push(format!("user_id = ${idx}"));
            params.push(v);
            idx += 1;
        }
        if let Some(ref v) = session_id {
            clauses.push(format!("session_id = ${idx}"));
            params.push(v);
            idx += 1;
        }
        if let Some(ref v) = event_type {
            clauses.push(format!("event_type = ${idx}"));
            params.push(v);
            idx += 1;
        }
        if let Some(ref v) = outcome {
            clauses.push(format!("outcome = ${idx}"));
            params.push(v);
            idx += 1;
        }
        let _ = idx; // final value not needed after loop

        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };

        let sql = format!(
            r#"SELECT id, created_at, event_type, user_id, session_id, job_id,
                      actor, tool_name, input_hash, input_summary, outcome,
                      error_msg, duration_ms, cost_usd, metadata
               FROM audit_log
               {where_sql}
               ORDER BY created_at DESC
               LIMIT {limit}"#
        );

        let rows = conn.query(&sql, &params).await?;
        let result = rows
            .iter()
            .map(|row| AuditLogRow {
                id: row.get("id"),
                created_at: row.get("created_at"),
                event_type: row.get("event_type"),
                user_id: row.get("user_id"),
                session_id: row.get("session_id"),
                job_id: row.get("job_id"),
                actor: row.get("actor"),
                tool_name: row.get("tool_name"),
                input_hash: row.get("input_hash"),
                input_summary: row.get("input_summary"),
                outcome: row.get("outcome"),
                error_msg: row.get("error_msg"),
                duration_ms: row.get("duration_ms"),
                cost_usd: row.get("cost_usd"),
                metadata: row.get("metadata"),
            })
            .collect();
        Ok(result)
    }

    async fn audit_log_summary(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<AuditEventCount>, DatabaseError> {
        let conn = self.store.conn().await?;
        let (where_clause, params): (&str, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) =
            if let Some(ref s) = since {
                (
                    "WHERE created_at >= $1",
                    vec![s as &(dyn tokio_postgres::types::ToSql + Sync)],
                )
            } else {
                ("", vec![])
            };

        let sql = format!(
            r#"SELECT event_type, COUNT(*)::bigint AS count
               FROM audit_log
               {where_clause}
               GROUP BY event_type
               ORDER BY count DESC"#
        );

        let rows = conn.query(&sql, &params).await?;
        let result = rows
            .iter()
            .map(|row| AuditEventCount {
                event_type: row.get("event_type"),
                count: row.get("count"),
            })
            .collect();
        Ok(result)
    }

    async fn prune_audit_log(&self, older_than: DateTime<Utc>) -> Result<u64, DatabaseError> {
        let conn = self.store.conn().await?;
        let n = conn
            .execute(
                "DELETE FROM audit_log WHERE created_at < $1",
                &[&older_than],
            )
            .await?;
        Ok(n)
    }
}
