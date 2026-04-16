//! libSQL/Turso backend for the Database trait.
//!
//! Provides an embedded SQLite-compatible database using Turso's libSQL fork.
//! Supports three modes:
//! - Local embedded (file-based, no server needed)
//! - Turso cloud with embedded replica (sync to cloud)
//! - In-memory (for testing)

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use libsql::{Connection, Database as LibSqlDatabase, params};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::agent::BrokenTool;
use crate::agent::routine::{
    NotifyConfig, Routine, RoutineAction, RoutineGuardrails, RoutineRun, RunStatus, Trigger,
};
use crate::context::{ActionRecord, JobContext, JobState};
use crate::db::{
    AuditEvent, AuditEventCount, AuditLogRow, AuditQuery, CostDataPoint, Database, JobAnalytics,
    ToolAnalytics,
};
use crate::error::{DatabaseError, WorkspaceError};
use crate::history::{
    ConversationMessage, ConversationSummary, JobEventRecord, LlmCallRecord, SandboxJobRecord,
    SandboxJobSummary, SettingRow,
};
use crate::llm::tracked::LlmCallStats;
use crate::workspace::{
    MemoryChunk, MemoryDocument, RankedResult, SearchConfig, SearchResult, WorkspaceEntry,
    reciprocal_rank_fusion,
};

use crate::db::libsql_migrations;

/// Explicit column list for routines table (matches positional access in `row_to_routine_libsql`).
const ROUTINE_COLUMNS: &str = "\
    id, name, description, user_id, enabled, \
    trigger_type, trigger_config, action_type, action_config, \
    cooldown_secs, max_concurrent, dedup_window_secs, \
    notify_channel, notify_user, notify_on_success, notify_on_failure, notify_on_attention, \
    state, last_run_at, next_fire_at, run_count, consecutive_failures, \
    created_at, updated_at";

/// Explicit column list for routine_runs table (matches positional access in `row_to_routine_run_libsql`).
const ROUTINE_RUN_COLUMNS: &str = "\
    id, routine_id, trigger_type, trigger_detail, started_at, \
    status, completed_at, result_summary, tokens_used, job_id, created_at";

/// libSQL/Turso database backend.
///
/// Stores the `Database` handle in an `Arc` so that the same underlying
/// database can be shared with stores (SecretsStore, WasmToolStore) that
/// create their own connections per-operation.
pub struct LibSqlBackend {
    db: Arc<LibSqlDatabase>,
}

impl LibSqlBackend {
    /// Create a new local embedded database.
    pub async fn new_local(path: &Path) -> Result<Self, DatabaseError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DatabaseError::Pool(format!("Failed to create database directory: {}", e))
            })?;
        }

        let db = libsql::Builder::new_local(path)
            .build()
            .await
            .map_err(|e| DatabaseError::Pool(format!("Failed to open libSQL database: {}", e)))?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Create a new in-memory database (for testing).
    pub async fn new_memory() -> Result<Self, DatabaseError> {
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .map_err(|e| {
                DatabaseError::Pool(format!("Failed to create in-memory database: {}", e))
            })?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Create with Turso cloud sync (embedded replica).
    pub async fn new_remote_replica(
        path: &Path,
        url: &str,
        auth_token: &str,
    ) -> Result<Self, DatabaseError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                DatabaseError::Pool(format!("Failed to create database directory: {}", e))
            })?;
        }

        let db = libsql::Builder::new_remote_replica(path, url.to_string(), auth_token.to_string())
            .build()
            .await
            .map_err(|e| DatabaseError::Pool(format!("Failed to open remote replica: {}", e)))?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Get a shared reference to the underlying database handle.
    ///
    /// Use this to pass the database to stores (SecretsStore, WasmToolStore)
    /// that need to create their own connections per-operation.
    pub fn shared_db(&self) -> Arc<LibSqlDatabase> {
        Arc::clone(&self.db)
    }

    /// Create a new connection to the database.
    pub fn connect(&self) -> Result<Connection, DatabaseError> {
        self.db
            .connect()
            .map_err(|e| DatabaseError::Pool(format!("Failed to create connection: {}", e)))
    }
}

// ==================== Helper functions ====================

/// Parse an ISO-8601 timestamp string from SQLite into DateTime<Utc>.
///
/// Tries multiple formats in order:
/// 1. RFC 3339 with timezone (e.g. `2024-01-15T10:30:00.123Z`)
/// 2. Naive datetime with fractional seconds (e.g. `2024-01-15 10:30:00.123`)
/// 3. Naive datetime without fractional seconds (e.g. `2024-01-15 10:30:00`)
///
/// Returns an error if none of the formats match.
fn parse_timestamp(s: &str) -> Result<DateTime<Utc>, String> {
    // RFC 3339 (our canonical write format)
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Naive with fractional seconds (legacy or SQLite datetime() output)
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(ndt.and_utc());
    }
    // Naive without fractional seconds (legacy format)
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(ndt.and_utc());
    }
    Err(format!("unparseable timestamp: {:?}", s))
}

