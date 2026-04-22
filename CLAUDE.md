# RustyTalon Development Guide

## Project Overview

**RustyTalon** is a secure personal AI assistant that protects your data and expands its capabilities on the fly.

### Core Philosophy
- **User-first security** - Your data stays yours, encrypted and local
- **Self-expanding** - Build new tools dynamically without vendor dependency
- **Defense in depth** - Multiple security layers against prompt injection and data exfiltration
- **Always available** - Multi-channel access with proactive background execution

### Features
- **Multi-channel input**: TUI (Ratatui), HTTP webhooks, WASM channels (Telegram, Slack), web gateway
- **Parallel job execution** with state machine and self-repair for stuck jobs
- **Sandbox execution**: Docker container isolation with orchestrator/worker pattern
- **Claude Code mode**: Delegate jobs to Claude CLI inside containers
- **Routines**: Scheduled (cron) and reactive (event, webhook) task execution
- **Web gateway**: Browser UI with SSE/WebSocket real-time streaming
- **Extension management**: Install, auth, activate MCP/WASM extensions
- **Extensible tools**: Built-in tools, WASM sandbox, MCP client, dynamic builder
- **Persistent memory**: Workspace with hybrid search (FTS + vector via RRF)
- **Prompt injection defense**: Sanitizer, validator, policy rules, leak detection
- **Heartbeat system**: Proactive periodic execution with checklist

## Build & Test

```bash
# Format code
cargo fmt

# Lint (address warnings before committing)
cargo clippy --all --benches --tests --examples --all-features

# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run with logging
RUST_LOG=rustytalon=debug cargo run
```

## Project Structure

