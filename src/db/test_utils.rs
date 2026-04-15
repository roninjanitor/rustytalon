//! Mock database for unit tests.
//!
//! Provides a `MockDatabase` that implements `record_llm_call` (for TrackedProvider tests)
//! and stubs all other methods with `unimplemented!()`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::agent::BrokenTool;
use crate::agent::routine::{Routine, RoutineRun, RunStatus};
use crate::context::{ActionRecord, JobContext, JobState};
use crate::db::Database;
use crate::error::{DatabaseError, WorkspaceError};
use crate::history::{
    ConversationMessage, ConversationSummary, JobEventRecord, LlmCallRecord, SandboxJobRecord,
    SandboxJobSummary, SettingRow,
};
use crate::llm::tracked::LlmCallStats;
use crate::workspace::{MemoryChunk, MemoryDocument, SearchConfig, SearchResult, WorkspaceEntry};

/// Captured fields from a single `record_llm_call` invocation.
#[derive(Debug, Clone)]
pub(crate) struct CapturedLlmCall {
    pub provider: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost: rust_decimal::Decimal,
    pub latency_ms: u64,
    pub purpose: Option<String>,
}

/// A mock database that counts `record_llm_call` invocations and stubs everything else.
pub(crate) struct MockDatabase {
    pub call_count: AtomicU32,
    pub captured: std::sync::Mutex<Vec<CapturedLlmCall>>,
}

impl MockDatabase {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            call_count: AtomicU32::new(0),
            captured: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn recorded_calls(&self) -> u32 {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Return a clone of all captured LLM call records.
    pub fn captured_calls(&self) -> Vec<CapturedLlmCall> {
        self.captured.lock().unwrap().clone()
    }
}

#[async_trait]
impl Database for MockDatabase {
    async fn run_migrations(&self) -> Result<(), DatabaseError> {
        Ok(())
    }

    // === LLM Calls (implemented) ===