/// Format a DateTime<Utc> for SQLite storage (RFC 3339 with millisecond precision).
fn fmt_ts(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Format an optional DateTime<Utc>.
fn fmt_opt_ts(dt: &Option<DateTime<Utc>>) -> libsql::Value {
    match dt {
        Some(dt) => libsql::Value::Text(fmt_ts(dt)),
        None => libsql::Value::Null,
    }
}

fn parse_job_state(s: &str) -> JobState {
    match s {
        "pending" => JobState::Pending,
        "in_progress" => JobState::InProgress,
        "completed" => JobState::Completed,
        "submitted" => JobState::Submitted,
        "accepted" => JobState::Accepted,
        "failed" => JobState::Failed,
        "stuck" => JobState::Stuck,
        "cancelled" => JobState::Cancelled,
        _ => JobState::Pending,
    }
}

/// Extract a text column from a libsql Row, returning empty string for NULL.
fn get_text(row: &libsql::Row, idx: i32) -> String {
    row.get::<String>(idx).unwrap_or_default()
}

/// Extract an optional text column.
/// Returns None for SQL NULL, preserves empty strings as Some("").
fn get_opt_text(row: &libsql::Row, idx: i32) -> Option<String> {
    row.get::<String>(idx).ok()
}

/// Convert an `Option<&str>` to a `libsql::Value` (Text or Null).
/// Use this instead of `.unwrap_or("")` to preserve NULL semantics.
fn opt_text(s: Option<&str>) -> libsql::Value {
    match s {
        Some(s) => libsql::Value::Text(s.to_string()),
        None => libsql::Value::Null,
    }
}

/// Convert an `Option<String>` to a `libsql::Value` (Text or Null).
fn opt_text_owned(s: Option<String>) -> libsql::Value {
    match s {
        Some(s) => libsql::Value::Text(s),
        None => libsql::Value::Null,
    }
}

/// Extract an i64 column, defaulting to 0.
fn get_i64(row: &libsql::Row, idx: i32) -> i64 {
    row.get::<i64>(idx).unwrap_or(0)
}

/// Extract an optional bool from an integer column.
fn get_opt_bool(row: &libsql::Row, idx: i32) -> Option<bool> {
    row.get::<i64>(idx).ok().map(|v| v != 0)
}

/// Parse a Decimal from a text column.
fn get_decimal(row: &libsql::Row, idx: i32) -> Decimal {
    row.get::<String>(idx)
        .ok()
        .and_then(|s| s.parse::<Decimal>().ok())
        .unwrap_or_default()
}

/// Parse an optional Decimal from a text column.
fn get_opt_decimal(row: &libsql::Row, idx: i32) -> Option<Decimal> {
    row.get::<String>(idx)
        .ok()
        .and_then(|s| s.parse::<Decimal>().ok())
}

/// Parse a JSON value from a text column.
fn get_json(row: &libsql::Row, idx: i32) -> serde_json::Value {
    row.get::<String>(idx)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Parse a timestamp from a text column.
///
/// If the column is NULL or the value cannot be parsed, logs a warning and
/// returns the Unix epoch (1970-01-01T00:00:00Z) so the error is detectable
/// rather than silently replaced by the current time.
fn get_ts(row: &libsql::Row, idx: i32) -> DateTime<Utc> {
    match row.get::<String>(idx) {
        Ok(s) => match parse_timestamp(&s) {
            Ok(dt) => dt,
            Err(e) => {
                tracing::warn!("Timestamp parse failure at column {}: {}", idx, e);
                DateTime::UNIX_EPOCH
            }
        },
        Err(_) => DateTime::UNIX_EPOCH,
    }
}

/// Parse an optional timestamp from a text column.
///
/// Returns None if the column is NULL. Logs a warning and returns None if the
/// value is present but cannot be parsed.
fn get_opt_ts(row: &libsql::Row, idx: i32) -> Option<DateTime<Utc>> {
    match row.get::<String>(idx) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => match parse_timestamp(&s) {
            Ok(dt) => Some(dt),
            Err(e) => {
                tracing::warn!("Timestamp parse failure at column {}: {}", idx, e);
                None
            }
        },
        Err(_) => None,
    }
}

#[async_trait]
impl Database for LibSqlBackend {
    async fn run_migrations(&self) -> Result<(), DatabaseError> {
        let conn = self.connect()?;

        // Apply the consolidated base schema (idempotent — all CREATE IF NOT EXISTS).
        conn.execute_batch(libsql_migrations::SCHEMA)
            .await
            .map_err(|e| DatabaseError::Migration(format!("libSQL base schema failed: {}", e)))?;

        // Apply incremental migrations that haven't been recorded yet.
        for (version, name, sql) in libsql_migrations::INCREMENTAL {
            let already_applied: bool = conn
                .query(
                    "SELECT 1 FROM _migrations WHERE version = ?1",
                    params![*version],
                )
                .await
                .map_err(|e| DatabaseError::Migration(e.to_string()))?
                .next()
                .await
                .map_err(|e| DatabaseError::Migration(e.to_string()))?
                .is_some();

            if already_applied {
                continue;
            }

            tracing::info!(version, name, "Applying libSQL incremental migration");

            conn.execute(sql, ()).await.map_err(|e| {
                DatabaseError::Migration(format!(
                    "libSQL migration V{version} ({name}) failed: {e}"
                ))
            })?;

            conn.execute(
                "INSERT INTO _migrations (version, name) VALUES (?1, ?2)",
                params![*version, *name],
            )
            .await
            .map_err(|e| DatabaseError::Migration(e.to_string()))?;
        }

        Ok(())
    }

    // ==================== Conversations ====================

    async fn create_conversation(
        &self,
        channel: &str,
        user_id: &str,
        thread_id: Option<&str>,
    ) -> Result<Uuid, DatabaseError> {
        let conn = self.connect()?;
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO conversations (id, channel, user_id, thread_id) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), channel, user_id, opt_text(thread_id)],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(id)
    }

    async fn touch_conversation(&self, id: Uuid) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            "UPDATE conversations SET last_activity = ?2 WHERE id = ?1",
            params![id.to_string(), now],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn add_conversation_message(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<Uuid, DatabaseError> {
        let conn = self.connect()?;
        let id = Uuid::new_v4();
        conn.execute(
                "INSERT INTO conversation_messages (id, conversation_id, role, content) VALUES (?1, ?2, ?3, ?4)",
                params![id.to_string(), conversation_id.to_string(), role, content],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        self.touch_conversation(conversation_id).await?;
        Ok(id)
    }

    async fn ensure_conversation(
        &self,
        id: Uuid,
        channel: &str,
        user_id: &str,
        thread_id: Option<&str>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            r#"
                INSERT INTO conversations (id, channel, user_id, thread_id)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (id) DO UPDATE SET last_activity = ?5
                "#,
            params![id.to_string(), channel, user_id, opt_text(thread_id), now],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_conversations_with_preview(
        &self,
        user_id: &str,
        channel: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ConversationSummary>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = if let Some(channel) = channel {
            conn.query(
                r#"
                SELECT
                    c.id,
                    c.started_at,
                    c.last_activity,
                    c.metadata,
                    (SELECT COUNT(*) FROM conversation_messages m WHERE m.conversation_id = c.id) AS message_count,
                    (SELECT substr(m2.content, 1, 100)
                     FROM conversation_messages m2
                     WHERE m2.conversation_id = c.id AND m2.role = 'user'
                     ORDER BY m2.created_at ASC
                     LIMIT 1
                    ) AS title
                FROM conversations c
                WHERE c.user_id = ?1 AND c.channel = ?2
                ORDER BY c.last_activity DESC
                LIMIT ?3
                "#,
                params![user_id, channel, limit],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        } else {
            conn.query(
                r#"
                SELECT
                    c.id,
                    c.started_at,
                    c.last_activity,
                    c.metadata,
                    (SELECT COUNT(*) FROM conversation_messages m WHERE m.conversation_id = c.id) AS message_count,
                    (SELECT substr(m2.content, 1, 100)
                     FROM conversation_messages m2
                     WHERE m2.conversation_id = c.id AND m2.role = 'user'
                     ORDER BY m2.created_at ASC
                     LIMIT 1
                    ) AS title
                FROM conversations c
                WHERE c.user_id = ?1
                ORDER BY c.last_activity DESC
                LIMIT ?2
                "#,
                params![user_id, limit],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        };

        let mut results = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let metadata = get_json(&row, 3);
            let thread_type = metadata
                .get("thread_type")
                .and_then(|v| v.as_str())
                .map(String::from);
            results.push(ConversationSummary {
                id: row
                    .get::<String>(0)
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or_default(),
                started_at: get_ts(&row, 1),
                last_activity: get_ts(&row, 2),
                message_count: get_i64(&row, 4),
                title: get_opt_text(&row, 5),
                thread_type,
            });
        }
        Ok(results)
    }

    async fn get_or_create_assistant_conversation(
        &self,
        user_id: &str,
        channel: &str,
    ) -> Result<Uuid, DatabaseError> {
        let conn = self.connect()?;
        // Try to find existing
        let mut rows = conn
            .query(
                r#"
                SELECT id FROM conversations
                WHERE user_id = ?1 AND channel = ?2
                  AND json_extract(metadata, '$.thread_type') = 'assistant'
                LIMIT 1
                "#,
                params![user_id, channel],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let id_str: String = row.get(0).unwrap_or_default();
            return id_str
                .parse()
                .map_err(|_| DatabaseError::Serialization("Invalid UUID".to_string()));
        }

        // Create new
        let id = Uuid::new_v4();
        let metadata = serde_json::json!({"thread_type": "assistant", "title": "Assistant"});
        conn.execute(
            "INSERT INTO conversations (id, channel, user_id, metadata) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), channel, user_id, metadata.to_string()],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(id)
    }

    async fn create_conversation_with_metadata(
        &self,
        channel: &str,
        user_id: &str,
        metadata: &serde_json::Value,
    ) -> Result<Uuid, DatabaseError> {
        let conn = self.connect()?;
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO conversations (id, channel, user_id, metadata) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), channel, user_id, metadata.to_string()],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(id)
    }

    async fn list_conversation_messages_paginated(
        &self,
        conversation_id: Uuid,
        before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<(Vec<ConversationMessage>, bool), DatabaseError> {
        let conn = self.connect()?;
        let fetch_limit = limit + 1;
        let cid = conversation_id.to_string();

        let mut rows = if let Some(before_ts) = before {
            conn.query(
                r#"
                    SELECT id, role, content, created_at
                    FROM conversation_messages
                    WHERE conversation_id = ?1 AND created_at < ?2
                    ORDER BY created_at DESC
                    LIMIT ?3
                    "#,
                params![cid, fmt_ts(&before_ts), fetch_limit],
            )
            .await
        } else {
            conn.query(
                r#"
                    SELECT id, role, content, created_at
                    FROM conversation_messages
                    WHERE conversation_id = ?1
                    ORDER BY created_at DESC
                    LIMIT ?2
                    "#,
                params![cid, fetch_limit],
            )
            .await
        }
        .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut all = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            all.push(ConversationMessage {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                role: get_text(&row, 1),
                content: get_text(&row, 2),
                created_at: get_ts(&row, 3),
            });
        }

        let has_more = all.len() as i64 > limit;
        all.truncate(limit as usize);
        all.reverse(); // oldest first
        Ok((all, has_more))
    }

    async fn update_conversation_metadata_field(
        &self,
        id: Uuid,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        // SQLite: use json_patch to merge the key
        let patch = serde_json::json!({ key: value });
        conn.execute(
            "UPDATE conversations SET metadata = json_patch(metadata, ?2) WHERE id = ?1",
            params![id.to_string(), patch.to_string()],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_conversation_metadata(
        &self,
        id: Uuid,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT metadata FROM conversations WHERE id = ?1",
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(get_json(&row, 0))),
            None => Ok(None),
        }
    }

    async fn list_conversation_messages(
        &self,
        conversation_id: Uuid,
    ) -> Result<Vec<ConversationMessage>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, role, content, created_at
                FROM conversation_messages
                WHERE conversation_id = ?1
                ORDER BY created_at ASC
                "#,
                params![conversation_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut messages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            messages.push(ConversationMessage {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                role: get_text(&row, 1),
                content: get_text(&row, 2),
                created_at: get_ts(&row, 3),
            });
        }
        Ok(messages)
    }

    async fn conversation_belongs_to_user(
        &self,
        conversation_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT 1 FROM conversations WHERE id = ?1 AND user_id = ?2",
                libsql::params![conversation_id.to_string(), user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        let found = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(found.is_some())
    }

    // ==================== Jobs ====================

    async fn save_job(&self, ctx: &JobContext) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let status = ctx.state.to_string();
        let estimated_time_secs = ctx.estimated_duration.map(|d| d.as_secs() as i64);

        conn
            .execute(
                r#"
                INSERT INTO agent_jobs (
                    id, conversation_id, title, description, category, status, source,
                    budget_amount, budget_token, bid_amount, estimated_cost, estimated_time_secs,
                    actual_cost, repair_attempts, created_at, started_at, completed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT (id) DO UPDATE SET
                    title = excluded.title,
                    description = excluded.description,
                    category = excluded.category,
                    status = excluded.status,
                    estimated_cost = excluded.estimated_cost,
                    estimated_time_secs = excluded.estimated_time_secs,
                    actual_cost = excluded.actual_cost,
                    repair_attempts = excluded.repair_attempts,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at
                "#,
                params![
                    ctx.job_id.to_string(),
                    opt_text_owned(ctx.conversation_id.map(|id| id.to_string())),
                    ctx.title.as_str(),
                    ctx.description.as_str(),
                    opt_text(ctx.category.as_deref()),
                    status,
                    "direct",
                    opt_text_owned(ctx.budget.map(|d| d.to_string())),
                    opt_text(ctx.budget_token.as_deref()),
                    opt_text_owned(ctx.bid_amount.map(|d| d.to_string())),
                    opt_text_owned(ctx.estimated_cost.map(|d| d.to_string())),
                    estimated_time_secs,
                    ctx.actual_cost.to_string(),
                    ctx.repair_attempts as i64,
                    fmt_ts(&ctx.created_at),
                    fmt_opt_ts(&ctx.started_at),
                    fmt_opt_ts(&ctx.completed_at),
                ],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_job(&self, id: Uuid) -> Result<Option<JobContext>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, conversation_id, title, description, category, status, user_id,
                       budget_amount, budget_token, bid_amount, estimated_cost, estimated_time_secs,
                       actual_cost, repair_attempts, created_at, started_at, completed_at
                FROM agent_jobs WHERE id = ?1
                "#,
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => {
                let status_str = get_text(&row, 5);
                let state = parse_job_state(&status_str);
                let estimated_time_secs: Option<i64> = row.get::<i64>(11).ok();

                Ok(Some(JobContext {
                    job_id: get_text(&row, 0).parse().unwrap_or_default(),
                    state,
                    user_id: get_text(&row, 6),
                    conversation_id: get_opt_text(&row, 1).and_then(|s| s.parse().ok()),
                    title: get_text(&row, 2),
                    description: get_text(&row, 3),
                    category: get_opt_text(&row, 4),
                    budget: get_opt_decimal(&row, 7),
                    budget_token: get_opt_text(&row, 8),
                    bid_amount: get_opt_decimal(&row, 9),
                    estimated_cost: get_opt_decimal(&row, 10),
                    estimated_duration: estimated_time_secs
                        .map(|s| std::time::Duration::from_secs(s as u64)),
                    actual_cost: get_decimal(&row, 12),
                    total_tokens_used: 0,
                    max_tokens: 0,
                    repair_attempts: get_i64(&row, 13) as u32,
                    created_at: get_ts(&row, 14),
                    started_at: get_opt_ts(&row, 15),
                    completed_at: get_opt_ts(&row, 16),
                    transitions: Vec::new(),
                    metadata: serde_json::Value::Null,
                }))
            }
            None => Ok(None),
        }
    }

    async fn update_job_status(
        &self,
        id: Uuid,
        status: JobState,
        failure_reason: Option<&str>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE agent_jobs SET status = ?2, failure_reason = ?3 WHERE id = ?1",
            params![id.to_string(), status.to_string(), opt_text(failure_reason)],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn mark_job_stuck(&self, id: Uuid) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            "UPDATE agent_jobs SET status = 'stuck', stuck_since = ?2 WHERE id = ?1",
            params![id.to_string(), now],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_stuck_jobs(&self) -> Result<Vec<Uuid>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query("SELECT id FROM agent_jobs WHERE status = 'stuck'", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            if let Ok(id_str) = row.get::<String>(0)
                && let Ok(id) = id_str.parse()
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    // ==================== Actions ====================

    async fn save_action(&self, job_id: Uuid, action: &ActionRecord) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let duration_ms = action.duration.as_millis() as i64;
        let warnings_json = serde_json::to_string(&action.sanitization_warnings)
            .map_err(|e| DatabaseError::Serialization(e.to_string()))?;

        conn.execute(
            r#"
                INSERT INTO job_actions (
                    id, job_id, sequence_num, tool_name, input, output_raw, output_sanitized,
                    sanitization_warnings, cost, duration_ms, success, error_message, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                "#,
            params![
                action.id.to_string(),
                job_id.to_string(),
                action.sequence as i64,
                action.tool_name.as_str(),
                action.input.to_string(),
                opt_text(action.output_raw.as_deref()),
                opt_text_owned(action.output_sanitized.as_ref().map(|v| v.to_string())),
                warnings_json,
                opt_text_owned(action.cost.map(|d| d.to_string())),
                duration_ms,
                action.success as i64,
                opt_text(action.error.as_deref()),
                fmt_ts(&action.executed_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_job_actions(&self, job_id: Uuid) -> Result<Vec<ActionRecord>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, sequence_num, tool_name, input, output_raw, output_sanitized,
                       sanitization_warnings, cost, duration_ms, success, error_message, created_at
                FROM job_actions WHERE job_id = ?1 ORDER BY sequence_num
                "#,
                params![job_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut actions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let warnings: Vec<String> =
                serde_json::from_str(&get_text(&row, 6)).unwrap_or_default();
            actions.push(ActionRecord {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                sequence: get_i64(&row, 1) as u32,
                tool_name: get_text(&row, 2),
                input: get_json(&row, 3),
                output_raw: get_opt_text(&row, 4),
                output_sanitized: get_opt_text(&row, 5).and_then(|s| serde_json::from_str(&s).ok()),
                sanitization_warnings: warnings,
                cost: get_opt_decimal(&row, 7),
                duration: std::time::Duration::from_millis(get_i64(&row, 8) as u64),
                success: get_i64(&row, 9) != 0,
                error: get_opt_text(&row, 10),
                executed_at: get_ts(&row, 11),
            });
        }
        Ok(actions)
    }

    // ==================== LLM Calls ====================

    async fn record_llm_call(&self, record: &LlmCallRecord<'_>) -> Result<Uuid, DatabaseError> {
        let conn = self.connect()?;
        let id = Uuid::new_v4();
        conn.execute(
                r#"
                INSERT INTO llm_calls (id, job_id, conversation_id, provider, model, input_tokens, output_tokens, cost, purpose, latency_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                "#,
                params![
                    id.to_string(),
                    opt_text_owned(record.job_id.map(|id| id.to_string())),
                    opt_text_owned(record.conversation_id.map(|id| id.to_string())),
                    record.provider,
                    record.model,
                    record.input_tokens as i64,
                    record.output_tokens as i64,
                    record.cost.to_string(),
                    opt_text(record.purpose),
                    record.latency_ms as i64,
                ],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(id)
    }

    async fn get_llm_call_stats(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<LlmCallStats>, DatabaseError> {
        let conn = self.connect()?;
        // libSQL lacks percentile functions; p95 is returned as NULL.
        let (where_clause, since_str) = if let Some(ts) = since {
            ("WHERE created_at >= ?1", Some(ts.to_rfc3339()))
        } else {
            ("", None)
        };
        let sql = format!(
            r#"
            SELECT
                provider,
                model,
                COUNT(*) AS total_calls,
                COALESCE(SUM(input_tokens), 0) AS total_input_tokens,
                COALESCE(SUM(output_tokens), 0) AS total_output_tokens,
                COALESCE(SUM(CAST(cost AS REAL)), 0.0) AS total_cost,
                CASE WHEN COUNT(*) > 0
                    THEN COALESCE(SUM(CAST(cost AS REAL)), 0.0) / COUNT(*)
                    ELSE 0.0
                END AS avg_cost_per_call,
                AVG(CAST(latency_ms AS REAL)) AS avg_latency_ms
            FROM llm_calls
            {where_clause}
            GROUP BY provider, model
            ORDER BY total_cost DESC
            "#
        );
        let mut rows = if let Some(ref ts_str) = since_str {
            conn.query(&sql, params![ts_str.as_str()])
                .await
                .map_err(|e| DatabaseError::Query(e.to_string()))?
        } else {
            conn.query(&sql, ())
                .await
                .map_err(|e| DatabaseError::Query(e.to_string()))?
        };

        let mut stats = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let provider: String = row
                .get(0)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let model: String = row
                .get(1)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total_calls: i64 = row
                .get(2)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total_input_tokens: i64 = row
                .get(3)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total_output_tokens: i64 = row
                .get(4)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total_cost_f64: f64 = row
                .get(5)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let avg_cost_f64: f64 = row
                .get(6)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let avg_latency_ms: Option<f64> = row.get(7).ok();

            stats.push(LlmCallStats {
                provider,
                model,
                total_calls,
                total_input_tokens,
                total_output_tokens,
                total_cost: Decimal::from_f64_retain(total_cost_f64).unwrap_or_default(),
                avg_cost_per_call: Decimal::from_f64_retain(avg_cost_f64).unwrap_or_default(),
                avg_latency_ms,
                // libSQL lacks a percentile aggregate; always NULL here.
                p95_latency_ms: None,
            });
        }

        Ok(stats)
    }

    async fn get_job_analytics(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<JobAnalytics, DatabaseError> {
        let conn = self.connect()?;
        let (where_clause, since_str) = if let Some(ts) = since {
            ("WHERE created_at >= ?1", Some(ts.to_rfc3339()))
        } else {
            ("", None)
        };
        let sql = format!(
            r#"
            SELECT
                COUNT(*) AS total_jobs,
                SUM(CASE WHEN status = 'accepted' THEN 1 ELSE 0 END) AS completed_jobs,
                SUM(CASE WHEN status = 'failed'   THEN 1 ELSE 0 END) AS failed_jobs,
                SUM(CASE WHEN status = 'in_progress' THEN 1 ELSE 0 END) AS in_progress_jobs,
                COALESCE(AVG(
                    CASE WHEN completed_at IS NOT NULL AND started_at IS NOT NULL
                        THEN (julianday(completed_at) - julianday(started_at)) * 86400.0
                        ELSE NULL
                    END
                ), 0.0) AS avg_duration_secs,
                COALESCE(SUM(CAST(actual_cost AS REAL)), 0.0) AS total_cost
            FROM agent_jobs
            {where_clause}
            "#
        );

        let mut rows = if let Some(ref ts_str) = since_str {
            conn.query(&sql, params![ts_str.as_str()])
                .await
                .map_err(|e| DatabaseError::Query(e.to_string()))?
        } else {
            conn.query(&sql, ())
                .await
                .map_err(|e| DatabaseError::Query(e.to_string()))?
        };

        let row = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
            .ok_or_else(|| DatabaseError::Query("No job_analytics row returned".to_string()))?;

        let total: i64 = row.get(0).unwrap_or(0);
        let completed: i64 = row.get(1).unwrap_or(0);
        let failed: i64 = row.get(2).unwrap_or(0);
        let in_progress: i64 = row.get(3).unwrap_or(0);
        let avg_duration: f64 = row.get(4).unwrap_or(0.0);
        let total_cost_f64: f64 = row.get(5).unwrap_or(0.0);

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
            total_cost_usd: Decimal::from_f64_retain(total_cost_f64)
                .unwrap_or_default()
                .to_string(),
        })
    }

    async fn get_tool_analytics(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ToolAnalytics>, DatabaseError> {
        let conn = self.connect()?;
        let (where_clause, since_str) = if let Some(ts) = since {
            ("WHERE created_at >= ?1", Some(ts.to_rfc3339()))
        } else {
            ("", None)
        };
        let sql = format!(
            r#"
            SELECT
                tool_name,
                COUNT(*) AS total_calls,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) AS successful_calls,
                SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS failed_calls,
                COALESCE(AVG(CAST(duration_ms AS REAL)), 0.0) AS avg_duration_ms,
                COALESCE(SUM(CAST(cost AS REAL)), 0.0) AS total_cost
            FROM job_actions
            {where_clause}
            GROUP BY tool_name
            ORDER BY total_calls DESC
            "#
        );

        let mut rows = if let Some(ref ts_str) = since_str {
            conn.query(&sql, params![ts_str.as_str()])
                .await
                .map_err(|e| DatabaseError::Query(e.to_string()))?
        } else {
            conn.query(&sql, ())
                .await
                .map_err(|e| DatabaseError::Query(e.to_string()))?
        };

        let mut stats = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let tool_name: String = row
                .get(0)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total: i64 = row
                .get(1)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let successful: i64 = row
                .get(2)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let failed_calls: i64 = row
                .get(3)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let avg_duration_ms: f64 = row.get(4).unwrap_or(0.0);
            let total_cost_f64: f64 = row.get(5).unwrap_or(0.0);

            stats.push(ToolAnalytics {
                tool_name,
                total_calls: total,
                successful_calls: successful,
                failed_calls,
                success_rate: if total > 0 {
                    successful as f64 / total as f64
                } else {
                    0.0
                },
                avg_duration_ms,
                total_cost_usd: Decimal::from_f64_retain(total_cost_f64)
                    .unwrap_or_default()
                    .to_string(),
            });
        }

        Ok(stats)
    }

    async fn get_cost_over_time(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<CostDataPoint>, DatabaseError> {
        let conn = self.connect()?;
        let since_str = since.to_rfc3339();
        let mut rows = conn
            .query(
                r#"
                SELECT
                    DATE(created_at) AS day,
                    COALESCE(SUM(CAST(cost AS REAL)), 0.0) AS daily_cost,
                    COUNT(*) AS call_count
                FROM llm_calls
                WHERE created_at >= ?1
                GROUP BY DATE(created_at)
                ORDER BY day ASC
                "#,
                params![since_str.as_str()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut points = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let day: String = row
                .get(0)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let daily_cost_f64: f64 = row.get(1).unwrap_or(0.0);
            let call_count: i64 = row
                .get(2)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            points.push(CostDataPoint {
                day,
                cost_usd: Decimal::from_f64_retain(daily_cost_f64)
                    .unwrap_or_default()
                    .to_string(),
                call_count,
            });
        }

        Ok(points)
    }

    async fn get_conversation_token_stats(
        &self,
        conversation_id: Uuid,
    ) -> Result<crate::db::ConversationTokenStats, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(CAST(cost AS REAL)), 0.0),
                    COUNT(*)
                FROM llm_calls
                WHERE conversation_id = ?1
                "#,
                params![conversation_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let total_input_tokens: i64 = row
                .get(0)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total_output_tokens: i64 = row
                .get(1)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let total_cost_f64: f64 = row
                .get(2)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            let call_count: i64 = row
                .get(3)
                .map_err(|e| DatabaseError::Query(e.to_string()))?;
            Ok(crate::db::ConversationTokenStats {
                conversation_id,
                total_input_tokens,
                total_output_tokens,
                total_cost: Decimal::from_f64_retain(total_cost_f64).unwrap_or_default(),
                call_count,
            })
        } else {
            Ok(crate::db::ConversationTokenStats {
                conversation_id,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost: Decimal::ZERO,
                call_count: 0,
            })
        }
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
        let conn = self.connect()?;
        let id = Uuid::new_v4();
        let tools_json = serde_json::to_string(tool_names)
            .map_err(|e| DatabaseError::Serialization(e.to_string()))?;

        conn.execute(
                r#"
                INSERT INTO estimation_snapshots (id, job_id, category, tool_names, estimated_cost, estimated_time_secs, estimated_value)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    id.to_string(),
                    job_id.to_string(),
                    category,
                    tools_json,
                    estimated_cost.to_string(),
                    estimated_time_secs as i64,
                    estimated_value.to_string(),
                ],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(id)
    }

    async fn update_estimation_actuals(
        &self,
        id: Uuid,
        actual_cost: Decimal,
        actual_time_secs: i32,
        actual_value: Option<Decimal>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
                "UPDATE estimation_snapshots SET actual_cost = ?2, actual_time_secs = ?3, actual_value = ?4 WHERE id = ?1",
                params![
                    id.to_string(),
                    actual_cost.to_string(),
                    actual_time_secs as i64,
                    actual_value.map(|d| d.to_string()).unwrap_or_default(),
                ],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    // ==================== Sandbox Jobs ====================

    async fn save_sandbox_job(&self, job: &SandboxJobRecord) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
            r#"
                INSERT INTO agent_jobs (
                    id, title, description, status, source, user_id, project_dir,
                    success, failure_reason, created_at, started_at, completed_at
                ) VALUES (?1, ?2, '', ?3, 'sandbox', ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT (id) DO UPDATE SET
                    status = excluded.status,
                    success = excluded.success,
                    failure_reason = excluded.failure_reason,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at
                "#,
            params![
                job.id.to_string(),
                job.task.as_str(),
                job.status.as_str(),
                job.user_id.as_str(),
                job.project_dir.as_str(),
                job.success.map(|b| b as i64),
                opt_text(job.failure_reason.as_deref()),
                fmt_ts(&job.created_at),
                fmt_opt_ts(&job.started_at),
                fmt_opt_ts(&job.completed_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_sandbox_job(&self, id: Uuid) -> Result<Option<SandboxJobRecord>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, title, status, user_id, project_dir,
                       success, failure_reason, created_at, started_at, completed_at
                FROM agent_jobs WHERE id = ?1 AND source = 'sandbox'
                "#,
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(SandboxJobRecord {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                task: get_text(&row, 1),
                status: get_text(&row, 2),
                user_id: get_text(&row, 3),
                project_dir: get_text(&row, 4),
                success: get_opt_bool(&row, 5),
                failure_reason: get_opt_text(&row, 6),
                created_at: get_ts(&row, 7),
                started_at: get_opt_ts(&row, 8),
                completed_at: get_opt_ts(&row, 9),
            })),
            None => Ok(None),
        }
    }

    async fn list_sandbox_jobs(&self) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, title, status, user_id, project_dir,
                       success, failure_reason, created_at, started_at, completed_at
                FROM agent_jobs WHERE source = 'sandbox'
                ORDER BY created_at DESC
                "#,
                (),
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut jobs = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            jobs.push(SandboxJobRecord {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                task: get_text(&row, 1),
                status: get_text(&row, 2),
                user_id: get_text(&row, 3),
                project_dir: get_text(&row, 4),
                success: get_opt_bool(&row, 5),
                failure_reason: get_opt_text(&row, 6),
                created_at: get_ts(&row, 7),
                started_at: get_opt_ts(&row, 8),
                completed_at: get_opt_ts(&row, 9),
            });
        }
        Ok(jobs)
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
        let conn = self.connect()?;
        conn.execute(
            r#"
                UPDATE agent_jobs SET
                    status = ?2,
                    success = COALESCE(?3, success),
                    failure_reason = COALESCE(?4, failure_reason),
                    started_at = COALESCE(?5, started_at),
                    completed_at = COALESCE(?6, completed_at)
                WHERE id = ?1 AND source = 'sandbox'
                "#,
            params![
                id.to_string(),
                status,
                success.map(|b| b as i64),
                message,
                fmt_opt_ts(&started_at),
                fmt_opt_ts(&completed_at),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn cleanup_stale_sandbox_jobs(&self) -> Result<u64, DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        let count = conn
            .execute(
                r#"
                UPDATE agent_jobs SET
                    status = 'interrupted',
                    failure_reason = 'Process restarted',
                    completed_at = ?1
                WHERE source = 'sandbox' AND status IN ('running', 'creating')
                "#,
                params![now],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        if count > 0 {
            tracing::info!("Marked {} stale sandbox jobs as interrupted", count);
        }
        Ok(count)
    }

    async fn sandbox_job_summary(&self) -> Result<SandboxJobSummary, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT status, COUNT(*) as cnt FROM agent_jobs WHERE source = 'sandbox' GROUP BY status",
                (),
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut summary = SandboxJobSummary::default();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let status = get_text(&row, 0);
            let count = get_i64(&row, 1) as usize;
            summary.total += count;
            match status.as_str() {
                "creating" => summary.creating += count,
                "running" => summary.running += count,
                "completed" => summary.completed += count,
                "failed" => summary.failed += count,
                "interrupted" => summary.interrupted += count,
                _ => {}
            }
        }
        Ok(summary)
    }

    async fn list_sandbox_jobs_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, title, status, user_id, project_dir,
                       success, failure_reason, created_at, started_at, completed_at
                FROM agent_jobs WHERE source = 'sandbox' AND user_id = ?1
                ORDER BY created_at DESC
                "#,
                libsql::params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut jobs = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            jobs.push(SandboxJobRecord {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                task: get_text(&row, 1),
                status: get_text(&row, 2),
                user_id: get_text(&row, 3),
                project_dir: get_text(&row, 4),
                success: get_opt_bool(&row, 5),
                failure_reason: get_opt_text(&row, 6),
                created_at: get_ts(&row, 7),
                started_at: get_opt_ts(&row, 8),
                completed_at: get_opt_ts(&row, 9),
            });
        }
        Ok(jobs)
    }

    async fn sandbox_job_summary_for_user(
        &self,
        user_id: &str,
    ) -> Result<SandboxJobSummary, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT status, COUNT(*) as cnt FROM agent_jobs WHERE source = 'sandbox' AND user_id = ?1 GROUP BY status",
                libsql::params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut summary = SandboxJobSummary::default();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            let status = get_text(&row, 0);
            let count = get_i64(&row, 1) as usize;
            summary.total += count;
            match status.as_str() {
                "creating" => summary.creating += count,
                "running" => summary.running += count,
                "completed" => summary.completed += count,
                "failed" => summary.failed += count,
                "interrupted" => summary.interrupted += count,
                _ => {}
            }
        }
        Ok(summary)
    }

    async fn sandbox_job_belongs_to_user(
        &self,
        job_id: Uuid,
        user_id: &str,
    ) -> Result<bool, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT 1 FROM agent_jobs WHERE id = ?1 AND user_id = ?2 AND source = 'sandbox'",
                libsql::params![job_id.to_string(), user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        let found = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(found.is_some())
    }

    async fn update_sandbox_job_mode(&self, id: Uuid, mode: &str) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE agent_jobs SET job_mode = ?2 WHERE id = ?1",
            params![id.to_string(), mode],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_sandbox_job_mode(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT job_mode FROM agent_jobs WHERE id = ?1",
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(get_text(&row, 0))),
            None => Ok(None),
        }
    }

    // ==================== Job Events ====================

    async fn save_job_event(
        &self,
        job_id: Uuid,
        event_type: &str,
        data: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO job_events (job_id, event_type, data) VALUES (?1, ?2, ?3)",
            params![job_id.to_string(), event_type, data.to_string()],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_job_events(&self, job_id: Uuid) -> Result<Vec<JobEventRecord>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, job_id, event_type, data, created_at
                FROM job_events WHERE job_id = ?1 ORDER BY id ASC
                "#,
                params![job_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            events.push(JobEventRecord {
                id: get_i64(&row, 0),
                job_id: get_text(&row, 1).parse().unwrap_or_default(),
                event_type: get_text(&row, 2),
                data: get_json(&row, 3),
                created_at: get_ts(&row, 4),
            });
        }
        Ok(events)
    }

    // ==================== Routines ====================

    async fn create_routine(&self, routine: &Routine) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let trigger_type = routine.trigger.type_tag();
        let trigger_config = routine.trigger.to_config_json();
        let action_type = routine.action.type_tag();
        let action_config = routine.action.to_config_json();
        let cooldown_secs = routine.guardrails.cooldown.as_secs() as i64;
        let max_concurrent = routine.guardrails.max_concurrent as i64;
        let dedup_window_secs = routine.guardrails.dedup_window.map(|d| d.as_secs() as i64);

        conn.execute(
                r#"
                INSERT INTO routines (
                    id, name, description, user_id, enabled,
                    trigger_type, trigger_config, action_type, action_config,
                    cooldown_secs, max_concurrent, dedup_window_secs,
                    notify_channel, notify_user, notify_on_success, notify_on_failure, notify_on_attention,
                    state, next_fire_at, created_at, updated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5,
                    ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17,
                    ?18, ?19, ?20, ?21
                )
                "#,
                params![
                    routine.id.to_string(),
                    routine.name.as_str(),
                    routine.description.as_str(),
                    routine.user_id.as_str(),
                    routine.enabled as i64,
                    trigger_type,
                    trigger_config.to_string(),
                    action_type,
                    action_config.to_string(),
                    cooldown_secs,
                    max_concurrent,
                    dedup_window_secs,
                    opt_text(routine.notify.channel.as_deref()),
                    routine.notify.user.as_str(),
                    routine.notify.on_success as i64,
                    routine.notify.on_failure as i64,
                    routine.notify.on_attention as i64,
                    routine.state.to_string(),
                    fmt_opt_ts(&routine.next_fire_at),
                    fmt_ts(&routine.created_at),
                    fmt_ts(&routine.updated_at),
                ],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_routine(&self, id: Uuid) -> Result<Option<Routine>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                &format!("SELECT {} FROM routines WHERE id = ?1", ROUTINE_COLUMNS),
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_routine_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_routine_by_name(
        &self,
        user_id: &str,
        name: &str,
    ) -> Result<Option<Routine>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {} FROM routines WHERE user_id = ?1 AND name = ?2",
                    ROUTINE_COLUMNS
                ),
                params![user_id, name],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(row_to_routine_libsql(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_routines(&self, user_id: &str) -> Result<Vec<Routine>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {} FROM routines WHERE user_id = ?1 ORDER BY name",
                    ROUTINE_COLUMNS
                ),
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut routines = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            routines.push(row_to_routine_libsql(&row)?);
        }
        Ok(routines)
    }

    async fn list_event_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {} FROM routines WHERE enabled = 1 AND trigger_type = 'event'",
                    ROUTINE_COLUMNS
                ),
                (),
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut routines = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            routines.push(row_to_routine_libsql(&row)?);
        }
        Ok(routines)
    }

    async fn list_due_cron_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {} FROM routines WHERE enabled = 1 AND trigger_type = 'cron' AND next_fire_at IS NOT NULL AND next_fire_at <= ?1",
                    ROUTINE_COLUMNS
                ),
                params![now],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut routines = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            routines.push(row_to_routine_libsql(&row)?);
        }
        Ok(routines)
    }

    async fn update_routine(&self, routine: &Routine) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let trigger_type = routine.trigger.type_tag();
        let trigger_config = routine.trigger.to_config_json();
        let action_type = routine.action.type_tag();
        let action_config = routine.action.to_config_json();
        let cooldown_secs = routine.guardrails.cooldown.as_secs() as i64;
        let max_concurrent = routine.guardrails.max_concurrent as i64;
        let dedup_window_secs = routine.guardrails.dedup_window.map(|d| d.as_secs() as i64);
        let now = fmt_ts(&Utc::now());

        conn.execute(
            r#"
                UPDATE routines SET
                    name = ?2, description = ?3, enabled = ?4,
                    trigger_type = ?5, trigger_config = ?6,
                    action_type = ?7, action_config = ?8,
                    cooldown_secs = ?9, max_concurrent = ?10, dedup_window_secs = ?11,
                    notify_channel = ?12, notify_user = ?13,
                    notify_on_success = ?14, notify_on_failure = ?15, notify_on_attention = ?16,
                    state = ?17, next_fire_at = ?18,
                    updated_at = ?19
                WHERE id = ?1
                "#,
            params![
                routine.id.to_string(),
                routine.name.as_str(),
                routine.description.as_str(),
                routine.enabled as i64,
                trigger_type,
                trigger_config.to_string(),
                action_type,
                action_config.to_string(),
                cooldown_secs,
                max_concurrent,
                dedup_window_secs,
                opt_text(routine.notify.channel.as_deref()),
                routine.notify.user.as_str(),
                routine.notify.on_success as i64,
                routine.notify.on_failure as i64,
                routine.notify.on_attention as i64,
                routine.state.to_string(),
                fmt_opt_ts(&routine.next_fire_at),
                now,
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
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
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            r#"
                UPDATE routines SET
                    last_run_at = ?2, next_fire_at = ?3,
                    run_count = ?4, consecutive_failures = ?5,
                    state = ?6, updated_at = ?7
                WHERE id = ?1
                "#,
            params![
                id.to_string(),
                fmt_ts(&last_run_at),
                fmt_opt_ts(&next_fire_at),
                run_count as i64,
                consecutive_failures as i64,
                state.to_string(),
                now,
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete_routine(&self, id: Uuid) -> Result<bool, DatabaseError> {
        let conn = self.connect()?;
        let count = conn
            .execute(
                "DELETE FROM routines WHERE id = ?1",
                params![id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(count > 0)
    }

    // ==================== Routine Runs ====================

    async fn create_routine_run(&self, run: &RoutineRun) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
            r#"
                INSERT INTO routine_runs (
                    id, routine_id, trigger_type, trigger_detail,
                    started_at, status, job_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
            params![
                run.id.to_string(),
                run.routine_id.to_string(),
                run.trigger_type.as_str(),
                opt_text(run.trigger_detail.as_deref()),
                fmt_ts(&run.started_at),
                run.status.to_string(),
                opt_text_owned(run.job_id.map(|id| id.to_string())),
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn complete_routine_run(
        &self,
        id: Uuid,
        status: RunStatus,
        result_summary: Option<&str>,
        tokens_used: Option<i32>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            r#"
                UPDATE routine_runs SET
                    completed_at = ?5, status = ?2,
                    result_summary = ?3, tokens_used = ?4
                WHERE id = ?1
                "#,
            params![
                id.to_string(),
                status.to_string(),
                opt_text(result_summary),
                tokens_used.map(|t| t as i64),
                now,
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_routine_runs(
        &self,
        routine_id: Uuid,
        limit: i64,
    ) -> Result<Vec<RoutineRun>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                &format!(
                    "SELECT {} FROM routine_runs WHERE routine_id = ?1 ORDER BY started_at DESC LIMIT ?2",
                    ROUTINE_RUN_COLUMNS
                ),
                params![routine_id.to_string(), limit],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut runs = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            runs.push(row_to_routine_run_libsql(&row)?);
        }
        Ok(runs)
    }

    async fn count_running_routine_runs(&self, routine_id: Uuid) -> Result<i64, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) as cnt FROM routine_runs WHERE routine_id = ?1 AND status = 'running'",
                params![routine_id.to_string()],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(get_i64(&row, 0)),
            None => Ok(0),
        }
    }

    // ==================== Tool Failures ====================

    async fn record_tool_failure(
        &self,
        tool_name: &str,
        error_message: &str,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            r#"
                INSERT INTO tool_failures (id, tool_name, error_message, error_count, last_failure)
                VALUES (?1, ?2, ?3, 1, ?4)
                ON CONFLICT (tool_name) DO UPDATE SET
                    error_message = ?3,
                    error_count = tool_failures.error_count + 1,
                    last_failure = ?4
                "#,
            params![Uuid::new_v4().to_string(), tool_name, error_message, now],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_broken_tools(&self, threshold: i32) -> Result<Vec<BrokenTool>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                r#"
                SELECT tool_name, error_message, error_count, first_failure, last_failure,
                       last_build_result, repair_attempts
                FROM tool_failures
                WHERE error_count >= ?1 AND repaired_at IS NULL
                ORDER BY error_count DESC
                "#,
                params![threshold as i64],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut tools = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            tools.push(BrokenTool {
                name: get_text(&row, 0),
                last_error: get_opt_text(&row, 1),
                failure_count: get_i64(&row, 2) as u32,
                first_failure: get_ts(&row, 3),
                last_failure: get_ts(&row, 4),
                last_build_result: get_opt_text(&row, 5)
                    .and_then(|s| serde_json::from_str(&s).ok()),
                repair_attempts: get_i64(&row, 6) as u32,
            });
        }
        Ok(tools)
    }

    async fn mark_tool_repaired(&self, tool_name: &str) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            "UPDATE tool_failures SET repaired_at = ?2, error_count = 0 WHERE tool_name = ?1",
            params![tool_name, now],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn increment_repair_attempts(&self, tool_name: &str) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE tool_failures SET repair_attempts = repair_attempts + 1 WHERE tool_name = ?1",
            params![tool_name],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    // ==================== Settings ====================

    async fn get_setting(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT value FROM settings WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(get_json(&row, 0))),
            None => Ok(None),
        }
    }

    async fn get_setting_full(
        &self,
        user_id: &str,
        key: &str,
    ) -> Result<Option<SettingRow>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT key, value, updated_at FROM settings WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(Some(SettingRow {
                key: get_text(&row, 0),
                value: get_json(&row, 1),
                updated_at: get_ts(&row, 2),
            })),
            None => Ok(None),
        }
    }

    async fn set_setting(
        &self,
        user_id: &str,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            r#"
                INSERT INTO settings (user_id, key, value, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (user_id, key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = ?4
                "#,
            params![user_id, key, value.to_string(), now],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete_setting(&self, user_id: &str, key: &str) -> Result<bool, DatabaseError> {
        let conn = self.connect()?;
        let count = conn
            .execute(
                "DELETE FROM settings WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(count > 0)
    }

    async fn list_settings(&self, user_id: &str) -> Result<Vec<SettingRow>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT key, value, updated_at FROM settings WHERE user_id = ?1 ORDER BY key",
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut settings = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            settings.push(SettingRow {
                key: get_text(&row, 0),
                value: get_json(&row, 1),
                updated_at: get_ts(&row, 2),
            });
        }
        Ok(settings)
    }

    async fn get_all_settings(
        &self,
        user_id: &str,
    ) -> Result<HashMap<String, serde_json::Value>, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT key, value FROM settings WHERE user_id = ?1",
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut map = HashMap::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            map.insert(get_text(&row, 0), get_json(&row, 1));
        }
        Ok(map)
    }

    async fn set_all_settings(
        &self,
        user_id: &str,
        settings: &HashMap<String, serde_json::Value>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let now = fmt_ts(&Utc::now());
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        for (key, value) in settings {
            if let Err(e) = conn
                .execute(
                    r#"
                    INSERT INTO settings (user_id, key, value, updated_at)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT (user_id, key) DO UPDATE SET
                        value = excluded.value,
                        updated_at = ?4
                    "#,
                    params![user_id, key.as_str(), value.to_string(), now.as_str()],
                )
                .await
            {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(DatabaseError::Query(e.to_string()));
            }
        }

        conn.execute("COMMIT", ())
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn has_settings(&self, user_id: &str) -> Result<bool, DatabaseError> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) as cnt FROM settings WHERE user_id = ?1",
                params![user_id],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        match rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            Some(row) => Ok(get_i64(&row, 0) > 0),
            None => Ok(false),
        }
    }

    // ==================== Workspace: Documents ====================

    async fn get_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let agent_id_str = agent_id.map(|id| id.to_string());
        let mut rows = conn
            .query(
                r#"
                SELECT id, user_id, agent_id, path, content,
                       created_at, updated_at, metadata
                FROM memory_documents
                WHERE user_id = ?1 AND agent_id IS ?2 AND path = ?3
                "#,
                params![user_id, agent_id_str.as_deref(), path],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?;

        match rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })? {
            Some(row) => Ok(row_to_memory_document(&row)),
            None => Err(WorkspaceError::DocumentNotFound {
                doc_type: path.to_string(),
                user_id: user_id.to_string(),
            }),
        }
    }

    async fn get_document_by_id(&self, id: Uuid) -> Result<MemoryDocument, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let mut rows = conn
            .query(
                r#"
                SELECT id, user_id, agent_id, path, content,
                       created_at, updated_at, metadata
                FROM memory_documents WHERE id = ?1
                "#,
                params![id.to_string()],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?;

        match rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })? {
            Some(row) => Ok(row_to_memory_document(&row)),
            None => Err(WorkspaceError::DocumentNotFound {
                doc_type: "unknown".to_string(),
                user_id: "unknown".to_string(),
            }),
        }
    }

    async fn get_or_create_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        // Try get
        match self.get_document_by_path(user_id, agent_id, path).await {
            Ok(doc) => return Ok(doc),
            Err(WorkspaceError::DocumentNotFound { .. }) => {}
            Err(e) => return Err(e),
        }

        // Create
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let id = Uuid::new_v4();
        let agent_id_str = agent_id.map(|id| id.to_string());
        conn.execute(
            r#"
                INSERT INTO memory_documents (id, user_id, agent_id, path, content, metadata)
                VALUES (?1, ?2, ?3, ?4, '', '{}')
                ON CONFLICT (user_id, agent_id, path) DO NOTHING
                "#,
            params![id.to_string(), user_id, agent_id_str.as_deref(), path],
        )
        .await
        .map_err(|e| WorkspaceError::SearchFailed {
            reason: format!("Insert failed: {}", e),
        })?;

        self.get_document_by_path(user_id, agent_id, path).await
    }

    async fn update_document(&self, id: Uuid, content: &str) -> Result<(), WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let now = fmt_ts(&Utc::now());
        conn.execute(
            "UPDATE memory_documents SET content = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), content, now],
        )
        .await
        .map_err(|e| WorkspaceError::SearchFailed {
            reason: format!("Update failed: {}", e),
        })?;
        Ok(())
    }

    async fn delete_document_by_path(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        path: &str,
    ) -> Result<(), WorkspaceError> {
        let doc = self.get_document_by_path(user_id, agent_id, path).await?;
        self.delete_chunks(doc.id).await?;

        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let agent_id_str = agent_id.map(|id| id.to_string());
        conn.execute(
            "DELETE FROM memory_documents WHERE user_id = ?1 AND agent_id IS ?2 AND path = ?3",
            params![user_id, agent_id_str.as_deref(), path],
        )
        .await
        .map_err(|e| WorkspaceError::SearchFailed {
            reason: format!("Delete failed: {}", e),
        })?;
        Ok(())
    }

    async fn list_directory(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        directory: &str,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        // Implement the list_workspace_files logic in Rust instead of PL/pgSQL.
        let dir = if !directory.is_empty() && !directory.ends_with('/') {
            format!("{}/", directory)
        } else {
            directory.to_string()
        };

        let agent_id_str = agent_id.map(|id| id.to_string());
        let pattern = if dir.is_empty() {
            "%".to_string()
        } else {
            format!("{}%", dir)
        };

        let mut rows = conn
            .query(
                r#"
                SELECT path, updated_at, substr(content, 1, 200) as content_preview
                FROM memory_documents
                WHERE user_id = ?1 AND agent_id IS ?2
                  AND (?3 = '%' OR path LIKE ?3)
                ORDER BY path
                "#,
                params![user_id, agent_id_str.as_deref(), pattern],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("List directory failed: {}", e),
            })?;

        let mut entries_map: HashMap<String, WorkspaceEntry> = HashMap::new();

        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?
        {
            let full_path = get_text(&row, 0);
            let updated_at = get_opt_ts(&row, 1);
            let content_preview = get_opt_text(&row, 2);

            // Extract the immediate child name relative to directory
            let relative = if dir.is_empty() {
                &full_path
            } else if let Some(stripped) = full_path.strip_prefix(&dir) {
                stripped
            } else {
                continue;
            };

            let child_name = if let Some(slash_pos) = relative.find('/') {
                &relative[..slash_pos]
            } else {
                relative
            };

            if child_name.is_empty() {
                continue;
            }

            let is_dir = relative.contains('/');
            let entry_path = if dir.is_empty() {
                child_name.to_string()
            } else {
                format!("{}{}", dir, child_name)
            };

            entries_map
                .entry(child_name.to_string())
                .and_modify(|e| {
                    // Mark as directory if any sub-paths exist
                    if is_dir {
                        e.is_directory = true;
                        e.content_preview = None;
                    }
                    // Update to latest timestamp
                    if let (Some(existing), Some(new)) = (&e.updated_at, &updated_at)
                        && new > existing
                    {
                        e.updated_at = Some(*new);
                    }
                })
                .or_insert(WorkspaceEntry {
                    path: entry_path,
                    is_directory: is_dir,
                    updated_at,
                    content_preview: if is_dir { None } else { content_preview },
                });
        }

        let mut entries: Vec<WorkspaceEntry> = entries_map.into_values().collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    async fn list_all_paths(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
    ) -> Result<Vec<String>, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let agent_id_str = agent_id.map(|id| id.to_string());
        let mut rows = conn
            .query(
                "SELECT path FROM memory_documents WHERE user_id = ?1 AND agent_id IS ?2 ORDER BY path",
                params![user_id, agent_id_str.as_deref()],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("List paths failed: {}", e),
            })?;

        let mut paths = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?
        {
            paths.push(get_text(&row, 0));
        }
        Ok(paths)
    }

    async fn list_documents(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
    ) -> Result<Vec<MemoryDocument>, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let agent_id_str = agent_id.map(|id| id.to_string());
        let mut rows = conn
            .query(
                r#"
                SELECT id, user_id, agent_id, path, content,
                       created_at, updated_at, metadata
                FROM memory_documents
                WHERE user_id = ?1 AND agent_id IS ?2
                ORDER BY updated_at DESC
                "#,
                params![user_id, agent_id_str.as_deref()],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?;

        let mut docs = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?
        {
            docs.push(row_to_memory_document(&row));
        }
        Ok(docs)
    }

    // ==================== Workspace: Chunks ====================

    async fn delete_chunks(&self, document_id: Uuid) -> Result<(), WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::ChunkingFailed {
            reason: e.to_string(),
        })?;
        conn.execute(
            "DELETE FROM memory_chunks WHERE document_id = ?1",
            params![document_id.to_string()],
        )
        .await
        .map_err(|e| WorkspaceError::ChunkingFailed {
            reason: format!("Delete failed: {}", e),
        })?;
        Ok(())
    }

    async fn insert_chunk(
        &self,
        document_id: Uuid,
        chunk_index: i32,
        content: &str,
        embedding: Option<&[f32]>,
    ) -> Result<Uuid, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::ChunkingFailed {
            reason: e.to_string(),
        })?;
        let id = Uuid::new_v4();
        let embedding_blob = embedding.map(|e| {
            // Convert f32 slice to bytes for F32_BLOB
            let bytes: Vec<u8> = e.iter().flat_map(|f| f.to_le_bytes()).collect();
            bytes
        });

        conn.execute(
            r#"
                INSERT INTO memory_chunks (id, document_id, chunk_index, content, embedding)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            params![
                id.to_string(),
                document_id.to_string(),
                chunk_index as i64,
                content,
                embedding_blob.map(libsql::Value::Blob),
            ],
        )
        .await
        .map_err(|e| WorkspaceError::ChunkingFailed {
            reason: format!("Insert failed: {}", e),
        })?;
        Ok(id)
    }

    async fn update_chunk_embedding(
        &self,
        chunk_id: Uuid,
        embedding: &[f32],
    ) -> Result<(), WorkspaceError> {
        let conn = self
            .connect()
            .map_err(|e| WorkspaceError::EmbeddingFailed {
                reason: e.to_string(),
            })?;
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();

        conn.execute(
            "UPDATE memory_chunks SET embedding = ?2 WHERE id = ?1",
            params![chunk_id.to_string(), libsql::Value::Blob(bytes)],
        )
        .await
        .map_err(|e| WorkspaceError::EmbeddingFailed {
            reason: format!("Update failed: {}", e),
        })?;
        Ok(())
    }

    async fn get_chunks_without_embeddings(
        &self,
        user_id: &str,
        agent_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<MemoryChunk>, WorkspaceError> {
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let agent_id_str = agent_id.map(|id| id.to_string());
        let mut rows = conn
            .query(
                r#"
                SELECT c.id, c.document_id, c.chunk_index, c.content, c.created_at
                FROM memory_chunks c
                JOIN memory_documents d ON d.id = c.document_id
                WHERE d.user_id = ?1 AND d.agent_id IS ?2
                  AND c.embedding IS NULL
                LIMIT ?3
                "#,
                params![user_id, agent_id_str.as_deref(), limit as i64],
            )
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?;

        let mut chunks = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WorkspaceError::SearchFailed {
                reason: format!("Query failed: {}", e),
            })?
        {
            chunks.push(MemoryChunk {
                id: get_text(&row, 0).parse().unwrap_or_default(),
                document_id: get_text(&row, 1).parse().unwrap_or_default(),
                chunk_index: get_i64(&row, 2) as i32,
                content: get_text(&row, 3),
                embedding: None,
                created_at: get_ts(&row, 4),
            });
        }
        Ok(chunks)
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
        let conn = self.connect().map_err(|e| WorkspaceError::SearchFailed {
            reason: e.to_string(),
        })?;
        let agent_id_str = agent_id.map(|id| id.to_string());
        let pre_limit = config.pre_fusion_limit as i64;

        // FTS search using FTS5
        let fts_results = if config.use_fts {
            let mut rows = conn
                .query(
                    r#"
                    SELECT c.id, c.document_id, c.content
                    FROM memory_chunks_fts fts
                    JOIN memory_chunks c ON c._rowid = fts.rowid
                    JOIN memory_documents d ON d.id = c.document_id
                    WHERE d.user_id = ?1 AND d.agent_id IS ?2
                      AND memory_chunks_fts MATCH ?3
                    ORDER BY rank
                    LIMIT ?4
                    "#,
                    params![user_id, agent_id_str.as_deref(), query, pre_limit],
                )
                .await
                .map_err(|e| WorkspaceError::SearchFailed {
                    reason: format!("FTS query failed: {}", e),
                })?;

            let mut results = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WorkspaceError::SearchFailed {
                    reason: format!("FTS row fetch failed: {}", e),
                })?
            {
                results.push(RankedResult {
                    chunk_id: get_text(&row, 0).parse().unwrap_or_default(),
                    document_id: get_text(&row, 1).parse().unwrap_or_default(),
                    content: get_text(&row, 2),
                    rank: results.len() as u32 + 1,
                });
            }
            results
        } else {
            Vec::new()
        };

        // Vector search using libsql_vector_idx
        let vector_results = if let (true, Some(emb)) = (config.use_vector, embedding) {
            // Format as JSON array string for vector() SQL function
            let vector_json = format!(
                "[{}]",
                emb.iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );

            // vector_top_k returns rowids from the vector index.
            // We join back to memory_chunks and filter by user/agent.
            let mut rows = conn
                .query(
                    r#"
                    SELECT c.id, c.document_id, c.content
                    FROM vector_top_k('idx_memory_chunks_embedding', vector(?1), ?2) AS top_k
                    JOIN memory_chunks c ON c._rowid = top_k.id
                    JOIN memory_documents d ON d.id = c.document_id
                    WHERE d.user_id = ?3 AND d.agent_id IS ?4
                    "#,
                    params![vector_json, pre_limit, user_id, agent_id_str.as_deref()],
                )
                .await
                .map_err(|e| WorkspaceError::SearchFailed {
                    reason: format!("Vector query failed: {}", e),
                })?;

            let mut results = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| WorkspaceError::SearchFailed {
                    reason: format!("Vector row fetch failed: {}", e),
                })?
            {
                results.push(RankedResult {
                    chunk_id: get_text(&row, 0).parse().unwrap_or_default(),
                    document_id: get_text(&row, 1).parse().unwrap_or_default(),
                    content: get_text(&row, 2),
                    rank: results.len() as u32 + 1,
                });
            }
            results
        } else {
            Vec::new()
        };

        if embedding.is_some() && !config.use_vector {
            tracing::warn!(
                "Embedding provided but vector search is disabled in config; using FTS-only results"
            );
        }

        Ok(reciprocal_rank_fusion(fts_results, vector_results, config))
    }

    // ==================== Audit Log ====================

    async fn append_audit_event(&self, event: &AuditEvent<'_>) -> Result<(), DatabaseError> {
        let conn = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = fmt_ts(&Utc::now());
        let meta_str = event.metadata.and_then(|m| serde_json::to_string(m).ok());
        conn.execute(
            r#"INSERT INTO audit_log (
                id, created_at, event_type, user_id, session_id, job_id, actor,
                tool_name, input_hash, input_summary, outcome, error_msg,
                duration_ms, cost_usd, metadata
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)"#,
            params![
                id,
                now,
                event.event_type,
                opt_text(event.user_id),
                opt_text(event.session_id.as_ref().map(|u| u.to_string()).as_deref()),
                opt_text(event.job_id.as_ref().map(|u| u.to_string()).as_deref()),
                opt_text(event.actor),
                opt_text(event.tool_name),
                opt_text(event.input_hash),
                opt_text(event.input_summary),
                opt_text(event.outcome),
                opt_text(event.error_msg),
                event
                    .duration_ms
                    .map(libsql::Value::Integer)
                    .unwrap_or(libsql::Value::Null),
                opt_text(event.cost_usd),
                opt_text_owned(meta_str)
            ],
        )
        .await
        .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(())
    }

    async fn query_audit_log(&self, query: AuditQuery) -> Result<Vec<AuditLogRow>, DatabaseError> {
        let conn = self.connect()?;
        let limit = query.limit.unwrap_or(100).min(500);

        // Build WHERE clause from filter options.
        let mut clauses: Vec<String> = Vec::new();
        let mut positional: Vec<libsql::Value> = Vec::new();
        let mut idx = 1usize;

        if let Some(ref v) = query.since {
            clauses.push(format!("created_at >= ?{idx}"));
            positional.push(libsql::Value::Text(fmt_ts(v)));
            idx += 1;
        }
        if let Some(ref v) = query.until {
            clauses.push(format!("created_at <= ?{idx}"));
            positional.push(libsql::Value::Text(fmt_ts(v)));
            idx += 1;
        }
        if let Some(ref v) = query.user_id {
            clauses.push(format!("user_id = ?{idx}"));
            positional.push(libsql::Value::Text(v.clone()));
            idx += 1;
        }
        if let Some(ref v) = query.session_id {
            clauses.push(format!("session_id = ?{idx}"));
            positional.push(libsql::Value::Text(v.to_string()));
            idx += 1;
        }
        if let Some(ref v) = query.event_type {
            clauses.push(format!("event_type = ?{idx}"));
            positional.push(libsql::Value::Text(v.clone()));
            idx += 1;
        }
        if let Some(ref v) = query.outcome {
            clauses.push(format!("outcome = ?{idx}"));
            positional.push(libsql::Value::Text(v.clone()));
            idx += 1;
        }
        let _ = idx;

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

        let mut rows = conn
            .query(&sql, libsql::params_from_iter(positional))
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut result = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            // Columns: 0=id, 1=created_at, 2=event_type, 3=user_id, 4=session_id,
            //          5=job_id, 6=actor, 7=tool_name, 8=input_hash, 9=input_summary,
            //         10=outcome, 11=error_msg, 12=duration_ms, 13=cost_usd, 14=metadata
            let id: Uuid = get_text(&row, 0).parse().unwrap_or_default();
            let created_at = get_ts(&row, 1);
            let event_type = get_text(&row, 2);
            let user_id = get_opt_text(&row, 3);
            let session_id = get_opt_text(&row, 4).and_then(|s| s.parse::<Uuid>().ok());
            let job_id = get_opt_text(&row, 5).and_then(|s| s.parse::<Uuid>().ok());
            let actor = get_opt_text(&row, 6);
            let tool_name = get_opt_text(&row, 7);
            let input_hash = get_opt_text(&row, 8);
            let input_summary = get_opt_text(&row, 9);
            let outcome = get_opt_text(&row, 10);
            let error_msg = get_opt_text(&row, 11);
            let duration_ms: Option<i64> = row.get::<i64>(12).ok();
            let cost_usd = get_opt_text(&row, 13);
            let metadata = get_opt_text(&row, 14).and_then(|s| serde_json::from_str(&s).ok());

            result.push(AuditLogRow {
                id,
                created_at,
                event_type,
                user_id,
                session_id,
                job_id,
                actor,
                tool_name,
                input_hash,
                input_summary,
                outcome,
                error_msg,
                duration_ms,
                cost_usd,
                metadata,
            });
        }
        Ok(result)
    }

    async fn audit_log_summary(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<AuditEventCount>, DatabaseError> {
        let conn = self.connect()?;
        let (where_sql, positional): (String, Vec<libsql::Value>) = if let Some(ref v) = since {
            (
                "WHERE created_at >= ?1".to_string(),
                vec![libsql::Value::Text(fmt_ts(v))],
            )
        } else {
            (String::new(), vec![])
        };

        let sql = format!(
            r#"SELECT event_type, COUNT(*) AS count
               FROM audit_log
               {where_sql}
               GROUP BY event_type
               ORDER BY count DESC"#
        );

        let mut rows = conn
            .query(&sql, libsql::params_from_iter(positional))
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;

        let mut result = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?
        {
            result.push(AuditEventCount {
                event_type: get_text(&row, 0),
                count: get_i64(&row, 1),
            });
        }
        Ok(result)
    }

    async fn prune_audit_log(&self, older_than: DateTime<Utc>) -> Result<u64, DatabaseError> {
        let conn = self.connect()?;
        let n = conn
            .execute(
                "DELETE FROM audit_log WHERE created_at < ?1",
                libsql::params![fmt_ts(&older_than)],
            )
            .await
            .map_err(|e| DatabaseError::Query(e.to_string()))?;
        Ok(n)
    }
}

