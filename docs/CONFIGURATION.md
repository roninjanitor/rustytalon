# Configuration Reference

RustyTalon is configured entirely through environment variables. This document is the complete reference for every variable. To get started:

```bash
cp .env.example .env
# Edit .env with your values, then start the agent
```

The minimum required:
1. A database connection (or the libSQL embedded default)
2. At least one LLM provider API key

To use extensions (Telegram, Discord, Google tools, MCP servers, etc.), you also need:

3. `SECRETS_MASTER_KEY` — see [Secrets & Extension Management](#secrets--extension-management)

> **Tip:** You do not need to set every variable. Unset variables use the defaults shown in the tables below. The `.env.example` file shows the most commonly changed options with inline comments — this page is the full reference.

---

## Database

Controls where RustyTalon stores conversations, jobs, memory, and all other persistent data.

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_BACKEND` | `libsql` (Docker) / `postgres` (source build) | Database backend: `postgres` or `libsql` |
| `DATABASE_URL` | `postgres://rustytalon:rustytalon@localhost/rustytalon` | PostgreSQL connection string (required when `DATABASE_BACKEND=postgres`) |
| `DATABASE_POOL_SIZE` | `10` | PostgreSQL connection pool size |
| `LIBSQL_PATH` | `~/.rustytalon/rustytalon.db` | Local libSQL file path (used when `DATABASE_BACKEND=libsql`) |
| `LIBSQL_URL` | — | Turso cloud URL (optional, enables cloud sync for libSQL) |
| `LIBSQL_AUTH_TOKEN` | — | Required when `LIBSQL_URL` is set |

### PostgreSQL Compose Credentials

These variables are used by `docker-compose.prod.yml` to create the database and build the internal `DATABASE_URL`. They are not needed for source builds (set `DATABASE_URL` directly instead).

| Variable | Default | Description |
|----------|---------|-------------|
| `POSTGRES_USER` | `rustytalon` | Database user |
| `POSTGRES_PASSWORD` | — | **Required.** Database password — must be set in `.env` |
| `POSTGRES_DB` | `rustytalon` | Database name |
| `DB_PORT` | `5432` | Host-side port exposed by the database container |

**Which backend to use:**
- `libsql` — Zero-dependency embedded SQLite. Good for personal use and development. No setup required. Memory search uses keyword matching only.
- `postgres` — **Recommended for production.** Uses [pgvector](https://github.com/pgvector/pgvector) for full hybrid search (keyword + semantic/vector similarity). Enables the agent to find memories by meaning, not just exact words. Requires a PostgreSQL 15+ instance with the `vector` extension.

---

## LLM Provider

Configure which AI model powers the agent. Pick one backend (or multiple for smart routing).

| Variable | Default | Description |
|----------|---------|-------------|
| `LLM_BACKEND` | `anthropic` | Primary provider: `anthropic`, `openai`, `ollama`, or `openai_compatible` |

### Anthropic (Claude)

| Variable | Default | Description |
|----------|---------|-------------|
| `ANTHROPIC_API_KEY` | — | Your Anthropic API key (required) |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-20250514` | Model to use |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Override for proxies or AI gateways |
| `ANTHROPIC_EXTRA_HEADERS` | — | Extra HTTP headers, comma-separated: `key=value,key2=value2` |

### OpenAI

| Variable | Default | Description |
|----------|---------|-------------|
| `OPENAI_API_KEY` | — | Your OpenAI API key |
| `OPENAI_MODEL` | `gpt-4o` | Model to use |

### Ollama (Local / Free)

| Variable | Default | Description |
|----------|---------|-------------|
| `OLLAMA_BASE_URL` | `http://localhost:11434` | Ollama server URL |
| `OLLAMA_MODEL` | `llama3` | Model name |

### OpenAI-compatible (LiteLLM, Together, Cloudflare AI Gateway, etc.)

| Variable | Default | Description |
|----------|---------|-------------|
| `LLM_BASE_URL` | — | Base URL of the compatible API |
| `LLM_API_KEY` | — | API key |
| `LLM_MODEL` | — | Model name |
| `LLM_EXTRA_HEADERS` | — | Extra HTTP headers: `key=value,key2=value2` |

---

## Smart Routing

When you have multiple providers configured, smart routing automatically picks the best one per request.

| Variable | Default | Description |
|----------|---------|-------------|
| `ROUTING_ENABLED` | `true` | Enable smart routing |
| `ROUTING_STRATEGY` | `balanced` | Strategy: `balanced`, `cost`, `quality`, or `local_first` |
| `ROUTING_ENABLE_FALLBACK` | `true` | Fall back to another provider on failure |
| `ROUTING_MAX_RETRIES` | `3` | Max retry attempts across providers |
| `ROUTING_MIN_QUALITY` | `0.5` | Minimum acceptable quality score (0.0–1.0) |
| `ROUTING_MAX_COST` | — | Max USD per request (optional cap) |
| `ROUTING_PREFERRED_PROVIDERS` | — | Comma-separated priority list: `anthropic,openai` |
| `ROUTING_EXCLUDED_PROVIDERS` | — | Providers to never use |

**Strategies:**
- `balanced` — Weighs cost, quality, and latency equally
- `cost` — Prefer the cheapest provider that meets quality threshold
- `quality` — Always use the highest-quality available provider
- `local_first` — Prefer Ollama/local, fall back to cloud

---

## Web Gateway

The browser UI and HTTP API.

| Variable | Default | Description |
|----------|---------|-------------|
| `GATEWAY_ENABLED` | `true` | Enable the web gateway |
| `GATEWAY_HOST` | `127.0.0.1` | Bind address. Use `0.0.0.0` for network access |
| `GATEWAY_PORT` | `3001` | Port to listen on |
| `GATEWAY_AUTH_TOKEN` | *(auto-generated)* | Bearer token required for all API calls. If unset, a random 32-character token is generated at startup and printed to the log. Set this to a fixed value for scripting or stable reverse-proxy config. |
| `GATEWAY_USER_ID` | `default` | Default user ID for web UI sessions |

> **Finding your token:** If you leave `GATEWAY_AUTH_TOKEN` unset, look for the startup log line: `Web UI: http://...:3001/?token=<token>`. The URL includes the token as a query parameter for one-click access.
>
> **Security:** See [DEPLOYMENT.md](DEPLOYMENT.md) for HTTPS setup with a reverse proxy.

---

## Agent Settings

| Variable | Default | Description |
|----------|---------|-------------|
| `AGENT_NAME` | `rustytalon` | The agent's display name |
| `AGENT_MAX_PARALLEL_JOBS` | `5` | Maximum concurrent jobs |
| `AGENT_JOB_TIMEOUT_SECS` | `3600` | Job timeout (1 hour) |
| `AGENT_STUCK_THRESHOLD_SECS` | `300` | Seconds before a job is considered stuck |
| `AGENT_USE_PLANNING` | `true` | Enable planning before job execution |

---

## Embeddings (Semantic Memory Search)

Required for vector-based similarity search in the workspace. Uses OpenAI's embedding API.

| Variable | Default | Description |
|----------|---------|-------------|
| `EMBEDDING_ENABLED` | `false` | Enable semantic search (requires OpenAI key) |
| `EMBEDDING_MODEL` | `text-embedding-3-small` | Embedding model to use |
| `OPENAI_API_KEY` | — | OpenAI API key (reused from LLM config if already set) |

Without embeddings, memory search uses keyword (full-text) matching only. Semantic search gives significantly better results for natural language queries.

---

## Channels

### HTTP Webhook

Accept messages via HTTP POST from external systems.

| Variable | Default | Description |
|----------|---------|-------------|
| `HTTP_HOST` | — | Bind address (e.g. `0.0.0.0`) |
| `HTTP_PORT` | `8080` | Port to listen on |
| `HTTP_WEBHOOK_SECRET` | — | Secret for validating incoming requests |

### Telegram

| Variable | Default | Description |
|----------|---------|-------------|
| `TELEGRAM_BOT_TOKEN` | — | Bot token from @BotFather |

See [TELEGRAM_SETUP.md](TELEGRAM_SETUP.md) for full setup instructions.

### Slack

| Variable | Default | Description |
|----------|---------|-------------|
| `SLACK_BOT_TOKEN` | — | Bot OAuth token (`xoxb-...`) |
| `SLACK_APP_TOKEN` | — | App-level token (`xapp-...`) |
| `SLACK_SIGNING_SECRET` | — | Signing secret for request validation |

---

## Secrets & Extension Management

Required to install and authenticate extensions (WASM tools, MCP servers, messaging channels).

| Variable | Default | Description |
|----------|---------|-------------|
| `SECRETS_MASTER_KEY` | — | Base64-encoded 32-byte key for AES-256-GCM encryption of stored credentials |

**Generate a key:**

```bash
openssl rand -base64 32
```

Without this key:
- The extension catalog is still browsable
- Install, authenticate, and activate actions are disabled
- The web UI displays a setup banner with instructions

> **Security note:** The master key encrypts API tokens and OAuth secrets at rest. Keep it safe — losing it means re-authenticating all installed extensions. For production deployments, consider using the OS keychain (RustyTalon will auto-detect a key stored there).

---

## Docker Sandbox

Isolated container execution for long-running or untrusted jobs.

| Variable | Default | Description |
|----------|---------|-------------|
| `SANDBOX_ENABLED` | `false` | Enable Docker sandbox |
| `SANDBOX_IMAGE` | `rustytalon-worker:latest` | Worker container image |
| `SANDBOX_MEMORY_LIMIT_MB` | `512` | Memory limit per container |
| `SANDBOX_TIMEOUT_SECS` | `1800` | Max job duration (30 minutes) |

> **Docker socket required:** When running RustyTalon in a container with `SANDBOX_ENABLED=true`, you must mount the Docker socket. In `docker-compose.prod.yml`, uncomment the volumes section:
> ```yaml
> volumes:
>   - /var/run/docker.sock:/var/run/docker.sock
> ```

### Claude Code Mode

Run Claude Code CLI inside sandbox containers for complex coding tasks.

> **Prerequisite:** `SANDBOX_ENABLED=true` must be set before enabling Claude Code mode.

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAUDE_CODE_ENABLED` | `false` | Enable Claude Code mode (requires `SANDBOX_ENABLED=true`) |
| `CLAUDE_CODE_MODEL` | `claude-sonnet-4-20250514` | Model for Claude Code |
| `CLAUDE_CODE_MAX_TURNS` | `50` | Max conversation turns per job |
| `CLAUDE_CODE_CONFIG_DIR` | `/home/worker/.claude` | Claude config directory inside container |

---

## Routines

Background task scheduling.

| Variable | Default | Description |
|----------|---------|-------------|
| `ROUTINES_ENABLED` | `true` | Enable the routines engine |
| `ROUTINES_CRON_INTERVAL` | `60` | How often to check for due cron routines (seconds) |
| `ROUTINES_MAX_CONCURRENT` | `3` | Max routines running simultaneously |

---

## Heartbeat

Proactive periodic check-ins.

| Variable | Default | Description |
|----------|---------|-------------|
| `HEARTBEAT_ENABLED` | `false` | Enable heartbeat |
| `HEARTBEAT_INTERVAL_SECS` | `1800` | How often to run (30 minutes) |
| `HEARTBEAT_NOTIFY_CHANNEL` | `cli` | Channel to send notifications to |
| `HEARTBEAT_NOTIFY_USER` | `default` | User ID for notifications |

---

## Self-Repair

Automatic detection and recovery of stuck jobs.

| Variable | Default | Description |
|----------|---------|-------------|
| `SELF_REPAIR_CHECK_INTERVAL_SECS` | `60` | How often to check for stuck jobs |
| `SELF_REPAIR_MAX_ATTEMPTS` | `3` | Max recovery attempts before marking as failed |

---

## Safety

Prompt injection defense.

| Variable | Default | Description |
|----------|---------|-------------|
| `SAFETY_MAX_OUTPUT_LENGTH` | `100000` | Max characters of tool output passed to LLM |
| `SAFETY_INJECTION_CHECK_ENABLED` | `true` | Enable prompt injection detection |

---

## Logging

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `rustytalon=info` | Log level filter |

Common values:

```bash
RUST_LOG=rustytalon=info          # Normal operation
RUST_LOG=rustytalon=debug         # Verbose (includes tool calls, routing decisions)
RUST_LOG=rustytalon=trace         # Very verbose (includes LLM messages)
RUST_LOG=rustytalon::agent=debug  # Debug just the agent module
RUST_LOG=rustytalon=debug,tower_http=debug  # Include HTTP request logs
```

Logs are also streamed live via the web gateway at `GET /api/logs/events`.