    async fn record_llm_call(&self, record: &LlmCallRecord<'_>) -> Result<Uuid, DatabaseError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.captured.lock().unwrap().push(CapturedLlmCall {
            provider: record.provider.to_string(),
            model: record.model.to_string(),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cost: record.cost,
            latency_ms: record.latency_ms,
            purpose: record.purpose.map(|s| s.to_string()),
        });
        Ok(Uuid::new_v4())
    }

    async fn get_llm_call_stats(&self) -> Result<Vec<LlmCallStats>, DatabaseError> {
        Ok(vec![])
    }

    // === Everything below is stubbed ===

    async fn create_conversation(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> Result<Uuid, DatabaseError> {
        unimplemented!()
    }
    async fn touch_conversation(&self, _: Uuid) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn add_conversation_message(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
    ) -> Result<Uuid, DatabaseError> {
        unimplemented!()
    }
    async fn ensure_conversation(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn list_conversations_with_preview(
        &self,
        _: &str,
        _: Option<&str>,
        _: i64,
    ) -> Result<Vec<ConversationSummary>, DatabaseError> {
        unimplemented!()
    }
    async fn get_or_create_assistant_conversation(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Uuid, DatabaseError> {
        unimplemented!()
    }
    async fn create_conversation_with_metadata(
        &self,
        _: &str,
        _: &str,
        _: &serde_json::Value,
    ) -> Result<Uuid, DatabaseError> {
        unimplemented!()
    }
    async fn list_conversation_messages_paginated(
        &self,
        _: Uuid,
        _: Option<DateTime<Utc>>,
        _: i64,
    ) -> Result<(Vec<ConversationMessage>, bool), DatabaseError> {
        unimplemented!()
    }
    async fn update_conversation_metadata_field(
        &self,
        _: Uuid,
        _: &str,
        _: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_conversation_metadata(
        &self,
        _: Uuid,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        unimplemented!()
    }
    async fn list_conversation_messages(
        &self,
        _: Uuid,
    ) -> Result<Vec<ConversationMessage>, DatabaseError> {
        unimplemented!()
    }
    async fn conversation_belongs_to_user(&self, _: Uuid, _: &str) -> Result<bool, DatabaseError> {
        unimplemented!()
    }

    async fn save_job(&self, _: &JobContext) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_job(&self, _: Uuid) -> Result<Option<JobContext>, DatabaseError> {
        unimplemented!()
    }
    async fn update_job_status(
        &self,
        _: Uuid,
        _: JobState,
        _: Option<&str>,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn mark_job_stuck(&self, _: Uuid) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_stuck_jobs(&self) -> Result<Vec<Uuid>, DatabaseError> {
        unimplemented!()
    }

    async fn save_action(&self, _: Uuid, _: &ActionRecord) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_job_actions(&self, _: Uuid) -> Result<Vec<ActionRecord>, DatabaseError> {
        unimplemented!()
    }

    async fn save_estimation_snapshot(
        &self,
        _: Uuid,
        _: &str,
        _: &[String],
        _: Decimal,
        _: i32,
        _: Decimal,
    ) -> Result<Uuid, DatabaseError> {
        unimplemented!()
    }
    async fn update_estimation_actuals(
        &self,
        _: Uuid,
        _: Decimal,
        _: i32,
        _: Option<Decimal>,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    async fn save_sandbox_job(&self, _: &SandboxJobRecord) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_sandbox_job(&self, _: Uuid) -> Result<Option<SandboxJobRecord>, DatabaseError> {
        unimplemented!()
    }
    async fn list_sandbox_jobs(&self) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        unimplemented!()
    }
    async fn update_sandbox_job_status(
        &self,
        _: Uuid,
        _: &str,
        _: Option<bool>,
        _: Option<&str>,
        _: Option<DateTime<Utc>>,
        _: Option<DateTime<Utc>>,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn cleanup_stale_sandbox_jobs(&self) -> Result<u64, DatabaseError> {
        unimplemented!()
    }
    async fn sandbox_job_summary(&self) -> Result<SandboxJobSummary, DatabaseError> {
        unimplemented!()
    }
    async fn list_sandbox_jobs_for_user(
        &self,
        _: &str,
    ) -> Result<Vec<SandboxJobRecord>, DatabaseError> {
        unimplemented!()
    }
    async fn sandbox_job_summary_for_user(
        &self,
        _: &str,
    ) -> Result<SandboxJobSummary, DatabaseError> {
        unimplemented!()
    }
    async fn sandbox_job_belongs_to_user(&self, _: Uuid, _: &str) -> Result<bool, DatabaseError> {
        unimplemented!()
    }
    async fn update_sandbox_job_mode(&self, _: Uuid, _: &str) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_sandbox_job_mode(&self, _: Uuid) -> Result<Option<String>, DatabaseError> {
        unimplemented!()
    }

    async fn save_job_event(
        &self,
        _: Uuid,
        _: &str,
        _: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn list_job_events(&self, _: Uuid) -> Result<Vec<JobEventRecord>, DatabaseError> {
        unimplemented!()
    }

    async fn create_routine(&self, _: &Routine) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_routine(&self, _: Uuid) -> Result<Option<Routine>, DatabaseError> {
        unimplemented!()
    }
    async fn get_routine_by_name(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<Routine>, DatabaseError> {
        unimplemented!()
    }
    async fn list_routines(&self, _: &str) -> Result<Vec<Routine>, DatabaseError> {
        unimplemented!()
    }
    async fn list_event_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        unimplemented!()
    }
    async fn list_due_cron_routines(&self) -> Result<Vec<Routine>, DatabaseError> {
        unimplemented!()
    }
    async fn update_routine(&self, _: &Routine) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn update_routine_runtime(
        &self,
        _: Uuid,
        _: DateTime<Utc>,
        _: Option<DateTime<Utc>>,
        _: u64,
        _: u32,
        _: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn delete_routine(&self, _: Uuid) -> Result<bool, DatabaseError> {
        unimplemented!()
    }

    async fn create_routine_run(&self, _: &RoutineRun) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn complete_routine_run(
        &self,
        _: Uuid,
        _: RunStatus,
        _: Option<&str>,
        _: Option<i32>,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn list_routine_runs(&self, _: Uuid, _: i64) -> Result<Vec<RoutineRun>, DatabaseError> {
        unimplemented!()
    }
    async fn count_running_routine_runs(&self, _: Uuid) -> Result<i64, DatabaseError> {
        unimplemented!()
    }

    async fn record_tool_failure(&self, _: &str, _: &str) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn get_broken_tools(&self, _: i32) -> Result<Vec<BrokenTool>, DatabaseError> {
        unimplemented!()
    }
    async fn mark_tool_repaired(&self, _: &str) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn increment_repair_attempts(&self, _: &str) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    async fn get_setting(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        unimplemented!()
    }
    async fn get_setting_full(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Option<SettingRow>, DatabaseError> {
        unimplemented!()
    }
    async fn set_setting(
        &self,
        _: &str,
        _: &str,
        _: &serde_json::Value,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn delete_setting(&self, _: &str, _: &str) -> Result<bool, DatabaseError> {
        unimplemented!()
    }
    async fn list_settings(&self, _: &str) -> Result<Vec<SettingRow>, DatabaseError> {
        unimplemented!()
    }
    async fn get_all_settings(
        &self,
        _: &str,
    ) -> Result<HashMap<String, serde_json::Value>, DatabaseError> {
        unimplemented!()
    }
    async fn set_all_settings(
        &self,
        _: &str,
        _: &HashMap<String, serde_json::Value>,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
    async fn has_settings(&self, _: &str) -> Result<bool, DatabaseError> {
        unimplemented!()
    }

    async fn get_document_by_path(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        unimplemented!()
    }
    async fn get_document_by_id(&self, _: Uuid) -> Result<MemoryDocument, WorkspaceError> {
        unimplemented!()
    }
    async fn get_or_create_document_by_path(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: &str,
    ) -> Result<MemoryDocument, WorkspaceError> {
        unimplemented!()
    }
    async fn update_document(&self, _: Uuid, _: &str) -> Result<(), WorkspaceError> {
        unimplemented!()
    }
    async fn delete_document_by_path(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: &str,
    ) -> Result<(), WorkspaceError> {
        unimplemented!()
    }
    async fn list_directory(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: &str,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        unimplemented!()
    }
    async fn list_all_paths(
        &self,
        _: &str,
        _: Option<Uuid>,
    ) -> Result<Vec<String>, WorkspaceError> {
        unimplemented!()
    }
    async fn list_documents(
        &self,
        _: &str,
        _: Option<Uuid>,
    ) -> Result<Vec<MemoryDocument>, WorkspaceError> {
        unimplemented!()
    }

    async fn delete_chunks(&self, _: Uuid) -> Result<(), WorkspaceError> {
        unimplemented!()
    }
    async fn insert_chunk(
        &self,
        _: Uuid,
        _: i32,
        _: &str,
        _: Option<&[f32]>,
    ) -> Result<Uuid, WorkspaceError> {
        unimplemented!()
    }
    async fn update_chunk_embedding(&self, _: Uuid, _: &[f32]) -> Result<(), WorkspaceError> {
        unimplemented!()
    }
    async fn get_chunks_without_embeddings(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: usize,
    ) -> Result<Vec<MemoryChunk>, WorkspaceError> {
        unimplemented!()
    }

    async fn hybrid_search(
        &self,
        _: &str,
        _: Option<Uuid>,
        _: &str,
        _: Option<&[f32]>,
        _: &SearchConfig,
    ) -> Result<Vec<SearchResult>, WorkspaceError> {
        unimplemented!()
    }
}
