//! Container lifecycle management for sandboxed jobs.
//!
//! Extends the existing `SandboxManager` infrastructure to support persistent
//! containers with their own agent loops (as opposed to ephemeral per-command containers).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::OrchestratorError;
use crate::orchestrator::auth::TokenStore;
use crate::sandbox::connect_docker;

/// Which mode a sandbox container runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobMode {
    /// Standard RustyTalon worker with proxied LLM calls.
    Worker,
    /// Claude Code bridge that spawns the `claude` CLI directly.
    ClaudeCode,
}

impl JobMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::ClaudeCode => "claude_code",
        }
    }
}

impl std::fmt::Display for JobMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Configuration for the container job manager.
#[derive(Debug, Clone)]
pub struct ContainerJobConfig {
    /// Docker image for worker containers.
    pub image: String,
    /// Default memory limit in MB.
    pub memory_limit_mb: u64,
    /// Default CPU shares.
    pub cpu_shares: u32,
    /// Port the orchestrator internal API listens on.
    pub orchestrator_port: u16,
    /// Host directory containing Claude auth config (mounted read-only for ClaudeCode mode).
    pub claude_config_dir: Option<PathBuf>,
    /// Claude model to use in ClaudeCode mode.
    pub claude_code_model: String,
    /// Maximum turns for Claude Code.
    pub claude_code_max_turns: u32,
    /// Memory limit in MB for Claude Code containers (heavier than workers).
    pub claude_code_memory_limit_mb: u64,
    /// Allowed tool patterns for Claude Code (passed as CLAUDE_CODE_ALLOWED_TOOLS env var).
    pub claude_code_allowed_tools: Vec<String>,
}

impl Default for ContainerJobConfig {
    fn default() -> Self {
        Self {
            image: "rustytalon-worker:latest".to_string(),
            memory_limit_mb: 2048,
            cpu_shares: 1024,
            orchestrator_port: 50051,
            claude_config_dir: None,
            claude_code_model: "sonnet".to_string(),
            claude_code_max_turns: 50,
            claude_code_memory_limit_mb: 4096,
            claude_code_allowed_tools: crate::config::ClaudeCodeConfig::default().allowed_tools,
        }
    }
}

/// State of a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Creating,
    Running,
    Stopped,
    Failed,
}

impl std::fmt::Display for ContainerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Creating => write!(f, "creating"),
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Handle to a running container job.
#[derive(Debug, Clone)]
pub struct ContainerHandle {
    pub job_id: Uuid,
    pub container_id: String,
    pub state: ContainerState,
    pub mode: JobMode,
    pub created_at: DateTime<Utc>,
    pub project_dir: Option<PathBuf>,
    pub task_description: String,
    /// Completion result from the worker (set when the worker reports done).
    pub completion_result: Option<CompletionResult>,
    // NOTE: auth_token is intentionally NOT in this struct.
    // It lives only in the TokenStore (never logged, serialized, or persisted).
}

/// Result reported by a worker on completion.
#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub success: bool,
    pub message: Option<String>,
}

/// Manages the lifecycle of Docker containers for sandboxed job execution.
pub struct ContainerJobManager {
    config: ContainerJobConfig,
    token_store: TokenStore,
    containers: Arc<RwLock<HashMap<Uuid, ContainerHandle>>>,
    /// Cached result of the last Docker reachability check, refreshed periodically
    /// by a background task (see `refresh_docker_health`). Starts `false` so the
    /// tool layer never advertises sandbox capability before the first check runs.
    docker_available: Arc<AtomicBool>,
}