// ==================== Row conversion helpers ====================

fn row_to_memory_document(row: &libsql::Row) -> MemoryDocument {
    MemoryDocument {
        id: get_text(row, 0).parse().unwrap_or_default(),
        user_id: get_text(row, 1),
        agent_id: get_opt_text(row, 2).and_then(|s| s.parse().ok()),
        path: get_text(row, 3),
        content: get_text(row, 4),
        created_at: get_ts(row, 5),
        updated_at: get_ts(row, 6),
        metadata: get_json(row, 7),
    }
}

fn row_to_routine_libsql(row: &libsql::Row) -> Result<Routine, DatabaseError> {
    let trigger_type = get_text(row, 5);
    let trigger_config = get_json(row, 6);
    let action_type = get_text(row, 7);
    let action_config = get_json(row, 8);
    let cooldown_secs = get_i64(row, 9);
    let max_concurrent = get_i64(row, 10);
    let dedup_window_secs: Option<i64> = row.get::<i64>(11).ok();

    let trigger =
        Trigger::from_db(&trigger_type, trigger_config).map_err(DatabaseError::Serialization)?;
    let action = RoutineAction::from_db(&action_type, action_config)
        .map_err(DatabaseError::Serialization)?;

    Ok(Routine {
        id: get_text(row, 0).parse().unwrap_or_default(),
        name: get_text(row, 1),
        description: get_text(row, 2),
        user_id: get_text(row, 3),
        enabled: get_i64(row, 4) != 0,
        trigger,
        action,
        guardrails: RoutineGuardrails {
            cooldown: std::time::Duration::from_secs(cooldown_secs as u64),
            max_concurrent: max_concurrent as u32,
            dedup_window: dedup_window_secs.map(|s| std::time::Duration::from_secs(s as u64)),
        },
        notify: NotifyConfig {
            channel: get_opt_text(row, 12),
            user: get_text(row, 13),
            on_success: get_i64(row, 14) != 0,
            on_failure: get_i64(row, 15) != 0,
            on_attention: get_i64(row, 16) != 0,
        },
        state: get_json(row, 17),
        last_run_at: get_opt_ts(row, 18),
        next_fire_at: get_opt_ts(row, 19),
        run_count: get_i64(row, 20) as u64,
        consecutive_failures: get_i64(row, 21) as u32,
        created_at: get_ts(row, 22),
        updated_at: get_ts(row, 23),
    })
}