```
src/
├── lib.rs              # Library root, module declarations
├── main.rs             # Entry point, CLI args, startup
├── config.rs           # Configuration from env vars
├── error.rs            # Error types (thiserror)
│
├── agent/              # Core agent logic
│   ├── agent_loop.rs   # Main Agent struct, message handling loop
│   ├── router.rs       # MessageIntent classification
│   ├── scheduler.rs    # Parallel job scheduling
│   ├── worker.rs       # Per-job execution with LLM reasoning
│   ├── self_repair.rs  # Stuck job detection and recovery
│   ├── heartbeat.rs    # Proactive periodic execution
│   ├── session.rs      # Session/thread/turn model with state machine
│   ├── session_manager.rs # Thread/session lifecycle management
│   ├── compaction.rs   # Context window management with turn summarization
│   ├── context_monitor.rs # Memory pressure detection
│   ├── undo.rs         # Turn-based undo/redo with checkpoints
│   ├── submission.rs   # Submission parsing (undo, redo, compact, clear, etc.)
│   ├── task.rs         # Sub-task execution framework
│   ├── routine.rs      # Routine types (Trigger, Action, Guardrails)
│   └── routine_engine.rs # Routine execution (cron ticker, event matcher)
│
├── channels/           # Multi-channel input
│   ├── channel.rs      # Channel trait, IncomingMessage, OutgoingResponse
│   ├── manager.rs      # ChannelManager merges streams
│   ├── cli/            # Full TUI with Ratatui
│   │   ├── mod.rs      # TuiChannel implementation
│   │   ├── app.rs      # Application state
│   │   ├── render.rs   # UI rendering
│   │   ├── events.rs   # Input handling
│   │   ├── overlay.rs  # Approval overlays
│   │   └── composer.rs # Message composition
│   ├── http.rs         # HTTP webhook (axum) with secret validation
│   ├── repl.rs         # Simple REPL (for testing)
│   ├── web/            # Web gateway (browser UI)
│   │   ├── mod.rs      # Gateway builder, startup
│   │   ├── server.rs   # Axum router, 40+ API endpoints
│   │   ├── sse.rs      # SSE broadcast manager
│   │   ├── ws.rs       # WebSocket gateway + connection tracking
│   │   ├── types.rs    # Request/response types, SseEvent enum
│   │   ├── auth.rs     # Bearer token auth middleware
│   │   ├── log_layer.rs # Tracing layer for log streaming
│   │   └── static/     # HTML, CSS, JS (single-page app)
│   └── wasm/           # WASM channel runtime
│       ├── mod.rs
│       ├── broker.rs   # Host-side persistent connection broker (WebSocket, long-poll, SSE)
│       ├── bundled.rs  # Bundled channel discovery
│       └── wrapper.rs  # Channel trait wrapper for WASM modules
│
├── orchestrator/       # Internal HTTP API for sandbox containers
│   ├── mod.rs
│   ├── api.rs          # Axum endpoints (LLM proxy, events, prompts)
│   ├── auth.rs         # Per-job bearer token store
│   └── job_manager.rs  # Container lifecycle (create, stop, cleanup)
│
├── worker/             # Runs inside Docker containers
│   ├── mod.rs
│   ├── runtime.rs      # Worker execution loop (tool calls, LLM)
│   ├── claude_bridge.rs # Claude Code bridge (spawns claude CLI)
│   ├── api.rs          # HTTP client to orchestrator
│   └── proxy_llm.rs    # LlmProvider that proxies through orchestrator
│
├── safety/             # Prompt injection defense
│   ├── sanitizer.rs    # Pattern detection, content escaping
│   ├── validator.rs    # Input validation (length, encoding, patterns)
│   ├── policy.rs       # PolicyRule system with severity/actions
│   └── leak_detector.rs # Secret detection (API keys, tokens, etc.)
│
├── llm/                # Multi-provider LLM integration
│   ├── provider.rs     # LlmProvider trait, message types
│   ├── rig_adapter.rs  # rig-core adapter (Anthropic, OpenAI, Ollama, compatible)
│   ├── reasoning.rs    # Planning, tool selection, evaluation
│   ├── costs.rs        # Per-model cost tables
│   ├── tracked.rs      # TrackedProvider: retry + cost recording wrapper
│   ├── failover.rs     # Multi-provider failover chain
│   ├── retry.rs        # Retry policy with exponential backoff
│   ├── test_utils.rs   # MockProvider for tests
│   └── routing/        # Smart routing
│       ├── mod.rs
│       ├── analyzer.rs # Query complexity scoring
│       ├── strategy.rs # Routing strategies (balanced, cost, quality, local_first)
│       ├── router.rs   # SmartRouter with health tracking
│       └── quality.rs  # Response quality validation
│
├── tools/              # Extensible tool system
│   ├── tool.rs         # Tool trait, ToolOutput, ToolError
│   ├── registry.rs     # ToolRegistry for discovery
│   ├── sandbox.rs      # Process-based sandbox (stub, superseded by wasm/)
│   ├── builtin/        # Built-in tools
│   │   ├── echo.rs, time.rs, json.rs, http.rs
│   │   ├── file.rs     # ReadFile, WriteFile, ListDir, ApplyPatch
│   │   ├── shell.rs    # Shell command execution
│   │   ├── memory.rs   # Memory tools (search, write, read, tree)
│   │   ├── job.rs      # CreateJob, ListJobs, JobStatus, CancelJob
│   │   ├── routine.rs  # routine_create/list/update/delete/history
│   │   ├── extension_tools.rs # Extension install/auth/activate/remove
│   │   └── marketplace.rs, ecommerce.rs, taskrabbit.rs, restaurant.rs (stubs)
│   ├── builder/        # Dynamic tool building
│   │   ├── core.rs     # BuildRequirement, SoftwareType, Language
│   │   ├── templates.rs # Project scaffolding
│   │   ├── testing.rs  # Test harness integration
│   │   └── validation.rs # WASM validation
│   ├── mcp/            # Model Context Protocol
│   │   ├── client.rs   # MCP client over HTTP
│   │   └── protocol.rs # JSON-RPC types
│   └── wasm/           # Full WASM sandbox (wasmtime)
│       ├── runtime.rs  # Module compilation and caching
│       ├── wrapper.rs  # Tool trait wrapper for WASM modules
│       ├── host.rs     # Host functions (logging, time, workspace)
│       ├── limits.rs   # Fuel metering and memory limiting
│       ├── allowlist.rs # Network endpoint allowlisting
│       ├── credential_injector.rs # Safe credential injection
│       ├── loader.rs   # WASM tool discovery from filesystem
│       ├── rate_limiter.rs # Per-tool rate limiting
│       └── storage.rs  # Linear memory persistence
│
├── db/                 # Database abstraction layer
│   ├── mod.rs          # Database trait (~60 async methods)
│   ├── postgres.rs     # PostgreSQL backend (delegates to Store + Repository)
│   ├── libsql_backend.rs # libSQL/Turso backend (embedded SQLite)
│   └── libsql_migrations.rs # SQLite-dialect schema (idempotent)
│
├── workspace/          # Persistent memory system (OpenClaw-inspired)
│   ├── mod.rs          # Workspace struct, memory operations
│   ├── document.rs     # MemoryDocument, MemoryChunk, WorkspaceEntry
│   ├── chunker.rs      # Document chunking (800 tokens, 15% overlap)
│   ├── embeddings.rs   # EmbeddingProvider trait, OpenAI implementation
│   ├── search.rs       # Hybrid search with RRF algorithm
│   └── repository.rs   # PostgreSQL CRUD and search operations
│
├── context/            # Job context isolation
│   ├── state.rs        # JobState enum, JobContext, state machine
│   ├── memory.rs       # ActionRecord, ConversationMemory
│   └── manager.rs      # ContextManager for concurrent jobs
│
├── estimation/         # Cost/time/value estimation
│   ├── cost.rs         # CostEstimator
│   ├── time.rs         # TimeEstimator
│   ├── value.rs        # ValueEstimator (profit margins)
│   └── learner.rs      # Exponential moving average learning
│
├── evaluation/         # Success evaluation
│   ├── success.rs      # SuccessEvaluator trait, RuleBasedEvaluator, LlmEvaluator
│   └── metrics.rs      # MetricsCollector, QualityMetrics
│
├── secrets/            # Secrets management
│   ├── crypto.rs       # AES-256-GCM encryption
│   ├── store.rs        # Secret storage
│   └── types.rs        # Credential types
│
└── history/            # Persistence
    ├── store.rs        # PostgreSQL repositories
    └── analytics.rs    # Aggregation queries (JobStats, ToolStats)
```

## Key Patterns

### Architecture