impl ContainerJobManager {
    pub fn new(config: ContainerJobConfig, token_store: TokenStore) -> Self {
        Self {
            config,
            token_store,
            containers: Arc::new(RwLock::new(HashMap::new())),
            docker_available: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether Docker was reachable as of the last background health check.
    ///
    /// This is a cached flag, not a live probe -- callers that need certainty
    /// (e.g. actually creating a container) should still handle `create_job`
    /// failures. This exists so the `create_job` tool can decide, cheaply and
    /// synchronously, whether to advertise sandboxed-container capability to
    /// the LLM at all.
    pub fn docker_available(&self) -> bool {
        self.docker_available.load(Ordering::Relaxed)
    }

    /// Ping Docker and update the cached availability flag. Intended to be called
    /// periodically from a background task started at startup.
    ///
    /// Failures log at `warn!` on *every* poll (not just on the
    /// available-to-unavailable transition) -- a Docker daemon that is
    /// unreachable from the very first check (e.g. a mounted socket the
    /// process's non-root user can't actually open) never has an
    /// available-to-unavailable edge to log, so it must warn every cycle or
    /// `create_job` silently stays on the local-only fallback path forever
    /// with nothing in the logs to explain why.
    pub async fn refresh_docker_health(&self) {
        let result = connect_docker().await;
        let available = result.is_ok();
        let was_available = self.docker_available.swap(available, Ordering::Relaxed);

        match result {
            Ok(_) if !was_available => {
                tracing::info!("Docker is now reachable; sandbox job execution enabled");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Docker unreachable; create_job will fall back to local execution"
                );
            }
        }
    }

    /// Create and start a new container for a job.
    ///
    /// The caller provides the `job_id` so it can be persisted to the database
    /// before the container is created. Returns the auth token for the worker.
    pub async fn create_job(
        &self,
        job_id: Uuid,
        task: &str,
        project_dir: Option<PathBuf>,
        mode: JobMode,
    ) -> Result<String, OrchestratorError> {
        // Generate auth token (stored in TokenStore, never logged)
        let token = self.token_store.create_token(job_id).await;

        // Record the handle
        let handle = ContainerHandle {
            job_id,
            container_id: String::new(), // set after container creation
            state: ContainerState::Creating,
            mode,
            created_at: Utc::now(),
            project_dir: project_dir.clone(),
            task_description: task.to_string(),
            completion_result: None,
        };
        self.containers.write().await.insert(job_id, handle);

        // Run the actual container creation. On any failure, revoke the token
        // and remove the handle so we don't leak resources.
        match self
            .create_job_inner(job_id, &token, project_dir, mode)
            .await
        {
            Ok(()) => Ok(token),
            Err(e) => {
                self.token_store.revoke(job_id).await;
                self.containers.write().await.remove(&job_id);
                Err(e)
            }
        }
    }

    /// Inner implementation of container creation (separated for cleanup).
    async fn create_job_inner(
        &self,
        job_id: Uuid,
        token: &str,
        project_dir: Option<PathBuf>,
        mode: JobMode,
    ) -> Result<(), OrchestratorError> {
        // Connect to Docker
        let docker = connect_docker()
            .await
            .map_err(|e| OrchestratorError::Docker {
                reason: e.to_string(),
            })?;

        // Build container configuration
        let orchestrator_host = if cfg!(target_os = "linux") {
            "172.17.0.1"
        } else {
            "host.docker.internal"
        };

        let orchestrator_url = format!(
            "http://{}:{}",
            orchestrator_host, self.config.orchestrator_port
        );

        let mut env_vec = vec![
            format!("RUSTYTALON_WORKER_TOKEN={}", token),
            format!("RUSTYTALON_JOB_ID={}", job_id),
            format!("RUSTYTALON_ORCHESTRATOR_URL={}", orchestrator_url),
        ];

        // Build volume mounts (validate project_dir stays within ~/.rustytalon/projects/)
        let mut binds = Vec::new();
        if let Some(ref dir) = project_dir {
            let canonical =
                dir.canonicalize()
                    .map_err(|e| OrchestratorError::ContainerCreationFailed {
                        job_id,
                        reason: format!(
                            "failed to canonicalize project dir {}: {}",
                            dir.display(),
                            e
                        ),
                    })?;
            let projects_base = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rustytalon")
                .join("projects");
            if let Ok(canonical_base) = projects_base.canonicalize()
                && !canonical.starts_with(&canonical_base)
            {
                return Err(OrchestratorError::ContainerCreationFailed {
                    job_id,
                    reason: format!(
                        "project directory {} is outside allowed base {}",
                        canonical.display(),
                        canonical_base.display()
                    ),
                });
            }
            binds.push(format!("{}:/workspace:rw", canonical.display()));
            env_vec.push("RUSTYTALON_WORKSPACE=/workspace".to_string());
        }

        // Claude Code mode: mount host ~/.claude read-only for auth,
        // and pass the tool allowlist so the bridge can write settings.json.
        if mode == JobMode::ClaudeCode {
            if let Some(ref claude_dir) = self.config.claude_config_dir {
                binds.push(format!("{}:/home/sandbox/.claude:ro", claude_dir.display()));
            }
            if !self.config.claude_code_allowed_tools.is_empty() {
                env_vec.push(format!(
                    "CLAUDE_CODE_ALLOWED_TOOLS={}",
                    self.config.claude_code_allowed_tools.join(",")
                ));
            }
        }

        // Memory limit: Claude Code gets more memory
        let memory_mb = match mode {
            JobMode::ClaudeCode => self.config.claude_code_memory_limit_mb,
            JobMode::Worker => self.config.memory_limit_mb,
        };

        // Create the container
        use bollard::models::{ContainerCreateBody, HostConfig};
        use bollard::query_parameters::CreateContainerOptionsBuilder;

        let host_config = HostConfig {
            binds: if binds.is_empty() { None } else { Some(binds) },
            memory: Some((memory_mb * 1024 * 1024) as i64),
            cpu_shares: Some(self.config.cpu_shares as i64),
            network_mode: Some("bridge".to_string()),
            extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
            cap_drop: Some(vec!["ALL".to_string()]),
            cap_add: Some(vec!["CHOWN".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            tmpfs: Some(
                [("/tmp".to_string(), "size=512M".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        // Build CMD based on mode
        let cmd = match mode {
            JobMode::Worker => vec![
                "worker".to_string(),
                "--job-id".to_string(),
                job_id.to_string(),
                "--orchestrator-url".to_string(),
                orchestrator_url,
            ],
            JobMode::ClaudeCode => vec![
                "claude-bridge".to_string(),
                "--job-id".to_string(),
                job_id.to_string(),
                "--orchestrator-url".to_string(),
                orchestrator_url,
                "--max-turns".to_string(),
                self.config.claude_code_max_turns.to_string(),
                "--model".to_string(),
                self.config.claude_code_model.clone(),
            ],
        };

        let container_config = ContainerCreateBody {
            image: Some(self.config.image.clone()),
            cmd: Some(cmd),
            env: Some(env_vec),
            host_config: Some(host_config),
            user: Some("1000:1000".to_string()),
            working_dir: Some("/workspace".to_string()),
            ..Default::default()
        };

        let container_name = match mode {
            JobMode::Worker => format!("rustytalon-worker-{}", job_id),
            JobMode::ClaudeCode => format!("rustytalon-claude-{}", job_id),
        };
        let options = CreateContainerOptionsBuilder::default()
            .name(&container_name)
            .build();

        let response = docker
            .create_container(Some(options), container_config)
            .await
            .map_err(|e| OrchestratorError::ContainerCreationFailed {
                job_id,
                reason: e.to_string(),
            })?;

        let container_id = response.id;

        // Start the container
        docker
            .start_container(&container_id, None)
            .await
            .map_err(|e| OrchestratorError::ContainerCreationFailed {
                job_id,
                reason: format!("failed to start container: {}", e),
            })?;

        // Update handle with container ID
        if let Some(handle) = self.containers.write().await.get_mut(&job_id) {
            handle.container_id = container_id;
            handle.state = ContainerState::Running;
        }

        tracing::info!(
            job_id = %job_id,
            "Created and started worker container"
        );

        Ok(())
    }

    /// Stop a running container job.
    pub async fn stop_job(&self, job_id: Uuid) -> Result<(), OrchestratorError> {
        let container_id = {
            let containers = self.containers.read().await;
            containers
                .get(&job_id)
                .map(|h| h.container_id.clone())
                .ok_or(OrchestratorError::ContainerNotFound { job_id })?
        };

        if container_id.is_empty() {
            return Err(OrchestratorError::InvalidContainerState {
                job_id,
                state: "creating (no container ID yet)".to_string(),
            });
        }

        let docker = connect_docker()
            .await
            .map_err(|e| OrchestratorError::Docker {
                reason: e.to_string(),
            })?;

        // Stop the container (10 second grace period)
        if let Err(e) = docker
            .stop_container(
                &container_id,
                Some(
                    bollard::query_parameters::StopContainerOptionsBuilder::default()
                        .t(10)
                        .build(),
                ),
            )
            .await
        {
            tracing::warn!(job_id = %job_id, error = %e, "Failed to stop container (may already be stopped)");
        }

        // Remove the container
        if let Err(e) = docker
            .remove_container(
                &container_id,
                Some(
                    bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .build(),
                ),
            )
            .await
        {
            tracing::warn!(job_id = %job_id, error = %e, "Failed to remove container (may require manual cleanup)");
        }

        // Update state
        if let Some(handle) = self.containers.write().await.get_mut(&job_id) {
            handle.state = ContainerState::Stopped;
        }

        // Revoke the auth token
        self.token_store.revoke(job_id).await;

        tracing::info!(job_id = %job_id, "Stopped worker container");

        Ok(())
    }

    /// Mark a job as complete with a result. The container is stopped but the
    /// handle is kept so `CreateJobTool` can read the completion message.
    pub async fn complete_job(
        &self,
        job_id: Uuid,
        result: CompletionResult,
    ) -> Result<(), OrchestratorError> {
        // Store the result before stopping. A worker-reported failure gets its
        // own `Failed` state rather than `Stopped`, so `CreateJobTool` and
        // anything else inspecting `ContainerHandle::state` (web UI, job
        // listings) can distinguish "ran to completion but failed" from "ran
        // to completion successfully" without inspecting `completion_result`.
        {
            let mut containers = self.containers.write().await;
            if let Some(handle) = containers.get_mut(&job_id) {
                handle.state = if result.success {
                    ContainerState::Stopped
                } else {
                    ContainerState::Failed
                };
                handle.completion_result = Some(result);
            }
        }

        // Stop container and revoke token (but keep handle in map)
        let container_id = {
            let containers = self.containers.read().await;
            containers.get(&job_id).map(|h| h.container_id.clone())
        };
        if let Some(cid) = container_id
            && !cid.is_empty()
        {
            match connect_docker().await {
                Ok(docker) => {
                    if let Err(e) = docker
                        .stop_container(
                            &cid,
                            Some(
                                bollard::query_parameters::StopContainerOptionsBuilder::default()
                                    .t(5)
                                    .build(),
                            ),
                        )
                        .await
                    {
                        tracing::warn!(job_id = %job_id, error = %e, "Failed to stop completed container");
                    }
                    if let Err(e) = docker
                        .remove_container(
                            &cid,
                            Some(
                                bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                                    .force(true)
                                    .build(),
                            ),
                        )
                        .await
                    {
                        tracing::warn!(job_id = %job_id, error = %e, "Failed to remove completed container");
                    }
                }
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "Failed to connect to Docker for container cleanup");
                }
            }
        }
        self.token_store.revoke(job_id).await;

        tracing::info!(job_id = %job_id, "Completed worker container");
        Ok(())
    }

    /// Remove a completed job handle from memory (called after result is read).
    pub async fn cleanup_job(&self, job_id: Uuid) {
        self.containers.write().await.remove(&job_id);
    }

    /// Detect containers that have died (crashed, OOM-killed, entrypoint
    /// failure, or were removed out-of-band) without the worker ever calling
    /// back to `complete_job`. Without this, a dead container's handle stays
    /// `Running` forever from `ContainerJobManager`'s perspective, and
    /// `CreateJobTool::execute_sandbox`'s poll loop only notices via its own
    /// 10-minute hard timeout -- a much worse failure experience than the
    /// ~30s the bootstrap-failure case produces. Intended to be polled
    /// periodically from a background task (mirrors `refresh_docker_health`).
    pub async fn reap_dead_containers(&self) {
        // Snapshot handles that are still "in flight" from our perspective --
        // a worker that already reported completion (Stopped/Failed) doesn't
        // need reaping, and Creating handles may not have a container_id yet.
        let candidates: Vec<(Uuid, String)> = {
            let containers = self.containers.read().await;
            containers
                .values()
                .filter(|h| h.state == ContainerState::Running && !h.container_id.is_empty())
                .map(|h| (h.job_id, h.container_id.clone()))
                .collect()
        };

        if candidates.is_empty() {
            return;
        }

        let docker = match connect_docker().await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "reap_dead_containers: failed to connect to Docker");
                return;
            }
        };

        for (job_id, container_id) in candidates {
            let reason = match docker.inspect_container(&container_id, None).await {
                Ok(resp) => {
                    let state = resp.state.unwrap_or_default();
                    if state.running.unwrap_or(true) {
                        continue; // still alive, nothing to do
                    }
                    Some(describe_dead_container(&state))
                }
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404, ..
                }) => Some("container no longer exists (removed out-of-band)".to_string()),
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "reap_dead_containers: inspect failed");
                    continue;
                }
            };
            let Some(reason) = reason else { continue };