fn row_to_routine_run_libsql(row: &libsql::Row) -> Result<RoutineRun, DatabaseError> {
    let status_str = get_text(row, 5);
    let status: RunStatus = status_str
        .parse()
        .map_err(|e: String| DatabaseError::Serialization(e))?;

    Ok(RoutineRun {
        id: get_text(row, 0).parse().unwrap_or_default(),
        routine_id: get_text(row, 1).parse().unwrap_or_default(),
        trigger_type: get_text(row, 2),
        trigger_detail: get_opt_text(row, 3),
        started_at: get_ts(row, 4),
        completed_at: get_opt_ts(row, 6),
        status,
        result_summary: get_opt_text(row, 7),
        tokens_used: row.get::<i64>(8).ok().map(|v| v as i32),
        job_id: get_opt_text(row, 9).and_then(|s| s.parse().ok()),
        created_at: get_ts(row, 10),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: open an in-memory libSQL DB, run migrations on the same connection,
    /// and return that connection for direct testing.
    ///
    /// libSQL in-memory databases are per-connection, so we must run all DDL and DML
    /// on the same connection rather than going through `connect()` each time.
    async fn open_conn() -> Connection {
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(crate::db::libsql_migrations::SCHEMA)
            .await
            .unwrap();
        conn
    }

    async fn insert_conversation(conn: &Connection, id: Uuid, channel: &str, user_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO conversations (id, channel, user_id, started_at, last_activity, metadata)
             VALUES (?1, ?2, ?3, datetime('now'), datetime('now'), '{}')",
            params![id.to_string(), channel, user_id],
        )
        .await
        .unwrap();
    }

    async fn list_preview(
        conn: &Connection,
        user_id: &str,
        channel: Option<&str>,
        limit: i64,
    ) -> Vec<String> {
        let mut rows = if let Some(ch) = channel {
            conn.query(
                "SELECT c.id FROM conversations c WHERE c.user_id = ?1 AND c.channel = ?2
                 ORDER BY c.last_activity DESC LIMIT ?3",
                params![user_id, ch, limit],
            )
            .await
            .unwrap()
        } else {
            conn.query(
                "SELECT c.id FROM conversations c WHERE c.user_id = ?1
                 ORDER BY c.last_activity DESC LIMIT ?2",
                params![user_id, limit],
            )
            .await
            .unwrap()
        };

        let mut ids = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            ids.push(row.get::<String>(0).unwrap());
        }
        ids
    }

    #[tokio::test]
    async fn test_list_conversations_channel_filter() {
        let conn = open_conn().await;
        let user = "user1";
        let id_gw = Uuid::new_v4();
        let id_dc = Uuid::new_v4();

        insert_conversation(&conn, id_gw, "gateway", user).await;
        insert_conversation(&conn, id_dc, "discord", user).await;

        let gw_only = list_preview(&conn, user, Some("gateway"), 50).await;
        assert_eq!(gw_only.len(), 1);
        assert_eq!(gw_only[0], id_gw.to_string());

        let dc_only = list_preview(&conn, user, Some("discord"), 50).await;
        assert_eq!(dc_only.len(), 1);
        assert_eq!(dc_only[0], id_dc.to_string());

        let all = list_preview(&conn, user, None, 50).await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_list_conversations_cross_user_isolation() {
        let conn = open_conn().await;
        let id_alice = Uuid::new_v4();
        let id_bob = Uuid::new_v4();

        insert_conversation(&conn, id_alice, "discord", "alice").await;
        insert_conversation(&conn, id_bob, "discord", "bob").await;

        let alice = list_preview(&conn, "alice", None, 50).await;
        assert_eq!(alice.len(), 1);
        assert_eq!(alice[0], id_alice.to_string());

        let bob = list_preview(&conn, "bob", None, 50).await;
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0], id_bob.to_string());
    }

    // ── Incremental migrations ────────────────────────────────────────────────

    /// Build a LibSqlBackend backed by an in-memory DB and run its full
    /// migration logic (base schema + incremental).
    async fn open_backend() -> LibSqlBackend {
        use crate::db::Database;
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let backend = LibSqlBackend { db: Arc::new(db) };
        backend.run_migrations().await.unwrap();
        backend
    }

    #[tokio::test]
    async fn test_incremental_migrations_recorded() {
        // After run_migrations, every entry in INCREMENTAL must appear in _migrations.
        let backend = open_backend().await;
        let conn = backend.connect().unwrap();

        for (version, name, _sql) in crate::db::libsql_migrations::INCREMENTAL {
            let mut rows = conn
                .query(
                    "SELECT name FROM _migrations WHERE version = ?1",
                    params![*version],
                )
                .await
                .unwrap();
            let row = rows.next().await.unwrap();
            assert!(
                row.is_some(),
                "migration V{version} ({name}) not recorded in _migrations"
            );
            let stored_name: String = row.unwrap().get(0).unwrap();
            assert_eq!(stored_name, *name);
        }
    }

    #[tokio::test]
    async fn test_incremental_migrations_idempotent() {
        // Running run_migrations twice must not fail (no duplicate key errors).
        use crate::db::Database;
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let backend = LibSqlBackend { db: Arc::new(db) };
        backend.run_migrations().await.unwrap();
        backend.run_migrations().await.unwrap(); // second run — must succeed
    }

    #[tokio::test]
    async fn test_llm_call_latency_round_trips() {
        // record_llm_call stores latency_ms; get_llm_call_stats returns avg_latency_ms.
        use crate::db::Database;
        use crate::history::LlmCallRecord;
        use rust_decimal::Decimal;

        let backend = open_backend().await;

        let record = LlmCallRecord {
            job_id: None,
            conversation_id: None,
            provider: "test-provider",
            model: "test-model",
            input_tokens: 100,
            output_tokens: 50,
            cost: Decimal::new(5, 4), // 0.0005
            purpose: Some("complete"),
            latency_ms: 1234,
        };
        backend.record_llm_call(&record).await.unwrap();

        let record2 = LlmCallRecord {
            latency_ms: 766,
            ..record
        };
        backend.record_llm_call(&record2).await.unwrap();

        let stats = backend.get_llm_call_stats(None).await.unwrap();
        assert_eq!(stats.len(), 1);

        let s = &stats[0];
        assert_eq!(s.provider, "test-provider");
        assert_eq!(s.model, "test-model");
        assert_eq!(s.total_calls, 2);
        assert_eq!(s.total_input_tokens, 200);
        assert_eq!(s.total_output_tokens, 100);

        // avg of 1234 and 766 = 1000
        let avg = s.avg_latency_ms.expect("avg_latency_ms should be Some");
        assert!(
            (avg - 1000.0).abs() < 1.0,
            "expected avg ~1000 ms, got {avg}"
        );
    }
}