When designing new features or systems, always prefer generic/extensible architectures over hardcoding specific integrations. Ask clarifying questions about the desired abstraction level before implementing.

### Connection Broker

WASM channels that need persistent connections (WebSocket, long-poll, SSE) declare a `connection` section in their `capabilities.json`. The host-side connection broker manages the connection lifecycle (connect, heartbeat, reconnect) and delivers events to WASM via `on_event` callbacks, preserving the fresh-instance-per-callback security model. The broker and polling coexist — channels choose which to use.

### Error Handling
- Use `thiserror` for error types in `error.rs`
- Never use `.unwrap()` or `.expect()` in production code (tests are fine)
- Map errors with context: `.map_err(|e| SomeError::Variant { reason: e.to_string() })?`
- Before committing, grep for `.unwrap()` and `.expect(` in changed files to catch violations mechanically

### Async
- All I/O is async with tokio
- Use `Arc<T>` for shared state across tasks
- Use `RwLock` for concurrent read/write access

### Traits for Extensibility
- `Database` - Add new database backends (must implement all ~60 methods)
- `Channel` - Add new input sources
- `Tool` - Add new capabilities
- `LlmProvider` - Add new LLM backends
- `SuccessEvaluator` - Custom evaluation logic
- `EmbeddingProvider` - Add embedding backends (workspace search)

### Tool Implementation
```rust
#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "Does something useful" }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "param": { "type": "string", "description": "A parameter" }
            },
            "required": ["param"]
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &JobContext)
        -> Result<ToolOutput, ToolError>
    {
        let start = std::time::Instant::now();
        // ... do work ...
        Ok(ToolOutput::text("result", start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool { true } // External data
}
```

### State Transitions
Job states follow a defined state machine in `context/state.rs`:
```
Pending -> InProgress -> Completed -> Submitted -> Accepted
                     \-> Failed
                     \-> Stuck -> InProgress (recovery)
                              \-> Failed
```

## Configuration

Environment variables (see `.env.example`):
```bash
# Database backend (default: postgres)
DATABASE_BACKEND=postgres               # or "libsql" / "turso"
DATABASE_URL=postgres://user:pass@localhost/rustytalon
LIBSQL_PATH=~/.rustytalon/rustytalon.db    # libSQL local path (default)
# LIBSQL_URL=libsql://xxx.turso.io    # Turso cloud (optional)
# LIBSQL_AUTH_TOKEN=xxx                # Required with LIBSQL_URL

# LLM Provider (pick one)
LLM_BACKEND=anthropic  # anthropic (default), openai, ollama, openai_compatible
ANTHROPIC_API_KEY=sk-ant-...
ANTHROPIC_MODEL=claude-sonnet-4-20250514
# ANTHROPIC_BASE_URL=https://api.anthropic.com  # Custom base URL (for proxies/gateways)
# ANTHROPIC_EXTRA_HEADERS="key=value,key2=value2"  # Extra HTTP headers (comma-separated)
#   NOTE: Quote values containing '=' signs in .env files (dotenvy requires it)

# Cloudflare AI Gateway example (Anthropic)
# LLM_BACKEND=anthropic
# ANTHROPIC_API_KEY=sk-ant-...
# ANTHROPIC_MODEL=claude-sonnet-4-20250514
# ANTHROPIC_BASE_URL=https://gateway.ai.cloudflare.com/v1/ACCOUNT/GATEWAY/anthropic
# ANTHROPIC_EXTRA_HEADERS="cf-aig-authorization=Bearer token"  # Quote required (value contains '=')

# OpenAI-compatible (LiteLLM, Together, etc.)
# LLM_BACKEND=openai_compatible
# LLM_BASE_URL=https://api.example.com/v1
# LLM_API_KEY=sk-...
# LLM_MODEL=gpt-4o
# LLM_EXTRA_HEADERS="key=value,key2=value2"  # Extra HTTP headers (quote if values contain '=')

# Agent settings
AGENT_NAME=rustytalon
MAX_PARALLEL_JOBS=5

# Smart routing
ROUTING_ENABLED=true
ROUTING_STRATEGY=balanced               # balanced, cost, quality, local_first
ROUTING_ENABLE_FALLBACK=true
ROUTING_MAX_RETRIES=3

# Web search (set any one to enable the web_search tool; priority: SearXNG > Brave > Tavily)
SEARXNG_URL=http://localhost:8888        # Self-hosted SearXNG (HTTP and private IPs allowed)
# BRAVE_SEARCH_API_KEY=...              # Brave Search API — free tier: 2 000 req/month, no credit card
# TAVILY_API_KEY=...                    # Tavily AI Search — free tier: 1 000 req/month, AI-optimised

# Embeddings (for semantic memory search)
OPENAI_API_KEY=sk-...                   # For OpenAI embeddings
EMBEDDING_ENABLED=true
EMBEDDING_MODEL=text-embedding-3-small  # or text-embedding-3-large

# Heartbeat (proactive periodic execution)
HEARTBEAT_ENABLED=true
HEARTBEAT_INTERVAL_SECS=1800            # 30 minutes
HEARTBEAT_NOTIFY_CHANNEL=tui
HEARTBEAT_NOTIFY_USER=default

# Web gateway
GATEWAY_ENABLED=true
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=3001
GATEWAY_AUTH_TOKEN=changeme           # Required for API access
GATEWAY_USER_ID=default

# Docker sandbox
SANDBOX_ENABLED=true
SANDBOX_IMAGE=rustytalon-worker:latest
SANDBOX_MEMORY_LIMIT_MB=512
SANDBOX_TIMEOUT_SECS=1800

# Claude Code mode (runs inside sandbox containers)
CLAUDE_CODE_ENABLED=false
CLAUDE_CODE_MODEL=claude-sonnet-4-20250514
CLAUDE_CODE_MAX_TURNS=50
CLAUDE_CODE_CONFIG_DIR=/home/worker/.claude

# Routines (scheduled/reactive execution)
ROUTINES_ENABLED=true
ROUTINES_CRON_INTERVAL=60            # Tick interval in seconds
ROUTINES_MAX_CONCURRENT=3
```