            let mut containers = self.containers.write().await;
            if let Some(handle) = containers.get_mut(&job_id) {
                // Re-check state under the lock: the worker may have called
                // complete_job in the gap between our snapshot and now.
                if handle.state == ContainerState::Running {
                    tracing::warn!(
                        job_id = %job_id,
                        reason = %reason,
                        "Detected dead container without a worker completion callback"
                    );
                    handle.state = ContainerState::Failed;
                    handle.completion_result = Some(CompletionResult {
                        success: false,
                        message: Some(reason),
                    });
                }
            }
        }
    }

    /// Get the handle for a job.
    pub async fn get_handle(&self, job_id: Uuid) -> Option<ContainerHandle> {
        self.containers.read().await.get(&job_id).cloned()
    }

    /// List all active container jobs.
    pub async fn list_jobs(&self) -> Vec<ContainerHandle> {
        self.containers.read().await.values().cloned().collect()
    }

    /// Get a reference to the token store.
    pub fn token_store(&self) -> &TokenStore {
        &self.token_store
    }
}

/// Build a human-readable failure reason from a dead container's inspected
/// state, for jobs whose worker never called back to `complete_job`.
fn describe_dead_container(state: &bollard::models::ContainerState) -> String {
    let status = state
        .status
        .map(|s| format!("{:?}", s).to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());

    if state.oom_killed.unwrap_or(false) {
        return format!("container was OOM-killed (status: {status})");
    }
    if let Some(err) = state.error.as_ref().filter(|e| !e.is_empty()) {
        return format!("container exited with error: {err} (status: {status})");
    }
    let exit_code = state.exit_code.unwrap_or(-1);
    format!("container exited unexpectedly with code {exit_code} (status: {status})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe_dead_container_oom_killed() {
        let state = bollard::models::ContainerState {
            status: Some(bollard::models::ContainerStateStatusEnum::EXITED),
            oom_killed: Some(true),
            exit_code: Some(137),
            ..Default::default()
        };
        let msg = describe_dead_container(&state);
        assert!(msg.contains("OOM-killed"), "got: {msg}");
    }

    #[test]
    fn test_describe_dead_container_error_message() {
        let state = bollard::models::ContainerState {
            status: Some(bollard::models::ContainerStateStatusEnum::EXITED),
            error: Some("no such image".to_string()),
            ..Default::default()
        };
        let msg = describe_dead_container(&state);
        assert!(msg.contains("no such image"), "got: {msg}");
    }

    #[test]
    fn test_describe_dead_container_exit_code_fallback() {
        let state = bollard::models::ContainerState {
            status: Some(bollard::models::ContainerStateStatusEnum::EXITED),
            exit_code: Some(1),
            ..Default::default()
        };
        let msg = describe_dead_container(&state);
        assert!(msg.contains('1'), "got: {msg}");
    }

    #[tokio::test]
    async fn test_reap_dead_containers_noop_when_nothing_running() {
        // No Running candidates -- must return without touching Docker at all,
        // so this must not hang or error even with no daemon reachable.
        let manager = ContainerJobManager::new(
            ContainerJobConfig::default(),
            crate::orchestrator::auth::TokenStore::new(),
        );
        let job_id = Uuid::new_v4();
        manager.containers.write().await.insert(
            job_id,
            ContainerHandle {
                job_id,
                container_id: String::new(),
                state: ContainerState::Creating, // no container_id yet
                mode: JobMode::Worker,
                created_at: Utc::now(),
                project_dir: None,
                task_description: "test".to_string(),
                completion_result: None,
            },
        );

        manager.reap_dead_containers().await;

        let handle = manager.get_handle(job_id).await.unwrap();
        assert_eq!(handle.state, ContainerState::Creating);
    }

    #[test]
    fn test_container_job_config_default() {
        let config = ContainerJobConfig::default();
        assert_eq!(config.orchestrator_port, 50051);
        assert_eq!(config.memory_limit_mb, 2048);
    }

    #[test]
    fn test_container_state_display() {
        assert_eq!(ContainerState::Running.to_string(), "running");
        assert_eq!(ContainerState::Stopped.to_string(), "stopped");
        assert_eq!(ContainerState::Failed.to_string(), "failed");
    }

    #[tokio::test]
    async fn test_complete_job_sets_failed_state_on_failure() {
        let manager = ContainerJobManager::new(
            ContainerJobConfig::default(),
            crate::orchestrator::auth::TokenStore::new(),
        );
        let job_id = Uuid::new_v4();
        manager.containers.write().await.insert(
            job_id,
            ContainerHandle {
                job_id,
                container_id: String::new(),
                state: ContainerState::Running,
                mode: JobMode::Worker,
                created_at: Utc::now(),
                project_dir: None,
                task_description: "test".to_string(),
                completion_result: None,
            },
        );

        manager
            .complete_job(
                job_id,
                CompletionResult {
                    success: false,
                    message: Some("build failed".to_string()),
                },
            )
            .await
            .unwrap();

        let handle = manager.get_handle(job_id).await.unwrap();
        assert_eq!(handle.state, ContainerState::Failed);
        assert_eq!(
            handle.completion_result.unwrap().message.unwrap(),
            "build failed"
        );
    }

    #[tokio::test]
    async fn test_complete_job_sets_stopped_state_on_success() {
        let manager = ContainerJobManager::new(
            ContainerJobConfig::default(),
            crate::orchestrator::auth::TokenStore::new(),
        );
        let job_id = Uuid::new_v4();
        manager.containers.write().await.insert(
            job_id,
            ContainerHandle {
                job_id,
                container_id: String::new(),
                state: ContainerState::Running,
                mode: JobMode::Worker,
                created_at: Utc::now(),
                project_dir: None,
                task_description: "test".to_string(),
                completion_result: None,
            },
        );

        manager
            .complete_job(
                job_id,
                CompletionResult {
                    success: true,
                    message: None,
                },
            )
            .await
            .unwrap();

        let handle = manager.get_handle(job_id).await.unwrap();
        assert_eq!(handle.state, ContainerState::Stopped);
    }
}