### LLM Providers

RustyTalon uses [rig-core](https://crates.io/crates/rig-core) for multi-provider LLM support. Supported backends:

| Backend | Env Prefix | Notes |
|---------|-----------|-------|
| Anthropic | `ANTHROPIC_` | Default, Claude models |
| OpenAI | `OPENAI_` | GPT models |
| Ollama | `OLLAMA_` | Local models, no API key needed |
| OpenAI-compatible | `LLM_` | Any OpenAI-compatible API; supports `LLM_EXTRA_HEADERS` for gateways |

The `SmartRouter` (when `ROUTING_ENABLED=true`) selects the best provider per-request based on query complexity, cost, and provider health. `TrackedProvider` wraps providers with retry logic and cost recording via `Database::record_llm_call()`.

## Database

RustyTalon supports two database backends, selected at compile time via Cargo feature flags and at runtime via the `DATABASE_BACKEND` environment variable.

**IMPORTANT: All new features that touch persistence MUST support both backends.** Implement the operation as a method on the `Database` trait in `src/db/mod.rs`, then add the implementation in both `src/db/postgres.rs` (delegate to Store/Repository) and `src/db/libsql_backend.rs` (native SQL).

### Backends

| Backend | Feature Flag | Default | Use Case |
|---------|-------------|---------|----------|
| PostgreSQL | `postgres` (default) | Yes | Production, existing deployments |
| libSQL/Turso | `libsql` | No | Zero-dependency local mode, edge, Turso cloud |

```bash
# Build with PostgreSQL only (default)
cargo build

# Build with libSQL only
cargo build --no-default-features --features libsql

# Build with both backends available
cargo build --features "postgres,libsql"
```

### Database Trait

The `Database` trait (`src/db/mod.rs`) defines ~60 async methods covering all persistence:
- Conversations, messages, metadata
- Jobs, actions, LLM calls, estimation snapshots
- Sandbox jobs, job events
- Routines, routine runs
- Tool failures, settings
- Workspace: documents, chunks, hybrid search

Both backends implement this trait. PostgreSQL delegates to the existing `Store` + `Repository`. libSQL implements native SQLite-dialect SQL.

### Schema

**PostgreSQL:** `migrations/V1__initial.sql` (351 lines). Uses pgvector for embeddings, tsvector for FTS, PL/pgSQL functions. Managed by `refinery`.

**libSQL:** `src/db/libsql_migrations.rs` (consolidated schema, ~480 lines). Translates PG types:
- `UUID` -> `TEXT`, `TIMESTAMPTZ` -> `TEXT` (ISO-8601), `JSONB` -> `TEXT`
- `VECTOR(1536)` -> `F32_BLOB(1536)` with `libsql_vector_idx`
- `tsvector`/`ts_rank_cd` -> FTS5 virtual table with sync triggers
- PL/pgSQL functions -> SQLite triggers

**Tables (both backends):**

**Core:**
- `conversations` - Multi-channel conversation tracking
- `agent_jobs` - Job metadata and status
- `job_actions` - Event-sourced tool executions
- `dynamic_tools` - Agent-built tools
- `llm_calls` - Cost tracking
- `estimation_snapshots` - Learning data

**Workspace/Memory:**
- `memory_documents` - Flexible path-based files (e.g., "context/vision.md", "daily/2024-01-15.md")
- `memory_chunks` - Chunked content with FTS and vector indexes
- `heartbeat_state` - Periodic execution tracking

**Other:**
- `routines`, `routine_runs` - Scheduled/reactive execution
- `settings` - Per-user key-value settings
- `tool_failures` - Self-repair tracking
- `secrets`, `wasm_tools`, `tool_capabilities` - Extension infrastructure

### Configuration

```bash
# Backend selection (default: postgres)
DATABASE_BACKEND=libsql

# PostgreSQL
DATABASE_URL=postgres://user:pass@localhost/rustytalon

# libSQL (embedded)
LIBSQL_PATH=~/.rustytalon/rustytalon.db    # Default path

# libSQL (Turso cloud sync)
LIBSQL_URL=libsql://your-db.turso.io
LIBSQL_AUTH_TOKEN=your-token            # Required when LIBSQL_URL is set
```

### Current Limitations (libSQL backend)

- **Workspace/memory system** not yet wired through Database trait (requires Store migration)
- **Secrets store** not yet available (still requires PostgresSecretsStore)
- **Hybrid search** uses FTS5 only (vector search via libsql_vector_idx not yet implemented)
- **Settings reload from DB** skipped (Config::from_db requires Store)
- No incremental migration versioning (schema is CREATE IF NOT EXISTS, no ALTER TABLE support yet)
- **No encryption at rest** -- The local SQLite database file stores conversation content, job data, workspace memory, and other application data in plaintext. Only secrets (API tokens, credentials) are encrypted via AES-256-GCM before storage. Users handling sensitive data should use full-disk encryption (FileVault, LUKS, BitLocker) or consider the PostgreSQL backend with TDE/encrypted storage.
- **JSON merge patch vs path-targeted update** -- The libSQL backend uses RFC 7396 JSON Merge Patch (`json_patch`) for metadata updates, while PostgreSQL uses path-targeted `jsonb_set`. Merge patch replaces top-level keys entirely, which may drop nested keys not present in the patch. Callers should avoid relying on partial nested object updates in metadata fields.

## Safety Layer

All external tool output passes through `SafetyLayer`:
1. **Sanitizer** - Detects injection patterns, escapes dangerous content
2. **Validator** - Checks length, encoding, forbidden patterns
3. **Policy** - Rules with severity (Critical/High/Medium/Low) and actions (Block/Warn/Review/Sanitize)

Tool outputs are wrapped before reaching LLM:
```xml
<tool_output name="search" sanitized="true">
[escaped content]
</tool_output>
```

## Testing

Tests are in `mod tests {}` blocks at the bottom of each file. Run specific module tests:
```bash
cargo test safety::sanitizer::tests
cargo test tools::registry::tests
```

Key test patterns:
- Unit tests for pure functions
- Async tests with `#[tokio::test]`
- No mocks, prefer real implementations or stubs

## Current Limitations / TODOs

1. **Domain-specific tools** - `marketplace.rs`, `restaurant.rs`, `taskrabbit.rs`, `ecommerce.rs` return placeholder responses; need real API integrations
2. **Integration tests** - Need testcontainers setup for PostgreSQL
3. **MCP stdio transport** - Only HTTP transport implemented
4. **WIT bindgen integration** - Auto-extract tool description/schema from WASM modules (stubbed)
5. **Capability granting after tool build** - Built tools get empty capabilities; need UX for granting HTTP/secrets access
6. **Tool versioning workflow** - No version tracking or rollback for dynamically built tools
7. **Webhook trigger endpoint** - Routines webhook trigger not yet exposed in web gateway
8. **Full channel status view** - Gateway status widget exists, but no per-channel connection dashboard
9. **Connection broker long-poll adapter** - Matrix `/sync` long-poll not yet implemented (config parsed but adapter stubbed)
10. **Connection broker SSE adapter** - SSE adapter not yet implemented (config parsed but adapter stubbed)
11. **Connection broker resume support** - Discord RESUME with session_id + sequence — config parsed (`resumable: true`) but not implemented

### Completed

- ✅ **Workspace integration** - Memory tools registered, workspace passed to Agent and heartbeat
- ✅ **WASM sandboxing** - Full implementation in `tools/wasm/` with fuel metering, memory limits, capabilities
- ✅ **Dynamic tool building** - `tools/builder/` has LlmSoftwareBuilder with iterative build loop
- ✅ **HTTP webhook security** - Secret validation implemented, proper error handling (no panics)
- ✅ **Embeddings integration** - OpenAI embeddings wired to workspace for semantic search
- ✅ **Workspace system prompt** - Identity files (AGENTS.md, SOUL.md, USER.md, IDENTITY.md) injected into LLM context
- ✅ **Heartbeat notifications** - Route through channel manager (broadcast API) instead of logging-only
- ✅ **Auto-context compaction** - Triggers automatically when context exceeds threshold
- ✅ **Embedding backfill** - Runs on startup when embeddings provider is enabled
- ✅ **Clippy clean** - All warnings addressed via config struct refactoring
- ✅ **Tool approval enforcement** - Tools with `requires_approval()` (shell, http, file write/patch, build_software) now gate execution, track auto-approved tools per session
- ✅ **Tool definition refresh** - Tool definitions refreshed each iteration so newly built tools become visible in same session
- ✅ **Worker tool call handling** - Uses `respond_with_tools()` to properly execute tool calls when `select_tools()` returns empty
- ✅ **Gateway control plane** - Web gateway with 40+ API endpoints, SSE/WebSocket
- ✅ **Web Control UI** - Browser-based dashboard with chat, memory, jobs, logs, extensions, routines
- ✅ **Slack/Telegram channels** - Implemented as WASM tools
- ✅ **Docker sandbox** - Orchestrator/worker containers with per-job auth
- ✅ **Claude Code mode** - Delegate jobs to Claude CLI inside containers
- ✅ **Routines system** - Cron, event, webhook, and manual triggers with guardrails
- ✅ **Extension management** - Install, auth, activate MCP/WASM extensions via CLI and web UI
- ✅ **libSQL/Turso backend** - Database trait abstraction (`src/db/`), feature-gated dual backend support (postgres/libsql), embedded SQLite for zero-dependency local mode
- ✅ **Connection broker (WebSocket adapter)** - Host-side persistent connections for WASM channels, with `on-event` WIT callback, event filtering, heartbeat, and reconnect

## Adding a New Tool

### Built-in Tools (Rust)

1. Create `src/tools/builtin/my_tool.rs`
2. Implement the `Tool` trait
3. Add `mod my_tool;` and `pub use` in `src/tools/builtin/mod.rs`
4. Register in `ToolRegistry::register_builtin_tools()` in `registry.rs`
5. Add tests

### WASM Tools (Recommended)

WASM tools are the preferred way to add new capabilities. They run in a sandboxed environment with explicit capabilities.

1. Create a new crate in `tools-src/<name>/`
2. Implement the WIT interface (`wit/tool.wit`)
3. Create `<name>.capabilities.json` declaring required permissions
4. Build with `cargo build --target wasm32-wasip2 --release`
5. Install with `rustytalon tool install path/to/tool.wasm`

See `tools-src/` for examples.

## Tool Architecture Principles

**CRITICAL: Keep tool-specific logic out of the main agent codebase.**

The main agent provides generic infrastructure; tools are self-contained units that declare their requirements through capabilities files.

### What Goes in Tools (capabilities.json)

- API endpoints the tool needs (HTTP allowlist)
- Credentials required (secret names, injection locations)
- Rate limits and timeouts
- Auth setup instructions (see below)
- Workspace paths the tool can read

### What Does NOT Go in Main Agent

- Service-specific auth flows (OAuth for Notion, Slack, etc.)
- Service-specific CLI commands (`auth notion`, `auth slack`)
- Service-specific configuration handling
- Hardcoded API URLs or token formats

### Tool Authentication

Tools declare their auth requirements in `<tool>.capabilities.json` under the `auth` section. Two methods are supported:

#### OAuth (Browser-based login)

For services that support OAuth, users just click through browser login:

```json
{
  "auth": {
    "secret_name": "notion_api_token",
    "display_name": "Notion",
    "oauth": {
      "authorization_url": "https://api.notion.com/v1/oauth/authorize",
      "token_url": "https://api.notion.com/v1/oauth/token",
      "client_id_env": "NOTION_OAUTH_CLIENT_ID",
      "client_secret_env": "NOTION_OAUTH_CLIENT_SECRET",
      "scopes": [],
      "use_pkce": false,
      "extra_params": { "owner": "user" }
    },
    "env_var": "NOTION_TOKEN"
  }
}
```

To enable OAuth for a tool:
1. Register a public OAuth app with the service (e.g., notion.so/my-integrations)
2. Configure redirect URIs: `http://localhost:9876/callback` through `http://localhost:9886/callback`
3. Set environment variables for client_id and client_secret

#### Manual Token Entry (Fallback)

For services without OAuth or when OAuth isn't configured:

```json
{
  "auth": {
    "secret_name": "openai_api_key",
    "display_name": "OpenAI",
    "instructions": "Get your API key from platform.openai.com/api-keys",
    "setup_url": "https://platform.openai.com/api-keys",
    "token_hint": "Starts with 'sk-'",
    "env_var": "OPENAI_API_KEY"
  }
}
```

#### Auth Flow Priority

When running `rustytalon tool auth <tool>`:

1. Check `env_var` - if set in environment, use it directly
2. Check `oauth` - if configured, open browser for OAuth flow
3. Fall back to `instructions` + manual token entry

The agent reads auth config from the tool's capabilities file and provides the appropriate flow. No service-specific code in the main agent.

### WASM Tools vs MCP Servers: When to Use Which

Both are first-class in the extension system (`rustytalon tool install` handles both), but they have different strengths.

**WASM Tools (RustyTalon native)**

- Sandboxed: fuel metering, memory limits, no access except what's allowlisted
- Credentials injected by host runtime, tool code never sees the actual token
- Output scanned for secret leakage before returning to the LLM
- Auth (OAuth/manual) declared in `capabilities.json`, agent handles the flow
- Single binary, no process management, works offline
- Cost: must build yourself in Rust, no ecosystem, synchronous only

**MCP Servers (Model Context Protocol)**

- Growing ecosystem of pre-built servers (GitHub, Notion, Postgres, etc.)
- Any language (TypeScript/Python most common)
- Can do websockets, streaming, background polling
- Cost: external process with full system access (no sandbox), manages own credentials, RustyTalon can't prevent leaks

**Decision guide:**

| Scenario | Use |
|----------|-----|
| Good MCP server already exists | **MCP** |
| Handles sensitive credentials (email send, banking) | **WASM** |
| Quick prototype or one-off integration | **MCP** |
| Core capability you'll maintain long-term | **WASM** |
| Needs background connections (websockets, polling) | **MCP** |
| Multiple tools share one OAuth token (e.g., Google suite) | **WASM** |

The LLM-facing interface is identical for both (tool name, schema, execute), so swapping between them is transparent to the agent.

## Adding a New Channel

1. Create `src/channels/my_channel.rs`
2. Implement the `Channel` trait
3. Add config in `src/config.rs`
4. Wire up in `main.rs` channel setup section

## Debugging

```bash
# Verbose logging
RUST_LOG=rustytalon=trace cargo run

# Just the agent module
RUST_LOG=rustytalon::agent=debug cargo run

# With HTTP request logging
RUST_LOG=rustytalon=debug,tower_http=debug cargo run
```

## Code Style

- Use `crate::` imports, not `super::`
- No `pub use` re-exports unless exposing to downstream consumers
- Prefer strong types over strings (enums, newtypes)
- Keep functions focused, extract helpers when logic is reused
- Comments for non-obvious logic only

## Changelog Management

Update `CHANGELOG.md` for all user-facing changes. This keeps the release notes accurate and helps users understand what's new.

### When to Update

**Update for:**
- ✅ New features
- ✅ Bug fixes
- ✅ Breaking changes
- ✅ Performance improvements
- ✅ Documentation updates
- ✅ Dependency upgrades
- ✅ Architecture changes affecting users

**Don't update for:**
- ❌ Internal refactoring (unless it changes behavior)
- ❌ Code cleanup with no functional change
- ❌ CI/CD improvements (unless they affect users)
- ❌ Test additions (unless they document new features)

### Format

Use the [Keep a Changelog](https://keepachangelog.com/) format under `[Unreleased]`:

```markdown
### Added
- New feature description

### Changed
- Behavior change that might affect users

### Fixed
- Bug fix description

### Deprecated
- Feature that will be removed soon

### Removed
- Previously deprecated feature removal

### Security
- Security fix with impact description
```

### Example

```markdown
## [Unreleased]

### Added
- GitHub Actions workflow for Docker image builds

### Changed
- Updated build-all.sh to show Makefile reference

### Fixed
- CHANGELOG hardcoded links causing broken URLs
```

## Release & Versioning Workflow

RustyTalon uses a `develop` → `main` branch model with GitHub Actions building releases on `main`.

### Version bump checklist

Before committing to `develop`, always check whether the current version in `Cargo.toml` has already been released on `main`:

```bash
# Check version on main vs develop
git show main:Cargo.toml | head -3
git show develop:Cargo.toml | head -3
```

If the versions match, **bump the patch version** in `Cargo.toml` on `develop` before committing. Otherwise the PR will merge a version that's already tagged and released, causing CI conflicts.

### Release steps

1. Bump version in `Cargo.toml` (patch for fixes, minor for features)
2. Run `cargo generate-lockfile` to update `Cargo.lock`
3. Move changelog entries from `[Unreleased]` to a new `[x.y.z] - YYYY-MM-DD` section
4. Commit version bump + changelog together
5. Push to `develop`, open PR to `main`
6. After CI passes and PR is merged, **create and push a git tag** to trigger the release:
   ```bash
   git fetch origin main
   git tag v<x.y.z> origin/main
   git push origin v<x.y.z>
   ```
   The `Release` workflow (`.github/workflows/release.yml`) only triggers on tag pushes matching `v*` -- merging the PR alone does NOT create a release

### Common mistakes to avoid

- **Don't commit code changes and version bumps separately across PRs** -- if a fix PR merges before the version bump, the bump lands in a second PR with no code delta, which is confusing
- **Don't leave entries under `[Unreleased]` when bumping** -- move them to the versioned section
- **Always check `main` version first** -- `git show main:Cargo.toml | head -3`

## Review & Fix Discipline

Hard-won lessons from code review -- follow these when fixing bugs or addressing review feedback.

### Fix the pattern, not just the instance
When a reviewer flags a bug (e.g., TOCTOU race in INSERT + SELECT-back), search the entire codebase for all instances of that same pattern. A fix in `SecretsStore::create()` that doesn't also fix `WasmToolStore::store()` is half a fix.

### Propagate architectural fixes to satellite types
If a core type changes its concurrency model (e.g., `LibSqlBackend` switches to connection-per-operation), every type that was handed a resource from the old model (e.g., `LibSqlSecretsStore`, `LibSqlWasmToolStore` holding a single `Connection`) must also be updated. Grep for the old type across the codebase.

### Schema translation is more than DDL
When translating a database schema between backends (PostgreSQL to libSQL, etc.), check for:
- **Indexes** -- diff `CREATE INDEX` statements between the two schemas
- **Seed data** -- check for `INSERT INTO` in migrations (e.g., `leak_detection_patterns`)
- **Semantic differences** -- document where SQL functions behave differently (e.g., `json_patch` vs `jsonb_set`)

### Feature flag testing
When adding feature-gated code, test compilation with each feature in isolation:
```bash
cargo check                                          # default features
cargo check --no-default-features --features libsql  # libsql only
cargo check --all-features                           # all features
```
Dead code behind the wrong `#[cfg]` gate will only show up when building with a single feature.

### WASM credential consistency
When a service has both a channel (`channels-src/`) and a tool (`tools-src/`), their `capabilities.json` credential configs must match. In particular:
- The `location` type and parameters must produce the same `Authorization` header format
- `"type": "bearer"` always produces `Authorization: Bearer {token}` -- if the API expects a different prefix (e.g., Discord's `Bot`), use `"type": "header"` with an explicit `name` and `prefix` instead
- After editing any capabilities file, diff the channel and tool versions: `diff <(jq '.capabilities.http.credentials' channels-src/X/X.capabilities.json) <(jq '.http.credentials' tools-src/X/X-tool.capabilities.json)`

### Mechanical verification before committing
Run these checks on changed files before committing:
- `grep -rnE '\.unwrap\(|\.expect\(' <files>` -- no panics in production
- `grep -rn 'super::' <files>` -- use `crate::` imports
- If you fixed a pattern bug, `grep` for other instances of that pattern across `src/`
- If touching WASM capabilities files, verify credential `location` consistency between channel and tool variants

## Workspace & Memory System

Inspired by [OpenClaw](https://github.com/openclaw/openclaw), the workspace provides persistent memory for agents with a flexible filesystem-like structure.

### Key Principles

1. **"Memory is database, not RAM"** - If you want to remember something, write it explicitly
2. **Flexible structure** - Create any directory/file hierarchy you need
3. **Self-documenting** - Use README.md files to describe directory structure
4. **Hybrid search** - Combines FTS (keyword) + vector (semantic) via Reciprocal Rank Fusion

### Filesystem Structure

```
workspace/
├── README.md              <- Root runbook/index
├── MEMORY.md              <- Long-term curated memory
├── HEARTBEAT.md           <- Periodic checklist
├── IDENTITY.md            <- Agent name, nature, vibe
├── SOUL.md                <- Core values
├── AGENTS.md              <- Behavior instructions
├── USER.md                <- User context
├── context/               <- Identity-related docs
│   ├── vision.md
│   └── priorities.md
├── daily/                 <- Daily logs
│   ├── 2024-01-15.md
│   └── 2024-01-16.md
├── projects/              <- Arbitrary structure
│   └── alpha/
│       ├── README.md
│       └── notes.md
└── ...
```

### Using the Workspace

```rust
use crate::workspace::{Workspace, OpenAiEmbeddings, paths};

// Create workspace for a user
let workspace = Workspace::new("user_123", pool)
    .with_embeddings(Arc::new(OpenAiEmbeddings::new(api_key)));

// Read/write any path
let doc = workspace.read("projects/alpha/notes.md").await?;
workspace.write("context/priorities.md", "# Priorities\n\n1. Feature X").await?;
workspace.append("daily/2024-01-15.md", "Completed task X").await?;

// Convenience methods for well-known files
workspace.append_memory("User prefers dark mode").await?;
workspace.append_daily_log("Session note").await?;

// List directory contents
let entries = workspace.list("projects/").await?;

// Search (hybrid FTS + vector)
let results = workspace.search("dark mode preference", 5).await?;

// Get system prompt from identity files
let prompt = workspace.system_prompt().await?;
```

### Memory Tools

Four tools for LLM use:

- **`memory_search`** - Hybrid search, MUST be called before answering questions about prior work
- **`memory_write`** - Write to any path (memory, daily_log, or custom paths)
- **`memory_read`** - Read any file by path
- **`memory_tree`** - View workspace structure as a tree (depth parameter, default 1)

### Hybrid Search (RRF)

Combines full-text search and vector similarity using Reciprocal Rank Fusion:

```
score(d) = Σ 1/(k + rank(d)) for each method where d appears
```

Default k=60. Results from both methods are combined, with documents appearing in both getting boosted scores.

**Backend differences:**
- **PostgreSQL:** `ts_rank_cd` for FTS, pgvector cosine distance for vectors, full RRF
- **libSQL:** FTS5 for keyword search only (vector search via `libsql_vector_idx` not yet wired)

### Heartbeat System

Proactive periodic execution (default: 30 minutes). Each tick does three things in order:

1. **Checklist** — reads `HEARTBEAT.md`, runs an LLM turn, notifies if findings (skipped if checklist is empty)
2. **Memory consolidation** — for each daily log older than today, makes a lightweight LLM call to extract atomic facts (`USER:` / `MEMORY:` lines) into `USER.md` and `MEMORY.md`, then deletes the raw log
3. **Pruning** — deletes any daily logs older than `audit_retention_days` (default 90) that consolidation missed; also prunes the audit log

Boot-time: `update_agents_md_if_outdated()` and `prune_old_daily_logs()` run on every startup regardless of whether the heartbeat is enabled, so cleanup and instruction upgrades are never gated on heartbeat being on.

```rust
use crate::agent::{HeartbeatConfig, spawn_heartbeat};

let config = HeartbeatConfig::default()
    .with_interval(Duration::from_secs(60 * 30))
    .with_notify("user_123", "telegram");

spawn_heartbeat(config, workspace, llm, response_tx);
```

#### AGENTS.md versioning

`AGENTS.md` carries a version marker (`<!-- agents-v3 -->`). `Workspace::update_agents_md_if_outdated()` rewrites the file on boot if the marker is absent, so existing users automatically pick up new behavioral instructions without wiping the file manually. Bump the marker constant (`AGENTS_VERSION_MARKER` in `workspace/mod.rs`) and update `AGENTS_SEED` whenever instructions change materially.

### Chunking Strategy

Documents are chunked for search indexing:
- Default: 800 words per chunk (roughly 800 tokens for English)
- 15% overlap between chunks for context preservation
- Minimum chunk size: 50 words (tiny trailing chunks merge with previous)
